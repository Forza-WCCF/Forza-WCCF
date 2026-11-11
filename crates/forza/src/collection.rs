//! Create-a-Team's collection: the grant log `grants.jsonl` and its fold into the cards and tokens the player holds,
//! the locked append, the economy table, the seeded draws and CONVERT. Pure code: the caller passes its own `Card` rows
//! (built from the catalogue), so the module needs no catalogue and the Linux server builds it ('--no-default-features').
//!
//! The table is draft 7.3 (`PLAN.md` 2.1-2.4): a card is worth base x rating step x series factor, a secret card (its
//! back prints the placeholder, [`secret`]) a flat price; a match pays 50 tokens until the week's 1,050, and the week
//! turns on Monday at 05:00.
//!
//! The log is the source of truth: one JSON object per line, append-only, each line's `prev` the SHA-256 of the line
//! before it. The state is a fold over the lines and is never stored. The log stores tokens, never card values, so a
//! later `RULES` does not rewrite history.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, Write};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
/// The log's time type, for callers without their own `time` dependency (the panel).
pub use time::OffsetDateTime;

/// The economy table's version: every line states the table it was written under, and the market refuses an older one.
pub const RULES: u32 = 2;

/// A card's craft value at TOTAL under 85: white 40, black 160, RARE 400, LEGEND 1,500, ALL TIME LEGEND 2,500
/// (`PLAN.md` 2.1; the black base rose from draft 6's 100).
const BASE: [u32; 5] = [40, 160, 400, 1_500, 2_500];
/// Every draw's odds per tier, in tenths of a percent: 68 %, 26 %, 5 %, 0.2 %, 0.8 % (6 % RARE or better).
const ODDS: [u32; 5] = [680, 260, 50, 2, 8];
/// The rating step: a card's TOTAL up to this value, and the step in percent (x1, x1.5, x2.5, x4).
const STEPS: [(u32, u32); 4] = [(84, 100), (89, 150), (94, 250), (u32::MAX, 400)];
/// A premium RARE's series factor in percent (`PLAN.md` 2.1): the award cards, worth more than their cell.
const PREMIUM: [(&str, u32); 10] = [
    ("MVP", 250),
    ("POY", 250),
    ("EMVP", 250),
    ("WMVP", 250),
    ("WBE", 150),
    ("BES", 150),
    ("BGK", 150),
    ("BDF", 150),
    ("BMF", 150),
    ("BFW", 150),
];
/// A secret card's flat price per series (`PLAN.md` 2.1, m1). The seven ALL TIME LEGEND series cover its 173 secret
/// cards; `S-ATLE` is the spelling of one of SATLE's seven.
const SECRET_PRICES: [(&str, u32); 8] = [
    ("ATLE", 3_750),
    ("HOLE", 6_250),
    ("LEOC", 6_250),
    ("SATLE", 6_250),
    ("S-ATLE", 6_250),
    ("LEU", 6_250),
    ("KOLE", 10_000),
    ("KLE", 10_000),
];
/// The 17 secret white reprints (`ATLE-RE-`, one without a code) take this, white x1; a secret series the table does
/// not name takes the lowest flat cell, as the plan writes "where unsure the price is the lower cell".
const SECRET_WHITE: u32 = 40;
const SECRET_UNKNOWN: u32 = 3_750;
/// Shares of a card's value in percent: CONVERT (exact), the market floor (up), server stock (exact), the ceiling (down).
pub const CONVERT_SHARE: u32 = 20;
pub const FLOOR_SHARE: u32 = 80;
pub const STOCK_SHARE: u32 = 90;
pub const CEILING_SHARE: u32 = 95;
/// A pack: its price in tokens and its cards. Every pack draws at match-card odds with the same guarantee, and no
/// pack holds one player twice (`PLAN.md` 2.3).
pub const PACK_CARDS: usize = 3;
pub const STANDARD_PACK: i64 = 100;
pub const SEASON_PACK: i64 = 150;
pub const NATION_PACK: i64 = 200;
/// Match tokens: 50 a match, win or lose, until the week's 1,050 (the first 21); past it the card still deals.
pub const MATCH_TOKENS: i64 = 50;
pub const WEEK_CAP: i64 = 1_050;
/// The first start under the release rules pays this once per installation (`PLAN.md` 2.2).
pub const WELCOME: i64 = 150;
/// The starter: 11 REGULAR cards (one GK, 4 DF, 4 MF, 2 FW) and 3 face-up RARE cards of the same choice.
pub const STARTER_DEAL: [(Position, usize); 4] = [
    (Position::Gk, 1),
    (Position::Df, 4),
    (Position::Mf, 4),
    (Position::Fw, 2),
];
pub const STARTER_RARES: usize = 3;
/// The 11-card mean a banded start lands in, and the deals it may be redrawn through before the closest one stays
/// (`PLAN.md` 2.4: 80-82, 1,000 deals; South America needs about 90).
pub const STARTER_BAND: (u32, u32) = (80, 82);
pub const STARTER_DEALS: u32 = 1_000;
/// The release's crafting tiers, white and black only: RARE and up come from packs, the market and the stock list
/// (`PLAN.md` 2.3; a later `rules` version opens them).
pub const CRAFT: [Rarity; 2] = [Rarity::Regular, Rarity::Special];

/// The 24 nations a Nation pack offers: the rest of the nation team styles has too short a card pool.
pub const NATIONS: [&str; 24] = [
    "Italy",
    "Spain",
    "Brazil",
    "Argentina",
    "France",
    "Japan",
    "Germany",
    "England",
    "Netherlands",
    "Portugal",
    "Belgium",
    "Uruguay",
    "Croatia",
    "Colombia",
    "Serbia",
    "Czech Republic",
    "Chile",
    "Sweden",
    "Poland",
    "Denmark",
    "Cameroon",
    "Russia",
    "Switzerland",
    "Korea Republic",
];

/// The nine leagues a league start offers (`PLAN.md` 2.4): the league's name and its clubs as the catalogue's
/// `club_name` spells them, 146 of the pool's 164 names. The other 18 still drop and show under "All seasons". A club
/// is mapped by its country's top league whatever division it played in that season (Piacenza, Le Mans ...).
pub const LEAGUES: [(&str, &[&str]); 9] = [
    (
        "Italy (Serie A)",
        &[
            "Juventus",
            "AC Milan",
            "Inter",
            "AS Roma",
            "Lazio",
            "Napoli",
            "Udinese",
            "Parma",
            "Bologna",
            "Perugia",
            "Fiorentina",
            "Brescia",
            "Torino",
            "Atalanta",
            "Piacenza",
            "Chievo Verona",
            "Venezia",
            "Sampdoria",
            "Reggina",
            "Modena",
            "Hellas Verona",
            "Empoli",
            "Como",
            "Lecce",
            "Genoa",
            "Catania",
            "Sassuolo",
            "Palermo",
            "Messina",
            "Cagliari",
        ],
    ),
    (
        "Spain (La Liga)",
        &[
            "FC Barcelona",
            "Real Madrid",
            "Atletico Madrid",
            "Valencia",
            "Villarreal",
            "Sevilla",
            "Celta Vigo",
            "Malaga",
            "Real Betis",
            "Deportivo La Coruna",
            "Real Sociedad",
            "Mallorca",
            "Getafe",
            "Athletic Club",
        ],
    ),
    (
        "England (Premier League)",
        &[
            "Arsenal",
            "Chelsea",
            "Manchester United",
            "Liverpool",
            "Manchester City",
            "Leicester City",
            "Newcastle United",
            "Southampton",
            "Tottenham Hotspur",
            "Everton",
            "West Ham United",
            "West Bromwich Albion",
            "Stoke City",
            "Queens Park Rangers",
            "Leeds United",
            "Crystal Palace",
        ],
    ),
    (
        "Germany (Bundesliga)",
        &[
            "Bayern Munchen",
            "Borussia Dortmund",
            "Schalke 04",
            "Bayer Leverkusen",
            "VfL Wolfsburg",
            "Hamburger SV",
            "VfB Stuttgart",
            "Hannover 96",
            "Hertha BSC",
            "Eintracht Frankfurt",
            "Mainz 05",
            "1. FC Nurnberg",
            "1. FC Koln",
            "VfL Bochum",
            "Hoffenheim",
            "FC Augsburg",
            "Werder Bremen",
            "Borussia Monchengladbach",
            "1860 Munchen",
        ],
    ),
    (
        "France (Ligue 1)",
        &[
            "Paris Saint-Germain",
            "Olympique Lyonnais",
            "Girondins de Bordeaux",
            "Olympique de Marseille",
            "LOSC Lille",
            "Montpellier",
            "Grenoble",
            "AS Monaco",
            "Saint-Etienne",
            "Le Mans",
            "FC Metz",
        ],
    ),
    (
        "Netherlands (Eredivisie)",
        &[
            "Ajax",
            "PSV Eindhoven",
            "Feyenoord",
            "AZ Alkmaar",
            "Vitesse",
            "VVV-Venlo",
            "Heerenveen",
            "FC Utrecht",
            "FC Dordrecht",
        ],
    ),
    (
        "Portugal (Primeira Liga)",
        &["FC Porto", "Benfica", "Sporting CP", "Maritimo"],
    ),
    (
        "Japan (J.League and forerunners)",
        &[
            "Gamba Osaka",
            "Urawa Reds",
            "FC Tokyo",
            "Kashima Antlers",
            "Kawasaki Frontale",
            "Vissel Kobe",
            "Sanfrecce Hiroshima",
            "Jubilo Iwata",
            "Yokohama F. Marinos",
            "Cerezo Osaka",
            "Kashiwa Reysol",
            "Sagan Tosu",
            "Albirex Niigata",
            "Shimizu S-Pulse",
            "Consadole Sapporo",
            "Ventforet Kofu",
            "Vegalta Sendai",
            "Omiya Ardija",
            "Nagoya Grampus",
            "Yokohama Marinos",
            "Verdy Kawasaki",
            "Fagiano Okayama",
            "Avispa Fukuoka",
            "Yokohama FC",
            "Yanmar",
            "Tokushima Vortis",
            "Nissan FC",
            "Mazda SC",
            "JEF United Ichihara",
            "JEF United Chiba",
            "Gainare Tottori",
            "Bellmare Hiratsuka",
        ],
    ),
    (
        "South America",
        &[
            "Boca Juniors",
            "Sao Paulo",
            "Flamengo",
            "River Plate",
            "Santos",
            "Corinthians",
            "Fluminense",
            "Atletico Mineiro",
            "Velez Sarsfield",
            "Newell's Old Boys",
            "Junior",
        ],
    ),
];

/// The league of a club name in [`LEAGUES`], `None` for the 18 names the table leaves out (they still drop and show
/// under "All seasons").
#[must_use]
pub fn league_of(club: &str) -> Option<&'static str> {
    LEAGUES
        .iter()
        .find(|(_, clubs)| clubs.contains(&club))
        .map(|(league, _)| *league)
}

/// A starter choice (`PLAN.md` 2.4): which cards the 11 come from, and whether the deal must hit the band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start<'a> {
    /// Every season, banded: the deal is redrawn until its 11 mean 80-82 (`s4`). The first entry of the picker.
    AllSeasons,
    /// One season, unbanded: the picker shows the season's own mean (2001-02: 75, 2013-14: 84).
    Season(u16),
    /// One league's clubs, any season, banded; a name [`LEAGUES`] does not hold deals from no card.
    League(&'a str),
}

impl Start<'_> {
    /// Whether a card belongs to this choice's pool (its season, or its club's league).
    #[must_use]
    pub fn has(self, card: &Card) -> bool {
        match self {
            Self::AllSeasons => true,
            Self::Season(season) => card.season == season,
            Self::League(name) => LEAGUES
                .iter()
                .find(|(league, _)| *league == name)
                .is_some_and(|(_, clubs)| clubs.contains(&card.club)),
        }
    }

    /// Whether the deal's 11 mean must land in [`STARTER_BAND`].
    #[must_use]
    pub fn banded(self) -> bool {
        !matches!(self, Self::Season(_))
    }
}

