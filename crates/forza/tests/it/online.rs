//! Replays the recordings of `online/seat.py` and `online/send_logs.py` (`tests/rec/seat.jsonl`, `send_logs.jsonl`,
//! made against the real seat desk and log inbox by `rec_i1_online.py`) against `forza::online`: a scripted server
//! plays the recorded replies and checks the client's bytes; packs are compared by entry names, sizes and hashes.
//! Step 9.4c renamed seat 1's panel log in `send_logs.jsonl` (seat1/wccfpanel.log -> seat1/forza_panel.log, the Rust
//! panel's); nothing else in the recording changed.

use forza::online::seat::{self, Fault, Timing};
use forza::online::send_logs::{self, Scrub, SendError};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
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
        };
        let game = r["game"].as_str().map(|g| root.join(g));
        let tail = match r["tail"].as_u64() {
            Some(0) | None => send_logs::TAIL,
            Some(t) => t as usize,
        };
        let (blob, n) = send_logs::pack(
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
        assert_eq!(entries(&blob), want, "{name}");
        assert_eq!(n as u64, r["n"].as_u64().unwrap(), "{name}");
        let _ = std::fs::remove_dir_all(root);
        packs += 1;
    }
    assert_eq!(packs, 11);
}

#[test]
fn pack_to_file_writes_the_logs_zip_scrubbed() {
    let root = scratch("save_logs");
    let (kit, seat) = (root.join("kit"), root.join("seat1"));
    std::fs::create_dir_all(kit.join("data/logs")).unwrap();
    std::fs::create_dir_all(&seat).unwrap();
    std::fs::write(kit.join("version.txt"), "1.2.3\n").unwrap();
    std::fs::write(kit.join("data/logs/run.txt"), "C:\\Users\\alice\\x ok").unwrap();
    std::fs::write(seat.join("other.log"), "seat line").unwrap();
    let scrub = Scrub { home: "C:\\Users\\alice".into(), user: "alice".into(), pc: "PC1".into() };
    let dest = root.join("out.zip");
    let n = send_logs::pack_to_file(&kit, Some(&root.join("game")), &scrub, &dest).unwrap();
    let mut z = zip::ZipArchive::new(std::fs::File::open(&dest).unwrap()).unwrap();
    assert_eq!(n, z.len());
    let mut read = |name: &str| {
        let mut t = String::new();
        z.by_name(name).unwrap().read_to_string(&mut t).unwrap();
        t
    };
    assert_eq!(read("version.txt"), "1.2.3\n");
    assert!(!read("logs/run.txt").contains("alice"));
    assert_eq!(read("seat1/other.log"), "seat line");
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
            send_logs::version(&root),
            r["result"].as_str().unwrap(),
            "{r}"
        );
        n += 1;
    }
    assert_eq!(n, 8);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sends_match_the_recorded_inbox() {
    let mut n = 0;
    for r in records("send_logs").iter().filter(|r| r["t"] == "send") {
        let name = r["name"].as_str().unwrap();
        let conns = script_of(r);
        let inbox = if conns.is_empty() {
            None
        } else {
            Some(Scripted::new(conns))
        };
        let addr = inbox.as_ref().map_or_else(closed_port, |i| i.addr);
        for (blob, result) in r["blobs"]
            .as_array()
            .unwrap()
            .iter()
            .zip(r["results"].as_array().unwrap())
        {
            let got = send_logs::send(
                addr,
                &unhex(blob.as_str().unwrap()),
                r["ver"].as_str().unwrap(),
                Duration::from_secs(10),
            );
            let what = result[1].as_str().unwrap();
            match (&got, result[0].as_bool().unwrap()) {
                (Ok(code), true) => assert_eq!(code, what, "{name}"),
                (Err(SendError::Refused(why)), false) => assert_eq!(why, what, "{name}"),
                (Err(SendError::NoAnswer(_)), false) => {
                    assert!(what.contains("did not answer"), "{name}: {what}")
                }
                _ => panic!("{name}: {got:?}, recorded {result}"),
            }
        }
        if let Some(i) = inbox {
            let problems = i.finish();
            assert!(problems.is_empty(), "{name}: {problems:?}");
        }
        n += 1;
    }
    assert_eq!(n, 5);
}

#[test]
fn an_answer_it_cannot_read_is_garbled() {
    for reply in [&b"WAT\n"[..], b"OK k7q2m\n", b"OK AB\n", b""] {
        let blob = b"PK\x03\x04".to_vec();
        let head = format!("WCCFLOGS1 {} 5.5\n", blob.len()).into_bytes();
        let mut script = vec![("c".to_string(), [head, blob.clone()].concat())];
        if !reply.is_empty() {
            script.push(("s".into(), reply.to_vec()));
        }
        let srv = Scripted::new(vec![script]);
        let got = send_logs::send(srv.addr, &blob, "5.5", Duration::from_secs(5));
        assert_eq!(got, Err(SendError::Garbled), "{reply:?}");
        assert!(srv.finish().is_empty());
    }
}

#[test]
fn upload_sends_the_report_once() {
    let root = scratch("upload");
    std::fs::create_dir_all(root.join("kit/data/logs")).unwrap();
    std::fs::write(root.join("kit/data/logs/run_a.txt"), "hello\n").unwrap();
    std::fs::write(root.join("kit/version.txt"), "kit-9.1\n").unwrap();
    let report = root.join("report.txt");
    std::fs::write(&report, "the server\nwhy it was sent\n").unwrap();
    let (blob, n) = send_logs::pack(
        &root.join("kit"),
        None,
        &Scrub::default(),
        send_logs::TAIL,
        Some("why it was sent\n"),
    )
    .unwrap();
    let head = format!("WCCFLOGS1 {} 9.1\n", blob.len()).into_bytes();
    let srv = Scripted::new(vec![vec![
        ("c".into(), [head, blob.clone()].concat()),
        ("s".into(), b"OK ABC12\n".to_vec()),
    ]]);
    let got = send_logs::upload(
        srv.addr,
        &root.join("kit"),
        None,
        &Scrub::default(),
        Some(&report),
    )
    .unwrap();
    assert_eq!(got, ("ABC12".to_string(), n, blob.len()));
    assert!(!Path::new(&report).exists());
    assert!(srv.finish().is_empty());
    let _ = std::fs::remove_dir_all(root);
}
