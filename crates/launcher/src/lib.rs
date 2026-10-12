//! Launcher.exe's shared code (in place of the .bat files, 2026-10-08).  It runs one kit script with the kit's own Python
//! (python\python.exe) in the console Launcher was started from, its arguments passed on as they are, and keeps the run's
//! watcher (watch.rs): once a game window is closed, it stops the rest.

pub mod job;
pub mod release;
mod watch;

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows_sys::core::BOOL;

pub use watch::{kit_program_running, run_recorded, spawn_watcher, take_over, watch};

/// The kit's Python scripts folder (`cargo xtask check` checks the staged kit has play.py, setup.py and english.py in it).
pub const SCRIPTS: &str = "code";

/// The kit folder: the one this exe is in.
pub struct Kit {
    dir: PathBuf,
}

impl Kit {
    pub fn here() -> Self {
        let exe = env::current_exe().unwrap_or_default();
        let dir = exe.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn python(&self) -> PathBuf {
        self.dir.join("python").join("python.exe")
    }

    fn data(&self) -> PathBuf {
        self.dir.join("data")
    }

    /// `python\python.exe code\<script>`, to add arguments to
    pub fn script(&self, script: &str) -> Command {
        let mut cmd = Command::new(self.python());
        cmd.arg(self.dir.join(SCRIPTS).join(script));
        cmd
    }

    /// Runs a kit script in this window; its exit code (1 if it could not run).
    pub fn run<S: AsRef<OsStr>>(&self, script: &str, args: &[S]) -> u8 {
        // Ctrl+C goes to the script (Python stops with its own message), not to Launcher.  A handler, not SetConsoleCtrlHandler(None, TRUE): that one the script would inherit.
        // SAFETY: `keep_window` is a valid handler for the life of the process.
        unsafe { SetConsoleCtrlHandler(Some(keep_window), TRUE) };
        match self.script(script).args(args).status() {
            Ok(status) => status.code().and_then(|c| u8::try_from(c).ok()).unwrap_or(1),
            Err(e) => {
                println!("Could not start the kit's Python ({}): {e}", self.python().display());
                1
            }
        }
    }
}

unsafe extern "system" fn keep_window(event: u32) -> BOOL {
    if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
        TRUE
    } else {
        FALSE
    }
}
