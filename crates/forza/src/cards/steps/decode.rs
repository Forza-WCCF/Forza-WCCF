//! The club card reader (port of `legacy/2010-11/code/cards/decode.py`'s reading part, and of
//! `kit_common.card_session`): the kit's card file is a 16-byte header and 256 blocks of 16 bytes; copy A = blocks
//! 8-112, copy B = blocks 113-217, each 0x68C bytes of fields (MSB-first bits, each group padded to a byte, the order of
//! [`schema`](super::schema)) and a checksum = the sum of the 419 little-endian dwords (+1 when the game wrote it).
//! Read-only: a card file is read in one go and never written here.

use super::schema::{FIELDS, GROUP_ORDER};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;
use std::time::SystemTime;

/// The kit's card file.
pub const CARD_BYTES: usize = 16 + 256 * 16;
/// A bare block dump, no header.
pub const RAW_BYTES: usize = 256 * 16;
/// One copy: payload and checksum.
pub const COPY_BYTES: usize = 0x690;
pub const PAYLOAD_BYTES: usize = 0x68C;

/// A card file read: its header (None for a bare dump) and its 256 blocks.
#[derive(Clone, Debug)]
pub struct CardFile {
    pub header: Option<[u8; 16]>,
    pub body: Vec<u8>,
}

impl CardFile {
    pub fn from_bytes(raw: &[u8]) -> Option<CardFile> {
        match raw.len() {
            CARD_BYTES => Some(CardFile {
                header: Some(raw[..16].try_into().unwrap()),
                body: raw[16..].to_vec(),
            }),
            RAW_BYTES => Some(CardFile {
                header: None,
                body: raw.to_vec(),
            }),
            _ => None,
        }
    }

    /// The header's Card Status byte and use counter (0xFFFF a new card).
    pub fn p_type(&self) -> Option<u8> {
        self.header.map(|h| h[4])
    }

    pub fn counter(&self) -> Option<u16> {
        self.header.map(|h| u16::from_le_bytes([h[5], h[6]]))
    }

    pub fn is_new(&self) -> bool {
        self.counter() == Some(0xFFFF)
    }

    pub fn block(&self, i: usize) -> &[u8] {
        &self.body[16 * i..16 * i + 16]
    }

    /// Copy "A" or, for anything else, "B" (decode.copy_of).
    pub fn copy(&self, which: Option<char>) -> &[u8] {
        let start = if which == Some('A') {
            8 * 16
        } else {
            8 * 16 + COPY_BYTES
        };
        &self.body[start..start + COPY_BYTES]
    }

    /// The file's bytes again.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = self.header.map(|h| h.to_vec()).unwrap_or_default();
        out.extend_from_slice(&self.body);
        out
    }
}

/// decode.read_card: the file, or the CardError text.
pub fn read_card(path: &Path) -> Result<CardFile, String> {
    let raw = fs::read(path).map_err(|e| format!("cannot read {} ({e})", path.display()))?;
    CardFile::from_bytes(&raw).ok_or_else(|| {
        format!(
            "{} is {} bytes; a kit card file is {CARD_BYTES} (or {RAW_BYTES} without its header)",
            path.display(),
            raw.len()
        )
    })
}

fn word(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

pub fn dword_sum(buf: &[u8]) -> u32 {
    (0..buf.len() / 4 * 4)
        .step_by(4)
        .fold(0u32, |t, i| t.wrapping_add(word(buf, i)))
}

/// One copy's checksum: good when stored == sum or sum + 1, and not all zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    pub sum: u32,
    pub stored: u32,
    pub good: bool,
    pub all_zero: bool,
}

pub fn copy_check(copy: &[u8]) -> Check {
    let sum = dword_sum(&copy[..PAYLOAD_BYTES]);
    let stored = word(copy, PAYLOAD_BYTES);
    let all_zero = sum == 0 && stored == 0;
    Check {
        sum,
        stored,
        good: (stored == sum || stored == sum.wrapping_add(1)) && !all_zero,
        all_zero,
    }
}

