//! The card pictures the panel shows (`<cards>\<number>.png`), made on the player's PC: from the player's own game
//! (each card's 128x128 DXT1 picture in `data/wccf_data.xaf`; the card face is its left 88 columns, the rest edge
//! smear) or, as an option, from the high-quality pack (a ZIP of card scans with `cards.db` linking them to both
//! games' card numbers). The game's pictures (`<number>.png`) are always there, so a card the pack lacks keeps its
//! own; the pack's (`<number>.jpg`) come beside them and the board takes them first. Newer cards also have the
//! game's own album art (`<face>_F.dds`, the whole card, 256x357 in a 256x512 texture): it comes out as
//! `<number>_f.jpg` and the board takes it before the pack's scan, then the face (`board::picture_names`); its back
//! (`<face>_B.dds`, same size) comes out as `<number>_b.jpg` for the card views that show it.
//! Both editions keep the same archive format, and each `player_data.bin` record names its card number and picture.

use crate::Line;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The face: the left 88 columns of the game's 128x128 picture (what the original overlay showed).
pub const FACE: (u32, u32) = (88, 128);
/// The folder's note of what made it; another answer from [`wanted`] makes it again.
const MARKER: &str = "source.txt";
pub const VERSION: u32 = 2; // 2: the album fronts and backs (`<number>_f.jpg`, `<number>_b.jpg`)
/// The album card in the top of its 256x512 texture (the rows under it are edge smear).
pub const ALBUM: (u32, u32) = (256, 357);
/// The album front's JPEG quality: about 40 % of the PNG's size, no visible loss at card size.
const ALBUM_QUALITY: u8 = 90;

fn bad(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

/// The game's `YS\x02\x00` LZW stream (yabukita::StreamAdapterLZW): 9- to 12-bit codes MSB first, the
/// width grows one code early, no clear code: the dictionary starts over at 4095 entries and the next code is a root.
pub fn lzw(data: &[u8], size: usize) -> io::Result<Vec<u8>> {
    if !data.starts_with(b"YS\x02\x00") {
        return Err(bad("not a YS stream"));
    }
    let (mut prefix, mut chr, mut first) = ([0u16; 4096], [0u8; 4096], [0u8; 4096]);
    for i in 0..256 {
        (chr[i], first[i]) = (i as u8, i as u8);
    }
    let (mut acc, mut nbits, mut pos) = (0u32, 0u32, 4usize);
    let (mut width, mut next, mut restart, mut prev) = (9u32, 256usize, true, 0usize);
    let mut out = Vec::with_capacity(size.min(1 << 24));
    let mut stack = Vec::with_capacity(4096);
    let emit =
        |out: &mut Vec<u8>, stack: &mut Vec<u8>, prefix: &[u16], chr: &[u8], mut c: usize| {
            stack.clear();
            loop {
                stack.push(chr[c]);
                if c < 256 {
                    break;
                }
                c = prefix[c] as usize;
            }
            out.extend(stack.iter().rev());
        };
    while out.len() < size {
        while nbits < width {
            let Some(&b) = data.get(pos) else {
                out.truncate(size);
                return Ok(out);
            };
            acc = (acc << 8) | u32::from(b);
            pos += 1;
            nbits += 8;
        }
        nbits -= width;
        let code = ((acc >> nbits) & ((1 << width) - 1)) as usize;
        if restart {
            if code >= 256 {
                return Err(bad("bad first code"));
            }
            out.push(code as u8);
            (prev, restart) = (code, false);
            continue;
        }
        let new_first = if code < next {
            emit(&mut out, &mut stack, &prefix, &chr, code);
            first[code]
        } else if code == next {
            emit(&mut out, &mut stack, &prefix, &chr, prev);
            out.push(first[prev]);
            first[prev]
        } else {
            return Err(bad("code past the dictionary"));
        };
        (prefix[next], chr[next], first[next]) = (prev as u16, new_first, first[prev]);
        next += 1;
        if next + 1 == 1 << width && width < 12 {
            width += 1;
        }
        prev = code;
        if next >= 4095 {
            (next, width, restart) = (256, 9, true);
        }
    }
    out.truncate(size);
    Ok(out)
}

/// One file in the archive.
struct Entry {
    offset: u64,
    stored: usize,
    size: usize,
}

/// The archive's `data/player_card/` files by lower-case name.
fn player_cards(xaf: &mut File) -> io::Result<HashMap<String, Entry>> {
    let mut hdr = [0u8; 0x100];
    xaf.read_exact(&mut hdr)?;
    if &hdr[..4] != b"xaf0" {
        return Err(bad("wccf_data.xaf is not an xaf0 archive"));
    }
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let (sector, n) = (u64::from(u32_at(&hdr, 8)), u32_at(&hdr, 12) as usize);
    let len = xaf.metadata()?.len();
    let toc_len = n
        .checked_mul(0xB0)
        .filter(|&b| b as u64 <= len)
        .ok_or_else(|| bad("xaf0 table larger than the file"))?;
    let mut toc = vec![0u8; toc_len];
    xaf.read_exact(&mut toc)?;
    let ents: Vec<&[u8]> = toc.chunks(0xB0).collect();
    let name = |e: &[u8]| {
        let s = &e[..0x80];
        String::from_utf8_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(0x80)]).to_lowercase()
    };
    let parent =
        |e: &[u8]| usize::try_from(i32::from_le_bytes(e[0x84..0x88].try_into().unwrap())).ok();
    // the folder data/player_card: named player_card, its parent data, that one at the top
    let is_dir = |i: usize| {
        name(ents[i]) == "player_card"
            && parent(ents[i])
                .is_some_and(|p| p < n && name(ents[p]) == "data" && parent(ents[p]).is_none())
    };
    Ok(ents
        .iter()
        .filter(|e| e[0x80] != 0 && parent(e).is_some_and(|p| p < n && is_dir(p)))
        .map(|e| {
            let entry = Entry {
                offset: u64::from(u32_at(e, 0xA0)) * sector,
                size: u32_at(e, 0x94) as usize,
                stored: u32_at(e, 0x98) as usize,
            };
            (name(e), entry)
        })
        .collect())
}

