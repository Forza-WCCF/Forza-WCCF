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

/// 2017-18 shop rooms: `CREATE shop ID` lists a room as a shop row (code, x/8, nothing else); a guest's ticket gets the
/// host's tunnel id and tells its own, the host's ticket gets the guests'; leaving and the host's `CLOSE` as for VS
/// rooms. No matcher needed: a Launcher's shop desk (`--no-gmsvr`) holds them too.
#[test]
fn shop_rooms_swap_tunnel_ids_and_list_nothing_else() {
    use forza::online::lobby::Peers;
    let d = desk(|c| c.vs = None);
    let a = d.addr();
    let (host_id, guest_id) = ("a1".repeat(32), "b2".repeat(32));
    assert_eq!(say(a, "CREATE shop 1.2.3.4"), "ERR id");
    let (id, code, host_t) = match lobby::create_shop(a, &host_id).unwrap() {
        Created::Room {
            id,
            code,
            ticket,
            seat: 1,
        } => (id, code, ticket),
        r => panic!("{r:?}"),
    };
    let row = |a| {
        lobby::fetch(a, "2017-18")
            .unwrap()
            .shops
            .into_iter()
            .find(|s| s.id == id)
    };
    let r = row(a).expect("the room is a shop row");
    assert_eq!(
        (r.code.as_deref(), r.seats, r.max),
        (Some(code.as_str()), 1, 8)
    );
    let guest_t = match lobby::reserve(a, &id).unwrap() {
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
    assert_eq!(row(a).unwrap().seats, 2);
    // privacy: the whole LOBBY line holds neither tunnel id nor ticket
    let line = say(a, "LOBBY 2017-18");
    for secret in [&host_id, &guest_id, &host_t, &guest_t] {
        assert!(!line.contains(secret.as_str()), "{secret} in {line}");
    }
    assert!(lobby::heartbeat(a, &guest_t).unwrap());
    assert!(lobby::release(a, &guest_t).unwrap(), "the guest leaves");
    assert_eq!(row(a).unwrap().seats, 1);
    assert!(
        lobby::close(a, &id, &host_t).unwrap(),
        "the host closes the room"
    );
    assert!(row(a).is_none());
    assert!(!lobby::heartbeat(a, &host_t).unwrap());
}

/// A 2010-11 desk knows no shop rooms.
#[test]
fn no_shop_rooms_on_2010_11() {
    let d = desk(|c| {
        c.edition = Edition::E1011;
        c.vs = None;
    });
    assert!(say(d.addr(), &format!("CREATE shop {}", "a1".repeat(32))).starts_with("ERR"));
}
