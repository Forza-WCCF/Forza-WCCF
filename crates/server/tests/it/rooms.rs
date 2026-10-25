//! The desk's VS room words (`CREATE vs [open]`, `RESERVE room`, `READY`, `CLOSE`, `HEARTBEAT`, `RELEASE`), their
//! races, limits and privacy, and what the desk tells the shared matcher.

use forza::online::lobby::{self, Created, Lobby, Reserved, VsRoom};
use forza::online::vs::{ENTRY_LEN, Matcher};
use forza_server::desk::{DeskConfig, start};
use forza_server::{Edition, Handle};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn desk(tune: impl FnOnce(&mut DeskConfig)) -> Handle {
    let mut c = DeskConfig::new(Edition::E1718);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.lobby_gap = Duration::ZERO;
    c.lobby_cache = Duration::ZERO;
    c.vs = Some(Arc::new(Matcher::default())); // rooms need the game server's matcher
    tune(&mut c);
    start(c, Arc::new(|_| {})).unwrap()
}

fn say(addr: SocketAddr, line: &str) -> String {
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(format!("{line}\n").as_bytes()).unwrap();
    let mut s = String::new();
    c.read_to_string(&mut s).unwrap();
    s.trim().to_string()
}

fn room(addr: SocketAddr, open: bool) -> (String, String, String) {
    match lobby::create(addr, open).unwrap() {
        Created::Room {
            id, code, ticket, ..
        } => (id, code, ticket),
        Created::Full => panic!("FULL"),
    }
}

fn guest(addr: SocketAddr, id: &str) -> String {
    match lobby::reserve(addr, id).unwrap() {
        Reserved::Ticket { ticket, seat: 2 } => ticket,
        r => panic!("{r:?}"),
    }
}

fn listed(addr: SocketAddr, id: &str) -> Option<VsRoom> {
    let l = lobby::fetch(addr, "2017-18").unwrap();
    l.vs.into_iter().find(|r| r.id == id)
}

/// (seat, ready) per occupant.
fn seats(r: &VsRoom) -> Vec<(u8, bool)> {
    r.occupants.iter().map(|o| (o.seat, o.ready)).collect()
}

#[test]
fn create_reserve_ready_and_the_seat_races() {
    let d = desk(|_| {});
    let a = d.addr();
    let (id, code, host) = room(a, false);
    assert_eq!(code.len(), 4);
    let r = listed(a, &id).unwrap();
    assert_eq!(
        (r.players, r.max, r.kind, seats(&r)),
        (1, 2, 2, vec![(1, false)])
    );
    // seat 1 is the host's; a second guest finds the room full, a seat asked for is taken
    assert_eq!(say(a, &format!("RESERVE {id} 1")), "TAKEN");
    let g = guest(a, &id);
    assert_eq!(lobby::reserve(a, &id), Ok(Reserved::Full));
    assert_eq!(say(a, &format!("RESERVE {id} 2")), "TAKEN");
    assert_eq!(lobby::reserve(a, "nope"), Ok(Reserved::Closed));
    // READY marks the ticket's own seat; a ticket of another room or none is GONE
    assert_eq!(lobby::ready(a, &id, &g), Ok(true));
    assert_eq!(seats(&listed(a, &id).unwrap()), [(1, false), (2, true)]);
    let (other, _, other_host) = room(a, false);
    assert_eq!(lobby::ready(a, &id, &other_host), Ok(false));
    assert_eq!(lobby::ready(a, &other, &g), Ok(false));
    assert_eq!(lobby::ready(a, &id, &host), Ok(true));
    assert_eq!(seats(&listed(a, &id).unwrap()), [(1, true), (2, true)]);
    // a room ticket claims no shop seat
    assert_eq!(say(a, &format!("WANT TICKET {g}")), "ERR ticket");
}

