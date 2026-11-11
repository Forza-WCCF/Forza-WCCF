//! Create-a-Team's market on the lobby's desk, steps 3.1 and 3.1b (PLAN.md 3.1, 3.4-3.6): the server's value table,
//! `market.db` beside `gamesvr.db`, the plausibility check of a player's grant log and the desk words `MARKET JOIN`,
//! `MARKET LEAVE` and `MARKET RESET` (their format: [`forza::market`]). Listings, purchases and server stock are later
//! steps.
//!
//! Trust first (PLAN.md 3.4): the check refuses a log that cannot come from an honest Launcher and says why in one
//! line; nothing is banned. The server keeps the last accepted log per key, so a log that drops accepted lines is a
//! fork; `MARKET RESET` (a profile reset) is the one allowed restart. No name is asked or stored. LEAVE deletes the log
//! at once (overwritten in the file, not only unlinked), returns the open listings and keeps the key's hash with its
//! counters for 7 days, longer while a sale waits for COLLECT.
//!
//! The value table (card number, tier, TOTAL, series, secret mark) is built on the server's host from an installed
//! 2017-18 `catalogue.tsv` by `forza-server market-table` ([`write_table`]) and never enters git. Without it every
//! `MARKET` word answers [`forza::market::NOT_SET_UP`].

use forza::collection::{self, Copies, Entry, Kind, RULES, Rarity};
use forza::market::{MAX_BODY, OLD, Request, Verb};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The tiers as the catalogue's `rarity_name` and the value table write them.
const TIERS: [(&str, Rarity); 5] = [
    ("REGULAR", Rarity::Regular),
    ("SPECIAL", Rarity::Special),
    ("RARE", Rarity::Rare),
    ("LEGEND", Rarity::Legend),
    ("ALL TIME LEGEND", Rarity::AllTimeLegend),
];

/// The one edition with a market: a 2010-11 catalogue numbers other cards with the same numbers.
const EDITION: &str = "2017-18";

fn tier(name: &str) -> Option<Rarity> {
    TIERS.iter().find(|(n, _)| *n == name).map(|(_, r)| *r)
}

fn tier_name(r: Rarity) -> &'static str {
    TIERS.iter().find(|(_, t)| *t == r).map_or("?", |(n, _)| n)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What the value of one card needs: [`collection::value`]'s inputs.
#[derive(Debug, Clone)]
struct Row {
    rarity: Rarity,
    total: u32,
    series: String,
    secret: bool,
}

/// The server's card values: each card's tier, TOTAL, series and secret mark, and what the table was built from.
#[derive(Debug, Default)]
pub struct Table {
    cards: HashMap<u32, Row>,
    /// The economy table it was built for ([`RULES`]).
    pub rules: u32,
    /// SHA-256 of the `catalogue.tsv` it was built from, hex.
    pub catalogue: String,
}

impl Table {
    /// A table [`write_table`] wrote: `edition`, `rules` and `catalogue` lines, then
    /// `card<TAB>tier<TAB>TOTAL<TAB>series<TAB>secret` lines; `#` comments. One built for another [`RULES`] or another
    /// edition is refused (rebuild it with this server's `market-table`).
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut t = Self::default();
        let mut edition = "";
        for (n, line) in text.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let bad = || {
                format!(
                    "{}: line {} is not a value table line",
                    path.display(),
                    n + 1
                )
            };
            match line.split('\t').collect::<Vec<_>>()[..] {
                ["edition", e] => edition = e,
                ["rules", r] => t.rules = r.parse().map_err(|_| bad())?,
                ["catalogue", h] => t.catalogue = h.to_owned(),
                [no, name, total, series, secret] => {
                    let no = no.parse().map_err(|_| bad())?;
                    let row = Row {
                        rarity: tier(name).ok_or_else(bad)?,
                        total: total.parse().map_err(|_| bad())?,
                        series: series.to_owned(),
                        secret: match secret {
                            "0" => false,
                            "1" => true,
                            _ => return Err(bad()),
                        },
                    };
                    t.cards.insert(no, row);
                }
                _ => return Err(bad()),
            }
        }
        if edition != EDITION {
            return Err(format!(
                "{}: not a {EDITION} table (edition {edition:?}): the market is {EDITION} only; run `forza-server market-table` with the {EDITION} catalogue",
                path.display()
            ));
        }
        if t.rules != RULES {
            return Err(format!(
                "{}: built for rules {}, this server's are {RULES}: run `forza-server market-table` again",
                path.display(),
                t.rules
            ));
        }
        if t.cards.is_empty() {
            return Err(format!("{}: no cards", path.display()));
        }
        Ok(t)
    }

    /// A table from cards, none of them secret or of a premium series (tests).
    #[must_use]
    pub fn of(cards: &[(u32, Rarity, u32)]) -> Self {
        Self {
            cards: cards
                .iter()
                .map(|&(c, rarity, total)| {
                    let series = String::new();
                    (
                        c,
                        Row {
                            rarity,
                            total,
                            series,
                            secret: false,
                        },
                    )
                })
                .collect(),
            rules: RULES,
            catalogue: String::new(),
        }
    }

    /// The card as [`collection`] prices it; the fields a value does not use are blank.
    fn card(&self, card: u32) -> Option<collection::Card<'_>> {
        self.cards.get(&card).map(|r| collection::Card {
            card_no: card,
            player: 0,
            rarity: r.rarity,
            total: r.total,
            series: &r.series,
            secret: r.secret,
            season: 0,
            nation: "",
            club: "",
            position: collection::Position::Gk,
        })
    }

    /// The card's value ([`collection::value`]).
    #[must_use]
    pub fn value(&self, card: u32) -> Option<u32> {
        self.card(card).map(|c| collection::value(&c))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.cards.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }
}

/// `forza-server market-table`: the value table at `out` from an installed 2017-18 `catalogue.tsv` (tab-separated, a
/// header naming the columns `card_no`, `rarity_name`, `stats_total`, `card_code`, and `is_placeholder` and
/// `image_archive_path` when present). The series and the secret mark follow `forza::cards::load_catalogue`: the card
/// code without its number (an older MVP card's mark from its picture), secret for every ALL TIME LEGEND and the
/// legend reprints. A catalogue without `card_code` is 2010-11's and refused. Placeholders and rows without a tier are
/// left out. Returns the number of cards written.
pub fn write_table(catalogue: &Path, out: &Path) -> Result<usize, String> {
    let bytes = std::fs::read(catalogue).map_err(|e| format!("{}: {e}", catalogue.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().unwrap_or_default().split('\t').collect();
    let col = |name: &str| head.iter().position(|h| h.trim() == name);
    let (Some(no), Some(rarity), Some(total)) =
        (col("card_no"), col("rarity_name"), col("stats_total"))
    else {
        return Err(format!(
            "{}: no card_no, rarity_name or stats_total column",
            catalogue.display()
        ));
    };
    let Some(code_col) = col("card_code") else {
        return Err(format!(
            "{}: no card_code column, so not a {EDITION} catalogue: the market is {EDITION} only",
            catalogue.display()
        ));
    };
    let (placeholder, picture) = (col("is_placeholder"), col("image_archive_path"));
    let mut cards = BTreeMap::new();
    for line in lines {
        let f: Vec<&str> = line.split('\t').collect();
        let get = |c: Option<usize>| c.and_then(|c| f.get(c)).map_or("", |s| s.trim());
        if get(placeholder) == "1" {
            continue;
        }
        let (Ok(n), Some(r), Ok(t)) = (
            get(Some(no)).parse::<u32>(),
            tier(get(Some(rarity))),
            get(Some(total)).parse::<u32>(),
        ) else {
            continue;
        };
        let code = Some(get(Some(code_col)))
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| forza::board::mvp_mark(get(picture)));
        let secret = r == Rarity::AllTimeLegend || forza::board::LEGEND_REPRINTS.contains(&n);
        if n != 0 {
            cards
                .entry(n)
                .or_insert((r, t, forza::board::series(code).to_owned(), secret));
        }
    }
    if cards.is_empty() {
        return Err(format!("{}: no cards", catalogue.display()));
    }
    let mut table = format!(
        "# forza-server market value table: card number, tier, TOTAL, series, secret. Built on this host from the installed catalogue; never commit it.\nedition\t{EDITION}\nrules\t{RULES}\ncatalogue\t{}\n",
        hex(&Sha256::digest(&bytes))
    );
    for (n, (r, t, series, secret)) in &cards {
        table.push_str(&format!(
            "{n}\t{}\t{t}\t{series}\t{}\n",
            tier_name(*r),
            u8::from(*secret)
        ));
    }
    std::fs::write(out, table).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(cards.len())
}

/// `match` lines this far apart at least (PLAN.md 3.5).
const MATCH_GAP: time::Duration = time::Duration::minutes(8);
/// Import lines only this long after the first `starter` line (PLAN.md 3.8).
const IMPORT_WINDOW: time::Duration = time::Duration::days(7);
/// The starter's copies: 11 REGULAR cards and the picked RARE.
const STARTER_CARDS: u32 = 12;
const PACK_PRICES: [i64; 3] = [
    collection::STANDARD_PACK,
    collection::SEASON_PACK,
    collection::NATION_PACK,
];

/// The week's match-token cap of a line written under `rules`; `None` for a table this server does not know.
fn match_cap(rules: u32) -> Option<i64> {
    match rules {
        2 => Some(collection::WEEK_CAP),
        _ => None,
    }
}

/// A log that passed [`check`].
#[derive(Debug)]
pub struct Checked<'a> {
    /// Its lines, blank ones and a torn last line left out.
    pub lines: Vec<&'a str>,
    /// Its `welcome` line, if any (one at most).
    pub welcome: Option<&'a str>,
}

