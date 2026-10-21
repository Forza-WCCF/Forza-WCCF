//! The game's text in English or Italian, built on the player's PC from their own game files and swapped in with the
//! originals kept (`on`, `off`, `check`). Ported from `legacy/2010-11/code/english.py` and
//! `legacy/2017-18/kit/scripts/english.py`: the backup manifest, the fingerprints, `put`/`put_field`, on/off/undo and
//! the sheet loading are shared; `Edition` selects the packers.
//!
//! 2010-11 (Rev D): `string_list.bin/.hf`, `player_data.bin` (short and card names), `cpu_team/cpu_N.dat`, and text
//! inside `client_Release.exe` (projector and seat 1's copy) and `control_Release.exe`. 2017-18 (2.00.00):
//! `string_list.bin/.hf` and the clients' `StringListTableRecord.bin`, `player_data.bin`, `cpu_team/cpu_N.dat`.
//!
//! The manifest (`<backup>/manifest.json`, `{rel: {original, english}}`, indent 1) and `language.txt` are the files
//! the Python scripts write and read, so a game switched by one can be switched back by the other. The pictures with
//! writing (`data/wccf_data.xaf`, [`pictures`]): 2017-18 redraws them on `on` as its script did; 2010-11 keeps them
//! off as its script did, and both editions take back pictures a Python kit added. `selftest` is not ported.

use crate::Edition;

mod bcn;
mod coach_title;
mod pictures;
mod pil;

const PICTURES_OFF: &str = "pictures stay Japanese";
use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};
use std::sync::LazyLock;
use unicode_normalization::{UnicodeNormalization, char::canonical_combining_class};

/// A refusal or a failed check; nothing changed.
#[derive(Debug)]
pub struct Failed(pub String);

impl fmt::Display for Failed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<io::Error> for Failed {
    fn from(e: io::Error) -> Self {
        Self(e.to_string())
    }
}

type R<T> = Result<T, Failed>;

fn fail<T>(s: impl Into<String>) -> R<T> {
    Err(Failed(s.into()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    It,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::It => "it",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::It => "Italian",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    On,
    Off,
    Check,
}

// ---------------------------------------------------------------- the editions' constants
const BIN: &str = "data/string/string_list.bin";
const HF: &str = "data/string/string_list.hf";
const ENUM: &str = "prog_data/enumdata/StringListTableRecord.bin";
const PLAYERS: &str = "prog_data/player_data/player_data.bin";
const TEAMS: &str = "prog_data/cpu_team";
const PROJECTOR: &str = "client_Release.exe";
const SEAT_EXE: &str = "seat1/client_Release.exe";
const SERVER: &str = "control_Release.exe";
const NAME_AT: usize = 0x144;
const NAME_END: usize = 0x17C;
const FIELD: usize = 0x40;

/// Families the game may use as data: kept Japanese unless switched on (the server keys its weather table by the
/// country and prefecture text).
const DATA_FAMILIES: [&str; 10] = [
    "MAKE_",
    "NET_RANK",
    "GAM_OROGINAL",
    "GAM_REAL",
    "GAM_STA",
    "SYS_TEAM",
    "INF_SUPPORTER",
    "SYS_COACH",
    "PRO_PRO",
    "INF_FRE",
];
const SWITCHED_ON: [&str; 10] = [
    "SYS_TEAM",
    "PRO_PRO",
    "GAM_OROGINAL",
    "GAM_REAL",
    "GAM_STA",
    "INF_FRE",
    "SYS_COACH",
    "INF_SUPPORTER",
    "MAKE_city",
    "MAKE_area",
];

struct Spec {
    /// player_data.bin: one record's size and the fields' offsets
    rec: usize,
    short_kana: usize,
    short_latin: usize,
    skill_en: usize,
    skill_jp: usize,
    /// 2010-11 only: the full Latin name and the card name it goes over
    full: Option<(usize, usize)>,
    originals: Vec<(&'static str, &'static str)>,
}

fn spec(e: Edition) -> Spec {
    match e {
        Edition::E1011 => Spec {
            rec: 0x328,
            short_kana: 0x8C,
            short_latin: 0xCC,
            skill_en: 0x1B7,
            skill_jp: 0x1F7,
            full: Some((0x0C, 0x4C)),
            originals: vec![
                (
                    BIN,
                    "F71368483B76E3DA89AA4651B33BFDE6BFEE9A3C381F9EEC8CA6E9E8C9DF43F4",
                ),
                (
                    HF,
                    "B6A56A3DFC6494017151371F4313FCAC8BDF80C5B83B3DD9D84A652CC25BAF2A",
                ),
                (
                    PLAYERS,
                    "B0D39E32C195C60E9D366071A94932CBCE4E2EEAF3C83A897664F147161C2519",
                ),
                (
                    PROJECTOR,
                    "AEF9998D8A77B4DD7A0CFF3753A11FC8689A6700E133265D6681EF28D3E9CEA4",
                ),
                (
                    SEAT_EXE,
                    "6243C394E9A57B33C23519326EF8A77B0992873A0033B5892A9155C5B827E20A",
                ),
                (
                    SERVER,
                    "23B9553096BB195FC991A1FC141D719DF3C8AE88460959A88CDFC18863A1376F",
                ),
            ],
        },
        Edition::E1718 => Spec {
            rec: 0x620,
            short_kana: 0x330,
            short_latin: 0x370,
            skill_en: 0x45F,
            skill_jp: 0x49F,
            full: None,
            originals: vec![
                (
                    BIN,
                    "EAB584BA8F96698F0FC438DD8C46282B852E5B7C72F885110FEB189FD004CE43",
                ),
                (
                    HF,
                    "23D63FE4468D7F820CA8CF4424F1995C4FEA02C90827015DD1FB936BADE61D43",
                ),
                (
                    ENUM,
                    "574E40B205632DC23E28852CBAD87DF45711EF05A46513F8FADF4E237BB93495",
                ),
                (coach_title::TITLES, coach_title::TITLES_SHA),
                (
                    PLAYERS,
                    "1F8D79BC980D9E67B3E2F6A975F1A669D4CE3C7EBB413DA09B4A8D5F5DC6FFFA",
                ),
            ],
        },
    }
}

/// Every conversion the C runtime takes, so a line keeps exactly the codes of the game's Japanese line.
static FMT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"%[-+ 0#]*(?:\*|[0-9]+)?(?:\.(?:\*|[0-9]+))?(?:hh|h|ll|l|L|I32|I64|I|w)?[cCdiouxXeEfgGaAnpsSZ%]")
        .unwrap()
});
/// `$c[RRGGBBAA] .. $c` colour, `$p[..] $p`, `$t[..] $t`.
static CODES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$[A-Za-z](?:\[[^\]]*\])?").unwrap());

pub(crate) fn sha(d: &[u8]) -> String {
    Sha256::digest(d)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

// ---------------------------------------------------------------- the working set
/// A game folder, the backup folder (`data/english_backup` of the kit) and the edition's sheets (`editions/<ed>`).
pub struct Ctx {
    ed: Edition,
    spec: Spec,
    game: PathBuf,
    backup: PathBuf,
    sheets: PathBuf,
    /// cpu_names.tsv: (file, original sha, english)
    cpu: Vec<(String, String, String)>,
}

type Row = HashMap<String, String>;

fn cell<'a>(r: &'a Row, k: &str) -> &'a str {
    r.get(k).map_or("", String::as_str)
}

impl Ctx {
    pub fn new(ed: Edition, game: &Path, backup: &Path, sheets: &Path) -> R<Self> {
        let mut c = Self {
            ed,
            spec: spec(ed),
            game: game.into(),
            backup: backup.into(),
            sheets: sheets.into(),
            cpu: vec![],
        };
        c.cpu = c
            .sheet("cpu_names.tsv", "english")?
            .iter()
            .map(|r| {
                (
                    cell(r, "file").into(),
                    cell(r, "original_sha256").into(),
                    cell(r, "english").into(),
                )
            })
            .collect();
        Ok(c)
    }

    /// A TSV in the edition's english or italian folder: 2010-11 keeps them under `languages/`.
    fn sheet(&self, name: &str, folder: &str) -> R<Vec<Row>> {
        let mut p = self.sheets.clone();
        if self.ed == Edition::E1011 {
            p.push("languages");
        }
        p.push(folder);
        p.push(name);
        let mut rd = csv::ReaderBuilder::new()
            .delimiter(b'\t')
            .quoting(false)
            .flexible(true)
            .from_path(&p)
            .map_err(|e| Failed(format!("{}: {e}", p.display())))?;
        let head: Vec<String> = rd
            .headers()
            .map_err(|e| Failed(format!("{}: {e}", p.display())))?
            .iter()
            .map(String::from)
            .collect();
        let mut out = vec![];
        for rec in rd.records() {
            let rec = rec.map_err(|e| Failed(format!("{}: {e}", p.display())))?;
            out.push(
                head.iter()
                    .cloned()
                    .zip(rec.iter().map(String::from))
                    .collect(),
            );
        }
        Ok(out)
    }