/// Which copy the game reads ('A', 'B'), or None and why it refuses the card (decode.game_choice).
pub fn game_choice(card: &CardFile) -> (Check, Check, Option<char>, &'static str) {
    let (a, b) = (
        copy_check(card.copy(Some('A'))),
        copy_check(card.copy(Some('B'))),
    );
    let (chosen, verdict) = if card.is_new() {
        (None, "new card: checksums are not checked")
    } else if a.all_zero || b.all_zero {
        (None, "REFUSED (error 0xA): a copy is all zeros")
    } else if a.good {
        (Some('A'), "copy A is good: the game reads copy A")
    } else if b.good {
        (
            Some('B'),
            "copy A is bad, copy B is good: the game reads copy B",
        )
    } else {
        (None, "REFUSED (error 0xA): both copies fail their checksum")
    };
    (a, b, chosen, verdict)
}

/// The detection checks the game makes that a file can show: (label, value, ok) - decode.card_checks.
pub fn card_checks(card: &CardFile) -> Vec<(&'static str, String, bool)> {
    let mut out = Vec::new();
    if let Some(p) = card.p_type() {
        out.push(("Card Status byte", format!("0x{p:02X}"), p != 0x20));
    }
    let sig = word(card.block(4), 8) & 0xF0FF_FFFF;
    out.push((
        "block 4 signature",
        format!("0x{sig:08X}"),
        sig == 0x4066_7195,
    ));
    let serial = u32::from_be_bytes(card.block(6)[12..16].try_into().unwrap());
    out.push(("block 6 serial", serial.to_string(), serial >= 10_000_000));
    if let Some(c) = card.counter() {
        out.push(("use counter (block 5)", format!("0x{c:04X}"), c & 0xFF != 0));
    }
    if !card.is_new() {
        let same = card.block(7) == card.block(0);
        out.push((
            "block 7 == block 0",
            if same { "True" } else { "False" }.into(),
            same,
        ));
        let ver = i32::from_be_bytes(card.block(8)[..4].try_into().unwrap());
        out.push((
            "SYSTEM_VERSION (block 8 bytes 0-3)",
            ver.to_string(),
            !(1..=4).contains(&ver),
        ));
    }
    out
}

/// Where a field lies in a copy: (group, instance, name, count, bits, first bit), in card order (decode.LAYOUT).
pub type Field = (&'static str, usize, &'static str, usize, usize, usize);

pub static LAYOUT: LazyLock<Vec<Field>> = LazyLock::new(|| {
    let (mut out, mut pos) = (Vec::new(), 0);
    for (group, reps) in GROUP_ORDER {
        let rows: Vec<_> = FIELDS.iter().filter(|r| r.0 == group).collect();
        let nbytes = rows.iter().map(|r| r.2 * r.3).sum::<usize>().div_ceil(8);
        for inst in 0..reps {
            let mut b = pos * 8;
            for r in &rows {
                out.push((group, inst, r.1, r.2, r.3, b));
                b += r.2 * r.3;
            }
            pos += nbytes;
        }
    }
    assert_eq!(pos, PAYLOAD_BYTES, "schema does not add up to 0x68C bytes");
    out
});

static INDEX: LazyLock<HashMap<(&'static str, usize, &'static str), Field>> =
    LazyLock::new(|| LAYOUT.iter().map(|f| ((f.0, f.1, f.2), *f)).collect());

/// A field by group, instance and name.
pub fn field(group: &str, inst: usize, name: &str) -> Field {
    *INDEX
        .get(&(group, inst, name))
        .unwrap_or_else(|| panic!("no field {group}[{inst}].{name}"))
}

pub fn bits_at(buf: &[u8], start: usize, width: usize) -> u32 {
    (start..start + width).fold(0, |v, i| {
        (v << 1) | u32::from((buf[i >> 3] >> (7 - (i & 7))) & 1)
    })
}

pub fn write_bits(buf: &mut [u8], start: usize, width: usize, value: u32) {
    for k in 0..width {
        let i = start + k;
        let m = 1 << (7 - (i & 7));
        if (u64::from(value) >> (width - 1 - k)) & 1 == 1 {
            buf[i >> 3] |= m;
        } else {
            buf[i >> 3] &= !m;
        }
    }
}

/// One copy decoded (decode.decode): its fields read on demand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded(pub Vec<u8>);

impl Decoded {
    pub fn of(copy: &[u8]) -> Decoded {
        Decoded(copy.to_vec())
    }

    pub fn get(&self, group: &str, inst: usize, name: &str) -> Vec<u32> {
        let (_, _, _, count, bits, first) = field(group, inst, name);
        (0..count)
            .map(|k| bits_at(&self.0, first + k * bits, bits))
            .collect()
    }

