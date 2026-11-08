//! 2017-18: a plain-ASCII name for the folder a game program starts in.  The original programs take paths through the
//! system's legacy code page, so a folder whose path has letters outside it (Chinese or Japanese names on a Western
//! Windows, or a kanji whose second byte is `\` on a Japanese one) stops them.  The run starts each one through a
//! directory junction `%ProgramData%\Forza-WCCF\run\<id>` to its real folder instead: ProgramData is ASCII on every
//! Windows and writable without administrator rights (the user's own folders hold the user name).  `<id>` comes from
//! the real path, so every folder of every installation has its own.  The junction exists only for the start: setup,
//! the guard check, settings.json and the Launcher's own reads and writes keep the real path.  An ASCII path starts as
//! it is.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

use sha2::{Digest, Sha256};

/// Where the junctions go: `%ProgramData%\Forza-WCCF\run`.
pub fn base() -> PathBuf {
    env::var_os("ProgramData")
        .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from)
        .join("Forza-WCCF")
        .join("run")
}

fn is_ascii(p: &Path) -> bool {
    p.to_str().is_some_and(str::is_ascii)
}

/// A path as text without the `\\?\` / `\??\` prefix, a trailing separator or case.
fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.trim_start_matches("\\??\\").trim_start_matches("\\\\?\\");
    s.trim_end_matches('\\').to_lowercase()
}

