//! HOST SHOP and a typed code end to end, without a game: an in-process lobby desk and iroh relay, a host whose
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
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use vslink::shop::{self, Ports, Role, ShopConfig};

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
fn host_shop_then_a_guest_joins_by_code_through_the_tunnel() {
    let d = desk();
    let server = d.addr().to_string();
    let addr: SocketAddr = d.addr();
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (relay, _url, _relay_server) = rt.block_on(iroh::test_utils::run_relay_server()).unwrap();
    let cfg = |role, ports, key: &str, log| ShopConfig {
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
    let host_id = shop::own_id(&dir.path().join("host.key")).unwrap();
    let (log, host_log) = logger("host");
    let room = HostRoom::open(&server, &shop::id_text(&host_id), log.clone()).unwrap();
    assert!(!room.code.is_empty() && room.code.chars().all(|c| c.is_ascii_alphanumeric()));
    let mut c = cfg(Role::Host, host_ports, "host.key", log);
    c.admit = Some(room.admit());
    let host = shop::spawn(c).unwrap();
    assert_eq!(host.id, host_id, "the id registered is the id guests dial");

    // a stranger with the host id alone: let in by no room
    let (log, _) = logger("stranger");
    let stranger_ports = Ports {
        control: free_tcp(),
        desk: free_tcp(),
        relay: free_udp(),
    };
    let stranger = shop::spawn(cfg(Role::Join(host_id), stranger_ports, "stranger.key", log)).unwrap();
    wait_for(&host_log, "refused: not in the shop's room");
    drop(stranger);

    // the guest: the code typed, its id told, the host id dialled; the control answers through the tunnel
    let own = shop::own_id(&dir.path().join("guest.key")).unwrap();
    let joined = join_code(addr, &room.code, &shop::id_text(&own)).unwrap();
    let CodeJoin::In { ticket, host: id } = joined else {
        panic!("{joined:?}")
    };
    assert_eq!(id, shop::id_text(&host_id));
    let (log, _) = logger("guest");
    let guest_ports = Ports {
        control: free_tcp(),
        desk: free_tcp(),
        relay: free_udp(),
    };
    let guest = shop::spawn(cfg(
        Role::Join(shop::parse_id(&id).unwrap()),
        guest_ports,
        "guest.key",
        log,
    ))
    .unwrap();
    let mut c = TcpStream::connect(("127.0.0.1", guest.ports.control)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    c.write_all(b"HELLO SHOP").unwrap();
    let mut b = [0u8; 10];
    c.read_exact(&mut b).unwrap();
    assert_eq!(&b, b"HELLO SHOP");
    wait_for(&host_log, "joined");
    assert!(
        forza::online::lobby::heartbeat(addr, &ticket).unwrap(),
        "the guest's seat is held"
    );

    // the host leaves: the room and its code are gone
    let code = room.code.clone();
    drop(room);
    assert_eq!(join_code(addr, &code, &shop::id_text(&own)).unwrap(), CodeJoin::NoRoom);
    assert!(
        !forza::online::lobby::heartbeat(addr, &ticket).unwrap(),
        "the guest's ticket went with the room"
    );
    drop((guest, host));
}
