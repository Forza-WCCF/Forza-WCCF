//! UPDATE: the kit's releases and test builds (pre-releases).  The chosen one's kit ZIP is downloaded,
//! unzipped into data\update and copied over the kit folder - data\ (club cards, keys, settings, logs) is never
//! written, and nothing is copied while the game or a kit tool runs.  Files the old kit shipped (files.txt) that the
//! new one does not are removed; then setup runs again, and English if on.
//!
//! The download and the unzip are Windows' own curl.exe and tar.exe (in System32 since Windows 10 1803): HTTPS with
//! Windows' certificates, and tar refuses a ZIP entry that would land outside its folder (an absolute path or "..").

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use std::{env, fs, thread};

use launcher::Kit;
use launcher::release::{UPDATE_URL, curl, no_target, system32};
use serde_json::Value;

use crate::text::{fill, t};
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
    zip_url: String,
    zip_size: u64,
}

/// The releases that have a kit ZIP, newest first (as the target lists them, in GitHub's shape).
fn releases(json: &str) -> Result<Vec<Release>, String> {
    let list: Vec<Value> = serde_json::from_str(json).map_err(|e| format!("not a list of releases ({e})"))?;
    let zip = |a: &&Value| {
        a["name"]
            .as_str()
            .is_some_and(|n| n.to_ascii_lowercase().ends_with(".zip"))
    };
    Ok(list
        .iter()
        .filter(|r| r["draft"].as_bool() != Some(true))
        .filter_map(|r| {
            let asset = r["assets"].as_array()?.iter().find(zip)?;
            let url = asset["browser_download_url"].as_str()?;
            Some(Release {
                tag: r["tag_name"].as_str()?.to_owned(),
                test: r["prerelease"].as_bool() == Some(true),
                date: r["published_at"].as_str().unwrap_or("").chars().take(10).collect(),
                page: r["html_url"].as_str().unwrap_or("").to_owned(),
                zip_url: url.starts_with("https://").then(|| url.to_owned())?,
                zip_size: asset["size"].as_u64().unwrap_or(0),
            })
        })
        .collect())
}

pub fn fetch() -> Result<Vec<Release>, String> {
    if no_target() {
        return Ok(Vec::new());
    }
    let api = format!("{UPDATE_URL}releases?per_page=30");
    let out = curl()
        .args(["-H", "Accept: application/vnd.github+json", "--url", &api])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    releases(&String::from_utf8_lossy(&out.stdout))
}

fn running_check(kit: &Kit) -> Result<(), String> {
    match launcher::kit_program_running(kit) {
        Some(p) => {
            let name = p.file_name().map_or(p.as_os_str(), OsStr::new).display().to_string();
            Err(fill(t().close_game, &[&name]))
        }
        None => Ok(()),
    }
}

/// Download, unzip, copy over the kit folder, remove what the new kit no longer ships.
pub fn apply(kit: &Kit, r: &Release) -> Result<(), String> {
    running_check(kit)?;
    let dir = kit.dir();
    let work = dir.join("data").join("update");
    let unzipped = work.join("kit");
    let zip = work.join("kit.zip");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&unzipped).map_err(|e| format!("{}: {e}", work.display()))?;

    log(&fill(t().downloading, &[&r.tag]));
    let mut child = curl()
        .arg("-o")
        .arg(&zip)
        .args(["--url", &r.zip_url])
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| fill(t().download_failed, &[&e.to_string()]))?;
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        let got = fs::metadata(&zip).map_or(0, |m| m.len());
        let mb = |b: u64| b as f64 / 1_048_576.0;
        say(
            format!(
                "{}  {:.1} / {:.1} MB",
                fill(t().downloading, &[&r.tag]),
                mb(got),
                mb(r.zip_size)
            ),
            (got.saturating_mul(900) / r.zip_size.max(1)).min(900) as u32,
        );
        thread::sleep(Duration::from_millis(200));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(fill(
            t().download_failed,
            &[String::from_utf8_lossy(&out.stderr).trim()],
        ));
    }

    log(t().unzipping);
    say(t().unzipping, 920);
    let out = system32("tar.exe")
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&unzipped)
        .output()
        .map_err(|e| fill(t().unzip_failed, &[&e.to_string()]))?;
    if !out.status.success() {
        return Err(fill(t().unzip_failed, &[String::from_utf8_lossy(&out.stderr).trim()]));
    }
    // the kit ZIP holds one folder (WCCF-2010-11-kit\) with the kit in it
    let top = fs::read_dir(&unzipped)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("Launcher.exe").is_file())
        .ok_or(t().not_a_kit)?;

    running_check(kit)?; // the game may have started during the download
    log(&fill(t().copying, &[&r.tag]));
    say(fill(t().copying, &[&r.tag]), 950);
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
    let _ = fs::remove_dir_all(&work);
    Ok(())
}

