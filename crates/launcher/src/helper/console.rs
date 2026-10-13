//! QuickEdit off in another program's console window (console.py's place).  While a console is in selection mode
//! ("click selects text") the program writing to it is FROZEN: on 2026-10-04 a click on the server's window froze the
//! control server, and both screens fell to Error 3000.  With QuickEdit off, a click does nothing.
//!
//! A process sits in one console at a time and attaching changes this one's, so this runs in a child of the run:
//! `Launcher.exe --no-quickedit PID`.

use std::ffi::OsString;
use std::io;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::Console::{
    AttachConsole, ENABLE_EXTENDED_FLAGS, ENABLE_QUICK_EDIT_MODE, FreeConsole, GetConsoleMode, SetConsoleMode,
};

/// The word that makes Launcher.exe do only this.
pub const ARG: &str = "--no-quickedit";

/// QuickEdit switched off in the console of process `pid`: the input mode before and after, or why not.
pub fn no_quickedit(pid: u32) -> Result<(u32, u32), String> {
    // SAFETY: Win32 console calls with valid arguments; the input handle is closed and the console left before return.
    unsafe {
        FreeConsole();
        if AttachConsole(pid) == 0 {
            return Err(format!(
                "cannot attach to its console ({}) - it may have none",
                io::Error::last_os_error()
            ));
        }
        let conin: Vec<u16> = "CONIN$".encode_utf16().chain([0]).collect();
        let h = CreateFileW(
            conin.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        );
        let result = if h == INVALID_HANDLE_VALUE {
            Err(format!("no console input ({})", io::Error::last_os_error()))
        } else {
            let (mut before, mut after) = (0, 0);
            GetConsoleMode(h, &mut before);
            let ok = SetConsoleMode(h, (before & !ENABLE_QUICK_EDIT_MODE) | ENABLE_EXTENDED_FLAGS) != 0;
            GetConsoleMode(h, &mut after);
            CloseHandle(h);
            if ok && after & ENABLE_QUICK_EDIT_MODE == 0 {
                Ok((before, after))
            } else {
                Err(format!("mode {before:#06X} -> {after:#06X}, QuickEdit still on"))
            }
        };
        FreeConsole();
        result
    }
}

/// `Launcher.exe --no-quickedit PID ...`: one line per process, exit 0 when all are done.
pub fn child_main(pids: &[OsString]) -> i32 {
    let mut rc = 0;
    for a in pids {
        let line = match a
            .to_string_lossy()
            .parse()
            .map_err(|_| "not a process id".to_owned())
            .and_then(no_quickedit)
        {
            Ok((before, after)) => format!(
                "pid {}: QuickEdit OFF (mode {before:#06X} -> {after:#06X})",
                a.to_string_lossy()
            ),
            Err(why) => {
                rc = 1;
                format!("pid {}: NOT changed ({why})", a.to_string_lossy())
            }
        };
        println!("{line}");
    }
    rc
}
