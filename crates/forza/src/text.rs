//! The game's text in English or Italian, built on the player's PC from their own game files and swapped in with the
//! originals kept (`on`, `off`, `check`). Ported from `legacy/2010-11/code/english.py` and
//! `legacy/2017-18/kit/scripts/english.py`: the backup manifest, the fingerprints, `put`/`put_field`, on/off/undo and
//! the sheet loading are shared; `Edition` selects the packers.
//!
//! 2010-11 (Rev D): `string_list.bin/.hf`, `player_data.bin` (short and card names), `cpu_team/cpu_N.dat`, and text
//! inside `client_Release.exe` (projector and seat 1's copy) and `control_Release.exe`. 2017-18 (2.00.00):
//! `string_list.bin/.hf` and the clients' `StringListTableRecord.bin`, `player_data.bin`, `cpu_team/cpu_N.dat` and
//! `guest_team/cpu_N.dat`. Team names: `cpu_names.tsv`, its `italiano` column in front of the English.
//!
//! The manifest (`<backup>/manifest.json`, `{rel: {original, english}}`, indent 1) and `language.txt` are the files
//! the Python scripts write and read, so a game switched by one can be switched back by the other. The pictures with
//! writing (`data/wccf_data.xaf`, [`pictures`]): 2017-18 redraws them on `on` as its script did; 2010-11 keeps them
//! off as its script did, and both editions take back pictures a Python kit added. `selftest` is not ported.

use crate::{Edition, Line};

mod bcn;
mod coach_title;
mod hook;
mod names;
mod pictures;
mod pil;
mod strings;
mod swap;

pub(crate) use names::fold;
pub(crate) use swap::text_is_original;
pub use swap::{Beside, sync_beside};
use {hook::*, strings::*, swap::*};

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

/// A refusal or a failed check; nothing changed. Its words are a [`Line`] like the notes': the player reads them in
/// their language (`forza_ui::line`). A `Raw` one is a path or a system or file-format error, the same everywhere.
#[derive(Debug)]
struct Failed(pub Line);

impl Failed {
    /// `%{file}: %{detail}`: a failure in a file, or the error its reader gave.
    fn in_file(file: impl fmt::Display, detail: impl fmt::Display) -> Self {
        Self(
            Line::key("text.fail_in_file")
                .arg("file", file)
                .arg("detail", detail),
        )
    }

    /// The same failure after the file it happened in.
    fn after(self, file: impl fmt::Display) -> Self {
        Self(
            Line::key("text.fail_in_file")
                .arg("file", file)
                .part("detail", self.0),
        )
    }
}

impl From<io::Error> for Failed {
    fn from(e: io::Error) -> Self {
        Self(Line::raw(e))
    }
}

type R<T> = Result<T, Failed>;

/// The files a build changes and the lines the player reads under them (each an `ui.yml` key of forza-ui).
type Built = (Vec<(String, Vec<u8>)>, Vec<Line>);

fn fail<T>(line: Line) -> R<T> {
    Err(Failed(line))
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

    /// The language's name in the player's words (`text.lang_en`, `text.lang_it`).
    pub fn line(self) -> Line {
        Line::key(match self {
            Self::En => "text.lang_en",
            Self::It => "text.lang_it",
        })
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

/// `spec(ed).originals[STRINGS]` is string_list.bin ([`BIN`]) in both editions.
const STRINGS: usize = 0;

struct Spec {
    /// player_data.bin: one record's size and the fields' offsets
    rec: usize,
    short_kana: usize,
    short_latin: usize,
    skill_en: usize,
    skill_jp: usize,
    /// The full Latin name and the full name it goes over (the gallery's detail name)
    full: Option<(usize, usize)>,
    /// 2017-18 only: the ASCII display name, used when a record has no short Latin name
    display: Option<usize>,
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
            display: None,
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
            full: Some((0x2B0, 0x2F0)),
            display: Some(0x3DF),
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
struct Ctx {
    ed: Edition,
    spec: Spec,
    game: PathBuf,
    backup: PathBuf,
    sheets: PathBuf,
    /// cpu_names.tsv: (file under the game folder, original sha, english, italiano or "")
    cpu: Vec<(String, String, String, String)>,
}

type Row = HashMap<String, String>;

/// In the backup: [`sheets_hash`] of the sheets the game's text was last built from.
const SHEETS: &str = "sheets.txt";

/// A SHA-256 over the kit's sheet folders (2017-18 `english/` and `italian/`, 2010-11 `languages/`): every file's
/// path and bytes, in name order.
fn sheets_hash(ed: Edition, sheets: &Path) -> String {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let (p, name) = (e.path(), e.file_name().to_string_lossy().into_owned());
            if p.is_dir() {
                walk(&p, &format!("{rel}{name}/"), out);
            } else {
                out.push((format!("{rel}{name}"), p));
            }
        }
    }
    let mut files = vec![];
    let roots: &[&str] = if ed == Edition::E1011 {
        &["languages"]
    } else {
        &["english", "italian"]
    };
    for r in roots {
        walk(&sheets.join(r), &format!("{r}/"), &mut files);
    }
    files.sort();
    let mut h = Sha256::new();
    for (rel, p) in files {
        h.update(rel.as_bytes());
        h.update([0]);
        h.update(fs::read(p).unwrap_or_default());
        h.update([0]);
    }
    h.finalize().iter().map(|b| format!("{b:02X}")).collect()
}

/// The game's text is in English or Italian, built from other sheets than the kit's: a kit copied over an older one
/// leaves the older text in the game files (2026-10-23: 0.3.0 over a pre-0.2.2 kit kept the Italian
/// "mese %d giorno %d", which overflows the secretary window's 10-byte box and stops seat 1). A text built before
/// `sheets.txt` existed counts as older.
pub fn stale(ed: Edition, backup: &Path, sheets: &Path) -> bool {
    backup.join("language.txt").is_file()
        && fs::read_to_string(backup.join(SHEETS))
            .map(|s| s.trim().to_owned())
            .ok()
            != Some(sheets_hash(ed, sheets))
}

fn cell<'a>(r: &'a Row, k: &str) -> &'a str {
    r.get(k).map_or("", String::as_str)
}

