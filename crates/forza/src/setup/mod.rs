//! The install steps (ROADMAP step 11), ported from the kit's `setup.py`.  One module per edition (`e1718`, `e1011`),
//! sharing the helpers below: SHA-256 and whole-file writes,
//! the pinned originals kept in `original\`, byte-checked patches, the import rename that makes a guarded copy, folder
//! junctions and the kit's `data\settings.json`.  The edition's folder names are in [`crate::layout`].

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub mod e1011;
pub mod e1718;

/// What went wrong, in words for the player.
#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// An I/O error with the path it happened on.
trait At<T> {
    fn at(self, path: &Path) -> Result<T>;
}

impl<T> At<T> for io::Result<T> {
    fn at(self, path: &Path) -> Result<T> {
        self.map_err(|e| Error(format!("{}: {e}", path.display())))
    }
}

pub fn sha(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).at(path)
}

/// Writes `data` and reads it back to check it was written whole.
pub fn put(path: &Path, data: &[u8]) -> Result<()> {
    fs::write(path, data).at(path)?;
    if sha(&read(path)?) != sha(data) {
        return Err(Error(format!("{} was not written whole", path.display())));
    }
    Ok(())
}

/// Writes `data` whole through a temporary file beside `path`, then swaps it in: a crash never leaves half a file, and
/// a hard link at `path` is replaced, never written through to the file it shares.
pub fn replace(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    fs::write(&tmp, data).at(&tmp)?;
    fs::rename(&tmp, path).at(path)
}

/// Copies a file the kit or Windows provides; a missing one is named for the player.
pub fn copy_in(src: &Path, dst: &Path, what: &str) -> Result<()> {
    if !src.is_file() {
        return Err(Error(format!(
            "Forza-WCCF is missing {} ({what})",
            src.display()
        )));
    }
    fs::copy(src, dst).at(dst)?;
    Ok(())
}

pub fn make_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).at(path)
}

/// The untouched `exe` bytes: from `backup` when it exists, else from `current`, which is then copied to `backup`.
/// Anything whose SHA-256 is not `pinned` is refused.
pub fn original(
    exe: &str,
    current: &Path,
    backup: &Path,
    pinned: &str,
    edition: &str,
) -> Result<Vec<u8>> {
    let src = if backup.is_file() { backup } else { current };
    let data = read(src)?;
    let got = sha(&data);
    if got != pinned {
        return Err(Error(format!(
            "{} is not the original WCCF {edition} program {exe} (SHA-256 {got}, expected {pinned}). Give setup the \
             decrypted {edition} game folder.",
            src.display()
        )));
    }
    if src == current {
        make_dir(backup.parent().unwrap_or(backup))?;
        put(backup, &data)?;
    }
    Ok(data)
}

/// The name the original logowin.exe is kept under beside it (the 2017-18 setup; the 2010-11 kit before 2026-10-09).
pub fn logowin_backup() -> String {
    format!(
        "logowin_{}.exe",
        String::from_utf8_lossy(&[0x73, 0x65, 0x67, 0x61])
    )
}

/// One byte-checked change to a program: the file offset, the original bytes, the new ones.
pub struct Patch {
    pub at: usize,
    pub old: &'static [u8],
    pub new: &'static [u8],
    pub what: &'static str,
}

/// `data` with every patch applied, each after checking that its original bytes are there.
pub fn apply(data: &[u8], patches: &[Patch]) -> Result<Vec<u8>> {
    let mut d = data.to_vec();
    for p in patches {
        if p.new.len() != p.old.len() || d.get(p.at..p.at + p.old.len()) != Some(p.old) {
            return Err(Error(format!(
                "0x{:06X} is not the expected original bytes ({})",
                p.at, p.what
            )));
        }
        d[p.at..p.at + p.new.len()].copy_from_slice(p.new);
    }
    Ok(d)
}

fn u16_at(d: &[u8], at: usize) -> Result<usize> {
    let b = d
        .get(at..at + 2)
        .ok_or_else(|| Error("the program is cut short".into()))?;
    Ok(usize::from(u16::from_le_bytes([b[0], b[1]])))
}