/// The plausibility check (PLAN.md 3.4) under rules 2: the chain (`seq` and `prev` of every line), `match` lines 8
/// minutes apart, paying 50 at most and within each week's cap ([`collection::week`], the line's own offset, as the
/// Launcher counts it), one `welcome` of [`collection::WELCOME`], pack lines at a pack's price, crafts of white and
/// black cards only at the table's price, conversions at [`collection::CONVERT_SHARE`] and only while another unlisted
/// earned copy remains, no `convert` or `list` of a starter or imported copy, `import` lines only before the first
/// `list`, `market_buy` or `stock_buy` and within 7 days after the first `starter`, market lines inside the price
/// band, each `stock_buy` matching one of `stock` (this key's stock sales: card and price), and a balance never below
/// zero. An unparsable last line is left out (the Launcher stopped mid-append; its next append cuts it). Returns the
/// log's lines, or the first line that fails and why.
pub fn check<'a>(log: &'a str, table: &Table, stock: &[(u32, i64)]) -> Result<Checked<'a>, String> {
    let mut out = Checked {
        lines: Vec::new(),
        welcome: None,
    };
    let mut stock = stock.to_vec();
    let mut held: HashMap<u32, Copies> = HashMap::new();
    let mut balance: i64 = 0;
    let mut last_match: Option<time::OffsetDateTime> = None;
    let mut match_weeks: HashMap<u32, i64> = HashMap::new();
    let mut first_starter: Option<time::OffsetDateTime> = None;
    let mut starters = 0;
    let mut market_used = false;
    let mut pack_left = 0;
    for (n, raw) in log.lines().enumerate().map(|(n, l)| (n + 1, l)) {
        if raw.trim().is_empty() {
            continue;
        }
        let e: Entry = match serde_json::from_str(raw) {
            Ok(e) => e,
            Err(_) if log.lines().skip(n).all(|l| l.trim().is_empty()) => break,
            Err(e) => return Err(format!("line {n} does not parse ({e})")),
        };
        let no = |why: String| format!("line {n}: {why}");
        let lines = &out.lines;
        if e.seq != lines.len() as u64 + 1 {
            return Err(no(format!(
                "seq {} where {} belongs",
                e.seq,
                lines.len() + 1
            )));
        }
        if e.prev
            != lines
                .last()
                .map(|l| collection::hash(l))
                .unwrap_or_default()
        {
            return Err(no("prev is not the SHA-256 of the line before".into()));
        }
        let cap = match_cap(e.rules).ok_or_else(|| {
            no(format!(
                "rules {} is not this server's ({RULES}): the server needs an update",
                e.rules
            ))
        })?;
        // every card the log names is a card of the server's table
        let card = match e.card {
            Some(c) => Some((
                c,
                table
                    .card(c)
                    .ok_or_else(|| no(format!("card {c} is not in this server's table")))?,
            )),
            None => None,
        };
        let need = || card.ok_or_else(|| no(format!("a {:?} line without a card", e.kind)));
        let zero = |what: &str| {
            if e.tokens == 0 {
                Ok(())
            } else {
                Err(no(format!("{what} moves {} tokens", e.tokens)))
            }
        };
        let paid = |want: i64| {
            if e.tokens == want {
                Ok(())
            } else {
                Err(no(format!(
                    "{:?} of card {} moves {} tokens, the table says {want}",
                    e.kind,
                    e.card.unwrap_or_default(),
                    e.tokens
                )))
            }
        };
        let band = |v: u32| {
            i64::from(collection::floor_value(v))..=i64::from(collection::ceiling_value(v))
        };
        // a pack's cards follow its paid line directly (one append writes them)
        if !(e.kind == Kind::Pack && e.card.is_some()) {
            pack_left = 0;
        }
        match e.kind {
            Kind::Starter => {
                let (c, _) = need()?;
                zero("a starter card")?;
                let h = held.entry(c).or_default();
                if h.starter > 0 || starters >= STARTER_CARDS {
                    return Err(no(format!(
                        "more starter cards than the {STARTER_CARDS} of the deal"
                    )));
                }
                h.starter += 1;
                starters += 1;
                first_starter.get_or_insert(e.at);
            }
            Kind::Welcome => {
                if card.is_some() || e.tokens != collection::WELCOME {
                    return Err(no(format!(
                        "a welcome of {} tokens; it pays {} and no card",
                        e.tokens,
                        collection::WELCOME
                    )));
                }
                if out.welcome.is_some() {
                    return Err(no("a second welcome; it pays once per installation".into()));
                }
                out.welcome = Some(raw);
            }
            Kind::Match => {
                if !(0..=collection::MATCH_TOKENS).contains(&e.tokens) {
                    return Err(no(format!(
                        "a match for {} tokens; one pays 0 to {}",
                        e.tokens,
                        collection::MATCH_TOKENS
                    )));
                }
                if let Some(last) = last_match
                    && e.at - last < MATCH_GAP
                {
                    return Err(no(format!(
                        "a match {} s after the one before; matches are 8 minutes apart at least",
                        (e.at - last).whole_seconds()
                    )));
                }
                last_match = Some(e.at);
                let week = collection::week(e.at);
                let earned = match_weeks.entry(week).or_default();
                *earned = earned.saturating_add(e.tokens);
                if *earned > cap {
                    return Err(no(format!(
                        "{earned} match tokens in the week {week}, over its cap of {cap}"
                    )));
                }
                if let Some((c, _)) = card {
                    held.entry(c).or_default().earned += 1;
                }
            }
            Kind::Pack => match card {
                None => {
                    if !PACK_PRICES.iter().any(|p| e.tokens == -p) {
                        return Err(no(format!(
                            "a pack for {} tokens, no pack costs that",
                            -e.tokens
                        )));
                    }
                    pack_left = collection::PACK_CARDS;
                }
                Some((c, _)) => {
                    zero("a pack's card")?;
                    if pack_left == 0 {
                        return Err(no("a pack card without a paid pack before it".into()));
                    }
                    pack_left -= 1;
                    held.entry(c).or_default().earned += 1;
                }
            },
            Kind::Craft => {
                let (c, card) = need()?;
                let Some(price) = collection::craft(&card) else {
                    return Err(no(format!(
                        "a craft of the {} card {c}; the release crafts white and black only",
                        tier_name(card.rarity)
                    )));
                };
                paid(-i64::from(price))?;
                held.entry(c).or_default().earned += 1;
            }
            Kind::Convert => {
                let (c, card) = need()?;
                let h = held.entry(c).or_default();
                match collection::convert(h, &card) {
                    Ok(pays) => paid(i64::from(pays))?,
                    Err(collection::NoConvert::NoEarned) if h.starter + h.imported > 0 => {
                        return Err(no(format!(
                            "a convert of a starter or imported copy of card {c}"
                        )));
                    }
                    Err(collection::NoConvert::NoEarned) => {
                        return Err(no(format!("a convert of card {c}, which is not held")));
                    }
                    Err(collection::NoConvert::LastEarned) => {
                        return Err(no(format!(
                            "a convert of the last unlisted earned copy of card {c}"
                        )));
                    }
                }
                h.earned -= 1;
            }
            Kind::Import => {
                let (c, _) = need()?;
                zero("an import")?;
                if market_used {
                    return Err(no("an import after a market listing or purchase".into()));
                }
                let Some(start) = first_starter else {
                    return Err(no("an import before the starter".into()));
                };
                if !(start..=start + IMPORT_WINDOW).contains(&e.at) {
                    return Err(no("an import outside the 7 days after the starter".into()));
                }
                let h = held.entry(c).or_default();
                if h.imported > 0 {
                    return Err(no(format!("a second imported copy of card {c}")));
                }
                h.imported += 1;
            }
            Kind::List => {
                let (c, _) = need()?;
                zero("a listing")?;
                let h = held.entry(c).or_default();
                if h.earned <= h.listed {
                    return Err(no(if h.starter + h.imported > 0 {
                        format!("a listing of a starter or imported copy of card {c}")
                    } else {
                        format!("a listing of card {c}, which is not held")
                    }));
                }
                h.listed += 1;
                market_used = true;
            }
            Kind::Unlist => {
                let (c, _) = need()?;
                zero("a return from the market")?;
                let h = held.entry(c).or_default();
                if h.listed == 0 {
                    return Err(no(format!("a return of card {c}, which is not listed")));
                }
                h.listed -= 1;
            }
            Kind::MarketBuy => {
                let (c, card) = need()?;
                if !band(collection::value(&card)).contains(&-e.tokens) {
                    return Err(no(format!(
                        "a purchase of card {c} for {} tokens, outside its price band",
                        -e.tokens
                    )));
                }
                held.entry(c).or_default().earned += 1;
                market_used = true;
            }
            Kind::MarketSell => {
                let (c, card) = need()?;
                if !band(collection::value(&card)).contains(&e.tokens) {
                    return Err(no(format!(
                        "a sale of card {c} for {} tokens, outside its price band",
                        e.tokens
                    )));
                }
                let h = held.entry(c).or_default();
                if h.listed == 0 || h.earned == 0 {
                    return Err(no(format!("a sale of card {c}, which is not listed")));
                }
                h.listed -= 1;
                h.earned -= 1;
            }
            Kind::StockBuy => {
                let (c, _) = need()?;
                let Some(i) = stock.iter().position(|&s| s == (c, -e.tokens)) else {
                    return Err(no(format!(
                        "a stock purchase of card {c} for {} tokens the server never sold to this installation",
                        -e.tokens
                    )));
                };
                stock.swap_remove(i);
                held.entry(c).or_default().earned += 1;
                market_used = true;
            }
        }
        balance = balance
            .checked_add(e.tokens)
            .filter(|b| *b >= 0)
            .ok_or_else(|| no("the token balance goes below zero".into()))?;
        out.lines.push(raw);
    }
    Ok(out)
}

