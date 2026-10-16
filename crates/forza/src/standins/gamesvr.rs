//! Local stand-in for the 2017–18 game server ("gmsvr") that `control_Release.exe` relays club requests to.
//! Port of `_gamesvr.py`: 40-byte little-endian header (magic, command, sequence, status, size, connection id,
//! checksum, text), request N answered by N + 1 with a payload of the right size whose first u32 echoes the cabinet's
//! slot. The control program's framing: 0x40A970 build, 0x40ABE0 check, 0x40AB80 checksum.
//!
//! The club messages (COACH_LIST, COACH_LIST_ADD, SAVE_DATA_SET, SAVE_DATA, EXTENDS_NEW_ID, INJUSTICE_COUNT) are
//! answered from a SQLite store, `<save_dir>/gamesvr.db`, by the recipe of docs/research/ONLINE-2017-18.md ("M1");
//! every other message gets zeros. A request the store refuses (short, wrong revision, unknown ids) gets errCode 1.

use super::{Handle, Log, accept_loop, read_some};
use rusqlite::{Connection, OptionalExtension, params};
use std::io::{self, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// The secretaries' unlock bytes in answer 238: GAME_CTRL bytes 1176..1211, the block starting at +16 (+8 and +12
/// are a date and time, left 0: the control may set the PC's clock from them).
const SECRETARIES: std::ops::Range<usize> = 16 + 1176..16 + 1212;
const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];
/// Response command and payload size (control.c response dispatcher 0x408C50 and the message table).
const RESP: &[(u16, usize)] = &[
    (202, 16),
    (224, 92),
    (226, 260),
    (228, 12),
    (234, 1092),
    (238, 0x5A54),
    (240, 12),
    (244, 12),
    (246, 248),
    (248, 12),
    (250, 504),
    (256, 1008),
    (258, 12),
    (260, 10168),
    (262, 2040),
    (264, 10080),
    (266, 200),
    (268, 33600),
    (272, 9292),
    (274, 1288),
    (276, 6176),
    (278, 76),
    (280, 16),
    (282, 8),
    (284, 8),
    (286, 704),
    (288, 12),
    (290, 12),
    (292, 20),
    (294, 8),
    (296, 16),
    (298, 16),
    (300, 8),
    (302, 8),
    (304, 8),
    (306, 1004),
    (308, 3332),
    (310, 8),
    (312, 3352),
    (314, 8),
    (316, 8),
    (318, 32),
];

fn checksum(h: &[u8]) -> u8 {
    h.iter().enumerate().fold(
        0u8,
        |s, (i, b)| if i == 24 { s } else { s.wrapping_add(*b) },
    )
}

