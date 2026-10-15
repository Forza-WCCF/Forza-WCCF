//! The kit folder (the one Launcher.exe is in) and what is in it: its edition, its Python, its scripts, data\ (settings,
//! logs, the club card, the cache), and the game folder setup recorded in data\settings.json with the seat and misc
//! folders beside it.  Port of kit_common.py's paths, load_settings, find_game, seat_dir and misc_dir.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The kit's Python scripts folder (`cargo xtask check` checks the staged kit has the scripts Launcher.exe runs in it).
pub const SCRIPTS: &str = "code";
/// The 2017-18 kit's scripts folder (setup.py, english.py, cards.py; its run is Launcher.exe's).
pub const SCRIPTS_1718: &str = "scripts";

/// Which game a kit runs.  The two kits are laid out differently: 2010-11 has `code\` and `programs\`, 2017-18 has
/// `scripts\`, `bin\` (the hook, logowin, FPR_Emu, forza-server) and `overlay\` (inject.exe, the panel, netguard.dll,
/// the catalogue).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edition {
    E1011,
    E1718,
}

impl Edition {
    /// "2010-11" or "2017-18": the run's words and `forza-server --edition`.
    pub fn name(self) -> &'static str {
        match self {
            Self::E1011 => "2010-11",
            Self::E1718 => "2017-18",
        }
    }

    /// The game's id: the keychip's answer and the shared memory's name prefix.
    pub fn game_id(self) -> &'static str {
        match self {
            Self::E1011 => "SBWG",
            Self::E1718 => "SDDY",
        }
    }
}

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

    /// The kit's edition, from its layout: a `scripts\` folder without `code\` is the 2017-18 kit's (an older 2010-11
    /// kit had scripts\ too, and an update over it may leave one beside code\).
    pub fn edition(&self) -> Edition {
        if self.dir.join(SCRIPTS_1718).is_dir() && !self.dir.join(SCRIPTS).is_dir() {
            Edition::E1718
        } else {
            Edition::E1011
        }
    }

    pub fn python(&self) -> PathBuf {
        self.dir.join("python").join("python.exe")
    }

    /// The kit's compiled programs: the hook, the panel, card-table.
    pub fn programs(&self) -> PathBuf {
        self.dir.join("programs")
    }

    /// One of the kit's programs (inject.exe, forza_panel.dll, forza-server.exe, netguard.dll) where its edition keeps
    /// it.
    pub fn program(&self, name: &str) -> PathBuf {
        match self.edition() {
            Edition::E1011 => self.programs().join(name),
            Edition::E1718 if matches!(name, "inject.exe" | "forza_panel.dll" | "netguard.dll") => {
                self.dir.join("overlay").join(name)
            }
            Edition::E1718 => self.dir.join("bin").join(name),
        }
    }

    pub fn data(&self) -> PathBuf {
        self.dir.join("data")
    }

    /// Made by setup from the player's game: catalogue.tsv, cards\ (2017-18: in overlay\).
    pub fn cache(&self) -> PathBuf {
        match self.edition() {
            Edition::E1011 => self.data().join("cache"),
            Edition::E1718 => self.dir.join("overlay"),
        }
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

    /// English is on: english.py's list of the original files it backed up is not empty (kit_common.english_on).
    pub fn english_on(&self) -> bool {
        fs::read_to_string(self.data().join("english_backup").join("manifest.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .is_some_and(|v| {
                v.as_object().is_some_and(|o| !o.is_empty()) || v.as_array().is_some_and(|a| !a.is_empty())
            })
    }

    /// The language english.py has in place (kit_common.game_language): "en" or "it" (its language.txt; English before
    /// that file existed), "" for the original Japanese.
    pub fn game_language(&self) -> &'static str {
        if !self.english_on() {
            return "";
        }
        let file = self.data().join("english_backup").join("language.txt");
        if fs::read_to_string(file).is_ok_and(|t| t.trim() == "it") {
            "it"
        } else {
            "en"
        }
    }

    /// `python\python.exe code\<script>` (2017-18: `scripts\<script>`), to add arguments to
    pub fn script(&self, script: &str) -> Command {
        let dir = match self.edition() {
            Edition::E1011 => SCRIPTS,
            Edition::E1718 => SCRIPTS_1718,
        };
        let mut cmd = Command::new(self.python());
        cmd.arg(self.dir.join(dir).join(script));
        cmd
    }
}

/// The game's three programs, as find_game looks for them.
pub const GAME_PROGRAMS: [&str; 3] = ["client_Release.exe", "control_Release.exe", "match_Release.exe"];

/// The game folder (the one with the three game programs) from a path to it or to the folder above it (its
/// `extracted\`); spaces and quotes around the path do not count.
pub fn find_game(path: &str) -> Option<PathBuf> {
    find_game_named(path, GAME_PROGRAMS)
}

/// find_game with the programs' names given (a run's tests use stand-ins under names of their own).
pub fn find_game_named<S: AsRef<str>>(path: &str, programs: [S; 3]) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let path = path.trim().trim_matches('"');
    let path = std::path::absolute(if path.is_empty() { "." } else { path }).ok()?;
    [path.clone(), path.join("extracted")]
        .into_iter()
        .find(|p| programs.iter().all(|n| p.join(n.as_ref()).is_file()))
}

/// The cabinet's folder beside the game folder.
pub fn seat_dir(game: &Path) -> PathBuf {
    game.parent().unwrap_or(game).join("seat1")
}

/// The FPR_Emu helper's folder beside the game folder.
pub fn misc_dir(game: &Path) -> PathBuf {
    game.parent().unwrap_or(game).join("misc")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edition_comes_from_the_layout() {
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::at(dir.path());
        assert_eq!(kit.edition(), Edition::E1011);
        fs::create_dir(dir.path().join(SCRIPTS_1718)).unwrap();
        assert_eq!(kit.edition(), Edition::E1718);
        fs::create_dir(dir.path().join(SCRIPTS)).unwrap();
        assert_eq!(
            kit.edition(),
            Edition::E1011,
            "an old 2010-11 kit's scripts\\ left beside code\\"
        );
    }
}
