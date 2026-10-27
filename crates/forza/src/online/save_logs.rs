//! SAVE LOGS (once `online/send_logs.py`): this kit's logs in one ZIP on the player's PC, for the player to send to
//! whoever helps them. Nothing is uploaded.
//!
//! In the ZIP: `data\logs` (this run and the one before, not the zipped archive), the panel's logs, `panel.txt`,
//! `keys.txt`, `running.json` and `version.txt`, each its last 2 MB at most; never the club card. What points at the
//! person is replaced in the text: the user folder and any other `X:\Users\NAME` (`%USERPROFILE%`), the user name
//! (`USER`) and the PC's name (`PC`). 2017-18: control's `wccfnet.log` from the game folder too. The newest [`DUMPS_SENT`] lean crash snapshots go whole, with the same names blanked to underscores (also
//! in UTF-16) so the file keeps its layout. A note (the player's words, why it was made) goes in as `report.txt`.

use std::fs;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Per file, the last bytes kept.
pub const TAIL: usize = 2 * 1024 * 1024;
/// A bigger ZIP is made again with shorter tails (it stays small enough to attach to a chat message).
pub const LIMIT: usize = 7 * 1024 * 1024;
/// A cabinet's lean snapshot is about 1 MB, 90 KB zipped.
pub const DUMPS_SENT: usize = 3;

/// This kit's version.txt ("kit-5.5", written by `cargo xtask package`) without "kit-": "5.5"; none: "dev".
pub fn version(kit: &Path) -> String {
    let Ok(text) = fs::read(crate::kit::app_dir(kit).join("version.txt")) else {
        return "dev".into();
    };
    let text = String::from_utf8_lossy(&text);
    let v = text.split_whitespace().next().unwrap_or("");
    let v = match v.get(..4) {
        Some(p) if p.eq_ignore_ascii_case("kit-") => &v[4..],
        _ => v,
    };
    if v.is_empty() { "dev".into() } else { v.into() }
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    v.sort();
    v
}

/// `(name in the ZIP, path)` of what goes in. `game` is the game folder: its `wccfnet.log`, and the `seat1\` beside it.
fn files(kit: &Path, game: Option<&Path>) -> Vec<(String, PathBuf)> {
    let data = kit.join("data");
    let mut out = vec![];
    for sub in ["", "previous", "hook"] {
        for p in files_in(&data.join("logs").join(sub)) {
            let n = p.file_name().unwrap_or_default().to_string_lossy();
            out.push((
                if sub.is_empty() {
                    format!("logs/{n}")
                } else {
                    format!("logs/{sub}/{n}")
                },
                p,
            ));
        }
    }
    for n in ["panel.txt", "keys.txt", "running.json"] {
        out.push((format!("data/{n}"), data.join(n)));
    }
    out.push((
        "version.txt".into(),
        crate::kit::app_dir(kit).join("version.txt"),
    ));
    if let Some(game) = game {
        // 2017-18: control's guard log, beside the game folder's wccfnet.dll
        out.push(("game/wccfnet.log".into(), game.join("wccfnet.log")));
    }
    if let Some(game) = game {
        for p in files_in(&crate::kit::seat_dir(game)) {
            let n = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if n.ends_with(".log") {
                out.push((format!("seat1/{n}"), p));
            }
        }
    }
    out.retain(|(_, p)| p.is_file());
    out
}

/// The newest [`DUMPS_SENT`] lean crash snapshots.
fn dumps(kit: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<PathBuf> = files_in(&kit.join("data").join("dumps"))
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with("_lean.dmp"))
        .collect();
    v.drain(..v.len().saturating_sub(DUMPS_SENT));
    v.into_iter()
        .map(|p| {
            (
                format!(
                    "dumps/{}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                ),
                p,
            )
        })
        .collect()
}