impl Ctx {
    fn new(ed: Edition, game: &Path, backup: &Path, sheets: &Path) -> R<Self> {
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
                    team_file(cell(r, "file")),
                    cell(r, "original_sha256").into(),
                    cell(r, "english").into(),
                    cell(r, "italiano").into(),
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
            .map_err(|e| Failed::in_file(p.display(), e))?;
        let head: Vec<String> = rd
            .headers()
            .map_err(|e| Failed::in_file(p.display(), e))?
            .iter()
            .map(String::from)
            .collect();
        let mut out = vec![];
        for rec in rd.records() {
            let rec = rec.map_err(|e| Failed::in_file(p.display(), e))?;
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
        self.cpu
            .iter()
            .find(|(n, ..)| n == rel)
            .map(|(_, h, ..)| h.clone())
    }
}

/// A team name the CPU team file's name field takes: printable ASCII, no printf `%`, room for the end byte.
fn team_name_fits(n: &str) -> bool {
    !n.is_empty()
        && n.len() < NAME_END - NAME_AT
        && !n.contains('%')
        && n.chars().all(|c| (' '..'\u{7f}').contains(&c))
}

/// cpu_names.tsv's file: `cpu_N.dat` in `prog_data/cpu_team`, or `../<folder>/cpu_N.dat` beside it (2017-18
/// `guest_team`).
fn team_file(f: &str) -> String {
    match f.strip_prefix("../") {
        Some(rest) => format!("prog_data/{rest}"),
        None => format!("{TEAMS}/{f}"),
    }
}

impl Ctx {
    /// 2010-11: text inside the programs, both client copies; the server only for its own rows.
    fn build_exe(&self, lang: Lang) -> R<Built> {
        let rows = self.sheet("exe_text.tsv", "english")?;
        let mut files = vec![];
        for (rel, program, what) in [
            (PROJECTOR, "client", "text.fail_what_projector"),
            (SEAT_EXE, "client", "text.fail_what_seat1"),
            (SERVER, "server", "text.fail_what_server"),
        ] {
            let what = Line::key(what);
            let mine: Vec<_> = rows
                .iter()
                .filter(|r| cell(r, "program") == program)
                .collect();
            if mine.is_empty() {
                continue;
            }
            let mut d = self.original_file(rel)?;
            if Some(sha(&d)) != self.original_hash(rel) {
                return fail(Line::key("text.fail_set_up_first").part("what", what.clone()));
            }
            let size = d.len();
            for r in mine {
                let off = cell(r, "offset");
                let o = usize::from_str_radix(off.trim_start_matches("0x"), 16)
                    .map_err(|_| Failed(Line::key("text.fail_sheet_offset").arg("cell", off)))?;
                let n: usize = cell(r, "length")
                    .parse()
                    .map_err(|_| Failed(Line::key("text.fail_sheet_length").arg("cell", off)))?;
                let en = if lang == Lang::It && !cell(r, "italiano").is_empty() {
                    cell(r, "italiano")
                } else {
                    cell(r, "english")
                };
                if d.get(o..o + n).map(sha).as_deref() != Some(cell(r, "original_sha256"))
                    || d.get(o + n) != Some(&0)
                {
                    return fail(
                        Line::key("text.fail_text_expected")
                            .part("what", what.clone())
                            .arg("at", off),
                    );
                }
                let mut room = n + 1;
                while o + room < d.len() && d[o + room] == 0 {
                    room += 1;
                }
                if en.len() + 1 > room || !en.chars().all(|c| (' '..'\u{7f}').contains(&c)) {
                    return fail(Line::key("text.fail_text_does_not_fit").arg("at", off));
                }
                // over exactly the room measured: never a byte more or less
                d[o..o + room].fill(0);
                d[o..o + en.len()].copy_from_slice(en.as_bytes());
            }
            if d.len() != size {
                return fail(Line::key("text.fail_would_change_size").part("what", what));
            }
            files.push((rel.to_string(), d));
        }
        if let Some(p) = rows
            .iter()
            .map(|r| cell(r, "program"))
            .find(|p| !["client", "server"].contains(p))
        {
            return fail(Line::key("text.fail_unknown_program").arg("program", p));
        }
        let n_client = rows
            .iter()
            .filter(|r| cell(r, "program") == "client")
            .count();
        let n_server = rows.len() - n_client;
        let server = if n_server > 0 {
            Line::key("text.note_exe_server").arg("count", n_server)
        } else {
            Line::raw("")
        };
        Ok((
            files,
            vec![
                Line::key("text.note_exe")
                    .arg("client", n_client)
                    .part("server", server),
            ],
        ))
    }

