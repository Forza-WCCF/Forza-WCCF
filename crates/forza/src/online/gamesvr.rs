//! Local stand-in for the 2017–18 game server ("gmsvr") that `control_Release.exe` relays club requests to.
//! Port of `_gamesvr.py`: 40-byte little-endian header (magic, command, sequence, status, size, connection id,
//! checksum, text), request N answered by N + 1 with a payload of the right size whose first u32 echoes the cabinet's
//! slot. The control program's framing: 0x40A970 build, 0x40ABE0 check, 0x40AB80 checksum.
//!
//! The club messages (COACH_LIST, COACH_LIST_ADD, SAVE_DATA_SET, SAVE_DATA, EXTENDS_NEW_ID, INJUSTICE_COUNT) are
//! answered from a SQLite store, `<save_dir>/gamesvr.db`, by the recipe of docs/research/ONLINE-2017-18.md ("M1");
//! every other message gets zeros. A request the store refuses (short, wrong revision, unknown ids) gets errCode 1.

use super::club::team_tag;
use super::sessions::{Session, Sessions};
use super::vs::{self, Matcher, VsConfig};
use crate::service::{Handle, Log, accept_loop, read_some};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::HashMap;
use std::io::{self, IoSlice, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

/// The secretaries' unlock bytes in answer 238: GAME_CTRL bytes 1176..1211, the block starting at +16 (+8 and +12
/// are a date and time, left 0: the control may set the PC's clock from them).
const SECRETARIES: std::ops::Range<usize> = 16 + 1176..16 + 1212;
/// The client's own GAME_CTRL defaults (sub_4A5640): `(GAME_CTRL offset, width in bytes, value)`, little-endian; the
/// secretaries (1,176..1,211) and the rest stay as [`SECRETARIES`] and zero. Answer 238 replaces the defaults on
/// every seat, so zeros there changed the client's behaviour (poll count, times, limits). Payload offset = offset + 16.
/// 12, 14, 15 are also the relay thresholds `vs::fill_game_ctrl` sets; 19,740 is the REQ_MATCH poll count, which VS
/// lowers to 9. Offsets 1,092..1,104 are the Universal Cup open times (HHMM, host clock); 1,172 = 1 lets the control
/// insert a Universal Cup (competition 31) into its schedule before each open time (control 0x462820; the client's
/// default 0 never schedules one), 1,173 = minutes per slot, 1,174 = minutes after control start before the first
/// cup; 14,608 / 14,610 take the common branch (A). 7,622..7,664 are the Grand Trophy block: the window (u16 HHMM
/// start and end, 0..2400 = open all day; the row is selectable iff start <= now <= end, and the control tags a VS
/// entry with the card's class only while it is open) and the per-match points as the client's GT object constructor
/// sets them (win 300, draw 100, per goal 100, clean sheet 50, per goal conceded 10, hat-trick 100, win-streak record
/// 100, CPU match parameters higher 4,500 / lower 2,000; their reader is unknown, A). The rest of the block (lose,
/// bonuses, ghost rates, playoff CPU level, promotion / playoff / staying thresholds) is 0 there as here. The week id
/// and the matches per class are [`fill_game_ctrl`]'s.
const GAME_CTRL_DEFAULTS: &[(usize, usize, u32)] = &[
    (1, 1, 5),
    (2, 2, 5),
    (4, 4, 5_000),
    (8, 4, 250_000),
    (12, 2, 200),
    (14, 1, 20),
    (15, 1, 50),
    (16, 1, 14),
    (17, 1, 3),
    (20, 4, 1_000_000),
    (34, 1, 2),
    (35, 1, 1),
    (996, 1, 1),
    (997, 1, 10),
    (998, 1, 1),
    // only the host's control asks 237/238 and schedules the cup (seats ask it NEXT_COMPE), so its OPEN_TIME keys win
    (1092, 4, 1_200),
    (1096, 4, 1_500),
    (1100, 4, 1_800),
    (1104, 4, 2_100),
    (1172, 1, 1),
    (1173, 1, 9),
    (1174, 1, 20),
    (1216, 1, 1),
    (1218, 2, 600),
    (7622, 2, 0),
    (7624, 2, 2_400),
    (7626, 2, 300),
    (7630, 2, 100),
    (7632, 2, 100),
    (7634, 2, 50),
    (7636, 2, 10),
    (7638, 2, 100),
    (7640, 2, 100),
    (7660, 2, 4_500),
    (7662, 2, 2_000),
    (14532, 4, 61_440),
    (14542, 1, 1),
    (14608, 1, 5),
    (14610, 1, 1),
    (14611, 1, 10),
    (19716, 1, 1),
    (19740, 1, 26),
];

// "shop" in this file is the original game's: its item shop and the venue fields of its packets, not an arcade room

/// The shop's sale list: GAME_CTRL +22,896 (payload +22,912), 10 entries of `{u32 category, u32 item, u32 start,
/// u32 end}` (client sub_834520). Categories: 1 Uniform, 2 SE, 3 BGM, 4 Flag, 5 Trainingwear; 0 is never valid. The
/// client offers an entry only when the item's master row also has its for-sale flag set, so rows outside the
/// flagged ones are skipped on the client. These ten are rows of the 2017–18 data tables that look like shop goods
/// (Uniform = the "shop_1718_*" rows of ShopUni, BGM 1..3 = Bgm1..3, Flag 1..3 and Trainingwear 1..2 = the first
/// coloured rows); the flags themselves are not read. SE (2) stays out: its 286 +93 mapping is unverified
/// ([`APPLY_SE`]). A data-driven rotation is a follow-up.
const SHOP_SALE: [(u32, u32); 10] = [
    (1, 1),
    (1, 2),
    (3, 1),
    (3, 2),
    (3, 3),
    (4, 1),
    (4, 2),
    (4, 3),
    (5, 1),
    (5, 2),
];
/// GAME_CTRL +22,896 as a 238 payload offset.
const SHOP_SALE_AT: usize = 16 + 22_896;
/// Sale period of every entry, YYYYMMDD.
const SHOP_FROM: u32 = 20_000_101;
const SHOP_TO: u32 = 20_991_231;
/// Whether a bought SE (category 2) is written to 286 +93. Unverified (the reader maps the value through another
/// table than the shop's), so off; [`SHOP_SALE`] has no SE entry either.
const APPLY_SE: bool = false;

/// Grand Trophy matches counted for the weekly record (the sum of the best N scores), per class: Local, Area B,
/// Area A, World B, World A (GAME_CTRL 7,666..7,670, u8, max 10: the client keeps 10 best scores). Provisional until
/// a player of the original confirms them. With 0 the client never grows the record, and the post-match record
/// screen skips the counter that stops its looping count-up sound.
const GT_MATCHES_PER_CLASS: [u8; 5] = [3, 5, 5, 7, 7];
/// 238 payload offsets of the Grand Trophy week id (u16) and [`GT_MATCHES_PER_CLASS`].
const GT_WEEK_AT: usize = 16 + 7_620;
const GT_MATCHES_AT: usize = 16 + 7_666;
/// Week 0 of [`gt_week`] starts on Sunday 2000-01-02 23:00 Italian time (the Monday 2000-01-03 rule shifted by an
/// hour); a u16 lasts about 1,250 years.
const GT_EPOCH: chrono::NaiveDate =
    chrono::NaiveDate::from_ymd_opt(2000, 1, 3).expect("a valid date");

/// The Grand Trophy week of a moment: weeks run Monday to Sunday and roll on Sunday at 23:00 Europe/Rome wall-clock
/// time (CET or CEST), like the original's Sunday 23:00 in its own time zone.
fn gt_week(unix_secs: i64) -> u16 {
    let rome = chrono::DateTime::from_timestamp(unix_secs, 0)
        .unwrap_or_default()
        .with_timezone(&chrono_tz::Europe::Rome)
        .naive_local();
    let days = (rome + chrono::TimeDelta::hours(1))
        .date()
        .signed_duration_since(GT_EPOCH)
        .num_days();
    days.div_euclid(7).clamp(0, i64::from(u16::MAX)) as u16
}

/// SAVE_DATA_SET 277 +5,572: the club's weekly Grand Trophy record (SERVER_COACH_GT_MAXPOINT, s32), the sum of its
/// best N scores; the client grows it only once it has played N matches this week (client 0x7FCB12), so a non-zero
/// record is a ranked entry. The client writes it from save key 28,428 (sub_817F10 0x818B4F).
const GT_RECORD_277: usize = 5572;
/// The highest record [`record_gt`] keeps; the record comes off a player's PC.
/// 100,000: about 3x the most this server's own scoring can give (4,500 a match at GAME_CTRL 7,660 x N <= 7).
const GT_RECORD_MAX: u32 = 100_000;
/// SAVE_DATA 276 +3,212: the club's rank (SERVER_COACH_GT_NOWRANKING, s32; 0 shows "-----"), which the client sets
/// from the answer (sub_81AC50 0x81B06D, key 28,472) and shows on its Grand Trophy ranking screen (0x718200). It sits
/// in [`TAIL_SPANS`]' first span, so the stored 277's value (+5,592) is there unless this server writes its own.
const GT_RANK_276: usize = 3212;
/// The weekly Grand Trophy fields of a 276, `(offset, length)`, zeroed at a club's first card-in of a new week: the
/// client never rolls the week itself (no reader found that resets them), and the original's server did. Record
/// (+3,192) and points (+3,196), rank (+3,212), week id (+3,220) and matches played (u8 +3,224), the 10 best scores
/// (+3,288). Keys 28,428, 28,424, 28,472, 28,420, 28,416 (sub_81AC50 0x81AFDC..0x81B0C8), 28,432 (the 10-score
/// loop, same order as sub_817F10's 277 +5,668). 276 = 277 - 2,380 throughout the first span.
const GT_WEEKLY_276: [(usize, usize); 4] = [(3192, 8), (3212, 4), (3220, 5), (3288, 40)];

/// Writes a club's Grand Trophy week into a 276 that already holds its stored 277's spans: a 277 saved in an earlier
/// week than `week` loses its weekly fields, and the rank (1-based, 0 = not ranked this week) replaces the stored one.
fn fill_gt_week(body: &mut [u8], saved_week: u16, week: u16, rank: u32) {
    // `!=`: a save stamped in a later week (an imported club, a clock set back) is no record of this week either
    if saved_week != week {
        for (at, len) in GT_WEEKLY_276 {
            body[at..at + len].fill(0);
        }
    }
    put(body, GT_RANK_276, rank);
}

/// SAVE_DATA_SET 277 +5,609: the club's Grand Trophy class (COACH_GT_CLASS, key 14,700, u8; sub_817F10 0x818E14):
/// 0 Local, 1 Area B, 2 Area A, 3 World B, 4 World A. The 276 applier never sets the key (sub_81AC50 skips +3,229);
/// the client changes it itself from the result fields below.
const GT_CLASS_277: usize = 5609;
/// SAVE_DATA 276 Grand Trophy result fields (sub_81AC50): the final rank (SERVER_COACH_GT_FINISHRANKING, s32 +3,216,
/// key 28,480, 0x81B084), the next class (SERVER_COACH_GT_NEXT_CLASS, u8 +3,230, key 28,484, 0x81B142) and the
/// play-off flag (u8 +3,285, key 28,488, 0x81B211). A final rank >= 1 makes the card-in show the result info
/// (sub_6108B0 0x6109E5), which sets the class to the next class unless the flag is 1 and then zeroes rank and next
/// class (sub_6CEBE0 0x6D09D2, sub_6D1770). The client clamps nothing: a next class below the class demotes, to
/// Local even. All three sit in the first tail span (277 +5,596/+5,610/+5,665).
const GT_FINAL_RANK_276: usize = 3216;
const GT_NEXT_CLASS_276: usize = 3230;
const GT_PLAYOFF_276: usize = 3285;
/// The closed week's record that moves a club up one class, per class; 0 = no promotion, so Area A is the cap. Also
/// GAME_CTRL raisePoint (7,676 + 4 x class, s32), the ranking screen's "est. promotion" (0x7171D0).
const GT_RAISE: [u32; 5] = [1_000, 1_200, 0, 0, 0];
const GT_RAISE_AT: usize = 16 + 7_676;

/// Writes a card-in's Grand Trophy result fields: a promotion from `class` with the closed week's `rank` (rank and
/// next class always together, next = class + 1, never above Area A), or zero, so a stored or imported 277 can never
/// send a result the server did not issue. The play-off flag is always 0 (no play-offs). Called only on a server with
/// VS: an arcade's stand-in passes the stored 277's fields through, so a promotion a VS server issued that the club
/// has not applied yet (saved before the info scene) shows at its next card-in at home, once.
fn fill_gt_result(body: &mut [u8], promote: Option<(u8, u32)>) {
    let (rank, next) = match promote {
        Some((class, rank)) if class < 2 => (rank.max(1), class + 1),
        _ => (0, 0),
    };
    put(body, GT_FINAL_RANK_276, rank);
    body[GT_NEXT_CLASS_276] = next;
    body[GT_PLAYOFF_276] = 0;
}

/// Records a 277's Grand Trophy record under the server's `week`, the one the save arrived in, per card (Aime id): a
/// central server imports a player's club for each session with fresh coachIds (online::club), so the card is the
/// one id that lasts, and a card's entry is the best record of its clubs. The client's own week id (277 +5,600) is not
/// used: it is the GAME_CTRL week the control fetched at its start, and a control may run for days. A record that
/// grows takes `at`, the tie-break. Records of 0 (not yet N matches) and above [`GT_RECORD_MAX`] are skipped.
fn record_gt(db: &Connection, week: u16, aime: u32, req: &[u8], at: i64) -> rusqlite::Result<()> {
    let points = u32_at(req, GT_RECORD_277);
    if (1..=GT_RECORD_MAX).contains(&points) {
        db.execute(
            "INSERT INTO gt_points VALUES (?1, ?2, ?3, ?4) ON CONFLICT (week, aime_id)
             DO UPDATE SET points = excluded.points, at = excluded.at WHERE excluded.points > points",
            params![week, aime, points, at],
        )?;
    }
    Ok(())
}

/// Answer 238's GAME_CTRL (from payload +16): the client's defaults, the unlocked secretaries and the Grand Trophy
/// week of `unix_secs`. +8 and +12 (date and time) stay 0: the control may set the PC's clock from them.
fn fill_game_ctrl(body: &mut [u8], unix_secs: i64) {
    for &(at, width, v) in GAME_CTRL_DEFAULTS {
        body[16 + at..16 + at + width].copy_from_slice(&v.to_le_bytes()[..width]);
    }
    body[GT_WEEK_AT..GT_WEEK_AT + 2].copy_from_slice(&gt_week(unix_secs).to_le_bytes());
    body[GT_MATCHES_AT..GT_MATCHES_AT + 5].copy_from_slice(&GT_MATCHES_PER_CLASS);
    body[SECRETARIES].fill(1);
    for (k, (cat, item)) in SHOP_SALE.into_iter().enumerate() {
        for (i, v) in [cat, item, SHOP_FROM, SHOP_TO].into_iter().enumerate() {
            put(body, SHOP_SALE_AT + 16 * k + 4 * i, v);
        }
    }
}

/// What the stored purchases (category, item, oldest first) set in a 286 ITEM_LIST answer: Uniform -> +700 and +701
/// (CLUB_SHOP_UNIFORM), BGM -> +42 (the goal jingle), SE -> +93 when [`APPLY_SE`]. The last purchase of a category
/// wins; nothing on the client ever sets these, so a purchase takes precedence over the block's value.
fn apply_to_item_list(body: &mut [u8], bought: &[(u32, u32)]) {
    for &(cat, item) in bought {
        let Ok(v) = u8::try_from(item) else { continue };
        match cat {
            1 => (body[700], body[701]) = (v, v),
            2 if APPLY_SE => body[93] = v,
            3 => body[42] = v,
            _ => {}
        }
    }
}

/// The 276 possess flags the purchases set (one byte per item, 1): Flag -> +5,444 + item (512), Trainingwear ->
/// +5,188 + item (256). Applied after the stored 277's spans on every 276, since the 277 of the session that made
/// the purchase still carries 0.
fn apply_to_save_data(body: &mut [u8], bought: &[(u32, u32)]) {
    for &(cat, item) in bought {
        let at = match (cat, item) {
            (4, 0..512) => 5444 + item,
            (5, 0..256) => 5188 + item,
            _ => continue,
        };
        body[at as usize] = 1;
    }
}

const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];
/// What an all-zero answer means for a [`RESP`] entry; the guard test (`zero_answers_are_safe_or_pending`) enforces it.
#[cfg_attr(not(test), allow(dead_code))] // the texts are read by the guard test
enum Zero {
    /// The stand-in fills the answer: it must not be all zero.
    Handled,
    /// Zero is right: the text names the parser (`sub_`/`0x` address) and why. A stale entry (the answer is not
    /// zero) fails the test.
    Safe(&'static str),
    /// Zero is wrong or partial and a parity unit still owes the work (the unit or decision): listed, not failed.
    Pending(&'static str),
}

/// Response command, payload size and zero-ness (control.c response dispatcher 0x408C50 and the message table).
const RESP: &[(u16, usize, Zero)] = &[
    (202, 16, Zero::Handled),
    (901, 8, Zero::Handled),
    (903, 8, Zero::Handled),
    (905, 48, Zero::Handled),
    (224, 92, Zero::Handled),
    (
        226,
        260,
        Zero::Safe("ctl sub_442C40: an empty banner (A: consumer not found)"),
    ),
    (
        228,
        12,
        Zero::Safe("ctl sub_447880: 227 has no sender (ctl sub_447550)"),
    ),
    (234, 1092, Zero::Pending("C6")),
    (238, 0x5A54, Zero::Handled),
    (240, 12, Zero::Handled),
    (244, 12, Zero::Safe("ctl sub_409920: stored, no reader")),
    (246, 248, Zero::Handled),
    (
        248,
        12,
        Zero::Safe("ctl sub_408C50 case only: 247 has no sender"),
    ),
    (250, 504, Zero::Handled),
    (
        256,
        1008,
        Zero::Safe(
            "ctl sub_443450: 255 is sent only when 238 +13,110 differs from the local version, and 238 sends 0",
        ),
    ),
    (258, 12, Zero::Handled),
    (
        260,
        10168,
        Zero::Safe("projector sub_473B40 0x473BED: buffNum 0 is an empty list"),
    ),
    (262, 2040, Zero::Handled),
    (
        264,
        10080,
        Zero::Safe("ctl sub_4431E0 0x44323C: kind 0 is not registered"),
    ),
    (266, 200, Zero::Pending("D7")),
    (
        268,
        33600,
        Zero::Safe("ctl sub_443270 0x4432C7: kind other than 6 only logs"),
    ),
    (272, 9292, Zero::Handled),
    (
        274,
        1288,
        Zero::Safe("client sub_77F5C0: no old records to inherit"),
    ),
    (276, 6176, Zero::Handled),
    (278, 76, Zero::Handled),
    (280, 16, Zero::Handled),
    (
        282,
        8,
        Zero::Safe("client sub_787200 state 8: unreachable while 274 is empty"),
    ),
    (
        284,
        8,
        Zero::Safe("client sub_4BAD20: errCode only, the PIN is never read back"),
    ),
    (286, 704, Zero::Handled),
    (288, 12, Zero::Handled),
    (290, 12, Zero::Handled),
    (292, 20, Zero::Safe("client sub_4BB630: no caller")),
    (294, 8, Zero::Safe("client sub_4BB780: errCode only, logs")),
    (
        296,
        16,
        Zero::Safe("client sub_658910: failure screen, unreachable without a QR camera"),
    ),
    (
        298,
        16,
        Zero::Safe("client sub_658B00: failure screen, unreachable without a QR camera"),
    ),
    (300, 8, Zero::Pending("C6")),
    (
        302,
        8,
        Zero::Safe("ctl sub_4491B0: errCode 0 stores the result"),
    ),
    (
        304,
        8,
        Zero::Safe("client sub_604E10: errCode 0 deletes the dump"),
    ),
    (
        306,
        1004,
        Zero::Safe("client sub_7FBA50: teamNum 0 is a CPU team"),
    ),
    (308, 3332, Zero::Handled),
    (
        310,
        8,
        Zero::Safe(
            "ctl sub_45D840: errCode 0 stops the upload; the stand-in keeps the 309 for 312",
        ),
    ),
    (312, 3352, Zero::Handled),
    (
        314,
        8,
        Zero::Safe("client sub_7DD1B0 state 3: errCode 0 prints the card locally"),
    ),
    (
        316,
        8,
        Zero::Safe(
            "client sub_788550 state 3: errCode 0 consumes the credit and clears the queue; the stand-in keeps the purchase",
        ),
    ),
    (
        318,
        32,
        Zero::Safe("client sub_77C7D0: an empty mission is accepted"),
    ),
];

pub fn checksum(h: &[u8]) -> u8 {
    h.iter().enumerate().fold(
        0u8,
        |s, (i, b)| if i == 24 { s } else { s.wrapping_add(*b) },
    )
}

pub fn header(cmd: u16, seq: u16, status: u32, size: usize, conn: u32) -> [u8; 40] {
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

pub(super) fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

pub(super) fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

pub(super) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The revision the control writes into every request.
const REVISION: u32 = 47565;
/// The club's save block: SAVE_DATA_SET +28, SAVE_DATA +8.
pub(super) const BLOCK: usize = 3072;
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
        .as_chunks::<RECORD>()
        .0
        .iter()
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

/// Bytes in chunk `idx` of the control's 0x3DA38-byte download (cmd 261 -> 262, 2,032 per chunk).
fn live_chunk_size(idx: u32) -> u32 {
    0x3DA38u32
        .saturating_sub(idx.saturating_mul(2032))
        .min(2032)
}

/// SAVE_DATA_SET's server-side summary starts here; the store keeps +3,100..+8,572.
pub(super) const TAIL: usize = 3100;

/// A stored club: block, version, size, the whole stored 277 (empty = none since the `requests` table) and when it
/// was stored (0 = none).
type Saved = (Vec<u8>, u16, u16, Vec<u8>, i64);

/// Where a stored SAVE_DATA_SET (277, 8,572 bytes) feeds a SAVE_DATA answer (276): `(276 offset, 277 offset, length)`.
/// The client sets these from the answer even when zero (sub_81AC50 applies them, sub_817F10 writes the 277), so a
/// span left out puts that state back to its default at every card-in. 3,100..3,556 is the club's league state
/// (SERVER_CLUB_RL_PRESENT_DIVISION at 277 +5,888 -> 276 +3,508, the season count, matches played, results, W/D/L; a
/// zero state puts a club back into league 2, sub_83D4C0); 3,674..4,154 the 16 player records of 30 bytes (experience,
/// level, confidence, form); 6,094 the manager level; the rest is the server-side summary the notes list
/// (docs/research/ONLINE-2017-18.md, SAVE_DATA). Spans do not overlap and stay inside both messages.
pub const TAIL_SPANS: [(usize, usize, usize); 9] = [
    (3100, 5480, 456),
    (3556, 5944, 60),
    (3674, 6068, 480),
    (4158, 6552, 6),
    (4164, 6559, 1792),
    (5956, 8358, 60),
    (6094, 3262, 2),
    (6136, 8532, 12),
    (6152, 8544, 24),
];

/// Applies [`TAIL_SPANS`] from a stored 277 to a SAVE_DATA answer. No stored 277 (none since the club's last
/// version) is skipped.
fn restore_tail(body: &mut [u8], req: &[u8]) {
    if req.len() != 8572 {
        return;
    }
    for (to, from, len) in TAIL_SPANS {
        body[to..to + len].copy_from_slice(&req[from..from + len]);
    }
}

/// The 309 upload re-packed as a 312 (3,352 bytes, errCode left to the caller): the shop name (28 bytes, 309 +3,194)
/// at +120, the region at +148 (309 +3,224; -1 when ALL.Net sent 0x80000000 = none), the 3,072-byte save block
/// (309 +8) at +152, and the 128 bytes of CPU attributes (309 +3,232) at +3,224. +8 and +64 (names the client does
/// not read) stay 0. `req` is a whole 3,368-byte 309.
fn best_eleven_answer(body: &mut [u8], req: &[u8]) {
    body[120..148].copy_from_slice(&req[3194..3222]);
    let region = u32_at(req, 3224);
    put(
        body,
        148,
        if region == 0x8000_0000 {
            u32::MAX
        } else {
            region
        },
    );
    body[152..152 + BLOCK].copy_from_slice(&req[8..8 + BLOCK]);
    body[3224..3352].copy_from_slice(&req[3232..3360]);
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

/// COACH_UNIVERSAL_CHAMPIONSHIP_ENTRY_DATE (client parameter #3676), YYYYMMDD in 25 bits of the save block. A Universal
/// Cup win sets it (client sub_8277A0, 0x828183) and MATCH SELECT greys the cup while it is not 0 (sub_7983F0 at
/// 0x798573, sub_795BC0 at 0x796455: "Data match entry is in progress", SYS_ADV_normal_0037). Only a championship
/// result clears it. Seen on 2026-11-06: a club that won on 11-01 held 20261101 at block bytes 2,883..2,886 (its 249
/// sent the same value at +32), and 0 there in the save after a play with the +354 RANK answer.
const UC_ENTRY_DATE_BIT: usize = 23_071;

/// Clears that date (25 bits) in a stored save block: what a 276 answer sends, so MATCH SELECT offers the cup again.
pub fn clear_uc_entry_date(block: &mut [u8]) {
    put_bits(block, UC_ENTRY_DATE_BIT, 25, 0);
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
pub(super) fn put_bits(block: &mut [u8], bit: usize, width: usize, value: u32) {
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
pub(super) fn get_bits(block: &[u8], bit: usize, width: usize) -> u32 {
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
/// The second field is the Grand Trophy week each (Aime id, coachId) carded in (275) on this run of the server, and
/// whether a save after the roll has already filed that week's record.
pub struct Store(pub(super) Connection, HashMap<(u32, u32), (u16, bool)>);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS coaches (
    aime_id    INTEGER NOT NULL,
    coach_id   INTEGER NOT NULL UNIQUE,
    card_id    INTEGER NOT NULL UNIQUE,
    name1      BLOB    NOT NULL DEFAULT x'', -- record +4, SAVE_DATA_SET +3,104 (56 raw bytes)
    name2      BLOB    NOT NULL DEFAULT x'', -- record +64, SAVE_DATA_SET +3,160
    rec122     INTEGER NOT NULL DEFAULT 0,   -- record +122, SAVE_DATA_SET +3,240
    play_count INTEGER NOT NULL DEFAULT 0,   -- record +123
    titles     INTEGER NOT NULL DEFAULT 0,   -- record +124, SAVE_DATA_SET +3,216 (competition mask; 0 kept)
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
    tail     BLOB    NOT NULL DEFAULT x'', -- SAVE_DATA_SET +3,100..+8,572 (server-side summary); empty = none yet
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
-- every accepted BUY_SHOP_ITEM 315, whole; per Aime id (the request carries no club), kept for good
CREATE TABLE IF NOT EXISTS purchases (
    aime_id INTEGER NOT NULL,
    at      INTEGER NOT NULL,
    payload BLOB    NOT NULL
);
-- an arcade guest's club imported for the session (online::club): its Aime id, the token that owns it, its seat
CREATE TABLE IF NOT EXISTS guests (
    aime_id INTEGER PRIMARY KEY,
    token   TEXT    NOT NULL,
    seat    INTEGER NOT NULL,
    since   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS requests (
    aime_id  INTEGER NOT NULL,
    coach_id INTEGER NOT NULL,  -- 0 for a request kept per shop
    cmd      INTEGER NOT NULL,
    payload  BLOB    NOT NULL,  -- the whole request, as the client sent it
    updated  INTEGER NOT NULL,
    PRIMARY KEY (aime_id, coach_id, cmd)
);
-- each card's best Grand Trophy record (277 +5,572) per week of `gt_week`, on a server with VS (the shared
-- ranking); `at` = when the record last grew, the tie-break
CREATE TABLE IF NOT EXISTS gt_points (
    week    INTEGER NOT NULL,
    aime_id INTEGER NOT NULL,
    points  INTEGER NOT NULL,
    at      INTEGER NOT NULL,
    PRIMARY KEY (week, aime_id)
);
-- the Grand Trophy promotion issued at a card-in of `week` from the card's record of week - 1: one per week and
-- card, to the club `coach_id` that carded in, which moved up to `class`; `rank` and `record` are the closed week's
-- at the grant, so a re-issue sends the same result; `created` is the club's (coaches.created), the one club field an
-- import keeps (online::club gives it new ids)
CREATE TABLE IF NOT EXISTS gt_promotions (
    week     INTEGER NOT NULL,
    aime_id  INTEGER NOT NULL,
    coach_id INTEGER NOT NULL,
    class    INTEGER NOT NULL,
    at       INTEGER NOT NULL,
    rank     INTEGER NOT NULL,
    record   INTEGER NOT NULL,
    created  INTEGER NOT NULL,
    PRIMARY KEY (week, aime_id)
);
";

/// `PRAGMA user_version` of a store this code has migrated.
const SCHEMA_VERSION: i64 = 4;

/// The 0 -> 1 backfill of [`migrate`]: a club saved before `requests` existed keeps only its tail in `saves`, so its
/// 277 is rebuilt from the row (ids at +4/+8/+12, kind at +24, block at +28, tail at +3,100) and the first card-in
/// already sends the stored levels and league state. Adds rows only; `INSERT OR IGNORE` leaves any request already
/// there alone.
fn backfill(tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
    type Row = (u32, u32, u32, u8, Vec<u8>, Vec<u8>, i64);
    let rows: Vec<Row> = tx
        .prepare(
            "SELECT s.aime_id, s.coach_id, c.card_id, s.kind, s.block, s.tail, s.updated
             FROM saves s JOIN coaches c USING (aime_id, coach_id) WHERE length(s.tail) = ?1",
        )?
        .query_map([(8572 - TAIL) as i64], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (aime, coach, card, kind, block, tail, updated) in rows {
        if block.len() != BLOCK {
            continue;
        }
        let mut req = vec![0u8; 8572];
        put(&mut req, 4, aime);
        put(&mut req, 8, coach);
        put(&mut req, 12, card);
        req[24] = kind;
        req[28..28 + BLOCK].copy_from_slice(&block);
        req[TAIL..].copy_from_slice(&tail);
        tx.execute(
            "INSERT OR IGNORE INTO requests VALUES (?1, ?2, 277, ?3, ?4)",
            params![aime, coach, req, updated],
        )?;
    }
    Ok(())
}

/// `PRAGMA user_version` 0 -> 1 runs [`backfill`]; 1 -> 2 only adds `purchases` (`CREATE TABLE IF NOT EXISTS` in
/// [`SCHEMA`], every open); 2 -> 3 adds `gt_points` the same way and fills it from the stored 277s, each under the
/// week it was saved in; 3 -> 4 only adds `gt_promotions` (`CREATE TABLE IF NOT EXISTS`). One transaction; nothing
/// is dropped or rewritten, and an older server ignores the tables.
fn migrate(db: &mut Connection) -> rusqlite::Result<()> {
    let version = db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    let tx = db.transaction()?;
    if version < 1 {
        backfill(&tx)?;
    }
    if version < 3 {
        let saves: Vec<(u32, Vec<u8>, i64)> = tx
            .prepare(
                "SELECT aime_id, payload, updated FROM requests WHERE cmd = 277 AND length(payload) = 8572
                 ORDER BY updated",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (aime, req, at) in saves {
            record_gt(&tx, gt_week(at), aime, &req, at)?;
        }
    }
    tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    tx.commit()
}

/// Next id from counter `name` (1, 2, ...), inside the caller's transaction.
pub(super) fn next_id(db: &Connection, name: &str) -> rusqlite::Result<u32> {
    db.query_row(
        "INSERT INTO ids VALUES (?1, 1) ON CONFLICT (name) DO UPDATE SET last = last + 1 RETURNING last",
        [name],
        |r| r.get(0),
    )
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let mut db = Connection::open(path)?;
        // WAL: a commit appends to gamesvr.db-wal; NORMAL syncs at checkpoints, so a power cut can lose the last
        // commits but never corrupts the store
        db.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        // the arcade host's desk writes guests' clubs from another process: take the write lock at BEGIN, so a
        // read-then-write transaction waits for the other (busy timeout) instead of failing on a stale snapshot
        db.set_transaction_behavior(TransactionBehavior::Immediate);
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
        // stores made before `tail` existed
        let has: bool = db.query_row(
            "SELECT count(*) > 0 FROM pragma_table_info('saves') WHERE name = 'tail'",
            [],
            |r| r.get(0),
        )?;
        if !has {
            db.execute_batch("ALTER TABLE saves ADD COLUMN tail BLOB NOT NULL DEFAULT x''")?;
        }
        migrate(&mut db)?;
        Ok(Self(db, HashMap::new()))
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
            "INSERT INTO saves VALUES (?1, ?2, ?3, 12, 3072, 1, ?4, x'')",
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
             titles = CASE ?9 WHEN 0 THEN titles ELSE ?9 END, updated = ?8
             WHERE aime_id = ?1 AND coach_id = ?2 AND card_id = ?3",
            params![
                aime,
                coach,
                card,
                &req[3104..3160],
                &req[3160..3216],
                req[3240],
                u8::from(kind == 2),
                now(),
                // +3,216: the club's competition mask, which the client builds from save parameter 3512 and sends 0
                // while its tutorial runs (sub_817F10); the club list shows "Tutorial" while record +124 has none
                // of its bits (sub_792740). 0 keeps what is stored: a skip club's TitleHolder bit.
                u32_at(req, 3216)
            ],
        )?;
        if n == 0 {
            return Ok(false);
        }
        tx.execute(
            "INSERT OR REPLACE INTO saves VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                aime,
                coach,
                &req[28..28 + BLOCK],
                u16_at(req, 3100),
                u16_at(req, 3102),
                kind,
                now(),
                &req[TAIL..8572]
            ],
        )?;
        // the whole request, which the next SAVE_DATA reads back (`saves.tail` stays written for an older server)
        tx.execute(
            "INSERT OR REPLACE INTO requests VALUES (?1, ?2, 277, ?3, ?4)",
            params![aime, coach, &req[..8572], now()],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// The stored (block, version, size, 277, when) of a club.
    fn save(&self, aime: u32, coach: u32, card: u32) -> rusqlite::Result<Option<Saved>> {
        self.0
            .query_row(
                "SELECT s.block, s.version, s.size, r.payload, r.updated FROM saves s JOIN coaches c USING (aime_id, coach_id)
                 LEFT JOIN requests r ON r.aime_id = s.aime_id AND r.coach_id = s.coach_id AND r.cmd = 277
                 WHERE s.aime_id = ?1 AND s.coach_id = ?2 AND c.card_id = ?3",
                params![aime, coach, card],
                |r| {
                    let req: Option<Vec<u8>> = r.get(3)?;
                    let at: Option<i64> = r.get(4)?;
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        req.unwrap_or_default(),
                        at.unwrap_or(0),
                    ))
                },
            )
            .optional()
    }

    /// The Grand Trophy ranking of `week`, first place first: (Aime id, record). Ties go to the record set earlier,
    /// then to the lower Aime id.
    fn gt_ranking(&self, week: u16) -> rusqlite::Result<Vec<(u32, i32)>> {
        self.0
            .prepare(
                "SELECT aime_id, points FROM gt_points WHERE week = ?1 ORDER BY points DESC, at, aime_id",
            )?
            .query_map([week], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    }

    /// The promotion a card-in of `week` gives club `coach` of `aime`, whose stored 277 is `saved`: (class, the
    /// card's rank in week - 1, its record) when that record reached [`GT_RAISE`] of the club's class and no club of
    /// the card has had one this week. Stored before it is returned, so it is granted once per week and card; the
    /// granted club gets the stored result again at each card-in of the week while its stored 277 is still one class
    /// below (a lost 276, or a session that ended before the info scene applied it). A club imported again (a guest's
    /// next join, online::club) has a new coachId and the granted one is gone: the row moves to it when it is the same
    /// club (same `created`), never to another club of the card. The club's latest grant of an earlier week is the
    /// floor of its saved class: a save that fell back below it is promoted from the granted class, or gets that
    /// grant's result again when this week brings no promotion.
    fn promote(
        &self,
        week: u16,
        aime: u32,
        coach: u32,
        saved: &[u8],
    ) -> rusqlite::Result<Option<(u8, u32, i32)>> {
        let (Some(last), Some(&class)) = (week.checked_sub(1), saved.get(GT_CLASS_277)) else {
            return Ok(None);
        };
        if saved.len() != 8572 {
            return Ok(None);
        }
        let granted: Option<(u32, u8, u32, i32)> = self
            .0
            .query_row(
                "SELECT coach_id, class, rank, record FROM gt_promotions WHERE week = ?1 AND aime_id = ?2",
                params![week, aime],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some((to, next, rank, record)) = granted {
            // the same result again, only to the club it went to (or its re-import) and only while it has not
            // applied it
            if class >= next {
                return Ok(None);
            }
            if to != coach
                && self.0.execute(
                    "UPDATE gt_promotions SET coach_id = ?3 WHERE week = ?1 AND aime_id = ?2
                     AND created = (SELECT created FROM coaches WHERE coach_id = ?3)
                     AND NOT EXISTS (SELECT 1 FROM coaches WHERE coach_id = ?4)",
                    params![week, aime, coach, to],
                )? == 0
            {
                return Ok(None);
            }
            // the granted class itself: a save more than one class below must not land one below the grant
            return Ok(Some((next - 1, rank, record)));
        }
        // the same club as at the current week's re-issue: its coach, or its re-import once that coach is gone
        let floor: Option<(u8, u32, i32)> = self
            .0
            .query_row(
                "SELECT class, rank, record FROM gt_promotions p WHERE aime_id = ?1 AND week < ?2
                 AND (coach_id = ?3 OR (created = (SELECT created FROM coaches WHERE coach_id = ?3)
                      AND NOT EXISTS (SELECT 1 FROM coaches c WHERE c.coach_id = p.coach_id)))
                 ORDER BY week DESC LIMIT 1",
                params![aime, week, coach],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let resend = floor
            .filter(|f| class < f.0)
            .map(|(next, rank, record)| (next - 1, rank, record));
        let class = floor.map_or(class, |f| class.max(f.0));
        let ranking = self.gt_ranking(last)?;
        let entry = ranking.iter().enumerate().find(|(_, e)| e.0 == aime);
        let need = GT_RAISE.get(usize::from(class)).copied().unwrap_or(0);
        let Some((i, &(_, record))) = entry else {
            return Ok(resend);
        };
        if need == 0 || i64::from(record) < i64::from(need) {
            return Ok(resend);
        }
        let n = self.0.execute(
            "INSERT INTO gt_promotions (week, aime_id, coach_id, class, at, rank, record, created)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, created FROM coaches WHERE coach_id = ?3
             ON CONFLICT DO NOTHING",
            params![week, aime, coach, class + 1, now(), i as u32 + 1, record],
        )?;
        Ok((n == 1).then_some((class, i as u32 + 1, record)))
    }

    /// Keeps the latest whole 309 under (0, shop id).
    fn keep_best_eleven(&self, shop: u32, req: &[u8]) -> rusqlite::Result<()> {
        self.0.execute(
            "INSERT OR REPLACE INTO requests VALUES (0, ?1, 309, ?2, ?3)",
            params![shop, req, now()],
        )?;
        Ok(())
    }

    /// The newest stored 309 of any shop. This stand-in serves one shop, so "the same shop's last 309" (project
    /// decision D3, default) is the newest one; on a server with several shops a rule per area replaces this.
    fn last_best_eleven(&self) -> rusqlite::Result<Option<Vec<u8>>> {
        self.0
            .query_row(
                "SELECT payload FROM requests WHERE cmd = 309 AND aime_id = 0 ORDER BY updated DESC, coach_id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
    }

    /// Keeps a BUY_SHOP_ITEM request whole; Ok only once the insert has committed.
    fn buy(&self, aime: u32, req: &[u8]) -> rusqlite::Result<()> {
        self.0.execute(
            "INSERT INTO purchases (aime_id, at, payload) VALUES (?1, ?2, ?3)",
            params![aime, now(), req],
        )?;
        Ok(())
    }

    /// The (category, item) of every purchase of `aime` (request +8 and +12), oldest first.
    fn bought(&self, aime: u32) -> rusqlite::Result<Vec<(u32, u32)>> {
        let mut q = self
            .0
            .prepare("SELECT payload FROM purchases WHERE aime_id = ?1 ORDER BY rowid")?;
        let rows = q.query_map([aime], |r| r.get::<_, Vec<u8>>(0))?;
        Ok(rows
            .filter_map(|p| p.ok().filter(|p| p.len() >= 16))
            .map(|p| (u32_at(&p, 8), u32_at(&p, 12)))
            .collect())
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
#[allow(clippy::too_many_arguments)] // `gt` (the Grand Trophy ranking) added the eighth
fn club(
    cmd: u16,
    req: &[u8],
    body: &mut [u8],
    store: &Mutex<Store>,
    skip: Option<&Path>,
    rare: &[u32],
    gt: bool,
    log: &Log,
) -> Result<(), String> {
    // request size and the revision's offset (docs/research/ONLINE-2017-18.md, "M1")
    let (need, rev_at) = match cmd {
        271 => (20, Some(16)),
        275 => (32, Some(28)),
        277 => (8572, Some(5940)),
        279 => (36, Some(32)),
        287 | 289 => (28, Some(24)),
        285 => (24, Some(20)),
        // BUY_SHOP_ITEM: ids, category and item; the replay records are 20 bytes, so no revision check
        315 => (16, None),
        // BEST_ELEVEN upload (the control's, no revision) and download (areaId at +4, not read)
        309 => (3368, None),
        311 => (0, None),
        _ => return Ok(()),
    };
    if req.len() < need {
        return Err(format!("{} bytes, want {need}", req.len()));
    }
    if let Some(at) = rev_at.filter(|&at| u32_at(req, at) != REVISION) {
        return Err(format!("revision {}, want {REVISION}", u32_at(req, at)));
    }
    // 311 may be as short as 8 bytes: a missing id reads as 0
    let id = |at: usize| req.get(at..at + 4).map_or(0, |_| u32_at(req, at));
    let (aime, coach, card) = (id(4), id(8), id(12));
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
            // the server pre-registers every 2D-code card 3 times (registrations 3, owned 0, flag 0): there is no
            // 2D-code reader to register them one by one, and 3 is the most the client's list offers (sub_4DFFB0);
            // special coaching adds 5 per copy past the first and does not clamp, so never more than 3. The list
            // holds 1,000 distinct cards; the card master has 943 (docs/research/ONLINE-2017-18.md, "2D-code cards")
            for (e, &no) in body[PLAYERS_LIST..]
                .as_chunks_mut::<8>()
                .0
                .iter_mut()
                .zip(rare)
            {
                put(e, 0, no);
                e[4] = 3;
            }
        }
        return Ok(());
    }
    match cmd {
        // +4 is the shop id here, not an Aime id
        309 => {
            st.keep_best_eleven(aime, req).map_err(db)?;
            return Ok(());
        }
        // errCode 0 with the newest 309 re-packed; none stored keeps the errCode 1 set by `answer` (CPU team)
        311 => {
            if let Some(p) = st.last_best_eleven().map_err(db)? {
                put(body, 4, 0);
                best_eleven_answer(body, &p);
            }
            return Ok(());
        }
        _ => {}
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
            let before = st
                .save(aime, coach, card)
                .map_err(db)?
                .map_or("-".into(), |(block, ..)| team_tag(&block));
            let t = now();
            let week = gt_week(t);
            // a play carded in before the Sunday 23:00 roll grew its card-in week's record: file the record there and
            // store the save without its weekly fields, so the club starts the new week from zero
            let carded = gt
                .then(|| st.1.get(&(aime, coach)).copied())
                .flatten()
                .filter(|&(w, _)| w < week);
            let reset: Vec<u8>;
            let stored = match carded {
                Some(_) => {
                    let mut r = req[..8572].to_vec();
                    for (at, len) in GT_WEEKLY_276 {
                        r[at + 2380..at + 2380 + len].fill(0);
                    }
                    reset = r;
                    &reset[..]
                }
                None => req,
            };
            if !st.set_save(aime, coach, card, stored).map_err(db)? {
                return Err(format!("no club aime {aime} coach {coach} card {card}"));
            }
            log(&format!(
                "club save: aime {aime} coach {coach} card {card} team {before} -> {}",
                team_tag(&req[28..28 + BLOCK])
            ));
            // after the roll the closed week's record is filed once; later saves of the session file nothing
            if gt && !matches!(carded, Some((_, true))) {
                // the save is stored: a ranking error is logged, never answered as a refused save
                let (week, points) = (carded.map_or(week, |(w, _)| w), u32_at(req, GT_RECORD_277));
                let kept = record_gt(&st.0, week, aime, req, t);
                // filed only once written: after a store error the session's next save files it again
                if let (Ok(()), Some((w, _))) = (&kept, carded) {
                    st.1.insert((aime, coach), (w, true));
                }
                match kept {
                    Err(e) => log(&format!(
                        "grand trophy: aime {aime} coach {coach} record {points} not kept: {e}"
                    )),
                    // an s32 field: 0 and negatives are no record, only 100,001..i32::MAX is an anomaly
                    Ok(()) if (GT_RECORD_MAX + 1..0x8000_0000).contains(&points) => log(&format!(
                        "grand trophy: aime {aime} coach {coach} record {points} refused, above {GT_RECORD_MAX}"
                    )),
                    Ok(()) if (1..=GT_RECORD_MAX).contains(&points) => log(&format!(
                        "grand trophy: aime {aime} coach {coach} week {week} record {points}"
                    )),
                    Ok(()) => {}
                }
            }
            put(body, 8, coach);
        }
        275 => {
            let (mut block, version, size, tail, saved_at) = st
                .save(aime, coach, card)
                .map_err(db)?
                .ok_or(format!("no save for aime {aime} coach {coach} card {card}"))?;
            if block.len() != BLOCK {
                return Err(format!("stored block of {} bytes", block.len()));
            }
            // no championship runs here, so no club is entered in one: the client reads this at MATCH SELECT, before
            // the RANK answer's reset (+354) runs at the play's start
            clear_uc_entry_date(&mut block);
            restore_tail(body, &tail);
            let week = gt_week(now());
            if gt {
                // shortcut: cleared when full and lost on restart (then a play across the roll files its record in
                // the new week); keep it in the store if that matters
                if st.1.len() >= 10_000 {
                    st.1.clear();
                }
                st.1.insert((aime, coach), (week, false));
                // the ranking is a display: a read error shows "-----", it never fails the card-in
                let ranking = st.gt_ranking(week).unwrap_or_else(|e| {
                    log(&format!(
                        "grand trophy: ranking of week {week} not read: {e}"
                    ));
                    Vec::new()
                });
                let rank = ranking
                    .iter()
                    .position(|&(a, _)| a == aime)
                    .map_or(0, |i| i as u32 + 1);
                if gt_week(saved_at) > week {
                    log(&format!(
                        "grand trophy: aime {aime} coach {coach} save stamped in week {}, now {week}: weekly fields reset",
                        gt_week(saved_at)
                    ));
                }
                fill_gt_week(body, gt_week(saved_at), week, rank);
                log(&format!(
                    "grand trophy: aime {aime} coach {coach} week {week} rank {rank} of {}",
                    ranking.len()
                ));
            }
            apply_to_save_data(body, &st.bought(aime).map_err(db)?);
            if gt {
                // a store error skips the promotion this card-in; a granted one whose 276 is lost goes out again at
                // the club's next card-in of the week (promote)
                let promoted = st.promote(week, aime, coach, &tail).unwrap_or_else(|e| {
                    log(&format!(
                        "grand trophy: aime {aime} coach {coach} promotion not checked: {e}"
                    ));
                    None
                });
                if let Some((class, rank, record)) = promoted {
                    log(&format!(
                        "grand trophy: aime {aime} coach {coach} week {week} class {class} -> {} (record {record}, rank {rank})",
                        class + 1
                    ));
                }
                fill_gt_result(body, promoted.map(|(class, rank, _)| (class, rank)));
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
            // the purchases of this Aime id, whichever club asks
            apply_to_item_list(body, &st.bought(aime).map_err(db)?);
        }
        315 => {
            // BUY_SHOP_ITEM: +8 category, +12 item; kept whole, errCode 0 only once committed (a store error is
            // errCode 1 and the client keeps the item queued and resends it)
            st.buy(aime, req).map_err(db)?;
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

/// Tests elsewhere in `online` (club): one request answered from `store` with no VS and no skip club; the answer
/// and the store's refusal.
#[cfg(test)]
pub(super) fn ask_for_test(
    store: &Mutex<Store>,
    cmd: u16,
    p: &[u8],
    log: &Log,
) -> (Vec<u8>, Option<String>) {
    let (_, body, why) = answer(cmd, p, store, None, &[], None, log).unwrap();
    if let Some(line) = match_line(cmd, p, why.as_deref()) {
        log(&line);
    }
    (body, why)
}

/// The response to one request: (command, payload, the store's refusal if any), or `None` for a command the table
/// does not know.
fn answer(
    cmd: u16,
    payload: &[u8],
    store: &Mutex<Store>,
    skip: Option<&Path>,
    rare: &[u32],
    vs: Option<(&Vs, IpAddr)>,
    log: &Log,
) -> Option<(u16, Vec<u8>, Option<String>)> {
    if cmd == 18 {
        // GMSVR_ECHO keep-alive
        let mut b = payload[..payload.len().min(4)].to_vec();
        b.resize(4, 0);
        return Some((18, b, None));
    }
    let rcmd = cmd.wrapping_add(1);
    let size = RESP.iter().find(|(c, ..)| *c == rcmd)?.1;
    let mut body = vec![0u8; size];
    if size >= 4 && payload.len() >= 4 {
        body[..4].copy_from_slice(&payload[..4]);
    }
    if cmd == 237 {
        // the control copies payload +16.. as GAME_CTRL (sub_4419F0) and sends it to every seat, over the client's
        // defaults; all zero leaves no secretary, and the club's SECRETARY step loads a picture that is not on the
        // disc and reads a null pointer (client 0x424AB2, docs/research/ONLINE-2017-18.md)
        fill_game_ctrl(&mut body, now());
    }
    if cmd == 261 && payload.len() >= 8 {
        // the control downloads a 0x3DA38-byte buffer in 2,032-byte chunks (request +4 = chunk) and copies the answer
        // +4 count of bytes from +8 of each (sub_443090); it never zeroes the buffer, so size 0 left heap garbage that
        // it later read as clubs and crashed on (error 3000 on the seats). Full sizes copy zeros: an empty list, CPU
        // teams. The last chunk is short: the control does not bound the copy. The buffer is the data-match replay
        // feed (VtrPlayData, control sub_47AFE0): an all-zero 6,480-byte match at +58 of each 8,602-byte round
        // (`live_game_at`) reads as "LiveGameInit bin is empty", so the projector gets no replays. Its CPU
        // championship is the control's own CPU-filled Universal Championship; with WCCF_UCLIVE=on the hook keeps it
        // off the projector unless an arcade player's club is in it (native/hook/src/uclive.rs).
        put(&mut body, 4, live_chunk_size(u32_at(payload, 4)));
    }
    if matches!(cmd, 307 | 311) {
        // GHOST_TEAM, BEST_ELEVEN: errCode 0 would install an all-zero team; 1 = none, the client plays a CPU team
        // (docs/research/ONLINE-2017-18.md)
        put(&mut body, 4, 1);
    }
    if cmd == 249 {
        // GMSVR_RANK: the whole answer is copied into the game state and drives the result and prize screens
        // (docs/research/ONLINE-2017-18.md, "GMSVR_RANK"); +354 is `universalChamp nothingResult`. At 1 the play's
        // start (client sub_60C9C0 at 0x60ed96, "UCSEntryReset") clears a Universal Cup winner's championship entry
        // (UC_ENTRY_DATE_BIT) and the end-of-play save keeps it cleared; the 276 clears it for MATCH SELECT, which
        // comes before that reset.
        body[354] = 1;
        // +12 is dataVSWeek (s16). The client keeps it (client 0x805175) and sends a Universal Cup winner's
        // DATA_VS_ENTRY only while it is not 0 (0x80EE60 "DVSES_DATA_VS_WEEK_IS_ZERO", 0x80EDF0). Without that entry
        // the control's championship block has no hasDvs flag and the control swaps the winner for a CPU team
        // ("change cpu!", control 0x47CA30; the entry sets it at 0x435960 -> 0x47C620)
        let week = gt_week(now()).max(1);
        body[12..14].copy_from_slice(&week.to_le_bytes());
        // one line per card-in: a player's logs then show the week the cup winner's entry depends on
        log(&format!(
            "rank answer (250): dataVSWeek {week} - a Universal Cup winner's client sends DATA_VS_ENTRY to the control"
        ));
    }
    if let Some((vs, peer)) = vs {
        vs.fill(cmd, payload, peer, &mut body);
    }
    // the weekly Grand Trophy ranking is one list for every player of a server, so only a server with VS (the one
    // players across the internet share) keeps it; an arcade's stand-in leaves the client's fields as they come
    let refused = club(
        cmd,
        payload,
        &mut body,
        store,
        skip,
        rare,
        vs.is_some(),
        log,
    )
    .err();
    if refused.is_some() {
        put(&mut body, 4, 1);
    }
    Some((rcmd, body, refused))
}

/// A 56-byte name of the game's records: up to its NUL, Shift-JIS (without the `full` feature its bytes as UTF-8).
pub(super) fn name_text(b: &[u8]) -> String {
    let b = b.split(|&c| c == 0).next().unwrap_or(&[]);
    #[cfg(feature = "full")]
    let t = encoding_rs::SHIFT_JIS.decode_without_bom_handling(b).0;
    #[cfg(not(feature = "full"))]
    let t = String::from_utf8_lossy(b);
    t.trim().to_owned()
}

/// The log's match lines, CPU and VS matches alike (docs/research/ONLINE-2017-18.md): INJUSTICE_COUNT 289 with +1 is a
/// play's start (sub_785BA0); SAVE_DATA_SET 277 its end, with the opponent (a CPU club has coachId 0), the score and
/// whether the store took the result. [`Session::request`] logs them with the seat's mode.
pub(super) fn match_line(cmd: u16, req: &[u8], refused: Option<&str>) -> Option<String> {
    let id = |at| u32_at(req, at);
    // goals and PKs are single bytes: the bytes after them belong to other fields (owner run logs showed packed u32s
    // such as 2950146 = 0x2D0502 for a 2 goal score)
    match cmd {
        289 if req.len() >= 20 && id(16) == 1 => Some(format!(
            "match start: aime {} coach {} card {}",
            id(4),
            id(8),
            id(12)
        )),
        277 if req.len() >= 8572 => Some(format!(
            "match end: aime {} coach {} vs {:?} (manager {:?}, coach {}) score {}-{} pk {}-{}, save kind {}: \
             result save {}",
            id(4),
            id(8),
            name_text(&req[3356..3412]),
            name_text(&req[3300..3356]),
            id(3296),
            req[3416],
            req[3428],
            req[3424],
            req[3436],
            req[24],
            refused.map_or("ok".into(), |why| format!("FAILED ({why})"))
        )),
        _ => None,
    }
}

/// The VS switch (M3): the reflect address 202 hands out and the entry matcher.
struct Vs {
    cfg: VsConfig,
    matcher: Arc<Matcher>,
}

impl Vs {
    /// 202, 224, 238, 239, 243, 245, 257 and the vslink 901, 903, 905 answers (the slot echo is in `body` already).
    fn fill(&self, cmd: u16, payload: &[u8], peer: IpAddr, body: &mut [u8]) {
        let t = Instant::now();
        match cmd {
            201 if self.matcher.capable(peer, t) => vs::fill_login_loopback(body, &self.cfg),
            201 => vs::fill_login(body, &self.cfg),
            // vslink (docs/ARCHITECTURE.md "Online"): status 1 = accepted
            // +36..+52: the lobby VS room's ticket, when the run has one; +52..+72 the det-math token, from newer
            // vslinks (vs::hello_payload). Status 2 (to a hello with a token only): its room partner's engine differs
            900 if payload.len() >= 36 => {
                let id = payload[4..36].try_into().unwrap();
                let field = |r| {
                    payload
                        .get(r)
                        .map(|b: &[u8]| {
                            String::from_utf8_lossy(b.split(|c| *c == 0).next().unwrap_or(&[]))
                                .into_owned()
                        })
                        .filter(|s| !s.is_empty())
                };
                let (ticket, dmath) = (field(vs::HELLO_TICKET), field(vs::HELLO_DMATH));
                let differs = self
                    .matcher
                    .hello(peer, id, ticket.as_deref(), dmath.as_deref(), t);
                put(
                    body,
                    4,
                    if differs && dmath.is_some() {
                        vs::HELLO_DMATH_DIFFERS
                    } else {
                        1
                    },
                );
            }
            902 if payload.len() >= 40 => {
                let id = payload[8..40].try_into().unwrap();
                let ok = self
                    .matcher
                    .register(u32_at(payload, 0), u16_at(payload, 4), id, t);
                put(body, 4, u32::from(ok));
            }
            904 if payload.len() >= 6 => {
                if let Some((tok, id)) =
                    self.matcher
                        .lookup(u32_at(payload, 0), u16_at(payload, 4), t)
                {
                    put(body, 4, 1);
                    body[8..14].copy_from_slice(&tok);
                    body[16..48].copy_from_slice(&id);
                }
            }
            // the data battle entry path: the control reads the ids it sent back (ctl sub_447DF0, sub_448FD0); a 0 id is
            // its own SvrErr path
            239 if payload.len() >= 12 => body[4..12].copy_from_slice(&payload[4..12]),
            257 if payload.len() >= 8 => {
                put(body, 4, u32_at(payload, 4));
                put(body, 8, u32::from(u32_at(payload, 4) != 0));
            }
            223 => vs::fill_round(body, now()),
            237 => {
                vs::fill_game_ctrl(body);
                // promotion runs on a server with VS only, so an arcade's stand-in shows no threshold
                for (c, &p) in GT_RAISE.iter().enumerate() {
                    put(body, GT_RAISE_AT + 4 * c, p);
                }
            }
            243 => {
                self.matcher.enter(payload, t);
            }
            245 => {
                if let Some(m) = self.matcher.poll(payload, t) {
                    body[4..].copy_from_slice(&m[4..]);
                }
            }
            _ => {}
        }
    }
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
/// so the mode screen offers NORMAL. Request dumps on, no connection limit (the Launcher's local stand-in).
pub fn spawn_with(
    listener: TcpListener,
    save_dir: PathBuf,
    log: Log,
    game: Option<PathBuf>,
    skip_tutorial: bool,
) -> io::Result<Handle> {
    let o = Options {
        game,
        skip_tutorial,
        ..Options::default()
    };
    spawn_opts(listener, save_dir, log, o)
}

/// What [`spawn_with`] takes, plus the server's switches.
#[derive(Clone)]
pub struct Options {
    /// Peers it returns false for are closed at accept (None = everyone)
    pub allow: Option<Arc<dyn Fn(IpAddr) -> bool + Send + Sync>>,
    pub game: Option<PathBuf>,
    pub skip_tutorial: bool,
    /// Write each request payload to `save_dir/req_<cmd>_<ms>.bin` (last 200 kept)
    pub dump: bool,
    /// Concurrent connections per peer IP; 0 = no limit
    pub max_per_ip: usize,
    /// Direct VS over the internet (M3); None keeps the local stand-in's behaviour
    pub vs: Option<VsConfig>,
    /// The matcher VS uses, when another service shares it (`forza-server all`: the desk lists its rooms); None =
    /// a new one
    pub matcher: Option<Arc<Matcher>>,
    /// Who is connected, when another service shares it (`forza-server all`: the desk counts the cards in its lobby
    /// row); None = a new one
    pub sessions: Option<Arc<Sessions>>,
    /// GAME_CTRL 1172 = 0 in answer 238: the control never inserts a cup at its open times, so it never counts four cups
    /// held and never opens the Universal Championship; cups come only from `schedule_option.conf` (the ovh1 hourly
    /// arcade, crates/linux-host `cups`)
    pub no_timed_cups: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            allow: None,
            game: None,
            skip_tutorial: false,
            dump: true,
            max_per_ip: 0,
            vs: None,
            matcher: None,
            sessions: None,
            no_timed_cups: false,
        }
    }
}

/// Largest legal frame: the biggest answer or request payload plus the 40-byte header.
pub const MAX_FRAME: usize = 33600 + 40;

/// Frame length from a header (floored at 40: the Python hangs below it), or the oversized value it claims.
fn frame_len(header: &[u8]) -> Result<usize, usize> {
    let n = (u32_at(header, 16) as usize).max(40);
    if n > MAX_FRAME { Err(n) } else { Ok(n) }
}

/// Fills `buf` from the link; false when the peer closed (or `stop` is set) first.
fn fill(conn: &mut TcpStream, buf: &mut [u8], stop: &AtomicBool) -> io::Result<bool> {
    let mut at = 0;
    while at < buf.len() {
        match read_some(conn, &mut buf[at..], stop)? {
            0 => return Ok(false),
            n => at += n,
        }
    }
    Ok(true)
}

/// Header and body in one vectored write (one segment, no copy into a frame buffer).
fn send(conn: &mut TcpStream, head: &[u8], body: &[u8]) -> io::Result<()> {
    let n = conn.write_vectored(&[IoSlice::new(head), IoSlice::new(body)])?;
    if n < head.len() {
        conn.write_all(&head[n..])?;
        conn.write_all(body)
    } else {
        conn.write_all(&body[n - head.len()..])
    }
}

struct Slot<'a>(&'a Mutex<HashMap<IpAddr, usize>>, IpAddr);
impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut m = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = m.get_mut(&self.1) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.1);
            }
        }
    }
}

/// [`spawn_with`] with [`Options`].
pub fn spawn_opts(
    listener: TcpListener,
    save_dir: PathBuf,
    log: Log,
    opts: Options,
) -> io::Result<Handle> {
    let Options {
        allow,
        game,
        skip_tutorial,
        dump,
        max_per_ip,
        vs,
        matcher,
        sessions,
        no_timed_cups,
    } = opts;
    let sessions = sessions.unwrap_or_default();
    let vs = vs.map(|cfg| {
        Arc::new(Vs {
            cfg,
            matcher: matcher.unwrap_or_default(),
        })
    });
    let peers: Arc<Mutex<HashMap<IpAddr, usize>>> = Arc::default();
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
    accept_loop(listener, stop, log.clone(), move |mut conn, peer| {
        if allow.as_ref().is_some_and(|f| !f(peer.ip())) {
            log(&format!("refused {peer}: not on the allowlist"));
            return;
        }
        let _slot = {
            let mut m = peers.lock().unwrap_or_else(PoisonError::into_inner);
            let n = m.entry(peer.ip()).or_insert(0);
            if max_per_ip > 0 && *n >= max_per_ip {
                log(&format!("refused {peer}: {max_per_ip} connections already"));
                return;
            }
            *n += 1;
            Slot(&peers, peer.ip())
        };
        log(&format!("connected {peer}"));
        let mut session: Session = sessions.open(peer, log.clone());
        // the header on the stack, one payload buffer per link (it grows to at most MAX_FRAME - 40 bytes)
        let (mut head, mut have) = ([0u8; 40], 0);
        let mut payload: Vec<u8> = Vec::new();
        loop {
            match fill(&mut conn, &mut head[have..], &flag) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    log(&format!("recv error {e}"));
                    break;
                }
            }
            if head[..8] != MAGIC {
                // keep from the next place a magic can start: a whole one, or its first bytes at the end
                let skip = (1..40)
                    .find(|&i| {
                        let n = (40 - i).min(8);
                        head[i..i + n] == MAGIC[..n]
                    })
                    .unwrap_or(40);
                log(&format!("resync: dropped {skip} bytes"));
                head.copy_within(skip.., 0);
                have = 40 - skip;
                continue;
            }
            have = 0;
            let size = match frame_len(&head) {
                Ok(n) => n,
                Err(n) => {
                    log(&format!("{peer}: frame of {n} bytes, closing"));
                    return;
                }
            };
            payload.resize(size - 40, 0);
            match fill(&mut conn, &mut payload, &flag) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    log(&format!("recv error {e}"));
                    break;
                }
            }
            let (cmd, seq) = (
                u16::from_le_bytes([head[8], head[9]]),
                u16::from_le_bytes([head[10], head[11]]),
            );
            let (status, id) = (u32_at(&head, 12), u32_at(&head, 20));
            let text =
                String::from_utf8_lossy(head[25..40].split(|b| *b == 0).next().unwrap_or(&[]));
            let first: String = payload
                .iter()
                .take(48)
                .map(|b| format!("{b:02x}"))
                .collect();
            // vslink's lookup comes every 0.5 s while a link is up: counted, said once a minute
            let quiet = cmd == 904;
            if quiet {
                if let Some(line) = session.vslink_poll() {
                    log(&line);
                }
            } else {
                log(&format!(
                    "<- cmd {cmd} seq {seq} status {status} payload {} conn {id} text {text:?} first {first}",
                    payload.len()
                ));
            }
            if dump && !payload.is_empty() {
                let ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let _ = std::fs::write(save_dir.join(format!("req_{cmd}_{ms}.bin")), &payload);
                prune(&save_dir, KEEP_FILES);
            }
            match answer(
                cmd,
                &payload,
                &store,
                skip_tutorial.as_deref(),
                &rare,
                vs.as_deref().map(|v| (v, peer.ip())),
                &log,
            ) {
                Some((rcmd, mut body, refused)) => {
                    if rcmd == 238 && no_timed_cups {
                        body[16 + 1172] = 0;
                    }
                    for line in session.request(cmd, &payload, refused.as_deref()) {
                        log(&line);
                    }
                    if let Some(why) = refused {
                        log(&format!("   refused cmd {cmd}: {why} (errCode 1)"));
                    }
                    if send(&mut conn, &header(rcmd, seq, 1, body.len(), id), &body).is_err() {
                        return;
                    }
                    if !quiet {
                        log(&format!("-> cmd {rcmd} payload {}", body.len()));
                    }
                }
                None => log(&format!("   (no answer for cmd {cmd})")),
            }
        }
        log(&format!("closed {peer}"));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // The framing rule on any header: a length between the 40-byte header and the cap, or the oversized length it
    // refused (`fill` sizes its buffer from the first, closes on the second). And a header `header` writes is one the
    // reader reads back: the size it claims, its fields and its checksum.
    proptest! {
        #[test]
        fn frame_len_stays_within_40_and_the_cap(
            raw in prop::collection::vec(any::<u8>(), 40),
            // the sizes that matter: the 40-byte floor, the cap, and whatever a peer sends
            size in prop_oneof![
                0u32..64,
                (MAX_FRAME as u32 - 32)..(MAX_FRAME as u32 + 32),
                any::<u32>(),
            ],
        ) {
            let mut h = raw;
            h[16..20].copy_from_slice(&size.to_le_bytes());
            match frame_len(&h) {
                Ok(n) => prop_assert!((40..=MAX_FRAME).contains(&n), "{} of {h:02x?}", n),
                Err(n) => prop_assert!(n > MAX_FRAME, "{} of {h:02x?}", n),
            }
            // and the rule the reader follows, from the field alone
            let claim = (size as usize).max(40);
            let want = if claim > MAX_FRAME { Err(claim) } else { Ok(claim) };
            prop_assert_eq!(frame_len(&h), want);
        }

        #[test]
        fn a_written_header_reads_back_whole(
            cmd in any::<u16>(),
            seq in any::<u16>(),
            status in any::<u32>(),
            size in 0..=(MAX_FRAME - 40),
            id in any::<u32>(),
        ) {
            let h = header(cmd, seq, status, size, id);
            prop_assert_eq!(frame_len(&h), Ok(40 + size));
            prop_assert_eq!(&h[..8], &MAGIC);
            prop_assert_eq!(u16::from_le_bytes([h[8], h[9]]), cmd);
            prop_assert_eq!(u16::from_le_bytes([h[10], h[11]]), seq);
            prop_assert_eq!(u32_at(&h, 12), status);
            prop_assert_eq!(u32_at(&h, 20), id);
            prop_assert_eq!(checksum(&h), h[24]);
        }
    }

    #[test]
    fn live_download_chunks_fill_the_buffer_exactly() {
        assert_eq!(
            (
                live_chunk_size(0),
                live_chunk_size(124),
                live_chunk_size(125)
            ),
            (2032, 504, 0)
        );
        assert_eq!((0..125).map(live_chunk_size).sum::<u32>(), 0x3DA38);
        let mut p = vec![0u8; 8];
        put(&mut p, 4, 124);
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let (cmd, body, _) = answer(261, &p, &store, None, &[], None, &log).unwrap();
        assert_eq!((cmd, body.len(), u32_at(&body, 4)), (262, 2040, 504));
    }

    /// Where round `round` (0..8) of competition `compe` (0..2) keeps its 6,480-byte match in the control's 261 buffer
    /// (the answers' bytes from +8, joined): the buffer's own 8-byte head (week id, weekday), then 68,816 bytes per
    /// competition, 8,602 per round, the match at +58 (control sub_47AFE0, sub_47A530).
    fn live_game_at(compe: usize, round: usize) -> usize {
        8 + 68_816 * compe + 8_602 * round + 58
    }

    #[test]
    fn the_live_download_holds_no_data_match_to_replay() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let mut buf = Vec::new();
        for chunk in 0..125 {
            let mut p = vec![0u8; 8];
            put(&mut p, 4, chunk);
            let (_, body, _) = answer(261, &p, &store, None, &[], None, &log).unwrap();
            let n = u32_at(&body, 4) as usize;
            buf.extend_from_slice(&body[8..8 + n]); // what sub_443090 copies: answer +8, size at +4
        }
        assert_eq!(buf.len(), 0x3DA38);
        // the layout: 8 rounds fill a competition, 2 competitions end where the team names start (+137,640)
        assert_eq!(live_game_at(0, 1) - live_game_at(0, 0), 8_602);
        assert_eq!(live_game_at(1, 0) - live_game_at(0, 0), 8 * 8_602);
        assert_eq!(live_game_at(2, 0) - 58, 137_640);
        // every match slot is zero ("LiveGameInit bin is empty"), as is the rest: names and emblems of nobody
        for (c, r) in (0..2).flat_map(|c| (0..8).map(move |r| (c, r))) {
            let at = live_game_at(c, r);
            assert!(
                buf[at..at + 6480].iter().all(|&b| b == 0),
                "competition {c} round {r}: a non-zero match would play on the projector"
            );
        }
        assert!(buf.iter().all(|&b| b == 0));
    }

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
            None,
            &(Arc::new(|_: &str| {}) as Log),
        )
        .unwrap();
        assert_eq!((cmd, body.len()), (238, 0x5A54));
        assert!(body[SECRETARIES].iter().all(|b| *b == 1));
        // the four secretaries with an id (26..29) and the date and time left 0
        assert!((26..30).all(|n| body[16 + 1176 + n] == 1));
        assert!(body[8..16].iter().all(|b| *b == 0));
    }

    #[test]
    fn tail_spans_stay_inside_and_apart() {
        let mut s = TAIL_SPANS.to_vec();
        s.sort();
        assert!(
            s.iter()
                .all(|&(to, from, len)| to + len <= 6176 && from + len <= 8572)
        );
        assert!(s.windows(2).all(|w| w[0].0 + w[0].2 <= w[1].0));
        let mut s: Vec<_> = TAIL_SPANS
            .iter()
            .map(|&(_, from, len)| (from, len))
            .collect();
        s.sort();
        assert!(s.windows(2).all(|w| w[0].0 + w[0].1 <= w[1].0));
    }

    #[test]
    fn an_old_store_gets_its_277_once_and_loses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gamesvr.db");
        let (block, tail): (Vec<u8>, Vec<u8>) = (
            (0..BLOCK).map(|i| (i % 251) as u8 + 1).collect(),
            (0..8572 - TAIL).map(|i| (i % 241) as u8 + 3).collect(),
        );
        {
            // a store from before `requests`: user_version 0, a club with a block and a tail
            let db = Connection::open(&path).unwrap();
            db.execute_batch(SCHEMA).unwrap();
            db.execute_batch("DROP TABLE requests").unwrap();
            db.execute(
                "INSERT INTO coaches (aime_id, coach_id, card_id, created, updated) VALUES (5, 1, 9, 1, 1)",
                [],
            )
            .unwrap();
            db.execute(
                "INSERT INTO saves VALUES (5, 1, ?1, 12, 3072, 2, 1, ?2)",
                params![block, tail],
            )
            .unwrap();
        }
        let log: Log = Arc::new(|_: &str| {});
        let card_in = |store: &Mutex<Store>| {
            answer(275, &req(275, [5, 1, 9]), store, None, &[], None, &log)
                .unwrap()
                .1
        };
        let store = Mutex::new(Store::open(&path).unwrap());
        let a = card_in(&store);
        assert_eq!(u32_at(&a, 4), 0);
        for (to, from, len) in TAIL_SPANS {
            assert_eq!(
                a[to..to + len],
                tail[from - TAIL..from - TAIL + len],
                "span {to}"
            );
        }
        {
            let db = &store.lock().unwrap().0;
            let n = |t: &str| -> i64 {
                db.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                    .unwrap()
            };
            assert_eq!((n("coaches"), n("saves"), n("requests")), (1, 1, 1));
            let kept: Vec<u8> = db
                .query_row("SELECT tail FROM saves", [], |r| r.get(0))
                .unwrap();
            assert_eq!(kept, tail, "saves.tail untouched");
            let req: Vec<u8> = db
                .query_row("SELECT payload FROM requests", [], |r| r.get(0))
                .unwrap();
            assert_eq!((req.len(), u32_at(&req, 8), req[24]), (8572, 1, 2));
            assert_eq!(&req[28..28 + BLOCK], &block[..]);
            // a later save replaces the rebuilt request; a second open must not bring the old one back
            db.execute("UPDATE requests SET payload = zeroblob(8572)", [])
                .unwrap();
        }
        drop(store);
        let store = Mutex::new(Store::open(&path).unwrap());
        let v: i64 = store
            .lock()
            .unwrap()
            .0
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let a = card_in(&store);
        assert!(
            a[3100..3556].iter().all(|b| *b == 0),
            "not backfilled twice"
        );
    }