/// A key's requests this far from the server's clock are refused (a PC's clock a day off is broken, not a time zone).
const CLOCK_SLACK_MS: u64 = 24 * 3600 * 1000;
/// LEAVE keeps the key's hash and counters this long (longer while a sale waits for COLLECT).
const KEEP_LEFT_MS: u64 = 7 * 24 * 3600 * 1000;
/// Bodies read at once across the desk, and from one address: each may hold [`MAX_BODY`] in memory.
const MAX_UPLOADS: usize = 8;
const MAX_UPLOADS_PER_ADDRESS: usize = 2;

const SCHEMA: &str = "
-- one row per installation key, by the SHA-256 of the public key: LEAVE keeps the hash and the counters only
CREATE TABLE IF NOT EXISTS keys (
    key_hash   TEXT PRIMARY KEY,
    log        TEXT,                       -- the last accepted grant log; NULL after LEAVE and RESET
    last_at    INTEGER NOT NULL,           -- the last request's time (Unix ms): later requests only
    welcome    TEXT,                       -- SHA-256 of the accepted `welcome` line: once per key, kept by RESET
    listings   INTEGER NOT NULL DEFAULT 0, -- the listing counter (step 3.2), kept by RESET and LEAVE
    sales      INTEGER NOT NULL DEFAULT 0, -- the sales of the week `sales_week` (step 3.2), kept by RESET and LEAVE
    sales_week INTEGER NOT NULL DEFAULT 0,
    forks      INTEGER NOT NULL DEFAULT 0, -- logs refused for dropping accepted lines (for the operator)
    left_at    INTEGER                     -- LEAVE's time (Unix ms); the row goes 7 days later once no sale waits
);
-- listings, sales and stock purchases by key (steps 3.2, 3.3 fill it). state: open (listed), sold (sold, the
-- proceeds wait for the seller's COLLECT), paid (collected), stock (this key's stock-list purchase: one `stock_buy`
-- line matches it), stock_reset (one from before a RESET: the slot stays sold, no line matches it)
CREATE TABLE IF NOT EXISTS ledger (
    id       INTEGER PRIMARY KEY,
    key_hash TEXT    NOT NULL,
    card     INTEGER NOT NULL,
    price    INTEGER NOT NULL,
    state    TEXT    NOT NULL,
    at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ledger_key ON ledger (key_hash);
";

/// The market's server state: the value table, `market.db` and the uploads in flight per address.
pub struct Market {
    table: Table,
    db: Mutex<Connection>,
    uploads: Mutex<HashMap<String, usize>>,
}

impl std::fmt::Debug for Market {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Market({} cards, rules {})",
            self.table.len(),
            self.table.rules
        )
    }
}

/// The SHA-256 of a public key, hex: `market.db`'s name for an installation.
#[must_use]
pub fn key_hash(key: &[u8; 32]) -> String {
    hex(&Sha256::digest(key))
}

fn store_err(e: rusqlite::Error) -> String {
    format!("the market's store failed ({e})")
}

const NOT_IN: &str = "this installation is not in the market";

impl Market {
    /// The market with the value table at `table` and its store at `db` (made when missing).
    pub fn open(table: &Path, db: &Path) -> Result<Self, String> {
        Self::with(Table::load(table)?, db)
    }

