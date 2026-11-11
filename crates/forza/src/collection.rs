//! Create-a-Team's collection: the grant log `grants.jsonl` and its fold into the cards and tokens the player holds,
//! the locked append, the economy table, the seeded draws and CONVERT. Pure code: the caller passes its own `Card` rows
//! (built from the catalogue), so the module needs no catalogue and the Linux server builds it ('--no-default-features').
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
use time::OffsetDateTime;

/// The economy table's version: every line states the table it was written under, and the market refuses an older one.
pub const RULES: u32 = 1;

/// The craft value of one card of each tier at TOTAL under 85.
const BASE: [u32; 5] = [40, 100, 400, 1_500, 2_500];
/// Every draw's odds per tier, in tenths of a percent: 68 %, 26 %, 5.5 %, 0.1 %, 0.4 % (6 % RARE or better).
const ODDS: [u32; 5] = [680, 260, 55, 1, 4];
/// The rating step: a card's TOTAL up to this value, and the step in percent (x1, x1.5, x2.5, x4).
const STEPS: [(u32, u32); 4] = [(84, 100), (89, 150), (94, 250), (u32::MAX, 400)];
/// Shares of a card's value in percent: CONVERT (exact), the market floor (up), server stock (exact), the ceiling (down).
pub const CONVERT_SHARE: u32 = 20;
pub const FLOOR_SHARE: u32 = 80;
pub const STOCK_SHARE: u32 = 90;
pub const CEILING_SHARE: u32 = 95;
/// A pack: its price in tokens and its cards. Every pack draws at match-card odds with the same guarantee.
pub const PACK_CARDS: usize = 3;
pub const STANDARD_PACK: i64 = 100;
pub const SEASON_PACK: i64 = 150;
pub const NATION_PACK: i64 = 150;
/// Match tokens: the first match of the business day, each later one, and the day's cap (40 + 6 x 15, the first 7).
pub const FIRST_MATCH: i64 = 40;
pub const LATER_MATCH: i64 = 15;
pub const DAILY_MATCH_CAP: i64 = 130;
/// The starter: 11 REGULAR cards of the picked season (one GK, 4 DF, 4 MF, 2 FW) and 3 face-up RARE cards.
pub const STARTER_DEAL: [(Position, usize); 4] = [
    (Position::Gk, 1),
    (Position::Df, 4),
    (Position::Mf, 4),
    (Position::Fw, 2),
];
pub const STARTER_RARES: usize = 3;

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
    /// The game's player record (catalogue `record_index`): the same number on every card of one player.
    pub player: u32,
    pub rarity: Rarity,
    /// The game's TOTAL, 60 to 100.
    pub total: u32,
    /// The season's first year (2001 for 2001-02).
    pub season: u16,
    /// The catalogue's `nationality_name`, as the nation list spells it.
    pub nation: &'a str,
    pub position: Position,
}

/// A card's craft value: its tier's base times its rating step. All 20 cells of the table are whole tokens.
#[must_use]
pub fn value(rarity: Rarity, total: u32) -> u32 {
    rarity.base() * step(total) / 100
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

/// What a match pays, given the match tokens already earned in the business day: the first 40, each later one 15,
/// nothing from the 8th on (the day's cap is 130).
#[must_use]
pub fn match_tokens(earned_today: i64) -> i64 {
    if earned_today >= DAILY_MATCH_CAP {
        0
    } else if earned_today <= 0 {
        FIRST_MATCH
    } else {
        LATER_MATCH
    }
}

/// The game's business day at `at` as `YYYYMMDD`: the day turns at 05:00 local and runs to the next 05:00, as the
/// control counts it (`forza::online::championship::date` has the rule).
#[must_use]
pub fn day(at: OffsetDateTime) -> u32 {
    crate::online::championship::date(at)
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

/// A pack's cards: `PACK_CARDS` at the match-card odds from the cards `pick` accepts, and when all of them came out
/// REGULAR the last is redrawn from SPECIAL or better at those tiers' relative odds.
pub fn pack<'a>(
    rng: &mut Rng,
    pool: &'a [Card<'a>],
    pick: impl Fn(&Card) -> bool + Copy,
) -> Vec<&'a Card<'a>> {
    let mut out: Vec<&Card> = (0..PACK_CARDS)
        .filter_map(|_| draw(rng, pool, pick, &Rarity::ALL))
        .collect();
    if out.len() == PACK_CARDS
        && out.iter().all(|c| c.rarity == Rarity::Regular)
        && let Some(better) = draw(rng, pool, pick, &Rarity::BETTER)
    {
        out[PACK_CARDS - 1] = better;
    }
    out
}

