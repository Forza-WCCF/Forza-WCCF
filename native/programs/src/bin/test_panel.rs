//! Not shipped. test_panel.exe <wccfnet.dll> <panel_probe.dll> <on|off>: loads the guard, opens a visible window and
//! pumps messages for 6 s. "on" (the Launcher sets FORZA_PANEL to the probe) wants the probe loaded by then, "off" (no
//! variable) wants it never loaded. Either way the variable is gone afterwards (children never inherit it). The guard's
//! loader waits 3 s after the window shows.
#[path = "../check.rs"]
mod check;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExA, DispatchMessageA, MSG, PM_REMOVE, PeekMessageA, WS_OVERLAPPEDWINDOW,
    WS_VISIBLE,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        return ExitCode::from(2);
    }
    let want = args[3] == "on";
    if check::load(&args[1]).is_null() {
        println!("FAIL: wccfnet.dll loads");
        return ExitCode::from(1);
    }
    // SAFETY: a plain window of a system class, and this thread's own message queue
    unsafe {
        let w = CreateWindowExA(
            0,
            c"STATIC".as_ptr().cast(),
            c"test_panel".as_ptr().cast(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            0,
            0,
            200,
            100,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        if w.is_null() {
            println!("FAIL: window");
            return ExitCode::from(1);
        }
        let end = Instant::now() + Duration::from_secs(6);
        let mut m: MSG = std::mem::zeroed();
        while Instant::now() < end {
            while PeekMessageA(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                DispatchMessageA(&m);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    // SAFETY: a NUL-terminated name
    let loaded = !unsafe { GetModuleHandleA(c"panel_probe.dll".as_ptr().cast()) }.is_null();
    println!(
        "{}: panel probe {} with FORZA_PANEL {}",
        if loaded == want { "ok  " } else { "FAIL" },
        if loaded { "loaded" } else { "not loaded" },
        if want { "set" } else { "unset" }
    );
    if std::env::var_os("FORZA_PANEL").is_some() {
        println!("FAIL: FORZA_PANEL still set (children would load the panel)");
        return ExitCode::from(1);
    }
    ExitCode::from(u8::from(loaded != want))
}
