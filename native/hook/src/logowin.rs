//! logowin.exe (2026-10-09): control_Release starts the original logowin.exe for its logo and error screens
//! (FUN_00453bd0: CreateProcessA("logowin.exe", "logowin.exe error=..." or "... warning=.. allnet=.. mainid=..
//! keychipid=..")), a white, always-on-top window over the whole screen. It only ever asks whether it still runs
//! (GetExitCodeProcess == STILL_ACTIVE, FUN_00453f70) and, to end it, posts WM_CLOSE to the window "logowin" or calls
//! TerminateProcess (FUN_00453ef0). So no program is started: the message goes to logowin_messages.txt in the current
//! folder (the game's), as the stand-in logowin.exe writes it, and the server gets a handle to its own process with
//! query rights only: "still running", as the stand-in was until the server ended, and a TerminateProcess on it fails
//! harmlessly. A windowless stand-in exe that waited on its parent was what antivirus programs flagged. Any other
//! CreateProcessA (match_Release, FPR_Emu) passes. 2017-18 (2026-10-29): its control_Release starts logowin.exe the
//! same way ("%s error=%s", "%s warning=%s ..." with "logowin.exe"), so bin\winmm.dll answers it too and the kit no
//! longer ships a stand-in logowin.exe.

/// Does this CreateProcessA start logowin.exe? The program's name after its last backslash (of the application name,
/// else of the command line).
pub fn is_logowin(app: Option<&[u8]>, cmd: Option<&[u8]>) -> bool {
    let Some(p) = app.or(cmd) else {
        return false;
    };
    let base = p.rsplit(|&b| b == b'\\').next().unwrap_or(p);
    base.len() >= 11
        && base[..11].eq_ignore_ascii_case(b"logowin.exe")
        && matches!(base.get(11), None | Some(b' ' | b'"'))
}

#[cfg(all(target_arch = "x86", any(feature = "e1011", feature = "e1718")))]
pub use hook::install;

#[cfg(all(target_arch = "x86", any(feature = "e1011", feature = "e1718")))]
mod hook {
    use crate::log::log;
    use crate::patch::{Real, k32};
    use std::ffi::{CStr, c_void};
    use std::io::Write;
    use windows_sys::Win32::Foundation::{
        CloseHandle, DuplicateHandle, ERROR_FILE_NOT_FOUND, HANDLE, SetLastError,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE,
    };

    type CreateProcessA = unsafe extern "system" fn(
        *const u8,
        *mut u8,
        *const c_void,
        *const c_void,
        i32,
        u32,
        *const c_void,
        *const u8,
        *const c_void,
        *mut PROCESS_INFORMATION,
    ) -> i32;
    static REAL: Real<CreateProcessA> = Real::new();

    /// True if the program's CreateProcessA import now comes here.
    pub fn install() -> bool {
        k32(&REAL, c"CreateProcessA", hook_create_process_a)
    }

    fn bytes<'a>(p: *const u8) -> Option<&'a [u8]> {
        // SAFETY: the game's NUL-terminated strings
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p.cast()) }.to_bytes())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe extern "system" fn hook_create_process_a(
        app: *const u8,
        cmd: *mut u8,
        pa: *const c_void,
        ta: *const c_void,
        inherit: i32,
        flags: u32,
        env: *const c_void,
        dir: *const u8,
        si: *const c_void,
        pi: *mut PROCESS_INFORMATION,
    ) -> i32 {
        if !super::is_logowin(bytes(app), bytes(cmd)) {
            // SAFETY: the game's own call, passed on
            return unsafe { REAL.get()(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) };
        }
        let what =
            String::from_utf8_lossy(bytes(cmd).or(bytes(app)).unwrap_or_default()).into_owned();
        let t = forza_native::local_time();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open("logowin_messages.txt")
        {
            let _ = writeln!(
                f,
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}  started by pid {}  {what}  (not run: Forza-WCCF's hook)",
                t.wYear,
                t.wMonth,
                t.wDay,
                t.wHour,
                t.wMinute,
                t.wSecond,
                std::process::id()
            );
        }
        log!("logowin: not started ({what})");
        // SAFETY: handles to our own process with query rights only, given to the game as the started program's
        unsafe {
            let me = GetCurrentProcess();
            let (mut h, mut t): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
            let rights = PROCESS_QUERY_LIMITED_INFORMATION;
            if pi.is_null()
                || DuplicateHandle(me, me, me, &mut h, rights | PROCESS_SYNCHRONIZE, 0, 0) == 0
                || DuplicateHandle(me, me, me, &mut t, rights, 0, 0) == 0
            {
                if !h.is_null() {
                    CloseHandle(h);
                }
                SetLastError(ERROR_FILE_NOT_FOUND);
                return 0; // the server's own "could not start it" path: nothing shows either way
            }
            pi.write(PROCESS_INFORMATION {
                hProcess: h,
                hThread: t,
                dwProcessId: 0,
                dwThreadId: 0,
            }); // the server closes hThread at once
        }
        1
    }
}

#[cfg(test)]
mod tests {
    use super::is_logowin;

    #[test]
    fn logowin_and_nothing_else() {
        assert!(is_logowin(
            Some(b"logowin.exe"),
            Some(b"logowin.exe error=SYS_NET_error_0014")
        ));
        assert!(is_logowin(None, Some(b"LOGOWIN.EXE warning=1")));
        // 2017-18 control_Release: application "logowin.exe", command line "%s warning=%s ..." (fields shortened)
        assert!(is_logowin(
            Some(b"logowin.exe"),
            Some(b"logowin.exe warning=1 allnet=0 mainid=0 keychipid=0")
        ));
        assert!(is_logowin(Some(br"C:\game\logowin.exe"), None));
        assert!(!is_logowin(Some(b"match_Release.exe"), None));
        assert!(!is_logowin(None, Some(br"FPR_Emu.exe 0 32")));
        assert!(!is_logowin(Some(b"logowin.exe.bak"), None));
        assert!(!is_logowin(None, None));
    }
}