    /// A file's place: under the game folder, or `seat1/...` = seat 1's folder beside it.
    fn at(&self, rel: &str) -> PathBuf {
        match rel.strip_prefix("seat1/") {
            Some(r) => self
                .game
                .parent()
                .unwrap_or(Path::new(""))
                .join("seat1")
                .join(r),
            None => self.game.join(rel),
        }
    }

    /// The original: the backup once English has been on, else the game's own file (a program's hook import
    /// renamed back to WINMM.dll, so the bytes are the original's whether or not setup has renamed it).
    fn original_file(&self, rel: &str) -> R<Vec<u8>> {
        let b = self.backup.join(rel);
        let d = fs::read(if b.is_file() { b } else { self.at(rel) })?;
        norm(d, rel)
    }

    fn original_hash(&self, rel: &str) -> Option<String> {
        if let Some((_, h)) = self.spec.originals.iter().find(|(r, _)| *r == rel) {
            return Some((*h).into());
        }
        let f = rel.strip_prefix(TEAMS)?.strip_prefix('/')?;
        self.cpu
            .iter()
            .find(|(n, ..)| n == f)
            .map(|(_, h, _)| h.clone())
    }
}

// ---------------------------------------------------------------- the programs' hook import (helpers/hook.py)
const HOOK_DLL: &[u8] = b"wccfk.dll";
const SYS_DLL: &[u8] = b"WINMM.dll";

