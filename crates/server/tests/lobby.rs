//! The desk's lobby words: `LOBBY`, `RESERVE`, `HEARTBEAT`, `WANT ... TICKET`, and the client calls in
//! `forza::online::lobby` against an in-process desk.

use forza::online::lobby::{self, Lobby, Reserved, State};
use forza_server::desk::{DeskConfig, start};
use forza_server::{Edition, Handle};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

fn desk(tune: impl FnOnce(&mut DeskConfig)) -> Handle {
    let mut c = DeskConfig::new(Edition::E1718);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.lobby_gap = Duration::ZERO;
    c.lobby_cache = Duration::ZERO;
    tune(&mut c);
    start(c, std::sync::Arc::new(|_| {})).unwrap()
}

fn say(addr: SocketAddr, line: &str) -> String {
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(format!("{line}\n").as_bytes()).unwrap();
    // one line (a seat answer is followed by a held connection, not a hang-up)
    let mut s = Vec::new();
    while !s.ends_with(b"\n") {
        let mut b = [0u8; 1];
        if c.read(&mut b).unwrap() == 0 {
            break;
        }
        s.push(b[0]);
    }
    String::from_utf8(s).unwrap().trim().to_string()
}

fn shop_id(addr: SocketAddr) -> String {
    lobby::fetch(addr, "2017-18").unwrap().shops[0].id.clone()
}

#[test]
fn lobby_json_parses_and_follows_the_seats() {
    let d = desk(|c| c.lang = Some("it".into()));
    let l = lobby::fetch(d.addr(), "2017-18").unwrap();
    assert_eq!((l.v, l.edition.as_str()), (1, "2017-18"));
    assert!(l.vs.is_empty());
    let s = &l.shops[0];
    assert_eq!((s.seats, s.max, s.state), (0, 8, State::Open));
    assert_eq!(
        (s.name.as_str(), s.lang.as_deref()),
        ("Forza-WCCF", Some("it"))
    );
    let mut held = TcpStream::connect(d.addr()).unwrap();
    held.write_all(b"WANT\n").unwrap();
    let _ = held.read(&mut [0; 64]);
    assert_eq!(lobby::fetch(d.addr(), "2017-18").unwrap().shops[0].seats, 1);
    assert!(lobby::fetch(d.addr(), "2010-11").is_err());
}

#[test]
fn lobby_answer_holds_no_address_or_stored_id() {
    let d = desk(|_| {});
    let mut held = TcpStream::connect(d.addr()).unwrap();
    held.write_all(b"WANT\n").unwrap();
    let _ = held.read(&mut [0; 64]);
    let json = say(d.addr(), "LOBBY 2017-18");
    let ipv4 = regex::Regex::new(r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}").unwrap();
    assert!(!ipv4.is_match(&json), "{json}");
    assert!(!json.contains("127.0.0.1") && !json.contains("localhost"));
    // only the shop's own random id; its name is the setting
    let l = Lobby::parse(&json).unwrap();
    assert_eq!(l.shops[0].id.len(), 8);
    assert!(json.len() <= lobby::MAX_ANSWER);
}

#[test]
fn lobby_rate_limit_and_cache() {
    let d = desk(|c| c.lobby_gap = Duration::from_millis(400));
    assert!(say(d.addr(), "LOBBY 2017-18").starts_with('{'));
    assert_eq!(say(d.addr(), "LOBBY 2017-18"), "ERR slow");
    std::thread::sleep(Duration::from_millis(450));
    assert!(say(d.addr(), "LOBBY 2017-18").starts_with('{'));
    // the cache: a seat taken inside the window is not shown yet
    let d = desk(|c| c.lobby_cache = Duration::from_secs(30));
    assert_eq!(lobby::fetch(d.addr(), "2017-18").unwrap().shops[0].seats, 0);
    let id = shop_id(d.addr());
    assert!(matches!(
        lobby::reserve(d.addr(), &id),
        Ok(Reserved::Ticket { .. })
    ));
    assert_eq!(lobby::fetch(d.addr(), "2017-18").unwrap().shops[0].seats, 0);
}

