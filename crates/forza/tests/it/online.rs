//! Replays the recordings of `online/seat.py` and `online/send_logs.py` (`tests/rec/seat.jsonl`, `send_logs.jsonl`,
//! made against the real seat desk by `rec_i1_online.py`) against `forza::online`: a scripted server plays the
//! recorded replies and checks the client's bytes; packs are compared by entry names, sizes and hashes (the
//! recording's uploads are not replayed: logs are only saved now).
//! Step 9.4c renamed seat 1's panel log in `send_logs.jsonl` (seat1/wccfpanel.log -> seat1/forza_panel.log, the Rust
//! panel's); nothing else in the recording changed.

use crate::records;
use forza::online::save_logs::{self, Scrub};
use forza::online::seat::{self, Fault, Timing};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::Duration;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn scratch(what: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("forza_test_{what}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A loopback server that plays recorded connections in order. Events: `c` the client's bytes (compared), `s` bytes
/// to send, `x` close. After a script it answers `PING` with `PONG` until the client goes (more pings than recorded
/// are fine). Problems are collected, not panicked, because they happen on its threads.
struct Scripted {
    addr: SocketAddr,
    seen: Arc<AtomicUsize>,
    problems: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Scripted {
    fn new(conns: Vec<Vec<(String, Vec<u8>)>>) -> Self {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let addr = l.local_addr().unwrap();
        let (seen, problems, stop) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(Mutex::new(vec![])),
            Arc::new(AtomicBool::new(false)),
        );
        let (s, p, f) = (seen.clone(), problems.clone(), stop.clone());
        thread::spawn(move || {
            while !f.load(Ordering::Relaxed) {
                let Ok((c, _)) = l.accept() else {
                    sleep(Duration::from_millis(10));
                    continue;
                };
                let i = s.fetch_add(1, Ordering::Relaxed);
                let script = conns.get(i).cloned();
                let (p, f) = (p.clone(), f.clone());
                thread::spawn(move || match script {
                    Some(script) => play(c, i, script, &p, &f),
                    None => p
                        .lock()
                        .unwrap()
                        .push(format!("connection {i} was not recorded")),
                });
            }
        });
        Self {
            addr,
            seen,
            problems,
            stop,
        }
    }

    /// Stops it and returns the problems found.
    fn finish(self) -> Vec<String> {
        sleep(Duration::from_millis(100));
        self.stop.store(true, Ordering::Relaxed);
        self.problems.lock().unwrap().clone()
    }
}

fn play(
    mut c: TcpStream,
    i: usize,
    script: Vec<(String, Vec<u8>)>,
    problems: &Mutex<Vec<String>>,
    stop: &AtomicBool,
) {
    let bad = |m: String| {
        problems
            .lock()
            .unwrap()
            .push(format!("connection {i}: {m}"))
    };
    c.set_nonblocking(false).unwrap();
    c.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    // reads exactly n bytes, patiently; None when the client went first or the test ended
    let read = |c: &mut TcpStream, n: usize| -> Option<Vec<u8>> {
        let mut got = vec![0u8; n];
        let (mut at, mut idle) = (0, 0);
        while at < n {
            match c.read(&mut got[at..]) {
                Ok(0) => return None,
                Ok(k) => (at, idle) = (at + k, 0),
                Err(_) if stop.load(Ordering::Relaxed) || idle > 100 => return None,
                Err(_) => idle += 1,
            }
        }
        Some(got)
    };
    for (at, (tag, bytes)) in script.iter().enumerate() {
        let (tag, bytes) = (tag.as_str(), bytes.clone());
        match tag {
            "c" => match read(&mut c, bytes.len()) {
                Some(got) if got == bytes => {}
                Some(got) => {
                    return bad(format!(
                        "client sent {:?}, recorded {:?}",
                        String::from_utf8_lossy(&got[..got.len().min(80)]),
                        String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
                    ));
                }
                // the lease pings on a clock: under load it fits fewer pings in its life than the recording has,
                // and the client going while only PING/PONG pairs remain is that, not a fault
                None if script[at..]
                    .iter()
                    .all(|(t, b)| (t == "c" && b == b"PING\n") || (t == "s" && b == b"PONG\n")) =>
                {
                    return;
                }
                None => {
                    return bad(format!(
                        "client gone before {:?}",
                        String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
                    ));
                }
            },
            "s" => {
                let _ = c.write_all(&bytes);
            }
            _ => {
                let _ = c.shutdown(Shutdown::Both);
                return;
            }
        }
    }
    let mut buf = Vec::new();
    let mut b = [0u8; 64];
    loop {
        match c.read(&mut b) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&b[..n]),
            Err(_) if stop.load(Ordering::Relaxed) => return,
            Err(_) => {}
        }
        while buf.len() >= 5 {
            if buf.starts_with(b"PING\n") {
                buf.drain(..5);
                let _ = c.write_all(b"PONG\n");
            } else {
                return bad(format!("unexpected bytes {buf:?}"));
            }
        }
    }
}

