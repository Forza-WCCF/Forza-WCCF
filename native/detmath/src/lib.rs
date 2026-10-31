//! dmath.dll: the 2017-18 match engine's deterministic math. Setup's copy of match_Release.exe imports it instead of
//! PSAPI.DLL (its four calls are forwarded to Windows' psapi, build.rs), so it loads before the engine's own code runs.
//! In DllMain it checks the engine (the pinned original but for that one import name, every site's bytes) and then
//! replaces each of the 140 approximate SSE instructions with an exact computation ([`cave`]) and the engine's
//! `_CIatan2` import with a plain-SSE2 atan2 ([`ci_atan2`]). Results are then the same on every CPU, so two seats on
//! different CPUs stay in step. Any check that fails patches nothing; dmath.log (beside the DLL) says what happened.
//!
//! `dmath.cfg` beside the DLL (words, for tests only): `off` patches nothing, `foreign` gives the other-CPU estimate
//! (RUN.md V2), `frames` logs (frame, random number) per sim frame to `dmath_frames_<pid>.log` (V3).
#![cfg(target_arch = "x86")]

pub mod cave;
#[cfg(windows)]
mod dll;
pub mod harness;
pub mod sites;

#[cfg(windows)]
pub use dll::*;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

/// `foreign` in dmath.cfg (RUN.md V2): another CPU's estimates stand-in.
static FOREIGN: AtomicBool = AtomicBool::new(false);

/// The deterministic atan2 behind `ci_atan2`; `foreign` (RUN.md V2) flips the last bit of a finite result.
pub extern "C" fn det_atan2(y: f64, x: f64) -> f64 {
    let r = libm::atan2(y, x);
    if FOREIGN.load(Relaxed) && r.is_finite() {
        return f64::from_bits(r.to_bits() ^ 1);
    }
    r
}
