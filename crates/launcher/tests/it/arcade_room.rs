//! HOST ARCADE and a typed code end to end, without a game: an in-process lobby desk and iroh relay, a host whose
//! "control" is a TCP echo, its room made at the desk and its tunnel admitting only that room's guests; a guest that
//! joins by the code, tells its id, dials the host id the desk gave it and reaches the control; a stranger with the
//! host id alone is refused; the room closed, its code joins nothing.

use forza::service::Log;
use forza_server::desk::{DeskConfig, start};
use forza_server::{Edition, Handle};
use iroh::tls::CaTlsConfig;
use launcher::lobby::{CodeJoin, HostRoom, join_code};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use vslink::arcade::{self, ArcadeConfig, Ports, Role, Verdict};

fn desk() -> Handle {
    let mut c = DeskConfig::new(Edition::E1718);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.lobby_gap = Duration::ZERO;
    c.lobby_cache = Duration::ZERO;
    start(c, Arc::new(|_| {})).unwrap()
}

type Lines = Arc<Mutex<Vec<String>>>;

fn logger(tag: &'static str) -> (Log, Lines) {
    let lines: Lines = Arc::default();
    let l = lines.clone();
    let log: Log = Arc::new(move |s| {
        eprintln!("[{tag}] {s}");
        l.lock().unwrap().push(s.to_owned());
    });
    (log, lines)
}

fn wait_for(lines: &Lines, what: &str) {
    let t = Instant::now();
    while !lines.lock().unwrap().iter().any(|l| l.contains(what)) {
        assert!(t.elapsed() < Duration::from_secs(10), "no \"{what}\" in the log");
        thread::sleep(Duration::from_millis(50));
    }
}

