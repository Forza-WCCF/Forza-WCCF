//! Not shipped. test_logowin.exe <path to the 2017-18 winmm.dll>: once the hook has loaded, this program's
//! CreateProcessA of logowin.exe, made as control_Release makes it, must be answered in-process: success, no new
//! process, a handle to this process that only queries (control's TerminateProcess on it fails), and the message in
//! logowin_messages.txt in the current folder. The start is made only once the import points into the hook; the test
//! folder has no logowin.exe, so even an unhooked call starts nothing.
#[path = "../check.rs"]
mod check;
use check::{check, done, load, owner, target};
use std::process::ExitCode;
use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
use windows_sys::Win32::System::Threading::{
    CreateProcessA, GetExitCodeProcess, GetProcessId, PROCESS_INFORMATION, STARTUPINFOA,
    TerminateProcess,
};

const MESSAGES: &str = "logowin_messages.txt";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        return ExitCode::from(2);
    }
    let _ = std::fs::remove_file(MESSAGES);
    let h = load(&args[1]);
    if !check(!h.is_null(), "winmm.dll loads") {
        return ExitCode::from(1);
    }
    let into = owner(target(std::hint::black_box(
        CreateProcessA as *const () as usize,
    ))) == h;
    if !check(into, "the CreateProcessA import points into winmm.dll") {
        return done("logowin hook test");
    }
    let mut cmd = *b"logowin.exe error=TEST_error_0001\0";
    // SAFETY: plain structs, all zero is their empty value
    let mut si: STARTUPINFOA = unsafe { std::mem::zeroed() };
    si.cb = size_of::<STARTUPINFOA>() as u32;
    // SAFETY: as above
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: NUL-terminated strings and live structs; the import points into the hook (checked above)
    let ok = unsafe {
        CreateProcessA(
            c"logowin.exe".as_ptr().cast(),
            cmd.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        )
    };
    if !check(ok != 0, "the start of logowin.exe succeeds") {
        return done("logowin hook test");
    }
    check(pi.dwProcessId == 0, "no new process (process id 0)");
    // SAFETY: the handles the hook gave; each is closed once
    unsafe {
        check(
            GetProcessId(pi.hProcess) == std::process::id(),
            "the handle is this process's own",
        );
        let mut code = 0;
        check(
            GetExitCodeProcess(pi.hProcess, &mut code) != 0 && code == STILL_ACTIVE as u32,
            "it reads as still running",
        );
        check(
            TerminateProcess(pi.hProcess, 1) == 0,
            "TerminateProcess on it fails (query rights only)",
        );
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
    let text = std::fs::read_to_string(MESSAGES).unwrap_or_default();
    check(
        text.contains("logowin.exe error=TEST_error_0001"),
        "the message is in logowin_messages.txt",
    );
    done("logowin hook test")
}
