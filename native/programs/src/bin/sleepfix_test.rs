//! Not shipped. The server hook's SLEEP fix ([forza-hook e1011-server]): with MXHOOK_SLEEPFIX=1 the hook replaces this
//! program's Sleep import; Sleep(16) and Sleep(1) must still wait at least about their time, and Sleep(0) must return.
//! Run under Wine beside the server's winmm.dll, winmm_orig.dll and netguard.dll; prints PASS.
#[path = "../check.rs"]
mod check;
use std::process::ExitCode;
use std::time::Instant;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Threading::Sleep;

fn ms_of(n: u32) -> f64 {
    let t = Instant::now();
    // SAFETY: no pointers
    unsafe { Sleep(n) };
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() -> ExitCode {
    // SAFETY: one thread so far
    unsafe { std::env::set_var("MXHOOK_SLEEPFIX", "1") };
    // SAFETY: NUL-terminated names; kernel32 is loaded in every process
    let (dll, kernel_sleep) = unsafe {
        (
            LoadLibraryA(c"winmm.dll".as_ptr().cast()),
            GetProcAddress(
                GetModuleHandleA(c"kernel32.dll".as_ptr().cast()),
                c"Sleep".as_ptr().cast(),
            ),
        )
    };
    if dll.is_null() {
        println!("FAIL: winmm.dll did not load");
        return ExitCode::from(1);
    }
    let hooked = check::target(std::hint::black_box(Sleep as *const () as usize))
        != kernel_sleep.map_or(0, |f| f as usize);
    let (mut t16, mut t1) = (f64::MAX, f64::MAX);
    for i in 0..450 {
        // past both sample windows: the replacement is what runs
        let (a, b) = (ms_of(16), ms_of(1));
        if i >= 400 {
            t16 = t16.min(a);
            t1 = t1.min(b);
        }
    }
    ms_of(0);
    let ok = hooked && t16 >= 15.0 && t1 >= 0.9;
    println!(
        "{}: min Sleep(16) {t16:.2} ms, min Sleep(1) {t1:.2} ms{}",
        if ok { "PASS" } else { "FAIL" },
        if hooked { "" } else { " (Sleep not hooked)" }
    );
    ExitCode::from(u8::from(!ok))
}
