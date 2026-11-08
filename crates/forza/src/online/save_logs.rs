//! SAVE LOGS (once `online/send_logs.py`): this kit's logs in one ZIP on the player's PC, for the player to send to
//! whoever helps them. Nothing is uploaded.
//!
//! In the ZIP: `data\logs` (this run and the one before, not the zipped archive), the panel's logs, `panel.txt`,
//! `keys.txt`, `running.json` and `version.txt`, each its last 2 MB at most. What points at the
//! person is replaced in the text: the user folder and any other `X:\Users\NAME` (`%USERPROFILE%`), the user name
//! (`USER`) and the PC's name (`PC`). A name of two ASCII letters goes only as a word of its own (a PC called `Al`
//! leaves `also` and `local` alone, a standalone `an` goes) and a single ASCII letter stays; 2017-18: control's
//! `wccfnet.log` from the game folder too. The newest [`DUMPS_SENT`] lean crash snapshots go whole, with the same names blanked to underscores (also
//! in UTF-16) so the file keeps its layout. A note (the player's words, why it was made) goes in as `report.txt`.
//! The save data goes whole under `save/`: `data\save`'s own files (the club card) up to [`SAVE_MAX`] each, never the
//! card's access code (`seat1_aime.txt`) or a `*.tmp`; `aimedb.json` with each access code turned into a short hash, so
//! the Aime ids stay readable; and the 2017-18 game server's store as a consistent copy (`VACUUM INTO`, as the Launcher
//! may have it open), its arcade guests' tokens cut short. The access codes are also replaced in the logs (the Aime
//! stand-ins log them) and blanked in the dumps (also as UTF-16 and packed BCD). When the ZIP is too big the log tails get shorter, down to
//! 64 KB; only if that is still too big the saves are left out and the tails start again from full. `report.txt` says
//! what of the save data was left out. `pc.txt`: Windows, CPU, RAM, GPUs and display (scrubbed like the logs).
//! A hosting PC's arcade logs (`seat_broker.txt`, `match_relay.txt`) are in `data\logs` already.

use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
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
/// A save file bigger than this is left out (it is never cut).
pub const SAVE_MAX: u64 = 3 * 1024 * 1024;

