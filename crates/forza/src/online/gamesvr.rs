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
use chrono::Datelike as _;
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
/// client shows every entry it accepts at once and does no rotation of its own: an entry needs today in `[start,
/// end + 1]` (on the start day from 10:00, on the day after `end` until 02:59) and the item's master row flagged for
/// sale. These are the flagged rows of the 2017–18 tables, item numbers only (= row numbers): ShopUni +12 = 1, GoalBgm
/// +12 and +16 = 1, Banner +8 and +64 = 1, Jersey +8 and +116 = 1. Per category: (category, entries a day, items).
/// SE (2, rows 0..13) stays out: its 286 +93 mapping is unverified ([`APPLY_SE`]).
const SHOP_GOODS: [(u32, usize, &[u32]); 4] = [
    (
        1,
        3,
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17],
    ),
    (
        3,
        2,
        &[38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51],
    ),
    (
        4,
        3,
        &[
            241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255, 256, 257,
            258, 259, 260,
        ],
    ),
    (5, 2, &[38, 39, 40, 54, 55, 56, 90, 140, 141, 142, 143, 144]),
];
// The entries a day fill exactly the list's ten 16-byte slots (160 bytes).
const _: () = {
    let (mut i, mut n) = (0, 0);
    while i < SHOP_GOODS.len() {
        n += SHOP_GOODS[i].1;
        i += 1;
    }
    assert!(n == 10, "SHOP_GOODS must yield exactly 10 entries a day");
};
/// GAME_CTRL +22,896 as a 238 payload offset.
const SHOP_SALE_AT: usize = 16 + 22_896;

/// The 10 sale entries for a 237 at `unix_secs`: each category steps through its items by the Europe/Rome day, so
/// the goods change daily and repeat every few days. The window runs from the day before (a client behind Rome's
/// date) to a year after.
fn shop_sale(unix_secs: i64) -> impl Iterator<Item = [u32; 4]> {
    let today = rome_time(unix_secs).date();
    let n = today.signed_duration_since(GT_EPOCH).num_days().max(0) as usize;
    let ymd = |d: chrono::NaiveDate| d.year() as u32 * 10_000 + d.month() * 100 + d.day();
    let (from, to) = (
        ymd(today - chrono::Days::new(1)),
        // the control asks 237 only at start-up and keeps it, so the end outlives any run (a new list per start-up day)
        ymd(today + chrono::Days::new(365)),
    );
    SHOP_GOODS
        .into_iter()
        .flat_map(move |(cat, per_day, items)| {
            (0..per_day).map(move |j| [cat, items[(per_day * n + j) % items.len()], from, to])
        })
}
/// Whether a bought SE (category 2) is written to 286 +93. Unverified (the reader maps the value through another
/// table than the shop's), so off; [`SHOP_GOODS`] has no SE entry either.
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

/// Europe/Rome wall-clock time (CET or CEST) of a moment.
fn rome_time(unix_secs: i64) -> chrono::NaiveDateTime {
    chrono::DateTime::from_timestamp(unix_secs, 0)
        .unwrap_or_default()
        .with_timezone(&chrono_tz::Europe::Rome)
        .naive_local()
}

