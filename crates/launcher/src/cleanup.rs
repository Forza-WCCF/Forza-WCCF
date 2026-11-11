//! CLEAN UP (the 2017-18 SETUP tab): what a crash, a freeze or a dropped arcade visit leaves behind that
//! can make the next start misbehave.  The bad-ending counter itself is CLEAR BAD ENDINGS' job (the panel; 2017-18:
//! `club::clear_bad_endings`); this clears the run's leftovers:
//! - `data\running.json` of a run nobody supervises, and the requests and answers a run left (`*.request`, `*.answer`);
//! - a club still marked out in an arcade (`club_out.json`, `club_last.json` in the active profile's `save\gamesvr`);
//! - game programs left running without a window (`match_Release`, `control_Release`, `FPR_Emu`, `logowin`), only those
//!   whose path is under this install's folders.
//!
//! The club save (the active profile's, `data\save` for free play) is never deleted: before anything is removed it is
//! copied, with each file to be removed, to `data\backup\bad-ending-<date>\`.  The seat lease is held by the Launcher's
//! connection, not by a file, and the panel's match state is read from the game's memory, so neither leaves anything on
//! disk.  Refused while a run is on, a game window is open or the game's client runs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use forza::online::club;
use forza::procs::Proc;

/// `data\` files a run leaves; with no run on, none of them means anything.
const LEFT: [&str; 8] = [
    "running.json",
    "run.request",
    "run.answer",
    "server.request",
    "server.answer",
    "restart.request",
    "savelogs.request",
    "report.request",
];

/// The arcade copy kept beside the club's store.
const ARCADE_COPY: [&str; 2] = ["club_out.json", "club_last.json"];

/// What a clean-up did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// A run is on, or a game window or the client is open: nothing was touched.
    pub refused: bool,
    /// One line (English, for the log) per item cleaned.
    pub cleaned: Vec<String>,
    /// One line per item that could not be cleaned.
    pub failed: Vec<String>,
    /// Where the backup went; none when there was nothing to clean.
    pub backup: Option<PathBuf>,
}

/// Cleans `data` (the install's `data\`). `procs` are the game programs running from this install (`procs::game_processes`);
/// `run_on` is the run lock; `has_window`, `kill` and `stamp` (`YYYY-MM-DD`) are the system's, given so tests can fake them.
pub fn clean(
    data: &Path,
    run_on: bool,
    procs: &[Proc],
    has_window: impl Fn(u32) -> bool,
    kill: impl Fn(u32) -> bool,
    stamp: &str,
) -> io::Result<Report> {
    let mut report = Report::default();
    let client = |p: &Proc| p.name.eq_ignore_ascii_case("client_release.exe");
    if run_on || procs.iter().any(|p| client(p) || has_window(p.pid)) {
        report.refused = true;
        return Ok(report);
    }
    let save = forza::kit::save_in(data);
    let store = save.join("gamesvr");
    let files: Vec<PathBuf> = LEFT
        .iter()
        .map(|n| data.join(n))
        .chain(ARCADE_COPY.iter().map(|n| store.join(n)))
        .filter(|p| p.is_file())
        .collect();
    if files.is_empty() && procs.is_empty() {
        return Ok(report);
    }
    if !files.is_empty() {
        let dir = backup_dir(data, stamp)?;
        copy_save(&save, &dir.join("save"))?;
        for f in &files {
            let rel = f.strip_prefix(data).unwrap_or(f);
            let to = dir.join(rel);
            if let Some(up) = to.parent() {
                fs::create_dir_all(up)?;
            }
            fs::copy(f, &to)?;
        }
        report.backup = Some(dir);
    }
    for f in files.iter().filter(|f| !f.starts_with(&store)) {
        let name = f.file_name().unwrap_or_default().to_string_lossy();
        match fs::remove_file(f) {
            Ok(()) => report.cleaned.push(format!("removed data\\{name}")),
            Err(e) => report.failed.push(format!("data\\{name}: {e}")),
        }
    }
    if ARCADE_COPY.iter().any(|n| store.join(n).is_file()) {
        // the home club stays as it was; the marker and the arcade's last copy go (the copy to save\gamesvr\backups too)
        match club::leftover(&store) {
            // a copy without its marker is stale too (both are in the backup); `leftover` may have removed it already
            Ok(_) => match fs::remove_file(store.join(ARCADE_COPY[1])) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => report.failed.push(format!("arcade copy: {e}")),
                _ => report.cleaned.push("arcade visit closed: the home club is kept".into()),
            },
            Err(e) => report.failed.push(format!("arcade copy: {e}")),
        }
    }
    for p in procs {
        if kill(p.pid) {
            report.cleaned.push(format!("stopped {} (pid {})", p.name, p.pid));
        } else {
            report.failed.push(format!("{} (pid {}) did not stop", p.name, p.pid));
        }
    }
    Ok(report)
}

