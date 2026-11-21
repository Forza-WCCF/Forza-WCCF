//! Create-a-Team's market on the lobby's desk, steps 3.1, 3.1b, 3.2 and 3.3 (PLAN.md 3.1, 3.4-3.6): the server's value
//! table, `market.db` beside `gamesvr.db`, the plausibility check of a player's grant log, the desk words `MARKET JOIN`,
//! `LEAVE` and `RESET`, the ledger's `LIST`, `CANCEL`, `BROWSE`, `BUY` and `COLLECT`, each key's own stock list,
//! `STOCK` and `STOCKBUY`, and `OWED`, the rows a log lacks (their format: [`forza::market`]).
//!
//! The ledger (PLAN.md 3.4, 3.5): a price inside the card's band from the server's own table, 5 open listings and 10
//! sales a week per key, 7 days on the market, proceeds held until the seller's COLLECT. LIST and BUY carry the log:
//! LIST needs an earned copy that neither the log nor the ledger has on the market (a starter or imported copy never
//! lists), BUY needs the log's balance less this key's purchases its log does not hold yet. No answer names a seller,
//! a buyer or a key. A ledger word's answer is kept with its key until the next request, so the same request sent
//! again (its answer lost) answers it again and changes nothing.
//!
//! Trust first (PLAN.md 3.4): the check refuses a log that cannot come from an honest Launcher and says why in one
//! line; nothing is banned. The server keeps the last accepted log per key, so a log that drops accepted lines is a
//! fork; `MARKET RESET` (a profile reset) is the one allowed restart. No name is asked or stored. LEAVE deletes the log
//! at once (overwritten in the file, not only unlinked), returns the open listings and keeps the key's hash with its
//! counters for 7 days, longer while a sale waits for COLLECT.
//!
//! The own stock list (PLAN.md 3.5): per key and server day (04:00 to 04:00 UTC, the server's clock), drawn from the
//! store's secret, the key and the day, plain random within each tier group of the whole table at a share of each
//! card's value ([`Stock`]), and kept from its first view with its sold slots: a reload, LEAVE then JOIN and RESET show
//! the same rows. A purchase needs the log's balance less this key's purchases its log does not hold yet, as BUY.
//!
//! The value table (card number, tier, TOTAL, series, secret mark) is built on the server's host from an installed
//! 2017-18 `catalogue.tsv` by `forza-server market-table` ([`write_table`]) and never enters git. Without it every
//! `MARKET` word answers [`forza::market::NOT_SET_UP`].

use forza::collection::{self, Copies, Entry, Kind, RULES, Rarity};
use forza::market::{BROWSE_PAGE, COLLECT_PAGE, MAX_BODY, OLD, Request, Verb};
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
/// header naming the columns `card_no`, `rarity_name`, `stats_total`, `card_code`, `stat_offence` .. `stat_stamina`,
/// and `is_placeholder` and `image_archive_path` when present). The series and the secret mark follow
/// `forza::cards::load_catalogue`: the card code without its number (an older MVP card's mark from its picture), secret
/// by the six printed stats ([`collection::secret`]). A catalogue without `card_code` is 2010-11's and refused. Placeholders and rows without a tier are
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
    let stat_cols = [
        "stat_offence",
        "stat_defence",
        "stat_technique",
        "stat_power",
        "stat_speed",
        "stat_stamina",
    ]
    .map(col);
    if stat_cols.contains(&None) {
        return Err(format!(
            "{}: no stat_offence .. stat_stamina columns, so no secret mark",
            catalogue.display()
        ));
    }
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
        let secret = collection::secret(stat_cols.map(|c| get(c).parse::<u8>().unwrap_or(0)));
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

/// `match` lines this far apart at least (PLAN.md 3.5): one match, `GAME-CLOCK.md`'s MATCHING 5 s, plus a game plus
/// the result screens. The doc gives no lower bound on a game (a forfeit ends one early), so 3 minutes stands for the
/// shortest honest pair.
const MATCH_GAP: time::Duration = time::Duration::minutes(3);
/// `match` lines a week holds at most. PLAN.md 2.2 pays the first 21 matches and deals a card on every match after
/// them with no card cap; its heaviest reference player plays 12 a day (84 a week), so 120 leaves that player room and
/// still bounds a forged log to 120 cards a week.
const MATCHES_PER_WEEK: u32 = 120;
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
#[derive(Debug, Default)]
pub struct Checked<'a> {
    /// Its lines, blank ones and a torn last line left out.
    pub lines: Vec<&'a str>,
    /// Its `welcome` line, if any (one at most).
    pub welcome: Option<&'a str>,
    /// The copies it holds, by card.
    pub held: HashMap<u32, Copies>,
    /// Its token balance at the end.
    pub balance: i64,
    /// Its `market_buy` lines: card and price.
    pub bought: Vec<(u32, i64)>,
    /// Its `stock_buy` lines: card and price.
    pub stock_bought: Vec<(u32, i64)>,
    /// Its `market_sell` lines: card and price.
    pub sold: Vec<(u32, i64)>,
}

/// The plausibility check (PLAN.md 3.4) under rules 2: the chain (`seq` and `prev` of every line), `match` lines 3
/// minutes apart and [`MATCHES_PER_WEEK`] a week at most, each paying what [`collection::match_tokens`] says at the
/// week's counter ([`collection::week`], the line's own offset, as the Launcher counts it), one `welcome` of
/// [`collection::WELCOME`], pack lines at a pack's price, crafts of white and
/// black cards only at the table's price, conversions at [`collection::CONVERT_SHARE`] and only while another unlisted
/// earned copy remains, no `convert` or `list` of a starter or imported copy, `import` lines only before the first
/// `list`, `market_buy` or `stock_buy` and within 7 days after the first `starter`, market lines inside the price
/// band, each `stock_buy` matching one of `stock` (this key's stock sales: card and price), and a balance never below
/// zero. An unparsable last line is left out (the Launcher stopped mid-append; its next append cuts it). Returns the
/// log's lines, or the first line that fails and why.
pub fn check<'a>(log: &'a str, table: &Table, stock: &[(u32, i64)]) -> Result<Checked<'a>, String> {
    let mut out = Checked::default();
    let mut stock = stock.to_vec();
    let mut held: HashMap<u32, Copies> = HashMap::new();
    let mut balance: i64 = 0;
    let mut last_match: Option<time::OffsetDateTime> = None;
    // a week: its match tokens earned and its `match` lines so far
    let mut match_weeks: HashMap<u32, (i64, u32)> = HashMap::new();
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
        // the line's tokens as a spend; i64::MIN has no positive form
        let spent = || {
            e.tokens
                .checked_neg()
                .ok_or_else(|| no(format!("{} tokens is not a price", e.tokens)))
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
                        "a match {} s after the one before; matches are {} minutes apart at least",
                        (e.at - last).whole_seconds(),
                        MATCH_GAP.whole_minutes()
                    )));
                }
                last_match = Some(e.at);
                let week = collection::week(e.at);
                let (earned, lines) = match_weeks.entry(week).or_default();
                if *lines >= MATCHES_PER_WEEK {
                    return Err(no(format!(
                        "match {} in the week {week}; a week holds {MATCHES_PER_WEEK}",
                        *lines + 1
                    )));
                }
                *lines += 1;
                let pays = collection::match_tokens(*earned);
                if e.tokens != pays {
                    return Err(no(format!(
                        "a match for {} tokens; the week {week} holds {earned} of its {cap}: this one pays {pays}",
                        e.tokens
                    )));
                }
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
                            spent()?
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
                let price = spent()?;
                if !band(collection::value(&card)).contains(&price) {
                    return Err(no(format!(
                        "a purchase of card {c} for {price} tokens, outside its price band"
                    )));
                }
                held.entry(c).or_default().earned += 1;
                market_used = true;
                out.bought.push((c, price));
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
                out.sold.push((c, e.tokens));
            }
            Kind::StockBuy => {
                let (c, _) = need()?;
                let price = spent()?;
                let Some(i) = stock.iter().position(|&s| s == (c, price)) else {
                    return Err(no(format!(
                        "a stock purchase of card {c} for {price} tokens the server never sold to this installation"
                    )));
                };
                stock.swap_remove(i);
                held.entry(c).or_default().earned += 1;
                market_used = true;
                out.stock_bought.push((c, price));
            }
        }
        balance = balance
            .checked_add(e.tokens)
            .filter(|b| *b >= 0)
            .ok_or_else(|| no("the token balance goes below zero".into()))?;
        out.lines.push(raw);
    }
    (out.held, out.balance) = (held, balance);
    Ok(out)
}

/// A key's requests this far from the server's clock are refused (a PC's clock a day off is broken, not a time zone).
const CLOCK_SLACK_MS: u64 = 24 * 3600 * 1000;
/// LEAVE keeps the key's hash and counters this long (longer while a sale waits for COLLECT).
const KEEP_LEFT_MS: u64 = 7 * 24 * 3600 * 1000;
/// A listing sells this long; after it, COLLECT returns it (PLAN.md 3.5).
const LISTING_MS: u64 = 7 * 24 * 3600 * 1000;
/// Open listings per key at once, and sales per key in a week (PLAN.md 3.5 "Guards").
const OPEN_LISTINGS: i64 = 5;
const SALES_PER_WEEK: u32 = 10;
/// BUY's refusal of a listing that sold, was cancelled, expired or never was.
const GONE: &str = "that listing is no longer on the market";
/// CANCEL's refusal of a listing that sold.
const SOLD: &str = "that listing sold: COLLECT pays it";
/// STOCKBUY's refusal of a slot bought today.
const STOCK_SOLD: &str = "that slot is sold: each sells once a day";
/// Rows of one stock list at most: about 20 bytes each, so a STOCK answer stays under the Launcher's 1 KiB read.
const STOCK_MAX: u32 = 30;
/// The stock list's tier groups, in the list's order (PLAN.md 3.5: 4 REGULAR, 3 SPECIAL, 2 RARE, 1 LEGEND or ALL TIME
/// LEGEND).
const STOCK_GROUPS: [&[Rarity]; 4] = [
    &[Rarity::Regular],
    &[Rarity::Special],
    &[Rarity::Rare],
    &[Rarity::Legend, Rarity::AllTimeLegend],
];

/// The own stock list's settings (PLAN.md 3.5 "Settings (a5)"), read at server start; a server day keeps those of
/// its first list, so a restart's new settings start with the next day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stock {
    /// Cards per list in each of [`STOCK_GROUPS`]; their sum is the list's size.
    pub mix: [u32; 4],
    /// A slot's price in percent of its card's value, rounded down.
    pub share: u32,
    /// Purchases per key and server day; 0: no limit.
    pub limit: u32,
}

impl Default for Stock {
    fn default() -> Self {
        Self {
            mix: [4, 3, 2, 1],
            share: 90,
            limit: 0,
        }
    }
}

impl Stock {
    /// The settings from `mix` as `regular,special,rare,legend` (`4,3,2,1`), a share of 1 to 100 % and a limit (0:
    /// none); refused with the reason.
    pub fn new(mix: &str, share: u32, limit: u32) -> Result<Self, String> {
        let n: Vec<u32> = mix
            .split(',')
            .map(|n| n.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|_| format!("--stock-mix {mix:?} is not four numbers"))?;
        let mix: [u32; 4] = n
            .try_into()
            .map_err(|_| format!("--stock-mix {mix:?} is not four numbers"))?;
        let cards = mix.iter().sum::<u32>();
        if !(1..=STOCK_MAX).contains(&cards) {
            return Err(format!(
                "--stock-mix {mix:?} makes a list of {cards} cards: 1 to {STOCK_MAX}"
            ));
        }
        if !(1..=100).contains(&share) {
            return Err(format!("--stock-share {share}: 1 to 100 %"));
        }
        Ok(Self { mix, share, limit })
    }

    fn mix_text(&self) -> String {
        self.mix.map(|n| n.to_string()).join(",")
    }
}

/// The server day at `now` (Unix ms) as days since 1970-01-01 04:00 UTC: the stock list turns at 04:00 UTC (05:00 CET,
/// 13:00 JST; PLAN.md 3.5 m6), on the server's clock, so a PC's date buys no new list.
fn stock_day(now: u64) -> i64 {
    ((now / 1000).saturating_sub(4 * 3600) / 86_400) as i64
}

/// One ledger row of a key that its log has no line for: the row's id, card and price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Owed {
    id: i64,
    card: u32,
    price: i64,
}