#[test]
fn many_guests_at_once_get_one_seat() {
    let d = desk(|c| c.ask_limit = 100);
    let a = d.addr();
    let (id, ..) = room(a, false);
    let answers: Vec<Reserved> = (0..12)
        .map(|_| {
            let id = id.clone();
            std::thread::spawn(move || lobby::reserve(a, &id).unwrap())
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();
    let tickets = answers
        .iter()
        .filter(|r| matches!(r, Reserved::Ticket { seat: 2, .. }))
        .count();
    let full = answers.iter().filter(|r| **r == Reserved::Full).count();
    assert_eq!((tickets, full), (1, 11), "{answers:?}");
}

#[test]
fn host_close_drops_the_waiting_guest() {
    let d = desk(|_| {});
    let a = d.addr();
    let (id, _, host) = room(a, false);
    let g = guest(a, &id);
    assert_eq!(
        lobby::close(a, &id, &g.replace(char::is_alphanumeric, "0")),
        Ok(false)
    );
    assert_eq!(lobby::close(a, &id, &host), Ok(true));
    // the guest's ticket went with the room: its HEARTBEAT answers GONE, the room is closed for new guests
    assert_eq!(lobby::heartbeat(a, &g), Ok(false));
    assert_eq!(lobby::heartbeat(a, &host), Ok(false));
    assert_eq!(lobby::reserve(a, &id), Ok(Reserved::Closed));
    assert_eq!(lobby::close(a, &id, &host), Ok(false));
    assert!(listed(a, &id).is_none());
}

#[test]
fn a_guest_leaves_and_the_room_stays() {
    let d = desk(|_| {});
    let a = d.addr();
    let (id, _, host) = room(a, false);
    let g = guest(a, &id);
    assert_eq!(lobby::close(a, &id, &g), Ok(true));
    assert_eq!(seats(&listed(a, &id).unwrap()), [(1, false)]);
    let g2 = guest(a, &id);
    assert_eq!(
        lobby::release(a, &g2),
        Ok(true),
        "RELEASE leaves the seat too"
    );
    assert_eq!(lobby::heartbeat(a, &g2), Ok(false));
    // the host's RELEASE closes the room like its CLOSE
    assert_eq!(lobby::release(a, &host), Ok(true));
    assert!(listed(a, &id).is_none());
}

#[test]
fn quick_match_pairs_the_oldest_open_room() {
    let d = desk(|_| {});
    let a = d.addr();
    let (plain, ..) = room(a, false);
    let seat = |open| match lobby::create(a, open).unwrap() {
        Created::Room { id, code, seat, .. } => (id, code, seat),
        Created::Full => panic!("FULL"),
    };
    let (first, code, 1) = seat(true) else {
        panic!("a new room is seat 1")
    };
    let (second, _, 2) = seat(true) else {
        panic!("joining a waiting room is seat 2")
    };
    // the second open ask sat down in the first open room; plain rooms are never joined by QUICK MATCH
    assert_eq!(first, second);
    let l = lobby::fetch(a, "2017-18").unwrap();
    let r = l.vs.iter().find(|r| r.id == first).unwrap();
    assert_eq!((r.code.as_str(), r.players), (code.as_str(), 2));
    assert_eq!(l.vs.iter().find(|r| r.id == plain).unwrap().players, 1);
    // the next open ask makes a new open room, the one after joins it
    let (third, ..) = room(a, true);
    let (fourth, ..) = room(a, true);
    assert_eq!(third, fourth);
    assert_eq!(lobby::fetch(a, "2017-18").unwrap().vs.len(), 3);
}

#[test]
fn room_leases_follow_the_shop_rules() {
    let d = desk(|c| c.reserve_lease = Duration::from_millis(500));
    let a = d.addr();
    let (id, _, host) = room(a, false);
    let g = guest(a, &id);
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(lobby::heartbeat(a, &host), Ok(true));
    }
    // the silent guest's seat is free again, the host's room lives on its heartbeats
    assert_eq!(lobby::heartbeat(a, &g), Ok(false));
    assert_eq!(seats(&listed(a, &id).unwrap()), [(1, false)]);
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        listed(a, &id).is_none(),
        "the host went silent: the room is gone"
    );
    assert_eq!(lobby::heartbeat(a, &host), Ok(false));
}

#[test]
fn room_cap_answers_full() {
    let d = desk(|c| c.max_rooms = 2);
    let a = d.addr();
    room(a, false);
    let (_, _, t) = room(a, true);
    assert_eq!(lobby::create(a, false), Ok(Created::Full));
    // a QUICK MATCH still sits down in the waiting open room
    assert!(matches!(lobby::create(a, true), Ok(Created::Room { .. })));
    assert_eq!(lobby::create(a, true), Ok(Created::Full));
    assert_eq!(lobby::release(a, &t), Ok(true));
    assert!(matches!(lobby::create(a, false), Ok(Created::Room { .. })));
}

