//! The catalogue made from the player's own copy of the game: `catalogue.tsv`, one line per card, and
//! `cards/<number>.png`, each card's 128x128 picture (the board shows its left 88 columns). Only reads the game.
//! Ported from `legacy/2010-11/code/cards/catalogue.py` (2010-11) and `legacy/2017-18/kit/scripts/cards1718.py`
//! (2017-18, built on the first). The archive reader, string helpers, picture decoder and TSV writer are shared;
//! `Edition` selects the record layout, the tables and the checks.
//!
//! 2010-11 (Rev D) reads `client_Release.exe` (card number -> record table at VA 0x00AF8610),
//! `prog_data/player_data/player_data.bin` (3,927 records of 0x328 bytes) and `data/wccf_data.xaf` (table of
//! contents, then `data/player_card/*.dds`). 2017-18 reads the same files with 8,434 records of 0x620 bytes (table at
//! VA 0x015B5B48), the club and nationality names in the exe (VA 0x0154C428) and `prog_data/player_data/VerName.txt`
//! (edition labels); its rows run newest season first.

use super::codes::{CLUB, CLUBS_1718, EDITION, NATIONALITY, NATIONS_1718, name_for};
use super::pictures::lzw;
use crate::Edition;
use image::{
    ExtendedColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const REC: usize = 0x328;
/// Rev D's `player_data.bin`.
pub const RECORDS: usize = 3927;
const CARD_TABLE_VA: u32 = 0x00AF_8610;
const CARD_TABLE_N: usize = 0x26E0;
const XAF_ENTRY: usize = 0xB0;
const POSITIONS: [&str; 4] = ["GK", "DF", "MF", "FW"];

/// The catalogue's columns, in file order.
pub const COLS: [&str; 49] = [
    "card_no",
    "record_index",
    "set_no",
    "edition",
    "edition_label",
    "season",
    "rarity",
    "rarity_name",
    "rarity_frame",
    "is_placeholder",
    "name_full_latin",
    "name_short_latin",
    "name_display_ascii",
    "name_full_kana",
    "name_short_kana",
    "position_name",
    "club_id",
    "club_name",
    "nationality_id",
    "nationality_name",
    "shirt_numbers",
    "birth_date",
    "height_cm",
    "weight_kg",
    "stat_offence",
    "stat_defence",
    "stat_technique",
    "stat_power",
    "stat_speed",
    "stat_stamina",
    "stats_total",
    "skill_en",
    "skill_jp",
    "model_key",
    "image_archive_path",
    "image_xaf_offset",
    "image_xaf_stored_size",
    "image_size",
    "image_packed",
    "alt_png_2005_06_build",
    "hidden_params",
    "u16_04",
    "u16_06",
    "u16_08",
    "bytes_125_12e",
    "byte_136",
    "byte_31a",
    "byte_31b",
    "bytes_31c_326",
];

fn bad(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

/// The three files under `game` (or `game/extracted`), or the first missing one.
pub struct GameFiles {
    pub exe: PathBuf,
    pub players: PathBuf,
    pub xaf: PathBuf,
    /// 2017-18 only: the edition labels.
    pub vername: PathBuf,
}

/// 2010-11 also takes the sbwg download's `extracted` folder; 2017-18 wants the folder with the exe in it.
pub fn game_files(edition: Edition, game: &Path, say: &dyn Fn(&str)) -> Option<GameFiles> {
    let at = |root: &Path| GameFiles {
        exe: root.join("client_Release.exe"),
        players: root.join("prog_data/player_data/player_data.bin"),
        xaf: root.join("data/wccf_data.xaf"),
        vername: root.join("prog_data/player_data/VerName.txt"),
    };
    let e1718 = edition == Edition::E1718;
    let need = |f: &GameFiles| {
        let mut v = vec![f.exe.clone(), f.players.clone(), f.xaf.clone()];
        if e1718 {
            v.push(f.vername.clone());
        }
        v
    };
    let roots = if e1718 {
        vec![game.to_path_buf()]
    } else {
        vec![game.to_path_buf(), game.join("extracted")]
    };
    for root in roots {
        let f = at(&root);
        if need(&f).iter().all(|p| p.is_file()) {
            return Some(f);
        }
    }
    let f = at(game);
    for p in &need(&f) {
        if !p.is_file() {
            say(&format!("missing: {}", p.display()));
        }
    }
    None
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn va_to_offset(d: &[u8], va: u32) -> io::Result<usize> {
    let short = || bad("not a Windows program");
    if d.len() < 0x40 {
        return Err(short());
    }
    let lfa = u32_at(d, 0x3C) as usize;
    if d.len() < lfa + 24 + 32 {
        return Err(short());
    }
    let nsec = u16_at(d, lfa + 6) as usize;
    let optsz = u16_at(d, lfa + 20) as usize;
    let base = u32_at(d, lfa + 24 + 28);
    for i in 0..nsec {
        let o = lfa + 24 + optsz + 40 * i;
        if d.len() < o + 24 {
            return Err(short());
        }
        let (sva, rsz, roff) = (u32_at(d, o + 12), u32_at(d, o + 16), u32_at(d, o + 20));
        let lo = base.wrapping_add(sva);
        if lo <= va && va < lo.wrapping_add(rsz) {
            return Ok((roff + va - lo) as usize);
        }
    }
    Err(bad(format!(
        "VA 0x{va:08x} is not in the file's initialised data"
    )))
}

/// (card number, record index) for every card number the exe's static table maps.
fn card_table(d: &[u8], va: u32, n: usize) -> io::Result<BTreeMap<usize, usize>> {
    let off = va_to_offset(d, va)?;
    if d.len() < off + 4 * n {
        return Err(bad("card table runs past the file"));
    }
    Ok((0..n)
        .filter_map(|c| {
            usize::try_from(u32_at(d, off + 4 * c) as i32)
                .ok()
                .map(|i| (c, i))
        })
        .collect())
}

fn utf16(r: &[u8], o: usize, n: usize) -> String {
    let u: Vec<u16> = (r[o..o + n].as_chunks::<2>().0.iter())
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let s = String::from_utf16_lossy(&u);
    s.split('\0').next().unwrap_or("").to_string()
}
fn field(r: &[u8], o: usize, n: usize) -> &[u8] {
    let s = &r[o..o + n];
    &s[..s.iter().position(|&c| c == 0).unwrap_or(n)]
}
fn cp932(r: &[u8], o: usize, n: usize) -> String {
    encoding_rs::SHIFT_JIS
        .decode_without_bom_handling(field(r, o, n))
        .0
        .into_owned()
}
fn ascii(r: &[u8], o: usize, n: usize) -> String {
    field(r, o, n).iter().map(|&b| b as char).collect() // latin-1: any stray byte survives
}
fn list(b: &[u8]) -> String {
    b.iter().map(u8::to_string).collect::<Vec<_>>().join(" ")
}

/// "1987/6/24" -> "1987-06-24"; anything else -> "".
fn birth_iso(s: &str) -> String {
    let p: Vec<&str> = s.split('/').collect();
    if p.len() == 3
        && p.iter()
            .all(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()))
    {
        let n = |x: &str| x.parse::<u64>().unwrap_or(0);
        return format!("{:04}-{:02}-{:02}", n(p[0]), n(p[1]), n(p[2]));
    }
    String::new()
}

struct Entry {
    name: String,
    path: String,
    is_file: bool,
    size: u32,
    stored: u32,
    offset: u64,
}

fn xaf_toc(path: &Path) -> io::Result<Vec<Entry>> {
    let mut f = File::open(path)?;
    let mut hdr = [0u8; 0x100];
    f.read_exact(&mut hdr)?;
    if &hdr[..4] != b"xaf0" {
        return Err(bad(format!("{} is not an xaf0 archive", path.display())));
    }
    let flen = f.metadata()?.len();
    let (sector, n) = (u64::from(u32_at(&hdr, 8)), u32_at(&hdr, 12) as usize);
    if n.checked_mul(XAF_ENTRY).is_none_or(|b| b as u64 > flen) {
        return Err(bad("xaf0 table larger than the file"));
    }
    let mut raw = vec![0u8; n * XAF_ENTRY];
    f.read_exact(&mut raw)?;
    let parent = |e: &[u8]| usize::try_from(u32_at(e, 0x84) as i32).ok();
    let mut ents: Vec<Entry> = raw
        .chunks(XAF_ENTRY)
        .map(|e| Entry {
            name: field(e, 0, 0x80).iter().map(|&b| b as char).collect(),
            path: String::new(),
            is_file: e[0x80] != 0,
            size: u32_at(e, 0x94),
            stored: u32_at(e, 0x98),
            offset: u64::from(u32_at(e, 0xA0)) * sector,
        })
        .collect();
    for i in 0..n {
        let mut parts = vec![ents[i].name.clone()];
        let mut p = parent(&raw[i * XAF_ENTRY..]);
        while let Some(j) = p {
            if j >= n || parts.len() > n {
                return Err(bad("xaf0 table: bad parent"));
            }
            parts.push(ents[j].name.clone());
            p = parent(&raw[j * XAF_ENTRY..]);
        }
        parts.reverse();
        ents[i].path = parts.join("/");
    }
    Ok(ents)
}

/// The exe, read whole; 2017-18 also reads its name table.
fn read_exe(exe: &Path) -> io::Result<Vec<u8>> {
    let d = fs::read(exe)?;
    if !d.starts_with(b"MZ") {
        return Err(bad(format!("{} is not a Windows program", exe.display())));
    }
    Ok(d)
}

/// One catalogue line: the file's cells in column order, and the values the 2010-11 checks look at.
#[derive(Default)]
pub struct Row {
    cells: Vec<String>,
    card_no: usize,
    placeholder: bool,
    position: u8,
    rarity: u8,
    stats: Vec<u8>,
    stats_total: u8,
    birth_date: String,
    edition_label: &'static str,
    image: Option<(u64, u32, u32)>,
    image_path: String,
}

/// Rows, the archive's picture entries by path, and every archive entry.
type Built = (Vec<Row>, HashMap<String, usize>, Vec<Entry>);
type Checks = Vec<(String, bool, String)>;

fn rarity(r: u8) -> (&'static str, &'static str) {
    match r {
        1 => ("REGULAR", "Normal"),
        2 => ("SPECIAL", "Black"),
        3 => ("RARE", "Silver"),
        4 => ("LEGEND", "Gold"),
        5 => ("ALL TIME LEGEND", "Gold"),
        _ => ("", ""),
    }
}

fn build_rows_1011(files: &GameFiles, checks: &mut Checks) -> io::Result<Built> {
    let mut check =
        |name: &str, ok: bool, detail: String| checks.push((name.to_string(), ok, detail));
    let table = card_table(&read_exe(&files.exe)?, CARD_TABLE_VA, CARD_TABLE_N)?;
    let data = fs::read(&files.players)?;
    if data.len() % REC != 0 {
        return Err(bad(format!(
            "player_data.bin size {} is not a multiple of 0x{REC:x}",
            data.len()
        )));
    }
    let recs: Vec<&[u8]> = data.chunks(REC).collect();
    check(
        &format!("record count {RECORDS}"),
        recs.len() == RECORDS,
        format!("{} records", recs.len()),
    );
    let mut vals: Vec<usize> = table.values().copied().collect();
    vals.sort_unstable();
    check(
        "the exe's table maps one card number to each record",
        vals.iter().copied().eq(0..recs.len()),
        format!("{} card numbers", table.len()),
    );
    let ents = xaf_toc(&files.xaf)?;
    let mut images = HashMap::new();
    let mut faces = HashSet::new();
    for (i, e) in ents.iter().enumerate().filter(|(_, e)| e.is_file) {
        if e.path.starts_with("data/player_card/") {
            images.insert(e.name.to_lowercase(), i);
        }
        if e.path.starts_with("data/models/face/") {
            faces.insert(e.name.to_lowercase());
        }
    }
    let (mut rows, mut clubs, mut nats) = (Vec::new(), HashSet::new(), HashSet::new());
    let (mut mism, mut exact_case, mut face_ok, mut beyond) = (0, 0, 0, 0);
    for (&card_no, &idx) in &table {
        let Some(r) = recs.get(idx) else {
            beyond += 1;
            continue;
        };
        let (set_no, u04, u06, u08) = (u16_at(r, 2), u16_at(r, 4), u16_at(r, 6), u16_at(r, 8));
        mism += usize::from(usize::from(u16_at(r, 0)) != card_no);
        let (edition, rar) = (r[0x0A], r[0x0B]);
        let (club_id, nat_id, position) = (r[0x10C], r[0x124], r[0x111]);
        let (rname, rframe) = rarity(rar);
        let (elabel, season) = EDITION
            .iter()
            .find(|e| e.0 == edition)
            .map_or(("", ""), |e| (e.1, e.2));
        let placeholder = edition == 13; // carry codes 0 / 5 by default, not a real club
        let (club_name, nat_name) = if placeholder {
            ("", "")
        } else {
            (name_for(CLUB, club_id), name_for(NATIONALITY, nat_id))
        };
        let image_name = ascii(r, 0x2DA, 0x40);
        let model_key = ascii(r, 0x29A, 0x40);
        let img = images
            .get(&format!("{image_name}.dds").to_lowercase())
            .map(|&i| &ents[i]);
        exact_case += usize::from(img.is_some_and(|e| e.name == format!("{image_name}.dds")));
        face_ok += usize::from(faces.contains(&format!("{model_key}.svo").to_lowercase()));
        let stats = r[0x12F..0x135].to_vec();
        let birth = birth_iso(&ascii(r, 0x112, 0x10));
        let b = |v: bool| if v { "1" } else { "0" }.to_string();
        let opt = |v: Option<String>| v.unwrap_or_default();
        let mut cells = vec![
            card_no.to_string(),
            idx.to_string(),
            set_no.to_string(),
            edition.to_string(),
            elabel.to_string(),
            season.to_string(),
            rar.to_string(),
            rname.to_string(),
            rframe.to_string(),
            b(placeholder),
            utf16(r, 0x0C, 0x40),
            utf16(r, 0xCC, 0x40),
            ascii(r, 0x137, 0x40),
            cp932(r, 0x4C, 0x40),
            cp932(r, 0x8C, 0x40),
            POSITIONS
                .get(position as usize)
                .copied()
                .unwrap_or("")
                .to_string(),
            club_id.to_string(),
            club_name.to_string(),
            nat_id.to_string(),
            nat_name.to_string(),
            list(&r[0x10D..0x111]),
            birth.clone(),
            r[0x122].to_string(),
            r[0x123].to_string(),
        ];
        cells.extend(stats.iter().map(u8::to_string));
        cells.extend([
            r[0x135].to_string(),
            ascii(r, 0x1B7, 0x40),
            cp932(r, 0x1F7, 0x40),
            model_key,
            format!("data/player_card/{image_name}.dds"),
            opt(img.map(|e| e.offset.to_string())),
            opt(img.map(|e| e.stored.to_string())),
            opt(img.map(|e| e.size.to_string())),
            opt(img.map(|e| b(e.stored != e.size))),
            String::new(), // alt_png_2005_06_build, kept empty
            list(&r[0x237..0x29A]),
            u04.to_string(),
            u06.to_string(),
            u08.to_string(),
            list(&r[0x125..0x12F]),
            r[0x136].to_string(),
            r[0x31A].to_string(),
            r[0x31B].to_string(),
            r[0x31C..0x327].iter().map(|b| format!("{b:02x}")).collect(),
        ]);
        if !placeholder {
            clubs.insert(club_id);
            nats.insert(nat_id);
        }
        rows.push(Row {
            cells,
            card_no,
            placeholder,
            position,
            rarity: rar,
            stats,
            stats_total: r[0x135],
            birth_date: birth,
            edition_label: elabel,
            image: img.map(|e| (e.offset, e.stored, e.size)),
            image_path: format!("data/player_card/{image_name}.dds"),
        });
    }
    let n = rows.len();
    check(
        "every card number in the exe's table has its record",
        beyond == 0,
        format!("{beyond} point past the file's end"),
    );
    check(
        "the number at the start of each record is its card number",
        mism == 0,
        format!("{mism} differ"),
    );
    check(
        "every card's picture is in the archive",
        rows.iter().all(|r| r.image.is_some()),
        String::new(),
    );
    check(
        "picture names match the archive's case",
        exact_case == n,
        format!("{exact_case}/{n}"),
    );
    // card 443 stores 77 while its six sum to 78 - the same in the 2005-06 game, so it is the original data
    let off_sum: Vec<usize> = rows
        .iter()
        .filter(|r| r.stats.iter().map(|&s| u32::from(s)).sum::<u32>() != u32::from(r.stats_total))
        .map(|r| r.card_no)
        .collect();
    check(
        "total = the six stats added up (except the original card 443)",
        off_sum.is_empty() || off_sum == [443],
        format!("{off_sum:?}"),
    );
    check(
        "every card's face model is in the archive",
        face_ok == n,
        format!("{face_ok}/{n}"),
    );
    check(
        "every position is GK, DF, MF or FW",
        rows.iter().all(|r| r.position < 4),
        String::new(),
    );
    check(
        "every rarity is 1..5",
        rows.iter().all(|r| (1..=5).contains(&r.rarity)),
        String::new(),
    );
    check(
        "every edition has a label",
        rows.iter().all(|r| !r.edition_label.is_empty()),
        String::new(),
    );
    check(
        "every stat is 1..20",
        rows.iter()
            .all(|r| r.stats.iter().all(|s| (1..=20).contains(s))),
        String::new(),
    );
    let nobirth = rows
        .iter()
        .filter(|r| !r.placeholder && r.birth_date.is_empty())
        .count();
    check(
        "every real card's birth date reads",
        nobirth == 0,
        format!("{nobirth} do not"),
    );
    check(
        "card numbers are unique",
        rows.iter().map(|r| r.card_no).collect::<HashSet<_>>().len() == n,
        String::new(),
    );
    check(
        "every club code has a name entry",
        clubs.iter().all(|c| CLUB.iter().any(|t| t.0 == *c)),
        String::new(),
    );
    check(
        "every nationality code has a name entry",
        nats.iter().all(|c| NATIONALITY.iter().any(|t| t.0 == *c)),
        String::new(),
    );
    let toc = ents
        .iter()
        .enumerate()
        .filter(|(_, e)| e.is_file && e.path.starts_with("data/player_card/"))
        .map(|(i, e)| (e.path.clone(), i))
        .collect();
    Ok((rows, toc, ents))
}

const REC_1718: usize = 0x620;
const RECORDS_1718: usize = 8434;
const CARD_TABLE_VA_1718: u32 = 0x015B_5B48;
const CARD_TABLE_N_1718: usize = 0x2CC1;
const NAMES_VA_1718: u32 = 0x0154_C428;
/// VerName.txt line 13 (0-based): "not players" (referees, coaches, GUESTPLAY stand-ins, the navigator).
const PLACEHOLDER_EDITION: u8 = 13;
/// The publisher's own joke cards: 3509's picture is a hand-scrawled "dummy", 9951 a staff Maradona parody.
const NOT_PLAYERS: [usize; 2] = [3509, 9951];

/// The 2017-18 catalogue's columns: 0-30 as 2010-11 writes them (the overlay reads them by position), then this
/// edition's own raw fields.
pub const COLS_1718: &[&str] = &[
    "card_no",
    "record_index",
    "set_no",
    "edition",
    "edition_label",
    "season",
    "rarity",
    "rarity_name",
    "rarity_frame",
    "is_placeholder",
    "name_full_latin",
    "name_short_latin",
    "name_display_ascii",
    "name_full_kana",
    "name_short_kana",
    "position_name",
    "club_id",
    "club_name",
    "nationality_id",
    "nationality_name",
    "shirt_numbers",
    "birth_date",
    "height_cm",
    "weight_kg",
    "stat_offence",
    "stat_defence",
    "stat_technique",
    "stat_power",
    "stat_speed",
    "stat_stamina",
    "stats_total",
    "skill_en",
    "skill_jp",
    "model_key",
    "image_archive_path",
    "image_xaf_offset",
    "image_xaf_stored_size",
    "image_size",
    "image_packed",
    "card_code",
    "club_name_jp",
    "nationality_name_jp",
    "u16_28e_298",
    "bytes_3cd_3d6",
    "byte_3de",
    "hidden_4df_523",
    "tail_5a4_61f",
];

/// Cards whose back was read by eye: (number, short name, line, club, nationality, six stats, birth date, height,
/// weight). 1007 and 4101 were read on 2026-10-07, the rest on 2026-10-08.
type Known = (
    usize,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    [u8; 6],
    &'static str,
    u8,
    u8,
);
const KNOWN: &[Known] = &[
    (
        7049,
        "MESSI",
        "FW",
        "FC Barcelona",
        "Argentina",
        [20, 8, 20, 16, 19, 15],
        "1987-06-24",
        170,
        72,
    ),
    (
        7064,
        "RONALDO",
        "FW",
        "Real Madrid",
        "Portugal",
        [20, 5, 19, 19, 19, 15],
        "1985-02-05",
        185,
        80,
    ),
    (
        6840,
        "M.SALAH",
        "FW",
        "Liverpool",
        "Egypt",
        [16, 8, 17, 14, 19, 15],
        "1992-06-15",
        175,
        71,
    ),
    (
        6904,
        "LEWANDOWSKI",
        "FW",
        "Bayern Munchen",
        "Poland",
        [19, 6, 17, 18, 16, 16],
        "1988-08-21",
        185,
        79,
    ),
    (
        7061,
        "MODRIĆ",
        "MF",
        "Real Madrid",
        "Croatia",
        [16, 12, 19, 13, 17, 17],
        "1985-09-09",
        174,
        65,
    ),
    (
        6883,
        "KAGAWA",
        "MF",
        "Borussia Dortmund",
        "Japan",
        [16, 10, 17, 13, 16, 16],
        "1989-03-17",
        175,
        68,
    ),
    (
        7054,
        "SERGIO RAMOS",
        "DF",
        "Real Madrid",
        "Spain",
        [12, 19, 14, 18, 15, 17],
        "1986-03-30",
        183,
        75,
    ),
    (
        7127,
        "NEUER",
        "GK",
        "Bayern Munchen",
        "Germany",
        [10, 20, 14, 18, 14, 13],
        "1986-03-27",
        193,
        92,
    ),
    (
        6954,
        "BUFFON",
        "GK",
        "Juventus",
        "Italy",
        [8, 20, 13, 18, 13, 13],
        "1978-01-28",
        191,
        91,
    ),
    (
        5980,
        "BUFFON",
        "GK",
        "Juventus",
        "Italy",
        [8, 20, 13, 18, 13, 13],
        "1978-01-28",
        192,
        94,
    ),
    (
        6826,
        "MIGNOLET",
        "GK",
        "Liverpool",
        "Belgium",
        [7, 18, 12, 18, 12, 11],
        "1988-03-06",
        193,
        87,
    ),
    (
        1007,
        "BA",
        "FW",
        "Chelsea",
        "Senegal",
        [19, 5, 15, 18, 17, 14],
        "1985-05-25",
        189,
        85,
    ),
    (
        4101,
        "SZCZESNY",
        "GK",
        "Arsenal",
        "Poland",
        [7, 18, 12, 18, 12, 11],
        "1990-04-18",
        195,
        93,
    ),
];

/// A 2017-18 card: what its row and the checks use.
struct Card1718 {
    cells: Vec<String>,
    card_no: usize,
    placeholder: bool,
    season: String,
    position: u8,
    rarity: u8,
    rarity_name: &'static str,
    stats: [u8; 6],
    stats_total: u8,
    birth: String,
    short: String,
    pos_name: &'static str,
    club: String,
    nat: String,
    height: u8,
    weight: u8,
    image: Option<(u64, u32, u32)>,
    image_path: String,
}

/// The 286 Japanese club (0-189) and nationality (0-95) names the exe points to.
fn exe_names(d: &[u8]) -> io::Result<Vec<String>> {
    let n = CLUBS_1718.len() + NATIONS_1718.len();
    let off = va_to_offset(d, NAMES_VA_1718)?;
    if d.len() < off + 4 * n {
        return Err(bad("name table runs past the file"));
    }
    (0..n)
        .map(|i| {
            let o = va_to_offset(d, u32_at(d, off + 4 * i))?;
            let end = d[o..]
                .iter()
                .position(|&c| c == 0)
                .ok_or_else(|| bad("name without an end"))?;
            Ok(encoding_rs::SHIFT_JIS
                .decode_without_bom_handling(&d[o..o + end])
                .0
                .into_owned())
        })
        .collect()
}

/// VerName.txt -> edition number -> label; each line is "<name> t|f" (the original flag, unused here).
fn editions_1718(path: &Path) -> io::Result<HashMap<u8, String>> {
    let raw = fs::read(path)?;
    let (text, _, err) = encoding_rs::SHIFT_JIS.decode(&raw);
    if err {
        return Err(bad("VerName.txt is not Shift-JIS"));
    }
    Ok(text
        .lines()
        .enumerate()
        .map(|(i, l)| {
            let t = l.trim();
            let t = match t.strip_suffix(['t', 'f']) {
                Some(h) if h.ends_with(char::is_whitespace) => h.trim_end(),
                _ => t,
            };
            let label = t
                .replace("ロケテスト", " location test")
                .replace("選手以外", "not players")
                .replace("未使用", "unused")
                .replace("テスト", "test");
            (i as u8, label)
        })
        .collect())
}

/// "AOU2002" and "Camp NAOMI" have no years in the name.
fn season_of(edition: u8, label: &str) -> String {
    match edition {
        1 => return "2001-02".into(),
        9 => return "2004-05/2005-06".into(),
        _ => {}
    }
    // "dddd-dddd" anywhere in the label -> "dddd-" + the last two digits
    label
        .as_bytes()
        .windows(9)
        .find(|w| {
            w[4] == b'-'
                && w.iter()
                    .enumerate()
                    .all(|(i, c)| i == 4 || c.is_ascii_digit())
        })
        .map(|w| {
            format!(
                "{}-{}",
                String::from_utf8_lossy(&w[..4]),
                String::from_utf8_lossy(&w[7..9])
            )
        })
        .unwrap_or_default()
}

/// One 0x620-byte record's raw fields (the 2010-11 record moved to +0x288, the club widened to a u16 at +0x28C, the
/// card code added at +0x2A0: proved against the 3,924 cards both games share).
struct Rec1718 {
    card_no: usize,
    set_no: u16,
    club_id: u16,
    u16s: Vec<u16>,
    edition: u8,
    rarity: u8,
    card_code: String,
    full_latin: String,
    full_kana: String,
    short_kana: String,
    short_latin: String,
    shirts: Vec<u8>,
    position: u8,
    birth_raw: String,
    height: u8,
    weight: u8,
    nat: u8,
    stats: [u8; 6],
    total: u8,
    display: String,
    skill_en: String,
    skill_jp: String,
    model_key: String,
    image_name: String,
}

fn parse_1718(r: &[u8]) -> Rec1718 {
    let u: Vec<u16> = (0..11).map(|i| u16_at(r, 0x288 + 2 * i)).collect();
    Rec1718 {
        card_no: usize::from(u[0]),
        set_no: u[1],
        club_id: u[2],
        u16s: u[3..].to_vec(),
        edition: r[0x29E],
        rarity: r[0x29F],
        card_code: cp932(r, 0x2A0, 0x10),
        full_latin: utf16(r, 0x2B0, 0x40),
        full_kana: cp932(r, 0x2F0, 0x40),
        short_kana: cp932(r, 0x330, 0x40),
        short_latin: utf16(r, 0x370, 0x40),
        shirts: r[0x3B0..0x3B9].to_vec(),
        position: r[0x3B9],
        birth_raw: ascii(r, 0x3BA, 0x10),
        height: r[0x3CA],
        weight: r[0x3CB],
        nat: r[0x3CC],
        stats: r[0x3D7..0x3DD].try_into().unwrap(),
        total: r[0x3DD],
        display: ascii(r, 0x3DF, 0x40),
        skill_en: ascii(r, 0x45F, 0x40),
        skill_jp: cp932(r, 0x49F, 0x40),
        model_key: ascii(r, 0x524, 0x40),
        image_name: ascii(r, 0x564, 0x40),
    }
}

fn build_rows_1718(files: &GameFiles, checks: &mut Checks) -> io::Result<Built> {
    let mut check =
        |name: &str, ok: bool, detail: String| checks.push((name.to_string(), ok, detail));
    let exe = read_exe(&files.exe)?;
    let table = card_table(&exe, CARD_TABLE_VA_1718, CARD_TABLE_N_1718)?;
    let names = exe_names(&exe)?;
    let data = fs::read(&files.players)?;
    check(
        "player_data.bin is whole 0x620-byte records",
        data.len() % REC_1718 == 0,
        format!("{} bytes", data.len()),
    );
    let recs: Vec<&[u8]> = data
        .as_chunks::<REC_1718>()
        .0
        .iter()
        .map(|c| c.as_slice())
        .collect();
    check(
        &format!("record count {RECORDS_1718}"),
        recs.len() == RECORDS_1718,
        format!("{} records", recs.len()),
    );
    let mut vals: Vec<usize> = table.values().copied().collect();
    vals.sort_unstable();
    check(
        "the exe's table maps one card number to each record",
        vals.iter().copied().eq(0..recs.len()),
        format!("{} card numbers", table.len()),
    );
    let jp = CLUBS_1718.iter().chain(NATIONS_1718).map(|t| t.0);
    let differ: Vec<usize> = jp
        .zip(&names)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .take(10)
        .collect();
    check(
        "the exe's club and nationality names are the ones translated here",
        differ.is_empty(),
        format!("differ at {differ:?}"),
    );
    let eds = editions_1718(&files.vername)?;
    let ents = xaf_toc(&files.xaf)?;
    let mut images = HashMap::new();
    let mut faces = HashSet::new();
    for (i, e) in ents.iter().enumerate().filter(|(_, e)| e.is_file) {
        if e.path.starts_with("data/player_card/") {
            images.insert(e.name.to_lowercase(), i);
        }
        if e.path.starts_with("data/models/face/") {
            faces.insert(e.name.to_lowercase());
        }
    }
    let (mut cards, mut mism, mut exact_case, mut face_ok, mut beyond) = (Vec::new(), 0, 0, 0, 0);
    let mut prefix_bad = Vec::new();
    for (&card_no, &idx) in &table {
        let Some(r) = recs.get(idx) else {
            beyond += 1;
            continue;
        };
        let f = parse_1718(r);
        mism += usize::from(f.card_no != card_no);
        let placeholder = f.edition == PLACEHOLDER_EDITION || NOT_PLAYERS.contains(&card_no);
        let (rname, rframe) = rarity(f.rarity);
        let elabel = eds.get(&f.edition).map_or("", String::as_str);
        let season = if placeholder {
            String::new()
        } else {
            season_of(f.edition, elabel)
        };
        // 7 placeholders name no picture: the game's error card
        let image = if f.image_name.is_empty() {
            "ERROR_CARD"
        } else {
            &f.image_name
        };
        let img = images
            .get(&format!("{image}.dds").to_lowercase())
            .map(|&i| &ents[i]);
        exact_case += usize::from(img.is_some_and(|e| e.name == format!("{image}.dds")));
        face_ok += usize::from(faces.contains(&format!("{}.svo", f.model_key).to_lowercase()));
        if f.edition >= 19
            && season.len() >= 7
            && !image.starts_with(&format!("{}{}", &season[2..4], &season[5..7]))
        {
            prefix_bad.push(card_no);
        }
        let club = CLUBS_1718
            .get(usize::from(f.club_id))
            .copied()
            .unwrap_or(("", ""));
        let nat = NATIONS_1718
            .get(usize::from(f.nat))
            .copied()
            .unwrap_or(("", ""));
        let (club_name, nat_name) = if placeholder {
            ("", "")
        } else {
            (club.1, nat.1)
        };
        let pos_name = POSITIONS
            .get(usize::from(f.position))
            .copied()
            .unwrap_or("");
        let birth = birth_iso(&f.birth_raw);
        let image_path = format!("data/player_card/{image}.dds");
        let b = |v: bool| if v { "1" } else { "0" }.to_string();
        let opt = |v: Option<String>| v.unwrap_or_default();
        let u16s: Vec<String> = f.u16s.iter().map(u16::to_string).collect();
        let mut cells = vec![
            card_no.to_string(),
            idx.to_string(),
            f.set_no.to_string(),
            f.edition.to_string(),
            elabel.to_string(),
            season.clone(),
            f.rarity.to_string(),
            rname.to_string(),
            rframe.to_string(),
            b(placeholder),
            f.full_latin,
            f.short_latin.clone(),
            f.display,
            f.full_kana,
            f.short_kana,
            pos_name.to_string(),
            f.club_id.to_string(),
            club_name.to_string(),
            f.nat.to_string(),
            nat_name.to_string(),
            list(&f.shirts),
            birth.clone(),
            f.height.to_string(),
            f.weight.to_string(),
        ];
        cells.extend(f.stats.iter().map(u8::to_string));
        cells.extend([
            f.total.to_string(),
            f.skill_en,
            f.skill_jp,
            f.model_key,
            image_path.clone(),
            opt(img.map(|e| e.offset.to_string())),
            opt(img.map(|e| e.stored.to_string())),
            opt(img.map(|e| e.size.to_string())),
            opt(img.map(|e| b(e.stored != e.size))),
            f.card_code,
            club.0.to_string(),
            nat.0.to_string(),
            u16s.join(" "),
            list(&r[0x3CD..0x3D7]),
            r[0x3DE].to_string(),
            list(&r[0x4DF..0x524]),
            r[0x5A4..0x620].iter().map(|b| format!("{b:02x}")).collect(),
        ]);
        cards.push(Card1718 {
            cells,
            card_no,
            placeholder,
            season,
            position: f.position,
            rarity: f.rarity,
            rarity_name: rname,
            stats: f.stats,
            stats_total: f.total,
            birth,
            short: f.short_latin,
            pos_name,
            club: club_name.to_string(),
            nat: nat_name.to_string(),
            height: f.height,
            weight: f.weight,
            image: img.map(|e| (e.offset, e.stored, e.size)),
            image_path,
        });
    }
    let n = cards.len();
    let real: Vec<&Card1718> = cards.iter().filter(|c| !c.placeholder).collect();
    check(
        "every card number in the exe's table has its record",
        beyond == 0,
        format!("{beyond} point past the file's end"),
    );
    check(
        "the number at +0x288 of each record is its card number",
        mism == 0,
        format!("{mism} differ"),
    );
    check(
        "every card's picture is in the archive",
        cards.iter().all(|c| c.image.is_some()),
        String::new(),
    );
    check(
        "picture names match the archive's case",
        exact_case == n,
        format!("{exact_case}/{n}"),
    );
    check(
        "every 2009-10+ picture name starts with its season (e.g. 1718_)",
        prefix_bad.is_empty(),
        format!("{:?}", &prefix_bad[..prefix_bad.len().min(10)]),
    );
    // card 443 (GARGO) stores 77 while its six sum to 78 - the same in 2005-06 and 2010-11, so it is the original data
    let off_sum: Vec<usize> = real
        .iter()
        .filter(|c| c.stats.iter().map(|&s| u32::from(s)).sum::<u32>() != u32::from(c.stats_total))
        .map(|c| c.card_no)
        .collect();
    check(
        "total = the six stats added up (except the original card 443)",
        off_sum.is_empty() || off_sum == [443],
        format!("{off_sum:?}"),
    );
    check(
        "every card's face model is in the archive",
        face_ok == n,
        format!("{face_ok}/{n}"),
    );
    check(
        "every position is GK, DF, MF or FW",
        cards.iter().all(|c| c.position < 4),
        String::new(),
    );
    check(
        "every real card's rarity is 1..5",
        real.iter().all(|c| (1..=5).contains(&c.rarity)),
        String::new(),
    );
    check(
        "every real card's edition has a season",
        real.iter().all(|c| !c.season.is_empty()),
        String::new(),
    );
    check(
        "every real card's stat is 1..20, total 60..100",
        real.iter().all(|c| {
            c.stats.iter().all(|s| (1..=20).contains(s)) && (60..=100).contains(&c.stats_total)
        }),
        String::new(),
    );
    let nobirth = real.iter().filter(|c| c.birth.is_empty()).count();
    check(
        "every real card's birth date reads",
        nobirth == 0,
        format!("{nobirth} do not"),
    );
    check(
        "card numbers are unique",
        cards
            .iter()
            .map(|c| c.card_no)
            .collect::<HashSet<_>>()
            .len()
            == n,
        String::new(),
    );
    let noname: Vec<usize> = real
        .iter()
        .filter(|c| c.club.is_empty() || c.nat.is_empty())
        .map(|c| c.card_no)
        .collect();
    check(
        "every real card's club and nationality have a name",
        noname.is_empty(),
        format!("{:?}", &noname[..noname.len().min(10)]),
    );
    let by_no: HashMap<usize, &Card1718> = cards.iter().map(|c| (c.card_no, c)).collect();
    for &(no, short, pos, club, nat, stats, birth, h, w) in KNOWN {
        let c = by_no.get(&no);
        let ok = c.is_some_and(|c| {
            (
                c.short.as_str(),
                c.pos_name,
                c.club.as_str(),
                c.nat.as_str(),
            ) == (short, pos, club, nat)
                && c.stats == stats
                && (c.birth.as_str(), c.height, c.weight) == (birth, h, w)
        });
        check(&format!("card {no} is {short}"), ok, String::new());
    }
    check(
        "cards 1 (referee/coach stand-in), 10570 (GUESTPLAY1) and 3509 (dummy) are placeholders",
        [1, 10570, 3509]
            .iter()
            .all(|n| by_no.get(n).is_some_and(|c| c.placeholder)),
        String::new(),
    );
    // its back prints "SECRET" and no stats: the record holds 16 16 16 16 16 20
    check(
        "card 7143 is MARADONA, ALL TIME LEGEND, stored stats 16x5 + 20",
        by_no.get(&7143).is_some_and(|c| {
            c.short == "MARADONA"
                && c.rarity_name == "ALL TIME LEGEND"
                && c.stats == [16, 16, 16, 16, 16, 20]
        }),
        String::new(),
    );
    // newest season first (stable): the overlay keeps only the first 4,096 non-placeholder lines, so this order
    // keeps 2012-13 .. 2017-18
    cards.sort_by(|a, b| b.season.cmp(&a.season));
    let toc = ents
        .iter()
        .enumerate()
        .filter(|(_, e)| e.is_file && e.path.starts_with("data/player_card/"))
        .map(|(i, e)| (e.path.clone(), i))
        .collect();
    let rows = cards
        .into_iter()
        .map(|c| Row {
            cells: c.cells,
            card_no: c.card_no,
            placeholder: c.placeholder,
            image: c.image,
            image_path: c.image_path,
            ..Row::default()
        })
        .collect();
    Ok((rows, toc, ents))
}

fn cell(v: &str) -> String {
    v.replace(['\t', '\r', '\n'], " ")
}

/// Written beside, then swapped in: nobody reads half a file.
fn write_catalogue(cols: &[&str], rows: &[Row], path: &Path) -> io::Result<()> {
    let mut s = cols.join("\t") + "\n";
    for r in rows {
        s += &r
            .cells
            .iter()
            .map(|c| cell(c))
            .collect::<Vec<_>>()
            .join("\t");
        s.push('\n');
    }
    let tmp = path.with_extension("tsv.tmp");
    fs::write(&tmp, s)?;
    fs::rename(&tmp, path)
}

/// A DXT1 (BC1) or DXT5 (BC3, one 2017-18 card) DDS -> (width, height, RGBA8). Written out instead of using
/// `image`'s decoder, which rounds the two blended colours differently (by 1) from the Python maker's Pillow; a
/// card's colours stay what they were.
fn dxt1(dds: &[u8]) -> io::Result<(u32, u32, Vec<u8>)> {
    if dds.len() < 128 || !matches!(&dds[84..88], b"DXT1" | b"DXT5") {
        return Err(bad("not a DXT1 or DXT5 picture"));
    }
    let dxt5 = &dds[84..88] == b"DXT5";
    let bsz = if dxt5 { 16 } else { 8 };
    let (h, w) = (u32_at(dds, 12), u32_at(dds, 16));
    let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    if w == 0 || h == 0 || w > 4096 || h > 4096 || dds.len() < 128 + bw * bh * bsz {
        return Err(bad("DDS size"));
    }
    let x8 = |v: u16, sh: u32, bits: u32| {
        let c = u32::from((v >> sh) & ((1 << bits) - 1));
        ((c << (8 - bits)) | (c >> (2 * bits - 8))) as u8
    };
    let mut px = vec![0u8; (w * h * 4) as usize];
    for (i, blk) in dds[128..].chunks_exact(bsz).take(bw * bh).enumerate() {
        let b = &blk[bsz - 8..]; // the colours; DXT5 puts 8 bytes of alpha first
        let (c0, c1) = (u16_at(b, 0), u16_at(b, 2));
        let rgb = |c: u16| [x8(c, 11, 5), x8(c, 5, 6), x8(c, 0, 5)];
        let (a, z) = (rgb(c0), rgb(c1));
        let mix = |p: u32, q: u32, d: u32| -> [u8; 3] {
            std::array::from_fn(|k| ((p * u32::from(a[k]) + q * u32::from(z[k])) / d) as u8)
        };
        let pal: [[u8; 4]; 4] = {
            let t = |c: [u8; 3], al| [c[0], c[1], c[2], al];
            if c0 > c1 || dxt5 {
                let (m, n) = (mix(2, 1, 3), mix(1, 2, 3));
                [t(a, 255), t(z, 255), t(m, 255), t(n, 255)]
            } else {
                [t(a, 255), t(z, 255), t(mix(1, 1, 2), 255), [0; 4]]
            }
        };
        let bits = u32_at(b, 4);
        let alpha = if dxt5 {
            bc3_alpha(&blk[..8])
        } else {
            [255; 16]
        };
        for k in 0..16usize {
            let (x, y) = ((i % bw) * 4 + k % 4, (i / bw) * 4 + k / 4);
            if x < w as usize && y < h as usize {
                let o = (y * w as usize + x) * 4;
                px[o..o + 4].copy_from_slice(&pal[((bits >> (2 * k)) & 3) as usize]);
                if dxt5 {
                    px[o + 3] = alpha[k];
                }
            }
        }
    }
    Ok((w, h, px))
}

/// DXT5's sixteen alpha values from its 8 alpha bytes: two end points and 3-bit indices.
fn bc3_alpha(b: &[u8]) -> [u8; 16] {
    let (a0, a1) = (u32::from(b[0]), u32::from(b[1]));
    let v: [u8; 8] = if a0 > a1 {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            _ => (((8 - i as u32) * a0 + (i as u32 - 1) * a1) / 7) as u8,
        })
    } else {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            6 => 0,
            7 => 255,
            _ => (((6 - i as u32) * a0 + (i as u32 - 1) * a1) / 5) as u8,
        })
    };
    let bits = b[2..8]
        .iter()
        .enumerate()
        .fold(0u64, |acc, (i, &x)| acc | u64::from(x) << (8 * i));
    std::array::from_fn(|k| v[((bits >> (3 * k)) & 7) as usize])
}

