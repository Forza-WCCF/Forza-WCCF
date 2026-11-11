//! Create-a-Team's market on the lobby's desk, step 3.1 (PLAN.md 3.1, 3.4-3.6): the server's value table, `market.db`
//! beside `gamesvr.db`, the plausibility check of a player's grant log and the desk words `MARKET JOIN`, `MARKET
//! LEAVE` and `MARKET RESET` (their format: [`forza::market`]). Listings, purchases and server stock are later steps.
//!
//! Trust first (PLAN.md 3.4): the check refuses a log that cannot come from an honest Launcher and says why in one
//! line; nothing is banned. The server keeps the last accepted log per key, so a log that drops accepted lines is a
//! fork; `MARKET RESET` (a profile reset) is the one allowed restart. LEAVE deletes the name, the log and the ledger
//! rows at once and keeps only the key's hash with its counters, for 7 days.
//!
//! The value table (card number, tier, TOTAL) is built on the server's host from an installed `catalogue.tsv` by
//! `forza-server market-table` ([`write_table`]) and never enters git. Without it every `MARKET` word answers
//! [`forza::market::NOT_SET_UP`].

use forza::collection::{self, Copies, Entry, Kind, RULES, Rarity};
use forza::market::{MAX_BODY, Request, Verb};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
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

fn tier(name: &str) -> Option<Rarity> {
    TIERS.iter().find(|(n, _)| *n == name).map(|(_, r)| *r)
}

fn tier_name(r: Rarity) -> &'static str {
    TIERS.iter().find(|(_, t)| *t == r).map_or("?", |(n, _)| n)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The server's card values: each card's tier and TOTAL, and what the table was built from.
#[derive(Debug, Default)]
pub struct Table {
    cards: HashMap<u32, (Rarity, u32)>,
    /// The economy table it was built for ([`RULES`]).
    pub rules: u32,
    /// SHA-256 of the `catalogue.tsv` it was built from, hex.
    pub catalogue: String,
}

impl Table {
    /// A table [`write_table`] wrote: `rules` and `catalogue` lines, then `card<TAB>tier<TAB>TOTAL` lines; `#` comments.
    /// One built for another [`RULES`] is refused (rebuild it with this server's `market-table`).
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut t = Self::default();
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
                ["rules", r] => t.rules = r.parse().map_err(|_| bad())?,
                ["catalogue", h] => t.catalogue = h.to_owned(),
                [no, name, total] => {
                    let no = no.parse().map_err(|_| bad())?;
                    let total = total.parse().map_err(|_| bad())?;
                    t.cards.insert(no, (tier(name).ok_or_else(bad)?, total));
                }
                _ => return Err(bad()),
            }
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

    /// A table from cards (tests).
    #[must_use]
    pub fn of(cards: &[(u32, Rarity, u32)]) -> Self {
        Self {
            cards: cards.iter().map(|&(c, r, t)| (c, (r, t))).collect(),
            rules: RULES,
            catalogue: String::new(),
        }
    }

    fn card(&self, card: u32) -> Option<(Rarity, u32)> {
        self.cards.get(&card).copied()
    }

    /// The card's craft value ([`collection::value`]).
    #[must_use]
    pub fn value(&self, card: u32) -> Option<u32> {
        self.card(card).map(|(r, t)| collection::value(r, t))
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

/// `forza-server market-table`: the value table at `out` from an installed `catalogue.tsv` (tab-separated, a header
/// naming the columns `card_no`, `rarity_name`, `stats_total`, `is_placeholder`). Placeholders and rows without a tier
/// are left out. Returns the number of cards written.
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
    let placeholder = col("is_placeholder");
    let mut cards = BTreeMap::new();
    for line in lines {
        let f: Vec<&str> = line.split('\t').collect();
        let get = |c: usize| f.get(c).map_or("", |s| s.trim());
        if placeholder.is_some_and(|p| get(p) == "1") {
            continue;
        }
        let (Ok(n), Some(r), Ok(t)) = (
            get(no).parse::<u32>(),
            tier(get(rarity)),
            get(total).parse::<u32>(),
        ) else {
            continue;
        };
        if n != 0 {
            cards.entry(n).or_insert((r, t));
        }
    }
    if cards.is_empty() {
        return Err(format!("{}: no cards", catalogue.display()));
    }
    let mut table = format!(
        "# forza-server market value table: card number, tier, TOTAL. Built on this host from the installed catalogue; never commit it.\nrules\t{RULES}\ncatalogue\t{}\n",
        hex(&Sha256::digest(&bytes))
    );
    for (n, (r, t)) in &cards {
        table.push_str(&format!("{n}\t{}\t{t}\n", tier_name(*r)));
    }
    std::fs::write(out, table).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(cards.len())
}

