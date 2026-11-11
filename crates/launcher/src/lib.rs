//! Launcher.exe's shared code: the kit folder, the run (`run`: play.py in Rust, step 10) and what it is built on - the
//! Job object, the debugger, the key driver, the game's settings files, the club card's session, the logs.

pub mod alias;
pub mod choices;
pub mod cleanup;
pub mod conf;
pub mod debug;
pub mod guard;
pub mod helper;
pub mod home;
pub mod job;
pub mod lobby;
pub mod logs;
pub mod notice;
pub mod release;
pub mod report;
pub mod run;
pub mod setup;

// the kit folder, the key driver, the club card's session and the game's processes: in forza, which the panel shares
pub use forza::{card, keys, kit, procs};
pub use kit::{Edition, Kit};

/// A scratch folder under `%TEMP%` whose name starts with `wccf-`, where the tests run: one Defender exclusion of
/// `%TEMP%\wccf-*` then covers every file they write, the copies of the test programs included (the model flagged the
/// `%TEMP%\.tmp*` ones the tests used before, `Trojan:Win32/Bearfoos.A!ml`, 2026-11-11).
#[cfg(test)]
pub(crate) fn scratch() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("wccf-").tempdir().unwrap()
}