fn write_pictures(
    rows: &[Row],
    toc: &HashMap<String, usize>,
    ents: &[Entry],
    xaf: &Path,
    dir: &Path,
    say: &dyn Fn(&str),
) -> io::Result<(usize, Vec<String>)> {
    let (mut done, mut fails, mut cache) = (0, Vec::new(), HashMap::<&str, Vec<u8>>::new());
    let mut f = File::open(xaf)?;
    let len = f.metadata()?.len();
    for (n, r) in rows.iter().enumerate() {
        let mut one = || -> io::Result<()> {
            let path = r.image_path.as_str();
            if !cache.contains_key(path) {
                let e = &ents[*toc.get(path).ok_or_else(|| bad("not in the archive"))?];
                if u64::from(e.stored) > len || e.size > 1 << 26 {
                    return Err(bad("size beyond the archive"));
                }
                let mut raw = vec![0u8; e.stored as usize];
                f.seek(SeekFrom::Start(e.offset))?;
                f.read_exact(&mut raw)?;
                let dds = if e.stored < e.size {
                    lzw(&raw, e.size as usize)?
                } else {
                    raw
                };
                if dds.len() != e.size as usize || !dds.starts_with(b"DDS ") {
                    return Err(bad(format!("decoded {} bytes", dds.len())));
                }
                cache.insert(path, dds);
            }
            let (w, h, px) = dxt1(&cache[path])?;
            let out = dir.join(format!("{}.png", r.card_no));
            let tmp = dir.join(format!("{}.png.tmp", r.card_no));
            PngEncoder::new_with_quality(
                File::create(&tmp)?,
                CompressionType::Level(6),
                FilterType::Adaptive,
            )
            .write_image(&px, w, h, ExtendedColorType::Rgba8)
            .map_err(io::Error::other)?;
            fs::rename(&tmp, out)
        };
        match one() {
            Ok(()) => done += 1,
            Err(e) => fails.push(format!("card {} ({}): {e}", r.card_no, r.image_path)),
        }
        if (n + 1) % 500 == 0 {
            say(&format!("  pictures {} / {}", n + 1, rows.len()));
        }
    }
    Ok((done, fails))
}

