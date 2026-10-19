//! The catalogue made from the player's own copy of the 2010-11 game (Rev D): `catalogue.tsv`, one line per card, and
//! `cards/<number>.png`, each card's 128x128 picture (the board shows its left 88 columns). Only reads the game.
//! Ported from `legacy/2010-11/code/cards/catalogue.py` and the 2017-18 kit's `cards.py`: the two are the same program
//! (same record layout, same code tables, same decompressor), so one function serves both kits and the edition
//! does not select anything. The 2017-18 game itself has other records and tables and keeps its own maker.
//!
//! Reads `client_Release.exe` (card number -> record table at VA 0x00AF8610), `prog_data/player_data/player_data.bin`
//! (3,927 records of 0x328 bytes) and `data/wccf_data.xaf` (table of contents, then `data/player_card/*.dds`).

use super::codes::{CLUB, EDITION, NATIONALITY, name_for};
use super::pictures::lzw;
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
}

pub fn game_files(game: &Path, say: &dyn Fn(&str)) -> Option<GameFiles> {
    let at = |root: &Path| GameFiles {
        exe: root.join("client_Release.exe"),
        players: root.join("prog_data/player_data/player_data.bin"),
        xaf: root.join("data/wccf_data.xaf"),
    };
    for root in [game.to_path_buf(), game.join("extracted")] {
        let f = at(&root);
        if [&f.exe, &f.players, &f.xaf].iter().all(|p| p.is_file()) {
            return Some(f);
        }
    }
    let f = at(game);
    for p in [&f.exe, &f.players, &f.xaf] {
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
fn card_table(exe: &Path) -> io::Result<BTreeMap<usize, usize>> {
    let d = fs::read(exe)?;
    if !d.starts_with(b"MZ") {
        return Err(bad(format!("{} is not a Windows program", exe.display())));
    }
    let off = va_to_offset(&d, CARD_TABLE_VA)?;
    if d.len() < off + 4 * CARD_TABLE_N {
        return Err(bad("card table runs past the file"));
    }
    Ok((0..CARD_TABLE_N)
        .filter_map(|c| {
            usize::try_from(u32_at(&d, off + 4 * c) as i32)
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

/// One catalogue line: the file's cells in column order, and the values the checks look at.
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

fn build_rows(files: &GameFiles, checks: &mut Checks) -> io::Result<Built> {
    let mut check =
        |name: &str, ok: bool, detail: String| checks.push((name.to_string(), ok, detail));
    let table = card_table(&files.exe)?;
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

fn cell(v: &str) -> String {
    v.replace(['\t', '\r', '\n'], " ")
}

/// Written beside, then swapped in: nobody reads half a file.
fn write_catalogue(rows: &[Row], path: &Path) -> io::Result<()> {
    let mut s = COLS.join("\t") + "\n";
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

/// A DXT1 (BC1) DDS -> (width, height, RGBA8). Written out instead of using `image`'s decoder, which rounds the two
/// blended colours differently (by 1) from the Python maker's Pillow; a card's colours stay what they were.
fn dxt1(dds: &[u8]) -> io::Result<(u32, u32, Vec<u8>)> {
    if dds.len() < 128 || &dds[84..88] != b"DXT1" {
        return Err(bad("not a DXT1 picture"));
    }
    let (h, w) = (u32_at(dds, 12), u32_at(dds, 16));
    let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    if w == 0 || h == 0 || w > 4096 || h > 4096 || dds.len() < 128 + bw * bh * 8 {
        return Err(bad("DDS size"));
    }
    let x8 = |v: u16, sh: u32, bits: u32| {
        let c = u32::from((v >> sh) & ((1 << bits) - 1));
        ((c << (8 - bits)) | (c >> (2 * bits - 8))) as u8
    };
    let mut px = vec![0u8; (w * h * 4) as usize];
    for (i, b) in dds[128..]
        .as_chunks::<8>()
        .0
        .iter()
        .take(bw * bh)
        .enumerate()
    {
        let (c0, c1) = (u16_at(b, 0), u16_at(b, 2));
        let rgb = |c: u16| [x8(c, 11, 5), x8(c, 5, 6), x8(c, 0, 5)];
        let (a, z) = (rgb(c0), rgb(c1));
        let mix = |p: u32, q: u32, d: u32| -> [u8; 3] {
            std::array::from_fn(|k| ((p * u32::from(a[k]) + q * u32::from(z[k])) / d) as u8)
        };
        let pal: [[u8; 4]; 4] = {
            let t = |c: [u8; 3], al| [c[0], c[1], c[2], al];
            if c0 > c1 {
                let (m, n) = (mix(2, 1, 3), mix(1, 2, 3));
                [t(a, 255), t(z, 255), t(m, 255), t(n, 255)]
            } else {
                [t(a, 255), t(z, 255), t(mix(1, 1, 2), 255), [0; 4]]
            }
        };
        let bits = u32_at(b, 4);
        for k in 0..16usize {
            let (x, y) = ((i % bw) * 4 + k % 4, (i / bw) * 4 + k / 4);
            if x < w as usize && y < h as usize {
                let o = (y * w as usize + x) * 4;
                px[o..o + 4].copy_from_slice(&pal[((bits >> (2 * k)) & 3) as usize]);
            }
        }
    }
    Ok((w, h, px))
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
pub fn run(game: &Path, out: &Path, say: &dyn Fn(&str)) -> i32 {
    let Some(files) = game_files(game, say) else {
        say("GAME must be the game folder, the one that holds client_Release.exe");
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
    let (rows, toc, ents) = match build_rows(&files, &mut checks) {
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
        say("not written: this copy does not match WCCF 2010-11 Rev D's player data");
        return 1;
    }
    if let Err(e) = write_catalogue(&rows, &out.join("catalogue.tsv")) {
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
    fn codes_withhold_guesses() {
        assert_eq!(name_for(NATIONALITY, 5), "Japan");
        assert_eq!(name_for(NATIONALITY, 79), ""); // low confidence
        assert_eq!(name_for(CLUB, 250), "");
    }
}
