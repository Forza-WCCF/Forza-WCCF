//! The cabinet's devices: every device open, file mapping and DeviceIoControl is logged and passed straight through
//! (observation), except the RingEdge devices MXHOOK_RINGEDGE=1 stands in for (2026-10-04).
//!
//! In real-cabinet mode (IS_RINGEDGE=1) the player cabinet opens the original kernel drivers, which do not exist on a
//! PC. The hook stands in for them, one at a time:
//!  - `\\.\mxsram` (battery-backed save memory, client amSramInit FUN_008317c0): a real file mxsram.bin in the working
//!    folder (2 MiB). amSramReadInternal / amSramWriteInternal use SetFilePointer + ReadFile / WriteFile, which work on
//!    the file as they are; the hook answers the device questions: 0x9C406000 version (low word must be 1), 0x00070000
//!    disk geometry (24 bytes), 0x9C406004 sector size (<= 0x800), 0x0007405C length (8 bytes).
//!  - `\\.\columba`: physical memory reads of the fake BIOS area ([`crate::dmi`]).
//!
//! Every other device the game opens is logged (and fails as before) so the next one can be added.

use crate::log::{Text, Wide, log};
use crate::patch::{Real, k32};
use crate::{dmi, serial};
use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering::Relaxed};
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_FUNCTION, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, SetLastError,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_NORMAL, FILE_BEGIN, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileSizeEx,
    OPEN_ALWAYS, OPEN_EXISTING, SetEndOfFile, SetFilePointerEx,
};

/// Set by MXHOOK_VERBOSE: log every file open.
pub static VERBOSE: AtomicBool = AtomicBool::new(false);
/// Set by MXHOOK_RINGEDGE=1: stand in for the mx devices.
pub static RINGEDGE: AtomicBool = AtomicBool::new(false);

type Sa = *const c_void;
pub type CreateFileW =
    unsafe extern "system" fn(*const u16, u32, u32, Sa, u32, u32, HANDLE) -> HANDLE;
pub type CreateFileA =
    unsafe extern "system" fn(*const u8, u32, u32, Sa, u32, u32, HANDLE) -> HANDLE;
type CreateFileMappingA = unsafe extern "system" fn(HANDLE, Sa, u32, u32, u32, *const u8) -> HANDLE;
type OpenFileMappingA = unsafe extern "system" fn(u32, i32, *const u8) -> HANDLE;
type DeviceIoControl = unsafe extern "system" fn(
    HANDLE,
    u32,
    *mut c_void,
    u32,
    *mut c_void,
    u32,
    *mut u32,
    *mut c_void,
) -> i32;

static REAL_CREATE_FILE_W: Real<CreateFileW> = Real::new();
pub static REAL_CREATE_FILE_A: Real<CreateFileA> = Real::new();
static REAL_CREATE_FILE_MAPPING_A: Real<CreateFileMappingA> = Real::new();
static REAL_OPEN_FILE_MAPPING_A: Real<OpenFileMappingA> = Real::new();
static REAL_DEVICE_IO_CONTROL: Real<DeviceIoControl> = Real::new();

/// Hooks the five device imports; which took, in the order the log names them.
pub fn install() -> [bool; 5] {
    [
        k32(&REAL_CREATE_FILE_W, c"CreateFileW", hook_create_file_w),
        k32(&REAL_CREATE_FILE_A, c"CreateFileA", hook_create_file_a),
        k32(
            &REAL_CREATE_FILE_MAPPING_A,
            c"CreateFileMappingA",
            hook_create_file_mapping_a,
        ),
        k32(
            &REAL_OPEN_FILE_MAPPING_A,
            c"OpenFileMappingA",
            hook_open_file_mapping_a,
        ),
        k32(
            &REAL_DEVICE_IO_CONTROL,
            c"DeviceIoControl",
            hook_device_io_control,
        ),
    ]
}

