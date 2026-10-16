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
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let child = Command::new(env!("CARGO_BIN_EXE_forza-server"))
        .args([
            "--edition",
            "2017-18",
            "gmsvr",
            "--port",
            &port.to_string(),
            "--store",
        ])
        .arg(dir)
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
