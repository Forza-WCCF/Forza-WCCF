//! The player's cards, read at runtime from the player's own folders: the catalogue setup made from their game
//! (`catalogue.tsv`, read by its header's names, so both editions' column sets work) and the game's card table
//! (`fpr_table0.txt`, what FPR_Emu.exe hands the game as table 0) with the formation copy the old Python panel keeps
//! beside it (`fpr_panel_state.json`). Ported from the C panel (`native/2010-11/panel/wccfpanel.c`: `parse_table`,
//! `write_table_locked`, `write_panel_state_locked`, `board_add_locked`, `load_catalogue`); both files come out byte
//! for byte as it writes them.

pub mod catalogue;
mod catalogue_tsv;
pub mod codes;
pub mod formations;
pub mod hq;
pub mod lineup;
pub mod links;
pub mod pictures;
mod placement;
pub mod steps;
mod table;

use crate::board::{Card, Rarity};

pub use catalogue_tsv::load_catalogue;
pub(crate) use placement::round3;
#[cfg(test)]
pub(crate) use placement::unknown;
pub use placement::{AddRefused, add, board, table_after};
pub(crate) use table::PITCH;
pub use table::{
    BENCH_X, BENCH_Y, MAX_CARDS, Placed, TableFile, bench_band, mirror_of, parse_table, state_text,
    swap_in, table_text,
};

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
