//! The kit folder (the one Launcher.exe is in) and what is in it: its Python, its scripts, data\ (settings, logs, the
//! club card, the cache), and the game folder setup recorded in data\settings.json with the seat and misc folders beside
//! it.  Port of kit_common.py's paths, load_settings, find_game, seat_dir and misc_dir.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows_sys::core::BOOL;

/// The kit's Python scripts folder (`cargo xtask check` checks the staged kit has play.py, setup.py and english.py in it).
pub const SCRIPTS: &str = "code";

/// The kit folder: the one this exe is in.
pub struct Kit {
    pub(crate) dir: PathBuf,
}

impl Kit {
    pub fn here() -> Self {
        let exe = env::current_exe().unwrap_or_default();
        let dir = exe.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self { dir }
    }

    /// A kit in `dir` (tests, tools).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn python(&self) -> PathBuf {
        self.dir.join("python").join("python.exe")
    }

    /// The kit's compiled programs: the hook, the panel, card-table.
    pub fn programs(&self) -> PathBuf {
        self.dir.join("programs")
    }

    pub fn data(&self) -> PathBuf {
        self.dir.join("data")
    }

    /// Made by setup from the player's game: catalogue.tsv, cards\.
    pub fn cache(&self) -> PathBuf {
        self.data().join("cache")
    }

    pub fn logs(&self) -> PathBuf {
        self.data().join("logs")
    }

    pub fn save(&self) -> PathBuf {
        self.data().join("save")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.data().join("settings.json")
    }

    /// data\settings.json; an empty object when it is missing or not JSON.
    pub fn settings(&self) -> serde_json::Value {
        fs::read_to_string(self.settings_file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    }

    /// The game folder setup recorded ("game" in data\settings.json), if it still has the game's programs.
    pub fn game(&self) -> Option<PathBuf> {
        find_game(self.settings().get("game")?.as_str()?)
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

/// The game folder (the one with the three game programs) from a path to it or to the folder above it (its
/// `extracted\`); spaces and quotes around the path do not count.
pub fn find_game(path: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let path = path.trim().trim_matches('"');
    let path = std::path::absolute(if path.is_empty() { "." } else { path }).ok()?;
    [path.clone(), path.join("extracted")].into_iter().find(|p| {
        ["client_Release.exe", "control_Release.exe", "match_Release.exe"]
            .iter()
            .all(|n| p.join(n).is_file())
    })
}

/// The cabinet's folder beside the game folder.
pub fn seat_dir(game: &Path) -> PathBuf {
    game.parent().unwrap_or(game).join("seat1")
}

/// The FPR_Emu helper's folder beside the game folder.
pub fn misc_dir(game: &Path) -> PathBuf {
    game.parent().unwrap_or(game).join("misc")
}
