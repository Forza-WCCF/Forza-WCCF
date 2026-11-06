//! The 2017-18 program text in English or Italian: texts compiled into client_Release.exe (menu labels, headers,
//! dialogs the game draws with its font) replaced in the game's memory, never in the file (the Launcher checks the
//! guarded programs' hashes). The sheet is the kit's `english\exe_text.tsv` or `italian\exe_text.tsv`, picked by the
//! language english.py put in place (`forza::kit::Kit::game_language`; the original Japanese: nothing replaced).
//!
//! Each row: the text's offset from the program's base (rva), the original bytes' length and SHA-256, the room (bytes
//! the text may take, NUL included: the original's own slot up to its 4-byte padding) and our text. A row is written
//! only when the bytes there hash to the original and the rest of its room is zeros; the whole room is written, our
//! text then zeros. Runs from the panel's start (seat 1 and the projector), before the game builds its menus: a text
//! the game copied before that keeps its Japanese until the game builds it again.
//!
//! The club and nationality names (`forza::cards::codes`, one table for the card catalogue and the game) are too
//! long for their slots, so the game's two pointer tables to them are pointed at our names instead: an entry is
//! changed only when it still points at the original name.

use forza::kit::Kit;
use sha2::{Digest, Sha256};
use std::ffi::c_void;
use std::path::Path;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};

#[derive(Debug, PartialEq)]
pub struct Row {
    pub rva: usize,
    pub len: usize,
    pub sha: [u8; 32],
    pub room: usize,
    pub text: Vec<u8>,
}

/// The sheet's rows (`#` lines and the header skipped); a row that does not parse or does not fit is an error.
pub fn parse(sheet: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for (n, line) in sheet.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') || line.starts_with("rva\t") {
            continue;
        }
        let bad = || format!("line {}: not rva, length, sha256, room, text", n + 1);
        let f: Vec<&str> = line.split('\t').collect();
        let [rva, len, sha, room, text, ..] = f[..] else {
            return Err(bad());
        };
        let rva = usize::from_str_radix(rva.trim_start_matches("0x"), 16).map_err(|_| bad())?;
        let sha: Vec<u8> = (0..sha.len())
            .step_by(2)
            .map(|i| {
                sha.get(i..i + 2)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
            })
            .collect::<Option<_>>()
            .ok_or_else(bad)?;
        let row = Row {
            rva,
            len: len.parse().map_err(|_| bad())?,
            sha: sha.try_into().map_err(|_| bad())?,
            room: room.parse().map_err(|_| bad())?,
            text: text.replace("\\n", "\n").into_bytes(),
        };
        if !row.text.is_ascii() || row.text.len() >= row.room || row.len >= row.room {
            return Err(format!(
                "line {}: the text is not ASCII or does not fit its room",
                n + 1
            ));
        }
        rows.push(row);
    }
    Ok(rows)
}

/// The room's bytes hold the original: its hash, then zeros to the room's end.
pub fn is_original(room: &[u8], row: &Row) -> bool {
    room.len() == row.room
        && Sha256::digest(&room[..row.len]).as_slice() == row.sha
        && room[row.len..].iter().all(|&b| b == 0)
}

/// What the room holds once replaced: our text, then zeros.
pub fn replaced(row: &Row) -> Vec<u8> {
    let mut b = row.text.clone();
    b.resize(row.room, 0);
    b
}

/// The program's image size from its PE header in memory (`SizeOfImage`).
fn image_size(base: usize) -> usize {
    // SAFETY: the program's own headers, mapped for as long as it runs
    unsafe {
        let pe = base + *((base + 0x3C) as *const u32) as usize;
        *((pe + 0x50) as *const u32) as usize
    }
}

