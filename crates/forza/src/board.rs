//! The player cards and the card table as data: what the catalogue lists, the board holds and `forza-ui` draws.

use emath::{Pos2, Rect, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Gk,
    Df,
    Mf,
    Fw,
}

impl Position {
    pub const ALL: [Position; 4] = [Position::Gk, Position::Df, Position::Mf, Position::Fw];

    pub fn code(self) -> &'static str {
        match self {
            Position::Gk => "GK",
            Position::Df => "DF",
            Position::Mf => "MF",
            Position::Fw => "FW",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rarity {
    AllTime,
    Legend,
    Rare,
    Special,
    Regular,
}

impl Rarity {
    pub const ALL: [Rarity; 5] = [
        Rarity::AllTime,
        Rarity::Legend,
        Rarity::Rare,
        Rarity::Special,
        Rarity::Regular,
    ];
}

/// A player card: what the catalogue lists and the board holds. Stats in card-back order: OFF DEF TEC POW SPD STA.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub number: u32,
    /// The card's record in the game's player_data.bin (the catalogue's record_index): its links' key.
    pub record: Option<u32>,
    pub name: String,
    /// The name the card's face shows (the catalogue's short name); empty = the full name's last word.
    pub short: String,
    pub club: String,
    pub country: String,
    pub season: String,
    /// The player's birth date as the catalogue gives it (1984-09-22); empty when unknown.
    pub born: String,
    pub pos: Position,
    pub rarity: Rarity,
    /// The code printed on the card (the catalogue's card_code: HOLE, WGK-3, ATLE-RE-4 ...); empty for 2010-11.
    pub code: String,
    /// A card the 2017-18 game treats as a legend: its back prints SECRET for the six stats (they grow by training).
    pub secret: bool,
    pub stats: [u8; 6],
    /// TOTAL as the card prints it.
    pub total: u8,
    pub aptitude: Aptitude,
}

impl Card {
    /// The card's series: its printed code without the trailing number (WGK-3 -> WGK, MVP-1 -> MVP, KLE stays); a
    /// code of digits only gives none.
    pub fn series(&self) -> &str {
        series(&self.code)
    }

    /// Another card of the same player: the same full name, accents and case aside (seasons spell it both ways),
    /// and the same birth date when both cards know it.
    pub fn same_player(&self, o: &Card) -> bool {
        let fold = |s: &str| deunicode::deunicode(s).to_lowercase();
        (self.born.is_empty() || o.born.is_empty() || self.born == o.born)
            && fold(&self.name) == fold(&o.name)
    }

    pub fn short_name(&self) -> &str {
        if self.short.is_empty() {
            self.name.rsplit(' ').next().unwrap_or(&self.name)
        } else {
            &self.short
        }
    }
}

/// [`Card::series`] of a code.
pub fn series(code: &str) -> &str {
    code.trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches(['-', ' '])
}

// ---- suitable positions ----

/// The 16 roles of a card's position ratings, in the order the original files keep them. The names are inferred by
/// the project from the players rated 10 in each, not the game's own.
pub const ROLES: [&str; 16] = [
    "CF", "TM", "WG", "SS", "OMF", "OH", "CMF", "DMF", "SMF", "WB", "SB", "CB", "STP", "CVR", "SW",
    "GK",
];

/// The position areas of the game's own table (as the fan database maps them): left, top, width, height in 0..1 of
/// the field, attack at the top, own goal at the bottom; the roles each area rates (the best of them counts) and its
/// line. Big areas first, so the nested ones (SS in CF, GK in CB) come later and win: [`Aptitude::at`] reads the last
/// area holding a point. CB's area holds SW's, SH's holds WB's (the game tells them apart by the rest of the team).
pub const AREAS: [([f32; 4], &[usize], Position); 13] = [
    (
        [0.206, 0.643, 0.588, 0.357],
        &[11, 12, 13, 14],
        Position::Df,
    ), // CB STP CVR SW
    ([0.0, 0.643, 0.206, 0.357], &[10], Position::Df), // SB
    ([0.794, 0.643, 0.206, 0.357], &[10], Position::Df),
    ([0.206, 0.519, 0.588, 0.124], &[7], Position::Mf), // DMF
    ([0.206, 0.386, 0.588, 0.133], &[6], Position::Mf), // CMF
    ([0.206, 0.262, 0.588, 0.124], &[5], Position::Mf), // OH
    ([0.206, 0.0, 0.588, 0.262], &[0, 1], Position::Fw), // CF TM
    ([0.0, 0.0, 0.206, 0.262], &[2], Position::Fw),     // WG
    ([0.794, 0.0, 0.206, 0.262], &[2], Position::Fw),
    ([0.206, 0.081, 0.588, 0.181], &[3, 4], Position::Fw), // SS OMF
    ([0.0, 0.262, 0.206, 0.381], &[8, 9], Position::Mf),   // SMF WB
    ([0.794, 0.262, 0.206, 0.381], &[8, 9], Position::Mf),
    ([0.4, 0.9, 0.2, 0.1], &[15], Position::Gk), // GK
];

/// A card's suitable positions, as the back of the original card shows them: 16 role ratings in [`ROLES`] order, each
/// 1..10; 0 is no data (editions 36+ get flat 8s from their position flags, see [`Aptitude::from_flags`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Aptitude {
    pub roles: [u8; 16],
}

/// How well a card fits a spot: rated 8 or more (the C panel's "good"), 5 or more, less.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Best,
    Fits,
    Out,
}

impl Fit {
    pub fn of(rating: f32) -> Fit {
        match rating {
            r if r >= 8.0 => Fit::Best,
            r if r >= 5.0 => Fit::Fits,
            _ => Fit::Out,
        }
    }
}

