//! Replays the recordings of the Python stand-ins (`tests/rec/*.jsonl`, made by the throwaway recorders of step 10
//! piece J1 from synthetic requests) against the Rust services and compares byte for byte.

use forza::standins::{Handle, Log, aimedb, gamesvr};
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
    // a new service on the same file knows the cards: the first access code of the recording keeps ID 1
    let svc = aimedb::Db::open(db).unwrap();
    assert_eq!(svc.len(), 5);
    assert_eq!(svc.aime_id("11112222333344445555"), 1);
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
        held.push(network::spawn(l, kind, name, quiet()).unwrap());
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