    pub fn one(&self, group: &str, name: &str) -> u32 {
        self.get(group, 0, name)[0]
    }

    /// Every field's values, in card order.
    pub fn all(&self) -> Vec<Vec<u32>> {
        LAYOUT
            .iter()
            .map(|&(g, i, n, ..)| self.get(g, i, n))
            .collect()
    }
}

/// A text field: its bytes up to the first 0, cp932 (latin-1 when that fails).
pub fn text(vals: &[u32]) -> String {
    let b: Vec<u8> = vals.iter().map(|&v| v as u8).collect();
    let b = &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())];
    encoding_rs::SHIFT_JIS
        .decode_without_bom_handling_and_without_replacement(b)
        .map_or_else(
            || b.iter().map(|&c| char::from(c)).collect(),
            |s| s.into_owned(),
        )
}

pub fn ymd8(v: u32) -> String {
    if v == 0 {
        return "-".into();
    }
    format!("{:04}-{:02}-{:02}", v / 10000, v / 100 % 100, v % 100)
}

pub fn ymd6(v: u32) -> String {
    if v == 0 {
        return "-".into();
    }
    format!("20{:02}-{:02}-{:02}", v / 10000, v / 100 % 100, v % 100)
}

/// The Regular League table from the card (decode.league): entries 1-7 of CLUB_RL_IMAGINE_* are the other teams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct League {
    pub win: u32,
    pub draw: u32,
    pub lose: u32,
    pub goals_for: u32,
    pub goals_against: u32,
    pub points: u32,
    pub position_computed: u32,
    pub division: u32,
    pub leg: u32,
}

pub fn league(d: &Decoded) -> League {
    let one = |n| d.one("Club", n);
    let (w, dr, l) = (one("CLUB_RL_WIN"), one("CLUB_RL_DRAW"), one("CLUB_RL_LOSE"));
    let (gf, ga) = (one("CLUB_RL_GET_GOAL"), one("CLUB_RL_LOSE_GOAL"));
    let tp = d.get("Club", 0, "CLUB_RL_IMAGINE_POINT");
    let tf = d.get("Club", 0, "CLUB_RL_IMAGINE_GOAL_POINT");
    let ta = d.get("Club", 0, "CLUB_RL_IMAGINE_GOAL_LOST_POINT");
    let key = |p: u32, f: u32, a: u32| (i64::from(p), i64::from(f) - i64::from(a), i64::from(f));
    let me = key(3 * w + dr, gf, ga);
    let better = (1..8).filter(|&k| key(tp[k], tf[k], ta[k]) > me).count() as u32;
    League {
        win: w,
        draw: dr,
        lose: l,
        goals_for: gf,
        goals_against: ga,
        points: 3 * w + dr,
        position_computed: better + 1,
        division: one("CLUB_RL_PRESENT_DIVISION"),
        leg: one("CLUB_RL_SECTION"),
    }
}

