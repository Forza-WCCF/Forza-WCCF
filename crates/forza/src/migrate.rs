//! A 2017-18 install from before app\: its programs, text, seat and setup record lay at the top beside Launcher.exe.
//! An update (or the installer) unpacks the new kit's `app\` beside them; then [`run`] moves the old top items into
//! `app\` by [`MOVES`]. Where `app\` already holds an item (the newer copy just unpacked), the old copy is removed;
//! the rest moves (a rename: seat 1's junctions and hard links to game\ stay as they are).
//!
//! Only the top folder given is touched, never `game\` or `data\` and never the other edition's folder. Nothing
//! happens until the new kit is complete (every file its `app\files.txt` lists is there). A second run, or one after
//! an interrupted run, finishes what is left.

use std::fs;
use std::io;
use std::path::Path;

use crate::kit::APP;

/// Old top item -> where it goes, both under the install's top folder.
pub const MOVES: &[(&str, &str)] = &[
    ("bin", "app/bin"),
    ("overlay", "app/overlay"),
    ("fonts", "app/fonts"),
    ("source", "app/source"),
    ("english", "app/languages/english"),
    ("italian", "app/languages/italian"),
    ("original", "app/original"),
    ("seat1", "app/seat1"),
    ("files.txt", "app/files.txt"),
    ("version.txt", "app/version.txt"),
    ("setup-hashes.json", "app/setup-hashes.json"),
];

/// Whether `top` still holds an item [`MOVES`] would move.
pub fn pending(top: &Path) -> bool {
    MOVES
        .iter()
        .any(|(from, _)| top.join(from).symlink_metadata().is_ok())
}

/// The new kit's `app\files.txt` is there and every file it lists (paths under `top`) is too.
fn new_kit_complete(top: &Path) -> bool {
    let Ok(text) = fs::read_to_string(top.join(APP).join("files.txt")) else {
        return false;
    };
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .all(|l| top.join(l.replace('\\', "/")).is_file())
}

/// Moves what is left of the old layout in `top` into `app\`; the number of items moved or removed (0: nothing to
/// do, or the new kit is not complete yet).
pub fn run(top: &Path) -> io::Result<usize> {
    if !pending(top) || !new_kit_complete(top) {
        return Ok(0);
    }
    let mut n = 0;
    for (from, to) in MOVES {
        let src = top.join(from);
        if src.symlink_metadata().is_ok() {
            merge(&src, &top.join(to), &mut n)?;
        }
    }
    Ok(n)
}

/// `src` into `dst`: renamed when `dst` is not there, a real folder merged item by item, else `src` removed (`dst`
/// is the newer copy).
fn merge(src: &Path, dst: &Path, n: &mut usize) -> io::Result<()> {
    let meta = src.symlink_metadata()?;
    // a junction or link counts as an item, never a folder to walk: seat 1's point into game\
    let real_dir = meta.is_dir() && !meta.file_type().is_symlink();
    if dst.symlink_metadata().is_err() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(src, dst)?;
        *n += 1;
    } else if real_dir && dst.is_dir() && !dst.symlink_metadata()?.file_type().is_symlink() {
        for entry in fs::read_dir(src)? {
            let name = entry?.file_name();
            merge(&src.join(&name), &dst.join(&name), n)?;
        }
        fs::remove_dir(src)?;
    } else {
        if meta.is_dir() || link_dir(&meta) {
            fs::remove_dir(src)?;
        } else {
            fs::remove_file(src)?;
        }
        *n += 1;
    }
    Ok(())
}

/// A junction or folder link (reads as neither file nor folder): it goes with remove_dir, alone, its target stays.
#[cfg(windows)]
fn link_dir(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::FileTypeExt;
    meta.file_type().is_symlink_dir()
}

