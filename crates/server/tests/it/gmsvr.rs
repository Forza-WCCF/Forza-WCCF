//! `forza-server --edition 2017-18 gmsvr` end to end over TCP, with synthetic payloads (no original data).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];
const REV: u32 = 47565;

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn le(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn start(dir: &std::path::Path) -> (Server, TcpStream) {
    start_with(dir, &[])
}

fn start_with(dir: &std::path::Path, extra: &[&str]) -> (Server, TcpStream) {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let child = Command::new(crate::common::server_exe())
        .args([
            "--edition",
            "2017-18",
            "gmsvr",
            "--port",
            &port.to_string(),
            "--store",
        ])
        .arg(dir)
        .args(extra)
        .env_remove("WCCF_BIND")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let srv = Server(child);
    let t = Instant::now();
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(c) => {
                c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                return (srv, c);
            }
            Err(_) if t.elapsed() < Duration::from_secs(10) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(e) => panic!("gmsvr did not bind: {e}"),
        }
    }
}

fn ask(c: &mut TcpStream, cmd: u16, payload: &[u8]) -> (u16, Vec<u8>) {
    let mut h = vec![0u8; 40];
    h[..8].copy_from_slice(&MAGIC);
    h[8..10].copy_from_slice(&cmd.to_le_bytes());
    le(&mut h, 16, 40 + payload.len() as u32);
    h.extend_from_slice(payload);
    c.write_all(&h).unwrap();
    let mut head = [0u8; 40];
    c.read_exact(&mut head).unwrap();
    let mut body = vec![0u8; word(&head, 16) as usize - 40];
    c.read_exact(&mut body).unwrap();
    (u16::from_le_bytes([head[8], head[9]]), body)
}

fn req(size: usize, rev_at: usize) -> Vec<u8> {
    let mut b = vec![0u8; size];
    le(&mut b, 0, 1);
    le(&mut b, 4, 0x1234_5678);
    le(&mut b, rev_at, REV);
    b
}