    #[test]
    fn game_ctrl_carries_the_clients_defaults() {
        let mut body = vec![0u8; 0x5A54];
        fill_game_ctrl(&mut body, 0);
        fn at(body: &[u8], off: usize, w: usize) -> u32 {
            let mut b = [0u8; 4];
            b[..w].copy_from_slice(&body[16 + off..16 + off + w]);
            u32::from_le_bytes(b)
        }
        assert!(
            GAME_CTRL_DEFAULTS
                .iter()
                .all(|&(o, w, v)| at(&body, o, w) == v)
        );
        assert_eq!(
            (
                at(&body, 4, 4),
                at(&body, 8, 4),
                at(&body, 19_740, 1),
                at(&body, 1_176, 1)
            ),
            (5_000, 250_000, 26, 1)
        );
        assert!(body[8..16].iter().all(|b| *b == 0), "date and time stay 0");
        // the Universal Cup: scheduling on (payload +1,188 = GAME_CTRL 1172; without it control 0x462820 never counts a
        // cup held and the Universal Championship never follows), 9-minute slots, 20 minutes, and the four open times
        assert_eq!(
            (body[1_188], body[1_189], body[1_190]),
            (1, 9, 20),
            "Universal Cup info"
        );
        assert_eq!(
            [1_092, 1_096, 1_100, 1_104].map(|o| at(&body, o, 4)),
            [1_200, 1_500, 1_800, 2_100]
        );
        // VS overlays the polls and keeps the Grand Trophy block
        vs::fill_game_ctrl(&mut body);
        assert_eq!(
            (
                at(&body, 19_740, 1),
                at(&body, 7_624, 2),
                at(&body, 7_670, 1),
                at(&body, 1_211, 1)
            ),
            (u32::from(vs::POLLS), 2400, 7, 1)
        );
        // no two defaults overlap each other, the secretaries, the week id or the matches per class
        let mut spans: Vec<_> = GAME_CTRL_DEFAULTS
            .iter()
            .map(|&(o, w, _)| (o, o + w))
            .collect();
        spans.extend([(1_176, 1_212), (7_620, 7_622), (7_666, 7_671)]);
        spans.sort();
        assert!(spans.windows(2).all(|w| w[0].1 <= w[1].0));
    }

