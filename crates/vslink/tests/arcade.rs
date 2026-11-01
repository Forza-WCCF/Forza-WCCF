//! The arcade tunnel on one host (M2 plan 5.2): an in-process iroh test relay, a host side whose "control" is a TCP
//! echo, whose "desk" never reads and whose "relay" is a UDP echo, and a join side on other loopback ports. Checked:
//! control bytes both ways, two local UDP sockets kept apart as two flows, the test delay at 0/50/100/150 ms on the
//! round trip, the forced relay path, and a reader that never reads neither blocking the writer nor growing past
//! the 4 MiB buffer. Flows ride an 8 s stall whole and in order, and fall back to datagrams with an old peer.

use forza::service::Log;
use iroh::tls::CaTlsConfig;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use vslink::arcade::{self, ArcadeConfig, Ports, Role};

type Lines = Arc<Mutex<Vec<String>>>;

fn logger(tag: &str) -> (Log, Lines) {
    let lines: Lines = Arc::default();
    let (l, tag) = (lines.clone(), tag.to_owned());
    let log: Log = Arc::new(move |s| {
        eprintln!("[{tag}] {s}");
        l.lock().unwrap().push(s.to_owned());
    });
    (log, lines)
}

fn logged(lines: &Lines, what: &str) -> bool {
    lines.lock().unwrap().iter().any(|l| l.contains(what))
}

fn free_tcp() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn free_udp() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The host PC's side: control (echo), desk (accepts, never reads), relay (UDP echo, sources kept).
struct HostServices {
    ports: Ports,
    relay_sources: Arc<Mutex<BTreeSet<SocketAddr>>>,
    desk_conns: Arc<Mutex<Vec<TcpStream>>>,
}

fn host_services() -> HostServices {
    let control = TcpListener::bind("127.0.0.1:0").unwrap();
    let desk = TcpListener::bind("127.0.0.1:0").unwrap();
    let relay = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ports = Ports {
        control: control.local_addr().unwrap().port(),
        desk: desk.local_addr().unwrap().port(),
        relay: relay.local_addr().unwrap().port(),
    };
    thread::spawn(move || {
        for mut c in control.incoming().flatten() {
            thread::spawn(move || {
                let mut b = [0u8; 4096];
                while let Ok(n) = c.read(&mut b) {
                    if n == 0 || c.write_all(&b[..n]).is_err() {
                        return;
                    }
                }
            });
        }
    });
    let desk_conns: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
    let held = desk_conns.clone();
    thread::spawn(move || {
        for c in desk.incoming().flatten() {
            held.lock().unwrap().push(c); // held open, never read
        }
    });
    let relay_sources: Arc<Mutex<BTreeSet<SocketAddr>>> = Arc::default();
    let seen = relay_sources.clone();
    thread::spawn(move || {
        let mut b = [0u8; 2048];
        while let Ok((n, src)) = relay.recv_from(&mut b) {
            seen.lock().unwrap().insert(src);
            let _ = relay.send_to(&b[..n], src);
        }
    });
    HostServices {
        ports,
        relay_sources,
        desk_conns,
    }
}

struct Pair {
    host: arcade::ArcadeLink,
    join: Option<arcade::ArcadeLink>,
    services: HostServices,
    join_ports: Ports,
    host_log: Lines,
    join_log: Lines,
    relay: iroh::RelayMap,
    relay_only: bool,
    /// The join side speaks only the 0.6.2 datagram flows
    old_join: bool,
    dir: std::path::PathBuf,
    _relay: iroh_relay::server::Server,
    _rt: tokio::runtime::Runtime,
}

impl Pair {
    fn config(
        &self,
        role: Role,
        ports: Ports,
        key: &str,
        log: Log,
        delay: Duration,
    ) -> ArcadeConfig {
        ArcadeConfig {
            role,
            ports,
            relay: self.relay.clone(),
            ca: Some(CaTlsConfig::insecure_skip_verify()),
            relay_only: self.relay_only,
            key: self.dir.join(key),
            log,
            delay,
            admit: None,
            datagram_flows: false,
        }
    }

