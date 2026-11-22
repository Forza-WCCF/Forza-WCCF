//! UPDATE: the kit's releases and test builds (pre-releases).  The chosen one's kit ZIP is downloaded,
//! unzipped into data\update and copied over the kit folder - data\ (club cards, keys, settings, logs) is never
//! written, and nothing is copied while the game or a kit tool runs.  Files the old kit shipped (files.txt) that the
//! new one does not are removed; then setup runs again, and English if on.
//!
//! The download and the unzip are Windows' own curl.exe and tar.exe (in System32 since Windows 10 1803): HTTPS with
//! Windows' certificates, and tar refuses a ZIP entry that would land outside its folder (an absolute path or "..").

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use std::{env, fs, thread};

use forza::apply::{Failed, LEGACY, manifest, obsolete};
use launcher::Kit;
use launcher::release::{curl, no_target, system32, update_url};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::text::{t, tf};
use crate::{log, plain_error, say};

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub test: bool,
    pub date: String,
    pub page: String,
    /// The release notes carry the marker `needs-installer`: the game data changed, UPDATE copies nothing.
    pub needs_installer: bool,
    /// The release notes (markdown, remote text: shown only through `launcher::release::notes`).
    pub notes: String,
    zip_url: String,
    zip_size: u64,
    /// The `<zip>.sha256` asset, and GitHub's own `digest` of the ZIP ("sha256:<hex>").
    sha_url: Option<String>,
    digest: Option<String>,
}

/// The releases that have this edition's ZIP, `Forza-WCCF-<edition>-<version>.zip` (the tag without its `v`), in the order
/// the target lists them (GitHub's shape).  The installer's ZIP and the other edition's are never taken.
fn releases(json: &str, edition: &str) -> Result<Vec<Release>, String> {
    let list: Vec<Value> = serde_json::from_str(json).map_err(|e| format!("not a list of releases ({e})"))?;
    Ok(list
        .iter()
        .filter(|r| r["draft"].as_bool() != Some(true))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            launcher::release::parse(tag)?;
            let name = format!("Forza-WCCF-{edition}-{}.zip", tag.strip_prefix('v').unwrap_or(tag));
            let assets = r["assets"].as_array()?;
            let find = |n: &str| assets.iter().find(|a| a["name"].as_str() == Some(n));
            let asset = find(&name)?;
            let https = |a: &Value| {
                a["browser_download_url"]
                    .as_str()
                    .filter(|u| u.starts_with("https://"))
                    .map(str::to_owned)
            };
            Some(Release {
                tag: tag.to_owned(),
                test: launcher::release::is_test(tag),
                date: r["published_at"].as_str().unwrap_or("").chars().take(10).collect(),
                page: r["html_url"].as_str().unwrap_or("").to_owned(),
                needs_installer: r["body"]
                    .as_str()
                    .is_some_and(|b| b.to_ascii_lowercase().contains("needs-installer")),
                notes: r["body"].as_str().unwrap_or("").to_owned(),
                zip_url: https(asset)?,
                zip_size: asset["size"].as_u64().unwrap_or(0),
                sha_url: find(&format!("{name}.sha256")).and_then(https),
                digest: asset["digest"].as_str().map(str::to_owned),
            })
        })
        .collect())
}

/// The SHA-256 a download must have: the first token of the `.sha256` asset's text (64 hex), else the asset's `digest`
/// ("sha256:<hex>").  Lower case; None when neither is there.
fn expected_hash(sha_file: Option<&str>, digest: Option<&str>) -> Option<String> {
    let hex = |s: &str| (s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase());
    sha_file
        .and_then(|t| hex(t.split_whitespace().next()?))
        .or_else(|| digest.and_then(|d| hex(d.strip_prefix("sha256:")?)))
}

/// The file's SHA-256 is `expected`, hashed as it is read.
pub(crate) fn verify(file: &Path, expected: &str) -> Result<(), String> {
    let mut f = fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hash.update(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("{}: {e}", file.display())),
        }
    }
    let got: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got == expected {
        Ok(())
    } else {
        Err(format!("sha256 {got}, expected {expected}"))
    }
}

/// The release to announce: the first listed that is not a test build and is newer than `installed`.
pub fn newest<'a>(list: &'a [Release], installed: &str) -> Option<&'a Release> {
    list.iter()
        .find(|r| !r.test && launcher::release::is_newer(&r.tag, installed))
}

pub fn fetch(edition: &str) -> Result<Vec<Release>, String> {
    if no_target() {
        return Ok(Vec::new());
    }
    let api = format!("{}releases?per_page=30", update_url());
    let out = curl()
        .args(["-H", "Accept: application/vnd.github+json", "--url", &api])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    releases(&String::from_utf8_lossy(&out.stdout), edition)
}

fn running_check(kit: &Kit) -> Result<(), String> {
    match launcher::procs::running_from(kit.dir()) {
        Some(p) => Err(close_game(&p)),
        None => Ok(()),
    }
}

fn close_game(p: &Path) -> String {
    let name = p.file_name().map_or(p.as_os_str(), OsStr::new).display().to_string();
    tf("launcher.msg_close_game", &[("name", &name)])
}

/// curl.exe fetches `url` into `file` (`resume`: on from the bytes already there, an HTTP Range request), with the
/// progress line "`label`  N / M MB" against `total` bytes every 200 ms. A server or proxy that refuses the Range
/// request gets the whole file once instead: the bytes there are deleted first.
pub(crate) fn download(file: &Path, url: &str, total: u64, label: &str, resume: bool) -> Result<(), String> {
    match curl_into(file, url, total, label, resume) {
        Err((code, _)) if resume && cannot_resume(code) => {
            log(&format!(
                "  {}: the server cannot resume (curl exit 33), from zero",
                file.display()
            ));
            let _ = fs::remove_file(file);
            curl_into(file, url, total, label, false)
        }
        r => r,
    }
    .map_err(|(_, e)| e)
}