fn events(v: &Value) -> Vec<(String, Vec<u8>)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e[0].as_str().unwrap().to_string(),
                unhex(e[1].as_str().unwrap()),
            )
        })
        .collect()
}

fn script_of(rec: &Value) -> Vec<Vec<(String, Vec<u8>)>> {
    rec["conns"]
        .as_array()
        .unwrap()
        .iter()
        .map(events)
        .collect()
}

fn closed_port() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

fn quiet() -> forza::standins::Log {
    Arc::new(|_| {})
}

#[test]
fn seat_replays_the_recorded_desk() {
    let recs = records("seat");
    assert_eq!(recs.len(), 9);
    for r in &recs {
        let name = r["name"].as_str().unwrap();
        let secs = |k: &str| Duration::from_secs_f64(r[k].as_f64().unwrap());
        let want = r["want"].as_u64().map(|w| w as u8);
        let silent = name == "no_answer";
        let t = Timing {
            connect: Duration::from_secs(2),
            answer: if silent {
                Duration::from_millis(400)
            } else {
                Duration::from_secs(5)
            },
            pong: Duration::from_secs(5),
            ping: secs("ping"),
            retry: secs("retry"),
        };
        let conns = script_of(r);
        let desk = if conns.is_empty() {
            None
        } else {
            Some(Scripted::new(conns.clone()))
        };
        let addr = desk.as_ref().map_or_else(closed_port, |d| d.addr);
        // the Rust lease stops pinging at its life, the Python sleeps a ping past it: give it half a second more
        let life = secs("life") + Duration::from_millis(500);
        let got = seat::claim(addr, want, life, t, quiet());
        let result = &r["result"];
        match (&got, result.get("seat")) {
            (Ok(l), Some(seat)) => {
                assert_eq!(
                    (u64::from(l.seat), l.projector),
                    (seat.as_u64().unwrap(), result["projector"] == 1),
                    "{name}"
                );
                sleep(life + Duration::from_millis(300));
            }
            (Err(e), None) => {
                assert_eq!(
                    e.fault.as_str(),
                    result["error"].as_str().unwrap(),
                    "{name}"
                );
                if !matches!(e.fault, Fault::NoBroker | Fault::NoAnswer) {
                    assert_eq!(e.detail, result["detail"].as_str().unwrap(), "{name}");
                }
            }
            (Ok(_), None) => panic!("{name}: a seat, recorded {result}"),
            (Err(e), Some(_)) => panic!("{name}: {e:?}, recorded {result}"),
        }
        drop(got);
        if let Some(d) = desk {
            let seen = d.seen.load(Ordering::Relaxed);
            assert_eq!(seen, conns.len(), "{name}: connections");
            let problems = d.finish();
            assert!(problems.is_empty(), "{name}: {problems:?}");
        }
    }
}

#[test]
fn dropping_the_lease_frees_the_seat() {
    // the client hangs up as soon as the lease drops, so the desk frees the seat at once
    let desk = Scripted::new(vec![vec![
        ("c".into(), b"WANT\n".to_vec()),
        ("s".into(), b"SEAT 4 PROJECTOR 0\n".to_vec()),
    ]]);
    let t = Timing {
        ping: Duration::from_secs(60),
        ..Timing::default()
    };
    let lease = seat::claim(desk.addr, None, Duration::from_secs(600), t, quiet()).unwrap();
    assert_eq!((lease.seat, lease.projector), (4, false));
    let started = std::time::Instant::now();
    drop(lease);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(desk.finish().is_empty());
}

// ---------------------------------------------------------------------------------------------------------------

fn bytes_of(spec: &Value) -> Vec<u8> {
    if let Some(t) = spec.get("text") {
        return t.as_str().unwrap().as_bytes().to_vec();
    }
    if let Some(h) = spec.get("hex") {
        return unhex(h.as_str().unwrap());
    }
    let (seed, size) = (
        spec["stream"].as_str().unwrap(),
        spec["size"].as_u64().unwrap() as usize,
    );
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < size {
        out.extend(Sha256::digest(format!("{seed}:{i}")));
        i += 1;
    }
    out.truncate(size);
    out
}

