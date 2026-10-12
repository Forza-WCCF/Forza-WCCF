//! Launcher.exe's shared code (in place of the .bat files, 2026-10-08).  It runs one kit script with the kit's own Python
//! (python\python.exe) in the console Launcher was started from, its arguments passed on as they are, and keeps the run's
//! watcher (watch.rs): once a game window is closed, it stops the rest.

pub mod card;
pub mod conf;
pub mod job;
pub mod kit;
pub mod release;
mod watch;

pub use kit::{Kit, SCRIPTS};
pub use watch::{kit_program_running, run_recorded, spawn_watcher, take_over, watch};
