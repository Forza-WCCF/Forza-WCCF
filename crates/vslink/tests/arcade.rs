//! The arcade tunnel on one host (M2 plan 5.2): an in-process iroh test relay, a host side whose "control" is a TCP
//! echo, whose "desk" never reads and whose "relay" is a UDP echo, and a join side on other loopback ports. Checked:
//! control bytes both ways, two local UDP sockets kept apart as two flows, the test delay at 0/50/100/150 ms on the
//! round trip, the forced relay path, and a reader that never reads neither blocking the writer nor growing past
//! the 4 MiB buffer; the streams in lz4 between new ends, plain when either end is old. Flows ride a stall whole and in
//! order (datagram flows lose packets in the same stall), carry packets over the datagram limit, open a fresh stream
//! after a reconnect, and stay datagrams with a 0.7.1 or 0.6.3 peer.

use forza::service::{Handle, Log};
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
    relay_ids: Arc<Mutex<Vec<u32>>>,
    desk_conns: Arc<Mutex<Vec<TcpStream>>>,
}

/// A loopback UDP socket with an 8 MB receive buffer: the burst after a stall overflows the default one.
fn roomy_udp() -> UdpSocket {
    let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None).unwrap();
    s.set_recv_buffer_size(8 << 20).unwrap();
    s.bind(&at(0).into()).unwrap();
    s.into()
}