/// The starter deal's 11 cards of `season`: 1 GK, 4 DF, 4 MF and 2 FW, REGULAR only, one card per player.
pub fn starter<'a>(rng: &mut Rng, pool: &'a [Card<'a>], season: u16) -> Vec<&'a Card<'a>> {
    let regular = |c: &Card| c.rarity == Rarity::Regular && c.season == season;
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

/// The starter's 3 face-up RARE cards of `season`, three players: the first start picks one.
pub fn starter_rares<'a>(rng: &mut Rng, pool: &'a [Card<'a>], season: u16) -> Vec<&'a Card<'a>> {
    let mut out: Vec<&Card> = vec![];
    while out.len() < STARTER_RARES {
        let idx: Vec<usize> = pool
            .iter()
            .enumerate()
            .filter(|(_, c)| c.rarity == Rarity::Rare && c.season == season)
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

/// The log folded: the cards, the token balance and the day's match tokens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub cards: BTreeMap<u32, Copies>,
    pub tokens: i64,
    /// Match tokens earned in the business day the fold was asked about (the daily cap's counter).
    pub match_today: i64,
    /// The last line's number (0 for an empty log).
    pub seq: u64,
}

impl State {
    /// The copies of `card`, zero when it is not in the log.
    #[must_use]
    pub fn copies(&self, card: u32) -> Copies {
        self.cards.get(&card).copied().unwrap_or_default()
    }

    /// What the next match pays now: [`match_tokens`] at today's counter.
    #[must_use]
    pub fn match_pays(&self) -> i64 {
        match_tokens(self.match_today)
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

/// Fold a log into the state, counting the day's match tokens against `today` ([`day`]). A line that does not parse
/// ends the fold, a `seq` that is not one more than the line before it and a `prev` that does not match that line are
/// warnings, and the lines around them are kept.
#[must_use]
pub fn fold_at(lines: &str, today: u32) -> Folded {
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
        out.state.apply(&entry, today);
        before = Some(line);
    }
    out
}

impl State {
    fn apply(&mut self, entry: &Entry, today: u32) {
        self.seq = entry.seq;
        self.tokens += entry.tokens;
        if entry.kind == Kind::Match && day(entry.at) == today {
            self.match_today += entry.tokens;
        }
        if let Some(card) = entry.card {
            let copies = self.cards.entry(card).or_default();
            match entry.kind {
                Kind::Starter => copies.starter += 1,
                Kind::Import => copies.imported += 1,
                Kind::Match | Kind::Pack | Kind::Craft | Kind::MarketBuy => copies.earned += 1,
                Kind::Convert => copies.earned = copies.earned.saturating_sub(1),
                // A sale is the sale of a listed copy: it leaves the collection and the market at once.
                Kind::MarketSell => {
                    copies.earned = copies.earned.saturating_sub(1);
                    copies.listed = copies.listed.saturating_sub(1);
                }
                Kind::List => copies.listed += 1,
                Kind::Unlist => copies.listed = copies.listed.saturating_sub(1),
            }
        }
    }
}

/// Fold a log into the state for the business day now.
#[must_use]
pub fn fold(lines: &str) -> Folded {
    fold_at(lines, day(now()))
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
        let folded = fold_at(&text, day(at));
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
pub fn convert(copies: &Copies, rarity: Rarity, total: u32) -> Result<u32, NoConvert> {
    if copies.earned == 0 {
        return Err(NoConvert::NoEarned);
    }
    if copies.earned.saturating_sub(copies.listed) < 2 {
        return Err(NoConvert::LastEarned);
    }
    Ok(value(rarity, total) * CONVERT_SHARE / 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;

    fn when(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &Rfc3339).unwrap()
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

    /// A fixed log of 9 lines, all on one business day except the last two.
    fn fixed() -> Fixture {
        let mut f = Fixture::default();
        f.add("2026-11-11T12:00:00+01:00", Kind::Starter, Some(1), 0)
            .add("2026-11-11T12:00:00+01:00", Kind::Starter, Some(2), 0)
            .add("2026-11-11T12:01:00+01:00", Kind::Match, Some(3), 40)
            .add("2026-11-11T12:02:00+01:00", Kind::Match, Some(3), 15)
            .add(
                "2026-11-11T12:03:00+01:00",
                Kind::Pack,
                None,
                -STANDARD_PACK,
            )
            .add("2026-11-11T12:03:00+01:00", Kind::Pack, Some(4), 0)
            .add("2026-11-11T12:04:00+01:00", Kind::Convert, Some(3), 30)
            .add("2026-11-11T12:05:00+01:00", Kind::Import, Some(1), 0)
            .add("2026-11-11T12:06:00+01:00", Kind::List, Some(2), 0)
            .add("2026-11-11T12:07:00+01:00", Kind::Unlist, Some(2), 0);
        f
    }

    /// (a) the fold's copies per source, the balance and the day's match tokens; a torn last line dropped.
    #[test]
    fn fold_counts_copies_and_drops_a_torn_last_line() {
        let f = fixed();
        let folded = fold_at(&f.text, 2026_1111);
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
        assert_eq!(s.tokens, 40 + 15 - STANDARD_PACK + 30);
        assert_eq!(s.match_today, 55);
        assert!(s.copies(3).owned());

        // A torn last line (line 10): dropped with a warning, the 9 lines before it kept.
        let mut torn = f.text.clone();
        torn.truncate(torn[..torn.len() - 1].rfind('\n').unwrap() + 1);
        torn.push_str("{\"seq\": 99, \"at\": \"2026-11-11T0");
        let folded = fold_at(&torn, 2026_1111);
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

    /// (a) a line whose `prev` is not the line before it: the chain reports it, the fold goes on.
    #[test]
    fn fold_reports_a_broken_chain() {
        let f = fixed();
        let broken = f.text.replacen("\"match\"", "\"craft\"", 1);
        let folded = fold_at(&broken, 2026_1111);
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
        f.add("2026-11-11T12:00:00+01:00", Kind::Match, Some(1), 40);
        f.seq += 1; // the next line claims 3
        f.add("2026-11-11T12:01:00+01:00", Kind::Match, Some(1), 15);
        let folded = fold_at(&f.text, 2026_1111);
        assert_eq!(folded.warnings.len(), 1, "{:?}", folded.warnings);
        assert!(
            folded.warnings[0].starts_with("line 2: seq is 3"),
            "{:?}",
            folded.warnings
        );
        assert!(!folded.unreadable);
        assert_eq!(folded.state.seq, 3, "the fold goes on");
        assert_eq!(folded.state.tokens, 55);
    }

    /// (b) the value table's 20 cells at the rating steps' edges, and a 150 card's floor, stock and ceiling.
    #[test]
    fn value_table() {
        // TOTAL 84 x1, 85 and 89 x1.5, 90 and 94 x2.5, 95 x4: the table's 20 cells at their edges.
        for (total, expected) in [
            (84, [40, 100, 400, 1_500, 2_500]),
            (85, [60, 150, 600, 2_250, 3_750]),
            (89, [60, 150, 600, 2_250, 3_750]),
            (90, [100, 250, 1_000, 3_750, 6_250]),
            (94, [100, 250, 1_000, 3_750, 6_250]),
            (95, [160, 400, 1_600, 6_000, 10_000]),
        ] {
            for (rarity, want) in Rarity::ALL.into_iter().zip(expected) {
                assert_eq!(value(rarity, total), want, "{rarity:?} at TOTAL {total}");
            }
        }
        assert_eq!(step(84), 100);
        assert_eq!(step(85), 150);
        assert_eq!(step(89), 150);
        assert_eq!(step(90), 250);
        assert_eq!(step(94), 250);
        assert_eq!(step(95), 400);
        assert_eq!(step(100), 400);
        let special87 = value(Rarity::Special, 87);
        assert_eq!(special87, 150);
        assert_eq!(floor_value(special87), 120);
        assert_eq!(stock_value(special87), 135);
        assert_eq!(ceiling_value(special87), 142);
        // The one line that needs rounding: 95 % of 142.5, and the narrowest band of all (a white 40: 32-38).
        assert_eq!(ceiling_value(150), 142);
        assert_eq!(floor_value(40), 32);
        assert_eq!(ceiling_value(40), 38);
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

    /// (c) 100,000 seeded draws keep the table's odds from the whole pool and from one small season.
    #[test]
    fn draws_keep_the_odds() {
        let odds = [68.0, 26.0, 5.5, 0.1, 0.4];
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
                    position: Position::Mf,
                })
            })
            .collect();
        let got = shares(|rng| match_card(rng, &whole).unwrap(), 100_000, 0x0005_EED1);
        for (tier, (got, want)) in got.iter().zip(odds).enumerate() {
            assert!(
                (got - want).abs() < 1.0,
                "{:?} {got} %, want {want} %",
                Rarity::ALL[tier]
            );
        }
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

    /// (c) a nation with no RARE, LEGEND or ALL TIME LEGEND: those rolls step down to SPECIAL, 68 / 32.
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

    /// The starter deal: 11 REGULAR cards of the season by position, no card twice; 3 distinct RARE cards.
    #[test]
    fn starter_deal_by_position() {
        let mut pool: Vec<Card> = vec![];
        for (i, position) in [Position::Gk, Position::Df, Position::Mf, Position::Fw]
            .into_iter()
            .enumerate()
        {
            for n in 0..6 {
                pool.push(Card {
                    card_no: (i * 6 + n) as u32,
                    player: (i * 6 + n) as u32,
                    rarity: Rarity::Regular,
                    total: 80,
                    season: 2004,
                    nation: "Italy",
                    position,
                });
            }
            for n in 0..4 {
                pool.push(Card {
                    card_no: (100 + i * 4 + n) as u32,
                    player: (100 + i * 4 + n) as u32,
                    rarity: Rarity::Rare,
                    total: 90,
                    season: 2004,
                    nation: "Italy",
                    position,
                });
            }
            pool.push(Card {
                card_no: (200 + i) as u32,
                player: (200 + i) as u32,
                rarity: Rarity::Rare,
                total: 90,
                season: 2017,
                nation: "Italy",
                position,
            });
        }
        let mut rng = Rng::new(11);
        let eleven = starter(&mut rng, &pool, 2004);
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
        let rares = starter_rares(&mut rng, &pool, 2004);
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
                    season: 2004,
                    nation: "Italy",
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
            let eleven = starter(&mut rng, &pool, 2004);
            assert_eq!(eleven.len(), 11);
            let mut players: Vec<u32> = eleven.iter().map(|c| c.player).collect();
            players.sort_unstable();
            players.dedup();
            assert_eq!(players.len(), 11, "one card per player");
        }
        for _ in 0..100 {
            let rares = starter_rares(&mut rng, &pool, 2004);
            assert_eq!(rares.len(), 3);
            let mut players: Vec<u32> = rares.iter().map(|c| c.player).collect();
            players.sort_unstable();
            players.dedup();
            assert_eq!(players.len(), 3, "three players face up");
        }
    }

    /// (d) match tokens: the first 40, then 15, nothing from the 8th; the day turns at 05:00; a convert adds nothing.
    #[test]
    fn match_tokens_and_the_day() {
        assert_eq!(match_tokens(0), 40);
        assert_eq!(match_tokens(40), 15);
        assert_eq!(match_tokens(40 + 5 * 15), 15);
        assert_eq!(match_tokens(DAILY_MATCH_CAP), 0);
        assert_eq!(match_tokens(200), 0);

        assert_eq!(day(when("2026-11-11T04:59:59+01:00")), 2026_1110);
        assert_eq!(day(when("2026-11-11T05:00:00+01:00")), 2026_1111);
        assert_eq!(day(when("2026-11-12T02:00:00+01:00")), 2026_1111);

        // Seven matches from 05:00 on: 130 today. A convert line and the match before 05:00 count for nothing.
        let mut f = Fixture::default();
        f.add("2026-11-11T04:00:00+01:00", Kind::Match, Some(1), 40);
        f.add("2026-11-11T05:00:00+01:00", Kind::Match, Some(1), 40);
        for n in 0..6 {
            f.add("2026-11-11T09:00:00+01:00", Kind::Match, Some(1), 15);
            let _ = n;
        }
        f.add("2026-11-11T09:30:00+01:00", Kind::Convert, Some(1), 8);
        let state = fold_at(&f.text, 2026_1111).state;
        assert_eq!(state.match_today, 130);
        assert_eq!(state.match_pays(), 0, "the 8th match adds nothing");
        assert_eq!(state.tokens, 40 + 130 + 8, "the balance counts every line");
        // The 04:00 match belongs to the business day before: read as 10 November, only its 40 counts.
        assert_eq!(fold_at(&f.text, 2026_1110).state.match_today, 40);
    }

    /// (e) CONVERT: a spare pays 20 %, the last earned copy and a starter or imported copy never do.
    #[test]
    fn convert_takes_a_spare_earned_copy_only() {
        let spare = Copies {
            earned: 2,
            ..Copies::default()
        };
        // A SPECIAL at TOTAL 87 (craft 150) converts for 30 and leaves 1 earned copy.
        assert_eq!(convert(&spare, Rarity::Special, 87), Ok(30));
        let mut state = State::default();
        state.cards.insert(7, spare);
        state.apply(
            &Entry {
                seq: 3,
                at: when("2026-11-11T12:00:00+01:00"),
                rules: RULES,
                kind: Kind::Convert,
                card: Some(7),
                tokens: 30,
                why: String::new(),
                prev: String::new(),
            },
            2026_1111,
        );
        assert_eq!(state.copies(7).earned, 1);

        assert_eq!(
            convert(
                &Copies {
                    earned: 1,
                    ..Copies::default()
                },
                Rarity::Special,
                87
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
                Rarity::Special,
                87
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
                Rarity::Special,
                87
            ),
            Err(NoConvert::LastEarned)
        );
        assert_eq!(
            convert(
                &Copies {
                    starter: 1,
                    ..Copies::default()
                },
                Rarity::Special,
                87
            ),
            Err(NoConvert::NoEarned)
        );
        assert_eq!(
            convert(
                &Copies {
                    imported: 1,
                    ..Copies::default()
                },
                Rarity::Special,
                87
            ),
            Err(NoConvert::NoEarned)
        );
        assert_eq!(
            convert(&Copies::default(), Rarity::Special, 87),
            Err(NoConvert::NoEarned)
        );
        // A spare top ALL TIME LEGEND pays 2,000.
        assert_eq!(convert(&spare, Rarity::AllTimeLegend, 95), Ok(2_000));
        // One of the two earned copies is out on the market: it is not spare, so nothing converts.
        assert_eq!(
            convert(
                &Copies {
                    earned: 2,
                    listed: 1,
                    ..Copies::default()
                },
                Rarity::Special,
                87
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
        assert_eq!(convert(&copies(2, 0), Rarity::Special, 87), Ok(30));
        assert_eq!(
            convert(&copies(2, 1), Rarity::Special, 87),
            Err(NoConvert::LastEarned),
            "2 earned with 1 listed leaves no unlisted spare"
        );
        assert_eq!(
            convert(&copies(2, 2), Rarity::Special, 87),
            Err(NoConvert::LastEarned)
        );
        assert_eq!(
            convert(&copies(3, 1), Rarity::Special, 87),
            Ok(30),
            "3 earned with 1 listed keeps one unlisted copy"
        );
    }

    /// (3.1) a `market_sell` line is the sale of a listed copy: it lowers `earned` and `listed` together, and a starter
    /// copy of the same card keeps it placeable.
    #[test]
    fn a_sale_takes_the_listed_copy() {
        let mut f = Fixture::default();
        f.add("2026-11-11T12:00:00+01:00", Kind::Starter, Some(2), 0)
            .add("2026-11-11T12:01:00+01:00", Kind::Match, Some(2), 40)
            .add("2026-11-11T12:02:00+01:00", Kind::List, Some(2), 0)
            .add("2026-11-11T12:03:00+01:00", Kind::MarketSell, Some(2), 120);
        let folded = fold_at(&f.text, 2026_1111);
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
        assert_eq!(folded.state.tokens, 160);
    }

    /// (e) two grants of one card fold to 2 earned copies and write no convert line.
    #[test]
    fn second_grant_is_two_earned_copies() {
        let mut f = Fixture::default();
        f.add("2026-11-11T12:00:00+01:00", Kind::Match, Some(42), 40)
            .add("2026-11-11T12:01:00+01:00", Kind::Match, Some(42), 15);
        assert!(!f.text.contains("\"convert\""));
        let state = fold_at(&f.text, 2026_1111).state;
        assert_eq!(state.copies(42).earned, 2);
        assert_eq!(state.copies(42).starter, 0);
        assert_eq!(state.tokens, 55);
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
                40,
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
        let folded = fold_at(&log.lines().unwrap(), 2026_1111);
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.seq, 2);
        assert_eq!(folded.state.tokens, 48);

        // A torn tail from an interrupted write: cut off, not glued to the new line.
        let mut file = OpenOptions::new().append(true).open(log.path()).unwrap();
        file.write_all(b"{\"seq\": 3, \"at\": \"2026-11").unwrap();
        drop(file);
        let third = log
            .append_at(
                when("2026-11-11T12:02:00+01:00"),
                Kind::Match,
                Some(2),
                15,
                "match 2",
            )
            .unwrap();
        assert_eq!(third.seq, 3, "the torn line never became an entry");
        let folded = fold_at(&log.lines().unwrap(), 2026_1111);
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.seq, 3);
    }

    /// A writer that holds the lock makes the append give up after its time box, and the grant lands after it.
    #[test]
    fn append_gives_up_after_the_lock_time_box() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.append(Kind::Match, Some(1), 40, "first").unwrap();
        let holder = OpenOptions::new()
            .read(true)
            .write(true)
            .open(log.path())
            .unwrap();
        holder.lock().unwrap();
        let start = Instant::now();
        let e = log.append(Kind::Match, Some(1), 15, "blocked").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::WouldBlock);
        assert!(start.elapsed() >= LOCK_WAIT, "{:?}", start.elapsed());
        holder.unlock().unwrap();
        let last = log.append(Kind::Match, Some(1), 15, "after").unwrap();
        assert_eq!(last.seq, 2);
        assert_eq!(
            fold_at(&log.lines().unwrap(), day(last.at))
                .state
                .match_today,
            55
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
                40,
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
                15,
                "match 2",
            )
            .unwrap();
        assert_eq!(second.seq, 2);
        assert_eq!(second.prev, hash(&first.line()));
        let folded = fold_at(&log.lines().unwrap(), 2026_1111);
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.seq, 2, "the whole line stayed one line");
        assert_eq!(folded.state.match_today, 55);
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
                15,
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
        log.append(Kind::Match, Some(1), 40, "first").unwrap();
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
            40,
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
            "40 tokens pay for nothing"
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
        let folded = fold_at(&log.lines().unwrap(), 2026_1111);
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.tokens, 40 + 70 - STANDARD_PACK);
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
                40,
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
                15,
                "match 2",
            )
            .unwrap();
        assert_eq!(second.seq, 2);
        assert_eq!(second.prev, hash(&first.line()));
        let folded = fold_at(&log.lines().unwrap(), 2026_1111);
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.match_today, 55);
    }
}
