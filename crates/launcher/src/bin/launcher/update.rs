//! UPDATE: the kit's releases and test builds (pre-releases).  The chosen one's kit ZIP is downloaded,
//! unzipped into data\update and copied over the kit folder - data\ (club cards, keys, settings, logs) is never
//! written, and nothing is copied while the game or a kit tool runs.  Files the old kit shipped (files.txt) that the
//! new one does not are removed; then setup runs again, and English if on.
//!
//! The download and the unzip are Windows' own curl.exe and tar.exe (in System32 since Windows 10 1803): HTTPS with
//! Windows' certificates, and tar refuses a ZIP entry that would land outside its folder (an absolute path or "..").

use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use std::{env, fs, thread};

use launcher::Kit;
use launcher::release::{curl, no_target, system32, update_url};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::text::{t, tf};
use crate::{log, say};

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

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub test: bool,
    pub date: String,
    pub page: String,
    /// The release notes carry the marker `needs-installer`: the game data changed, UPDATE copies nothing.
    pub needs_installer: bool,
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
fn verify(file: &Path, expected: &str) -> Result<(), String> {
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
        Some(p) => {
            let name = p.file_name().map_or(p.as_os_str(), OsStr::new).display().to_string();
            Err(tf("launcher.msg_close_game", &[("name", &name)]))
        }
        None => Ok(()),
    }
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

    log(&tf("launcher.msg_downloading", &[("tag", &r.tag)]));
    let mut child = curl()
        .arg("-o")
        .arg(&zip)
        .args(["--url", &r.zip_url])
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| tf("launcher.msg_download_failed", &[("error", &e.to_string())]))?;
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        let got = fs::metadata(&zip).map_or(0, |m| m.len());
        let mb = |b: u64| b as f64 / 1_048_576.0;
        say(
            format!(
                "{}  {:.1} / {:.1} MB",
                tf("launcher.msg_downloading", &[("tag", &r.tag)]),
                mb(got),
                mb(r.zip_size)
            ),
            (got.saturating_mul(900) / r.zip_size.max(1)).min(900) as u32,
        );
        thread::sleep(Duration::from_millis(200));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(tf(
            "launcher.msg_download_failed",
            &[("error", String::from_utf8_lossy(&out.stderr).trim())],
        ));
    }

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
        .map_err(|e| tf("launcher.msg_unzip_failed", &[("error", &e.to_string())]))?;
    if !out.status.success() {
        return Err(tf(
            "launcher.msg_unzip_failed",
            &[("error", String::from_utf8_lossy(&out.stderr).trim())],
        ));
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
    let mut old = manifest(&dir.join("files.txt")).unwrap_or_default();
    old.extend(LEGACY.iter().map(|&f| f.to_owned()));
    let new = manifest(&top.join("files.txt"));
    // an older kit brings neither: the newer kit's must not stay behind and speak for it
    for f in ["files.txt", "version.txt"] {
        let _ = fs::remove_file(dir.join(f));
    }
    copy_over(&top, dir, true)?;
    // without the new kit's list nothing is known to be gone: nothing is removed
    if let Some(new) = new {
        for f in obsolete(&old, &new) {
            if fs::remove_file(dir.join(f)).is_ok() {
                log(&format!("  removed {f}"));
            }
        }
    }
    // this running Launcher.exe last: self-replace swaps it for the new one, which the next start runs
    self_replace::self_replace(top.join("Launcher.exe")).map_err(|e| {
        tf(
            "launcher.msg_write_failed",
            &[("path", "Launcher.exe"), ("error", &e.to_string())],
        )
    })?;
    let _ = fs::remove_dir_all(&work);
    Ok(())
}

/// Every file of `from` over `to`; the kit folder's own data\ and the Launcher.exe at the top are left out.
fn copy_over(from: &Path, to: &Path, top: bool) -> Result<(), String> {
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if top && (name.eq_ignore_ascii_case("data") || name.eq_ignore_ascii_case("Launcher.exe")) {
            continue; // data\ is the player's; Launcher.exe is swapped in by self_replace
        }
        let (src, dst) = (entry.path(), to.join(&name));
        let dir = entry.file_type().is_ok_and(|t| t.is_dir());
        let written = if dir {
            fs::create_dir_all(&dst)
        } else {
            fs::copy(&src, &dst).map(drop)
        };
        written.map_err(|e| {
            tf(
                "launcher.msg_write_failed",
                &[("path", &dst.display().to_string()), ("error", &e.to_string())],
            )
        })?;
        if dir {
            copy_over(&src, &dst, false)?;
        }
    }
    Ok(())
}

/// files.txt: every file a kit ships, one path under the kit folder per line (source\package.ps1 writes it).
fn manifest(path: &Path) -> Option<Vec<String>> {
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
fn obsolete<'a>(old: &'a [String], new: &[String]) -> Vec<&'a str> {
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

/// At the start: the Launcher.exe.old an earlier update left, and the launchers an earlier kit had that this one replaced (a kit
/// unzipped by hand over an older one).
pub fn tidy(kit: &Kit) {
    if let Ok(mut me) = env::current_exe().map(PathBuf::into_os_string) {
        me.push(".old");
        let _ = fs::remove_file(me);
    }
    if let Some(shipped) = manifest(&kit.dir().join("files.txt")) {
        let legacy: Vec<String> = LEGACY.iter().map(|&f| f.to_owned()).collect();
        for f in obsolete(&legacy, &shipped) {
            let _ = fs::remove_file(kit.dir().join(f));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn invalid_target_finds_no_update() {
        assert!(launcher::release::invalid("https://updates.invalid/wccf/"));
        assert!(!launcher::release::invalid(launcher::release::UPDATE_URL));
    }

    #[test]
    fn copy_over_keeps_data() {
        let base = env::temp_dir().join(format!("wccf-update-test-{}", std::process::id()));
        let (from, to) = (base.join("zip"), base.join("kit"));
        fs::create_dir_all(from.join("data")).unwrap();
        fs::create_dir_all(from.join("bin")).unwrap();
        fs::create_dir_all(to.join("data")).unwrap();
        fs::write(from.join("data").join("keys.txt"), "new").unwrap();
        fs::write(from.join("bin").join("a.dll"), "new").unwrap();
        fs::write(to.join("data").join("keys.txt"), "mine").unwrap();
        copy_over(&from, &to, true).unwrap();
        assert_eq!(fs::read_to_string(to.join("data").join("keys.txt")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(to.join("bin").join("a.dll")).unwrap(), "new");
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
        assert_eq!(obsolete(&old, &new), ["ENGLISH.exe", "SETUP.exe", "scripts\\old.py"]);
    }
}
