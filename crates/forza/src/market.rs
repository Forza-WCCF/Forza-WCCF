//! Create-a-Team's market requests (PLAN.md 3.1, 3.4, 3.5): the installation's key `data\market.key` and the signed
//! desk words `MARKET JOIN`, `LEAVE`, `RESET`, `LIST`, `CANCEL`, `BROWSE`, `BUY`, `COLLECT`, `STOCK`, `STOCKBUY` and `OWED`. The Launcher builds and
//! sends them; the server (`forza-server`, `crates/server/src/market.rs`) reads them with [`Request::open`], so the
//! format lives here once.
//!
//! A request is one line and a body: `MARKET VERB size sig` + `size` bytes. The body's first line is
//! `VERB key at rules` (the public key in hex, `at` in Unix milliseconds, the economy table's `RULES`) and the verb's
//! numbers ([`Verb::args`]), then on JOIN, LIST, BUY, STOCK, STOCKBUY and OWED the grant log `grants.jsonl` as it is on disk. No name is sent:
//! listings never show the seller (PLAN.md 3.5). `sig` is the ed25519 signature of the whole body in hex. Answers:
//! `OK ...` or `NO reason`, one line. A request whose `at` is not after the key's last one answers [`OLD`] with the
//! last time, and [`ask`] signs it again once past it (a PC clock set back must not lock the key out); a ledger word
//! (below) sent again byte for byte, before any other request of its key, answers what it answered the first time and
//! changes nothing, so a Launcher whose answer was lost (a reset connection, a timeout) sends the same bytes again
//! until one comes, and only then signs anything new. Each new request's `at` is strictly above its key's last one,
//! even within one millisecond (two COLLECT pages): a new request with the same `at`, verb and numbers is taken for
//! that resend. LEAVE answers
//! `OK left`, then `returned` and the card numbers of the open listings it closed (the Launcher writes their `unlist`
//! lines), then `waiting n` when n sales wait for COLLECT.
//!
//! The ledger's words (step 3.2) answer with `id:card:price` triples and never a key, a key's hash or a name:
//! - `LIST card price` + log: `OK listed id:card:price` (the Launcher writes the `list` line);
//! - `CANCEL listing`: `OK cancelled id:card` (an `unlist` line);
//! - `BROWSE after`: `OK listings` and up to [`BROWSE_PAGE`] open listings of other keys with an id above `after`,
//!   lowest id first, none of a seller at the week's 10 sales (ask again from the last id; fewer than a page is the
//!   end);
//! - `BUY listing card price` + log: `OK bought id:card:price` (a `market_buy` line); the card and price must be the
//!   listing's, so a stale id buys nothing else;
//! - `COLLECT`: `OK collected`, then `sold` and `id:card:price` for each sale whose proceeds it pays now (a
//!   `market_sell` line each), then `returned` and `id:card` for each listing past 7 days (an `unlist` line each); at
//!   most [`COLLECT_PAGE`] rows, so ask again until it answers `OK collected` alone. It also works after LEAVE while a
//!   sale waits.
//! - `OWED` + log: `OK owed`, then `bought` and `id:card:price` for each listing this key bought that the log has no
//!   `market_buy` line for, `stock` and `id:card:price` for each stock slot it bought without a `stock_buy` line, and
//!   `sold` and `id:card:price` for each sale COLLECT paid without a `market_sell` line: what a lost answer and a lost
//!   outbox left out of the log. At most [`COLLECT_PAGE`] rows; ask again once their lines are written.
//!
//! The own stock list (step 3.3, PLAN.md 3.5): each key's list for the server day (04:00 to 04:00 UTC), drawn from a
//! server secret, the key and the day and kept from its first view, so a reload, LEAVE then JOIN or RESET shows the
//! same rows; the cards, tier mix, price share and a purchase limit per day are the server's settings:
//! - `STOCK` + log: `OK stock`, then `left n` when the server has a daily limit (purchases left today), then
//!   `slot:card:price` for each row from slot 1, `slot:card:price:sold` for one bought today;
//! - `STOCKBUY slot card price` + log: `OK stockbought slot:card:price` (a `stock_buy` line of `-price`); the card and
//!   price must be the slot's today, so a list from before 04:00 UTC buys nothing else.
//!
//! The key is made on first use and lives outside the profile folder, so a profile reset keeps it (PLAN.md 3.6).
//!
//! The Launcher sends through [`Client`] (step 4.1): one request in flight per installation, kept with its bytes in
//! [`STATE_FILE`] and sent again before anything else until it is answered, and each answer's log lines written once.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// The key's file in the Launcher's `data` folder, beside `vslink.key`.
pub const KEY_FILE: &str = "market.key";
/// The biggest body a desk reads: a grant log of about 40,000 lines (a line is about 200 bytes), some 9 years at 4,500
/// lines a year; [`Request::signed`] refuses a bigger one with the reason.
pub const MAX_BODY: usize = 8 << 20;
/// The body's first line at most: four words, about 90 bytes.
pub const MAX_HEAD: usize = 256;
/// What every `MARKET` word answers on a desk without the market's value table.
pub const NOT_SET_UP: &str = "the market is not set up on this server";
/// How a refusal of a request not later than the key's last one starts; the server adds `, the last at <ms>`. The
/// key's last request itself, a ledger word sent again, answers as the first time instead.
pub const OLD: &str = "an old request (sent twice?)";
/// The desk's refusal of a word from a key not in the market (the server's words): a LEAVE whose answer was lost.
pub const NOT_IN: &str = "this installation is not in the market";
/// Listings in one `BROWSE` answer at most: about 900 bytes, under [`send`]'s 1 KiB read.
pub const BROWSE_PAGE: usize = 40;
/// Rows in one `COLLECT` answer at most, as [`BROWSE_PAGE`].
pub const COLLECT_PAGE: usize = 30;

/// What a request asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    /// Opt in, or send the grant log again: the server checks it and keeps it.
    Join,
    /// Leave the market: the server deletes the log and returns the open listings.
    Leave,
    /// A profile reset: the server starts a new history for the same key.
    Reset,
    /// Put one earned copy on the market at a price inside its band.
    List,
    /// Take one of this key's open listings back.
    Cancel,
    /// A page of the other keys' open listings.
    Browse,
    /// Buy one listing.
    Buy,
    /// Take the proceeds of sold listings and the listings past 7 days.
    Collect,
    /// This key's own stock list for the server day.
    Stock,
    /// Buy one slot of that list.
    StockBuy,
    /// The purchases and paid sales the log lacks.
    Owed,
}

