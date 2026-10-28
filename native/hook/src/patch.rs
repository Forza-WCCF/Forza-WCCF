//! The game program's imports, pointed at the hooks (in-process, by name or by ordinal: the winsock calls are imported
//! by ordinal), and the original functions the hooks pass calls on to.

use forza_native::{By, imports, redirect};
use std::ffi::CStr;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleA, GetModuleHandleW, GetProcAddress,
};

/// The function a hook passes calls on to.
pub struct Real<F>(AtomicUsize, PhantomData<F>);

impl<F: Copy> Real<F> {
    pub const fn new() -> Self {
        Real(AtomicUsize::new(0), PhantomData)
    }

    pub fn get(&self) -> F {
        const { assert!(size_of::<F>() == size_of::<usize>()) };
        let a = self.0.load(Relaxed);
        assert!(a != 0, "a hook without its original function");
        // SAFETY: only `patch` and `k32` set it, to the original of an import of type F
        unsafe { std::mem::transmute_copy(&a) }
    }
}

/// Set when an import could not be read or patched (no name table, the page not writable): "not imported" and "could
/// not patch" are both false from `patch`, and the DHCP guard must tell them apart.
pub static UNSURE: AtomicBool = AtomicBool::new(false);

/// The game's own program, whose imports are patched.
pub fn exe() -> *const u8 {
    // SAFETY: the process's main module, loaded for its whole life
    unsafe { GetModuleHandleW(std::ptr::null()) as *const u8 }
}

/// Points the game program's import of `dll`!`by` at `hook` (a function of the import's own type), the original kept in `real` first (a call on another
/// thread already finds it). False when the program does not import it, or when it could not be patched (then
/// [`UNSURE`] is set).
pub fn patch<F: Copy>(real: &Real<F>, dll: &str, by: By, hook: F) -> bool {
    const { assert!(size_of::<F>() == size_of::<usize>()) };
    // SAFETY: F is a function pointer type (Real's), the size of an address
    let hook: usize = unsafe { std::mem::transmute_copy(&hook) };
    // SAFETY: the process's main module
    for d in unsafe { imports(exe()) } {
        if !d.name().to_bytes().eq_ignore_ascii_case(dll.as_bytes()) {
            continue;
        }
        if !d.has_names() {
            UNSURE.store(true, Relaxed); // the name/ordinal table is needed to match
            continue;
        }
        if let Some(slot) = d.find(by) {
            // SAFETY: an import slot of the loaded program; `hook` has the import's signature (each caller's hook)
            unsafe {
                real.0.store(slot.read(), Relaxed);
                if redirect(slot, hook).is_none() {
                    UNSURE.store(true, Relaxed);
                    return false;
                }
            }
            return true;
        }
    }
    false
}

/// Hooks a kernel32 import of the game's program. The original is kept either way (the import's, else kernel32's own),
/// so a hook never calls nothing. True if the import now points at the hook.
pub fn k32<F: Copy>(real: &Real<F>, name: &CStr, hook: F) -> bool {
    if patch(real, "kernel32.dll", By::Name(name), hook) {
        return true;
    }
    // SAFETY: kernel32 is loaded in every process
    let f = unsafe {
        GetProcAddress(
            GetModuleHandleA(c"kernel32.dll".as_ptr().cast()),
            name.as_ptr().cast(),
        )
    };
    real.0.store(f.map_or(0, |f| f as usize), Relaxed);
    false
}

/// The program's file name (`control_Release.exe`), compared without case.
pub fn exe_is(name: &str) -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
        })
        .unwrap_or(false)
}