/// Replace the texts of the current game language in the program's memory; one log line.
pub fn install(e1718: bool, data: &Path, log: fn(&str)) {
    if !e1718 {
        return; // the 2010-11 kit changes its program text in the files (english.py)
    }
    let kit = Kit::at(data.parent().unwrap_or(data));
    let (lang, folder) = match kit.game_language() {
        "en" => ("en", "english"),
        "it" => ("it", "italian"),
        _ => return log("exe text: the original Japanese - nothing replaced"),
    };
    let path = kit.dir().join(folder).join("exe_text.tsv");
    let rows = match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| parse(&t))
    {
        Ok(r) => r,
        Err(e) => {
            return log(&format!(
                "exe text: {} not read ({e}) - nothing replaced",
                path.display()
            ));
        }
    };
    // SAFETY: GetModuleHandleW(NULL) only reads the process's own module list
    let base = unsafe { GetModuleHandleW(None) }.map_or(0, |h| h.0 as usize);
    let size = if base == 0 { 0 } else { image_size(base) };
    let mut done = 0;
    for row in &rows {
        if row.rva + row.room > size {
            continue;
        }
        let at = base + row.rva;
        // SAFETY: inside the program's image (checked above), which stays mapped; .rdata is readable
        let now = unsafe { std::slice::from_raw_parts(at as *const u8, row.room) };
        if !is_original(now, row) {
            continue;
        }
        let new = replaced(row);
        let mut old = PAGE_PROTECTION_FLAGS(0);
        // SAFETY: the original bytes were checked; the pages are writable for the copy only
        unsafe {
            if VirtualProtect(at as *const c_void, row.room, PAGE_READWRITE, &mut old).is_err() {
                continue;
            }
            std::ptr::copy_nonoverlapping(new.as_ptr(), at as *mut u8, row.room);
            let _ = VirtualProtect(at as *const c_void, row.room, old, &mut old);
        }
        done += 1;
    }
    log(&format!(
        "exe text: {done} of {} replaced ({lang}); {} club and nationality names",
        rows.len(),
        names(base, size, lang == "it")
    ));
    if crate::patches::is_client(&crate::patches::exe_name()) {
        log(&format!(
            "exe text: Dati record by area, {}",
            key_rows(DATI_ROWS, base)
        ));
        log(&format!(
            "exe text: Challenge Tour conquests, {}",
            key_rows(CTD_ROWS, base)
        ));
    }
}

/// Two screens fill their rows from string-table keys put on the stack, one `mov dword [esp+disp], imm32` each
/// (the key's VA last). The first four are MAKE_country keys, data the game must keep in Japanese (`forza::text`
/// DATA_FAMILIES), so those four instructions get the INF_COACH_country keys instead: the same countries,
/// translated. Each row: the instruction's VA and its original bytes at the base 0x40_0000.
const OUR_KEYS: [&str; 4] = [
    "INF_COACH_country_0000", // MAKE_country_014 England
    "INF_COACH_country_0001", // MAKE_country_023 Spain
    "INF_COACH_country_0002", // MAKE_country_015 Italy
    "INF_COACH_country_0003", // MAKE_country_028 Germany
];

/// Dati, record by area (sub_715A40): `C7 84 24 disp32 imm32`.
const DATI_ROWS: [(usize, [u8; 11]); 4] = [
    (
        0x0071_64F2,
        [0xC7, 0x84, 0x24, 0x88, 0, 0, 0, 0xFC, 0xF2, 0x39, 0x01],
    ),
    (
        0x0071_64FD,
        [0xC7, 0x84, 0x24, 0x8C, 0, 0, 0, 0x48, 0xF2, 0x39, 0x01],
    ),
    (
        0x0071_6508,
        [0xC7, 0x84, 0x24, 0x90, 0, 0, 0, 0xE8, 0xF2, 0x39, 0x01],
    ),
    (
        0x0071_6513,
        [0xC7, 0x84, 0x24, 0x94, 0, 0, 0, 0xE4, 0xF1, 0x39, 0x01],
    ),
];

/// Challenge Tour data, conquest status, rows CTD_nation1..4 (sub_714820): `C7 44 24 disp8 imm32`.
const CTD_ROWS: [(usize, [u8; 8]); 4] = [
    (
        0x0071_4866,
        [0xC7, 0x44, 0x24, 0x30, 0xFC, 0xF2, 0x39, 0x01],
    ),
    (
        0x0071_486E,
        [0xC7, 0x44, 0x24, 0x34, 0x48, 0xF2, 0x39, 0x01],
    ),
    (
        0x0071_4876,
        [0xC7, 0x44, 0x24, 0x38, 0xE8, 0xF2, 0x39, 0x01],
    ),
    (
        0x0071_487E,
        [0xC7, 0x44, 0x24, 0x3C, 0xE4, 0xF1, 0x39, 0x01],
    ),
];