    /// The market with `table` (tests build theirs) and its store at `db`.
    pub fn with(table: Table, db: &Path) -> Result<Self, String> {
        let open = || -> rusqlite::Result<Connection> {
            let c = Connection::open(db)?;
            // secure_delete: a deleted log is overwritten in the file, not left in free space (PLAN.md 3.4)
            c.execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000; PRAGMA secure_delete = ON;",
            )?;
            c.execute_batch(SCHEMA)?;
            Ok(c)
        };
        Ok(Self {
            table,
            db: Mutex::new(open().map_err(|e| format!("{}: {e}", db.display()))?),
            uploads: Mutex::new(HashMap::new()),
        })
    }

    #[must_use]
    pub fn table(&self) -> &Table {
        &self.table
    }

    /// One request at the server's time `now` (Unix ms): `Ok` with the words after `OK`, `Err` with the reason. The
    /// replay test comes before the log's check, and the store stays locked through both.
    pub fn handle(&self, req: &Request, now: u64) -> Result<String, String> {
        if req.at.abs_diff(now) > CLOCK_SLACK_MS {
            return Err("this PC's clock is more than a day off the server's".into());
        }
        if req.verb == Verb::Join && req.rules != self.table.rules {
            return Err(if req.rules < self.table.rules {
                "update first: this Launcher's market rules are older than the server's".into()
            } else {
                "this server's market rules are older than this Launcher's".into()
            });
        }
        let kh = key_hash(&req.key);
        // shortcut: one lock across the check (a big log takes some ms); per-key locks if the market gets busy
        let mut db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        // on its own: a refused request below rolls its transaction back, the expiry stays done
        let gone = db
            .execute(
                "DELETE FROM keys WHERE left_at <= ?1 AND NOT EXISTS
                 (SELECT 1 FROM ledger l WHERE l.key_hash = keys.key_hash AND l.state = 'sold')",
                params![now.saturating_sub(KEEP_LEFT_MS) as i64],
            )
            .map_err(store_err)?;
        if gone > 0 {
            db.execute(
                "DELETE FROM ledger WHERE NOT EXISTS (SELECT 1 FROM keys k WHERE k.key_hash = ledger.key_hash)",
                [],
            )
            .map_err(store_err)?;
        }
        let tx = db.transaction().map_err(store_err)?;
        type Stored = (i64, Option<String>, Option<i64>, Option<String>);
        let row: Option<Stored> = tx
            .query_row(
                "SELECT last_at, log, left_at, welcome FROM keys WHERE key_hash = ?1",
                params![kh],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(store_err)?;
        let at = i64::try_from(req.at).map_err(|_| OLD.to_string())?;
        if let Some((last, ..)) = &row
            && at <= *last
        {
            return Err(format!("{OLD}, the last at {last}"));
        }
        let joined = row.as_ref().is_some_and(|(_, _, left, _)| left.is_none());
        let answer = match req.verb {
            Verb::Join => {
                let stock: Vec<(u32, i64)> = tx
                    .prepare(
                        "SELECT card, price FROM ledger WHERE key_hash = ?1 AND state = 'stock'",
                    )
                    .and_then(|mut s| {
                        s.query_map(params![kh], |r| Ok((r.get(0)?, r.get(1)?)))?
                            .collect()
                    })
                    .map_err(store_err)?;
                let checked = check(&req.log, &self.table, &stock)?;
                let lines = &checked.lines;
                let old: Vec<&str> = row
                    .as_ref()
                    .and_then(|(_, log, ..)| log.as_deref())
                    .map(|l| l.lines().filter(|l| !l.is_empty()).collect())
                    .unwrap_or_default();
                let kept = old.iter().zip(lines).take_while(|(a, b)| a == b).count();
                if kept < old.len() {
                    tx.execute(
                        "UPDATE keys SET forks = forks + 1, last_at = ?2 WHERE key_hash = ?1",
                        params![kh, at],
                    )
                    .map_err(store_err)?;
                    tx.commit().map_err(store_err)?;
                    return Err(format!(
                        "a fork: this log drops {} of the {} lines the server accepted (a profile reset sends MARKET RESET first)",
                        old.len() - kept,
                        old.len()
                    ));
                }
                let welcome = checked.welcome.map(collection::hash);
                if let (Some(new), Some((.., Some(had)))) = (&welcome, &row)
                    && new != had
                {
                    return Err(
                        "a second welcome: this installation had its welcome tokens before".into(),
                    );
                }
                let mut log = lines.join("\n");
                log.push('\n');
                tx.execute(
                    "INSERT INTO keys (key_hash, log, last_at, welcome) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (key_hash) DO UPDATE SET log = ?2, last_at = ?3, left_at = NULL,
                     welcome = COALESCE(welcome, ?4)",
                    params![kh, log, at, welcome],
                )
                .map_err(store_err)?;
                format!("{} lines", lines.len())
            }
            Verb::Reset => {
                if !joined {
                    return Err(NOT_IN.into());
                }
                // the listings close and the waiting proceeds go with the log; the day's stock slots stay sold
                tx.execute(
                    "UPDATE keys SET log = NULL, last_at = ?2 WHERE key_hash = ?1",
                    params![kh, at],
                )
                .map_err(store_err)?;
                for sql in [
                    "DELETE FROM ledger WHERE key_hash = ?1 AND state IN ('open', 'sold', 'paid')",
                    "UPDATE ledger SET state = 'stock_reset' WHERE key_hash = ?1 AND state = 'stock'",
                ] {
                    tx.execute(sql, params![kh]).map_err(store_err)?;
                }
                "reset".into()
            }
            Verb::Leave => {
                if !joined {
                    return Err(NOT_IN.into());
                }
                tx.execute(
                    "UPDATE keys SET log = NULL, last_at = ?2, left_at = ?3 WHERE key_hash = ?1",
                    params![kh, at, now as i64],
                )
                .map_err(store_err)?;
                // the open listings come back now (the Launcher writes their `unlist` lines); a sale waiting for
                // COLLECT keeps its row, and the key's, until it is paid
                let returned: Vec<u32> = tx
                    .prepare("SELECT card FROM ledger WHERE key_hash = ?1 AND state = 'open' ORDER BY id")
                    .and_then(|mut s| s.query_map(params![kh], |r| r.get(0))?.collect())
                    .map_err(store_err)?;
                tx.execute(
                    "DELETE FROM ledger WHERE key_hash = ?1 AND state IN ('open', 'paid')",
                    params![kh],
                )
                .map_err(store_err)?;
                let waiting: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM ledger WHERE key_hash = ?1 AND state = 'sold'",
                        params![kh],
                        |r| r.get(0),
                    )
                    .map_err(store_err)?;
                let mut said = "left".to_owned();
                if !returned.is_empty() {
                    said.push_str(" returned");
                    for c in returned {
                        said.push_str(&format!(" {c}"));
                    }
                }
                if waiting > 0 {
                    said.push_str(&format!(" waiting {waiting}"));
                }
                said
            }
        };
        tx.commit().map_err(store_err)?;
        if req.verb != Verb::Join {
            // the deleted log leaves the write-ahead log too, not only the table
            db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
                .map_err(store_err)?;
        }
        Ok(answer)
    }

    /// One `MARKET` request on the desk from `ip` (`words`: the line's words after `MARKET`; `rest`: what the desk
    /// read past the line): reads the body, answers `OK ...` or `NO why` on `conn`, and returns what the desk logs.
    pub fn serve(
        &self,
        words: &[&str],
        rest: Vec<u8>,
        conn: &mut TcpStream,
        now: u64,
        ip: &str,
    ) -> io::Result<String> {
        fn no(conn: &mut TcpStream, verb: &str, why: &str) -> io::Result<String> {
            conn.write_all(format!("NO {why}\n").as_bytes())?;
            Ok(format!("market {verb} refused - {why}"))
        }
        let verb = words.first().copied().unwrap_or_default();
        // refused from the line, the body unread: the desk's hang-up reads it off
        let (v, size, sig) = match Request::head(words) {
            Ok(h) => h,
            Err(why) => return no(conn, verb, &why),
        };
        let Some(_upload) = Upload::take(&self.uploads, ip) else {
            return no(conn, verb, "busy, try again in a minute");
        };
        let body = read_body(conn, rest, size)?;
        let req = match Request::open(v, &sig, &body) {
            Ok(r) => r,
            Err(why) => return no(conn, verb, &why),
        };
        drop(body);
        let who = key_hash(&req.key).get(..8).unwrap_or_default().to_owned();
        match self.handle(&req, now) {
            Ok(said) => {
                conn.write_all(format!("OK {said}\n").as_bytes())?;
                Ok(format!("market {verb} {who}: {said}"))
            }
            Err(why) => no(conn, &format!("{verb} {who}"), &why),
        }
    }
}

/// One body in flight from one address, counted against [`MAX_UPLOADS`] and [`MAX_UPLOADS_PER_ADDRESS`] until
/// dropped.
struct Upload<'a> {
    slots: &'a Mutex<HashMap<String, usize>>,
    ip: String,
}

impl<'a> Upload<'a> {
    fn take(slots: &'a Mutex<HashMap<String, usize>>, ip: &str) -> Option<Self> {
        let mut s = slots.lock().unwrap_or_else(PoisonError::into_inner);
        let here = s.get(ip).copied().unwrap_or(0);
        if here >= MAX_UPLOADS_PER_ADDRESS || s.values().sum::<usize>() >= MAX_UPLOADS {
            return None;
        }
        s.insert(ip.to_owned(), here + 1);
        Some(Self {
            slots,
            ip: ip.to_owned(),
        })
    }
}

