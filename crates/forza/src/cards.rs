//! The player's cards, read at runtime from the player's own folders: the catalogue setup made from their game
//! (`catalogue.tsv`, read by its header's names, so both editions' column sets work) and the game's card table
//! (`fpr_table0.txt`, what FPR_Emu.exe hands the game as table 0) with the formation copy the old Python panel keeps
//! beside it (`fpr_panel_state.json`). Ported from the C panel (`native/2010-11/panel/wccfpanel.c`: `parse_table`,
//! `write_table_locked`, `write_panel_state_locked`, `board_add_locked`, `load_catalogue`); both files come out byte
//! for byte as it writes them.

pub mod catalogue;
mod codes;
pub mod hq;
pub mod links;
pub mod pictures;
pub mod steps;

use crate::ui::board::{Aptitude, Board, Card, Position, Rarity};
use egui::{Pos2, pos2};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// One card on the table: FPR_Emu's slot (1-20), the card number, and where it lies: x -1..1 touchline to touchline,
/// y -1 (the far goal line) .. 1 (the own goal line); x past 1.16 is the bench strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub slot: i32,
    pub no: u32,
    pub x: f32,
    pub y: f32,
}

impl Placed {
    pub fn on_bench(&self) -> bool {
        self.x > ON_BENCH
    }
}

/// The table holds at most 16 cards: 11 on the pitch and 5 on the bench.
pub const MAX_CARDS: usize = 16;
/// Bench cards lie at x 1.24, y on one of the game's five band spots.
pub const BENCH_X: f32 = 1.24;
pub const BENCH_Y: [f32; 5] = [-0.68, -0.34, 0.0, 0.34, 0.68];
const ON_BENCH: f32 = 1.16;

/// The 4-4-2 spots a card added with no drop point ("+ CARD") takes, with their lines (the C panel's
/// `board_add_locked`, fpr_panel.py's default_spot). Dropped cards lie where they are dropped.
const SPOTS_442: [(Position, f32, f32); 11] = [
    (Position::Gk, 0.0, 0.9),
    (Position::Df, -0.6, 0.55),
    (Position::Df, -0.2, 0.6),
    (Position::Df, 0.2, 0.6),
    (Position::Df, 0.6, 0.55),
    (Position::Mf, -0.6, 0.1),
    (Position::Mf, -0.2, 0.15),
    (Position::Mf, 0.2, 0.15),
    (Position::Mf, 0.6, 0.1),
    (Position::Fw, -0.25, -0.45),
    (Position::Fw, 0.25, -0.45),
];

/// The bench band whose spot is nearest `y`.
pub fn bench_band(y: f32) -> usize {
    // the first of two as near, as the C panel's loop
    (0..5)
        .min_by(|&a, &b| (BENCH_Y[a] - y).abs().total_cmp(&(BENCH_Y[b] - y).abs()))
        .unwrap_or(0)
}

// ---- the table file ----

/// The table file's text -> its cards, by FPR_Emu's rules: `#` starts a comment, commas count as spaces, a line is
/// "slot card x y" with slot 1-20 and card 1 or more; anything else is skipped; at most 16 cards.
pub fn parse_table(text: &[u8]) -> Vec<Placed> {
    // ponytail: a number with trailing junk ("3x") is skipped, where the C panel's sscanf took its leading digits
    String::from_utf8_lossy(text)
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or("").replace(',', " ");
            let mut w = line.split_whitespace();
            let slot: i32 = w.next()?.parse().ok()?;
            let no: i64 = w.next()?.parse().ok()?;
            let x: f32 = w.next()?.parse().ok()?;
            let y: f32 = w.next()?.parse().ok()?;
            ((1..=20).contains(&slot) && no >= 1).then_some(Placed {
                slot,
                no: u32::try_from(no).ok()?,
                x,
                y,
            })
        })
        .take(MAX_CARDS)
        .collect()
}

/// The table file as the C panel writes it: its first line, then "slot card x y" with three decimals.
pub fn table_text(cards: &[Placed]) -> String {
    let mut t = String::from(
        "# written by the in-game card board (wccfpanel.dll) - read by FPR_Emu.exe (table 0)\n",
    );
    for c in cards {
        t += &format!("{} {} {:.3} {:.3}\n", c.slot, c.no, c.x, c.y);
    }
    t
}

