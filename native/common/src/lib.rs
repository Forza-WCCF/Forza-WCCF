//! What the 32-bit DLLs and programs in and beside the game share. The hook (`forza-hook`) and the guards (`wccfnet`,
//! `netguard`) patch the import table of the game's own program in its process: [`imports`] walks it, [`redirect`]
//! points one slot elsewhere. FPR_Emu.exe and logowin.exe wait on the program that started them ([`parent_pid`]). Both
//! editions' guards keep the publisher's network hosts unresolved ([`hosts`]).
#![cfg(target_arch = "x86")]
// This code runs in the game's process, where a panic ends the game: no call that can panic (tests may, clippy.toml)
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::string_slice
)]

pub mod hosts;

use std::ffi::CStr;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, SYSTEMTIME};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32, Process32First, Process32Next, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

/// The 2017-18 client studied (`client_Release.exe`, SHA-256 8b55456f... before setup): its PE header's TimeDateStamp
/// and SizeOfImage. wccfnet's boot hold and the panel's patches act on this build only.
pub const CLIENT_1718_STAMP: (u32, u32) = (0x5AAA_3F0A, 0x06C8_E000);

/// The window property between wccfnet.dll and the panel while the 2017-18 client's window is held hidden through
/// the boot screens. Value 1: wccfnet holds it and its fallback shows the window; value 2: the panel holds it, and the
/// fallback posts [`WM_BOOT_SHOW`] so the panel shows it (with the projector setting). Whoever removes it shows the
/// window.
pub const BOOT_PROP: *const u16 = windows_sys::core::w!("ForzaWCCF.BootHidden");
/// The value of [`BOOT_PROP`] while the panel holds the window.
pub const BOOT_PANEL: usize = 2;
/// The panel's window message that shows the held window on its own thread (WM_APP + 0x5A); wParam 1 = wccfnet's
/// fallback sent it.
pub const WM_BOOT_SHOW: u32 = 0x8000 + 0x5A;

/// How an import is named in the table: most by name, the games' winsock calls by ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By<'a> {
    Name(&'a CStr),
    Ordinal(u16),
}

/// One DLL a loaded module imports from.
pub struct Dll {
    base: *const u8,
    name: *const u8,
    /// the name/ordinal table (OriginalFirstThunk); 0 = none, the slots alone (already resolved addresses)
    names: u32,
    slots: u32,
}

impl Dll {
    /// The DLL's name as the module imports it (`kernel32.dll`, `WS2_32.dll`).
    pub fn name(&self) -> &CStr {
        // SAFETY: an import descriptor's Name is a NUL-terminated string in the loaded image (`imports`' contract)
        unsafe { CStr::from_ptr(self.name.cast()) }
    }

    /// False when the module keeps no name table for this DLL: its slots then hold resolved addresses that cannot be
    /// matched to a function.
    pub fn has_names(&self) -> bool {
        self.names != 0
    }