/// The Grand Trophy week of a moment: weeks run Monday to Sunday and roll on Sunday at 23:00 Europe/Rome wall-clock
/// time (CET or CEST), like the original's Sunday 23:00 in its own time zone.
fn gt_week(unix_secs: i64) -> u16 {
    let days = (rome_time(unix_secs) + chrono::TimeDelta::hours(1))
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

/// Answer 238's GAME_CTRL (from payload +16): the client's defaults, the unlocked secretaries, the Grand Trophy
/// week and the shop's sale list of `unix_secs`. +8 and +12 (date and time) stay 0: the control may set the PC's
/// clock from them.
fn fill_game_ctrl(body: &mut [u8], unix_secs: i64) {
    for &(at, width, v) in GAME_CTRL_DEFAULTS {
        body[16 + at..16 + at + width].copy_from_slice(&v.to_le_bytes()[..width]);
    }
    body[GT_WEEK_AT..GT_WEEK_AT + 2].copy_from_slice(&gt_week(unix_secs).to_le_bytes());
    body[GT_MATCHES_AT..GT_MATCHES_AT + 5].copy_from_slice(&GT_MATCHES_PER_CLASS);
    body[SECRETARIES].fill(1);
    for (k, entry) in shop_sale(unix_secs).enumerate() {
        for (i, v) in entry.into_iter().enumerate() {
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

/// What the control shows when it has no team ranking: "NO ENTRY" in every name of answer 234, everything else 0.
const NO_ENTRY: &[u8] = b"NO ENTRY\0";
/// The name fields of the team ranking answer 234 (1,092 bytes; the control keeps it whole and feeds its ranking
/// screens from it): u32 +0 / +4 the term (YYYYMMDD from, to), the champion's names char[28] +8, char[56] +36 and
/// +92, u32 +148 / +152 / +156 its figures, u32 +160 a packed emblem, then 8 rows of 116 bytes from +164: char[56],
/// char[56], u32.
fn team_ranking_names() -> impl Iterator<Item = usize> {
    [8, 36, 92]
        .into_iter()
        .chain((0..8).flat_map(|k| [164 + 116 * k, 220 + 116 * k]))
}

const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];
/// What an all-zero answer means for a [`RESP`] entry; the guard test (`zero_answers_are_handled_or_safe`) enforces it.
#[cfg_attr(not(test), allow(dead_code))] // the texts are read by the guard test
enum Zero {
    /// The stand-in fills the answer: it must not be all zero.
    Handled,
    /// Zero is right: the text names the parser (`sub_`/`0x` address) and why. A stale entry (the answer is not
    /// zero) fails the test.
    Safe(&'static str),
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
    // the control's own defaults (control 0x440844 fills the block with "NO ENTRY" names and zeros); the ranking
    // itself stays a parity unit (C6)
    (234, 1092, Zero::Handled),
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
    (
        266,
        200,
        Zero::Safe(
            "ctl 0x436150: no-server default is 200 zero bytes, forwarded unread; client 0x8083E0 only matches ids",
        ),
    ),
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
    (
        300,
        8,
        Zero::Safe(
            "ctl sub_408C50 case 300: errCode only; deletions do not stick, a card can always be registered again",
        ),
    ),
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

/// A Grand Trophy result for a card-in ([`Store::promote`]): the class it moves up from, the closed week's rank, the
/// record, and None for a new grant or the granted week for a result sent again.
type Granted = (u8, u32, i32, Option<u16>);

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

/// SAVE_DATA 276 +6,088/+6,090/+6,092: the "net setting" FK, CK and PK card ids (u16). At card-in the client sets
/// PLAYER_FK/CK/PK = (card no == id) for every player unless the id is 16,383 (sub_816510), so 0 cleared the kickers.
pub const NET_KICKERS_276: [usize; 3] = [6088, 6090, 6092];
/// The client's "no setting" id: it keeps the kickers the save block holds.
pub const NET_KICKER_KEEP: u16 = 16_383;

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
-- import keeps (online::club gives it new ids), unique per card (new_created; shortcut: two clubs a store made in
-- one second before that keep a shared key, fix them by hand if one ever shows)
CREATE TABLE IF NOT EXISTS gt_promotions (
    week     INTEGER NOT NULL,
    aime_id  INTEGER NOT NULL,
    coach_id INTEGER NOT NULL,
    class    INTEGER NOT NULL,
    at       INTEGER NOT NULL,
    rank     INTEGER NOT NULL,
    record   INTEGER NOT NULL,
    created  INTEGER NOT NULL,
    -- the week a card-in of a save below this grant got its result again (Store::promote, once per grant); 0 never
    resent   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (week, aime_id)
);
";

/// `PRAGMA user_version` of a store this code has migrated.
const SCHEMA_VERSION: i64 = 5;

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
/// week it was saved in; 3 -> 4 only adds `gt_promotions` (`CREATE TABLE IF NOT EXISTS`); 4 -> 5 adds its `resent`
/// column (0 on every stored grant). One transaction; nothing is dropped or rewritten, and an older server ignores
/// the tables and the column.
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
    // a version-4 table: SCHEMA made the column for any older store
    let has: bool = tx.query_row(
        "SELECT count(*) > 0 FROM pragma_table_info('gt_promotions') WHERE name = 'resent'",
        [],
        |r| r.get(0),
    )?;
    if !has {
        tx.execute_batch("ALTER TABLE gt_promotions ADD COLUMN resent INTEGER NOT NULL DEFAULT 0")?;
    }
    tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    tx.commit()
}

/// A new club's `created` for `aime`: now, or one past the newest `created` of the card's clubs and Grand Trophy
/// grants when that is later, so no two clubs of one card share it (it keys the Grand Trophy floor across an import,
/// [`Store::promote`]). A clock set back gets the same treatment.
fn new_created(db: &Connection, aime: u32) -> rusqlite::Result<i64> {
    db.query_row(
        "SELECT max(?2, coalesce((SELECT max(created) + 1 FROM coaches WHERE aime_id = ?1), 0),
                    coalesce((SELECT max(created) + 1 FROM gt_promotions WHERE aime_id = ?1), 0))",
        params![aime, now()],
        |r| r.get(0),
    )
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
            "INSERT INTO coaches (aime_id, coach_id, card_id, created, updated) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![aime, coach, card, new_created(&tx, aime)?, now()],
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
             VALUES (?1, ?2, ?3, ?4, ?5, 100, 0, ?6, ?7, ?8)",
            params![aime, coach, card, n1, n2, SKIP_TITLES, new_created(&tx, aime)?, now()],
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
        // the client offers a contract renewal only below 5 unfinished plays (#267): a finished play clears the count
        if kind == 2 {
            tx.execute(
                "DELETE FROM injustice WHERE aime_id = ?1 AND coach_id = ?2",
                params![aime, coach],
            )?;
        }
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
    /// club (same `created`, which [`new_created`] keeps unique per card), never to another club of the card. The
    /// club's latest grant of an earlier week is the floor of its saved class: a save that fell back below it is
    /// promoted from the granted class, or gets that grant's result again, once per grant (`resent`), when this week
    /// brings no promotion: a save that keeps falling below does not get the old result scene at every card-in.
    fn promote(
        &self,
        week: u16,
        aime: u32,
        coach: u32,
        saved: &[u8],
    ) -> rusqlite::Result<Option<Granted>> {
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
            return Ok(Some((next - 1, rank, record, Some(week))));
        }
        // the same club as at the current week's re-issue: its coach, or its re-import once that coach is gone
        let floor: Option<(u8, u32, i32, u16, u16)> = self
            .0
            .query_row(
                "SELECT class, rank, record, week, resent FROM gt_promotions p WHERE aime_id = ?1 AND week < ?2
                 AND (coach_id = ?3 OR (created = (SELECT created FROM coaches WHERE coach_id = ?3)
                      AND NOT EXISTS (SELECT 1 FROM coaches c WHERE c.coach_id = p.coach_id)))
                 ORDER BY week DESC LIMIT 1",
                params![aime, week, coach],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let resend = floor
            .filter(|f| class < f.0 && f.4 == 0)
            .map(|(next, rank, record, at, _)| (next - 1, rank, record, Some(at)));
        let class = floor.map_or(class, |f| class.max(f.0));
        let ranking = self.gt_ranking(last)?;
        let entry = ranking.iter().enumerate().find(|(_, e)| e.0 == aime);
        let need = GT_RAISE.get(usize::from(class)).copied().unwrap_or(0);
        let Some((i, &(_, record))) =
            entry.filter(|(_, e)| need != 0 && i64::from(e.1) >= i64::from(need))
        else {
            if let Some((.., Some(at))) = resend {
                self.0.execute(
                    "UPDATE gt_promotions SET resent = ?3 WHERE week = ?1 AND aime_id = ?2",
                    params![at, aime, week],
                )?;
            }
            return Ok(resend);
        };
        let n = self.0.execute(
            "INSERT INTO gt_promotions (week, aime_id, coach_id, class, at, rank, record, created)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, created FROM coaches WHERE coach_id = ?3
             ON CONFLICT DO NOTHING",
            params![week, aime, coach, class + 1, now(), i as u32 + 1, record],
        )?;
        Ok((n == 1).then_some((class, i as u32 + 1, record, None)))
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
                match promoted {
                    Some((class, rank, record, None)) => log(&format!(
                        "grand trophy: aime {aime} coach {coach} week {week} class {class} -> {} (record {record}, rank {rank})",
                        class + 1
                    )),
                    // the save is below a grant already given: the same result again, not a new promotion
                    Some((class, rank, record, Some(of))) => log(&format!(
                        "grand trophy: aime {aime} coach {coach} week {week} save below the week {of} grant of class {}: \
                         its result sent again, no new promotion (record {record}, rank {rank})",
                        class + 1
                    )),
                    None => {}
                }
                fill_gt_result(body, promoted.map(|(class, rank, ..)| (class, rank)));
            }
            for at in NET_KICKERS_276 {
                body[at..at + 2].copy_from_slice(&NET_KICKER_KEEP.to_le_bytes());
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
    if cmd == 233 {
        // TEAM RANKING: the control copies the whole answer over its defaults whenever the header status is above 0
        // (control 0x442EC0) and shows it on its ranking screens (RNK_TRM/JCP/ACP/EMB_STATE, 0x471D50..0x472100).
        // Zeros there showed empty champion and row names and a 0000-00-00 term; the stand-in keeps no team ranking,
        // so it answers what the control shows without one (its constructor at 0x440844): "NO ENTRY" names, zeros.
        // +0 is the term start, not a slot (the dispatcher routes 234 without one), so the request is not echoed.
        body.fill(0);
        for at in team_ranking_names() {
            body[at..at + NO_ENTRY.len()].copy_from_slice(NO_ENTRY);
        }
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
mod tests;