/// What [`pack`] puts in the ZIP, by its names there (the report dialog lists them).
pub fn names(kit: &Path, game: Option<&Path>) -> Vec<String> {
    dumps(kit)
        .into_iter()
        .chain(files(kit, game))
        .map(|(n, _)| n)
        .collect()
}

/// The strings that point at the person: the Windows user folder, the user name and the PC's name.
#[derive(Clone, Debug, Default)]
pub struct Scrub {
    pub home: String,
    pub user: String,
    pub pc: String,
    /// Also every `X:\Users\NAME` ([`user_folders`]); off only where a test replays the Python, which left them in.
    pub folders: bool,
}

impl Scrub {
    /// From the environment as `send_logs.py` reads it: `USERPROFILE` (else `HOME`), `USERNAME`, `COMPUTERNAME`.
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let home = Some(var("USERPROFILE"))
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| var("HOME"));
        Self {
            home,
            user: var("USERNAME"),
            pc: var("COMPUTERNAME"),
            folders: true,
        }
    }

    /// The names as bytes in each encoding: UTF-8, then Latin-1 (what the Python takes on Linux; Windows' ANSI page
    /// is close), a name Latin-1 cannot hold skipped; `utf16` adds Windows' own text for binary files.
    fn encodings(name: &str, utf16: bool) -> Vec<Vec<u8>> {
        let mut v = vec![name.as_bytes().to_vec()];
        if let Some(l) = name
            .chars()
            .map(|c| u8::try_from(u32::from(c)).ok())
            .collect::<Option<Vec<u8>>>()
        {
            v.push(l);
        }
        if utf16 {
            v.push(name.encode_utf16().flat_map(u16::to_le_bytes).collect());
        }
        v
    }

    /// (needle, replacement, whole word) in the order they apply to text.
    fn text_pairs(&self) -> Vec<(Vec<u8>, &'static [u8], bool)> {
        let mut out = vec![];
        for enc in 0..2 {
            let forward = self.home.replace('\\', "/");
            for (old, new, whole) in [
                (self.home.as_str(), &b"%USERPROFILE%"[..], false),
                (forward.as_str(), b"%USERPROFILE%", false),
                (self.user.as_str(), b"USER", true),
                (self.pc.as_str(), b"PC", true),
            ] {
                // too short to take out of text without breaking it
                if old.chars().count() < 3 {
                    continue;
                }
                if let Some(n) = Self::encodings(old, false).into_iter().nth(enc) {
                    out.push((n, new, whole));
                }
            }
        }
        out
    }

    /// Text: the names replaced (case-insensitively), the user and PC names only as whole words.
    pub fn scrub(&self, mut data: Vec<u8>) -> Vec<u8> {
        for (needle, new, whole) in self.text_pairs() {
            data = replace(&data, &needle, whole, |_| new.to_vec());
        }
        if self.folders {
            data = user_folders(&data);
        }
        data
    }

    /// A binary file: each match turned into as many underscores, nothing moves, so a snapshot stays a valid file.
    pub fn blank(&self, mut data: Vec<u8>) -> Vec<u8> {
        for old in [&self.home, &self.user, &self.pc] {
            if old.chars().count() < 3 {
                continue;
            }
            for needle in Self::encodings(old, true) {
                data = replace(&data, &needle, false, |m| vec![b'_'; m.len()]);
            }
        }
        data
    }
}

