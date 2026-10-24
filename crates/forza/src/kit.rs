//! The kit folder (the one Launcher.exe is in) and what is in it: its edition, its programs, data\ (settings,
//! logs, the club card, the cache), and the game folder setup recorded in data\settings.json with the seat and misc
//! folders beside it.  Port of kit_common.py's paths, load_settings, find_game, seat_dir and misc_dir.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Which game a kit runs.  The two kits are laid out differently: 2010-11 has `programs\`, 2017-18 has `bin\` (the
/// hook, logowin, FPR_Emu, forza-server) and `overlay\` (the panel, the catalogue; 2010-11 keeps inject.exe in programs\);
/// [`Kit::edition`] tells them apart.
pub use crate::Edition;

/// The kit folder: the one this exe is in.
pub struct Kit {
    pub(crate) dir: PathBuf,
}

impl Kit {
    /// version.txt from the release ZIP (cargo xtask package writes it): kit-0.3.0.
    pub fn version(&self) -> Option<String> {
        let v = fs::read_to_string(self.dir.join("version.txt")).ok()?;
        Some(v.trim().to_owned()).filter(|v| !v.is_empty())
    }

    pub fn here() -> Self {
        let exe = env::current_exe().unwrap_or_default();
        let dir = exe
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self { dir }
    }

    /// This program's own file name (Launcher.exe, or whatever the release calls it).
    pub fn exe_name() -> std::ffi::OsString {
        env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(ToOwned::to_owned))
            .unwrap_or_else(|| "Launcher.exe".into())
    }

    /// The edition whose current kit layout `dir` holds ([`Edition::of_kit`]).  An older kit with only `scripts\` has
    /// neither.
    pub fn layout(dir: &Path) -> Option<Edition> {
        Edition::of_kit(|name| dir.join(name).is_dir())
    }

    /// The folder holds a kit of either edition.
    pub fn is_kit(&self) -> bool {
        Self::layout(&self.dir).is_some()
    }

    /// A kit in `dir` (tests, tools).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The kit's edition, from its layout; 2010-11 when it holds neither.
    pub fn edition(&self) -> Edition {
        Self::layout(&self.dir).unwrap_or(Edition::E1011)
    }

    /// The kit's compiled programs: the hook, the panel, card-table.
    pub fn programs(&self) -> PathBuf {
        self.dir.join("programs")
    }

    /// One of the kit's programs (inject.exe, forza_panel.dll, forza-server.exe) where its edition keeps
    /// it.
    pub fn program(&self, name: &str) -> PathBuf {
        let lay = self.edition().layout();
        self.dir.join(lay.program_dir(name)).join(name)
    }

    pub fn data(&self) -> PathBuf {
        self.dir.join("data")
    }

    /// Made by setup from the player's game: catalogue.tsv, cards\ (2017-18: in overlay\).
    pub fn cache(&self) -> PathBuf {
        self.dir.join(self.edition().layout().cache)
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

    /// English is on: the game text's list of the original files it backed up is not empty (kit_common.english_on).
    pub fn english_on(&self) -> bool {
        fs::read_to_string(self.data().join("english_backup").join("manifest.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .is_some_and(|v| {
                v.as_object().is_some_and(|o| !o.is_empty())
                    || v.as_array().is_some_and(|a| !a.is_empty())
            })
    }

    /// The game text was switched: the backup holds a manifest (the file the undo of setup refuses on), valid or not.
    pub fn text_changed(&self) -> bool {
        self.data()
            .join("english_backup")
            .join("manifest.json")
            .is_file()
    }

    /// The language the game's text has in place (kit_common.game_language): "en" or "it" (its language.txt; English before
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
}

/// The game's three programs, as find_game looks for them.
pub const GAME_PROGRAMS: [&str; 3] = [
    "client_Release.exe",
    "control_Release.exe",
    "match_Release.exe",
];

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
        fs::create_dir(dir.path().join("overlay")).unwrap();
        assert_eq!(kit.edition(), Edition::E1718);
        fs::create_dir(dir.path().join("programs")).unwrap();
        assert_eq!(
            kit.edition(),
            Edition::E1011,
            "an old 2010-11 kit's overlay\\ left beside programs\\"
        );
    }

    #[test]
    fn is_kit_needs_a_current_layout() {
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::at(dir.path());
        assert!(!kit.is_kit());
        fs::create_dir(dir.path().join("scripts")).unwrap();
        assert!(!kit.is_kit(), "an older kit with only scripts\\");
        fs::create_dir(dir.path().join("overlay")).unwrap();
        assert!(kit.is_kit());
        assert_eq!(Kit::layout(dir.path()), Some(Edition::E1718));
        let other = tempfile::tempdir().unwrap();
        fs::create_dir(other.path().join("programs")).unwrap();
        assert_eq!(Kit::layout(other.path()), Some(Edition::E1011));
    }

    #[test]
    fn text_changed_follows_the_manifest_file_even_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::at(dir.path());
        assert!(!kit.text_changed());
        let backup = dir.path().join("data").join("english_backup");
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("manifest.json"), "").unwrap();
        assert!(
            kit.text_changed(),
            "the undo of setup refuses on the file, valid or not"
        );
        assert!(!kit.english_on());
    }
}