impl Verb {
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Join => "JOIN",
            Self::Leave => "LEAVE",
            Self::Reset => "RESET",
            Self::List => "LIST",
            Self::Cancel => "CANCEL",
            Self::Browse => "BROWSE",
            Self::Buy => "BUY",
            Self::Collect => "COLLECT",
            Self::Stock => "STOCK",
            Self::StockBuy => "STOCKBUY",
            Self::Owed => "OWED",
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        [
            Self::Join,
            Self::Leave,
            Self::Reset,
            Self::List,
            Self::Cancel,
            Self::Browse,
            Self::Buy,
            Self::Collect,
            Self::Stock,
            Self::StockBuy,
            Self::Owed,
        ]
        .into_iter()
        .find(|v| v.word() == word)
    }

    /// The numbers the body's first line carries after `rules`, by name.
    #[must_use]
    pub fn args(self) -> &'static [&'static str] {
        match self {
            Self::List => &["card", "price"],
            Self::Cancel => &["listing"],
            Self::Browse => &["after"],
            Self::Buy => &["listing", "card", "price"],
            Self::StockBuy => &["slot", "card", "price"],
            Self::Join | Self::Leave | Self::Reset | Self::Collect | Self::Stock | Self::Owed => {
                &[]
            }
        }
    }

    /// Whether the body carries the grant log, which the server checks: JOIN, LIST, BUY, STOCK, STOCKBUY and OWED.
    #[must_use]
    pub fn logged(self) -> bool {
        matches!(
            self,
            Self::Join | Self::List | Self::Buy | Self::Stock | Self::StockBuy | Self::Owed
        )
    }
}

/// One market request, as signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub verb: Verb,
    /// The installation's public key.
    pub key: [u8; 32],
    /// When the Launcher made it, Unix milliseconds: the server takes each key's requests in rising order only.
    pub at: u64,
    /// The Launcher's economy table ([`crate::collection::RULES`]).
    pub rules: u32,
    /// The verb's numbers ([`Verb::args`]).
    pub args: Vec<u64>,
    /// The words [`Verb::logged`] names: `grants.jsonl` as it is; else empty.
    pub log: String,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let mut out = [0u8; N];
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    for (o, pair) in out.iter_mut().zip(s.as_bytes().chunks(2)) {
        *o = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

impl Request {
    /// The bytes to send: the request line and the body, signed with `key` (whose public half the body names); refused
    /// with the reason when the body passes [`MAX_BODY`], so the Launcher says so before it sends anything, or when
    /// `args` does not match [`Verb::args`].
    pub fn signed(&self, key: &SigningKey) -> Result<Vec<u8>, String> {
        if self.args.len() != self.verb.args().len() {
            return Err(usage(self.verb));
        }
        let mut body = format!(
            "{} {} {} {}",
            self.verb.word(),
            hex(key.verifying_key().as_bytes()),
            self.at,
            self.rules
        );
        for a in &self.args {
            body.push_str(&format!(" {a}"));
        }
        body.push('\n');
        body.push_str(&self.log);
        if body.len() > MAX_BODY {
            return Err(format!(
                "the grant log is {} KiB, over the market's limit of {} KiB: this collection cannot join the market",
                body.len() / 1024,
                MAX_BODY / 1024
            ));
        }
        let sig = key.sign(body.as_bytes());
        let mut out = format!(
            "MARKET {} {} {}\n",
            self.verb.word(),
            body.len(),
            hex(&sig.to_bytes())
        )
        .into_bytes();
        out.extend_from_slice(body.as_bytes());
        Ok(out)
    }

    /// The request line's words after `MARKET`: the verb, the body's size (at most [`MAX_BODY`]) and the signature.
    /// Read before the body, so a bad line is refused unread.
    pub fn head(words: &[&str]) -> Result<(Verb, usize, [u8; 64]), String> {
        let [verb, size, sig] = words else {
            return Err("say MARKET JOIN|LEAVE|RESET size signature".into());
        };
        let verb = Verb::parse(verb).ok_or("an unknown market word")?;
        let size = size
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_BODY).contains(n))
            .ok_or_else(|| format!("a request of 1 to {MAX_BODY} bytes"))?;
        let sig = unhex::<64>(sig).ok_or("the signature is not 128 hex digits")?;
        Ok((verb, size, sig))
    }

    /// The body of a request whose line said `verb` and `sig`: the signature checked against the key the body names,
    /// the words parsed.
    pub fn open(verb: Verb, sig: &[u8; 64], body: &[u8]) -> Result<Self, String> {
        let end = body
            .iter()
            .take(MAX_HEAD)
            .position(|&b| b == b'\n')
            .ok_or("the request's first line is missing or too long")?;
        let (head, rest) = body.split_at(end);
        let head =
            std::str::from_utf8(head).map_err(|_| "the request's first line is not UTF-8")?;
        let words: Vec<&str> = head.split_whitespace().collect();
        let [word, key, at, rules, args @ ..] = words.as_slice() else {
            return Err(usage(Verb::Join));
        };
        if Verb::parse(word) != Some(verb) {
            return Err("the request's line and body name different words".into());
        }
        if args.len() != verb.args().len() {
            return Err(usage(verb));
        }
        let args = args
            .iter()
            .map(|a| a.parse())
            .collect::<Result<_, _>>()
            .map_err(|_| "the request's numbers are not numbers")?;
        let key = unhex::<32>(key).ok_or("the key is not 64 hex digits")?;
        let public = VerifyingKey::from_bytes(&key)
            .ok()
            .filter(|k| !k.is_weak())
            .ok_or("not an ed25519 key")?;
        public
            .verify_strict(body, &Signature::from_bytes(sig))
            .map_err(|_| "the signature does not match")?;
        let at = at.parse().map_err(|_| "the time is not a number")?;
        let rules = rules.parse().map_err(|_| "the rules are not a number")?;
        let log = std::str::from_utf8(rest.get(1..).unwrap_or_default())
            .map_err(|_| "the grant log is not UTF-8")?;
        Ok(Self {
            verb,
            key,
            at,
            rules,
            args,
            log: log.to_owned(),
        })
    }
}

/// The refusal of a first line with the wrong number of words: JOIN, LEAVE and RESET take none after `rules`.
fn usage(verb: Verb) -> String {
    let mut s = "the request's first line is not VERB key at rules".to_owned();
    for a in verb.args() {
        s.push_str(&format!(" {a}"));
    }
    s
}