/// Every `X:\Users\NAME` (any drive, any case, `\`, `/` or doubled separators) turned into `%USERPROFILE%`: the
/// folders of other Windows users, or of this one under another drive letter. NAME runs to the next separator, quote or
/// control character, so a name with spaces goes whole (one cannot start with a space).
pub fn user_folders(data: &[u8]) -> Vec<u8> {
    let sep = |b: u8| b == b'\\' || b == b'/';
    let seps = |i: usize| {
        data[i.min(data.len())..]
            .iter()
            .take_while(|&&b| sep(b))
            .count()
    };
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i].is_ascii_alphabetic() && data.get(i + 1) == Some(&b':') {
            let a = seps(i + 2);
            let u = i + 2 + a;
            if a > 0 && data.len() >= u + 5 && data[u..u + 5].eq_ignore_ascii_case(b"users") {
                let b = seps(u + 5);
                let name = u + 5 + b;
                let end = name
                    + data[name..]
                        .iter()
                        .take_while(|&&c| !sep(c) && !b"\"'<>|:*?".contains(&c) && c >= 0x20)
                        .count();
                if b > 0 && end > name && data[name] != b' ' {
                    out.extend_from_slice(b"%USERPROFILE%");
                    i = end;
                    continue;
                }
            }
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

/// `data` with every ASCII-case-insensitive, non-overlapping match of `needle` (left to right) replaced; `whole`: only
/// where no ASCII letter or digit sits on either side.
fn replace(data: &[u8], needle: &[u8], whole: bool, with: impl Fn(&[u8]) -> Vec<u8>) -> Vec<u8> {
    if needle.is_empty() {
        return data.to_vec();
    }
    let alnum = |b: Option<&u8>| b.is_some_and(u8::is_ascii_alphanumeric);
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let end = i + needle.len();
        let hit = end <= data.len()
            && data[i..end].eq_ignore_ascii_case(needle)
            && !(whole
                && (alnum(i.checked_sub(1).and_then(|p| data.get(p))) || alnum(data.get(end))));
        if hit {
            out.extend(with(&data[i..end]));
            i = end;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

fn zip_err(e: zip::result::ZipError) -> io::Error {
    io::Error::other(e)
}

/// The ZIP (bytes) and how many files are in it. A file that is busy or gone is left out; the rest still goes.
pub fn pack(
    kit: &Path,
    game: Option<&Path>,
    scrub: &Scrub,
    tail: usize,
    note: Option<&str>,
) -> io::Result<(Vec<u8>, usize)> {
    pack_within(kit, game, scrub, tail, note, LIMIT)
}

/// [`pack`], made again with shorter tails while the ZIP is over `limit` bytes (down to 64 KB a file).
pub fn pack_within(
    kit: &Path,
    game: Option<&Path>,
    scrub: &Scrub,
    mut tail: usize,
    note: Option<&str>,
    limit: usize,
) -> io::Result<(Vec<u8>, usize)> {
    loop {
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        let mut n = 0;
        if let Some(note) = note {
            z.start_file("report.txt", opts).map_err(zip_err)?;
            z.write_all(&scrub.scrub(note.as_bytes().to_vec()))?;
            n += 1;
        }
        for (arc, path) in dumps(kit) {
            if let Ok(data) = fs::read(&path) {
                z.start_file(arc, opts).map_err(zip_err)?;
                z.write_all(&scrub.blank(data))?;
                n += 1;
            }
        }
        for (arc, path) in files(kit, game) {
            let Ok(mut f) = fs::File::open(&path) else {
                continue;
            };
            let Ok(size) = f.seek(SeekFrom::End(0)) else {
                continue;
            };
            let size = usize::try_from(size).unwrap_or(usize::MAX);
            let mut data = Vec::new();
            if f.seek(SeekFrom::Start((size.saturating_sub(tail)) as u64))
                .is_err()
                || f.read_to_end(&mut data).is_err()
            {
                continue;
            }
            if size > tail {
                let mut head = format!(
                    "[... the first {} bytes left out - the last {tail} follow]\n",
                    size - tail
                )
                .into_bytes();
                head.append(&mut data);
                data = head;
            }
            z.start_file(arc, opts).map_err(zip_err)?;
            z.write_all(&scrub.scrub(data))?;
            n += 1;
        }
        let blob = z.finish().map_err(zip_err)?.into_inner();
        if blob.len() > limit && tail > 64 * 1024 {
            tail /= 4;
            continue;
        }
        return Ok((blob, n));
    }
}

/// The player's words for `report.txt`: scrubbed like the logs, at most [`MAX_NOTE_CHARS`].
pub fn note(text: &str, scrub: &Scrub) -> String {
    let clean = scrub.scrub(text.trim().as_bytes().to_vec());
    String::from_utf8_lossy(&clean)
        .chars()
        .take(MAX_NOTE_CHARS)
        .collect()
}

/// The player's words: at most this many characters.
pub const MAX_NOTE_CHARS: usize = 2000;

/// `forza-wccf-logs-YYYYMMDD-HHMMSS.zip`: each saved ZIP has its own name.
pub fn zip_name(t: time::OffsetDateTime) -> String {
    format!(
        "forza-wccf-logs-{:04}{:02}{:02}-{:02}{:02}{:02}.zip",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// Where SAVE LOGS puts the ZIP: the user's Downloads folder (Windows' known folder; else `%USERPROFILE%\Downloads`).
pub fn downloads() -> PathBuf {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Com::CoTaskMemFree;
        use windows_sys::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath};
        let mut p = std::ptr::null_mut();
        // SAFETY: a known folder id and an out pointer; the string is the system's and freed once copied
        unsafe {
            if SHGetKnownFolderPath(&FOLDERID_Downloads, 0, std::ptr::null_mut(), &mut p) == 0
                && !p.is_null()
            {
                let len = (0..).take_while(|&i| *p.add(i) != 0).count();
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                CoTaskMemFree(p.cast());
                return PathBuf::from(s);
            }
            CoTaskMemFree(p.cast());
        }
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .unwrap_or_default();
    PathBuf::from(home).join("Downloads")
}

/// SAVE LOGS: the ZIP ([`pack`], `note` as `report.txt`) written to `dir` as [`zip_name`] of now; its path.
pub fn save(
    dir: &Path,
    kit: &Path,
    game: Option<&Path>,
    scrub: &Scrub,
    note: Option<&str>,
) -> io::Result<PathBuf> {
    let (blob, _) = pack(kit, game, scrub, TAIL, note)?;
    fs::create_dir_all(dir)?;
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let path = dir.join(zip_name(now));
    fs::write(&path, blob)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_folders_become_userprofile_on_any_drive_and_case() {
        let s = |t: &str| String::from_utf8(user_folders(t.as_bytes())).unwrap();
        assert_eq!(
            s(r"C:\Users\Mario\AppData\x.log"),
            r"%USERPROFILE%\AppData\x.log"
        );
        assert_eq!(s(r"d:\USERS\Ann Lee\Desktop"), r"%USERPROFILE%\Desktop");
        assert_eq!(
            s("e:/users/bob/x and E:\\\\Users\\\\bob\\\\y"),
            "%USERPROFILE%/x and %USERPROFILE%\\\\y"
        );
        assert_eq!(s("\"C:\\Users\\zed\" ok"), "\"%USERPROFILE%\" ok");
        assert_eq!(
            s(r"C:\Users\ then C:\Program Files\x"),
            r"C:\Users\ then C:\Program Files\x"
        );
        assert_eq!(s("no path here: Users"), "no path here: Users");
    }

    #[test]
    fn note_is_scrubbed_and_capped() {
        let scrub = Scrub {
            home: r"C:\Users\Mario".into(),
            user: "Mario".into(),
            pc: "DESKTOP-ABC".into(),
            folders: true,
        };
        let n = note(
            r" Mario here, on DESKTOP-ABC: crash in D:\Users\Mario\Games ",
            &scrub,
        );
        assert_eq!(n, r"USER here, on PC: crash in %USERPROFILE%\Games");
        assert_eq!(
            note(&"é".repeat(3000), &scrub).chars().count(),
            MAX_NOTE_CHARS
        );
    }

    #[test]
    fn zip_name_has_the_date_and_time() {
        let t = time::OffsetDateTime::from_unix_timestamp(1_793_091_903).unwrap();
        assert_eq!(zip_name(t), "forza-wccf-logs-20261027-090503.zip");
    }
}