#[test]
fn gmsvr_answers_coach_list_and_replays_a_save() {
    let dir = std::env::temp_dir().join(format!("forza_gmsvr_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (_srv, mut c) = start(&dir);

    let (rc, body) = ask(&mut c, 271, &req(20, 16));
    assert_eq!(rc, 272);
    assert_eq!(body.len(), 9292);
    assert_eq!(word(&body, 0), 1, "slot echoed");

    // a club, then SAVE_DATA_SET 277 -> SAVE_DATA 275 returns the same block byte for byte
    let mut add = req(36, 32);
    add[12] = 1;
    let (_, a) = ask(&mut c, 279, &add);
    let (coach, card) = (word(&a, 8), word(&a, 12));
    let blk: Vec<u8> = (0..768u32)
        .flat_map(|i| i.wrapping_mul(0x9E37_79B9).wrapping_add(7).to_le_bytes())
        .collect();
    let mut set = req(8572, 5940);
    le(&mut set, 8, coach);
    le(&mut set, 12, card);
    set[24] = 1;
    set[28..3100].copy_from_slice(&blk);
    set[3100..3104].copy_from_slice(&[12, 0, 0, 12]);
    set[3104..3109].copy_from_slice(b"FORZA");
    set[3160..3164].copy_from_slice(b"WCCF");
    set[3240] = 5;
    let sum = blk.chunks(4).fold(0u32, |s, w| {
        s.wrapping_add(u32::from_le_bytes(w.try_into().unwrap()))
    });
    le(&mut set, 5236, sum);
    let (rc, r) = ask(&mut c, 277, &set);
    assert_eq!((rc, word(&r, 4)), (278, 0), "SAVE_DATA_SET accepted");
    let mut get = req(32, 28);
    le(&mut get, 8, coach);
    le(&mut get, 12, card);
    let (rc, got) = ask(&mut c, 275, &get);
    assert_eq!(rc, 276);
    assert_eq!(got.len(), 6176);
    assert_eq!(&got[8..3080], &blk[..]);
    assert!(
        std::fs::read_dir(&dir).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("req_")),
        "no request dumps without --dump"
    );
}

#[test]
fn vslink_hello_turns_the_202_reflect_loopback() {
    let free_udp = || {
        std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
            .to_string()
    };
    let dir = std::env::temp_dir().join(format!("forza_gmsvr_vs_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let reflect = free_udp();
    let (_srv, mut c) = start_with(
        &dir,
        &[
            "--vs-public-ip",
            "9.9.9.9",
            "--reflect-port",
            &reflect,
            "--vslink-port",
            "20091",
        ],
    );
    let login = |c: &mut TcpStream| ask(c, 201, &req(28, 24)).1;
    let before = login(&mut c);
    assert_eq!(before[8..12], [9, 9, 9, 9]);
    assert_eq!(
        u16::from_le_bytes([before[12], before[13]]).to_string(),
        reflect
    );

    let mut hello = vec![0u8; 36];
    hello[4..].fill(0xAB);
    let (rc, r) = ask(&mut c, 900, &hello);
    assert_eq!((rc, r.len(), word(&r, 4)), (901, 8, 1));
    let after = login(&mut c);
    assert_eq!(after[8..12], [127, 0, 0, 1]);
    assert_eq!(u16::from_le_bytes([after[12], after[13]]), 20091);

    // register, then lookup while unpaired answers zeros
    let mut reg = vec![0u8; 40];
    reg[..4].copy_from_slice(&[127, 5, 6, 7]);
    reg[4..6].copy_from_slice(&20001u16.to_le_bytes());
    reg[8..].fill(0xCD);
    assert_eq!(ask(&mut c, 902, &reg).1[4], 1);
    assert_eq!(ask(&mut c, 902, &reg).1[4], 1, "same id again");
    reg[8] = 0;
    assert_eq!(ask(&mut c, 902, &reg).1[4], 0, "collision");
    let (rc, l) = ask(&mut c, 904, &reg[..6]);
    assert_eq!((rc, l.len(), word(&l, 4)), (905, 48, 0));
    assert!(l[8..].iter().all(|b| *b == 0));
}

#[test]
/// The gate's loopback run: CREATE, the guest's RESERVE, READY twice, and both runs' 243 entries pair with each other.
fn all_pairs_a_lobby_room_on_the_game_server() {
    let free = |udp: bool| {
        if udp {
            let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            s.local_addr().unwrap().port()
        } else {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        }
        .to_string()
    };
    let dir = std::env::temp_dir().join(format!("forza_all_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (desk, gm) = (free(false), free(false));
    let gm_port: u16 = gm.parse().unwrap();
    let child = Command::new(crate::common::server_exe())
        .args([
            "--edition",
            "2017-18",
            "all",
            "--desk-port",
            &desk,
            "--relay-port",
        ])
        .arg(free(true))
        .args([
            "--gmsvr-port",
            &gm,
            "--vs-public-ip",
            "9.9.9.9",
            "--reflect-port",
        ])
        .arg(free(true))
        .arg("--store")
        .arg(&dir)
        .args(["30", &dir.join("logs").to_string_lossy()])
        .env("WCCF_BIND", "127.0.0.1")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _srv = Server(child);
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    let t = Instant::now();
    let mut c = loop {
        match TcpStream::connect(("127.0.0.1", gm.parse::<u16>().unwrap())) {
            Ok(c) => break c,
            Err(_) if t.elapsed() < Duration::from_secs(10) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(e) => panic!("all did not bind the game server: {e}"),
        }
    };
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    use forza::online::lobby::{self, Created, Reserved};
    let desk: std::net::SocketAddr = format!("127.0.0.1:{desk}").parse().unwrap();
    // host makes a room, the guest reserves its seat 2, both press READY
    let Ok(Created::Room {
        id,
        code,
        ticket: host,
        seat: 1,
    }) = lobby::create(desk, false)
    else {
        panic!("no room")
    };
    let Ok(Reserved::Ticket {
        ticket: guest,
        seat: 2,
    }) = lobby::reserve(desk, &id)
    else {
        panic!("no guest seat")
    };
    assert_eq!(lobby::ready(desk, &id, &host), Ok(true));
    assert_eq!(lobby::ready(desk, &id, &guest), Ok(true));
    // three PCs' runs on the game server: a stranger in the open pool first, then the room's host and guest. Each
    // vslink says hello (900, the room ticket at +36), registers its token (902), then its control sends the 243.
    let pc = |c: &mut TcpStream, n: u8, ticket: Option<&str>| {
        let mut gm = TcpStream::connect(("127.0.0.1", gm_port)).unwrap();
        gm.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (rc, r) = ask(
            &mut gm,
            900,
            &forza::online::vs::hello_payload(&[n; 32], ticket),
        );
        assert_eq!((rc, word(&r, 4)), (901, 1));
        let mut reg = vec![0u8; 40];
        reg[..4].copy_from_slice(&[127, n, n, n]);
        reg[4..6].copy_from_slice(&(20000 + u16::from(n)).to_le_bytes());
        reg[8..].fill(n);
        assert_eq!(ask(&mut gm, 902, &reg).1[4], 1);
        let mut entry = vec![0u8; 280];
        le(&mut entry, 0, u32::from(n));
        le(&mut entry, 4, u32::from(n)); // other rounds: a room pairs across them
        entry[8..14].copy_from_slice(&reg[..6]);
        le(&mut entry, 44, 900_000_000 + u32::from(n));
        assert_eq!(ask(c, 243, &entry).0, 244);
    };
    let poll = |c: &mut TcpStream, n: u8| {
        let mut p = vec![0u8; 20];
        le(&mut p, 0, u32::from(n));
        le(&mut p, 8, 900_000_000 + u32::from(n));
        let (rc, r) = ask(c, 245, &p);
        assert_eq!(rc, 246);
        word(&r, 12)
    };
    pc(&mut c, 1, None);
    pc(&mut c, 2, Some(&host));
    pc(&mut c, 3, Some(&guest));
    assert_eq!(poll(&mut c, 3), 900_000_002, "the guest gets the host");
    assert_eq!(poll(&mut c, 2), 900_000_003, "the host gets the guest");
    assert_eq!(poll(&mut c, 1), 0, "the stranger still waits");
    let json = {
        let mut raw = TcpStream::connect(desk).unwrap();
        raw.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        raw.write_all(b"LOBBY 2017-18\n").unwrap();
        let mut json = String::new();
        raw.read_to_string(&mut json).unwrap();
        json
    };
    // the room as the lobby lists it (one LOBBY per address every 2 s, so only here): random ids, no address,
    // card number or ticket
    let l = lobby::Lobby::parse(&json).unwrap();
    let room = l.vs.iter().find(|r| r.id == id).unwrap();
    assert_eq!((room.code.as_str(), room.players), (code.as_str(), 2));
    assert!(room.occupants.iter().all(|o| o.ready), "{json}");
    assert!(
        !json.contains("127.") && !json.contains("900000") && !json.contains(&host),
        "{json}"
    );
}
