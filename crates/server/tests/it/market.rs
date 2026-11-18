//! Create-a-Team's market words on a real desk over loopback: `MARKET JOIN / LEAVE / RESET`, the ledger and the own
//! stock list signed by `forza::market`, the "not set up" answer without a value table, and the old words answering as
//! before.

use crate::common::*;
use ed25519_dalek::SigningKey;
use forza::collection::RULES;
use forza::market::{self, NOT_SET_UP, Request, Verb};
use forza_server::desk::{DeskConfig, start};
use forza_server::market::{Market, Stock, Table};
use forza_server::{Edition, Handle};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

fn desk(market: Option<Market>) -> Handle {
    let mut c = DeskConfig::new(Edition::E1718);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.market = market.map(Arc::new);
    start(c, capture().0).unwrap()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The deal's 12 starter lines of cards 1-12, chained as the Launcher writes them.
fn starter_log() -> String {
    let mut text = String::new();
    let mut last = String::new();
    for c in 1..=12u32 {
        let line = forza::collection::Entry {
            seq: u64::from(c),
            at: time::OffsetDateTime::now_utc(),
            rules: RULES,
            kind: forza::collection::Kind::Starter,
            card: Some(c),
            tokens: 0,
            why: "starter".into(),
            prev: if c == 1 {
                String::new()
            } else {
                forza::collection::hash(&last)
            },
        }
        .line();
        text.push_str(&line);
        text.push('\n');
        last = line;
    }
    text
}

fn request(verb: Verb, key: &SigningKey, at: u64, log: &str) -> Vec<u8> {
    request_args(verb, key, at, &[], log)
}

fn request_args(verb: Verb, key: &SigningKey, at: u64, args: &[u64], log: &str) -> Vec<u8> {
    Request {
        verb,
        key: *key.verifying_key().as_bytes(),
        at,
        rules: RULES,
        args: args.to_vec(),
        log: log.into(),
    }
    .signed(key)
    .unwrap()
}

/// `log` with one paid match more that deals `card`, chained to its last line.
fn with_match(log: &str, card: u32) -> String {
    let last = log.lines().last().unwrap();
    let line = forza::collection::Entry {
        seq: log.lines().count() as u64 + 1,
        at: time::OffsetDateTime::now_utc(),
        rules: RULES,
        kind: forza::collection::Kind::Match,
        card: Some(card),
        tokens: 50,
        why: "match".into(),
        prev: forza::collection::hash(last),
    }
    .line();
    format!("{log}{line}\n")
}

/// What the desk sends back to `bytes` until it hangs up or goes quiet for half a second.
fn answer(addr: SocketAddr, bytes: &[u8]) -> Vec<u8> {
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    c.write_all(bytes).unwrap();
    let mut got = Vec::new();
    let mut b = [0u8; 512];
    while let Ok(n) = c.read(&mut b) {
        if n == 0 {
            break;
        }
        got.extend_from_slice(&b[..n]);
    }
    got
}

/// Acceptance 3.1 (e), second half: without a value table every MARKET word answers the "not set up" line.
#[test]
fn without_a_value_table_every_market_word_is_not_set_up() {
    let d = desk(None);
    let key = SigningKey::from_bytes(&[3; 32]);
    let want = format!("NO {NOT_SET_UP}\n");
    for verb in [Verb::Join, Verb::Leave, Verb::Reset] {
        let sent = request(verb, &key, now_ms(), &starter_log());
        assert_eq!(String::from_utf8(answer(d.addr(), &sent)).unwrap(), want);
        assert_eq!(market::send(d.addr(), &sent).unwrap_err(), NOT_SET_UP);
    }
    for line in ["MARKET\n", "MARKET LIST 1 2\n", "MARKET JOIN x\n"] {
        assert_eq!(answer(d.addr(), line.as_bytes()), want.as_bytes(), "{line}");
    }
}

/// The whole round over loopback: a signed JOIN is checked and kept, the same request with one byte changed is
/// refused, a refused log says why, and LEAVE answers.
#[test]
fn a_signed_join_and_leave_over_the_desk() {
    let root = scratch("market_desk");
    let table = Table::of(
        &(1..=40)
            .map(|c| (c, forza::collection::Rarity::Regular, 80))
            .collect::<Vec<_>>(),
    );
    let d = desk(Some(Market::with(table, &root.join("market.db")).unwrap()));
    let key = SigningKey::from_bytes(&[4; 32]);
    let log = starter_log();
    let at = now_ms();
    let sent = request(Verb::Join, &key, at, &log);
    let mut bad = sent.clone();
    let last = bad.len() - 3;
    bad[last] ^= 1;
    assert_eq!(
        market::send(d.addr(), &bad).unwrap_err(),
        "the signature does not match"
    );
    assert_eq!(market::send(d.addr(), &sent).unwrap(), "12 lines");
    assert_eq!(
        market::send(d.addr(), &sent).unwrap_err(),
        format!("{}, the last at {at}", market::OLD)
    );
    // a log the check refuses: a 13th starter card
    let mut more = log.clone();
    let tail = log.lines().last().unwrap();
    let mut e: forza::collection::Entry = serde_json::from_str(tail).unwrap();
    (e.seq, e.card, e.prev) = (13, Some(13), forza::collection::hash(tail));
    more.push_str(&e.line());
    more.push('\n');
    let why = market::send(d.addr(), &request(Verb::Join, &key, at + 1, &more)).unwrap_err();
    assert!(why.starts_with("line 13: more starter cards"), "{why}");
    assert_eq!(
        market::send(d.addr(), &request(Verb::Leave, &key, at + 2, "")).unwrap(),
        "left"
    );
    // a body bigger than the cap is refused from the line, unread
    let line = format!("MARKET JOIN {} {}\n", market::MAX_BODY + 1, "ab".repeat(64));
    let got = String::from_utf8(answer(d.addr(), line.as_bytes())).unwrap();
    assert!(got.starts_with("NO a request of 1 to"), "{got}");
}

/// The old words answer byte for byte as on a desk without the market.
#[test]
fn old_words_answer_as_before() {
    let root = scratch("market_old_words");
    let table = Table::of(&[(1, forza::collection::Rarity::Regular, 80)]);
    let with = desk(Some(Market::with(table, &root.join("market.db")).unwrap()));
    let without = desk(None);
    for line in [
        "WANT SEAT 3\n",
        "WANT\n",
        "REPORT 100 5 0.4.1\n",
        "RELEASE 0123456789abcdef\n",
        "HEARTBEAT 0123456789abcdef\n",
        "RESERVE nope\n",
        "CREATE vs\n",
        "JOIN ABCDE\n",
        "SHOP 0123\n",
        "CLUB GET 1 2 3\n",
        "HOLD\n",
        "HELLO\n",
    ] {
        let (a, b) = (
            answer(with.addr(), line.as_bytes()),
            answer(without.addr(), line.as_bytes()),
        );
        assert!(!b.is_empty() || line == "HOLD\n", "{line}: no answer");
        assert_eq!(a, b, "{line}");
    }
}

/// M1: a JOIN made with the PC's clock 10 hours fast, then the clock set right: a plain request is refused as old,
/// and `market::ask` signs it again past the server's last time.
#[test]
fn a_clock_set_back_does_not_lock_the_key_out() {
    let root = scratch("market_clock");
    let table = Table::of(
        &(1..=40)
            .map(|c| (c, forza::collection::Rarity::Regular, 80))
            .collect::<Vec<_>>(),
    );
    let d = desk(Some(Market::with(table, &root.join("market.db")).unwrap()));
    let key = SigningKey::from_bytes(&[5; 32]);
    let ahead = now_ms() + 10 * 3600 * 1000;
    let join = request(Verb::Join, &key, ahead, &starter_log());
    assert_eq!(market::send(d.addr(), &join).unwrap(), "12 lines");
    let why = market::send(d.addr(), &request(Verb::Leave, &key, now_ms(), "")).unwrap_err();
    assert!(why.starts_with(market::OLD), "{why}");
    assert_eq!(
        market::ask(d.addr(), &key, Verb::Leave, &[], "", now_ms()).unwrap(),
        "left"
    );
}

/// Acceptance 3.2 over loopback: a listing, a purchase by a second key and the seller's COLLECT, signed and sent as
/// the Launcher sends them; no answer's bytes hold either key or its hash.
#[test]
fn a_listing_sells_over_the_desk() {
    let root = scratch("market_ledger");
    let table = Table::of(
        &(1..=40)
            .map(|c| (c, forza::collection::Rarity::Regular, 80))
            .collect::<Vec<_>>(),
    );
    let d = desk(Some(Market::with(table, &root.join("market.db")).unwrap()));
    let (seller, buyer) = (
        SigningKey::from_bytes(&[6; 32]),
        SigningKey::from_bytes(&[7; 32]),
    );
    let (sold, funds) = (
        with_match(&starter_log(), 13),
        with_match(&starter_log(), 14),
    );
    let at = now_ms();
    let steps: [(&SigningKey, Verb, &[u64], &str, &str); 6] = [
        (&seller, Verb::Join, &[], &sold, "OK 13 lines\n"),
        (&seller, Verb::List, &[13, 38], &sold, "OK listed 1:13:38\n"),
        (&buyer, Verb::Join, &[], &funds, "OK 13 lines\n"),
        (&buyer, Verb::Browse, &[0], "", "OK listings 1:13:38\n"),
        (
            &buyer,
            Verb::Buy,
            &[1, 13, 38],
            &funds,
            "OK bought 1:13:38\n",
        ),
        (
            &seller,
            Verb::Collect,
            &[],
            "",
            "OK collected sold 1:13:38\n",
        ),
    ];
    for (n, (key, verb, args, log, want)) in steps.into_iter().enumerate() {
        let got = answer(d.addr(), &request_args(verb, key, at + n as u64, args, log));
        let got = String::from_utf8(got).unwrap();
        assert_eq!(got, want, "{verb:?}");
        for k in [&seller, &buyer] {
            let public = k.verifying_key();
            let pk: String = public
                .as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let kh = forza_server::market::key_hash(public.as_bytes());
            assert!(!got.contains(&pk) && !got.contains(&kh[..8]), "{got}");
        }
    }
}

/// Acceptance 3.3 over loopback: a list of the most rows a server may set (30) answers in one line under the
/// Launcher's 1 KiB read, and a slot bought over the desk shows sold on the next view.
#[test]
fn a_stock_list_sells_over_the_desk() {
    let root = scratch("market_stock");
    let table = Table::of(
        &(1..=40)
            .map(|c| (c, forza::collection::Rarity::Regular, 80))
            .collect::<Vec<_>>(),
    );
    let market = Market::with(table, &root.join("market.db"))
        .unwrap()
        .with_stock(Stock::new("30,0,0,0", 100, 0).unwrap());
    let d = desk(Some(market));
    let key = SigningKey::from_bytes(&[8; 32]);
    let log = with_match(&starter_log(), 13); // 50 tokens
    let at = now_ms();
    assert_eq!(
        market::send(d.addr(), &request(Verb::Join, &key, at, &log)).unwrap(),
        "13 lines"
    );
    let raw = answer(d.addr(), &request(Verb::Stock, &key, at + 1, &log));
    let said = String::from_utf8(raw).unwrap();
    assert!(said.len() < 1024 && said.ends_with('\n'), "{said}");
    let rows: Vec<&str> = said.trim_end().split(' ').skip(2).collect();
    assert_eq!(rows.len(), 30, "{said}");
    let first: Vec<u64> = rows[0].split(':').map(|n| n.parse().unwrap()).collect();
    assert_eq!((first[0], first[2]), (1, 40), "{said}");
    let bought = format!("stockbought 1:{}:40", first[1]);
    assert_eq!(
        market::send(
            d.addr(),
            &request_args(Verb::StockBuy, &key, at + 2, &first, &log)
        )
        .unwrap(),
        bought
    );
    let again = market::send(d.addr(), &request(Verb::Stock, &key, at + 3, &log)).unwrap();
    assert!(
        again.starts_with(&format!("stock 1:{}:40:sold 2:", first[1])),
        "{again}"
    );
}

/// Review m3: the widest STOCK answer a server may give, 30 rows of 4-digit cards at 10,000 tokens, all sold, under
/// the widest `left`, still fits the Launcher's 1 KiB read.
#[test]
fn the_widest_stock_list_fits_1_kib() {
    let root = scratch("market_stock_wide");
    let mut cards: Vec<_> = (1..=12)
        .map(|c| (c, forza::collection::Rarity::Regular, 80))
        .collect();
    cards.extend((9960..=9999).map(|c| (c, forza::collection::Rarity::AllTimeLegend, 99)));
    let market = Market::with(Table::of(&cards), &root.join("market.db"))
        .unwrap()
        .with_stock(Stock::new("0,0,0,30", 100, u32::MAX).unwrap());
    let d = desk(Some(market));
    let key = SigningKey::from_bytes(&[9; 32]);
    let log = starter_log();
    let at = now_ms();
    assert_eq!(
        market::send(d.addr(), &request(Verb::Join, &key, at, &log)).unwrap(),
        "12 lines"
    );
    market::send(d.addr(), &request(Verb::Stock, &key, at + 1, &log)).unwrap();
    rusqlite::Connection::open(root.join("market.db"))
        .unwrap()
        .execute("UPDATE stock SET sold = 1", [])
        .unwrap();
    let said =
        String::from_utf8(answer(d.addr(), &request(Verb::Stock, &key, at + 2, &log))).unwrap();
    assert!(said.starts_with("OK stock left 4294967265 "), "{said}");
    let rows: Vec<&str> = said.trim_end().split(' ').skip(4).collect();
    assert_eq!(rows.len(), 30, "{said}");
    assert!(
        rows.iter()
            .all(|r| r.ends_with(":10000:sold") && r.split(':').nth(1).unwrap().len() == 4),
        "{said}"
    );
    assert!(
        said.len() < 1024 && said.ends_with('\n'),
        "{} bytes: {said}",
        said.len()
    );
}

// ---- step 4.1: the Launcher's client (`forza::market::Client`) against a real desk ----

use forza::collection::{Kind, Log, fold};
use forza::market::{Client, Fail};
use std::path::{Path, PathBuf};

/// A desk whose market has 40 REGULAR cards of value 40 (band 32 to 38) and a stock list of 10 at 90 % (36).
fn market_desk(root: &Path) -> Handle {
    let table = Table::of(
        &(1..=40)
            .map(|c| (c, forza::collection::Rarity::Regular, 80))
            .collect::<Vec<_>>(),
    );
    let market = Market::with(table, &root.join("market.db"))
        .unwrap()
        .with_stock(Stock::new("10,0,0,0", 90, 0).unwrap());
    desk(Some(market))
}

/// One installation as the Launcher makes it: `data` (key, state, welcome mark) and the profile's log with the 12
/// starter cards 1-12, the welcome (150) and one paid match (50) per card of `earned`, 4 minutes apart.
fn install(root: &Path, name: &str, earned: &[u32]) -> (PathBuf, PathBuf) {
    let (data, coll) = (
        root.join(name).join("data"),
        root.join(name).join("collection"),
    );
    std::fs::create_dir_all(&coll).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    profile(&data, &coll, earned);
    (data, coll)
}

/// The profile's log as [`install`] writes it (a reset keeps the welcome mark, so a second one has no welcome).
fn profile(data: &Path, coll: &Path, earned: &[u32]) {
    let log = Log::new(coll);
    let t0 = time::OffsetDateTime::now_utc() - time::Duration::hours(2);
    forza::collection::first_start(
        &log,
        &data.join(forza::collection::WELCOME_MARK),
        t0,
        &(1..=12).collect::<Vec<_>>(),
        "starter All seasons",
    )
    .unwrap();
    for (i, &c) in earned.iter().enumerate() {
        let at = t0 + time::Duration::minutes(4 * (i as i64 + 1));
        log.append_at(at, Kind::Match, Some(c), 50, "match")
            .unwrap();
    }
}

fn client(d: &Handle, (data, coll): &(PathBuf, PathBuf)) -> Client {
    Client::open(d.addr(), data, coll).unwrap()
}

/// The log of `coll` folded: no warning, readable.
fn folded(coll: &Path) -> forza::collection::State {
    let f = fold(&Log::new(coll).lines().unwrap());
    assert!(f.warnings.is_empty() && !f.unreadable, "{:?}", f.warnings);
    f.state
}

/// The log's lines of `kind`.
fn count(coll: &Path, kind: Kind) -> usize {
    Log::new(coll)
        .lines()
        .unwrap()
        .lines()
        .filter(|l| {
            serde_json::from_str::<forza::collection::Entry>(l)
                .unwrap()
                .kind
                == kind
        })
        .count()
}

/// Acceptance 4.1 (a): one installation lists, a second buys from BROWSE, the first collects; both logs fold right and
/// pass the server's check again.
#[test]
fn the_launcher_lists_buys_and_collects_over_the_desk() {
    let root = scratch("market_client_a");
    let d = market_desk(&root);
    let (s, b) = (
        install(&root, "seller", &[13]),
        install(&root, "buyer", &[14]),
    );
    let (seller, buyer) = (client(&d, &s), client(&d, &b));
    seller.ask(Verb::Join, &[]).unwrap();
    assert_eq!(seller.ask(Verb::List, &[13, 38]).unwrap(), "listed 1:13:38");
    assert_eq!(market::listed(&Log::new(&s.1).lines().unwrap()).len(), 1);
    buyer.ask(Verb::Join, &[]).unwrap();
    let offer = buyer.browse(0).unwrap();
    assert_eq!(
        offer
            .iter()
            .map(|r| (r.id, r.card, r.price))
            .collect::<Vec<_>>(),
        [(1, 13, 38)]
    );
    assert_eq!(
        buyer.ask(Verb::Buy, &[1, 13, 38]).unwrap(),
        "bought 1:13:38"
    );
    seller.collect().unwrap();
    let (sold, got) = (folded(&s.1), folded(&b.1));
    assert_eq!((sold.copies(13).earned, sold.copies(13).listed), (0, 0));
    assert_eq!(sold.tokens, 150 + 50 + 38);
    assert_eq!(got.copies(13).earned, 1);
    assert_eq!(got.tokens, 150 + 50 - 38);
    assert!(market::listed(&Log::new(&s.1).lines().unwrap()).is_empty());
    // both logs, with their market lines, pass the server's check
    seller.ask(Verb::Join, &[]).unwrap();
    buyer.ask(Verb::Join, &[]).unwrap();
}

/// Acceptance 4.1 (b): outside the market there is no list; in it, buying 2 cards of the own list writes 2 `stock_buy`
/// lines and marks the 2 slots sold, and a restarted Launcher gets the same list back.
#[test]
fn two_stock_cards_write_two_lines_and_the_list_comes_back_after_a_restart() {
    let root = scratch("market_client_b");
    let d = market_desk(&root);
    let inst = install(&root, "one", &[13]);
    let c = client(&d, &inst);
    assert!(
        matches!(c.stock(), Err(Fail::Refused(_))),
        "no list outside the market"
    );
    c.ask(Verb::Join, &[]).unwrap();
    let list = c.stock().unwrap();
    assert_eq!(list.rows.len(), 10);
    assert!(list.rows.iter().all(|r| r.price == 36 && !r.sold));
    for r in &list.rows[..2] {
        c.ask(Verb::StockBuy, &[r.id, r.card.into(), r.price.into()])
            .unwrap();
    }
    assert_eq!(count(&inst.1, Kind::StockBuy), 2);
    assert_eq!(folded(&inst.1).tokens, 150 + 50 - 2 * 36);
    let again = client(&d, &inst).stock().unwrap(); // a restarted Launcher
    assert_eq!(again.rows.len(), 10);
    for (a, b) in list.rows.iter().zip(&again.rows) {
        assert_eq!((a.id, a.card, a.price), (b.id, b.card, b.price));
    }
    assert_eq!(
        again.rows.iter().map(|r| r.sold).collect::<Vec<_>>(),
        [
            true, true, false, false, false, false, false, false, false, false
        ]
    );
}

/// Acceptance 4.1 (c): a listing past 7 days comes back at COLLECT with one `unlist` line, and a second COLLECT
/// writes none.
#[test]
fn an_expired_listing_comes_back_with_one_unlist_line() {
    let root = scratch("market_client_c");
    let d = market_desk(&root);
    let inst = install(&root, "one", &[13]);
    let c = client(&d, &inst);
    c.ask(Verb::Join, &[]).unwrap();
    c.ask(Verb::List, &[13, 36]).unwrap();
    assert_eq!(folded(&inst.1).copies(13).listed, 1);
    rusqlite::Connection::open(root.join("market.db"))
        .unwrap()
        .execute("UPDATE ledger SET at = at - 8 * 86400000", [])
        .unwrap();
    c.collect().unwrap();
    c.collect().unwrap();
    assert_eq!(count(&inst.1, Kind::Unlist), 1);
    assert_eq!(folded(&inst.1).copies(13).listed, 0);
    assert!(market::listed(&Log::new(&inst.1).lines().unwrap()).is_empty());
}

/// Acceptance 4.1 (e): START OVER queues RESET, which goes before the next request; the key is the same one after it
/// (one key row on the server), and a 6th open listing is refused.
#[test]
fn after_a_reset_the_key_stays_and_a_sixth_listing_is_refused() {
    let root = scratch("market_client_e");
    let d = market_desk(&root);
    let earned: Vec<u32> = (13..=18).collect();
    let inst = install(&root, "one", &earned);
    let c = client(&d, &inst);
    c.ask(Verb::Join, &[]).unwrap();
    c.ask(Verb::List, &[13, 38]).unwrap();
    let key = std::fs::read(inst.0.join(market::KEY_FILE)).unwrap();
    // START OVER: the profile to its archive, a new first start (no welcome: the mark stays), RESET queued
    std::fs::rename(&inst.1, root.join("one").join("archive")).unwrap();
    std::fs::create_dir_all(&inst.1).unwrap();
    profile(&inst.0, &inst.1, &earned);
    c.queue_reset().unwrap();
    for (n, card) in earned.iter().enumerate() {
        let said = c.ask(Verb::List, &[(*card).into(), 38]);
        if n < 5 {
            said.unwrap();
        } else {
            let Err(Fail::Refused(why)) = said else {
                panic!("a 6th listing: {said:?}")
            };
            assert!(why.starts_with("5 listings are open"), "{why}");
        }
    }
    assert_eq!(std::fs::read(inst.0.join(market::KEY_FILE)).unwrap(), key);
    let keys: i64 = rusqlite::Connection::open(root.join("market.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(keys, 1);
    assert_eq!(folded(&inst.1).copies(18).listed, 0);
}

thread_local! {
    /// What [`record`] sent, in order.
    static SENT: std::cell::RefCell<Vec<Vec<u8>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A transport whose answer is lost on the way back: the desk got the request.
fn lose(desk: SocketAddr, bytes: &[u8]) -> Result<Result<String, String>, String> {
    let _ = market::exchange(desk, bytes);
    Err("connection reset".into())
}

/// The real transport, keeping the bytes sent.
fn record(desk: SocketAddr, bytes: &[u8]) -> Result<Result<String, String>, String> {
    SENT.with(|s| s.borrow_mut().push(bytes.to_vec()));
    market::exchange(desk, bytes)
}

/// Acceptance 4.1 (f): LIST's answer is lost and the Launcher restarts; the next request sends the same bytes first and
/// the `list` line is written once. A STOCKBUY whose answer is lost goes again before a JOIN; its answer applied but
/// the outbox not cleared (a crash between) is sent again and writes nothing more.
#[test]
fn a_lost_answer_is_sent_again_after_a_restart_and_written_once() {
    let root = scratch("market_client_f");
    let d = market_desk(&root);
    let inst = install(&root, "one", &[13]);
    let state = inst.0.join(market::STATE_FILE);
    let mut c = client(&d, &inst);
    c.ask(Verb::Join, &[]).unwrap();
    c.transport = lose;
    assert!(matches!(
        c.ask(Verb::List, &[13, 38]),
        Err(Fail::Unanswered(_))
    ));
    assert_eq!(count(&inst.1, Kind::List), 0, "no answer, no line");
    let kept = Client::saved(&inst.0).outbox.unwrap().bytes;
    // a restarted Launcher, still offline: the same request stays in the outbox
    let mut c = client(&d, &inst);
    c.transport = lose;
    assert!(matches!(c.browse(0), Err(Fail::Unanswered(_))));
    assert_eq!(Client::saved(&inst.0).outbox.unwrap().bytes, kept);
    // online again: BROWSE sends the LIST's bytes first
    let mut c = client(&d, &inst);
    c.transport = record;
    c.browse(0).unwrap();
    SENT.with(|s| {
        let s = s.borrow();
        assert_eq!(s[0], kept.as_bytes(), "the same bytes again");
        assert!(String::from_utf8_lossy(&s[1]).starts_with("MARKET BROWSE "));
    });
    assert_eq!(count(&inst.1, Kind::List), 1);
    assert_eq!(folded(&inst.1).copies(13).listed, 1);
    assert!(Client::saved(&inst.0).outbox.is_none());
    // a lost STOCKBUY goes before JOIN, and its line comes once
    let row = c.stock().unwrap().rows[0];
    c.transport = lose;
    let args = [row.id, row.card.into(), row.price.into()];
    assert!(matches!(
        c.ask(Verb::StockBuy, &args),
        Err(Fail::Unanswered(_))
    ));
    let saved = std::fs::read_to_string(&state).unwrap();
    c.transport = record;
    c.flush().unwrap();
    assert_eq!(count(&inst.1, Kind::StockBuy), 1);
    std::fs::write(&state, &saved).unwrap(); // the outbox as before the answer: a crash after the line
    SENT.with(|s| s.borrow_mut().clear());
    c.ask(Verb::Join, &[]).unwrap();
    SENT.with(|s| {
        let s = s.borrow();
        assert!(String::from_utf8_lossy(&s[0]).starts_with("MARKET STOCKBUY "));
        assert!(String::from_utf8_lossy(&s[1]).starts_with("MARKET JOIN "));
    });
    assert_eq!(count(&inst.1, Kind::StockBuy), 1);
    assert_eq!(folded(&inst.1).tokens, 150 + 50 - 36);
}

/// Each new request's `at` is strictly above the last, also within one millisecond and with the PC's clock behind the
/// last one sent.
#[test]
fn each_new_request_goes_after_the_last() {
    let root = scratch("market_client_at");
    let d = market_desk(&root);
    let inst = install(&root, "one", &[13]);
    let c = client(&d, &inst);
    c.ask(Verb::Join, &[]).unwrap();
    let mut s = Client::saved(&inst.0);
    let ahead = s.last_at + 3_600_000; // a clock an hour ahead signed the last request
    s.last_at = ahead;
    std::fs::write(
        inst.0.join(market::STATE_FILE),
        serde_json::to_string(&s).unwrap(),
    )
    .unwrap();
    for _ in 0..3 {
        c.browse(0).unwrap();
    }
    assert_eq!(Client::saved(&inst.0).last_at, ahead + 3);
}
