//! Create-a-Team's market words on a real desk over loopback: `MARKET JOIN / LEAVE / RESET` signed by
//! `forza::market`, the "not set up" answer without a value table, and the old words answering as before.

use crate::common::*;
use ed25519_dalek::SigningKey;
use forza::collection::RULES;
use forza::market::{self, NOT_SET_UP, Request, Verb};
use forza_server::desk::{DeskConfig, start};
use forza_server::market::{Market, Table};
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
    Request {
        verb,
        key: *key.verifying_key().as_bytes(),
        at,
        rules: RULES,
        log: log.into(),
    }
    .signed(key)
    .unwrap()
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
        market::ask(d.addr(), &key, Verb::Leave, "", now_ms()).unwrap(),
        "left"
    );
}