/// fpr_panel_state.json's "mirror" choice, read as the C panel reads it: the first `"mirror"`, its colon, then `t`.
pub fn mirror_of(state: &str) -> bool {
    state
        .find("\"mirror\"")
        .and_then(|i| state[i..].find(':').map(|j| i + j + 1))
        .is_some_and(|i| {
            state[i..]
                .trim_start_matches([' ', '\t', '\r', '\n'])
                .starts_with('t')
        })
}

/// fpr_panel_state.json as the C panel writes it: the table's cards with x unmirrored (the table holds the mirrored x
/// when "mirror" is on), and the mirror choice kept.
pub fn state_text(cards: &[Placed], mirror: bool) -> String {
    let mut t = String::from("{\n \"placed\": [");
    for (i, c) in cards.iter().enumerate() {
        let x = if mirror && c.x <= ON_BENCH { -c.x } else { c.x };
        t += &format!(
            "{}\n  {{\"slot\": {}, \"no\": {}, \"x\": {:.3}, \"y\": {:.3}}}",
            if i > 0 { "," } else { "" },
            c.slot,
            c.no,
            x,
            c.y
        );
    }
    t += &format!("\n ],\n \"mirror\": {mirror}\n}}\n");
    t
}

/// The table and the formation copy in the game's own folder, and when we last read or wrote the table.
pub struct TableFile {
    pub table: PathBuf,
    pub state: PathBuf,
    stamp: Option<SystemTime>,
    /// A write could not be swapped in: the next try writes again, and the file is not read back meanwhile.
    pub pending: bool,
}

impl TableFile {
    /// `fpr_table0.txt` and `fpr_panel_state.json` in `dir` (the game's own folder: seat1\, or a test's).
    pub fn in_dir(dir: &Path) -> TableFile {
        TableFile {
            table: dir.join("fpr_table0.txt"),
            state: dir.join("fpr_panel_state.json"),
            stamp: None,
            pending: false,
        }
    }

    fn modified(&self) -> Option<SystemTime> {
        fs::metadata(&self.table).and_then(|m| m.modified()).ok()
    }

    /// Someone else wrote the table since we last read or wrote it (FPR_Emu's panel, a hand edit).
    pub fn changed(&self) -> bool {
        !self.pending && self.modified().is_some_and(|m| Some(m) != self.stamp)
    }

    /// The table's cards; `None` when it cannot be read (missing, or being swapped).
    pub fn read(&mut self) -> Option<Vec<Placed>> {
        let m = self.modified()?;
        let cards = parse_table(&fs::read(&self.table).ok()?);
        self.stamp = Some(m);
        Some(cards)
    }

    /// The table written whole (a temporary file, then one swap: FPR_Emu never sees half a file), then the formation
    /// copy. The temporary file gets a write time after the last one: FPR_Emu reads again only when the time
    /// changes, and two moves in one clock tick would look like one.
    pub fn write(&mut self, cards: &[Placed]) -> io::Result<()> {
        self.pending = true;
        let tmp = with_suffix(&self.table, ".board");
        let mut when = SystemTime::now();
        if let Some(last) = self.stamp
            && when <= last
        {
            when = last + Duration::from_millis(1);
        }
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(table_text(cards).as_bytes())?;
            f.set_modified(when)?;
        }
        swap_in(&tmp, &self.table, 4, 1)?;
        self.stamp = Some(when);
        self.pending = false;
        self.write_state(cards)
    }

    fn write_state(&self, cards: &[Placed]) -> io::Result<()> {
        let mirror = fs::read(&self.state).is_ok_and(|b| mirror_of(&String::from_utf8_lossy(&b)));
        let tmp = with_suffix(&self.state, ".board");
        fs::write(&tmp, state_text(cards, mirror))?;
        swap_in(&tmp, &self.state, 20, 2)
    }
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

/// `tmp` over `to` (MoveFileEx with replace); the reader may hold `to` this instant: a few tries a moment apart.
fn swap_in(tmp: &Path, to: &Path, tries: u32, ms: u64) -> io::Result<()> {
    let mut last = Ok(());
    for _ in 0..tries {
        last = fs::rename(tmp, to);
        if last.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(ms));
    }
    last
}

/// Why a card did not go on the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddRefused {
    AlreadyOn,
    Full,
    NoPlace,
}