    fn build_all(&self, lang: Lang) -> R<Built> {
        let (mut files, mut notes) = (vec![], vec![]);
        let mut add = |(f, n): Built| {
            files.extend(f);
            notes.extend(n);
        };
        add(self.build_strings(lang)?);
        add(self.build_names(lang)?);
        if self.ed == Edition::E1718 {
            add(self.build_titles(lang)?);
        }
        if self.ed == Edition::E1011 {
            add(self.build_exe(lang)?);
        }
        Ok((files, notes))
    }
}

/// `english.py on|off|check [it]`: the same words, the same exit codes (0 done, 1 refused or a check failed).
pub fn run(
    ed: Edition,
    game: &Path,
    backup: &Path,
    sheets: &Path,
    mode: Mode,
    lang: Lang,
    say: crate::Say,
) -> i32 {
    let m = match mode {
        Mode::On => "text.title_on",
        Mode::Off => "text.title_off",
        Mode::Check => "text.title_check",
    };
    say(
        "",
        &Line::key(m)
            .arg("edition", ed.name())
            .part("lang", lang.line())
            .arg("game", game.display()),
    );
    let r = (|| -> R<()> {
        let c = Ctx::new(ed, game, backup, sheets)?;
        if mode != Mode::Check {
            c.set_aside_damaged_list()?;
        }
        let state = backup.join(pictures::STATE);
        if mode == Mode::Off {
            if c.load_manifest()?.is_empty() && !state.is_file() {
                say("  ", &Line::key("text.already_original"));
                return Ok(());
            }
            let p = pictures::restore(game, &state)?;
            // the backup beside the game stays: it may hold originals of files another folder changed
            let n = c.turn_off()?;
            say(
                "  ok    ",
                &Line::key("text.restored")
                    .arg("files", n)
                    .arg("pictures", p),
            );
            return Ok(());
        }
        say("  ", &Line::key("text.building"));
        let (files, notes) = c.build_all(lang)?;
        // 2010-11: the pictures stay off (with them the game did not read a club card); "on" only takes back any
        // a Python kit with WCCF_PICTURES=1 added
        let (pics, pnotes) = if ed == Edition::E1718 {
            let spec = std::env::var("WCCF_PICTURES").ok();
            pictures::build(game, sheets, lang == Lang::It, spec.as_deref(), &state)?
        } else {
            (vec![], vec![Line::key("text.pictures_off")])
        };
        for n in &notes {
            say("  ok    ", n);
        }
        for n in &pnotes {
            say("  ok    ", n);
        }
        if mode == Mode::Check {
            let man = c.load_manifest()?;
            let fresh = files
                .iter()
                .filter(|(r, _)| !man.contains_key(&key(r)))
                .count();
            say(
                "  ",
                &Line::key("text.check_only")
                    .arg("files", files.len())
                    .arg("fresh", fresh)
                    .arg("pictures", pics.len()),
            );
            return Ok(());
        }
        c.turn_on(&files, lang)?;
        pictures::apply(game, &pics, &state)?;
        if ed == Edition::E1718
            && let Err(e) = c.save_beside()
        {
            say(
                "  ",
                &Line::key("text.fail_save_beside").part("detail", e.0),
            );
        }
        say(
            "  ok    ",
            &Line::key("text.replaced")
                .arg("files", files.len())
                .arg("pictures", pics.len()),
        );
        say("", &Line::key("text.on").part("lang", lang.line()));
        Ok(())
    })();
    match r {
        Ok(()) => 0,
        Err(e) => {
            say("  FAIL  ", &e.0);
            1
        }
    }
}

#[cfg(test)]
mod tests;
