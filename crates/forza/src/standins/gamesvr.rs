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

/// One stored club, as a COACH_LIST record needs it.
struct Club {
    coach: u32,
    card: u32,
    name1: Vec<u8>,
    name2: Vec<u8>,
    rec122: u8,
    plays: u8,
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
        Ok(Self(db))
    }

    /// The clubs of `aime` that hold a save (a listed club without one would end in error 3107), oldest first.
    fn clubs(&self, aime: u32) -> rusqlite::Result<Vec<Club>> {
        let mut q = self.0.prepare(
            "SELECT c.coach_id, c.card_id, c.name1, c.name2, c.rec122, c.play_count FROM coaches c
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
fn club(cmd: u16, req: &[u8], body: &mut [u8], store: &Mutex<Store>) -> Result<(), String> {
    // request size and the revision's offset (docs/research/ONLINE-2017-18.md, "M1")
    let (need, rev_at) = match cmd {
        271 => (20, 16),
        275 => (32, 28),
        277 => (8572, 5940),
        279 => (36, 32),
        287 | 289 => (28, 24),
        285 => return Err("ITEM_LIST: no item store, the club keeps its own values".into()),
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
        // unknown Aime id or 0: no records, the client starts the new-club flow
        for (i, c) in st.clubs(aime).map_err(db)?.iter().enumerate() {
            let r = &mut body[8 + 128 * i..][..128];
            put(r, 0, c.coach);
            r[4..4 + c.name1.len().min(56)].copy_from_slice(&c.name1[..c.name1.len().min(56)]);
            put(r, 60, c.card);
            r[64..64 + c.name2.len().min(56)].copy_from_slice(&c.name2[..c.name2.len().min(56)]);
            r[122] = c.rec122;
            r[123] = c.plays;
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
    let refused = club(cmd, payload, &mut body, store).err();
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
    std::fs::create_dir_all(&save_dir)?;
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
                match answer(cmd, &payload, &store) {
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
    fn the_secretaries_are_unlocked() {
        let dir = tempfile::tempdir().unwrap();
        let store = Mutex::new(Store::open(&dir.path().join("gamesvr.db")).unwrap());
        let (cmd, body, _) = answer(237, &[0; 20], &store).unwrap();
        assert_eq!((cmd, body.len()), (238, 0x5A54));
        assert!(body[SECRETARIES].iter().all(|b| *b == 1));
        // the four secretaries with an id (26..29) and the date and time left 0
        assert!((26..30).all(|n| body[16 + 1176 + n] == 1));
        assert!(body[8..16].iter().all(|b| *b == 0));
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