/// Each card's number and picture name from `player_data.bin` (2010-11: 0x328-byte records from +0; 2017-18:
/// 0x620-byte records, the same fields from +0x288 and the picture name at +0x564).
fn cards(players: &[u8], edition: &str) -> io::Result<Vec<(u16, String)>> {
    let (rec, no, img) = if edition == "1718" {
        (0x620, 0x288, 0x564)
    } else {
        (0x328, 0, 0x2DA)
    };
    if !players.len().is_multiple_of(rec) {
        return Err(bad("player_data.bin: not this edition's records"));
    }
    Ok(players
        .chunks(rec)
        .map(|r| {
            let s = &r[img..img + 0x40];
            let name =
                String::from_utf8_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(0x40)]);
            (
                u16::from_le_bytes([r[no], r[no + 1]]),
                format!("{name}.dds").to_lowercase(),
            )
        })
        .filter(|(n, _)| *n > 0)
        .collect())
}

/// Written beside, then swapped in: the panel never reads half a picture.
fn save(img: &DynamicImage, path: &Path) -> io::Result<()> {
    let tmp = path.with_extension("png.tmp");
    img.save_with_format(&tmp, ImageFormat::Png)
        .map_err(io::Error::other)?;
    fs::rename(&tmp, path)
}

/// The album card of a decoded `<face>_F.dds` or `<face>_B.dds`: its top 256x357. Any other texture size is not
/// the album's, and a crop of it would outrank the right picture on the board: `None`.
fn album_card(tex: &DynamicImage) -> Option<DynamicImage> {
    ((tex.width(), tex.height()) == (ALBUM.0, 512)).then(|| tex.crop_imm(0, 0, ALBUM.0, ALBUM.1))
}

/// Written beside, then swapped in, like [`save`].
fn save_jpg(img: &DynamicImage, path: &Path) -> io::Result<()> {
    let tmp = path.with_extension("jpg.tmp");
    let rgb = img.to_rgb8();
    let done = File::create(&tmp)
        .and_then(|f| {
            let mut w = io::BufWriter::new(f);
            JpegEncoder::new_with_quality(&mut w, ALBUM_QUALITY)
                .write_image(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
                .map_err(io::Error::other)?;
            w.flush() // a failed tail write is an error, not a short file renamed into place
        })
        .and_then(|()| fs::rename(&tmp, path));
    if done.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    done
}

fn fail_count<'a>(made: &'a mut Made, side: &str) -> &'a mut usize {
    if side == "f" {
        &mut made.art_fails
    } else {
        &mut made.back_fails
    }
}

/// What [`from_game`] made.
#[derive(Debug, Default)]
pub struct Made {
    /// Faces written (`<number>.png`).
    pub faces: usize,
    /// Album fronts written (`<number>_f.jpg`); their backs come with them.
    pub fronts: usize,
    /// Album fronts that could not be read or written (a texture of another size is skipped, not counted).
    pub art_fails: usize,
    /// The same for backs: reported, but not what [`ensure`] retries on (no view reads them yet).
    pub back_fails: usize,
    /// Every problem, for the setup output.
    pub fails: Vec<String>,
}

/// The archive file `e` as a decoded DXT1 picture.
fn load(xaf: &mut File, file_len: u64, e: &Entry) -> io::Result<DynamicImage> {
    if e.stored as u64 > file_len || e.size > 1 << 26 {
        return Err(bad("size beyond the archive"));
    }
    let mut raw = vec![0u8; e.stored];
    xaf.seek(SeekFrom::Start(e.offset))?;
    xaf.read_exact(&mut raw)?;
    let dds = if e.stored < e.size {
        lzw(&raw, e.size)?
    } else {
        raw
    };
    image::load_from_memory_with_format(&dds, ImageFormat::Dds).map_err(io::Error::other)
}