    #[test]
    fn grand_trophy_block_without_vs() {
        // the arcade's stand-in: no VS switch, so the 238 is the default filler alone
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let mut req = vec![0u8; 16];
        put(&mut req, 0, 1);
        let (body, _) = ask_for_test(&store, 237, &req, &log);
        let at = |off: usize| u16_at(&body, 16 + off);
        assert_eq!((at(7_622), at(7_624)), (0, 2_400), "open all day");
        assert_eq!(body[16 + 7_666..16 + 7_671], [3, 5, 5, 7, 7], "N per class");
        assert_eq!(
            [
                7_626, 7_628, 7_630, 7_632, 7_634, 7_636, 7_638, 7_640, 7_642
            ]
            .map(at),
            [300, 0, 100, 100, 50, 10, 100, 100, 0],
            "win, lose, draw, goal, clean sheet, conceded, hat-trick, streak, streak step"
        );
        assert_eq!((at(7_660), at(7_662), at(7_664)), (4_500, 2_000, 0));
        assert!(
            body[16 + 7_671..16 + 7_736].iter().all(|b| *b == 0),
            "playoff CPU level and the class thresholds"
        );
        assert_eq!(at(7_620), gt_week(now()));
    }

    #[test]
    fn gt_week_rolls_on_sunday_23_italian_time() {
        // Unix seconds of a Rome wall-clock time: winter UTC+1, summer UTC+2
        let utc = |y, mo, d, h, mi| {
            chrono::NaiveDate::from_ymd_opt(y, mo, d)
                .unwrap()
                .and_hms_opt(h, mi, 0)
                .unwrap()
                .and_utc()
                .timestamp()
        };
        // winter: Sunday 2026-11-08 22:59 / 23:00 CET = 21:59 / 22:00 UTC
        let w = gt_week(utc(2026, 11, 8, 21, 59));
        assert_eq!(gt_week(utc(2026, 11, 8, 22, 0)), w + 1);
        assert_eq!(
            gt_week(utc(2026, 11, 2, 0, 0)),
            w,
            "Monday 01:00 is the same week"
        );
        // summer: Sunday 2026-07-05 22:59 / 23:00 CEST = 20:59 / 21:00 UTC
        let s = gt_week(utc(2026, 7, 5, 20, 59));
        assert_eq!(gt_week(utc(2026, 7, 5, 21, 0)), s + 1);
        assert_eq!(
            w - s,
            18,
            "18 weeks from Monday 2026-06-29 to Monday 2026-11-02"
        );
        // week 0 starts on Sunday 2000-01-02 23:00 CET
        assert_eq!(gt_week(utc(2000, 1, 2, 22, 0)), 0);
        assert_eq!(
            gt_week(utc(2000, 1, 2, 21, 59)),
            0,
            "clamped below the epoch"
        );
        assert_eq!(gt_week(utc(2000, 1, 9, 22, 0)), 1);
    }