/// A card added with no drop point, where fpr_panel.py would put it: its own line's free 4-4-2 spot, else any line's
/// (DF, MF, FW, GK), else the first free bench box; with the lowest free slot number. At most 16 cards. A pitch spot
/// is free with no pitch card within 0.15 of it (cards lie anywhere, so not only exactly on it); a bench box with no
/// card on its band.
pub fn add(cards: &mut Vec<Placed>, no: u32, pos: Position) -> Result<Placed, AddRefused> {
    if cards.iter().any(|c| c.no == no) {
        return Err(AddRefused::AlreadyOn);
    }
    if cards.len() >= MAX_CARDS {
        return Err(AddRefused::Full);
    }
    let taken = |x: f32, y: f32| {
        cards.iter().any(|c| {
            if x > ON_BENCH {
                c.on_bench() && bench_band(c.y) == bench_band(y)
            } else {
                !c.on_bench() && (c.x - x).abs() < 0.15 && (c.y - y).abs() < 0.15
            }
        })
    };
    let on_field = cards.iter().filter(|c| !c.on_bench()).count();
    let order = [pos, Position::Df, Position::Mf, Position::Fw, Position::Gk];
    let field = (on_field < 11)
        .then(|| {
            order.iter().find_map(|line| {
                SPOTS_442
                    .iter()
                    .find(|(l, x, y)| l == line && !taken(*x, *y))
                    .map(|(_, x, y)| (*x, *y))
            })
        })
        .flatten();
    let (x, y) = field
        .or_else(|| {
            BENCH_Y
                .iter()
                .find(|&&y| !taken(BENCH_X, y))
                .map(|&y| (BENCH_X, y))
        })
        .ok_or(AddRefused::NoPlace)?;
    let slot = (1..=MAX_CARDS as i32)
        .find(|s| !cards.iter().any(|c| c.slot == *s))
        .unwrap_or(1);
    let p = Placed { slot, no, x, y };
    cards.push(p);
    Ok(p)
}

// ---- the table on the formation board ----

/// A table point as a board spot: 0..1 across, 0 (attack) .. 1 (own goal) down.
fn spot_of(x: f32, y: f32) -> Pos2 {
    pos2((x + 1.0) / 2.0, (y + 1.0) / 2.0)
}

/// Three decimals, the file's precision, rounded as the C panel rounds.
fn round3(v: f32) -> f32 {
    (v * 1000.0 + 0.5).floor() / 1000.0
}

/// A card number the catalogue does not have: its number on a plain card, the line from where it lies.
fn unknown(no: u32, y: f32) -> Card {
    let pos = match y {
        y if y < -0.32 => Position::Fw,
        y if y < 0.28 => Position::Mf,
        y if y < 0.76 => Position::Df,
        _ => Position::Gk,
    };
    Card {
        number: no,
        record: None,
        name: no.to_string(),
        short: no.to_string(),
        club: String::new(),
        country: String::new(),
        season: String::new(),
        pos,
        rarity: Rarity::Regular,
        code: String::new(),
        secret: false,
        stats: [0; 6],
        total: 0,
        aptitude: Aptitude::default(),
    }
}

/// The table on the board: `board.cards[i]` is `cards[i]`. Pitch cards fill the pitch slots in the table's order at
/// their own places; bench cards take their band's box (or the next free one); the empty pitch slots (not drawn) keep
/// the free 4-4-2 spots, for tests that aim at them. Cards past what the board holds (more than 11 on the pitch, or 5 on the bench) stay in the table
/// without a slot.
pub fn board(cards: &[Placed], catalogue: &[Card]) -> Board {
    let mut slots = [None; 16];
    let mut spots = [Pos2::ZERO; 11];
    let mut pitch = 0;
    for (i, c) in cards.iter().enumerate() {
        if !c.on_bench() {
            if pitch < 11 {
                slots[pitch] = Some(i);
                spots[pitch] = spot_of(c.x, c.y);
                pitch += 1;
            }
        } else {
            let b = bench_band(c.y);
            if let Some(s) = (0..5)
                .map(|k| 11 + (b + k) % 5)
                .find(|&s| slots[s].is_none())
            {
                slots[s] = Some(i);
            }
        }
    }
    // the empty pitch slots on the 4-4-2 spots no card is near
    let near = |x: f32, y: f32| {
        cards
            .iter()
            .any(|c| !c.on_bench() && (c.x - x).abs() < 0.2 && (c.y - y).abs() < 0.2)
    };
    let mut free = SPOTS_442
        .iter()
        .filter(|(_, x, y)| !near(*x, *y))
        .chain(SPOTS_442.iter().filter(|(_, x, y)| near(*x, *y)));
    for s in spots.iter_mut().skip(pitch) {
        if let Some((_, x, y)) = free.next() {
            *s = spot_of(*x, *y);
        }
    }
    Board {
        cards: cards
            .iter()
            .map(|c| {
                catalogue
                    .iter()
                    .find(|k| k.number == c.no)
                    .cloned()
                    .unwrap_or_else(|| unknown(c.no, c.y))
            })
            .collect(),
        slots,
        spots,
    }
}

