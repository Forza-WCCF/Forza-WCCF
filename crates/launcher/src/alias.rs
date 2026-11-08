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
    base_in(env::var_os("ProgramData"))
}

/// `base` for a ProgramData value: an empty or relative one counts as unset, so the junctions never depend on the
/// Launcher's working folder.
fn base_in(program_data: Option<std::ffi::OsString>) -> PathBuf {
    program_data
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
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

/// Names tried for one folder: `<id>`, then `<id>-1`.. when another folder or a file holds it.
const NAMES: usize = 8;

/// The folder a game program in `dir` starts from: None when `dir`'s path is plain ASCII (it starts there, as it
/// always did), else a junction in `base` that leads to exactly `dir` (same folder on disk, not just the same text),
/// made or mended now.  A name held by another folder's junction, a file or a folder with files is left as it is and
/// the next name is tried.  Err: no junction could be made (ProgramData not writable, a network folder, every name
/// taken, ...); the program then starts from `dir`.
pub fn for_dir(dir: &Path, base: &Path) -> io::Result<Option<PathBuf>> {
    if is_ascii(dir) {
        return Ok(None);
    }
    let real = fs::canonicalize(dir)?;
    let id = id(dir);
    let mut why = Vec::new();
    for n in 0..NAMES {
        let link = base.join(if n == 0 { id.clone() } else { format!("{id}-{n}") });
        if !is_ascii(&link) {
            return Err(io::Error::other(format!(
                "{} is not a plain ASCII path",
                link.display()
            )));
        }
        match take(&link, dir, &real) {
            Ok(()) => return Ok(Some(link)),
            Err(e) => why.push(format!("{}: {e}", link.display())),
        }
    }
    Err(io::Error::other(format!(
        "no junction name is free for {}: {}",
        dir.display(),
        why.join("; ")
    )))
}

/// Makes `link` lead to `dir` (on disk `real`), or says why that name cannot be used.
fn take(link: &Path, dir: &Path, real: &Path) -> io::Result<()> {
    if fs::symlink_metadata(link).is_ok() {
        match fs::canonicalize(link) {
            Ok(to) if to == real && junction::exists(link).unwrap_or(false) => return Ok(()),
            Ok(_) if junction::exists(link).unwrap_or(false) => {
                return Err(io::Error::other("leads to another folder"));
            }
            // broken (its target is gone) or an empty folder: removes the link only, never what it points at
            // (RemoveDirectoryW); a file or a folder with files stays and the name is skipped
            _ => fs::remove_dir(link)?,
        }
    } else if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    junction::create(dir, link)?;
    if fs::canonicalize(link).ok().as_deref() != Some(real) {
        let _ = fs::remove_dir(link);
        return Err(io::Error::other(format!("does not lead to {}", dir.display())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{GAME_PROGRAMS, find_game_named, seat_dir};

    /// A scratch folder on a plain-ASCII path: the system's temp folder holds the user name, which may not be ASCII
    /// (the machines this module is for), so it goes under ProgramData then.
    fn top() -> tempfile::TempDir {
        let tmp = env::temp_dir();
        let at = if is_ascii(&tmp) { tmp } else { base().join("tests") };
        fs::create_dir_all(&at).unwrap();
        tempfile::tempdir_in(at).unwrap()
    }

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
        let top = top();
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
        let top = top();
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
    fn a_name_held_by_another_folder_is_never_used_and_the_next_name_is() {
        let top = top();
        let base = top.path().join("run");
        let (real, other) = (top.path().join("游戏").join("game"), top.path().join("other"));
        for d in [&real, &other] {
            fs::create_dir_all(d.join("save")).unwrap();
            fs::write(d.join("save").join("club"), d.display().to_string()).unwrap();
        }
        let link = base.join(id(&real));
        fs::create_dir_all(&base).unwrap();
        // another folder's junction under this one's name: a 48-bit collision, or a folder whose path differs only in
        // case in a case-sensitive directory (same id, another folder on disk)
        junction::create(&other, &link).unwrap();
        let next = base.join(format!("{}-1", id(&real)));
        assert_eq!(for_dir(&real, &base).unwrap(), Some(next.clone()));
        assert_eq!(fs::canonicalize(&next).unwrap(), fs::canonicalize(&real).unwrap());
        assert_eq!(
            fs::canonicalize(&link).unwrap(),
            fs::canonicalize(&other).unwrap(),
            "left as it was"
        );
        assert_eq!(
            for_dir(&real, &base).unwrap(),
            Some(next.clone()),
            "the next start finds it again"
        );
        assert_eq!(for_dir(&other, &base).unwrap(), None, "plain ASCII: no junction");
        for d in [&real, &other] {
            assert_eq!(
                fs::read_to_string(d.join(r"save\club")).unwrap(),
                d.display().to_string()
            );
        }
    }

    #[test]
    fn a_broken_junction_or_an_empty_folder_under_the_name_is_replaced() {
        let top = top();
        let base = top.path().join("run");
        let real = top.path().join("游戏").join("game");
        fs::create_dir_all(real.join("save")).unwrap();
        fs::write(real.join(r"save\club"), b"x").unwrap();
        let link = base.join(id(&real));
        fs::create_dir_all(&base).unwrap();
        let gone = top.path().join("gone");
        fs::create_dir(&gone).unwrap();
        junction::create(&gone, &link).unwrap();
        fs::remove_dir(&gone).unwrap();
        assert_eq!(for_dir(&real, &base).unwrap(), Some(link.clone()));
        assert!(link.join(r"save\club").is_file());
        fs::remove_dir(&link).unwrap();
        fs::create_dir(&link).unwrap();
        assert_eq!(for_dir(&real, &base).unwrap(), Some(link.clone()));
        assert!(link.join(r"save\club").is_file());
    }

    #[test]
    fn a_file_or_a_folder_with_files_under_the_name_stays_and_the_next_name_is_used() {
        let top = top();
        let base = top.path().join("run");
        let real = top.path().join("游戏").join("game");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(&base).unwrap();
        let id = id(&real);
        let names: Vec<PathBuf> = (0..NAMES)
            .map(|n| base.join(if n == 0 { id.clone() } else { format!("{id}-{n}") }))
            .collect();
        fs::write(&names[0], b"a file someone left").unwrap();
        fs::create_dir(&names[1]).unwrap();
        fs::write(names[1].join("keep.txt"), b"x").unwrap();
        assert_eq!(for_dir(&real, &base).unwrap(), Some(names[2].clone()));
        assert_eq!(fs::read(&names[0]).unwrap(), b"a file someone left");
        assert!(names[1].join("keep.txt").is_file());
        // every name taken: a clear error, nothing removed
        fs::remove_dir(&names[2]).unwrap();
        for n in &names[2..] {
            fs::write(n, b"taken").unwrap();
        }
        let e = for_dir(&real, &base).unwrap_err().to_string();
        assert!(e.contains("no junction name is free"), "{e}");
        assert!(names.iter().all(|n| n.exists()));
    }

    #[test]
    fn an_empty_or_relative_program_data_falls_back_to_the_system_folder() {
        let system = PathBuf::from(r"C:\ProgramData\Forza-WCCF\run");
        for v in [None, Some(""), Some("ProgramData"), Some(r"Forza\run")] {
            assert_eq!(base_in(v.map(Into::into)), system, "{v:?}");
        }
        assert_eq!(base_in(Some(r"D:\PD".into())), PathBuf::from(r"D:\PD\Forza-WCCF\run"));
        assert!(base().is_absolute());
    }

    #[test]
    fn no_junction_is_an_error_and_the_start_text_is_in_every_language() {
        let top = top();
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
        let top = top();
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