/// The junction's name for `dir`: 12 hex digits of the SHA-256 of its path (case folded).
pub fn id(dir: &Path) -> String {
    Sha256::digest(norm(dir).as_bytes())[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The folder a game program in `dir` starts from: None when `dir`'s path is plain ASCII (it starts there, as it
/// always did), else the junction in `base` that points at `dir`, made or pointed again now.  Err: no junction could
/// be made (ProgramData not writable, a network folder, ...); the program then starts from `dir`.
pub fn for_dir(dir: &Path, base: &Path) -> io::Result<Option<PathBuf>> {
    if is_ascii(dir) {
        return Ok(None);
    }
    let link = base.join(id(dir));
    if !is_ascii(&link) {
        return Err(io::Error::other(format!(
            "{} is not a plain ASCII path",
            link.display()
        )));
    }
    match junction::get_target(&link) {
        Ok(to) if norm(&to) == norm(dir) && link.is_dir() => return Ok(Some(link)),
        // another folder's, or broken: removes the link only, never what it points at (RemoveDirectoryW on the link;
        // a real folder there goes only when it is empty)
        _ if fs::symlink_metadata(&link).is_ok() => fs::remove_dir(&link)?,
        _ => fs::create_dir_all(base)?,
    }
    junction::create(dir, &link)?;
    if !link.is_dir() {
        let _ = fs::remove_dir(&link);
        return Err(io::Error::other(format!(
            "{} does not lead to {}",
            link.display(),
            dir.display()
        )));
    }
    Ok(Some(link))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{GAME_PROGRAMS, find_game_named, seat_dir};

    /// A game folder at `game` as setup leaves it, and its seat 1: the three programs and a settings file in each.
    fn game_at(game: &Path) -> (PathBuf, PathBuf) {
        let seat = seat_dir(game);
        for dir in [game, seat.as_path()] {
            fs::create_dir_all(dir.join("local")).unwrap();
            for exe in GAME_PROGRAMS {
                fs::write(dir.join(exe), format!("{exe} in {}", dir.display())).unwrap();
            }
            fs::write(dir.join(r"local\ctrl_user_option.conf"), b"GAME_SVR_PORT=20050\r\n").unwrap();
        }
        (
            find_game_named(&game.display().to_string(), GAME_PROGRAMS).unwrap(),
            seat,
        )
    }

    /// What a start gives a program in `dir` (run::Run::start_game, debug::run): its exe, and its working folder is the
    /// exe's.
    fn plan(dir: &Path, base: &Path) -> (PathBuf, PathBuf) {
        let from = for_dir(dir, base).unwrap().unwrap_or_else(|| dir.to_path_buf());
        let exe = from.join(GAME_PROGRAMS[1]);
        let cwd = exe.parent().unwrap().to_path_buf();
        (exe, cwd)
    }

    /// The installer's layout, the release ZIP unzipped into the original game folder (`game` = that folder, seat 1
    /// beside it) and a `game` setting on a folder outside the kit: each with a plain and a Chinese/Japanese path.
    fn layouts(top: &Path, name: &str) -> Vec<PathBuf> {
        vec![
            top.join(name).join(r"Forza-WCCF\2017-2018\game"),
            top.join(name).join("SDDY"),
            top.join(r"elsewhere").join(name).join(r"WCCF 2017-18\game"),
        ]
    }

    #[test]
    fn a_non_ascii_folder_starts_through_an_ascii_junction_to_the_real_files() {
        let top = tempfile::tempdir().unwrap();
        let base = top.path().join(r"ProgramData\Forza-WCCF\run");
        for real in layouts(top.path(), "テスト游戏") {
            let (game, seat) = game_at(&real);
            for dir in [&game, &seat] {
                let (exe, cwd) = plan(dir, &base);
                assert!(is_ascii(&exe) && is_ascii(&cwd), "{exe:?} {cwd:?}");
                assert!(
                    exe.starts_with(&base) && cwd.parent() == Some(base.as_path()),
                    "{exe:?}"
                );
                assert_eq!(fs::read(&exe).unwrap(), fs::read(dir.join(GAME_PROGRAMS[1])).unwrap());
                // the game reads its settings relative to its working folder; the Launcher writes them by the real path
                fs::write(dir.join(r"local\ctrl_user_option.conf"), b"WAIT_PROJECTOR=1\r\n").unwrap();
                assert_eq!(
                    fs::read(cwd.join(r"local\ctrl_user_option.conf")).unwrap(),
                    b"WAIT_PROJECTOR=1\r\n"
                );
                assert_eq!(plan(dir, &base), (exe, cwd), "the next start finds the same junction");
            }
            assert_ne!(id(&game), id(&seat));
        }
    }

    #[test]
    fn an_ascii_folder_starts_where_it_is_and_makes_nothing() {
        let top = tempfile::tempdir().unwrap();
        let base = top.path().join(r"ProgramData\Forza-WCCF\run");
        for real in layouts(top.path(), "Games") {
            let (game, seat) = game_at(&real);
            for dir in [&game, &seat] {
                assert_eq!(for_dir(dir, &base).unwrap(), None);
                assert_eq!(plan(dir, &base), (dir.join(GAME_PROGRAMS[1]), dir.clone()));
            }
        }
        assert!(!base.exists(), "no junction folder for plain paths");
    }

    #[test]
    fn a_stale_or_broken_junction_is_replaced_and_its_target_keeps_its_files() {
        let top = tempfile::tempdir().unwrap();
        let base = top.path().join("run");
        let (real, other) = (top.path().join("游戏").join("game"), top.path().join("other"));
        for d in [&real, &other] {
            fs::create_dir_all(d.join("save")).unwrap();
            fs::write(d.join("save").join("club"), d.display().to_string()).unwrap();
        }
        let link = base.join(id(&real));
        fs::create_dir_all(&base).unwrap();
        junction::create(&other, &link).unwrap(); // another folder's junction under this one's name
        assert_eq!(for_dir(&real, &base).unwrap(), Some(link.clone()));
        assert_eq!(norm(&junction::get_target(&link).unwrap()), norm(&real));
        for d in [&real, &other] {
            assert_eq!(
                fs::read_to_string(d.join(r"save\club")).unwrap(),
                d.display().to_string()
            );
        }
        // broken: its target is gone
        let gone = top.path().join("gone");
        fs::create_dir(&gone).unwrap();
        fs::remove_dir(&link).unwrap();
        junction::create(&gone, &link).unwrap();
        fs::remove_dir(&gone).unwrap();
        assert_eq!(for_dir(&real, &base).unwrap(), Some(link.clone()));
        assert!(link.join(r"save\club").is_file());
        // an empty real folder under the name goes; one with files is never emptied
        fs::remove_dir(&link).unwrap();
        fs::create_dir(&link).unwrap();
        assert_eq!(for_dir(&real, &base).unwrap(), Some(link.clone()));
        fs::remove_dir(&link).unwrap();
        fs::create_dir(&link).unwrap();
        fs::write(link.join("keep.txt"), b"x").unwrap();
        assert!(for_dir(&real, &base).is_err());
        assert!(link.join("keep.txt").is_file());
        assert!(real.join(r"save\club").is_file());
    }

    #[test]
    fn no_junction_is_an_error_and_the_start_text_is_in_every_language() {
        let top = tempfile::tempdir().unwrap();
        let real = top.path().join("游戏");
        fs::create_dir(&real).unwrap();
        let file = top.path().join("ProgramData");
        fs::write(&file, b"a file where the folder should be").unwrap();
        assert!(for_dir(&real, &file.join("run")).is_err());
        let line = crate::run::Text::key("launcher.run_path_not_ascii");
        let all = [forza_ui::Lang::En, forza_ui::Lang::It, forza_ui::Lang::Ja].map(|l| line.get(l));
        assert!(
            all.iter().all(|t| !t.is_empty() && !t.contains("run_path_not_ascii")),
            "{all:?}"
        );
        assert!(all[0] != all[1] && all[1] != all[2], "{all:?}");
    }

    /// The guard's hash check reads the same bytes through the junction as by the real path: a changed file is
    /// refused both ways, so starting through the junction hides nothing.
    #[test]
    fn a_changed_file_is_refused_through_the_junction_too() {
        let top = tempfile::tempdir().unwrap();
        let real = top.path().join("游戏").join("seat1");
        fs::create_dir_all(&real).unwrap();
        let exe = real.join("client_Release.exe");
        fs::write(&exe, b"the guarded copy").unwrap();
        let sum: String = Sha256::digest(b"the guarded copy")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let record = top.path().join("setup-hashes.json");
        fs::write(
            &record,
            serde_json::json!({ "seat1/client_Release.exe": sum }).to_string(),
        )
        .unwrap();
        let link = for_dir(&real, &top.path().join("run")).unwrap().unwrap();
        let key = vec!["seat1/client_Release.exe".to_owned()];
        let check = |dir: &Path| crate::guard::hashes(&record, &[(dir.join("client_Release.exe"), key.clone())]);
        check(&real).unwrap();
        check(&link).unwrap();
        fs::write(&exe, b"not the guarded copy").unwrap();
        for dir in [&real, &link] {
            assert!(check(dir).unwrap_err().contains("is not the file Setup made"));
        }
    }

    #[test]
    fn the_id_is_stable_and_case_blind() {
        assert_eq!(id(Path::new(r"D:\游戏\game")), id(Path::new(r"\\?\d:\游戏\GAME\")));
        assert_ne!(id(Path::new(r"D:\游戏\game")), id(Path::new(r"D:\游戏\seat1")));
        assert_eq!(id(Path::new(r"D:\a")).len(), 12);
    }
}