    /// A new join side (the old one, if any, stopped first) on fresh local ports.
    fn join(&mut self, tag: &str, knob_ms: u64) {
        drop(self.join.take());
        self.join_ports = Ports {
            control: free_tcp(),
            desk: free_tcp(),
            relay: free_udp(),
        };
        let (log, lines) = logger(tag);
        let mut cfg = self.config(
            Role::Join(self.host.id),
            self.join_ports,
            &format!("{tag}.key"),
            log,
            Duration::from_millis(knob_ms / 2),
        );
        cfg.datagram_flows = self.old_join;
        let link = arcade::spawn(cfg).unwrap();
        assert_eq!(
            link.ports, self.join_ports,
            "free ports: the ones asked for"
        );
        self.join = Some(link);
        assert!(logged(&lines, "link to the arcade up"));
        self.join_log = lines;
    }
}

/// A host and a join side through a fresh test relay; the join side has the test delay `knob_ms` / 2 each way.
fn pair(name: &str, knob_ms: u64, relay_only: bool) -> Pair {
    pair_with(name, knob_ms, relay_only, None, (false, false))
}

/// [`pair`], the relay reached through [`stall_proxy`] when `stall` is given, and `old` (host, join) sides speaking
/// only the 0.6.2 datagram flows.
fn pair_with(
    name: &str,
    knob_ms: u64,
    relay_only: bool,
    stall: Option<Arc<AtomicBool>>,
    old: (bool, bool),
) -> Pair {
    let dir = std::env::temp_dir().join(format!("vslink_arcade_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (mut relay, url, server) = rt.block_on(iroh::test_utils::run_relay_server()).unwrap();
    if let Some(blocked) = stall {
        let to = SocketAddr::from((Ipv4Addr::LOCALHOST, url.port().unwrap()));
        let via = stall_proxy(to, blocked);
        relay = iroh::RelayMap::from(
            format!("https://127.0.0.1:{via}")
                .parse::<iroh::RelayUrl>()
                .unwrap(),
        );
    }
    let services = host_services();
    let (host_log, host_lines) = logger(&format!("{name} host"));
    let host = arcade::spawn(ArcadeConfig {
        role: Role::Host,
        ports: services.ports,
        relay: relay.clone(),
        ca: Some(CaTlsConfig::insecure_skip_verify()),
        relay_only,
        key: dir.join("host.key"),
        log: host_log,
        delay: Duration::ZERO,
        admit: None,
        datagram_flows: old.0,
    })
    .unwrap();
    let mut p = Pair {
        host,
        join: None,
        services,
        join_ports: Ports {
            control: 0,
            desk: 0,
            relay: 0,
        },
        host_log: host_lines,
        join_log: Lines::default(),
        relay,
        relay_only,
        old_join: old.1,
        dir,
        _relay: server,
        _rt: rt,
    };
    p.join(&format!("{name} join"), knob_ms);
    p
}

fn at(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

/// One control round trip through the tunnel (after a first one that opens the stream).
fn control_rtt(p: &Pair) -> Duration {
    let mut c = TcpStream::connect(at(p.join_ports.control)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    c.set_nodelay(true).unwrap();
    let mut b = [0u8; 5];
    c.write_all(b"first").unwrap();
    c.read_exact(&mut b).unwrap();
    assert_eq!(&b, b"first");
    let t = Instant::now();
    c.write_all(b"again").unwrap();
    c.read_exact(&mut b).unwrap();
    assert_eq!(&b, b"again");
    t.elapsed()
}

/// Two local sockets to the relay port: each gets only its own echo; the relay saw two sources. The second round's
/// worst round trip.
fn datagram_rtt(p: &Pair) -> Duration {
    let socks: Vec<UdpSocket> = (0..2)
        .map(|_| {
            let s = UdpSocket::bind("127.0.0.1:0").unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            s
        })
        .collect();
    let mut worst = Duration::ZERO;
    for round in 0..2 {
        for (i, s) in socks.iter().enumerate() {
            let msg = [b'a' + i as u8, round];
            let t = Instant::now();
            s.send_to(&msg, at(p.join_ports.relay)).unwrap();
            let mut b = [0u8; 64];
            let (n, src) = s.recv_from(&mut b).expect("the relay's echo");
            assert_eq!(
                (&b[..n], src),
                (&msg[..], at(p.join_ports.relay)),
                "socket {i}"
            );
            if round == 1 {
                worst = worst.max(t.elapsed());
            }
        }
    }
    assert_eq!(
        p.services.relay_sources.lock().unwrap().len(),
        2,
        "two flows, two sources"
    );
    worst
}

#[test]
fn control_and_datagrams_at_0_50_100_150_ms() {
    for knob in [0u64, 50, 100, 150] {
        let p = pair(&format!("d{knob}"), knob, false);
        let (tcp, udp) = (control_rtt(&p), datagram_rtt(&p));
        eprintln!("knob {knob} ms: control round trip {tcp:?}, datagram round trip {udp:?}");
        let d = Duration::from_millis(knob);
        assert!(
            tcp >= d && udp >= d,
            "knob {knob}: {tcp:?} / {udp:?} below the delay"
        );
        assert!(
            tcp < d + Duration::from_secs(2) && udp < d + Duration::from_secs(2),
            "knob {knob}: {tcp:?} / {udp:?}"
        );
        if knob > 0 {
            assert!(logged(&p.join_log, &format!("test delay: {} ms", knob / 2)));
        }
        // a burst is held once, not once per datagram: the delay is a line, not a queue that waits per item
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let t = Instant::now();
        for i in 0..100u8 {
            s.send_to(&[b'z', i], at(p.join_ports.relay)).unwrap();
        }
        let mut b = [0u8; 8];
        for _ in 0..100 {
            s.recv_from(&mut b)
                .expect("every datagram of the burst back");
        }
        let burst = t.elapsed();
        assert!(
            burst < d + Duration::from_millis(1500),
            "knob {knob}: 100 datagrams took {burst:?}"
        );
        assert!(logged(&p.host_log, "control connection to port"));
    }
}

#[test]
fn through_the_relay_only() {
    let p = pair("relay", 0, true);
    control_rtt(&p);
    datagram_rtt(&p);
    let t = Instant::now();
    while !logged(&p.join_log, "path relay") {
        assert!(
            t.elapsed() < Duration::from_secs(15),
            "no 'path relay' in the join log"
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert!(!logged(&p.join_log, "path direct") && !logged(&p.host_log, "path direct"));
}

/// A desk that never reads: 2 MiB from the seat still go out at once (buffered on the PCs, not in the game's
/// `send`); past 4 MiB waiting the stream is closed and logged.
#[test]
fn a_reader_that_never_reads() {
    let p = pair("slow", 0, false);
    let mut c = TcpStream::connect(at(p.join_ports.desk)).unwrap();
    c.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
    let chunk = vec![7u8; 64 * 1024];
    let t = Instant::now();
    for _ in 0..32 {
        c.write_all(&chunk).unwrap();
    }
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "2 MiB took {:?}",
        t.elapsed()
    );
    let t = Instant::now();
    while p.services.desk_conns.lock().unwrap().is_empty() {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "the desk got no connection"
        );
        thread::sleep(Duration::from_millis(50));
    }
    // the rest in the background: once the tunnel stops reading, the writes block (as a full TCP socket does)
    thread::spawn(move || {
        for _ in 0..96 {
            if c.write_all(&chunk).is_err() {
                return;
            }
        }
    });
    let t = Instant::now();
    let over = |l: &Lines| logged(l, "MiB waiting to cross, closed");
    while !(over(&p.host_log) || over(&p.join_log)) {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "the 4 MiB limit was never hit"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_join_without_a_host_fails_at_start() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (relay, _url, _server) = rt.block_on(iroh::test_utils::run_relay_server()).unwrap();
    let dir = std::env::temp_dir().join(format!("vslink_arcade_nohost_{}", std::process::id()));
    let (log, _) = logger("nohost");
    let nobody = *iroh::SecretKey::generate().public().as_bytes();
    let t = Instant::now();
    let r = arcade::spawn(ArcadeConfig {
        role: Role::Join(nobody),
        ports: Ports {
            control: free_tcp(),
            desk: free_tcp(),
            relay: free_udp(),
        },
        relay,
        ca: Some(CaTlsConfig::insecure_skip_verify()),
        relay_only: false,
        key: dir.join("join.key"),
        log,
        delay: Duration::ZERO,
        admit: None,
        datagram_flows: false,
    });
    let e = r.err().expect("no host: an error");
    assert!(e.to_string().contains("did not answer"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(40));
}

/// Guests come and go: once one has left, the host holds none of its flow sockets or streams.
#[test]
fn a_guest_that_leaves_leaves_nothing_open() {
    let mut p = pair("leave", 0, false);
    let wait_open = |p: &Pair, want: (usize, usize)| {
        let t = Instant::now();
        while p.host.open() != want {
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "open {:?}, want {want:?}",
                p.host.open()
            );
            thread::sleep(Duration::from_millis(50));
        }
    };
    for round in 0..3 {
        if round > 0 {
            p.join(&format!("leave join {round}"), 0);
        }
        p.services.relay_sources.lock().unwrap().clear();
        datagram_rtt(&p);
        let mut c = TcpStream::connect(at(p.join_ports.control)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        c.write_all(b"x").unwrap();
        c.read_exact(&mut [0u8; 1]).unwrap();
        wait_open(&p, (2, 1));
        assert_eq!(
            p.host.guests().load(Ordering::Relaxed),
            1,
            "one guest connected"
        );
        drop(p.join.take());
        wait_open(&p, (0, 0));
        let t = Instant::now();
        while p.host.guests().load(Ordering::Relaxed) != 0 {
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "the guest still counted"
            );
            thread::sleep(Duration::from_millis(50));
        }
        drop(c);
    }
}

/// A guest holds at most 4 streams; the 5th is refused while the others keep working.
#[test]
fn streams_past_the_cap_are_refused() {
    let p = pair("cap", 0, false);
    let open = |i: u8| {
        let mut c = TcpStream::connect(at(p.join_ports.control)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        c.write_all(&[i]).unwrap();
        let mut b = [0u8; 1];
        let ok = c.read_exact(&mut b).is_ok() && b[0] == i;
        (c, ok)
    };
    let held: Vec<_> = (0..4).map(open).collect();
    assert!(held.iter().all(|(_, ok)| *ok), "4 streams work");
    let (_fifth, ok) = open(5);
    assert!(!ok, "the 5th gets no echo");
    assert!(logged(&p.host_log, "more than 4 streams, one refused"));
    drop(held);
}

/// A guest that opens nothing usable is closed after the grace period.
#[test]
fn an_idle_guest_is_closed() {
    let p = pair("idle", 0, false);
    let t = Instant::now();
    while !logged(&p.host_log, "nothing usable in 15 s, closed") {
        assert!(
            t.elapsed() < Duration::from_secs(25),
            "the idle guest was never closed"
        );
        thread::sleep(Duration::from_millis(200));
    }
}

/// The arcade's ports taken on the joining PC (it runs a server of its own): the join side takes free ones and says
/// which, and the tunnel works on them.
#[test]
fn taken_ports_move_the_join_side() {
    let mut p = pair("moved", 0, false);
    drop(p.join.take());
    let busy_tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let busy_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let want = Ports {
        control: busy_tcp.local_addr().unwrap().port(),
        desk: free_tcp(),
        relay: busy_udp.local_addr().unwrap().port(),
    };
    let (log, lines) = logger("moved join 2");
    let cfg = p.config(
        Role::Join(p.host.id),
        want,
        "moved2.key",
        log,
        Duration::ZERO,
    );
    let link = arcade::spawn(cfg).unwrap();
    assert_ne!(link.ports.control, want.control);
    assert_ne!(link.ports.relay, want.relay);
    assert_eq!(link.ports.desk, want.desk);
    assert!(logged(
        &lines,
        &format!("local ports TCP {} and", link.ports.control)
    ));
    p.join_ports = link.ports;
    p.join = Some(link);
    control_rtt(&p);
    datagram_rtt(&p);
}

/// A host with a room list lets in only the guests on it: the one the room knows plays, a stranger is closed.
#[test]
fn only_the_rooms_guests_come_in() {
    let mut p = pair("admit", 0, false);
    drop(p.join.take());
    let allowed: Arc<Mutex<Vec<[u8; 32]>>> = Arc::default();
    let list = allowed.clone();
    let (log, host_lines) = logger("admit host 2");
    let mut cfg = p.config(
        Role::Host,
        p.services.ports,
        "admit-host2.key",
        log,
        Duration::ZERO,
    );
    // the room's list does not come for the guest's first try: it is told to come back, and dials again
    let unsure = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let first = unsure.clone();
    cfg.admit = Some(Arc::new(move |id: &[u8; 32]| {
        if !list.lock().unwrap().contains(id) {
            arcade::Verdict::NotInRoom
        } else if first.swap(false, std::sync::atomic::Ordering::Relaxed) {
            arcade::Verdict::NoAnswer
        } else {
            arcade::Verdict::In
        }
    }));
    p.host = arcade::spawn(cfg).unwrap();
    p.host_log = host_lines;
    // a stranger: the link comes up, then the host closes it, and the stranger stops dialling and says why
    p.join("admit stranger", 0);
    let t = Instant::now();
    while !logged(&p.join_log, "the arcade refused this PC: not in its room") {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the stranger was let in, or not told"
        );
        thread::sleep(Duration::from_millis(50));
    }
    assert!(logged(&p.host_log, "refused: not in the arcade's room"));
    assert!(!logged(&p.join_log, "dialling again"));
    // the room's guest: its id on the list before it dials
    drop(p.join.take());
    let key = iroh::SecretKey::generate();
    std::fs::write(p.dir.join("admit guest.key"), key.to_bytes()).unwrap();
    allowed.lock().unwrap().push(*key.public().as_bytes());
    p.join("admit guest", 0);
    let t = Instant::now();
    while !logged(&p.host_log, "joined") {
        assert!(
            t.elapsed() < Duration::from_secs(15),
            "the guest did not come back"
        );
        thread::sleep(Duration::from_millis(50));
    }
    assert!(logged(
        &p.host_log,
        "did not come in time, told to come back"
    ));
    assert!(logged(&p.join_log, "dialling again"));
    assert!(!unsure.load(std::sync::atomic::Ordering::Relaxed));
    control_rtt(&p);
}

/// A TCP proxy to `to` that, while `blocked` is set, moves no byte either way (new connections wait too): the path
/// through the relay stalls, and what iroh queues meanwhile may be dropped.
fn stall_proxy(to: SocketAddr, blocked: Arc<AtomicBool>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        for a in l.incoming().flatten() {
            let Ok(b) = TcpStream::connect(to) else {
                continue;
            };
            for (mut r, mut w) in [(a.try_clone().unwrap(), b.try_clone().unwrap()), (b, a)] {
                let blocked = blocked.clone();
                thread::spawn(move || {
                    let mut buf = [0u8; 16 * 1024];
                    while let Ok(n) = r.read(&mut buf) {
                        while blocked.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_millis(10));
                        }
                        if n == 0 || w.write_all(&buf[..n]).is_err() {
                            let _ = w.shutdown(std::net::Shutdown::Both);
                            return;
                        }
                    }
                });
            }
        }
    });
    port
}

/// The game's peer packets (about 15 a second each way, 520 bytes) through an 8 s stall of the only path: every one
/// comes back from the relay's echo, once and in order, after the stall.
#[test]
fn a_stall_pauses_the_flow_and_loses_nothing() {
    let blocked = Arc::new(AtomicBool::new(false));
    let p = pair_with("stall", 0, true, Some(blocked.clone()), (false, false));
    assert!(logged(&p.join_log, "arcade link: flows on a stream"));
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let r = s.try_clone().unwrap();
    const N: u32 = 200; // 13 s at 15 a second
    let got = thread::spawn(move || {
        let (mut got, mut gap, mut last) = (Vec::new(), Duration::ZERO, Instant::now());
        let mut b = [0u8; 1024];
        while got.len() < N as usize {
            let Ok(n) = r.recv(&mut b) else { break };
            assert_eq!(n, 520);
            got.push(u32::from_le_bytes(b[..4].try_into().unwrap()));
            gap = gap.max(last.elapsed());
            last = Instant::now();
        }
        (got, gap)
    });
    let mut pkt = [0u8; 520];
    let mut stalled = None;
    for i in 0..N {
        if i == 30 {
            blocked.store(true, Ordering::Relaxed);
            stalled = Some(Instant::now());
        }
        if stalled.is_some_and(|t| t.elapsed() >= Duration::from_secs(8)) {
            blocked.store(false, Ordering::Relaxed);
            stalled = None;
        }
        pkt[..4].copy_from_slice(&i.to_le_bytes());
        s.send_to(&pkt, at(p.join_ports.relay)).unwrap();
        thread::sleep(Duration::from_millis(66));
    }
    let (got, gap) = got.join().unwrap();
    assert_eq!(
        got,
        (0..N).collect::<Vec<_>>(),
        "every packet, once, in order"
    );
    // the proxy really cut the only path
    assert!(gap >= Duration::from_secs(6), "longest silence {gap:?}");
}

/// A new side with a 0.6.2 one, either way round: datagram flows, and the packets still cross.
#[test]
fn an_old_peer_gets_datagram_flows() {
    for (name, old) in [("oldhost", (true, false)), ("oldjoin", (false, true))] {
        let p = pair_with(name, 0, false, None, old);
        datagram_rtt(&p);
        control_rtt(&p);
        assert!(logged(&p.host_log, "joined, flows as datagrams"), "{name}");
        if !old.1 {
            assert!(
                logged(&p.join_log, "arcade link: flows as datagrams"),
                "{name}"
            );
        }
    }
}