    /// Unix seconds of a UTC wall-clock time.
    fn utc_secs(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    }

    #[test]
    fn gt_week_rolls_at_23_on_the_dst_sundays() {
        // DST ends on Sunday 2026-10-25 (03:00 CEST -> 02:00 CET): the roll that evening is 23:00 CET = 22:00 UTC
        let w = gt_week(utc_secs(2026, 10, 25, 21, 59));
        assert_eq!(gt_week(utc_secs(2026, 10, 25, 22, 0)), w + 1);
        // the week before rolled at 23:00 CEST = 21:00 UTC, one week earlier
        assert_eq!(gt_week(utc_secs(2026, 10, 18, 20, 59)), w - 1);
        assert_eq!(gt_week(utc_secs(2026, 10, 18, 21, 0)), w);
        // DST starts on Sunday 2027-03-28 (02:00 CET -> 03:00 CEST): the roll is 23:00 CEST = 21:00 UTC
        let (before, after) = (utc_secs(2027, 3, 28, 20, 59), utc_secs(2027, 3, 28, 21, 0));
        assert_eq!(gt_week(after), gt_week(before) + 1);
        assert_eq!(
            gt_week(utc_secs(2027, 3, 21, 22, 0)),
            gt_week(before),
            "rolled at 23:00 CET the week before"
        );
        // a 277 saved a minute before that roll is reset at a card-in a minute after it; one saved at the roll is not
        let mut body = vec![0xAAu8; 6176];
        fill_gt_week(&mut body, gt_week(after), gt_week(after), 0);
        assert!(body[3192..3200].iter().all(|b| *b == 0xAA));
        fill_gt_week(&mut body, gt_week(before), gt_week(after), 0);
        assert!(body[3192..3200].iter().all(|b| *b == 0));
        // a save stamped in a later week resets too
        body.fill(0xAA);
        fill_gt_week(&mut body, gt_week(after), gt_week(before), 0);
        assert!(body[3192..3200].iter().all(|b| *b == 0));
    }

