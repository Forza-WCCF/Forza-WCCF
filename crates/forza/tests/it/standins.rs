//! Replays the recordings of the Python stand-ins (`tests/rec/*.jsonl`, made by the throwaway recorders of step 10
//! piece J1 from synthetic requests) against the Rust services and compares byte for byte.

use forza::online::gamesvr;
use forza::standins::{Handle, Log, aimedb};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

fn records(name: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/rec")
        .join(format!("{name}.jsonl"));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn quiet() -> Log {
    Arc::new(|_| {})
}

fn scratch(what: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("forza_test_{what}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Sends `req` (in two writes at `split` when non-zero), then reads until the service is quiet for 400 ms.
fn exchange(c: &mut TcpStream, req: &[u8], split: usize) -> Vec<u8> {
    if split > 0 {
        c.write_all(&req[..split]).unwrap();
        sleep(Duration::from_millis(150));
        c.write_all(&req[split..]).unwrap();
    } else {
        c.write_all(req).unwrap();
    }
    sleep(Duration::from_millis(50));
    let mut out = Vec::new();
    let mut buf = vec![0u8; 65536];
    loop {
        match c.read(&mut buf) {
            Ok(0) | Err(_) => return out,
            Ok(n) => out.extend_from_slice(&buf[..n]),
        }
    }
}

fn connect(l: &TcpListener) -> TcpStream {
    let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
    c.set_read_timeout(Some(Duration::from_millis(400)))
        .unwrap();
    c
}

#[test]
fn gamesvr_replay() {
    let dir = scratch("gamesvr");
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let svc: Handle = gamesvr::spawn(l.try_clone().unwrap(), dir.join("save"), quiet()).unwrap();
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_millis(400)))
        .unwrap();
    let recs = records("gamesvr");
    for (i, r) in recs.iter().enumerate() {
        let resp = exchange(
            &mut c,
            &unhex(r["req"].as_str().unwrap()),
            r["split"].as_u64().unwrap() as usize,
        );
        assert_eq!(
            resp.len() as u64,
            r["resp_len"].as_u64().unwrap(),
            "record {i}: length"
        );
        assert_eq!(
            hex(&Sha256::digest(&resp)),
            r["resp_sha256"].as_str().unwrap(),
            "record {i}: sha256"
        );
        if let Some(h) = r["resp"].as_str() {
            assert_eq!(hex(&resp), h, "record {i}");
        }
    }
    assert!(
        std::fs::read_dir(dir.join("save")).unwrap().count() > 0,
        "request payloads are kept"
    );
    drop(svc);
    let _ = std::fs::remove_dir_all(dir);
}

// The club store (docs/research/ONLINE-2017-18.md, "M1"): synthetic requests at the documented offsets.
const AIME: u32 = 582_349_731;
const REV: u32 = 47565;

fn le(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn sum(b: &[u8]) -> u32 {
    b.chunks(4).fold(0u32, |s, w| {
        s.wrapping_add(u32::from_le_bytes(w.try_into().unwrap()))
    })
}

/// A request of `size` bytes with slot 1, the Aime id at +4 and the revision at `rev_at`; `set` fills the rest.
fn req(size: usize, rev_at: usize, set: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let mut b = vec![0u8; size];
    le(&mut b, 0, 1);
    le(&mut b, 4, AIME);
    le(&mut b, rev_at, REV);
    set(&mut b);
    b
}

/// Sends one framed request and returns the answer's payload, checking the answer's command is `cmd + 1`.
fn ask(c: &mut TcpStream, cmd: u16, payload: &[u8]) -> Vec<u8> {
    let mut h = vec![0u8; 40];
    h[..8].copy_from_slice(&[0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C]);
    h[8..10].copy_from_slice(&cmd.to_le_bytes());
    le(&mut h, 16, 40 + payload.len() as u32);
    h.extend_from_slice(payload);
    c.write_all(&h).unwrap();
    let mut head = [0u8; 40];
    c.read_exact(&mut head).unwrap();
    assert_eq!(u16::from_le_bytes([head[8], head[9]]), cmd + 1);
    let mut body = vec![0u8; word(&head, 16) as usize - 40];
    c.read_exact(&mut body).unwrap();
    assert_eq!(word(&body, 0), 1, "cmd {cmd}: slot echoed");
    body
}

fn coach_list(c: &mut TcpStream) -> Vec<u8> {
    ask(c, 271, &req(20, 16, |_| {}))
}

/// COACH_LIST_ADD kind 1: (coachId, cardId).
fn new_club(c: &mut TcpStream) -> (u32, u32) {
    let a = ask(c, 279, &req(36, 32, |b| b[12] = 1));
    assert_eq!(word(&a, 4), 0, "COACH_LIST_ADD errCode");
    (word(&a, 8), word(&a, 12))
}

/// A synthetic save block: word i = i * 0x9E37_79B9 (non-zero, wraps).
fn block() -> Vec<u8> {
    (0..768u32)
        .flat_map(|i| i.wrapping_mul(0x9E37_79B9).wrapping_add(7).to_le_bytes())
        .collect()
}

/// SAVE_DATA_SET of `blk` for a club, with the client's block sum at +5,236 (`bad` spoils it).
fn save_set(coach: u32, card: u32, kind: u8, blk: &[u8], bad: bool) -> Vec<u8> {
    req(8572, 5940, |b| {
        le(b, 8, coach);
        le(b, 12, card);
        b[24] = kind;
        b[28..3100].copy_from_slice(blk);
        b[3100..3104].copy_from_slice(&[12, 0, 0, 12]); // version 12, size 3,072
        b[3104..3109].copy_from_slice(b"FORZA");
        b[3160..3164].copy_from_slice(b"WCCF");
        b[3240] = 5;
        le(b, 5236, sum(blk).wrapping_add(u32::from(bad)));
    })
}

fn save_data(c: &mut TcpStream, coach: u32, card: u32) -> Vec<u8> {
    ask(
        c,
        275,
        &req(32, 28, |b| {
            le(b, 8, coach);
            le(b, 12, card);
        }),
    )
}

/// A SAVE_DATA answer holds `blk` and passes the client's two checks (sub_780DC0).
fn assert_loads(a: &[u8], blk: &[u8]) {
    assert_eq!(a.len(), 6176);
    assert_eq!(word(a, 4), 0, "SAVE_DATA errCode");
    assert_eq!(&a[8..3080], blk, "block round trip");
    assert_eq!(&a[3080..3084], &[12, 0, 0, 12], "version and size");
    let c1 = word(a, 3084);
    assert!(c1 != 0 && c1 == sum(blk), "C1");
    let mut z = a.to_vec();
    le(&mut z, 3088, 0);
    let c2 = word(a, 3088);
    assert!(c2 != 0 && c2 == sum(&z), "C2");
}

#[test]
fn gamesvr_save_round_trip() {
    let dir = scratch("gamesvr_rt");
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let _svc = gamesvr::spawn(l.try_clone().unwrap(), dir.clone(), quiet()).unwrap();
    let mut c = connect(&l);
    let (coach, card) = new_club(&mut c);
    assert!(coach != 0 && card != 0);
    // no save yet: refused, never zeros (3107 on screen either way, but the club is not listed)
    assert_eq!(word(&save_data(&mut c, coach, card), 4), 1);
    assert_eq!(
        word(&coach_list(&mut c), 8 + 60),
        0,
        "unsaved club not listed"
    );
    let blk = block();
    let bad = ask(&mut c, 277, &save_set(coach, card, 1, &blk, true));
    assert_eq!(word(&bad, 4), 1, "wrong block sum refused");
    let ok = ask(&mut c, 277, &save_set(coach, card, 1, &blk, false));
    assert_eq!((ok.len(), word(&ok, 4), word(&ok, 8)), (76, 0, coach));
    assert_loads(&save_data(&mut c, coach, card), &blk);
    // wrong revision and unknown ids are refused
    let mut r = save_set(coach, card, 2, &blk, false);
    le(&mut r, 5940, 1);
    assert_eq!(word(&ask(&mut c, 277, &r), 4), 1, "revision");
    assert_eq!(
        word(&save_data(&mut c, coach + 1, card), 4),
        1,
        "unknown coach"
    );
    // INJUSTICE_COUNT: max(0, count + delta)
    let inj = |c: &mut TcpStream, d: i32| {
        let a = ask(
            c,
            289,
            &req(28, 24, |b| {
                le(b, 8, coach);
                le(b, 12, card);
                le(b, 16, d as u32);
            }),
        );
        assert_eq!(word(&a, 4), 0);
        word(&a, 8)
    };
    assert_eq!(
        [
            inj(&mut c, 1),
            inj(&mut c, -1),
            inj(&mut c, -1),
            inj(&mut c, 0)
        ],
        [1, 0, 0, 0]
    );
    // EXTENDS_NEW_ID: a new cardId; the save follows the club, the old card no longer loads it
    let a = ask(
        &mut c,
        287,
        &req(28, 24, |b| {
            le(b, 8, coach);
            le(b, 12, card);
        }),
    );
    let new = word(&a, 8);
    assert!(word(&a, 4) == 0 && new != 0 && new != card);
    assert_eq!(word(&save_data(&mut c, coach, card), 4), 1);
    assert_loads(&save_data(&mut c, coach, new), &blk);
    // ITEM_LIST: errCode 0 (card-in shows error 3107 on any other), also for a club it does not know
    assert_eq!(word(&ask(&mut c, 285, &req(24, 20, |_| {})), 4), 0);
    drop((c, _svc));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn gamesvr_new_club_survives_restart() {
    let dir = scratch("gamesvr_restart");
    let blk = block();
    let (coach, card) = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let _svc = gamesvr::spawn(l.try_clone().unwrap(), dir.clone(), quiet()).unwrap();
        let mut c = connect(&l);
        let list = coach_list(&mut c);
        assert_eq!((list.len(), word(&list, 4)), (9292, 0));
        assert!(
            list[8..].iter().all(|b| *b == 0),
            "unknown Aime id: no clubs"
        );
        let (coach, card) = new_club(&mut c);
        let ok = ask(&mut c, 277, &save_set(coach, card, 1, &blk, false));
        assert_eq!(word(&ok, 4), 0);
        (coach, card)
    }; // the stand-in stops here
    // WAL: once its links have closed, the store is checkpointed into gamesvr.db and -wal is gone, so a copy of
    // data\save after a run (update, installer, a player's backup) holds every club
    let wal = dir.join("gamesvr.db-wal");
    let t = std::time::Instant::now();
    while wal.exists() && t.elapsed() < std::time::Duration::from_secs(3) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!wal.exists(), "a stopped store leaves no -wal");
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let _svc = gamesvr::spawn(l.try_clone().unwrap(), dir.clone(), quiet()).unwrap();
    let mut c = connect(&l);
    let list = coach_list(&mut c);
    assert_eq!(word(&list, 4), 0);
    let r = &list[8..8 + 128];
    assert_eq!(
        (word(r, 0), word(r, 60)),
        (coach, card),
        "record 0 is the club"
    );
    assert_eq!(
        (&r[4..9], &r[64..68], r[122], r[123]),
        (&b"FORZA"[..], &b"WCCF"[..], 5, 0)
    );
    assert_eq!(word(&list, 8 + 128 + 60), 0, "one club only");
    assert_loads(&save_data(&mut c, coach, card), &blk);
    // the next club gets new ids
    let (c2, k2) = new_club(&mut c);
    assert!(c2 != coach && k2 != card);
    assert!(dir.join("gamesvr.db").is_file());
    drop((c, _svc));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn aimedb_replay() {
    let dir = scratch("aimedb");
    let db = dir.join("aimedb.json");
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let svc = aimedb::spawn(l.try_clone().unwrap(), db.clone(), quiet()).unwrap();
    let (mut c, mut conn) = (connect(&l), 0);
    for (i, r) in records("aimedb").iter().enumerate() {
        if r["conn"].as_u64().unwrap() != conn {
            conn = r["conn"].as_u64().unwrap();
            c = connect(&l);
        }
        let req = unhex(r["req"].as_str().unwrap());
        let resp = exchange(&mut c, &req, if req.len() > 20 { 20 } else { 0 });
        assert_eq!(hex(&resp), r["resp"].as_str().unwrap(), "record {i}");
    }
    drop(svc);
    // a new service on the same file knows the cards: the first access code of the recording keeps its hashed ID
    let svc = aimedb::Db::open(db).unwrap();
    assert_eq!(svc.len(), 5);
    assert_eq!(svc.aime_id("11112222333344445555"), 582_349_731);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[test]
fn aime_replay() {
    use forza::standins::aime;
    use std::fs::OpenOptions;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    let recs = records("aime");
    let dir = scratch("aime");
    let card = dir.join("card.txt");
    std::fs::write(&card, format!("{}\n", recs[0]["card"].as_str().unwrap())).unwrap();
    let pipe = format!(r"\\.\pipe\forza_test_aime_{}", std::process::id());
    let svc = aime::spawn(&pipe, Some(&card), quiet()).unwrap();
    let open = || {
        for _ in 0..100 {
            if let Ok(f) = OpenOptions::new().read(true).write(true).open(&pipe) {
                return f;
            }
            sleep(Duration::from_millis(50));
        }
        panic!("pipe never opened");
    };
    let (mut f, mut conn) = (open(), 0);
    for (i, r) in recs[1..].iter().enumerate() {
        if r["conn"].as_u64().unwrap() != conn {
            conn = r["conn"].as_u64().unwrap();
            drop(f);
            sleep(Duration::from_millis(500));
            f = open();
        }
        let req = unhex(r["req"].as_str().unwrap());
        let split = r["split"].as_u64().unwrap() as usize;
        if split > 0 {
            f.write_all(&req[..split]).unwrap();
            sleep(Duration::from_millis(100));
            f.write_all(&req[split..]).unwrap();
        } else {
            f.write_all(&req).unwrap();
        }
        sleep(Duration::from_millis(200));
        let mut resp = Vec::new();
        loop {
            let mut avail = 0u32;
            // SAFETY: the handle is the open pipe client; only the bytes-available out-pointer is given.
            let ok = unsafe {
                PeekNamedPipe(
                    f.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut avail,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || avail == 0 {
                break;
            }
            let mut buf = vec![0u8; avail as usize];
            f.read_exact(&mut buf).unwrap();
            resp.extend(buf);
            sleep(Duration::from_millis(50));
        }
        assert_eq!(hex(&resp), r["resp"].as_str().unwrap(), "record {i}");
    }
    drop(f);
    drop(svc); // stops cleanly
    let _ = std::fs::remove_dir_all(dir);
}

/// `YYYYMMDD-HHMMSS` in a file name -> `<t>` (the recorder's normalisation of the time stamps in backup names).
#[cfg(windows)]
fn norm_stamp(name: &str) -> String {
    let b = name.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let is_stamp = i + 15 <= b.len()
            && b[i..i + 8].iter().all(u8::is_ascii_digit)
            && b[i + 8] == b'-'
            && b[i + 9..i + 15].iter().all(u8::is_ascii_digit);
        if is_stamp {
            out.push_str("<t>");
            i += 15;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// Every file under `root`: (relative name with `/` and the stamp as `<t>`, size, sha256), sorted.
#[cfg(windows)]
fn listing(root: &std::path::Path) -> Vec<(String, u64, String)> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, u64, String)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                let data = std::fs::read(&p).unwrap();
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((
                    norm_stamp(&rel),
                    data.len() as u64,
                    hex(&Sha256::digest(&data)),
                ));
            }
        }
    }
    let mut v = Vec::new();
    walk(root, root, &mut v);
    v.sort();
    v
}

/// Replays `tests/rec/card_reader.jsonl`: per scenario a fresh folder with the recorded files, the reader on a
/// pipe named after this process, every recorded write and its answer byte for byte, and the folder's files
/// (names, sizes, sha256) wherever the Python listed them.
#[cfg(windows)]
#[test]
fn card_reader_replay() {
    use forza::standins::card_reader;
    use std::collections::VecDeque;
    use std::fs::OpenOptions;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    let recs = records("card_reader");
    let mut i = 0;
    let (mut scenarios, mut ios) = (0, 0);
    while i < recs.len() {
        let s = &recs[i];
        assert_eq!(s["t"], "scenario");
        let name = s["name"].as_str().unwrap();
        let dir = scratch(&format!("card_{name}"));
        for (rel, h) in s["files"].as_object().unwrap() {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, unhex(h.as_str().unwrap())).unwrap();
        }
        let card = s["card"].as_str().map(|c| dir.join(c));
        let new = s["new"].as_str().map(|c| dir.join(c));
        let mut urandom: VecDeque<Vec<u8>> = s["urandom"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| unhex(v.as_str().unwrap()))
            .collect();
        let used = urandom.len();
        let pipe = format!(r"\\.\pipe\forza_test_card_{}_{name}", std::process::id());
        let svc = card_reader::spawn_with_random(
            &pipe,
            card.as_deref(),
            new.as_deref(),
            quiet(),
            Box::new(move |buf| {
                let next = urandom
                    .pop_front()
                    .expect("more random bytes than recorded");
                assert_eq!(next.len(), buf.len());
                buf.copy_from_slice(&next);
            }),
        )
        .unwrap();
        let open = || {
            for _ in 0..100 {
                if let Ok(f) = OpenOptions::new().read(true).write(true).open(&pipe) {
                    return f;
                }
                sleep(Duration::from_millis(50));
            }
            panic!("pipe never opened");
        };
        let (mut f, mut conn) = (open(), 0);
        i += 1;
        while i < recs.len() && recs[i]["t"] != "scenario" {
            let r = &recs[i];
            i += 1;
            match r["t"].as_str().unwrap() {
                "put" => std::fs::write(
                    dir.join(r["name"].as_str().unwrap()),
                    unhex(r["hex"].as_str().unwrap()),
                )
                .unwrap(),
                "files" => {
                    sleep(Duration::from_millis(300));
                    let want: Vec<(String, u64, String)> = r["list"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|e| {
                            (
                                e[0].as_str().unwrap().to_string(),
                                e[1].as_u64().unwrap(),
                                e[2].as_str().unwrap().to_string(),
                            )
                        })
                        .collect();
                    assert_eq!(listing(&dir), want, "{name}: files after record {i}");
                }
                "io" => {
                    ios += 1;
                    if r["conn"].as_u64().unwrap() != conn {
                        conn = r["conn"].as_u64().unwrap();
                        drop(f);
                        sleep(Duration::from_millis(500));
                        f = open();
                    }
                    let req = unhex(r["req"].as_str().unwrap());
                    let split = r["split"].as_u64().unwrap() as usize;
                    if split > 0 {
                        f.write_all(&req[..split]).unwrap();
                        sleep(Duration::from_millis(100));
                        f.write_all(&req[split..]).unwrap();
                    } else {
                        f.write_all(&req).unwrap();
                    }
                    sleep(Duration::from_millis(200));
                    let mut resp = Vec::new();
                    loop {
                        let mut avail = 0u32;
                        // SAFETY: the handle is the open pipe client; only the bytes-available out-pointer is given.
                        let ok = unsafe {
                            PeekNamedPipe(
                                f.as_raw_handle(),
                                std::ptr::null_mut(),
                                0,
                                std::ptr::null_mut(),
                                &mut avail,
                                std::ptr::null_mut(),
                            )
                        };
                        if ok == 0 || avail == 0 {
                            break;
                        }
                        let mut buf = vec![0u8; avail as usize];
                        f.read_exact(&mut buf).unwrap();
                        resp.extend(buf);
                        sleep(Duration::from_millis(50));
                    }
                    assert_eq!(
                        hex(&resp),
                        r["resp"].as_str().unwrap(),
                        "{name}: record {i}"
                    );
                }
                t => panic!("unknown record {t}"),
            }
        }
        drop(f);
        drop(svc);
        let _ = used;
        let _ = std::fs::remove_dir_all(dir);
        scenarios += 1;
    }
    assert!(
        scenarios >= 14 && ios > 400,
        "{scenarios} scenarios, {ios} exchanges"
    );
}

#[test]
fn network_replay() {
    use forza::standins::network::{self, Kind};
    let recs = records("network");
    let svcs: [(&str, Kind); 5] = [
        ("master", Kind::Text),
        ("installer", Kind::Text),
        ("keychip", Kind::Text),
        ("amnet", Kind::Amnet),
        ("binary", Kind::Binary),
    ];
    let mut held = Vec::new();
    let mut ports = std::collections::HashMap::new();
    for (name, kind) in svcs {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        ports.insert(name, l.local_addr().unwrap());
        held.push(network::spawn(l, kind, name, "SBWG", quiet()).unwrap());
    }
    let (mut c, mut conn, mut svc) = (None::<TcpStream>, u64::MAX, "");
    for (i, r) in recs.iter().enumerate() {
        let (s, n) = (r["svc"].as_str().unwrap(), r["conn"].as_u64().unwrap());
        let req = unhex(r["req"].as_str().unwrap());
        if c.is_none() || s != svc || n != conn {
            let t = TcpStream::connect(ports[s]).unwrap();
            t.set_read_timeout(Some(Duration::from_millis(350)))
                .unwrap();
            c = Some(t);
            (svc, conn) = (s, n);
        }
        let c = c.as_mut().unwrap();
        let resp = if req.is_empty() {
            exchange(c, b"", 0)
        } else {
            exchange(c, &req, r["split"].as_u64().unwrap() as usize)
        };
        assert_eq!(hex(&resp), r["resp"].as_str().unwrap(), "record {i}");
    }
    drop(held);
}

/// M3: two fake cabinets log in, probe the reflect service, enter (243), poll (245) and get mirrored 246s.
#[test]
fn vs_two_clients_pair_through_reflect() {
    use forza::online::vs::{VsConfig, spawn_reflect};
    use std::net::{Ipv4Addr, UdpSocket};
    let dir = scratch("vs");
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let reflect_port = udp.local_addr().unwrap().port();
    let _r = spawn_reflect(udp, quiet()).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let o = gamesvr::Options {
        vs: Some(VsConfig {
            public_ip: Ipv4Addr::LOCALHOST,
            reflect_port,
            vslink_port: 20090,
        }),
        ..Default::default()
    };
    let _svc = gamesvr::spawn_opts(l.try_clone().unwrap(), dir, quiet(), o).unwrap();
    // a client: login, probe, returns (tcp, public ip, public port)
    let client = |shop: u32| {
        let mut c = connect(&l);
        let login = ask(&mut c, 201, &req(28, 24, |b| le(b, 8, shop)));
        assert_eq!(login[8..12], [127, 0, 0, 1]);
        let port = u16::from_le_bytes([login[12], login[13]]);
        assert_eq!(port, reflect_port);
        let u = UdpSocket::bind("127.0.0.1:0").unwrap();
        u.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut p = [0u8; 58];
        p[..8].copy_from_slice(&[0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C]);
        p[8] = 1;
        p[16] = 58;
        u.send_to(&p, ("127.0.0.1", port)).unwrap();
        let mut r = [0u8; 128];
        assert_eq!(u.recv(&mut r).unwrap(), 58);
        assert_eq!(r[52..56], [127, 0, 0, 1]);
        let mine = u.local_addr().unwrap().port();
        assert_eq!(u16::from_le_bytes([r[56], r[57]]), mine);
        (c, mine)
    };
    let (mut a, pa) = client(5);
    let (mut b, pb) = client(6);
    let round = word(&ask(&mut a, 223, &req(20, 16, |_| {})), 4);
    let entry = |port: u16, card: u32, shop: u32| {
        let mut e = vec![0u8; 280];
        le(&mut e, 0, 1);
        le(&mut e, 4, round);
        e[8..12].copy_from_slice(&[127, 0, 0, 1]);
        e[12..14].copy_from_slice(&port.to_le_bytes());
        le(&mut e, 44, card);
        le(&mut e, 48, shop);
        e
    };
    let poll = |c: &mut TcpStream, card: u32, shop: u32| {
        let mut q = vec![0u8; 20];
        le(&mut q, 0, 1);
        le(&mut q, 4, round);
        le(&mut q, 8, card);
        le(&mut q, 12, shop);
        ask(c, 245, &q)
    };
    assert_eq!(ask(&mut a, 243, &entry(pa, 111, 5)).len(), 12);
    let early = poll(&mut a, 111, 5);
    assert_eq!((word(&early, 12), early[122]), (0, 0), "keep waiting");
    // a cancelled entry never pairs
    ask(&mut b, 243, &entry(0, 222, 6));
    assert_eq!(poll(&mut a, 111, 5)[122], 0);
    ask(&mut b, 243, &entry(pb, 222, 6));
    let (ra, rb) = (poll(&mut a, 111, 5), poll(&mut b, 222, 6));
    assert_eq!((word(&ra, 12), word(&rb, 12)), (222, 111));
    assert_eq!((ra[122], rb[122]), (1, 1));
    assert_eq!(word(&ra, 116), word(&rb, 116), "same seed");
    assert_eq!(ra[16..20], [127, 0, 0, 1]);
    assert_eq!(
        (
            u16::from_le_bytes([ra[120], ra[121]]),
            u16::from_le_bytes([rb[120], rb[121]])
        ),
        (pb, pa)
    );
    assert_eq!(poll(&mut a, 111, 5)[116..120], ra[116..120], "stable");
}
