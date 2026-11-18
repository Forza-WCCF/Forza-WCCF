//! An unzipped package over a kit folder, shared by the Launcher's UPDATE and the installer: nothing is written while a
//! program from the folder runs or a file the package lists is missing or changed; each file replaced goes aside first
//! and comes back when a write fails; files the old kit listed that the new one does not are removed.

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt::Display;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// Launchers of earlier kits that this one replaced: removed unless the kit lists them.  SETUP.exe went into the
/// one window in 2026-10-10; that window was PLAY.exe until it became Launcher.exe.
pub const LEGACY: &[&str] = &[
    "PLAY.bat",
    "STOP.bat",
    "SETUP.bat",
    "ENGLISH.bat",
    "STOP.exe",
    "ENGLISH.exe",
    "UPDATE.exe",
    "SETUP.exe",
    "PLAY.exe",
];

/// Why [`apply`] stopped.
#[derive(Debug, PartialEq)]
pub enum Failed<E> {
    /// A program from the kit folder runs (its path): nothing was written.
    Running(PathBuf),
    /// The files the package lists that are missing or changed: nothing was written.
    Damaged(Vec<String>),
    /// A write failed and every file replaced is back.
    RolledBack(E),
    /// A write failed and not every file came back: the rest are in `work`\old.
    Stuck(E),
}

/// The unzipped package `top` over the kit folder `dir`: the top-level names in `skip` (any capitals) are left out,
/// `replace` runs last; `work` (it holds `old`, the files replaced) goes when done.  Nothing is touched unless every
/// listed file is there and whole; a failure on the way puts the old kit back.  `write_failed` words a failed write,
/// `log` gets the lines.
pub fn apply<E: Display + From<String>>(
    top: &Path,
    dir: &Path,
    work: &Path,
    skip: &[&str],
    write_failed: impl Fn(&Path, io::Error) -> E,
    replace: impl FnOnce() -> Result<(), E>,
    log: &dyn Fn(&str),
) -> Result<(), Failed<E>> {
    #[cfg(windows)]
    if let Some(p) = crate::procs::running_from(dir) {
        return Err(Failed::Running(p));
    }
    // every file the new kit lists, there and whole, before the installed kit is touched: an antivirus may have taken some
    let bad = damaged(top);
    if !bad.is_empty() {
        log(&format!("  missing or changed: {}", bad.join(", ")));
        let _ = fs::remove_dir_all(work);
        return Err(Failed::Damaged(bad));
    }

    let mut old = manifest(&dir.join("files.txt")).unwrap_or_default();
    old.extend(LEGACY.iter().map(|&f| f.to_owned()));
    let new = manifest(&top.join("files.txt"));
    // the files replaced go aside into work\old: a copy that fails puts them back, the old kit stays whole
    let aside = work.join("old");
    let mut undo = Vec::new();
    let copied = fs::create_dir_all(&aside)
        .map_err(|e| E::from(format!("{}: {e}", aside.display())))
        .and_then(|()| copy_over(top, dir, skip, &aside, &mut undo, &write_failed))
        .and_then(|()| replace());
    if let Err(e) = copied {
        log(&format!("  {e}"));
        if !roll_back(&undo, log) {
            return Err(Failed::Stuck(e)); // what could not go back is still in work\old
        }
        let _ = fs::remove_dir_all(work);
        return Err(Failed::RolledBack(e));
    }
    // an older kit brings neither: the newer kit's must not stay behind and speak for it
    for f in ["files.txt", "version.txt"] {
        if !top.join(f).is_file() {
            let _ = fs::remove_file(dir.join(f));
        }
    }
    // without the new kit's list nothing is known to be gone: nothing is removed
    if let Some(new) = new {
        for f in obsolete(&old, &new) {
            if fs::remove_file(dir.join(f)).is_ok() {
                log(&format!("  removed {f}"));
            }
        }
    }
    let _ = fs::remove_dir_all(work);
    Ok(())
}

/// A file's SHA-256, lower-case hex.
pub fn sha256_of(path: &Path) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 1 << 20]);
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(buf.get(..n).unwrap_or_default());
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The files the unzipped kit's files.sha256 lists with another hash or not at all, and those its files.txt lists that
/// are not there.  Older kits have only files.txt: for them, only that each file is there.
pub fn damaged(top: &Path) -> Vec<String> {
    let mut bad = Vec::new();
    let sums = fs::read_to_string(top.join("files.sha256")).ok();
    let mut hashed = HashSet::new();
    for line in sums
        .as_deref()
        .unwrap_or("")
        .trim_start_matches('\u{feff}')
        .lines()
    {
        if let Some((hash, f)) = line.trim().split_once("  ") {
            hashed.insert(f.to_ascii_lowercase());
            if sha256_of(&top.join(f)).ok() != Some(hash.to_ascii_lowercase()) {
                bad.push(f.to_owned());
            }
        }
    }
    // with files.sha256, every other file files.txt lists must have its line there too
    let lists = ["files.txt", "files.sha256"];
    for f in manifest(&top.join("files.txt")).unwrap_or_default() {
        let low = f.to_ascii_lowercase();
        let unhashed = sums.is_some() && !lists.contains(&low.as_str()) && !hashed.contains(&low);
        if (unhashed || !top.join(&f).is_file()) && !bad.contains(&f) {
            bad.push(f);
        }
    }
    bad
}

