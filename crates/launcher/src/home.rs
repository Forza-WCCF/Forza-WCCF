//! The home screen's data (ROADMAP D4): one row per edition, saying where its folder is and what state it is in.  No UI.
//!
//! The edition of a folder comes from its name and from the game's `<id>_Table.dat` (as the installer decides it), never
//! from the layout alone: a folder whose game belongs to the other edition is `NotOurs`.

use std::path::{Path, PathBuf};

use crate::{Edition, Kit, choices, run};

const EDITIONS: [Edition; 2] = [Edition::E1011, Edition::E1718];
/// The installer's v0.1.0 game folder, beside a kit in the root (`install.rs` `LEGACY_GAME_DIR`).
const LEGACY_GAME_DIR: &str = "wccf-revival";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    /// the installer's game files, no kit
    GameOnly,
    /// an older kit layout (only `scripts\`)
    OldFiles,
    /// a kit with no game recorded in data\settings.json
    NeedsSetup,
    Ready,
    /// data\running.json's run is on
    Running,
    /// the folder is named for this edition but holds the other one's game
    NotOurs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub edition: Edition,
    pub dir: Option<PathBuf>,
    pub state: State,
}

/// The installer's default root: `%SystemDrive%\Games\Forza-WCCF`.
pub fn default_root() -> PathBuf {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    PathBuf::from(format!("{drive}\\Games\\Forza-WCCF"))
}

pub fn rows(here: &Path) -> Vec<Row> {
    rows_in(here, &[default_root()])
}

/// `rows` with the roots to search given (tests).
pub fn rows_in(here: &Path, roots: &[PathBuf]) -> Vec<Row> {
    let mut roots = roots.to_vec();
    let named = |ed: Edition| here.file_name().is_some_and(|n| n.eq_ignore_ascii_case(ed.folder()));
    if EDITIONS.iter().any(|&e| named(e)) {
        roots.splice(0..0, here.parent().map(Path::to_path_buf));
    }
    EDITIONS
        .map(|ed| {
            let mut found = None;
            for dir in candidates(here, ed, &roots) {
                if let Some(state) = classify(&dir, ed) {
                    found = Some((dir, state));
                    break;
                }
            }
            match found {
                Some((dir, state)) => Row {
                    edition: ed,
                    dir: Some(dir),
                    state,
                },
                None => Row {
                    edition: ed,
                    dir: None,
                    state: State::NotInstalled,
                },
            }
        })
        .to_vec()
}

/// The edition this folder is for when it has no edition name: its game's table, else its layout (an older
/// scripts-only kit is 2010-11's).
fn edition_of_here(here: &Path) -> Option<Edition> {
    game_edition(&game_of(here)).or_else(|| Kit::layout(here)).or_else(|| {
        here.join(Edition::E1718.layout().scripts)
            .is_dir()
            .then_some(Edition::E1011)
    })
}

fn candidates(here: &Path, ed: Edition, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let name = here.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let mine = if EDITIONS.iter().any(|e| e.folder().eq_ignore_ascii_case(name)) {
        EDITIONS.into_iter().find(|e| e.folder().eq_ignore_ascii_case(name))
    } else {
        edition_of_here(here)
    };
    if mine == Some(ed) {
        out.push(here.to_path_buf());
    }
    out.extend(roots.iter().map(|r| r.join(ed.folder())));
    if ed == Edition::E1011 {
        // an install from v0.1.0: the kit in the root, the game in wccf-revival (only while no 2010-2011 folder exists)
        out.extend(roots.iter().filter(|r| !r.join(ed.folder()).exists()).cloned());
    }
    out
}

/// The game folder of a kit folder: the one data\settings.json records, else the installer's `game\`.
fn game_of(dir: &Path) -> PathBuf {
    match choices::setting(&Kit::at(dir).settings_file(), "game") {
        Some(g) if !g.is_empty() => PathBuf::from(g),
        _ if dir.join(LEGACY_GAME_DIR).is_dir() => dir.join(LEGACY_GAME_DIR),
        _ => dir.join("game"),
    }
}

/// The edition whose `<id>_Table.dat` the game folder holds (exactly one).
fn game_edition(game: &Path) -> Option<Edition> {
    let mut ids = EDITIONS
        .into_iter()
        .filter(|e| game.join("local").join(format!("{}_Table.dat", e.game_id())).is_file());
    let ed = ids.next()?;
    ids.next().is_none().then_some(ed)
}

