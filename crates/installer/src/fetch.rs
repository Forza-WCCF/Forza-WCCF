//! The newest 2017-18 package from the project's GitHub releases: pick the release, download over HTTPS with Windows' own
//! curl (the Launcher's UPDATE does the same, `forza::curl`), check the SHA-256 and refuse a mismatch.
use crate::err::InstallError as E;
use anyhow::{Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

pub const API: &str = "https://api.github.com/repos/Forza-WCCF/Forza-WCCF/releases?per_page=30";
/// Only the 2017-18 edition publishes a package.
pub const PREFIX_1718: &str = "Forza-WCCF-2017-18-";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    /// "0.2.0": the zip's name between the prefix and ".zip".
    pub version: String,
    pub zip_name: String,
    pub zip_url: String,
    pub zip_size: u64,
    pub sum_url: String,
    /// The checksum asset is a SHA256SUMS.txt list, not `<zip>.sha256`.
    pub sum_list: bool,
}

fn https(a: &Value) -> Option<String> {
    let u = a["browser_download_url"].as_str()?;
    u.starts_with("https://").then(|| u.to_owned())
}

/// The newest non-draft release (pre-releases included) that has `<prefix>*.zip` and its checksum.
pub fn pick(json: &str, prefix: &str) -> Result<Package> {
    let list: Vec<Value> = serde_json::from_str(json).map_err(|e| E::Download {
        detail: format!("not a list of releases ({e})"),
    })?;
    let mut best: Option<(&str, Package)> = None;
    for r in list.iter().filter(|r| r["draft"].as_bool() != Some(true)) {
        let assets = r["assets"].as_array().map_or(&[][..], |a| a);
        let name = |a: &Value| a["name"].as_str().unwrap_or("").to_owned();
        let Some(zip) = assets.iter().find(|a| {
            let n = name(a);
            n.starts_with(prefix) && n.ends_with(".zip")
        }) else {
            continue;
        };
        let zip_name = name(zip);
        let sum = assets
            .iter()
            .find(|a| name(a) == format!("{zip_name}.sha256"))
            .map(|a| (a, false))
            .or_else(|| {
                assets
                    .iter()
                    .find(|a| name(a) == "SHA256SUMS.txt")
                    .map(|a| (a, true))
            });
        let (Some(zip_url), Some((sum, sum_list))) = (https(zip), sum) else {
            continue;
        };
        let Some(sum_url) = https(sum) else { continue };
        let at = r["published_at"].as_str().unwrap_or("");
        if best.as_ref().is_none_or(|(b, _)| at > *b) {
            let version = zip_name[prefix.len()..zip_name.len() - 4].to_owned();
            let zip_size = zip["size"].as_u64().unwrap_or(0);
            best = Some((
                at,
                Package {
                    version,
                    zip_name,
                    zip_url,
                    zip_size,
                    sum_url,
                    sum_list,
                },
            ));
        }
    }
    best.map(|b| b.1).ok_or_else(|| E::NoPackage.into())
}

/// The digest for `name` in a `.sha256` file (`<hex>` or `<hex>  <name>`) or a SHA256SUMS.txt list (`name` required).
pub fn parse_sum(text: &str, name: &str, list: bool) -> Option<String> {
    text.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        let hex = w.next()?;
        let file = w.next().map(|f| f.trim_start_matches('*'));
        let ok = hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit());
        (ok && (if list {
            file == Some(name)
        } else {
            file.is_none_or(|f| f == name)
        }))
        .then(|| hex.to_ascii_lowercase())
    })
}

fn get_text(url: &str) -> Result<String> {
    let out = forza::curl::curl()
        .args(["-H", "Accept: application/vnd.github+json", "--url", url])
        .output()
        .map_err(|e| E::Download {
            detail: e.to_string(),
        })?;
    ensure!(
        out.status.success(),
        E::Download {
            detail: String::from_utf8_lossy(&out.stderr).trim().to_owned()
        }
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Ask GitHub which package is the newest.
pub fn latest(prefix: &str) -> Result<Package> {
    pick(&get_text(API)?, prefix)
}

pub fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 1 << 20]);
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The file must hash to `want`; a mismatch deletes it.
pub fn verify(path: &Path, want: &str) -> Result<()> {
    let got = sha256_of(path).map_err(|e| E::Download {
        detail: e.to_string(),
    })?;
    if got != want.to_ascii_lowercase() {
        let _ = std::fs::remove_file(path);
        return Err(E::HashMismatch.into());
    }
    Ok(())
}