/// Every card's face from the game in `game` (the folder with `prog_data` and `data`) into `out` as
/// `<number>.png`, 88x128; in 2017-18 also, where the game has the album art (`<face>_F.dds` and `_B.dds`, 256x512
/// textures), the card's front and back as `<number>_f.jpg` and `<number>_b.jpg`, 256x357. One bad picture never
/// stops the rest.
pub fn from_game(game: &Path, edition: &str, out: &Path) -> io::Result<Made> {
    let players = fs::read(game.join("prog_data/player_data/player_data.bin"))?;
    let mut xaf = File::open(game.join("data/wccf_data.xaf"))?;
    let file_len = xaf.metadata()?.len();
    let toc = player_cards(&mut xaf)?; // names in lower case: `_F` and `_f` are one
    fs::create_dir_all(out)?;
    let mut made = Made::default();
    for (no, name) in cards(&players, edition)? {
        let mut one = || -> io::Result<()> {
            let e = toc.get(&name).ok_or_else(|| bad("not in the archive"))?;
            let img = load(&mut xaf, file_len, e)?;
            let (w, h) = (FACE.0.min(img.width()), FACE.1.min(img.height()));
            save(&img.crop_imm(0, 0, w, h), &out.join(format!("{no}.png")))
        };
        match one() {
            Ok(()) => made.faces += 1,
            Err(e) => made.fails.push(format!("card {no} ({name}): {e}")),
        }
        // older cards have no album art: they keep the face (and the pack's scan, if chosen)
        for side in ["f", "b"].into_iter().filter(|_| edition == "1718") {
            let art = name.replace(".dds", &format!("_{side}.dds"));
            let Some(e) = toc.get(&art) else { continue };
            match load(&mut xaf, file_len, e) {
                Err(e) => {
                    *fail_count(&mut made, side) += 1;
                    made.fails.push(format!("card {no} ({art}): {e}"));
                }
                Ok(t) => match album_card(&t) {
                    None => made.fails.push(format!(
                        "card {no} ({art}): {}x{}, not the album's 256x512",
                        t.width(),
                        t.height()
                    )),
                    Some(card) => match save_jpg(&card, &out.join(format!("{no}_{side}.jpg"))) {
                        Ok(()) => made.fronts += usize::from(side == "f"),
                        Err(e) => {
                            *fail_count(&mut made, side) += 1;
                            made.fails.push(format!("card {no} ({art}): {e}"));
                        }
                    },
                },
            }
        }
    }
    Ok(made)
}

/// The pack's card per game card number: the name+stats link first, then a regular printing, then the oldest
/// (the pack's own export order).
const PICK: &str = "SELECT l.game_card_no, c.front FROM game_link l JOIN cards c USING (ver, card)
    WHERE l.game = ?1 AND c.front IS NOT NULL
    ORDER BY l.game_card_no, l.method != 'name+stats', c.card LIKE '%EXTRA%', c.ver, c.card";