#[test]
fn reserve_and_create_are_rate_limited_per_address() {
    let d = desk(|c| {
        c.ask_limit = 3;
        c.ask_window = Duration::from_millis(500);
    });
    let a = d.addr();
    let (id, ..) = room(a, false);
    guest(a, &id);
    assert_eq!(lobby::reserve(a, &id), Ok(Reserved::Full));
    assert_eq!(say(a, "CREATE vs"), "ERR slow");
    assert_eq!(say(a, &format!("RESERVE {id}")), "ERR slow");
    // the other words are not counted
    assert!(lobby::fetch(a, "2017-18").is_ok());
    std::thread::sleep(Duration::from_millis(600));
    assert!(say(a, "CREATE vs").starts_with("ROOM "));
}

#[test]
fn room_json_holds_no_address_ticket_or_card() {
    let d = desk(|_| {});
    let a = d.addr();
    let (id, code, host) = room(a, false);
    let g = guest(a, &id);
    lobby::ready(a, &id, &g).unwrap();
    let json = say(a, "LOBBY 2017-18");
    let ipv4 = regex::Regex::new(r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}").unwrap();
    assert!(!ipv4.is_match(&json), "{json}");
    for secret in [&host, &g] {
        assert!(!json.contains(secret.as_str()), "{json}");
        // nor a piece of one: club ids are drawn on their own
        assert!(!json.to_lowercase().contains(&secret[..6]), "{json}");
    }
    let l = Lobby::parse(&json).unwrap();
    let r = l.vs.iter().find(|r| r.id == id).unwrap();
    let plain = |s: &str| {
        s.bytes()
            .all(|b| b"ABCDEFGHJKMNPQRSTUVWXYZ23456789".contains(&b))
    };
    assert!(
        plain(&code)
            && r.occupants
                .iter()
                .all(|o| o.club.len() == 4 && plain(&o.club))
    );
    assert_eq!(r.id.len(), 8);
    // the fields a room has, and no others
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let keys: Vec<&str> = v["vs"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "code", "players", "max", "kind", "occupants"]);
}

/// A vslink PC on the matcher: hello with `ticket`, token 127.n.n.n registered, its 243 entered.
fn pc(m: &Matcher, n: u8, ticket: Option<&str>) {
    let t = Instant::now();
    let (ip, port) = ([127, n, n, n], 20000 + u16::from(n));
    m.hello(IpAddr::from([10, 0, 0, n]), [n; 32], ticket, t);
    assert!(m.register(u32::from_le_bytes(ip), port, [n; 32], t));
    let mut e = vec![0u8; ENTRY_LEN];
    e[0..4].copy_from_slice(&u32::from(n).to_le_bytes());
    e[8..12].copy_from_slice(&ip);
    e[12..14].copy_from_slice(&port.to_le_bytes());
    e[44..48].copy_from_slice(&(100 + u32::from(n)).to_le_bytes());
    assert!(m.enter(&e, t));
}

fn partner(m: &Matcher, n: u8) -> Option<u32> {
    let mut p = vec![0u8; 20];
    p[0..4].copy_from_slice(&u32::from(n).to_le_bytes());
    p[8..12].copy_from_slice(&(100 + u32::from(n)).to_le_bytes());
    m.poll(&p, Instant::now())
        .map(|r| u32::from_le_bytes(r[12..16].try_into().unwrap()))
}

#[test]
fn the_desk_tells_the_matcher_who_sits_where() {
    let m = Arc::new(Matcher::default());
    let d = desk(|c| c.vs = Some(m.clone()));
    let a = d.addr();
    let (one, _, h1) = room(a, false);
    let g1 = guest(a, &one);
    let (two, _, h2) = room(a, false);
    let g2 = guest(a, &two);
    pc(&m, 1, Some(&h1));
    pc(&m, 2, Some(&h2));
    pc(&m, 3, None);
    pc(&m, 4, Some(&g2));
    assert_eq!(
        partner(&m, 4),
        Some(102),
        "room two's guest gets room two's host"
    );
    assert_eq!(partner(&m, 3), None, "the open pool has nobody else");
    // room one's host closes while its guest's run waits: that guest pairs with nobody now
    assert_eq!(lobby::close(a, &one, &h1), Ok(true));
    pc(&m, 5, Some(&g1));
    assert_eq!(partner(&m, 5), None);
    assert_eq!(partner(&m, 1), None);
}