/// The five tiers, cheapest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rarity {
    Regular,
    Special,
    Rare,
    Legend,
    AllTimeLegend,
}

impl Rarity {
    /// Every tier, cheapest first: a match card and a pack draw from these at the table's odds.
    pub const ALL: [Self; 5] = [
        Self::Regular,
        Self::Special,
        Self::Rare,
        Self::Legend,
        Self::AllTimeLegend,
    ];
    /// SPECIAL and better: the tier a pack's guarantee falls back to.
    pub const BETTER: [Self; 4] = [Self::Special, Self::Rare, Self::Legend, Self::AllTimeLegend];

    /// The craft value at TOTAL under 85.
    #[must_use]
    pub fn base(self) -> u32 {
        BASE[self as usize]
    }

    /// Its odds in tenths of a percent (680 = 68 %).
    #[must_use]
    pub fn odds(self) -> u32 {
        ODDS[self as usize]
    }
}

/// The four places a starter deal fills; a card's catalogue position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Gk,
    Df,
    Mf,
    Fw,
}

/// What a draw needs to know about a card: the catalogue's row without the catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card<'a> {
    /// The game's card number (catalogue `card_no`), the log's and the market's identity of a card.
    pub card_no: u32,
    /// One number per player, the same on every card of that player: the caller derives it with
    /// `board::Card::same_player` (name and birth date). Not `record_index`: that differs on every card.
    pub player: u32,
    pub rarity: Rarity,
    /// The game's TOTAL, 60 to 100: 100 on a card whose back prints the placeholder.
    pub total: u32,
    /// The card's series, its printed code without the number (`board::series`: `MVP-1` -> `MVP`, `S-ATLE` stays):
    /// the premium factor and a secret card's flat price key on it.
    pub series: &'a str,
    /// A card whose back prints SECRET ([`secret`]): the six stats are the placeholder, so its TOTAL is no rating.
    pub secret: bool,
    /// The season's first year (2001 for 2001-02).
    pub season: u16,
    /// The catalogue's `nationality_name`, as the nation list spells it.
    pub nation: &'a str,
    /// The catalogue's `club_name`: the league table maps it, and a league start picks by it.
    pub club: &'a str,
    pub position: Position,
}

/// A card's craft value: its tier's base times its rating step times its series factor, or a secret card's flat price.
/// All cells of the table are whole tokens.
#[must_use]
pub fn value(card: &Card) -> u32 {
    if card.secret {
        return secret_value(card);
    }
    card.rarity.base() * step(card.total) / 100 * factor(card.rarity, card.series) / 100
}

/// Whether a card's back prints SECRET rather than its six stats: only the placeholder card prints the six 16s and the
/// 20 (TOTAL 100). Every other card, an ALL TIME LEGEND that prints real stats included, is priced from its TOTAL.
#[must_use]
pub fn secret(stats: [u8; 6]) -> bool {
    stats == [16, 16, 16, 16, 16, 20]
}

/// A secret card's flat price (`PLAN.md` 2.1): its series' cell, or 40 for the white reprints.
fn secret_value(card: &Card) -> u32 {
    if card.rarity == Rarity::Regular {
        return SECRET_WHITE;
    }
    SECRET_PRICES
        .iter()
        .find(|(series, _)| *series == card.series)
        .map_or(SECRET_UNKNOWN, |(_, price)| *price)
}

/// The series factor in percent: a premium RARE is worth 2.5 or 1.5 times its cell, every other card 1.
#[must_use]
pub fn factor(rarity: Rarity, series: &str) -> u32 {
    if rarity != Rarity::Rare {
        return 100;
    }
    PREMIUM
        .iter()
        .find(|(code, _)| *code == series)
        .map_or(100, |(_, percent)| *percent)
}

/// The craft price of `card`: its value, no margin; `None` for a tier the release does not craft (white and black
/// only, `PLAN.md` 2.3).
#[must_use]
pub fn craft(card: &Card) -> Option<u32> {
    CRAFT.contains(&card.rarity).then(|| value(card))
}

/// The rating step in percent (100 = x1, 150 = x1.5, 250 = x2.5, 400 = x4).
#[must_use]
pub fn step(total: u32) -> u32 {
    STEPS
        .iter()
        .find(|(up_to, _)| total <= *up_to)
        .map_or(STEPS[STEPS.len() - 1].1, |(_, step)| *step)
}

/// The market floor: 80 % of the value, rounded up.
#[must_use]
pub fn floor_value(value: u32) -> u32 {
    (value * FLOOR_SHARE).div_ceil(100)
}

/// The server's stock price: 90 % of the value, exact for all 20 cells.
#[must_use]
pub fn stock_value(value: u32) -> u32 {
    value * STOCK_SHARE / 100
}

/// The market ceiling: 95 % of the value, rounded down.
#[must_use]
pub fn ceiling_value(value: u32) -> u32 {
    value * CEILING_SHARE / 100
}

/// What a match pays, given the match tokens already earned in the week: 50 a match until the week's 1,050 (the first
/// 21), nothing from the 22nd on. The card still deals past the cap (`PLAN.md` 2.2, g2).
#[must_use]
pub fn match_tokens(earned_this_week: i64) -> i64 {
    (WEEK_CAP - earned_this_week).clamp(0, MATCH_TOKENS)
}

/// The game's business day at `at` as `YYYYMMDD`: the day turns at 05:00 local and runs to the next 05:00, as the
/// control counts it (`forza::online::championship::date` has the rule).
#[must_use]
pub fn day(at: OffsetDateTime) -> u32 {
    crate::online::championship::date(at)
}

/// The week's label at `at` as `YYYYWW`: the ISO week of the time 5 hours earlier, so it turns on Monday at 05:00 like
/// the business day, and each line's own offset counts (`PLAN.md` 2.2).
#[must_use]
pub fn week(at: OffsetDateTime) -> u32 {
    let (year, week, _) = (at - time::Duration::hours(5)).date().to_iso_week_date();
    year.unsigned_abs() * 100 + u32::from(week)
}

/// A seeded draw's source (splitmix64): one seed and one pool give the same cards, in tests and in a replay.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number below `n` (`n` > 0).
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// `n` of `idx`'s cards, no card twice.
    fn take<'a>(
        &mut self,
        mut idx: Vec<usize>,
        pool: &'a [Card<'a>],
        n: usize,
    ) -> Vec<&'a Card<'a>> {
        let mut out = vec![];
        while out.len() < n && !idx.is_empty() {
            out.push(&pool[idx.remove(self.below(idx.len() as u64) as usize)]);
        }
        out
    }
}

/// One card of `pool` that `pick` accepts: the rarity first at the odds of `tiers`, stepping down to the next lower
/// tier the pool has, then a plain random card inside that tier. `None` when no tier of `tiers` has a card.
pub fn draw<'a>(
    rng: &mut Rng,
    pool: &'a [Card<'a>],
    pick: impl Fn(&Card) -> bool,
    tiers: &[Rarity],
) -> Option<&'a Card<'a>> {
    let mut roll = rng.below(u64::from(tiers.iter().map(|r| r.odds()).sum::<u32>())) as u32;
    let mut tier = *tiers.last()?;
    for r in tiers {
        if roll < r.odds() {
            tier = *r;
            break;
        }
        roll -= r.odds();
    }
    // Step down: the drawn tier's pool may be empty in a nation or a season (Korea Republic has no RARE).
    let drawn = tiers.iter().position(|r| *r == tier)?;
    for &r in tiers[..=drawn].iter().rev() {
        let n = pool.iter().filter(|c| c.rarity == r && pick(c)).count();
        if n > 0 {
            let k = rng.below(n as u64) as usize;
            return pool.iter().filter(|c| c.rarity == r && pick(c)).nth(k);
        }
    }
    None
}

/// A match card: the whole pool at the match-card odds.
pub fn match_card<'a>(rng: &mut Rng, pool: &'a [Card<'a>]) -> Option<&'a Card<'a>> {
    draw(rng, pool, |_| true, &Rarity::ALL)
}

/// A pack's cards: `PACK_CARDS` at the match-card odds from the cards `pick` accepts, no player twice (a tier with
/// no new player steps down), and when all of them came out REGULAR the last is redrawn from SPECIAL or better at
/// those tiers' relative odds.
pub fn pack<'a>(
    rng: &mut Rng,
    pool: &'a [Card<'a>],
    pick: impl Fn(&Card) -> bool,
) -> Vec<&'a Card<'a>> {
    let mut out: Vec<&Card> = vec![];
    while out.len() < PACK_CARDS {
        let next = draw(
            rng,
            pool,
            |c| pick(c) && out.iter().all(|taken| taken.player != c.player),
            &Rarity::ALL,
        );
        let Some(next) = next else { break };
        out.push(next);
    }
    if out.len() == PACK_CARDS && out.iter().all(|c| c.rarity == Rarity::Regular) {
        // The last card first; when the other two hold every SPECIAL-or-better player (Korea Republic's 2 SPECIAL
        // players drawn as REGULAR cards), an earlier card makes room instead (1.1b review N5).
        for slot in (0..PACK_CARDS).rev() {
            let kept: Vec<u32> = (0..PACK_CARDS)
                .filter(|&i| i != slot)
                .map(|i| out[i].player)
                .collect();
            if let Some(better) = draw(
                rng,
                pool,
                |c| pick(c) && !kept.contains(&c.player),
                &Rarity::BETTER,
            ) {
                out[slot] = better;
                break;
            }
        }
    }
    out
}

/// A pack the store sells (`PLAN.md` 2.3): every season, one season (its first year) or one of [`NATIONS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pack<'a> {
    Standard,
    Season(u16),
    Nation(&'a str),
}

impl Pack<'_> {
    #[must_use]
    pub fn price(self) -> i64 {
        match self {
            Pack::Standard => STANDARD_PACK,
            Pack::Season(_) => SEASON_PACK,
            Pack::Nation(_) => NATION_PACK,
        }
    }

    /// Whether `card` is in this pack's pool.
    #[must_use]
    pub fn has(self, card: &Card) -> bool {
        match self {
            Pack::Standard => true,
            Pack::Season(y) => card.season == y,
            Pack::Nation(n) => card.nation == n,
        }
    }

    /// The log's `why` for the pack's lines, in English as the log is.
    #[must_use]
    pub fn why(self) -> String {
        match self {
            Pack::Standard => "pack standard".into(),
            Pack::Season(y) => format!("pack season {y}"),
            Pack::Nation(n) => format!("pack nation {n}"),
        }
    }
}

/// One pack card's odds per tier ([`Rarity::ALL`]' order, tenths of a percent) after the step-down: a tier the pack's
/// pool lacks passes its odds to the next lower tier it has. Before the guarantee and the no-player-twice rule.
#[must_use]
pub fn pack_odds(pool: &[Card], kind: Pack) -> [u32; 5] {
    let has = Rarity::ALL.map(|r| pool.iter().any(|c| c.rarity == r && kind.has(c)));
    let mut out = [0; 5];
    for (i, r) in Rarity::ALL.iter().enumerate() {
        if let Some(j) = (0..=i).rev().find(|&j| has[j]) {
            out[j] += r.odds();
        }
    }
    out
}

/// The refusal [`buy_pack`] gives when the balance is under the pack's price.
pub const NOT_ENOUGH_TOKENS: &str = "not enough tokens";

/// Buy and open a pack under the log's lock: refused under its price ([`NOT_ENOUGH_TOKENS`]) or when its pool cannot
/// fill it, else one `pack` line that pays the price and one `pack` line per card, in one write.
pub fn buy_pack(
    log: &Log,
    at: OffsetDateTime,
    rng: &mut Rng,
    pool: &[Card],
    kind: Pack,
) -> io::Result<Vec<Entry>> {
    log.append_lines_at(at, |state| {
        if state.tokens < kind.price() {
            return Err(io::Error::other(NOT_ENOUGH_TOKENS));
        }
        let cards = pack(rng, pool, |c| kind.has(c));
        if cards.len() < PACK_CARDS {
            return Err(io::Error::other(format!("{}: too few cards", kind.why())));
        }
        let line = |card: Option<u32>, tokens| Line {
            kind: Kind::Pack,
            card,
            tokens,
            why: kind.why(),
        };
        Ok(std::iter::once(line(None, -kind.price()))
            .chain(cards.iter().map(|c| line(Some(c.card_no), 0)))
            .collect())
    })
}