    /// A play carded in before the Sunday 23:00 roll and saved after it files its record under the card-in week and
    /// the club's next card-in starts from zero; a store error on `gt_points` neither refuses the save nor the
    /// card-in.
    #[test]
    fn a_play_across_the_roll_keeps_its_card_in_week() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let vs = Vs {
            cfg: VsConfig {
                public_ip: std::net::Ipv4Addr::LOCALHOST,
                reflect_port: 1,
                vslink_port: 2,
            },
            matcher: Arc::default(),
        };
        let vs = Some((&vs, IpAddr::from([127, 0, 0, 1])));
        let ask = |cmd: u16, p: &[u8]| {
            let (_, body, why) = answer(cmd, p, &store, None, &[], vs, &log).unwrap();
            assert!(why.is_none(), "{cmd}: {why:?}");
            body
        };
        let b = ask(279, &fixture(279, [5, 0, 0]));
        let (club, card) = (u32_at(&b, 8), u32_at(&b, 12));
        ask(277, &fixture(277, [5, club, card]));
        ask(275, &req(275, [5, club, card]));
        let week = gt_week(now());
        // the card-in notes its week, unfiled
        assert_eq!(
            store.lock().unwrap().1.get(&(5, club)),
            Some(&(week, false))
        );
        store.lock().unwrap().1.insert((5, club), (week - 1, false));
        let mut r = fixture(277, [5, club, card]);
        r[5564..5708].fill(0x11);
        put(&mut r, GT_RECORD_277, 800);
        // a save across the roll whose record is not written stays unfiled, and the next save files it
        store
            .lock()
            .unwrap()
            .0
            .execute_batch("ALTER TABLE gt_points RENAME TO gone")
            .unwrap();
        ask(277, &r);
        assert_eq!(
            store.lock().unwrap().1.get(&(5, club)),
            Some(&(week - 1, false))
        );
        store
            .lock()
            .unwrap()
            .0
            .execute_batch("ALTER TABLE gone RENAME TO gt_points")
            .unwrap();
        ask(277, &r);
        assert_eq!(
            store.lock().unwrap().1.get(&(5, club)),
            Some(&(week - 1, true))
        );
        assert_eq!(
            store.lock().unwrap().gt_ranking(week - 1).unwrap(),
            [(5, 800)]
        );
        assert!(store.lock().unwrap().gt_ranking(week).unwrap().is_empty());
        // a later save of the same session leaves the closed week's row and the live week alone
        let at_w = |s: &Store| -> i64 {
            s.0.query_row(
                "SELECT at FROM gt_points WHERE week = ?1",
                [week - 1],
                |r| r.get(0),
            )
            .unwrap()
        };
        let first_at = at_w(&store.lock().unwrap());
        put(&mut r, GT_RECORD_277, 900);
        ask(277, &r);
        assert_eq!(
            store.lock().unwrap().gt_ranking(week - 1).unwrap(),
            [(5, 800)]
        );
        assert_eq!(at_w(&store.lock().unwrap()), first_at);
        assert!(store.lock().unwrap().gt_ranking(week).unwrap().is_empty());
        let a = ask(275, &req(275, [5, club, card]));
        for (at, len) in GT_WEEKLY_276 {
            assert!(a[at..at + len].iter().all(|b| *b == 0), "+{at} reset");
        }