/// What `copy_over` did, newest last: each file written, and where the one it replaced went (None: it was new).
type Undo = Vec<(PathBuf, Option<PathBuf>)>;

/// Every file of `from` over `to`, the top-level names in `skip` left out.  A file it replaces is first moved into
/// `aside` and noted in `undo`, for `roll_back`.
fn copy_over<E: From<String>>(
    from: &Path,
    to: &Path,
    skip: &[&str],
    aside: &Path,
    undo: &mut Undo,
    write_failed: &impl Fn(&Path, io::Error) -> E,
) -> Result<(), E> {
    for entry in fs::read_dir(from).map_err(|e| E::from(format!("{}: {e}", from.display())))? {
        let entry = entry.map_err(|e| E::from(e.to_string()))?;
        let name = entry.file_name();
        if skip.iter().any(|s| name.eq_ignore_ascii_case(s)) {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        let dir = entry.file_type().is_ok_and(|t| t.is_dir());
        let written = if dir {
            fs::create_dir_all(&dst)
        } else {
            let moved = aside.join(undo.len().to_string());
            let kept = if dst.is_file() {
                fs::rename(&dst, &moved).map(|()| Some(moved))
            } else {
                Ok(None)
            };
            kept.and_then(|kept| {
                undo.push((dst.clone(), kept));
                fs::copy(&src, &dst).map(drop)
            })
        };
        written.map_err(|e| write_failed(&dst, e))?;
        if dir {
            copy_over(&src, &dst, &[], aside, undo, write_failed)?;
        }
    }
    Ok(())
}

/// Undoes `copy_over`, newest first: each file written goes, the one it replaced comes back.  False: not all came back.
fn roll_back(undo: &Undo, log: &dyn Fn(&str)) -> bool {
    let mut all = true;
    for (dst, kept) in undo.iter().rev() {
        let _ = fs::remove_file(dst);
        if let Some(kept) = kept
            && let Err(e) = fs::rename(kept, dst)
        {
            log(&format!("  could not put back {}: {e}", dst.display()));
            all = false;
        }
    }
    all
}

/// files.txt: every file a kit ships, one path under the kit folder per line (source\package.ps1 writes it).
pub fn manifest(path: &Path) -> Option<Vec<String>> {
    let text = fs::read_to_string(path).ok()?;
    let lines = text.trim_start_matches('\u{feff}').lines();
    Some(
        lines
            .map(|l| l.trim().replace('/', "\\"))
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

/// What `old` lists and `new` does not: only plain paths inside the kit folder, never under data\.
pub fn obsolete<'a>(old: &'a [String], new: &[String]) -> Vec<&'a str> {
    let new: HashSet<String> = new.iter().map(|n| n.to_ascii_lowercase()).collect();
    old.iter()
        .map(String::as_str)
        .filter(|o| {
            let parts: Vec<Component> = Path::new(o).components().collect();
            parts.iter().all(|c| matches!(c, Component::Normal(_)))
                && !parts
                    .first()
                    .is_some_and(|c| c.as_os_str().eq_ignore_ascii_case("data"))
                && !new.contains(&o.to_ascii_lowercase())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_over_keeps_data() {
        let base = std::env::temp_dir().join(format!("wccf-update-test-{}", std::process::id()));
        let (from, to) = (base.join("zip"), base.join("kit"));
        fs::create_dir_all(from.join("data")).unwrap();
        fs::create_dir_all(from.join("bin")).unwrap();
        fs::create_dir_all(to.join("data")).unwrap();
        fs::create_dir_all(base.join("old")).unwrap();
        fs::write(from.join("data").join("keys.txt"), "new").unwrap();
        fs::write(from.join("bin").join("a.dll"), "new").unwrap();
        fs::write(to.join("data").join("keys.txt"), "mine").unwrap();
        let mut undo = Undo::new();
        let named = |p: &Path, e: io::Error| format!("{}: {e}", p.display());
        copy_over(
            &from,
            &to,
            &["data", "Launcher.exe"],
            &base.join("old"),
            &mut undo,
            &named,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(to.join("data").join("keys.txt")).unwrap(),
            "mine"
        );
        assert_eq!(
            fs::read_to_string(to.join("bin").join("a.dll")).unwrap(),
            "new"
        );
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn obsolete_only_kit_files_gone_from_the_new_kit() {
        let old: Vec<String> = [
            "Launcher.exe",
            "ENGLISH.exe",
            "SETUP.exe",
            "scripts\\old.py",
            "data\\keys.txt",
            "..\\x",
            "C:\\x",
        ]
        .map(String::from)
        .into();
        let new: Vec<String> = ["launcher.exe"].map(String::from).into();
        assert_eq!(
            obsolete(&old, &new),
            ["ENGLISH.exe", "SETUP.exe", "scripts\\old.py"]
        );
    }
}