/// curl's exit code 33: "HTTP server does not seem to support byte ranges. Cannot resume."
fn cannot_resume(code: Option<i32>) -> bool {
    code == Some(33)
}

/// One curl.exe run of [`download`] -> its exit code with the error.
fn curl_into(file: &Path, url: &str, total: u64, label: &str, resume: bool) -> Result<(), (Option<i32>, String)> {
    let mut cmd = curl();
    if resume {
        cmd.args(["-C", "-"]);
    }
    let mut child = cmd
        .arg("-o")
        .arg(file)
        .args(["--url", url])
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            (
                None,
                tf(
                    "launcher.msg_download_failed",
                    &[("error", &plain_error(&e.to_string()))],
                ),
            )
        })?;
    while child.try_wait().map_err(|e| (None, e.to_string()))?.is_none() {
        let got = fs::metadata(file).map_or(0, |m| m.len());
        let mb = |b: u64| b as f64 / 1_048_576.0;
        say(
            format!("{label}  {:.1} / {:.1} MB", mb(got), mb(total)),
            (got.saturating_mul(900) / total.max(1)).min(900) as u32,
        );
        thread::sleep(Duration::from_millis(200));
    }
    let out = child.wait_with_output().map_err(|e| (None, e.to_string()))?;
    if !out.status.success() {
        let why = plain_error(String::from_utf8_lossy(&out.stderr).trim());
        return Err((
            out.status.code(),
            tf("launcher.msg_download_failed", &[("error", &why)]),
        ));
    }
    Ok(())
}

/// Download, unzip, copy over the kit folder, remove what the new kit no longer ships.
pub fn apply(kit: &Kit, r: &Release) -> Result<(), String> {
    if r.needs_installer {
        return Err(tf("launcher.msg_needs_installer", &[("tag", &r.tag)]));
    }
    running_check(kit)?;
    let dir = kit.dir();
    let work = dir.join("data").join("update");
    let unzipped = work.join("kit");
    let zip = work.join("kit.zip");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&unzipped).map_err(|e| format!("{}: {e}", work.display()))?;

    // the SHA-256 published for the ZIP, before anything is downloaded
    let listed = r.sha_url.as_ref().and_then(|u| {
        let out = curl().args(["--url", u]).output().ok().filter(|o| o.status.success())?;
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    });
    let Some(expected) = expected_hash(listed.as_deref(), r.digest.as_deref()) else {
        let _ = fs::remove_dir_all(&work);
        return Err(t("launcher.msg_unverified"));
    };

    let label = tf("launcher.msg_downloading", &[("tag", &r.tag)]);
    log(&label);
    download(&zip, &r.zip_url, r.zip_size, &label, false)?;

    if let Err(e) = verify(&zip, &expected) {
        log(&format!("  {e}"));
        let _ = fs::remove_dir_all(&work);
        return Err(t("launcher.msg_unverified"));
    }

    log(&t("launcher.msg_unzipping"));
    say(t("launcher.msg_unzipping"), 920);
    let out = system32("tar.exe")
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&unzipped)
        .output()
        .map_err(|e| {
            log(&format!("  tar: {e}"));
            t("launcher.msg_unzip_failed")
        })?;
    if !out.status.success() {
        log(&format!("  tar: {}", String::from_utf8_lossy(&out.stderr).trim()));
        return Err(t("launcher.msg_unzip_failed"));
    }
    // the ZIP holds one folder (Forza-WCCF-2010-11\, or WCCF-2010-11-kit\ in older releases): whichever holds Launcher.exe
    let top = fs::read_dir(&unzipped)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("Launcher.exe").is_file())
        .ok_or_else(|| t("launcher.msg_not_a_kit"))?;

    running_check(kit)?; // the game may have started during the download
    log(&tf("launcher.msg_copying", &[("tag", &r.tag)]));
    say(tf("launcher.msg_copying", &[("tag", &r.tag)]), 950);
    // this running Launcher.exe last: `replace_running` swaps it for the new one, which the next start runs.  If the swap
    // fails with no Launcher.exe left, the copy kept beside it goes back
    let me = dir.join("Launcher.exe");
    let kept = dir.join("Launcher.exe.old");
    install(&top, dir, &work, || {
        let failed = |(p, e): (&Path, io::Error)| {
            let name = p.file_name().map_or(p.as_os_str(), OsStr::new).display().to_string();
            tf(
                "launcher.msg_write_failed",
                &[("path", &name), ("error", &e.to_string())],
            )
        };
        swap_self(&me, &kept, || replace_running(&me, &top.join("Launcher.exe"))).map_err(failed)
    })
}

/// The Launcher.exe swap: `me` goes into `kept` first, so it goes back when the swap has already moved it away and then
/// fails (the folder would hold no Launcher.exe at all).  When even that copy fails, both errors are reported and the copy
/// stays: `kept` sits beside `me`, since data\update goes on a rollback.  `kept` is held open without delete sharing
/// until the swap is over: a second Launcher starting in between ([`tidy`] removes `Launcher.exe.old`) cannot take it.
/// An error comes with the file it was on.
fn swap_self<'a>(
    me: &'a Path,
    kept: &'a Path,
    swap: impl FnOnce() -> io::Result<()>,
) -> Result<(), (&'a Path, io::Error)> {
    use std::io::{Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt;
    // `me` first: after a failed put-back it may be gone, and the spare is then the only copy, not to be truncated
    let mut running = fs::File::open(me).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound if kept.is_file() => {
            let left = io::Error::new(e.kind(), format!("{e}; the Launcher left is {}", kept.display()));
            (me, left)
        }
        _ => (me, e),
    })?;
    let mut spare = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(1) // FILE_SHARE_READ: no delete, no rename while held
        .open(kept)
        .map_err(|e| (kept, e))?;
    io::copy(&mut running, &mut spare)
        .and_then(|_| spare.sync_all()) // on the disk before Launcher.exe moves: a power loss keeps a whole spare
        .map_err(|e| (kept, e))?;
    drop(running);
    let done = swap().map_err(|e| (me, e));
    if let Err((_, e)) = &done
        && !me.is_file()
    {
        // written beside and flushed, then renamed in: a put-back that fails half way or a power loss right after it
        // leaves no short Launcher.exe (the spare goes next).  A scanner may hold the fresh file for a moment: the
        // rename tries again for up to 1 s
        let restore = me.with_extension("exe.restore");
        let back = spare
            .seek(SeekFrom::Start(0))
            .and_then(|_| {
                let mut to = fs::File::create(&restore)?;
                io::copy(&mut spare, &mut to)?;
                to.sync_all()
            })
            .and_then(|()| forza::cards::swap_in(&restore, me, 50, 20));
        if let Err(back) = back {
            let _ = fs::remove_file(&restore);
            return Err((
                me,
                io::Error::new(e.kind(), format!("{e}; {back} ({})", kept.display())),
            ));
        }
    }
    drop(spare);
    if me.is_file() {
        let _ = fs::remove_file(kept); // the kit folder holds its Launcher.exe: the spare goes
    }
    done
}