/// The starter deal: 1 GK, 4 DF, 4 MF and 2 FW, REGULAR only, one card per player, no secret card (the band needs a
/// TOTAL). A banded start redraws its 11 until their mean lands in [`STARTER_BAND`], [`STARTER_DEALS`] times at most,
/// then keeps the deal closest to it; a season start keeps its first deal.
pub fn starter<'a>(rng: &mut Rng, pool: &'a [Card<'a>], start: Start) -> Vec<&'a Card<'a>> {
    let mut best = deal(rng, pool, start);
    // An unknown league name deals nothing: the first deal settles it, not a thousand empty ones.
    if best.is_empty() {
        return best;
    }
    let mut off = band_off(&best);
    for _ in 1..STARTER_DEALS {
        if !start.banded() || off == 0 {
            break;
        }
        let next = deal(rng, pool, start);
        let next_off = band_off(&next);
        if next_off < off {
            best = next;
            off = next_off;
        }
    }
    best
}

/// One deal of [`starter`], unbanded.
fn deal<'a>(rng: &mut Rng, pool: &'a [Card<'a>], start: Start) -> Vec<&'a Card<'a>> {
    let regular = |c: &Card| c.rarity == Rarity::Regular && !c.secret && start.has(c);
    let mut out: Vec<&Card> = vec![];
    for (position, n) in STARTER_DEAL {
        for _ in 0..n {
            let idx: Vec<usize> = pool
                .iter()
                .enumerate()
                .filter(|(_, c)| regular(c) && c.position == position)
                .filter(|(_, c)| out.iter().all(|taken| taken.player != c.player))
                .map(|(i, _)| i)
                .collect();
            out.extend(rng.take(idx, pool, 1));
        }
    }
    out
}

/// How far a deal's 11-card mean TOTAL lies outside [`STARTER_BAND`], summed; `u32::MAX` for a deal short of 11 cards.
fn band_off(deal: &[&Card]) -> u32 {
    if deal.len() != STARTER_DEAL.iter().map(|(_, n)| n).sum::<usize>() {
        return u32::MAX;
    }
    let sum: u32 = deal.iter().map(|c| c.total).sum();
    let (low, high) = STARTER_BAND;
    if sum < low * 11 {
        low * 11 - sum
    } else {
        sum.saturating_sub(high * 11)
    }
}

/// The starter's 3 face-up RARE cards of the same choice, three players: the first start picks one.
pub fn starter_rares<'a>(rng: &mut Rng, pool: &'a [Card<'a>], start: Start) -> Vec<&'a Card<'a>> {
    let mut out: Vec<&Card> = vec![];
    while out.len() < STARTER_RARES {
        let idx: Vec<usize> = pool
            .iter()
            .enumerate()
            .filter(|(_, c)| c.rarity == Rarity::Rare && start.has(c))
            .filter(|(_, c)| out.iter().all(|taken| taken.player != c.player))
            .map(|(i, _)| i)
            .collect();
        if idx.is_empty() {
            break;
        }
        out.extend(rng.take(idx, pool, 1));
    }
    out
}

/// A line's kind: what granted or moved the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Starter,
    /// The first start under the release rules: 150 tokens, once per installation, no card.
    Welcome,
    Match,
    /// A pack's card; a pack's own line has no `card` and carries its price.
    Pack,
    Craft,
    Convert,
    Import,
    List,
    Unlist,
    MarketBuy,
    MarketSell,
    /// A card bought from the own stock list of the day, with its price (`PLAN.md` 3.5).
    StockBuy,
}

/// One line of `grants.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// 1 on the first line, one more on each.
    pub seq: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The economy table it was written under ([`RULES`]).
    pub rules: u32,
    pub kind: Kind,
    /// The game's card number; `None` on a line that moves tokens only (a pack's price).
    pub card: Option<u32>,
    /// The signed change of the token balance.
    pub tokens: i64,
    /// The short English reason a reader of the log needs ("match 3 today", "pack standard").
    pub why: String,
    /// The SHA-256 of the previous line, hex; empty on the first line.
    pub prev: String,
}

impl Entry {
    /// The line as the log stores it, without its newline.
    #[must_use]
    pub fn line(&self) -> String {
        serde_json::to_string(self).expect("an entry is JSON")
    }
}

/// SHA-256 of one line, hex: the `prev` of the line after it.
#[must_use]
pub fn hash(line: &str) -> String {
    Sha256::digest(line.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The copies of one card the player holds, by source. A starter or imported copy is playable but never sells and
/// never converts, and does not make an earned copy a spare.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Copies {
    pub earned: u32,
    pub starter: u32,
    pub imported: u32,
    /// Out on the market: still owned, not placeable, and not a spare (a sale takes it out of both counts).
    pub listed: u32,
}

impl Copies {
    /// earned + starter + imported - listed > 0: the catalogue shows it and the table may use it.
    #[must_use]
    pub fn owned(&self) -> bool {
        self.earned + self.starter + self.imported > self.listed
    }
}

/// The log folded: the cards, the token balance and the week's match tokens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub cards: BTreeMap<u32, Copies>,
    pub tokens: i64,
    /// Match tokens earned in the week the fold was asked about (the weekly cap's counter).
    pub match_week: i64,
    /// The last line's number (0 for an empty log).
    pub seq: u64,
}

impl State {
    /// The copies of `card`, zero when it is not in the log.
    #[must_use]
    pub fn copies(&self, card: u32) -> Copies {
        self.cards.get(&card).copied().unwrap_or_default()
    }

    /// What the next match pays now: [`match_tokens`] at the week's counter.
    #[must_use]
    pub fn match_pays(&self) -> i64 {
        match_tokens(self.match_week)
    }
}

/// The last line a fold reads: the last one not blank.
fn last_line(text: &str) -> Option<&str> {
    text.lines().rev().find(|line| !line.trim().is_empty())
}

/// A folded log and what a reader should be told about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folded {
    pub state: State,
    /// A line the fold cannot read: it stopped there, so the state is the log up to the line before it.
    pub unreadable: bool,
    pub warnings: Vec<String>,
}

/// Fold a log into the state, counting the week's match tokens against `this_week`, a [`week`] label (YYYYWW) and never
/// a [`day`] one (YYYYMMDD): both are `u32`, so a `day` argument compiles and the cap then never applies. A line that
/// does not parse ends the fold, a `seq` that is not one more than the line before it and a `prev` that does not match
/// that line are warnings, and the lines around them are kept.
#[must_use]
pub fn fold_at(lines: &str, this_week: u32) -> Folded {
    let mut out = Folded::default();
    let mut before: Option<&str> = None;
    let mut seq = 0;
    let mut chain = true;
    for (n, line) in lines.lines().enumerate().map(|(n, l)| (n + 1, l)) {
        if line.trim().is_empty() {
            continue;
        }
        let entry: Entry = match serde_json::from_str(line) {
            Ok(entry) => entry,
            Err(e) => {
                out.unreadable = true;
                out.warnings.push(format!(
                    "line {n} does not parse ({e}), the log is read up to line {}",
                    n - 1
                ));
                break;
            }
        };
        if entry.seq != seq + 1 {
            out.warnings.push(format!(
                "line {n}: seq is {} and not the {}",
                entry.seq,
                seq + 1
            ));
        }
        seq = entry.seq;
        if let Some(prev) = before {
            let expected = hash(prev);
            if chain && entry.prev != expected {
                chain = false;
                out.warnings.push(format!(
                    "line {n}: prev is not the SHA-256 of line {}",
                    n - 1
                ));
            }
        }
        out.state.apply(&entry, this_week);
        before = Some(line);
    }
    out
}

impl State {
    fn apply(&mut self, entry: &Entry, this_week: u32) {
        self.seq = entry.seq;
        self.tokens += entry.tokens;
        if entry.kind == Kind::Match && week(entry.at) == this_week {
            self.match_week += entry.tokens;
        }
        if let Some(card) = entry.card {
            let mut copies = self.cards.get(&card).copied().unwrap_or_default();
            match entry.kind {
                Kind::Starter => copies.starter += 1,
                Kind::Import => copies.imported += 1,
                Kind::Match | Kind::Pack | Kind::Craft | Kind::MarketBuy | Kind::StockBuy => {
                    copies.earned += 1;
                }
                // The welcome pays tokens only; a line carrying a card with it counts nothing.
                Kind::Welcome => {}
                Kind::Convert => copies.earned = copies.earned.saturating_sub(1),
                // A sale is the sale of a listed copy: it leaves the collection and the market at once.
                Kind::MarketSell => {
                    copies.earned = copies.earned.saturating_sub(1);
                    copies.listed = copies.listed.saturating_sub(1);
                }
                Kind::List => copies.listed += 1,
                Kind::Unlist => copies.listed = copies.listed.saturating_sub(1),
            }
            // A welcome line pays tokens only, and a convert, sale or unlist of a card never held changes nothing:
            // neither leaves an entry for a card the player does not own.
            if copies != Copies::default() || self.cards.contains_key(&card) {
                self.cards.insert(card, copies);
            }
        }
    }
}

/// Fold a log into the state for the week now.
#[must_use]
pub fn fold(lines: &str) -> Folded {
    fold_at(lines, week(now()))
}

/// This PC's clock, local time where Windows gives it.
#[must_use]
pub fn now() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}

/// How long a writer or a reader waits for the log's lock: the panel appends in the game process and must never wait
/// long. A caller in the game process calls from a worker thread, so the box is never spent on a frame it would stall.
pub const LOCK_WAIT: Duration = Duration::from_secs(2);

/// `file` locked (shared for a reader, exclusive for a writer), waiting up to [`LOCK_WAIT`]: `WouldBlock` when another
/// handle holds it.
fn lock(file: &File, shared: bool) -> io::Result<()> {
    let start = Instant::now();
    loop {
        let taken = if shared {
            file.try_lock_shared()
        } else {
            file.try_lock()
        };
        match taken {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) if start.elapsed() < LOCK_WAIT => {
                sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "another handle holds the grant log",
                ));
            }
            Err(TryLockError::Error(e)) => return Err(e),
        }
    }
}

/// One line a caller asks [`Log::append_lines_at`] to write: what the caller knows, without `seq` and `prev`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub card: Option<u32>,
    pub tokens: i64,
    pub why: String,
}

/// `grants.jsonl` in a profile's collection folder, the one writer's door to it.
#[derive(Clone, Debug)]
pub struct Log {
    path: std::path::PathBuf,
}