/// `match` lines this far apart at least (PLAN.md 3.5).
const MATCH_GAP: time::Duration = time::Duration::minutes(8);
/// Import lines only this long after the first `starter` line (PLAN.md 3.8).
const IMPORT_WINDOW: time::Duration = time::Duration::days(7);
/// The starter's copies: 11 REGULAR cards and the hero.
const STARTER_CARDS: u32 = 12;
const PACK_PRICES: [i64; 3] = [
    collection::STANDARD_PACK,
    collection::SEASON_PACK,
    collection::NATION_PACK,
];

/// The day's match-token cap of a line written under `rules`; `None` for a table this server does not know.
fn match_cap(rules: u32) -> Option<i64> {
    match rules {
        1 => Some(collection::DAILY_MATCH_CAP),
        _ => None,
    }
}

/// The plausibility check (PLAN.md 3.4): the chain (`seq` and `prev` of every line), `match` lines 8 minutes apart and
/// within each business day's cap of their `rules`, pack and craft lines paid at the table's price, conversions at
/// [`collection::CONVERT_SHARE`] and only while another unlisted earned copy remains, no `convert` or `list` of a
/// starter or imported copy, `import` lines only before the first `list` or `market_buy` and within 7 days of the first
/// `starter`, market lines inside the price band, and a balance never below zero. Returns the log's lines (blank ones
/// left out), or the first line that fails and why.
pub fn check<'a>(log: &'a str, table: &Table) -> Result<Vec<&'a str>, String> {
    let mut lines: Vec<&str> = Vec::new();
    let mut held: HashMap<u32, Copies> = HashMap::new();
    let mut balance: i64 = 0;
    let mut last_match: Option<time::OffsetDateTime> = None;
    let mut match_days: HashMap<u32, i64> = HashMap::new();
    let mut first_starter: Option<time::OffsetDateTime> = None;
    let mut starters = 0;
    let mut market_used = false;
    let mut pack_left = 0;
    for (n, raw) in log.lines().enumerate().map(|(n, l)| (n + 1, l)) {
        if raw.trim().is_empty() {
            continue;
        }
        let e: Entry =
            serde_json::from_str(raw).map_err(|e| format!("line {n} does not parse ({e})"))?;
        let no = |why: String| format!("line {n}: {why}");
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
        // every card the log names is a card of the server's table, with its tier and TOTAL
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
            Kind::Match => {
                if e.tokens < 0 {
                    return Err(no("a match takes tokens".into()));
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
                let day = collection::day(e.at);
                let today = match_days.entry(day).or_default();
                *today = today.saturating_add(e.tokens);
                if *today > cap {
                    return Err(no(format!(
                        "{today} match tokens in the business day {day}, over its cap of {cap}"
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
                let (c, (r, t)) = need()?;
                paid(-i64::from(collection::value(r, t)))?;
                held.entry(c).or_default().earned += 1;
            }
            Kind::Convert => {
                let (c, (r, t)) = need()?;
                let h = held.entry(c).or_default();
                match collection::convert(h, r, t) {
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
                if e.at - start > IMPORT_WINDOW {
                    return Err(no("an import more than 7 days after the starter".into()));
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
                let (c, (r, t)) = need()?;
                if !band(collection::value(r, t)).contains(&-e.tokens) {
                    return Err(no(format!(
                        "a purchase of card {c} for {} tokens, outside its price band",
                        -e.tokens
                    )));
                }
                held.entry(c).or_default().earned += 1;
                market_used = true;
            }
            Kind::MarketSell => {
                let (c, (r, t)) = need()?;
                if !band(collection::value(r, t)).contains(&e.tokens) {
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
        }
        balance = balance
            .checked_add(e.tokens)
            .filter(|b| *b >= 0)
            .ok_or_else(|| no("the token balance goes below zero".into()))?;
        lines.push(raw);
    }
    Ok(lines)
}

/// A key's requests this far from the server's clock are refused (a PC's clock a day off is broken, not a time zone).
const CLOCK_SLACK_MS: u64 = 24 * 3600 * 1000;
/// LEAVE keeps the key's hash and counters this long.
const KEEP_LEFT_MS: u64 = 7 * 24 * 3600 * 1000;
/// Bodies read at once across the desk: each may hold [`MAX_BODY`] in memory.
const MAX_UPLOADS: usize = 8;

const SCHEMA: &str = "
-- one row per installation key, by the SHA-256 of the public key: LEAVE keeps the hash and the counters only
CREATE TABLE IF NOT EXISTS keys (
    key_hash   TEXT PRIMARY KEY,
    name       TEXT,                       -- the cleaned club name; NULL after LEAVE
    log        TEXT,                       -- the last accepted grant log; NULL after LEAVE and RESET
    last_at    INTEGER NOT NULL,           -- the last request's time (Unix ms): later requests only
    listings   INTEGER NOT NULL DEFAULT 0, -- the listing counter (step 3.2), kept by RESET and LEAVE
    sales      INTEGER NOT NULL DEFAULT 0, -- the sales of the week `sales_week` (step 3.2), kept by RESET and LEAVE
    sales_week INTEGER NOT NULL DEFAULT 0,
    forks      INTEGER NOT NULL DEFAULT 0, -- logs refused for dropping accepted lines (for the operator)
    left_at    INTEGER                     -- LEAVE's time (Unix ms); the row goes 7 days later
);
-- listings and sales by key (step 3.2 fills it): RESET drops the open listings and waiting proceeds, LEAVE every row
CREATE TABLE IF NOT EXISTS ledger (
    id       INTEGER PRIMARY KEY,
    key_hash TEXT    NOT NULL,
    card     INTEGER NOT NULL,
    price    INTEGER NOT NULL,
    state    TEXT    NOT NULL, -- open, sold, proceeds, paid
    at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ledger_key ON ledger (key_hash);
";

/// The market's server state: the value table, `market.db` and the uploads in flight.
pub struct Market {
    table: Table,
    db: Mutex<Connection>,
    uploads: AtomicUsize,
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

/// `NO why` for a request whose time is not after the key's last one.
const OLD: &str = "an old request (sent twice?)";
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
            c.execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
            )?;
            c.execute_batch(SCHEMA)?;
            Ok(c)
        };
        Ok(Self {
            table,
            db: Mutex::new(open().map_err(|e| format!("{}: {e}", db.display()))?),
            uploads: AtomicUsize::new(0),
        })
    }

    #[must_use]
    pub fn table(&self) -> &Table {
        &self.table
    }

    /// One checked request at the server's time `now` (Unix ms): `Ok` with the words after `OK`, `Err` with the reason.
    pub fn handle(&self, req: &Request, now: u64) -> Result<String, String> {
        if req.at.abs_diff(now) > CLOCK_SLACK_MS {
            return Err("this PC's clock is more than a day off the server's".into());
        }
        let kh = key_hash(&req.key);
        // the check reads no stored state: done before the store is locked
        let lines = match req.verb {
            Verb::Join => {
                if req.rules != self.table.rules {
                    return Err(if req.rules < self.table.rules {
                        "update first: this Launcher's market rules are older than the server's"
                            .into()
                    } else {
                        "this server's market rules are older than this Launcher's".into()
                    });
                }
                if req.name.is_empty() {
                    return Err("a market name is needed".into());
                }
                check(&req.log, &self.table)?
            }
            Verb::Leave | Verb::Reset => vec![],
        };
        let mut db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        // on its own: a refused request below rolls its transaction back, the expiry stays done
        db.execute(
            "DELETE FROM keys WHERE left_at IS NOT NULL AND left_at <= ?1",
            params![now.saturating_sub(KEEP_LEFT_MS) as i64],
        )
        .map_err(store_err)?;
        let tx = db.transaction().map_err(store_err)?;
        let row: Option<(i64, Option<String>, Option<i64>)> = tx
            .query_row(
                "SELECT last_at, log, left_at FROM keys WHERE key_hash = ?1",
                params![kh],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(store_err)?;
        let at = i64::try_from(req.at).map_err(|_| OLD.to_string())?;
        if row.as_ref().is_some_and(|(last, ..)| at <= *last) {
            return Err(OLD.into());
        }
        let joined = row.as_ref().is_some_and(|(.., left)| left.is_none());
        let answer = match req.verb {
            Verb::Join => {
                let old: Vec<&str> = row
                    .as_ref()
                    .and_then(|(_, log, _)| log.as_deref())
                    .map(|l| l.lines().filter(|l| !l.is_empty()).collect())
                    .unwrap_or_default();
                let kept = old.iter().zip(&lines).take_while(|(a, b)| a == b).count();
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
                let mut log = lines.join("\n");
                log.push('\n');
                tx.execute(
                    "INSERT INTO keys (key_hash, name, log, last_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (key_hash) DO UPDATE SET name = ?2, log = ?3, last_at = ?4, left_at = NULL",
                    params![kh, req.name, log, at],
                )
                .map_err(store_err)?;
                format!("{} lines", lines.len())
            }
            Verb::Reset => {
                if !joined {
                    return Err(NOT_IN.into());
                }
                tx.execute(
                    "UPDATE keys SET log = NULL, last_at = ?2 WHERE key_hash = ?1",
                    params![kh, at],
                )
                .map_err(store_err)?;
                tx.execute(
                    "DELETE FROM ledger WHERE key_hash = ?1 AND state IN ('open', 'proceeds')",
                    params![kh],
                )
                .map_err(store_err)?;
                "reset".into()
            }
            Verb::Leave => {
                if !joined {
                    return Err(NOT_IN.into());
                }
                tx.execute(
                    "UPDATE keys SET name = NULL, log = NULL, last_at = ?2, left_at = ?3 WHERE key_hash = ?1",
                    params![kh, at, now as i64],
                )
                .map_err(store_err)?;
                tx.execute("DELETE FROM ledger WHERE key_hash = ?1", params![kh])
                    .map_err(store_err)?;
                "left".into()
            }
        };
        tx.commit().map_err(store_err)?;
        Ok(answer)
    }

    /// One `MARKET` request on the desk (`words`: the line's words after `MARKET`; `rest`: what the desk read past the
    /// line): reads the body, answers `OK ...` or `NO why` on `conn`, and returns what the desk logs.
    pub fn serve(
        &self,
        words: &[&str],
        rest: Vec<u8>,
        conn: &mut TcpStream,
        now: u64,
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
        let Some(_upload) = Upload::take(&self.uploads) else {
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

/// One body in flight, counted against [`MAX_UPLOADS`] until dropped.
struct Upload<'a>(&'a AtomicUsize);

impl<'a> Upload<'a> {
    fn take(n: &'a AtomicUsize) -> Option<Self> {
        n.try_update(Ordering::AcqRel, Ordering::Acquire, |k| {
            (k < MAX_UPLOADS).then_some(k + 1)
        })
        .ok()
        .map(|_| Self(n))
    }
}

impl Drop for Upload<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
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

    /// Cards 1-40 REGULAR (TOTAL 80, value 40), 41-50 SPECIAL (90: 250), 51-55 RARE (85: 600), 56 LEGEND (95: 6,000).
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

    /// A grant log written line by line, as `collection::Log` chains it.
    #[derive(Default)]
    struct Log {
        text: String,
        last: String,
        seq: u64,
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
                why: String::new(),
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

    const START: &str = "2026-11-01T10:00:00+09:00";

    /// The first-run deal: 11 REGULAR starters (1-11) and the RARE hero (51).
    fn started() -> Log {
        let mut l = Log::default();
        for c in (1..=11).chain([51]) {
            l.add(t(START), Kind::Starter, Some(c), 0);
        }
        l
    }

    fn refused(log: &Log) -> String {
        check(&log.text, &table()).unwrap_err()
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
                if i == 0 { 40 } else { 15 },
            );
        }
        let why = refused(&l);
        assert!(
            why.contains("line 14") && why.contains("8 minutes apart"),
            "{why}"
        );
    }

    /// Acceptance 3.1 (b): 200 match tokens in one business day are refused, with the reason.
    #[test]
    fn match_tokens_over_the_days_cap_are_refused() {
        let mut l = started();
        let at = t("2026-11-01T12:00:00+09:00");
        for i in 0..5 {
            l.add(
                at + time::Duration::minutes(10 * i),
                Kind::Match,
                Some(20),
                40,
            );
        }
        let why = refused(&l);
        assert!(
            why.contains("160 match tokens") && why.contains("cap of 130"),
            "{why}"
        );
        // the same five matches over two business days (the day turns at 05:00) pass
        let mut l = started();
        for (i, at) in ["12:00", "12:10", "12:20", "23:00"].iter().enumerate() {
            l.add(
                t(&format!("2026-11-01T{at}:00+09:00")),
                Kind::Match,
                Some(20),
                if i == 0 { 40 } else { 15 },
            );
        }
        l.add(t("2026-11-02T05:00:00+09:00"), Kind::Match, Some(21), 40);
        check(&l.text, &table()).unwrap();
    }

    /// Acceptance 3.1 (b): an import after a listing is refused, with the reason.
    #[test]
    fn an_import_after_a_listing_is_refused() {
        let mut l = started();
        l.add(t("2026-11-01T12:00:00+09:00"), Kind::Match, Some(20), 40)
            .add(t("2026-11-01T12:01:00+09:00"), Kind::List, Some(20), 0)
            .add(t("2026-11-01T12:02:00+09:00"), Kind::Import, Some(30), 0);
        let why = refused(&l);
        assert!(why.contains("import after a market listing"), "{why}");
        // and 8 days after the starter, and before it
        let mut l = started();
        l.add(t("2026-11-09T10:00:01+09:00"), Kind::Import, Some(30), 0);
        assert!(refused(&l).contains("more than 7 days"));
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
        l.add(at, Kind::Match, Some(1), 40).add(
            at + time::Duration::minutes(8),
            Kind::Convert,
            Some(1),
            8,
        );
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
        // a pack bought on credit
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 40)
            .add(at, Kind::Pack, None, -100);
        assert!(refused(&l).contains("below zero"));
        // a card the server does not know, a broken chain, a skipped seq
        let mut l = started();
        l.add(at, Kind::Match, Some(999), 40);
        assert!(refused(&l).contains("card 999 is not in this server's table"));
        let l = started();
        let text = l.text.replacen("\"why\":\"\"", "\"why\":\"x\"", 1);
        assert!(check(&text, &table()).unwrap_err().contains("line 2: prev"));
        let text: String = l
            .text
            .lines()
            .filter(|x| !x.contains("\"seq\":3,"))
            .map(|x| format!("{x}\n"))
            .collect();
        assert!(
            check(&text, &table())
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
            check("not json\n", &table())
                .unwrap_err()
                .contains("line 1 does not parse")
        );
    }

    /// An honest month: the deal, an import on day 3, matches every day up to the cap, Standard and Season packs, a
    /// craft, kept copies, conversions of spares only, a listing returned, a sale and a purchase.
    fn honest_month() -> String {
        let mut l = started();
        let day0 = t("2026-11-01T11:00:00+09:00");
        let mut balance = 0i64;
        for d in 0..30 {
            let morning = day0 + time::Duration::days(d);
            if d == 2 {
                for c in [30, 31, 2] {
                    l.add(morning, Kind::Import, Some(c), 0);
                }
            }
            // 7 matches 9 minutes apart: 40 + 6 x 15 = 130, then an 8th that pays nothing
            let mut today = 0;
            for m in 0..8 {
                let pays = collection::match_tokens(today);
                today += pays;
                balance += pays;
                let card = 12 + ((d * 8 + m) % 10) as u32; // REGULAR 12-21: copies pile up
                l.add(
                    morning + time::Duration::minutes(9 * m),
                    Kind::Match,
                    Some(card),
                    pays,
                );
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
                l.add(
                    evening + time::Duration::minutes(1),
                    Kind::Convert,
                    Some(12),
                    8,
                );
                balance += 8;
            }
            if d == 20 {
                l.add(
                    evening + time::Duration::minutes(2),
                    Kind::Craft,
                    Some(52),
                    -600,
                );
                balance -= 600;
                l.add(
                    evening + time::Duration::minutes(3),
                    Kind::List,
                    Some(13),
                    0,
                )
                .add(
                    evening + time::Duration::minutes(4),
                    Kind::Unlist,
                    Some(13),
                    0,
                )
                .add(
                    evening + time::Duration::minutes(5),
                    Kind::List,
                    Some(14),
                    0,
                )
                .add(
                    evening + time::Duration::minutes(6),
                    Kind::MarketSell,
                    Some(14),
                    36,
                )
                .add(
                    evening + time::Duration::minutes(7),
                    Kind::MarketBuy,
                    Some(42),
                    -225,
                );
                balance += 36 - 225;
            }
            assert!(balance >= 0, "the fixture spends what it has: day {d}");
        }
        l.text
    }

    /// Acceptance 3.1 (c): an honest 1-month log with kept copies, conversions and an import passes.
    #[test]
    fn an_honest_month_passes() {
        let text = honest_month();
        let lines = check(&text, &table()).unwrap();
        assert!(lines.len() > 300, "{}", lines.len());
        let folded = collection::fold(&text);
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert!(folded.state.copies(12).earned >= 2, "kept copies");
        assert_eq!(folded.state.copies(30).imported, 1);
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
            name: "Club Nine".into(),
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

    /// Acceptance 3.1 (d): a second log that drops accepted lines is a fork; after MARKET RESET a new history for the
    /// same key is accepted and the sale counter stays.
    #[test]
    fn a_dropped_line_is_a_fork_until_a_reset() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
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
        // a rewritten history: the first 12 lines kept, the rest another story
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
            "13 lines"
        );
        assert_eq!(
            row(dir.path(), "SELECT sales, listings, name FROM keys"),
            [[s("3"), s("2"), s("Club Nine")]]
        );
        // a request sent again is refused
        assert_eq!(
            m.handle(&req(Verb::Join, NOW + 4, &other), NOW)
                .unwrap_err(),
            OLD
        );
    }

    /// Acceptance 3.1 (e): LEAVE removes the name, the log and the ledger rows and keeps only the key's hash with its
    /// counters, for 7 days.
    #[test]
    fn leave_keeps_only_the_key_hash_and_its_counters() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        m.handle(&req(Verb::Join, NOW, &started().text), NOW)
            .unwrap();
        let c = Connection::open(dir.path().join("market.db")).unwrap();
        c.execute("UPDATE keys SET sales = 4, listings = 1", [])
            .unwrap();
        let kh = key_hash(&[9; 32]);
        for state in ["open", "sold", "proceeds"] {
            c.execute(
                "INSERT INTO ledger (key_hash, card, price, state, at) VALUES (?1, 5, 36, ?2, 0)",
                params![kh, state],
            )
            .unwrap();
        }
        // a reset drops the open listing and the waiting proceeds only
        m.handle(&req(Verb::Reset, NOW + 1, ""), NOW).unwrap();
        assert_eq!(row(dir.path(), "SELECT state FROM ledger"), [[s("sold")]]);
        assert_eq!(
            m.handle(&req(Verb::Leave, NOW + 2, ""), NOW).unwrap(),
            "left"
        );
        assert_eq!(
            row(
                dir.path(),
                "SELECT key_hash, name, log, sales, listings FROM keys"
            ),
            [[s(&kh), None, None, s("4"), s("1")]]
        );
        assert!(row(dir.path(), "SELECT * FROM ledger").is_empty());
        assert_eq!(
            m.handle(&req(Verb::Leave, NOW + 3, ""), NOW).unwrap_err(),
            NOT_IN
        );
        // back within 7 days: the counters are still there
        m.handle(&req(Verb::Join, NOW + 4, &started().text), NOW + 5)
            .unwrap();
        assert_eq!(row(dir.path(), "SELECT sales FROM keys"), [[s("4")]]);
        // gone 7 days after a LEAVE
        m.handle(&req(Verb::Leave, NOW + 6, ""), NOW + 6).unwrap();
        let later = NOW + 6 + KEEP_LEFT_MS;
        assert_eq!(
            m.handle(&req(Verb::Reset, later, ""), later).unwrap_err(),
            NOT_IN
        );
        assert!(row(dir.path(), "SELECT * FROM keys").is_empty());
    }

    #[test]
    fn join_needs_the_servers_rules_a_name_and_a_sane_clock() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let log = started().text;
        let mut r = req(Verb::Join, NOW, &log);
        r.rules = RULES - 1;
        assert!(m.handle(&r, NOW).unwrap_err().starts_with("update first"));
        let mut r = req(Verb::Join, NOW, &log);
        r.name.clear();
        assert!(m.handle(&r, NOW).unwrap_err().contains("name"));
        let r = req(Verb::Join, NOW + 2 * CLOCK_SLACK_MS, &log);
        assert!(m.handle(&r, NOW).unwrap_err().contains("clock"));
        assert!(m.handle(&req(Verb::Reset, NOW, ""), NOW).unwrap_err() == NOT_IN);
    }

    #[test]
    fn the_table_is_written_from_a_catalogue_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let cat = dir.path().join("catalogue.tsv");
        std::fs::write(
            &cat,
            "card_no\trecord_index\trarity_name\tis_placeholder\tstats_total\n\
             101\t0\tREGULAR\t0\t80\n\
             102\t1\tALL TIME LEGEND\t0\t100\n\
             103\t2\tREGULAR\t1\t70\n\
             0\t3\tRARE\t0\t90\n\
             104\t4\t\t0\t90\n",
        )
        .unwrap();
        let out = dir.path().join("values.tsv");
        assert_eq!(write_table(&cat, &out).unwrap(), 2);
        let t = Table::load(&out).unwrap();
        assert_eq!(t.value(101), Some(40));
        assert_eq!(t.value(102), Some(10_000));
        assert_eq!(t.value(103), None);
        assert_eq!(t.catalogue.len(), 64);
        // a table of another economy table is refused
        let text = std::fs::read_to_string(&out)
            .unwrap()
            .replace("rules\t1", "rules\t0");
        std::fs::write(&out, text).unwrap();
        assert!(Table::load(&out).unwrap_err().contains("market-table"));
        assert!(write_table(&dir.path().join("none.tsv"), &out).is_err());
    }
}