/// The pack's scans for `edition` ("1011" or "1718") into `out` as `<number>.jpg`, copied as they are (180x254
/// whole cards; the board trims them as it draws) -> (written, failures). Cards the pack lacks are left as they are.
pub fn from_pack(pack: &Path, edition: &str, out: &Path) -> io::Result<(usize, Vec<String>)> {
    let mut zip = zip::ZipArchive::new(File::open(pack)?).map_err(io::Error::other)?;
    if zip.index_for_name("cards.db").is_none() {
        return from_prebuilt(&mut zip, edition, out);
    }
    let mut db = tempfile::NamedTempFile::new()?;
    io::copy(
        &mut zip.by_name("cards.db").map_err(io::Error::other)?,
        &mut db,
    )?;
    let picks: Vec<(u32, String)> = {
        let con = rusqlite::Connection::open_with_flags(
            db.path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(io::Error::other)?;
        let mut q = con.prepare(PICK).map_err(io::Error::other)?;
        let rows = q
            .query_map([edition], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(io::Error::other)?;
        rows.collect::<Result<_, _>>().map_err(io::Error::other)?
    };
    fs::create_dir_all(out)?;
    let (mut done, mut fails, mut last) = (0, Vec::new(), None);
    // the real stats come along into the same folder (the board reads them from there)
    if let Err(e) = super::hq::from_db(db.path(), edition, out) {
        fails.push(format!("stats: {e}"));
    }
    for (no, front) in picks {
        if last.replace(no) == Some(no) {
            continue; // a reprint: the first one wins
        }
        let mut one = || -> io::Result<()> {
            let mut f = zip.by_name(&front).map_err(io::Error::other)?;
            let path = out.join(format!("{no}.jpg"));
            let tmp = path.with_extension("jpg.tmp");
            io::copy(&mut f, &mut File::create(&tmp)?)?;
            fs::rename(&tmp, &path)
        };
        match one() {
            Ok(()) => done += 1,
            Err(e) => fails.push(format!("card {no} ({front}): {e}")),
        }
    }
    Ok((done, fails))
}

/// The pack as players get it (forza-wccf-card-pictures-hq-*.zip, no cards.db): each scan once in `pictures/`, and
/// per edition `2017-2018/cards.tsv` / `2010-2011/cards.tsv` (`<number>\t<picture>`, one card a line) and the
/// edition's hq_stats.tsv. The edition's cards go into `out` as `<number>.jpg`.
fn from_prebuilt(
    zip: &mut zip::ZipArchive<File>,
    edition: &str,
    out: &Path,
) -> io::Result<(usize, Vec<String>)> {
    let top = if edition == "1011" {
        "2010-2011"
    } else {
        "2017-2018"
    };
    let mut list = String::new();
    io::Read::read_to_string(
        &mut zip.by_name(&format!("{top}/cards.tsv")).map_err(|_| {
            bad("the ZIP is not a card pack: no cards.db and no cards.tsv for this edition")
        })?,
        &mut list,
    )?;
    fs::create_dir_all(out)?;
    let (mut done, mut fails) = (0, Vec::new());
    let mut copy = |from: &str, to: &str| -> io::Result<()> {
        let mut f = zip.by_name(from).map_err(io::Error::other)?;
        let path = out.join(to);
        let tmp = path.with_extension("tmp");
        io::copy(&mut f, &mut File::create(&tmp)?)?;
        fs::rename(&tmp, &path)
    };
    if let Err(e) = copy(&format!("{top}/{}", super::hq::FILE), super::hq::FILE) {
        fails.push(format!("stats: {e}"));
    }
    for line in list.lines() {
        let Some((no, pic)) = line.split_once('\t').filter(|(n, p)| {
            n.parse::<u32>().is_ok() && !p.contains(['/', '\\']) && p.ends_with(".jpg")
        }) else {
            continue;
        };
        match copy(&format!("pictures/{pic}"), &format!("{no}.jpg")) {
            Ok(()) => done += 1,
            Err(e) => fails.push(format!("card {no} ({pic}): {e}")),
        }
    }
    Ok((done, fails))
}

/// What the folder should hold: the game's pictures, or the pack's over them.
fn wanted(pack: Option<&Path>) -> String {
    match pack {
        // its size too: a newer pack saved over the old one is taken again
        Some(p) => format!(
            "{VERSION} hq {} {}",
            fs::metadata(p).map_or(0, |m| m.len()),
            p.display()
        ),
        None => format!("{VERSION} game"),
    }
}

/// Deletes the `.jpg` files in `dir` named `<number>` (the pack's scans) or, with `art`, `<number>_f` / `<number>_b`.
fn remove_jpgs(dir: &Path, art: bool) -> io::Result<()> {
    for f in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = f.path();
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned());
        let number = stem.as_deref().and_then(|s| {
            let tail = s.len().saturating_sub(2);
            if !art {
                Some(s)
            } else if s.is_char_boundary(tail)
                && matches!(&s[tail..].to_ascii_lowercase()[..], "_f" | "_b")
            {
                Some(&s[..tail])
            } else {
                None
            }
        });
        if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg"))
            && number.is_some_and(|n| n.parse::<u32>().is_ok())
        {
            fs::remove_file(p)?;
        }
    }
    Ok(())
}

/// The note a failed [`ensure`] leaves: [`is_wanted`] takes it as done, so PLAY does not rebuild at every start;
/// SET UP and the pictures command ([`ensure`]) do try again.
fn failed() -> String {
    format!("{VERSION} failed")
}

/// What the folder's note says (empty when there is none).
fn note(cards: &Path) -> String {
    fs::read_to_string(cards.join(MARKER)).map_or_else(|_| String::new(), |m| m.trim().to_owned())
}

/// Whether `cards` is as the setting asks (`pack` as in [`ensure`]) or this version already failed to make it: false
/// means [`ensure`] has work. PLAY asks this; [`ensure`] itself retries a failure.
pub fn is_wanted(cards: &Path, pack: Option<&Path>) -> bool {
    let n = note(cards);
    n == wanted(pack) || n == failed()
}

/// `cards` as the setting asks (`pack`: the high-quality pack, else the game's own), made only when the folder was
/// made otherwise or not at all. `edition` is "1011" or "1718"; `say` hears what is done. A failure leaves the note
/// "<version> failed" (the folder may be half made: shortcut: no build-and-swap in a temp folder, when a failed
/// rebuild must keep the old pictures).
pub fn ensure(
    game: &Path,
    edition: &str,
    cards: &Path,
    pack: Option<&Path>,
    say: crate::Say,
) -> io::Result<()> {
    if note(cards) == wanted(pack) {
        return Ok(());
    }
    // the game's pictures stay while this version made them (not a failed try); the pack's go, and come back if wanted
    let made = note(cards).starts_with(&format!("{VERSION} ")) && note(cards) != failed();
    let r = build(game, edition, cards, pack, made, say);
    if r.is_err() {
        let _ = fs::create_dir_all(cards);
        let _ = fs::write(cards.join(MARKER), failed());
    }
    r
}