fn u32_at(d: &[u8], at: usize) -> Result<usize> {
    let b = d
        .get(at..at + 4)
        .ok_or_else(|| Error("the program is cut short".into()))?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// The file offset of a virtual address in a PE image.
fn rva_off(d: &[u8], rva: usize) -> Result<usize> {
    let nt = u32_at(d, 60)?;
    let (sections, opt) = (u16_at(d, nt + 6)?, u16_at(d, nt + 20)?);
    for i in 0..sections {
        let s = nt + 24 + opt + i * 40;
        let (vs, va, rs, raw) = (
            u32_at(d, s + 8)?,
            u32_at(d, s + 12)?,
            u32_at(d, s + 16)?,
            u32_at(d, s + 20)?,
        );
        if (va..va + vs.max(rs)).contains(&rva) {
            return Ok(raw + rva - va);
        }
    }
    Err(Error(format!("{rva:#x} is in no section")))
}

/// The file offsets of the 32-bit program's import names equal to `dll` (NUL-terminated), matched without regard to case.
fn import_names(d: &[u8], dll: &[u8]) -> Result<Vec<usize>> {
    let nt = u32_at(d, 60)?;
    let mut off = rva_off(d, u32_at(d, nt + 24 + 96 + 8)?)?;
    let mut hits = Vec::new();
    while u32_at(d, off + 12)? != 0 {
        let name = rva_off(d, u32_at(d, off + 12)?)?;
        if d.get(name..name + dll.len())
            .is_some_and(|n| n.eq_ignore_ascii_case(dll))
        {
            hits.push(name);
        }
        off += 20;
    }
    Ok(hits)
}

/// Whether the 32-bit program imports `dll` (NUL-terminated); a file that is not one does not.
pub fn imports(data: &[u8], dll: &[u8]) -> bool {
    import_names(data, dll).is_ok_and(|h| !h.is_empty())
}

/// The 32-bit program with its one import of the DLL `old` renamed to `new` (same length, as the file holds the name
/// with its NUL: pass both NUL-terminated and equally long); the name is matched without regard to case.
pub fn rename_import(data: &[u8], old: &[u8], new: &[u8]) -> Result<Vec<u8>> {
    assert_eq!(old.len(), new.len());
    let mut d = data.to_vec();
    let hits = import_names(&d, old)?;
    let [at] = hits[..] else {
        let name = String::from_utf8_lossy(&old[..old.len() - 1]).into_owned();
        return Err(Error(format!(
            "the program has {} {name} imports, expected 1",
            hits.len()
        )));
    };
    d[at..at + old.len()].copy_from_slice(new);
    Ok(d)
}

/// A folder link `link` to `target`, unless something is at `link` already.
pub fn make_junction(link: &Path, target: &Path) -> Result<()> {
    if link.symlink_metadata().is_ok() {
        return Ok(());
    }
    junction_to(target, link)?;
    if !link.is_dir() {
        return Err(Error(format!(
            "could not make the folder link {}",
            link.display()
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn junction_to(target: &Path, link: &Path) -> Result<()> {
    junction::create(target, link).at(link)
}

/// Where the folder link `link` points, or None when it is not one.
#[cfg(windows)]
pub fn junction_target(link: &Path) -> Option<PathBuf> {
    junction::get_target(link).ok()
}

#[cfg(not(windows))]
pub fn junction_target(_: &Path) -> Option<PathBuf> {
    None
}

#[cfg(not(windows))]
fn junction_to(_: &Path, link: &Path) -> Result<()> {
    Err(Error(format!(
        "{}: folder links need Windows",
        link.display()
    )))
}

/// Text as the kit's Python wrote it on Windows (text mode: CR LF line ends), so the files setup writes match.
pub fn crlf(text: &str) -> Vec<u8> {
    text.replace('\n', "\r\n").into_bytes()
}

/// JSON text as Python's `json.dump(.., indent=N)` lays it out.
pub fn json_indented(value: &serde_json::Value, indent: &[u8]) -> String {
    use serde::Serialize;
    let mut out = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(indent);
    value
        .serialize(&mut serde_json::Serializer::with_formatter(&mut out, fmt))
        .expect("a JSON value serialises");
    String::from_utf8(out).expect("JSON is UTF-8")
}

/// The kit's data\settings.json; an empty object when it is missing or not JSON.
pub fn load_settings(settings: &Path) -> serde_json::Map<String, serde_json::Value> {
    fs::read_to_string(settings)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Writes settings.json whole through a temporary file beside it.
pub fn save_settings(settings: &Path, s: serde_json::Map<String, serde_json::Value>) -> Result<()> {
    make_dir(settings.parent().unwrap_or(settings))?;
    let tmp = PathBuf::from(format!("{}.tmp", settings.display()));
    fs::write(&tmp, crlf(&json_indented(&s.into(), b" "))).at(&tmp)?;
    fs::rename(&tmp, settings).at(settings)
}

/// settings.json with `key` set; every other key stays.
pub fn merge_settings(settings: &Path, key: &str, value: &str) -> Result<()> {
    let mut s = load_settings(settings);
    s.insert(key.into(), value.into());
    save_settings(settings, s)
}

/// One line of setup-hashes.json: a program's original and guarded hashes, or one file's hash.
pub enum Hash {
    Pair(String, String),
    One(String),
}

/// What setup made, in order; setup-hashes.json holds it and `Launcher.exe` starts nothing unless it still matches.
#[derive(Default)]
pub struct Report(pub Vec<(String, Hash)>);

impl Report {
    /// The file's text, laid out as Python's `json.dumps(report, indent=2) + "\n"` wrote it.
    pub fn json(&self) -> Vec<u8> {
        let q = |s: &str| serde_json::to_string(s).expect("a string serialises");
        let items: Vec<String> = self
            .0
            .iter()
            .map(|(k, h)| match h {
                Hash::One(v) => format!("  {}: {}", q(k), q(v)),
                Hash::Pair(o, g) => format!(
                    "  {}: {{\n    \"original\": {},\n    \"guarded\": {}\n  }}",
                    q(k),
                    q(o),
                    q(g)
                ),
            })
            .collect();
        crlf(&format!("{{\n{}\n}}\n", items.join(",\n")))
    }
}

/// A game folder of the edition: `path` (spaces and quotes around it do not count) or its `game\` holds all of `programs`.
pub fn find_game(path: &str, programs: &[&str], below: &str) -> Option<PathBuf> {
    let path = path.trim().trim_matches('"');
    if path.is_empty() {
        return None;
    }
    let path = std::path::absolute(path).ok()?;
    [path.clone(), path.join(below)]
        .into_iter()
        .find(|p| programs.iter().all(|n| p.join(n).is_file()))
}

/// The game folder of `ed` in `path` (or below it), as setup finds it.
pub fn find_edition_game(ed: crate::Edition, path: &str) -> Option<PathBuf> {
    match ed {
        crate::Edition::E1011 => e1011::find_game(path),
        crate::Edition::E1718 => e1718::find_game(path),
    }
}

/// Setup (or `undo`) of the game in `folder` for the kit in `kit`, both editions, as `setup.py [undo] GAME [cards]`
/// ran it: the game found, the steps said line by line, the last line.  The exit code is the script's: 0 done,
/// 1 failed, 2 no game files there.
pub fn kit_setup(
    ed: crate::Edition,
    kit: &Path,
    folder: &str,
    undo: bool,
    cards: bool,
    say: &dyn Fn(&str),
) -> i32 {
    use crate::Edition::{E1011, E1718};
    let Some(game) = find_edition_game(ed, folder) else {
        say(&if folder.trim().is_empty() {
            "no game folder is set up - choose it under SETUP".to_owned()
        } else {
            let programs = match ed {
                E1011 => "client_Release.exe, control_Release.exe, match_Release.exe",
                E1718 => "client_Release.exe, control_Release.exe",
            };
            format!("no WCCF {} game files in {folder} ({programs})", ed.name())
        });
        return 2;
    };
    let what = if undo { "undo" } else { "setup" };
    say(&format!(
        "Forza-WCCF {} - {what} - game: {}",
        ed.name(),
        game.display()
    ));
    let done = match (ed, undo) {
        (E1011, true) => e1011::undo(kit, &game, say),
        (E1011, false) => e1011::run(kit, &game, cards, say),
        (E1718, true) => e1718::undo(kit, &game, &e1718::Spec::game()).map(|()| {
            say(&format!(
                "Undone: the original programs are back in {}. Your club card stays in data\\save.",
                game.display()
            ));
        }),
        (E1718, false) => e1718::run(kit, &game, &e1718::Spec::game())
            .map(|r| r.0.iter().for_each(|(k, _)| say(&format!("  ok    {k}"))))
            .and_then(|()| e1718::setup_catalogue(kit, &game, cards, say)),
    };
    match done {
        Ok(()) => {
            if !undo {
                say("Setup done.");
            }
            0
        }
        Err(e) => {
            say(&format!("  FAIL  {e}"));
            1
        }
    }
}

/// A minimal 32-bit PE with one section (VA 0x1000, file 0x200) whose import table names `dlls`, and `extra` after it.
#[cfg(test)]
pub(crate) fn pe(dlls: &[&str], extra: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 0x200 + 0x1000];
    d[60..64].copy_from_slice(&0x40u32.to_le_bytes());
    d[0x40..0x44].copy_from_slice(b"PE\0\0");
    d[0x46..0x48].copy_from_slice(&1u16.to_le_bytes()); // one section
    d[0x54..0x56].copy_from_slice(&0xE0u16.to_le_bytes()); // optional header size
    d[0x40 + 24 + 96 + 8..][..4].copy_from_slice(&0x1000u32.to_le_bytes()); // import table at RVA 0x1000
    let s = 0x40 + 24 + 0xE0;
    for (at, v) in [(8, 0x1000u32), (12, 0x1000), (16, 0x1000), (20, 0x200)] {
        d[s + at..][..4].copy_from_slice(&v.to_le_bytes());
    }
    let mut name = 0x1100;
    for (i, dll) in dlls.iter().enumerate() {
        d[0x200 + i * 20 + 12..][..4].copy_from_slice(&(name as u32).to_le_bytes());
        d[0x200 + name - 0x1000..][..dll.len()].copy_from_slice(dll.as_bytes());
        name += 0x20;
    }
    d[0x200 + 0x800..][..extra.len()].copy_from_slice(extra);
    d
}