impl Aptitude {
    /// From the 12 position flags as 24 hex digits (CF SS WG OH CH DH SH WB SB CB SW GK, one byte each): every set
    /// flag rates its role 8, flat. Left over from the flags: TM, OMF, STP, CVR stay 0.
    pub fn from_flags(hex: &str) -> Aptitude {
        // flag -> index in ROLES
        const AT: [usize; 12] = [0, 3, 2, 5, 6, 7, 8, 9, 10, 11, 14, 15];
        let mut a = Aptitude::default();
        for (i, &role) in AT.iter().enumerate() {
            if hex.get(i * 2..i * 2 + 2).is_some_and(|b| b != "00") {
                a.roles[role] = 8;
            }
        }
        a
    }

    pub fn known(&self) -> bool {
        self.roles.iter().any(|&r| r > 0)
    }

    /// The best role of an area and its rating (the first named on a tie).
    pub fn best(&self, roles: &[usize]) -> (&'static str, u8) {
        let i = roles
            .iter()
            .copied()
            .rev()
            .max_by_key(|&i| self.roles[i])
            .unwrap_or(0);
        (ROLES[i], self.roles[i])
    }

    /// The rating at a field point (0..1 as in [`AREAS`]): the best role of the last area holding it.
    pub fn at(&self, p: Pos2) -> f32 {
        AREAS
            .iter()
            .rev()
            .find(|([x, y, w, h], ..)| Rect::from_min_size(pos2(*x, *y), vec2(*w, *h)).contains(p))
            .map_or(0.0, |(_, roles, _)| self.best(roles).1 as f32)
    }

    /// The three best roles, best first, for the big card.
    pub fn top(&self) -> Vec<(&'static str, u8)> {
        let mut v: Vec<_> = ROLES
            .iter()
            .zip(self.roles)
            .filter(|(_, r)| *r > 0)
            .map(|(n, r)| (*n, r))
            .collect();
        v.sort_by_key(|(_, r)| std::cmp::Reverse(*r));
        v.truncate(3);
        v
    }
}

/// The card table: 16 slots, 0-10 on the pitch, 11-15 the bench's five boxes; each holds a card index or nothing. A
/// pitch slot is not a place: its card lies at its spot, anywhere on the pitch, and an empty pitch slot is not drawn.
#[derive(Clone, Debug)]
pub struct Board {
    pub cards: Vec<Card>,
    pub slots: [Option<usize>; 16],
    /// Where each pitch slot sits, 0..1 across the card lane and 0 (attack) .. 1 (own goal) down.
    pub spots: [Pos2; 11],
    /// A bench card in hand (slot, -1..1): the table lays it off its spot, inside its box, so the game sees it rub.
    pub rub: Option<(usize, f32)>,
}

impl Board {
    /// The card number in a slot.
    pub fn number(&self, slot: usize) -> Option<u32> {
        self.slots[slot].map(|ci| self.cards[ci].number)
    }
}

impl Board {
    fn first_free(&self, slots: std::ops::Range<usize>) -> Option<usize> {
        slots.into_iter().find(|&s| self.slots[s].is_none())
    }

    /// A card on the table already (by number).
    pub fn holds(&self, number: u32) -> bool {
        (0..16).any(|s| self.number(s) == Some(number))
    }

    /// A card dragged within the board dropped on `slot`: the two trade places (an empty slot takes the card).
    pub fn swap(&mut self, from: usize, slot: usize) {
        self.slots.swap(from, slot);
    }

    /// A card dropped on open pitch lies exactly at `spot`: a pitch card moves there, a bench card goes on the pitch
    /// there (11 on the pitch already: it stays). Returns whether anything moved.
    pub fn move_to(&mut self, from: usize, spot: Pos2) -> bool {
        let to = if from < 11 {
            from
        } else if let Some(s) = self.first_free(0..11) {
            s
        } else {
            return false;
        };
        self.slots.swap(from, to);
        self.spots[to] = spot;
        true
    }

    /// The table's other card of `card`'s player taken off (a team holds one card per player): the slot it left.
    fn take_same_player(&mut self, card: &Card) -> Option<usize> {
        let s = (0..16).find(|&s| {
            self.slots[s].is_some_and(|ci| {
                self.cards[ci].number != card.number && self.cards[ci].same_player(card)
            })
        })?;
        self.slots[s] = None;
        Some(s)
    }

    /// A catalogue card dropped on `slot`: it takes the place, and the card there goes to the first free bench box
    /// (or off the table when the bench is full). A card already on the table is left alone; another card of the same
    /// player leaves the table.
    pub fn put(&mut self, card: &Card, slot: usize) -> bool {
        if self.holds(card.number) {
            return false;
        }
        self.take_same_player(card);
        self.cards.push(card.clone());
        if let Some(old) = self.slots[slot].take()
            && let Some(b) = self.first_free(11..16)
        {
            self.slots[b] = Some(old);
        }
        self.slots[slot] = Some(self.cards.len() - 1);
        true
    }

    /// A catalogue card dropped on open pitch lies exactly at `spot` (11 on the pitch already: the first free bench
    /// box); nothing when the table is full or holds the card already. Another card of the same player leaves the
    /// table.
    pub fn put_free(&mut self, card: &Card, spot: Pos2) -> bool {
        if self.holds(card.number) {
            return false;
        }
        self.take_same_player(card);
        let Some(s) = self.first_free(0..16) else {
            return false;
        };
        self.cards.push(card.clone());
        self.slots[s] = Some(self.cards.len() - 1);
        if s < 11 {
            self.spots[s] = spot;
        }
        true
    }
}
