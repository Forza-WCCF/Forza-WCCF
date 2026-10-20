//! The card pictures the panel shows (`<cards>\<number>.png`), made on the player's PC: from the player's own game
//! (each card's 128x128 DXT1 picture in `data/wccf_data.xaf`; the card face is its left 88 columns, the rest edge
//! smear) or, as an option, from the high-quality pack (a ZIP of card scans with `cards.db` linking them to both
//! games' card numbers). The game's pictures (`<number>.png`) are always there, so a card the pack lacks keeps its
//! own; the pack's (`<number>.jpg`) come beside them and the board takes them first.
//! Both editions keep the same archive format, and each `player_data.bin` record names its card number and picture.

use image::{DynamicImage, ImageFormat};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The face: the left 88 columns of the game's 128x128 picture (what the original overlay showed).
pub const FACE: (u32, u32) = (88, 128);
/// The folder's note of what made it; another answer from [`wanted`] makes it again.
const MARKER: &str = "source.txt";
const VERSION: u32 = 1;

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

/// Every card's face from the game in `game` (the folder with `prog_data` and `data`) into `out` as
/// `<number>.png`, 88x128 -> (written, failures). One bad picture never stops the rest.
pub fn from_game(game: &Path, edition: &str, out: &Path) -> io::Result<(usize, Vec<String>)> {
    let players = fs::read(game.join("prog_data/player_data/player_data.bin"))?;
    let mut xaf = File::open(game.join("data/wccf_data.xaf"))?;
    let file_len = xaf.metadata()?.len();
    let toc = player_cards(&mut xaf)?;
    fs::create_dir_all(out)?;
    let (mut done, mut fails) = (0, Vec::new());
    for (no, name) in cards(&players, edition)? {
        let mut one = || -> io::Result<()> {
            let e = toc.get(&name).ok_or_else(|| bad("not in the archive"))?;
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
            let img = image::load_from_memory_with_format(&dds, ImageFormat::Dds)
                .map_err(io::Error::other)?;
            let (w, h) = (FACE.0.min(img.width()), FACE.1.min(img.height()));
            save(&img.crop_imm(0, 0, w, h), &out.join(format!("{no}.png")))
        };
        match one() {
            Ok(()) => done += 1,
            Err(e) => fails.push(format!("card {no} ({name}): {e}")),
        }
    }
    Ok((done, fails))
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

/// `cards` as the setting asks (`pack`: the high-quality pack, else the game's own), made only when the folder was
/// made otherwise or not at all. `edition` is "1011" or "1718"; `say` hears what is done.
pub fn ensure(
    game: &Path,
    edition: &str,
    cards: &Path,
    pack: Option<&Path>,
    say: &dyn Fn(&str),
) -> io::Result<()> {
    let marker: PathBuf = cards.join(MARKER);
    let want = wanted(pack);
    if fs::read_to_string(&marker).is_ok_and(|m| m.trim() == want) {
        return Ok(());
    }
    // the game's pictures stay while this version made them; the pack's go, and come back if wanted
    let made = fs::read_to_string(&marker).is_ok_and(|m| m.starts_with(&format!("{VERSION} ")));
    let _ = fs::remove_file(&marker); // half made stays "not made"
    let _ = fs::remove_file(cards.join(super::hq::FILE));
    for f in fs::read_dir(cards).into_iter().flatten().flatten() {
        if f.path().extension().is_some_and(|e| e == "jpg") {
            fs::remove_file(f.path())?;
        }
    }
    if !made {
        say("  Making card pictures from your game files...");
        let (n, fails) = from_game(game, edition, cards)?;
        say(&format!(
            "  {n} card pictures from the game, {} failed",
            fails.len()
        ));
        for f in fails.iter().take(5) {
            say(&format!("    {f}"));
        }
        if n == 0 {
            return Err(bad("no card picture could be made from the game"));
        }
    }
    if let Some(p) = pack {
        say(&format!(
            "  Adding high-quality pictures from {}...",
            p.display()
        ));
        let (n, fails) = from_pack(p, edition, cards)?;
        say(&format!(
            "  {n} high-quality card pictures, {} failed",
            fails.len()
        ));
        for f in fails.iter().take(5) {
            say(&format!("    {f}"));
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

    /// A made-up 2017-18 game folder: one record, one stored 128x128 DXT1 picture (left half red, right half blue).
    fn fake_game(dir: &Path) {
        let mut rec = vec![0u8; 0x620];
        rec[0x288..0x28A].copy_from_slice(&7049u16.to_le_bytes());
        rec[0x564..0x564 + 7].copy_from_slice(b"Pic_One");
        fs::create_dir_all(dir.join("prog_data/player_data")).unwrap();
        fs::write(dir.join("prog_data/player_data/player_data.bin"), rec).unwrap();

        let mut dds = b"DDS ".to_vec();
        let mut h = [0u32; 31];
        (h[0], h[1], h[2], h[3], h[4]) = (124, 0x1007, 128, 128, 128 * 128 / 2);
        (h[18], h[19]) = (32, 4); // pixel format: size, DDPF_FOURCC
        for v in h {
            dds.extend(v.to_le_bytes());
        }
        dds[84..88].copy_from_slice(b"DXT1");
        for _y in 0..32 {
            for x in 0..32 {
                let c: u16 = if x < 16 { 0xF800 } else { 0x001F };
                dds.extend(c.to_le_bytes());
                dds.extend(c.to_le_bytes());
                dds.extend([0u8; 4]);
            }
        }
        let mut xaf = vec![0u8; 0x100];
        xaf[..4].copy_from_slice(b"xaf0");
        xaf[8..12].copy_from_slice(&0x100u32.to_le_bytes()); // sector
        xaf[12..16].copy_from_slice(&3u32.to_le_bytes());
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
        entry("data", false, -1, 0, 0);
        entry("player_card", false, 0, 0, 0);
        entry("PIC_ONE.dds", true, 1, dds.len(), 4);
        xaf.resize(0x400, 0);
        xaf.extend(&dds);
        fs::create_dir_all(dir.join("data")).unwrap();
        fs::write(dir.join("data/wccf_data.xaf"), xaf).unwrap();
    }

    #[test]
    fn a_game_picture_comes_out_as_its_face() {
        let d = tempfile::tempdir().unwrap();
        fake_game(d.path());
        let out = d.path().join("cards");
        let said = std::cell::RefCell::new(Vec::new());
        ensure(d.path(), "1718", &out, None, &|l| {
            said.borrow_mut().push(l.to_owned())
        })
        .unwrap();
        let img = image::open(out.join("7049.png")).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), FACE);
        assert_eq!(img.get_pixel(10, 10).0, [255, 0, 0]);
        assert_eq!(img.get_pixel(80, 10).0, [0, 0, 255]);
        // made once: the second call finds the folder as wanted
        said.borrow_mut().clear();
        ensure(d.path(), "1718", &out, None, &|l| {
            said.borrow_mut().push(l.to_owned())
        })
        .unwrap();
        assert!(said.borrow().is_empty());
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
        ensure(d.path(), "1718", &out, Some(&pack), &|_| {}).unwrap();
        assert_eq!(fs::read(out.join("7049.jpg")).unwrap(), b"A");
        assert!(out.join("7049.png").is_file() && !out.join("1.jpg").exists());
        assert!(out.join(super::super::hq::FILE).is_file());
        // back to the game's own: the scans go, the game's pictures stay
        ensure(d.path(), "1718", &out, None, &|_| {}).unwrap();
        assert!(!out.join("7049.jpg").exists() && out.join("7049.png").is_file());
        assert!(!out.join(super::super::hq::FILE).exists());
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
        let (n, fails) = from_game(&var("FORZA_PICTURES_GAME").unwrap(), &ed, &out).unwrap();
        println!(
            "game: {n} pictures, {} failed {:?}, {:?}",
            fails.len(),
            &fails[..fails.len().min(5)],
            t.elapsed()
        );
        assert!(n > 3000 && fails.len() < 50);
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