impl Log {
    /// The log in `dir` (the profile's `collection` folder, which the caller makes).
    pub fn new(dir: impl AsRef<std::path::Path>) -> Self {
        Self {
            path: dir.as_ref().join("grants.jsonl"),
        }
    }

    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Every line as it is on disk; a missing file is an empty log. Reads under the log's shared lock (the same time
    /// box): a write in progress is waited out instead of failing on a lock violation (Windows, os error 33).
    pub fn lines(&self) -> io::Result<String> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(e),
        };
        lock(&file, true)?;
        let mut text = String::new();
        (&file).read_to_string(&mut text)?;
        Ok(text)
    }

    /// The log open for reading and appending: `not append(true)`, which on Windows asks for FILE_APPEND_DATA only,
    /// while `set_len` and `seek` want more.
    fn open(&self) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.path)
    }

    /// Append one line now, for the business day now.
    pub fn append(
        &self,
        kind: Kind,
        card: Option<u32>,
        tokens: i64,
        why: &str,
    ) -> io::Result<Entry> {
        self.append_at(now(), kind, card, tokens, why)
    }

    /// Append one line at `at`: the one-line door to [`Self::append_lines_at`].
    pub fn append_at(
        &self,
        at: OffsetDateTime,
        kind: Kind,
        card: Option<u32>,
        tokens: i64,
        why: &str,
    ) -> io::Result<Entry> {
        let line = Line {
            kind,
            card,
            tokens,
            why: why.to_string(),
        };
        Ok(self.append_lines_at(at, |_| Ok(vec![line]))?.remove(0))
    }

    /// Append lines at `at` under one lock and one write: `lines` reads the state folded under the lock and returns the
    /// lines to write, or a refusal, which leaves the file as it was. A torn last line is cut off first (or its missing
    /// newline written), and a log the fold cannot read past a line in it is refused whole: a new `seq` after the break
    /// would leave every later line unreadable. A broken chain or a `seq` gap warns but appends.
    pub fn append_lines_at(
        &self,
        at: OffsetDateTime,
        lines: impl FnOnce(&State) -> io::Result<Vec<Line>>,
    ) -> io::Result<Vec<Entry>> {
        let mut file = self.open()?;
        lock(&file, false)?;
        let mut text = String::new();
        file.seek(io::SeekFrom::Start(0))?;
        file.read_to_string(&mut text)?;
        // A last line cut short by an interrupted write is dropped; a whole one that only lost its newline is kept.
        let complete = text.rfind('\n').map_or(0, |i| i + 1);
        let cut = (complete < text.len()
            && serde_json::from_str::<Entry>(&text[complete..]).is_err())
        .then_some(complete);
        if let Some(complete) = cut {
            text.truncate(complete);
        }
        let folded = fold_at(&text, week(at));
        if folded.unreadable {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                folded.warnings.join("; "),
            ));
        }
        let asked = lines(&folded.state)?;
        if asked.is_empty() {
            return Ok(vec![]);
        }
        let mut prev = last_line(&text).map(hash).unwrap_or_default();
        let mut block = String::new();
        if !text.is_empty() && !text.ends_with('\n') {
            block.push('\n');
        }
        let mut out = Vec::with_capacity(asked.len());
        for (n, line) in asked.into_iter().enumerate() {
            let entry = Entry {
                seq: folded.state.seq + 1 + n as u64,
                at,
                rules: RULES,
                kind: line.kind,
                card: line.card,
                tokens: line.tokens,
                why: line.why,
                prev: prev.clone(),
            };
            let written = entry.line();
            prev = hash(&written);
            block.push_str(&written);
            block.push('\n');
            out.push(entry);
        }
        if let Some(complete) = cut {
            file.set_len(complete as u64)?;
        }
        file.seek(io::SeekFrom::End(0))?; // the lock is ours, so the end stays the end until this write
        file.write_all(block.as_bytes())?;
        file.sync_data()?;
        Ok(out)
    }
}

/// The welcome mark's file, in `data\` beside `market.key` (`PLAN.md` 3.1): outside the profile, so a reset keeps it
/// and the welcome pays once per installation.
pub const WELCOME_MARK: &str = "market.welcome";

/// The first start (`PLAN.md` 2.4): one `starter` line per card (the 11 dealt and the RARE picked) and, when `mark`
/// is not there yet, the [`WELCOME`] line, under one lock; then the mark. A log that already holds a starter copy gets
/// nothing (an empty result). `why` names the choice ("starter All seasons").
pub fn first_start(
    log: &Log,
    mark: &std::path::Path,
    at: OffsetDateTime,
    cards: &[u32],
    why: &str,
) -> io::Result<Vec<Entry>> {
    let welcome = !mark.exists();
    let out = log.append_lines_at(at, |state| {
        if state.cards.values().any(|c| c.starter > 0) {
            return Ok(vec![]);
        }
        let mut lines: Vec<Line> = cards
            .iter()
            .map(|&no| Line {
                kind: Kind::Starter,
                card: Some(no),
                tokens: 0,
                why: why.to_owned(),
            })
            .collect();
        if welcome {
            lines.push(Line {
                kind: Kind::Welcome,
                card: None,
                tokens: WELCOME,
                why: "welcome".to_owned(),
            });
        }
        Ok(lines)
    })?;
    if out.iter().any(|e| e.kind == Kind::Welcome) {
        // shortcut: a crash between the log's write and this one leaves no mark, so a later reset pays the welcome
        // again; the market refuses a second welcome per key (3.1b). Upgrade if local tokens ever matter alone.
        std::fs::write(mark, format!("{}\n", at.unix_timestamp()))?;
    }
    Ok(out)
}

/// Why a copy cannot convert; the caller words it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoConvert {
    /// No earned copy at all: a starter or imported copy is never converted.
    NoEarned,
    /// The last unlisted earned copy stays: only a spare converts, and a copy out on the market is not one.
    LastEarned,
}