/// This kit's version.txt ("kit-5.5", written by `cargo xtask package`) without "kit-": "5.5"; none: "dev".
pub fn version(kit: &Path) -> String {
    let Ok(text) = fs::read(kit.join("version.txt")) else {
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
    // the last run and the two before it (the launcher's logs::rotate keeps them as folders)
    for sub in ["", "previous", "previous-2", "previous-3", "hook"] {
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
    out.push(("version.txt".into(), kit.join("version.txt")));
    if let Some(game) = game {
        // 2017-18: control's guard log, beside the game folder's wccfnet.dll
        out.push(("game/wccfnet.log".into(), game.join("wccfnet.log")));
        // the Universal Championship's day files (super::championship), today's and yesterday's
        let mut uc = files_in(&game.join("log"));
        uc.retain(|p| p.to_string_lossy().ends_with("_uc.bin"));
        for p in uc.split_off(uc.len().saturating_sub(2)) {
            out.push((
                format!(
                    "game/log/{}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                ),
                p,
            ));
        }
    }
    if let Some(dir) = game.and_then(Path::parent) {
        for p in files_in(&dir.join("seat1")) {
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

/// The save data: `data\save`'s own files (not the access code, not `*.tmp`) and the game server's store. The store's
/// WAL is not here: [`snapshot`] folds it in.
fn saves(kit: &Path) -> Vec<(String, PathBuf)> {
    let save = kit.join("data").join("save");
    let mut out: Vec<(String, PathBuf)> = files_in(&save)
        .into_iter()
        .map(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy();
            (format!("save/{n}"), p)
        })
        .filter(|(n, _)| n != "save/seat1_aime.txt" && !n.ends_with(".tmp"))
        .collect();
    out.push((
        "save/gamesvr/gamesvr.db".into(),
        save.join("gamesvr").join("gamesvr.db"),
    ));
    out.retain(|(_, p)| p.is_file());
    out
}

fn small(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|m| m.len() <= SAVE_MAX)
}

/// [`saves`] as they go in the ZIP, and a line for each one left out.
fn save_data(kit: &Path) -> (Vec<(String, Vec<u8>)>, Vec<String>) {
    let (mut out, mut notes) = (vec![], vec![]);
    let too_big = format!("bigger than {} MB", SAVE_MAX >> 20);
    for (arc, path) in saves(kit) {
        let data = match arc.as_str() {
            _ if !small(&path) => Err(too_big.clone()),
            "save/gamesvr/gamesvr.db" => snapshot(&path),
            "save/aimedb.json" => fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|d| masked_aimedb(&d).ok_or_else(|| "not a JSON object".into())),
            // whole and as they are: a cut or rewritten save is no use
            _ => fs::read(&path).map_err(|e| e.to_string()),
        }
        // the store's snapshot folds in its WAL, so it can outgrow the file on disk
        .and_then(|d| {
            if d.len() as u64 <= SAVE_MAX {
                Ok(d)
            } else {
                Err(too_big.clone())
            }
        });
        match data {
            Ok(d) => out.push((arc, d)),
            Err(e) => notes.push(format!("{arc} left out: {e}")),
        }
    }
    (out, notes)
}

/// The access codes this PC knows (`seat1_aime.txt`, `aimedb.json`'s keys), each as the Aime stand-ins log it: the 20
/// digits and the reader's spaced hex (`50 12 34 ...`).
fn access_codes(kit: &Path) -> Vec<String> {
    let save = kit.join("data").join("save");
    let mut codes: Vec<String> = fs::read_to_string(save.join("seat1_aime.txt"))
        .map(|c| vec![c.trim().to_owned()])
        .unwrap_or_default();
    if let Ok(d) = fs::read(save.join("aimedb.json")) {
        let map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&d).unwrap_or_default();
        codes.extend(map.into_iter().map(|(c, _)| c));
    }
    codes.retain(|c| c.len() >= 8);
    let spaced: Vec<String> = codes
        .iter()
        .filter(|c| c.len() % 2 == 0 && c.is_ascii())
        .map(|c| {
            (0..c.len() / 2)
                .map(|i| &c[i * 2..i * 2 + 2])
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    codes.extend(spaced);
    codes
}

/// A consistent copy of the game server's store (the Launcher's stand-in may hold it open in WAL mode, so a file copy
/// could be torn), the arcade guests' tokens cut to 8 characters: a token owns a guest's club on this host's desk.
fn snapshot(db: &Path) -> Result<Vec<u8>, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let out = dir.path().join("gamesvr.db");
    let run = || -> rusqlite::Result<()> {
        let src = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        src.execute("VACUUM INTO ?1", [out.to_string_lossy()])?;
        drop(src);
        let copy = Connection::open(&out)?;
        // secure_delete: the old tokens must not stay behind in freed pages
        copy.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA secure_delete = ON;")?;
        let guests: bool = copy.query_row(
            "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'guests'",
            [],
            |r| r.get(0),
        )?;
        if guests {
            copy.execute("UPDATE guests SET token = substr(token, 1, 8)", [])?;
        }
        copy.close().map_err(|(_, e)| e)
    };
    run().map_err(|e| e.to_string())?;
    fs::read(&out).map_err(|e| e.to_string())
}

/// `aimedb.json` (access code -> Aime id) with each code replaced by `sha256:` and its SHA-256: the ids stay readable,
/// the codes (which would let anyone play as that card) do not leave the PC.
fn masked_aimedb(data: &[u8]) -> Option<Vec<u8>> {
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(data).ok()?;
    let masked: serde_json::Map<String, serde_json::Value> = map
        .into_iter()
        .map(|(code, id)| {
            let h = Sha256::digest(code.as_bytes());
            let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
            (format!("sha256:{hex}"), id)
        })
        .collect();
    serde_json::to_vec_pretty(&masked).ok()
}

/// What [`pack`] puts in the ZIP, by its names there (the report dialog lists them).
pub fn names(kit: &Path, game: Option<&Path>) -> Vec<String> {
    std::iter::once("pc.txt".to_owned())
        .chain(
            dumps(kit)
                .into_iter()
                .chain(saves(kit).into_iter().filter(|(_, p)| small(p)))
                .chain(files(kit, game))
                .map(|(n, _)| n),
        )
        .collect()
}

/// `pc.txt`: Windows, the CPU (vendor matters: AMD and Intel can drift apart in a match), RAM, GPUs and the main
/// display; "unknown" where a query fails.
pub fn pc_text() -> String {
    let logical = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    let mut t = format!("logical processors: {logical}\n");
    #[cfg(windows)]
    {
        let unknown = |v: Option<String>| v.unwrap_or_else(|| "unknown".into());
        let nt = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let cpu = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";
        t += &format!(
            "windows: {} {}, build {}.{}\ncpu: {}\ncpu vendor: {}\ncpu id: {}\nram: {}\ndisplay: {}\n",
            unknown(reg(nt, "ProductName")),
            unknown(reg(nt, "DisplayVersion")),
            unknown(reg(nt, "CurrentBuild")),
            unknown(reg(nt, "UBR")),
            unknown(reg(cpu, "ProcessorNameString")),
            unknown(reg(cpu, "VendorIdentifier")),
            unknown(reg(cpu, "Identifier")),
            unknown(ram_mb().map(|m| format!("{m} MB"))),
            unknown(display()),
        );
        let mut gpus = 0;
        for i in 0..16 {
            let key = format!(
                r"SYSTEM\CurrentControlSet\Control\Class\{{4d36e968-e325-11ce-bfc1-08002be10318}}\{i:04}"
            );
            if let Some(name) = reg(&key, "DriverDesc") {
                t += &format!(
                    "gpu: {name}, driver {}\n",
                    unknown(reg(&key, "DriverVersion"))
                );
                gpus += 1;
            }
        }
        if gpus == 0 {
            t += "gpu: unknown\n";
        }
    }
    #[cfg(not(windows))]
    {
        t += "windows: unknown\n";
    }
    t
}

/// `pc.txt`'s session part, to tell the players' PCs apart: seat 1's card (its Aime id and a short hash, never the
/// access code), the card's clubs in this PC's store (name, card and coach ids, plays, [`super::club::team_tag`]) and
/// each kept run's `session:` line from run_launcher.txt (the arcade role and id, the seat, the projector).
pub fn session_text(kit: &Path) -> String {
    let save = kit.join("data").join("save");
    let mut t = String::from("\n[session]\n");
    let code = fs::read_to_string(save.join("seat1_aime.txt")).map(|c| c.trim().to_owned());
    match code {
        Ok(code) if !code.is_empty() => {
            let online = crate::standins::aimedb::hash_id(&code);
            let map: serde_json::Map<String, serde_json::Value> =
                fs::read(save.join("aimedb.json"))
                    .ok()
                    .and_then(|d| serde_json::from_slice(&d).ok())
                    .unwrap_or_default();
            let home = map
                .get(&code)
                .and_then(serde_json::Value::as_i64)
                .map_or(online, |v| v as i32);
            let hex: String = Sha256::digest(code.as_bytes())[..6]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            t += &format!("card: sha256:{hex}, Aime id {home} here, {online} online\n");
            t += &clubs_text(&save.join("gamesvr").join("gamesvr.db"), home as u32);
        }
        _ => t += "card: none on this PC\n",
    }
    for sub in ["", "previous", "previous-2", "previous-3"] {
        let log = kit
            .join("data")
            .join("logs")
            .join(sub)
            .join("run_launcher.txt");
        let text = fs::read(&log).unwrap_or_default();
        for l in String::from_utf8_lossy(&text)
            .lines()
            .filter(|l| l.contains("session: "))
        {
            let run = if sub.is_empty() { "last run" } else { sub };
            t += &format!("{run}: {}\n", l.trim());
        }
    }
    t
}

/// One `club:` line per club of `aime` in the store at `db`, read only (the stand-in may hold it open).
fn clubs_text(db: &Path, aime: u32) -> String {
    let club = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|c| super::club::export(&c, aime));
    match club {
        Ok(c) if c.coaches.is_empty() => "club: none yet\n".into(),
        Ok(c) => c
            .coaches
            .iter()
            .map(|k| {
                // the club's name lives in the save block at byte 324 (docs/research/ONLINE-2017-18.md)
                let name = k
                    .save
                    .as_ref()
                    .and_then(|s| s.block.get(324..380))
                    .map_or_else(
                        || super::gamesvr::name_text(&k.name2),
                        super::gamesvr::name_text,
                    );
                let team = k
                    .save
                    .as_ref()
                    .map_or("-".into(), |s| super::club::team_tag(&s.block));
                format!(
                    "club: {name:?} (card {}, coach {}, plays {}, team {team})\n",
                    k.card, k.coach, k.play_count
                )
            })
            .collect(),
        Err(e) => format!("club: unknown ({e})\n"),
    }
}

/// A string or DWORD value under HKEY_LOCAL_MACHINE.
#[cfg(windows)]
fn reg(key: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, REG_DWORD, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, value) = (w(key), w(value));
    let mut buf = [0u16; 512];
    let mut len = size_of_val(&buf) as u32;
    let mut kind = 0;
    // SAFETY: the strings are NUL-terminated and outlive the call; `buf` and `len` describe the output buffer.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_DWORD,
            &mut kind,
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return None;
    }
    if kind == REG_DWORD {
        return Some((u32::from(buf[0]) | u32::from(buf[1]) << 16).to_string());
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]).trim().to_owned()).filter(|s| !s.is_empty())
}