/// The installation's market key from `path` (`data\market.key`), made and saved there on first use.
pub fn load_key(path: &Path) -> io::Result<SigningKey> {
    match std::fs::read(path) {
        Ok(b) => <[u8; 32]>::try_from(b)
            .map(|b| SigningKey::from_bytes(&b))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: not 32 bytes", path.display()),
                )
            }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut b = [0u8; 32];
            getrandom::fill(&mut b).map_err(|e| io::Error::other(e.to_string()))?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, b)?;
            Ok(SigningKey::from_bytes(&b))
        }
        Err(e) => Err(e),
    }
}

/// Sends `request` ([`Request::signed`]) to the desk at `desk`: `Ok` with the words after `OK`, `Err` with the reason
/// after `NO` (or what went wrong on the way).
pub fn send(desk: SocketAddr, request: &[u8]) -> Result<String, String> {
    exchange(desk, request).and_then(|said| said)
}

/// [`send`], telling an answer from none: the outer `Err` is no answer (no server, a reset connection, a timeout, a line
/// that is neither `OK` nor `NO`), so the request may or may not have reached the server; the inner one is `NO`'s
/// reason.
pub fn exchange(desk: SocketAddr, request: &[u8]) -> Result<Result<String, String>, String> {
    let mut c =
        TcpStream::connect_timeout(&desk, Duration::from_secs(6)).map_err(|e| e.to_string())?;
    c.set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    c.write_all(request).map_err(|e| e.to_string())?;
    let mut got = Vec::new();
    c.take(1024)
        .read_to_end(&mut got)
        .map_err(|e| e.to_string())?;
    let got = String::from_utf8_lossy(&got);
    let line = got.lines().next().unwrap_or_default();
    match line.split_once(' ') {
        Some(("OK", rest)) => Ok(Ok(rest.to_owned())),
        Some(("NO", why)) => Ok(Err(why.to_owned())),
        _ if line == "OK" => Ok(Ok(String::new())),
        _ => Err(format!("the desk answered {line:?}")),
    }
}

/// The key's last accepted time in an [`OLD`] refusal.
#[must_use]
pub fn last_at(why: &str) -> Option<u64> {
    why.strip_prefix(OLD)?.rsplit(' ').next()?.parse().ok()
}

/// Signs `verb` with its numbers `args` ([`Verb::args`]) and `log` ([`Verb::logged`]'s words) at `at` (Unix ms, the
/// PC's clock) and sends it to the desk at `desk`, as [`send`]. Refused as [`OLD`] because the key's last request is not
/// before `at` (the PC's clock went back after it, or two requests in one millisecond), it signs again once just after
/// that time. No outbox: the Launcher's ledger words go through [`Client`].
pub fn ask(
    desk: SocketAddr,
    key: &SigningKey,
    verb: Verb,
    args: &[u64],
    log: &str,
    at: u64,
) -> Result<String, String> {
    let signed = |at| {
        Request {
            verb,
            key: *key.verifying_key().as_bytes(),
            at,
            rules: crate::collection::RULES,
            args: args.to_vec(),
            log: log.to_owned(),
        }
        .signed(key)
    };
    match send(desk, &signed(at)?) {
        Err(why) => match last_at(&why) {
            Some(last) if last >= at => send(desk, &signed(last + 1)?),
            _ => Err(why),
        },
        answer => answer,
    }
}

// ---- the Launcher's side: the opt-in, the outbox and the log lines (step 4.1) ----

/// The installation's market state, beside [`KEY_FILE`]: in the market or not, the last request's time and the one
/// request in flight. Outside the profile, so a profile reset keeps it (PLAN.md 3.6).
pub const STATE_FILE: &str = "market.json";
/// COLLECT pages one visit takes at most (each [`COLLECT_PAGE`] rows); the rest comes at the next visit.
const COLLECT_PAGES: usize = 20;

/// Words whose answer moves the server's ledger or the log: kept in the outbox until an answer comes, and sent again
/// byte for byte before any other request. RESET waits there too: offline, it goes before the next market request.
fn outboxed(verb: Verb) -> bool {
    matches!(
        verb,
        Verb::List | Verb::Cancel | Verb::Buy | Verb::Collect | Verb::StockBuy | Verb::Reset
    )
}

/// [`STATE_FILE`]'s contents.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Saved {
    /// JOIN answered OK and no LEAVE since.
    pub joined: bool,
    /// The newest `at` this installation signed: each new request goes strictly above it.
    pub last_at: u64,
    /// Sales LEAVE said wait for COLLECT.
    #[serde(default)]
    pub waiting: u32,
    /// The request sent and not answered yet.
    pub outbox: Option<Outbox>,
}

/// A request in flight: its verb and numbers (the answer's lines need them), its time and its bytes as sent.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outbox {
    pub verb: String,
    pub args: Vec<u64>,
    pub at: u64,
    pub bytes: String,
}

/// Why a market request did not go through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// No answer: an outbox request stays there and goes again first next time.
    Unanswered(String),
    /// The server's `NO` reason, or a refusal before sending.
    Refused(String),
    /// This PC's files (the key, [`STATE_FILE`], the log) could not be read or written.
    Disk(String),
}

/// How a [`Client`] reaches the desk: [`exchange`]; a test swaps in one that loses the answer.
pub type Transport = fn(SocketAddr, &[u8]) -> Result<Result<String, String>, String>;

/// One row of the own stock list or of BROWSE: `id` is the slot or the listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: u64,
    pub card: u32,
    pub price: u32,
    /// A stock slot bought today.
    pub sold: bool,
}

/// The own stock list of the day: purchases left under a daily limit, and the rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StockList {
    pub left: Option<u32>,
    pub rows: Vec<Row>,
}

/// One of this installation's listings still on the market, from the log: its `list` line without an `unlist` or
/// `market_sell` line of the same id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub id: u64,
    pub card: u32,
    pub price: u32,
    pub at: time::OffsetDateTime,
}

/// The `why` of the lines an answer writes: the ledger's id (or the request's time and the slot), so a line is
/// written once however often its answer comes.
fn why(kind: &str, id: u64) -> String {
    format!("market {kind} {id}")
}

/// `a:b:c` as numbers.
fn nums(s: &str) -> Option<Vec<u64>> {
    s.split(':').map(|n| n.parse().ok()).collect()
}

