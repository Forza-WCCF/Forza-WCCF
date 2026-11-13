//! The desk's VS room words (`CREATE vs [open]`, `RESERVE room`, `READY`, `CLOSE`, `HEARTBEAT`, `RELEASE`), their
//! races, limits and privacy, and what the desk tells the shared matcher.

use forza::online::lobby::{self, Created, Lobby, Mode, Reserved, VsRoom};
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
    // a room ticket claims no arcade seat
    assert_eq!(say(a, &format!("WANT TICKET {g}")), "ERR ticket");
}

/// `MODE` marks the ticket's own seat only; a wrong ticket or word changes nothing.
#[test]
fn mode_marks_the_tickets_own_seat() {
    let d = desk(|_| {});
    let a = d.addr();
    let (id, _, host) = room(a, false);
    let g = guest(a, &id);
    let (other, _, other_host) = room(a, false);
    let modes = || {
        let r = listed(a, &id).unwrap();
        r.occupants.iter().map(|o| o.mode).collect::<Vec<_>>()
    };
    assert_eq!(modes(), [Mode::Free, Mode::Free]);
    // (b) another room's ticket, a made-up one, a ticket on another room's id, a word it does not know
    assert_eq!(lobby::mode(a, &id, &other_host, Mode::Cat), Ok(false));
    assert_eq!(
        lobby::mode(a, &id, "0123456789abcdef", Mode::Cat),
        Ok(false)
    );
    assert_eq!(lobby::mode(a, &other, &g, Mode::Cat), Ok(false));
    assert_eq!(say(a, &format!("MODE {id} {g} pro")), "ERR mode");
    assert_eq!(modes(), [Mode::Free, Mode::Free]);
    assert!(!say(a, "LOBBY 2017-18").contains("mode"));
    assert_eq!(lobby::mode(a, &id, &g, Mode::Cat), Ok(true));
    assert_eq!(modes(), [Mode::Free, Mode::Cat]);
    assert!(say(a, "LOBBY 2017-18").contains(r#""ready":false,"mode":"cat"}"#));
    // and back; the host's seat is its own
    assert_eq!(lobby::mode(a, &id, &host, Mode::Cat), Ok(true));
    assert_eq!(lobby::mode(a, &id, &g, Mode::Free), Ok(true));
    assert_eq!(modes(), [Mode::Cat, Mode::Free]);
    // a closed room's tickets mark nothing
    assert_eq!(lobby::close(a, &id, &host), Ok(true));
    assert_eq!(lobby::mode(a, &id, &g, Mode::Cat), Ok(false));
}

/// The words before `MODE` answer the same bytes: a free room's `LOBBY` entry has no `mode`.
#[test]
fn old_words_answer_byte_identically() {
    let raw = |a: SocketAddr, line: &str| {
        let mut c = TcpStream::connect(a).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(format!("{line}\n").as_bytes()).unwrap();
        let mut s = String::new();
        c.read_to_string(&mut s).unwrap();
        s
    };
    let d = desk(|_| {});
    let a = d.addr();
    let (id, code, host) = room(a, false);
    let g = guest(a, &id);
    assert_eq!(raw(a, &format!("READY {id} {g}")), "OK\n");
    assert_eq!(raw(a, &format!("READY {id} 0123456789abcdef")), "GONE\n");
    assert_eq!(raw(a, &format!("HEARTBEAT {host}")), "OK\n");
    let r = listed(a, &id).unwrap();
    let (c1, c2) = (&r.occupants[0].club, &r.occupants[1].club);
    let entry = format!(
        r#"{{"id":"{id}","code":"{code}","players":2,"max":2,"kind":2,"occupants":[{{"seat":1,"club":"{c1}","ready":false}},{{"seat":2,"club":"{c2}","ready":true}}]}}"#
    );
    let l = raw(a, "LOBBY 2017-18");
    assert!(l.starts_with(r#"{"v":1,"#) && l.ends_with('\n'), "{l}");
    assert!(l.contains(&entry), "{l}");
    assert_eq!(raw(a, &format!("CLOSE {id} {g}")), "OK\n");
    assert_eq!(raw(a, &format!("RELEASE {host}")), "OK\n");
    assert_eq!(raw(a, &format!("RESERVE {id}")), "CLOSED\n");
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
fn room_leases_follow_the_arcade_rules() {
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
    m.hello(IpAddr::from([10, 0, 0, n]), [n; 32], ticket, None, t);
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
        for w in [
            "CREATE vs",
            "CREATE vs open",
            "READY 0a1b2c3d t",
            "MODE 0a1b2c3d t cat",
        ] {
            assert!(say(a, w).starts_with("ERR"), "{name} {w}");
        }
        // 2017-18 knows CLOSE for its arcade rooms, which need no matcher
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

/// A host's room secret: 64 hex digits.
fn room_secret(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
}

fn arcade_room(a: SocketAddr, host: &str, public: bool) -> (String, String, String) {
    kept_room(a, host, None, public)
}

/// An arcade room made with the host's room secret (`None`: as a Launcher before secrets).
fn kept_room(
    a: SocketAddr,
    host: &str,
    secret: Option<&str>,
    public: bool,
) -> (String, String, String) {
    match lobby::create_arcade(a, host, secret, public, "").unwrap() {
        Created::Room {
            id,
            code,
            ticket,
            seat: 1,
        } => (id, code, ticket),
        r => panic!("{r:?}"),
    }
}

/// 2017-18 arcade rooms: joined by code alone; a guest's ticket gets the host's tunnel id and tells its own, the host's
/// ticket gets the guests'; leaving and the host's `CLOSE` as for VS rooms. No matcher needed: a Launcher's arcade desk
/// (`--no-gmsvr`) holds them too.
#[test]
fn arcade_rooms_join_by_code_and_swap_tunnel_ids() {
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
    let (id, code, host_t) = arcade_room(a, &host_id, false);
    assert_eq!(code.len(), 8);
    let guest_t = match lobby::join_arcade(a, &code.to_lowercase(), None).unwrap() {
        Reserved::Ticket { ticket, seat: 2 } => ticket,
        r => panic!("{r:?}"),
    };
    assert_eq!(
        lobby::arcade_peers(a, &host_t, None).unwrap(),
        Some(Peers::Guests(vec![]))
    );
    assert_eq!(
        lobby::arcade_peers(a, &guest_t, Some(&guest_id)).unwrap(),
        Some(Peers::Host(host_id.clone()))
    );
    assert_eq!(
        lobby::arcade_peers(a, &host_t, None).unwrap(),
        Some(Peers::Guests(vec![guest_id.clone()]))
    );
    assert_eq!(
        lobby::arcade_peers(a, "0000000000000000", None).unwrap(),
        None,
        "GONE"
    );
    assert!(lobby::heartbeat(a, &guest_t).unwrap());
    assert!(lobby::release(a, &guest_t).unwrap(), "the guest leaves");
    assert_eq!(
        lobby::arcade_peers(a, &host_t, None).unwrap(),
        Some(Peers::Guests(vec![]))
    );
    assert!(
        lobby::close(a, &id, &host_t).unwrap(),
        "the host closes the room"
    );
    assert!(!lobby::heartbeat(a, &host_t).unwrap());
    assert_eq!(
        lobby::join_arcade(a, &code, None).unwrap(),
        Reserved::Closed
    );
}

/// What `LOBBY` says lets a stranger into a public room and nowhere else: a private room is not listed and its id
/// joins nothing; a public one is listed (marked a room) without its code, and `JOIN` of its id seats a guest; the
/// code still works for both; `RESERVE` of a room id gives nothing.
#[test]
fn listed_rooms_join_by_id_private_ones_by_code_only() {
    let d = desk(|c| c.vs = None);
    let a = d.addr();
    let host_id = tunnel_id(1);
    let (private_id, private_code, private_t) = arcade_room(a, &host_id, false);
    let other_host = tunnel_id(2); // one room per host id
    // a named room: the desk keeps the name as the words carry it
    let (public_id, public_code, public_t) =
        match lobby::create_arcade(a, &other_host, None, true, " Sala \t Roma ").unwrap() {
            Created::Room {
                id, code, ticket, ..
            } => (id, code, ticket),
            r => panic!("{r:?}"),
        };
    let line = say(a, "LOBBY 2017-18");
    let l = Lobby::parse(&line).unwrap();
    assert!(
        !l.arcades.iter().any(|s| s.id == private_id),
        "private: not listed"
    );
    let row = l
        .arcades
        .iter()
        .find(|s| s.id == public_id)
        .expect("public: listed");
    assert_eq!((row.seats, row.max, row.room), (1, 8, true));
    assert_eq!(row.name, "Sala Roma");
    assert_eq!(
        l.arcades.iter().filter(|s| s.room).count(),
        1,
        "the desk's own arcade is no room"
    );
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
    assert_eq!(lobby::reserve(a, &public_id).unwrap(), Reserved::Closed);
    assert_eq!(
        lobby::join_arcade(a, &private_id, None).unwrap(),
        Reserved::Closed,
        "a private room's id"
    );
    assert!(matches!(
        lobby::join_arcade(a, &public_id, None).unwrap(),
        Reserved::Ticket { seat: 2, .. }
    ));
    assert!(matches!(
        lobby::join_arcade(a, &public_code, None).unwrap(),
        Reserved::Ticket { seat: 3, .. }
    ));
    assert!(matches!(
        lobby::join_arcade(a, &private_code, None).unwrap(),
        Reserved::Ticket { seat: 2, .. }
    ));
    let line = say(a, "LOBBY 2017-18");
    for secret in [&private_code, &public_code] {
        assert!(!line.contains(secret.as_str()), "{secret} in {line}");
    }
}

/// One address holds at most 2 arcade rooms (and 8 VS rooms: players behind one NAT) at once, and one tunnel id one
/// room; closing one makes room again.
#[test]
fn rooms_per_address_and_per_host_are_capped() {
    let d = desk(|_| {});
    let a = d.addr();
    let (id, _, t) = arcade_room(a, &tunnel_id(1), false);
    assert_eq!(
        say(a, &format!("CREATE shop {}", tunnel_id(1))),
        "ERR host",
        "one room per host"
    );
    arcade_room(a, &tunnel_id(2), false);
    assert_eq!(
        lobby::create_arcade(a, &tunnel_id(3), None, false, "").unwrap(),
        Created::Full
    );
    assert!(lobby::close(a, &id, &t).unwrap());
    arcade_room(a, &tunnel_id(1), false);
    let vs: Vec<_> = (0..8).map(|_| room(a, false)).collect();
    assert_eq!(
        lobby::create(a, false).unwrap(),
        Created::Full,
        "VS rooms have their own 8"
    );
    assert!(lobby::close(a, &vs[0].0, &vs[0].2).unwrap());
    room(a, false);
}

/// Wrong codes are braked per address (that address waits, the right code included); a desk-wide ceiling exists
/// too, far above it.
#[test]
fn wrong_codes_slow_the_address_that_sends_them() {
    let d = desk(|c| {
        c.join_fail_per_address = 3;
        c.join_fail_address_window = Duration::from_millis(800);
    });
    let a = d.addr();
    let (_, code, _) = arcade_room(a, &tunnel_id(1), false);
    for _ in 0..3 {
        assert_eq!(
            lobby::join_arcade(a, "AAAAAAAA", None).unwrap(),
            Reserved::Closed
        );
    }
    assert_eq!(say(a, &format!("JOIN {code}")), "ERR slow");
    std::thread::sleep(Duration::from_millis(900));
    assert!(matches!(
        lobby::join_arcade(a, &code, None).unwrap(),
        Reserved::Ticket { seat: 2, .. }
    ));
}

#[test]
fn the_desk_wide_ceiling_still_trips() {
    let d = desk(|c| {
        (c.join_fail_per_address, c.join_fail_limit) = (100, 2);
        c.join_fail_window = Duration::from_millis(800);
    });
    let a = d.addr();
    let (_, code, _) = arcade_room(a, &tunnel_id(1), false);
    for _ in 0..2 {
        assert_eq!(
            lobby::join_arcade(a, "AAAAAAAA", None).unwrap(),
            Reserved::Closed
        );
    }
    assert_eq!(say(a, &format!("JOIN {code}")), "ERR slow");
    let defaults = DeskConfig::new(Edition::E1718);
    assert!(
        defaults.join_fail_limit >= 100 * defaults.join_fail_per_address,
        "a high ceiling by default"
    );
}

/// A host from before room secrets that crashed and came back within the lease opens a new room once its old one has
/// had no heartbeat for a while; a live room is never taken over.
#[test]
fn a_host_replaces_its_own_stale_room() {
    let d = desk(|c| c.old_host_stale = Duration::from_millis(300));
    let a = d.addr();
    let (_, _, old_t) = arcade_room(a, &tunnel_id(1), false);
    assert_eq!(
        say(a, &format!("CREATE shop {}", tunnel_id(1))),
        "ERR host",
        "alive: kept"
    );
    std::thread::sleep(Duration::from_millis(400));
    let (_, _, new_t) = arcade_room(a, &tunnel_id(1), false);
    assert!(
        !lobby::heartbeat(a, &old_t).unwrap(),
        "the stale room is gone"
    );
    assert!(lobby::heartbeat(a, &new_t).unwrap());
    assert_eq!(
        say(a, &format!("CREATE shop {}", tunnel_id(1))),
        "ERR host",
        "just beat: alive"
    );
}

/// The 0.7.0 timeline at 1/20 of the time: a host without a room secret beats, crashes and is back at once (`ERR
/// host`); the retry its Launcher has compiled in, 15 + 2 s later, gets the room. By default the old rule waits less.
#[test]
fn a_0_7_0_host_back_from_a_crash_gets_its_room_on_the_retry() {
    const RETRY_0_7_0: Duration = Duration::from_secs(15 + 2);
    let defaults = DeskConfig::new(Edition::E1718);
    assert!(defaults.old_host_stale < RETRY_0_7_0);
    let d = desk(|c| c.old_host_stale = defaults.old_host_stale / 20);
    let a = d.addr();
    let host = tunnel_id(1);
    let (_, _, t) = arcade_room(a, &host, false);
    assert!(lobby::heartbeat(a, &t).unwrap(), "its last beat");
    assert_eq!(
        say(a, &format!("CREATE shop {host}")),
        "ERR host",
        "at once"
    );
    std::thread::sleep(RETRY_0_7_0 / 20);
    assert!(say(a, &format!("CREATE shop {host}")).starts_with("ROOM "));
}

/// A room made with a room secret is replaced by that secret alone and only once quiet, with its code (the host
/// restarted): a secret read off the wire takes no live room, under its tunnel id or another, and whoever learnt the
/// tunnel id, with another secret or none, gets `ERR host` even when the room has gone quiet. A stale time of two
/// missed 20 s heartbeats by default.
#[test]
fn only_its_secret_replaces_a_quiet_room() {
    let d = desk(|c| c.host_stale = Duration::from_millis(300));
    let a = d.addr();
    let (host, mine) = (tunnel_id(1), room_secret(1));
    let (_, code, old_t) = kept_room(a, &host, Some(&mine), false);
    for id in [&host, &tunnel_id(2)] {
        let line = format!("CREATE shop {id} {mine} private");
        assert_eq!(say(a, &line), "ERR host", "live: {id}");
    }
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        say(a, &format!("CREATE shop {host}")),
        "ERR host",
        "no secret"
    );
    assert_eq!(
        say(a, &format!("CREATE shop {host} {} private", room_secret(2))),
        "ERR host",
        "another secret"
    );
    assert!(
        lobby::heartbeat(a, &old_t).unwrap(),
        "the room is untouched"
    );
    let line = format!("CREATE shop {host} {mine} private");
    assert_eq!(say(a, &line), "ERR host", "beating again: live");
    std::thread::sleep(Duration::from_millis(400));
    let (_, again, _) = kept_room(a, &host, Some(&mine), false);
    assert_eq!(again, code);
    assert!(!lobby::heartbeat(a, &old_t).unwrap(), "replaced");
    let defaults = DeskConfig::new(Edition::E1718);
    assert!(defaults.host_stale >= Duration::from_secs(2 * lobby::HEARTBEAT_EVERY));
}

/// A kept code belongs to the room secret, not to the address or the tunnel id: from the host's own address, another
/// secret or none (a guest behind the same NAT who learnt the tunnel id) gets a fresh code, and its rooms take nothing
/// away from the holder, who gets the code back from any tunnel id once its room is closed (one live room per secret:
/// while it lives, another tunnel id with that secret gets `ERR host`). The secret shows in no lobby answer or log line.
#[test]
fn a_kept_code_belongs_to_its_room_secret() {
    let said = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = said.clone();
    let mut c = DeskConfig::new(Edition::E1718);
    (c.bind, c.port, c.lobby_gap, c.lobby_cache) =
        ("127.0.0.1".into(), 0, Duration::ZERO, Duration::ZERO);
    let d = start(c, Arc::new(move |l| log.lock().unwrap().push(l.into()))).unwrap();
    let a = d.addr();
    let (host, mine) = (tunnel_id(1), room_secret(1));
    let made = |secret: Option<&str>| {
        let (id, code, t) = kept_room(a, &host, secret, true);
        let line = say(a, "LOBBY 2017-18");
        assert!(
            !line.contains(&mine) && !line.contains(&room_secret(2)),
            "{line}"
        );
        assert!(lobby::close(a, &id, &t).unwrap());
        code
    };
    let code = made(Some(&mine));
    assert_ne!(
        made(Some(&room_secret(2))),
        code,
        "same address, another secret"
    );
    assert_ne!(made(None), code, "same address, no secret");
    assert_eq!(made(Some(&mine)), code, "still the holder's");
    let (id, live, t) = kept_room(a, &host, Some(&mine), false);
    assert_eq!(live, code);
    let line = format!("CREATE shop {} {mine} private", tunnel_id(2));
    assert_eq!(say(a, &line), "ERR host", "its room is live: no second one");
    assert!(lobby::heartbeat(a, &t).unwrap(), "untouched");
    assert!(lobby::close(a, &id, &t).unwrap());
    let (_, other, _) = kept_room(a, &tunnel_id(2), Some(&mine), false);
    assert_eq!(other, code, "the secret's code, whatever the tunnel id");
    let said = said.lock().unwrap();
    assert!(!said.is_empty());
    assert!(!said.iter().any(|l| l.contains(&mine)), "{said:?}");
}

/// A Launcher before room secrets (0.7.0) sends none: its room is made as before, with a fresh code each time.
#[test]
fn a_create_without_a_secret_gets_a_fresh_code() {
    let d = desk(|c| c.arcade_rooms_per_address = 3);
    let a = d.addr();
    let (id, code, t) = arcade_room(a, &tunnel_id(1), false);
    assert!(lobby::close(a, &id, &t).unwrap());
    let (_, again, _) = arcade_room(a, &tunnel_id(1), false);
    assert_ne!(again, code);
    assert!(say(a, &format!("CREATE shop {} public", tunnel_id(2))).starts_with("ROOM "));
    assert!(
        say(
            a,
            &format!("CREATE shop {} private Sala Roma", tunnel_id(3))
        )
        .starts_with("ROOM ")
    );
}

/// At its address's room cap, a host back from a crash replaces its own stale room (not `FULL`) and gets its code.
#[test]
fn a_stale_room_is_replaced_at_the_address_cap() {
    let d = desk(|c| {
        c.host_stale = Duration::from_millis(300);
        c.arcade_rooms_per_address = 2;
    });
    let a = d.addr();
    let secret = room_secret(1);
    let (_, code, _) = kept_room(a, &tunnel_id(1), Some(&secret), false);
    let (_, _, beat) = arcade_room(a, &tunnel_id(2), false);
    assert_eq!(
        lobby::create_arcade(a, &tunnel_id(3), None, false, "").unwrap(),
        Created::Full,
        "at the cap"
    );
    std::thread::sleep(Duration::from_millis(400));
    assert!(lobby::heartbeat(a, &beat).unwrap());
    let (_, again, _) = kept_room(a, &tunnel_id(1), Some(&secret), false);
    assert_eq!(again, code);
}

/// An arcade's own desk behind its tunnel seats its PCs and nothing else: the room words are refused there.
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
    assert!(
        l.vs.is_empty() && l.arcades.len() == 1,
        "only its own arcade"
    );
    assert!(
        matches!(
            lobby::reserve(a, &l.arcades[0].id).unwrap(),
            Reserved::Ticket { .. }
        ),
        "seats still"
    );
}

/// A 2010-11 desk knows no arcade rooms.
#[test]
fn no_arcade_rooms_on_2010_11() {
    let d = desk(|c| {
        c.edition = Edition::E1011;
        c.vs = None;
    });
    assert!(say(d.addr(), &format!("CREATE shop {}", tunnel_id(1))).starts_with("ERR"));
    assert!(say(d.addr(), "JOIN K7QMZDRX").starts_with("ERR"));
}
