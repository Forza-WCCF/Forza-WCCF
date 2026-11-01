//! The Docker NAT harness's two programs (`crates/server/tests/nat/README.md`), not shipped:
//!
//! - `nat_peer relay [--https ADDR] [--quic ADDR]`: an iroh relay with a self-signed certificate (the test relay,
//!   bound where the harness's homes reach it), so no run touches the public relay.
//! - `nat_peer client --server IP --relay URL --relay-quic PORT --role host|guest|solo|open --shared DIR`: one home's
//!   Launcher and game on the network. Lobby (`CREATE vs` / `RESERVE`, `READY`, `HEARTBEAT`), vslink with the room
//!   ticket, the control's 201/202, the reflect probe, 223, 243 and 245 polls until 246, then the client's peer link
//!   through vslink: hello/ack, a 3,232-byte team block in 16-byte DIVIDE chunks each way, 20 pings. One JSON line
//!   on stdout; the harness compares the two homes' lines (each 246 must name the other's token).
//!
//! Roles: `host` creates a room and writes its id to `DIR/room`, `guest` reserves that room, `solo` creates a room of
//! its own (the cross-room negative), `open` takes no room (M3's open pool).

use forza::online::gamesvr;
use forza::online::lobby::{self, Created, Reserved};
use iroh::tls::CaTlsConfig;
use iroh::{RelayConfig, RelayMap, RelayUrl};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];
const REV: u32 = 47565;
const CHUNKS: u32 = 202; // 3,232 bytes: the ~3,230-byte DIVIDE HEADER/DATA team block
const PINGS: u32 = 20;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut flags = HashMap::new();
    for kv in args[1..].chunks(2) {
        flags.insert(
            kv[0].trim_start_matches("--").to_owned(),
            kv.get(1).cloned().unwrap_or_default(),
        );
    }
    let f = |k: &str, d: &str| flags.get(k).cloned().unwrap_or_else(|| d.to_owned());
    match args.first().map(String::as_str) {
        Some("relay") => relay(&f("https", "0.0.0.0:3340"), &f("quic", "0.0.0.0:7842")),
        Some("client") => client(&f),
        _ => {
            eprintln!("usage: nat_peer relay|client [--flag value]...");
            std::process::exit(2)
        }
    }
}

fn relay(https: &str, quic: &str) {
    use iroh_relay::server::{
        CertConfig, QuicConfig, RelayConfig, Server, ServerConfig, TlsConfig,
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let (_, tls) = iroh_relay::server::testing::self_signed_tls_certs_and_config();
        // plain HTTP (captive-portal checks) on the next port; the relay itself is HTTPS
        let mut http: SocketAddr = https.parse().unwrap();
        http.set_port(http.port() + 1);
        let mut r = RelayConfig::new(http);
        r.tls = Some(TlsConfig::new(
            https.parse::<SocketAddr>().unwrap(),
            CertConfig::Manual { server_config: tls },
        ));
        r.key_cache_capacity = Some(1024);
        let mut c = ServerConfig::default();
        c.relay = Some(r);
        c.quic = Some(QuicConfig::new(quic.parse::<SocketAddr>().unwrap()));
        let s = Server::spawn(c).await.unwrap();
        println!("relay https {:?} quic {:?}", s.https_addr(), s.quic_addr());
        std::future::pending::<()>().await;
    });
}