#[cfg(not(windows))]
fn link_dir(_: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn put(p: PathBuf, text: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn read(p: PathBuf) -> String {
        fs::read_to_string(p).unwrap()
    }

    /// An install from before app\ (0.4.1's top) with the new kit unpacked over it, as an update leaves it.
    fn updated_old_install(top: &Path) {
        for f in [
            "bin/wccfnet.dll",
            "overlay/forza_panel.dll",
            "fonts/a.ttf",
            "english/screen_text.tsv",
            "italian/screen_text.tsv",
            "source/README.txt",
            "files.txt",
            "version.txt",
        ] {
            put(top.join(f), "old");
        }
        put(top.join("overlay/catalogue.tsv"), "made by setup");
        put(top.join("overlay/cards/1.png"), "made by setup");
        put(top.join("original/client_Release.exe"), "original");
        put(top.join("seat1/client_Release.exe"), "guarded seat");
        put(top.join("setup-hashes.json"), "{}");
        put(top.join("game/client_Release.exe"), "game");
        put(top.join("data/settings.json"), "{}");
        let shipped = [
            "Launcher.exe",
            "README.txt",
            "app/bin/wccfnet.dll",
            "app/overlay/forza_panel.dll",
            "app/fonts/a.ttf",
            "app/languages/english/screen_text.tsv",
            "app/languages/italian/screen_text.tsv",
            "app/source/README.txt",
            "app/version.txt",
            "app/files.txt",
        ];
        for f in shipped {
            put(top.join(f), "new");
        }
        put(
            top.join("app/files.txt"),
            &shipped.map(|f| f.replace('/', "\\")).join("\r\n"),
        );
    }

    fn assert_new_layout(top: &Path) {
        assert!(
            !pending(top),
            "nothing of the old layout is left at the top"
        );
        let mut names: Vec<String> = fs::read_dir(top)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into())
            .collect();
        names.sort();
        assert_eq!(names, ["Launcher.exe", "README.txt", "app", "data", "game"]);
        assert_eq!(
            read(top.join("app/bin/wccfnet.dll")),
            "new",
            "the unpacked copy wins over the old one"
        );
        assert_eq!(
            read(top.join("app/languages/english/screen_text.tsv")),
            "new"
        );
        assert_eq!(read(top.join("app/version.txt")), "new");
        assert_eq!(read(top.join("app/overlay/catalogue.tsv")), "made by setup");
        assert_eq!(read(top.join("app/overlay/cards/1.png")), "made by setup");
        assert_eq!(
            read(top.join("app/original/client_Release.exe")),
            "original"
        );
        assert_eq!(
            read(top.join("app/seat1/client_Release.exe")),
            "guarded seat"
        );
        assert_eq!(read(top.join("app/setup-hashes.json")), "{}");
        assert_eq!(
            read(top.join("game/client_Release.exe")),
            "game",
            "game\\ untouched"
        );
        assert_eq!(
            read(top.join("data/settings.json")),
            "{}",
            "data\\ untouched"
        );
    }

    #[test]
    fn an_updated_old_install_moves_into_app() {
        let t = tempfile::tempdir().unwrap();
        updated_old_install(t.path());
        assert!(run(t.path()).unwrap() > 0);
        assert_new_layout(t.path());
        assert_eq!(run(t.path()).unwrap(), 0, "a second run changes nothing");
        assert_new_layout(t.path());
    }

    /// Interrupted half way: seat1\ already moved, part of overlay\ too, bin\ and the record still at the top.
    #[test]
    fn a_half_moved_install_is_finished() {
        let t = tempfile::tempdir().unwrap();
        let top = t.path();
        updated_old_install(top);
        fs::rename(top.join("seat1"), top.join("app/seat1")).unwrap();
        fs::create_dir_all(top.join("app/overlay/cards")).unwrap();
        fs::rename(
            top.join("overlay/cards/1.png"),
            top.join("app/overlay/cards/1.png"),
        )
        .unwrap();
        assert!(run(top).unwrap() > 0);
        assert_new_layout(top);
    }

    /// Seat 1's junction into game\ moves as a link, and an old one beside a moved one goes alone: game\ stays whole.
    #[cfg(all(windows, feature = "full"))]
    #[test]
    fn seat_junctions_move_as_links() {
        let t = tempfile::tempdir().unwrap();
        let top = t.path();
        updated_old_install(top);
        put(top.join("game/data/x.bin"), "game data");
        crate::setup::make_junction(&top.join("seat1/data"), &top.join("game/data")).unwrap();
        fs::create_dir_all(top.join("app/seat1")).unwrap();
        crate::setup::make_junction(&top.join("app/seat1/data"), &top.join("game/data")).unwrap();
        crate::setup::make_junction(&top.join("seat1/AI"), &top.join("game/data")).unwrap();
        run(top).unwrap();
        assert_new_layout(top);
        assert_eq!(read(top.join("game/data/x.bin")), "game data");
        assert_eq!(read(top.join("app/seat1/AI/x.bin")), "game data");
        assert!(
            top.join("app/seat1/AI")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn a_new_install_and_an_incomplete_kit_are_left_alone() {
        let t = tempfile::tempdir().unwrap();
        let top = t.path();
        put(
            top.join("app/files.txt"),
            "app\\files.txt\r\napp\\bin\\wccfnet.dll",
        );
        put(top.join("app/bin/wccfnet.dll"), "new");
        assert_eq!(run(top).unwrap(), 0, "already the new layout");

        // the old layout with the new kit's list but a listed file missing: the unpack did not finish
        put(top.join("bin/wccfnet.dll"), "old");
        fs::remove_file(top.join("app/bin/wccfnet.dll")).unwrap();
        assert_eq!(run(top).unwrap(), 0);
        assert_eq!(
            read(top.join("bin/wccfnet.dll")),
            "old",
            "no old copy goes before the new one is there"
        );

        // an old install no update has reached (no app\): nothing moves
        let old = tempfile::tempdir().unwrap();
        put(old.path().join("bin/wccfnet.dll"), "old");
        assert_eq!(run(old.path()).unwrap(), 0);
        assert!(old.path().join("bin/wccfnet.dll").is_file());
    }
}
