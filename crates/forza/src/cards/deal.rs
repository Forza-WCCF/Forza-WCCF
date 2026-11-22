use crate::board::{Card, Rarity};

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