/// The running Launcher.exe `me` swapped for the new one `new` (#361).  The copy is written beside it and flushed first,
/// so only two renames back to back leave the folder without a Launcher.exe.  A running exe can be renamed but not
/// deleted: the old one stays as `.Launcher.<pid>.__relocated__.exe` until [`tidy`] removes it once it has exited.  When the
/// second rename fails the old one is renamed back at once; a scanner holding the fresh copy gets the pair tried again
/// for up to 1 s, so no try leaves the folder without a Launcher.exe for longer than the two renames.
fn replace_running(me: &Path, new: &Path) -> io::Result<()> {
    let (Some(dir), Some(stem)) = (me.parent(), me.file_stem().and_then(OsStr::to_str)) else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, me.display().to_string()));
    };
    let name = |end: &str| dir.join(format!(".{stem}.{}.{end}", std::process::id()));
    let (temp, away) = (name("__temp__.exe"), name("__relocated__.exe"));
    let swapped = fs::File::create(&temp)
        .and_then(|mut to| {
            io::copy(&mut fs::File::open(new)?, &mut to)?;
            to.sync_all()
        })
        .and_then(|()| {
            let mut tries = 20;
            loop {
                fs::rename(me, &away)?;
                let Err(e) = fs::rename(&temp, me) else {
                    return Ok(());
                };
                if fs::rename(&away, me).is_err() {
                    return Err(e); // no Launcher.exe: `swap_self` puts the spare back
                }
                tries -= 1;
                let busy = e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(32 | 33));
                if !busy || tries == 0 {
                    return Err(e);
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
    if swapped.is_err() {
        let _ = fs::remove_file(&temp);
    }
    swapped
}

/// The unzipped kit `top` over the kit folder `dir`, `replace` (the Launcher.exe swap) last; `work` (data\update) goes
/// when done.  Nothing is touched unless every listed file is there and whole; a failure on the way puts the old kit back.
fn install(top: &Path, dir: &Path, work: &Path, replace: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let write_failed = |p: &Path, e: io::Error| {
        tf(
            "launcher.msg_write_failed",
            &[("path", &p.display().to_string()), ("error", &e.to_string())],
        )
    };
    // data\ is the player's; Launcher.exe is swapped in by `replace` (`replace_running`)
    let skip = ["data", "Launcher.exe"];
    forza::apply::apply(top, dir, work, &skip, write_failed, replace, &|l| log(l)).map_err(|f| match f {
        Failed::Running(p) => close_game(&p),
        Failed::Damaged(bad) => tf("launcher.msg_update_files_missing", &[("files", &bad.join(", "))]),
        Failed::RolledBack(e) => tf("launcher.msg_update_rolled_back", &[("error", &e)]),
        Failed::Stuck(e) => e, // what could not go back is still in data\update\old
    })
}

/// At the start: the Launcher.exe.old an earlier update left, and the launchers an earlier kit had that this one replaced (a kit
/// unzipped by hand over an older one).
pub fn tidy(kit: &Kit) {
    if let Ok(me) = env::current_exe() {
        if let Some(e) = tidy_beside(&me) {
            log(&format!("update: cannot remove {e}"));
        }
        // the Launcher an update just replaced may still be closing: its renamed-away copy goes once it has exited
        if let Some(dir) = me.parent().map(Path::to_path_buf)
            && sweep_replaced(&dir, &me)
        {
            thread::spawn(move || {
                for _ in 0..30 {
                    thread::sleep(Duration::from_secs(1));
                    if !sweep_replaced(&dir, &me) {
                        return;
                    }
                }
                log(&format!(
                    "update: a replaced Launcher in {} is still in use, left for the next start",
                    dir.display()
                ));
            });
        }
    }
    if let Some(shipped) = manifest(&kit.dir().join("files.txt")) {
        let legacy: Vec<String> = LEGACY.iter().map(|&f| f.to_owned()).collect();
        for f in obsolete(&legacy, &shipped) {
            let _ = fs::remove_file(kit.dir().join(f));
        }
    }
}

/// The swap's leftovers, the `Launcher.exe.old` beside `me`, the running Launcher.exe, and the
/// `Launcher.exe.restore` a killed put-back left.  Returns why `Launcher.exe.old` could not go, unless a swap holds it.
fn tidy_beside(me: &Path) -> Option<io::Error> {
    if let Some(dir) = me.parent() {
        sweep_replaced(dir, me);
    }
    let mut old = me.as_os_str().to_owned();
    old.push(".old");
    let old = Path::new(&old);
    match fs::remove_file(old) {
        // held without delete sharing (a sharing violation): a swap runs in another Launcher ([`swap_self`]), and its
        // put-back may be writing the restore copy
        Err(e) if old.is_file() && matches!(e.raw_os_error(), Some(32 | 33)) => None,
        r => {
            let _ = fs::remove_file(me.with_extension("exe.restore"));
            let e = r.err().filter(|e| e.kind() != io::ErrorKind::NotFound)?;
            Some(io::Error::new(e.kind(), format!("{}: {e}", old.display())))
        }
    }
}

/// What a swap leaves next to Launcher.exe: `.Launcher.<pid>.__relocated__.exe`, the old Launcher.exe itself, and
/// `.__temp__.exe`, a new one a killed swap left ([`replace_running`]) once it is a minute old (a younger one may be
/// a swap in another Launcher, still writing it); and from earlier kits, which swapped with the
/// self-replace crate, `.Launcher.<random>.__selfdelete__.exe`, its helper that deleted the old Launcher.exe once it had
/// exited (Windows did not always let it delete itself: 33 MB a time piled up).  One still running is locked: its delete
/// fails, it stays and the result is true.
fn sweep_replaced(dir: &Path, me: &Path) -> bool {
    let Some(stem) = me.file_stem().and_then(OsStr::to_str) else {
        return false;
    };
    let mut stayed = false;
    let prefix = format!(".{}.", stem.to_ascii_lowercase());
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let left = [".__selfdelete__.exe", ".__relocated__.exe", ".__temp__.exe"]
            .iter()
            .any(|s| name.ends_with(s));
        let live = name.ends_with(".__temp__.exe")
            && !entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age >= Duration::from_secs(60)));
        if name.starts_with(&prefix) && left && !live && entry.path() != me {
            stayed |= fs::remove_file(entry.path()).is_err();
        }
    }
    stayed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_refused_resume_starts_over_and_other_failures_keep_the_part() {
        assert!(cannot_resume(Some(33)));
        // a cut connection (56), a timeout (28), an HTTP error (22) or no curl at all keeps the bytes for the next try
        for code in [Some(56), Some(28), Some(22), None] {
            assert!(!cannot_resume(code), "{code:?}");
        }
    }

    /// Modelled on the real v0.3.0 release list.
    const LIST: &str = r#"[
        {"tag_name": "v0.5.0", "prerelease": true, "draft": false, "published_at": "2026-11-01T10:00:00Z",
         "html_url": "https://github.com/x/releases/tag/v0.5.0", "body": "New game data.\n<!-- Needs-Installer -->",
         "assets": [{"name": "Forza-WCCF-2017-18-0.5.0.zip", "browser_download_url": "https://a/z5", "size": 9}]},
        {"tag_name": "v0.4.0-rc.1", "prerelease": true, "draft": false, "published_at": "2026-10-30T10:00:00Z",
         "html_url": "", "body": "",
         "assets": [{"name": "Forza-WCCF-2017-18-0.4.0-rc.1.zip", "browser_download_url": "https://a/rc"}]},
        {"tag_name": "v0.4.0", "draft": true,
         "assets": [{"name": "Forza-WCCF-2017-18-0.4.0.zip", "browser_download_url": "https://a/d"}]},
        {"tag_name": "v0.3.1", "prerelease": true, "published_at": "2026-10-25T10:00:00Z", "html_url": "",
         "assets": [{"name": "Forza-WCCF-2010-11-0.3.1.zip", "browser_download_url": "https://a/old"}]},
        {"tag_name": "v0.3.0", "prerelease": true, "draft": false, "published_at": "2026-10-23T15:18:17Z",
         "html_url": "https://github.com/x/releases/tag/v0.3.0", "body": "Fixes.",
         "assets": [{"name": "Forza-WCCF-Installer-0.3.0-windows-x64.zip", "browser_download_url": "https://a/inst"},
                    {"name": "Forza-WCCF-2017-18-0.3.0.zip.sha256", "browser_download_url": "https://a/sha"},
                    {"name": "Forza-WCCF-2017-18-0.3.0.zip", "browser_download_url": "https://a/z3", "size": 7,
                     "digest": "sha256:ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789"}]},
        {"tag_name": "v0.2.2", "assets": [{"name": "Forza-WCCF-2017-18-0.2.2.zip", "browser_download_url": "http://a/h"}]}
    ]"#;

    fn tags(list: &[Release]) -> Vec<&str> {
        list.iter().map(|r| r.tag.as_str()).collect()
    }

    #[test]
    fn edition_filter_takes_only_the_editions_zip() {
        let list = releases(LIST, "2017-18").unwrap();
        assert_eq!(tags(&list), ["v0.5.0", "v0.4.0-rc.1", "v0.3.0"]); // no draft, no 2010-11 release, no plain http
        let v3 = list.last().unwrap();
        assert_eq!(
            (v3.zip_url.as_str(), v3.zip_size, v3.date.as_str()),
            ("https://a/z3", 7, "2026-10-23")
        );
        assert_ne!(v3.zip_url, "https://a/inst"); // never the installer's ZIP
        assert_eq!(v3.sha_url.as_deref(), Some("https://a/sha"));
        let old = releases(LIST, "2010-11").unwrap();
        assert_eq!(tags(&old), ["v0.3.1"]);
        assert!(releases(r#"{"message": "API rate limit exceeded"}"#, "2017-18").is_err());
    }

    #[test]
    fn test_builds_and_installer_marker() {
        let list = releases(LIST, "2017-18").unwrap();
        // the prerelease flag is on every alpha release: only the tag's -rc part makes a test build
        assert_eq!(list.iter().map(|r| r.test).collect::<Vec<_>>(), [false, true, false]);
        assert_eq!(
            list.iter().map(|r| r.needs_installer).collect::<Vec<_>>(),
            [true, false, false]
        );
    }

    #[test]
    fn checksum_from_the_sha256_file_else_the_digest() {
        let hex = "ab".repeat(32);
        let file = format!("{}  Forza-WCCF-2017-18-0.3.0.zip\r\n", hex.to_ascii_uppercase());
        let digest = format!("sha256:{}", "cd".repeat(32));
        assert_eq!(expected_hash(Some(&file), Some(&digest)), Some(hex));
        assert_eq!(expected_hash(Some("nonsense"), Some(&digest)), Some("cd".repeat(32)));
        assert_eq!(expected_hash(None, Some(&digest)), Some("cd".repeat(32)));
        assert_eq!(expected_hash(None, Some("sha1:abc")), None);
        assert_eq!(expected_hash(None, None), None);
        let v3 = &releases(LIST, "2017-18").unwrap()[2];
        assert_eq!(
            expected_hash(None, v3.digest.as_deref()).unwrap(),
            v3.digest.as_ref().unwrap()[7..].to_ascii_lowercase()
        );
    }

    #[test]
    fn a_wrong_hash_is_refused() {
        let file = env::temp_dir().join(format!("wccf-verify-test-{}", std::process::id()));
        fs::write(&file, "abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify(&file, abc).is_ok());
        assert!(verify(&file, &"0".repeat(64)).is_err());
        fs::remove_file(&file).unwrap();
    }

    #[test]
    fn newest_is_the_first_newer_release_not_a_test_build() {
        let list = releases(LIST, "2017-18").unwrap(); // v0.5.0, v0.4.0-rc.1, v0.3.0
        assert_eq!(newest(&list, "kit-0.3.0").map(|r| r.tag.as_str()), Some("v0.5.0"));
        assert!(newest(&list, "kit-0.5.0").is_none()); // up to date: no notice
        assert!(newest(&list[1..], "kit-0.3.0").is_none()); // only a test build is newer
        assert!(newest(&[], "kit-0.3.0").is_none());
    }

    #[test]
    fn no_checksum_anywhere_is_none() {
        assert_eq!(expected_hash(None, None), None); // apply() refuses: unverified
    }

    #[test]
    fn invalid_target_finds_no_update() {
        assert!(launcher::release::invalid("https://updates.invalid/wccf/"));
        assert!(!launcher::release::invalid(launcher::release::UPDATE_URL));
    }

    /// A kit folder `kit` with a.dll "old" and files.txt, and an unzipped kit `top` with a.dll, bin\\b.exe "new" and its lists.
    fn kits(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let base = env::temp_dir().join(format!("wccf-install-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (kit, work) = (base.join("kit"), base.join("kit").join("data").join("update"));
        let top = work.join("kit").join("Forza-WCCF-2017-18");
        fs::create_dir_all(top.join("bin")).unwrap();
        fs::write(kit.join("a.dll"), "old").unwrap();
        fs::write(kit.join("files.txt"), "a.dll\r\nfiles.txt\r\n").unwrap();
        fs::write(top.join("a.dll"), "new").unwrap();
        fs::write(top.join("bin").join("b.exe"), "new").unwrap();
        fs::write(
            top.join("files.txt"),
            "\u{feff}a.dll\r\nbin\\b.exe\r\nfiles.sha256\r\nfiles.txt\r\n",
        )
        .unwrap();
        let new: String = Sha256::digest(b"new").iter().map(|b| format!("{b:02x}")).collect();
        fs::write(
            top.join("files.sha256"),
            format!("{new}  a.dll\r\n{new}  bin\\b.exe\r\n"),
        )
        .unwrap();
        (base, kit, work, top)
    }

    fn old_kit_unchanged(kit: &Path) {
        assert_eq!(fs::read_to_string(kit.join("a.dll")).unwrap(), "old");
        assert_eq!(
            fs::read_to_string(kit.join("files.txt")).unwrap(),
            "a.dll\r\nfiles.txt\r\n"
        );
        assert!(!kit.join("bin").join("b.exe").exists());
    }

    #[test]
    fn a_missing_or_changed_file_refuses_the_update() {
        let (base, kit, work, top) = kits("missing");
        fs::remove_file(top.join("bin").join("b.exe")).unwrap(); // the antivirus took it
        fs::write(top.join("a.dll"), "eaten").unwrap();
        let e = install(&top, &kit, &work, || Ok(())).unwrap_err();
        assert!(e.contains("a.dll") && e.contains("bin\\b.exe"), "{e}");
        old_kit_unchanged(&kit);
        assert!(!work.exists());
        // files.sha256 without a line for a file files.txt lists: that file is not checked, so it is refused
        let (base3, _, _, top3) = kits("unhashed");
        let new: String = Sha256::digest(b"new").iter().map(|b| format!("{b:02x}")).collect();
        fs::write(top3.join("files.sha256"), format!("{new}  a.dll\r\n")).unwrap();
        assert_eq!(forza::apply::damaged(&top3), ["bin\\b.exe"]);
        fs::remove_dir_all(&base3).unwrap();
        // an older kit, files.txt only: a file it lists that is not there is enough
        let (base2, kit2, work2, top2) = kits("missing-old");
        fs::remove_file(top2.join("files.sha256")).unwrap();
        fs::remove_file(top2.join("bin").join("b.exe")).unwrap();
        assert_eq!(forza::apply::damaged(&top2), ["bin\\b.exe", "files.sha256"]);
        assert!(install(&top2, &kit2, &work2, || Ok(())).is_err());
        old_kit_unchanged(&kit2);
        fs::remove_dir_all(&base).unwrap();
        fs::remove_dir_all(&base2).unwrap();
    }

    #[test]
    fn a_failure_on_the_way_puts_the_old_kit_back() {
        let (base, kit, work, top) = kits("rollback");
        // the Launcher.exe swap fails after every file was copied
        let e = install(&top, &kit, &work, || Err("locked".into())).unwrap_err();
        assert!(e.contains("locked"), "{e}");
        old_kit_unchanged(&kit);
        assert!(!work.exists());
        // a copy that fails half way: a folder where bin\\b.exe must go (a.dll is already written)
        let (base2, kit2, work2, top2) = kits("rollback-mid");
        fs::create_dir_all(kit2.join("bin").join("b.exe")).unwrap();
        assert!(install(&top2, &kit2, &work2, || Ok(())).is_err());
        assert_eq!(fs::read_to_string(kit2.join("a.dll")).unwrap(), "old");
        assert_eq!(
            fs::read_to_string(kit2.join("files.txt")).unwrap(),
            "a.dll\r\nfiles.txt\r\n"
        );
        fs::remove_dir_all(&base).unwrap();
        fs::remove_dir_all(&base2).unwrap();
    }

    #[test]
    fn a_whole_kit_is_installed() {
        let (base, kit, work, top) = kits("whole");
        install(&top, &kit, &work, || Ok(())).unwrap();
        assert_eq!(fs::read_to_string(kit.join("a.dll")).unwrap(), "new");
        assert_eq!(fs::read_to_string(kit.join("bin").join("b.exe")).unwrap(), "new");
        assert!(
            fs::read_to_string(kit.join("files.txt"))
                .unwrap()
                .contains("files.sha256")
        );
        assert!(!work.exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn self_replace_leftovers_go_a_locked_one_stays() {
        let dir = env::temp_dir().join(format!("wccf-sweep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let me = dir.join("Launcher.exe");
        let names = [
            ".Launcher.abc.__selfdelete__.exe",
            ".Launcher.def.__relocated__.exe",
            ".Launcher.ghi.__selfdelete__.exe",
            ".Other.abc.__selfdelete__.exe",
            "Launcher.exe",
        ];
        for n in names {
            fs::write(dir.join(n), "x").unwrap();
        }
        #[cfg(windows)]
        let _lock = {
            use std::os::windows::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .read(true)
                .share_mode(1) // FILE_SHARE_READ only: no delete while open, as a running helper
                .open(dir.join(names[2]))
                .unwrap()
        };
        assert_eq!(sweep_replaced(&dir, &me), cfg!(windows), "a locked one stayed");
        let left = |n: &str| dir.join(n).exists();
        assert!(!left(names[0]) && !left(names[1]));
        assert_eq!(left(names[2]), cfg!(windows));
        assert!(left(names[3]) && left(names[4]));
        #[cfg(windows)]
        drop(_lock);
        assert!(
            !sweep_replaced(&dir, &me),
            "it goes once unlocked: the start's retry ends"
        );
        assert!(!left(names[2]));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A swap that has moved the running Launcher.exe aside and then fails puts the kept copy back.
    #[test]
    fn a_failed_swap_puts_the_launcher_back() {
        let t = crate::scratch();
        let (me, kept) = (
            t.path().join("Launcher.exe"),
            t.path().join("data").join("update").join("Launcher.exe"),
        );
        fs::create_dir_all(kept.parent().unwrap()).unwrap();
        fs::write(&me, b"the running launcher").unwrap();
        fs::write(&kept, b"last update's copy").unwrap();
        let e = swap_self(&me, &kept, || {
            fs::remove_file(&me)?; // the swap renames it away first
            Err(io::Error::other("the kit folder is locked"))
        })
        .unwrap_err();
        assert_eq!(
            (e.0, e.1.to_string().as_str()),
            (me.as_path(), "the kit folder is locked")
        );
        assert_eq!(fs::read(&me).unwrap(), b"the running launcher");
    }

    /// When the kept copy cannot go back either, both errors are reported and the copy stays: it is then the only
    /// Launcher.exe left.
    #[test]
    fn a_failed_swap_that_cannot_go_back_reports_both_and_keeps_the_copy() {
        let t = crate::scratch();
        let (me, kept) = (t.path().join("Launcher.exe"), t.path().join("Launcher.exe.old"));
        fs::write(&me, b"the running launcher").unwrap();
        let e = swap_self(&me, &kept, || {
            fs::remove_file(&me)?; // the swap renames it away first
            fs::create_dir(&me)?; // and it will not take the copy back
            Err(io::Error::other("the kit folder is locked"))
        })
        .unwrap_err();
        let e = e.1.to_string();
        assert!(
            e.contains("the kit folder is locked") && e.contains("Launcher.exe.old"),
            "{e}"
        );
        assert_eq!(fs::read(&kept).unwrap(), b"the running launcher");
        // the update is still offered: a second try must not empty the only copy left
        swap_self(&me, &kept, || Err(io::Error::other("the kit folder is locked"))).unwrap_err();
        assert_eq!(fs::read(&kept).unwrap(), b"the running launcher");
        assert!(!t.path().join("Launcher.exe.restore").exists());
    }

    /// A spare that cannot be written is named, not Launcher.exe (#361); the swap is not tried.
    #[test]
    fn a_spare_that_cannot_be_written_is_the_file_named() {
        let t = crate::scratch();
        let (me, kept) = (t.path().join("Launcher.exe"), t.path().join("Launcher.exe.old"));
        fs::write(&me, b"the running launcher").unwrap();
        fs::create_dir(&kept).unwrap(); // it will not open as a file
        let e = swap_self(&me, &kept, || panic!("no swap without a spare")).unwrap_err();
        assert_eq!(e.0, kept);
        assert_eq!(fs::read(&me).unwrap(), b"the running launcher");
    }

    /// A try after a put-back that failed finds no Launcher.exe: the error names the spare, the only copy left (#361).
    #[test]
    fn a_retry_with_no_launcher_names_the_spare_left() {
        let t = crate::scratch();
        let (me, kept) = (t.path().join("Launcher.exe"), t.path().join("Launcher.exe.old"));
        fs::write(&kept, b"the only copy").unwrap();
        let e = swap_self(&me, &kept, || panic!("no swap without a Launcher.exe")).unwrap_err();
        assert_eq!(e.0, me);
        assert!(e.1.to_string().contains(&kept.display().to_string()), "{}", e.1);
        assert_eq!(fs::read(&kept).unwrap(), b"the only copy");
    }

    /// A scanner holding the fresh restore copy for a moment (no delete sharing) does not fail the put-back (#361).
    #[test]
    fn a_put_back_waits_for_a_scanner_on_the_fresh_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = crate::scratch();
        let (me, kept) = (t.path().join("Launcher.exe"), t.path().join("Launcher.exe.old"));
        let restore = t.path().join("Launcher.exe.restore");
        fs::write(&me, b"the running launcher").unwrap();
        let scanner = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(3) // FILE_SHARE_READ | FILE_SHARE_WRITE: the copy is written, the rename waits
            .open(&restore)
            .unwrap();
        let done = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            drop(scanner);
        });
        let e = swap_self(&me, &kept, || {
            fs::remove_file(&me)?; // the swap renames it away first
            Err(io::Error::other("the kit folder is locked"))
        })
        .unwrap_err();
        done.join().unwrap();
        assert_eq!(
            e.1.to_string(),
            "the kit folder is locked",
            "the put-back itself worked"
        );
        assert_eq!(fs::read(&me).unwrap(), b"the running launcher");
        assert!(!restore.exists() && !kept.exists());
    }

    /// The start removes a `Launcher.exe.restore` a killed put-back left, but not while another Launcher's swap holds
    /// the spare: its put-back may be writing that file (#361).
    #[test]
    fn the_start_removes_a_stray_restore_unless_a_swap_runs() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = crate::scratch();
        let (me, kept) = (t.path().join("Launcher.exe"), t.path().join("Launcher.exe.old"));
        let restore = t.path().join("Launcher.exe.restore");
        fs::write(&me, b"the running launcher").unwrap();
        fs::write(&restore, b"half a launcher").unwrap();
        let held = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(1) // as swap_self holds it
            .open(&kept)
            .unwrap();
        assert!(tidy_beside(&me).is_none(), "a held spare is not a failure");
        assert!(restore.exists() && kept.exists(), "a swap runs: both stay");
        drop(held);
        assert!(tidy_beside(&me).is_none());
        assert!(!restore.exists() && !kept.exists());
        assert_eq!(fs::read(&me).unwrap(), b"the running launcher");
    }

    /// A `Launcher.exe.old` that can never go (here a folder; a read-only file or an ACL alike) is no swap: the stray
    /// restore copy still goes, and the failure is returned for the log (#361).
    #[test]
    fn a_spare_that_cannot_go_does_not_keep_a_stray_restore() {
        let t = crate::scratch();
        let (me, kept) = (t.path().join("Launcher.exe"), t.path().join("Launcher.exe.old"));
        let restore = t.path().join("Launcher.exe.restore");
        fs::write(&me, b"the running launcher").unwrap();
        fs::write(&restore, b"half a launcher").unwrap();
        fs::create_dir(&kept).unwrap();
        let e = tidy_beside(&me).expect("the failure on Launcher.exe.old");
        assert!(e.to_string().contains("Launcher.exe.old"), "{e}");
        assert!(!restore.exists());
    }

    /// A second Launcher started between the copy and the swap runs its start's tidy: the kept copy must survive it,
    /// or a swap that then fails leaves the kit folder with no Launcher.exe (#361).
    #[test]
    fn a_second_launcher_starting_during_the_swap_leaves_the_kept_copy() {
        let t = crate::scratch();
        let (me, kept) = (t.path().join("Launcher.exe"), t.path().join("Launcher.exe.old"));
        fs::write(&me, b"the running launcher").unwrap();
        swap_self(&me, &kept, || {
            tidy_beside(&me); // the second Launcher's start
            fs::remove_file(&me)?; // the swap renames it away first
            Err(io::Error::other("the kit folder is locked"))
        })
        .unwrap_err();
        assert_eq!(fs::read(&me).unwrap(), b"the running launcher");
        assert!(!kept.exists());
    }

    /// A swap that works leaves the new Launcher.exe and no spare; a `Launcher.exe.old` left anyway (the Launcher was
    /// killed during the swap) goes at the next start.
    #[test]
    fn a_swap_replaces_the_launcher_and_the_next_start_removes_the_spare() {
        let t = crate::scratch();
        let (me, kept) = (t.path().join("Launcher.exe"), t.path().join("Launcher.exe.old"));
        fs::write(&me, b"the running launcher").unwrap();
        swap_self(&me, &kept, || {
            fs::rename(&me, t.path().join(".Launcher.abc.__relocated__.exe"))?;
            fs::write(&me, b"the new launcher")
        })
        .unwrap();
        assert_eq!(fs::read(&me).unwrap(), b"the new launcher");
        assert!(!kept.exists());
        fs::write(&kept, b"a spare").unwrap();
        tidy_beside(&me);
        assert!(!kept.exists());
        assert!(!t.path().join(".Launcher.abc.__relocated__.exe").exists());
        assert_eq!(fs::read(&me).unwrap(), b"the new launcher");
    }

    /// The swap puts the new Launcher.exe in place and the running one aside under a name the next start's tidy
    /// removes; no copy is left half way and the spare goes (#361).
    #[test]
    fn the_swap_renames_the_new_launcher_in_and_the_next_start_removes_the_old() {
        let t = crate::scratch();
        let (me, kept, new) = (
            t.path().join("Launcher.exe"),
            t.path().join("Launcher.exe.old"),
            t.path().join("unzipped.exe"),
        );
        fs::write(&me, b"the running launcher").unwrap();
        fs::write(&new, b"the new launcher").unwrap();
        swap_self(&me, &kept, || replace_running(&me, &new)).unwrap();
        let away = left_by_swap(t.path(), "__relocated__.exe");
        let temp = left_by_swap(t.path(), "__temp__.exe");
        assert_eq!(fs::read(&me).unwrap(), b"the new launcher");
        assert_eq!(fs::read(&away).unwrap(), b"the running launcher");
        assert!(!temp.exists() && !kept.exists());
        assert!(tidy_beside(&me).is_none());
        assert!(!away.exists());
        assert_eq!(fs::read(&me).unwrap(), b"the new launcher");
    }

    /// A second rename that fails (a scanner holds the fresh copy past the retries) renames the running Launcher.exe
    /// back: the folder keeps it, no put-back from the spare is needed.  The copy was whole before Launcher.exe moved:
    /// the window with no Launcher.exe is the two renames only (self_replace copied the new one after the move, #361).
    #[test]
    fn a_failing_second_rename_renames_the_old_launcher_back() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = crate::scratch();
        let (me, kept, new) = (
            t.path().join("Launcher.exe"),
            t.path().join("Launcher.exe.old"),
            t.path().join("unzipped.exe"),
        );
        let temp = left_by_swap(t.path(), "__temp__.exe");
        fs::write(&me, b"the running launcher").unwrap();
        fs::write(&new, b"the new launcher").unwrap();
        let mut scanner = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(3) // FILE_SHARE_READ | FILE_SHARE_WRITE: the copy is written, no rename while held
            .open(&temp)
            .unwrap();
        let e = swap_self(&me, &kept, || replace_running(&me, &new)).unwrap_err();
        assert_eq!(
            (e.0, e.1.raw_os_error()),
            (me.as_path(), Some(32)),
            "a sharing violation"
        );
        let mut seen = Vec::new();
        scanner.read_to_end(&mut seen).unwrap();
        assert_eq!(seen, b"the new launcher", "written and whole before the swap");
        drop(scanner);
        assert_eq!(fs::read(&me).unwrap(), b"the running launcher");
        assert!(!kept.exists());
        let away = left_by_swap(t.path(), "__relocated__.exe");
        assert!(!away.exists());
        tidy_beside(&me);
        assert!(temp.exists(), "a fresh temp may be another swap's: it stays");
        age(&temp);
        tidy_beside(&me); // the temp the held delete left goes at a start a minute later
        assert!(!temp.exists());
    }

    /// A scanner holding the fresh copy for a moment does not fail the swap, and while the pair is tried again the
    /// folder keeps its Launcher.exe: the old one goes back between tries (#361).
    #[test]
    fn a_scanner_on_the_fresh_copy_keeps_launcher_exe_in_place_between_tries() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = crate::scratch();
        let (me, new) = (t.path().join("Launcher.exe"), t.path().join("unzipped.exe"));
        fs::write(&me, b"the running launcher").unwrap();
        fs::write(&new, b"the new launcher").unwrap();
        let scanner = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(3) // no delete sharing: the renames wait
            .open(left_by_swap(t.path(), "__temp__.exe"))
            .unwrap();
        let swap = {
            let (me, new) = (me.clone(), new.clone());
            thread::spawn(move || replace_running(&me, &new))
        };
        thread::sleep(Duration::from_millis(300)); // a few tries in
        // three looks: one may fall into the two renames of a try, not all three
        let present = (0..3).any(|_| {
            thread::sleep(Duration::from_millis(7));
            me.is_file()
        });
        drop(scanner);
        swap.join().unwrap().unwrap();
        assert!(present, "no Launcher.exe while the swap waited on the scanner");
        assert_eq!(fs::read(&me).unwrap(), b"the new launcher");
        assert!(!left_by_swap(t.path(), "__temp__.exe").exists());
    }

    /// The start's sweep leaves a `.__temp__.exe` under a minute old (another Launcher's swap may be writing it) and
    /// removes an older one a killed swap left (#361).
    #[test]
    fn the_sweep_leaves_a_fresh_temp_and_removes_an_old_one() {
        let t = crate::scratch();
        let me = t.path().join("Launcher.exe");
        let temp = t.path().join(".Launcher.7.__temp__.exe");
        fs::write(&me, b"the running launcher").unwrap();
        fs::write(&temp, b"half a launcher").unwrap();
        assert!(!sweep_replaced(t.path(), &me), "a fresh one is skipped, not stuck");
        assert!(temp.exists());
        age(&temp);
        sweep_replaced(t.path(), &me);
        assert!(!temp.exists() && me.exists());
    }

    /// A Launcher.exe that cannot be moved away leaves it as it was and no temp copy.
    #[test]
    fn a_launcher_that_cannot_move_away_stays_and_the_copy_goes() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = crate::scratch();
        let (me, new) = (t.path().join("Launcher.exe"), t.path().join("unzipped.exe"));
        fs::write(&me, b"the running launcher").unwrap();
        fs::write(&new, b"the new launcher").unwrap();
        let held = fs::OpenOptions::new().read(true).share_mode(1).open(&me).unwrap();
        let e = replace_running(&me, &new).unwrap_err();
        drop(held);
        assert_eq!(e.raw_os_error(), Some(32), "the move away was refused: {e}");
        assert_eq!(fs::read(&me).unwrap(), b"the running launcher");
        assert!(!left_by_swap(t.path(), "__temp__.exe").exists());
        assert!(!left_by_swap(t.path(), "__relocated__.exe").exists());
        let names: Vec<_> = fs::read_dir(t.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    /// A new Launcher missing from the unzipped kit fails before Launcher.exe is touched and leaves no temp copy.
    #[test]
    fn a_missing_new_launcher_fails_before_the_running_one_moves() {
        let t = crate::scratch();
        let me = t.path().join("Launcher.exe");
        fs::write(&me, b"the running launcher").unwrap();
        let e = replace_running(&me, &t.path().join("unzipped.exe")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(fs::read(&me).unwrap(), b"the running launcher");
        assert!(!left_by_swap(t.path(), "__temp__.exe").exists());
        assert!(!left_by_swap(t.path(), "__relocated__.exe").exists());
    }

    /// `p` as a file last written two minutes ago.
    fn age(p: &Path) {
        let then = std::time::SystemTime::now() - Duration::from_secs(120);
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(then)
            .unwrap();
    }

    /// The name `replace_running` gives its copies beside Launcher.exe in `dir`.
    fn left_by_swap(dir: &Path, end: &str) -> PathBuf {
        dir.join(format!(".Launcher.{}.{end}", std::process::id()))
    }
}