#[test]
fn reserve_then_want_ticket_claims_that_seat() {
    let d = desk(|_| {});
    let id = shop_id(d.addr());
    let Reserved::Ticket { ticket, seat } = lobby::reserve(d.addr(), &id).unwrap() else {
        panic!()
    };
    assert_eq!(seat, 1);
    // the reserved seat is busy for others, and the ticket's owner gets it
    assert_eq!(say(d.addr(), "WANT"), "SEAT 2 PROJECTOR 1");
    assert_eq!(say(d.addr(), "WANT SEAT 1"), "TAKEN 1");
    let mut c = TcpStream::connect(d.addr()).unwrap();
    c.write_all(format!("WANT TICKET {ticket}\n").as_bytes())
        .unwrap();
    let mut b = [0u8; 64];
    let n = c.read(&mut b).unwrap();
    assert_eq!(&b[..n], b"SEAT 1 PROJECTOR 1\n");
    // a ticket is good once
    assert_eq!(
        say(d.addr(), &format!("WANT SEAT 3 TICKET {ticket}")),
        "ERR ticket"
    );
    assert_eq!(say(d.addr(), "WANT TICKET nope"), "ERR ticket");
}

#[test]
fn last_seat_race_and_taken() {
    let d = desk(|_| {});
    let id = shop_id(d.addr());
    let _held: Vec<TcpStream> = (0..7)
        .map(|_| {
            let mut c = TcpStream::connect(d.addr()).unwrap();
            c.write_all(b"WANT\n").unwrap();
            let _ = c.read(&mut [0; 64]);
            c
        })
        .collect();
    let (a, b) = (
        lobby::reserve(d.addr(), &id).unwrap(),
        lobby::reserve(d.addr(), &id).unwrap(),
    );
    assert!(matches!(a, Reserved::Ticket { seat: 8, .. }), "{a:?}");
    assert_eq!(b, Reserved::Full);
    assert_eq!(say(d.addr(), &format!("RESERVE {id} 8")), "TAKEN");
    assert_eq!(say(d.addr(), &format!("RESERVE {id} 3")), "TAKEN");
    assert_eq!(
        lobby::fetch(d.addr(), "2017-18").unwrap().shops[0].state,
        State::Full
    );
    assert_eq!(lobby::reserve(d.addr(), "zzzz").unwrap(), Reserved::Closed);
}

#[test]
fn lease_expires_and_heartbeat_extends() {
    let d = desk(|c| c.reserve_lease = Duration::from_millis(500));
    let id = shop_id(d.addr());
    let Reserved::Ticket { ticket: kept, seat } = lobby::reserve(d.addr(), &id).unwrap() else {
        panic!()
    };
    let Reserved::Ticket { ticket: lost, .. } = lobby::reserve(d.addr(), &id).unwrap() else {
        panic!()
    };
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(lobby::heartbeat(d.addr(), &kept), Ok(true));
    }
    assert_eq!(lobby::heartbeat(d.addr(), &lost), Ok(false));
    assert_eq!(lobby::fetch(d.addr(), "2017-18").unwrap().shops[0].seats, 1);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(lobby::heartbeat(d.addr(), &kept), Ok(false));
    assert_eq!(lobby::fetch(d.addr(), "2017-18").unwrap().shops[0].seats, 0);
    assert!(
        matches!(lobby::reserve(d.addr(), &id), Ok(Reserved::Ticket { seat: s, .. }) if s == seat)
    );
}

#[test]
fn per_address_connection_cap() {
    let d = desk(|_| {});
    let held: Vec<TcpStream> = (0..20)
        .map(|_| {
            TcpStream::connect(d.addr()).unwrap() // silent: the desk waits for its line
        })
        .collect();
    // past 16 open connections from one address the desk hangs up without an answer
    let mut c = TcpStream::connect(d.addr()).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let _ = c.write_all(b"LOBBY 2017-18\n");
    let mut s = String::new();
    let _ = c.read_to_string(&mut s);
    assert!(s.is_empty(), "{s}");
    drop(held);
}