/// Today on this PC's clock, `YYYY-MM-DD`.
pub fn today() -> String {
    let t = crate::logs::local(std::time::SystemTime::now());
    format!("{:04}-{:02}-{:02}", t.year(), u8::from(t.month()), t.day())
}

/// `data\backup\bad-ending-<stamp>`, new (a second clean-up the same day gets `-2`, `-3` ...).
fn backup_dir(data: &Path, stamp: &str) -> io::Result<PathBuf> {
    let base = data.join("backup");
    let mut dir = base.join(format!("bad-ending-{stamp}"));
    for n in 2.. {
        if !dir.exists() {
            break;
        }
        dir = base.join(format!("bad-ending-{stamp}-{n}"));
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// `from` copied to `to`, without the backup folders and the captured `req_*` files.
fn copy_save(from: &Path, to: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        // unreadable is not empty: no backup, nothing removed
        Err(e) => return Err(e),
    };
    for e in entries {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_lowercase();
        let (path, dest) = (e.path(), to.join(e.file_name()));
        if path.is_dir() {
            if !name.starts_with("backup") {
                copy_save(&path, &dest)?;
            }
        } else if !name.starts_with("req_") {
            fs::create_dir_all(to)?;
            fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn proc(pid: u32, name: &str) -> Proc {
        Proc {
            pid,
            parent: 1,
            name: name.into(),
            path: PathBuf::from(r"C:\Games\Forza-WCCF\2017-2018\game").join(name),
        }
    }

    /// An install with every leftover and a club save.
    fn install() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let store = data.join("save").join("gamesvr");
        fs::create_dir_all(store.join("backups")).unwrap();
        fs::write(store.join("gamesvr.db"), b"the club").unwrap();
        fs::write(store.join("club_out.json"), b"{\"arcade\":\"x\"}").unwrap();
        fs::write(store.join("club_last.json"), b"{\"last\":1}").unwrap();
        fs::write(data.join("save").join("seat1_aime.txt"), b"50123456789012345678").unwrap();
        for n in LEFT {
            fs::write(data.join(n), b"stale").unwrap();
        }
        t
    }

    fn run(data: &Path, run_on: bool, procs: &[Proc], windows: &[u32], killed: &RefCell<Vec<u32>>) -> Report {
        clean(
            data,
            run_on,
            procs,
            |pid| windows.contains(&pid),
            |pid| {
                killed.borrow_mut().push(pid);
                true
            },
            "2026-11-02",
        )
        .unwrap()
    }

    #[test]
    fn every_leftover_is_cleaned_after_a_backup_and_the_club_save_stays() {
        let t = install();
        let data = t.path().join("data");
        let store = data.join("save").join("gamesvr");
        let killed = RefCell::new(Vec::new());
        let procs = [proc(10, "match_Release.exe"), proc(11, "FPR_Emu.exe")];
        let r = run(&data, false, &procs, &[], &killed);
        assert!(!r.refused && r.failed.is_empty(), "{r:?}");
        assert_eq!(*killed.borrow(), [10, 11]);
        assert_eq!(r.cleaned.len(), LEFT.len() + 1 + 2, "{r:?}");
        for n in LEFT {
            assert!(!data.join(n).exists(), "{n} goes");
        }
        assert!(!store.join("club_out.json").exists() && !store.join("club_last.json").exists());
        // the club save is untouched
        assert_eq!(fs::read(store.join("gamesvr.db")).unwrap(), b"the club");
        assert_eq!(
            fs::read(data.join("save").join("seat1_aime.txt")).unwrap(),
            b"50123456789012345678"
        );
        // the backup has the club save and each removed file
        let b = r.backup.unwrap();
        assert_eq!(b, data.join("backup").join("bad-ending-2026-11-02"));
        assert_eq!(fs::read(b.join(r"save\gamesvr\gamesvr.db")).unwrap(), b"the club");
        assert_eq!(
            fs::read(b.join(r"save\gamesvr\club_out.json")).unwrap(),
            b"{\"arcade\":\"x\"}"
        );
        assert_eq!(fs::read(b.join("running.json")).unwrap(), b"stale");
        // the second clean-up the same day does not overwrite it
        fs::write(data.join("running.json"), b"again").unwrap();
        let again = run(&data, false, &[], &[], &killed);
        assert_eq!(
            again.backup.unwrap(),
            data.join("backup").join("bad-ending-2026-11-02-2")
        );
        assert_eq!(fs::read(b.join("running.json")).unwrap(), b"stale");
    }

    #[test]
    fn a_game_that_runs_refuses_and_nothing_is_touched() {
        let t = install();
        let data = t.path().join("data");
        let killed = RefCell::new(Vec::new());
        // the client, a window on a helper, a run on
        for (run_on, procs, windows) in [
            (
                false,
                vec![proc(1, "client_Release.exe"), proc(2, "match_Release.exe")],
                vec![],
            ),
            (false, vec![proc(2, "match_Release.exe")], vec![2]),
            (true, vec![], vec![]),
        ] {
            let r = run(&data, run_on, &procs, &windows, &killed);
            assert!(r.refused && r.cleaned.is_empty() && r.backup.is_none());
        }
        assert!(killed.borrow().is_empty());
        assert!(data.join("running.json").exists());
        assert!(!data.join("backup").exists());
    }

    #[test]
    fn nothing_to_clean_makes_no_backup() {
        let t = tempfile::tempdir().unwrap();
        let killed = RefCell::new(Vec::new());
        let r = run(&t.path().join("data"), false, &[], &[], &killed);
        assert_eq!(r, Report::default());
        assert!(!t.path().join("data").exists());
    }

    #[test]
    fn an_unreadable_save_is_an_error_and_nothing_is_removed() {
        let t = install();
        let data = t.path().join("data");
        // a `save` that cannot be listed (here a file) is not an empty one
        fs::remove_dir_all(data.join("save")).unwrap();
        fs::write(data.join("save"), b"not a folder").unwrap();
        let r = clean(&data, false, &[], |_| false, |_| true, "2026-11-02");
        assert!(r.is_err(), "{r:?}");
        assert!(data.join("running.json").exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_copy_that_cannot_be_removed_is_reported_failed() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = install();
        let data = t.path().join("data");
        let store = data.join("save").join("gamesvr");
        fs::remove_file(store.join("club_out.json")).unwrap();
        // held open without delete sharing (FILE_SHARE_READ): copied to the backup, not removable
        let _held = fs::File::options()
            .read(true)
            .share_mode(1)
            .open(store.join("club_last.json"))
            .unwrap();
        let r = run(&data, false, &[], &[], &RefCell::new(Vec::new()));
        assert_eq!(r.failed.len(), 1, "{r:?}");
        assert!(r.failed[0].starts_with("arcade copy"), "{r:?}");
        assert!(!r.cleaned.iter().any(|l| l.starts_with("arcade visit closed")), "{r:?}");
    }

    #[test]
    fn a_stray_alone_is_stopped_without_a_backup() {
        let t = tempfile::tempdir().unwrap();
        let killed = RefCell::new(Vec::new());
        let r = run(t.path(), false, &[proc(7, "control_Release.exe")], &[], &killed);
        assert_eq!(*killed.borrow(), [7]);
        assert_eq!(r.cleaned.len(), 1);
        assert!(r.backup.is_none());
    }
}