/// The board after a move, back to the table `before` it came from: a card in a pitch slot lies at that slot's spot,
/// a bench card on its box's band; a card shown before and in no slot now was taken off. A card whose place did not
/// change keeps its line as it was, so a move rewrites only what moved.
pub fn table_after(before: &[Placed], board: &Board) -> Vec<Placed> {
    let shown = self::board(before, &[]).slots;
    let mut out: Vec<Placed> = before
        .iter()
        .enumerate()
        .filter_map(|(i, old)| {
            let Some(s) = board.slots.iter().position(|&s| s == Some(i)) else {
                // taken off, unless the board never had room for it
                return (!shown.contains(&Some(i))).then_some(*old);
            };
            let (x, y) = if s < 11 {
                let sp = board.spots[s];
                let (x, y) = (round3(sp.x * 2.0 - 1.0), round3(sp.y * 2.0 - 1.0));
                if !old.on_bench() && (old.x - x).abs() < 0.0006 && (old.y - y).abs() < 0.0006 {
                    (old.x, old.y)
                } else {
                    (x, y)
                }
            } else if old.on_bench() && bench_band(old.y) == s - 11 {
                (old.x, old.y)
            } else {
                (BENCH_X, BENCH_Y[s - 11])
            };
            Some(Placed { x, y, ..*old })
        })
        .collect();
    // cards the board took in (dropped from the catalogue): new lines, on the lowest free slot numbers
    for (s, ci) in board.slots.iter().enumerate() {
        let Some(i) = ci.filter(|&i| i >= before.len()) else {
            continue;
        };
        let (x, y) = if s < 11 {
            let sp = board.spots[s];
            (round3(sp.x * 2.0 - 1.0), round3(sp.y * 2.0 - 1.0))
        } else {
            (BENCH_X, BENCH_Y[s - 11])
        };
        let slot = (1..=MAX_CARDS as i32)
            .find(|n| !out.iter().any(|c| c.slot == *n))
            .unwrap_or(1);
        out.push(Placed {
            slot,
            no: board.cards[i].number,
            x,
            y,
        });
    }
    out
}

// ---- the catalogue ----

/// The white ATLE-RE reprints the 2017-18 game treats as legends beside every ALL TIME LEGEND card (its own list of
/// card numbers, read from the client).
const LEGEND_REPRINTS: [u32; 18] = [
    4659, 5747, 5748, 5749, 5752, 5754, 5755, 6151, 6153, 6154, 6299, 6417, 6635, 6720, 6721, 7131,
    7132, 7133,
];