fn entries(blob: &[u8]) -> Vec<(String, usize, String)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(blob)).unwrap();
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).unwrap();
            let mut data = Vec::new();
            f.read_to_end(&mut data).unwrap();
            (
                f.name().unwrap().to_string(),
                data.len(),
                hex(&Sha256::digest(&data)),
            )
        })
        .collect()
}

#[test]
fn packs_match_the_python() {
    let recs = records("send_logs");
    let mut packs = 0;
    for r in recs.iter().filter(|r| r["t"] == "pack") {
        let name = r["name"].as_str().unwrap();
        let root = scratch(&format!("pack_{name}"));
        for (rel, spec) in r["files"].as_object().unwrap() {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes_of(spec)).unwrap();
        }
        let env = |k: &str| {
            r["env"]
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let scrub = Scrub {
            home: env("USERPROFILE"),
            user: env("USERNAME"),
            pc: env("COMPUTERNAME"),
            folders: false, // the Python left other users' folders in
        };
        let game = r["game"].as_str().map(|g| root.join(g));
        let tail = match r["tail"].as_u64() {
            Some(0) | None => save_logs::TAIL,
            Some(t) => t as usize,
        };
        let (blob, n) = save_logs::pack(
            &root.join("kit"),
            game.as_deref(),
            &scrub,
            tail,
            r["note"].as_str(),
        )
        .unwrap();
        let want: Vec<(String, usize, String)> = r["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e[0].as_str().unwrap().into(),
                    e[1].as_u64().unwrap() as usize,
                    e[2].as_str().unwrap().into(),
                )
            })
            .collect();
        // pc.txt and the save data came after the Python
        let mut got = entries(&blob);
        let added = got.len();
        got.retain(|(e, _, _)| e != "pc.txt" && !e.starts_with("save/"));
        let added = added - got.len();
        assert_eq!(got, want, "{name}");
        assert_eq!((n - added) as u64, r["n"].as_u64().unwrap(), "{name}");
        let _ = std::fs::remove_dir_all(root);
        packs += 1;
    }
    assert_eq!(packs, 11);
}

#[test]
fn save_writes_the_logs_zip_scrubbed_into_the_folder() {
    let root = scratch("save_logs");
    let (kit, seat) = (root.join("kit"), root.join("seat1"));
    std::fs::create_dir_all(kit.join("data/logs")).unwrap();
    std::fs::create_dir_all(&seat).unwrap();
    std::fs::write(kit.join("version.txt"), "1.2.3\n").unwrap();
    std::fs::write(kit.join("data/logs/run.txt"), "C:\\Users\\alice\\x ok").unwrap();
    std::fs::write(seat.join("other.log"), "seat line").unwrap();
    let scrub = Scrub {
        home: "C:\\Users\\alice".into(),
        user: "alice".into(),
        pc: "PC1".into(),
        folders: true,
    };
    let dir = root.join("Downloads");
    let dest = save_logs::save(
        &dir,
        &kit,
        Some(&root.join("game")),
        &scrub,
        Some("alice: it froze"),
    )
    .unwrap();
    assert_eq!(dest.parent(), Some(dir.as_path()));
    let name = dest.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("forza-wccf-logs-") && name.ends_with(".zip"),
        "{name}"
    );
    let mut z = zip::ZipArchive::new(std::fs::File::open(&dest).unwrap()).unwrap();
    let mut read = |name: &str| {
        let mut t = String::new();
        z.by_name(name).unwrap().read_to_string(&mut t).unwrap();
        t
    };
    assert_eq!(read("version.txt"), "1.2.3\n");
    assert!(!read("logs/run.txt").contains("alice"));
    assert_eq!(read("seat1/other.log"), "seat line");
    assert_eq!(read("report.txt"), "USER: it froze");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn version_matches_the_python() {
    let root = scratch("version");
    let mut n = 0;
    for r in records("send_logs").iter().filter(|r| r["t"] == "version") {
        let _ = std::fs::remove_file(root.join("version.txt"));
        if let Some(h) = r["text"].as_str() {
            std::fs::write(root.join("version.txt"), unhex(h)).unwrap();
        }
        assert_eq!(
            save_logs::version(&root),
            r["result"].as_str().unwrap(),
            "{r}"
        );
        n += 1;
    }
    assert_eq!(n, 8);
    let _ = std::fs::remove_dir_all(root);
}