/// Download the package into the temp folder (an earlier download that still matches is reused) and return its path.
pub fn download(p: &Package, cancel: &AtomicBool, progress: &dyn Fn(u64, u64)) -> Result<PathBuf> {
    let want = parse_sum(&get_text(&p.sum_url)?, &p.zip_name, p.sum_list).ok_or(E::NoPackage)?;
    let dir = std::env::temp_dir().join("Forza-WCCF-Installer");
    std::fs::create_dir_all(&dir).map_err(|e| E::Download {
        detail: e.to_string(),
    })?;
    let (file, part) = (
        dir.join(&p.zip_name),
        dir.join(format!("{}.part", p.zip_name)),
    );
    if file.is_file() && verify(&file, &want).is_ok() {
        return Ok(file);
    }
    let mut child = forza::curl::curl()
        .arg("-o")
        .arg(&part)
        .args(["--url", &p.zip_url])
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| E::Download {
            detail: e.to_string(),
        })?;
    while child.try_wait()?.is_none() {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&part);
            return Err(E::Cancelled.into());
        }
        progress(std::fs::metadata(&part).map_or(0, |m| m.len()), p.zip_size);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let out = child.wait_with_output()?;
    ensure!(
        out.status.success(),
        E::Download {
            detail: String::from_utf8_lossy(&out.stderr).trim().to_owned()
        }
    );
    verify(&part, &want)?;
    std::fs::rename(&part, &file).map_err(|e| E::Download {
        detail: e.to_string(),
    })?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, draft: bool, pre: bool, at: &str, assets: &[&str]) -> String {
        let a: Vec<String> = assets
            .iter()
            .map(|n| format!(r#"{{"name":"{n}","size":10,"browser_download_url":"https://example.test/{tag}/{n}"}}"#))
            .collect();
        format!(
            r#"{{"tag_name":"{tag}","draft":{draft},"prerelease":{pre},"published_at":"{at}","assets":[{}]}}"#,
            a.join(",")
        )
    }

    fn list(rels: &[String]) -> String {
        format!("[{}]", rels.join(","))
    }

    #[test]
    fn newest_usable_release_wins() {
        let z = |v: &str| format!("Forza-WCCF-2017-18-{v}.zip");
        let all = |v: &str| vec![z(v), format!("{}.sha256", z(v))];
        let a = |v: &str| {
            all(v)
                .iter()
                .map(String::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let r = |tag: &str, draft, pre, at: &str, v: &str| {
            let a = a(v);
            rel(
                tag,
                draft,
                pre,
                at,
                &a.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        };
        let json = list(&[
            r("v0.1.0", false, false, "2026-10-01T00:00:00Z", "0.1.0"),
            r("v0.3.0", true, false, "2026-10-20T00:00:00Z", "0.3.0"), // draft
            r("v0.2.0", false, true, "2026-10-15T00:00:00Z", "0.2.0"), // pre-release, newest usable
            rel(
                "v0.2.1",
                false,
                false,
                "2026-10-18T00:00:00Z",
                &["Forza-WCCF-2017-18-0.2.1.zip"],
            ), // no checksum
            rel(
                "v0.2.2",
                false,
                false,
                "2026-10-19T00:00:00Z",
                &["Forza-WCCF-2010-11-0.2.2.zip", "x.sha256"],
            ), // other edition
        ]);
        let p = pick(&json, PREFIX_1718).unwrap();
        assert_eq!((p.version.as_str(), p.sum_list), ("0.2.0", false));
        assert_eq!(
            p.zip_url,
            "https://example.test/v0.2.0/Forza-WCCF-2017-18-0.2.0.zip"
        );
        // the list form
        let json = list(&[rel(
            "v1",
            false,
            false,
            "2026-10-01T00:00:00Z",
            &["Forza-WCCF-2017-18-1.0.zip", "SHA256SUMS.txt"],
        )]);
        assert!(pick(&json, PREFIX_1718).unwrap().sum_list);
        // only a release without its checksum: refused
        let json = list(&[rel(
            "v2",
            false,
            false,
            "2026-10-01T00:00:00Z",
            &["Forza-WCCF-2017-18-2.0.zip"],
        )]);
        assert_eq!(
            pick(&json, PREFIX_1718).unwrap_err().downcast_ref::<E>(),
            Some(&E::NoPackage)
        );
        assert!(pick("{}", PREFIX_1718).is_err());
    }

    #[test]
    fn checksum_lines() {
        let h = "ab".repeat(32);
        assert_eq!(
            parse_sum(&format!("{h}  z.zip\n"), "z.zip", false),
            Some(h.clone())
        );
        assert_eq!(
            parse_sum(&format!("{h}\n"), "z.zip", false),
            Some(h.clone())
        );
        assert_eq!(
            parse_sum(&format!("{h}  other.zip\n"), "z.zip", false),
            None
        );
        let list = format!("{}  a.zip\n{h} *z.zip\n", "cd".repeat(32));
        assert_eq!(parse_sum(&list, "z.zip", true), Some(h.clone()));
        assert_eq!(parse_sum(&format!("{h}\n"), "z.zip", true), None);
        assert_eq!(parse_sum("short  z.zip", "z.zip", true), None);
    }

    #[test]
    fn a_mismatching_download_is_refused_and_deleted() {
        let f = std::env::temp_dir().join(format!("fetch-test-{}.zip", std::process::id()));
        std::fs::write(&f, b"package").unwrap();
        let good = sha256_of(&f).unwrap();
        assert!(verify(&f, &good.to_uppercase()).is_ok());
        let bad = verify(&f, &"0".repeat(64)).unwrap_err();
        assert_eq!(bad.downcast_ref::<E>(), Some(&E::HashMismatch));
        assert!(!f.exists());
    }
}