#[cfg(windows)]
fn ram_mb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: a zeroed MEMORYSTATUSEX with its length set, as the call asks
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `m` is a valid, sized MEMORYSTATUSEX
    (unsafe { GlobalMemoryStatusEx(&mut m) } != 0).then_some(m.ullTotalPhys / (1024 * 1024))
}

#[cfg(windows)]
fn display() -> Option<String> {
    use windows_sys::Win32::Graphics::Gdi::{
        DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW,
    };
    // SAFETY: a zeroed DEVMODEW with its size set, as the call asks
    let mut m: DEVMODEW = unsafe { std::mem::zeroed() };
    m.dmSize = size_of::<DEVMODEW>() as u16;
    // SAFETY: a null device name means the main display; `m` is a valid, sized DEVMODEW
    (unsafe { EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, &mut m) } != 0).then(
        || {
            format!(
                "{}x{} at {} Hz",
                m.dmPelsWidth, m.dmPelsHeight, m.dmDisplayFrequency
            )
        },
    )
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

    /// The names as needles in each encoding: UTF-8, then Latin-1 (what the Python takes on Linux; Windows' ANSI
    /// page is close), then the same in lower and upper case (the match itself folds ASCII only: `É` against `é`),
    /// then CP932 (a Japanese Windows' ANSI page, which the 2017-18 game writes); `utf16` adds Windows' own text for
    /// binary files. A form an encoding cannot hold is skipped, one equal to an earlier form ASCII case aside is
    /// dropped, and so is a form [`Self::short`] for `min` by its own script (`ı` upper-cases to a lone `I`).
    fn encodings(name: &str, utf16: bool, min: usize) -> Vec<Needle> {
        // shortcut: whole-string lower and upper case only, so a name mixing cases in two non-ASCII letters ("Éè")
        // is matched only as written, lower or upper; fold per character if such names turn up
        let forms: Vec<String> = [name.to_owned(), name.to_lowercase(), name.to_uppercase()]
            .into_iter()
            .filter(|f| !Self::short(f, min))
            .collect();
        let mut all: Vec<Needle> = vec![];
        for f in &forms {
            all.extend(
                [
                    Needle::new(f, utf8_char, true),
                    Needle::new(f, latin1_char, false),
                ]
                .into_iter()
                .flatten(),
            );
        }
        all.extend(
            forms
                .iter()
                .filter_map(|f| Needle::new(f, cp932_char, false)),
        );
        if utf16 {
            all.extend(
                forms
                    .iter()
                    .filter_map(|f| Needle::new(f, utf16_char, false)),
            );
        }
        let mut v: Vec<Needle> = vec![];
        for n in all {
            if !v.iter().any(|o| o.key() == n.key()) {
                v.push(n);
            }
        }
        v
    }

    /// Too short to take out: under `ascii` characters for an ASCII name, under `ascii - 1` for any other. Text takes
    /// 2 and 1: a 2-letter ASCII name goes as a whole word, a single ASCII letter ("a", "I") would wreck every line
    /// and points at nobody, a single kanji goes. Binary files take 3 and 2, so a blank stays rare in a snapshot.
    fn short(name: &str, ascii: usize) -> bool {
        name.chars().count() < if name.is_ascii() { ascii } else { ascii - 1 }
    }

    /// (needle, replacement, whole word) in the order they apply to text. Only an ASCII form goes as a whole word:
    /// Japanese has no spaces between words, so a non-ASCII name goes wherever it sits; a match inside a longer word
    /// only takes out more text than it must, which a log can lose.
    fn text_pairs(&self) -> Vec<(Needle, &'static [u8], bool)> {
        let forward = self.home.replace('\\', "/");
        let names = [
            (self.home.as_str(), &b"%USERPROFILE%"[..], false),
            (forward.as_str(), b"%USERPROFILE%", false),
            (self.user.as_str(), b"USER", true),
            (self.pc.as_str(), b"PC", true),
        ]
        .map(|(old, new, whole)| (Self::encodings(old, false, 2), new, whole));
        let most = names.iter().map(|n| n.0.len()).max().unwrap_or(0);
        let mut out = vec![];
        // each encoding in turn, the home before the names inside it
        for enc in 0..most {
            for (encs, new, whole) in &names {
                if let Some(n) = encs.get(enc) {
                    out.push((n.clone(), *new, *whole && n.ascii));
                }
            }
        }
        out
    }

    /// Text: the names replaced (case-insensitively), the ASCII user and PC names only as whole words.
    pub fn scrub(&self, mut data: Vec<u8>) -> Vec<u8> {
        // this project writes UTF-8, where a Latin-1 or CP932 form's one byte cuts a character in half (`0xE7` leads
        // 火 and 田, `0xB1` sits inside 米), so those forms run only on data that is not text. A log's tail can start
        // inside a character: up to three of its continuation bytes come first.
        let start = data
            .iter()
            .take(3)
            .take_while(|&&b| matches!(b, 0x80..=0xBF))
            .count();
        let text = std::str::from_utf8(&data[start..]).is_ok();
        for (needle, new, whole) in self.text_pairs() {
            if text && !needle.utf8 {
                continue;
            }
            data = replace(&data, &needle.bytes, &needle.fold, whole, |_| new.to_vec());
        }
        if self.folders {
            data = user_folders(&data);
        }
        data
    }

    /// A binary file: each match turned into as many underscores, nothing moves, so a snapshot stays a valid file.
    pub fn blank(&self, mut data: Vec<u8>) -> Vec<u8> {
        for old in [&self.home, &self.user, &self.pc] {
            for n in Self::encodings(old, true, 3) {
                data = replace(&data, &n.bytes, &n.fold, false, |m| vec![b'_'; m.len()]);
            }
        }
        data
    }
}

