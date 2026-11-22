//! The player's cards, read at runtime from the player's own folders: the catalogue setup made from their game
//! (`catalogue.tsv`, read by its header's names, so both editions' column sets work) and the game's card table
//! (`fpr_table0.txt`, what FPR_Emu.exe hands the game as table 0) with the formation copy the old Python panel keeps
//! beside it (`fpr_panel_state.json`). Ported from the C panel (`native/2010-11/panel/wccfpanel.c`: `parse_table`,
//! `write_table_locked`, `write_panel_state_locked`, `board_add_locked`, `load_catalogue`); both files come out byte
//! for byte as it writes them.

pub mod catalogue;
pub mod codes;
pub mod formations;
pub mod hq;
pub mod lineup;
pub mod links;
pub mod pictures;
pub mod steps;
mod table;

use crate::board::{Aptitude, Board, Card, Position, Rarity};
use emath::{Pos2, pos2};
use std::io;
use std::path::Path;
use table::ON_BENCH;

pub(crate) use table::PITCH;
pub use table::{
    BENCH_X, BENCH_Y, MAX_CARDS, Placed, TableFile, bench_band, mirror_of, parse_table, state_text,
    swap_in, table_text,
};

/// The 4-4-2 spots a card added with no drop point ("+ CARD") takes, with their lines (the C panel's
/// `board_add_locked`, fpr_panel.py's default_spot). Dropped cards lie where they are dropped.
const SPOTS_442: [(Position, f32, f32); PITCH] = [
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
    let field = (on_field < PITCH)
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
        .ok_or(AddRefused::NoPlace)?;
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
        born: String::new(),
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
    let mut spots = [Pos2::ZERO; PITCH];
    let mut pitch = 0;
    for (i, c) in cards.iter().enumerate() {
        if !c.on_bench() {
            if pitch < PITCH {
                slots[pitch] = Some(i);
                spots[pitch] = spot_of(c.x, c.y);
                pitch += 1;
            }
        } else {
            let b = bench_band(c.y);
            if let Some(s) = (0..5)
                .map(|k| PITCH + (b + k) % 5)
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
        rub: None,
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
            let (x, y) = if s < PITCH {
                let sp = board.spots[s];
                let (x, y) = (round3(sp.x * 2.0 - 1.0), round3(sp.y * 2.0 - 1.0));
                if !old.on_bench() && (old.x - x).abs() < 0.0006 && (old.y - y).abs() < 0.0006 {
                    (old.x, old.y)
                } else {
                    (x, y)
                }
            } else if let Some((_, t)) = board.rub.filter(|r| r.0 == s) {
                // the game measures motion as dx * dy, so x and y move together; y stays under half a band, so the
                // card keeps its band whether read by nearest or by floor
                (
                    BENCH_X + 0.05 + 0.05 * t,
                    BENCH_Y[s - PITCH] + 0.09 + 0.07 * t,
                )
            } else if old.on_bench()
                && bench_band(old.y) == s - PITCH
                && (old.x - BENCH_X).abs() < 0.0006
            {
                (old.x, old.y)
            } else {
                (BENCH_X, BENCH_Y[s - PITCH])
            };
            Some(Placed { x, y, ..*old })
        })
        .collect();
    // cards the board took in (dropped from the catalogue): new lines, on the lowest free slot numbers
    for (s, ci) in board.slots.iter().enumerate() {
        let Some(i) = ci.filter(|&i| i >= before.len()) else {
            continue;
        };
        let (x, y) = if s < PITCH {
            let sp = board.spots[s];
            (round3(sp.x * 2.0 - 1.0), round3(sp.y * 2.0 - 1.0))
        } else {
            (BENCH_X, BENCH_Y[s - PITCH])
        };
        let slot = (1..=MAX_CARDS as i32).find(|n| !out.iter().any(|c| c.slot == *n));
        let Some(slot) = slot else {
            // every slot is taken: the card stays off the table rather than taking slot 1 again
            continue;
        };
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
    let c_pic = col(&["image_archive_path"]);
    let [
        season,
        born,
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
        &["birth_date"],
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
            born: get(born),
            pos: match get(pos).as_str() {
                "GK" => Position::Gk,
                "DF" => Position::Df,
                "FW" => Position::Fw,
                _ => Position::Mf,
            },
            rarity,
            code: Some(get(c_code))
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| mvp_mark(&get(c_pic)).into()),
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

/// An older MVP card's mark, which the catalogue has no card code for: its picture's name carries it
/// (`EMVP0506_RONALDINHO_f.dds` -> EMVP, `WMVP_RONALDINHO.dds` -> WMVP, `CMVP_001_NEDVED.dds` -> CMVP, `MVP_1.dds` -> MVP).
fn mvp_mark(picture: &str) -> &str {
    let file = picture.rsplit('/').next().unwrap_or_default();
    file.split(['_', '.'])
        .map(|t| t.trim_end_matches(|c: char| c.is_ascii_digit()))
        .find(|t| ["MVP", "EMVP", "WMVP", "CMVP"].contains(t))
        .unwrap_or_default()
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
    use super::table::tests::p;
    use super::*;
    use std::fs;

    #[test]
    fn older_mvp_cards_get_their_mark_from_the_picture() {
        for (pic, want) in [
            ("data/player_card/EMVP0506_RONALDINHO_f.dds", "EMVP"),
            ("data/player_card/WMVP0506_RONALDINHO_f.dds", "WMVP"),
            ("data/player_card/WMVP_RONALDINHO.dds", "WMVP"),
            ("data/player_card/CMVP_001_NEDVED.dds", "CMVP"),
            ("data/player_card/MVP_1.dds", "MVP"),
            ("data/player_card/CNS0405_192_RONALDINHO.dds", ""),
            ("data/player_card/1718_CardImage_8993.dds", ""),
            ("", ""),
        ] {
            assert_eq!(mvp_mark(pic), want, "{pic}");
        }
    }

    /// A card the board takes in with every slot taken is left off the table, not put on slot 1 again.
    #[test]
    fn a_card_with_no_free_slot_keeps_off_the_table() {
        // 12 on the pitch and 4 substitutes: the board shows 11 and 4, so the file's slots 1-16 are all taken
        let mut before: Vec<Placed> = (0..12)
            .map(|i| p(i + 1, 100 + i as u32, -0.8 + 0.13 * i as f32, 0.0))
            .collect();
        for b in 0..4 {
            before.push(p(13 + b, 200 + b as u32, BENCH_X, BENCH_Y[b as usize]));
        }
        let mut seventeen = before.clone();
        seventeen.push(p(17, 300, BENCH_X, BENCH_Y[4]));
        let board = self::board(&seventeen, &[]);
        assert_eq!(
            board.slots[15],
            Some(16),
            "the last card sits in the free box"
        );
        let t = table_after(&before, &board);
        assert_eq!(t.len(), 16, "no free slot: the card is not added");
        assert_eq!(
            t.iter().filter(|c| c.slot == 1).count(),
            1,
            "no slot reused"
        );
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
        assert_eq!(t.len(), PITCH);
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
        let mut full: Vec<Placed> = (0..PITCH)
            .map(|i| p(i as i32 + 1, 100 + i as u32, 0.0, 0.0))
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
        assert_eq!(b.slots[PITCH + 3], Some(2));
        assert_eq!(b.cards[2].name, "103");
        // nothing moved: the same lines
        assert_eq!(table_after(&t, &b), t);
        // the bench card onto an empty pitch slot (a free 4-4-2 spot), the keeper to the bench's first box, card 102
        // taken off
        let mut m = b.clone();
        m.slots.swap(14, 5);
        m.slots.swap(0, PITCH);
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
        .map(crate::board::series);
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
        assert_eq!(deal(&[], [0, 0, 0]), None); // each tier from its own cards: forza-ui's sample test
    }
}