/// kit_common.card_session: ("none" / "unreadable" / "new" / "open" / "cut" / "closed", bad endings when known) of a
/// card file, read as the game reads it; "open" needs the file written after `since` (None: every mark is open).
pub fn session(card: &Path, since: Option<SystemTime>) -> (&'static str, Option<u32>) {
    let (Ok(raw), Ok(meta)) = (fs::read(card), fs::metadata(card)) else {
        return ("none", None);
    };
    if raw.len() != CARD_BYTES {
        return ("unreadable", None);
    }
    let c = CardFile::from_bytes(&raw).unwrap();
    if c.is_new() {
        return ("new", None);
    }
    let Some(chosen) = game_choice(&c).2 else {
        return ("unreadable", None);
    };
    let d = Decoded::of(c.copy(Some(chosen)));
    let bad = Some(d.one("UserInfo", "USER_INJUSTICE_NUM"));
    if d.one("UserInfo", "USER_INJUSTICE_FLAG") == 0 {
        return ("closed", bad);
    }
    let written = meta.modified().ok();
    let open = since.is_none_or(|s| written.is_some_and(|w| w > s));
    (if open { "open" } else { "cut" }, bad)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A card file the game accepts (edit.py's self-test card), its copy from `copy` with the game's checksum.
    pub fn card_with(counter: u16, copy: &[u8]) -> Vec<u8> {
        let mut raw = vec![0u8; CARD_BYTES];
        raw[..7].copy_from_slice(&[
            0xDE,
            0xAD,
            0xBE,
            0xEF,
            0x18,
            counter as u8,
            (counter >> 8) as u8,
        ]);
        raw[16 + 4 * 16 + 8..16 + 4 * 16 + 12].copy_from_slice(&[0x95, 0x71, 0x66, 0x40]);
        raw[16 + 6 * 16 + 12..16 + 7 * 16].copy_from_slice(&[0x00, 0x98, 0x96, 0x81]);
        let mut copy = copy.to_vec();
        let s = dword_sum(&copy[..PAYLOAD_BYTES]).wrapping_add(1);
        copy[PAYLOAD_BYTES..].copy_from_slice(&s.to_le_bytes());
        for start in [16 + 8 * 16, 16 + 113 * 16] {
            raw[start..start + COPY_BYTES].copy_from_slice(&copy);
        }
        raw
    }

    pub fn set(copy: &mut [u8], group: &str, inst: usize, name: &str, vals: &[u32]) {
        let (.., count, bits, first) = field(group, inst, name);
        assert!(vals.len() <= count);
        for (k, &v) in vals.iter().enumerate() {
            write_bits(copy, first + k * bits, bits, v);
        }
    }

    #[test]
    fn layout_adds_up_and_bits_round_trip() {
        assert_eq!(LAYOUT.last().unwrap().2, "CLUB_DOWNLOAD_CPU_WIN"); // built: the assert inside checks 0x68C
        // USER_INJUSTICE_FLAG / _NUM where kit_common reads them
        assert_eq!(field("UserInfo", 0, "USER_INJUSTICE_FLAG").5, 595);
        assert_eq!(field("UserInfo", 0, "USER_INJUSTICE_NUM").5, 596);
        let mut copy = vec![0u8; COPY_BYTES];
        set(&mut copy, "Club", 0, "CLUB_GET_PRIZE", &[12345]);
        set(&mut copy, "Player", 15, "PLAYER_CARD_NO", &[777]);
        let d = Decoded::of(&copy);
        assert_eq!(d.one("Club", "CLUB_GET_PRIZE"), 12345);
        assert_eq!(d.get("Player", 15, "PLAYER_CARD_NO"), [777]);
        assert_eq!(d.get("Player", 14, "PLAYER_CARD_NO"), [0]);
    }

    #[test]
    fn choice_checks_and_session() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.bin");
        assert_eq!(session(&p, None), ("none", None));
        let mut copy = vec![0u8; COPY_BYTES];
        set(&mut copy, "UserInfo", 0, "USER_INJUSTICE_NUM", &[3]);
        let raw = card_with(0xFFF0, &copy);
        let c = CardFile::from_bytes(&raw).unwrap();
        assert_eq!(game_choice(&c).2, Some('A'));
        assert!(card_checks(&c).iter().all(|x| x.2), "{:?}", card_checks(&c));
        fs::write(&p, &raw).unwrap();
        assert_eq!(session(&p, None), ("closed", Some(3)));
        set(&mut copy, "UserInfo", 0, "USER_INJUSTICE_FLAG", &[1]);
        fs::write(&p, card_with(0xFFF0, &copy)).unwrap();
        assert_eq!(session(&p, None), ("open", Some(3)));
        let later = SystemTime::now() + std::time::Duration::from_secs(3600);
        assert_eq!(session(&p, Some(later)), ("cut", Some(3)));
        let mut bad_a = card_with(0xFFF0, &copy);
        bad_a[16 + 8 * 16] ^= 0x55;
        assert_eq!(
            game_choice(&CardFile::from_bytes(&bad_a).unwrap()).2,
            Some('B')
        );
        fs::write(&p, card_with(0xFFFF, &copy)).unwrap();
        assert_eq!(session(&p, None), ("new", None));
        fs::write(&p, [0u8; RAW_BYTES]).unwrap();
        assert_eq!(session(&p, None), ("unreadable", None));
        assert!(read_card(&p).is_ok());
    }

    #[test]
    fn text_and_dates() {
        assert_eq!(text(&[0x41, 0x42, 0, 0x43]), "AB");
        assert_eq!(text(&[0x83, 0x41]), "ア");
        assert_eq!(text(&[0x81]), "\u{81}"); // not cp932: latin-1
        assert_eq!(ymd8(20101231), "2010-12-31");
        assert_eq!(ymd6(101231), "2010-12-31");
        assert_eq!(ymd6(0), "-");
    }
}