/// Makes `out/catalogue.tsv` and `out/cards/<number>.png` from the game in `game`. Returns the old script's exit
/// code: 0 written and every check passed, 1 a check failed (nothing written if it was a catalogue check),
/// 2 a game file is missing or `out` cannot be written. `say` hears the progress lines.
pub fn run(edition: Edition, game: &Path, out: &Path, say: &dyn Fn(&str)) -> i32 {
    let Some(files) = game_files(edition, game, say) else {
        say(&format!(
            "GAME must be the {} game folder, the one that holds client_Release.exe",
            edition.name()
        ));
        return 2;
    };
    let cards_dir = out.join("cards");
    if fs::symlink_metadata(&cards_dir).is_ok_and(|m| m.file_type().is_symlink()) {
        say(&format!(
            "refused: {} is a link to another folder - writing would change that folder",
            cards_dir.display()
        ));
        return 2;
    }
    if let Err(e) = fs::create_dir_all(&cards_dir) {
        say(&format!("cannot make {}: {e}", cards_dir.display()));
        return 2;
    }
    say(&format!(
        "game files: {}",
        files.exe.parent().unwrap().display()
    ));
    let mut checks = Vec::new();
    let (rows, toc, ents) = match match edition {
        Edition::E1011 => build_rows_1011(&files, &mut checks),
        Edition::E1718 => build_rows_1718(&files, &mut checks),
    } {
        Ok(x) => x,
        Err(e) => {
            say(&format!("FAIL  reading the game's files: {e}"));
            return 1;
        }
    };
    for (name, ok, detail) in &checks {
        let d = if *ok || detail.is_empty() {
            String::new()
        } else {
            format!("  - {detail}")
        };
        say(&format!(
            "  {}  {name}{d}",
            if *ok { "PASS" } else { "FAIL" }
        ));
    }
    if !checks.iter().all(|c| c.1) {
        say(&format!(
            "not written: this copy does not match WCCF {}'s player data",
            match edition {
                Edition::E1011 => "2010-11 Rev D",
                Edition::E1718 => "2017-18",
            }
        ));
        return 1;
    }
    if let Err(e) = write_catalogue(
        match edition {
            Edition::E1011 => &COLS,
            Edition::E1718 => COLS_1718,
        },
        &rows,
        &out.join("catalogue.tsv"),
    ) {
        say(&format!("cannot write catalogue.tsv: {e}"));
        return 2;
    }
    let real = rows.iter().filter(|r| !r.placeholder).count();
    say(&format!(
        "catalogue.tsv: {} cards ({real} real, {} placeholders)",
        rows.len(),
        rows.len() - real
    ));
    match write_pictures(&rows, &toc, &ents, &files.xaf, &cards_dir, say) {
        Ok((done, fails)) => {
            say(&format!("pictures: {done} of {} written", rows.len()));
            fails
                .iter()
                .take(20)
                .for_each(|m| say(&format!("  FAIL  {m}")));
            i32::from(!fails.is_empty())
        }
        Err(e) => {
            say(&format!("cannot read {}: {e}", files.xaf.display()));
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn birth_dates() {
        assert_eq!(birth_iso("1987/6/24"), "1987-06-24");
        assert_eq!(birth_iso("1987/6"), "");
        assert_eq!(birth_iso("a/b/c"), "");
    }

    #[test]
    fn cells_stay_on_one_line() {
        assert_eq!(cell("a\tb\r\nc"), "a b  c");
    }

    #[test]
    fn dxt1_blends_and_transparency() {
        // synthetic 4x4: c0 = white, c1 = black, indices 0,1,2,3 on the first row
        let mut d = vec![0u8; 128];
        d[84..88].copy_from_slice(b"DXT1");
        d[12] = 4;
        d[16] = 4;
        d.extend([0xFF, 0xFF, 0, 0, 0b1110_0100, 0, 0, 0]);
        let (w, h, px) = dxt1(&d).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(&px[0..4], [255, 255, 255, 255]);
        assert_eq!(&px[4..8], [0, 0, 0, 255]);
        assert_eq!(&px[8..12], [170, 170, 170, 255]);
        assert_eq!(&px[12..16], [85, 85, 85, 255]);
        // c0 <= c1: index 3 is transparent black
        d[128..132].copy_from_slice(&[0, 0, 0xFF, 0xFF]);
        assert_eq!(&dxt1(&d).unwrap().2[12..16], [0, 0, 0, 0]);
    }

    #[test]
    fn dxt5_alpha() {
        // 4x4 DXT5: alpha end points 255 and 0 (8-value ramp), indices 0..7 then 0, white/black colours
        let mut d = vec![0u8; 128];
        d[84..88].copy_from_slice(b"DXT5");
        d[12] = 4;
        d[16] = 4;
        let idx: u64 = (0..8).fold(0, |a, k| a | k << (3 * k));
        let mut blk = vec![255u8, 0];
        blk.extend(&idx.to_le_bytes()[..6]);
        blk.extend([0xFF, 0xFF, 0, 0, 0, 0, 0, 0]);
        d.extend(blk);
        let px = dxt1(&d).unwrap().2;
        let a: Vec<u8> = (0..8).map(|k| px[4 * k + 3]).collect();
        assert_eq!(a, [255, 0, 218, 182, 145, 109, 72, 36]);
        assert_eq!(&px[0..3], [255, 255, 255]);
    }

    #[test]
    fn seasons_from_labels() {
        assert_eq!(season_of(30, "WCCF 2017-1819 Ver"), "2017-19");
        assert_eq!(season_of(30, "no years"), "");
        assert_eq!(season_of(1, "AOU2002"), "2001-02");
        assert_eq!(season_of(9, "Camp NAOMI"), "2004-05/2005-06");
    }

    #[test]
    fn ver_name_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("VerName.txt");
        let (raw, _, _) = encoding_rs::SHIFT_JIS.encode(
            "WCCF 2016-1718 t
選手以外 f
テスト
",
        );
        fs::write(&p, raw).unwrap();
        let e = editions_1718(&p).unwrap();
        assert_eq!(e[&0], "WCCF 2016-1718");
        assert_eq!(e[&1], "not players");
        assert_eq!(e[&2], "test");
    }

    #[test]
    fn record_1718_fields() {
        let mut r = vec![0u8; REC_1718];
        r[0x288..0x28A].copy_from_slice(&7049u16.to_le_bytes());
        r[0x28C..0x28E].copy_from_slice(&33u16.to_le_bytes());
        r[0x29E] = 30;
        r[0x29F] = 5;
        r[0x3B9] = 3;
        r[0x3D7..0x3DD].copy_from_slice(&[20, 8, 20, 16, 19, 15]);
        r[0x3BA..0x3C4].copy_from_slice(b"1987/6/24 ");
        for (i, c) in "MESSI".encode_utf16().enumerate() {
            r[0x370 + 2 * i..0x372 + 2 * i].copy_from_slice(&c.to_le_bytes());
        }
        r[0x564..0x56B].copy_from_slice(b"1718_01");
        let f = parse_1718(&r);
        assert_eq!(
            (f.card_no, f.club_id, f.edition, f.rarity),
            (7049, 33, 30, 5)
        );
        assert_eq!(
            (f.short_latin.as_str(), f.position, f.stats[0]),
            ("MESSI", 3, 20)
        );
        assert_eq!(birth_iso(&f.birth_raw), "1987-06-24");
        assert_eq!(f.image_name, "1718_01");
        assert_eq!(f.u16s.len(), 8);
    }

    #[test]
    fn tables_1718_are_whole() {
        assert_eq!((CLUBS_1718.len(), NATIONS_1718.len()), (190, 96));
        assert_eq!(COLS_1718.len(), 47);
    }

    #[test]
    fn codes_withhold_guesses() {
        assert_eq!(name_for(NATIONALITY, 5), "Japan");
        assert_eq!(name_for(NATIONALITY, 79), ""); // low confidence
        assert_eq!(name_for(CLUB, 250), "");
    }
}