/// Is this file name worth logging? The device namespace, or a cabinet token. Ordinary game files (textures, data) are
/// skipped to keep the log readable.
fn interesting(name: impl Iterator<Item = char>) -> bool {
    let n: String = name
        .take(599)
        .map(|c| {
            if c.is_ascii() {
                c.to_ascii_lowercase()
            } else {
                '?'
            }
        })
        .collect();
    n.starts_with(r"\\")
        || [
            "moai", "mx", "5c49e1fe", "keychip", "memcard", "eeprom", "busram", "sram", "jvs",
            "columba", "superio", "parallel",
        ]
        .iter()
        .any(|t| n.contains(t))
}

fn ok(h: HANDLE) -> &'static str {
    if h == INVALID_HANDLE_VALUE {
        "FAIL"
    } else {
        "ok"
    }
}

unsafe extern "system" fn hook_create_file_w(
    name: *const u16,
    acc: u32,
    share: u32,
    sa: Sa,
    disp: u32,
    flags: u32,
    tmpl: HANDLE,
) -> HANDLE {
    let w = Wide(name);
    if RINGEDGE.load(Relaxed) && !name.is_null() {
        let n: String = w.chars().collect();
        if n.eq_ignore_ascii_case(r"\\.\mxsram") {
            return sram_open();
        }
        if n.eq_ignore_ascii_case(r"\\.\columba") {
            return columba_open();
        }
    }
    // SAFETY: the game's own call, passed on unchanged
    let h = unsafe { REAL_CREATE_FILE_W.get()(name, acc, share, sa, disp, flags, tmpl) };
    // SAFETY: plain Win32 calls; logging must not disturb the caller's last-error
    let err = unsafe { GetLastError() };
    if VERBOSE.load(Relaxed) || (!name.is_null() && interesting(w.chars())) {
        log!(
            "CreateFileW   \"{w}\"  access=0x{acc:08X} -> {}  err={err}  (h=0x{:08X})",
            ok(h),
            h as usize
        );
    }
    // SAFETY: plain Win32 call
    unsafe { SetLastError(err) };
    h
}

unsafe extern "system" fn hook_create_file_a(
    name: *const u8,
    acc: u32,
    share: u32,
    sa: Sa,
    disp: u32,
    flags: u32,
    tmpl: HANDLE,
) -> HANDLE {
    if let Some(i) = serial::port_for_name(name) {
        return serial::open(i);
    }
    // SAFETY: the game's own call, passed on unchanged
    let h = unsafe { REAL_CREATE_FILE_A.get()(name, acc, share, sa, disp, flags, tmpl) };
    // SAFETY: plain Win32 calls; logging must not disturb the caller's last-error
    let err = unsafe { GetLastError() };
    let t = Text(name);
    if VERBOSE.load(Relaxed) || (!name.is_null() && interesting(t.to_string().chars())) {
        log!(
            "CreateFileA   \"{t}\"  access=0x{acc:08X} -> {}  err={err}  (h=0x{:08X})",
            ok(h),
            h as usize
        );
    }
    // SAFETY: plain Win32 call
    unsafe { SetLastError(err) };
    h
}

unsafe extern "system" fn hook_create_file_mapping_a(
    fh: HANDLE,
    sa: Sa,
    prot: u32,
    hi: u32,
    lo: u32,
    name: *const u8,
) -> HANDLE {
    // SAFETY: the game's own call, passed on unchanged; the game checks ERROR_ALREADY_EXISTS after it
    unsafe {
        let h = REAL_CREATE_FILE_MAPPING_A.get()(fh, sa, prot, hi, lo, name);
        let err = GetLastError();
        let n = if name.is_null() {
            "(anon)".into()
        } else {
            Text(name).to_string()
        };
        log!(
            "CreateFileMappingA  \"{n}\"  size={lo} -> 0x{:08X}  err={err}",
            h as usize
        );
        SetLastError(err);
        h
    }
}

