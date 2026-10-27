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

/// 2017-18's folder for everything but Launcher.exe, the READMEs, game\ and data\ (see [`crate::migrate`]).
pub const APP: &str = "app";

/// `top\app` when it is there, else `top` itself: 2010-11 kits, 2017-18 installs from before app\ and the
/// developers' runtimes keep everything at the top.
pub fn app_dir(top: &Path) -> PathBuf {
    let app = top.join(APP);
    if app.is_dir() { app } else { top.to_path_buf() }
}

/// The kit folder: the one this exe is in.
pub struct Kit {
    pub(crate) dir: PathBuf,
}

impl Kit {
    /// version.txt from the release ZIP (cargo xtask package writes it): kit-0.3.0.
    pub fn version(&self) -> Option<String> {
        let v = fs::read_to_string(self.app().join("version.txt")).ok()?;
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

    /// The edition whose current kit layout `dir` holds ([`Edition::of_kit`]), in `dir\app` or at the top.  An older
    /// kit with only `scripts\` has neither.
    pub fn layout(dir: &Path) -> Option<Edition> {
        let app = app_dir(dir);
        Edition::of_kit(|name| app.join(name).is_dir())
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

    /// Where the kit's own files are ([`app_dir`]): 2017-18's `app\`, else the kit folder.
    pub fn app(&self) -> PathBuf {
        app_dir(&self.dir)
    }

    /// The folder [`crate::text`] reads the game text's sheets from: 2010-11 the kit folder (it has `languages\`),
    /// 2017-18 the folder with `english\` and `italian\` (`app\languages`, or the top in an older install).
    pub fn sheets(&self) -> PathBuf {
        let app = self.app();
        match self.edition() {
            Edition::E1011 => self.dir.clone(),
            Edition::E1718 if app.join("languages").is_dir() => app.join("languages"),
            Edition::E1718 => app,
        }
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
        self.app().join(lay.program_dir(name)).join(name)
    }

    pub fn data(&self) -> PathBuf {
        self.dir.join("data")
    }

    /// Made by setup from the player's game: catalogue.tsv, cards\ (2017-18: in overlay\).
    pub fn cache(&self) -> PathBuf {
        self.app().join(self.edition().layout().cache)
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

/// The folder setup keeps seat1\, original\ and setup-hashes.json in: the runtime above the game folder, its `app\`
/// when it has one ([`app_dir`]).
pub fn runtime_dir(game: &Path) -> PathBuf {
    app_dir(game.parent().unwrap_or(game))
}

/// The cabinet's folder in the runtime ([`runtime_dir`]).
pub fn seat_dir(game: &Path) -> PathBuf {
    runtime_dir(game).join("seat1")
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

    /// An install from before app\ (bin\ at the top) and one after it (app\bin\) are both 2017-18; programs,
    /// version, sheets, cache and seat follow app\ when it is there; data\ stays at the top.
    #[test]
    fn both_2017_18_layouts_are_found() {
        let old = tempfile::tempdir().unwrap();
        fs::create_dir_all(old.path().join("bin")).unwrap();
        fs::create_dir_all(old.path().join("english")).unwrap();
        fs::write(old.path().join("version.txt"), "kit-0.4.1\r\n").unwrap();
        let k = Kit::at(old.path());
        assert_eq!(Kit::layout(old.path()), Some(Edition::E1718));
        assert_eq!(
            k.program("wccfnet.dll"),
            old.path().join("bin").join("wccfnet.dll")
        );
        assert_eq!(k.version().as_deref(), Some("kit-0.4.1"));
        assert_eq!(k.sheets(), old.path());
        assert_eq!(seat_dir(&old.path().join("game")), old.path().join("seat1"));

        let new = tempfile::tempdir().unwrap();
        let app = new.path().join(APP);
        fs::create_dir_all(app.join("bin")).unwrap();
        fs::create_dir_all(app.join("languages").join("english")).unwrap();
        fs::write(app.join("version.txt"), "kit-0.5.0\r\n").unwrap();
        let k = Kit::at(new.path());
        assert_eq!(Kit::layout(new.path()), Some(Edition::E1718));
        assert!(k.is_kit());
        assert_eq!(
            k.program("wccfnet.dll"),
            app.join("bin").join("wccfnet.dll")
        );
        assert_eq!(
            k.program("forza_panel.dll"),
            app.join("overlay").join("forza_panel.dll")
        );
        assert_eq!(k.cache(), app.join("overlay"));
        assert_eq!(k.version().as_deref(), Some("kit-0.5.0"));
        assert_eq!(k.sheets(), app.join("languages"));
        assert_eq!(k.data(), new.path().join("data"));
        assert_eq!(seat_dir(&new.path().join("game")), app.join("seat1"));
        assert_eq!(runtime_dir(&new.path().join("game")), app);
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