#[test]
fn no_rooms_without_the_matcher() {
    // 2010-11, or a 2017-18 desk without its game server: the room words are unknown words
    for (ed, name) in [(Edition::E1011, "2010-11"), (Edition::E1718, "2017-18")] {
        let mut c = DeskConfig::new(ed);
        (c.bind, c.port, c.lobby_gap, c.lobby_cache) =
            ("127.0.0.1".into(), 0, Duration::ZERO, Duration::ZERO);
        let d = start(c, Arc::new(|_| {})).unwrap();
        let a = d.addr();
        for w in ["CREATE vs", "CREATE vs open", "READY 0a1b2c3d t"] {
            assert!(say(a, w).starts_with("ERR"), "{name} {w}");
        }
        // 2017-18 knows CLOSE for its shop rooms, which need no matcher
        let close = say(a, "CLOSE 0a1b2c3d t");
        if ed == Edition::E1718 {
            assert_eq!(close, "GONE");
        } else {
            assert!(close.starts_with("ERR"), "{close}");
        }
        assert_eq!(say(a, "RESERVE 0a1b2c3d 2"), "CLOSED");
        assert!(lobby::fetch(a, name).unwrap().vs.is_empty());
    }
}

/// A real tunnel id: the public key of a fixed secret.
fn tunnel_id(seed: u8) -> String {
    let k = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]).verifying_key();
    k.to_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn shop_room(a: SocketAddr, host: &str, public: bool) -> (String, String, String) {
    match lobby::create_shop(a, host, public).unwrap() {
        Created::Room {
            id,
            code,
            ticket,
            seat: 1,
        } => (id, code, ticket),
        r => panic!("{r:?}"),
    }
}

/// 2017-18 shop rooms: joined by code alone; a guest's ticket gets the host's tunnel id and tells its own, the host's
/// ticket gets the guests'; leaving and the host's `CLOSE` as for VS rooms. No matcher needed: a Launcher's shop desk
/// (`--no-gmsvr`) holds them too.
#[test]
fn shop_rooms_join_by_code_and_swap_tunnel_ids() {
    use forza::online::lobby::Peers;
    let d = desk(|c| c.vs = None);
    let a = d.addr();
    let (host_id, guest_id) = (tunnel_id(1), tunnel_id(2));
    assert_eq!(say(a, "CREATE shop 1.2.3.4"), "ERR id");
    assert_eq!(
        say(a, &format!("CREATE shop {}", "ff".repeat(32))),
        "ERR id",
        "hex, but no public key"
    );
    let (id, code, host_t) = shop_room(a, &host_id, false);
    assert_eq!(code.len(), 8);
    let guest_t = match lobby::join_shop(a, &code.to_lowercase(), None).unwrap() {
        Reserved::Ticket { ticket, seat: 2 } => ticket,
        r => panic!("{r:?}"),
    };
    assert_eq!(
        lobby::shop_peers(a, &host_t, None).unwrap(),
        Some(Peers::Guests(vec![]))
    );
    assert_eq!(
        lobby::shop_peers(a, &guest_t, Some(&guest_id)).unwrap(),
        Some(Peers::Host(host_id.clone()))
    );
    assert_eq!(
        lobby::shop_peers(a, &host_t, None).unwrap(),
        Some(Peers::Guests(vec![guest_id.clone()]))
    );
    assert_eq!(
        lobby::shop_peers(a, "0000000000000000", None).unwrap(),
        None,
        "GONE"
    );
    assert!(lobby::heartbeat(a, &guest_t).unwrap());
    assert!(lobby::release(a, &guest_t).unwrap(), "the guest leaves");
    assert_eq!(
        lobby::shop_peers(a, &host_t, None).unwrap(),
        Some(Peers::Guests(vec![]))
    );
    assert!(
        lobby::close(a, &id, &host_t).unwrap(),
        "the host closes the room"
    );
    assert!(!lobby::heartbeat(a, &host_t).unwrap());
    assert_eq!(lobby::join_shop(a, &code, None).unwrap(), Reserved::Closed);
}

