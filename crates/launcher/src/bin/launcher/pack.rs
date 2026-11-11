//! The high-quality card pictures (2017-18): one ZIP, pinned here by address, size and SHA-256 (a new pack comes with a
//! Launcher release). DOWNLOAD fetches it into data\downloads\ with Windows' curl.exe, on from the bytes already there
//! after a cut or a restart (`.part`, an HTTP Range request), checks it, and the card pictures step applies it
//! (forza::cards::pictures::ensure). The ZIP stays: REMOVE takes the pictures away, a later DOWNLOAD only checks it.

use std::fs;
use std::path::{Path, PathBuf};

use crate::text::{t, tf};
use crate::{log, update};

pub struct Pack<'a> {
    pub url: &'a str,
    pub size: u64,
    pub sha256: &'a str,
}

/// An asset on an existing release (not its own: the release workflows take the newest release as their base).
pub const PACK: Pack<'static> = Pack {
    url: "https://github.com/Forza-WCCF/Forza-WCCF/releases/download/v0.7.0/card-pictures-2017-18-1.zip",
    size: 1_218_295_797,
    sha256: "788e7d352c5432a5c23b4563475f4ac4336fd6812198aecee2a8da7fd590a67f",
};

/// The disk space a download needs: what is left to fetch, the pictures it makes (about the ZIP's size) and 50 MB.
fn room(size: u64, have: u64) -> u64 {
    size.saturating_sub(have) + size + (50 << 20)
}

impl Pack<'_> {
    fn name(&self) -> &str {
        self.url.rsplit('/').next().unwrap_or("card-pictures.zip")
    }

    /// The checked ZIP in `dir`, downloaded or resumed first unless it is there already. `free`: the drive's free bytes
    /// (None: not known, the download goes ahead). A failed download keeps its `.part` for the next try; a wrong
    /// SHA-256 deletes it.
    pub fn fetch(&self, dir: &Path, free: impl Fn(&Path) -> Option<u64>) -> Result<PathBuf, String> {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let zip = dir.join(self.name());
        if fs::metadata(&zip).is_ok_and(|m| m.len() == self.size) && update::verify(&zip, self.sha256).is_ok() {
            return Ok(zip);
        }
        let _ = fs::remove_file(&zip);
        let part = dir.join(format!("{}.part", self.name()));
        let mut have = fs::metadata(&part).map_or(0, |m| m.len());
        if have > self.size {
            let _ = fs::remove_file(&part);
            have = 0;
        }
        let need = room(self.size, have);
        if free(dir).is_some_and(|f| f < need) {
            return Err(tf("launcher.msg_pack_no_space", &[("need", &(need >> 20).to_string())]));
        }
        if have < self.size {
            update::download(&part, self.url, self.size, &t("launcher.msg_pack_downloading"), true)?;
        }
        if let Err(e) = update::verify(&part, self.sha256) {
            log(&format!("  {e}"));
            let _ = fs::remove_file(&part);
            return Err(t("launcher.msg_pack_bad_hash"));
        }
        fs::rename(&part, &zip).map_err(|e| format!("{}: {e}", zip.display()))?;
        Ok(zip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// A pack served from a `file://` address (curl resumes those too) in a scratch folder.
    fn served(bytes: &[u8]) -> (tempfile::TempDir, String, String) {
        let d = crate::scratch();
        let src = d.path().join("card-pictures-test.zip");
        fs::write(&src, bytes).unwrap();
        let url = format!("file:///{}", src.display().to_string().replace('\\', "/"));
        let hash = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
        (d, url, hash)
    }

    #[test]
    fn the_pinned_pack_is_an_asset_with_a_full_hash() {
        assert!(
            PACK.url
                .starts_with("https://github.com/Forza-WCCF/Forza-WCCF/releases/download/v")
        );
        assert!(
            !PACK.name().starts_with("Forza-WCCF-"),
            "the update and installer would take it for a package"
        );
        assert!(PACK.sha256.len() == 64 && PACK.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(room(100 << 20, 40 << 20), (60 + 100 + 50) << 20);
    }

    #[test]
    fn a_cut_download_resumes_and_is_checked() {
        let bytes: Vec<u8> = (0..300_000u32).map(|i| (i * 7) as u8).collect();
        let (d, url, hash) = served(&bytes);
        let pack = Pack {
            url: &url,
            size: bytes.len() as u64,
            sha256: &hash,
        };
        let dir = d.path().join("downloads");
        fs::create_dir_all(&dir).unwrap();
        // the first 100 KB from an earlier try
        fs::write(dir.join("card-pictures-test.zip.part"), &bytes[..100_000]).unwrap();
        let zip = pack.fetch(&dir, |_| None).unwrap();
        assert_eq!(fs::read(&zip).unwrap(), bytes);
        assert!(!dir.join("card-pictures-test.zip.part").exists());
        // there already: checked, not fetched again (the source is gone)
        fs::remove_file(d.path().join("card-pictures-test.zip")).unwrap();
        assert_eq!(pack.fetch(&dir, |_| None).unwrap(), zip);
    }

    #[test]
    fn a_wrong_hash_deletes_the_download_and_no_room_refuses_before_it() {
        let bytes = vec![5u8; 50_000];
        let (d, url, _) = served(&bytes);
        let dir = d.path().join("downloads");
        let bad = Pack {
            url: &url,
            size: bytes.len() as u64,
            sha256: &"0".repeat(64),
        };
        assert_eq!(bad.fetch(&dir, |_| None).unwrap_err(), t("launcher.msg_pack_bad_hash"));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "nothing left");
        let small = Pack {
            url: &url,
            size: 1 << 30,
            sha256: &"0".repeat(64),
        };
        let e = small.fetch(&dir, |_| Some(1 << 30)).unwrap_err();
        assert_eq!(
            e,
            tf("launcher.msg_pack_no_space", &[("need", &((2048 + 50).to_string()))])
        );
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "nothing downloaded");
    }

    #[test]
    fn no_address_fails_in_one_line_and_keeps_nothing() {
        let d = crate::scratch();
        let url = format!("file:///{}/none.zip", d.path().display().to_string().replace('\\', "/"));
        let p = Pack {
            url: &url,
            size: 10,
            sha256: &"0".repeat(64),
        };
        let e = p.fetch(&d.path().join("downloads"), |_| None).unwrap_err();
        assert!(!e.contains('\n'), "{e}");
        assert!(
            fs::read_dir(d.path().join("downloads"))
                .unwrap()
                .all(|f| { f.unwrap().metadata().unwrap().len() == 0 })
        );
    }
}
