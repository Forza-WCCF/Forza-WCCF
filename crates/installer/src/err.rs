//! Every error the installer shows a person: a typed variant with its parameters, spoken through `ui.yml`
//! (`installer.err_*`) in the window's language. Headless mode prints the same keys in English.
//! Low-level failures that carry only OS or library text travel as `Other` and keep that text as it is.
use forza_ui::{Lang, text};
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub enum InstallError {
    Cancelled,
    NewerTrueCrypt,
    ChecksumTable {
        name: String,
    },
    TableMismatch,
    ShortRead,
    BlockCorrupt {
        block: usize,
    },
    CannotOpen {
        path: String,
        detail: String,
    },
    UnsupportedSize {
        path: String,
        len: u64,
    },
    WrongKey,
    NoKeyEntry {
        id: String,
    },
    Download {
        detail: String,
    },
    NoPackage,
    NoDownload,
    HashMismatch,
    BadVolumeSize,
    UnknownEdition,
    ZipName,
    NotKitZip,
    NotZip,
    EditionMismatch,
    NeedGame,
    GameFolderExists,
    EnglishNeedsSetup,
    NoKey,
    KeyRead {
        detail: String,
    },
    GameMissing {
        path: String,
    },
    CannotCreate {
        path: String,
        detail: String,
    },
    CannotWrite {
        path: String,
        detail: String,
    },
    NoSpace,
    NotNtfsDrive,
    NotNtfsVolume,
    ExtractIncomplete,
    KitUnsafePath,
    KitEdition {
        kit: &'static str,
        game: &'static str,
    },
    Script {
        detail: String,
    },
    Other {
        detail: String,
    },
}

pub fn path(p: &Path) -> String {
    p.display().to_string()
}

impl InstallError {
    pub fn key(&self) -> &'static str {
        use InstallError::*;
        match self {
            Cancelled => "installer.cancelled",
            NewerTrueCrypt => "installer.err_truecrypt_new",
            ChecksumTable { .. } => "installer.err_disc_table",
            TableMismatch => "installer.err_table_mismatch",
            ShortRead => "installer.err_short_read",
            BlockCorrupt { .. } => "installer.err_block",
            CannotOpen { .. } => "installer.err_open",
            UnsupportedSize { .. } => "installer.err_size",
            WrongKey => "installer.err_wrong_key",
            NoKeyEntry { .. } => "installer.err_key_entry",
            Download { .. } => "installer.err_download",
            NoPackage => "installer.err_no_package",
            NoDownload => "installer.err_no_download",
            HashMismatch => "installer.err_hash",
            BadVolumeSize => "installer.err_volume_size",
            UnknownEdition => "installer.err_edition",
            ZipName => "installer.err_zip_name",
            NotKitZip => "installer.err_not_kit",
            NotZip => "installer.err_not_zip",
            EditionMismatch => "installer.err_edition_mismatch",
            NeedGame => "installer.err_need_game",
            GameFolderExists => "installer.err_game_exists",
            EnglishNeedsSetup => "installer.err_english_setup",
            NoKey => "installer.err_no_key",
            KeyRead { .. } => "installer.err_key_read",
            GameMissing { .. } => "installer.err_game_missing",
            CannotCreate { .. } => "installer.err_create",
            CannotWrite { .. } => "installer.err_write",
            NoSpace => "installer.err_space",
            NotNtfsDrive => "installer.err_drive_ntfs",
            NotNtfsVolume => "installer.err_volume_ntfs",
            ExtractIncomplete => "installer.err_extract_incomplete",
            KitUnsafePath => "installer.err_kit_unsafe",
            KitEdition { .. } => "installer.err_kit_edition",
            Script { .. } => "installer.err_script",
            Other { .. } => "installer.err_other",
        }
    }

    pub fn args(&self) -> Vec<(&'static str, String)> {
        use InstallError::*;
        match self {
            ChecksumTable { name } => vec![("name", name.clone())],
            BlockCorrupt { block } => vec![("block", block.to_string())],
            CannotOpen { path, detail }
            | CannotCreate { path, detail }
            | CannotWrite { path, detail } => {
                vec![("path", path.clone()), ("detail", detail.clone())]
            }
            UnsupportedSize { path, len } => vec![("path", path.clone()), ("len", len.to_string())],
            KeyRead { detail } => vec![("e", detail.clone())],
            NoKeyEntry { id } => vec![("id", id.clone())],
            GameMissing { path } => vec![("path", path.clone())],
            KitEdition { kit, game } => vec![("kit", kit.to_string()), ("game", game.to_string())],
            Script { detail } | Other { detail } | Download { detail } => {
                vec![("detail", detail.clone())]
            }
            _ => vec![],
        }
    }

    pub fn localized(&self, lang: Lang) -> String {
        let args = self.args();
        let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        text(lang, self.key(), &args)
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.localized(Lang::En))
    }
}