/// A stranger with only what `LOBBY` says gets no ticket and no tunnel id: a private room is not listed, a public one
/// is listed without its code, and `RESERVE` of its id gives nothing.
#[test]
fn a_stranger_with_the_lobby_cannot_join_a_shop_room() {
    let d = desk(|c| c.vs = None);
    let a = d.addr();
    let host_id = tunnel_id(1);
    let (private_id, private_code, private_t) = shop_room(a, &host_id, false);
    let other_host = tunnel_id(2); // one room per host id
    let (public_id, public_code, public_t) = shop_room(a, &other_host, true);
    let line = say(a, "LOBBY 2017-18");
    let l = Lobby::parse(&line).unwrap();
    assert!(
        !l.shops.iter().any(|s| s.id == private_id),
        "private: not listed"
    );
    let row = l
        .shops
        .iter()
        .find(|s| s.id == public_id)
        .expect("public: listed");
    assert_eq!((row.seats, row.max), (1, 8));
    for secret in [
        &host_id,
        &other_host,
        &private_code,
        &public_code,
        &private_t,
        &public_t,
    ] {
        assert!(!line.contains(secret.as_str()), "{secret} in {line}");
    }
    // everything the stranger can try with what it read (the desk's own shop row is the plain shop, seats by RESERVE)
    for s in l.shops.iter().filter(|s| s.name == "Shop room") {
        assert_eq!(
            lobby::reserve(a, &s.id).unwrap(),
            Reserved::Closed,
            "RESERVE {}",
            s.id
        );
        assert_eq!(
            lobby::join_shop(a, &s.id, None).unwrap(),
            Reserved::Closed,
            "JOIN {}",
            s.id
        );
        assert_eq!(lobby::shop_peers(a, &s.id, None).unwrap(), None);
    }
}

/// One address holds at most 2 shop rooms (and 8 VS rooms: players behind one NAT) at once, and one tunnel id one
/// room; closing one makes room again.
#[test]
fn rooms_per_address_and_per_host_are_capped() {
    let d = desk(|_| {});
    let a = d.addr();
    let (id, _, t) = shop_room(a, &tunnel_id(1), false);
    assert_eq!(
        say(a, &format!("CREATE shop {}", tunnel_id(1))),
        "ERR host",
        "one room per host"
    );
    shop_room(a, &tunnel_id(2), false);
    assert_eq!(
        lobby::create_shop(a, &tunnel_id(3), false).unwrap(),
        Created::Full
    );
    assert!(lobby::close(a, &id, &t).unwrap());
    shop_room(a, &tunnel_id(1), false);
    let vs: Vec<_> = (0..8).map(|_| room(a, false)).collect();
    assert_eq!(
        lobby::create(a, false).unwrap(),
        Created::Full,
        "VS rooms have their own 8"
    );
    assert!(lobby::close(a, &vs[0].0, &vs[0].2).unwrap());
    room(a, false);
}

/// Wrong codes from anywhere share one budget: past it every JOIN waits for the window, the right code included.
#[test]
fn wrong_codes_slow_every_join() {
    let d = desk(|c| {
        c.join_fail_limit = 3;
        c.join_fail_window = Duration::from_millis(800);
    });
    let a = d.addr();
    let (_, code, _) = shop_room(a, &tunnel_id(1), false);
    for _ in 0..3 {
        assert_eq!(
            lobby::join_shop(a, "AAAAAAAA", None).unwrap(),
            Reserved::Closed
        );
    }
    assert_eq!(say(a, &format!("JOIN {code}")), "ERR slow");
    std::thread::sleep(Duration::from_millis(900));
    assert!(matches!(
        lobby::join_shop(a, &code, None).unwrap(),
        Reserved::Ticket { seat: 2, .. }
    ));
}

/// A shop's own desk behind its tunnel seats its PCs and nothing else: the room words are refused there.
#[test]
fn a_tunnel_desk_holds_no_rooms() {
    let d = desk(|c| c.tunnel = true);
    let a = d.addr();
    for w in [
        format!("CREATE shop {}", tunnel_id(1)),
        "CREATE vs".into(),
        "JOIN K7QMZDRX".into(),
        "SHOP 0123456789abcdef".into(),
        "READY 0a1b2c3d t".into(),
        "CLOSE 0a1b2c3d t".into(),
    ] {
        assert_eq!(say(a, &w), "ERR rooms are on the lobby", "{w}");
    }
    let l = lobby::fetch(a, "2017-18").unwrap();
    assert!(l.vs.is_empty() && l.shops.len() == 1, "only its own shop");
    assert!(
        matches!(
            lobby::reserve(a, &l.shops[0].id).unwrap(),
            Reserved::Ticket { .. }
        ),
        "seats still"
    );
}

/// A 2010-11 desk knows no shop rooms.
#[test]
fn no_shop_rooms_on_2010_11() {
    let d = desk(|c| {
        c.edition = Edition::E1011;
        c.vs = None;
    });
    assert!(say(d.addr(), &format!("CREATE shop {}", tunnel_id(1))).starts_with("ERR"));
    assert!(say(d.addr(), "JOIN K7QMZDRX").starts_with("ERR"));
}