/// The state of `dir` as the folder of `ed`; None when it holds nothing of ours (a missing folder, a dead junction).
fn classify(dir: &Path, ed: Edition) -> Option<State> {
    if !dir.is_dir() {
        return None; // also a junction whose target is gone
    }
    let kit = Kit::at(dir);
    let game = game_of(dir);
    let table = game_edition(&game);
    if table.is_some_and(|t| t != ed) {
        return Some(State::NotOurs);
    }
    if Kit::layout(dir).is_some_and(|l| l != ed) {
        return Some(State::NotOurs);
    }
    if kit.is_kit() {
        let recorded = choices::setting(&kit.settings_file(), "game").is_some_and(|g| !g.is_empty());
        return Some(if !recorded {
            State::NeedsSetup
        } else if run::any_on(&kit) {
            State::Running
        } else {
            State::Ready
        });
    }
    if dir.join(Edition::E1718.layout().scripts).is_dir() {
        return Some(State::OldFiles);
    }
    (table == Some(ed) && game.join("client_Release.exe").is_file()).then_some(State::GameOnly)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn dirs(base: &Path, list: &[&str]) {
        for d in list {
            fs::create_dir_all(base.join(d)).unwrap();
        }
    }

    fn game(dir: &Path, ed: Edition) {
        dirs(dir, &["game/local"]);
        fs::write(dir.join("game/client_Release.exe"), b"").unwrap();
        fs::write(dir.join(format!("game/local/{}_Table.dat", ed.game_id())), b"").unwrap();
    }

    fn record(dir: &Path) {
        dirs(dir, &["data"]);
        fs::write(dir.join("data/settings.json"), r#"{"game": "G:\\game"}"#).unwrap();
    }

    fn state(root: &Path, ed: Edition) -> State {
        let nowhere = root.join("nowhere");
        rows_in(&nowhere, &[root.to_path_buf()])
            .into_iter()
            .find(|r| r.edition == ed)
            .unwrap()
            .state
    }

    #[test]
    fn every_state() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        let ed = Edition::E1011;
        assert_eq!(state(r, ed), State::NotInstalled);
        let d = r.join(ed.folder());
        fs::create_dir_all(&d).unwrap();
        assert_eq!(state(r, ed), State::NotInstalled, "an empty folder");
        game(&d, ed);
        assert_eq!(state(r, ed), State::GameOnly);
        dirs(&d, &["code"]);
        assert_eq!(state(r, ed), State::NeedsSetup);
        record(&d);
        assert_eq!(state(r, ed), State::Ready);
        // Running needs the live run's lock (run.rs); the same kit is Ready until one holds it
        fs::remove_dir_all(d.join("code")).unwrap();
        dirs(&d, &["scripts"]);
        assert_eq!(
            state(r, ed),
            State::OldFiles,
            "an older kit with only scripts, still 2010-11"
        );
        // the other edition's game in this edition's folder
        let o = r.join(Edition::E1718.folder());
        game(&o, ed);
        assert_eq!(state(r, Edition::E1718), State::NotOurs);
        assert_eq!(state(r, ed), State::OldFiles);
    }

    #[test]
    fn both_editions_under_one_root() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        for ed in EDITIONS {
            game(&r.join(ed.folder()), ed);
        }
        dirs(&r.join("2017-2018"), &["scripts", "bin"]);
        let rows = rows_in(&r.join("2017-2018"), &[]);
        assert_eq!(rows[0].state, State::GameOnly);
        assert_eq!(
            rows[0].dir,
            Some(r.join("2010-2011")),
            "found through the parent of an edition-named folder"
        );
        assert_eq!(rows[1].state, State::NeedsSetup);
        assert_eq!(rows[1].dir, Some(r.join("2017-2018")));
    }

    #[test]
    fn the_legacy_install_is_2010_11() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        dirs(r, &["wccf-revival/local"]);
        fs::write(r.join("wccf-revival/local/SBWG_Table.dat"), b"").unwrap();
        fs::write(r.join("wccf-revival/client_Release.exe"), b"").unwrap();
        assert_eq!(state(r, Edition::E1011), State::GameOnly);
        assert_eq!(state(r, Edition::E1718), State::NotInstalled);
    }

    #[test]
    fn a_dead_junction_is_not_installed() {
        let root = tempfile::tempdir().unwrap();
        let (r, target) = (root.path(), root.path().join("gone"));
        fs::create_dir(&target).unwrap();
        junction::create(&target, r.join("2010-2011")).unwrap();
        game(&target, Edition::E1011);
        assert_eq!(state(r, Edition::E1011), State::GameOnly);
        fs::remove_dir_all(&target).unwrap();
        assert_eq!(state(r, Edition::E1011), State::NotInstalled);
    }

    #[test]
    fn this_folder_without_an_edition_name() {
        let root = tempfile::tempdir().unwrap();
        let here = root.path().join("Downloads kit");
        dirs(&here, &["code"]);
        let rows = rows_in(&here, &[]);
        assert_eq!(
            (rows[0].dir.as_deref(), rows[0].state),
            (Some(here.as_path()), State::NeedsSetup)
        );
        assert_eq!(rows[1].state, State::NotInstalled);
    }
}