/// Each row's instruction at `base`: its address and the key's VA relocated (reloc type 3).
fn originals<const N: usize>(rows: [(usize, [u8; N]); 4], base: usize) -> [(usize, [u8; N]); 4] {
    rows.map(|(va, mut b)| {
        let key = u32::from_le_bytes(b[N - 4..].try_into().unwrap()) as usize;
        let key = key.wrapping_sub(0x40_0000).wrapping_add(base) as u32;
        b[N - 4..].copy_from_slice(&key.to_le_bytes());
        (base + va - 0x40_0000, b)
    })
}

/// All four instructions are the original's (`read`: the bytes at an address, `None` where unreadable).
fn found<const N: usize>(
    rows: [(usize, [u8; N]); 4],
    base: usize,
    read: impl Fn(usize) -> Option<[u8; N]>,
) -> bool {
    originals(rows, base)
        .iter()
        .all(|(at, want)| read(*at) == Some(*want))
}

/// Point the four country rows at our keys; the log's words.
fn key_rows<const N: usize>(rows: [(usize, [u8; N]); 4], base: usize) -> String {
    if base == 0 || !found(rows, base, crate::patches::read::<N>) {
        return "not the original code, left as is".into();
    }
    let mut done = 0;
    for ((at, _), key) in originals(rows, base).iter().zip(OUR_KEYS) {
        // the key is copied into a std::string on each use; ours stays for as long as the game runs
        let ours: &'static [u8] = Box::leak(format!("{key}\0").into_bytes().into_boxed_slice());
        if crate::patches::patch_code(imm32::<N>(*at), &(ours.as_ptr() as u32).to_le_bytes()) {
            done += 1;
        }
    }
    format!("{done} of 4 country rows on the translated keys")
}

/// Where the key goes: the instruction's imm32, its last four bytes (dispatch imm32 at `+7`, `C7 44 24 disp8 imm32`
/// at `+4`).
fn imm32<const N: usize>(at: usize) -> usize {
    at + N - 4
}