fn build(
    game: &Path,
    edition: &str,
    cards: &Path,
    pack: Option<&Path>,
    made: bool,
    say: crate::Say,
) -> io::Result<()> {
    let marker: PathBuf = cards.join(MARKER);
    let want = wanted(pack);
    let _ = fs::remove_file(&marker); // half made stays "not made"
    let _ = fs::remove_file(cards.join(super::hq::FILE));
    remove_jpgs(cards, false)?; // the pack's scans; the album art (`_f`, `_b`) is the game's and stays
    if !made {
        remove_jpgs(cards, true)?; // a full rebuild: no stale art may outrank the new pictures
        say("  ", &Line::key("setup.pic_making"));
        let m = from_game(game, edition, cards)?;
        say(
            "  ",
            &Line::key("setup.pic_made")
                .arg("faces", m.faces)
                .arg("fronts", m.fronts)
                .arg("failed", m.fails.len()),
        );
        for f in m.fails.iter().take(5) {
            say("    ", &Line::raw(f));
        }
        if m.faces == 0 {
            return Err(bad("no card picture could be made from the game"));
        }
        // more than 1 in 20 album fronts lost (disk full, a scanner holding files): no marker, the next start retries
        if m.art_fails * 20 > m.fronts + m.art_fails {
            return Err(bad(format!("{} album fronts failed", m.art_fails)));
        }
    }
    if let Some(p) = pack {
        say(
            "  ",
            &Line::key("setup.pic_adding_hq").arg("pack", p.display()),
        );
        let (n, fails) = from_pack(p, edition, cards)?;
        say(
            "  ",
            &Line::key("setup.pic_hq_made")
                .arg("count", n)
                .arg("failed", fails.len()),
        );
        for f in fails.iter().take(5) {
            say("    ", &Line::raw(f));
        }
    }
    fs::write(&marker, want)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// codes MSB first at `width` bits, after the stream's magic
    fn pack_codes(codes: &[(u32, u32)]) -> Vec<u8> {
        let (mut out, mut acc, mut n) = (b"YS\x02\x00".to_vec(), 0u64, 0);
        for &(c, w) in codes {
            acc = (acc << w) | u64::from(c);
            n += w;
            while n >= 8 {
                n -= 8;
                out.push((acc >> n) as u8);
            }
        }
        if n > 0 {
            out.push((acc << (8 - n)) as u8);
        }
        out
    }

    #[test]
    fn lzw_reads_a_known_dictionary_entry_and_the_kwkwk_case() {
        // A, B, 256 ("AB"), 258 (not yet made: "AB" + "A")
        let s = pack_codes(&[(65, 9), (66, 9), (256, 9), (258, 9)]);
        assert_eq!(lzw(&s, 7).unwrap(), b"ABABABA");
    }

    /// A DXT1 DDS of `w`x`h` whose 4x4 block (bx, by) is the solid RGB565 colour `colour(bx, by)`.
    fn dxt1(w: u32, h: u32, colour: impl Fn(u32, u32) -> u16) -> Vec<u8> {
        let mut dds = b"DDS ".to_vec();
        let mut hdr = [0u32; 31];
        (hdr[0], hdr[1], hdr[2], hdr[3], hdr[4]) = (124, 0x1007, h, w, w * h / 2);
        (hdr[18], hdr[19]) = (32, 4); // pixel format: size, DDPF_FOURCC
        for v in hdr {
            dds.extend(v.to_le_bytes());
        }
        dds[84..88].copy_from_slice(b"DXT1");
        for by in 0..h / 4 {
            for bx in 0..w / 4 {
                let c = colour(bx, by);
                dds.extend(c.to_le_bytes());
                dds.extend(c.to_le_bytes());
                dds.extend([0u8; 4]);
            }
        }
        dds
    }

    const RED: u16 = 0xF800;
    const GREEN: u16 = 0x07E0;
    const BLUE: u16 = 0x001F;

    /// A made-up 2017-18 game folder: one record, its stored 128x128 DXT1 picture (left half red, right half blue)
    /// and its 256x512 album front (green down to row 359, red under it: the edge smear the crop must drop).
    fn fake_game(dir: &Path) {
        let mut rec = vec![0u8; 0x620];
        rec[0x288..0x28A].copy_from_slice(&7049u16.to_le_bytes());
        rec[0x564..0x564 + 7].copy_from_slice(b"Pic_One");
        fs::create_dir_all(dir.join("prog_data/player_data")).unwrap();
        fs::write(dir.join("prog_data/player_data/player_data.bin"), rec).unwrap();

        let dds = dxt1(128, 128, |x, _| if x < 16 { RED } else { BLUE });
        let front = dxt1(256, 512, |_, y| if y < 90 { GREEN } else { RED });
        let back = dxt1(256, 512, |_, y| if y < 90 { BLUE } else { RED });
        let mut xaf = vec![0u8; 0x100];
        xaf[..4].copy_from_slice(b"xaf0");
        xaf[8..12].copy_from_slice(&0x100u32.to_le_bytes()); // sector
        xaf[12..16].copy_from_slice(&5u32.to_le_bytes());
        let mut entry = |name: &str, file: bool, parent: i32, size: usize, sector: u32| {
            let mut e = vec![0u8; 0xB0];
            e[..name.len()].copy_from_slice(name.as_bytes());
            e[0x80] = u8::from(file);
            e[0x84..0x88].copy_from_slice(&parent.to_le_bytes());
            e[0x94..0x98].copy_from_slice(&(size as u32).to_le_bytes());
            e[0x98..0x9C].copy_from_slice(&(size as u32).to_le_bytes());
            e[0xA0..0xA4].copy_from_slice(&sector.to_le_bytes());
            xaf.extend(e);
        };
        let front_at = 5 + dds.len().div_ceil(0x100) as u32;
        let back_at = front_at + front.len().div_ceil(0x100) as u32;
        entry("data", false, -1, 0, 0);
        entry("player_card", false, 0, 0, 0);
        entry("PIC_ONE.dds", true, 1, dds.len(), 5);
        entry("PIC_ONE_F.dds", true, 1, front.len(), front_at);
        entry("PIC_ONE_B.dds", true, 1, back.len(), back_at);
        xaf.resize(0x500, 0);
        xaf.extend(&dds);
        xaf.resize(front_at as usize * 0x100, 0);
        xaf.extend(&front);
        xaf.resize(back_at as usize * 0x100, 0);
        xaf.extend(&back);
        fs::create_dir_all(dir.join("data")).unwrap();
        fs::write(dir.join("data/wccf_data.xaf"), xaf).unwrap();
    }

    #[test]
    fn a_game_picture_comes_out_as_its_face() {
        let d = tempfile::tempdir().unwrap();
        fake_game(d.path());
        let out = d.path().join("cards");
        let said = std::cell::RefCell::new(Vec::new());
        ensure(d.path(), "1718", &out, None, &|_, l| {
            said.borrow_mut().push(l.clone())
        })
        .unwrap();
        let img = image::open(out.join("7049.png")).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), FACE);
        assert_eq!(img.get_pixel(10, 10).0, [255, 0, 0]);
        assert_eq!(img.get_pixel(80, 10).0, [0, 0, 255]);
        // the album back, cropped the same way
        let back = image::open(out.join("7049_b.jpg")).unwrap().to_rgb8();
        assert_eq!(back.dimensions(), ALBUM);
        let [r, g, b] = back.get_pixel(250, 356).0;
        assert!(r < 24 && g < 24 && b > 230, "{r} {g} {b}");
        // the album front beside it: the card's top 256x357 (no smear rows), green, as a JPEG
        let front = image::open(out.join("7049_f.jpg")).unwrap().to_rgb8();
        assert_eq!(front.dimensions(), ALBUM);
        for (x, y) in [(10, 10), (128, 200), (250, 356)] {
            let [r, g, b] = front.get_pixel(x, y).0;
            assert!(r < 24 && g > 230 && b < 24, "({x},{y}): {r} {g} {b}");
        }
        // made once: the second call finds the folder as wanted
        said.borrow_mut().clear();
        ensure(d.path(), "1718", &out, None, &|_, l| {
            said.borrow_mut().push(l.clone())
        })
        .unwrap();
        assert!(said.borrow().is_empty());
    }

    #[test]
    fn a_folder_made_by_the_last_version_is_made_again_with_the_album_art() {
        let d = tempfile::tempdir().unwrap();
        fake_game(d.path());
        let out = d.path().join("cards");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join(MARKER), "1 game").unwrap(); // version 1: faces only
        fs::write(out.join("7049.png"), b"old").unwrap();
        ensure(d.path(), "1718", &out, None, &|_, _| {}).unwrap();
        assert!(out.join("7049_f.jpg").is_file());
        assert_eq!(image::open(out.join("7049.png")).unwrap().width(), FACE.0);
        assert_eq!(fs::read_to_string(out.join(MARKER)).unwrap(), wanted(None));
    }

    #[test]
    fn only_a_256x512_texture_is_an_album_card() {
        let c = album_card(&DynamicImage::new_rgb8(256, 512)).unwrap();
        assert_eq!((c.width(), c.height()), ALBUM);
        assert!(album_card(&DynamicImage::new_rgb8(128, 128)).is_none());
        assert!(album_card(&DynamicImage::new_rgb8(256, 256)).is_none());
    }

    #[test]
    fn the_2010_11_folder_gets_no_album_art() {
        let d = tempfile::tempdir().unwrap();
        fake_game(d.path());
        // the same archive, the 2010-11 record (0x328 bytes: number at +0, picture name at +0x2DA)
        let mut rec = vec![0u8; 0x328];
        rec[..2].copy_from_slice(&7049u16.to_le_bytes());
        rec[0x2DA..0x2DA + 7].copy_from_slice(b"Pic_One");
        fs::write(d.path().join("prog_data/player_data/player_data.bin"), rec).unwrap();
        let out = d.path().join("cards");
        let m = from_game(d.path(), "1011", &out).unwrap();
        assert_eq!((m.faces, m.fronts, m.art_fails), (1, 0, 0));
        assert!(out.join("7049.png").is_file());
        assert!(!out.join("7049_f.jpg").exists() && !out.join("7049_b.jpg").exists());
    }

    #[test]
    fn an_older_marker_is_not_current_and_ensure_makes_it_again_in_either_edition() {
        let d = tempfile::tempdir().unwrap();
        fake_game(d.path());
        let out = d.path().join("cards");
        assert!(!is_wanted(&out, None));
        for ed in ["1718", "1011"] {
            if ed == "1011" {
                let mut rec = vec![0u8; 0x328];
                rec[..2].copy_from_slice(&7049u16.to_le_bytes());
                rec[0x2DA..0x2DA + 7].copy_from_slice(b"Pic_One");
                fs::write(d.path().join("prog_data/player_data/player_data.bin"), rec).unwrap();
            }
            fs::create_dir_all(&out).unwrap();
            fs::write(out.join(MARKER), "1 game").unwrap();
            assert!(!is_wanted(&out, None), "{ed}");
            ensure(d.path(), ed, &out, None, &|_, _| {}).unwrap();
            assert!(
                is_wanted(&out, None) && out.join("7049.png").is_file(),
                "{ed}"
            );
            assert!(
                !is_wanted(&out, Some(Path::new("pack.zip"))),
                "{ed}: a pack not made yet"
            );
        }
    }

    #[test]
    fn lost_album_pictures_leave_a_failed_note_and_no_temp_file_and_a_rebuild_drops_stale_art() {
        let d = tempfile::tempdir().unwrap();
        fake_game(d.path());
        let out = d.path().join("cards");
        fs::create_dir_all(out.join("7049_f.jpg.tmp")).unwrap(); // the temp file cannot be made
        assert!(ensure(d.path(), "1718", &out, None, &|_, _| {}).is_err());
        // the failure is noted: PLAY leaves it be, SET UP below tries again from scratch
        assert_eq!(fs::read_to_string(out.join(MARKER)).unwrap(), failed());
        assert!(is_wanted(&out, None) && is_wanted(&out, Some(Path::new("p.zip"))));
        assert!(!out.join("7049_b.jpg.tmp").exists());
        // SET UP retries; stale art of a card the game no longer has goes with the rebuild
        fs::remove_dir(out.join("7049_f.jpg.tmp")).unwrap();
        fs::write(out.join("1_f.JPG"), b"stale").unwrap();
        fs::write(out.join("2_B.JPG"), b"stale").unwrap();
        ensure(d.path(), "1718", &out, None, &|_, _| {}).unwrap();
        assert!(out.join("7049_f.jpg").is_file());
        assert!(!out.join("1_f.JPG").exists() && !out.join("2_B.JPG").exists());
        assert!(out.join(MARKER).is_file());
    }

    #[test]
    fn a_lost_back_is_reported_but_never_stops_the_setup() {
        let d = tempfile::tempdir().unwrap();
        fake_game(d.path());
        let out = d.path().join("cards");
        fs::create_dir_all(out.join("7049_b.jpg.tmp")).unwrap();
        ensure(d.path(), "1718", &out, None, &|_, _| {}).unwrap();
        assert!(out.join(MARKER).is_file() && out.join("7049_f.jpg").is_file());
        fs::remove_file(out.join(MARKER)).unwrap();
        let m = from_game(d.path(), "1718", &out).unwrap();
        assert_eq!((m.fronts, m.art_fails, m.back_fails), (1, 0, 1));
    }

    #[test]
    fn the_pack_adds_its_scans_and_goes_again() {
        let d = tempfile::tempdir().unwrap();
        fake_game(d.path());
        // a pack: cards.db linking two scans to card 7049 (the name+stats one wins) and a scan for 2010-11 only
        let db = d.path().join("cards.db");
        let con = rusqlite::Connection::open(&db).unwrap();
        con.execute_batch(
            "CREATE TABLE cards (ver INTEGER, card TEXT, front TEXT, position TEXT, off INTEGER, def INTEGER, tec INTEGER,
                                 pow INTEGER, spe INTEGER, sta INTEGER, tot INTEGER);
             CREATE TABLE game_link (ver INTEGER, card TEXT, game TEXT, game_card_no INTEGER, method TEXT);
             INSERT INTO cards VALUES (28, '240', 'images/a.jpg', 'FW', 1, 2, 3, 4, 5, 6, 21),
                 (29, 'A01', 'images/b.jpg', 'MF', 1, 1, 1, 1, 1, 1, 6), (13, '1', 'images/c.jpg', 'GK', 1, 1, 1, 1, 1, 1, 6);
             INSERT INTO game_link VALUES (29, 'A01', '1718', 7049, 'name+season'), (28, '240', '1718', 7049, 'name+stats'),
                                          (13, '1', '1011', 1, 'name+stats');",
        )
        .unwrap();
        drop(con);
        let pack = d.path().join("pack.zip");
        let mut z = zip::ZipWriter::new(File::create(&pack).unwrap());
        for (name, bytes) in [
            ("cards.db", fs::read(&db).unwrap()),
            ("images/a.jpg", b"A".to_vec()),
            ("images/b.jpg", b"B".to_vec()),
            ("images/c.jpg", b"C".to_vec()),
        ] {
            z.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut z, &bytes).unwrap();
        }
        z.finish().unwrap();
        let out = d.path().join("cards");
        ensure(d.path(), "1718", &out, Some(&pack), &|_, _| {}).unwrap();
        assert_eq!(fs::read(out.join("7049.jpg")).unwrap(), b"A");
        assert!(out.join("7049.png").is_file() && !out.join("1.jpg").exists());
        assert!(out.join("7049_f.jpg").is_file()); // beside the pack's scan: the board takes it first
        assert!(out.join(super::super::hq::FILE).is_file());
        // back to the game's own: the scans go, the game's pictures stay
        ensure(d.path(), "1718", &out, None, &|_, _| {}).unwrap();
        assert!(!out.join("7049.jpg").exists() && out.join("7049.png").is_file());
        assert!(out.join("7049_f.jpg").is_file() && out.join("7049_b.jpg").is_file()); // the game's own stay
        assert!(!out.join(super::super::hq::FILE).exists());
    }

    #[test]
    fn the_prebuilt_pack_adds_this_editions_scans() {
        let d = tempfile::tempdir().unwrap();
        fake_game(d.path());
        let pack = d.path().join("forza-wccf-card-pictures-hq.zip");
        let mut z = zip::ZipWriter::new(File::create(&pack).unwrap());
        // one scan for card 7049 (2017-18) and card 1 (2010-11), stored once
        for (name, bytes) in [
            ("README.txt", b"r".to_vec()),
            ("pictures/ab.jpg", b"A".to_vec()),
            (
                "2017-2018/cards.tsv",
                b"7049\tab.jpg\n7050\t../x.jpg\n".to_vec(),
            ),
            (
                "2017-2018/hq_stats.tsv",
                b"7049\tFW\t1\t1\t1\t1\t1\t1\t6\n".to_vec(),
            ),
            ("2010-2011/cards.tsv", b"1\tab.jpg\n".to_vec()),
        ] {
            z.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut z, &bytes).unwrap();
        }
        z.finish().unwrap();
        let out = d.path().join("cards");
        ensure(d.path(), "1718", &out, Some(&pack), &|_, _| {}).unwrap();
        assert_eq!(fs::read(out.join("7049.jpg")).unwrap(), b"A");
        assert!(out.join(super::super::hq::FILE).is_file());
        assert!(!out.join("1.jpg").exists() && !out.join("7050.jpg").exists()); // the other edition; a path in the list
        // 2010-11 takes the same scan; its missing stats file is a failure, not a stop
        let (n, fails) = from_pack(&pack, "1011", &d.path().join("x")).unwrap();
        assert_eq!((n, fails.len()), (1, 1));
        // a ZIP with neither cards.db nor cards.tsv is refused
        let empty = d.path().join("empty.zip");
        zip::ZipWriter::new(File::create(&empty).unwrap())
            .finish()
            .unwrap();
        assert!(from_pack(&empty, "1718", &d.path().join("y")).is_err());
    }

    /// The player's own game and, if given, the pack: `FORZA_PICTURES_GAME` (the game folder), `FORZA_PICTURES_OUT`
    /// (a scratch folder), `FORZA_PICTURES_PACK` and `FORZA_PICTURES_EDITION` (1011 or 1718).
    #[test]
    #[ignore = "needs the player's own files"]
    fn the_players_own_files() {
        let var = |n| std::env::var_os(n).map(PathBuf::from);
        let out = var("FORZA_PICTURES_OUT").unwrap();
        let t = std::time::Instant::now();
        let ed = std::env::var("FORZA_PICTURES_EDITION").unwrap();
        let m = from_game(&var("FORZA_PICTURES_GAME").unwrap(), &ed, &out).unwrap();
        println!(
            "game: {} pictures, {} album fronts, {} failed {:?}, {:?}",
            m.faces,
            m.fronts,
            m.fails.len(),
            &m.fails[..m.fails.len().min(5)],
            t.elapsed()
        );
        assert!(m.faces > 3000 && m.fails.len() < 50);
        // the album art is in the 2017-18 archive; none was found if this fails
        assert!(ed != "1718" || m.fronts > 2000, "{} album fronts", m.fronts);
        if let Some(pack) = var("FORZA_PICTURES_PACK") {
            let (n, fails) = from_pack(&pack, &ed, &out).unwrap();
            println!(
                "pack: {n} pictures, {} failed {:?}, {:?}",
                fails.len(),
                &fails[..fails.len().min(5)],
                t.elapsed()
            );
        }
    }
}