/// The key `kh`'s ledger rows its log (`checked`) has no line for, each line taking the oldest row of its card and
/// price: listings bought (`bought`, no `market_buy` line), stock slots (`stock`, no `stock_buy` line) and sales COLLECT
/// paid (`sold`, no `market_sell` line), in that order. A `stock_gone` row (bought before the key expired) takes its
/// line first and is never owed.
fn owed(
    tx: &Connection,
    kh: &str,
    checked: &Checked<'_>,
) -> Result<Vec<(&'static str, Owed)>, String> {
    let mut out = vec![];
    for (word, states, written) in [
        ("bought", "'bought'", &checked.bought),
        ("stock", "'stock_gone', 'stock'", &checked.stock_bought),
        ("sold", "'paid'", &checked.sold),
    ] {
        let mut lines: HashMap<(u32, i64), usize> = HashMap::new();
        for b in written {
            *lines.entry(*b).or_default() += 1;
        }
        let rows: Vec<(i64, u32, i64, String)> = tx
            .prepare(&format!(
                "SELECT id, card, price, state FROM ledger WHERE key_hash = ?1 AND state IN ({states})
                 ORDER BY state <> 'stock_gone', id"
            ))
            .and_then(|mut s| {
                s.query_map(params![kh], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                    .collect()
            })
            .map_err(store_err)?;
        for (id, card, price, state) in rows {
            match lines.get_mut(&(card, price)) {
                Some(n) if *n > 0 => *n -= 1,
                _ if state != "stock_gone" => out.push((word, Owed { id, card, price })),
                _ => {}
            }
        }
    }
    Ok(out)
}

/// The log's balance less this key's purchases its log does not hold yet ([`owed`]'s `bought` and `stock` rows).
/// [`check`] already matched every `stock_buy` line to a sold slot.
fn spendable(tx: &Connection, kh: &str, checked: &Checked<'_>) -> Result<i64, String> {
    let withheld: i64 = owed(tx, kh, checked)?
        .iter()
        .filter(|(word, _)| *word != "sold")
        .map(|(_, o)| o.price)
        .sum();
    Ok(checked.balance - withheld)
}

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
    sales      INTEGER NOT NULL DEFAULT 0, -- the sales of the week `sales_week` (YYYYWW), kept by RESET and LEAVE
    sales_week INTEGER NOT NULL DEFAULT 0,
    forks      INTEGER NOT NULL DEFAULT 0, -- logs refused for dropping accepted lines (for the operator)
    left_at    INTEGER,                    -- LEAVE's time (Unix ms); the row goes 7 days after it and the last request, once no sale waits
    asked      TEXT,                       -- the last request's verb and numbers, a ledger word's only, and
    answer     TEXT                        -- its answer: that request sent again answers it once more
);
-- listings, sales and purchases by key (steps 3.2, 3.3 fill it). state: open (listed at `at`), sold (sold, the
-- proceeds wait for the seller's COLLECT), paid (collected), bought (this key's purchase of a listing: one
-- `market_buy` line matches it), stock (this key's stock-list purchase: one `stock_buy` line matches it),
-- stock_reset (one from before a RESET: the slot stays sold, no line matches it), stock_gone (a stock row of a key
-- dropped 7 days after LEAVE: its line still matches it when that log JOINs again, and it withholds nothing). Ids are never reused: a listing's
-- id names it in CANCEL and BUY.
CREATE TABLE IF NOT EXISTS ledger (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    key_hash TEXT    NOT NULL,
    card     INTEGER NOT NULL,
    price    INTEGER NOT NULL,
    state    TEXT    NOT NULL,
    at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ledger_key ON ledger (key_hash);
-- the stock lists' secret, drawn with the store and never sent: a list cannot be predicted from outside
CREATE TABLE IF NOT EXISTS secret (one INTEGER PRIMARY KEY CHECK (one = 1), bytes BLOB NOT NULL);
-- each key's stock list of a server day (days since 1970-01-01 04:00 UTC), kept from its first view; sold: bought
CREATE TABLE IF NOT EXISTS stock (
    key_hash TEXT    NOT NULL,
    day      INTEGER NOT NULL,
    slot     INTEGER NOT NULL,             -- from 1
    card     INTEGER NOT NULL,
    price    INTEGER NOT NULL,
    sold     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (key_hash, day, slot)
);
-- the settings of a server day, from its first list: a restart's new settings start with the next day
CREATE TABLE IF NOT EXISTS stock_days (
    day   INTEGER PRIMARY KEY,
    mix   TEXT    NOT NULL,
    share INTEGER NOT NULL,
    lim   INTEGER NOT NULL
);
";

/// The market's server state: the value table, `market.db` and the uploads in flight per address.
pub struct Market {
    table: Table,
    db: Mutex<Connection>,
    uploads: Mutex<HashMap<String, usize>>,
    stock: Stock,
    /// The stock lists' secret (`market.db`'s `secret`).
    secret: Vec<u8>,
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

/// The (card, price) rows of `sql` for the key `kh`.
fn pairs(db: &Connection, sql: &str, kh: &str) -> Result<Vec<(u32, i64)>, String> {
    db.prepare(sql)
        .and_then(|mut s| {
            s.query_map(params![kh], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect()
        })
        .map_err(store_err)
}

/// The server's sales week at `now` (Unix ms): [`collection::week`] on UTC, so it turns on Monday at 05:00 UTC.
fn sales_week(now: u64) -> u32 {
    let at = i64::try_from(now / 1000)
        .ok()
        .and_then(|s| time::OffsetDateTime::from_unix_timestamp(s).ok())
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    collection::week(at)
}

const NOT_IN: &str = "this installation is not in the market";

impl Market {
    /// The market with the value table at `table` and its store at `db` (made when missing).
    pub fn open(table: &Path, db: &Path) -> Result<Self, String> {
        Self::with(Table::load(table)?, db)
    }

    /// The market with `table` (tests build theirs) and its store at `db`, with the default [`Stock`].
    pub fn with(table: Table, db: &Path) -> Result<Self, String> {
        Self::with_draw(table, db, |b| getrandom::fill(b).map_err(|e| e.to_string()))
    }

    /// [`Market::with`], `draw` filling a new store's secret; a store that holds one keeps it and draws nothing.
    fn with_draw(
        table: Table,
        db: &Path,
        draw: impl FnOnce(&mut [u8; 32]) -> Result<(), String>,
    ) -> Result<Self, String> {
        // the inner `Err`: no secret could be drawn (the store left as it was)
        type Opened = Result<(Connection, Vec<u8>), String>;
        let open = || -> rusqlite::Result<Opened> {
            let c = Connection::open(db)?;
            // secure_delete: a deleted log is overwritten in the file, not left in free space (PLAN.md 3.4)
            c.execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000; PRAGMA secure_delete = ON;",
            )?;
            // a store made before step 3.2 has ledger ids without AUTOINCREMENT, which reuse a deleted highest id: its
            // rows move to a new table, whose counter starts past the highest id kept (3.1 wrote no ledger rows)
            let old: Option<String> = c
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'ledger'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            let old = old.is_some_and(|sql| !sql.contains("AUTOINCREMENT"));
            // one transaction for every change below: a stop half way leaves the store as it was
            c.execute_batch("BEGIN")?;
            if old {
                c.execute_batch(
                    "DROP INDEX IF EXISTS ledger_key; ALTER TABLE ledger RENAME TO ledger_old;",
                )?;
            }
            c.execute_batch(SCHEMA)?;
            for column in ["asked", "answer"] {
                let has: i64 = c.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('keys') WHERE name = ?1",
                    [column],
                    |r| r.get(0),
                )?;
                if has == 0 {
                    c.execute_batch(&format!("ALTER TABLE keys ADD COLUMN {column} TEXT;"))?;
                }
            }
            if old {
                c.execute_batch(
                    "INSERT INTO ledger (id, key_hash, card, price, state, at)
                     SELECT id, key_hash, card, price, state, at FROM ledger_old;
                     DROP TABLE ledger_old;",
                )?;
            }
            let stored: Option<Vec<u8>> = c
                .query_row("SELECT bytes FROM secret", [], |r| r.get(0))
                .optional()?;
            let secret = match stored {
                Some(s) => s,
                None => {
                    let mut fresh = [0u8; 32];
                    if let Err(e) = draw(&mut fresh) {
                        return Ok(Err(format!("{}: no random secret ({e})", db.display())));
                    }
                    c.execute(
                        "INSERT INTO secret (one, bytes) VALUES (1, ?1)",
                        [&fresh[..]],
                    )?;
                    fresh.to_vec()
                }
            };
            c.execute_batch("COMMIT")?;
            Ok(Ok((c, secret)))
        };
        let (c, secret) = open().map_err(|e| format!("{}: {e}", db.display()))??;
        Ok(Self {
            table,
            db: Mutex::new(c),
            uploads: Mutex::new(HashMap::new()),
            stock: Stock::default(),
            secret,
        })
    }

    /// The market with the server's stock settings.
    #[must_use]
    pub fn with_stock(mut self, stock: Stock) -> Self {
        self.stock = stock;
        self
    }

    #[must_use]
    pub fn stock(&self) -> Stock {
        self.stock
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
        if req.verb.logged() && req.rules != self.table.rules {
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
                // last_at too: a COLLECT after LEAVE keeps its answer for a resend (one day of clock slack at most)
                "DELETE FROM keys WHERE left_at <= ?1 AND last_at <= ?1 AND NOT EXISTS
                 (SELECT 1 FROM ledger l WHERE l.key_hash = keys.key_hash AND l.state = 'sold')",
                params![now.saturating_sub(KEEP_LEFT_MS) as i64],
            )
            .map_err(store_err)?;
        if gone > 0 {
            // stock purchases stay, retired: the player's log keeps their `stock_buy` lines for a later JOIN
            for sql in [
                "UPDATE ledger SET state = 'stock_gone' WHERE state = 'stock' AND NOT EXISTS (SELECT 1 FROM keys k WHERE k.key_hash = ledger.key_hash)",
                "DELETE FROM ledger WHERE state <> 'stock_gone' AND NOT EXISTS (SELECT 1 FROM keys k WHERE k.key_hash = ledger.key_hash)",
                "DELETE FROM stock WHERE NOT EXISTS (SELECT 1 FROM keys k WHERE k.key_hash = stock.key_hash)",
            ] {
                db.execute(sql, []).map_err(store_err)?;
            }
        }
        let tx = db.transaction().map_err(store_err)?;
        type Stored = (i64, Option<String>, Option<i64>, Option<String>);
        let row: Option<(Stored, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT last_at, log, left_at, welcome, asked, answer FROM keys WHERE key_hash = ?1",
                params![kh],
                |r| {
                    Ok((
                        (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?),
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(store_err)?;
        let at = i64::try_from(req.at).map_err(|_| OLD.to_string())?;
        // a ledger word's verb and numbers: sent again at the same time (its answer lost on the way), it answers as the
        // first time and does nothing twice; JOIN, LEAVE and RESET answer OLD as before
        let asked = (!matches!(req.verb, Verb::Join | Verb::Leave | Verb::Reset)).then(|| {
            req.args
                .iter()
                .fold(req.verb.word().to_owned(), |s, a| format!("{s} {a}"))
        });
        let (row, first) = row.map_or((None, None), |(r, a, said)| (Some(r), a.zip(said)));
        if let Some((last, ..)) = &row
            && at <= *last
        {
            return match first {
                Some((a, said)) if at == *last && asked.as_ref() == Some(&a) => Ok(said),
                _ => Err(format!("{OLD}, the last at {last}")),
            };
        }
        let joined = row.as_ref().is_some_and(|(_, _, left, _)| left.is_none());
        // COLLECT also after LEAVE, while the key's row waits for it
        if !(req.verb == Verb::Join || joined || req.verb == Verb::Collect && row.is_some()) {
            return Err(NOT_IN.into());
        }
        // JOIN, LIST, BUY, STOCK, STOCKBUY and OWED: the log is checked against the table and the key's last log, then kept
        let mut checked = Checked::default();
        if req.verb.logged() {
            let stock = pairs(
                &tx,
                "SELECT card, price FROM ledger WHERE key_hash = ?1 AND state IN ('stock', 'stock_gone')",
                &kh,
            )?;
            checked = check(&req.log, &self.table, &stock)?;
            let lines = &checked.lines;
            let old: Vec<&str> = row
                .as_ref()
                .and_then(|(_, log, ..)| log.as_deref())
                .map(|l| l.lines().filter(|l| !l.is_empty()).collect())
                .unwrap_or_default();
            let kept = old.iter().zip(lines).take_while(|(a, b)| a == b).count();
            if kept < old.len() {
                tx.execute(
                    "UPDATE keys SET forks = forks + 1, last_at = ?2, asked = NULL, answer = NULL WHERE key_hash = ?1",
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
            if let (Some(new), Some((_, log, left, Some(had)))) = (&welcome, &row) {
                // after RESET the log is gone and the key is not left: only the mark of the first welcome remains, so
                // not even that line comes back; after LEAVE the same log may
                let reset = log.is_none() && left.is_none();
                if reset || new != had {
                    return Err(
                        "a second welcome: this installation had its welcome tokens before".into(),
                    );
                }
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
        }
        let num = |i: usize| req.args.get(i).copied().unwrap_or_default();
        let fresh = (now as i64).saturating_sub(LISTING_MS as i64);
        let answer = match req.verb {
            Verb::Join => format!("{} lines", checked.lines.len()),
            Verb::List => self.list(&tx, &kh, &checked, num(0), num(1), now)?,
            Verb::Cancel => {
                let id = i64::try_from(num(0)).unwrap_or(-1);
                let r: Option<(u32, String)> = tx
                    .query_row(
                        "SELECT card, state FROM ledger WHERE id = ?1 AND key_hash = ?2
                         AND state IN ('open', 'sold', 'paid')",
                        params![id, kh],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(store_err)?;
                match r {
                    // conditional, as BUY's sale: a listing sold since the SELECT stays sold
                    Some((c, state)) if state == "open" => {
                        if tx
                            .execute(
                                "DELETE FROM ledger WHERE id = ?1 AND state = 'open'",
                                params![id],
                            )
                            .map_err(store_err)?
                            != 1
                        {
                            return Err(SOLD.into());
                        }
                        format!("cancelled {id}:{c}")
                    }
                    Some(_) => return Err(SOLD.into()),
                    None => return Err("this installation has no listing with that id".into()),
                }
            }
            Verb::Browse => {
                let after = i64::try_from(num(0)).unwrap_or(i64::MAX);
                // the other keys' live listings, none of a seller at the week's sales cap
                let rows: Vec<(i64, u32, i64)> = tx
                    .prepare(
                        "SELECT l.id, l.card, l.price FROM ledger l JOIN keys k ON k.key_hash = l.key_hash
                         WHERE l.state = 'open' AND l.at >= ?1 AND l.id > ?2 AND l.key_hash <> ?3
                         AND NOT (k.sales_week = ?4 AND k.sales >= ?5)
                         ORDER BY l.id LIMIT ?6",
                    )
                    .and_then(|mut s| {
                        s.query_map(
                            params![
                                fresh,
                                after,
                                kh,
                                sales_week(now),
                                SALES_PER_WEEK,
                                BROWSE_PAGE as i64
                            ],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                        )?
                        .collect()
                    })
                    .map_err(store_err)?;
                let mut said = "listings".to_owned();
                for (id, c, p) in rows {
                    said.push_str(&format!(" {id}:{c}:{p}"));
                }
                said
            }
            Verb::Buy => self.buy(&tx, &kh, &checked, [num(0), num(1), num(2)], now)?,
            Verb::Stock => {
                let (_, s, rows) = self.today(&tx, &kh, now)?;
                let mut said = "stock".to_owned();
                if s.limit > 0 {
                    let sold = rows.iter().filter(|r| r.3).count() as u32;
                    said.push_str(&format!(" left {}", s.limit.saturating_sub(sold)));
                }
                for (slot, c, p, sold) in rows {
                    said.push_str(&format!(
                        " {slot}:{c}:{p}{}",
                        if sold { ":sold" } else { "" }
                    ));
                }
                said
            }
            Verb::StockBuy => self.stock_buy(&tx, &kh, &checked, [num(0), num(1), num(2)], now)?,
            Verb::Owed => {
                let mut said = "owed".to_owned();
                let mut last = "";
                for (word, o) in owed(&tx, &kh, &checked)?.into_iter().take(COLLECT_PAGE) {
                    if word != last {
                        said.push(' ');
                        said.push_str(word);
                        last = word;
                    }
                    said.push_str(&format!(" {}:{}:{}", o.id, o.card, o.price));
                }
                said
            }
            Verb::Collect => {
                let rows: Vec<(i64, u32, i64, String)> = tx
                    .prepare(
                        "SELECT id, card, price, state FROM ledger WHERE key_hash = ?1
                         AND (state = 'sold' OR state = 'open' AND at < ?2) ORDER BY id LIMIT ?3",
                    )
                    .and_then(|mut s| {
                        s.query_map(params![kh, fresh, COLLECT_PAGE as i64], |r| {
                            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                        })?
                        .collect()
                    })
                    .map_err(store_err)?;
                let (mut sold, mut returned) = (String::new(), String::new());
                for (id, c, p, state) in rows {
                    if state == "sold" {
                        tx.execute(
                            "UPDATE ledger SET state = 'paid' WHERE id = ?1",
                            params![id],
                        )
                        .map_err(store_err)?;
                        sold.push_str(&format!(" {id}:{c}:{p}"));
                    } else {
                        tx.execute("DELETE FROM ledger WHERE id = ?1", params![id])
                            .map_err(store_err)?;
                        returned.push_str(&format!(" {id}:{c}"));
                    }
                }
                if !joined {
                    // after LEAVE a paid sale's row goes at once (PLAN.md 3.4 privacy)
                    tx.execute(
                        "DELETE FROM ledger WHERE key_hash = ?1 AND state = 'paid'",
                        params![kh],
                    )
                    .map_err(store_err)?;
                }
                let mut said = "collected".to_owned();
                for (word, rows) in [(" sold", sold), (" returned", returned)] {
                    if !rows.is_empty() {
                        said.push_str(word);
                        said.push_str(&rows);
                    }
                }
                said
            }
            Verb::Reset => {
                // the listings close and the waiting proceeds go with the log; the day's stock slots stay sold
                tx.execute(
                    "UPDATE keys SET log = NULL, last_at = ?2 WHERE key_hash = ?1",
                    params![kh, at],
                )
                .map_err(store_err)?;
                for sql in [
                    "DELETE FROM ledger WHERE key_hash = ?1 AND state IN ('open', 'sold', 'paid', 'bought')",
                    "UPDATE ledger SET state = 'stock_reset' WHERE key_hash = ?1 AND state IN ('stock', 'stock_gone')",
                ] {
                    tx.execute(sql, params![kh]).map_err(store_err)?;
                }
                "reset".into()
            }
            Verb::Leave => {
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
        tx.execute(
            "UPDATE keys SET last_at = ?2, asked = ?3, answer = ?4 WHERE key_hash = ?1",
            params![kh, at, asked, asked.as_ref().map(|_| &answer)],
        )
        .map_err(store_err)?;
        tx.commit().map_err(store_err)?;
        if matches!(req.verb, Verb::Leave | Verb::Reset) {
            // the deleted log leaves the write-ahead log too, not only the table
            db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
                .map_err(store_err)?;
        }
        Ok(answer)
    }

    /// LIST of `card` at `price` by the key `kh`, whose log passed as `checked`: the band from the server's table, the
    /// open-listing cap, and an earned copy that neither the log (`list` lines) nor the ledger (open or sold, the
    /// proceeds not collected) already has on the market.
    fn list(
        &self,
        tx: &Connection,
        kh: &str,
        checked: &Checked<'_>,
        card: u64,
        price: u64,
        now: u64,
    ) -> Result<String, String> {
        let not_in = || format!("card {card} is not in this server's table");
        let c = u32::try_from(card).map_err(|_| not_in())?;
        let value = self.table.value(c).ok_or_else(not_in)?;
        let (lo, hi) = (
            collection::floor_value(value),
            collection::ceiling_value(value),
        );
        if !(u64::from(lo)..=u64::from(hi)).contains(&price) {
            return Err(format!(
                "a price of {price} for card {c}: its band is {lo} to {hi} tokens"
            ));
        }
        let fresh = (now as i64).saturating_sub(LISTING_MS as i64);
        let open: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM ledger WHERE key_hash = ?1 AND state = 'open' AND at >= ?2",
                params![kh, fresh],
                |r| r.get(0),
            )
            .map_err(store_err)?;
        if open >= OPEN_LISTINGS {
            return Err(format!(
                "{OPEN_LISTINGS} listings are open: one must sell, come back or be cancelled first"
            ));
        }
        let out: u32 = tx
            .query_row(
                "SELECT COUNT(*) FROM ledger WHERE key_hash = ?1 AND card = ?2 AND state IN ('open', 'sold')",
                params![kh, c],
                |r| r.get(0),
            )
            .map_err(store_err)?;
        let h = checked.held.get(&c).copied().unwrap_or_default();
        let listed = h.listed.max(out);
        if h.earned <= listed {
            return Err(if h.earned == 0 && h.starter + h.imported > 0 {
                format!("card {c} is held only as a starter or imported copy, which never lists")
            } else {
                format!("no earned copy of card {c} is left off the market")
            });
        }
        tx.execute(
            "INSERT INTO ledger (key_hash, card, price, state, at) VALUES (?1, ?2, ?3, 'open', ?4)",
            params![kh, c, price as i64, now as i64],
        )
        .map_err(store_err)?;
        Ok(format!("listed {}:{c}:{price}", tx.last_insert_rowid()))
    }

    /// BUY of the listing `id` of `card` at `price` by the key `kh`, whose log passed as `checked`: the listing still
    /// open, live and another key's, its seller under the week's sales cap, and the log's balance, less this key's
    /// purchases the log does not hold yet, enough for it.
    fn buy(
        &self,
        tx: &Connection,
        kh: &str,
        checked: &Checked<'_>,
        [id, card, price]: [u64; 3],
        now: u64,
    ) -> Result<String, String> {
        let id = i64::try_from(id).map_err(|_| GONE.to_owned())?;
        let fresh = (now as i64).saturating_sub(LISTING_MS as i64);
        let r: Option<(String, u32, i64)> = tx
            .query_row(
                "SELECT key_hash, card, price FROM ledger WHERE id = ?1 AND state = 'open' AND at >= ?2",
                params![id, fresh],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(store_err)?;
        let Some((seller, c, p)) =
            r.filter(|(_, c, p)| u64::from(*c) == card && u64::try_from(*p) == Ok(price))
        else {
            return Err(GONE.into());
        };
        if seller == kh {
            return Err("that is this installation's own listing: CANCEL takes it back".into());
        }
        let week = sales_week(now);
        let (sales, sold_week): (u32, u32) = tx
            .query_row(
                "SELECT sales, sales_week FROM keys WHERE key_hash = ?1",
                params![seller],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(store_err)?;
        if sold_week == week && sales >= SALES_PER_WEEK {
            return Err(format!(
                "that listing's seller sold {SALES_PER_WEEK} cards this week: it sells again next week"
            ));
        }
        // a purchase the log lacks (its `market_buy` or `stock_buy` line not written yet) still spends its tokens
        let tokens = spendable(tx, kh, checked)?;
        if tokens < p {
            return Err(format!("{tokens} tokens to spend, the listing costs {p}"));
        }
        // conditional, so a second buyer finds it sold even outside the store's lock
        if tx
            .execute(
                "UPDATE ledger SET state = 'sold' WHERE id = ?1 AND state = 'open'",
                params![id],
            )
            .map_err(store_err)?
            != 1
        {
            return Err(GONE.into());
        }
        tx.execute(
            "UPDATE keys SET sales = CASE WHEN sales_week = ?2 THEN sales + 1 ELSE 1 END, sales_week = ?2
             WHERE key_hash = ?1",
            params![seller, week],
        )
        .map_err(store_err)?;
        tx.execute(
            "INSERT INTO ledger (key_hash, card, price, state, at) VALUES (?1, ?2, ?3, 'bought', ?4)",
            params![kh, c, p, now as i64],
        )
        .map_err(store_err)?;
        Ok(format!("bought {id}:{c}:{p}"))
    }

    /// The key `kh`'s stock list of the server day at `now`: the day, its settings and the rows (slot, card, price,
    /// sold), drawn and kept on the day's first view. Earlier days' lists go.
    #[allow(clippy::type_complexity)]
    fn today(
        &self,
        tx: &Connection,
        kh: &str,
        now: u64,
    ) -> Result<(i64, Stock, Vec<(u32, u32, i64, bool)>), String> {
        let day = stock_day(now);
        for sql in [
            "DELETE FROM stock WHERE day < ?1",
            "DELETE FROM stock_days WHERE day < ?1",
        ] {
            tx.execute(sql, params![day]).map_err(store_err)?;
        }
        let s = self.stock;
        tx.execute(
            "INSERT OR IGNORE INTO stock_days (day, mix, share, lim) VALUES (?1, ?2, ?3, ?4)",
            params![day, s.mix_text(), s.share, s.limit],
        )
        .map_err(store_err)?;
        let (mix, share, limit): (String, u32, u32) = tx
            .query_row(
                "SELECT mix, share, lim FROM stock_days WHERE day = ?1",
                params![day],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(store_err)?;
        let s = Stock::new(&mix, share, limit)?;
        let read = || -> Result<Vec<(u32, u32, i64, bool)>, String> {
            tx.prepare(
                "SELECT slot, card, price, sold FROM stock WHERE key_hash = ?1 AND day = ?2 ORDER BY slot",
            )
            .and_then(|mut st| {
                st.query_map(params![kh, day], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect()
            })
            .map_err(store_err)
        };
        let rows = read()?;
        if !rows.is_empty() {
            return Ok((day, s, rows));
        }
        for (slot, (c, p)) in (1u32..).zip(self.draw(kh, day, &s)) {
            tx.execute(
                "INSERT INTO stock (key_hash, day, slot, card, price) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![kh, day, slot, c, p],
            )
            .map_err(store_err)?;
        }
        Ok((day, s, read()?))
    }

    /// The stock list of the key `kh` on `day` under `s`: a seed from the store's secret, the key and the day, then per
    /// tier group its cards of the table drawn plain random, none twice, each at `s.share` % of its value.
    fn draw(&self, kh: &str, day: i64, s: &Stock) -> Vec<(u32, i64)> {
        let seed = Sha256::new()
            .chain_update(&self.secret)
            .chain_update(kh)
            .chain_update(day.to_le_bytes())
            .finalize();
        let seed = seed
            .get(..8)
            .and_then(|b| <[u8; 8]>::try_from(b).ok())
            .map_or(0, u64::from_le_bytes);
        let mut rng = collection::Rng::new(seed);
        let mut rows = vec![];
        for (group, n) in STOCK_GROUPS.iter().zip(s.mix) {
            let mut pool: Vec<u32> = self
                .table
                .cards
                .iter()
                .filter(|(_, r)| group.contains(&r.rarity))
                .map(|(c, _)| *c)
                .collect();
            pool.sort_unstable();
            for _ in 0..n {
                if pool.is_empty() {
                    break;
                }
                let c = pool.swap_remove(rng.below(pool.len() as u64) as usize);
                let value = self.table.value(c).unwrap_or_default();
                rows.push((c, i64::from(value) * i64::from(s.share) / 100));
            }
        }
        rows
    }

    /// STOCKBUY of today's `slot` of `card` at `price` by the key `kh`, whose log passed as `checked`: the slot as the
    /// list holds it, not sold, the day's limit not reached, and the tokens [`spendable`]; the slot sells once (a
    /// conditional update) and a `stock` ledger row waits for the log's `stock_buy` line.
    fn stock_buy(
        &self,
        tx: &Connection,
        kh: &str,
        checked: &Checked<'_>,
        [slot, card, price]: [u64; 3],
        now: u64,
    ) -> Result<String, String> {
        let (day, s, rows) = self.today(tx, kh, now)?;
        let Some(&(slot, c, p, sold)) = rows.iter().find(|(n, c, p, _)| {
            u64::from(*n) == slot && u64::from(*c) == card && u64::try_from(*p) == Ok(price)
        }) else {
            return Err(format!(
                "today's stock list has no slot {slot} of card {card} at {price}: the list turns at 04:00 UTC"
            ));
        };
        if sold {
            return Err(STOCK_SOLD.into());
        }
        let bought = rows.iter().filter(|r| r.3).count() as u32;
        if s.limit > 0 && bought >= s.limit {
            return Err(format!(
                "{bought} stock purchases today, this server's limit: the list turns at 04:00 UTC"
            ));
        }
        let tokens = spendable(tx, kh, checked)?;
        if tokens < p {
            return Err(format!("{tokens} tokens to spend, the slot costs {p}"));
        }
        // conditional, as BUY's sale: a slot sold since the read stays sold once
        if tx
            .execute(
                "UPDATE stock SET sold = 1 WHERE key_hash = ?1 AND day = ?2 AND slot = ?3 AND sold = 0",
                params![kh, day, slot],
            )
            .map_err(store_err)?
            != 1
        {
            return Err(STOCK_SOLD.into());
        }
        tx.execute(
            "INSERT INTO ledger (key_hash, card, price, state, at) VALUES (?1, ?2, ?3, 'stock', ?4)",
            params![kh, c, p, now as i64],
        )
        .map_err(store_err)?;
        Ok(format!("stockbought {slot}:{c}:{p}"))
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

    /// Cards 1-40 REGULAR (TOTAL 80, value 40), 41-50 SPECIAL (90: 400), 51-55 RARE (85: 600), 56 LEGEND (95: 6,000),
    /// 57 SPECIAL (87: 240).
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
        cards.push((57, Rarity::Special, 87));
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
    fn matches_closer_than_the_gap_are_refused() {
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
            why.contains("line 15") && why.contains("3 minutes apart"),
            "{why}"
        );
    }

    /// M2: two honest matches [`MATCH_GAP`] apart pass; one second closer is refused. The gap is 3 minutes because
    /// `GAME-CLOCK.md` gives the menus around a match (MATCHING 5 s, a 5 s result screen in one mode) but no lower
    /// bound on the game itself.
    #[test]
    fn two_honest_matches_at_the_gap_pass() {
        let at = t("2026-11-01T12:00:00+09:00");
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 50)
            .add(at + MATCH_GAP, Kind::Match, Some(20), 50);
        assert_eq!(passes(&l), 15);
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 50).add(
            at + MATCH_GAP - time::Duration::seconds(1),
            Kind::Match,
            Some(20),
            50,
        );
        let why = refused(&l);
        assert!(
            why.contains("line 15") && why.contains("179 s after the one before"),
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
                && why.contains("the week 202645 holds 1050 of its 1050: this one pays 0"),
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

    /// M1: a week holds [`MATCHES_PER_WEEK`] `match` lines; an honest week of them passes and the one past them is
    /// refused, card or not.
    #[test]
    fn match_lines_over_the_weeks_count_are_refused() {
        let at = t("2026-11-03T10:00:00+09:00");
        let mut l = started();
        for i in 0..i64::from(MATCHES_PER_WEEK) {
            l.add(
                at + mins(3 * i),
                Kind::Match,
                Some(20),
                if i < 21 { 50 } else { 0 },
            );
        }
        assert_eq!(passes(&l), 13 + MATCHES_PER_WEEK as usize);
        l.add(
            at + mins(3 * i64::from(MATCHES_PER_WEEK)),
            Kind::Match,
            Some(21),
            0,
        );
        let why = refused(&l);
        assert!(
            why.contains("line 134") && why.contains("match 121 in the week 202645"),
            "{why}"
        );
    }

    /// M1: a `match` line pays what the week's counter says, so it cannot underpay to stay inside the cap.
    #[test]
    fn an_underpaid_match_is_refused() {
        let at = t("2026-11-03T10:00:00+09:00");
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 50)
            .add(at + mins(3), Kind::Match, Some(20), 49);
        let why = refused(&l);
        assert!(
            why.contains("line 15")
                && why.contains("the week 202645 holds 50 of its 1050: this one pays 50"),
            "{why}"
        );
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 0);
        assert!(refused(&l).contains("this one pays 50"));
    }

    /// N1: `i64::MIN` tokens has no positive spend; a purchase, a stock purchase or a pack of them is refused with the
    /// reason, never an overflow panic.
    #[test]
    fn the_lowest_tokens_is_not_a_price() {
        let at = t("2026-11-01T12:00:00+09:00");
        for (kind, card) in [
            (Kind::MarketBuy, Some(20)),
            (Kind::StockBuy, Some(20)),
            (Kind::Pack, None),
        ] {
            let mut l = started();
            l.add(at, kind, card, i64::MIN);
            let why = refused(&l);
            assert!(
                why.starts_with("line 14")
                    && why.contains("-9223372036854775808 tokens is not a price"),
                "{kind:?}: {why}"
            );
        }
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
        l.add(t("2026-11-01T12:00:00+09:00"), Kind::Match, Some(20), 50)
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
        l.add(at, Kind::Match, Some(1), 50)
            .add(at + mins(8), Kind::Convert, Some(1), 8);
        assert!(refused(&l).contains("last unlisted earned copy"));
    }

    #[test]
    fn prices_chain_and_balance_are_checked() {
        let at = t("2026-11-01T12:00:00+09:00");
        // a craft below the table's price
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 50)
            .add(at, Kind::Craft, Some(21), -30);
        assert!(refused(&l).contains("the table says -40"));
        // a pack for a price no pack has, and a pack card without a paid pack
        let mut l = started();
        l.add(at, Kind::Match, Some(20), 50)
            .add(at, Kind::Pack, None, -10);
        assert!(refused(&l).contains("no pack costs that"));
        let mut l = started();
        l.add(at, Kind::Pack, Some(20), 0);
        assert!(refused(&l).contains("without a paid pack"));
        // a pack bought on credit: the welcome's 150 is not its 200
        let mut l = started();
        l.add(at, Kind::Pack, None, -200);
        assert!(refused(&l).contains("below zero"));
        // a card the server does not know, a broken chain, a skipped seq
        let mut l = started();
        l.add(at, Kind::Match, Some(999), 50);
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
        l.add(at, Kind::Match, Some(20), 50)
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
            args: Vec::new(),
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
            .execute("UPDATE keys SET sales = 3", [])
            .unwrap();
        // a rewritten history: the starter cards and a match, no welcome (the key keeps its mark)
        let other = {
            let mut l = Log::default();
            for c in (1..=11).chain([51]) {
                l.add(t(START), Kind::Starter, Some(c), 0);
            }
            l.add(t("2026-11-01T12:00:00+09:00"), Kind::Match, Some(40), 50);
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
        assert_eq!(row(dir.path(), "SELECT sales FROM keys"), [[s("3")]]);
        // a request sent again is refused, with the time to go past
        assert_eq!(
            m.handle(&req(Verb::Join, NOW + 4, &other), NOW)
                .unwrap_err(),
            format!("{OLD}, the last at {}", NOW + 4)
        );
    }

    /// `verb` with `args` from the key `[k; 32]` at `at`, the server's clock at `at` too.
    fn ask(
        m: &Market,
        k: u8,
        verb: Verb,
        at: u64,
        args: &[u64],
        log: &str,
    ) -> Result<String, String> {
        let r = Request {
            verb,
            key: [k; 32],
            at,
            rules: RULES,
            args: args.to_vec(),
            log: log.into(),
        };
        m.handle(&r, at)
    }

    /// The first start and one paid match per card of `cards`, 9 minutes apart: an earned copy each, 150 tokens and 50
    /// a match.
    fn earned(cards: &[u32]) -> String {
        earned_from(START, cards)
    }

    /// [`earned`] with the first start at `start`: another installation's log.
    fn earned_from(start: &str, cards: &[u32]) -> String {
        let mut l = started_at(start);
        for (i, c) in cards.iter().enumerate() {
            let at = t("2026-11-01T12:00:00+09:00") + mins(9 * i as i64);
            l.add(at, Kind::Match, Some(*c), 50);
        }
        l.text
    }

    /// Acceptance 3.2 (a): for a 240 card (a black at TOTAL 87), 191 and 229 are refused, 192 and 228 list; a third
    /// listing of its two earned copies is refused while their `list` lines are not written yet.
    #[test]
    fn a_listing_is_priced_inside_the_band() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let log = earned(&[57, 57]);
        assert_eq!(table().value(57), Some(240));
        ask(&m, 1, Verb::Join, NOW, &[], &log).unwrap();
        for (at, price) in [(NOW + 1, 191), (NOW + 2, 229)] {
            let why = ask(&m, 1, Verb::List, at, &[57, price], &log).unwrap_err();
            assert_eq!(
                why,
                format!("a price of {price} for card 57: its band is 192 to 228 tokens")
            );
        }
        assert_eq!(
            ask(&m, 1, Verb::List, NOW + 3, &[57, 192], &log).unwrap(),
            "listed 1:57:192"
        );
        assert_eq!(
            ask(&m, 1, Verb::List, NOW + 4, &[57, 228], &log).unwrap(),
            "listed 2:57:228"
        );
        let why = ask(&m, 1, Verb::List, NOW + 5, &[57, 200], &log).unwrap_err();
        assert!(why.starts_with("no earned copy of card 57"), "{why}");
    }

    /// Acceptance 3.2 (b): two buyers race for one listing from two threads: exactly one buys it, the other finds it
    /// gone, and the seller has one sale.
    #[test]
    fn two_buyers_race_and_one_wins() {
        let dir = tempfile::tempdir().unwrap();
        let m = std::sync::Arc::new(market(dir.path()));
        let seller = earned(&[57]);
        ask(&m, 1, Verb::Join, NOW, &[], &seller).unwrap();
        ask(&m, 1, Verb::List, NOW + 1, &[57, 228], &seller).unwrap();
        let funds = earned(&[20, 20]);
        for k in [2, 3] {
            ask(&m, k, Verb::Join, NOW + 2, &[], &funds).unwrap();
        }
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let racers = [2u8, 3].map(|k| {
            let (m, gate, funds) = (m.clone(), gate.clone(), funds.clone());
            std::thread::spawn(move || {
                gate.wait();
                ask(&m, k, Verb::Buy, NOW + 3, &[1, 57, 228], &funds)
            })
        });
        let answers: Vec<_> = racers.into_iter().map(|h| h.join().unwrap()).collect();
        let won: Vec<_> = answers.iter().filter_map(|a| a.as_ref().ok()).collect();
        assert_eq!(won, ["bought 1:57:228"], "{answers:?}");
        assert!(answers.contains(&Err(GONE.into())), "{answers:?}");
        assert_eq!(
            row(
                dir.path(),
                "SELECT state, COUNT(*) FROM ledger GROUP BY state ORDER BY state"
            ),
            [[s("bought"), s("1")], [s("sold"), s("1")]]
        );
        assert_eq!(
            row(
                dir.path(),
                &format!(
                    "SELECT sales FROM keys WHERE key_hash = '{}'",
                    key_hash(&[1; 32])
                )
            ),
            [[s("1")]]
        );
    }

    /// M1: the answers to LIST, BUY and COLLECT are lost and each request is sent again as it was: it answers as the
    /// first time and does nothing twice (one listing, one purchase and one sale, one payout); a later request is new.
    #[test]
    fn a_ledger_word_sent_again_answers_the_same_once() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let seller = earned(&[57, 57]);
        let funds = earned(&[20, 20]);
        ask(&m, 1, Verb::Join, NOW, &[], &seller).unwrap();
        ask(&m, 2, Verb::Join, NOW, &[], &funds).unwrap();
        let twice = |k, verb, at, args: &[u64], log: &str| {
            let first = ask(&m, k, verb, at, args, log);
            assert_eq!(ask(&m, k, verb, at, args, log), first, "{verb:?}");
            first.unwrap()
        };
        assert_eq!(
            twice(1, Verb::List, NOW + 1, &[57, 228], &seller),
            "listed 1:57:228"
        );
        assert_eq!(
            twice(2, Verb::Buy, NOW + 2, &[1, 57, 228], &funds),
            "bought 1:57:228"
        );
        assert_eq!(
            twice(1, Verb::Collect, NOW + 3, &[], ""),
            "collected sold 1:57:228"
        );
        let ledger = "SELECT key_hash = ?1, card, price, state FROM ledger ORDER BY id";
        let ledger = ledger.replace("?1", &format!("'{}'", key_hash(&[1; 32])));
        assert_eq!(
            row(dir.path(), &ledger),
            [
                [s("1"), s("57"), s("228"), s("paid")],
                [s("0"), s("57"), s("228"), s("bought")]
            ]
        );
        let sales = format!(
            "SELECT sales FROM keys WHERE key_hash = '{}'",
            key_hash(&[1; 32])
        );
        assert_eq!(row(dir.path(), &sales), [[s("1")]]);
        assert_eq!(
            ask(&m, 1, Verb::Collect, NOW + 4, &[], "").unwrap(),
            "collected"
        );
        // the buyer's 250 tokens hold the one purchase once, its line not written yet
        assert_eq!(
            ask(&m, 1, Verb::List, NOW + 5, &[57, 192], &seller).unwrap(),
            "listed 3:57:192"
        );
        assert_eq!(
            ask(&m, 2, Verb::Buy, NOW + 6, &[3, 57, 192], &funds).unwrap_err(),
            "22 tokens to spend, the listing costs 192"
        );
        // a request sent again after a later one is old, and another word at the same time is not the same request
        let why = ask(&m, 1, Verb::Collect, NOW + 3, &[], "").unwrap_err();
        assert!(why.starts_with(OLD), "{why}");
        let why = ask(&m, 1, Verb::Browse, NOW + 5, &[0], "").unwrap_err();
        assert!(why.starts_with(OLD), "{why}");
    }

    /// m1: the seller's CANCEL races a BUY of the same listing: exactly one goes through, and the ledger agrees with it
    /// (cancelled: no row and no purchase; sold: CANCEL says so and the sale stands).
    #[test]
    fn cancel_races_a_buy_and_one_wins() {
        let dir = tempfile::tempdir().unwrap();
        let m = std::sync::Arc::new(market(dir.path()));
        let seller = earned(&[57]);
        let funds = earned(&[20, 20]);
        ask(&m, 1, Verb::Join, NOW, &[], &seller).unwrap();
        ask(&m, 1, Verb::List, NOW + 1, &[57, 228], &seller).unwrap();
        ask(&m, 2, Verb::Join, NOW + 2, &[], &funds).unwrap();
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let racers = [
            (1u8, Verb::Cancel, vec![1]),
            (2, Verb::Buy, vec![1, 57, 228]),
        ]
        .map(|(k, verb, args)| {
            let (m, gate) = (m.clone(), gate.clone());
            let log = if k == 2 { funds.clone() } else { String::new() };
            std::thread::spawn(move || {
                gate.wait();
                ask(&m, k, verb, NOW + 3, &args, &log)
            })
        });
        let [cancel, buy] = racers.map(|h| h.join().unwrap());
        let states = row(dir.path(), "SELECT state FROM ledger ORDER BY id");
        match (&cancel, &buy) {
            (Ok(c), Err(b)) => {
                assert_eq!((c.as_str(), b.as_str()), ("cancelled 1:57", GONE));
                assert!(states.is_empty(), "{states:?}");
            }
            (Err(c), Ok(b)) => {
                assert_eq!((c.as_str(), b.as_str()), (SOLD, "bought 1:57:228"));
                assert_eq!(states, [[s("sold")], [s("bought")]]);
            }
            _ => panic!("{cancel:?} {buy:?}"),
        }
    }

    /// m2: a store made by step 3.1 (ledger ids without AUTOINCREMENT) never gives a deleted highest id again: its
    /// rows keep their ids, and a LIST after the highest row is cancelled takes the next one.
    #[test]
    fn a_store_from_before_the_ledger_never_reuses_an_id() {
        let dir = tempfile::tempdir().unwrap();
        Connection::open(dir.path().join("market.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE keys (key_hash TEXT PRIMARY KEY, log TEXT, last_at INTEGER NOT NULL, welcome TEXT,
                     listings INTEGER NOT NULL DEFAULT 0, sales INTEGER NOT NULL DEFAULT 0,
                     sales_week INTEGER NOT NULL DEFAULT 0, forks INTEGER NOT NULL DEFAULT 0, left_at INTEGER);
                 CREATE TABLE ledger (id INTEGER PRIMARY KEY, key_hash TEXT NOT NULL, card INTEGER NOT NULL,
                     price INTEGER NOT NULL, state TEXT NOT NULL, at INTEGER NOT NULL);
                 CREATE INDEX ledger_key ON ledger (key_hash);
                 INSERT INTO ledger VALUES (1, 'x', 45, 360, 'stock', 0);",
            )
            .unwrap();
        let m = market(dir.path());
        let seller = earned(&[57, 57]);
        ask(&m, 1, Verb::Join, NOW, &[], &seller).unwrap();
        assert_eq!(
            ask(&m, 1, Verb::List, NOW + 1, &[57, 228], &seller).unwrap(),
            "listed 2:57:228"
        );
        assert_eq!(
            ask(&m, 1, Verb::Cancel, NOW + 2, &[2], "").unwrap(),
            "cancelled 2:57"
        );
        assert_eq!(
            ask(&m, 1, Verb::List, NOW + 3, &[57, 228], &seller).unwrap(),
            "listed 3:57:228"
        );
        // and after a restart
        drop(m);
        let m = market(dir.path());
        ask(&m, 1, Verb::Cancel, NOW + 4, &[3], "").unwrap();
        assert_eq!(
            ask(&m, 1, Verb::List, NOW + 5, &[57, 228], &seller).unwrap(),
            "listed 4:57:228"
        );
        assert_eq!(
            row(dir.path(), "SELECT id, state FROM ledger ORDER BY id"),
            [[s("1"), s("stock")], [s("4"), s("open")]]
        );
    }

    /// Advisor 2: a LIST refused as a fork moves the key's last time, and the same request sent again is old, not the
    /// earlier LIST's `OK`.
    #[test]
    fn a_fork_refused_repeat_is_old() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let seller = earned(&[57, 57]);
        ask(&m, 1, Verb::Join, NOW, &[], &seller).unwrap();
        assert_eq!(
            ask(&m, 1, Verb::List, NOW + 1, &[57, 228], &seller).unwrap(),
            "listed 1:57:228"
        );
        // the same words over a log missing its last accepted line
        let forked = earned(&[57]);
        let why = ask(&m, 1, Verb::List, NOW + 2, &[57, 228], &forked).unwrap_err();
        assert!(why.starts_with("a fork"), "{why}");
        let again = ask(&m, 1, Verb::List, NOW + 2, &[57, 228], &forked);
        assert_eq!(again, Err(format!("{OLD}, the last at {}", NOW + 2)));
        assert_eq!(row(dir.path(), "SELECT COUNT(*) FROM ledger"), [[s("1")]]);
    }

    /// Advisor 3, 6: a COLLECT after a real LEAVE pays the waiting sale once; its answer lost and the COLLECT sent again
    /// once LEAVE is 7 days old (still inside the clock slack of the COLLECT), it answers the same and pays nothing more.
    #[test]
    fn a_collect_after_leave_sent_again_past_7_days_pays_once() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let seller = earned(&[57]);
        let funds = earned(&[20, 20]);
        ask(&m, 1, Verb::Join, NOW, &[], &seller).unwrap();
        ask(&m, 1, Verb::List, NOW + 1, &[57, 228], &seller).unwrap();
        ask(&m, 2, Verb::Join, NOW + 1, &[], &funds).unwrap();
        ask(&m, 2, Verb::Buy, NOW + 2, &[1, 57, 228], &funds).unwrap();
        assert_eq!(
            ask(&m, 1, Verb::Leave, NOW + 3, &[], "").unwrap(),
            "left waiting 1"
        );
        let collect = NOW + 3 + KEEP_LEFT_MS - 1000;
        assert_eq!(
            ask(&m, 1, Verb::Collect, collect, &[], "").unwrap(),
            "collected sold 1:57:228"
        );
        let mut again = req(Verb::Collect, collect, "");
        again.key = [1; 32];
        let past = NOW + 3 + KEEP_LEFT_MS + 1000;
        assert_eq!(m.handle(&again, past).unwrap(), "collected sold 1:57:228");
        assert_eq!(
            row(dir.path(), "SELECT state FROM ledger ORDER BY id"),
            [[s("bought")]]
        );
        let seller_rows = format!(
            "SELECT COUNT(*) FROM keys WHERE key_hash = '{}'",
            key_hash(&[1; 32])
        );
        assert_eq!(row(dir.path(), &seller_rows), [[s("1")]]);
        // once the COLLECT is 7 days old too, the key goes
        let gone = collect + KEEP_LEFT_MS + 1;
        assert_eq!(
            ask(&m, 1, Verb::Collect, gone, &[], "").unwrap_err(),
            NOT_IN
        );
        assert_eq!(row(dir.path(), &seller_rows), [[s("0")]]);
    }

    /// Advisor 4: a store stopped between the two column additions (only `asked` added) opens and answers a resend.
    #[test]
    fn a_store_with_one_answer_column_opens() {
        let dir = tempfile::tempdir().unwrap();
        Connection::open(dir.path().join("market.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE keys (key_hash TEXT PRIMARY KEY, log TEXT, last_at INTEGER NOT NULL, welcome TEXT,
                     sales INTEGER NOT NULL DEFAULT 0, sales_week INTEGER NOT NULL DEFAULT 0,
                     forks INTEGER NOT NULL DEFAULT 0, left_at INTEGER, asked TEXT);",
            )
            .unwrap();
        let m = market(dir.path());
        let seller = earned(&[57]);
        ask(&m, 1, Verb::Join, NOW, &[], &seller).unwrap();
        for _ in 0..2 {
            assert_eq!(
                ask(&m, 1, Verb::List, NOW + 1, &[57, 228], &seller).unwrap(),
                "listed 1:57:228"
            );
        }
        drop(m);
        market(dir.path());
    }

    /// Acceptance 3.2 (c): LIST of a card held only as a starter (a REGULAR and the picked RARE) or as an imported copy
    /// is refused; an earned copy lists.
    #[test]
    fn a_starter_or_imported_copy_is_never_listed() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let mut l = started();
        l.add(
            t(START) + time::Duration::days(1),
            Kind::Import,
            Some(30),
            0,
        )
        .add(t("2026-11-03T12:00:00+09:00"), Kind::Match, Some(20), 50);
        ask(&m, 1, Verb::Join, NOW, &[], &l.text).unwrap();
        for (at, c) in [(NOW + 1, 1u32), (NOW + 2, 51), (NOW + 3, 30)] {
            let price = collection::ceiling_value(table().value(c).unwrap());
            let why = ask(&m, 1, Verb::List, at, &[c.into(), price.into()], &l.text).unwrap_err();
            assert_eq!(
                why,
                format!("card {c} is held only as a starter or imported copy, which never lists")
            );
        }
        assert_eq!(
            ask(&m, 1, Verb::List, NOW + 4, &[20, 38], &l.text).unwrap(),
            "listed 1:20:38"
        );
    }

    /// Acceptance 3.2 (d): a listing sells for 7 days on the server's clock; one moment later BROWSE leaves it out,
    /// BUY finds it gone and COLLECT returns it to the seller, once.
    #[test]
    fn a_listing_past_7_days_comes_back_at_collect() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let seller = earned(&[57]);
        let funds = earned(&[20, 20]);
        ask(&m, 1, Verb::Join, NOW, &[], &seller).unwrap();
        ask(&m, 2, Verb::Join, NOW, &[], &funds).unwrap();
        let listed = NOW + 1;
        ask(&m, 1, Verb::List, listed, &[57, 228], &seller).unwrap();
        let last = listed + LISTING_MS;
        assert_eq!(
            ask(&m, 1, Verb::Collect, last, &[], "").unwrap(),
            "collected"
        );
        assert_eq!(
            ask(&m, 2, Verb::Browse, last, &[0], "").unwrap(),
            "listings 1:57:228"
        );
        assert_eq!(
            ask(&m, 2, Verb::Browse, last + 1, &[0], "").unwrap(),
            "listings"
        );
        assert_eq!(
            ask(&m, 2, Verb::Buy, last + 2, &[1, 57, 228], &funds).unwrap_err(),
            GONE
        );
        assert_eq!(
            ask(&m, 1, Verb::Collect, last + 1, &[], "").unwrap(),
            "collected returned 1:57"
        );
        assert_eq!(
            ask(&m, 1, Verb::Collect, last + 2, &[], "").unwrap(),
            "collected"
        );
        assert!(row(dir.path(), "SELECT * FROM ledger").is_empty());
    }

    /// Acceptance 3.2 (e): a listing, its sale and the waiting proceeds survive two restarts of the server (the store
    /// opened again), and no BROWSE, BUY or COLLECT answer holds a key, a key's hash or anything but its words and
    /// numbers.
    #[test]
    fn the_ledger_survives_a_restart_and_names_no_one() {
        let dir = tempfile::tempdir().unwrap();
        let seller = earned(&[57]);
        let funds = earned(&[20, 20]);
        {
            let m = market(dir.path());
            ask(&m, 1, Verb::Join, NOW, &[], &seller).unwrap();
            ask(&m, 1, Verb::List, NOW + 1, &[57, 228], &seller).unwrap();
            ask(&m, 2, Verb::Join, NOW + 2, &[], &funds).unwrap();
        }
        let mut said = vec![];
        {
            let m = market(dir.path());
            said.push(ask(&m, 2, Verb::Browse, NOW + 3, &[0], "").unwrap());
            said.push(ask(&m, 2, Verb::Buy, NOW + 4, &[1, 57, 228], &funds).unwrap());
        }
        let m = market(dir.path());
        said.push(ask(&m, 1, Verb::Collect, NOW + 5, &[], "").unwrap());
        assert_eq!(
            said,
            [
                "listings 1:57:228",
                "bought 1:57:228",
                "collected sold 1:57:228"
            ]
        );
        for k in [1u8, 2] {
            let (pk, kh) = (hex(&[k; 32]), key_hash(&[k; 32]));
            for a in &said {
                assert!(
                    !a.contains(&pk) && !a.contains(&kh) && !a.contains(kh.get(..8).unwrap()),
                    "{a}"
                );
            }
        }
        let words = ["listings", "bought", "collected", "sold", "returned"];
        for a in &said {
            for w in a.split(' ') {
                assert!(
                    words.contains(&w) || w.chars().all(|c| c.is_ascii_digit() || c == ':'),
                    "{a}"
                );
            }
        }
        assert_eq!(
            row(dir.path(), "SELECT card, state FROM ledger ORDER BY id"),
            [[s("57"), s("paid")], [s("57"), s("bought")]]
        );
    }

    /// The stock fixture, `table()`'s cards at the values PLAN.md row 3.3 names: REGULAR 1-40 at 40 (TOTAL 80),
    /// SPECIAL 41-50 and 57 at 240 (TOTAL 87), RARE 51-55 at 1,000 (TOTAL 90), ALL TIME LEGEND 56 at 10,000 (TOTAL 95).
    fn stock_table() -> Table {
        let mut cards = vec![];
        for c in 1..=40 {
            cards.push((c, Rarity::Regular, 80));
        }
        for c in (41..=50).chain([57]) {
            cards.push((c, Rarity::Special, 87));
        }
        for c in 51..=55 {
            cards.push((c, Rarity::Rare, 90));
        }
        cards.push((56, Rarity::AllTimeLegend, 95));
        Table::of(&cards)
    }

    fn stock_market(dir: &Path, stock: Stock) -> Market {
        Market::with(stock_table(), &dir.join("market.db"))
            .unwrap()
            .with_stock(stock)
    }

    const DAY_MS: u64 = 24 * 3600 * 1000;

    /// A STOCK answer's rows: (slot, card, price, sold).
    fn stock_rows(said: &str) -> Vec<(u64, u64, u64, bool)> {
        said.split(' ')
            .filter(|w| w.contains(':'))
            .map(|w| {
                let f: Vec<&str> = w.split(':').collect();
                (
                    f[0].parse().unwrap(),
                    f[1].parse().unwrap(),
                    f[2].parse().unwrap(),
                    f.get(3) == Some(&"sold"),
                )
            })
            .collect()
    }

    /// The first start and `weeks` full weeks of 21 paying matches from Tuesday 3 November: 150 + 1,050 a week.
    fn rich(weeks: i64) -> String {
        let mut l = started();
        for w in 0..weeks {
            for i in 0..21 {
                let at = t("2026-11-03T10:00:00+09:00") + time::Duration::weeks(w) + mins(9 * i);
                l.add(at, Kind::Match, Some(20), 50);
            }
        }
        l.text
    }

    /// `log` with one more line, chained to its last.
    fn plus(log: &str, kind: Kind, card: u32, tokens: i64) -> String {
        let last = log.lines().last().unwrap();
        let line = Entry {
            seq: log.lines().count() as u64 + 1,
            at: t("2026-12-01T10:00:00+09:00"),
            rules: RULES,
            kind,
            card: Some(card),
            tokens,
            why: String::new(),
            prev: collection::hash(last),
        }
        .line();
        format!("{log}{line}\n")
    }

    /// Acceptance 3.3 (a): one key gets the same 10 rows on two views and after a restart, drawn again from the same
    /// secret, key and day when the stored list is gone; another key and the next server day get other rows; the rows
    /// are 4 REGULAR, 3 SPECIAL, 2 RARE and 1 LEGEND or ALL TIME LEGEND at 90 % (36, 216, 900, 9,000), and a secret
    /// HOLE at 5,625.
    #[test]
    fn a_keys_stock_list_is_the_same_all_day_and_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let log = earned(&[20]);
        let a = {
            let m = stock_market(dir.path(), Stock::default());
            ask(&m, 1, Verb::Join, NOW, &[], &log).unwrap();
            ask(&m, 2, Verb::Join, NOW, &[], &log).unwrap();
            let a = ask(&m, 1, Verb::Stock, NOW + 1, &[], &log).unwrap();
            assert_eq!(ask(&m, 1, Verb::Stock, NOW + 2, &[], &log).unwrap(), a);
            assert_ne!(ask(&m, 2, Verb::Stock, NOW + 1, &[], &log).unwrap(), a);
            a
        };
        let rows = stock_rows(&a);
        let groups = [0, 0, 0, 0, 1, 1, 1, 2, 2, 3];
        let prices = [36, 36, 36, 36, 216, 216, 216, 900, 900, 9_000];
        assert_eq!(rows.len(), 10, "{a}");
        assert!(a.starts_with("stock 1:"), "{a}");
        for (i, (slot, card, price, sold)) in rows.iter().enumerate() {
            assert_eq!(
                (*slot, *price, *sold),
                (i as u64 + 1, prices[i], false),
                "{a}"
            );
            let tier = stock_table().card(*card as u32).unwrap().rarity;
            assert!(STOCK_GROUPS[groups[i]].contains(&tier), "{a}");
        }
        let m = stock_market(dir.path(), Stock::default());
        assert_eq!(ask(&m, 1, Verb::Stock, NOW + 3, &[], &log).unwrap(), a);
        Connection::open(dir.path().join("market.db"))
            .unwrap()
            .execute("DELETE FROM stock", [])
            .unwrap();
        assert_eq!(ask(&m, 1, Verb::Stock, NOW + 4, &[], &log).unwrap(), a);
        let next = ask(&m, 1, Verb::Stock, NOW + DAY_MS, &[], &log).unwrap();
        assert_eq!(stock_rows(&next).len(), 10);
        assert_ne!(next, a);
        // a secret HOLE, the one card of its group: 6,250 at 90 %
        let dir = tempfile::tempdir().unwrap();
        let mut table = stock_table();
        table.cards.insert(
            56,
            Row {
                rarity: Rarity::AllTimeLegend,
                total: 100,
                series: "HOLE".into(),
                secret: true,
            },
        );
        let m = Market::with(table, &dir.path().join("market.db")).unwrap();
        ask(&m, 1, Verb::Join, NOW, &[], &log).unwrap();
        let said = ask(&m, 1, Verb::Stock, NOW + 1, &[], &log).unwrap();
        assert!(said.ends_with(" 10:56:5625"), "{said}");
    }

    /// Acceptance 3.3 (b): a key outside the market, one that left, and one whose log fails the check get no list
    /// and no sale, each with the reason; nothing is drawn or sold for them.
    #[test]
    fn a_key_outside_the_market_or_with_a_failing_log_gets_no_stock() {
        let dir = tempfile::tempdir().unwrap();
        let m = stock_market(dir.path(), Stock::default());
        let log = earned(&[20]);
        for verb in [Verb::Stock, Verb::StockBuy] {
            let args: &[u64] = if verb == Verb::Stock {
                &[]
            } else {
                &[1, 1, 36]
            };
            assert_eq!(ask(&m, 3, verb, NOW, args, &log).unwrap_err(), NOT_IN);
        }
        ask(&m, 1, Verb::Join, NOW, &[], &log).unwrap();
        let mut bad = started();
        bad.add(t("2026-11-03T10:00:00+09:00"), Kind::Match, Some(20), 1_050);
        for (at, verb) in [(NOW + 1, Verb::Stock), (NOW + 2, Verb::StockBuy)] {
            let args: &[u64] = if verb == Verb::Stock {
                &[]
            } else {
                &[1, 1, 36]
            };
            let why = ask(&m, 1, verb, at, args, &bad.text).unwrap_err();
            assert!(
                why.contains("line 14") && why.contains("one pays 0 to 50"),
                "{why}"
            );
        }
        ask(&m, 2, Verb::Join, NOW, &[], &log).unwrap();
        ask(&m, 2, Verb::Leave, NOW + 1, &[], "").unwrap();
        assert_eq!(
            ask(&m, 2, Verb::Stock, NOW + 2, &[], &log).unwrap_err(),
            NOT_IN
        );
        assert_eq!(row(dir.path(), "SELECT COUNT(*) FROM stock"), [[s("0")]]);
        assert!(row(dir.path(), "SELECT * FROM ledger").is_empty());
    }

    /// Acceptance 3.3 (c): one key with the tokens buys all 10 slots in a day; a slot sells once, sent again (the same
    /// answer, one sale), asked again later (refused) or raced from two threads; one token short is refused, and a
    /// purchase whose `stock_buy` line is not written yet still counts against the balance, once.
    #[test]
    fn one_key_buys_its_whole_list_and_each_slot_sells_once() {
        let dir = tempfile::tempdir().unwrap();
        let m = std::sync::Arc::new(stock_market(dir.path(), Stock::default()));
        let clock = std::cell::Cell::new(NOW);
        let next = || {
            clock.set(clock.get() + 1);
            clock.get()
        };
        let log = rich(11); // 11,700 tokens; the list costs 11,592
        ask(&m, 1, Verb::Join, next(), &[], &log).unwrap();
        let list = stock_rows(&ask(&m, 1, Verb::Stock, next(), &[], &log).unwrap());
        for (slot, card, price, _) in &list {
            let at = next();
            let first = ask(&m, 1, Verb::StockBuy, at, &[*slot, *card, *price], &log);
            assert_eq!(first, Ok(format!("stockbought {slot}:{card}:{price}")));
            assert_eq!(
                ask(&m, 1, Verb::StockBuy, at, &[*slot, *card, *price], &log),
                first
            );
        }
        let (slot, card, price, _) = list[0];
        assert_eq!(
            ask(&m, 1, Verb::StockBuy, next(), &[slot, card, price], &log).unwrap_err(),
            STOCK_SOLD
        );
        let said = ask(&m, 1, Verb::Stock, next(), &[], &log).unwrap();
        assert!(stock_rows(&said).iter().all(|r| r.3), "{said}");
        let stock = format!(
            "SELECT COUNT(*), SUM(price) FROM ledger WHERE key_hash = '{}' AND state = 'stock'",
            key_hash(&[1; 32])
        );
        assert_eq!(row(dir.path(), &stock), [[s("10"), s("11592")]]);
        // two racing requests for one slot: one sale
        ask(&m, 2, Verb::Join, next(), &[], &log).unwrap();
        let list = stock_rows(&ask(&m, 2, Verb::Stock, next(), &[], &log).unwrap());
        let (slot, card, price, _) = list[0];
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let at = next();
        next();
        let racers = [at, at + 1].map(|at| {
            let (m, gate, log) = (m.clone(), gate.clone(), log.clone());
            std::thread::spawn(move || {
                gate.wait();
                ask(&m, 2, Verb::StockBuy, at, &[slot, card, price], &log)
            })
        });
        let answers: Vec<_> = racers.into_iter().map(|h| h.join().unwrap()).collect();
        let won: Vec<_> = answers.iter().filter_map(|a| a.as_ref().ok()).collect();
        assert_eq!(
            won,
            [&format!("stockbought {slot}:{card}:{price}")],
            "{answers:?}"
        );
        let stock2 = format!(
            "SELECT COUNT(*) FROM ledger WHERE key_hash = '{}' AND state = 'stock'",
            key_hash(&[2; 32])
        );
        assert_eq!(row(dir.path(), &stock2), [[s("1")]]);
        // 899 tokens: 150 + 11 matches + a sale of card 57 at 199
        let mut l = started();
        l.add(t("2026-11-03T10:00:00+09:00"), Kind::Match, Some(57), 50)
            .add(t("2026-11-03T10:01:00+09:00"), Kind::List, Some(57), 0)
            .add(
                t("2026-11-03T10:02:00+09:00"),
                Kind::MarketSell,
                Some(57),
                199,
            );
        for i in 0..10 {
            l.add(
                t("2026-11-03T11:00:00+09:00") + mins(9 * i),
                Kind::Match,
                Some(20),
                50,
            );
        }
        let short = l.text;
        ask(&m, 3, Verb::Join, next(), &[], &short).unwrap();
        let list = stock_rows(&ask(&m, 3, Verb::Stock, next(), &[], &short).unwrap());
        let rare = [list[7].0, list[7].1, list[7].2];
        assert_eq!(
            ask(&m, 3, Verb::StockBuy, next(), &rare, &short).unwrap_err(),
            "899 tokens to spend, the slot costs 900"
        );
        let (slot, card, price, _) = list[0];
        ask(&m, 3, Verb::StockBuy, next(), &[slot, card, price], &short).unwrap();
        let held = "863 tokens to spend, the slot costs 900";
        assert_eq!(
            ask(&m, 3, Verb::StockBuy, next(), &rare, &short).unwrap_err(),
            held
        );
        let written = plus(&short, Kind::StockBuy, card as u32, -(price as i64));
        assert_eq!(
            ask(&m, 3, Verb::StockBuy, next(), &rare, &written).unwrap_err(),
            held
        );
        // and BUY counts it as well
        let buy = Checked {
            balance: 899,
            ..Checked::default()
        };
        let db = m.db.lock().unwrap();
        assert_eq!(spendable(&db, &key_hash(&[3; 32]), &buy).unwrap(), 863);
    }

    /// Acceptance 3.3 (d): a restart with 12 cards and a share of 85 % leaves the lists already served and the day's
    /// settings alone, and changes the next day's lists; a limit of 2 refuses the 3rd purchase of a day.
    #[test]
    fn new_settings_start_with_the_next_day_and_a_limit_holds() {
        let dir = tempfile::tempdir().unwrap();
        let log = earned(&[20]);
        let served = {
            let m = stock_market(dir.path(), Stock::default());
            ask(&m, 1, Verb::Join, NOW, &[], &log).unwrap();
            ask(&m, 2, Verb::Join, NOW, &[], &log).unwrap();
            ask(&m, 1, Verb::Stock, NOW + 1, &[], &log).unwrap()
        };
        let m = stock_market(dir.path(), Stock::new("5,4,2,1", 85, 2).unwrap());
        assert_eq!(ask(&m, 1, Verb::Stock, NOW + 2, &[], &log).unwrap(), served);
        let other = ask(&m, 2, Verb::Stock, NOW + 2, &[], &log).unwrap();
        assert_eq!(stock_rows(&other).len(), 10, "{other}");
        assert!(!other.contains("left"), "{other}");
        let day = NOW + DAY_MS;
        let said = ask(&m, 1, Verb::Stock, day, &[], &log).unwrap();
        let prices: Vec<u64> = stock_rows(&said).iter().map(|r| r.2).collect();
        assert_eq!(
            prices,
            [34, 34, 34, 34, 34, 204, 204, 204, 204, 850, 850, 8_500],
            "{said}"
        );
        assert!(said.starts_with("stock left 2 1:"), "{said}");
        let list = stock_rows(&said);
        for (n, (slot, card, price, _)) in list.iter().take(3).enumerate() {
            let got = ask(
                &m,
                1,
                Verb::StockBuy,
                day + 1 + n as u64,
                &[*slot, *card, *price],
                &log,
            );
            if n < 2 {
                assert_eq!(got, Ok(format!("stockbought {slot}:{card}:{price}")));
            } else {
                assert_eq!(
                    got.unwrap_err(),
                    "2 stock purchases today, this server's limit: the list turns at 04:00 UTC"
                );
            }
        }
        let said = ask(&m, 1, Verb::Stock, day + 4, &[], &log).unwrap();
        assert!(said.starts_with("stock left 0 1:"), "{said}");
        // the settings a server reads at start are checked, and a refusal names its flag (review m4)
        for (mix, share, flag) in [
            ("4,3,2", 90, "--stock-mix "),
            ("30,1,0,0", 90, "--stock-mix "),
            ("4,3,2,1", 0, "--stock-share "),
            ("4,3,2,1", 101, "--stock-share "),
        ] {
            let why = Stock::new(mix, share, 0).unwrap_err();
            assert!(why.starts_with(flag), "{why}");
        }
    }

    /// Unix ms of an RFC 3339 time.
    fn ms(at: &str) -> u64 {
        (t(at).unix_timestamp_nanos() / 1_000_000) as u64
    }

    /// Acceptance 3.3 (e): a view at 03:59 UTC shows that server day's list and 04:00 the next; LEAVE then JOIN, and
    /// RESET then JOIN, show the same list with its sold slot.
    #[test]
    fn the_list_turns_at_4_utc_and_stays_through_leave_and_reset() {
        let dir = tempfile::tempdir().unwrap();
        let m = stock_market(dir.path(), Stock::default());
        let log = earned(&[20]);
        ask(&m, 1, Verb::Join, ms("2026-11-19T03:00:00Z"), &[], &log).unwrap();
        let night = ask(&m, 1, Verb::Stock, ms("2026-11-19T03:00:01Z"), &[], &log).unwrap();
        let late = ask(
            &m,
            1,
            Verb::Stock,
            ms("2026-11-19T03:59:59.999Z"),
            &[],
            &log,
        )
        .unwrap();
        assert_eq!(late, night);
        let at = ms("2026-11-19T04:00:00Z");
        let morning = ask(&m, 1, Verb::Stock, at, &[], &log).unwrap();
        assert_ne!(morning, night);
        assert_eq!(stock_day(at), stock_day(at - 1) + 1);
        let (slot, card, price, _) = stock_rows(&morning)[0];
        ask(&m, 1, Verb::StockBuy, at + 1, &[slot, card, price], &log).unwrap();
        let sold = morning.replacen(
            &format!(" {slot}:{card}:{price}"),
            &format!(" {slot}:{card}:{price}:sold"),
            1,
        );
        assert_eq!(ask(&m, 1, Verb::Stock, at + 2, &[], &log).unwrap(), sold);
        ask(&m, 1, Verb::Leave, at + 3, &[], "").unwrap();
        ask(&m, 1, Verb::Join, at + 4, &[], &log).unwrap();
        assert_eq!(ask(&m, 1, Verb::Stock, at + 5, &[], &log).unwrap(), sold);
        ask(&m, 1, Verb::Reset, at + 6, &[], "").unwrap();
        let reset: String = log.lines().take(12).map(|l| format!("{l}\n")).collect();
        ask(&m, 1, Verb::Join, at + 7, &[], &reset).unwrap();
        assert_eq!(ask(&m, 1, Verb::Stock, at + 8, &[], &reset).unwrap(), sold);
    }

    /// A stock-bought card is an earned copy: once its `stock_buy` line is in the log it lists at 95 % of its value,
    /// the band's top, like any earned copy.
    #[test]
    fn a_stock_bought_card_lists_at_95_percent() {
        let dir = tempfile::tempdir().unwrap();
        let m = stock_market(dir.path(), Stock::default());
        let log = earned(&[20, 20, 20]);
        ask(&m, 1, Verb::Join, NOW, &[], &log).unwrap();
        let list = stock_rows(&ask(&m, 1, Verb::Stock, NOW + 1, &[], &log).unwrap());
        let (slot, card, price, _) = list[4];
        assert_eq!(price, 216);
        ask(&m, 1, Verb::StockBuy, NOW + 2, &[slot, card, price], &log).unwrap();
        let top = u64::from(collection::ceiling_value(240));
        assert_eq!(top, 228);
        let why = ask(&m, 1, Verb::List, NOW + 3, &[card, top], &log).unwrap_err();
        assert!(why.starts_with("no earned copy"), "{why}");
        let written = plus(&log, Kind::StockBuy, card as u32, -216);
        assert_eq!(
            ask(&m, 1, Verb::List, NOW + 4, &[card, top], &written).unwrap(),
            format!("listed 2:{card}:228")
        );
    }

    /// Review M1: a key that bought stock and LEAVEs is dropped 7 days later; it JOINs again with its log, whose
    /// `stock_buy` line still matches the purchase (kept, retired), and that purchase withholds no tokens. A new
    /// purchase of the same card at the same price is still withheld until its own line is written.
    #[test]
    fn a_stock_buyer_rejoins_with_its_log_after_its_key_expired() {
        let dir = tempfile::tempdir().unwrap();
        let m = stock_market(dir.path(), Stock::default());
        let log = earned(&[20, 20, 20]);
        ask(&m, 1, Verb::Join, NOW, &[], &log).unwrap();
        let list = stock_rows(&ask(&m, 1, Verb::Stock, NOW + 1, &[], &log).unwrap());
        let (slot, card, price, _) = list[0];
        ask(&m, 1, Verb::StockBuy, NOW + 2, &[slot, card, price], &log).unwrap();
        let (card, price) = (card as u32, price as i64);
        let written = plus(&log, Kind::StockBuy, card, -price);
        ask(&m, 1, Verb::Stock, NOW + 3, &[], &written).unwrap();
        ask(&m, 1, Verb::Leave, NOW + 4, &[], "").unwrap();
        let back = NOW + 5 + KEEP_LEFT_MS;
        assert_eq!(
            ask(&m, 1, Verb::Join, back, &[], &written),
            Ok(format!("{} lines", written.lines().count()))
        );
        let kh = key_hash(&[1; 32]);
        let checked = check(&written, &m.table, &[(card, price)]).unwrap();
        let db = m.db.lock().unwrap();
        assert_eq!(spendable(&db, &kh, &checked).unwrap(), checked.balance);
        db.execute(
            "INSERT INTO ledger (key_hash, card, price, state, at) VALUES (?1, ?2, ?3, 'stock', 0)",
            params![kh, card, price],
        )
        .unwrap();
        assert_eq!(
            spendable(&db, &kh, &checked).unwrap(),
            checked.balance - price
        );
    }

    /// Review m1: a store that holds its secret keeps it and draws no new bytes (it opens without entropy); a new
    /// store draws once, and without entropy it does not open.
    #[test]
    fn a_stored_secret_is_kept_and_nothing_is_drawn() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("market.db");
        let first = Market::with(stock_table(), &db).unwrap().secret;
        let mut draws = 0;
        let again = Market::with_draw(stock_table(), &db, |_| {
            draws += 1;
            Err("no entropy".into())
        })
        .unwrap();
        assert_eq!((again.secret, draws), (first, 0));
        let why = Market::with_draw(stock_table(), &dir.path().join("new.db"), |_| {
            Err("no entropy".into())
        })
        .unwrap_err();
        assert!(why.ends_with("no random secret (no entropy)"), "{why}");
    }

    /// Advisor m1: the draw is seeded with the store's secret, so two new stores give the same key on the same day
    /// different lists (a key and a day alone would let anyone compute a list).
    #[test]
    fn two_stores_give_one_key_different_lists_the_same_day() {
        let log = earned(&[20]);
        let lists: Vec<String> = (0..2)
            .map(|_| {
                let dir = tempfile::tempdir().unwrap();
                let m = stock_market(dir.path(), Stock::default());
                ask(&m, 1, Verb::Join, NOW, &[], &log).unwrap();
                ask(&m, 1, Verb::Stock, NOW + 1, &[], &log).unwrap()
            })
            .collect();
        assert_eq!(stock_rows(&lists[0]).len(), 10);
        assert_ne!(lists[0], lists[1]);
    }

    /// Advisor m2: the server day comes from the server's clock, not the request's: a PC clock 23 h ahead, past
    /// 04:00 UTC, still gets the server's today list.
    #[test]
    fn a_pc_clock_a_day_ahead_still_gets_todays_list() {
        let dir = tempfile::tempdir().unwrap();
        let m = stock_market(dir.path(), Stock::default());
        let log = earned(&[20]);
        let now = ms("2026-11-19T03:00:00Z");
        ask(&m, 1, Verb::Join, now, &[], &log).unwrap();
        let today = ask(&m, 1, Verb::Stock, now + 1, &[], &log).unwrap();
        let ahead = now + 23 * 3600 * 1000;
        assert_eq!(stock_day(ahead), stock_day(now) + 1);
        let r = Request {
            verb: Verb::Stock,
            key: [1; 32],
            at: ahead,
            rules: RULES,
            args: vec![],
            log: log.clone(),
        };
        assert_eq!(m.handle(&r, now + 2).unwrap(), today);
    }

    /// The other guards: 5 open listings per key, CANCEL, the seller's own listing, a purchase's tokens held until
    /// its `market_buy` line is in the log, and 10 sales a week per seller (BROWSE leaves that seller out).
    #[test]
    fn the_ledger_guards() {
        let dir = tempfile::tempdir().unwrap();
        let m = market(dir.path());
        let clock = std::cell::Cell::new(NOW);
        let next = || {
            clock.set(clock.get() + 1);
            clock.get()
        };
        let seller = earned(&[20, 21, 22, 23, 24, 25]);
        ask(&m, 1, Verb::Join, next(), &[], &seller).unwrap();
        for c in 20..25 {
            ask(&m, 1, Verb::List, next(), &[c, 38], &seller).unwrap();
        }
        let why = ask(&m, 1, Verb::List, next(), &[25, 38], &seller).unwrap_err();
        assert!(why.starts_with("5 listings are open"), "{why}");
        assert_eq!(
            ask(&m, 1, Verb::Cancel, next(), &[5], "").unwrap(),
            "cancelled 5:24"
        );
        assert_eq!(
            ask(&m, 1, Verb::List, next(), &[25, 38], &seller).unwrap(),
            "listed 6:25:38"
        );
        let why = ask(&m, 1, Verb::Buy, next(), &[1, 20, 38], &seller).unwrap_err();
        assert!(why.contains("own listing"), "{why}");
        // 150 tokens: three purchases at 38 whose lines the log lacks leave 36, with the lines too
        let buyer = started().text;
        ask(&m, 2, Verb::Join, next(), &[], &buyer).unwrap();
        for id in 1..=3 {
            assert_eq!(
                ask(&m, 2, Verb::Buy, next(), &[id, 19 + id, 38], &buyer).unwrap(),
                format!("bought {id}:{}:38", 19 + id)
            );
        }
        let short = "36 tokens to spend, the listing costs 38";
        assert_eq!(
            ask(&m, 2, Verb::Buy, next(), &[4, 23, 38], &buyer).unwrap_err(),
            short
        );
        let mut written = started();
        for c in 20..23 {
            written.add(
                t("2026-11-02T12:00:00+09:00"),
                Kind::MarketBuy,
                Some(c),
                -38,
            );
        }
        assert_eq!(
            ask(&m, 2, Verb::Buy, next(), &[4, 23, 38], &written.text).unwrap_err(),
            short
        );
        assert!(
            ask(&m, 1, Verb::Cancel, next(), &[1], "")
                .unwrap_err()
                .contains("COLLECT")
        );
        assert_eq!(
            ask(&m, 2, Verb::Cancel, next(), &[4], "").unwrap_err(),
            "this installation has no listing with that id"
        );
        // the seller at the week's 10 sales: its listings leave BROWSE and do not sell until the week turns
        ask(&m, 3, Verb::Join, next(), &[], &buyer).unwrap();
        let set = |n: u32| {
            Connection::open(dir.path().join("market.db"))
                .unwrap()
                .execute(
                    "UPDATE keys SET sales = ?1 WHERE key_hash = ?2",
                    params![n, key_hash(&[1; 32])],
                )
                .unwrap();
        };
        set(10);
        assert_eq!(
            ask(&m, 3, Verb::Browse, next(), &[0], "").unwrap(),
            "listings"
        );
        let why = ask(&m, 3, Verb::Buy, next(), &[4, 23, 38], &buyer).unwrap_err();
        assert!(why.contains("sells again next week"), "{why}");
        set(9);
        assert_eq!(
            ask(&m, 3, Verb::Browse, next(), &[0], "").unwrap(),
            "listings 4:23:38 6:25:38"
        );
        assert_eq!(
            ask(&m, 3, Verb::Browse, next(), &[4], "").unwrap(),
            "listings 6:25:38"
        );
        // two days on (NOW is a Saturday) the week has turned and the counter starts again
        let week = clock.get() + 2 * 24 * 3600 * 1000;
        assert_ne!(sales_week(week), sales_week(NOW));
        set(10);
        assert_eq!(
            ask(&m, 3, Verb::Buy, week, &[6, 25, 38], &buyer).unwrap(),
            "bought 6:25:38"
        );
        assert_eq!(
            row(
                dir.path(),
                &format!(
                    "SELECT sales FROM keys WHERE key_hash = '{}'",
                    key_hash(&[1; 32])
                )
            ),
            [[s("1")]]
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
        c.execute("UPDATE keys SET sales = 4", []).unwrap();
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
        // a new log after the reset: no welcome line, the mark stayed
        let reset: String = started()
            .text
            .lines()
            .take(12)
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(
            m.handle(&req(Verb::Join, NOW + 2, &reset), NOW).unwrap(),
            "12 lines"
        );
        ledger(
            dir.path(),
            &[(5, "open"), (7, "open"), (6, "sold"), (9, "paid")],
        );
        assert_eq!(
            m.handle(&req(Verb::Leave, NOW + 3, ""), NOW).unwrap(),
            "left returned 5 7 waiting 1"
        );
        assert_eq!(
            row(dir.path(), "SELECT key_hash, log, sales FROM keys"),
            [[s(&kh), None, s("4")]]
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

    /// Acceptance 3.1b (e) and N2: a JOIN carries no name and `market.db` has no name column; a second welcome after a
    /// RESET is refused even when the line is the identical one (the balance starts at 0 again, the mark does not go),
    /// while after LEAVE the same log comes back with the welcome it already paid.
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
        // the identical welcome line again, after the reset: the log's balance restarts, the mark does not
        let why = m
            .handle(&req(Verb::Join, NOW + 5, &started().text), NOW)
            .unwrap_err();
        assert!(why.starts_with("a second welcome"), "{why}");
        // after LEAVE the same log comes back with its welcome, which the key already paid
        m.handle(&req(Verb::Join, NOW + 6, &without), NOW).unwrap();
        m.handle(&req(Verb::Leave, NOW + 7, ""), NOW).unwrap();
        assert_eq!(
            m.handle(&req(Verb::Join, NOW + 8, &started().text), NOW)
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
            "card_no\trecord_index\trarity_name\tis_placeholder\tstats_total\tcard_code\timage_archive_path\tstat_offence\tstat_defence\tstat_technique\tstat_power\tstat_speed\tstat_stamina\n\
             101\t0\tREGULAR\t0\t80\tWGK-3\t\t14\t13\t13\t13\t13\t14\n\
             102\t1\tALL TIME LEGEND\t0\t100\tHOLE-3\t\t16\t16\t16\t16\t16\t20\n\
             103\t2\tREGULAR\t1\t70\t\t\t14\t13\t13\t13\t13\t14\n\
             0\t3\tRARE\t0\t90\t\t\t14\t13\t13\t13\t13\t14\n\
             104\t4\t\t0\t90\t\t\t14\t13\t13\t13\t13\t14\n\
             105\t5\tRARE\t0\t85\t\tcards/EMVP0506_X_f.dds\t14\t13\t13\t13\t13\t14\n\
             4659\t6\tREGULAR\t0\t100\tATLE-RE-4\t\t16\t16\t16\t16\t16\t20\n",
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

    /// M1: the secret mark comes from the six printed stats, not the tier. An ALL TIME LEGEND that prints its stats is
    /// priced from its TOTAL; one that prints the placeholder 16/16/16/16/16/20 gets its series' flat price.
    #[test]
    fn only_placeholder_stats_make_a_card_secret() {
        let dir = tempfile::tempdir().unwrap();
        let cat = dir.path().join("catalogue.tsv");
        std::fs::write(
            &cat,
            "card_no\trarity_name\tstats_total\tcard_code\tstat_offence\tstat_defence\tstat_technique\tstat_power\tstat_speed\tstat_stamina\n\
             201\tALL TIME LEGEND\t97\tATLE-1\t18\t12\t17\t16\t17\t17\n\
             202\tALL TIME LEGEND\t100\tATLE-2\t16\t16\t16\t16\t16\t20\n",
        )
        .unwrap();
        let out = dir.path().join("values.tsv");
        assert_eq!(write_table(&cat, &out).unwrap(), 2);
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(
            text.contains("\n201\tALL TIME LEGEND\t97\tATLE\t0\n"),
            "{text}"
        );
        assert!(
            text.contains("\n202\tALL TIME LEGEND\t100\tATLE\t1\n"),
            "{text}"
        );
        let t = Table::load(&out).unwrap();
        assert_eq!(t.value(202), Some(3_750));
        assert_ne!(t.value(201), Some(3_750));
        // a catalogue without the stat columns cannot mark secrets and is refused
        std::fs::write(
            &cat,
            "card_no\trarity_name\tstats_total\tcard_code\n201\tALL TIME LEGEND\t97\tATLE-1\n",
        )
        .unwrap();
        assert!(
            write_table(&cat, &out)
                .unwrap_err()
                .contains("stat_offence")
        );
    }

    /// #375: a BUY, a STOCKBUY and a COLLECT applied with their answers and outbox lost: OWED names each row the log
    /// lacks, also after the stock list turned, and nothing once the lines are written.
    #[test]
    fn owed_names_the_purchases_and_paid_sales_a_log_lacks() {
        let dir = tempfile::tempdir().unwrap();
        let m = stock_market(dir.path(), Stock::default());
        let seller = earned(&[57]);
        ask(&m, 1, Verb::Join, NOW, &[], &seller).unwrap();
        ask(&m, 1, Verb::List, NOW + 1, &[57, 228], &seller).unwrap();
        let seller = plus(&seller, Kind::List, 57, 0);
        let buyer = earned_from("2026-11-01T10:00:07+09:00", &[20, 20, 20]); // 300 tokens
        ask(&m, 2, Verb::Join, NOW + 2, &[], &buyer).unwrap();
        ask(&m, 2, Verb::Buy, NOW + 3, &[1, 57, 228], &buyer).unwrap();
        let list = stock_rows(&ask(&m, 2, Verb::Stock, NOW + 4, &[], &buyer).unwrap());
        let (slot, card, price, _) = list[0];
        ask(&m, 2, Verb::StockBuy, NOW + 5, &[slot, card, price], &buyer).unwrap();
        let owed = format!("owed bought 2:57:228 stock 3:{card}:{price}");
        assert_eq!(ask(&m, 2, Verb::Owed, NOW + 6, &[], &buyer).unwrap(), owed);
        assert_eq!(
            ask(&m, 2, Verb::Owed, NOW + DAY_MS, &[], &buyer).unwrap(),
            owed
        );
        let written = plus(&buyer, Kind::MarketBuy, 57, -228);
        let written = plus(&written, Kind::StockBuy, card as u32, -(price as i64));
        assert_eq!(
            ask(&m, 2, Verb::Owed, NOW + DAY_MS + 1, &[], &written).unwrap(),
            "owed"
        );
        assert_eq!(
            ask(&m, 1, Verb::Collect, NOW + 7, &[], "").unwrap(),
            "collected sold 1:57:228"
        );
        assert_eq!(
            ask(&m, 1, Verb::Owed, NOW + 8, &[], &seller).unwrap(),
            "owed sold 1:57:228"
        );
        let seller = plus(&seller, Kind::MarketSell, 57, 228);
        assert_eq!(
            ask(&m, 1, Verb::Owed, NOW + 9, &[], &seller).unwrap(),
            "owed"
        );
    }
}
