//! The kit folder (the one Launcher.exe is in) and what is in it: its edition, its programs, data\ (settings,
//! logs, the club card, the cache), and the game folder setup recorded in data\settings.json with the seat and misc
//! folders beside it.  Port of kit_common.py's paths, load_settings, find_game, seat_dir and misc_dir.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Which game a kit runs.  The two kits are laid out differently: 2010-11 has `programs\`, 2017-18 has `bin\` (the
/// hook, FPR_Emu, forza-server) and `overlay\` (the panel, the catalogue; 2010-11 keeps inject.exe in programs\);
/// [`Kit::edition`] tells them apart.
pub use crate::Edition;

pub use crate::profile::Profile;

/// The kit folder: the one this exe is in, and the profile active when it was looked at (settings.json "profile").
pub struct Kit {
    pub(crate) dir: PathBuf,
    profile: Profile,
}

impl Kit {
    /// version.txt from the release ZIP (cargo xtask package writes it): kit-0.3.0.
    pub fn version(&self) -> Option<String> {
        let v = fs::read_to_string(self.dir.join("version.txt")).ok()?;
        Some(v.trim().to_owned()).filter(|v| !v.is_empty())
    }

    /// The release as the Launcher and the panel show it: version.txt's first line without `kit-` ("0.3.0").
    pub fn release(&self) -> Option<String> {
        let v = self.version()?;
        let v = v
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .trim_start_matches("kit-");
        Some(v.to_owned()).filter(|v| !v.is_empty())
    }

    pub fn here() -> Self {
        let exe = env::current_exe().unwrap_or_default();
        let dir = exe
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self::at(dir)
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
        let dir = dir.into();
        let profile = Profile::of(&read_settings(&dir.join("data").join("settings.json")));
        Self { dir, profile }
    }

