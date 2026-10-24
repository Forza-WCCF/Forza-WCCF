//! Not shipped. test_icmp.exe <path to ICMP.DLL>: loads the stand-in, checks its three exports, and that IcmpSendEcho
//! returns 0 at once (well under the game's 5 s timeout) and leaves the reply buffer untouched.
#[path = "../check.rs"]
mod check;
use check::{check, done, load};
use std::ffi::c_void;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::LibraryLoader::GetProcAddress;

type Create = unsafe extern "system" fn() -> HANDLE;
type Close = unsafe extern "system" fn(HANDLE) -> i32;
type Echo = unsafe extern "system" fn(
    HANDLE,
    u32,
    *const c_void,
    u16,
    *mut c_void,
    *mut c_void,
    u32,
    u32,
) -> u32;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        return ExitCode::from(2);
    }
    let m = load(&args[1]);
    if !check(!m.is_null(), "ICMP.DLL loads") {
        return ExitCode::from(1);
    }
    // SAFETY: NUL-terminated names in the loaded stand-in; each is cast to its documented signature
    let (create, close, echo) = unsafe {
        let f = |n: &std::ffi::CStr| GetProcAddress(m, n.as_ptr().cast());
        (
            f(c"IcmpCreateFile")
                .map(|p| std::mem::transmute::<unsafe extern "system" fn() -> isize, Create>(p)),
            f(c"IcmpCloseHandle")
                .map(|p| std::mem::transmute::<unsafe extern "system" fn() -> isize, Close>(p)),
            f(c"IcmpSendEcho")
                .map(|p| std::mem::transmute::<unsafe extern "system" fn() -> isize, Echo>(p)),
        )
    };
    let (Some(create), Some(close), Some(echo)) = (create, close, echo) else {
        check(
            false,
            "exports IcmpCreateFile, IcmpCloseHandle, IcmpSendEcho",
        );
        return ExitCode::from(1);
    };
    check(
        true,
        "exports IcmpCreateFile, IcmpCloseHandle, IcmpSendEcho",
    );
    // SAFETY: the stand-in's own calls, with a reply buffer of the size given
    unsafe {
        let h = create();
        check(
            h != INVALID_HANDLE_VALUE && !h.is_null(),
            "IcmpCreateFile returns a usable handle",
        );
        let mut reply = [0xA5u8; 64];
        let t = Instant::now();
        let n = echo(
            h,
            0xFE2E_A8C0,
            c"x".as_ptr().cast(),
            1,
            std::ptr::null_mut(),
            reply.as_mut_ptr().cast(),
            64,
            5000,
        );
        check(n == 0, "IcmpSendEcho returns 0");
        check(
            t.elapsed() < Duration::from_millis(500),
            "IcmpSendEcho returns at once (timeout argument 5000 ms)",
        );
        check(reply == [0xA5; 64], "reply buffer untouched");
        check(close(h) != 0, "IcmpCloseHandle succeeds");
    }
    done("icmp test")
}