fn le(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

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

fn pkt(typ: u16, extra: &[u8]) -> Vec<u8> {
    let mut p = vec![0u8; 52];
    p[..8].copy_from_slice(&MAGIC);
    p[8..10].copy_from_slice(&typ.to_le_bytes());
    p.extend_from_slice(extra);
    p
}

/// The 16 bytes of chunk `i` of the team block a PC with token `t` sends: the receiver rebuilds them from its 246.
fn chunk(t: SocketAddrV4, i: u32) -> [u8; 16] {
    let s = u32::from_le_bytes(t.ip().octets()) ^ u32::from(t.port());
    std::array::from_fn(|k| {
        (s.wrapping_add(i * 16 + k as u32)
            .wrapping_mul(2_654_435_761)
            >> 24) as u8
    })
}

fn rnd() -> u32 {
    getrandom::u32().unwrap() | 1
}

fn client(f: &dyn Fn(&str, &str) -> String) {
    let start = Instant::now();
    let ms = |t: Instant| t.duration_since(start).as_millis();
    let server: Ipv4Addr = f("server", "10.99.0.10").parse().unwrap();
    let desk = SocketAddr::from((server, f("desk-port", "20030").parse::<u16>().unwrap()));
    let gm = SocketAddr::from((server, f("gmsvr-port", "20060").parse::<u16>().unwrap()));
    let role = f("role", "open");
    let shared = PathBuf::from(f("shared", "/shared"));
    let wait = Duration::from_secs(f("pair-secs", "45").parse().unwrap());
    let seat: u8 = f("seat", "1").parse().unwrap();
    let mut out: BTreeMap<&str, String> = BTreeMap::new();
    let q = |s: &str| format!("\"{s}\"");
    out.insert("role", q(&role));

    // the lobby, as the Launcher's ONLINE page does it
    let room_file = shared.join("room");
    let (room, ticket) = match role.as_str() {
        "host" | "solo" => match lobby::create(desk, false).unwrap() {
            Created::Room {
                id,
                ticket,
                seat: 1,
                ..
            } => {
                if role == "host" {
                    std::fs::write(&room_file, &id).unwrap();
                }
                (Some(id), Some(ticket))
            }
            c => panic!("CREATE vs: {c:?}"),
        },
        "guest" => {
            let t = Instant::now();
            let id = loop {
                if let Ok(s) = std::fs::read_to_string(&room_file) {
                    break s;
                }
                assert!(
                    t.elapsed() < Duration::from_secs(30),
                    "no room from the host"
                );
                std::thread::sleep(Duration::from_millis(100));
            };
            match lobby::reserve(desk, &id).unwrap() {
                Reserved::Ticket { ticket, seat: 2 } => (Some(id), Some(ticket)),
                r => panic!("RESERVE: {r:?}"),
            }
        }
        _ => (None, None),
    };
    if let (Some(id), Some(t)) = (&room, &ticket) {
        assert_eq!(lobby::ready(desk, id, t), Ok(true), "READY");
        assert_eq!(lobby::heartbeat(desk, t), Ok(true), "HEARTBEAT");
        // every 20 s for as long as the run lasts, as the Launcher's beat
        let t = t.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(20));
                let _ = lobby::heartbeat(desk, &t);
            }
        });
    }
    out.insert("lobby", q(if room.is_some() { "room" } else { "open" }));

    // vslink, as the run starts it
    let lines: Arc<Mutex<Vec<String>>> = Arc::default();
    let l = lines.clone();
    let map: RelayMap = RelayConfig::new(
        f("relay", "https://10.99.0.10:3340")
            .parse::<RelayUrl>()
            .unwrap(),
        Some(iroh_relay::RelayQuicConfig::new(
            f("relay-quic", "7842").parse().unwrap(),
        )),
    )
    .into();
    let link = vslink::spawn(vslink::Config {
        gmsvr: gm,
        reflect: SocketAddrV4::new(Ipv4Addr::LOCALHOST, vslink::net::REFLECT_PORT),
        relay: map,
        ca: Some(CaTlsConfig::insecure_skip_verify()),
        relay_only: f("relay-only", "0") == "1",
        key: std::env::temp_dir().join(format!("nat_peer_{}.key", std::process::id())),
        log: Arc::new(move |s| {
            eprintln!("[vslink] {s}");
            l.lock().unwrap().push(s.to_owned());
        }),
        ticket: ticket.clone(),
    })
    .unwrap();

    // the control: login, then the client's reflect probe to the address 202 names
    let mut ctl = TcpStream::connect(gm).unwrap();
    ctl.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut login = vec![0u8; 28];
    le(&mut login, 0, 1);
    le(&mut login, 4, rnd() >> 1);
    le(&mut login, 24, REV);
    let r = ask(&mut ctl, 201, &login);
    let reflect = SocketAddrV4::new(
        Ipv4Addr::new(r[8], r[9], r[10], r[11]),
        u16::from_le_bytes([r[12], r[13]]),
    );
    assert_eq!(
        reflect,
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, vslink::net::REFLECT_PORT),
        "202 after the hello names the loopback reflect"
    );
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 20000 + u16::from(seat))).unwrap();
    udp.set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    let mut buf = [0u8; 2048];
    let mut probe = pkt(1, &[0; 6]);
    probe[16] = 58;
    let mut token = None;
    for _ in 0..50 {
        udp.send_to(&probe, reflect).unwrap();
        if let Ok((n, _)) = udp.recv_from(&mut buf)
            && n == 58
        {
            let e = &buf[..n];
            token = Some(SocketAddrV4::new(
                Ipv4Addr::new(e[52], e[53], e[54], e[55]),
                u16::from_le_bytes([e[56], e[57]]),
            ));
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let token = token.expect("no probe echo");
    out.insert("token", q(&token.to_string()));

    // 223, 243, 245 until 246
    let round = |ctl: &mut TcpStream| {
        let mut r = vec![0u8; 20];
        le(&mut r, 0, 1);
        word(&ask(ctl, 223, &r), 4)
    };
    let (card, arcade) = (rnd() >> 1, rnd() >> 1);
    let enter = |ctl: &mut TcpStream, round: u32| {
        let mut e = vec![0u8; 280];
        le(&mut e, 0, 1);
        le(&mut e, 4, round);
        e[8..12].copy_from_slice(&token.ip().octets());
        e[12..14].copy_from_slice(&token.port().to_le_bytes());
        le(&mut e, 44, card);
        le(&mut e, 48, arcade);
        ask(ctl, 243, &e);
    };
    let mut rd = round(&mut ctl);
    enter(&mut ctl, rd);
    let entered = Instant::now();
    let mut opp = None;
    while entered.elapsed() < wait {
        let mut p = vec![0u8; 20];
        le(&mut p, 0, 1);
        le(&mut p, 4, rd);
        le(&mut p, 8, card);
        le(&mut p, 12, arcade);
        let m = ask(&mut ctl, 245, &p);
        if m.len() > 122 && m[122] == 1 {
            opp = Some((
                SocketAddrV4::new(
                    Ipv4Addr::new(m[16], m[17], m[18], m[19]),
                    u16::from_le_bytes([m[120], m[121]]),
                ),
                word(&m, 116),
                word(&m, 20) == card,
            ));
            break;
        }
        // the open pool pairs within a round: a new window means a new entry, as the control's next ENTRY
        if room.is_none() {
            let now = round(&mut ctl);
            if now != rd {
                rd = now;
                enter(&mut ctl, rd);
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    out.insert("paired", (opp.is_some()).to_string());
    let Some((opp, seed, home)) = opp else {
        finish(&mut out, &lines, desk, &room, &ticket, &role);
        return;
    };
    out.insert("pair_ms", ms(Instant::now()).to_string());
    out.insert("opp", q(&opp.to_string()));
    out.insert("seed", seed.to_string());
    out.insert("home", home.to_string());

    // the peer link: hello/ack, the team block each way, pings; every datagram to and from the opponent's token
    let deadline = Instant::now() + Duration::from_secs(f("link-secs", "40").parse().unwrap());
    let (mut up, mut sent, mut peer_done) = (None, false, false);
    let mut got = vec![false; CHUNKS as usize];
    let mut bad = 0u32;
    let (mut pinged, mut ping_at, mut rtts) = (0u32, HashMap::new(), Vec::new());
    let mut tick = Instant::now() - Duration::from_secs(1);
    let mut last_ping = Instant::now();
    let complete = |got: &[bool]| got.iter().all(|g| *g);
    while Instant::now() < deadline {
        if tick.elapsed() >= Duration::from_millis(200) {
            tick = Instant::now();
            if up.is_none() {
                udp.send_to(&pkt(2, b"hello"), opp).unwrap();
            }
            if complete(&got) {
                udp.send_to(&pkt(9, b"block ok"), opp).unwrap();
            }
            if sent && pinged < PINGS {
                last_ping = Instant::now();
                ping_at.insert(pinged, last_ping);
                udp.send_to(&pkt(4, &pinged.to_le_bytes()), opp).unwrap();
                pinged += 1;
            }
        }
        if up.is_some() && !sent {
            for i in 0..CHUNKS {
                let mut c = i.to_le_bytes().to_vec();
                c.extend_from_slice(&chunk(token, i));
                udp.send_to(&pkt(10, &c), opp).unwrap();
                std::thread::sleep(Duration::from_millis(2));
            }
            sent = true;
        }
        if complete(&got)
            && peer_done
            && pinged == PINGS
            && (ping_at.is_empty() || last_ping.elapsed() >= Duration::from_secs(1))
        {
            break;
        }
        let Ok((n, src)) = udp.recv_from(&mut buf) else {
            continue;
        };
        if src != SocketAddr::V4(opp) || n < 52 || buf[..8] != MAGIC {
            bad += 1;
            continue;
        }
        let p = &buf[..n];
        match u16::from_le_bytes([p[8], p[9]]) {
            2 => {
                udp.send_to(&pkt(3, b"ack"), opp).unwrap();
                up.get_or_insert(Instant::now());
            }
            3 => {
                up.get_or_insert(Instant::now());
            }
            10 if n == 52 + 20 => {
                let i = word(p, 52);
                if i < CHUNKS && p[56..72] == chunk(opp, i) {
                    got[i as usize] = true;
                } else {
                    bad += 1;
                }
            }
            9 => peer_done = true,
            4 => {
                udp.send_to(&pkt(5, &p[52..]), opp).unwrap();
            }
            5 if n >= 56 => {
                if let Some(t) = ping_at.remove(&word(p, 52)) {
                    rtts.push(t.elapsed().as_secs_f64() * 1e3);
                }
            }
            _ => {}
        }
    }
    // a little longer for the opponent's last chunks, acks and pongs
    let linger = Instant::now();
    while linger.elapsed() < Duration::from_secs(1) {
        if let Ok((n, src)) = udp.recv_from(&mut buf)
            && src == SocketAddr::V4(opp)
            && n >= 52
        {
            match u16::from_le_bytes([buf[8], buf[9]]) {
                2 => drop(udp.send_to(&pkt(3, b"ack"), opp)),
                4 => drop(udp.send_to(&pkt(5, &buf[52..n]), opp)),
                _ => {}
            }
        }
        if complete(&got) {
            let _ = udp.send_to(&pkt(9, b"block ok"), opp);
        }
    }
    out.insert("link_up", up.is_some().to_string());
    if let Some(t) = up {
        out.insert("link_ms", ms(t).to_string());
    }
    out.insert(
        "block_rx",
        format!("{}", got.iter().filter(|g| **g).count() * 16),
    );
    out.insert("block_rx_ok", complete(&got).to_string());
    out.insert("block_tx_acked", peer_done.to_string());
    out.insert("bad_datagrams", bad.to_string());
    rtts.sort_by(f64::total_cmp);
    if let Some(m) = rtts.get(rtts.len() / 2) {
        out.insert("ping_median_ms", format!("{m:.1}"));
    }
    out.insert("pongs", rtts.len().to_string());
    // iroh moves a relayed link to a direct path once the hole punch works: give it a moment
    let t = Instant::now();
    let path = |lines: &Mutex<Vec<String>>| {
        lines
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|l| {
                l.split_once(": path ")
                    .map(|(_, r)| r.split(',').next().unwrap_or("").to_owned())
            })
            .unwrap_or_else(|| "none".into())
    };
    while path(&lines) != "direct" && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(200));
    }
    out.insert("path", q(&path(&lines)));
    drop(link);
    finish(&mut out, &lines, desk, &room, &ticket, &role);
}

/// Leaves the lobby as the Launcher does (CLOSE for the host, RELEASE for a guest) and prints the JSON line.
fn finish(
    out: &mut BTreeMap<&str, String>,
    lines: &Mutex<Vec<String>>,
    desk: SocketAddr,
    room: &Option<String>,
    ticket: &Option<String>,
    role: &str,
) {
    let ever_direct = lines
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.contains("path direct"));
    out.insert("ever_direct", ever_direct.to_string());
    if let (Some(id), Some(t)) = (room, ticket) {
        let left = if role == "guest" {
            lobby::release(desk, t)
        } else {
            lobby::close(desk, id, t)
        };
        out.insert("left_lobby", (left == Ok(true)).to_string());
    }
    let body: Vec<String> = out.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
    println!("{{{}}}", body.join(","));
}
