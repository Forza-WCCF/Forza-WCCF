//! bin\logowin.exe, the stand-in for the original logowin.exe (2017-18). control_Release.exe starts the original with
//! its message on the command line ("logowin.exe error=SYS_NET_error_0014" for an error, "logowin.exe warning=..
//! allnet=.. mainid=.. keychipid=.." for the logo screen), and it opens a full-screen, white, always-on-top window,
//! which covered the player's whole screen.
//!
//! This one shows nothing: it appends the time, the starting process and its own command line to logowin_messages.txt
//! in the current folder (control starts it in the game folder), then waits until the program that started it ends,
//! as the original stays up while control runs. (2010-11 has no logowin.exe: the hook answers instead.)
#![windows_subsystem = "windows"]
use std::io::Write;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Environment::GetCommandLineW;
use windows_sys::Win32::System::Threading::{
    INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

fn main() {
    let ppid = forza_native::parent_pid();
    let parent = if ppid == 0 {
        std::ptr::null_mut()
    } else {
        // SAFETY: a wait-only handle to the parent, closed at the end
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, ppid) }
    };
    let t = forza_native::local_time();
    // SAFETY: the process's own command line, NUL-terminated
    let cmd = unsafe {
        let p = GetCommandLineW();
        String::from_utf16_lossy(std::slice::from_raw_parts(
            p,
            (0..).take_while(|&i| *p.add(i) != 0).count(),
        ))
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open("logowin_messages.txt")
    {
        let _ = writeln!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}  started by pid {ppid}  {cmd}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        );
    }
    if !parent.is_null() {
        // SAFETY: the handle opened above
        unsafe {
            WaitForSingleObject(parent, INFINITE);
            CloseHandle(parent);
        }
    }
}