impl std::error::Error for InstallError {}

/// What the window shows for any error from the pipeline.
pub fn localize(e: &anyhow::Error, lang: Lang) -> String {
    match e.downcast_ref::<InstallError>() {
        Some(e) => e.localized(lang),
        None => InstallError::Other {
            detail: format!("{e:#}"),
        }
        .localized(lang),
    }
}

pub fn is_cancelled(e: &anyhow::Error) -> bool {
    e.downcast_ref::<InstallError>() == Some(&InstallError::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of every variant: each key must read differently from the key itself in all three languages.
    #[test]
    fn every_variant_has_text_in_three_languages() {
        use InstallError::*;
        let s = || "x".to_string();
        let all = [
            Cancelled,
            NewerTrueCrypt,
            ChecksumTable { name: s() },
            TableMismatch,
            ShortRead,
            BlockCorrupt { block: 1 },
            CannotOpen {
                path: s(),
                detail: s(),
            },
            UnsupportedSize { path: s(), len: 1 },
            WrongKey,
            NoKeyEntry { id: s() },
            Download { detail: s() },
            NoPackage,
            NoDownload,
            HashMismatch,
            BadVolumeSize,
            UnknownEdition,
            ZipName,
            NotKitZip,
            NotZip,
            EditionMismatch,
            NeedGame,
            GameFolderExists,
            EnglishNeedsSetup,
            NoKey,
            KeyRead { detail: s() },
            GameMissing { path: s() },
            CannotCreate {
                path: s(),
                detail: s(),
            },
            CannotWrite {
                path: s(),
                detail: s(),
            },
            NoSpace,
            NotNtfsDrive,
            NotNtfsVolume,
            ExtractIncomplete,
            KitUnsafePath,
            KitEdition {
                kit: "a",
                game: "b",
            },
            Script { detail: s() },
            Other { detail: s() },
        ];
        // at run time first: nextest's `--workspace-remap` sets it for an archive built on another machine
        let dir = std::env::var_os("CARGO_MANIFEST_DIR").map_or_else(
            || env!("CARGO_MANIFEST_DIR").into(),
            std::path::PathBuf::from,
        );
        let yml = std::fs::read_to_string(dir.join("../forza-ui/locales/ui.yml")).unwrap();
        for e in &all {
            let line = yml
                .lines()
                .find(|l| l.starts_with(&format!("{}:", e.key())))
                .unwrap_or_else(|| panic!("{} is not in ui.yml", e.key()));
            assert!(
                ["en: ", "it: ", "ja: "].iter().all(|l| line.contains(l)),
                "{line}"
            );
            for (k, _) in e.args() {
                assert!(
                    line.contains(&format!("%{{{k}}}")),
                    "{} lacks %{{{k}}}",
                    e.key()
                );
            }
            for lang in Lang::ALL {
                let t = e.localized(lang);
                assert!(
                    !t.contains("%{") && !t.starts_with("installer."),
                    "{}: {t}",
                    e.key()
                );
            }
        }
    }
}