/// The host's control: an echo.
fn echo() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            thread::spawn(move || {
                let mut b = [0u8; 1024];
                while let Ok(n) = c.read(&mut b) {
                    if n == 0 || c.write_all(&b[..n]).is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

fn free_tcp() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn free_udp() -> u16 {
    UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn host_arcade_then_a_guest_joins_by_code_through_the_tunnel() {
    let d = desk();
    let server = d.addr().to_string();
    let addr: SocketAddr = d.addr();
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (relay, _url, _relay_server) = rt.block_on(iroh::test_utils::run_relay_server()).unwrap();
    let cfg = |role, ports, key: &str, log| ArcadeConfig {
        role,
        ports,
        relay: relay.clone(),
        ca: Some(CaTlsConfig::insecure_skip_verify()),
        relay_only: false,
        key: dir.path().join(key),
        log,
        delay: Duration::ZERO,
        admit: None,
    };

    // the host: its room first (the id from its key), then its tunnel with the room's admit rule
    let host_ports = Ports {
        control: echo(),
        desk: free_tcp(),
        relay: free_udp(),
    };
    let host_id = arcade::own_id(&dir.path().join("host.key")).unwrap();
    let (log, host_log) = logger("host");
    let room = HostRoom::open(&server, &arcade::id_text(&host_id), false, "", log.clone(), &|_| {}).unwrap();
    // the desk's code length is the one the Launcher's box takes
    assert_eq!(room.code.len(), forza::online::lobby::ARCADE_CODE_LEN);
    assert!(room.code.chars().all(|c| c.is_ascii_alphanumeric()));
    let mut c = cfg(Role::Host, host_ports, "host.key", log);
    c.admit = Some(room.admit());
    let host = arcade::spawn(c).unwrap();
    assert_eq!(host.id, host_id, "the id registered is the id guests dial");

    // a stranger with the host id alone: let in by no room
    let (log, stranger_log) = logger("stranger");
    let stranger_ports = Ports {
        control: free_tcp(),
        desk: free_tcp(),
        relay: free_udp(),
    };
    let stranger = arcade::spawn(cfg(Role::Join(host_id), stranger_ports, "stranger.key", log)).unwrap();
    wait_for(&host_log, "refused: not in the arcade's room");
    drop(stranger);
    wait_for(&stranger_log, "the arcade refused this PC: not in its room");

    // the guest: the code typed, its id told, the host id dialled; the control answers through the tunnel
    let own = arcade::own_id(&dir.path().join("guest.key")).unwrap();
    let joined = join_code(addr, &room.code, &arcade::id_text(&own)).unwrap();
    let CodeJoin::In { ticket, host: id } = joined else {
        panic!("{joined:?}")
    };
    assert_eq!(id, arcade::id_text(&host_id));
    let (log, _) = logger("guest");
    let guest_ports = Ports {
        control: free_tcp(),
        desk: free_tcp(),
        relay: free_udp(),
    };
    let guest = arcade::spawn(cfg(
        Role::Join(arcade::parse_id(&id).unwrap()),
        guest_ports,
        "guest.key",
        log,
    ))
    .unwrap();
    let mut c = TcpStream::connect(("127.0.0.1", guest.ports.control)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    c.write_all(b"HELLO ROOM").unwrap();
    let mut b = [0u8; 10];
    c.read_exact(&mut b).unwrap();
    assert_eq!(&b, b"HELLO ROOM");
    wait_for(&host_log, "joined");
    assert!(
        forza::online::lobby::heartbeat(addr, &ticket).unwrap(),
        "the guest's seat is held"
    );

    // the host leaves: the room and its code are gone
    let code = room.code.clone();
    drop(room);
    assert_eq!(
        join_code(addr, &code, &arcade::id_text(&own)).unwrap(),
        CodeJoin::NoRoom
    );
    assert!(
        !forza::online::lobby::heartbeat(addr, &ticket).unwrap(),
        "the guest's ticket went with the room"
    );
    drop((guest, host));
}

/// A host back from a crash, its old room still on the desk: `ERR host`, a wait past the desk's stale time (said),
/// then a new room. Slow: the wait is the real one.
#[test]
fn a_host_back_from_a_crash_gets_a_new_room() {
    let d = desk();
    let dir = tempfile::tempdir().unwrap();
    let id = arcade::id_text(&arcade::own_id(&dir.path().join("host.key")).unwrap());
    let (log, _) = logger("crashed host");
    let old = HostRoom::open(&d.addr().to_string(), &id, false, "", log.clone(), &|_| {}).unwrap();
    let said = Mutex::new(vec![]);
    let note = |l: &launcher::run::Text| said.lock().unwrap().push(l.en());
    let new = HostRoom::open(&d.addr().to_string(), &id, false, "", log, &note).unwrap();
    assert_ne!(old.code, new.code);
    assert!(
        said.lock().unwrap().iter().any(|l| l.contains("asking again")),
        "the wait is said"
    );
}

/// The desk loses the room (its lease lapsed): the room says so and lets nobody in, not even a guest it named.
#[test]
fn a_lost_room_lets_nobody_in() {
    let mut c = DeskConfig::new(Edition::E1718);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.reserve_lease = Duration::from_millis(2500);
    let d = start(c, Arc::new(|_| {})).unwrap();
    let addr = d.addr();
    let dir = tempfile::tempdir().unwrap();
    let id = arcade::id_text(&arcade::own_id(&dir.path().join("host.key")).unwrap());
    let (log, lines) = logger("lost host");
    let room = HostRoom::open(&addr.to_string(), &id, false, "", log, &|_| {}).unwrap();
    let guest = arcade::own_id(&dir.path().join("guest.key")).unwrap();
    assert!(matches!(
        join_code(addr, &room.code, &arcade::id_text(&guest)).unwrap(),
        CodeJoin::In { .. }
    ));
    let admit = room.admit();
    assert_eq!(admit(&guest), Verdict::In, "the room's guest comes in");
    thread::sleep(Duration::from_secs(3)); // no heartbeat within the lease: the desk forgets the room
    // a stranger at the door makes the room ask; the answer, GONE, empties the list
    let stranger = arcade::own_id(&dir.path().join("stranger.key")).unwrap();
    assert_eq!(admit(&stranger), Verdict::NotInRoom);
    assert_eq!(admit(&guest), Verdict::NotInRoom, "a room the desk lost admits nobody");
    assert!(room.lost().load(Ordering::Relaxed));
    wait_for(&lines, "no longer holds it");
}

/// A room closed by its host (dropped) admits nobody, and is not "lost": its own CLOSE is no news.
#[test]
fn a_closed_room_admits_nobody_and_is_not_lost() {
    let d = desk();
    let addr = d.addr();
    let dir = tempfile::tempdir().unwrap();
    let id = arcade::id_text(&arcade::own_id(&dir.path().join("host.key")).unwrap());
    let (log, lines) = logger("closing host");
    let room = HostRoom::open(&addr.to_string(), &id, false, "", log, &|_| {}).unwrap();
    let guest = arcade::own_id(&dir.path().join("guest.key")).unwrap();
    join_code(addr, &room.code, &arcade::id_text(&guest)).unwrap();
    let (admit, lost) = (room.admit(), room.lost());
    assert_eq!(admit(&guest), Verdict::In);
    drop(room);
    assert_eq!(admit(&guest), Verdict::NotInRoom, "not even a guest it named");
    thread::sleep(Duration::from_millis(300));
    assert!(!lost.load(Ordering::Relaxed));
    assert!(!lines.lock().unwrap().iter().any(|l| l.contains("no longer holds it")));
}

/// "List my arcade" on: the room is a row in `LOBBY`, and a guest who picks that row (its id, no code) gets in.
#[test]
fn a_listed_room_is_joined_from_its_row() {
    let d = desk();
    let addr = d.addr();
    let dir = tempfile::tempdir().unwrap();
    let id = arcade::id_text(&arcade::own_id(&dir.path().join("host.key")).unwrap());
    let (log, _) = logger("listed host");
    let room = HostRoom::open(&addr.to_string(), &id, true, "Sala Roma", log, &|_| {}).unwrap();
    let lobby = forza::online::lobby::fetch(addr, "2017-18").unwrap();
    let row = lobby.arcades.iter().find(|s| s.room).expect("listed");
    assert_eq!(row.name, "Sala Roma");
    let guest = arcade::own_id(&dir.path().join("guest.key")).unwrap();
    assert!(matches!(
        join_code(addr, &row.id, &arcade::id_text(&guest)).unwrap(),
        CodeJoin::In { .. }
    ));
    assert_eq!(room.admit()(&guest), Verdict::In);
}

/// A desk from before room names answers ERR to the named CREATE: the host asks once more without the name.
#[test]
fn an_old_desk_gets_the_room_without_its_name() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let asked: Lines = Arc::default();
    let seen = asked.clone();
    let seen2 = asked.clone();
    thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            let mut b = [0u8; 256];
            let n = c.read(&mut b).unwrap_or(0);
            let line = String::from_utf8_lossy(&b[..n]).trim().to_owned();
            let named = line.split_whitespace().count() > 4;
            seen.lock().unwrap().push(line);
            let answer: &[u8] = if named {
                b"ERR say WANT or WANT SEAT n\n"
            } else {
                b"ROOM r1 K7QMZDRX t1 1\n"
            };
            let _ = c.write_all(answer);
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let id = arcade::id_text(&arcade::own_id(&dir.path().join("host.key")).unwrap());
    let (log, _) = logger("old desk host");
    let room = HostRoom::open(&addr.to_string(), &id, true, "Sala Roma", log, &|_| {}).unwrap();
    assert_eq!(room.code, "K7QMZDRX");
    let asked = asked.lock().unwrap();
    assert_eq!(
        asked[..2],
        [
            format!("CREATE shop {id} public Sala Roma"),
            format!("CREATE shop {id} public")
        ]
    );
    drop(asked);
    drop(room);
    // a private room sends no name: one CREATE, even on an old desk
    let n = seen2.lock().unwrap().len();
    let (log, _) = logger("old desk private host");
    HostRoom::open(&addr.to_string(), &id, false, "Sala Roma", log, &|_| {}).unwrap();
    let asked = seen2.lock().unwrap();
    assert_eq!(
        asked[n..]
            .iter()
            .filter(|l| l.starts_with("CREATE"))
            .collect::<Vec<_>>(),
        [&format!("CREATE shop {id}")]
    );
}

/// An old desk that also still holds the host's room from before (ERR host on the plain CREATE): the retry after
/// the wait stays without the name. Waits DESK_HOST_STALE + 2 s, a const with no test knob.
#[test]
fn an_old_desk_with_a_stale_room_never_gets_the_name_again() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let asked: Lines = Arc::default();
    let seen = asked.clone();
    thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            let mut b = [0u8; 256];
            let n = c.read(&mut b).unwrap_or(0);
            let line = String::from_utf8_lossy(&b[..n]).trim().to_owned();
            let named = line.split_whitespace().count() > 4;
            let mut seen = seen.lock().unwrap();
            seen.push(line);
            let answer: &[u8] = if named {
                b"ERR say WANT or WANT SEAT n\n"
            } else if seen.len() == 2 {
                b"ERR host\n"
            } else {
                b"ROOM r1 K7QMZDRX t1 1\n"
            };
            let _ = c.write_all(answer);
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let id = arcade::id_text(&arcade::own_id(&dir.path().join("host.key")).unwrap());
    let (log, _) = logger("old desk stale host");
    let room = HostRoom::open(&addr.to_string(), &id, true, "Sala Roma", log, &|_| {}).unwrap();
    assert_eq!(room.code, "K7QMZDRX");
    let asked = asked.lock().unwrap();
    assert_eq!(
        asked[1..3],
        [format!("CREATE shop {id} public"), format!("CREATE shop {id} public")]
    );
}