/// Every file of `from` over `to`; the kit folder's own data\ is left out.
fn copy_over(from: &Path, to: &Path, top: bool) -> Result<(), String> {
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if top && name.eq_ignore_ascii_case("data") {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        let dir = entry.file_type().is_ok_and(|t| t.is_dir());
        let written = if dir {
            fs::create_dir_all(&dst)
        } else {
            replace(&src, &dst)
        };
        written.map_err(|e| fill(t().write_failed, &[&dst.display().to_string(), &e.to_string()]))?;
        if dir {
            copy_over(&src, &dst, false)?;
        }
    }
    Ok(())
}

/// A program that runs (this Launcher.exe) cannot be written over, but it can be moved aside: Launcher.exe.old, deleted at
/// the next start.
fn replace(src: &Path, dst: &Path) -> std::io::Result<()> {
    match fs::copy(src, dst) {
        Ok(_) => Ok(()),
        Err(e) if env::current_exe().is_ok_and(|me| me == dst) => {
            let old = old_exe(dst);
            let _ = fs::remove_file(&old);
            fs::rename(dst, &old).map_err(|_| e)?;
            fs::copy(src, dst).map(drop)
        }
        Err(e) => Err(e),
    }
}

pub fn old_exe(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".old");
    name.into()
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

/// At the start: the Launcher.exe.old an update left, and the launchers an earlier kit had that this one replaced (a kit
/// unzipped by hand over an older one).
pub fn tidy(kit: &Kit) {
    if let Ok(me) = env::current_exe() {
        let _ = fs::remove_file(old_exe(&me));
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

    #[test]
    fn releases_with_a_kit_zip() {
        let json = r#"[
            {"tag_name": "kit-5.5-test1", "prerelease": true, "draft": false, "published_at": "2026-10-09T15:18:17Z",
             "html_url": "https://github.com/x/releases/tag/kit-5.5-test1",
             "assets": [{"name": "WCCF-2010-11-kit-5.5-test1.zip.sha256", "browser_download_url": "https://a/s"},
                        {"name": "WCCF-2010-11-kit-5.5-test1.ZIP", "browser_download_url": "https://a/z", "size": 7}]},
            {"tag_name": "kit-5.4", "prerelease": false, "published_at": "2026-10-09T12:06:08Z", "html_url": "",
             "assets": [{"name": "k.zip", "browser_download_url": "https://a/k"}]},
            {"tag_name": "notes-only", "assets": []},
            {"tag_name": "draft", "draft": true, "assets": [{"name": "d.zip", "browser_download_url": "https://a/d"}]},
            {"tag_name": "plain-http", "assets": [{"name": "h.zip", "browser_download_url": "http://a/h"}]}
        ]"#;
        let list = releases(json).unwrap();
        assert_eq!(
            list.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(),
            ["kit-5.5-test1", "kit-5.4"]
        );
        assert!(list[0].test && !list[1].test);
        assert_eq!(
            (list[0].zip_url.as_str(), list[0].zip_size, list[0].date.as_str()),
            ("https://a/z", 7, "2026-10-09")
        );
        assert!(releases(r#"{"message": "API rate limit exceeded"}"#).is_err());
    }

    #[test]
    fn placeholder_finds_no_update() {
        assert!(no_target());
        assert!(fetch().is_ok_and(|list| list.is_empty())); // the window then says t().none_listed, not an error
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
        let new: Vec<String> = ["play.exe"].map(String::from).into();
        assert_eq!(obsolete(&old, &new), ["ENGLISH.exe", "SETUP.exe", "scripts\\old.py"]);
    }
}