/// File offset of the import-descriptor name equal to `dll` (any case).
fn import_name_offset(d: &[u8], dll: &[u8]) -> Option<usize> {
    let u32at = |o: usize| {
        Some(u32::from_le_bytes(
            d.get(o..o.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    let u16at =
        |o: usize| Some(u16::from_le_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?) as usize);
    let pe = u32at(0x3C)? as usize;
    let opt = pe + 24;
    let sec0 = opt + u16at(pe + 20)?;
    let imp_rva = u32at(opt + 104)?;
    let secs: Vec<(u32, u32, u32, u32)> = (0..u16at(pe + 6)?)
        .map(|i| {
            Some((
                u32at(sec0 + 40 * i + 8)?,
                u32at(sec0 + 40 * i + 12)?,
                u32at(sec0 + 40 * i + 16)?,
                u32at(sec0 + 40 * i + 20)?,
            ))
        })
        .collect::<Option<_>>()?;
    let off = |rva: u32| {
        secs.iter()
            .find(|(vs, va, rs, _)| *va <= rva && rva < va + (*vs).max(*rs))
            .map(|(_, va, _, raw)| (rva - va + raw) as usize)
    };
    let mut p = off(imp_rva)?;
    loop {
        let name_rva = u32at(p + 12)?;
        if u32at(p)? == 0 && name_rva == 0 {
            return None;
        }
        if name_rva != 0
            && let Some(s) = off(name_rva)
            && let Some(n) = d.get(s..).and_then(|t| t.iter().position(|&b| b == 0))
            && d[s..s + n].eq_ignore_ascii_case(dll)
        {
            return Some(s);
        }
        p += 20;
    }
}

fn rename_import(d: &[u8], frm: &[u8], to: &[u8]) -> R<Vec<u8>> {
    let Some(s) = import_name_offset(d, frm) else {
        return fail(if import_name_offset(d, to).is_some() {
            format!("already renamed to {}", String::from_utf8_lossy(to))
        } else {
            format!(
                "this program does not import {}",
                String::from_utf8_lossy(frm)
            )
        });
    };
    let mut out = d.to_vec();
    out[s..s + frm.len()].fill(0);
    out[s..s + to.len()].copy_from_slice(to);
    Ok(out)
}

fn is_exe(rel: &str) -> bool {
    rel.to_ascii_lowercase().ends_with(".exe")
}

fn norm(d: Vec<u8>, rel: &str) -> R<Vec<u8>> {
    if is_exe(rel) && import_name_offset(&d, HOOK_DLL).is_some() {
        rename_import(&d, HOOK_DLL, SYS_DLL)
    } else {
        Ok(d)
    }
}

/// After a program is placed: its import renamed to the kit's hook, so it loads wccfk.dll. Idempotent.
fn apply_hook(rel: &str, path: &Path) -> R<()> {
    if is_exe(rel) {
        let d = fs::read(path)?;
        if import_name_offset(&d, HOOK_DLL).is_none() {
            put(path, &rename_import(&d, SYS_DLL, HOOK_DLL)?)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- bytes of the screen text
fn splitlines(b: &[u8]) -> Vec<&[u8]> {
    let (mut out, mut s, mut i) = (vec![], 0, 0);
    while i < b.len() {
        if b[i] == b'\n' || b[i] == b'\r' {
            i += if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            out.push(&b[s..i]);
            s = i;
        } else {
            i += 1;
        }
    }
    if s < b.len() {
        out.push(&b[s..]);
    }
    out
}

fn words(line: &[u8]) -> Vec<&[u8]> {
    line.split(|b| b.is_ascii_whitespace())
        .filter(|w| !w.is_empty())
        .collect()
}

/// The NUL-ended string at `p` (Python's `body[p:body.find(b"\0", p)]`).
fn at(body: &[u8], p: usize) -> &[u8] {
    let end = body
        .get(p..)
        .and_then(|t| t.iter().position(|&b| b == 0))
        .map_or(body.len().saturating_sub(1), |n| p + n);
    body.get(p..end).unwrap_or(&[])
}

type Keys = HashMap<String, (usize, Vec<usize>)>;

fn read_strings(raw: &[u8], hf_raw: &[u8]) -> R<(Vec<u8>, Keys)> {
    let mut keys = Keys::new();
    for (i, line) in splitlines(hf_raw).into_iter().enumerate().skip(1) {
        let w = words(line);
        let Some((k, nums)) = w.split_first() else {
            continue;
        };
        let nums = nums
            .iter()
            .map(|x| std::str::from_utf8(x).ok().and_then(|x| x.parse().ok()))
            .collect::<Option<Vec<usize>>>();
        let (Some(nums), Ok(k)) = (nums, std::str::from_utf8(k)) else {
            return fail("string_list.hf: a line is not a key and numbers");
        };
        keys.insert(k.into(), (i, nums));
    }
    Ok((raw.get(8..).unwrap_or(&[]).to_vec(), keys))
}

fn blocked(k: &str) -> bool {
    let f = k.splitn(3, '_').take(2).collect::<Vec<_>>().join("_");
    DATA_FAMILIES.iter().any(|d| f.starts_with(d)) && !SWITCHED_ON.iter().any(|e| f.starts_with(e))
}

fn cp932(b: &[u8]) -> String {
    encoding_rs::SHIFT_JIS
        .decode_without_bom_handling(b)
        .0
        .into_owned()
}

fn has_high(b: &[u8]) -> bool {
    b.iter().any(|&c| c >= 0x80)
}

/// The line as the game's bytes, or what is wrong with it: a line keeps exactly the printf and `$` codes of the
/// game's Japanese line.
fn encoded(ed: Edition, jp: &str, text: &str, nl: &[u8]) -> Result<Vec<u8>, String> {
    let sorted = |r: &Regex, s: &str| {
        let mut v: Vec<String> = r.find_iter(s).map(|m| m.as_str().into()).collect();
        v.sort();
        v
    };
    let fmt = |s: &str| {
        FMT.find_iter(s)
            .map(|m| m.as_str().to_string())
            .collect::<Vec<_>>()
    };
    if text.trim() == "<empty>" {
        let codes = match ed {
            Edition::E1011 => jp.contains("$c") || jp.contains("$p"),
            Edition::E1718 => CODES.is_match(jp),
        };
        return if FMT.is_match(jp) || codes {
            Err("<empty> would drop the Japanese line's codes".into())
        } else {
            Ok(vec![])
        };
    }
    let en = text.replace("\\n", "\n");
    if fmt(jp) != fmt(&en) {
        return Err(format!(
            "codes {:?}, the game's line has {:?}",
            fmt(&en),
            fmt(jp)
        ));
    }
    if FMT.replace_all(&en, "").matches('%').count() > FMT.replace_all(jp, "").matches('%').count()
    {
        return Err("a % sign that is no printf code - the game would show junk or stop".into());
    }
    match ed {
        Edition::E1011 => {
            // every $ code letter ($c $p $t ...) as often as the game's line has it; a colour's value may differ
            let letters = |s: &str| {
                let mut v: Vec<String> = CODES
                    .find_iter(s)
                    .map(|m| m.as_str()[..2].to_string())
                    .collect();
                v.sort();
                v
            };
            if jp.matches("$c[").count() != en.matches("$c[").count() || letters(jp) != letters(&en)
            {
                return Err("$ codes (colour, layout) differ from the game's line".into());
            }
        }
        Edition::E1718 => {
            if sorted(&CODES, jp) != sorted(&CODES, &en) {
                return Err(format!(
                    "$ codes {:?}, the game's line has {:?}",
                    sorted(&CODES, &en),
                    sorted(&CODES, jp)
                ));
            }
        }
    }
    let (b, _, bad) = encoding_rs::SHIFT_JIS.encode(&en);
    if bad {
        return Err("a letter the game's font cannot show".into());
    }
    let mut b = b.into_owned();
    if ed == Edition::E1011 && nl != b"\n" {
        b = b.split(|&c| c == b'\n').collect::<Vec<_>>().join(nl);
    }
    if b.contains(&0) {
        return Err("holds an end byte".into());
    }
    Ok(b)
}

type Choices = BTreeMap<String, Vec<(String, &'static str)>>;

impl Ctx {
    /// Each Japanese line's choices, best first. 2010-11: one English line per key (the kit's, else the original's
    /// second column, else the original European English), Italian in front of it. 2017-18: the kit's Italian, the
    /// kit's English, then the original's as the next choices.
    fn choices(&self, body: &[u8], keys: &Keys, lang: Lang) -> R<Choices> {
        let mut ch = Choices::new();
        let mut kit = HashSet::new();
        for r in self.sheet("screen_text.tsv", "english")? {
            if !cell(&r, "english").trim().is_empty() && kit.insert(cell(&r, "key").to_string()) {
                ch.insert(
                    cell(&r, "key").into(),
                    vec![(cell(&r, "english").trim_matches('\r').into(), "kit")],
                );
            }
        }
        let rstring: HashMap<String, String> = self
            .sheet("original_rstring.tsv", "english")?
            .into_iter()
            .map(|r| (cell(&r, "key").into(), cell(&r, "english").into()))
            .collect();
        let e1718 = self.ed == Edition::E1718;
        let mut sorted_keys: Vec<_> = keys.iter().collect();
        sorted_keys.sort_by(|a, b| a.0.cmp(b.0));
        let from_original = |ch: &mut Choices| {
            for (k, (_, nums)) in &sorted_keys {
                if (!e1718 && kit.contains(*k)) || !has_high(at(body, nums[0])) {
                    continue;
                }
                let s2 = at(body, nums[1]);
                let c = if !s2.is_empty() && !has_high(s2) {
                    (
                        String::from_utf8_lossy(s2).into_owned(),
                        "original second column",
                    )
                } else if let Some(r) = rstring.get(*k) {
                    (r.clone(), "original European English")
                } else {
                    continue;
                };
                ch.entry((*k).clone()).or_default().push(c);
            }
        };
        let italian = |ch: &mut Choices, folder_sheet: &str, src: &'static str| -> R<()> {
            for r in self.sheet(folder_sheet, "italian")? {
                if !cell(&r, "italiano").trim().is_empty() {
                    ch.entry(cell(&r, "key").into())
                        .or_default()
                        .insert(0, (cell(&r, "italiano").trim_matches('\r').into(), src));
                }
            }
            Ok(())
        };
        if e1718 {
            if lang == Lang::It {
                italian(&mut ch, "screen_text.tsv", "kit Italian")?;
            }
            from_original(&mut ch);
        } else {
            from_original(&mut ch);
            if lang == Lang::It {
                italian(&mut ch, "original_rstring.tsv", "original European Italian")?;
                italian(&mut ch, "screen_text.tsv", "kit Italian")?;
            }
        }
        Ok(ch)
    }
}

/// {key: bytes} and {key: source} for every line with a choice that fits, the first that does; the kit's lines that
/// did not fit (they give way to the next choice), and the unusable original ones.
/// Offsets of the packed strings, the keys that spilled, and the free (start, end) gaps.
type Slots<'a> = (BTreeMap<String, usize>, Vec<&'a String>, Vec<(i64, i64)>);
/// What `encode_lines` returns: encoded lines, the choice taken per key, the keys that did not fit, a count.
type Encoded = (
    BTreeMap<String, Vec<u8>>,
    HashMap<String, &'static str>,
    Vec<String>,
    usize,
);
fn encode_lines(ed: Edition, body: &[u8], keys: &Keys, ch: &Choices, nl: &[u8]) -> Encoded {
    let (mut enc, mut used, mut misfit, mut original_bad) =
        (BTreeMap::new(), HashMap::new(), vec![], 0);
    for (k, options) in ch {
        let Some((_, nums)) = keys.get(k) else {
            misfit.extend(
                options
                    .iter()
                    .filter(|(_, s)| s.starts_with("kit"))
                    .map(|_| format!("{k}: no such line in the game")),
            );
            continue;
        };
        if blocked(k) {
            continue;
        }
        let jp = cp932(at(body, nums[0]));
        for (text, src) in options {
            match encoded(ed, &jp, text, nl) {
                Ok(b) => {
                    enc.insert(k.clone(), b);
                    used.insert(k.clone(), *src);
                    break;
                }
                Err(p) if src.starts_with("kit") => misfit.push(if ed == Edition::E1718 {
                    format!("{k} ({src}): {p}")
                } else {
                    format!("{k}: {p}")
                }),
                Err(_) => original_bad += 1,
            }
        }
    }
    (enc, used, misfit, original_bad)
}

/// New string_list.bin, .hf and the offset of each packed key.
type Packed = (Vec<u8>, Vec<u8>, BTreeMap<String, usize>);
/// string_list.bin/.hf with slot 0 (Japanese) of each key in `enc` replaced; same size, read back.
fn pack_strings(raw: &[u8], hf_raw: &[u8], enc: &BTreeMap<String, Vec<u8>>) -> R<Packed> {
    let (body, keys) = read_strings(raw, hf_raw)?;
    let hf_lines = splitlines(hf_raw);
    let mut refs: HashMap<usize, HashSet<(&str, usize)>> = HashMap::new();
    for (k, (_, nums)) in &keys {
        for (s, n) in nums.iter().enumerate() {
            refs.entry(*n).or_default().insert((k, s));
        }
    }
    let mut ref_sorted: Vec<usize> = refs.keys().copied().collect();
    ref_sorted.sort_unstable();
    let mut own: HashMap<&str, (usize, usize)> = HashMap::new();
    for k in enc.keys() {
        let Some((_, nums)) = keys.get(k) else {
            return fail(format!("no such line {k}"));
        };
        let p = nums[0];
        let end = p + at(&body, p).len() + 1;
        let (lo, hi) = (
            ref_sorted.partition_point(|&x| x < p),
            ref_sorted.partition_point(|&x| x < end),
        );
        if refs[&p].len() == 1 && refs[&p].contains(&(k.as_str(), 0)) && hi - lo == 1 {
            own.insert(k, (p, end));
        }
    }
    let used_len = body.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let pad_at = (used_len + 1).max(ref_sorted.last().map_or(0, |l| l + 1)) + 16;
    let mut nbody = body.clone();
    for &(p, end) in own.values() {
        nbody[p..end].fill(0);
    }
    let (mut pos, mut spill, mut free): Slots<'_> = (BTreeMap::new(), vec![], vec![]);
    let mut order: Vec<_> = enc.iter().collect();
    order.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    for (k, b) in order {
        match own.get(k.as_str()) {
            Some(&(p, end)) if b.len() < end - p => {
                nbody[p..p + b.len()].copy_from_slice(b);
                pos.insert(k.clone(), p);
                if end as i64 - (p + b.len() + 1) as i64 > 1 {
                    free.push(((p + b.len() + 1) as i64, end as i64));
                }
            }
            o => {
                spill.push(k);
                if let Some(&(p, end)) = o {
                    free.push((p as i64, end as i64));
                }
            }
        }
    }
    free.push((pad_at as i64, body.len() as i64 - 1));
    for k in spill {
        let b = &enc[k];
        let Some(i) = free.iter().position(|&(s, e)| e - s > b.len() as i64) else {
            return fail(format!("no room in string_list.bin for {k}"));
        };
        let s = free[i].0 as usize;
        nbody[s..s + b.len()].copy_from_slice(b);
        pos.insert(k.clone(), s);
        free[i].0 = (s + b.len() + 1) as i64;
    }
    let mut new_bin = raw[..4].to_vec();
    new_bin.extend((nbody.len() as u32).to_le_bytes());
    new_bin.extend(&nbody);
    let mut new_hf: Vec<Vec<u8>> = hf_lines.iter().map(|l| l.to_vec()).collect();
    for (k, p) in &pos {
        let i = keys[k].0;
        let l = hf_lines[i];
        let a = l.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
        let a = a + l[a..]
            .iter()
            .position(|b| b.is_ascii_whitespace())
            .unwrap_or(0);
        let a = a + l[a..]
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(0);
        let z = a + l[a..]
            .iter()
            .position(|b| !b.is_ascii_digit())
            .unwrap_or(l.len() - a);
        new_hf[i] = [&l[..a], p.to_string().as_bytes(), &l[z..]].concat();
    }
    let mut wrong = 0;
    for line in new_hf.iter().skip(1) {
        let w = words(line);
        let Some((k, nums)) = w.split_first() else {
            continue;
        };
        let k = String::from_utf8_lossy(k).into_owned();
        let nums: Vec<usize> = nums
            .iter()
            .filter_map(|x| std::str::from_utf8(x).ok()?.parse().ok())
            .collect();
        let old = &keys[&k].1;
        if nums[1..] != old[1..] {
            wrong += 1;
        }
        for (s, n) in nums.iter().enumerate() {
            let want = if s == 0 && pos.contains_key(&k) {
                &enc[&k][..]
            } else {
                at(&body, old[s])
            };
            if at(&nbody, *n) != want {
                wrong += 1;
            }
        }
    }
    if wrong > 0 || new_bin.len() != raw.len() || new_hf[0] != hf_lines[0] {
        return fail(format!(
            "the built screen text does not read back as intended ({wrong} places) - nothing changed"
        ));
    }
    Ok((new_bin, new_hf.concat(), pos))
}

/// StringListTableRecord.bin: u32 count, then per line u32 id + 8 x (u32 length, cp932 bytes, no end byte): key, the
/// original comment, japanese, english, italian, spanish, french, german.
type Rec = (u32, Vec<Vec<u8>>);

fn read_enum(d: &[u8]) -> R<Vec<Rec>> {
    let u32at = |p: usize| {
        d.get(p..p + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| Failed("StringListTableRecord.bin is cut short".into()))
    };
    let (n, mut p, mut recs) = (u32at(0)?, 4, vec![]);
    for _ in 0..n {
        let id = u32at(p)?;
        p += 4;
        let mut f = vec![];
        for _ in 0..8 {
            let ln = u32at(p)? as usize;
            f.push(
                d.get(p + 4..p + 4 + ln)
                    .ok_or_else(|| Failed("StringListTableRecord.bin is cut short".into()))?
                    .to_vec(),
            );
            p += 4 + ln;
        }
        recs.push((id, f));
    }
    if p != d.len() {
        return fail(format!(
            "StringListTableRecord.bin has {} bytes after its last line",
            d.len() - p
        ));
    }
    Ok(recs)
}

fn write_enum(recs: &[Rec]) -> Vec<u8> {
    let mut out = (recs.len() as u32).to_le_bytes().to_vec();
    for (id, f) in recs {
        out.extend(id.to_le_bytes());
        for x in f {
            out.extend((x.len() as u32).to_le_bytes());
            out.extend(x);
        }
    }
    out
}

/// The Japanese column of each key in `enc` replaced, only where it is the server file's (the codes were checked
/// against that); read back. Returns the file and how many lines changed.
fn pack_enum(
    enum_raw: &[u8],
    body: &[u8],
    keys: &Keys,
    enc: &BTreeMap<String, Vec<u8>>,
) -> R<(Vec<u8>, usize)> {
    let old = read_enum(enum_raw)?;
    let mut recs = old.clone();
    let mut done = HashMap::new();
    for (_, f) in &mut recs {
        let k = String::from_utf8_lossy(&f[0]).into_owned();
        if let (Some(b), Some((_, nums))) = (enc.get(&k), keys.get(&k))
            && f[2] == at(body, nums[0])
        {
            f[2] = b.clone();
            done.insert(k, b.clone());
        }
    }
    let new = write_enum(&recs);
    let back = read_enum(&new)?;
    let ok = back.len() == old.len()
        && back.iter().zip(&old).all(|((i, b), (j, o))| {
            let k = String::from_utf8_lossy(&o[0]).into_owned();
            i == j && b[..2] == o[..2] && b[3..] == o[3..] && b[2] == *done.get(&k).unwrap_or(&o[2])
        });
    if !ok {
        return fail(
            "the built client screen text does not read back as intended - nothing changed",
        );
    }
    Ok((new, done.len()))
}

type Built = (Vec<(String, Vec<u8>)>, String);

impl Ctx {
    fn build_strings(&self, lang: Lang) -> R<Built> {
        let (raw, hf_raw) = (self.original_file(BIN)?, self.original_file(HF)?);
        let enum_raw = if self.ed == Edition::E1718 {
            Some(self.original_file(ENUM)?)
        } else {
            None
        };
        for (rel, d) in [
            (BIN, Some(&raw)),
            (HF, Some(&hf_raw)),
            (ENUM, enum_raw.as_ref()),
        ] {
            if let Some(d) = d
                && Some(sha(d)) != self.original_hash(rel)
            {
                return fail(match self.ed {
                    Edition::E1011 => "data\\string\\string_list.* are not original Rev D files - the English is made for those only".into(),
                    Edition::E1718 => format!("{rel} is not the original 2.00.00 file - the English is made for that only"),
                });
            }
        }
        let (body, keys) = read_strings(&raw, &hf_raw)?;
        let nl: &[u8] = if body.windows(2).any(|w| w == b"\r\n") {
            b"\r\n"
        } else {
            b"\n"
        };
        let ch = self.choices(&body, &keys, lang)?;
        let (enc, used, misfit, original_bad) = encode_lines(self.ed, &body, &keys, &ch, nl);
        if self.ed == Edition::E1011 && !misfit.is_empty() {
            return fail(format!(
                "the kit's text has {} problem(s), e.g. {}",
                misfit.len(),
                misfit
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let (new_bin, new_hf, pos) = pack_strings(&raw, &hf_raw, &enc)?;
        let mut by: BTreeMap<&str, usize> = BTreeMap::new();
        for k in pos.keys() {
            *by.entry(used[k]).or_default() += 1;
        }
        let mut note = format!(
            "{} lines in {} ({})",
            pos.len(),
            lang.name(),
            by.iter()
                .map(|(s, n)| format!("{s} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut files = vec![(BIN.to_string(), new_bin), (HF.to_string(), new_hf)];
        if let Some(e) = enum_raw {
            let (new_enum, done) = pack_enum(&e, &body, &keys, &enc)?;
            note += &format!("; {done} in the clients' table");
            files.push((ENUM.to_string(), new_enum));
            if !misfit.is_empty() {
                note += &format!(
                    "; {} of the kit's lines do not fit this game's line and gave way to the next choice ({})",
                    misfit.len(),
                    misfit[0]
                );
            }
        } else {
            if original_bad > 0 {
                note += &format!(
                    "; {original_bad} of the original lines unusable, the next choice taken"
                );
            }
            let mut left: Vec<_> = keys
                .iter()
                .filter(|(k, (_, nums))| {
                    !pos.contains_key(*k)
                        && !blocked(k)
                        && has_high(at(&body, nums[0]))
                        && !["SYS_DEBUG", "COM_debug"].contains(
                            &k.splitn(3, '_')
                                .take(2)
                                .collect::<Vec<_>>()
                                .join("_")
                                .as_str(),
                        )
                })
                .map(|(k, _)| k.as_str())
                .collect();
            left.sort_unstable();
            if !left.is_empty() {
                note += &format!(
                    "; {} still Japanese ({}{})",
                    left.len(),
                    left[..left.len().min(3)].join(", "),
                    if left.len() > 3 { ", ..." } else { "" }
                );
            }
        }
        Ok((files, note))
    }
}

// ---------------------------------------------------------------- player and team names
pub(crate) fn fold(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| match c {
            'Ø' => "O".to_string(),
            'ø' => "o".into(),
            'Æ' => "AE".into(),
            'æ' => "ae".into(),
            'ß' => "SS".into(),
            'Ł' => "L".into(),
            'ł' => "l".into(),
            'Đ' => "D".into(),
            'đ' => "d".into(),
            'Œ' => "OE".into(),
            'œ' => "oe".into(),
            'ı' => "i".into(),
            'Þ' => "TH".into(),
            'ð' => "d".into(),
            c => c.to_string(),
        })
        .collect();
    s.nfkd()
        .filter(|&c| canonical_combining_class(c) == 0)
        .map(|c| if (' '..'\u{7f}').contains(&c) { c } else { '?' })
        .collect()
}

/// value + end byte, then the field's own fill (whatever followed the original end byte).
fn put_field(field: &[u8], value: &[u8], size: usize) -> Vec<u8> {
    let fill = match field.iter().position(|&b| b == 0) {
        Some(e) if e + 1 < size => field[e + 1],
        _ => 0,
    };
    let mut out: Vec<u8> = value[..value.len().min(size - 1)].to_vec();
    out.push(0);
    out.resize(size, fill);
    out
}

/// A field's text up to its first end byte.
fn first(f: &[u8]) -> &[u8] {
    f.split(|&b| b == 0).next().unwrap_or(&[])
}

fn utf16(f: &[u8]) -> String {
    let u: Vec<u16> = f
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&u)
        .split('\0')
        .next()
        .unwrap_or("")
        .trim()
        .into()
}

impl Ctx {
    fn build_names(&self) -> R<Built> {
        let s = &self.spec;
        let mut pd = self.original_file(PLAYERS)?;
        if Some(sha(&pd)) != self.original_hash(PLAYERS) || pd.len() % s.rec != 0 {
            return fail(match self.ed {
                Edition::E1011 => {
                    "prog_data\\player_data\\player_data.bin is not the original Rev D file"
                }
                Edition::E1718 => {
                    "prog_data\\player_data\\player_data.bin is not the original 2.00.00 file"
                }
            });
        }
        let set = |pd: &mut Vec<u8>, at: usize, v: &[u8]| {
            let n = put_field(&pd[at..at + FIELD], v, FIELD);
            pd[at..at + FIELD].copy_from_slice(&n);
        };
        for i in 0..pd.len() / s.rec {
            let r = i * s.rec;
            let kana = cp932(first(&pd[r + s.short_kana..r + s.short_kana + FIELD]));
            let latin = utf16(&pd[r + s.short_latin..r + s.short_latin + FIELD]);
            if latin.is_empty() {
                continue;
            }
            let mut name = fold(&latin);
            let mut kc = kana.chars();
            if let (Some(a @ 'Ａ'..='Ｚ'), Some('．')) = (kc.next(), kc.next()) {
                let ini = format!("{}.", char::from_u32(a as u32 - 0xFEE0).unwrap());
                if !name.starts_with(&ini) {
                    name = ini + &name;
                }
            }
            set(&mut pd, r + s.short_kana, name.as_bytes());
            if let Some((full_latin, card_name)) = s.full {
                let full = fold(&utf16(&pd[r + full_latin..r + full_latin + FIELD]));
                if !full.is_empty() {
                    set(&mut pd, r + card_name, full.as_bytes());
                }
            }
            let sk = first(&pd[r + s.skill_en..r + s.skill_en + FIELD]).to_vec();
            if !sk.is_empty() && sk.iter().all(|c| (32..127).contains(c)) && !sk.contains(&b'%') {
                set(&mut pd, r + s.skill_jp, &sk);
            }
        }
        let n_players = pd.len() / s.rec;
        let mut files = vec![(PLAYERS.to_string(), pd)];
        let mut skipped = vec![];
        for (file, h, en) in &self.cpu {
            let rel = format!("{TEAMS}/{file}");
            let mut d = self.original_file(&rel)?;
            if sha(&d) != *h {
                skipped.push(file.clone());
                continue;
            }
            if en.chars().count() > NAME_END - NAME_AT - 1
                || en.contains('%')
                || !en.chars().all(|c| (' '..'\u{7f}').contains(&c))
            {
                return fail(format!("the kit's team name for {file} does not fit"));
            }
            let n = put_field(&d[NAME_AT..NAME_END], en.as_bytes(), NAME_END - NAME_AT);
            d[NAME_AT..NAME_END].copy_from_slice(&n);
            files.push((rel, d));
        }
        let mut note = format!("{n_players} players, {} CPU teams", files.len() - 1);
        if !skipped.is_empty() {
            note += &format!(
                "; {} team files are not original ones and stay as they are ({}...)",
                skipped.len(),
                skipped[0]
            );
        }
        Ok((files, note))
    }

    /// 2010-11: text inside the programs, both client copies; the server only for its own rows.
    fn build_exe(&self, lang: Lang) -> R<Built> {
        let rows = self.sheet("exe_text.tsv", "english")?;
        let mut files = vec![];
        for (rel, program, what) in [
            (
                PROJECTOR,
                "client",
                "the projector's client_Release.exe (the original Rev D)",
            ),
            (
                SEAT_EXE,
                "client",
                "seat 1's client_Release.exe (the copy setup makes)",
            ),
            (
                SERVER,
                "server",
                "the server's control_Release.exe (the original Rev D)",
            ),
        ] {
            let mine: Vec<_> = rows
                .iter()
                .filter(|r| cell(r, "program") == program)
                .collect();
            if mine.is_empty() {
                continue;
            }
            let mut d = self.original_file(rel)?;
            if Some(sha(&d)) != self.original_hash(rel) {
                return fail(format!(
                    "{what} is not the expected file - set up again first (Launcher.exe, Setup and updates)"
                ));
            }
            let size = d.len();
            for r in mine {
                let off = cell(r, "offset");
                let o = usize::from_str_radix(off.trim_start_matches("0x"), 16)
                    .map_err(|_| Failed(format!("exe_text.tsv: bad offset {off}")))?;
                let n: usize = cell(r, "length")
                    .parse()
                    .map_err(|_| Failed(format!("exe_text.tsv: bad length at {off}")))?;
                let en = if lang == Lang::It && !cell(r, "italiano").is_empty() {
                    cell(r, "italiano")
                } else {
                    cell(r, "english")
                };
                if d.get(o..o + n).map(sha).as_deref() != Some(cell(r, "original_sha256"))
                    || d.get(o + n) != Some(&0)
                {
                    return fail(format!("{what} at {off} is not the text the kit expects"));
                }
                let mut room = n + 1;
                while o + room < d.len() && d[o + room] == 0 {
                    room += 1;
                }
                if en.len() + 1 > room || !en.chars().all(|c| (' '..'\u{7f}').contains(&c)) {
                    return fail(format!("the kit's text at {off} does not fit"));
                }
                // over exactly the room measured: never a byte more or less
                d[o..o + room].fill(0);
                d[o..o + en.len()].copy_from_slice(en.as_bytes());
            }
            if d.len() != size {
                return fail(format!("{what} would change size - nothing written"));
            }
            files.push((rel.to_string(), d));
        }
        if let Some(p) = rows
            .iter()
            .map(|r| cell(r, "program"))
            .find(|p| !["client", "server"].contains(p))
        {
            return fail(format!(
                "exe_text.tsv names a program the kit does not know: {p}"
            ));
        }
        let n_client = rows
            .iter()
            .filter(|r| cell(r, "program") == "client")
            .count();
        let n_server = rows.len() - n_client;
        let more = if n_server > 0 {
            format!(", and {n_server} in the server")
        } else {
            String::new()
        };
        Ok((
            files,
            format!(
                "{n_client} texts inside the program, in the projector's and seat 1's copies{more}"
            ),
        ))
    }

    pub fn build_all(&self, lang: Lang) -> R<Built> {
        let (mut files, mut notes) = (vec![], vec![]);
        let mut add = |(f, n): Built| {
            files.extend(f);
            notes.push(n);
        };
        add(self.build_strings(lang)?);
        add(self.build_names()?);
        if self.ed == Edition::E1718 {
            add(self.build_titles(lang)?);
        }
        if self.ed == Edition::E1011 {
            add(self.build_exe(lang)?);
        }
        Ok((files, notes.join("\n")))
    }
}

// ---------------------------------------------------------------- swapping files in and out
/// A manifest key: the platform's separator, as the Python scripts write it.
fn key(rel: &str) -> String {
    rel.replace('/', MAIN_SEPARATOR_STR)
}

fn put(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut t = tempfile::NamedTempFile::new_in(dir)?;
    t.write_all(data)?;
    t.persist(path).map(drop).map_err(|e| e.error)
}

/// The original's fingerprint in a backup-list entry (beside "english"): older kits wrote it under another key.
fn original_of(h: &Value) -> Option<&str> {
    let m = h.as_object()?;
    m.get("original")
        .or_else(|| m.iter().find(|(k, _)| *k != "english").map(|(_, v)| v))?
        .as_str()
}

impl Ctx {
    fn manifest_path(&self) -> PathBuf {
        self.backup.join("manifest.json")
    }

    fn load_manifest(&self) -> Map<String, Value> {
        fs::read(self.manifest_path())
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default()
    }

    fn save_manifest(&self, m: &Map<String, Value>) -> R<()> {
        let mut out = vec![];
        let mut ser = serde_json::Serializer::with_formatter(
            &mut out,
            serde_json::ser::PrettyFormatter::with_indent(b" "),
        );
        serde::Serialize::serialize(m, &mut ser).map_err(|e| Failed(e.to_string()))?;
        Ok(put(&self.manifest_path(), &out)?)
    }

    /// The original file copied into the backup (a program stored as the original WINMM import) and read back; it
    /// stays in place until its replacement lands in one step.
    fn backup_copy(&self, rel: &str, dst: &Path) -> R<()> {
        let data = norm(fs::read(self.at(rel))?, rel)?;
        put(dst, &data)?;
        if fs::read(dst)? != data {
            return fail(format!("could not copy {rel} to the backup"));
        }
        Ok(())
    }

    /// Back each file up (the first time), then replace it in ONE step; the game's file is never missing.
    pub fn turn_on(&self, files: &[(String, Vec<u8>)], lang: Lang) -> R<()> {
        let mut man = self.load_manifest();
        // not listed, or its backup gone (an "off" stopped half way): a backup is needed
        let unsaved = |man: &Map<String, Value>, rel: &str| {
            !man.contains_key(&key(rel)) || !self.backup.join(rel).is_file()
        };
        for (rel, _) in files {
            if unsaved(&man, rel)
                && Some(sha(&norm(fs::read(self.at(rel))?, rel)?)) != self.original_hash(rel)
            {
                return fail(format!(
                    "{rel} is neither original nor backed up - refusing to replace it"
                ));
            }
        }
        for (rel, data) in files {
            let tgt = self.at(rel);
            if unsaved(&man, rel) {
                self.backup_copy(rel, &self.backup.join(rel))?;
                let mut e = Map::new();
                e.insert("original".into(), self.original_hash(rel).into());
                man.insert(key(rel), Value::Object(e));
                self.save_manifest(&man)?; // after each backup: an interruption never loses one
            }
            put(&tgt, data)?;
            if sha(&fs::read(&tgt)?) != sha(data) {
                return fail(format!(
                    "{rel} did not write correctly - switch the game text back to Japanese"
                ));
            }
            if let Some(Value::Object(e)) = man.get_mut(&key(rel)) {
                e.insert("english".into(), sha(data).into());
            }
            apply_hook(rel, &tgt)?;
        }
        self.save_manifest(&man)?;
        Ok(put(
            &self.backup.join("language.txt"),
            format!("{}\n", lang.code()).as_bytes(),
        )?)
    }

    /// The originals back, one step each. Returns how many files the manifest named.
    pub fn turn_off(&self) -> R<usize> {
        let man = self.load_manifest();
        let mut lost = vec![];
        for (k, h) in &man {
            let rel = k.replace('\\', "/");
            let (bak, tgt) = (self.backup.join(&rel), self.at(&rel));
            let want = original_of(h).unwrap_or("");
            if bak.is_file() {
                // a program's backup as the original WINMM import (an older kit's raw copy is the same bytes)
                let data = norm(fs::read(&bak)?, &rel)?;
                if sha(&data) != want {
                    return fail(format!(
                        "the backup of {rel} is not the original file - left as it is"
                    ));
                }
                put(&tgt, &data)?; // one step: the original file back, never a moment without one
                apply_hook(&rel, &tgt)?;
                fs::remove_file(&bak)?;
            } else if tgt.is_file() && sha(&norm(fs::read(&tgt)?, &rel)?) != want {
                lost.push(rel); // no backup: put back already, or lost
            }
        }
        if !lost.is_empty() {
            return fail(format!(
                "{} file(s) are not original and have no backup of the original, so they stay as they are ({}{})",
                lost.len(),
                lost[0],
                if lost.len() > 1 { ", ..." } else { "" }
            ));
        }
        for f in [self.manifest_path(), self.backup.join("language.txt")] {
            if f.is_file() {
                fs::remove_file(f)?;
            }
        }
        prune(&self.backup);
        Ok(man.len())
    }
}

/// Empty folders removed, bottom up, `dir` itself included.
fn prune(dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                prune(&e.path());
            }
        }
    }
    let _ = fs::remove_dir(dir);
}

/// `english.py on|off|check [it]`: the same words, the same exit codes (0 done, 1 refused or a check failed).
pub fn run(
    ed: Edition,
    game: &Path,
    backup: &Path,
    sheets: &Path,
    mode: Mode,
    lang: Lang,
    say: &dyn Fn(&str),
) -> i32 {
    let m = match mode {
        Mode::On => "on",
        Mode::Off => "off",
        Mode::Check => "check",
    };
    say(&format!(
        "Forza-WCCF {} - {} {m} - game: {}",
        ed.name(),
        lang.name(),
        game.display()
    ));
    let r = (|| -> R<()> {
        let c = Ctx::new(ed, game, backup, sheets)?;
        let state = backup.join(pictures::STATE);
        if mode == Mode::Off {
            if c.load_manifest().is_empty() && !state.is_file() {
                say("  Already the original Japanese.");
                return Ok(());
            }
            let p = pictures::restore(game, &state)?;
            let n = c.turn_off()?;
            say(&format!(
                "  ok    original Japanese restored ({n} files, {p} pictures)."
            ));
            return Ok(());
        }
        say("  Building from your game files...");
        let (files, notes) = c.build_all(lang)?;
        // 2010-11: the pictures stay off (with them the game did not read a club card); "on" only takes back any
        // a Python kit with WCCF_PICTURES=1 added
        let (pics, pnotes) = if ed == Edition::E1718 {
            let spec = std::env::var("WCCF_PICTURES").ok();
            pictures::build(game, sheets, lang == Lang::It, spec.as_deref(), &state)?
        } else {
            (vec![], vec![PICTURES_OFF.into()])
        };
        for n in notes.lines().chain(pnotes.iter().map(String::as_str)) {
            say(&format!("  ok    {n}"));
        }
        if mode == Mode::Check {
            let man = c.load_manifest();
            let fresh = files
                .iter()
                .filter(|(r, _)| !man.contains_key(&key(r)))
                .count();
            say(&format!(
                "  Check only, nothing changed. \"on\" would replace {} files ({fresh} backed up first) and redraw {} pictures.",
                files.len(),
                pics.len()
            ));
            return Ok(());
        }
        c.turn_on(&files, lang)?;
        pictures::apply(game, &pics, &state)?;
        say(&format!(
            "  ok    {} files and {} pictures replaced. Originals are in data\\english_backup.",
            files.len(),
            pics.len()
        ));
        say(&format!("{} is on.", lang.name()));
        Ok(())
    })();
    match r {
        Ok(()) => 0,
        Err(e) => {
            say(&format!("  FAIL  {e}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_field_keeps_the_fill() {
        assert_eq!(put_field(b"ab\0\xAA\xAA\xAA", b"xyz", 6), b"xyz\0\xAA\xAA");
        assert_eq!(put_field(b"abcdef", b"toolong", 6), b"toolo\0");
        assert_eq!(put_field(b"abcde\0", b"q", 6), b"q\0\0\0\0\0");
    }

    #[test]
    fn fold_drops_accents() {
        assert_eq!(fold("Kaká Øre ß"), "Kaka Ore SS");
        assert_eq!(fold("日"), "?");
    }

    #[test]
    fn codes_must_match() {
        let e = Edition::E1718;
        assert!(encoded(e, "%dポイント", "%d points", b"\n").is_ok());
        assert!(encoded(e, "%dポイント", "points", b"\n").is_err());
        assert!(encoded(e, "$c[FF0000FF]赤$c", "red", b"\n").is_err());
        assert!(
            encoded(Edition::E1011, "$pあ$t", "a$p", b"\n").is_err(),
            "a layout code dropped"
        );
        assert!(encoded(Edition::E1011, "$pあ$t", "a$p$t", b"\n").is_ok());
        assert!(encoded(e, "あ", "<empty>", b"\n").unwrap().is_empty());
        assert!(encoded(e, "%dあ", "<empty>", b"\n").is_err());
        assert!(encoded(e, "あ", "100% sure", b"\n").is_err());
        assert_eq!(
            encoded(Edition::E1011, "あ", "a\\nb", b"\r\n").unwrap(),
            b"a\r\nb"
        );
    }

    /// string_list.bin: tag, length, body; .hf: a header, then `key slot0 slot1`.
    fn strings() -> (Vec<u8>, Vec<u8>) {
        let mut body = b"\0".to_vec();
        for s in ["jp one", "second", "jp two"] {
            body.extend(s.as_bytes());
            body.push(0);
        }
        body.extend([0; 80]);
        let hf = b"header\r\nA_B_1 1 8\r\nA_B_2 15 8\r\n".to_vec();
        let mut bin = b"TAG!".to_vec();
        bin.extend((body.len() as u32).to_le_bytes());
        bin.extend(body);
        (bin, hf)
    }

    #[test]
    fn pack_strings_swaps_slot_zero() {
        let (bin, hf) = strings();
        let (b0, h0, pos) = pack_strings(&bin, &hf, &BTreeMap::new()).unwrap();
        assert_eq!((b0, h0, pos.len()), (bin.clone(), hf.clone(), 0));
        let enc = BTreeMap::from([
            ("A_B_1".to_string(), b"a much longer english line".to_vec()),
            ("A_B_2".to_string(), b"two".to_vec()),
        ]);
        let (nb, nh, pos) = pack_strings(&bin, &hf, &enc).unwrap();
        assert_eq!(nb.len(), bin.len());
        let (body, keys) = read_strings(&nb, &nh).unwrap();
        for (k, v) in &enc {
            assert_eq!(at(&body, keys[k].1[0]), &v[..], "{k}");
            assert_eq!(at(&body, keys[k].1[1]), b"second");
        }
        assert_eq!(pos["A_B_2"], 15, "a line that fits stays where it was");
    }

    #[test]
    fn enum_round_trip_and_swap() {
        let f = |k: &str, jp: &str| -> Rec {
            (
                7,
                [k, "c", jp, "e", "i", "s", "f", "g"]
                    .iter()
                    .map(|x| x.as_bytes().to_vec())
                    .collect(),
            )
        };
        let raw = write_enum(&[f("A_B_1", "jp one"), f("A_B_9", "other")]);
        assert_eq!(write_enum(&read_enum(&raw).unwrap()), raw);
        let (_, keys) = read_strings(&strings().0, &strings().1).unwrap();
        let body = strings().0[8..].to_vec();
        let (new, n) = pack_enum(
            &raw,
            &body,
            &keys,
            &BTreeMap::from([("A_B_1".to_string(), b"one".to_vec())]),
        )
        .unwrap();
        assert_eq!(n, 1);
        assert_eq!(read_enum(&new).unwrap()[0].1[2], b"one");
        assert_eq!(read_enum(&new).unwrap()[1].1[2], b"other");
    }

    /// A PE with one section holding an import table that names `dll`.
    fn pe(dll: &[u8]) -> Vec<u8> {
        let mut d = vec![0; 0x400];
        d[0x3C] = 0x40;
        d[0x40 + 6] = 1; // one section
        d[0x40 + 20] = 0xE0; // optional header size
        let opt = 0x40 + 24;
        d[opt + 104..opt + 108].copy_from_slice(&0x1000u32.to_le_bytes()); // import table rva
        let sec = opt + 0xE0;
        for (o, v) in [(8, 0x200u32), (12, 0x1000), (16, 0x200), (20, 0x200)] {
            d[sec + o..sec + o + 4].copy_from_slice(&v.to_le_bytes());
        }
        d[0x200 + 12..0x200 + 16].copy_from_slice(&0x1100u32.to_le_bytes()); // name rva -> file 0x300
        d[0x300..0x300 + dll.len()].copy_from_slice(dll);
        d
    }

    #[test]
    fn hook_import_renames_and_returns() {
        let orig = pe(SYS_DLL);
        let hooked = rename_import(&orig, SYS_DLL, HOOK_DLL).unwrap();
        assert_ne!(hooked, orig);
        assert!(import_name_offset(&hooked, HOOK_DLL).is_some());
        assert_eq!(norm(hooked, "client_Release.exe").unwrap(), orig);
        assert_eq!(norm(orig.clone(), "client_Release.exe").unwrap(), orig);
        assert!(rename_import(&orig, HOOK_DLL, SYS_DLL).is_err());
    }

    #[test]
    fn on_then_off_restores_and_matches_the_python_manifest() {
        let t = tempfile::tempdir().unwrap();
        let (game, backup) = (t.path().join("game"), t.path().join("backup"));
        let rel = "prog_data/cpu_team/cpu_1.dat";
        let orig = vec![7u8; 0x200];
        fs::create_dir_all(game.join("prog_data/cpu_team")).unwrap();
        fs::write(game.join(rel), &orig).unwrap();
        let mut c = Ctx::new_bare(Edition::E1718, &game, &backup);
        c.cpu = vec![("cpu_1.dat".into(), sha(&orig), "x".into())];
        let new = vec![(rel.to_string(), vec![9u8; 0x200])];
        c.turn_on(&new, Lang::It).unwrap();
        c.turn_on(&new, Lang::It).unwrap(); // again: builds from the backup, same result
        assert_eq!(fs::read(game.join(rel)).unwrap(), new[0].1);
        assert_eq!(
            fs::read_to_string(backup.join("language.txt")).unwrap(),
            "it\n"
        );
        let k = key(rel).replace('\\', "\\\\");
        assert_eq!(
            fs::read_to_string(backup.join("manifest.json")).unwrap(),
            format!(
                "{{\n \"{k}\": {{\n  \"original\": \"{}\",\n  \"english\": \"{}\"\n }}\n}}",
                sha(&orig),
                sha(&new[0].1)
            )
        );
        assert_eq!(c.turn_off().unwrap(), 1);
        assert_eq!(fs::read(game.join(rel)).unwrap(), orig);
        assert!(!backup.exists(), "the backup folder is gone");
    }

    #[test]
    fn a_foreign_file_is_not_replaced() {
        let t = tempfile::tempdir().unwrap();
        let game = t.path().join("game");
        fs::create_dir_all(game.join("prog_data/cpu_team")).unwrap();
        fs::write(
            game.join("prog_data/cpu_team/cpu_1.dat"),
            b"not the original",
        )
        .unwrap();
        let mut c = Ctx::new_bare(Edition::E1718, &game, &t.path().join("backup"));
        c.cpu = vec![("cpu_1.dat".into(), sha(b"the original"), "x".into())];
        assert!(
            c.turn_on(
                &[("prog_data/cpu_team/cpu_1.dat".into(), vec![1])],
                Lang::En
            )
            .is_err()
        );
        assert_eq!(
            fs::read(game.join("prog_data/cpu_team/cpu_1.dat")).unwrap(),
            b"not the original"
        );
    }

    #[test]
    fn original_of_reads_the_old_key_and_the_new() {
        let old = String::from_utf8(vec![0x73, 0x65, 0x67, 0x61]).unwrap(); // the name older kits wrote
        let mut cases = vec![
            serde_json::json!({ old.as_str(): "AB", "english": "CD" }),
            serde_json::json!({ "original": "AB", "english": "CD" }),
            serde_json::json!({ old.as_str(): "AB" }),
        ];
        cases.push(serde_json::json!({ "original": "AB" }));
        assert!(
            cases.iter().all(|e| original_of(e) == Some("AB")),
            "{cases:?}"
        );
        assert_eq!(original_of(&serde_json::json!({ "english": "CD" })), None);
    }

    /// Two files backed up and switched on by an older Python kit: the old key and Windows backslash keys. With
    /// `half` the second file's "off" had already run (backup gone, game file original).
    fn old_kit_state(half: bool) -> (tempfile::TempDir, Ctx, Vec<(String, Vec<u8>)>) {
        let t = tempfile::tempdir().unwrap();
        let (game, backup) = (t.path().join("game"), t.path().join("backup"));
        let old = String::from_utf8(vec![0x73, 0x65, 0x67, 0x61]).unwrap();
        let (mut man, mut files) = (Map::new(), vec![]);
        for (i, rel) in [
            "prog_data/cpu_team/cpu_1.dat",
            "data/string/string_list.bin",
        ]
        .into_iter()
        .enumerate()
        {
            let (orig, new) = (vec![i as u8 + 1; 64], vec![i as u8 + 9; 64]);
            let done = half && i == 1;
            fs::create_dir_all(game.join(rel).parent().unwrap()).unwrap();
            fs::write(game.join(rel), if done { &orig } else { &new }).unwrap();
            if !done {
                fs::create_dir_all(backup.join(rel).parent().unwrap()).unwrap();
                fs::write(backup.join(rel), &orig).unwrap();
            }
            let e = serde_json::json!({ old.as_str(): sha(&orig), "english": sha(&new) });
            man.insert(rel.replace('/', "\\"), e);
            files.push((rel.to_string(), orig));
        }
        fs::create_dir_all(&backup).unwrap();
        fs::write(
            backup.join("manifest.json"),
            serde_json::to_vec(&man).unwrap(),
        )
        .unwrap();
        fs::write(backup.join("language.txt"), "it\n").unwrap();
        (t, Ctx::new_bare(Edition::E1718, &game, &backup), files)
    }

    #[test]
    fn an_older_kits_manifest_turns_off() {
        for half in [false, true] {
            let (t, c, files) = old_kit_state(half);
            assert_eq!(c.turn_off().unwrap(), 2, "half={half}");
            for (rel, orig) in files {
                assert_eq!(fs::read(t.path().join("game").join(rel)).unwrap(), orig);
            }
            assert!(!t.path().join("backup").exists(), "half={half}");
        }
    }

    #[test]
    fn the_hook_import_rename_is_a_pure_name_swap() {
        let orig = pe(SYS_DLL);
        let hooked = rename_import(&orig, SYS_DLL, HOOK_DLL).unwrap();
        assert_eq!(hooked.len(), orig.len());
        assert!(orig.iter().zip(&hooked).filter(|(a, b)| a != b).count() <= SYS_DLL.len());
        assert!(
            !hooked
                .to_ascii_lowercase()
                .windows(5)
                .any(|w| w == b"winmm")
        );
        assert_eq!(rename_import(&hooked, HOOK_DLL, SYS_DLL).unwrap(), orig);
        assert!(
            rename_import(&hooked, SYS_DLL, HOOK_DLL).is_err(),
            "already renamed"
        );
        assert!(
            rename_import(&pe(b"other.dll"), SYS_DLL, HOOK_DLL).is_err(),
            "no such import"
        );
    }

    /// Sheets in a scratch folder, 2017-18 layout (no `languages/`).
    fn sheets_1718(files: &[(&str, &str)]) -> (tempfile::TempDir, Ctx) {
        let t = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let p = t.path().join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        let mut c = Ctx::new_bare(Edition::E1718, Path::new("g"), Path::new("b"));
        c.sheets = t.path().into();
        (t, c)
    }

    #[test]
    fn game_text_1718_puts_italian_first_and_english_never_shows_italian() {
        let (_t, c) = sheets_1718(&[
            (
                "english/screen_text.tsv",
                "key\tenglish\nK_BOTH\tkit\nK_EN\tkit\n",
            ),
            ("english/original_rstring.tsv", "key\tenglish\n"),
            (
                "italian/screen_text.tsv",
                "key\titaliano\nK_BOTH\tkit Italian\n",
            ),
        ]);
        let none = Keys::new();
        let en = c.choices(b"\0", &none, Lang::En).unwrap();
        let it = c.choices(b"\0", &none, Lang::It).unwrap();
        assert_eq!(
            it["K_BOTH"],
            [("kit Italian".into(), "kit Italian"), ("kit".into(), "kit")]
        );
        assert_eq!(
            it["K_BOTH"][1], en["K_BOTH"][0],
            "the English stays behind it"
        );
        assert_eq!(
            it["K_EN"], en["K_EN"],
            "a key with no Italian stays English"
        );
        assert!(
            en.values().flatten().all(|(_, s)| *s == "kit"),
            "English shows no Italian"
        );
    }

    #[test]
    fn game_text_1718_falls_back_when_a_line_loses_a_code_or_a_letter() {
        let (body, _, _) = encoding_rs::SHIFT_JIS.encode("あ$c[FF6633FF]%d$c\0");
        let keys = Keys::from([("CLM_X_0000".to_string(), (1, vec![0, 0]))]);
        let c = |first: &str| -> Choices {
            Choices::from([(
                "CLM_X_0000".to_string(),
                vec![
                    (first.to_string(), "kit Italian"),
                    ("$c[FF6633FF]%d$c points".to_string(), "kit"),
                ],
            )])
        };
        let e = Edition::E1718;
        let (enc, used, misfit, _) = encode_lines(e, &body, &keys, &c("%d punti"), b"\n");
        assert_eq!(
            enc["CLM_X_0000"], b"$c[FF6633FF]%d$c points",
            "lost the colour code"
        );
        assert_eq!((used["CLM_X_0000"], misfit.len()), ("kit", 1));
        let (enc, ..) = encode_lines(e, &body, &keys, &c("$c[FF6633FF]%d$c punti"), b"\n");
        assert_eq!(enc["CLM_X_0000"], b"$c[FF6633FF]%d$c punti");
        // a letter outside the game's font (cp932) also falls back
        let (enc, _, misfit, _) =
            encode_lines(e, &body, &keys, &c("$c[FF6633FF]%d$c \u{141}"), b"\n");
        assert_eq!(
            (enc["CLM_X_0000"].as_slice(), misfit.len()),
            (&b"$c[FF6633FF]%d$c points"[..], 1)
        );
    }

    #[test]
    fn the_italian_tables_2010_11_match_the_english() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editions/2010-11");
        let mut c = Ctx::new_bare(Edition::E1011, Path::new("g"), Path::new("b"));
        c.sheets = dir;
        let mut english = HashMap::new(); // the kit's own English wins, as in `choices`
        for name in ["original_rstring.tsv", "screen_text.tsv"] {
            for r in c.sheet(name, "english").unwrap() {
                if !cell(&r, "english").trim().is_empty() {
                    english.insert(cell(&r, "key").to_string(), cell(&r, "english").to_string());
                }
            }
        }
        let (mut bad, mut n) = (vec![], 0);
        for name in ["screen_text.tsv", "original_rstring.tsv"] {
            let rows = c.sheet(name, "italian").unwrap();
            let mut seen = HashSet::new();
            for r in &rows {
                let (k, it) = (cell(r, "key"), cell(r, "italiano"));
                n += 1;
                if !seen.insert(k) {
                    bad.push(format!("{name} {k}: twice"));
                }
                if encoding_rs::SHIFT_JIS.encode(it).2 {
                    bad.push(format!("{name} {k}: a letter the game's font cannot show"));
                }
                let Some(en) = english
                    .get(k)
                    .filter(|e| name == "screen_text.tsv" && e.trim() != "<empty>")
                else {
                    continue; // the original's lines: a misfit is left for the English
                };
                let fmt = |s: &str| {
                    FMT.find_iter(s)
                        .map(|m| m.as_str().to_string())
                        .collect::<Vec<_>>()
                };
                if fmt(it) != fmt(en) || it.matches("$c").count() != en.matches("$c").count() {
                    bad.push(format!(
                        "{name} {k}: codes {:?}, the English has {:?}",
                        fmt(it),
                        fmt(en)
                    ));
                }
            }
        }
        assert!(
            n > 0 && bad.is_empty(),
            "{} problem(s): {:?}",
            bad.len(),
            &bad[..bad.len().min(5)]
        );
    }

    #[test]
    fn the_italian_tables_2017_18_match_the_english() {
        let mut c = Ctx::new_bare(Edition::E1718, Path::new("g"), Path::new("b"));
        c.sheets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editions/2017-18");
        let english: HashMap<String, String> = c
            .sheet("screen_text.tsv", "english")
            .unwrap()
            .iter()
            .map(|r| (cell(r, "key").into(), cell(r, "english").into()))
            .collect();
        let codes = |s: &str| {
            let mut v: Vec<String> = CODES.find_iter(s).map(|m| m.as_str().into()).collect();
            v.sort();
            let f: Vec<String> = FMT.find_iter(s).map(|m| m.as_str().into()).collect();
            (f, v)
        };
        let (mut bad, mut n) = (vec![], 0);
        for r in c.sheet("screen_text.tsv", "italian").unwrap() {
            let (k, it) = (cell(&r, "key"), cell(&r, "italiano"));
            n += 1;
            match english.get(k).filter(|e| !e.trim().is_empty()) {
                None => bad.push(format!("{k}: no English row")),
                Some(en) if en.trim() != "<empty>" && codes(en) != codes(it) => bad.push(format!(
                    "{k}: codes {:?}, the English has {:?}",
                    codes(it),
                    codes(en)
                )),
                _ => {}
            }
        }
        assert!(
            n > 0 && bad.is_empty(),
            "{} problem(s): {:?}",
            bad.len(),
            &bad[..bad.len().min(5)]
        );
    }

    impl Ctx {
        fn new_bare(ed: Edition, game: &Path, backup: &Path) -> Self {
            Self {
                ed,
                spec: spec(ed),
                game: game.into(),
                backup: backup.into(),
                sheets: PathBuf::new(),
                cpu: vec![],
            }
        }
    }
}