unsafe extern "system" fn hook_open_file_mapping_a(
    acc: u32,
    inherit: i32,
    name: *const u8,
) -> HANDLE {
    // SAFETY: the game's own call, passed on unchanged
    unsafe {
        let h = REAL_OPEN_FILE_MAPPING_A.get()(acc, inherit, name);
        let err = GetLastError();
        let found = if h.is_null() { "NOT FOUND" } else { "FOUND" };
        log!(
            "OpenFileMappingA    \"{}\"  -> {found}  err={err}  (h=0x{:08X})",
            Text(name),
            h as usize
        );
        SetLastError(err);
        h
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn hook_device_io_control(
    h: HANDLE,
    code: u32,
    ib: *mut c_void,
    is: u32,
    ob: *mut c_void,
    os: u32,
    ret: *mut u32,
    ov: *mut c_void,
) -> i32 {
    // SAFETY: the buffers and sizes are the game's own call's
    unsafe {
        if RINGEDGE.load(Relaxed)
            && let Some(r) = mxdev_ioctl(h, code, ib, is, ob, os, ret)
        {
            let err = GetLastError();
            log!(
                "DeviceIoControl     h=0x{:08X}  ioctl=0x{code:08X}  inSz={is} outSz={os} -> {} (emulated)",
                h as usize,
                if r { "ok" } else { "FAIL" }
            );
            SetLastError(err);
            return i32::from(r);
        }
        let r = REAL_DEVICE_IO_CONTROL.get()(h, code, ib, is, ob, os, ret, ov);
        let err = GetLastError();
        log!(
            "DeviceIoControl     h=0x{:08X}  ioctl=0x{code:08X}  inSz={is} outSz={os} -> {}  err={err}",
            h as usize,
            if r != 0 { "ok" } else { "FAIL" }
        );
        SetLastError(err);
        r
    }
}

// ---- the stand-in devices ----

const MAX: usize = 16;
const SRAM: u8 = 1;
const COLUMBA: u8 = 2;
const SRAM_SIZE: i64 = 2 * 1024 * 1024;
const SRAM_SECTOR: u32 = 512;
static DEV: [AtomicUsize; MAX] = [const { AtomicUsize::new(0) }; MAX];
static KIND: [AtomicU8; MAX] = [const { AtomicU8::new(0) }; MAX];
static AREA: OnceLock<Vec<u8>> = OnceLock::new();

fn kind_of(h: HANDLE) -> u8 {
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        return 0;
    }
    (0..MAX)
        .find(|&i| DEV[i].load(Relaxed) == h as usize)
        .map_or(0, |i| KIND[i].load(Relaxed))
}

fn add(h: HANDLE, kind: u8) {
    for i in 0..MAX {
        if DEV[i]
            .compare_exchange(0, h as usize, Relaxed, Relaxed)
            .is_ok()
        {
            KIND[i].store(kind, Relaxed);
            return;
        }
    }
}

/// A closed handle is no device any more (CloseHandle's hook).
pub fn forget(h: HANDLE) {
    for d in &DEV {
        let _ = d.compare_exchange(h as usize, 0, Relaxed, Relaxed);
    }
}

fn sram_open() -> HANDLE {
    // SAFETY: plain file calls on the handle opened here
    unsafe {
        let h = REAL_CREATE_FILE_A.get()(
            c"mxsram.bin".as_ptr().cast(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            log!("mxsram: cannot open mxsram.bin (err {})", GetLastError());
            return h;
        }
        let mut size = 0;
        if GetFileSizeEx(h, &mut size) != 0 && size < SRAM_SIZE {
            SetFilePointerEx(h, SRAM_SIZE, std::ptr::null_mut(), FILE_BEGIN);
            SetEndOfFile(h);
            SetFilePointerEx(h, 0, std::ptr::null_mut(), FILE_BEGIN);
            log!("mxsram: new mxsram.bin, {SRAM_SIZE} bytes of zeros");
        }
        add(h, SRAM);
        log!(r"mxsram: \\.\mxsram -> mxsram.bin (h=0x{:08X})", h as usize);
        SetLastError(0);
        h
    }
}

fn columba_open() -> HANDLE {
    // SAFETY: the NUL device, opened as a handle for the game to hold
    unsafe {
        let h = REAL_CREATE_FILE_A.get()(
            c"NUL".as_ptr().cast(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if h != INVALID_HANDLE_VALUE {
            AREA.get_or_init(dmi::build);
            add(h, COLUMBA);
        }
        log!(
            r"columba: \\.\columba -> fake BIOS area 0x{:05X}-0x{:05X}, SMBIOS table at 0x{:05X}, anchor at 0x{:05X} (h=0x{:08X})",
            dmi::BASE,
            dmi::BASE + dmi::SIZE - 1,
            dmi::TABLE,
            dmi::ANCHOR,
            h as usize
        );
        SetLastError(0);
        h
    }
}

/// Answers a device question: Some(success) if `h` is a stand-in device, None to pass the call through.
///
/// # Safety
/// The buffers are valid for their sizes (the game's own call).
unsafe fn mxdev_ioctl(
    h: HANDLE,
    code: u32,
    ib: *mut c_void,
    is: u32,
    ob: *mut c_void,
    os: u32,
    ret: *mut u32,
) -> Option<bool> {
    let kind = kind_of(h);
    if kind == 0 {
        return None;
    }
    let out = |n: u32| {
        // SAFETY: the game's output buffer holds `os` >= n bytes (checked by each answer)
        unsafe { std::slice::from_raw_parts_mut(ob.cast::<u8>(), n as usize) }
    };
    let n = if kind == COLUMBA {
        if code != 0x9C40_6104 || is < 16 || ib.is_null() || ob.is_null() {
            log!("columba: unhandled ioctl 0x{code:08X} (in {is} out {os})");
            // SAFETY: plain Win32 call
            unsafe { SetLastError(ERROR_INVALID_FUNCTION) };
            return Some(false);
        }
        // SAFETY: the input holds at least 16 bytes (checked above)
        let addr = unsafe { ib.cast::<u32>().read_unaligned() };
        dmi::read(AREA.get_or_init(dmi::build), addr, out(os));
        log!("columba: read {os} bytes at physical 0x{addr:05X}");
        os
    } else {
        match code {
            0x9C40_6000 if os >= 4 => {
                out(4).copy_from_slice(&1u32.to_le_bytes());
                4
            }
            // DISK_GEOMETRY: Cylinders (i64), MediaType FixedMedia (12), TracksPerCylinder, SectorsPerTrack, BytesPerSector
            0x0007_0000 if os >= 24 => {
                let g = out(24);
                g[..8].copy_from_slice(&1i64.to_le_bytes());
                g[8..12].copy_from_slice(&12u32.to_le_bytes());
                g[12..16].copy_from_slice(&1u32.to_le_bytes());
                g[16..20].copy_from_slice(&((SRAM_SIZE as u32) / SRAM_SECTOR).to_le_bytes());
                g[20..24].copy_from_slice(&SRAM_SECTOR.to_le_bytes());
                24
            }
            0x9C40_6004 if os >= 4 => {
                out(4).copy_from_slice(&SRAM_SECTOR.to_le_bytes());
                4
            }
            0x0007_405C if os >= 8 => {
                out(8).copy_from_slice(&SRAM_SIZE.to_le_bytes());
                8
            }
            _ => {
                log!("mxsram: unhandled ioctl 0x{code:08X} (out {os})");
                // SAFETY: plain Win32 call
                unsafe { SetLastError(ERROR_INVALID_FUNCTION) };
                return Some(false);
            }
        }
    };
    if !ret.is_null() {
        // SAFETY: the game's bytes-returned pointer
        unsafe { ret.write(n) };
    }
    Some(true)
}