/// `catalogue.tsv` (one card per line, tab-separated, UTF-8, a header naming the columns) -> its cards, in the
/// file's order. Columns by name: card_no, season, rarity_name, is_placeholder (1 = skipped), name_full_latin,
/// name_short_latin, position_name, club_name, nationality_name, stat_offence .. stat_stamina, stats_total, and the
/// record_index (the card's player_data.bin record, for [`links`]), card_code (2017-18's printed code), the
/// 16 role ratings as the first 16 numbers of hidden_params (2010-11) or hidden_4df_523 (2017-18), kept only when
/// all are 1-10; else the 12 position flags of tail_5a4_61f (hex) rate their roles 8.
pub fn load_catalogue(path: &Path) -> io::Result<Vec<Card>> {
    let mut r = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .quoting(false)
        .flexible(true)
        .from_path(path)
        .map_err(io::Error::other)?;
    let head = r.byte_headers().map_err(io::Error::other)?.clone();
    let col = |names: &[&str]| {
        head.iter()
            .position(|h| names.iter().any(|n| h == n.as_bytes()))
    };
    let c_no = col(&["card_no"]).ok_or_else(|| io::Error::other("no card_no column"))?;
    let c_rec = col(&["record_index"]);
    let c_code = col(&["card_code"]);
    let [
        season,
        rarity,
        placeholder,
        full,
        short,
        pos,
        club,
        nat,
        total,
        roles,
        tail,
    ] = [
        &["season"][..],
        &["rarity_name"],
        &["is_placeholder"],
        &["name_full_latin"],
        &["name_short_latin"],
        &["position_name"],
        &["club_name"],
        &["nationality_name"],
        &["stats_total"],
        &["hidden_params", "hidden_4df_523"],
        &["tail_5a4_61f"],
    ]
    .map(col);
    let stats = [
        "stat_offence",
        "stat_defence",
        "stat_technique",
        "stat_power",
        "stat_speed",
        "stat_stamina",
    ]
    .map(|n| col(&[n]));
    let mut out = Vec::new();
    for rec in r.byte_records() {
        let Ok(rec) = rec else { continue };
        let get = |c: Option<usize>| {
            c.and_then(|c| rec.get(c))
                .map(|b| String::from_utf8_lossy(b).trim().to_owned())
                .unwrap_or_default()
        };
        let num = |c: Option<usize>| get(c).parse::<u8>().unwrap_or(0);
        let Ok(number) = get(Some(c_no)).parse::<u32>() else {
            continue;
        };
        if number == 0 || get(placeholder) == "1" {
            continue;
        }
        let ratings: Vec<u8> = get(roles)
            .split_whitespace()
            .take(16)
            .map(|v| v.parse().unwrap_or(0))
            .collect();
        let rarity = match get(rarity).as_str() {
            "ALL TIME LEGEND" => Rarity::AllTime,
            "LEGEND" => Rarity::Legend,
            "RARE" => Rarity::Rare,
            "SPECIAL" => Rarity::Special,
            _ => Rarity::Regular,
        };
        let mut aptitude = Aptitude::default();
        if ratings.len() == 16 && ratings.iter().all(|r| (1..=10).contains(r)) {
            aptitude.roles.copy_from_slice(&ratings);
        } else if let Some(flags) = get(tail).get(10..34) {
            // 2017-18 cards of editions 36+ have no ratings, only the 12 position flags (tail bytes 5..17)
            aptitude = Aptitude::from_flags(flags);
        }
        out.push(Card {
            number,
            record: get(c_rec).parse().ok(),
            name: get(full),
            short: get(short),
            club: get(club),
            country: get(nat),
            season: get(season),
            pos: match get(pos).as_str() {
                "GK" => Position::Gk,
                "DF" => Position::Df,
                "FW" => Position::Fw,
                _ => Position::Mf,
            },
            rarity,
            code: get(c_code),
            // 2017-18 only (the catalogue with card codes): 2010-11's legends print their stats
            secret: c_code.is_some()
                && (rarity == Rarity::AllTime || LEGEND_REPRINTS.contains(&number)),
            stats: stats.map(num),
            total: num(total),
            aptitude,
        });
    }
    Ok(out)
}

// ---- the card deal after a match ----

/// A card's tier in the deal (`wccfpanel.c` `card_tier`): 0 better than black, 1 white (REGULAR), 2 black (SPECIAL).
pub fn deal_class(c: &Card) -> u8 {
    match c.rarity {
        Rarity::Regular => 1,
        Rarity::Special => 2,
        _ => 0,
    }
}

/// The tier dealt from two random numbers, as `deal_tier`: 2.5 % better than black; of the rest 70 % white, 30 % black.
pub fn deal_tier(a: u32, b: u32) -> u8 {
    if a % 1000 < 25 {
        0
    } else if b % 100 < 70 {
        1
    } else {
        2
    }
}

/// A card dealt after a match (the player's wish, 2026-10-09: only shown, nothing saved): a tier by [`deal_tier`], then
/// a catalogue card of that tier, every season; `None` when the catalogue has none of it. Three random numbers.
pub fn deal(cards: &[Card], r: [u32; 3]) -> Option<usize> {
    let tier = deal_tier(r[0], r[1]);
    let n = cards.iter().filter(|c| deal_class(c) == tier).count();
    if n == 0 {
        return None;
    }
    let k = r[2] as usize % n;
    cards
        .iter()
        .enumerate()
        .filter(|(_, c)| deal_class(c) == tier)
        .nth(k)
        .map(|(i, _)| i)
}