/// The installation's listings still on the market, from the log's text ([`Listed`]).
#[must_use]
pub fn listed(log: &str) -> Vec<Listed> {
    let entries: Vec<crate::collection::Entry> = log
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let closed: std::collections::HashSet<&str> = entries
        .iter()
        .filter_map(|e| {
            e.why
                .strip_prefix("market unlist ")
                .or_else(|| e.why.strip_prefix("market sell "))
        })
        .collect();
    entries
        .iter()
        .filter(|e| e.kind == crate::collection::Kind::List)
        .filter_map(|e| {
            let n = nums(e.why.strip_prefix("market list ")?)?;
            let [id, card, price] = n[..] else {
                return None;
            };
            if closed.contains(id.to_string().as_str()) {
                return None;
            }
            Some(Listed {
                id,
                card: u32::try_from(card).ok()?,
                price: u32::try_from(price).ok()?,
                at: e.at,
            })
        })
        .collect()
}

/// The log lines an OK answer to `verb` (signed at `at`) asks for, less those `log` already holds: `list` for LIST,
/// `unlist` for CANCEL, a COLLECT's returns and LEAVE's, `market_buy` for BUY, `market_sell` for a COLLECT's sales,
/// `stock_buy` for STOCKBUY.
#[must_use]
pub fn lines_for(verb: Verb, at: u64, answer: &str, log: &str) -> Vec<crate::collection::Line> {
    use crate::collection::{Kind, Line};
    let line = |kind, card: u64, tokens: i64, why| Line {
        kind,
        card: u32::try_from(card).ok(),
        tokens,
        why,
    };
    let mut words = answer.split(' ');
    let mut out = vec![];
    match (verb, words.next()) {
        (Verb::List, Some("listed")) => {
            if let Some(&[id, card, price]) = words.next().and_then(nums).as_deref() {
                let w = format!("market list {id}:{card}:{price}");
                out.push(line(Kind::List, card, 0, w));
            }
        }
        (Verb::Cancel, Some("cancelled")) => {
            if let Some(&[id, card]) = words.next().and_then(nums).as_deref() {
                out.push(line(Kind::Unlist, card, 0, why("unlist", id)));
            }
        }
        (Verb::Buy, Some("bought")) => {
            if let Some(&[id, card, price]) = words.next().and_then(nums).as_deref() {
                out.push(line(Kind::MarketBuy, card, -(price as i64), why("buy", id)));
            }
        }
        (Verb::StockBuy, Some("stockbought")) => {
            if let Some(&[slot, card, price]) = words.next().and_then(nums).as_deref() {
                let w = format!("stock buy {at} slot {slot}");
                out.push(line(Kind::StockBuy, card, -(price as i64), w));
            }
        }
        (Verb::Collect, Some("collected")) => {
            let mut sold = true;
            for w in words {
                match (w, nums(w).as_deref()) {
                    ("sold", _) => sold = true,
                    ("returned", _) => sold = false,
                    (_, Some(&[id, card, price])) if sold => {
                        out.push(line(Kind::MarketSell, card, price as i64, why("sell", id)));
                    }
                    (_, Some(&[id, card])) if !sold => {
                        out.push(line(Kind::Unlist, card, 0, why("unlist", id)));
                    }
                    _ => {}
                }
            }
        }
        (Verb::Leave, Some("left")) => {
            // LEAVE names the returned cards, not their ids: each takes the oldest listing of its card
            let mut open = listed(log);
            let mut returned = false;
            for w in words {
                match (w, w.parse::<u32>()) {
                    ("returned", _) => returned = true,
                    ("waiting", _) => returned = false,
                    (_, Ok(card)) if returned => {
                        if let Some(i) = open.iter().position(|l| l.card == card) {
                            let l = open.remove(i);
                            out.push(line(Kind::Unlist, card.into(), 0, why("unlist", l.id)));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    let had: std::collections::HashSet<String> = log
        .lines()
        .filter_map(|l| serde_json::from_str::<crate::collection::Entry>(l).ok())
        .map(|e| e.why)
        .collect();
    out.retain(|l| !had.contains(&l.why));
    out
}

/// The parts of a STOCK answer (`stock [left n] slot:card:price[:sold] ...`) or a BROWSE one (`listings id:card:price
/// ...`) after their first word.
fn rows(words: std::str::SplitWhitespace<'_>) -> Vec<Row> {
    words
        .filter_map(|w| {
            let (head, sold) = w.strip_suffix(":sold").map_or((w, false), |h| (h, true));
            let n = nums(head)?;
            let [id, card, price] = n[..] else {
                return None;
            };
            Some(Row {
                id,
                card: u32::try_from(card).ok()?,
                price: u32::try_from(price).ok()?,
                sold,
            })
        })
        .collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The Launcher's market client for one installation and its profile's log (PLAN.md 3.5, the 3.2 and 3.3 gates):
/// one request in flight, kept in [`STATE_FILE`] with its bytes before it goes out, sent again byte for byte before any
/// other request (JOIN too, also after a restart) until an answer comes; each new request's `at` strictly above the
/// last; an answer's lines written once ([`lines_for`] skips the ones the log holds), and the outbox cleared only
/// after they are written.
pub struct Client {
    pub desk: SocketAddr,
    data: std::path::PathBuf,
    log: crate::collection::Log,
    key: SigningKey,
    pub transport: Transport,
    /// A request sent again from the outbox and refused, as `VERB: why`: the player's money action did not happen.
    resent_refused: std::cell::Cell<Option<String>>,
}

impl Client {
    /// The client of the installation whose `data` folder holds [`KEY_FILE`] (made on first use) and [`STATE_FILE`],
    /// for the profile's collection folder `collection`.
    pub fn open(desk: SocketAddr, data: &Path, collection: &Path) -> Result<Self, Fail> {
        Ok(Self {
            desk,
            key: load_key(&data.join(KEY_FILE)).map_err(disk)?,
            data: data.to_owned(),
            log: crate::collection::Log::new(collection),
            transport: exchange,
            resent_refused: std::cell::Cell::new(None),
        })
    }

    /// The outbox request sent again and refused since the last call, as `VERB: why`.
    pub fn resent_refused(&self) -> Option<String> {
        self.resent_refused.take()
    }

    /// [`STATE_FILE`] as saved, for the page; none, or one that cannot be read, is a key never in the market
    /// ([`Self::load`] reports the unreadable one).
    #[must_use]
    pub fn saved(data: &Path) -> Saved {
        std::fs::read(data.join(STATE_FILE))
            .ok()
            .and_then(|t| serde_json::from_slice(&t).ok())
            .unwrap_or_default()
    }

    /// [`STATE_FILE`] before a request: none is a key never in the market; one that does not parse is moved aside to
    /// `market.json.bad-<ms>` and reported once (its `joined` and outbox are lost), never taken silently as new.
    pub fn load(data: &Path) -> Result<Saved, Fail> {
        let path = data.join(STATE_FILE);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Saved::default()),
            Err(e) => return Err(disk(format!("{}: {e}", path.display()))),
        };
        serde_json::from_slice(&bytes).or_else(|e| {
            let bad = path.with_extension(format!("json.bad-{}", now_ms()));
            std::fs::rename(&path, &bad).map_err(disk)?;
            Err(disk(format!(
                "{} could not be read ({e}), it is kept as {}; this installation starts outside the market",
                path.display(),
                bad.display()
            )))
        })
    }

    /// Writes `s` as [`STATE_FILE`]: a temporary file flushed to the disk, then renamed over it.
    pub fn save(&self, s: &Saved) -> Result<(), Fail> {
        let path = self.data.join(STATE_FILE);
        let tmp = path.with_extension("json.tmp");
        std::fs::create_dir_all(&self.data).map_err(disk)?;
        let mut f = std::fs::File::create(&tmp).map_err(disk)?;
        f.write_all(serde_json::to_string(s).map_err(disk)?.as_bytes())
            .and_then(|()| f.sync_all())
            .map_err(disk)?;
        drop(f);
        std::fs::rename(&tmp, &path).map_err(disk)
    }

    /// Signs `verb` at `at` with the log as it is now, saves it as the newest time and, for an [`outboxed`] word, in
    /// the outbox: on disk before it goes out.
    fn sign(&self, s: &mut Saved, verb: Verb, args: &[u64], at: u64) -> Result<Vec<u8>, Fail> {
        let log = if verb.logged() {
            self.log.lines().map_err(disk)?
        } else {
            String::new()
        };
        let bytes = Request {
            verb,
            key: *self.key.verifying_key().as_bytes(),
            at,
            rules: crate::collection::RULES,
            args: args.to_vec(),
            log,
        }
        .signed(&self.key)
        .map_err(Fail::Refused)?;
        s.last_at = s.last_at.max(at);
        if outboxed(verb) {
            s.outbox = Some(Outbox {
                verb: verb.word().to_owned(),
                args: args.to_vec(),
                at,
                bytes: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        self.save(s)?;
        Ok(bytes)
    }

    /// Sends signed bytes and settles the answer. An [`OLD`] refusal past `at` (this PC's clock behind the key's last
    /// request) means the server took nothing: signed again once just after it. A RESET sent again whose own time is
    /// the key's last was taken (its answer was lost).
    fn deliver(
        &self,
        s: &mut Saved,
        verb: Verb,
        args: &[u64],
        mut at: u64,
        mut bytes: Vec<u8>,
    ) -> Result<String, Fail> {
        let mut again = true;
        loop {
            let said = (self.transport)(self.desk, &bytes).map_err(Fail::Unanswered)?;
            let last = said.as_ref().err().and_then(|why| last_at(why));
            match last {
                Some(last) if last > at && again => {
                    again = false;
                    at = last + 1;
                    bytes = self.sign(s, verb, args, at)?;
                }
                Some(last) if last == at && verb == Verb::Reset => {
                    return self.settle(s, verb, at, Ok("reset".into()));
                }
                _ => return self.settle(s, verb, at, said),
            }
        }
    }

    /// An answer: its lines written (a disk error keeps the outbox, so the request goes again and its lines then),
    /// the state moved, the outbox cleared.
    fn settle(
        &self,
        s: &mut Saved,
        verb: Verb,
        at: u64,
        said: Result<String, String>,
    ) -> Result<String, Fail> {
        if let Ok(answer) = &said {
            let lines = lines_for(verb, at, answer, &self.log.lines().map_err(disk)?);
            if !lines.is_empty() {
                self.log
                    .append_lines_at(crate::collection::now(), |_| Ok(lines))
                    .map_err(disk)?;
            }
            match verb {
                Verb::Join => s.joined = true,
                Verb::Leave => {
                    s.joined = false;
                    s.waiting = answer
                        .rsplit_once("waiting ")
                        .and_then(|(_, n)| n.parse().ok())
                        .unwrap_or(0);
                }
                Verb::Collect if answer == "collected" => s.waiting = 0,
                _ => {}
            }
        } else if said.as_ref().is_err_and(|why| why == NOT_IN) {
            // the server holds no LEAVE we know of (its answer was lost): out, so the page offers JOIN again
            s.joined = false;
        }
        if outboxed(verb) {
            s.outbox = None;
        }
        self.save(s)?;
        said.map_err(Fail::Refused)
    }

    /// The request in flight, sent again as it was. Its refusal clears it and is not this call's (it changed nothing on
    /// the server); no answer keeps it.
    /// [`Self::resent_refused`] tells the player.
    pub fn flush(&self) -> Result<(), Fail> {
        let mut s = Self::load(&self.data)?;
        let Some(o) = s.outbox.clone() else {
            return Ok(());
        };
        let Some(verb) = Verb::parse(&o.verb) else {
            s.outbox = None;
            return self.save(&s);
        };
        let (at, bytes) = if verb == Verb::Reset {
            // RESET carries no log: signed afresh, so one kept past the server's clock slack still goes
            let at = now_ms().max(s.last_at + 1);
            (at, self.sign(&mut s, verb, &[], at)?)
        } else {
            (o.at, o.bytes.into_bytes())
        };
        match self.deliver(&mut s, verb, &o.args, at, bytes) {
            Ok(_) => Ok(()),
            // a key the server holds nothing for: nothing to reset
            Err(Fail::Refused(why)) if verb == Verb::Reset && why == NOT_IN => Ok(()),
            Err(Fail::Refused(why)) => {
                self.resent_refused.set(Some(format!("{}: {why}", o.verb)));
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// `verb` with its numbers, after the request in flight: the answer's words after `OK`.
    pub fn ask(&self, verb: Verb, args: &[u64]) -> Result<String, Fail> {
        self.flush()?;
        let mut s = Self::load(&self.data)?;
        let at = now_ms().max(s.last_at + 1);
        let bytes = self.sign(&mut s, verb, args, at)?;
        self.deliver(&mut s, verb, args, at, bytes)
    }

    /// START OVER in the market (PLAN.md 3.6): a RESET into the outbox, in place of a request in flight (the RESET
    /// undoes whatever that did on the server). Sent by the next [`Self::flush`] or request, also outside the market as
    /// this PC sees it (a JOIN whose answer was lost); nothing for a key that never signed. A sale waiting since LEAVE is
    /// dropped with the old log. Gives the state before, for [`Self::save`] to put back when the profile reset fails.
    pub fn queue_reset(&self) -> Result<Saved, Fail> {
        let before = Self::load(&self.data)?;
        if before.last_at == 0 {
            return Ok(before);
        }
        let mut s = Saved {
            outbox: None,
            waiting: 0,
            ..before.clone()
        };
        let at = now_ms().max(s.last_at + 1);
        self.sign(&mut s, Verb::Reset, &[], at)?;
        Ok(before)
    }

    /// COLLECT until it answers `collected` alone: the sales paid and the listings past 7 days back, in the log.
    pub fn collect(&self) -> Result<(), Fail> {
        for _ in 0..COLLECT_PAGES {
            if self.ask(Verb::Collect, &[])? == "collected" {
                break;
            }
        }
        Ok(())
    }

    /// The own stock list of the day.
    pub fn stock(&self) -> Result<StockList, Fail> {
        let said = self.ask(Verb::Stock, &[])?;
        let mut words = said.split_whitespace();
        words.next(); // "stock"
        let mut left = None;
        let mut rest = words.clone();
        if rest.next() == Some("left") {
            left = rest.next().and_then(|n| n.parse().ok());
            words = rest;
        }
        Ok(StockList {
            left,
            rows: rows(words),
        })
    }

    /// One BROWSE page of other installations' listings after the id `after`.
    pub fn browse(&self, after: u64) -> Result<Vec<Row>, Fail> {
        let said = self.ask(Verb::Browse, &[after])?;
        let mut words = said.split_whitespace();
        words.next(); // "listings"
        Ok(rows(words))
    }
}

fn disk(e: impl std::fmt::Display) -> Fail {
    Fail::Disk(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn join() -> Request {
        Request {
            verb: Verb::Join,
            key: *key(7).verifying_key().as_bytes(),
            at: 1_762_000_000_000,
            rules: 1,
            args: Vec::new(),
            log: "{\"seq\":1}\n{\"seq\":2}\n".into(),
        }
    }

    /// Splits sent bytes into the line's words after `MARKET` and the body.
    fn split(sent: &[u8]) -> (Vec<String>, Vec<u8>) {
        let nl = sent.iter().position(|&b| b == b'\n').unwrap();
        let line = std::str::from_utf8(&sent[..nl]).unwrap();
        let words = line.split(' ').skip(1).map(String::from).collect();
        (words, sent[nl + 1..].to_vec())
    }

    fn open(sent: &[u8]) -> Result<Request, String> {
        let (words, body) = split(sent);
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let (verb, size, sig) = Request::head(&words)?;
        assert_eq!(size, body.len());
        Request::open(verb, &sig, &body)
    }

    /// Acceptance 3.1 (a): a signed log verifies, and fails with any one byte of the body changed.
    #[test]
    fn a_signed_log_verifies_and_fails_with_one_byte_changed() {
        let sent = join().signed(&key(7)).unwrap();
        let got = open(&sent).unwrap();
        assert_eq!(got, join());
        let body_at = sent.iter().position(|&b| b == b'\n').unwrap() + 1;
        for i in body_at..sent.len() {
            let mut bad = sent.clone();
            bad[i] ^= 0x01;
            assert!(open(&bad).is_err(), "byte {i} changed and still verified");
        }
    }

    #[test]
    fn another_key_or_another_word_is_refused() {
        // signed by key 8, naming key 7's public half
        let mut sent = join().signed(&key(8)).unwrap();
        let k8 = hex(key(8).verifying_key().as_bytes());
        let k7 = hex(key(7).verifying_key().as_bytes());
        let s = String::from_utf8(sent.clone())
            .unwrap()
            .replacen(&k8, &k7, 1);
        sent = s.into_bytes();
        assert_eq!(open(&sent).unwrap_err(), "the signature does not match");
        // a LEAVE line over a JOIN body
        let sent = join().signed(&key(7)).unwrap();
        let s = String::from_utf8(sent)
            .unwrap()
            .replacen("MARKET JOIN", "MARKET LEAVE", 1);
        assert!(open(s.as_bytes()).unwrap_err().contains("different words"));
    }

    #[test]
    fn bad_lines_are_refused_before_the_body() {
        let sig = "ab".repeat(64);
        for words in [
            vec!["JOIN"],
            vec!["SELL", "10", sig.as_str()],
            vec!["JOIN", "0", sig.as_str()],
            vec!["JOIN", "99999999", sig.as_str()],
            vec!["JOIN", "-1", sig.as_str()],
            vec!["JOIN", "10", "zz"],
        ] {
            assert!(Request::head(&words).is_err(), "{words:?}");
        }
        // a body without a first line, or with a short one
        let s = [0u8; 64];
        assert!(Request::open(Verb::Join, &s, &[b'x'; 300]).is_err());
        assert!(Request::open(Verb::Join, &s, b"JOIN abc\n").is_err());
        assert!(Request::open(Verb::Join, &s, b"\xff\xfe\n").is_err());
    }

    /// No name travels: a first line with a fifth word (an older Launcher's club name) is refused.
    #[test]
    fn a_join_carries_no_name() {
        let sent = join().signed(&key(7)).unwrap();
        let (_, body) = split(&sent);
        let head = String::from_utf8(body).unwrap();
        let head = head.lines().next().unwrap().to_owned();
        assert_eq!(head.split(' ').count(), 4, "{head}");
        let named = format!("{head} Real Club\n").into_bytes();
        let sig = key(7).sign(&named).to_bytes();
        assert!(
            Request::open(Verb::Join, &sig, &named)
                .unwrap_err()
                .contains("not VERB key at rules")
        );
    }

    /// M5: a log over the cap is refused before sending, with the size in the reason; one that fits is signed.
    #[test]
    fn a_log_over_the_cap_is_refused_with_its_size() {
        let mut r = join();
        r.log = "x".repeat(MAX_BODY);
        let why = r.signed(&key(7)).unwrap_err();
        assert!(why.contains("over the market's limit of 8192 KiB"), "{why}");
        r.log.truncate(MAX_BODY - 200);
        assert!(r.signed(&key(7)).is_ok());
    }

    /// The words released before the ledger send the same bytes: no number after `rules`, the log as it is.
    #[test]
    fn join_leave_and_reset_bytes_are_unchanged() {
        let k = key(7);
        let pk = hex(k.verifying_key().as_bytes());
        let [mut leave, mut reset] = [join(), join()];
        (leave.verb, leave.log) = (Verb::Leave, String::new());
        (reset.verb, reset.log) = (Verb::Reset, String::new());
        for (r, body) in [
            (
                join(),
                format!("JOIN {pk} 1762000000000 1\n{{\"seq\":1}}\n{{\"seq\":2}}\n"),
            ),
            (leave, format!("LEAVE {pk} 1762000000000 1\n")),
            (reset, format!("RESET {pk} 1762000000000 1\n")),
        ] {
            let sig = hex(&k.sign(body.as_bytes()).to_bytes());
            let want = format!("MARKET {} {} {sig}\n{body}", r.verb.word(), body.len());
            assert_eq!(String::from_utf8(r.signed(&k).unwrap()).unwrap(), want);
        }
    }

    /// The ledger's words carry their numbers on the first line, LIST and BUY the log after it; a wrong count of
    /// numbers is refused on both ends.
    #[test]
    fn ledger_words_carry_their_numbers() {
        let mut r = join();
        (r.verb, r.args) = (Verb::Buy, vec![17, 57, 228]);
        let sent = r.signed(&key(7)).unwrap();
        let (_, body) = split(&sent);
        assert!(
            String::from_utf8(body)
                .unwrap()
                .contains(" 1 17 57 228\n{\"seq\":1}")
        );
        assert_eq!(open(&sent).unwrap(), r);
        r.args.pop();
        assert!(
            r.signed(&key(7))
                .unwrap_err()
                .ends_with("rules listing card price")
        );
        let body =
            format!("BUY {} 1 1 17 57\n", hex(key(7).verifying_key().as_bytes())).into_bytes();
        let sig = key(7).sign(&body).to_bytes();
        assert!(
            Request::open(Verb::Buy, &sig, &body)
                .unwrap_err()
                .ends_with("rules listing card price")
        );
        let body =
            format!("CANCEL {} 1 1 x\n", hex(key(7).verifying_key().as_bytes())).into_bytes();
        let sig = key(7).sign(&body).to_bytes();
        assert!(
            Request::open(Verb::Cancel, &sig, &body)
                .unwrap_err()
                .contains("not numbers")
        );
    }

    /// Step 3.3: STOCK carries the log and no number, STOCKBUY its slot, card and price and the log; both open again
    /// as signed.
    #[test]
    fn stock_words_carry_the_log_and_their_numbers() {
        let pk = hex(key(7).verifying_key().as_bytes());
        let mut r = join();
        r.verb = Verb::Stock;
        let sent = r.signed(&key(7)).unwrap();
        let (words, body) = split(&sent);
        assert_eq!(words[0], "STOCK");
        assert_eq!(
            String::from_utf8(body).unwrap(),
            format!("STOCK {pk} 1762000000000 1\n{{\"seq\":1}}\n{{\"seq\":2}}\n")
        );
        assert_eq!(open(&sent).unwrap(), r);
        (r.verb, r.args) = (Verb::StockBuy, vec![3, 41, 216]);
        let sent = r.signed(&key(7)).unwrap();
        let (_, body) = split(&sent);
        assert!(String::from_utf8(body).unwrap().starts_with(&format!(
            "STOCKBUY {pk} 1762000000000 1 3 41 216\n{{\"seq\":1}}"
        )));
        assert_eq!(open(&sent).unwrap(), r);
        assert!(Verb::Stock.logged() && Verb::StockBuy.logged() && Verb::Owed.logged());
        assert_eq!(Verb::parse("OWED"), Some(Verb::Owed));
    }

    #[test]
    fn an_old_refusal_gives_the_last_time() {
        assert_eq!(last_at(&format!("{OLD}, the last at 1762")), Some(1762));
        assert_eq!(last_at("the signature does not match"), None);
    }

    #[test]
    fn the_key_is_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join(KEY_FILE);
        let a = load_key(&path).unwrap();
        let b = load_key(&path).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes());
        std::fs::write(&path, b"short").unwrap();
        assert!(load_key(&path).is_err());
    }
    /// What a [`Transport`] gives back.
    type Answer = Result<Result<String, String>, String>;

    thread_local! {
        /// The answers [`scripted`] gives, in order, and the verbs it was sent.
        static SCRIPT: std::cell::RefCell<(std::collections::VecDeque<Answer>, Vec<String>)> =
            std::cell::RefCell::default();
    }

    /// A desk that answers from [`SCRIPT`]; `Err` is an answer lost on the way.
    fn scripted(_: SocketAddr, bytes: &[u8]) -> Answer {
        SCRIPT.with(|s| {
            let mut s = s.borrow_mut();
            s.1.push(String::from_utf8_lossy(bytes).into_owned());
            s.0.pop_front().expect("an answer scripted")
        })
    }

    fn script(answers: impl IntoIterator<Item = Answer>) {
        SCRIPT.with(|s| *s.borrow_mut() = (answers.into_iter().collect(), Vec::new()));
    }

    /// The verbs sent, in order.
    fn sent() -> Vec<String> {
        SCRIPT.with(|s| {
            s.borrow()
                .1
                .iter()
                .map(|r| split(r.as_bytes()).0[0].clone())
                .collect()
        })
    }

    fn scripted_client(dir: &Path) -> Client {
        let mut c = Client::open(
            ([127, 0, 0, 1], 9).into(),
            &dir.join("data"),
            &dir.join("coll"),
        )
        .unwrap();
        c.transport = scripted;
        c
    }

    /// M1: LEAVE's answer is lost, so the client still thinks it is in the market; the next word's `NOT_IN` takes it
    /// out, and JOIN works again.
    #[test]
    fn a_lost_leave_answer_is_recovered_from_not_in() {
        let dir = tempfile::tempdir().unwrap();
        let c = scripted_client(dir.path());
        let data = dir.path().join("data");
        script([
            Ok(Ok("0 lines".into())),
            Err("connection reset".into()),
            Ok(Err(NOT_IN.into())),
            Ok(Ok("0 lines".into())),
        ]);
        c.ask(Verb::Join, &[]).unwrap();
        assert!(matches!(c.ask(Verb::Leave, &[]), Err(Fail::Unanswered(_))));
        assert!(Client::saved(&data).joined, "no answer, no change");
        assert_eq!(c.stock().unwrap_err(), Fail::Refused(NOT_IN.into()));
        assert!(!Client::saved(&data).joined, "NOT_IN: out of the market");
        c.ask(Verb::Join, &[]).unwrap();
        assert!(Client::saved(&data).joined);
        assert_eq!(sent(), ["JOIN", "LEAVE", "STOCK", "JOIN"]);
    }

    /// M3: a `market.json` that does not parse is kept aside and reported, not taken as a new installation; the next
    /// request starts from nothing.
    #[test]
    fn an_unreadable_state_file_is_kept_aside_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let c = scripted_client(dir.path());
        let data = dir.path().join("data");
        std::fs::write(data.join(STATE_FILE), b"{\"joined\":tr").unwrap();
        script([]);
        assert!(matches!(c.flush(), Err(Fail::Disk(e)) if e.contains("bad-")));
        assert!(!data.join(STATE_FILE).exists());
        let bad: Vec<_> = std::fs::read_dir(&data)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with("market.json.bad-"))
            .collect();
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert_eq!(
            std::fs::read(data.join(&bad[0])).unwrap(),
            b"{\"joined\":tr"
        );
        assert_eq!(Client::load(&data).unwrap(), Saved::default());
        assert!(
            sent().is_empty(),
            "nothing sent from a state it could not read"
        );
    }

    /// F3, F2, F4: START OVER queues RESET for a key that ever signed, even one this PC thinks is out (its JOIN's
    /// answer lost); the kept RESET is signed afresh when it goes; a `NOT_IN` refusal of it clears it silently; a sale
    /// waiting since LEAVE goes with the old log. A key that never signed queues nothing.
    #[test]
    fn start_over_resets_a_key_that_ever_signed() {
        let dir = tempfile::tempdir().unwrap();
        let c = scripted_client(dir.path());
        let data = dir.path().join("data");
        assert_eq!(c.queue_reset().unwrap().last_at, 0);
        assert!(
            Client::saved(&data).outbox.is_none(),
            "never signed: nothing to reset"
        );
        script([Err("connection reset".into()), Ok(Ok("reset".into()))]);
        assert!(matches!(c.ask(Verb::Join, &[]), Err(Fail::Unanswered(_))));
        let mut s = Client::saved(&data);
        assert!(!s.joined && s.last_at > 0);
        s.waiting = 2;
        c.save(&s).unwrap();
        c.queue_reset().unwrap();
        let mut s = Client::saved(&data);
        assert_eq!(s.waiting, 0);
        let o = s.outbox.as_mut().expect("RESET queued");
        assert_eq!(o.verb, "RESET");
        // kept a long time: its own time is old
        o.at = 5;
        c.save(&s).unwrap();
        c.flush().unwrap();
        let (_, body) = SCRIPT.with(|s| split(s.borrow().1[1].as_bytes()));
        let head = String::from_utf8(body).unwrap();
        let at: u64 = head.split(' ').nth(2).unwrap().trim().parse().unwrap();
        assert!(head.starts_with("RESET ") && at > s.last_at, "{head}");
        assert!(Client::saved(&data).outbox.is_none());
        // the same once the server holds nothing for the key: no note
        script([Ok(Err(NOT_IN.into()))]);
        c.queue_reset().unwrap();
        c.flush().unwrap();
        assert_eq!(c.resent_refused(), None);
        assert!(Client::saved(&data).outbox.is_none());
        assert_eq!(sent(), ["RESET"]);
    }

    /// m1: a request from the outbox refused when sent again is reported once, and the new request still goes.
    #[test]
    fn a_resent_request_refused_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let c = scripted_client(dir.path());
        script([
            Ok(Ok("0 lines".into())),
            Err("connection reset".into()),
            Ok(Err("the listing is gone".into())),
            Ok(Ok("listings".into())),
        ]);
        c.ask(Verb::Join, &[]).unwrap();
        assert!(matches!(
            c.ask(Verb::Buy, &[3, 12, 90]),
            Err(Fail::Unanswered(_))
        ));
        assert_eq!(c.resent_refused(), None);
        assert!(c.browse(0).unwrap().is_empty());
        assert_eq!(
            c.resent_refused().as_deref(),
            Some("BUY: the listing is gone")
        );
        assert_eq!(c.resent_refused(), None);
        assert!(Client::saved(&dir.path().join("data")).outbox.is_none());
    }

    /// Step 4.1: each answer writes its lines, and the same answer again writes none; open listings come from the log.
    #[test]
    fn an_answer_writes_its_lines_once() {
        use crate::collection::{Kind, Line, Log};
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        let write = |verb, at, answer: &str| {
            let lines: Vec<Line> = lines_for(verb, at, answer, &log.lines().unwrap());
            let n = lines.len();
            if n > 0 {
                log.append_lines_at(crate::collection::now(), |_| Ok(lines))
                    .unwrap();
            }
            n
        };
        for (verb, answer, n) in [
            (Verb::List, "listed 7:57:228", 1),
            (Verb::List, "listed 7:57:228", 0),
            (Verb::List, "listed 8:58:100", 1),
            (Verb::List, "listed 9:58:100", 1),
            (Verb::Buy, "bought 3:12:90", 1),
            (Verb::Buy, "bought 3:12:90", 0),
            (Verb::StockBuy, "stockbought 2:41:216", 1),
            (Verb::StockBuy, "stockbought 2:41:216", 0),
            (Verb::Cancel, "cancelled 8:58", 1),
            (Verb::Collect, "collected sold 7:57:228 returned 8:58", 1),
            (Verb::Collect, "collected", 0),
        ] {
            assert_eq!(write(verb, 5, answer), n, "{verb:?} {answer}");
        }
        // a STOCKBUY of another request (another day's slot 2) is a new line
        assert_eq!(write(Verb::StockBuy, 6, "stockbought 2:41:216"), 1);
        let text = log.lines().unwrap();
        let open = listed(&text);
        assert_eq!(
            open.iter()
                .map(|l| (l.id, l.card, l.price))
                .collect::<Vec<_>>(),
            [(9, 58, 100)]
        );
        // LEAVE names cards: the oldest open listing of each is the one returned
        assert_eq!(write(Verb::Leave, 7, "left returned 58 waiting 1"), 1);
        assert!(listed(&log.lines().unwrap()).is_empty());
        let s = crate::collection::fold(&log.lines().unwrap()).state;
        assert_eq!(s.tokens, 228 - 90 - 216 - 216);
        assert_eq!(s.copies(57).listed, 0);
        assert_eq!(s.copies(58).listed, 0);
        let kinds: Vec<Kind> = text
            .lines()
            .map(|l| {
                serde_json::from_str::<crate::collection::Entry>(l)
                    .unwrap()
                    .kind
            })
            .collect();
        assert_eq!(kinds.iter().filter(|k| **k == Kind::StockBuy).count(), 2);
    }
}