fn host_services() -> HostServices {
    let control = TcpListener::bind("127.0.0.1:0").unwrap();
    let desk = TcpListener::bind("127.0.0.1:0").unwrap();
    let relay = roomy_udp();
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
    let relay_ids: Arc<Mutex<Vec<u32>>> = Arc::default();
    let ids = relay_ids.clone();
    thread::spawn(move || {
        let mut b = vec![0u8; 65536];
        let mut echoed = 0u32;
        while let Ok((n, src)) = relay.recv_from(&mut b) {
            seen.lock().unwrap().insert(src);
            if n >= 4 {
                ids.lock()
                    .unwrap()
                    .push(u32::from_le_bytes(b[..4].try_into().unwrap()));
            }
            let _ = relay.send_to(&b[..n], src);
            echoed += 1;
            if echoed.is_multiple_of(16) {
                // the pace the stall's sender keeps: a burst echoed at once overflows the host flow socket's default
                // receive buffer, a loopback artifact the game's pace does not produce
                thread::sleep(Duration::from_millis(1));
            }
        }
    });
    HostServices {
        ports,
        relay_sources,
        relay_ids,
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
    /// The newest ALPN the join side speaks (`None`: all), as an older Forza-WCCF
    old_join: Option<&'static [u8]>,
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
            speak_up_to: None,
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
        cfg.speak_up_to = self.old_join;
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
    pair_with(name, knob_ms, relay_only, None, (None, None))
}

/// [`pair`], the relay reached through [`stall_proxy`] when `stall` is given, and the `old` (host, join) sides speaking
/// that ALPN as their newest, as an older Forza-WCCF.
fn pair_with(
    name: &str,
    knob_ms: u64,
    relay_only: bool,
    stall: Option<Arc<AtomicBool>>,
    old: (Option<&'static [u8]>, Option<&'static [u8]>),
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
        speak_up_to: old.0,
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
        speak_up_to: None,
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

/// The host stops its arcade: the guest hears "the host closed the arcade" (code 6) and dials again, which is also all
/// a 0.6.1 guest does with any close but "not in the room".
#[test]
fn a_host_that_stops_tells_its_guest() {
    let p = pair("closing", 0, false);
    let closed = p.join.as_ref().unwrap().host_closed();
    assert!(!closed.load(Ordering::Relaxed));
    drop(p.host);
    let t = Instant::now();
    while !closed.load(Ordering::Relaxed) {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the guest never heard the host close"
        );
        thread::sleep(Duration::from_millis(50));
    }
    assert!(logged(&p.join_log, "the host closed the arcade"));
    assert!(
        logged(&p.join_log, "arcade closed (code 6)"),
        "the host's own code, not the endpoint's 0"
    );
    assert!(logged(&p.join_log, "dialling again"));
}

/// The guest stops its own tunnel (the game closed): that is not the host closing the arcade.
#[test]
fn a_guest_that_stops_is_not_told_the_host_closed() {
    let mut p = pair("guest_stops", 0, false);
    let join = p.join.take().unwrap();
    let closed = join.host_closed();
    drop(join);
    assert!(!closed.load(Ordering::Relaxed));
    assert!(!logged(&p.join_log, "the host closed the arcade"));
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

/// Waits up to 10 s for a log line containing `what`.
fn wait_logged(lines: &Lines, what: &str) {
    let t = Instant::now();
    while !logged(lines, what) {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "no {what:?} in the log"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// The 261 feed's size and shape (125 answers of 2,080 bytes, nearly all zero) through the control echo.
fn feed_round_trip(control: u16) {
    let mut feed = Vec::new();
    for i in 0..125u32 {
        let mut answer = vec![0u8; 2080];
        answer[..4].copy_from_slice(&i.to_le_bytes());
        feed.extend_from_slice(&answer);
    }
    let mut c = TcpStream::connect(at(control)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut w = c.try_clone().unwrap();
    let sent = feed.clone();
    let writer = thread::spawn(move || w.write_all(&sent).unwrap());
    let mut back = vec![0u8; feed.len()];
    c.read_exact(&mut back).unwrap();
    writer.join().unwrap();
    assert!(back == feed, "the feed came back changed");
}

/// New host, new guest: both offer the lz4 ALPN, the streams are compressed, and each end logs what crossed.
#[test]
fn new_and_new_compress() {
    let p = pair("lz4", 0, false);
    assert!(logged(
        &p.join_log,
        "link to the arcade up (streams in lz4, flows on a stream)"
    ));
    wait_logged(&p.host_log, "joined (streams in lz4, flows on a stream)");
    feed_round_trip(p.join_ports.control);
    control_rtt(&p);
    // 260 KB each way through the echo: the guest's line shows them shrunk, its own bytes the same as the plain line
    wait_logged(
        &p.join_log,
        "control connection closed (260000 bytes out, 260000 in)",
    );
    // the lz4 line is the next one out: wait for it rather than race the two log calls
    wait_logged(&p.join_log, "control connection lz4: out 260 KB -> ");
    let line = p
        .join_log
        .lock()
        .unwrap()
        .iter()
        .find(|l| l.starts_with("control connection lz4: out 260 KB -> "))
        .cloned()
        .expect("the guest's lz4 line");
    let across: u64 = line
        .split(" -> ")
        .nth(1)
        .and_then(|s| s.strip_suffix(" KB, in 260 KB"))
        .expect(&line)
        .parse()
        .expect(&line);
    assert!(across < 26, "{line}: under a tenth");
    wait_logged(&p.host_log, "control connection lz4: out 260 KB -> ");
}

/// An endpoint of 0.6.3 or before: only the plain ALPN, no frames.
fn old_endpoint(p: &Pair, host: bool) -> iroh::Endpoint {
    let mut b = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(iroh::RelayMode::Custom(p.relay.clone()))
        .ca_tls_config(CaTlsConfig::insecure_skip_verify())
        .clear_address_lookup();
    if host {
        b = b.alpns(vec![arcade::ARCADE_ALPN.to_vec()]);
    }
    p._rt.block_on(async {
        let ep = b.bind().await.unwrap();
        ep.online().await;
        ep
    })
}

/// New host, old guest: the link speaks the plain ALPN and the bytes cross as they are.
#[test]
fn new_host_old_guest_stays_plain() {
    let mut p = pair("oldguest", 0, false);
    drop(p.join.take());
    let guest = old_endpoint(&p, false);
    let url = p.relay.urls::<Vec<iroh::RelayUrl>>().remove(0);
    let host = iroh::EndpointAddr::new(iroh::PublicKey::from_bytes(&p.host.id).unwrap())
        .with_relay_url(url);
    let back = p._rt.block_on(async {
        let conn = guest.connect(host, arcade::ARCADE_ALPN).await.unwrap();
        assert_eq!(conn.alpn(), arcade::ARCADE_ALPN);
        let (mut w, mut r) = conn.open_bi().await.unwrap();
        w.write_all(&[arcade::KIND_CONTROL]).await.unwrap();
        w.write_all(&[0u8; 3000]).await.unwrap();
        w.write_all(b"plain").await.unwrap();
        let mut b = vec![0u8; 3005];
        r.read_exact(&mut b).await.unwrap();
        // the stream ends, not the link: a guest's close aborts its streams before their "closed" lines
        w.finish().unwrap();
        (conn, b)
    });
    let (conn, back) = (back.0, back.1);
    assert_eq!(&back[3000..], b"plain");
    assert!(back[..3000].iter().all(|&b| b == 0));
    wait_logged(
        &p.host_log,
        "streams uncompressed, flows as datagrams (the other PC runs Forza-WCCF 0.6.3 or older)",
    );
    wait_logged(
        &p.host_log,
        "control connection closed (3005 bytes out, 3005 in)",
    );
    assert!(!logged(&p.host_log, "connection lz4"));
    conn.close(0u8.into(), b"bye");
    p._rt.block_on(guest.close());
}

/// Old host, new guest: the guest falls back to the plain ALPN and its game's bytes cross as they are.
#[test]
fn old_host_new_guest_stays_plain() {
    let p = pair("oldhost", 0, false);
    let host = old_endpoint(&p, true);
    // the old host's echo: the kind byte, then everything back unchanged
    let h = host.clone();
    p._rt.spawn(async move {
        let Some(incoming) = h.accept().await else {
            return;
        };
        let Ok(conn) = incoming.await else {
            return;
        };
        while let Ok((mut w, mut r)) = conn.accept_bi().await {
            tokio::spawn(async move {
                let mut k = [0u8; 1];
                if r.read_exact(&mut k).await.is_err() || k[0] != arcade::KIND_CONTROL {
                    return;
                }
                let mut b = vec![0u8; 4096];
                while let Ok(Some(n)) = r.read(&mut b).await {
                    if w.write_all(&b[..n]).await.is_err() {
                        return;
                    }
                }
                let _ = w.finish();
            });
        }
    });
    let (log, lines) = logger("oldhost new join");
    let ports = Ports {
        control: free_tcp(),
        desk: free_tcp(),
        relay: free_udp(),
    };
    let cfg = p.config(
        Role::Join(*host.id().as_bytes()),
        ports,
        "oldhost-join.key",
        log,
        Duration::ZERO,
    );
    let join = arcade::spawn(cfg).unwrap();
    assert!(logged(
        &lines,
        "link to the arcade up (streams uncompressed, flows as datagrams (the other PC runs Forza-WCCF 0.6.3 or older))"
    ));
    feed_round_trip(join.ports.control);
    wait_logged(
        &lines,
        "control connection closed (260000 bytes out, 260000 in)",
    );
    assert!(!logged(&lines, "connection lz4"));
    drop(join);
    p._rt.block_on(host.close());
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

/// Packets of [`STALL_SIZE`] bytes from the join side to the host's relay port through the stall proxy, counted
/// where the host receives them: 20 sent and all arrived; then `during` sent while the proxy holds every byte (none may
/// arrive meanwhile); then 20 after it lets go. The ids the host got and the ids of the echoes back at the sender, each
/// in arrival order, once all have come or nothing came for `quiet`.
fn through_a_stall(
    p: &Pair,
    blocked: &AtomicBool,
    during: u32,
    quiet: Duration,
) -> (Vec<u32>, Vec<u32>) {
    const AROUND: u32 = 20;
    let total = (2 * AROUND + during) as usize;
    let ids = &p.services.relay_ids;
    ids.lock().unwrap().clear();
    let s = roomy_udp();
    let echoes: Arc<Mutex<Vec<u32>>> = Arc::default();
    let (r, back, done) = (
        s.try_clone().unwrap(),
        echoes.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    r.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let reader = {
        let done = done.clone();
        thread::spawn(move || {
            let mut b = vec![0u8; 65536];
            while !done.load(Ordering::Relaxed) {
                if let Ok(n) = r.recv(&mut b)
                    && n >= 4
                {
                    back.lock()
                        .unwrap()
                        .push(u32::from_le_bytes(b[..4].try_into().unwrap()));
                }
            }
        })
    };
    let mut pkt = vec![0u8; STALL_SIZE];
    let mut send = |i: u32| {
        pkt[..4].copy_from_slice(&i.to_le_bytes());
        s.send_to(&pkt, at(p.join_ports.relay)).unwrap();
    };
    let wait_for = |ids: &Arc<Mutex<Vec<u32>>>, n: usize, quiet: Duration| {
        let (mut seen, mut last) = (0, Instant::now());
        while seen < n && last.elapsed() < quiet {
            thread::sleep(Duration::from_millis(20));
            let now = ids.lock().unwrap().len();
            if now > seen {
                (seen, last) = (now, Instant::now());
            }
        }
    };
    let wait = |n: usize, quiet: Duration| wait_for(ids, n, quiet);
    for i in 0..AROUND {
        send(i);
        thread::sleep(Duration::from_millis(20));
    }
    wait(AROUND as usize, Duration::from_secs(15));
    assert_eq!(
        ids.lock().unwrap().len(),
        AROUND as usize,
        "the first packets"
    );
    blocked.store(true, Ordering::Relaxed);
    for i in AROUND..AROUND + during {
        send(i);
        if i % 16 == 0 {
            thread::sleep(Duration::from_millis(1)); // the game's pace is lower; this keeps loopback buffers in bounds
        }
    }
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        ids.lock().unwrap().len(),
        AROUND as usize,
        "a packet crossed the cut path"
    );
    blocked.store(false, Ordering::Relaxed);
    wait((AROUND + during) as usize, quiet);
    for i in AROUND + during..total as u32 {
        send(i);
        thread::sleep(Duration::from_millis(20));
    }
    wait(total, quiet);
    wait_for(&echoes, total, quiet);
    done.store(true, Ordering::Relaxed);
    reader.join().unwrap();
    let got = ids.lock().unwrap().clone();
    let back = echoes.lock().unwrap().clone();
    (got, back)
}

/// Stall packets: 200 bytes each, 7,240 of them (1.5 MB) in the stall below. Measured on Windows, a stream holds
/// 7,072 in its window and the queue behind it, and 7,401 once its producer waits (the relay port's 64 KB receive
/// buffer takes the rest): this stall fills the queue, so a producer that drops when it is full fails here. A datagram
/// connection keeps about 1 MB.
const STALL_SIZE: usize = 200;
const STALL_PACKETS: u32 = 7_240;

/// One packet of `size` bytes from a fresh socket to the relay port; its echo, if one comes in `wait`.
fn echo(p: &Pair, size: usize, wait: Duration) -> Option<Vec<u8>> {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(wait)).unwrap();
    let pkt: Vec<u8> = (0..size).map(|i| i as u8).collect();
    s.send_to(&pkt, at(p.join_ports.relay)).unwrap();
    let mut b = vec![0u8; 65536];
    let n = s.recv(&mut b).ok()?;
    b.truncate(n);
    assert_eq!(b, pkt, "the echo, whole");
    Some(b)
}

/// The flows on a stream through a stall of the only path that holds more than the stream's window and the queue
/// behind it: the join side's producer waits for room (logged), every packet reaches the host, once and in order, and
/// every echo comes back the same way. Then a packet far over the datagram limit crosses whole, and
/// after the host restarts the join side dials again and its flows cross on a fresh stream.
#[test]
fn a_stall_pauses_the_flow_and_loses_nothing() {
    let blocked = Arc::new(AtomicBool::new(false));
    let mut p = pair_with("stall", 0, true, Some(blocked.clone()), (None, None));
    // lz4 on the streams too: this is the new link whole
    assert!(logged(
        &p.join_log,
        "link to the arcade up (streams in lz4, flows on a stream)"
    ));
    let (got, back) = through_a_stall(&p, &blocked, STALL_PACKETS, Duration::from_secs(30));
    let all = (0..STALL_PACKETS + 40).collect::<Vec<_>>();
    assert!(
        got == all,
        "every packet, once, in order: {} arrived",
        got.len()
    );
    assert!(
        back == all,
        "every echo, once, in order: {} came back",
        back.len()
    );
    // the stall filled the queue and the producer waited: a producer that drops would not log this
    assert!(logged(
        &p.join_log,
        "flows: a queue is full, waiting for room"
    ));
    assert!(
        echo(&p, 60_000, Duration::from_secs(10)).is_some(),
        "the 60,000-byte packet"
    );
    // the host restarts with its key: the link drops, the join side dials again, the flows take a fresh stream
    // the old host's endpoint is gone (its thread joined) before the new one takes its key
    drop(std::mem::replace(
        &mut p.host.handle,
        Handle::new(Arc::default(), thread::spawn(|| {})),
    ));
    let (log, lines) = logger("stall host 2");
    p.host = arcade::spawn(p.config(
        Role::Host,
        p.services.ports,
        "host.key",
        log,
        Duration::ZERO,
    ))
    .unwrap();
    p.host_log = lines;
    let t = Instant::now();
    while !logged(&p.join_log, "link to the arcade up again") {
        assert!(
            t.elapsed() < Duration::from_secs(45),
            "the join side did not come back"
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        echo(&p, STALL_SIZE, Duration::from_secs(10)).is_some(),
        "a packet after the reconnect"
    );
}

/// The control run: the same stall with datagram flows (both sides 0.7.1: lz4 streams, datagram flows) loses packets,
/// and drops the packet over the datagram limit (logged).
#[test]
fn datagram_flows_lose_packets_in_the_same_stall() {
    let blocked = Arc::new(AtomicBool::new(false));
    let lz4 = Some(arcade::ARCADE_ALPN_LZ4);
    let p = pair_with("dgramstall", 0, true, Some(blocked.clone()), (lz4, lz4));
    let t = Instant::now();
    while !logged(&p.host_log, "joined (streams in lz4, flows as datagrams") {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the host did not log the guest"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let (got, _) = through_a_stall(&p, &blocked, STALL_PACKETS, Duration::from_secs(5));
    assert!(
        got.len() < (STALL_PACKETS + 40) as usize,
        "datagram flows lost nothing ({} arrived): the stall test proves nothing",
        got.len()
    );
    assert!(echo(&p, 60_000, Duration::from_secs(2)).is_none());
    assert!(logged(&p.join_log, "dropped a 60004-byte datagram"));
    assert!(
        !logged(&p.join_log, "waiting for room"),
        "datagram flows never wait"
    );
}

/// A shop/3 guest whose flow stream sends a frame over the cap: the host logs why and closes that link
/// ([`arcade::CLOSE_BAD_FLOWS`]), as the join side does with a bad stream from the host, and dials again; on the
/// fresh link a fresh flow stream carries flows both ways again.
#[test]
fn a_bad_flow_frame_closes_the_link_and_a_fresh_one_carries_flows() {
    let mut p = pair("badflow", 0, false);
    drop(p.join.take());
    let guest = old_endpoint(&p, false); // a bare endpoint: its ALPN is the one it dials with
    let url = p.relay.urls::<Vec<iroh::RelayUrl>>().remove(0);
    let host = iroh::EndpointAddr::new(iroh::PublicKey::from_bytes(&p.host.id).unwrap())
        .with_relay_url(url);
    let frame = |flow: u32, d: &[u8]| {
        let mut f = u16::try_from(4 + d.len()).unwrap().to_le_bytes().to_vec();
        f.extend(flow.to_le_bytes());
        f.extend(d);
        f
    };
    let run = async {
        let conn = guest
            .connect(host.clone(), arcade::ARCADE_ALPN_3)
            .await
            .unwrap();
        assert_eq!(conn.alpn(), arcade::ARCADE_ALPN_3);
        let mut w = conn.open_uni().await.unwrap();
        w.write_all(&u16::MAX.to_le_bytes()).await.unwrap();
        w.write_all(&[0u8; 64]).await.unwrap();
        let why = conn.closed().await;
        assert!(
            matches!(&why, iroh::endpoint::ConnectionError::ApplicationClosed(c)
                if c.error_code == arcade::CLOSE_BAD_FLOWS.into()),
            "{why:?}"
        );

        let conn = guest.connect(host, arcade::ARCADE_ALPN_3).await.unwrap();
        let mut w = conn.open_uni().await.unwrap();
        let sent = frame(1, b"\x05\0\0\0again");
        w.write_all(&sent).await.unwrap();
        let mut r = conn.accept_uni().await.unwrap();
        let mut back = vec![0u8; sent.len()];
        r.read_exact(&mut back).await.unwrap();
        assert_eq!(back, sent, "the echo on the fresh flow stream");
        conn.close(0u8.into(), b"bye");
    };
    p._rt
        .block_on(async { tokio::time::timeout(Duration::from_secs(30), run).await })
        .expect("in 30 s");
    wait_logged(
        &p.host_log,
        "the flow stream failed (a 65535-byte frame, over the 65511-byte cap), closing the link",
    );
    p._rt.block_on(guest.close());
}

/// A host whose flow stream always opens with a frame over the cap: the join side closes each link and dials again,
/// spaced by its backoff (1 s, 2 s, 4 s ...), not in a tight loop, one log line per drop.
#[test]
fn a_host_that_always_breaks_its_flow_stream_is_dialled_ever_slower() {
    let p = pair("badhost", 0, false);
    let host = p._rt.block_on(async {
        let ep = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Custom(p.relay.clone()))
            .ca_tls_config(CaTlsConfig::insecure_skip_verify())
            .clear_address_lookup()
            .alpns(vec![arcade::ARCADE_ALPN_3.to_vec()])
            .bind()
            .await
            .unwrap();
        ep.online().await;
        ep
    });
    let dials: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let (h, d) = (host.clone(), dials.clone());
    p._rt.spawn(async move {
        while let Some(incoming) = h.accept().await {
            let Ok(conn) = incoming.await else { continue };
            d.lock().unwrap().push(Instant::now());
            tokio::spawn(async move {
                if let Ok(mut w) = conn.open_uni().await {
                    let _ = w.write_all(&u16::MAX.to_le_bytes()).await;
                }
                let _ = conn.closed().await;
            });
        }
    });
    let (log, lines) = logger("badhost join");
    let ports = Ports {
        control: free_tcp(),
        desk: free_tcp(),
        relay: free_udp(),
    };
    let cfg = p.config(
        Role::Join(*host.id().as_bytes()),
        ports,
        "badhost-join.key",
        log,
        Duration::ZERO,
    );
    let join = arcade::spawn(cfg).unwrap();
    wait_logged(&lines, "(bad flow stream 3)");
    let t = dials.lock().unwrap().clone();
    assert!(t.len() >= 3, "{t:?}");
    assert!(t[1] - t[0] >= Duration::from_millis(900), "{t:?}");
    assert!(t[2] - t[1] >= Duration::from_millis(1900), "{t:?}");
    let downs = lines
        .lock()
        .unwrap()
        .iter()
        .filter(|l| l.contains("link to the arcade down"))
        .count();
    assert!(downs <= 3, "one line per drop: {downs}");
    assert!(logged(&lines, "dialling again in 1 s (bad flow stream 1)"));
    assert!(logged(&lines, "dialling again in 2 s (bad flow stream 2)"));
    drop(join);
    p._rt.block_on(host.close());
}

/// A new side with a 0.7.1 side (lz4 streams, datagram flows) or a 0.6.3 one (plain streams, datagram flows), either
/// way round: the link is what the old side speaks today, both ends say so, and the packets and control bytes cross.
#[test]
fn an_old_peer_gets_datagram_flows() {
    use arcade::{ARCADE_ALPN, ARCADE_ALPN_LZ4};
    let lz4 = "streams in lz4, flows as datagrams (the other PC runs Forza-WCCF 0.7.1 or older)";
    let plain =
        "streams uncompressed, flows as datagrams (the other PC runs Forza-WCCF 0.6.3 or older)";
    for (name, old, text) in [
        ("lz4host", (Some(ARCADE_ALPN_LZ4), None), lz4),
        ("lz4join", (None, Some(ARCADE_ALPN_LZ4)), lz4),
        ("oldhost", (Some(ARCADE_ALPN), None), plain),
        ("oldjoin", (None, Some(ARCADE_ALPN)), plain),
    ] {
        let p = pair_with(name, 0, false, None, old);
        datagram_rtt(&p);
        control_rtt(&p);
        wait_logged(&p.host_log, &format!("joined ({text})"));
        assert!(
            logged(&p.join_log, &format!("link to the arcade up ({text})")),
            "{name}"
        );
    }
}
