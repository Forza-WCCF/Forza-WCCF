//! Two vslinks on one host (docs/ARCHITECTURE.md "Online"): an in-process gmsvr and iroh test relay, two fake
//! client + control pairs on different seats. Probe, 243, 245 polls, 246, then the peer traffic the game sends:
//! hello/ack, a 3.2 KB team block in 16-byte chunks, relay-mode packets through the reflect address and 1,000 pings.
//! Every datagram a fake client receives must come from its reflect address or the opponent's token.

use forza::online::{gamesvr, vs::VsConfig};
use forza::service::Log;
use iroh::tls::CaTlsConfig;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];
const AIME: u32 = 582_349_731;
const REV: u32 = 47565;

fn le(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// One framed request to gmsvr; the answer's payload.
fn ask(c: &mut TcpStream, cmd: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = gamesvr::header(cmd, 0, 0, payload.len(), 0).to_vec();
    f.extend_from_slice(payload);
    c.write_all(&f).unwrap();
    let mut h = [0u8; 40];
    c.read_exact(&mut h).unwrap();
    assert_eq!(u16::from_le_bytes([h[8], h[9]]), cmd + 1);
    let mut body = vec![0u8; word(&h, 16) as usize - 40];
    c.read_exact(&mut body).unwrap();
    body
}

/// A game datagram: magic, type at +8, `extra` from +52.
fn pkt(typ: u16, extra: &[u8]) -> Vec<u8> {
    let mut p = vec![0u8; 52];
    p[..8].copy_from_slice(&MAGIC);
    p[8..10].copy_from_slice(&typ.to_le_bytes());
    p.extend_from_slice(extra);
    p
}

/// A fake cabinet PC: its vslink, the client's peer socket on 127.0.0.1:20000+seat and the control's gmsvr link.
struct Pc {
    link: vslink::Link,
    seat: u8,
    udp: UdpSocket,
    ctl: TcpStream,
    lines: Arc<Mutex<Vec<String>>>,
    token: SocketAddrV4,
    /// The opponent's token, from 246
    opp: SocketAddrV4,
}

impl Pc {
    fn new(
        name: &str,
        seat: u8,
        gm: SocketAddr,
        relay: &iroh::RelayMap,
        relay_only: bool,
        dir: &Path,
    ) -> Self {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let (l, tag) = (lines.clone(), name.to_owned());
        let log: Log = Arc::new(move |s| {
            eprintln!("[{tag}] {s}");
            l.lock().unwrap().push(s.to_owned());
        });
        let link = vslink::spawn(vslink::Config {
            gmsvr: gm,
            reflect: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
            relay: relay.clone(),
            ca: Some(CaTlsConfig::insecure_skip_verify()),
            relay_only,
            key: dir.join(format!("{name}.key")),
            log,
            ticket: None,
            dmath: None,
            on_dmath_differs: None,
        })
        .unwrap();
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 20000 + u16::from(seat))).unwrap();
        udp.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let ctl = TcpStream::connect(gm).unwrap();
        ctl.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let none = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
        Pc {
            link,
            seat,
            udp,
            ctl,
            lines,
            token: none,
            opp: none,
        }
    }

    /// The next datagram, checked to come from the reflect address or the opponent's token.
    fn recv(&self) -> Option<(Vec<u8>, SocketAddrV4)> {
        let mut b = [0u8; 2048];
        let (n, src) = self.udp.recv_from(&mut b).ok()?;
        let SocketAddr::V4(src) = src else {
            panic!("v6 source {src}")
        };
        assert!(
            src == self.link.reflect || src == self.opp,
            "seat {}: datagram from {src}, expected {} or {}",
            self.seat,
            self.link.reflect,
            self.opp
        );
        Some((b[..n].to_vec(), src))
    }

    /// Waits for a datagram of type `typ` (others are not expected).
    fn expect(&self, typ: u16, within: Duration) -> Option<(Vec<u8>, SocketAddrV4)> {
        let end = Instant::now() + within;
        while Instant::now() < end {
            if let Some((p, src)) = self.recv() {
                assert_eq!(u16::from_le_bytes([p[8], p[9]]), typ, "unexpected type");
                return Some((p, src));
            }
        }
        None
    }

    fn logged(&self, what: &str) -> bool {
        self.lines.lock().unwrap().iter().any(|l| l.contains(what))
    }

    /// Login (202 must name the loopback reflect), then probe until the echo carries the token.
    fn login_and_probe(&mut self) {
        let mut login = vec![0u8; 28];
        le(&mut login, 0, 1);
        le(&mut login, 4, AIME);
        le(&mut login, 24, REV);
        let r = ask(&mut self.ctl, 201, &login);
        assert_eq!(r[8..12], [127, 0, 0, 1], "loopback reflect after the hello");
        assert_eq!(u16::from_le_bytes([r[12], r[13]]), 20090);
        let mut probe = pkt(1, &[0; 6]);
        probe[16] = 58;
        for _ in 0..10 {
            self.udp.send_to(&probe, self.link.reflect).unwrap();
            if let Some((e, src)) = self.recv() {
                assert_eq!(
                    (src, e.len(), e[..10] == probe[..10]),
                    (self.link.reflect, 58, true)
                );
                let ip = Ipv4Addr::new(e[52], e[53], e[54], e[55]);
                self.token = SocketAddrV4::new(ip, u16::from_le_bytes([e[56], e[57]]));
                assert!(ip.is_loopback() && ip != Ipv4Addr::LOCALHOST, "token {ip}");
                assert_eq!(self.token.port(), 20000 + u16::from(self.seat));
                return;
            }
        }
        panic!("seat {}: no probe echo", self.seat);
    }

    fn entry(&self, round: u32, card: u32, shop: u32) -> Vec<u8> {
        let mut e = vec![0u8; 280];
        le(&mut e, 0, 1);
        le(&mut e, 4, round);
        e[8..12].copy_from_slice(&self.token.ip().octets());
        e[12..14].copy_from_slice(&self.token.port().to_le_bytes());
        le(&mut e, 44, card);
        le(&mut e, 48, shop);
        e
    }

    fn poll(&mut self, round: u32, card: u32, shop: u32) -> Vec<u8> {
        let mut q = vec![0u8; 20];
        le(&mut q, 0, 1);
        le(&mut q, 4, round);
        le(&mut q, 8, card);
        le(&mut q, 12, shop);
        ask(&mut self.ctl, 245, &q)
    }
}

