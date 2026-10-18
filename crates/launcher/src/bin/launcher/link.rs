//! First run (ROADMAP step 5): a kit unzipped into a path with spaces, accents or a user name in non-English letters
//! gets a proper name.  Launcher.exe links its folder to the place the installer uses,
//! `%SystemDrive%\Games\Forza-WCCF\2010-2011` (installer `install.rs` `version_paths`), with a directory junction (no
//! administrator rights), and starts again from the link path, so the game sees the plain path.  Paths are compared
//! as text and never resolved: `fs::canonicalize` would undo the junction.
//!
//! `decide` is the rule, a pure function over what `probe` saw; `apply` does it.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW};

/// GetDriveTypeW: a network drive (the constant lives in a feature this crate does not otherwise need).
const DRIVE_REMOTE: u32 = 4;

/// The installer's name for this edition (`disc.rs`), under `Games\Forza-WCCF`.
const EDITION: &str = "2010-2011";
/// Set for the restarted copy, so a link that does not hold (the process reports the target's path) cannot loop.
const LINKED: &str = "WCCF_LAUNCHER_LINKED";

/// What the link path holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slot {
    /// no name
    Free,
    /// a real folder (an installer install), or something that is not a folder
    Taken,
    /// a junction; `live`: its target exists, `ours`: its target is this folder
    Junction { live: bool, ours: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub slot: Slot,
    /// the link's drive is NTFS and this folder's drive is local: a junction can be made
    pub can_junction: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// make the junction to this folder, then restart from it
    Link,
    /// the junction is dead: point it at this folder, then restart from it
    Repoint,
    /// the junction is ours already: restart from it
    UseLink,
    /// leave it alone and run here, because the name is taken
    InPlaceTaken,
    /// run here, because Windows cannot link this folder
    InPlaceNoJunction,
}

pub fn decide(seen: Seen) -> Action {
    use Action::*;
    match seen.slot {
        Slot::Junction { live: true, ours: true } => UseLink,
        Slot::Junction {
            live: true,
            ours: false,
        }
        | Slot::Taken => InPlaceTaken,
        Slot::Free if seen.can_junction => Link,
        Slot::Junction { live: false, .. } if seen.can_junction => Repoint,
        Slot::Free | Slot::Junction { .. } => InPlaceNoJunction,
    }
}

// ---- the filesystem ---------------------------------------------------------------------------------------------

pub fn link_path() -> PathBuf {
    let drive = env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    PathBuf::from(format!("{drive}\\Games\\Forza-WCCF\\{EDITION}"))
}

/// Paths as text, without the `\\?\` / `\??\` prefix, trailing separators or case.
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = s.trim_start_matches("\\??\\").trim_start_matches("\\\\?\\");
        s.trim_end_matches('\\').to_lowercase()
    };
    norm(a) == norm(b)
}

/// What the name holds, for a kit in `here`.
fn slot(link: &Path, here: &Path) -> Slot {
    // get_target, not junction::exists: that one follows the link and says "no" for a dead junction
    if let Ok(target) = junction::get_target(link) {
        return Slot::Junction {
            live: target.exists(),
            ours: same_path(&target, here),
        };
    }
    match fs::symlink_metadata(link) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Slot::Free,
        _ => Slot::Taken,
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// `X:\` of a path that starts with a drive letter.
fn drive_root(p: &Path) -> Option<String> {
    let s = p.to_str()?;
    let b = s.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':').then(|| format!("{}:\\", &s[..1]))
}

/// The link's drive is NTFS, this folder's drive is a local drive (a network share or a UNC path has no junction).
fn volumes_allow_junction(link: &Path, here: &Path) -> bool {
    let (Some(link_root), Some(here_root)) = (drive_root(link), drive_root(here)) else {
        return false;
    };
    // SAFETY: `here_root` is a NUL-terminated wide string that lives through the call.
    let local = unsafe { GetDriveTypeW(wide(&here_root).as_ptr()) } != DRIVE_REMOTE;
    let mut fs_name = [0u16; 32];
    // SAFETY: the root is NUL-terminated; `fs_name` is writable for the 32 units given; the other outputs are null.
    let ok = unsafe {
        GetVolumeInformationW(
            wide(&link_root).as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_name.as_mut_ptr(),
            32,
        )
    } != 0;
    let len = fs_name.iter().position(|&c| c == 0).unwrap_or(0);
    local && ok && String::from_utf16_lossy(&fs_name[..len]).eq_ignore_ascii_case("NTFS")
}

pub fn probe(link: &Path, here: &Path) -> Seen {
    Seen {
        slot: slot(link, here),
        can_junction: volumes_allow_junction(link, here),
    }
}

/// Makes (or points) the junction.
fn make(link: &Path, here: &Path, repoint: bool) -> io::Result<()> {
    if repoint {
        junction::delete(link)?; // leaves an empty folder, which create would refuse
        fs::remove_dir(link)?;
    } else if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    junction::create(here, link)
}