/// [`deal`] with the system's random numbers.
pub fn deal_now(cards: &[Card]) -> Option<usize> {
    let mut r = [0u32; 3];
    for x in &mut r {
        *x = getrandom::u32().unwrap_or(0);
    }
    deal(cards, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(slot: i32, no: u32, x: f32, y: f32) -> Placed {
        Placed { slot, no, x, y }
    }

    /// What the C panel's `write_table_locked` writes for these cards: its header line, then
    /// `"%d %d %.3f %.3f\n"` per card (the MSVC runtime's printf), no carriage returns.
    #[test]
    fn the_table_file_is_the_c_panels() {
        let cards = [
            p(1, 101, 0.0, 0.9),
            p(2, 2345, -0.6, 0.55),
            p(3, 7, 1.24, -0.68),
            p(16, 40000, 0.25, -0.45),
            p(5, 9, -0.0, 1.0),
        ];
        let want = b"# written by the in-game card board (wccfpanel.dll) - read by FPR_Emu.exe (table 0)\n\
1 101 0.000 0.900\n\
2 2345 -0.600 0.550\n\
3 7 1.240 -0.680\n\
16 40000 0.250 -0.450\n\
5 9 -0.000 1.000\n";
        assert_eq!(table_text(&cards).as_bytes(), want);
        assert_eq!(parse_table(want), cards);
        assert_eq!(
            table_text(&[]).as_bytes(),
            &b"# written by the in-game card board (wccfpanel.dll) - read by FPR_Emu.exe (table 0)\n"[..]
        );
    }

    /// FPR_Emu's reading rules (`parse_table`): comments, commas, bad slots and card numbers, short lines, 16 at most.
    #[test]
    fn the_table_is_read_by_fpr_emus_rules() {
        let text = b"# a note\r\n1,101,0.1,0.2 # the keeper\r\n0 5 0 0\n21 5 0 0\n3 0 0 0\n4 5 0\n\n  7   8  -0.5  0.25\n";
        assert_eq!(
            parse_table(text),
            [p(1, 101, 0.1, 0.2), p(7, 8, -0.5, 0.25)]
        );
        let many: String = (1..=20).map(|s| format!("{s} {s} 0 0\n")).collect();
        assert_eq!(parse_table(many.as_bytes()).len(), 16);
    }

    /// `write_panel_state_locked`: the mirror choice as the old file had it, a pitch card's x unmirrored, a bench
    /// card's x as it is; no cards gives an empty list.
    #[test]
    fn the_formation_copy_is_the_c_panels() {
        let cards = [p(1, 101, 0.6, 0.55), p(2, 7, 1.24, 0.34)];
        assert_eq!(
            state_text(&cards, false),
            "{\n \"placed\": [\n  {\"slot\": 1, \"no\": 101, \"x\": 0.600, \"y\": 0.550},\n  \
             {\"slot\": 2, \"no\": 7, \"x\": 1.240, \"y\": 0.340}\n ],\n \"mirror\": false\n}\n"
        );
        assert_eq!(
            state_text(&cards, true),
            "{\n \"placed\": [\n  {\"slot\": 1, \"no\": 101, \"x\": -0.600, \"y\": 0.550},\n  \
             {\"slot\": 2, \"no\": 7, \"x\": 1.240, \"y\": 0.340}\n ],\n \"mirror\": true\n}\n"
        );
        assert_eq!(
            state_text(&[], false),
            "{\n \"placed\": [\n ],\n \"mirror\": false\n}\n"
        );
        assert!(mirror_of("{\"placed\": [], \"mirror\":\n  true}"));
        assert!(!mirror_of("{\"mirror\": false}"));
        assert!(!mirror_of("{}"));
    }

    /// The table and its copy written beside each other, temporary files gone, the copy keeping the mirror; a
    /// second write in the same instant still gets a later write time.
    #[test]
    fn the_files_are_swapped_in_whole() {
        let d = tempfile::tempdir().unwrap();
        let mut f = TableFile::in_dir(d.path());
        fs::write(&f.state, "{\"placed\": [], \"mirror\": true}").unwrap();
        let cards = vec![p(1, 101, 0.5, 0.5)];
        f.write(&cards).unwrap();
        let first = fs::metadata(&f.table).unwrap().modified().unwrap();
        f.write(&cards).unwrap();
        let second = fs::metadata(&f.table).unwrap().modified().unwrap();
        assert!(second > first, "{first:?} then {second:?}");
        assert!(!f.changed());
        assert_eq!(fs::read_to_string(&f.table).unwrap(), table_text(&cards));
        assert_eq!(
            fs::read_to_string(&f.state).unwrap(),
            state_text(&cards, true)
        );
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 2);
        assert_eq!(f.read().unwrap(), cards);
    }

    /// `board_add_locked`'s places: the card's own line first, then DF MF FW GK, then the bench; the lowest free slot.
    #[test]
    fn a_new_card_goes_where_fpr_panel_py_puts_it() {
        let mut t = vec![p(1, 50, 0.0, 0.9)];
        assert_eq!(add(&mut t, 60, Position::Gk), Ok(p(2, 60, -0.6, 0.55)));
        assert_eq!(add(&mut t, 61, Position::Fw), Ok(p(3, 61, -0.25, -0.45)));
        assert_eq!(add(&mut t, 61, Position::Fw), Err(AddRefused::AlreadyOn));
        for no in 70..78 {
            add(&mut t, no, Position::Mf).unwrap();
        }
        assert_eq!(t.len(), 11);
        assert_eq!(add(&mut t, 80, Position::Fw), Ok(p(12, 80, 1.24, -0.68)));
        for no in 81..85 {
            add(&mut t, no, Position::Df).unwrap();
        }
        assert_eq!(add(&mut t, 90, Position::Df), Err(AddRefused::Full));
        t.remove(0);
        assert_eq!(add(&mut t, 91, Position::Df), Ok(p(1, 91, 0.0, 0.9)));
        // a keeper moved a little off its spot still holds it: the next card does not land on top of it
        let mut t = vec![p(1, 50, 0.05, 0.84), p(2, 51, 1.24, -0.7)];
        assert_eq!(add(&mut t, 60, Position::Gk), Ok(p(3, 60, -0.6, 0.55)));
        let mut full: Vec<Placed> = (0..11)
            .map(|i| p(i + 1, 100 + i as u32, 0.0, 0.0))
            .collect();
        full.push(p(12, 200, 1.24, -0.7));
        assert_eq!(add(&mut full, 60, Position::Gk), Ok(p(13, 60, 1.24, -0.34)));
    }

    fn sample() -> Vec<Placed> {
        vec![
            p(1, 101, 0.0, 0.9),
            p(2, 102, -0.6, 0.55),
            p(3, 103, 1.24, 0.34),
            p(4, 104, 0.123, -0.456),
        ]
    }

    #[test]
    fn the_table_goes_on_the_board_and_back() {
        let t = sample();
        let b = board(&t, &[]);
        assert_eq!(b.slots[..3], [Some(0), Some(1), Some(3)]);
        assert_eq!(b.slots[11 + 3], Some(2));
        assert_eq!(b.cards[2].name, "103");
        // nothing moved: the same lines
        assert_eq!(table_after(&t, &b), t);
        // the bench card onto an empty pitch slot (a free 4-4-2 spot), the keeper to the bench's first box, card 102
        // taken off
        let mut m = b.clone();
        m.slots.swap(14, 5);
        m.slots.swap(0, 11);
        m.slots[1] = None;
        let sp = m.spots[5];
        let after = table_after(&t, &m);
        assert_eq!(
            after,
            [
                p(1, 101, 1.24, -0.68),
                p(3, 103, round3(sp.x * 2.0 - 1.0), round3(sp.y * 2.0 - 1.0)),
                p(4, 104, 0.123, -0.456),
            ]
        );
        assert!(!after[1].on_bench());
        // a swap: the two trade places
        let mut s = b.clone();
        s.slots.swap(0, 2);
        let after = table_after(&t, &s);
        assert_eq!((after[0].x, after[0].y), (0.123, -0.456));
        assert_eq!((after[3].x, after[3].y), (0.0, 0.9));
    }

    /// The catalogue by its header's names, in either edition's column set; placeholders skipped; the roles kept only
    /// when all 16 are 1-10.
    #[test]
    fn the_catalogue_is_read_by_column_names() {
        let d = tempfile::tempdir().unwrap();
        let roles: String = (0..16)
            .map(|i| (1 + i % 10).to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let path = d.path().join("catalogue.tsv");
        let head = "card_no\trecord_index\tseason\trarity_name\tis_placeholder\tname_full_latin\tname_short_latin\t\
                    position_name\tclub_name\tnationality_name\tstat_offence\tstat_defence\tstat_technique\tstat_power\t\
                    stat_speed\tstat_stamina\tstats_total\thidden_4df_523\tcard_code\ttail_5a4_61f\n";
        let rows = format!(
            "12\t0\t10-11\tLEGEND\t0\tAldo Bàrtoli\tBÀRTOLI\tFW\tPorto Azzurro\tItalia\t18\t5\t16\t14\t17\t12\t82\t{roles} 99 3\t\t\n\
             13\t1\t10-11\tREGULAR\t1\tNobody\tNOBODY\tMF\t\t\t1\t1\t1\t1\t1\t1\t60\t\n\
             14\t2\t15-16\tALL TIME LEGEND\t0\tKenji Sato\tSATO\tGK\tKitakaze\t日本\t3\t15\t9\t12\t8\t10\t71\t0 0 0\tHOLE\t\n\
             15\t3\t17-18\tREGULAR\t0\tNew Card\tNEW\tMF\tX\tY\t1\t1\t1\t1\t1\t1\t60\t0 0 0\t\t0000000000010100000000000000000000000000"
        );
        fs::write(&path, format!("{head}{rows}")).unwrap();
        let cat = load_catalogue(&path).unwrap();
        assert_eq!(cat.len(), 3);
        assert_eq!((cat[0].record, cat[1].record), (Some(0), Some(2)));
        let a = &cat[0];
        assert_eq!(
            (a.number, a.name.as_str(), a.short.as_str(), a.pos, a.rarity),
            (12, "Aldo Bàrtoli", "BÀRTOLI", Position::Fw, Rarity::Legend)
        );
        assert_eq!((a.stats, a.total), ([18, 5, 16, 14, 17, 12], 82));
        assert_eq!(a.aptitude.roles[..3], [1, 2, 3]);
        assert_eq!(a.aptitude.roles[15], 6);
        // no ratings, flags CF and SS (hex bytes 5 and 6 of the tail): both roles rated 8
        let c = &cat[2];
        assert_eq!((c.aptitude.roles[0], c.aptitude.roles[3]), (8, 8));
        assert_eq!(c.aptitude.roles.iter().filter(|&&r| r > 0).count(), 2);
        let b = &cat[1];
        assert_eq!(
            (b.country.as_str(), b.rarity, b.pos),
            ("日本", Rarity::AllTime, Position::Gk)
        );
        assert!(!b.aptitude.known());
        assert_eq!(
            (a.code.as_str(), b.code.as_str(), b.series()),
            ("", "HOLE", "HOLE")
        );
        // the ALL TIME LEGEND keeps its stats secret, the LEGEND does not
        assert_eq!((a.secret, b.secret), (false, true));
        let series = [
            "WGK-3",
            "MVP-1",
            "KLE",
            "12",
            "ATLE-RE-4",
            "P WOS-2",
            "JT-A11",
            "",
        ]
        .map(crate::ui::board::series);
        assert_eq!(
            series,
            ["WGK", "MVP", "KLE", "", "ATLE-RE", "P WOS", "JT-A", ""]
        );
    }

    /// 200,000 deals land 2.5 / 68.25 / 29.25 % (within 0.3 points), as the C panel's deal test; each tier from its
    /// own cards.
    #[test]
    fn deal_odds_and_tiers() {
        let mut x = 12345u32;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let mut n = [0u32; 3];
        for _ in 0..200_000 {
            n[deal_tier(next(), next()) as usize] += 1;
        }
        for (k, want) in [2.5, 68.25, 29.25].into_iter().enumerate() {
            let got = f64::from(n[k]) * 100.0 / 200_000.0;
            assert!((got - want).abs() <= 0.3, "tier {k}: {got} % (want {want})");
        }
        let cards = crate::ui::sample::cards();
        for r in [[0, 0, 7], [999, 0, 3], [999, 99, 5]] {
            let i = deal(&cards, r).expect("the sample has every tier");
            assert_eq!(deal_class(&cards[i]), deal_tier(r[0], r[1]));
        }
        assert_eq!(deal(&[], [0, 0, 0]), None);
    }
}