/// A name in one encoding: its bytes, which of them may match in either ASCII case (only those of an ASCII
/// character: a CP932 or UTF-16 byte of a kanji is often an ASCII letter, and 人 is not 伸), whether the form is
/// ASCII (then it goes as a whole word), and whether it is the UTF-8 form (the only one this project's text takes).
#[derive(Clone, Debug)]
struct Needle {
    bytes: Vec<u8>,
    fold: Vec<bool>,
    ascii: bool,
    utf8: bool,
}

impl Needle {
    /// `form` one character at a time through `one`; None if a character has no form there.
    fn new(form: &str, one: fn(char) -> Option<Vec<u8>>, utf8: bool) -> Option<Self> {
        let (mut bytes, mut fold) = (vec![], vec![]);
        for c in form.chars() {
            let b = one(c)?;
            fold.extend(std::iter::repeat_n(c.is_ascii(), b.len()));
            bytes.extend(b);
        }
        Some(Self {
            bytes,
            fold,
            ascii: form.is_ascii(),
            utf8,
        })
    }

    /// The bytes with the foldable ones in lower case: two needles with one key match the same text.
    fn key(&self) -> Vec<u8> {
        self.bytes
            .iter()
            .zip(&self.fold)
            .map(|(b, f)| if *f { b.to_ascii_lowercase() } else { *b })
            .collect()
    }
}

fn utf8_char(c: char) -> Option<Vec<u8>> {
    Some(c.to_string().into_bytes())
}

fn latin1_char(c: char) -> Option<Vec<u8>> {
    u8::try_from(u32::from(c)).ok().map(|b| vec![b])
}

/// CP932 is Shift-JIS as Windows has it.
fn cp932_char(c: char) -> Option<Vec<u8>> {
    let s = c.to_string();
    let (b, _, bad) = encoding_rs::SHIFT_JIS.encode(&s);
    (!bad).then(|| b.into_owned())
}

fn utf16_char(c: char) -> Option<Vec<u8>> {
    Some(
        c.encode_utf16(&mut [0; 2])
            .iter()
            .flat_map(|u| u.to_le_bytes())
            .collect(),
    )
}

/// Every `X:\Users\NAME` (any drive, any case, `\`, `/` or doubled separators) turned into `%USERPROFILE%`: the
/// folders of other Windows users, or of this one under another drive letter. NAME runs to the next separator, quote or
/// control character, so a name with spaces goes whole (one cannot start with a space).
///
/// shortcut: CP932 is not decoded, so a name is cut at a kanji whose second byte is a separator (`表` is `95 5C`, so
/// `D:\Users\表田` leaves `田`); decode as CP932 once a collected path holds such a kanji.
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

