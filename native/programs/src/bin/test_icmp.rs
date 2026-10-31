//! Not shipped. test_icmp.exe <path to wccfnet.dll>: loads the guard as the game's import does, then by bare name as the
//! game's (renamed) LoadLibraryA("ICMP.DLL") does, checks its three ICMP exports, and that IcmpSendEcho returns 0 at once
//! (well under the game's 5 s timeout), leaves the reply buffer untouched and logs one line.
#[path = "../check.rs"]
mod check;
use check::{check, done, load, owner};
use std::ffi::c_void;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

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
    if !check(!m.is_null(), "wccfnet.dll loads") {
        return ExitCode::from(1);
    }
    // SAFETY: a NUL-terminated name, the string setup writes over "ICMP.DLL"
    let by_name = unsafe { LoadLibraryA(c"wccfnet.dll".as_ptr().cast()) };
    check(
        by_name == m,
        "LoadLibraryA(\"wccfnet.dll\") gives the loaded guard",
    );
    let log = std::path::Path::new(&args[1]).with_file_name("wccfnet.log");
    let _ = std::fs::remove_file(&log);
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
    let inside = [
        create.map(|f| f as usize),
        close.map(|f| f as usize),
        echo.map(|f| f as usize),
    ]
    .iter()
    .all(|f| f.is_some_and(|f| owner(f) == m));
    let (Some(create), Some(close), Some(echo), true) = (create, close, echo, inside) else {
        check(
            false,
            "exports IcmpCreateFile, IcmpCloseHandle, IcmpSendEcho, inside wccfnet.dll",
        );
        return ExitCode::from(1);
    };
    check(
        true,
        "exports IcmpCreateFile, IcmpCloseHandle, IcmpSendEcho, inside wccfnet.dll",
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
        echo(
            h,
            0,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            reply.as_mut_ptr().cast(),
            64,
            5000,
        );
    }
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    check(
        text.lines().count() == 1
            && text.contains("icmp: ping to 192.168.46.254 answered no reply"),
        "wccfnet.log has one line for two pings, with the address",
    );
    done("icmp test")
}