    /// The active profile: free play or Create-a-Team.
    pub fn profile(&self) -> Profile {
        self.profile
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

    /// Forza-WCCF's own programs that are missing from this folder, by name. An antivirus quarantines them (Microsoft
    /// flagged FPR_Emu.exe from 0.6.0 on) and the game then runs without what they do: no card table, no panel, and
    /// nothing says why. The Launcher names them and refuses the start.
    pub fn missing_programs(&self) -> Vec<&'static str> {
        const NEEDED: [&str; 2] = ["FPR_Emu.exe", "forza_panel.dll"];
        NEEDED
            .into_iter()
            .filter(|name| !self.program(name).is_file())
            .collect()
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

    /// The active profile's save: `data\save` for free play, `data\profiles\create-a-team\save` for Create-a-Team.
    pub fn save(&self) -> PathBuf {
        self.profile.save(&self.data())
    }

    pub fn settings_file(&self) -> PathBuf {
        self.data().join("settings.json")
    }

    /// data\settings.json; an empty object when it is missing or not JSON.
    pub fn settings(&self) -> serde_json::Value {
        read_settings(&self.settings_file())
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

/// settings.json "projector" (2017-18, default on): the projector's window shown. Off hides it, at once (the panel in
/// the projector reads it each second); with the CPU championship off too, a run on this PC starts no projector.
pub const PROJECTOR: &str = "projector";
/// settings.json "cpu_championship" (2017-18, default off): the control's all-CPU Universal Championship plays on the
/// projector. Off, the control keeps it off the projector (WCCF_UCLIVE=on). Read at the start of a run on this PC.
/// Missing means off, the old "projector" switch included: its off was off, and its on was the old default.
pub const CPU_CHAMPIONSHIP: &str = "cpu_championship";
/// The start of the projector window's title (the panel writes it): a hidden window with it is still the projector's.
pub const PROJECTOR_TITLE: &str = "Projector - Forza-WCCF";

pub fn projector_shown(settings: &serde_json::Value) -> bool {
    settings[PROJECTOR].as_bool() != Some(false)
}

pub fn cpu_championship_on(settings: &serde_json::Value) -> bool {
    settings[CPU_CHAMPIONSHIP].as_bool() == Some(true)
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

/// The active profile's save for a kit's `data` folder (callers that know only that folder: the panel, the card steps).
pub fn save_in(data: &Path) -> PathBuf {
    Profile::of(&read_settings(&data.join("settings.json"))).save(data)
}

/// A settings.json file; an empty object when it is missing or not JSON.
pub fn read_settings(file: &Path) -> serde_json::Value {
    fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// settings.json "frame_cap" (2017-18, the Launcher's FRAME CAP; on unless false): the panel's frame wait sleeps
/// instead of spinning a core.
pub fn frame_cap(settings: &serde_json::Value) -> bool {
    settings["frame_cap"].as_bool() != Some(false)
}

/// settings.json "vsync" (2017-18, the Launcher's VSYNC; off unless true): the panel's picture waits for the screen.
pub fn vsync(settings: &serde_json::Value) -> bool {
    settings["vsync"].as_bool() == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cpu_championship_is_off_unless_saved_on_and_apart_from_the_projector() {
        use serde_json::json;
        let both = |v: serde_json::Value| (projector_shown(&v), cpu_championship_on(&v));
        assert_eq!(both(json!({})), (true, false));
        // the old "Projector and CPU championship" switch no longer decides the championship
        assert_eq!(both(json!({"projector": false})), (false, false));
        assert_eq!(both(json!({"projector": true})), (true, false));
        // a saved choice stays, whatever the window
        assert_eq!(
            both(json!({"projector": false, "cpu_championship": true})),
            (false, true)
        );
        assert_eq!(
            both(json!({"projector": true, "cpu_championship": true})),
            (true, true)
        );
        assert_eq!(
            both(json!({"projector": true, "cpu_championship": false})),
            (true, false)
        );
        assert_eq!(
            both(json!({"cpu_championship": "yes"})),
            (true, false),
            "not a bool"
        );
    }

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
    fn missing_programs_follows_the_layout_of_both_editions() {
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::at(dir.path());
        assert_eq!(kit.missing_programs(), ["FPR_Emu.exe", "forza_panel.dll"]);
        fs::create_dir_all(dir.path().join("bin")).unwrap();
        fs::create_dir_all(dir.path().join("overlay")).unwrap();
        fs::write(dir.path().join("bin").join("FPR_Emu.exe"), "").unwrap();
        assert_eq!(kit.missing_programs(), ["forza_panel.dll"]);
        fs::write(dir.path().join("overlay").join("forza_panel.dll"), "").unwrap();
        assert!(kit.missing_programs().is_empty());
        // 2010-11 keeps both in programs\
        let old = tempfile::tempdir().unwrap();
        let kit = Kit::at(old.path());
        fs::create_dir_all(old.path().join("programs")).unwrap();
        fs::write(old.path().join("programs").join("forza_panel.dll"), "").unwrap();
        assert_eq!(kit.missing_programs(), ["FPR_Emu.exe"]);
    }

    #[test]
    fn release_drops_the_kit_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::at(dir.path());
        assert_eq!(kit.release(), None);
        fs::write(dir.path().join("version.txt"), "kit-0.6.0\r\n").unwrap();
        assert_eq!(kit.release().as_deref(), Some("0.6.0"));
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

    /// (a) Free play's save is data\save, as before profiles; Create-a-Team's is its own folder.
    #[test]
    fn the_save_follows_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        assert_eq!(Kit::at(dir.path()).profile(), Profile::Free);
        assert_eq!(Kit::at(dir.path()).save(), data.join("save"));
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("settings.json"), r#"{"profile": "free"}"#).unwrap();
        assert_eq!(Kit::at(dir.path()).save(), data.join("save"));
        assert_eq!(save_in(&data), data.join("save"));
        fs::write(
            data.join("settings.json"),
            r#"{"profile": "create-a-team"}"#,
        )
        .unwrap();
        let kit = Kit::at(dir.path());
        assert_eq!(kit.profile(), Profile::CreateATeam);
        assert_eq!(kit.save(), data.join(r"profiles\create-a-team\save"));
        assert_eq!(save_in(&data), kit.save());
    }
}
