//! What the test programs share: one line per check (`ok  : ...` or `FAIL: ...`), a last line with the count, and
//! where an imported function's calls go now.
#![allow(dead_code)] // each test program uses its own part

use std::ffi::c_void;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleHandleExW, LoadLibraryW,
};

static FAILS: AtomicU32 = AtomicU32::new(0);

pub fn check(ok: bool, what: &str) -> bool {
    println!("{}: {what}", if ok { "ok  " } else { "FAIL" });
    if !ok {
        FAILS.fetch_add(1, Relaxed);
    }
    ok
}

/// The last line, and the exit code: 0 when every check passed.
pub fn done(test: &str) -> ExitCode {
    match FAILS.load(Relaxed) {
        0 => println!("{test}: all passed"),
        n => println!("{test}: {n} FAILED"),
    }
    ExitCode::from(u8::from(FAILS.load(Relaxed) != 0))
}

/// LoadLibraryW of a path from the command line (null if it does not load).
pub fn load(path: &str) -> HMODULE {
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-terminated path
    unsafe { LoadLibraryW(wide.as_ptr()) }
}

/// The loaded module `addr` lies in (null if none).
pub fn owner(addr: usize) -> HMODULE {
    let mut m: HMODULE = std::ptr::null_mut();
    let flags =
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    // SAFETY: only the address is looked up; nothing is read through it
    unsafe { GetModuleHandleExW(flags, addr as *const u16, &mut m) };
    m
}

/// Where calls of an imported function go now: the address an import slot holds. `f` is the function's address as the
/// compiler gives it: the import slot's value itself, or this program's `jmp [slot]` thunk (FF 25 and the slot's
/// address), whose slot is read.
pub fn target(f: usize) -> usize {
    // SAFETY: f is code in this process; a thunk holds the absolute address of its slot after its two opcode bytes
    unsafe {
        let p = f as *const u8;
        if p.read() == 0xFF && p.add(1).read() == 0x25 {
            (p.add(2).cast::<*const usize>().read_unaligned()).read_volatile()
        } else {
            f
        }
    }
}

pub type IpCall = unsafe extern "system" fn(*mut c_void) -> u32;

/// The checks of a DLL that moves the IP helper imports into itself: each import must point into `dll` (`label` in the
/// lines) before any is called, and then each must return NO_ERROR. Nothing reaches the real IP helper.
pub fn imports_into(dll: HMODULE, label: &str, verb: &str, calls: &[(&str, IpCall)]) {
    let mut all = true;
    for &(name, f) in calls {
        let inside = owner(target(std::hint::black_box(f as usize))) == dll;
        all &= check(inside, &format!("{name} {verb} into {label}"));
    }
    if all {
        for &(name, f) in calls {
            // SAFETY: the import points into the DLL under test (checked above), which ignores the argument
            check(
                unsafe { f(std::ptr::null_mut()) } == 0,
                &format!("{name}(NULL) returns NO_ERROR"),
            );
        }
    }
}