/// `data` with every non-overlapping match of `needle` (left to right) replaced, the bytes `fold` marks in either ASCII
/// case (one past its end: exact); `whole`: only where no ASCII letter or digit sits on either side. In data that is
/// not UTF-8, that letter or digit may be the second byte of a CP932 character (人 is `90 6C`), which is no word.
fn replace(
    data: &[u8],
    needle: &[u8],
    fold: &[bool],
    whole: bool,
    with: impl Fn(&[u8]) -> Vec<u8>,
) -> Vec<u8> {
    if needle.is_empty() {
        return data.to_vec();
    }
    let alnum = |b: Option<&u8>| b.is_some_and(u8::is_ascii_alphanumeric);
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    // whether the byte at `i` starts a CP932 character, and whether the one before it is the second byte of one
    let (mut start, mut trail) = (true, false);
    while i < data.len() {
        let end = i + needle.len();
        let hit = end <= data.len()
            && data[i..end]
                .iter()
                .zip(needle)
                .enumerate()
                .all(|(k, (a, b))| {
                    a == b || (fold.get(k) == Some(&true) && a.eq_ignore_ascii_case(b))
                })
            && !(whole
                && ((start && !trail && alnum(i.checked_sub(1).and_then(|p| data.get(p))))
                    || (start && alnum(data.get(end)))));
        if hit {
            out.extend(with(&data[i..end]));
            i = end;
            trail = false;
        } else {
            let (b, was) = (data[i], start);
            out.push(b);
            i += 1;
            start = !(was && matches!(b, 0x81..=0x9F | 0xE0..=0xFC));
            trail = !was;
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

/// [`pack`], made again while the ZIP is over `limit` bytes: shorter tails (down to 64 KB a file); if still over,
/// without the save data and from the full tails again.
pub fn pack_within(
    kit: &Path,
    game: Option<&Path>,
    scrub: &Scrub,
    full: usize,
    note: Option<&str>,
    limit: usize,
) -> io::Result<(Vec<u8>, usize)> {
    let pc = scrub.scrub((pc_text() + &session_text(kit)).into_bytes());
    let (saves, save_notes) = save_data(kit);
    let codes = access_codes(kit);
    // a crash dump also holds the codes as the stand-ins keep them: UTF-16 text and the 10 packed BCD bytes
    let dump_codes: Vec<Vec<u8>> = codes
        .iter()
        .flat_map(|c| {
            let mut v = vec![
                c.as_bytes().to_vec(),
                c.encode_utf16().flat_map(u16::to_le_bytes).collect(),
            ];
            if c.len() == 20 && c.is_ascii() {
                v.extend(
                    (0..10)
                        .map(|i| u8::from_str_radix(&c[i * 2..i * 2 + 2], 16).ok())
                        .collect::<Option<Vec<u8>>>(),
                );
            }
            v
        })
        .collect();
    let (mut tail, mut with_saves) = (full, true);
    loop {
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        let mut n = 0;
        let mut notes = save_notes.clone();
        if !with_saves {
            notes.extend(
                saves
                    .iter()
                    .map(|(arc, _)| format!("{arc} left out: the ZIP was too big with it")),
            );
        }
        let mut report = note.map(|t| t.as_bytes().to_vec());
        if !notes.is_empty() {
            let r = report.get_or_insert_with(Vec::new);
            if !r.is_empty() {
                r.extend(b"\n\n");
            }
            r.extend(format!("[SAVE LOGS] {}\n", notes.join("\n[SAVE LOGS] ")).into_bytes());
        }
        if let Some(report) = report {
            z.start_file("report.txt", opts).map_err(zip_err)?;
            z.write_all(&scrub.scrub(report))?;
            n += 1;
        }
        z.start_file("pc.txt", opts).map_err(zip_err)?;
        z.write_all(&pc)?;
        n += 1;
        for (arc, data) in saves.iter().filter(|_| with_saves) {
            z.start_file(arc.as_str(), opts).map_err(zip_err)?;
            z.write_all(data)?;
            n += 1;
        }
        for (arc, path) in dumps(kit) {
            if let Ok(data) = fs::read(&path) {
                z.start_file(arc, opts).map_err(zip_err)?;
                let mut data = scrub.blank(data);
                for code in &dump_codes {
                    // either case: a code may be written in lower or upper case, as the stand-in that logs it does
                    let fold = vec![true; code.len()];
                    data = replace(&data, code, &fold, false, |m| vec![b'_'; m.len()]);
                }
                z.write_all(&data)?;
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
            // before the line that says what was left out goes in: a tail can start inside a character, and
            // [`Scrub::scrub`] reads UTF-8 with such a start at the front as text
            let mut data = scrub.scrub(data);
            for code in &codes {
                // either case: the aimedb stand-in logs its hex in lower case, a reading may be upper
                let fold = vec![true; code.len()];
                data = replace(&data, code.as_bytes(), &fold, false, |_| {
                    b"ACCESS-CODE".to_vec()
                });
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
            z.write_all(&data)?;
            n += 1;
        }
        let blob = z.finish().map_err(zip_err)?.into_inner();
        if blob.len() > limit && tail > 64 * 1024 {
            tail = (tail / 4).max(64 * 1024);
            continue;
        }
        if blob.len() > limit && with_saves && !saves.is_empty() {
            (tail, with_saves) = (full, false);
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
    use proptest::prelude::*;

    type Enc = fn(&str) -> Option<Vec<u8>>;

    /// Whether `data` still holds `needle` in encoding `enc` anywhere, the case of its ASCII characters aside (the way
    /// [`Scrub`] itself matches; the other bytes exactly, as a kanji's CP932 byte may be an ASCII letter).
    fn holds(data: &[u8], needle: &str, enc: Enc) -> bool {
        let (mut b, mut f) = (vec![], vec![]);
        for c in needle.chars() {
            let Some(e) = enc(&c.to_string()) else {
                return false;
            };
            f.extend(std::iter::repeat_n(c.is_ascii(), e.len()));
            b.extend(e);
        }
        !b.is_empty()
            && data.windows(b.len()).any(|w| {
                w.iter()
                    .zip(&b)
                    .zip(&f)
                    .all(|((x, y), f)| x == y || (*f && x.eq_ignore_ascii_case(y)))
            })
    }

    /// `text` in the other two encodings a log takes: Latin-1 (None for a char it cannot hold) and UTF-16LE.
    fn latin1(text: &str) -> Option<Vec<u8>> {
        text.chars()
            .map(|c| u8::try_from(u32::from(c)).ok())
            .collect()
    }

    fn utf16(text: &str) -> Option<Vec<u8>> {
        Some(text.encode_utf16().flat_map(u16::to_le_bytes).collect())
    }

    fn utf8(text: &str) -> Option<Vec<u8>> {
        Some(text.as_bytes().to_vec())
    }

    /// `text` as CP932, None for a char it cannot hold (the test's own copy, so it also runs against older scrubs).
    fn sjis(text: &str) -> Option<Vec<u8>> {
        let (b, _, bad) = encoding_rs::SHIFT_JIS.encode(text);
        (!bad).then(|| b.into_owned())
    }

    /// `s` in one of four cases: lower, upper, as written, or every other letter upper.
    fn vary(s: &str, case: u8) -> String {
        match case {
            0 => s.to_lowercase(),
            1 => s.to_uppercase(),
            2 => s.to_owned(),
            _ => s
                .chars()
                .enumerate()
                .map(|(i, c)| match i % 2 {
                    0 => c.to_uppercase().collect::<String>(),
                    _ => c.to_lowercase().collect(),
                })
                .collect(),
        }
    }

    /// User names as Windows has them: Latin with an accent, ASCII with digits (2 letters and up), kanji, kana (one
    /// character and up).
    fn user_name() -> impl Strategy<Value = String> {
        prop_oneof![
            "[a-z]{2,5}[éeèü]",
            "[A-Za-z][a-z0-9]{1,6}",
            "[太郎花子田中翔]{2,3}",
            "[あいうかき]{1,2}",
        ]
    }

    /// PC names: a word, the `DESKTOP-AB12CD` Windows makes up, two or three letters and digits, kanji.
    fn pc_name() -> impl Strategy<Value = String> {
        prop_oneof![
            "[A-Z][a-z]{2,6}",
            "(DESKTOP|LAPTOP)-[A-Z0-9]{4,7}",
            "[A-Z0-9]{2,3}",
            "[太郎花子田中]{2,3}",
        ]
    }

    /// A name the placeholders spell themselves ("use" in "USER", "PC") cannot be looked for in the output.
    fn in_placeholder(n: &str) -> bool {
        holds(b"USER %USERPROFILE% PC", n, utf8)
    }

    // The scrub's promise, stated as a rule for what these generators make: the user name, the PC name and the home
    // path (with `\` and with `/`), each in any case, sit at any offset of the text, in every encoding it comes in
    // (UTF-8, Latin-1, CP932), and none of them survives; the home goes as one `%USERPROFILE%`, with or without
    // `folders`. A non-ASCII name may touch letters, digits and kana ("glue"), as in Japanese; an ASCII one is
    // generated alone, as the whole-word rule spares a letter beside it. The case of a name is spelled out whole, so
    // the mixed-case `shortcut` above is not exercised here; the UTF-8 text stays UTF-8.
    proptest! {
        #[test]
        fn scrub_leaves_no_name_behind(
            user in user_name(),
            pc in pc_name(),
            drive in "[A-Za-z]",
            (cu, cp, ch) in (0..4u8, 0..4u8, 0..4u8),
            ends in ["[ ._\\\\/:,;()#!\n-]{0,3}"; 2],
            cores in ["[ ._\\\\/:,;()#!\n-]{1,3}"; 3],
            glue in ["[a-zA-Z0-9あ日本]{0,3}"; 4],
            folders in any::<bool>(),
        ) {
            prop_assume!(!in_placeholder(&user) && !in_placeholder(&pc));
            prop_assume!(glue.iter().all(|g| [&user, &pc].iter().all(|n| !holds(g.as_bytes(), n, utf8))));
            let home = format!("{drive}:\\Users\\{user}");
            let scrub = Scrub {
                home: home.clone(),
                user: user.clone(),
                pc: pc.clone(),
                folders,
            };
            let g = |name: &str, i: usize| if name.is_ascii() { "" } else { glue[i].as_str() };
            let (u, p, h) = (vary(&user, cu), vary(&pc, cp), vary(&home, ch));
            let f = h.replace('\\', "/");
            let text = format!(
                "{}{}{u}{}{}{}{p}{}{}{h}{}{f}{}",
                ends[0], g(&user, 0), g(&user, 1), cores[0], g(&pc, 2), g(&pc, 3), cores[1], cores[2], ends[1]
            );
            for (enc, text_enc) in [(utf8 as Enc, true), (latin1, false), (sjis, false)] {
                // only text the encoding can hold is checked in it
                let Some(data) = enc(&text) else { continue };
                let out = scrub.scrub(data.clone());
                // what this project wrote as UTF-8 stays valid UTF-8
                if text_enc {
                    prop_assert!(std::str::from_utf8(&out).is_ok(), "{:?}", String::from_utf8_lossy(&out));
                }
                let shown = String::from_utf8_lossy(&out);
                for needle in [&u, &p, &h, &f] {
                    prop_assert!(holds(&data, needle, enc), "{needle:?} was never in the text");
                    prop_assert!(!holds(&out, needle, enc), "{needle:?} survived in {shown:?}");
                }
                let homes = out.windows(13).filter(|w| w == b"%USERPROFILE%").count();
                prop_assert_eq!(homes, 2, "{:?}", shown);
            }
        }

        // A binary file (`blank`): every encoding, UTF-16LE and upper case included, and nothing moves, so a snapshot
        // stays one.
        #[test]
        fn blank_keeps_the_length_and_every_encoding_goes(
            user in prop_oneof!["[a-z]{2,5}[éeèü]", "[太郎花子田中翔]{2,3}"],
            pc in prop_oneof!["[A-Z][a-z]{2,6}", "(DESKTOP|LAPTOP)-[A-Z0-9]{4,7}"],
            drive in "[A-Za-z]",
            (before, mid, after) in ("[ ._\\\\/:,;()#!-]{0,10}", "[ ._\\\\/:,;()#!-]{0,10}", "[ ._\\\\/:,;()#!-]{0,10}"),
        ) {
            let home = format!("{drive}:\\Users\\{user}");
            let text = format!("{before}{user}{mid}{pc}{after}{home}|");
            let upper = text.to_uppercase();
            let mut dump = vec![];
            for t in [&text, &upper] {
                dump.extend(t.as_bytes());
                dump.extend(latin1(t).unwrap_or_default());
                dump.extend(sjis(t).unwrap_or_default());
                dump.extend(utf16(t).unwrap());
            }
            let scrub = Scrub {
                home: home.clone(),
                user: user.clone(),
                pc: pc.clone(),
                folders: true,
            };
            let out = scrub.blank(dump.clone());
            prop_assert_eq!(out.len(), dump.len());
            for needle in [&user, &pc, &home] {
                for n in [needle.clone(), needle.to_uppercase()] {
                    let encs: [Enc; 4] = [utf8, latin1, sjis, utf16];
                    for enc in encs.into_iter().filter(|e| e(&n).is_some()) {
                        prop_assert!(holds(&dump, &n, enc), "{n:?} was never in the dump");
                        prop_assert!(!holds(&out, &n, enc), "{n:?} left in as {:02x?}", enc(&n));
                    }
                }
            }
        }

        // `aimedb.json` keeps the ids; the codes, which would let anyone play as that card, are the only thing that
        // goes. The 20 digits are the game's own code, far too long to sit inside a 64-character hash by accident.
        #[test]
        fn masked_aimedb_drops_every_access_code(
            codes in prop::collection::btree_map("[0-9]{20}", 0u32..1_000_000, 1..8),
        ) {
            let json = serde_json::to_vec(&codes).unwrap();
            let out = masked_aimedb(&json).unwrap();
            let text = String::from_utf8(out.clone()).unwrap();
            for code in codes.keys() {
                prop_assert!(!text.contains(code.as_str()), "{code} left in");
            }
            let back: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&out).unwrap();
            prop_assert_eq!(back.len(), codes.len());
            prop_assert!(back.iter().all(|(k, _)| k.len() == "sha256:".len() + 64));
            for (code, id) in &codes {
                let hex: String = Sha256::digest(code.as_bytes())
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                let id = serde_json::Value::from(*id);
                prop_assert_eq!(back.get(&format!("sha256:{hex}")), Some(&id));
            }
        }
    }

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
    fn a_cp932_folder_name_keeps_its_tail() {
        // shortcut: 表 is 95 5C, so the separator ends the name and 田 stays; the test pins today's output
        let s = |t: &str| user_folders(&sjis(t).unwrap());
        assert_eq!(
            s(r"D:\Users\表田\x"),
            [b"%USERPROFILE%\\".to_vec(), sjis("田\\x").unwrap()].concat()
        );
    }

    #[test]
    fn two_letter_names_leave_text() {
        let scrub = Scrub {
            home: "C:\\Users\\太郎".into(),
            user: "太郎".into(),
            pc: "Al".into(),
            folders: false,
        };
        let out = scrub.scrub(
            "太郎さん on Al: 山田太郎, Also AL. C:/Users/太郎/x"
                .as_bytes()
                .to_vec(),
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "USERさん on PC: 山田USER, Also PC. %USERPROFILE%/x"
        );
        // a single ASCII letter stays (it would take every "a" with it), a single kana goes
        let one = |user: &str, text: &str| {
            let s = Scrub {
                user: user.into(),
                ..Scrub::default()
            };
            String::from_utf8(s.scrub(text.as_bytes().to_vec())).unwrap()
        };
        assert_eq!(one("a", "a file"), "a file");
        assert_eq!(one("あ", "あさん"), "USERさん");
        // a one-letter name whose other case is a lone ASCII letter keeps only its own form; an ASCII form of two
        // letters goes as a whole word
        assert_eq!(one("ı", "INFO: ı"), "INFO: USER");
        assert_eq!(one("ſ", "Save ſ"), "Save USER");
        assert_eq!(one("ſt", "STOP ST xſty"), "STOP USER xUSERy");
        // a 2-letter ASCII name goes only as a word of its own, so a standalone word of that shape goes too
        assert_eq!(one("Al", "also local Al al"), "also local USER USER");
        assert_eq!(one("An", "an input"), "USER input");
        assert_eq!(
            one("IT", "lobby.wccf.it is not an address"),
            "lobby.wccf.USER is not an address"
        );
    }

    #[test]
    fn a_two_letter_name_beside_a_cp932_kanji_goes() {
        // 人 is 90 6C: the second byte is an "l", and it is no word beside the name
        let scrub = Scrub {
            pc: "Al".into(),
            ..Scrub::default()
        };
        assert_eq!(
            scrub.scrub(sjis("人Al 田Al 年Al").unwrap()),
            sjis("人PC 田PC 年PC").unwrap()
        );
    }

    #[test]
    fn one_byte_names_leave_utf8_text_whole() {
        // ç is E7 in Latin-1 and ｱ is B1 in CP932; in UTF-8 E7 leads 火 and 田, B1 is the middle of 米, so neither
        // form may touch text that decodes
        let text = "クラブ 火 田 生 私 空 終 線 米 ç ｱ";
        for user in ["ç", "ｱ"] {
            let scrub = Scrub {
                user: user.into(),
                ..Scrub::default()
            };
            let out = scrub.scrub(text.as_bytes().to_vec());
            assert_eq!(
                String::from_utf8(out).unwrap(),
                text.replace(user, "USER"),
                "{user}"
            );
        }
        // a tail that starts inside a character is still text
        let scrub = Scrub {
            user: "ç".into(),
            ..Scrub::default()
        };
        let tail = &"生 私".as_bytes()[1..];
        let out = scrub.scrub(tail.to_vec());
        assert_eq!(out, tail, "a tail keeps its bytes");
    }

    #[test]
    fn cp932_names_leave_text() {
        let scrub = Scrub {
            home: "C:\\Users\\田中".into(),
            user: "田中".into(),
            pc: "DESKTOP-AB12CD".into(),
            folders: false,
        };
        let out = scrub.scrub(sjis("田中のクラブ desktop-ab12cd C:\\Users\\田中\\save").unwrap());
        assert_eq!(out, sjis("USERのクラブ PC %USERPROFILE%\\save").unwrap());
        // 人 is 90 6C and 伸 90 4C in CP932: only the ASCII characters of a name match in either case
        let one = Scrub {
            user: "人".into(),
            ..Scrub::default()
        };
        let text = sjis("伸 と人, 伸人").unwrap();
        assert_eq!(one.scrub(text), sjis("伸 とUSER, 伸USER").unwrap());
        let two = Scrub {
            user: "人人".into(),
            ..Scrub::default()
        };
        let want = [sjis("伸伸 ").unwrap(), vec![b'_'; 4]].concat();
        assert_eq!(two.blank(sjis("伸伸 人人").unwrap()), want);
        // the same in UTF-16: Ł is 41 01, š 61 01
        let pl = Scrub {
            user: "Łuk".into(),
            ..Scrub::default()
        };
        let (l, s) = (utf16("ŁUK łuk").unwrap(), utf16("šuk ŠUK").unwrap());
        assert_eq!(
            pl.blank(l),
            [vec![b'_'; 6], utf16(" ").unwrap(), vec![b'_'; 6]].concat()
        );
        assert_eq!(pl.blank(s.clone()), s);
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

    fn read(z: &mut zip::ZipArchive<Cursor<Vec<u8>>>, n: &str) -> Vec<u8> {
        let mut v = vec![];
        z.by_name(n).unwrap().read_to_end(&mut v).unwrap();
        v
    }

    #[test]
    fn saves_go_whole_without_the_access_code_and_pc_txt_is_in() {
        let d = tempfile::tempdir().unwrap();
        let save = d.path().join("data").join("save");
        fs::create_dir_all(save.join("gamesvr")).unwrap();
        // the user name inside a save stays: saves are not rewritten
        let card: Vec<u8> = (0..=255u8).chain(*b"Mario").collect();
        fs::write(save.join("seat1_club.bin"), &card).unwrap();
        fs::write(save.join("big.bin"), vec![0u8; SAVE_MAX as usize + 1]).unwrap();
        let code = "50123456789012345678";
        fs::write(save.join("seat1_aime.txt"), code).unwrap();
        fs::write(save.join("aimedb.json.tmp"), code).unwrap();
        fs::write(save.join("aimedb.json"), format!("{{\"{code}\": 7}}")).unwrap();
        let logs = d.path().join("data").join("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(
            logs.join("aime_log.txt"),
            format!("card access code {code} on the reader\n[00 00 00 00 00 00 50 12 34 56 78 90 12 34 56 78]\n"),
        )
        .unwrap();
        fs::write(
            logs.join("run_launcher.txt"),
            "14:31:02  session: role guest, mode remote, arcade abc, seat 2, projector off\n",
        )
        .unwrap();
        // the stand-in holds the store open in WAL mode: the rows sit in gamesvr.db-wal, not yet in gamesvr.db
        let live = Connection::open(save.join("gamesvr").join("gamesvr.db")).unwrap();
        live.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;
             CREATE TABLE guests (aime_id INTEGER PRIMARY KEY, token TEXT NOT NULL);
             INSERT INTO guests VALUES (42, 'abcdef0123456789abcdef');",
        )
        .unwrap();
        let scrub = Scrub {
            user: "Mario".into(),
            ..Scrub::default()
        };
        let names = names(d.path(), None);
        for n in [
            "pc.txt",
            "save/seat1_club.bin",
            "save/aimedb.json",
            "save/gamesvr/gamesvr.db",
        ] {
            assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
        }
        for n in [
            "save/big.bin",
            "save/seat1_aime.txt",
            "save/aimedb.json.tmp",
            "save/gamesvr/gamesvr.db-wal",
        ] {
            assert!(!names.iter().any(|x| x == n), "{n} not in {names:?}");
        }
        let (blob, _) = pack(d.path(), None, &scrub, TAIL, None).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(blob)).unwrap();
        let report = String::from_utf8(read(&mut z, "report.txt")).unwrap();
        assert_eq!(
            report,
            "[SAVE LOGS] save/big.bin left out: bigger than 3 MB\n"
        );
        let log = String::from_utf8(read(&mut z, "logs/aime_log.txt")).unwrap();
        assert!(
            !log.contains("0123456789") && !log.contains("34 56 78 90"),
            "{log}"
        );
        assert_eq!(read(&mut z, "save/seat1_club.bin"), card);
        let aimedb = String::from_utf8(read(&mut z, "save/aimedb.json")).unwrap();
        assert!(
            !aimedb.contains("0123456789")
                && aimedb.contains("\"sha256:")
                && aimedb.contains(": 7"),
            "{aimedb}"
        );
        let copy = d.path().join("copy.db");
        fs::write(&copy, read(&mut z, "save/gamesvr/gamesvr.db")).unwrap();
        let token: String = Connection::open(&copy)
            .unwrap()
            .query_row("SELECT token FROM guests WHERE aime_id = 42", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(token, "abcdef01");
        assert!(
            !fs::read(&copy)
                .unwrap()
                .windows(10)
                .any(|w| w == b"abcdef0123")
        );
        // the session header: the card as its Aime id and a hash, never the access code, and the run's role
        let pc = String::from_utf8(read(&mut z, "pc.txt")).unwrap();
        for want in [
            "logical processors: ",
            "\n[session]\ncard: sha256:",
            ", Aime id 7 here, ",
            "\nclub: ",
            "last run: 14:31:02  session: role guest, mode remote, arcade abc, seat 2, projector off\n",
        ] {
            assert!(pc.contains(want), "{want:?} in {pc}");
        }
        assert!(!pc.contains("0123456789"), "{pc}");
        // a closed WAL store (no -shm left) copies too
        drop(live);
        let (blob, _) = pack(d.path(), None, &scrub, TAIL, None).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(blob)).unwrap();
        assert!(z.by_name("save/gamesvr/gamesvr.db").is_ok());
    }

    #[test]
    fn a_store_whose_snapshot_outgrows_the_limit_is_left_out_with_a_note() {
        let d = tempfile::tempdir().unwrap();
        let save = d.path().join("data").join("save");
        fs::create_dir_all(save.join("gamesvr")).unwrap();
        fs::write(save.join("seat1_club.bin"), b"club").unwrap();
        // the rows sit in a big uncheckpointed WAL: gamesvr.db itself stays small
        let live = Connection::open(save.join("gamesvr").join("gamesvr.db")).unwrap();
        live.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0; CREATE TABLE t (b BLOB);",
        )
        .unwrap();
        live.execute(
            "INSERT INTO t VALUES (?1)",
            [noise(SAVE_MAX as usize + 1024, 5)],
        )
        .unwrap();
        assert!(small(&save.join("gamesvr").join("gamesvr.db")));
        let (blob, _) = pack(d.path(), None, &Scrub::default(), TAIL, None).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(blob)).unwrap();
        assert!(z.by_name("save/gamesvr/gamesvr.db").is_err());
        assert_eq!(read(&mut z, "save/seat1_club.bin"), b"club");
        assert_eq!(
            String::from_utf8(read(&mut z, "report.txt")).unwrap(),
            "[SAVE LOGS] save/gamesvr/gamesvr.db left out: bigger than 3 MB\n"
        );
        drop(live);
    }

    #[test]
    fn dumps_lose_the_access_code_in_every_form_and_keep_their_length() {
        let d = tempfile::tempdir().unwrap();
        let (save, dumps) = (d.path().join("data/save"), d.path().join("data/dumps"));
        fs::create_dir_all(&save).unwrap();
        fs::create_dir_all(&dumps).unwrap();
        let code = "50123456789012345678";
        fs::write(save.join("seat1_aime.txt"), code).unwrap();
        let bcd = [0x50, 0x12, 0x34, 0x56, 0x78, 0x90, 0x12, 0x34, 0x56, 0x78];
        let utf16: Vec<u8> = code.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let forms: [&[u8]; 4] = [
            code.as_bytes(),
            b"50 12 34 56 78 90 12 34 56 78",
            &bcd,
            &utf16,
        ];
        let mut dump = b"MDMP".to_vec();
        for f in forms {
            dump.extend_from_slice(f);
            dump.extend_from_slice(&[0xEE; 8]);
        }
        fs::write(dumps.join("x_lean.dmp"), &dump).unwrap();
        let (blob, _) = pack(d.path(), None, &Scrub::default(), TAIL, None).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(blob)).unwrap();
        let out = read(&mut z, "dumps/x_lean.dmp");
        assert_eq!(out.len(), dump.len());
        assert!(out.starts_with(b"MDMP"));
        for f in forms {
            assert!(!out.windows(f.len()).any(|w| w == f), "{f:02x?} left in");
        }
    }

    /// Bytes that do not compress (xorshift).
    fn noise(len: usize, mut x: u64) -> Vec<u8> {
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    #[test]
    fn logs_get_shorter_before_the_saves_go() {
        let d = tempfile::tempdir().unwrap();
        let (logs, save) = (d.path().join("data/logs"), d.path().join("data/save"));
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(&save).unwrap();
        fs::write(logs.join("run.log"), noise(1024 * 1024, 1)).unwrap();
        fs::write(save.join("seat1_club.bin"), noise(4112, 2)).unwrap();
        let log_len = |z: &mut zip::ZipArchive<Cursor<Vec<u8>>>| read(z, "logs/run.log").len();
        // 1 MB of log does not fit in 300 KB: the tails shrink (2 MB, 512 KB, 128 KB) and the card stays
        let (blob, _) =
            pack_within(d.path(), None, &Scrub::default(), TAIL, None, 300 * 1024).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(blob)).unwrap();
        assert_eq!(read(&mut z, "save/seat1_club.bin").len(), 4112);
        assert!((128 * 1024..140 * 1024).contains(&log_len(&mut z)));
        // saves too big even with 64 KB tails: they go, and the tails start again from full
        fs::write(save.join("big.bin"), noise(400 * 1024, 3)).unwrap();
        let (blob, _) = pack_within(
            d.path(),
            None,
            &Scrub::default(),
            TAIL,
            Some("hi"),
            300 * 1024,
        )
        .unwrap();
        assert!(blob.len() <= 300 * 1024);
        let mut z = zip::ZipArchive::new(Cursor::new(blob)).unwrap();
        assert!(z.file_names().all(|n| !n.unwrap().starts_with("save/")));
        assert!((128 * 1024..140 * 1024).contains(&log_len(&mut z)));
        let report = String::from_utf8(read(&mut z, "report.txt")).unwrap();
        assert!(
            report.starts_with(
                "hi\n\n[SAVE LOGS] save/big.bin left out: the ZIP was too big with it\n\
                 [SAVE LOGS] save/seat1_club.bin left out: the ZIP was too big with it\n"
            ),
            "{report}"
        );
    }

    #[test]
    fn the_championship_day_files_go_today_and_yesterday() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("game").join("log");
        fs::create_dir_all(&log).unwrap();
        for n in [
            "20261031_uc.bin",
            "20261101_uc.bin",
            "20261102_uc.bin",
            "otameshi.bin",
        ] {
            fs::write(log.join(n), b"x").unwrap();
        }
        let names = names(d.path(), Some(&d.path().join("game")));
        let uc: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with("game/log/"))
            .collect();
        assert_eq!(uc, ["game/log/20261101_uc.bin", "game/log/20261102_uc.bin"]);
    }

    #[test]
    fn zip_name_has_the_date_and_time() {
        let t = time::OffsetDateTime::from_unix_timestamp(1_793_091_903).unwrap();
        assert_eq!(zip_name(t), "forza-wccf-logs-20261027-090503.zip");
    }
}