/// CONVERT: [`CONVERT_SHARE`] of the card's value for a spare earned copy. Refused for the last unlisted earned copy
/// (a listed copy is out on the market, so it does not count) and for a starter or imported copy (a starter or imported
/// copy does not make an earned one a spare).
pub fn convert(copies: &Copies, card: &Card) -> Result<u32, NoConvert> {
    if copies.earned == 0 {
        return Err(NoConvert::NoEarned);
    }
    if copies.earned.saturating_sub(copies.listed) < 2 {
        return Err(NoConvert::LastEarned);
    }
    Ok(value(card) * CONVERT_SHARE / 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;

    fn when(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &Rfc3339).unwrap()
    }

    /// The week of the test log's own instant: Wednesday 11 November 2026, noon.
    fn this_week() -> u32 {
        week(when("2026-11-11T12:00:00+01:00"))
    }

    /// A plain card of the value table: no series, no secret, one season and club.
    fn card(rarity: Rarity, total: u32) -> Card<'static> {
        Card {
            card_no: 0,
            player: 0,
            rarity,
            total,
            series: "",
            secret: false,
            season: 2017,
            nation: "Italy",
            club: "",
            position: Position::Mf,
        }
    }

    /// A log built line by line, each line chained to the one before it.
    #[derive(Default)]
    struct Fixture {
        text: String,
        seq: u64,
    }

    impl Fixture {
        fn add(&mut self, at: &str, kind: Kind, card: Option<u32>, tokens: i64) -> &mut Self {
            self.seq += 1;
            let entry = Entry {
                seq: self.seq,
                at: when(at),
                rules: RULES,
                kind,
                card,
                tokens,
                why: format!("{kind:?} {card:?}"),
                prev: self.text.lines().next_back().map(hash).unwrap_or_default(),
            };
            self.text.push_str(&entry.line());
            self.text.push('\n');
            self
        }
    }

    /// A fixed log of 10 lines, all on one business day except the last two.
    fn fixed() -> Fixture {
        let mut f = Fixture::default();
        f.add("2026-11-11T12:00:00+01:00", Kind::Starter, Some(1), 0)
            .add("2026-11-11T12:00:00+01:00", Kind::Starter, Some(2), 0)
            .add("2026-11-11T12:01:00+01:00", Kind::Match, Some(3), 50)
            .add("2026-11-11T12:02:00+01:00", Kind::Match, Some(3), 50)
            .add(
                "2026-11-11T12:03:00+01:00",
                Kind::Pack,
                None,
                -STANDARD_PACK,
            )
            .add("2026-11-11T12:03:00+01:00", Kind::Pack, Some(4), 0)
            .add("2026-11-11T12:04:00+01:00", Kind::Convert, Some(3), 48)
            .add("2026-11-11T12:05:00+01:00", Kind::Import, Some(1), 0)
            .add("2026-11-11T12:06:00+01:00", Kind::List, Some(2), 0)
            .add("2026-11-11T12:07:00+01:00", Kind::Unlist, Some(2), 0);
        f
    }

    /// (3.1) the fold's copies per source, the balance and the week's match tokens; a torn last line dropped.
    #[test]
    fn fold_counts_copies_and_drops_a_torn_last_line() {
        let f = fixed();
        let folded = fold_at(&f.text, this_week());
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        let s = &folded.state;
        assert_eq!(s.seq, 10);
        assert_eq!(
            s.copies(1),
            Copies {
                starter: 1,
                imported: 1,
                ..Copies::default()
            }
        );
        assert_eq!(
            s.copies(2),
            Copies {
                starter: 1,
                ..Copies::default()
            },
            "listed then unlisted"
        );
        assert_eq!(
            s.copies(3),
            Copies {
                earned: 1,
                ..Copies::default()
            },
            "2 earned, one converted"
        );
        assert_eq!(
            s.copies(4),
            Copies {
                earned: 1,
                ..Copies::default()
            }
        );
        assert_eq!(s.copies(9), Copies::default());
        assert_eq!(s.tokens, 50 + 50 - STANDARD_PACK + 48);
        assert_eq!(s.match_week, 100);
        assert!(s.copies(3).owned());

        // A torn last line (line 10): dropped with a warning, the 9 lines before it kept.
        let mut torn = f.text.clone();
        torn.truncate(torn[..torn.len() - 1].rfind('\n').unwrap() + 1);
        torn.push_str("{\"seq\": 99, \"at\": \"2026-11-11T0");
        let folded = fold_at(&torn, this_week());
        assert_eq!(folded.warnings.len(), 1, "{:?}", folded.warnings);
        assert!(
            folded.warnings[0].starts_with("line 10 does not parse"),
            "{:?}",
            folded.warnings
        );
        assert_eq!(folded.state.seq, 9);
        assert_eq!(
            folded.state.tokens, s.tokens,
            "the torn line moved no token"
        );
        assert_eq!(
            folded.state.copies(2).listed,
            1,
            "the unlist was in the torn line"
        );
    }

    /// (3.1) a line whose `prev` is not the line before it: the chain reports it, the fold goes on.
    #[test]
    fn fold_reports_a_broken_chain() {
        let f = fixed();
        let broken = f.text.replacen("\"match\"", "\"craft\"", 1);
        let folded = fold_at(&broken, this_week());
        assert_eq!(folded.warnings.len(), 1, "{:?}", folded.warnings);
        assert!(
            folded.warnings[0].contains("prev is not the SHA-256"),
            "{:?}",
            folded.warnings
        );
        assert_eq!(
            folded.state.seq, 10,
            "the lines after the break are still read"
        );
    }

    /// (finding 10) a `seq` that is not the line before plus one warns, and the fold still reads the lines after it.
    #[test]
    fn fold_reports_a_seq_out_of_order() {
        let mut f = Fixture::default();
        f.add("2026-11-11T12:00:00+01:00", Kind::Match, Some(1), 50);
        f.seq += 1; // the next line claims 3
        f.add("2026-11-11T12:01:00+01:00", Kind::Match, Some(1), 50);
        let folded = fold_at(&f.text, this_week());
        assert_eq!(folded.warnings.len(), 1, "{:?}", folded.warnings);
        assert!(
            folded.warnings[0].starts_with("line 2: seq is 3"),
            "{:?}",
            folded.warnings
        );
        assert!(!folded.unreadable);
        assert_eq!(folded.state.seq, 3, "the fold goes on");
        assert_eq!(folded.state.tokens, 100);
    }

    /// (a) PLAN 2.1: base x rating step x series factor, the secret flat prices and the second block's TOTAL.
    #[test]
    fn value_table() {
        // TOTAL 84 x1, 85 and 89 x1.5, 90 and 94 x2.5, 95 x4, no series factor: the table's cells at their edges.
        for (total, expected) in [
            (84, [40, 160, 400, 1_500, 2_500]),
            (85, [60, 240, 600, 2_250, 3_750]),
            (89, [60, 240, 600, 2_250, 3_750]),
            (90, [100, 400, 1_000, 3_750, 6_250]),
            (94, [100, 400, 1_000, 3_750, 6_250]),
            (95, [160, 640, 1_600, 6_000, 10_000]),
        ] {
            for (rarity, want) in Rarity::ALL.into_iter().zip(expected) {
                assert_eq!(
                    value(&card(rarity, total)),
                    want,
                    "{rarity:?} at TOTAL {total}"
                );
            }
        }
        assert_eq!(step(84), 100);
        assert_eq!(step(85), 150);
        assert_eq!(step(89), 150);
        assert_eq!(step(90), 250);
        assert_eq!(step(94), 250);
        assert_eq!(step(95), 400);
        assert_eq!(step(100), 400);
        // A black at 87 is 160 x 1.5 = 240; its floor, stock and ceiling are 192, 216 and 228.
        assert_eq!(value(&card(Rarity::Special, 87)), 240);
        assert_eq!(floor_value(240), 192);
        assert_eq!(stock_value(240), 216);
        assert_eq!(ceiling_value(240), 228);
        // A premium RARE: MVP at 96 is 400 x 4 x 2.5, WBE at 91 is 400 x 2.5 x 1.5, a plain RARE keeps x1.
        let mvp = Card {
            series: "MVP",
            ..card(Rarity::Rare, 96)
        };
        assert_eq!(value(&mvp), 4_000);
        assert_eq!(factor(Rarity::Rare, "MVP"), 250);
        assert_eq!(factor(Rarity::Rare, "WMVP"), 250);
        let wbe = Card {
            series: "WBE",
            ..card(Rarity::Rare, 91)
        };
        assert_eq!(value(&wbe), 1_500);
        assert_eq!(factor(Rarity::Rare, "BFW"), 150);
        assert_eq!(factor(Rarity::Rare, "GRP"), 100, "not premium (s1)");
        assert_eq!(factor(Rarity::AllTimeLegend, "MVP"), 100, "RARE only");
        // A secret card takes its series' flat price: HOLE 6,250, KOLE 10,000, a white reprint 40.
        let secret = |rarity: Rarity, series: &'static str| Card {
            series,
            secret: true,
            total: 100,
            ..card(rarity, 100)
        };
        assert_eq!(value(&secret(Rarity::AllTimeLegend, "HOLE")), 6_250);
        assert_eq!(value(&secret(Rarity::AllTimeLegend, "LEOC")), 6_250);
        assert_eq!(value(&secret(Rarity::AllTimeLegend, "S-ATLE")), 6_250);
        assert_eq!(value(&secret(Rarity::AllTimeLegend, "KOLE")), 10_000);
        assert_eq!(value(&secret(Rarity::AllTimeLegend, "ATLE")), 3_750);
        assert_eq!(value(&secret(Rarity::Regular, "ATLE-RE")), 40);
        // A printed KLE at 99 is 2,500 x 4, the cell its three secret siblings share.
        let printed = Card {
            series: "KLE",
            ..card(Rarity::AllTimeLegend, 99)
        };
        assert_eq!(value(&printed), 10_000);
        // The one RARE that prints a real TOTAL 100 is not secret: 400 x 4, no flat price.
        assert_eq!(value(&card(Rarity::Rare, 100)), 1_600);
        // The one line that needs rounding: 95 % of 142.5, and the narrowest band of all (a white 40: 32-38).
        assert_eq!(floor_value(40), 32);
        assert_eq!(ceiling_value(40), 38);
    }

    /// (M1) the secret mark is the six printed stats, not the tier: the one RARE that prints a real TOTAL 100 and an
    /// ALL TIME LEGEND that prints real stats are priced from their TOTAL.
    #[test]
    fn secret_is_the_printed_placeholder() {
        assert!(secret([16, 16, 16, 16, 16, 20]), "the placeholder");
        // The one RARE that prints a real TOTAL 100: 20/8/20/16/20/16, a stat block like any other.
        assert!(!secret([20, 8, 20, 16, 20, 16]), "real stats are no secret");
        let rare = Card {
            secret: secret([20, 8, 20, 16, 20, 16]),
            ..card(Rarity::Rare, 100)
        };
        assert!(!rare.secret);
        assert_eq!(value(&rare), 1_600, "400 x 4, no flat cell");
        // An ALL TIME LEGEND that prints real stats at TOTAL 95 is 2,500 x 4, not the flat 3,750.
        let atle = Card {
            secret: secret([18, 12, 19, 16, 14, 20]),
            ..card(Rarity::AllTimeLegend, 95)
        };
        assert!(!atle.secret);
        assert_eq!(value(&atle), 10_000);
        // The placeholder on the same tier, with its series, takes the flat cell.
        let hole = Card {
            secret: secret([16, 16, 16, 16, 16, 20]),
            series: "HOLE",
            ..card(Rarity::AllTimeLegend, 100)
        };
        assert!(hole.secret);
        assert_eq!(value(&hole), 6_250);
    }

    /// (e) PLAN 2.3: the release crafts white and black only, at the card's value.
    #[test]
    fn craft_is_white_and_black_only() {
        assert_eq!(craft(&card(Rarity::Regular, 84)), Some(40));
        assert_eq!(craft(&card(Rarity::Regular, 100)), Some(160));
        assert_eq!(craft(&card(Rarity::Special, 87)), Some(240));
        assert_eq!(craft(&card(Rarity::Special, 96)), Some(640));
        for rarity in [Rarity::Rare, Rarity::Legend, Rarity::AllTimeLegend] {
            assert_eq!(craft(&card(rarity, 90)), None, "{rarity:?} is not crafted");
        }
    }

    /// The share of each tier in `n` draws, in percent, after step-down.
    fn shares<'a>(draw: impl Fn(&mut Rng) -> &'a Card<'a>, n: usize, seed: u64) -> [f64; 5] {
        let mut rng = Rng::new(seed);
        let mut count = [0u32; 5];
        for _ in 0..n {
            count[draw(&mut rng).rarity as usize] += 1;
        }
        count.map(|c| f64::from(c) * 100.0 / n as f64)
    }

    /// (b) PLAN 2.1: 100,000 seeded draws keep the table's odds from the whole pool and from one small season whose
    /// pool carries every tier (a tier the pool lacks steps down, see the fixture below).
    #[test]
    fn draws_keep_the_odds() {
        let odds = [68.0, 26.0, 5.0, 0.2, 0.8];
        // The whole pool: every tier, several cards each (the pool's own shares do not move the odds).
        let whole: Vec<Card> = (0..5)
            .flat_map(|t| {
                let rarity = Rarity::ALL[t];
                (0..20).map(move |i| Card {
                    card_no: (t * 20 + i) as u32,
                    player: (t * 20 + i) as u32,
                    rarity,
                    total: 85,
                    season: 2017,
                    nation: "Italy",
                    club: "Juventus",
                    series: "",
                    secret: false,
                    position: Position::Mf,
                })
            })
            .collect();
        let got = shares(|rng| match_card(rng, &whole).unwrap(), 100_000, 0x0005_EED1);
        // The three tiers a 100,000-draw sample measures alone; the top two are 1 card in 100 together.
        for (tier, (got, want)) in got.iter().zip(odds).enumerate().take(3) {
            assert!(
                (got - want).abs() < 0.3,
                "{:?} {got} %, want {want} %",
                Rarity::ALL[tier]
            );
        }
        assert!(
            (got[3] + got[4] - 1.0).abs() < 0.1,
            "LEGEND + ALL TIME LEGEND {} %",
            got[3] + got[4]
        );
        assert!(got[3] < got[4], "LEGEND is the rarer tier: {got:?}");
        // One small season: 26 cards of 2004, every tier (a season whose pool has all five keeps the table's odds).
        let counts = [10, 8, 5, 2, 1];
        let season: Vec<Card> = (0..5)
            .flat_map(|t| {
                let rarity = Rarity::ALL[t];
                (0..counts[t]).map(move |i| Card {
                    card_no: (100 + t * 10 + i) as u32,
                    player: (100 + t * 10 + i) as u32,
                    rarity,
                    total: 90,
                    season: 2004,
                    nation: "Italy",
                    club: "Juventus",
                    series: "",
                    secret: false,
                    position: Position::Df,
                })
            })
            .collect();
        let got = shares(
            |rng| draw(rng, &season, |c| c.season == 2004, &Rarity::ALL).unwrap(),
            100_000,
            0x0005_EED2,
        );
        for (tier, (got, want)) in got.iter().zip(odds).enumerate() {
            assert!(
                (got - want).abs() < 1.0,
                "season {:?} {got} %, want {want} %",
                Rarity::ALL[tier]
            );
        }
    }

    /// (b) a nation with no RARE, LEGEND or ALL TIME LEGEND: those rolls step down to SPECIAL, 68 / 32.
    #[test]
    fn draws_step_down_in_a_short_nation() {
        let mut pool: Vec<Card> = vec![];
        for i in 0..25 {
            pool.push(Card {
                card_no: i,
                player: i,
                rarity: Rarity::Regular,
                total: 80,
                season: 2010,
                nation: "Korea Republic",
                club: "",
                series: "",
                secret: false,
                position: Position::Mf,
            });
        }
        for i in 25..27 {
            pool.push(Card {
                card_no: i,
                player: i,
                rarity: Rarity::Special,
                total: 88,
                season: 2010,
                nation: "Korea Republic",
                club: "",
                series: "",
                secret: false,
                position: Position::Gk,
            });
        }
        let got = shares(|rng| match_card(rng, &pool).unwrap(), 100_000, 0x0005_EED3);
        assert!((got[0] - 68.0).abs() < 1.0, "REGULAR {} %", got[0]);
        assert!((got[1] - 32.0).abs() < 1.0, "SPECIAL {} %", got[1]);
        assert_eq!(got[2..], [0.0, 0.0, 0.0]);
    }

    /// Every pack holds 3 cards, all from the pool asked for, and at least one SPECIAL or better.
    #[test]
    fn pack_guarantee_and_pool() {
        let mut pool: Vec<Card> = vec![];
        for i in 0..30 {
            let rarity = Rarity::ALL[i % 5];
            pool.push(Card {
                card_no: i as u32,
                player: i as u32,
                rarity,
                total: 92,
                season: 2017,
                nation: if i < 15 { "Japan" } else { "Brazil" },
                club: if i < 15 { "Urawa Reds" } else { "Sao Paulo" },
                series: "",
                secret: false,
                position: Position::Mf,
            });
        }
        // A pool of REGULAR cards only: the guarantee cannot be met and the 3 cards stay REGULAR.
        let flat: Vec<Card> = pool
            .iter()
            .filter(|c| c.rarity == Rarity::Regular)
            .map(|c| Card {
                rarity: Rarity::Regular,
                ..*c
            })
            .collect();
        let mut rng = Rng::new(7);
        for _ in 0..200 {
            let cards = pack(&mut rng, &pool, |_| true);
            assert_eq!(cards.len(), PACK_CARDS);
            assert!(cards.iter().any(|c| c.rarity != Rarity::Regular));
            let japan = pack(&mut rng, &pool, |c| c.nation == "Japan");
            assert_eq!(japan.len(), PACK_CARDS);
            assert!(japan.iter().all(|c| c.nation == "Japan"), "{japan:?}");
            let season = pack(&mut rng, &pool, |c| c.season == 2017);
            assert!(season.iter().all(|c| c.season == 2017));
            assert_eq!(pack(&mut rng, &flat, |_| true).len(), PACK_CARDS);
        }
    }

    /// A fixture pool card: `player` and `card_no` its own, Juventus and Italy, season 2004, no series.
    fn pool_card(n: u32, rarity: Rarity, total: u32, position: Position) -> Card<'static> {
        Card {
            card_no: n,
            player: n,
            rarity,
            total,
            series: "",
            secret: false,
            season: 2004,
            nation: "Italy",
            club: "Juventus",
            position,
        }
    }

    /// Two fixture cards of each of `n` players, their ids from `base`: a pack that ignored the rule would repeat one.
    fn two_per_player(
        pool: &mut Vec<Card<'static>>,
        base: u32,
        n: u32,
        rarity: Rarity,
        nation: &'static str,
        season: u16,
    ) {
        for i in 0..n {
            pool.push(Card {
                card_no: pool.len() as u32,
                player: base + i / 2,
                rarity,
                total: 85,
                series: "",
                secret: false,
                season,
                nation,
                club: "",
                position: Position::Mf,
            });
        }
    }

    /// The starter deal: 11 REGULAR cards of the season by position, no card twice; 3 distinct RARE cards.
    #[test]
    fn starter_deal_by_position() {
        let mut pool: Vec<Card> = vec![];
        for (i, position) in [Position::Gk, Position::Df, Position::Mf, Position::Fw]
            .into_iter()
            .enumerate()
        {
            for n in 0..6 {
                pool.push(pool_card((i * 6 + n) as u32, Rarity::Regular, 80, position));
            }
            for n in 0..4 {
                pool.push(pool_card(
                    (100 + i * 4 + n) as u32,
                    Rarity::Rare,
                    90,
                    position,
                ));
            }
            // A RARE of another season: the face-up three stay the choice's own.
            pool.push(Card {
                season: 2017,
                ..pool_card((200 + i) as u32, Rarity::Rare, 90, position)
            });
        }
        let mut rng = Rng::new(11);
        let eleven = starter(&mut rng, &pool, Start::Season(2004));
        assert_eq!(eleven.len(), 11);
        assert!(
            eleven
                .iter()
                .all(|c| c.rarity == Rarity::Regular && c.season == 2004)
        );
        let count = |p: Position| eleven.iter().filter(|c| c.position == p).count();
        assert_eq!(
            [
                count(Position::Gk),
                count(Position::Df),
                count(Position::Mf),
                count(Position::Fw)
            ],
            [1, 4, 4, 2]
        );
        let mut nos: Vec<u32> = eleven.iter().map(|c| c.card_no).collect();
        nos.sort_unstable();
        nos.dedup();
        assert_eq!(nos.len(), 11, "no card twice");
        let mut players: Vec<u32> = eleven.iter().map(|c| c.player).collect();
        players.sort_unstable();
        players.dedup();
        assert_eq!(players.len(), 11, "one card per player");
        let rares = starter_rares(&mut rng, &pool, Start::Season(2004));
        assert_eq!(rares.len(), 3);
        assert!(
            rares
                .iter()
                .all(|c| c.rarity == Rarity::Rare && c.season == 2004)
        );
    }

    /// (2.4 steps 2 and 3) one card per player: a pool holding two cards of a player never deals both, and the three
    /// face-up RARE cards are three players.
    #[test]
    fn starter_deal_takes_one_card_per_player() {
        let mut pool: Vec<Card> = vec![];
        {
            let mut push = |card_no: u32, player: u32, position: Position, rarity: Rarity| {
                pool.push(Card {
                    card_no,
                    player,
                    rarity,
                    total: 80,
                    series: "",
                    secret: false,
                    season: 2004,
                    nation: "Italy",
                    club: "Juventus",
                    position,
                });
            };
            // GK: two cards of player 7. DF: 5 cards, two of player 13. MF: 6 cards, two of player 20. FW: 3 cards.
            push(1, 7, Position::Gk, Rarity::Regular);
            push(2, 7, Position::Gk, Rarity::Regular);
            for (card_no, player) in [(3, 10), (4, 11), (5, 12), (6, 13), (7, 13)] {
                push(card_no, player, Position::Df, Rarity::Regular);
            }
            for (card_no, player) in [(8, 20), (9, 21), (10, 22), (11, 23), (12, 24), (13, 20)] {
                push(card_no, player, Position::Mf, Rarity::Regular);
            }
            for (card_no, player) in [(14, 30), (15, 31), (16, 31)] {
                push(card_no, player, Position::Fw, Rarity::Regular);
            }
            // The RARE heroes: three players, four cards (player 40 has two).
            for (card_no, player) in [(17, 40), (18, 40), (19, 41), (20, 42)] {
                push(card_no, player, Position::Mf, Rarity::Rare);
            }
        }
        let mut rng = Rng::new(5);
        for _ in 0..100 {
            let eleven = starter(&mut rng, &pool, Start::Season(2004));
            assert_eq!(eleven.len(), 11);
            let mut players: Vec<u32> = eleven.iter().map(|c| c.player).collect();
            players.sort_unstable();
            players.dedup();
            assert_eq!(players.len(), 11, "one card per player");
        }
        for _ in 0..100 {
            let rares = starter_rares(&mut rng, &pool, Start::Season(2004));
            assert_eq!(rares.len(), 3);
            let mut players: Vec<u32> = rares.iter().map(|c| c.player).collect();
            players.sort_unstable();
            players.dedup();
            assert_eq!(players.len(), 3, "three players face up");
        }
    }

    /// (c) PLAN 2.2: 50 a match until the week's 1,050 (21 matches), nothing on the 22nd but its card; the week turns
    /// on Monday at 05:00; a convert adds outside the cap.
    #[test]
    fn match_tokens_and_the_week() {
        assert_eq!(MATCH_TOKENS, 50);
        assert_eq!(WEEK_CAP, 21 * MATCH_TOKENS);
        assert_eq!(match_tokens(0), 50);
        assert_eq!(match_tokens(1_000), 50);
        assert_eq!(match_tokens(WEEK_CAP), 0);
        assert_eq!(match_tokens(2_000), 0);

        // The week label turns at Monday 05:00, on each line's own offset; the business day turns with it.
        let monday_0459 = "2026-11-09T04:59:00+01:00";
        let monday_0500 = "2026-11-09T05:00:00+01:00";
        assert_eq!(day(when(monday_0459)), 2026_1108);
        assert_eq!(day(when(monday_0500)), 2026_1109);
        assert_ne!(week(when(monday_0459)), week(when(monday_0500)));
        assert_eq!(
            week(when(monday_0500)),
            week(when("2026-11-15T23:59:00+01:00")),
            "Sunday night is the same week"
        );
        assert_ne!(
            week(when(monday_0500)),
            week(when("2026-11-16T05:00:00+01:00")),
            "the next Monday opens the next one"
        );
        assert_eq!(
            week(when("2026-11-11T12:00:00+01:00")),
            week(when("2026-11-11T11:00:00+00:00")),
            "the same instant in another offset"
        );

        // 22 matches in one week: the first 21 add 1,050, the 22nd adds nothing and still deals its card.
        let mut f = Fixture::default();
        let mut earned = 0;
        for n in 0..22 {
            let pays = match_tokens(earned);
            f.add("2026-11-11T09:00:00+01:00", Kind::Match, Some(n), pays);
            earned += pays;
        }
        f.add("2026-11-11T09:30:00+01:00", Kind::Convert, Some(1), 8);
        let state = fold_at(&f.text, this_week()).state;
        assert_eq!(state.match_week, WEEK_CAP);
        assert_eq!(state.match_pays(), 0, "the 22nd match adds nothing");
        assert_eq!(
            state.copies(21).earned,
            1,
            "the 22nd match still deals its card"
        );
        assert_eq!(state.tokens, WEEK_CAP + 8, "a convert adds past the cap");
        // A match at Monday 04:59 belongs to the week before; the one at 05:00 opens the new week.
        let mut f = Fixture::default();
        f.add(monday_0459, Kind::Match, Some(1), 50)
            .add(monday_0500, Kind::Match, Some(2), 50);
        let old = fold_at(&f.text, week(when(monday_0459)));
        assert_eq!(old.state.match_week, 50, "only the 04:59 match");
        assert_eq!(old.state.tokens, 100, "the balance counts both");
        assert_eq!(
            fold_at(&f.text, week(when(monday_0500))).state.match_week,
            50
        );
    }

    /// (d) PLAN 2.3: 10,000 seeded packs of each kind never hold one player twice, and a Nation pack costs 200.
    #[test]
    fn packs_never_hold_one_player_twice() {
        let mut pool: Vec<Card> = vec![];
        // Korea Republic as the pool has it: 25 REGULAR and 2 SPECIAL, two cards of each player.
        two_per_player(&mut pool, 0, 25, Rarity::Regular, "Korea Republic", 2010);
        two_per_player(&mut pool, 100, 2, Rarity::Special, "Korea Republic", 2010);
        // Chile, the smallest season, and two more nations the Standard pack draws from.
        two_per_player(&mut pool, 200, 20, Rarity::Regular, "Chile", 2014);
        two_per_player(&mut pool, 300, 5, Rarity::Special, "Chile", 2014);
        two_per_player(&mut pool, 400, 2, Rarity::Rare, "Chile", 2014);
        two_per_player(&mut pool, 500, 12, Rarity::Regular, "Italy", 2006);
        two_per_player(&mut pool, 600, 3, Rarity::Special, "Italy", 2006);
        two_per_player(&mut pool, 700, 4, Rarity::Regular, "Japan", 2017);
        type Pick = fn(&Card) -> bool;
        let kinds: [(&str, Pick); 4] = [
            ("Korea Republic", |c: &Card| c.nation == "Korea Republic"),
            ("Chile", |c: &Card| c.nation == "Chile"),
            ("the smallest season", |c: &Card| c.season == 2006),
            ("Standard", |_: &Card| true),
        ];
        let mut rng = Rng::new(0x2026_1111);
        for (name, pick) in kinds {
            for _ in 0..10_000 {
                let cards = pack(&mut rng, &pool, pick);
                assert_eq!(cards.len(), PACK_CARDS, "{name}");
                let mut players: Vec<u32> = cards.iter().map(|c| c.player).collect();
                players.sort_unstable();
                players.dedup();
                assert_eq!(players.len(), PACK_CARDS, "{name}: {cards:?}");
            }
        }
        assert_eq!(NATION_PACK, 200);
    }

    /// The store's fixture: Korea Republic short as the catalogue is (its 2 SPECIAL cards belong to players who also
    /// have a REGULAR card, review N5's case, here with 4 REGULAR players so it comes up often), Chile and Japan in
    /// 2014, Italy in 2006.
    fn store_pool() -> Vec<Card<'static>> {
        let mut pool: Vec<Card> = vec![];
        two_per_player(&mut pool, 0, 8, Rarity::Regular, "Korea Republic", 2010);
        two_per_player(&mut pool, 0, 4, Rarity::Special, "Korea Republic", 2010);
        two_per_player(&mut pool, 200, 20, Rarity::Regular, "Chile", 2014);
        two_per_player(&mut pool, 300, 5, Rarity::Special, "Chile", 2014);
        two_per_player(&mut pool, 400, 2, Rarity::Rare, "Chile", 2014);
        two_per_player(&mut pool, 500, 12, Rarity::Regular, "Italy", 2006);
        two_per_player(&mut pool, 600, 3, Rarity::Special, "Italy", 2006);
        two_per_player(&mut pool, 700, 6, Rarity::Regular, "Japan", 2014);
        pool
    }

    /// 2.1 (b), (c): 10,000 seeded packs of each kind, Korea Republic included, each hold 1 SPECIAL or better, only
    /// the pack's season or nation, and no player twice.
    #[test]
    fn every_pack_keeps_its_guarantee_and_its_pool() {
        let pool = store_pool();
        let mut rng = Rng::new(0x2026_1121);
        for kind in [
            Pack::Standard,
            Pack::Season(2014),
            Pack::Nation("Korea Republic"),
            Pack::Nation("Chile"),
        ] {
            for _ in 0..10_000 {
                let cards = pack(&mut rng, &pool, |c| kind.has(c));
                assert_eq!(cards.len(), PACK_CARDS, "{kind:?}");
                assert!(
                    cards.iter().any(|c| c.rarity != Rarity::Regular),
                    "{kind:?}: {cards:?}"
                );
                assert!(cards.iter().all(|c| kind.has(c)), "{kind:?}: {cards:?}");
                let mut players: Vec<u32> = cards.iter().map(|c| c.player).collect();
                players.sort_unstable();
                players.dedup();
                assert_eq!(players.len(), PACK_CARDS, "{kind:?}: {cards:?}");
            }
        }
    }

    /// 2.1: the odds the store shows, after the step-down.
    #[test]
    fn pack_odds_step_down() {
        let pool = store_pool();
        assert_eq!(
            pack_odds(&pool, Pack::Nation("Korea Republic")),
            [680, 320, 0, 0, 0]
        );
        assert_eq!(
            pack_odds(&pool, Pack::Nation("Chile")),
            [680, 260, 60, 0, 0]
        );
        assert_eq!(pack_odds(&pool, Pack::Season(2006)), [680, 320, 0, 0, 0]);
        assert_eq!(pack_odds(&[], Pack::Standard), [0; 5]);
        let full = pack_odds(&pool, Pack::Standard);
        assert_eq!(full.iter().sum::<u32>(), 1_000);
    }

    /// 2.1 (a): 99 tokens buy nothing and leave the log as it was; 100 write the price line and 3 card lines.
    #[test]
    fn a_pack_costs_its_price() {
        let pool = store_pool();
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.append_at(
            when("2026-11-11T12:00:00+01:00"),
            Kind::Welcome,
            None,
            99,
            "welcome",
        )
        .unwrap();
        let before = std::fs::read_to_string(log.path()).unwrap();
        let mut rng = Rng::new(5);
        let e = buy_pack(
            &log,
            when("2026-11-11T12:01:00+01:00"),
            &mut rng,
            &pool,
            Pack::Standard,
        )
        .unwrap_err();
        assert_eq!(e.to_string(), NOT_ENOUGH_TOKENS);
        assert_eq!(std::fs::read_to_string(log.path()).unwrap(), before);
        log.append_at(
            when("2026-11-11T12:02:00+01:00"),
            Kind::Convert,
            Some(1),
            1,
            "convert",
        )
        .unwrap();
        let wrote = buy_pack(
            &log,
            when("2026-11-11T12:03:00+01:00"),
            &mut rng,
            &pool,
            Pack::Standard,
        )
        .unwrap();
        assert_eq!(wrote.len(), 1 + PACK_CARDS);
        assert!(
            wrote
                .iter()
                .all(|e| e.kind == Kind::Pack && e.why == "pack standard")
        );
        assert_eq!((wrote[0].card, wrote[0].tokens), (None, -STANDARD_PACK));
        assert!(wrote[1..].iter().all(|e| e.card.is_some() && e.tokens == 0));
        let folded = fold_at(&log.lines().unwrap(), this_week());
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.tokens, 0);
        for e in &wrote[1..] {
            assert_eq!(folded.state.copies(e.card.unwrap()).earned, 1);
        }
        // a pool that cannot fill a pack takes nothing either
        log.append_at(
            when("2026-11-11T12:04:00+01:00"),
            Kind::Convert,
            Some(1),
            200,
            "convert",
        )
        .unwrap();
        let before = std::fs::read_to_string(log.path()).unwrap();
        assert!(
            buy_pack(
                &log,
                when("2026-11-11T12:05:00+01:00"),
                &mut rng,
                &pool,
                Pack::Nation("Ghana")
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(log.path()).unwrap(), before);
    }

    /// 2.1 (d): the Nation picker's 24, none of the 7 dropped.
    #[test]
    fn the_nation_list() {
        assert_eq!(NATIONS.len(), 24);
        let mut sorted = NATIONS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 24);
        for out in [
            "SAMURAI BLUE",
            "Mexico",
            "Romania",
            "Paraguay",
            "Cote d'Ivoire",
            "Ghana",
            "Greece",
            "Nigeria",
        ] {
            assert!(!NATIONS.contains(&out), "{out}");
        }
    }

    /// (e) PLAN 2.4: a season start is unbanded; All seasons and a league deal their 11 inside 80-82 with no secret
    /// card; a league start holds only that league's clubs; the table's names are the catalogue's (see the ignored
    /// test below).
    #[test]
    fn starter_modes_and_the_league_table() {
        let mut pool: Vec<Card> = vec![];
        for (i, position) in [Position::Gk, Position::Df, Position::Mf, Position::Fw]
            .into_iter()
            .enumerate()
        {
            for n in 0..4 {
                // Two clubs, four cards of each position: the band is reachable from either alone (4 DF, 4 MF).
                let player = (i * 10 + n) as u32;
                pool.push(Card {
                    card_no: player,
                    player,
                    total: 81,
                    club: "Arsenal",
                    ..pool_card(player, Rarity::Regular, 81, position)
                });
                pool.push(Card {
                    card_no: 200 + player,
                    player: 200 + player,
                    ..pool_card(200 + player, Rarity::Regular, 81, position)
                });
            }
            // A secret card of each club at 100: the band's deal must skip them.
            pool.push(Card {
                secret: true,
                total: 100,
                series: "ATLE-RE",
                club: "Arsenal",
                ..pool_card(400 + i as u32, Rarity::Regular, 100, position)
            });
            pool.push(Card {
                secret: true,
                total: 100,
                series: "ATLE-RE",
                ..pool_card(500 + i as u32, Rarity::Regular, 100, position)
            });
        }
        let mean =
            |deal: &[&Card]| deal.iter().map(|c| c.total).sum::<u32>() as f64 / deal.len() as f64;
        let mut rng = Rng::new(9);
        let all = starter(&mut rng, &pool, Start::AllSeasons);
        assert_eq!(all.len(), 11);
        assert!((80.0..=82.0).contains(&mean(&all)), "{}", mean(&all));
        assert!(all.iter().all(|c| !c.secret), "no secret card: {all:?}");
        let england = starter(&mut rng, &pool, Start::League("England (Premier League)"));
        assert_eq!(england.len(), 11);
        assert!(
            england.iter().all(|c| c.club == "Arsenal"),
            "only that league: {england:?}"
        );
        assert!(
            (80.0..=82.0).contains(&mean(&england)),
            "{}",
            mean(&england)
        );
        // A season start is unbanded: it takes its first deal as it comes, and the picker shows the season's mean.
        assert_eq!(starter(&mut rng, &pool, Start::Season(2004)).len(), 11);
        assert!(!Start::Season(2004).banded(), "no band on a season");
        assert!(Start::AllSeasons.banded());
        assert!(Start::League("Italy (Serie A)").banded());
        // The league table: a club of a league, an unmapped name, and no club in two leagues.
        assert_eq!(league_of("Juventus"), Some("Italy (Serie A)"));
        assert_eq!(
            league_of("Kawasaki Frontale"),
            Some("Japan (J.League and forerunners)")
        );
        assert_eq!(league_of("Nowhere United"), None);
        assert!(!Start::League("Nowhere League").has(&pool[0]));
        assert!(
            starter(&mut rng, &pool, Start::League("Nowhere League")).is_empty(),
            "an unknown league name deals nothing"
        );
        let mut seen = std::collections::BTreeSet::new();
        for (league, clubs) in LEAGUES {
            assert!(!clubs.is_empty(), "{league}");
            for club in clubs {
                assert!(seen.insert(club), "{club} is in two leagues");
            }
        }
    }

    /// (e) the league table's club names are the reference catalogue's, as `board::Card::club` spells them: run it
    /// with `FORZA_CATALOGUE_TSV=<catalogue.tsv>` (it reads the player's own file and is off in CI).
    #[test]
    #[ignore = "reads the reference catalogue: set FORZA_CATALOGUE_TSV"]
    #[cfg(feature = "full")]
    fn league_clubs_are_the_catalogue_names() {
        let Ok(path) = std::env::var("FORZA_CATALOGUE_TSV") else {
            return;
        };
        let cards = crate::cards::load_catalogue(std::path::Path::new(&path)).unwrap();
        let clubs: std::collections::BTreeSet<&str> =
            cards.iter().map(|c| c.club.as_str()).collect();
        let missing: Vec<&str> = LEAGUES
            .iter()
            .flat_map(|(_, names)| *names)
            .copied()
            .filter(|name| !clubs.contains(name))
            .collect();
        assert!(missing.is_empty(), "not in the catalogue: {missing:?}");
    }

    /// (e) CONVERT: a spare pays 20 %, the last earned copy and a starter or imported copy never do.
    #[test]
    fn convert_takes_a_spare_earned_copy_only() {
        let spare = Copies {
            earned: 2,
            ..Copies::default()
        };
        // A SPECIAL at TOTAL 87 (value 240) converts for 48 and leaves 1 earned copy.
        assert_eq!(convert(&spare, &card(Rarity::Special, 87)), Ok(48));
        let mut state = State::default();
        state.cards.insert(7, spare);
        state.apply(
            &Entry {
                seq: 3,
                at: when("2026-11-11T12:00:00+01:00"),
                rules: RULES,
                kind: Kind::Convert,
                card: Some(7),
                tokens: 48,
                why: String::new(),
                prev: String::new(),
            },
            this_week(),
        );
        assert_eq!(state.copies(7).earned, 1);

        assert_eq!(
            convert(
                &Copies {
                    earned: 1,
                    ..Copies::default()
                },
                &card(Rarity::Special, 87)
            ),
            Err(NoConvert::LastEarned)
        );
        assert_eq!(
            convert(
                &Copies {
                    earned: 1,
                    starter: 1,
                    ..Copies::default()
                },
                &card(Rarity::Special, 87)
            ),
            Err(NoConvert::LastEarned)
        );
        assert_eq!(
            convert(
                &Copies {
                    earned: 1,
                    imported: 1,
                    ..Copies::default()
                },
                &card(Rarity::Special, 87)
            ),
            Err(NoConvert::LastEarned)
        );
        assert_eq!(
            convert(
                &Copies {
                    starter: 1,
                    ..Copies::default()
                },
                &card(Rarity::Special, 87)
            ),
            Err(NoConvert::NoEarned)
        );
        assert_eq!(
            convert(
                &Copies {
                    imported: 1,
                    ..Copies::default()
                },
                &card(Rarity::Special, 87)
            ),
            Err(NoConvert::NoEarned)
        );
        assert_eq!(
            convert(&Copies::default(), &card(Rarity::Special, 87)),
            Err(NoConvert::NoEarned)
        );
        // A spare top ALL TIME LEGEND pays 2,000.
        assert_eq!(convert(&spare, &card(Rarity::AllTimeLegend, 95)), Ok(2_000));
        // One of the two earned copies is out on the market: it is not spare, so nothing converts.
        assert_eq!(
            convert(
                &Copies {
                    earned: 2,
                    listed: 1,
                    ..Copies::default()
                },
                &card(Rarity::Special, 87)
            ),
            Err(NoConvert::LastEarned)
        );
    }

    /// (2.6) a copy out on the market is not spare: only earned - listed >= 2 converts, so one unlisted earned copy
    /// stays.
    #[test]
    fn convert_needs_an_unlisted_spare() {
        let copies = |earned, listed| Copies {
            earned,
            listed,
            ..Copies::default()
        };
        assert_eq!(convert(&copies(2, 0), &card(Rarity::Special, 87)), Ok(48));
        assert_eq!(
            convert(&copies(2, 1), &card(Rarity::Special, 87)),
            Err(NoConvert::LastEarned),
            "2 earned with 1 listed leaves no unlisted spare"
        );
        assert_eq!(
            convert(&copies(2, 2), &card(Rarity::Special, 87)),
            Err(NoConvert::LastEarned)
        );
        assert_eq!(
            convert(&copies(3, 1), &card(Rarity::Special, 87)),
            Ok(48),
            "3 earned with 1 listed keeps one unlisted copy"
        );
    }

    /// (3.1) a `market_sell` line is the sale of a listed copy: it lowers `earned` and `listed` together, and a starter
    /// copy of the same card keeps it placeable.
    #[test]
    fn a_sale_takes_the_listed_copy() {
        let mut f = Fixture::default();
        f.add("2026-11-11T12:00:00+01:00", Kind::Starter, Some(2), 0)
            .add("2026-11-11T12:01:00+01:00", Kind::Match, Some(2), 50)
            .add("2026-11-11T12:02:00+01:00", Kind::List, Some(2), 0)
            .add("2026-11-11T12:03:00+01:00", Kind::MarketSell, Some(2), 120);
        let folded = fold_at(&f.text, this_week());
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(
            folded.state.copies(2),
            Copies {
                starter: 1,
                ..Copies::default()
            },
            "earned 1 out, listed 1 out"
        );
        assert!(
            folded.state.copies(2).owned(),
            "the starter copy still plays"
        );
        assert_eq!(folded.state.tokens, 170);
    }

    /// (N7) a `welcome` line pays tokens only, and a `convert`, sale or `unlist` of a card the log never granted adds
    /// no entry: a page counting `state.cards` shows only the cards the player has held.
    #[test]
    fn unknown_card_lines_add_no_entry() {
        let mut f = Fixture::default();
        f.add("2026-11-11T12:00:00+01:00", Kind::Welcome, Some(9), WELCOME)
            .add("2026-11-11T12:01:00+01:00", Kind::Convert, Some(9), 48)
            .add("2026-11-11T12:02:00+01:00", Kind::MarketSell, Some(8), 192)
            .add("2026-11-11T12:03:00+01:00", Kind::Unlist, Some(8), 0)
            .add("2026-11-11T12:04:00+01:00", Kind::Welcome, None, WELCOME);
        let state = fold_at(&f.text, this_week()).state;
        assert!(state.cards.is_empty(), "{:?}", state.cards);
        assert_eq!(state.tokens, 2 * WELCOME + 48 + 192);

        // A card the log granted keeps its entry, and a welcome of it adds no copy.
        let mut f = Fixture::default();
        f.add("2026-11-11T12:00:00+01:00", Kind::Match, Some(1), 50)
            .add("2026-11-11T12:01:00+01:00", Kind::Welcome, Some(1), WELCOME);
        let state = fold_at(&f.text, this_week()).state;
        assert_eq!(state.cards.len(), 1);
        assert_eq!(state.copies(1).earned, 1);
    }

    /// (2.1) two grants of one card fold to 2 earned copies and write no convert line.
    #[test]
    fn second_grant_is_two_earned_copies() {
        let mut f = Fixture::default();
        f.add("2026-11-11T12:00:00+01:00", Kind::Match, Some(42), 50)
            .add("2026-11-11T12:01:00+01:00", Kind::Match, Some(42), 50);
        assert!(!f.text.contains("\"convert\""));
        let state = fold_at(&f.text, this_week()).state;
        assert_eq!(state.copies(42).earned, 2);
        assert_eq!(state.copies(42).starter, 0);
        assert_eq!(state.tokens, 100);
    }

    /// The append takes the lock, re-reads the log for `seq` and `prev`, and heals a torn last line first.
    #[test]
    fn append_chains_and_heals() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        assert_eq!(log.lines().unwrap(), "", "a missing file is an empty log");
        let first = log
            .append_at(
                when("2026-11-11T12:00:00+01:00"),
                Kind::Match,
                Some(1),
                50,
                "match 1",
            )
            .unwrap();
        assert_eq!(
            (first.seq, first.rules, first.prev.as_str()),
            (1, RULES, "")
        );
        let second = log
            .append_at(
                when("2026-11-11T12:01:00+01:00"),
                Kind::Convert,
                Some(1),
                8,
                "convert",
            )
            .unwrap();
        assert_eq!(second.seq, 2);
        assert_eq!(second.prev, hash(&first.line()));
        let folded = fold_at(&log.lines().unwrap(), this_week());
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.seq, 2);
        assert_eq!(folded.state.tokens, 58);

        // A torn tail from an interrupted write: cut off, not glued to the new line.
        let mut file = OpenOptions::new().append(true).open(log.path()).unwrap();
        file.write_all(b"{\"seq\": 3, \"at\": \"2026-11").unwrap();
        drop(file);
        let third = log
            .append_at(
                when("2026-11-11T12:02:00+01:00"),
                Kind::Match,
                Some(2),
                50,
                "match 2",
            )
            .unwrap();
        assert_eq!(third.seq, 3, "the torn line never became an entry");
        let folded = fold_at(&log.lines().unwrap(), this_week());
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.seq, 3);
    }

    /// A writer that holds the lock makes the append give up after its time box, and the grant lands after it.
    #[test]
    fn append_gives_up_after_the_lock_time_box() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.append(Kind::Match, Some(1), 50, "first").unwrap();
        let holder = OpenOptions::new()
            .read(true)
            .write(true)
            .open(log.path())
            .unwrap();
        holder.lock().unwrap();
        let start = Instant::now();
        let e = log.append(Kind::Match, Some(1), 50, "blocked").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::WouldBlock);
        assert!(start.elapsed() >= LOCK_WAIT, "{:?}", start.elapsed());
        holder.unlock().unwrap();
        let last = log.append(Kind::Match, Some(1), 50, "after").unwrap();
        assert_eq!(last.seq, 2);
        assert_eq!(
            fold_at(&log.lines().unwrap(), week(last.at))
                .state
                .match_week,
            100
        );
    }

    /// (finding 4) a whole last line whose newline never landed is kept and joined, not cut off.
    #[test]
    fn append_keeps_a_whole_last_line_without_its_newline() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        let first = log
            .append_at(
                when("2026-11-11T12:00:00+01:00"),
                Kind::Match,
                Some(1),
                50,
                "match 1",
            )
            .unwrap();
        let text = std::fs::read_to_string(log.path()).unwrap();
        std::fs::write(log.path(), text.trim_end_matches('\n')).unwrap();
        let second = log
            .append_at(
                when("2026-11-11T12:01:00+01:00"),
                Kind::Match,
                Some(1),
                50,
                "match 2",
            )
            .unwrap();
        assert_eq!(second.seq, 2);
        assert_eq!(second.prev, hash(&first.line()));
        let folded = fold_at(&log.lines().unwrap(), this_week());
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.seq, 2, "the whole line stayed one line");
        assert_eq!(folded.state.match_week, 100);
    }

    /// (finding 5) a line the fold cannot read inside the log refuses the append whole, and the file stays as it was.
    #[test]
    fn append_refuses_a_log_it_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        let mut text = fixed().text;
        let start = text[..text.find("2026-11-11T12:03").unwrap()]
            .rfind('\n')
            .unwrap()
            + 1;
        text.insert_str(start, "not a line\n");
        std::fs::write(log.path(), &text).unwrap();
        let before = std::fs::read_to_string(log.path()).unwrap();
        let e = log
            .append_at(
                when("2026-11-11T12:08:00+01:00"),
                Kind::Match,
                Some(9),
                50,
                "match 9",
            )
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("does not parse"), "{e}");
        assert_eq!(
            std::fs::read_to_string(log.path()).unwrap(),
            before,
            "the log is unchanged"
        );
    }

    /// (finding 6) a reader takes the log's shared lock: it waits out a write in progress (a bare read fails on Windows
    /// with a lock violation) and its time box then reports `WouldBlock`.
    #[test]
    fn lines_waits_for_the_writer_lock() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.append(Kind::Match, Some(1), 50, "first").unwrap();
        let text = log.lines().unwrap();
        let holder = OpenOptions::new()
            .read(true)
            .write(true)
            .open(log.path())
            .unwrap();
        holder.lock().unwrap();
        let reader = log.clone();
        let read = std::thread::spawn(move || {
            let start = Instant::now();
            let kind = reader.lines().unwrap_err().kind();
            (kind, start.elapsed())
        });
        let (kind, waited) = read.join().unwrap();
        assert_eq!(kind, io::ErrorKind::WouldBlock);
        assert!(waited >= LOCK_WAIT, "{waited:?}");
        holder.unlock().unwrap();
        assert_eq!(log.lines().unwrap(), text);
    }

    /// (finding 7, step 2.1) a pack's spend line and its 3 card lines land under one lock and one write, and the
    /// closure's refusal leaves the log untouched.
    #[test]
    fn a_pack_writes_its_lines_under_one_write() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.append_at(
            when("2026-11-11T12:00:00+01:00"),
            Kind::Match,
            Some(1),
            50,
            "match 1",
        )
        .unwrap();
        let before = std::fs::read_to_string(log.path()).unwrap();
        let line = |card: Option<u32>| Line {
            kind: Kind::Pack,
            card,
            tokens: if card.is_none() { -STANDARD_PACK } else { 0 },
            why: "pack standard".to_string(),
        };
        let buy = |state: &State| {
            if state.tokens < STANDARD_PACK {
                return Err(io::Error::other("not enough tokens"));
            }
            let mut out = vec![line(None)];
            out.extend([2, 3, 4].map(|card| line(Some(card))));
            Ok(out)
        };
        let e = log
            .append_lines_at(when("2026-11-11T12:01:00+01:00"), buy)
            .unwrap_err();
        assert_eq!(e.to_string(), "not enough tokens");
        assert_eq!(
            std::fs::read_to_string(log.path()).unwrap(),
            before,
            "50 tokens pay for nothing"
        );
        log.append_at(
            when("2026-11-11T12:02:00+01:00"),
            Kind::Convert,
            Some(5),
            70,
            "convert",
        )
        .unwrap();
        let wrote = log
            .append_lines_at(when("2026-11-11T12:03:00+01:00"), buy)
            .unwrap();
        assert_eq!(wrote.len(), 4);
        assert_eq!(
            wrote.iter().map(|e| e.seq).collect::<Vec<u64>>(),
            [3, 4, 5, 6],
            "one number per line, on from the fold under the lock"
        );
        assert_eq!(wrote[0].tokens, -STANDARD_PACK);
        assert_eq!(wrote[1].prev, hash(&wrote[0].line()));
        let folded = fold_at(&log.lines().unwrap(), this_week());
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.tokens, 50 + 70 - STANDARD_PACK);
        assert_eq!(
            folded.state.copies(2),
            Copies {
                earned: 1,
                ..Copies::default()
            }
        );
        assert_eq!(folded.state.copies(4).earned, 1);
    }

    /// (finding 10) a blank line at the end of the log does not become the next line's `prev`: the last non-blank line
    /// is hashed, as the fold reads it.
    #[test]
    fn append_hashes_the_last_non_blank_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        let first = log
            .append_at(
                when("2026-11-11T12:00:00+01:00"),
                Kind::Match,
                Some(1),
                50,
                "match 1",
            )
            .unwrap();
        let mut text = std::fs::read_to_string(log.path()).unwrap();
        text.push('\n');
        std::fs::write(log.path(), text).unwrap();
        let second = log
            .append_at(
                when("2026-11-11T12:01:00+01:00"),
                Kind::Match,
                Some(1),
                50,
                "match 2",
            )
            .unwrap();
        assert_eq!(second.seq, 2);
        assert_eq!(second.prev, hash(&first.line()));
        let folded = fold_at(&log.lines().unwrap(), this_week());
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.match_week, 100);
    }

    /// 1.4 (a), (b): the first start writes 12 `starter` lines of the choice and 1 `welcome` under the release rules,
    /// then the mark; a second start writes nothing; a new log (a reset) gets its starters again but no welcome.
    #[test]
    fn the_first_start_writes_once_and_the_welcome_once_per_installation() {
        let dir = tempfile::tempdir().unwrap();
        let mark = dir.path().join(WELCOME_MARK);
        let log = Log::new(dir.path().join("a"));
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        let at = when("2026-11-11T12:00:00+01:00");
        let cards: Vec<u32> = (101..=112).collect();
        let out = first_start(&log, &mark, at, &cards, "starter All seasons").unwrap();
        assert_eq!(out.len(), 13);
        let text = log.lines().unwrap();
        let entries: Vec<Entry> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let starters: Vec<&Entry> = entries.iter().filter(|e| e.kind == Kind::Starter).collect();
        assert_eq!(starters.len(), 12);
        assert!(
            starters
                .iter()
                .all(|e| e.why == "starter All seasons" && e.tokens == 0)
        );
        assert_eq!(
            starters.iter().map(|e| e.card.unwrap()).collect::<Vec<_>>(),
            cards
        );
        let welcome: Vec<&Entry> = entries.iter().filter(|e| e.kind == Kind::Welcome).collect();
        assert_eq!((welcome.len(), welcome[0].tokens), (1, WELCOME));
        assert!(entries.iter().all(|e| e.rules == RULES));
        assert!(mark.is_file());
        let folded = fold_at(&text, this_week());
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(
            (folded.state.tokens, folded.state.copies(112).starter),
            (150, 1)
        );

        // (b) a second start: nothing written
        assert!(
            first_start(&log, &mark, at, &cards, "starter 2013")
                .unwrap()
                .is_empty()
        );
        assert_eq!(log.lines().unwrap(), text);

        // a reset's new log: the starters again, the welcome not
        let fresh = Log::new(dir.path().join("b"));
        std::fs::create_dir_all(dir.path().join("b")).unwrap();
        let out = first_start(&fresh, &mark, at, &cards, "starter 2013").unwrap();
        assert_eq!(out.len(), 12);
        assert!(out.iter().all(|e| e.kind == Kind::Starter));

        // the reset-after-crash path: starters in the log and the mark gone, so no second welcome
        std::fs::remove_file(&mark).unwrap();
        assert!(
            first_start(&log, &mark, at, &cards, "starter 2013")
                .unwrap()
                .is_empty()
        );
        assert!(!mark.exists());
    }
}