        store
            .lock()
            .unwrap()
            .0
            .execute_batch("DROP TABLE gt_points")
            .unwrap();
        ask(277, &r);
        assert_eq!(
            u32_at(&ask(275, &req(275, [5, club, card])), GT_RANK_276),
            0
        );
    }

    /// The shared ranking on a server with VS: two cards' records in one week come back as ranks 1 and 2 at
    /// card-in, in the exact 276 bytes the client reads; a stored save from an earlier week loses its weekly
    /// fields; ties are deterministic; an arcade's stand-in (no VS) keeps none of it.
    #[test]
    fn gt_ranking_orders_records_and_fills_the_276() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let vs = Vs {
            cfg: VsConfig {
                public_ip: std::net::Ipv4Addr::LOCALHOST,
                reflect_port: 1,
                vslink_port: 2,
            },
            matcher: Arc::default(),
        };
        let ip = IpAddr::from([127, 0, 0, 1]);
        let ask = |cmd: u16, p: &[u8], vs: Option<(&Vs, IpAddr)>| {
            let (_, body, why) = answer(cmd, p, &store, None, &[], vs, &log).unwrap();
            assert!(why.is_none(), "{cmd}: {why:?}");
            body
        };
        // the 277's Grand Trophy fields, +5,564..+5,708, every byte non-zero; the record (+5,572) set per club
        let save = |aime: u32, record: u32| {
            let b = ask(279, &fixture(279, [aime, 0, 0]), Some((&vs, ip)));
            let (club, card) = (u32_at(&b, 8), u32_at(&b, 12));
            let mut r = fixture(277, [aime, club, card]);
            for (i, b) in r[5564..5708].iter_mut().enumerate() {
                *b = i as u8 | 0x80;
            }
            put(&mut r, GT_RECORD_277, record);
            ask(277, &r, Some((&vs, ip)));
            (club, card, r)
        };
        let (c5, k5, r5) = save(5, 900);
        let (c6, k6, _) = save(6, 1_200);
        let week = gt_week(now());
        assert_eq!(
            store.lock().unwrap().gt_ranking(week).unwrap(),
            [(6, 1_200), (5, 900)]
        );
        // another club of card 5 (or the same club, imported again under a new coachId) with a lower record: the
        // card keeps its best
        save(5, 300);
        assert_eq!(
            store.lock().unwrap().gt_ranking(week).unwrap(),
            [(6, 1_200), (5, 900)]
        );
        let card_in = |aime: u32, club: u32, card: u32| {
            ask(275, &req(275, [aime, club, card]), Some((&vs, ip)))
        };
        assert_eq!(u32_at(&card_in(6, c6, k6), GT_RANK_276), 1);
        let a = card_in(5, c5, k5);
        // exact bytes: 276 +3,184..+3,328 = the 277's +5,564..+5,708, but the rank (s32 +3,212) is this server's
        let mut want = r5[5564..5708].to_vec();
        want[GT_RANK_276 - 3184..GT_RANK_276 - 3180].copy_from_slice(&2u32.to_le_bytes());
        // and so are the result fields: no promotion was issued, so final rank, next class and play-off flag are 0
        let zero_result = |v: &mut [u8]| {
            v[GT_FINAL_RANK_276 - 3184..GT_FINAL_RANK_276 - 3180].fill(0);
            v[GT_NEXT_CLASS_276 - 3184] = 0;
            v[GT_PLAYOFF_276 - 3184] = 0;
        };
        zero_result(&mut want);
        assert_eq!(a[3184..3328], want[..]);
        assert_eq!(
            u32_at(&a, 3192),
            900,
            "the record survives a card-in in its week"
        );

        // the stored 277 (and its record) is from last week: the weekly fields are zero, the rest is kept
        {
            let db = &store.lock().unwrap().0;
            let last = now() - 8 * 86_400;
            db.execute("UPDATE requests SET updated = ?1 WHERE aime_id = 5", [last])
                .unwrap();
            db.execute(
                "UPDATE gt_points SET week = ?1 WHERE aime_id = 5",
                [gt_week(last)],
            )
            .unwrap();
        }
        let a = card_in(5, c5, k5);
        for (at, len) in GT_WEEKLY_276 {
            assert!(a[at..at + len].iter().all(|b| *b == 0), "+{at} reset");
        }
        let mut kept = r5[5564..5708].to_vec();
        for (at, len) in GT_WEEKLY_276 {
            kept[at - 3184..at - 3184 + len].fill(0);
        }
        zero_result(&mut kept);
        assert_eq!(a[3184..3328], kept[..], "shop, time, card, class... kept");
        assert_eq!(u32_at(&a, 3088), {
            let mut z = a.clone();
            put(&mut z, 3088, 0);
            sum32(&z)
        });

        // ties: the record set first, then the lower Aime id
        {
            let st = store.lock().unwrap();
            let mut r = vec![0u8; 8572];
            put(&mut r, GT_RECORD_277, 500);
            for (aime, at) in [(3, 50), (2, 60), (1, 60)] {
                record_gt(&st.0, 1, aime, &r, at).unwrap();
            }
            // the same record again keeps its time
            record_gt(&st.0, 1, 3, &r, 99).unwrap();
            // a record above the bound, or one that is negative as an s32, is not kept
            for forged in [GT_RECORD_MAX + 1, 0x8000_0000] {
                put(&mut r, GT_RECORD_277, forged);
                record_gt(&st.0, 1, 4, &r, 10).unwrap();
            }
            let order: Vec<_> = st.gt_ranking(1).unwrap().iter().map(|e| e.0).collect();
            assert_eq!(order, [3, 1, 2]);
        }

        // no VS (an arcade's stand-in): the stored rank bytes come back as saved and nothing is recorded
        let (_, b, _) =
            answer(279, &fixture(279, [7, 0, 0]), &store, None, &[], None, &log).unwrap();
        let (c7, k7) = (u32_at(&b, 8), u32_at(&b, 12));
        let mut r = fixture(277, [7, c7, k7]);
        put(&mut r, GT_RECORD_277, 4_000);
        put(&mut r, GT_RANK_276 + 2380, 33);
        answer(277, &r, &store, None, &[], None, &log).unwrap();
        let a = answer(275, &req(275, [7, c7, k7]), &store, None, &[], None, &log)
            .unwrap()
            .1;
        assert_eq!(u32_at(&a, GT_RANK_276), 33);
        let n: i64 = store
            .lock()
            .unwrap()
            .0
            .query_row(
                "SELECT count(*) FROM gt_points WHERE aime_id = 7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    /// A version-2 store (before `gt_points`) keeps every row and gets each stored 277's record under the week it
    /// was saved in.
    #[test]
    fn a_v2_store_gets_gt_points_and_loses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gamesvr.db");
        let t = utc_secs(2026, 11, 4, 18, 0);
        let mut r = fixture(277, [5, 1, 9]);
        put(&mut r, GT_RECORD_277, 700);
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(SCHEMA).unwrap();
            db.execute_batch("DROP TABLE gt_points; PRAGMA user_version = 2")
                .unwrap();
            db.execute(
                "INSERT INTO coaches (aime_id, coach_id, card_id, created, updated) VALUES (5, 1, 9, 1, 1)",
                [],
            )
            .unwrap();
            db.execute(
                "INSERT INTO saves VALUES (5, 1, zeroblob(3072), 12, 3072, 2, ?1, ?2)",
                params![t, &r[TAIL..]],
            )
            .unwrap();
            // the same record from a later session, inserted first so the scan (rowid order) meets it first: the
            // earlier session keeps the tie-break only through ORDER BY updated
            db.execute(
                "INSERT INTO requests VALUES (5, 0, 277, ?1, ?2)",
                params![r, t + 3_600],
            )
            .unwrap();
            db.execute(
                "INSERT INTO requests VALUES (5, 1, 277, ?1, ?2)",
                params![r, t],
            )
            .unwrap();
        }
        let st = Store::open(&path).unwrap();
        let n = |q: &str| -> i64 { st.0.query_row(q, [], |r| r.get(0)).unwrap() };
        assert_eq!(n("PRAGMA user_version"), SCHEMA_VERSION);
        assert_eq!(
            [
                n("SELECT count(*) FROM coaches"),
                n("SELECT count(*) FROM saves"),
                n("SELECT count(*) FROM requests")
            ],
            [1, 1, 2]
        );
        let kept: Vec<u8> =
            st.0.query_row("SELECT payload FROM requests WHERE coach_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kept, r);
        assert_eq!(st.gt_ranking(gt_week(t)).unwrap(), [(5, 700)]);
        let at: i64 =
            st.0.query_row("SELECT at FROM gt_points", [], |r| r.get(0))
                .unwrap();
        assert_eq!(at, t);
    }

    /// Promotion at card-in on a server with VS: the card's record of the closed week against the threshold of the
    /// club's saved class (277 +5,609), one class up, once per week and card, never past Area A; every other card-in
    /// sends no result, whatever the stored 277 carries.
    #[test]
    fn a_closed_week_record_promotes_once_and_never_past_area_a() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let log: Log = {
            let lines = lines.clone();
            Arc::new(move |l: &str| lines.lock().unwrap().push(l.to_owned()))
        };
        let vs = Vs {
            cfg: VsConfig {
                public_ip: std::net::Ipv4Addr::LOCALHOST,
                reflect_port: 1,
                vslink_port: 2,
            },
            matcher: Arc::default(),
        };
        let vs = Some((&vs, IpAddr::from([127, 0, 0, 1])));
        let ask = |cmd: u16, p: &[u8]| {
            let (_, body, why) = answer(cmd, p, &store, None, &[], vs, &log).unwrap();
            assert!(why.is_none(), "{cmd}: {why:?}");
            body
        };
        let week = gt_week(now());
        // a save of club (club, card) of `aime` in `class`, carrying a stale result (rank 7, next Local, play-off 1)
        let save = |aime: u32, (club, card): (u32, u32), class: u8| {
            let mut r = fixture(277, [aime, club, card]);
            r[GT_CLASS_277] = class;
            put(&mut r, GT_FINAL_RANK_276 + 2380, 7);
            r[GT_PLAYOFF_276 + 2380] = 1;
            ask(277, &r);
        };
        // a new club of `aime` saved in `class`
        let club = |aime: u32, class: u8| {
            let b = ask(279, &fixture(279, [aime, 0, 0]));
            let k = (u32_at(&b, 8), u32_at(&b, 12));
            save(aime, k, class);
            k
        };
        let last_week = |aime: u32, record: u32| {
            let mut r = vec![0u8; 8572];
            put(&mut r, GT_RECORD_277, record);
            record_gt(&store.lock().unwrap().0, week - 1, aime, &r, 1).unwrap();
        };
        // (final rank, next class, play-off flag) of a card-in
        let result = |aime: u32, (club, card): (u32, u32)| {
            let a = ask(275, &req(275, [aime, club, card]));
            assert_eq!(u32_at(&a, 3088), {
                let mut z = a.clone();
                put(&mut z, 3088, 0);
                sum32(&z)
            });
            (
                u32_at(&a, GT_FINAL_RANK_276),
                a[GT_NEXT_CLASS_276],
                a[GT_PLAYOFF_276],
            )
        };
        last_week(9, 3_000);

        let local = club(5, 0);
        last_week(5, 999);
        assert_eq!(result(5, local), (0, 0, 0), "999 stays Local");
        last_week(5, 1_000);
        assert_eq!(
            result(5, local),
            (2, 1, 0),
            "1,000: Area B, rank 2 behind card 9"
        );
        // the save still shows Local (the 276 was lost, or the session ended before the info scene): the same again
        assert_eq!(result(5, local), (2, 1, 0), "re-issued until applied");
        // the closed week's row pruned: the stored rank again, never a made-up first place
        store
            .lock()
            .unwrap()
            .0
            .execute(
                "DELETE FROM gt_points WHERE week = ?1 AND aime_id = 5",
                [week - 1],
            )
            .unwrap();
        assert_eq!(
            result(5, local),
            (2, 1, 0),
            "stored rank, no closed-week row"
        );
        // the card's other club carded in later the same week: the card's promotion is spent
        let other = club(5, 0);
        assert_eq!(result(5, other), (0, 0, 0));
        save(5, local, 1);
        assert_eq!(result(5, local), (0, 0, 0), "applied: once per week");

        let area_b = club(6, 1);
        last_week(6, 1_199);
        assert_eq!(result(6, area_b), (0, 0, 0), "1,199 stays Area B");
        last_week(6, 1_200);
        assert_eq!(
            result(6, area_b),
            (2, 2, 0),
            "1,200: Area A, rank 2 behind card 9"
        );

        // one card, two clubs: the class is the carding club's; an Area A club carding in first spends nothing
        let area_a = club(13, 2);
        let local13 = club(13, 0);
        last_week(13, 1_100);
        assert_eq!(result(13, area_a), (0, 0, 0), "Area A is the cap");
        assert_eq!(result(13, local13), (3, 1, 0), "Local -> Area B, rank 3");
        assert_eq!(result(13, area_a), (0, 0, 0), "not to the other club");

        // a guest's club, imported again at its next join (new coach and card ids): the promotion follows it
        let guest = club(20, 0);
        last_week(20, 1_000);
        assert_eq!(result(20, guest), (4, 1, 0), "Local -> Area B, rank 4");
        store
            .lock()
            .unwrap()
            .0
            .execute("INSERT INTO guests VALUES (20, 't', 1, 0)", [])
            .unwrap();
        let rejoin = || {
            let mut st = store.lock().unwrap();
            let c = st.club(20).unwrap();
            let link = st.import_public(&c, 20, 1, "t", &|_| false).unwrap();
            (link.coaches[0].1, link.cards[0].1)
        };
        let again = rejoin();
        assert_ne!(again, guest);
        assert_eq!(result(20, again), (4, 1, 0), "re-imported, not applied");
        let again = rejoin();
        assert_eq!(result(20, again), (4, 1, 0), "each join of the week");
        save(20, again, 1);
        let applied = rejoin();
        assert_eq!(result(20, applied), (0, 0, 0), "applied");

        for (aime, class) in [(7, 2), (8, 3), (10, 4), (11, 200)] {
            let k = club(aime, class);
            last_week(aime, 5_000);
            assert_eq!(result(aime, k), (0, 0, 0), "class {class}: no promotion");
        }
        // a record of this week counts next week, not now
        let fresh = club(12, 0);
        let mut r = vec![0u8; 8572];
        put(&mut r, GT_RECORD_277, 5_000);
        record_gt(&store.lock().unwrap().0, week, 12, &r, 1).unwrap();
        assert_eq!(result(12, fresh), (0, 0, 0));

        let rows: Vec<(u32, u32, u8)> = store
            .lock()
            .unwrap()
            .0
            .prepare("SELECT aime_id, coach_id, class FROM gt_promotions WHERE week = ?1 ORDER BY aime_id")
            .unwrap()
            .query_map([week], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                (5, local.0, 1),
                (6, area_b.0, 2),
                (13, local13.0, 1),
                (20, again.0, 1)
            ]
        );
        let lines = lines.lock().unwrap();
        let want = format!(
            "grand trophy: aime 5 coach {} week {week} class 0 -> 1 (record 1000, rank 2)",
            local.0
        );
        assert!(lines.contains(&want), "{want}");

        // never a next class below the class or above Area A, whatever the caller passes
        let mut b = vec![0xFFu8; 6176];
        for (promote, want) in [
            (Some((0, 0)), (1, 1)),
            (Some((1, 4)), (4, 2)),
            (Some((2, 1)), (0, 0)),
            (Some((4, 1)), (0, 0)),
            (None, (0, 0)),
        ] {
            fill_gt_result(&mut b, promote);
            assert_eq!(
                (
                    u32_at(&b, GT_FINAL_RANK_276),
                    b[GT_NEXT_CLASS_276],
                    b[GT_PLAYOFF_276]
                ),
                (want.0, want.1, 0),
                "{promote:?}"
            );
        }
        assert!(GT_RAISE[2..].iter().all(|&p| p == 0), "Area A is the cap");
    }

    /// A club granted Area B last week whose save fell back to Local: its class is the grant, also after a re-import
    /// (a public server's clubs get new ids each session), so this week's record promotes it Area B -> Area A, each
    /// card-in of the week sends that grant, and a record below that threshold re-sends last week's grant, writing
    /// nothing; never Local -> Area B.
    #[test]
    fn an_earlier_weeks_grant_is_the_floor_of_the_saved_class() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let vs = Vs {
            cfg: VsConfig {
                public_ip: std::net::Ipv4Addr::LOCALHOST,
                reflect_port: 1,
                vslink_port: 2,
            },
            matcher: Arc::default(),
        };
        let vs = Some((&vs, IpAddr::from([127, 0, 0, 1])));
        let ask = |cmd: u16, p: &[u8]| answer(cmd, p, &store, None, &[], vs, &log).unwrap().1;
        let week = gt_week(now());
        let record = |w: u16, aime: u32, points: u32| {
            let mut r = vec![0u8; 8572];
            put(&mut r, GT_RECORD_277, points);
            record_gt(&store.lock().unwrap().0, w, aime, &r, 1).unwrap();
        };
        // a Local club of `aime`, granted Area B at a card-in of last week, saved Local again
        let demoted = |aime: u32| {
            let b = ask(279, &fixture(279, [aime, 0, 0]));
            let (club, card) = (u32_at(&b, 8), u32_at(&b, 12));
            let mut r = fixture(277, [aime, club, card]);
            r[GT_CLASS_277] = 0;
            ask(277, &r);
            record(week - 2, aime, 1_000);
            let local = vec![0u8; 8572];
            let granted = store.lock().unwrap().promote(week - 1, aime, club, &local);
            assert_eq!(
                granted.unwrap().map(|g| g.0),
                Some(0),
                "last week: Local -> Area B"
            );
            (club, card)
        };
        let result = |aime: u32, (club, card): (u32, u32)| {
            let a = ask(275, &req(275, [aime, club, card]));
            (u32_at(&a, GT_FINAL_RANK_276), a[GT_NEXT_CLASS_276])
        };

        // the club imported again (a guest's next join, online::club): new coach and card ids, the old coach gone
        let rejoin = |aime: u32| {
            let mut st = store.lock().unwrap();
            st.0.execute("INSERT INTO guests VALUES (?1, 't', 1, 0)", [aime])
                .unwrap();
            let c = st.club(aime).unwrap();
            let link = st.import_public(&c, aime, 1, "t", &|_| false).unwrap();
            (link.coaches[0].1, link.cards[0].1)
        };

        let old = demoted(5);
        let k = rejoin(5);
        assert_ne!(k, old);
        record(week - 1, 5, 1_300);
        assert_eq!(
            result(5, k),
            (1, 2),
            "re-imported: Area B -> Area A, not Local -> Area B"
        );
        assert_eq!(
            result(5, k),
            (1, 2),
            "the week's next card-in, save still Local: the grant again, never Area B"
        );

        let k = demoted(6);
        record(week - 1, 6, 500);
        assert_eq!(
            result(6, k),
            (2, 1),
            "no promotion this week: last week's grant (rank 2) again"
        );
        let rows: i64 = store
            .lock()
            .unwrap()
            .0
            .query_row(
                "SELECT count(*) FROM gt_promotions WHERE week = ?1 AND aime_id = 6",
                [week],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0, "a re-sent grant spends nothing this week");
    }

    /// A server with VS sends the promotion thresholds as GAME_CTRL raisePoint; an arcade's stand-in sends none.
    #[test]
    fn game_ctrl_carries_the_promotion_thresholds_with_vs() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let vs = Vs {
            cfg: VsConfig {
                public_ip: std::net::Ipv4Addr::LOCALHOST,
                reflect_port: 1,
                vslink_port: 2,
            },
            matcher: Arc::default(),
        };
        let mut req = vec![0u8; 16];
        put(&mut req, 0, 1);
        let raise = |vs| {
            let body = answer(237, &req, &store, None, &[], vs, &log).unwrap().1;
            [0, 1, 2, 3, 4].map(|c| u32_at(&body, 16 + 7_676 + 4 * c))
        };
        assert_eq!(
            raise(Some((&vs, IpAddr::from([127, 0, 0, 1])))),
            [1_000, 1_200, 0, 0, 0]
        );
        assert_eq!(raise(None), [0; 5]);
    }

    /// A version-3 store (before `gt_promotions`) keeps every row, gets the table (from [`SCHEMA`], as `purchases`
    /// and `gt_points` did) and version 4, and its `gt_points` is not filled again from the stored 277s; a second open
    /// changes nothing.
    #[test]
    fn a_v3_store_gets_gt_promotions_and_loses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gamesvr.db");
        let mut r = fixture(277, [5, 1, 9]);
        put(&mut r, GT_RECORD_277, 700);
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(SCHEMA).unwrap();
            db.execute_batch("DROP TABLE gt_promotions; PRAGMA user_version = 3")
                .unwrap();
            db.execute(
                "INSERT INTO coaches (aime_id, coach_id, card_id, created, updated) VALUES (5, 1, 9, 1, 1)",
                [],
            )
            .unwrap();
            db.execute(
                "INSERT INTO saves VALUES (5, 1, zeroblob(3072), 12, 3072, 2, 1, ?1)",
                [&r[TAIL..]],
            )
            .unwrap();
            db.execute("INSERT INTO requests VALUES (5, 1, 277, ?1, 1)", [&r])
                .unwrap();
            db.execute("INSERT INTO gt_points VALUES (40, 5, 1234, 77)", [])
                .unwrap();
        }
        for open in 0..2 {
            let st = Store::open(&path).unwrap();
            let n = |q: &str| -> i64 { st.0.query_row(q, [], |r| r.get(0)).unwrap() };
            assert_eq!(n("PRAGMA user_version"), 4, "open {open}");
            assert_eq!(
                [
                    n("SELECT count(*) FROM coaches"),
                    n("SELECT count(*) FROM saves"),
                    n("SELECT count(*) FROM requests"),
                    n("SELECT count(*) FROM gt_promotions")
                ],
                [1, 1, 1, open],
                "open {open}"
            );
            let kept: Vec<u8> =
                st.0.query_row("SELECT payload FROM requests", [], |r| r.get(0))
                    .unwrap();
            assert_eq!(kept, r);
            assert_eq!(n("SELECT count(*) FROM gt_points"), 1, "no second backfill");
            assert_eq!(st.gt_ranking(40).unwrap(), [(5, 1234)]);
            assert_eq!(n("SELECT at FROM gt_points"), 77);
            // the table takes a promotion, and the row outlives the reopen
            st.0.execute(
                "INSERT OR IGNORE INTO gt_promotions VALUES (41, 5, 1, 1, 78, 1, 1234, 70)",
                [],
            )
            .unwrap();
        }
    }

    #[test]
    fn vs_echoes_the_entry_user_and_the_data_battle() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let vs = Vs {
            cfg: VsConfig {
                public_ip: std::net::Ipv4Addr::LOCALHOST,
                reflect_port: 1,
                vslink_port: 2,
            },
            matcher: Arc::default(),
        };
        let ip = IpAddr::from([127, 0, 0, 1]);
        let ask = |cmd: u16, ids: [u32; 3]| {
            let mut p = vec![0u8; 16];
            put(&mut p, 0, 1);
            for (i, v) in ids.into_iter().enumerate() {
                put(&mut p, 4 + 4 * i, v);
            }
            answer(cmd, &p, &store, None, &[], Some((&vs, ip)), &log)
                .unwrap()
                .1
        };
        // 239: cardId, coachId back; 257: coachId back and status 1
        let b = ask(239, [7, 9, 0]);
        assert_eq!((u32_at(&b, 4), u32_at(&b, 8)), (7, 9));
        let b = ask(257, [9, 7, 0]);
        assert_eq!((u32_at(&b, 4), u32_at(&b, 8)), (9, 1));
        // 0 ids stay zero: the control's SvrErr
        assert!(ask(239, [0, 0, 0])[4..].iter().all(|b| *b == 0));
        assert!(ask(257, [0, 5, 0])[4..].iter().all(|b| *b == 0));
        // no VS: zeros as before
        let b = answer(
            239,
            &[1, 0, 0, 0, 7, 0, 0, 0, 9, 0, 0, 0],
            &store,
            None,
            &[],
            None,
            &log,
        )
        .unwrap()
        .1;
        assert!(b[4..].iter().all(|b| *b == 0));
    }

    /// A request for the guard test: slot 1, ids at +4/+8/+12, the revision where the club messages carry it.
    fn fixture(cmd: u16, ids: [u32; 3]) -> Vec<u8> {
        let (len, rev) = match cmd {
            271 => (20, 16),
            275 => (32, 28),
            277 => (8572, 5940),
            279 => (36, 32),
            285 => (24, 20),
            287 | 289 => (28, 24),
            309 => (3368, 0),
            _ => (600, 0),
        };
        let mut r = vec![0u8; len];
        put(&mut r, 0, 1);
        for (i, id) in ids.into_iter().enumerate() {
            put(&mut r, 4 + 4 * i, id);
        }
        if rev != 0 {
            put(&mut r, rev, REVISION);
        }
        match cmd {
            277 => {
                r[24] = 2;
                r[3100..3104].copy_from_slice(&[12, 0, 0, 12]);
            }
            279 => r[12] = 1,
            309 => r[8..3080].fill(0x5A),
            289 => put(&mut r, 16, 1),
            _ => {}
        }
        r
    }

    /// Every [`RESP`] answer is filled, or all zero with a parser address that proves zero is safe, or pending
    /// (listed with --nocapture).
    #[test]
    fn zero_answers_are_safe_or_pending() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let log: Log = Arc::new(|_: &str| {});
        let vs = Vs {
            cfg: VsConfig {
                public_ip: std::net::Ipv4Addr::LOCALHOST,
                reflect_port: 1,
                vslink_port: 2,
            },
            matcher: Arc::default(),
        };
        let ip = IpAddr::from([127, 0, 0, 1]);
        let ask = |cmd: u16, p: &[u8]| answer(cmd, p, &store, None, &[], Some((&vs, ip)), &log);
        let aime = 5;
        // a saved club, so the club messages have ids to work on
        let (_, b, _) = ask(279, &fixture(279, [aime, 0, 0])).unwrap();
        let (coach, card) = (u32_at(&b, 8), u32_at(&b, 12));
        let ids = [aime, coach, card];
        assert_eq!(u32_at(&ask(277, &fixture(277, ids)).unwrap().1, 4), 0);
        // a purchase (category = the coach id, item = the card id: Uniform 1), so 286 has something to say
        assert_eq!(u32_at(&ask(315, &fixture(315, ids)).unwrap().1, 4), 0);
        // two paired VS entries (243 -> 245), so the vslink lookup 904 and the match answer 246 have something to say
        let entry = |card: u32, seat: u32, port: u16| {
            let mut e = fixture(243, [0, 0, 0]);
            e.truncate(280);
            put(&mut e, 8, 0x0100_007F);
            e[12..14].copy_from_slice(&port.to_le_bytes());
            put(&mut e, 44, card);
            put(&mut e, 48, seat);
            e
        };
        let poll = |card: u32, seat: u32| {
            let mut p = vec![0u8; 20];
            put(&mut p, 0, 1);
            put(&mut p, 8, card);
            put(&mut p, 12, seat);
            p
        };
        ask(243, &entry(10, 1, 1111)).unwrap();
        ask(243, &entry(11, 2, 2222)).unwrap();
        assert!(
            ask(245, &poll(10, 1)).unwrap().1[4..]
                .iter()
                .any(|b| *b != 0)
        );
        let mut lookup = vec![0u8; 8];
        put(&mut lookup, 0, 0x0100_007F);
        lookup[4..6].copy_from_slice(&1111u16.to_le_bytes());

        let mut pending = vec![];
        for (rcmd, _, zero) in RESP {
            let cmd = rcmd - 1;
            let payload = match cmd {
                245 => poll(10, 1),
                904 => lookup.clone(),
                _ => fixture(cmd, ids),
            };
            let (_, body, refused) = ask(cmd, &payload).unwrap();
            assert_eq!(refused, None, "{rcmd}: the fixture is refused");
            let is_zero = body[4..].iter().all(|b| *b == 0);
            match zero {
                Zero::Handled => assert!(
                    !is_zero,
                    "{rcmd} is all zero and not marked Safe or Pending"
                ),
                Zero::Safe(why) => {
                    assert!(
                        why.contains("sub_") || why.contains("0x"),
                        "{rcmd}: Safe names no address: {why}"
                    );
                    assert!(
                        is_zero,
                        "{rcmd} is marked Safe but the answer is not zero (stale entry)"
                    );
                }
                Zero::Pending(unit) => pending.push(format!("{rcmd} ({unit})")),
            }
        }
        eprintln!("pending: {}", pending.join(", "));
    }

    /// 2026-11-06: a Universal Cup winner's saved championship entry date comes back as 0 in the 276, so MATCH
    /// SELECT offers the cup again at the first card-in; the bits around it and both sums stay right.
    #[test]
    fn the_save_answer_clears_the_championship_entry() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let b = ask_store(&store, 279, &fixture(279, [5, 0, 0])).0;
        let (club, card) = (u32_at(&b, 8), u32_at(&b, 12));
        let mut save = fixture(277, [5, club, card]);
        save[28..28 + BLOCK].fill(0xFF);
        // block bytes 2,883..2,886 of the 11-01 winner's save: 01 35 28 ED
        put_bits(&mut save[28..], UC_ENTRY_DATE_BIT - 7, 32, 20_261_101);
        assert_eq!(&save[28 + 2883..28 + 2887], &[0x01, 0x35, 0x28, 0xED]);
        let sum = sum32(&save[28..28 + BLOCK]);
        put(&mut save, 5236, sum);
        assert_eq!(u32_at(&ask_store(&store, 277, &save).0, 4), 0);
        let a = ask_store(&store, 275, &req(275, [5, club, card])).0;
        let block = &a[8..8 + BLOCK];
        assert_eq!(get_bits(block, UC_ENTRY_DATE_BIT, 25), 0);
        let mut want = save[28..28 + BLOCK].to_vec();
        put_bits(&mut want, UC_ENTRY_DATE_BIT, 25, 0);
        assert_eq!(block, &want[..], "only the date changes");
        assert_eq!(u32_at(&a, 3084), sum32(block), "C1");
        let mut z = a.clone();
        put(&mut z, 3088, 0);
        assert_eq!(u32_at(&a, 3088), sum32(&z), "C2");
    }

    /// 2026-11-02: the RANK answer says "no Universal Championship result" (+354) instead of all zeros, so the
    /// client clears a Universal Cup winner's championship entry at the play's start.
    #[test]
    fn the_rank_answer_says_no_championship_result() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = lines.clone();
        let log: Log = Arc::new(move |l: &str| seen.lock().unwrap().push(l.to_owned()));
        let (body, refused) = ask_for_test(&store, 249, &fixture(249, [5, 0, 0]), &log);
        assert_eq!(refused, None);
        assert_eq!(body.len(), 504);
        assert_eq!(body[354], 1, "universalChamp nothingResult");
        assert_eq!(body[8], 0, "the CS rank stays 0");
        // 2026-11-07: a data-match week, never 0, so a cup winner's DATA_VS_ENTRY goes out and keeps its place
        let week = i16::from_le_bytes([body[12], body[13]]);
        assert!(week > 0, "dataVSWeek {week}");
        assert_eq!(week as u16, gt_week(now()).max(1));
        assert!(
            lines.lock().unwrap().iter().any(|l| l
                == &format!("rank answer (250): dataVSWeek {week} - a Universal Cup winner's client sends DATA_VS_ENTRY to the control")),
            "{:?}",
            lines.lock().unwrap()
        );
        let others: Vec<usize> = (4..504)
            .filter(|i| ![12, 13, 354].contains(i) && body[*i] != 0)
            .collect();
        assert!(others.is_empty(), "unexpected bytes at {others:?}");
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
            answer(cmd, &req(cmd, ids), &store, skip, &[], None, &log)
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
            (UC_ENTRY_DATE_BIT, 25, 0),
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
        let (_, body, why) = answer(
            271,
            &req(271, [6, 0, 0]),
            &store,
            Some(&game),
            &[],
            None,
            &log,
        )
        .unwrap();
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
        let body = answer(271, &req(271, [5, 0, 0]), &store, None, &rare, None, &log)
            .unwrap()
            .1;
        assert_eq!(u32_at(&body, 4), 0);
        // World Players List entries: card, registrations 3, owned 0, flag 0
        assert_eq!(&body[1292..1300], &[0xD7, 0x1B, 0, 0, 3, 0, 0, 0]);
        assert_eq!(&body[1300..1308], &[0xEF, 0x03, 0, 0, 3, 0, 0, 0]);
        assert!(body[1308..].iter().all(|b| *b == 0));
        // Aime id 0 (guest): no list
        let guest = answer(271, &req(271, [0, 0, 0]), &store, None, &rare, None, &log)
            .unwrap()
            .1;
        assert!(guest[1292..].iter().all(|b| *b == 0));
        // QR_REGIST, QR_DEL_SERIAL, QR_DEL_CARDID: accepted
        for cmd in [295, 297, 299] {
            let (_, b, why) = answer(cmd, &[0; 32], &store, None, &rare, None, &log).unwrap();
            assert!(why.is_none() && u32_at(&b, 4) == 0, "cmd {cmd}");
        }
    }

    /// Asks `store` through [`answer`] with no VS and no skip club.
    fn ask_store(store: &Mutex<Store>, cmd: u16, p: &[u8]) -> (Vec<u8>, Option<String>) {
        let log: Log = Arc::new(|_: &str| {});
        let (_, body, why) = answer(cmd, p, store, None, &[], None, &log).unwrap();
        (body, why)
    }

    /// A play's start (289 +1) and end (277 with its report) as the log says them; other 289s say nothing.
    #[test]
    fn match_lines_name_the_opponent_score_and_save() {
        let mut start = vec![0u8; 28];
        for (at, v) in [(4, 7), (8, 2), (12, 3), (16, 1)] {
            put(&mut start, at, v);
        }
        assert_eq!(
            match_line(289, &start, None).unwrap(),
            "match start: aime 7 coach 2 card 3"
        );
        put(&mut start, 16, u32::MAX); // -1: the save went through
        assert_eq!(match_line(289, &start, None), None);
        let mut end = vec![0u8; 8572];
        for (at, v) in [(4, 7), (8, 2)] {
            put(&mut end, at, v);
        }
        // a real body's neighbours are nonzero: home 2 then 0x05 0x2D 0x00, away 1 then 0x00 0x3B 0x01 (run log)
        end[3416..3420].copy_from_slice(&[3, 5, 0x2D, 0]);
        end[3428..3432].copy_from_slice(&[1, 0, 0x3B, 1]);
        end[24] = 2;
        end[3300..3303].copy_from_slice(b"CPU");
        end[3356..3366].copy_from_slice(b"BEST ELEVN");
        assert_eq!(
            match_line(277, &end, None).unwrap(),
            "match end: aime 7 coach 2 vs \"BEST ELEVN\" (manager \"CPU\", coach 0) score 3-1 pk 0-0, save kind 2: \
             result save ok"
        );
        assert!(
            match_line(277, &end, Some("block sum 0x1"))
                .unwrap()
                .ends_with("result save FAILED (block sum 0x1)")
        );
    }

    #[test]
    fn the_sale_list_has_ten_valid_entries() {
        let (body, _) = ask_store(
            &Mutex::new(Store::open(Path::new(":memory:")).unwrap()),
            237,
            &[0; 20],
        );
        let entries: Vec<_> = (0..10)
            .map(|k| {
                let e = SHOP_SALE_AT + 16 * k;
                [0, 4, 8, 12].map(|i| u32_at(&body, e + i))
            })
            .collect();
        assert!(
            entries.iter().all(|e| (1..=5).contains(&e[0])),
            "no category 0"
        );
        assert!(entries.iter().all(|e| e[2] == SHOP_FROM && e[3] == SHOP_TO));
        assert!(entries.iter().all(|e| e[0] != 2), "no SE");
        let mut kinds: Vec<_> = entries.iter().map(|e| e[0]).collect();
        kinds.dedup();
        assert_eq!(kinds, [1, 3, 4, 5]);
        assert_eq!(SHOP_SALE_AT + 160, 23_072);
    }

    #[test]
    fn purchases_are_stored_and_applied_to_286_and_276() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gamesvr.db");
        let buy = |store: &Mutex<Store>, cat: u32, item: u32| {
            let mut r = req(315, [5, cat, item]);
            put(&mut r, 16, 99);
            ask_store(store, 315, &r)
        };
        let (club, card);
        {
            let store = Mutex::new(Store::open(&path).unwrap());
            let b = ask_store(&store, 279, &fixture(279, [5, 0, 0])).0;
            (club, card) = (u32_at(&b, 8), u32_at(&b, 12));
            assert_eq!(
                u32_at(&ask_store(&store, 277, &fixture(277, [5, club, card])).0, 4),
                0
            );
            for (cat, item) in [(4, 7), (5, 3), (1, 3), (3, 9)] {
                let (b, why) = buy(&store, cat, item);
                assert!(why.is_none() && u32_at(&b, 4) == 0, "{cat}/{item}");
            }
        }
        // reopened: still bought, and applied for another club of the same Aime id too
        let store = Mutex::new(Store::open(&path).unwrap());
        let a = ask_store(&store, 275, &req(275, [5, club, card])).0;
        assert_eq!((a[5444 + 7], a[5188 + 3], a[5444 + 3]), (1, 1, 0));
        // the stored 277 echoed a 0 for the bit: still 1 after another save
        assert_eq!(
            u32_at(&ask_store(&store, 277, &fixture(277, [5, club, card])).0, 4),
            0
        );
        let a = ask_store(&store, 275, &req(275, [5, club, card])).0;
        assert_eq!((a[5444 + 7], a[5188 + 3]), (1, 1));
        assert_eq!(
            u32_at(&a, 3088),
            {
                let mut z = a.clone();
                put(&mut z, 3088, 0);
                sum32(&z)
            },
            "C2 covers the flags"
        );
        let i = ask_store(&store, 285, &req(285, [5, club, card])).0;
        assert_eq!((i[700], i[701], i[42], i[93]), (3, 3, 9, 0));
        // another Aime id has nothing
        let i = ask_store(&store, 285, &req(285, [6, club, card])).0;
        assert_eq!((i[700], i[42]), (0, 0));
        // a later uniform replaces the earlier one
        buy(&store, 1, 4);
        assert_eq!(ask_store(&store, 285, &req(285, [5, club, card])).0[700], 4);
        // a store error is errCode 1
        store
            .lock()
            .unwrap()
            .0
            .execute_batch("DROP TABLE purchases")
            .unwrap();
        let (b, why) = buy(&store, 4, 1);
        assert!(why.is_some() && u32_at(&b, 4) == 1);
    }

    /// A 311 request: slot 1 and an area id of 0.
    const SLOT: [u8; 8] = [1, 0, 0, 0, 0, 0, 0, 0];

    /// A synthetic 309: every byte a function of its offset, shop id 7.
    fn upload_309(seed: usize) -> Vec<u8> {
        let mut p: Vec<u8> = (0..3368)
            .map(|i| ((i * 13 + seed) % 251) as u8 + 1)
            .collect();
        put(&mut p, 4, 7);
        p
    }

    #[test]
    fn best_eleven_309_comes_back_as_312() {
        let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
        // nothing stored: errCode 1, the CPU team
        let (b, why) = ask_store(&store, 311, &SLOT);
        assert_eq!((b.len(), u32_at(&b, 4), why), (3352, 1, None));
        let (b, why) = ask_store(&store, 309, &upload_309(1));
        assert_eq!((b.len(), u32_at(&b, 4), why), (8, 0, None));
        let p = upload_309(2);
        assert_eq!(u32_at(&ask_store(&store, 309, &p).0, 4), 0);
        let (b, why) = ask_store(&store, 311, &SLOT);
        assert_eq!((b.len(), u32_at(&b, 4), why), (3352, 0, None));
        assert_eq!(b[8..120], [0; 112], "names the client does not read");
        assert_eq!(b[120..148], p[3194..3222]);
        assert_eq!(b[148..152], p[3224..3228]);
        assert_eq!(b[152..3224], p[8..3080]);
        assert_eq!(b[3224..3352], p[3232..3360]);
        // ALL.Net's "none" is -1; a short 309 is refused (errCode 1) and keeps the last good one
        let mut q = upload_309(3);
        put(&mut q, 3224, 0x8000_0000);
        ask_store(&store, 309, &q);
        assert_eq!(u32_at(&ask_store(&store, 311, &SLOT).0, 148), u32::MAX);
        let (b, why) = ask_store(&store, 309, &q[..100]);
        assert!(why.is_some() && u32_at(&b, 4) == 1);
        assert_eq!(ask_store(&store, 311, &SLOT).0[152], q[8]);
    }

    #[test]
    fn frame_cap_rejects_before_buffering() {
        let mut h = [0u8; 40];
        h[16..20].copy_from_slice(&(MAX_FRAME as u32).to_le_bytes());
        assert_eq!(frame_len(&h), Ok(MAX_FRAME));
        h[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(frame_len(&h), Err(u32::MAX as usize));
        h[16..20].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(frame_len(&h), Ok(40), "a size below 40 is floored");
        const { assert!(MAX_FRAME >= 8572 + 40) }
    }

    #[test]
    fn oversized_header_closes_and_per_ip_limit_holds() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let o = Options {
            max_per_ip: 1,
            dump: false,
            ..Options::default()
        };
        let _svc = spawn_opts(l, dir.path().into(), Arc::new(|_| {}), o).unwrap();
        let mut a = std::net::TcpStream::connect(addr).unwrap();
        let mut h = [0u8; 40];
        h[..8].copy_from_slice(&MAGIC);
        h[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        std::io::Write::write_all(&mut a, &h).unwrap();
        a.set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        assert_eq!(a.read(&mut [0u8; 8]).unwrap(), 0, "closed by the server");
        // the slot is free again: a second connection is served, a third at the same time is not
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _b = std::net::TcpStream::connect(addr).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mut c = std::net::TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        assert_eq!(c.read(&mut [0u8; 8]).unwrap(), 0, "over the per-IP limit");
        assert!(
            std::fs::read_dir(dir.path()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("req_")),
            "dump off"
        );
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