// ---- at the start -----------------------------------------------------------------------------------------------

/// What the start does about the link.
pub enum Outcome {
    /// run here; `warn` is a message for the player (only when this folder's path is not plain ASCII)
    InPlace { warn: Option<String> },
    /// the link is in place: start `Launcher.exe` there and end
    Restart(PathBuf),
}

/// `here` is this kit folder as the process was started, never resolved.
pub fn first_run(here: &Path) -> Outcome {
    use crate::text::{t, tf};
    let link = link_path();
    if env::var_os(LINKED).is_some() || same_path(&link, here) {
        return Outcome::InPlace { warn: None };
    }
    let ascii = here.to_str().is_some_and(str::is_ascii);
    let say = |msg: String| Outcome::InPlace {
        warn: (!ascii).then_some(msg),
    };
    let action = decide(probe(&link, here));
    match action {
        Action::InPlaceTaken => say(tf("launcher.msg_link_taken", &[("path", &link.display().to_string())])),
        Action::InPlaceNoJunction => say(t("launcher.msg_link_no_junction")),
        Action::UseLink => Outcome::Restart(link),
        Action::Link | Action::Repoint => match make(&link, here, action == Action::Repoint) {
            Ok(()) => Outcome::Restart(link),
            Err(e) => say(tf(
                "launcher.msg_link_failed",
                &[("path", &link.display().to_string()), ("error", &e.to_string())],
            )),
        },
    }
}

/// Starts the copy in the link, which is then the one that runs.
pub fn restart(link: &Path) -> bool {
    std::process::Command::new(link.join("Launcher.exe"))
        .current_dir(link)
        .env(LINKED, "1")
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    fn seen(slot: Slot, can_junction: bool) -> Seen {
        Seen { slot, can_junction }
    }

    #[test]
    fn decision_table() {
        let j = |live, ours| Slot::Junction { live, ours };
        assert_eq!(decide(seen(Slot::Free, true)), Link); // no name
        assert_eq!(decide(seen(Slot::Taken, true)), InPlaceTaken); // a real folder
        assert_eq!(decide(seen(j(true, true), true)), UseLink); // live junction to us
        assert_eq!(decide(seen(j(true, false), true)), InPlaceTaken); // live junction elsewhere
        assert_eq!(decide(seen(j(false, false), true)), Repoint); // dead junction
        assert_eq!(decide(seen(j(false, true), true)), Repoint); // dead, though it names us (cannot happen, safe)
        assert_eq!(decide(seen(Slot::Free, false)), InPlaceNoJunction); // non-NTFS or network
        assert_eq!(decide(seen(j(false, false), false)), InPlaceNoJunction);
        assert_eq!(decide(seen(Slot::Taken, false)), InPlaceTaken); // the name wins: nothing to make
        assert_eq!(decide(seen(j(true, true), false)), UseLink); // an existing link keeps working
    }

    #[test]
    fn paths_compared_as_text() {
        assert!(same_path(Path::new(r"C:\Kit\"), Path::new(r"\??\c:\kit")));
        assert!(same_path(Path::new(r"\\?\C:\Jürgen\x"), Path::new("c:/jürgen/x")));
        assert!(!same_path(Path::new(r"C:\a"), Path::new(r"C:\b")));
        assert_eq!(drive_root(Path::new(r"d:\x\y")).as_deref(), Some(r"d:\"));
        assert_eq!(drive_root(Path::new(r"\\srv\share")), None);
    }

    #[test]
    fn probe_sees_what_is_there() {
        let base = env::temp_dir().join(format!("wccf-link-test-{}", std::process::id()));
        let (here, other, link) = (
            base.join("Jürgen kit"),
            base.join("other"),
            base.join("Games").join("link"),
        );
        fs::create_dir_all(&here).unwrap();
        fs::create_dir_all(&other).unwrap();
        assert_eq!(slot(&link, &here), Slot::Free);
        make(&link, &here, false).unwrap(); // also makes Games\
        assert_eq!(slot(&link, &here), Slot::Junction { live: true, ours: true });
        assert_eq!(
            slot(&link, &other),
            Slot::Junction {
                live: true,
                ours: false
            }
        );
        fs::remove_dir_all(&here).unwrap();
        assert_eq!(
            slot(&link, &here),
            Slot::Junction {
                live: false,
                ours: true
            }
        );
        make(&link, &other, true).unwrap(); // repoint the dead one
        assert_eq!(slot(&link, &other), Slot::Junction { live: true, ours: true });
        junction::delete(&link).unwrap();
        fs::remove_dir(&link).unwrap();
        fs::create_dir_all(&link).unwrap();
        assert_eq!(slot(&link, &here), Slot::Taken); // a real folder
        fs::remove_dir_all(&base).unwrap();
    }
}