/// Point the club and nationality name tables at our names; how many entries changed.
fn names(base: usize, size: usize, italian: bool) -> usize {
    use forza::cards::codes::{NAME_TABLES_1718, game_names_1718};
    let names = game_names_1718(italian);
    let image = base..base + size;
    let mut done = 0;
    for va in NAME_TABLES_1718 {
        let table = base + (va as usize - 0x40_0000);
        if base == 0 || table + 4 * names.len() > image.end {
            continue;
        }
        for (i, n) in names.iter().enumerate() {
            let Some((jp, shown)) = n else { continue };
            let slot = table + 4 * i;
            // SAFETY: inside the program's image (checked above), which stays mapped; .data is readable
            let p = unsafe { *(slot as *const usize) };
            if !image.contains(&p) || p + jp.len() >= image.end {
                continue;
            }
            // SAFETY: inside the image (checked above)
            let now = unsafe { std::slice::from_raw_parts(p as *const u8, jp.len() + 1) };
            if now[..jp.len()] != jp[..] || now[jp.len()] != 0 {
                continue;
            }
            // ours for as long as the game runs; ASCII without an end byte (forza's test)
            let ours: &'static [u8] =
                Box::leak(format!("{shown}\0").into_bytes().into_boxed_slice());
            let mut old = PAGE_PROTECTION_FLAGS(0);
            // SAFETY: the slot held the original's pointer (checked); writable for the store only
            unsafe {
                if VirtualProtect(slot as *const c_void, 4, PAGE_READWRITE, &mut old).is_err() {
                    continue;
                }
                *(slot as *mut usize) = ours.as_ptr() as usize;
                let _ = VirtualProtect(slot as *const c_void, 4, old, &mut old);
            }
            done += 1;
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped sheets: every row parses and fits its room (`parse`), both languages list the same rows, and no
    /// two rooms overlap.
    #[test]
    fn shipped_sheets_fit() {
        let en = parse(include_str!(
            "../../../editions/2017-18/english/exe_text.tsv"
        ))
        .unwrap();
        let it = parse(include_str!(
            "../../../editions/2017-18/italian/exe_text.tsv"
        ))
        .unwrap();
        let key = |r: &Row| (r.rva, r.len, r.sha, r.room);
        assert_eq!(
            en.iter().map(key).collect::<Vec<_>>(),
            it.iter().map(key).collect::<Vec<_>>()
        );
        // rows that would break the game (city readings, lookup keys); see the file for why
        let protected = include_str!("../../../editions/2017-18/exe_text_protected.tsv");
        for l in protected
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let rva =
                usize::from_str_radix(l[2..l.find('\t').unwrap()].trim_start_matches('0'), 16)
                    .unwrap();
            assert!(
                en.iter().all(|r| r.rva != rva),
                "protected rva {rva:#X} is in exe_text.tsv"
            );
        }
        let mut rooms: Vec<_> = en.iter().map(|r| (r.rva, r.rva + r.room)).collect();
        rooms.sort();
        for w in rooms.windows(2) {
            assert!(w[0].1 <= w[1].0, "rooms overlap at {:#X}", w[1].0);
        }
        // sub_7DA590 strcmp's a record against these rows in order: two equal texts return the wrong region
        const REGION_KEYS: [usize; 14] = [
            0x0F9EE7C, 0x0F9EB44, 0x0E5C79C, 0x0F9EA20, 0x0E5C778, 0x0E5C62C, 0x0E5C74C, 0x0F9EA04,
            0x0E5C784, 0x0F9E91C, 0x0E5C47C, 0x0E5C764, 0x0E5C7A8, 0x0F9E880,
        ];
        for sheet in [&en, &it] {
            let texts: Vec<_> = sheet
                .iter()
                .filter(|r| REGION_KEYS.contains(&r.rva))
                .map(|r| &r.text)
                .collect();
            assert!(texts.len() >= 6, "region rows missing");
            let mut uniq = texts.clone();
            uniq.sort();
            uniq.dedup();
            assert_eq!(uniq.len(), texts.len(), "two region rows share a text");
        }
    }

    /// [`DATI_ROWS`] (read from the original `client_Release.exe` by hand) at the ASLR base 0x50_0000: addresses and keys 0x10_0000 higher (reloc type 3 on each key).
    const DATI_RELOCATED: [(usize, [u8; 11]); 4] = [
        (
            0x0081_64F2,
            [0xC7, 0x84, 0x24, 0x88, 0, 0, 0, 0xFC, 0xF2, 0x49, 0x01],
        ),
        (
            0x0081_64FD,
            [0xC7, 0x84, 0x24, 0x8C, 0, 0, 0, 0x48, 0xF2, 0x49, 0x01],
        ),
        (
            0x0081_6508,
            [0xC7, 0x84, 0x24, 0x90, 0, 0, 0, 0xE8, 0xF2, 0x49, 0x01],
        ),
        (
            0x0081_6513,
            [0xC7, 0x84, 0x24, 0x94, 0, 0, 0, 0xE4, 0xF1, 0x49, 0x01],
        ),
    ];

    /// [`CTD_ROWS`] (read from the original `client_Release.exe` by hand) at the ASLR base 0x50_0000: addresses and keys 0x10_0000 higher (reloc type 3 on each key).
    const CTD_RELOCATED: [(usize, [u8; 8]); 4] = [
        (
            0x0081_4866,
            [0xC7, 0x44, 0x24, 0x30, 0xFC, 0xF2, 0x49, 0x01],
        ),
        (
            0x0081_486E,
            [0xC7, 0x44, 0x24, 0x34, 0x48, 0xF2, 0x49, 0x01],
        ),
        (
            0x0081_4876,
            [0xC7, 0x44, 0x24, 0x38, 0xE8, 0xF2, 0x49, 0x01],
        ),
        (
            0x0081_487E,
            [0xC7, 0x44, 0x24, 0x3C, 0xE4, 0xF1, 0x49, 0x01],
        ),
    ];

    #[test]
    fn country_rows_need_the_original_code() {
        assert_eq!(originals(DATI_ROWS, 0x40_0000), DATI_ROWS);
        assert_eq!(originals(DATI_ROWS, 0x50_0000), DATI_RELOCATED);
        assert_eq!(originals(CTD_ROWS, 0x50_0000), CTD_RELOCATED);
        // the pinned write site: each imm32 at byte 4, the PE relocations of the original exe
        assert_eq!(
            CTD_ROWS.map(|(at, _)| imm32::<8>(at)),
            [0x0071_486A, 0x0071_4872, 0x0071_487A, 0x0071_4882]
        );
        assert_eq!(
            DATI_ROWS.map(|(at, _)| imm32::<11>(at)),
            [0x0071_64F9, 0x0071_6504, 0x0071_650F, 0x0071_651A]
        );
        // both screens push the same four MAKE_country keys, in OUR_KEYS' order
        let keys = |b: &[u8]| u32::from_le_bytes(b[b.len() - 4..].try_into().unwrap());
        assert_eq!(
            DATI_ROWS.map(|(_, b)| keys(&b)),
            CTD_ROWS.map(|(_, b)| keys(&b))
        );
        let mem = |t: &'static [(usize, [u8; 11]); 4]| {
            move |at: usize| t.iter().find(|(a, _)| *a == at).map(|(_, b)| *b)
        };
        assert!(found(DATI_ROWS, 0x40_0000, mem(&DATI_ROWS)));
        assert!(found(DATI_ROWS, 0x50_0000, mem(&DATI_RELOCATED)));
        assert!(!found(DATI_ROWS, 0x40_0000, |_| None));
        let changed = |at: usize| {
            let mut b = mem(&DATI_ROWS)(at)?;
            if at == 0x0071_6513 {
                b[10] ^= 1;
            }
            Some(b)
        };
        assert!(!found(DATI_ROWS, 0x40_0000, changed));
    }

    fn sheet_for(orig: &[u8], room: usize, text: &str) -> String {
        let sha: String = Sha256::digest(orig)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect();
        format!(
            "# comment\nrva\tlength\toriginal_sha256\troom\tenglish\twhere\n0x10\t{}\t{sha}\t{room}\t{text}\tnote\n",
            orig.len()
        )
    }

    #[test]
    fn parses_and_refuses() {
        let orig = "チーム登録".as_bytes(); // UTF-8 here, any bytes do
        let rows = parse(&sheet_for(orig, 16, "Team\\nEntry")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].rva, rows[0].len, rows[0].room), (0x10, 15, 16));
        assert_eq!(rows[0].text, b"Team\nEntry");
        assert!(parse(&sheet_for(orig, 16, "far too long for its room")).is_err());
        assert!(parse(&sheet_for(orig, 16, "Équipe")).is_err());
        assert!(parse("0x10\t3\tnothex\t4\tabc\n").is_err());
        assert!(parse("0x10\t3\n").is_err());
    }

    #[test]
    fn replaces_only_the_original() {
        let orig = b"\x83\x60\x81\x5B\x83\x80"; // 6 bytes, room 8
        let row = &parse(&sheet_for(orig, 8, "Team")).unwrap()[0];
        let mut buf = [0xAAu8; 0x20];
        buf[0x10..0x16].copy_from_slice(orig);
        buf[0x16..0x18].fill(0);
        let room = &buf[row.rva..row.rva + row.room];
        assert!(is_original(room, row));
        let new = replaced(row);
        assert_eq!(new, b"Team\0\0\0\0");
        buf[0x10..0x18].copy_from_slice(&new);
        assert!(
            !is_original(&buf[0x10..0x18], row),
            "a second pass leaves it"
        );
        assert_eq!(buf[0x18], 0xAA, "nothing past the room");
        let mut other = orig.to_vec();
        other[0] ^= 1;
        other.extend([0, 0]);
        assert!(!is_original(&other, row), "other bytes: refused");
        let mut tail = orig.to_vec();
        tail.extend([0, 7]);
        assert!(!is_original(&tail, row), "the room is not free: refused");
    }
}