fn header(cmd: u16, seq: u16, status: u32, size: usize, conn: u32) -> [u8; 40] {
    let mut h = [0u8; 40];
    h[..8].copy_from_slice(&MAGIC);
    h[8..10].copy_from_slice(&cmd.to_le_bytes());
    h[10..12].copy_from_slice(&seq.to_le_bytes());
    h[12..16].copy_from_slice(&status.to_le_bytes());
    h[16..20].copy_from_slice(&((40 + size) as u32).to_le_bytes());
    h[20..24].copy_from_slice(&conn.to_le_bytes());
    h[25..40].copy_from_slice(b"000000000000000");
    h[24] = checksum(&h);
    h
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The revision the control writes into every request.
const REVISION: u32 = 47565;
/// The club's save block: SAVE_DATA_SET +28, SAVE_DATA +8.
const BLOCK: usize = 3072;
/// Raw request files kept in `save_dir` as evidence; older ones are deleted.
const KEEP_FILES: usize = 200;
/// The card master `prog_data\player_data\player_data.bin`: records of 0x620 bytes, +648 u16 the card number, +0x5DD
/// 2 = a 2D-code card, which plays only once the Aime's World Players List registers it (client sub_5BF6E0).
const RECORD: usize = 0x620;
/// COACH_LIST 272's World Players List: 1,000 entries of 8 bytes at +1,292.
const PLAYERS_LIST: usize = 1292;

/// The card numbers of the game's 2D-code cards (943 in the 2017-18 master), from the card master in `game`.
pub fn rare_cards(game: &Path) -> io::Result<Vec<u32>> {
    let master = std::fs::read(
        game.join("prog_data")
            .join("player_data")
            .join("player_data.bin"),
    )?;
    Ok(master
        .chunks_exact(RECORD)
        .filter(|r| r[0x5DD] == 2)
        .map(|r| u32::from(u16_at(r, 648)))
        .collect())
}

/// Wrapping sum of the little-endian u32 words: C1 over a block, C2 over a whole SAVE_DATA answer.
fn sum32(b: &[u8]) -> u32 {
    b.as_chunks::<4>()
        .0
        .iter()
        .fold(0u32, |s, w| s.wrapping_add(u32::from_le_bytes(*w)))
}

/// Fills a 6,176-byte SAVE_DATA answer (slot already at +0) with a stored block: +8 block, +3,080 version,
/// +3,082 size, +3,084 C1 = sum of the block, +3,088 C2 = sum of the whole answer with +3,088 taken as 0.
fn fill_save_data(body: &mut [u8], block: &[u8], version: u16, size: u16) {
    body[8..8 + BLOCK].copy_from_slice(block);
    body[3080..3082].copy_from_slice(&version.to_le_bytes());
    body[3082..3084].copy_from_slice(&size.to_le_bytes());
    put(body, 3084, sum32(block));
    put(body, 3088, 0);
    let c2 = sum32(body);
    put(body, 3088, c2);
}

/// Record +124 of a skip club: bit 4 (TitleHolder) of the mask `sub_792740` reads; with none of 1..8, 19, 21 set the
/// club list shows "tutorial in progress".
const SKIP_TITLES: u32 = 1 << 4;
/// The skip club's manager and club names, full-width Shift-JIS as the game's name entry writes them: "ＦＯＲＺＡ",
/// "ＦＯＲＺＡ　ＷＣＣＦ".
const SKIP_COACH: &[u8] = b"\x82\x65\x82\x6E\x82\x71\x82\x79\x82\x60";
const SKIP_CLUB: &[u8] =
    b"\x82\x65\x82\x6E\x82\x71\x82\x79\x82\x60\x81\x40\x82\x76\x82\x62\x82\x62\x82\x65";

/// Writes `value` into the low `width` bits at `bit` of a bit-packed block (bit 0 = most significant bit of byte 0).
fn put_bits(block: &mut [u8], bit: usize, width: usize, value: u32) {
    for i in 0..width {
        let (byte, mask) = ((bit + i) / 8, 0x80 >> ((bit + i) % 8));
        if value >> (width - 1 - i) & 1 == 1 {
            block[byte] |= mask;
        } else {
            block[byte] &= !mask;
        }
    }
}

/// The `width`-bit field at `bit` of a save block, most significant bit first (put_bits' reverse).
fn get_bits(block: &[u8], bit: usize, width: usize) -> u32 {
    (0..width).fold(0, |v, i| {
        v << 1 | u32::from(block[(bit + i) / 8] >> (7 - (bit + i) % 8) & 1)
    })
}

/// One stored club, as a COACH_LIST record needs it.
struct Club {
    coach: u32,
    card: u32,
    name1: Vec<u8>,
    name2: Vec<u8>,
    rec122: u8,
    plays: u8,
    titles: u32,
}

/// The clubs and their saves, `<save_dir>/gamesvr.db`. Ids are allocated from counters and never 0.
pub struct Store(Connection);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS coaches (
    aime_id    INTEGER NOT NULL,
    coach_id   INTEGER NOT NULL UNIQUE,
    card_id    INTEGER NOT NULL UNIQUE,
    name1      BLOB    NOT NULL DEFAULT x'', -- record +4, SAVE_DATA_SET +3,104 (56 raw bytes)
    name2      BLOB    NOT NULL DEFAULT x'', -- record +64, SAVE_DATA_SET +3,160
    rec122     INTEGER NOT NULL DEFAULT 0,   -- record +122, SAVE_DATA_SET +3,240
    play_count INTEGER NOT NULL DEFAULT 0,   -- record +123
    titles     INTEGER NOT NULL DEFAULT 0,   -- record +124 (enterable-titles mask; set for skip clubs only)
    created    INTEGER NOT NULL,
    updated    INTEGER NOT NULL,
    PRIMARY KEY (aime_id, coach_id)
);
CREATE TABLE IF NOT EXISTS saves (
    aime_id  INTEGER NOT NULL,
    coach_id INTEGER NOT NULL,
    block    BLOB    NOT NULL, -- SAVE_DATA_SET +28..+3,100
    version  INTEGER NOT NULL, -- +3,100
    size     INTEGER NOT NULL, -- +3,102
    kind     INTEGER NOT NULL, -- +24: 1 new club, 2 end of play, 3 new card
    updated  INTEGER NOT NULL,
    PRIMARY KEY (aime_id, coach_id),
    FOREIGN KEY (aime_id, coach_id) REFERENCES coaches (aime_id, coach_id)
);
CREATE TABLE IF NOT EXISTS injustice (
    aime_id  INTEGER NOT NULL,
    coach_id INTEGER NOT NULL,
    count    INTEGER NOT NULL,
    PRIMARY KEY (aime_id, coach_id)
);
CREATE TABLE IF NOT EXISTS ids (name TEXT PRIMARY KEY, last INTEGER NOT NULL);
";

/// Next id from counter `name` (1, 2, ...), inside the caller's transaction.
fn next_id(db: &Connection, name: &str) -> rusqlite::Result<u32> {
    db.query_row(
        "INSERT INTO ids VALUES (?1, 1) ON CONFLICT (name) DO UPDATE SET last = last + 1 RETURNING last",
        [name],
        |r| r.get(0),
    )
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(SCHEMA)?;
        // stores made before `titles` existed
        let has: bool = db.query_row(
            "SELECT count(*) > 0 FROM pragma_table_info('coaches') WHERE name = 'titles'",
            [],
            |r| r.get(0),
        )?;
        if !has {
            db.execute_batch("ALTER TABLE coaches ADD COLUMN titles INTEGER NOT NULL DEFAULT 0")?;
        }
        Ok(Self(db))
    }

    /// The clubs of `aime` that hold a save (a listed club without one would end in error 3107), oldest first.
    fn clubs(&self, aime: u32) -> rusqlite::Result<Vec<Club>> {
        let mut q = self.0.prepare(
            "SELECT c.coach_id, c.card_id, c.name1, c.name2, c.rec122, c.play_count, c.titles FROM coaches c
             JOIN saves s USING (aime_id, coach_id) WHERE c.aime_id = ?1 ORDER BY c.coach_id LIMIT 10",
        )?;
        q.query_map([aime], |r| {
            Ok(Club {
                coach: r.get(0)?,
                card: r.get(1)?,
                name1: r.get(2)?,
                name2: r.get(3)?,
                rec122: r.get(4)?,
                plays: r.get(5)?,
                titles: r.get(6)?,
            })
        })?
        .collect()
    }

    /// A new club for `aime`: (coachId, cardId), or None when it already has 10 saved clubs.
    fn new_club(&mut self, aime: u32) -> rusqlite::Result<Option<(u32, u32)>> {
        let tx = self.0.transaction()?;
        let n: u32 = tx.query_row(
            "SELECT count(*) FROM saves WHERE aime_id = ?1",
            [aime],
            |r| r.get(0),
        )?;
        if n >= 10 {
            return Ok(None);
        }
        let (coach, card) = (next_id(&tx, "coach")?, next_id(&tx, "card")?);
        tx.execute(
            "INSERT INTO coaches (aime_id, coach_id, card_id, created, updated) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![aime, coach, card, now()],
        )?;
        tx.commit()?;
        Ok(Some((coach, card)))
    }

    /// A saved club for `aime` built from a template block (the tutorial-skip club): (coachId, cardId), or None at 10 clubs.
    fn new_skip_club(
        &mut self,
        aime: u32,
        template: &[u8],
    ) -> rusqlite::Result<Option<(u32, u32)>> {
        let tx = self.0.transaction()?;
        let n: u32 = tx.query_row(
            "SELECT count(*) FROM saves WHERE aime_id = ?1",
            [aime],
            |r| r.get(0),
        )?;
        if n >= 10 {
            return Ok(None);
        }
        let (coach, card) = (next_id(&tx, "coach")?, next_id(&tx, "card")?);
        let (mut n1, mut n2) = (SKIP_COACH.to_vec(), SKIP_CLUB.to_vec());
        n1.resize(56, 0);
        n2.resize(56, 0);
        // the client upgrades a save with SYSTEM_VERSION < 11 through a bad address, so patch the template
        let mut block = template.to_vec();
        // the names inside the block, which the club screens show (the template's are CPU and World Best Eleven),
        // each followed by its reading: block bytes 76 manager, 132 its reading, 324 club, 380 its reading
        for (at, name) in [
            (76, &n1[..]),
            (132, b"FORZA"),
            (324, &n2[..]),
            (380, b"FORZA WCCF"),
        ] {
            block[at..at + 56].fill(0);
            block[at..at + name.len()].copy_from_slice(name);
        }
        for (bit, width, value) in [
            (0, 32, 12),
            (185, 32, card),
            (217, 32, coach),
            (20736, 32, 12),
            (21538, 32, 12),
            (22221, 6, 13), // COACH_TUTORIAL_PROGRESS: finished
            // SAVEDATA_DEBUG_VERSION, which club making sets: any other value and the client never sends the
            // end-of-play save 277 (client sub_804E00, docs/research/ONLINE-2017-18.md)
            (18752, 32, 20120221),
            (1504, 25, 20000101), // the manager's birthday, YYYYMMDD (the template's is 1900-01-01)
        ] {
            put_bits(&mut block, bit, width, value);
        }
        tx.execute(
            "INSERT INTO coaches (aime_id, coach_id, card_id, name1, name2, rec122, play_count, titles, created, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, 100, 0, ?6, ?7, ?7)",
            params![aime, coach, card, n1, n2, SKIP_TITLES, now()],
        )?;
        tx.execute(
            "INSERT INTO saves VALUES (?1, ?2, ?3, 12, 3072, 1, ?4)",
            params![aime, coach, block, now()],
        )?;
        tx.commit()?;
        Ok(Some((coach, card)))
    }

    /// Stores a SAVE_DATA_SET request's block and record fields; false when (aime, coach, card) is no club.
    fn set_save(&mut self, aime: u32, coach: u32, card: u32, req: &[u8]) -> rusqlite::Result<bool> {
        let kind = req[24];
        let tx = self.0.transaction()?;
        let n = tx.execute(
            "UPDATE coaches SET name1 = ?4, name2 = ?5, rec122 = ?6, play_count = min(255, play_count + ?7),
             updated = ?8 WHERE aime_id = ?1 AND coach_id = ?2 AND card_id = ?3",
            params![
                aime,
                coach,
                card,
                &req[3104..3160],
                &req[3160..3216],
                req[3240],
                u8::from(kind == 2),
                now()
            ],
        )?;
        if n == 0 {
            return Ok(false);
        }
        tx.execute(
            "INSERT OR REPLACE INTO saves VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                aime,
                coach,
                &req[28..28 + BLOCK],
                u16_at(req, 3100),
                u16_at(req, 3102),
                kind,
                now()
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// The stored (block, version, size) of a club.
    fn save(
        &self,
        aime: u32,
        coach: u32,
        card: u32,
    ) -> rusqlite::Result<Option<(Vec<u8>, u16, u16)>> {
        self.0
            .query_row(
                "SELECT s.block, s.version, s.size FROM saves s JOIN coaches c USING (aime_id, coach_id)
                 WHERE s.aime_id = ?1 AND s.coach_id = ?2 AND c.card_id = ?3",
                params![aime, coach, card],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
    }

    /// Moves a club from `old` to a new cardId; None when (aime, coach, old) is no club.
    fn new_card(&mut self, aime: u32, coach: u32, old: u32) -> rusqlite::Result<Option<u32>> {
        let tx = self.0.transaction()?;
        let card = next_id(&tx, "card")?;
        let n = tx.execute(
            "UPDATE coaches SET card_id = ?4, updated = ?5 WHERE aime_id = ?1 AND coach_id = ?2 AND card_id = ?3",
            params![aime, coach, old, card, now()],
        )?;
        if n == 0 {
            return Ok(None); // the dropped transaction rolls the counter back
        }
        tx.commit()?;
        Ok(Some(card))
    }

    /// Adds `delta` to a club's improper-end counter, floored at 0, and returns it.
    fn injustice(&mut self, aime: u32, coach: u32, delta: i32) -> rusqlite::Result<u32> {
        self.0.query_row(
            "INSERT INTO injustice VALUES (?1, ?2, max(0, ?3)) ON CONFLICT (aime_id, coach_id)
             DO UPDATE SET count = max(0, min(4294967295, count + ?3)) RETURNING count",
            params![aime, coach, delta],
            |r| r.get(0),
        )
    }
}

/// Answers a club message from the store into `body` (slot at +0 already). Err = refused, with the reason; the
/// caller answers errCode 1. Other messages are left as zeros.
fn club(
    cmd: u16,
    req: &[u8],
    body: &mut [u8],
    store: &Mutex<Store>,
    skip: Option<&Path>,
    rare: &[u32],
    log: &Log,
) -> Result<(), String> {
    // request size and the revision's offset (docs/research/ONLINE-2017-18.md, "M1")
    let (need, rev_at) = match cmd {
        271 => (20, 16),
        275 => (32, 28),
        277 => (8572, 5940),
        279 => (36, 32),
        287 | 289 => (28, 24),
        285 => (24, 20),
        _ => return Ok(()),
    };
    if req.len() < need {
        return Err(format!("{} bytes, want {need}", req.len()));
    }
    if u32_at(req, rev_at) != REVISION {
        return Err(format!("revision {}, want {REVISION}", u32_at(req, rev_at)));
    }
    let (aime, coach, card) = (u32_at(req, 4), u32_at(req, 8), u32_at(req, 12));
    let db = |e: rusqlite::Error| format!("store: {e}");
    let mut st = store.lock().unwrap_or_else(PoisonError::into_inner);
    if cmd == 271 {
        // unknown Aime id or 0: no records, the client starts the new-club flow (unless the tutorial is skipped)
        let mut clubs = st.clubs(aime).map_err(db)?;
        if let (true, Some(game)) = (clubs.is_empty() && aime != 0, skip) {
            let path = game.join("prog_data").join("cpu_team").join("cpu_50.dat");
            let template = std::fs::read(&path)
                .ok()
                .filter(|t| t.len() >= BLOCK)
                .ok_or_else(|| {
                    format!(
                        "skip_tutorial: {} is missing or shorter than {BLOCK} bytes",
                        path.display()
                    )
                })?;
            if st
                .new_skip_club(aime, &template[..BLOCK])
                .map_err(db)?
                .is_some()
            {
                log(&format!(
                    "skip_tutorial: club made for Aime id {aime} from cpu_50.dat"
                ));
                clubs = st.clubs(aime).map_err(db)?;
            }
        }
        for (i, c) in clubs.iter().enumerate() {
            let r = &mut body[8 + 128 * i..][..128];
            put(r, 0, c.coach);
            r[4..4 + c.name1.len().min(56)].copy_from_slice(&c.name1[..c.name1.len().min(56)]);
            put(r, 60, c.card);
            r[64..64 + c.name2.len().min(56)].copy_from_slice(&c.name2[..c.name2.len().min(56)]);
            r[122] = c.rec122;
            r[123] = c.plays;
            put(r, 124, c.titles);
        }
        if aime != 0 {
            // the server pre-registers every 2D-code card (registrations 1, owned 0, flag 0): there is no 2D-code
            // reader to register them one by one (docs/research/ONLINE-2017-18.md, "2D-code cards")
            for (e, &no) in body[PLAYERS_LIST..].chunks_exact_mut(8).zip(rare) {
                put(e, 0, no);
                e[4] = 1;
            }
        }
        return Ok(());
    }
    if aime == 0 {
        return Err("Aime id 0".into());
    }
    match cmd {
        279 => match req[12] {
            1 => {
                let (coach, card) = st.new_club(aime).map_err(db)?.ok_or("already 10 clubs")?;
                put(body, 8, coach);
                put(body, 12, card);
            }
            2 => {} // register an existing record: accepted
            k => return Err(format!("COACH_LIST_ADD kind {k}")),
        },
        277 => {
            let kind = req[24];
            if !(1..=3).contains(&kind) {
                return Err(format!("save kind {kind}"));
            }
            let sum = sum32(&req[28..28 + BLOCK]);
            if u32_at(req, 5236) != sum {
                return Err(format!(
                    "block sum {sum:#x}, the client says {:#x}",
                    u32_at(req, 5236)
                ));
            }
            if usize::from(u16_at(req, 3102)) > BLOCK {
                return Err(format!("save size {}", u16_at(req, 3102)));
            }
            if !st.set_save(aime, coach, card, req).map_err(db)? {
                return Err(format!("no club aime {aime} coach {coach} card {card}"));
            }
            put(body, 8, coach);
        }
        275 => {
            let (block, version, size) = st
                .save(aime, coach, card)
                .map_err(db)?
                .ok_or(format!("no save for aime {aime} coach {coach} card {card}"))?;
            if block.len() != BLOCK {
                return Err(format!("stored block of {} bytes", block.len()));
            }
            fill_save_data(body, &block, version, size);
        }
        285 => {
            // ITEM_LIST: errCode 0 is a must at card-in (else error 3107); the answer is the server's NET values,
            // kept in no save, so zeros, but +700/+701 CLUB_SHOP_UNIFORM, which the block keeps at bits 24249/24257
            if let Some((block, ..)) = st.save(aime, coach, card).map_err(db)? {
                body[700] = get_bits(&block, 24249, 8) as u8;
                body[701] = get_bits(&block, 24257, 8) as u8;
            }
        }
        287 => {
            let new = st
                .new_card(aime, coach, card)
                .map_err(db)?
                .ok_or(format!("no club aime {aime} coach {coach} card {card}"))?;
            put(body, 8, new);
        }
        289 => {
            if coach == 0 || card == 0 {
                return Err("coachId or cardId 0".into());
            }
            let delta = u32_at(req, 16) as i32;
            put(body, 8, st.injustice(aime, coach, delta).map_err(db)?);
        }
        _ => unreachable!("listed above"),
    }
    Ok(())
}

/// The response to one request: (command, payload, the store's refusal if any), or `None` for a command the table
/// does not know.
fn answer(
    cmd: u16,
    payload: &[u8],
    store: &Mutex<Store>,
    skip: Option<&Path>,
    rare: &[u32],
    log: &Log,
) -> Option<(u16, Vec<u8>, Option<String>)> {
    if cmd == 18 {
        // GMSVR_ECHO keep-alive
        let mut b = payload[..payload.len().min(4)].to_vec();
        b.resize(4, 0);
        return Some((18, b, None));
    }
    let rcmd = cmd.wrapping_add(1);
    let size = RESP.iter().find(|(c, _)| *c == rcmd)?.1;
    let mut body = vec![0u8; size];
    if size >= 4 && payload.len() >= 4 {
        body[..4].copy_from_slice(&payload[..4]);
    }
    if cmd == 237 {
        // the control copies payload +16.. as GAME_CTRL (sub_4419F0) and sends it to every seat, over the client's
        // defaults; GAME_CTRL bytes 1176..1211 unlock the 36 secretaries (default 1); all zero leaves none, and the club's SECRETARY step loads a picture that is not on
        // the disc and reads a null pointer (client 0x424AB2, docs/research/ONLINE-2017-18.md)
        body[SECRETARIES].fill(1);
    }
    if matches!(cmd, 307 | 311) {
        // GHOST_TEAM, BEST_ELEVEN: errCode 0 would install an all-zero team; 1 = none, the client plays a CPU team
        // (docs/research/ONLINE-2017-18.md)
        put(&mut body, 4, 1);
    }
    let refused = club(cmd, payload, &mut body, store, skip, rare, log).err();
    if refused.is_some() {
        put(&mut body, 4, 1);
    }
    Some((rcmd, body, refused))
}

/// Keeps the newest `keep` `.bin` files in `dir`.
fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "bin"))
        .map(|e| (e.metadata().and_then(|m| m.modified()).ok(), e.path()))
        .collect();
    if files.len() > keep {
        files.sort();
        for (_, p) in &files[..files.len() - keep] {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Serves the game server stand-in on `listener` (use `127.0.0.1:0` in tests). Requests are logged, the clubs kept
/// in `save_dir/gamesvr.db`, the last 200 request payloads in `save_dir`. Stops when the returned handle is dropped.
pub fn spawn(listener: TcpListener, save_dir: PathBuf, log: Log) -> io::Result<Handle> {
    spawn_with(listener, save_dir, log, None, false)
}

/// [`spawn`] with the game folder: COACH_LIST pre-registers the game's 2D-code cards ([`rare_cards`]), and with
/// `skip_tutorial` a first COACH_LIST for an Aime id without a club makes one from `prog_data\cpu_team\cpu_50.dat`,
/// so the mode screen offers NORMAL.
pub fn spawn_with(
    listener: TcpListener,
    save_dir: PathBuf,
    log: Log,
    game: Option<PathBuf>,
    skip_tutorial: bool,
) -> io::Result<Handle> {
    std::fs::create_dir_all(&save_dir)?;
    let rare = match game.as_deref().map(rare_cards) {
        Some(Ok(r)) => r,
        Some(Err(e)) => {
            log(&format!(
                "no 2D-code cards pre-registered: player_data.bin ({e})"
            ));
            Vec::new()
        }
        None => Vec::new(),
    };
    let skip_tutorial = game.filter(|_| skip_tutorial);
    let store = Arc::new(Mutex::new(
        Store::open(&save_dir.join("gamesvr.db")).map_err(io::Error::other)?,
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    accept_loop(listener, stop, move |mut conn, peer| {
        log(&format!("connected {peer}"));
        let mut buf: Vec<u8> = Vec::new();
        let mut tmp = vec![0u8; 65536];
        loop {
            match read_some(&mut conn, &mut tmp, &flag) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(e) => {
                    log(&format!("recv error {e}"));
                    break;
                }
            }
            while buf.len() >= 40 {
                if buf[..8] != MAGIC {
                    let skip = buf
                        .windows(8)
                        .skip(1)
                        .position(|w| w == MAGIC)
                        .map(|i| i + 1);
                    log(&format!(
                        "resync: dropped {} bytes",
                        skip.unwrap_or(buf.len())
                    ));
                    buf.drain(..skip.unwrap_or(buf.len()));
                    continue;
                }
                // ponytail: the Python hangs on a size below 40; floor it so a bad header cannot spin.
                let size = (u32_at(&buf, 16) as usize).max(40);
                if buf.len() < size {
                    break;
                }
                let (cmd, seq) = (
                    u16::from_le_bytes([buf[8], buf[9]]),
                    u16::from_le_bytes([buf[10], buf[11]]),
                );
                let (status, id) = (u32_at(&buf, 12), u32_at(&buf, 20));
                let payload = buf[40..size].to_vec();
                let text =
                    String::from_utf8_lossy(buf[25..40].split(|b| *b == 0).next().unwrap_or(&[]))
                        .into_owned();
                buf.drain(..size);
                let first: String = payload
                    .iter()
                    .take(48)
                    .map(|b| format!("{b:02x}"))
                    .collect();
                log(&format!(
                    "<- cmd {cmd} seq {seq} status {status} payload {} conn {id} text {text:?} first {first}",
                    payload.len()
                ));
                if !payload.is_empty() {
                    let ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    let _ = std::fs::write(save_dir.join(format!("req_{cmd}_{ms}.bin")), &payload);
                    prune(&save_dir, KEEP_FILES);
                }
                match answer(cmd, &payload, &store, skip_tutorial.as_deref(), &rare, &log) {
                    Some((rcmd, body, refused)) => {
                        if let Some(why) = refused {
                            log(&format!("   refused cmd {cmd}: {why} (errCode 1)"));
                        }
                        let mut out = header(rcmd, seq, 1, body.len(), id).to_vec();
                        out.extend_from_slice(&body);
                        if conn.write_all(&out).is_err() {
                            return;
                        }
                        log(&format!("-> cmd {rcmd} payload {}", body.len()));
                    }
                    None => log(&format!("   (no answer for cmd {cmd})")),
                }
            }
        }
        log(&format!("closed {peer}"));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c1_c2_of_hand_built_blocks() {
        // 768 words of 1: C1 = 768; 0xFFFF_FFFF words wrap to -768
        assert_eq!(sum32(&[1, 0, 0, 0].repeat(768)), 768);
        assert_eq!(sum32(&[0xFF; BLOCK]), 0u32.wrapping_sub(768));
        let mut body = vec![0u8; 6176];
        put(&mut body, 0, 1); // slot
        fill_save_data(&mut body, &[1, 0, 0, 0].repeat(768), 12, 3072);
        assert_eq!(u32_at(&body, 8 + BLOCK - 4), 1);
        assert_eq!(u32_at(&body, 3080), 12 | (3072 << 16));
        assert_eq!(u32_at(&body, 3084), 768);
        // C2 = slot + block + version/size word + C1, with its own field as 0
        assert_eq!(u32_at(&body, 3088), 1 + 768 + (12 | (3072 << 16)) + 768);
        assert!(body[3092..].iter().all(|b| *b == 0));
    }

    #[test]
    fn put_bits_is_msb_first() {
        let mut b = [0u8; 2780];
        put_bits(&mut b, 0, 32, 12);
        assert_eq!(b[..4], [0, 0, 0, 12]);
        put_bits(&mut b, 22221, 6, 13); // 001101 at bits 22221..22227: byte 2777 bits 5..7, byte 2778 bits 0..2
        assert_eq!((b[2777], b[2778]), (0b0000_0001, 0b1010_0000));
        put_bits(&mut b, 22221, 6, 0);
        assert_eq!((b[2777], b[2778]), (0, 0));
        put_bits(&mut b, 185, 32, u32::MAX); // 1 + 31 bits across bytes 23..27
        assert_eq!(b[23..28], [0x7F, 0xFF, 0xFF, 0xFF, 0x80]);
    }

    #[test]
    fn the_secretaries_are_unlocked() {
        let dir = tempfile::tempdir().unwrap();
        let store = Mutex::new(Store::open(&dir.path().join("gamesvr.db")).unwrap());
        let (cmd, body, _) = answer(
            237,
            &[0; 20],
            &store,
            None,
            &[],
            &(Arc::new(|_: &str| {}) as Log),
        )
        .unwrap();
        assert_eq!((cmd, body.len()), (238, 0x5A54));
        assert!(body[SECRETARIES].iter().all(|b| *b == 1));
        // the four secretaries with an id (26..29) and the date and time left 0
        assert!((26..30).all(|n| body[16 + 1176 + n] == 1));
        assert!(body[8..16].iter().all(|b| *b == 0));
    }

    fn req(cmd: u16, ids: [u32; 3]) -> Vec<u8> {
        let (len, rev) = match cmd {
            271 => (20, 16),
            285 => (24, 20),
            _ => (32, 28),
        };
        let mut r = vec![0u8; len];
        put(&mut r, 4, ids[0]);
        put(&mut r, 8, ids[1]);
        put(&mut r, 12, ids[2]);
        put(&mut r, rev, REVISION);
        r
    }

    fn pad(name: &[u8]) -> Vec<u8> {
        let mut v = name.to_vec();
        v.resize(56, 0);
        v
    }

    #[test]
    fn skip_tutorial_makes_one_club_from_the_template() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir_all(game.join("prog_data").join("cpu_team")).unwrap();
        let template: Vec<u8> = (0..BLOCK + 8).map(|i| (i * 7 + 1) as u8).collect();
        std::fs::write(game.join("prog_data/cpu_team/cpu_50.dat"), &template).unwrap();
        let store = Mutex::new(Store::open(&dir.path().join("gamesvr.db")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let ask = |cmd, ids, skip: Option<&Path>| {
            answer(cmd, &req(cmd, ids), &store, skip, &[], &log)
                .unwrap()
                .1
        };

        // off: no club, errCode 0, unchanged
        assert_eq!(u32_at(&ask(271, [5, 0, 0], None), 8 + 60), 0);
        // on: one record with ids and the template
        let body = ask(271, [5, 0, 0], Some(&game));
        let (coach, card) = (u32_at(&body, 8), u32_at(&body, 8 + 60));
        assert!(coach != 0 && card != 0);
        assert_eq!(&body[8 + 4..8 + 4 + 56], &pad(SKIP_COACH)[..]);
        assert_eq!(&body[8 + 64..8 + 64 + 56], &pad(SKIP_CLUB)[..]);
        // a full term (100 matches): 0 is a term that has ended, and the client offers to retire the club
        assert_eq!((body[8 + 122], body[8 + 123]), (100, 0));
        assert_eq!(u32_at(&body, 8 + 124), SKIP_TITLES);
        assert_eq!(u32_at(&body, 8 + 128 + 60), 0, "a single record");
        let save = ask(275, [5, coach, card], Some(&game));
        assert_eq!(u32_at(&save, 4), 0);
        // the template with the seven fields patched
        let mut want = template[..BLOCK].to_vec();
        for (bit, width, value) in [
            (0, 32, 12),
            (185, 32, card),
            (217, 32, coach),
            (20736, 32, 12),
            (21538, 32, 12),
            (22221, 6, 13),
            (18752, 32, 20120221),
            (1504, 25, 20000101),
        ] {
            let at = |b: &[u8]| get_bits(b, bit, width);
            put_bits(&mut want, bit, width, value);
            assert_eq!(at(&want), value);
            assert_eq!(at(&save[8..8 + BLOCK]), value, "bit {bit}");
        }
        // the names: manager and club with their readings
        for (at, name) in [
            (76, pad(SKIP_COACH)),
            (132, pad(b"FORZA")),
            (324, pad(SKIP_CLUB)),
            (380, pad(b"FORZA WCCF")),
        ] {
            want[at..at + 56].copy_from_slice(&name);
        }
        assert_eq!(&save[8..8 + BLOCK], &want[..]);
        assert_eq!(u32_at(&save, 3080), 12 | (3072 << 16));
        assert_eq!(u32_at(&save, 3084), sum32(&want));
        assert_ne!(u32_at(&save, 3088), 0);
        // ITEM_LIST: errCode 0, the shop uniforms from the block
        let items = ask(285, [5, coach, card], Some(&game));
        assert_eq!(u32_at(&items, 4), 0);
        let uniform = |bit| get_bits(&want, bit, 8) as u8;
        assert_eq!((items[700], items[701]), (uniform(24249), uniform(24257)));
        // a second 271 returns the same club
        let again = ask(271, [5, 0, 0], Some(&game));
        assert_eq!(again[8..8 + 128], body[8..8 + 128]);
        assert_eq!(u32_at(&again, 8 + 128 + 60), 0);
        // Aime id 0 and a short template make nothing
        assert_eq!(u32_at(&ask(271, [0, 0, 0], Some(&game)), 8 + 60), 0);
        std::fs::write(game.join("prog_data/cpu_team/cpu_50.dat"), [0; 100]).unwrap();
        let (_, body, why) =
            answer(271, &req(271, [6, 0, 0]), &store, Some(&game), &[], &log).unwrap();
        assert!(why.is_some() && u32_at(&body, 8 + 60) == 0);
    }

    #[test]
    fn coach_list_pre_registers_the_2d_code_cards() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir_all(game.join("prog_data/player_data")).unwrap();
        // three card-master records: 7127 and 1007 need a 2D code (+0x5DD = 2), 7054 does not (1)
        let mut master = vec![0u8; 3 * RECORD];
        for (i, (no, kind)) in [(7127u16, 2u8), (7054, 1), (1007, 2)]
            .into_iter()
            .enumerate()
        {
            master[i * RECORD + 648..][..2].copy_from_slice(&no.to_le_bytes());
            master[i * RECORD + 0x5DD] = kind;
        }
        std::fs::write(game.join("prog_data/player_data/player_data.bin"), &master).unwrap();
        let rare = rare_cards(&game).unwrap();
        assert_eq!(rare, [7127, 1007]);

        let store = Mutex::new(Store::open(&dir.path().join("gamesvr.db")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let body = answer(271, &req(271, [5, 0, 0]), &store, None, &rare, &log)
            .unwrap()
            .1;
        assert_eq!(u32_at(&body, 4), 0);
        // World Players List entries: card, registrations 1, owned 0, flag 0
        assert_eq!(&body[1292..1300], &[0xD7, 0x1B, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&body[1300..1308], &[0xEF, 0x03, 0, 0, 1, 0, 0, 0]);
        assert!(body[1308..].iter().all(|b| *b == 0));
        // Aime id 0 (guest): no list
        let guest = answer(271, &req(271, [0, 0, 0]), &store, None, &rare, &log)
            .unwrap()
            .1;
        assert!(guest[1292..].iter().all(|b| *b == 0));
        // QR_REGIST, QR_DEL_SERIAL, QR_DEL_CARDID: accepted
        for cmd in [295, 297, 299] {
            let (_, b, why) = answer(cmd, &[0; 32], &store, None, &rare, &log).unwrap();
            assert!(why.is_none() && u32_at(&b, 4) == 0, "cmd {cmd}");
        }
    }

    #[test]
    fn raw_files_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..205 {
            std::fs::write(dir.path().join(format!("req_1_{i}.bin")), [0]).unwrap();
        }
        std::fs::write(dir.path().join("gamesvr.db"), [0]).unwrap();
        prune(dir.path(), 200);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            201,
            "200 .bin files and the store"
        );
    }
}