impl Drop for Upload<'_> {
    fn drop(&mut self) {
        let mut s = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = s.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                s.remove(&self.ip);
            }
        }
    }
}

/// `size` bytes of body ([`MAX_BODY`] at most, checked by the caller): `data` (read past the line) and the rest from
/// `conn`, within a minute.
fn read_body(conn: &mut TcpStream, mut data: Vec<u8>, size: usize) -> io::Result<Vec<u8>> {
    let size = size.min(MAX_BODY);
    let end = Instant::now() + Duration::from_secs(60);
    conn.set_read_timeout(Some(Duration::from_secs(30)))?;
    data.truncate(size);
    let mut chunk = vec![0u8; 64 * 1024];
    while data.len() < size {
        if Instant::now() > end {
            return Err(io::Error::other("market upload too slow"));
        }
        let want = size.saturating_sub(data.len()).min(chunk.len());
        let buf = chunk.get_mut(..want).unwrap_or_default();
        match conn.read(buf)? {
            0 => return Err(io::Error::other("the PC hung up mid-request")),
            n => data.extend_from_slice(buf.get(..n).unwrap_or_default()),
        }
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    /// Cards 1-40 REGULAR (TOTAL 80, value 40), 41-50 SPECIAL (90: 400), 51-55 RARE (85: 600), 56 LEGEND (95: 6,000).
    fn table() -> Table {
        let mut cards = vec![];
        for c in 1..=40 {
            cards.push((c, Rarity::Regular, 80));
        }
        for c in 41..=50 {
            cards.push((c, Rarity::Special, 90));
        }
        for c in 51..=55 {
            cards.push((c, Rarity::Rare, 85));
        }
        cards.push((56, Rarity::Legend, 95));
        Table::of(&cards)
    }

    /// A grant log written line by line, as `collection::Log` chains it; `why` goes on each line added after it is set.
    #[derive(Default)]
    struct Log {
        text: String,
        last: String,
        seq: u64,
        why: String,
    }

    impl Log {
        fn add(
            &mut self,
            at: OffsetDateTime,
            kind: Kind,
            card: Option<u32>,
            tokens: i64,
        ) -> &mut Self {
            self.seq += 1;
            let line = Entry {
                seq: self.seq,
                at,
                rules: RULES,
                kind,
                card,
                tokens,
                why: self.why.clone(),
                prev: if self.seq == 1 {
                    String::new()
                } else {
                    collection::hash(&self.last)
                },
            }
            .line();
            self.text.push_str(&line);
            self.text.push('\n');
            self.last = line;
            self
        }
    }

    fn t(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &Rfc3339).unwrap()
    }

    fn mins(m: i64) -> time::Duration {
        time::Duration::minutes(m)
    }

    const START: &str = "2026-11-01T10:00:00+09:00";

    /// The first start at `at`: 11 REGULAR starters (1-11), the picked RARE (51) and the welcome: 13 lines, 150 tokens.
    fn started_at(at: &str) -> Log {
        let mut l = Log::default();
        for c in (1..=11).chain([51]) {
            l.add(t(at), Kind::Starter, Some(c), 0);
        }
        l.add(t(at), Kind::Welcome, None, collection::WELCOME);
        l
    }

    fn started() -> Log {
        started_at(START)
    }

    fn refused(log: &Log) -> String {
        check(&log.text, &table(), &[]).unwrap_err()
    }

    fn passes(log: &Log) -> usize {
        check(&log.text, &table(), &[]).unwrap().lines.len()
    }

    /// Acceptance 3.1 (b): 8 matches in 10 minutes are refused, with the reason.
    #[test]
    fn matches_closer_than_8_minutes_are_refused() {
        let mut l = started();
        let at = t("2026-11-01T12:00:00+09:00");
        for i in 0..8 {
            l.add(
                at + time::Duration::seconds(75 * i),
                Kind::Match,
                Some(20),
                50,
            );
        }
        let why = refused(&l);
        assert!(
            why.contains("line 15") && why.contains("8 minutes apart"),
            "{why}"
        );
    }

    /// `n` matches paying 50 each, 9 minutes apart from `first`, after the first start.
    fn matches(first: &str, n: i64) -> Log {
        let mut l = started();
        for i in 0..n {
            l.add(t(first) + mins(9 * i), Kind::Match, Some(20), 50);
        }
        l
    }

    /// Acceptance 3.1b (a): 22 paying matches in a week fail, 21 pass, and 21 on each side of the week's turn (Monday
    /// 05:00) pass twice; past the cap a match deals its card for nothing.
    #[test]
    fn match_tokens_over_the_weeks_cap_are_refused() {
        assert_eq!(passes(&matches("2026-11-03T10:00:00+09:00", 21)), 34);
        let why = refused(&matches("2026-11-03T10:00:00+09:00", 22));
        assert!(
            why.contains("line 35")
                && why.contains("1100 match tokens in the week 202645, over its cap of 1050"),
            "{why}"
        );
        let mut l = matches("2026-11-03T10:00:00+09:00", 21);
        l.add(t("2026-11-04T10:00:00+09:00"), Kind::Match, Some(21), 0);
        assert_eq!(passes(&l), 35);
        // Sunday night to 04:50 Monday, then from 05:00
        let mut l = matches("2026-11-09T01:50:00+09:00", 21);
        for i in 0..21 {
            l.add(
                t("2026-11-09T05:00:00+09:00") + mins(9 * i),
                Kind::Match,
                Some(21),
                50,
            );
        }
        assert_eq!(passes(&l), 55);
        // one line paying more than a match pays
        let mut l = started();
        l.add(t("2026-11-03T10:00:00+09:00"), Kind::Match, Some(20), 1_050);
        assert!(refused(&l).contains("one pays 0 to 50"));
    }

    /// N2: each line's week follows its own offset, as the Launcher counts it (normalizing would refuse an honest log
    /// across a DST change); hopping offsets still meets each week's cap, so it gains at most the week at either end.
    #[test]
    fn offset_hopping_still_caps_each_week() {
        let mut l = matches("2026-11-05T10:00:00+09:00", 21);
        // Sunday 16:00 UTC: Monday 06:00 at +14:00 (the new week), then 9 minutes later Sunday 04:09 at -12:00
        l.add(t("2026-11-09T06:00:00+14:00"), Kind::Match, Some(21), 50)
            .add(t("2026-11-08T04:09:00-12:00"), Kind::Match, Some(21), 50);
        let why = refused(&l);
        assert!(
            why.contains("line 36") && why.contains("week 202645"),
            "{why}"
        );
    }

    /// Acceptance 3.1 (b): an import after a listing is refused, with the reason; so is one outside the 7 days after
    /// the starter, before it in time (N3) or before any starter line.
    #[test]
    fn an_import_after_a_listing_is_refused() {
        let mut l = started();
        l.add(t("2026-11-01T12:00:00+09:00"), Kind::Match, Some(20), 40)
            .add(t("2026-11-01T12:01:00+09:00"), Kind::List, Some(20), 0)
            .add(t("2026-11-01T12:02:00+09:00"), Kind::Import, Some(30), 0);
        let why = refused(&l);
        assert!(why.contains("import after a market listing"), "{why}");
        for late in ["2026-11-09T10:00:01+09:00", "2026-11-01T09:59:59+09:00"] {
            let mut l = started();
            l.add(t(late), Kind::Import, Some(30), 0);
            assert!(
                refused(&l).contains("outside the 7 days after the starter"),
                "{late}"
            );
        }
        let mut l = started();
        l.add(t("2026-11-08T10:00:00+09:00"), Kind::Import, Some(30), 0);
        assert_eq!(passes(&l), 14);
        let mut l = Log::default();
        l.add(t(START), Kind::Import, Some(30), 0);
        assert!(refused(&l).contains("before the starter"));
    }

    /// Acceptance 3.1 (b): a convert or a listing of a starter or imported copy is refused, with the reason; a spare
    /// earned copy beside a starter copy is not a spare (the starter copy never makes an earned one convertible).
    #[test]
    fn a_starter_or_imported_copy_never_converts_or_lists() {
        let at = t("2026-11-01T12:00:00+09:00");
        let mut l = started();
        l.add(at, Kind::Convert, Some(1), 8);
        assert!(refused(&l).contains("convert of a starter or imported copy of card 1"));
        let mut l = started();
        l.add(at, Kind::Import, Some(30), 0)
            .add(at, Kind::Convert, Some(30), 8);
        assert!(refused(&l).contains("convert of a starter or imported copy of card 30"));
        let mut l = started();
        l.add(at, Kind::Import, Some(30), 0)
            .add(at, Kind::List, Some(30), 0);
        assert!(refused(&l).contains("listing of a starter or imported copy of card 30"));
        // 1 earned + 1 starter copy: the earned copy is the last earned one
        let mut l = started();
        l.add(at, Kind::Match, Some(1), 40)
            .add(at + mins(8), Kind::Convert, Some(1), 8);
        assert!(refused(&l).contains("last unlisted earned copy"));
    }

    #[test]
    fn prices_chain_and_balance_are_checked() {
        let at = t("2026-11-01T12:00:00+09:00");
        // a craft below the table's price
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 40)
            .add(at, Kind::Craft, Some(21), -30);
        assert!(refused(&l).contains("the table says -40"));
        // a pack for a price no pack has, and a pack card without a paid pack
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 40)
            .add(at, Kind::Pack, None, -10);
        assert!(refused(&l).contains("no pack costs that"));
        let mut l = started();
        l.add(at, Kind::Pack, Some(20), 0);
        assert!(refused(&l).contains("without a paid pack"));
        // a pack bought on credit: 150 + 40 < 200
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 40)
            .add(at, Kind::Pack, None, -200);
        assert!(refused(&l).contains("below zero"));
        // a card the server does not know, a broken chain, a skipped seq
        let mut l = started();
        l.add(at, Kind::Match, Some(999), 40);
        assert!(refused(&l).contains("card 999 is not in this server's table"));
        let l = started();
        let text = l.text.replacen("\"why\":\"\"", "\"why\":\"x\"", 1);
        assert!(
            check(&text, &table(), &[])
                .unwrap_err()
                .contains("line 2: prev")
        );
        let text: String = l
            .text
            .lines()
            .filter(|x| !x.contains("\"seq\":3,"))
            .map(|x| format!("{x}\n"))
            .collect();
        assert!(
            check(&text, &table(), &[])
                .unwrap_err()
                .contains("line 3: seq 4")
        );
        // a sale above the band
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 40)
            .add(at, Kind::List, Some(20), 0)
            .add(at, Kind::MarketSell, Some(20), 39);
        assert!(refused(&l).contains("outside its price band"));
        assert!(
            check(&format!("not json\n{}", l.text), &table(), &[])
                .unwrap_err()
                .contains("line 1 does not parse")
        );
    }

    /// M6: a torn last line (the Launcher stopped mid-append) is left out; a torn line before another is refused.
    #[test]
    fn a_torn_last_line_is_left_out() {
        let l = started();
        let half = l.text.get(..40).unwrap();
        for log in [format!("{}{half}", l.text), format!("{}{half}\n\n", l.text)] {
            let c = check(&log, &table(), &[]).unwrap();
            assert_eq!(c.lines.len(), 13);
        }
        let why = check(&format!("{half}\n{}", l.text), &table(), &[]).unwrap_err();
        assert!(why.starts_with("line 1 does not parse"), "{why}");
    }

    /// Acceptance 3.1b (b): a RARE craft, a `stock_buy` the server never sold (or sold at another price, or twice) and
    /// one beyond the balance are refused, each with the reason; a sold one at its price passes.
    #[test]
    fn a_rare_craft_and_unsold_stock_are_refused() {
        let at = t("2026-11-01T12:00:00+09:00");
        let mut l = started();
        l.add(at, Kind::Craft, Some(52), -600);
        assert!(
            refused(&l)
                .contains("a craft of the RARE card 52; the release crafts white and black only")
        );
        let mut l = started();
        l.add(at, Kind::Craft, Some(21), -40);
        assert_eq!(passes(&l), 14);
        let mut l = matches("2026-11-03T10:00:00+09:00", 5);
        l.add(
            t("2026-11-03T12:00:00+09:00"),
            Kind::StockBuy,
            Some(41),
            -360,
        );
        let never = "a stock purchase of card 41 for 360 tokens the server never sold";
        assert!(refused(&l).contains(never));
        assert!(
            check(&l.text, &table(), &[(41, 300)])
                .unwrap_err()
                .contains(never)
        );
        assert_eq!(
            check(&l.text, &table(), &[(41, 360)]).unwrap().lines.len(),
            19
        );
        l.add(
            t("2026-11-03T12:01:00+09:00"),
            Kind::StockBuy,
            Some(41),
            -360,
        );
        assert!(
            check(&l.text, &table(), &[(41, 360)])
                .unwrap_err()
                .contains(never)
        );
        let mut l = started();
        l.add(at, Kind::StockBuy, Some(56), -5_400);
        let why = check(&l.text, &table(), &[(56, 5_400)]).unwrap_err();
        assert!(why.contains("below zero"), "{why}");
    }

    /// Acceptance 3.1b (e), in one log: a second `welcome` line, or one of another amount, is refused.
    #[test]
    fn a_second_welcome_in_a_log_is_refused() {
        let mut l = started();
        l.add(
            t("2026-11-02T10:00:00+09:00"),
            Kind::Welcome,
            None,
            collection::WELCOME,
        );
        assert!(refused(&l).contains("line 14: a second welcome"));
        let mut l = Log::default();
        l.add(t(START), Kind::Welcome, None, 200);
        assert!(refused(&l).contains("a welcome of 200 tokens"));
    }

    /// The honest month's one stock purchase, as `market.db` holds it.
    const STOCK: [(u32, i64); 1] = [(45, 360)];

    /// An honest month: the deal and the welcome, an import on day 3, 8 matches every day paying up to the week's cap,
    /// Standard and Season packs, a black craft, kept copies, conversions of spares only, a listing returned, a sale, a
    /// purchase and a stock purchase.
    fn honest_month() -> String {
        let mut l = started();
        let day0 = t("2026-11-01T11:00:00+09:00");
        let mut balance = collection::WELCOME;
        let mut week = (0, 0);
        for d in 0..30 {
            let morning = day0 + time::Duration::days(d);
            if d == 2 {
                for c in [30, 31, 2] {
                    l.add(morning, Kind::Import, Some(c), 0);
                }
            }
            for m in 0..8 {
                let at = morning + mins(9 * m);
                if week.0 != collection::week(at) {
                    week = (collection::week(at), 0);
                }
                let pays = collection::match_tokens(week.1);
                week.1 += pays;
                balance += pays;
                let card = 12 + ((d * 8 + m) % 10) as u32; // REGULAR 12-21: copies pile up
                l.add(at, Kind::Match, Some(card), pays);
            }
            let evening = morning + time::Duration::hours(10);
            if d % 4 == 0 && balance >= 150 {
                l.add(evening, Kind::Pack, None, -150);
                for c in [41, 22, 23] {
                    l.add(evening, Kind::Pack, Some(c), 0);
                }
                balance -= 150;
            } else if d % 4 == 2 && balance >= 100 {
                l.add(evening, Kind::Pack, None, -100);
                for c in [24, 25, 26] {
                    l.add(evening, Kind::Pack, Some(c), 0);
                }
                balance -= 100;
            }
            // convert one spare of card 12 each week (8 earned per 10 days: spares exist)
            if d % 7 == 6 {
                l.add(evening + mins(1), Kind::Convert, Some(12), 8);
                balance += 8;
            }
            if d == 20 {
                l.add(evening + mins(2), Kind::Craft, Some(43), -400)
                    .add(evening + mins(3), Kind::List, Some(13), 0)
                    .add(evening + mins(4), Kind::Unlist, Some(13), 0)
                    .add(evening + mins(5), Kind::List, Some(14), 0)
                    .add(evening + mins(6), Kind::MarketSell, Some(14), 36)
                    .add(evening + mins(7), Kind::MarketBuy, Some(42), -360)
                    .add(evening + mins(8), Kind::StockBuy, Some(45), -360);
                balance += -400 + 36 - 360 - 360;
            }
            assert!(balance >= 0, "the fixture spends what it has: day {d}");
        }
        l.text
    }

    /// Acceptance 3.1 (c), 3.1b (c): an honest 1-month log under rules 2 with kept copies, conversions, an import and a
    /// stock purchase passes.
    #[test]
    fn an_honest_month_passes() {
        let text = honest_month();
        let checked = check(&text, &table(), &STOCK).unwrap();
        assert!(checked.lines.len() > 300, "{}", checked.lines.len());
        assert!(checked.welcome.is_some());
        let folded = collection::fold(&text);
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert!(folded.state.copies(12).earned >= 2, "kept copies");
        assert_eq!(folded.state.copies(30).imported, 1);
        assert_eq!(folded.state.copies(45).earned, 1);
    }

    fn market(dir: &Path) -> Market {
        Market::with(table(), &dir.join("market.db")).unwrap()
    }

    const NOW: u64 = 1_762_000_000_000;

    fn s(v: &str) -> Option<String> {
        Some(v.to_owned())
    }

    fn req(verb: Verb, at: u64, log: &str) -> Request {
        Request {
            verb,
            key: [9; 32],
            at,
            rules: RULES,
            log: log.into(),
        }
    }

    fn row(dir: &Path, sql: &str) -> Vec<Vec<Option<String>>> {
        let c = Connection::open(dir.join("market.db")).unwrap();
        let mut s = c.prepare(sql).unwrap();
        let n = s.column_count();
        s.query_map([], |r| {
            (0..n)
                .map(|i| r.get::<_, rusqlite::types::Value>(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .map(|v| {
            v.unwrap()
                .into_iter()
                .map(|x| match x {
                    rusqlite::types::Value::Null => None,
                    rusqlite::types::Value::Integer(i) => Some(i.to_string()),
                    rusqlite::types::Value::Text(t) => Some(t),
                    other => Some(format!("{other:?}")),
                })
                .collect()
        })
        .collect()
    }

    fn ledger(dir: &Path, rows: &[(u32, &str)]) {
        let c = Connection::open(dir.join("market.db")).unwrap();
        for (card, state) in rows {
            c.execute(
                "INSERT INTO ledger (key_hash, card, price, state, at) VALUES (?1, ?2, 36, ?3, 0)",
                params![key_hash(&[9; 32]), card, state],
            )
            .unwrap();
        }
    }

    /// Acceptance 3.1 (d): a second log that drops accepted lines is a fork; after MARKET RESET a new history for the
    /// same key is accepted and the sale counter stays.
    #[test]
    fn a_dropped_line_is_a_fork_until_a_reset() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        Connection::open(dir.path().join("market.db"))
            .unwrap()
            .execute(
                "INSERT INTO ledger (key_hash, card, price, state, at) VALUES (?1, 45, 360, 'stock', 0)",
                params![key_hash(&[9; 32])],
            )
            .unwrap();
        let month = honest_month();
        let first: String = month.lines().take(100).map(|l| format!("{l}\n")).collect();
        assert_eq!(
            m.handle(&req(Verb::Join, NOW, &first), NOW).unwrap(),
            "100 lines"
        );
        // the same history grown: accepted
        assert_eq!(
            m.handle(&req(Verb::Join, NOW + 1, &month), NOW).unwrap(),
            format!("{} lines", month.lines().count())
        );
        // the 3.2 counter as a sale would leave it
        Connection::open(dir.path().join("market.db"))
            .unwrap()
            .execute("UPDATE keys SET sales = 3, listings = 2", [])
            .unwrap();
        // a rewritten history: the first 13 lines kept, the rest another story
        let other = {
            let mut l = started();
            l.add(t("2026-11-01T12:00:00+09:00"), Kind::Match, Some(40), 40);
            l.text
        };
        let why = m
            .handle(&req(Verb::Join, NOW + 2, &other), NOW)
            .unwrap_err();
        assert!(why.starts_with("a fork: this log drops"), "{why}");
        assert_eq!(row(dir.path(), "SELECT forks FROM keys"), [[s("1")]]);
        // the same log after a reset is a new history
        assert_eq!(
            m.handle(&req(Verb::Reset, NOW + 3, ""), NOW).unwrap(),
            "reset"
        );
        assert_eq!(
            m.handle(&req(Verb::Join, NOW + 4, &other), NOW).unwrap(),
            "14 lines"
        );
        assert_eq!(
            row(dir.path(), "SELECT sales, listings FROM keys"),
            [[s("3"), s("2")]]
        );
        // a request sent again is refused, with the time to go past
        assert_eq!(
            m.handle(&req(Verb::Join, NOW + 4, &other), NOW)
                .unwrap_err(),
            format!("{OLD}, the last at {}", NOW + 4)
        );
    }

    /// M1: a PC clock 10 hours fast at JOIN, then set right: the refusal names the last time, and a request past it
    /// is taken (`forza::market::ask` signs it again).
    #[test]
    fn an_old_request_names_the_last_time() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let ahead = NOW + 10 * 3600 * 1000;
        m.handle(&req(Verb::Join, ahead, &started().text), NOW)
            .unwrap();
        let why = m.handle(&req(Verb::Leave, NOW, ""), NOW).unwrap_err();
        assert_eq!(forza::market::last_at(&why), Some(ahead), "{why}");
        assert_eq!(
            m.handle(&req(Verb::Leave, ahead + 1, ""), NOW).unwrap(),
            "left"
        );
    }

    /// N5: a request sent again is refused as old before its log is checked.
    #[test]
    fn a_replay_is_refused_before_its_log_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        m.handle(&req(Verb::Join, NOW, &started().text), NOW)
            .unwrap();
        let why = m
            .handle(&req(Verb::Join, NOW, "not json\n{}\n"), NOW)
            .unwrap_err();
        assert!(why.starts_with(OLD), "{why}");
    }

    /// Acceptance 3.1 (e), M3, M4: RESET drops the open listings and every sale of the old history, keeping the day's
    /// stock slots sold; LEAVE deletes the log, returns the open listings by card, keeps a sale waiting for COLLECT
    /// and only the key's hash with its counters, for 7 days and until no sale waits.
    #[test]
    fn leave_returns_listings_and_keeps_only_the_key_hash_and_its_counters() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        m.handle(&req(Verb::Join, NOW, &started().text), NOW)
            .unwrap();
        let c = Connection::open(dir.path().join("market.db")).unwrap();
        c.execute("UPDATE keys SET sales = 4, listings = 1", [])
            .unwrap();
        let kh = key_hash(&[9; 32]);
        ledger(
            dir.path(),
            &[(5, "open"), (6, "sold"), (7, "paid"), (8, "stock")],
        );
        m.handle(&req(Verb::Reset, NOW + 1, ""), NOW).unwrap();
        assert_eq!(
            row(dir.path(), "SELECT card, state FROM ledger"),
            [[s("8"), s("stock_reset")]]
        );
        m.handle(&req(Verb::Join, NOW + 2, &started().text), NOW)
            .unwrap();
        ledger(
            dir.path(),
            &[(5, "open"), (7, "open"), (6, "sold"), (9, "paid")],
        );
        assert_eq!(
            m.handle(&req(Verb::Leave, NOW + 3, ""), NOW).unwrap(),
            "left returned 5 7 waiting 1"
        );
        assert_eq!(
            row(
                dir.path(),
                "SELECT key_hash, log, sales, listings FROM keys"
            ),
            [[s(&kh), None, s("4"), s("1")]]
        );
        assert_eq!(
            row(dir.path(), "SELECT card, state FROM ledger ORDER BY card"),
            [[s("6"), s("sold")], [s("8"), s("stock_reset")]]
        );
        assert_eq!(
            m.handle(&req(Verb::Leave, NOW + 4, ""), NOW).unwrap_err(),
            NOT_IN
        );
        // back within 7 days: the counters are still there
        m.handle(&req(Verb::Join, NOW + 5, &started().text), NOW + 5)
            .unwrap();
        assert_eq!(row(dir.path(), "SELECT sales FROM keys"), [[s("4")]]);
        assert_eq!(
            m.handle(&req(Verb::Leave, NOW + 6, ""), NOW + 6).unwrap(),
            "left waiting 1"
        );
        // 7 days later the waiting sale keeps the key; once paid (3.2's COLLECT) the key and its rows go
        let later = NOW + 6 + KEEP_LEFT_MS;
        assert_eq!(
            m.handle(&req(Verb::Reset, later, ""), later).unwrap_err(),
            NOT_IN
        );
        assert_eq!(row(dir.path(), "SELECT COUNT(*) FROM keys"), [[s("1")]]);
        c.execute("UPDATE ledger SET state = 'paid' WHERE state = 'sold'", [])
            .unwrap();
        assert_eq!(
            m.handle(&req(Verb::Reset, later + 1, ""), later + 1)
                .unwrap_err(),
            NOT_IN
        );
        assert!(row(dir.path(), "SELECT * FROM keys").is_empty());
        assert!(row(dir.path(), "SELECT * FROM ledger").is_empty());
    }

    /// M2: after LEAVE the log's bytes are in neither `market.db` nor its write-ahead log.
    #[test]
    fn leave_leaves_no_trace_of_the_log_in_the_files() {
        const MARK: &[u8] = b"log-mark-7c1e";
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let mut l = started();
        // past a page: the log's tail sits in overflow pages, which a delete frees but does not clear by itself
        l.why = "p".repeat(400);
        for i in 0..10 {
            l.add(
                t("2026-11-01T12:00:00+09:00") + mins(9 * i),
                Kind::Match,
                Some(20),
                50,
            );
        }
        l.why = String::from_utf8(MARK.to_vec()).unwrap();
        l.add(t("2026-11-01T14:00:00+09:00"), Kind::Match, Some(20), 50);
        let found = || {
            ["market.db", "market.db-wal"].iter().any(|f| {
                std::fs::read(dir.path().join(f))
                    .unwrap_or_default()
                    .windows(MARK.len())
                    .any(|w| w == MARK)
            })
        };
        m.handle(&req(Verb::Join, NOW, &l.text), NOW).unwrap();
        assert!(found(), "the stored log is on disk before LEAVE");
        m.handle(&req(Verb::Leave, NOW + 1, ""), NOW).unwrap();
        assert!(!found(), "the log's bytes stay on disk after LEAVE");
    }

    /// Acceptance 3.1b (e): a JOIN carries no name and `market.db` has no name column; a second welcome after a
    /// RESET is refused, the first one's log again is not.
    #[test]
    fn a_second_welcome_after_a_reset_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        m.handle(&req(Verb::Join, NOW, &started().text), NOW)
            .unwrap();
        assert!(
            row(
                dir.path(),
                "SELECT 1 FROM pragma_table_info('keys') WHERE name = 'name'"
            )
            .is_empty()
        );
        m.handle(&req(Verb::Reset, NOW + 1, ""), NOW).unwrap();
        let again = started_at("2026-11-05T10:00:00+09:00");
        let why = m
            .handle(&req(Verb::Join, NOW + 2, &again.text), NOW)
            .unwrap_err();
        assert!(why.starts_with("a second welcome"), "{why}");
        let without: String = again
            .text
            .lines()
            .take(12)
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(
            m.handle(&req(Verb::Join, NOW + 3, &without), NOW).unwrap(),
            "12 lines"
        );
        m.handle(&req(Verb::Reset, NOW + 4, ""), NOW).unwrap();
        assert_eq!(
            m.handle(&req(Verb::Join, NOW + 5, &started().text), NOW)
                .unwrap(),
            "13 lines"
        );
    }

    #[test]
    fn join_needs_the_servers_rules_and_a_sane_clock() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let log = started().text;
        let mut r = req(Verb::Join, NOW, &log);
        r.rules = RULES - 1;
        assert!(m.handle(&r, NOW).unwrap_err().starts_with("update first"));
        let r = req(Verb::Join, NOW + 2 * CLOCK_SLACK_MS, &log);
        assert!(m.handle(&r, NOW).unwrap_err().contains("clock"));
        assert!(m.handle(&req(Verb::Reset, NOW, ""), NOW).unwrap_err() == NOT_IN);
    }

    /// N4: one address holds 2 of the 8 body slots at most.
    #[test]
    fn one_address_holds_two_upload_slots_at_most() {
        let slots = Mutex::new(HashMap::new());
        let a = Upload::take(&slots, "10.0.0.1").unwrap();
        let _b = Upload::take(&slots, "10.0.0.1").unwrap();
        assert!(Upload::take(&slots, "10.0.0.1").is_none());
        let others: Vec<_> = (2..=7)
            .map(|i| Upload::take(&slots, &format!("10.0.0.{i}")).unwrap())
            .collect();
        assert_eq!(others.len(), 6);
        assert!(Upload::take(&slots, "10.0.0.9").is_none(), "8 in flight");
        drop(a);
        assert!(Upload::take(&slots, "10.0.0.1").is_some());
    }

    /// Acceptance 3.1b (d) and N6: the table carries series and secret mark from a 2017-18 catalogue (a secret HOLE at
    /// 6,250, a secret white reprint at 40, an older EMVP card from its picture at x2.5) and its edition; a 2010-11
    /// catalogue or table is refused.
    #[test]
    fn the_table_is_written_from_a_catalogue_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let cat = dir.path().join("catalogue.tsv");
        std::fs::write(
            &cat,
            "card_no\trecord_index\trarity_name\tis_placeholder\tstats_total\tcard_code\timage_archive_path\n\
             101\t0\tREGULAR\t0\t80\tWGK-3\t\n\
             102\t1\tALL TIME LEGEND\t0\t100\tHOLE-3\t\n\
             103\t2\tREGULAR\t1\t70\t\t\n\
             0\t3\tRARE\t0\t90\t\t\n\
             104\t4\t\t0\t90\t\t\n\
             105\t5\tRARE\t0\t85\t\tcards/EMVP0506_X_f.dds\n\
             4659\t6\tREGULAR\t0\t100\tATLE-RE-4\t\n",
        )
        .unwrap();
        let out = dir.path().join("values.tsv");
        assert_eq!(write_table(&cat, &out).unwrap(), 4);
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("\nedition\t2017-18\n"), "{text}");
        assert!(
            text.contains("\n102\tALL TIME LEGEND\t100\tHOLE\t1\n"),
            "{text}"
        );
        assert!(text.contains("\n101\tREGULAR\t80\tWGK\t0\n"), "{text}");
        let t = Table::load(&out).unwrap();
        assert_eq!(t.value(101), Some(40));
        assert_eq!(t.value(102), Some(6_250));
        assert_eq!(t.value(105), Some(1_500));
        assert_eq!(t.value(4659), Some(40));
        assert_eq!(t.value(103), None);
        assert_eq!(t.catalogue.len(), 64);
        // a table of another economy table, or of 2010-11, is refused
        for (from, to, why) in [
            ("rules\t2", "rules\t0", "market-table"),
            ("edition\t2017-18", "edition\t2010-11", "2017-18 only"),
        ] {
            std::fs::write(&out, text.replace(from, to)).unwrap();
            assert!(Table::load(&out).unwrap_err().contains(why), "{to}");
        }
        // a 2010-11 catalogue (no card codes) is refused
        std::fs::write(
            &cat,
            "card_no\trarity_name\tis_placeholder\tstats_total\n101\tREGULAR\t0\t80\n",
        )
        .unwrap();
        assert!(
            write_table(&cat, &out)
                .unwrap_err()
                .contains("2017-18 only")
        );
        assert!(write_table(&dir.path().join("none.tsv"), &out).is_err());
    }
}