fn token_in(m: &[u8]) -> SocketAddrV4 {
    SocketAddrV4::new(
        Ipv4Addr::new(m[16], m[17], m[18], m[19]),
        u16::from_le_bytes([m[120], m[121]]),
    )
}

fn run(name: &str, seats: (u8, u8), relay_only: bool) {
    let started = Instant::now();
    let dir = std::env::temp_dir().join(format!("vslink_loopback_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (relay, _url, _server) = rt.block_on(iroh::test_utils::run_relay_server()).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let gm = l.local_addr().unwrap();
    let opts = gamesvr::Options {
        vs: Some(VsConfig {
            public_ip: Ipv4Addr::new(9, 9, 9, 9),
            reflect_port: 1,
            vslink_port: 20090,
        }),
        dump: false,
        ..Default::default()
    };
    let _gmsvr = gamesvr::spawn_opts(l, dir.join("gmsvr"), Arc::new(|_| {}), opts).unwrap();

    let mut a = Pc::new("a", seats.0, gm, &relay, relay_only, &dir);
    let mut b = Pc::new("b", seats.1, gm, &relay, relay_only, &dir);
    assert_ne!(a.link.id, b.link.id);
    a.login_and_probe();
    b.login_and_probe();
    assert_ne!(a.token, b.token);

    // 223 round, 243 entries with the tokens, 245 until 246
    let round = word(
        &ask(&mut a.ctl, 223, &{
            let mut r = vec![0u8; 20];
            le(&mut r, 0, 1);
            r
        }),
        4,
    );
    let ea = a.entry(round, 111, 5);
    assert_eq!(word(&ask(&mut a.ctl, 243, &ea), 4), 0);
    assert_eq!(a.poll(round, 111, 5)[122], 0, "alone: keep waiting");
    let eb = b.entry(round, 222, 6);
    ask(&mut b.ctl, 243, &eb);
    let (ma, mb) = (a.poll(round, 111, 5), b.poll(round, 222, 6));
    assert_eq!((ma[122], mb[122]), (1, 1), "paired");
    assert_eq!(word(&ma, 116), word(&mb, 116), "equal seeds");
    assert_eq!(
        (token_in(&ma), token_in(&mb)),
        (b.token, a.token),
        "mirrored tokens"
    );
    a.opp = token_in(&ma);
    b.opp = token_in(&mb);

    // hello until it crosses (vslink pairs at its next lookup, then dials), then the ack
    let t = Instant::now();
    let hello = loop {
        assert!(t.elapsed() < Duration::from_secs(20), "no hello across");
        a.udp.send_to(&pkt(2, b"hello"), a.opp).unwrap();
        if let Some(h) = b.expect(2, Duration::from_millis(200)) {
            break h;
        }
    };
    assert_eq!(
        (hello.0[52..].to_vec(), hello.1),
        (b"hello".to_vec(), b.opp),
        "from A's token"
    );
    let link_up = t.elapsed();
    // drop extra hellos still in flight
    while b.recv().is_some() {}
    b.udp.send_to(&pkt(3, b"ack"), b.opp).unwrap();
    let ack = a.expect(3, Duration::from_secs(2)).expect("ack");
    assert_eq!(ack.1, a.opp);

    // the team block: 3,200 bytes in 16-byte chunks, one datagram each
    for i in 0..200u32 {
        let mut chunk = [0u8; 16];
        chunk[..4].copy_from_slice(&i.to_le_bytes());
        a.udp.send_to(&pkt(10, &chunk), a.opp).unwrap();
        std::thread::sleep(Duration::from_micros(500));
    }
    let mut got = BTreeSet::new();
    while let Some((p, src)) = b.expect(10, Duration::from_secs(1)) {
        assert_eq!((src, p.len()), (b.opp, 68));
        got.insert(word(&p, 52));
        if got.len() == 200 {
            break;
        }
    }
    assert_eq!(got, (0..200).collect(), "every chunk once");

    // relay mode: to the reflect address with the destination in the header; arrives from the far reflect address
    for i in 0..5u8 {
        let mut p = pkt(10, &[i; 16]);
        p[44..48].copy_from_slice(&a.opp.ip().octets());
        p[50..52].copy_from_slice(&a.opp.port().to_le_bytes());
        a.udp.send_to(&p, a.link.reflect).unwrap();
        let (r, src) = b
            .expect(10, Duration::from_secs(2))
            .expect("relayed packet");
        assert_eq!(
            (src, &r),
            (b.link.reflect, &p),
            "unchanged, from B's reflect address"
        );
    }

    // 1,000 pings, lock-step
    let t = Instant::now();
    for i in 0..1000u32 {
        a.udp.send_to(&pkt(4, &i.to_le_bytes()), a.opp).unwrap();
        let (p, _) = b
            .expect(4, Duration::from_secs(2))
            .unwrap_or_else(|| panic!("ping {i} lost"));
        assert_eq!(word(&p, 52), i);
        b.udp.send_to(&pkt(5, &i.to_le_bytes()), b.opp).unwrap();
        let (p, _) = a
            .expect(5, Duration::from_secs(2))
            .unwrap_or_else(|| panic!("pong {i} lost"));
        assert_eq!(word(&p, 52), i);
    }
    let pings = t.elapsed();

    // the path log: forced relay says relay; otherwise iroh moves to a direct path
    let want = if relay_only {
        "path relay"
    } else {
        "path direct"
    };
    // keep pinging, as the game does in a match: only the dialling side opens the direct path (iroh), the accepting
    // side follows from path events, and once on CI it stayed on the relay for over 15 s on the quiet link. Datagrams
    // lost during the move are not checked.
    let t = Instant::now();
    let mut i = 1000u32;
    while !(a.logged(want) && b.logged(want)) {
        assert!(
            t.elapsed() < Duration::from_secs(15),
            "no '{want}' in both logs"
        );
        a.udp.send_to(&pkt(4, &i.to_le_bytes()), a.opp).unwrap();
        if b.expect(4, Duration::from_millis(500)).is_some() {
            b.udp.send_to(&pkt(5, &i.to_le_bytes()), b.opp).unwrap();
            a.expect(5, Duration::from_millis(500));
        }
        i += 1;
        std::thread::sleep(Duration::from_millis(100));
    }
    let path = t.elapsed();
    if relay_only {
        assert!(!a.logged("path direct") && !b.logged("path direct"));
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        a.recv().is_none() && b.recv().is_none(),
        "nothing left over"
    );
    eprintln!(
        "{name}: link up after {link_up:?}, 1,000 pings in {pings:?}, '{want}' after {path:?}, total {:?}",
        started.elapsed()
    );
    drop((a, b));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn direct() {
    run("direct", (1, 2), false);
}

#[test]
fn relay_forced() {
    run("relay", (3, 4), true);
}