    /// Each import from this DLL and its slot, the address the module calls through. Empty without a name table.
    pub fn entries(&self) -> impl Iterator<Item = (By<'_>, *mut usize)> + '_ {
        let names = (self.names != 0).then(|| {
            // SAFETY: the RVAs come from the loaded image's own import descriptor (`imports`' contract)
            unsafe {
                (
                    self.base.add(self.names as usize).cast::<u32>(),
                    self.base.add(self.slots as usize),
                )
            }
        });
        let mut i = 0;
        std::iter::from_fn(move || {
            let (names, slots) = names?;
            // SAFETY: the name table ends with a zero entry; every entry before it has a slot at the same index
            let (entry, slot) = unsafe {
                (
                    names.add(i).read_unaligned(),
                    slots.cast::<usize>().cast_mut().add(i),
                )
            };
            if entry == 0 {
                return None;
            }
            i += 1;
            let by = if entry & 0x8000_0000 != 0 {
                By::Ordinal(entry as u16)
            } else {
                // SAFETY: IMAGE_IMPORT_BY_NAME: a 2-byte hint, then the NUL-terminated name
                By::Name(unsafe { CStr::from_ptr(self.base.add(entry as usize + 2).cast()) })
            };
            Some((by, slot))
        })
    }

    /// The slot of `want`, if this DLL's name table has it.
    pub fn find(&self, want: By<'_>) -> Option<*mut usize> {
        self.entries()
            .find(|(by, _)| match (by, want) {
                (By::Name(a), By::Name(b)) => *a == b,
                (By::Ordinal(a), By::Ordinal(b)) => *a == b,
                _ => false,
            })
            .map(|(_, slot)| slot)
    }
}

/// The DLLs the module loaded at `base` imports from (none when `base` is no PE image or has no import table).
///
/// # Safety
/// `base` is a module loaded in this process (`GetModuleHandle`), which stays loaded while the result is used.
pub unsafe fn imports(base: *const u8) -> impl Iterator<Item = Dll> {
    let u32_at = move |off: usize| {
        // SAFETY: inside the headers of a loaded image (the caller's contract)
        unsafe { base.add(off).cast::<u32>().read_unaligned() }
    };
    // SAFETY: every loaded image starts with its DOS header
    let mz = unsafe { base.cast::<u16>().read_unaligned() } == 0x5A4D;
    let nt = if mz { u32_at(0x3C) as usize } else { 0 };
    // PE32: the optional header follows the 4-byte signature and the 20-byte file header; the import directory is
    // its data directory 1, after 96 bytes of fields
    let table = if mz && u32_at(nt) == 0x4550 {
        u32_at(nt + 24 + 96 + 8)
    } else {
        0
    };
    let mut desc = table as usize;
    std::iter::from_fn(move || {
        if desc == 0 {
            return None;
        }
        // IMAGE_IMPORT_DESCRIPTOR: OriginalFirstThunk, TimeDateStamp, ForwarderChain, Name, FirstThunk; a zero Name ends it
        let (names, name, slots) = (u32_at(desc), u32_at(desc + 12), u32_at(desc + 16));
        if name == 0 {
            desc = 0;
            return None;
        }
        desc += 20;
        Some(Dll {
            base,
            // SAFETY: an RVA inside the loaded image
            name: unsafe { base.add(name as usize) },
            names,
            slots,
        })
    })
}

/// Points an import slot at `new` and returns what it pointed at; None (the slot unchanged) when its page cannot be
/// made writable.
///
/// # Safety
/// `slot` is an import slot from [`Dll::entries`] of a module that is still loaded, and `new` a function with the
/// imported one's signature and calling convention.
pub unsafe fn redirect(slot: *mut usize, new: usize) -> Option<usize> {
    let mut prot: PAGE_PROTECTION_FLAGS = 0;
    // SAFETY: the slot lies in the loaded image (the caller's contract); its protection is put back after the write
    unsafe {
        if VirtualProtect(slot.cast(), size_of::<usize>(), PAGE_READWRITE, &mut prot) == 0 {
            return None;
        }
        let old = slot.read();
        slot.write(new);
        VirtualProtect(slot.cast(), size_of::<usize>(), prot, &mut prot);
        Some(old)
    }
}

/// The local time, as the logs and message files write it.
pub fn local_time() -> SYSTEMTIME {
    // SAFETY: GetLocalTime fills the struct it is given
    unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    }
}

/// The process that started this one (0 if it cannot be found).
pub fn parent_pid() -> u32 {
    // SAFETY: a process snapshot read entry by entry, then closed
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return 0;
        }
        let (me, mut parent) = (GetCurrentProcessId(), 0);
        let mut pe: PROCESSENTRY32 = std::mem::zeroed();
        pe.dwSize = size_of::<PROCESSENTRY32>() as u32;
        let mut more = Process32First(snap, &mut pe) != 0;
        while more {
            if pe.th32ProcessID == me {
                parent = pe.th32ParentProcessID;
                break;
            }
            more = Process32Next(snap, &mut pe) != 0;
        }
        CloseHandle(snap);
        parent
    }
}
