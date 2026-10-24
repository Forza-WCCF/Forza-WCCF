//! Not shipped. The server hook's SPIN fix ([forza-hook e1011-server]): run with MXHOOK_SPINFIX=1 under Wine beside the
//! server's winmm.dll, winmm_orig.dll and netguard.dll (WINEDLLOVERRIDES=winmm=n,b). Forced, every timeGetTime call of
//! this program waits 10 ms first, so 20 calls take at least about 200 ms and the returned time still advances; prints
//! PASS.
use std::process::ExitCode;
use std::time::Instant;

#[link(
    name = "winmm.dll",
    kind = "raw-dylib",
    modifiers = "+verbatim",
    import_name_type = "undecorated"
)]
unsafe extern "system" {
    fn timeGetTime() -> u32;
}

fn main() -> ExitCode {
    let t = Instant::now();
    // SAFETY: no arguments
    let t0 = unsafe { timeGetTime() };
    let mut t1 = t0;
    for _ in 0..19 {
        // SAFETY: no arguments
        t1 = unsafe { timeGetTime() };
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let ok = ms >= 180.0 && t1.wrapping_sub(t0) >= 170;
    println!(
        "{}: 20 timeGetTime calls took {ms:.1} ms, timeGetTime advanced {} ms",
        if ok { "PASS" } else { "FAIL" },
        t1.wrapping_sub(t0)
    );
    ExitCode::from(u8::from(!ok))
}
