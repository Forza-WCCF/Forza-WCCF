//! The shop tunnel on one host (M2 plan 5.2): an in-process iroh test relay, a host side whose "control" is a TCP
//! echo, whose "desk" never reads and whose "relay" is a UDP echo, and a join side on other loopback ports. Checked:
//! control bytes both ways, two local UDP sockets kept apart as two flows, the test delay at 0/50/100/150 ms on the
//! round trip, the forced relay path, and a reader that never reads neither blocking the writer nor growing past
//! the 4 MiB buffer.

use forza::service::Log;
use iroh::tls::CaTlsConfig;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use vslink::shop::{self, Ports, Role, ShopConfig};

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
    host: shop::ShopLink,
    join: Option<shop::ShopLink>,
    services: HostServices,
    join_ports: Ports,
    host_log: Lines,
    join_log: Lines,
    relay: iroh::RelayMap,
    relay_only: bool,
    dir: std::path::PathBuf,
    _relay: iroh_relay::server::Server,
    _rt: tokio::runtime::Runtime,
}

impl Pair {
    fn config(&self, role: Role, ports: Ports, key: &str, log: Log, delay: Duration) -> ShopConfig {
        ShopConfig {
            role,
            ports,
            relay: self.relay.clone(),
            ca: Some(CaTlsConfig::insecure_skip_verify()),
            relay_only: self.relay_only,
            key: self.dir.join(key),
            log,
            delay,
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
        let cfg = self.config(
            Role::Join(self.host.id),
            self.join_ports,
            &format!("{tag}.key"),
            log,
            Duration::from_millis(knob_ms / 2),
        );
        self.join = Some(shop::spawn(cfg).unwrap());
        assert!(logged(&lines, "link to the shop up"));
        self.join_log = lines;
    }
}

/// A host and a join side through a fresh test relay; the join side has the test delay `knob_ms` / 2 each way.
fn pair(name: &str, knob_ms: u64, relay_only: bool) -> Pair {
    let dir = std::env::temp_dir().join(format!("vslink_shop_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (relay, _url, server) = rt.block_on(iroh::test_utils::run_relay_server()).unwrap();
    let services = host_services();
    let (host_log, host_lines) = logger(&format!("{name} host"));
    let host = shop::spawn(ShopConfig {
        role: Role::Host,
        ports: services.ports,
        relay: relay.clone(),
        ca: Some(CaTlsConfig::insecure_skip_verify()),
        relay_only,
        key: dir.join("host.key"),
        log: host_log,
        delay: Duration::ZERO,
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
    let dir = std::env::temp_dir().join(format!("vslink_shop_nohost_{}", std::process::id()));
    let (log, _) = logger("nohost");
    let nobody = *iroh::SecretKey::generate().public().as_bytes();
    let t = Instant::now();
    let r = shop::spawn(ShopConfig {
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
        drop(p.join.take());
        wait_open(&p, (0, 0));
        drop(c);
    }
}
