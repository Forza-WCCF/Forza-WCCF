use super::table::{BENCH_X, BENCH_Y, MAX_CARDS, ON_BENCH, PITCH, Placed, bench_band};
use crate::board::{Aptitude, Board, Card, Position, Rarity};
use emath::{Pos2, pos2};

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
pub(crate) fn round3(v: f32) -> f32 {
    (v * 1000.0 + 0.5).floor() / 1000.0
}

/// A card number the catalogue does not have: its number on a plain card, the line from where it lies.
pub(crate) fn unknown(no: u32, y: f32) -> Card {
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

#[cfg(test)]
mod tests {
    use super::super::table::tests::p;
    use super::*;

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
}
