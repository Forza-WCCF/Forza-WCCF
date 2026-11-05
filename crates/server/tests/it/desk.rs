//! Replays `rec/desk.jsonl` (recorded against the Python seat desks by `rec_i2_desk.py`) against `forza_server::desk`:
//! every client byte sent, every answer and hang-up compared, the firewall file and log lines checked. Also ports
//! `test_seat_desk_game.py` and `test_hold_projector.py` (the recorded `game` and `hold*` scenarios).

use crate::common::*;
use forza_server::desk::{DeskConfig, read_hold_ip, start};
use forza_server::{Edition, Handle};
use std::time::{Duration, Instant};

fn desk_of(rec: &serde_json::Value, root: &std::path::Path, log: forza_server::Log) -> Handle {
    let cfg = &rec["cfg"];
    let e1718 = cfg["edition"] == "2017-18";
    let mut c = DeskConfig::new(if e1718 {
        Edition::E1718
    } else {
        Edition::E1011
    });
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.silent = Duration::from_secs_f64(cfg["silent"].as_f64().unwrap());
    c.control_log = Some(root.join("data/logs/_dbg_control.txt"));
    c.hold_file = Some(root.join("data/hold_projector.txt"));
    c.hold_fw_test = Some(root.join(if cfg["fw"] == "folder" {
        "data"
    } else {
        "fw.txt"
    }));
    c.hold_every = Duration::from_millis(200);
    if let Some(h) = cfg["hold"].as_str() {
        std::fs::write(root.join("data/hold_projector.txt"), h).unwrap();
    }
    start(c, log).unwrap()
}

fn fw_lines(root: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(root.join("fw.txt"))
        .map(|t| t.lines().map(String::from).collect())
        .unwrap_or_default()
}

#[test]
fn the_desk_answers_as_the_python_desk_did() {
    let recs = records("desk");
    assert_eq!(recs.len(), 9);
    for rec in &recs {
        let name = rec["name"].as_str().unwrap();
        let root = scratch(&format!("desk_{name}"));
        std::fs::create_dir_all(root.join("data/logs")).unwrap();
        let (log, lines) = capture();
        let desk = desk_of(rec, &root, log);
        let mut tcp = Tcp::new(desk.addr());
        for (i, op) in rec["ops"].as_array().unwrap().iter().enumerate() {
            if tcp.run(op, name, i) {
                continue;
            }
            match op["op"].as_str().unwrap() {
                "ctl" => {
                    let p = root.join("data/logs/_dbg_control.txt");
                    let bytes = unhex(op["hex"].as_str().unwrap());
                    match op["mode"].as_str().unwrap() {
                        "remove" => std::fs::remove_file(p).unwrap(),
                        "write" => std::fs::write(p, bytes).unwrap(),
                        _ => {
                            use std::io::Write;
                            let mut f = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(p)
                                .unwrap();
                            f.write_all(&bytes).unwrap();
                        }
                    }
                }
                "fw" => {
                    let until = op["until"].as_str();
                    let end = Instant::now() + Duration::from_secs(4);
                    while until
                        .is_some_and(|u| fw_lines(&root).last().map(String::as_str) != Some(u))
                        && Instant::now() < end
                    {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    let want: Vec<&str> = op["lines"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|l| l.as_str().unwrap())
                        .collect();
                    assert_eq!(fw_lines(&root), want, "{name} op {i}: the firewall rule");
                }
                "log" => {
                    let text = lines.lock().unwrap().join("\n");
                    for (sub, found) in op["has"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .zip(op["found"].as_array().unwrap())
                    {
                        assert_eq!(
                            text.contains(sub.as_str().unwrap()),
                            found.as_bool().unwrap(),
                            "{name} op {i}: log {sub}"
                        );
                    }
                }
                other => panic!("{name}: unknown op {other}"),
            }
        }
        drop(tcp);
        drop(desk);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn hold_file_addresses() {
    let root = scratch("hold_file");
    let ip = |text: &str| {
        std::fs::write(root.join("h.txt"), text).unwrap();
        read_hold_ip(&root.join("h.txt"))
    };
    assert_eq!(ip("10.0.0.9\n"), Some("10.0.0.9".into()));
    assert_eq!(ip("10.0.0.9 the box"), Some("10.0.0.9".into()));
    assert_eq!(ip("010.000.0.255"), Some("010.000.0.255".into()));
    for bad in [
        "",
        "not an address 999",
        "10.0.0",
        "10.0.0.256",
        "10.0.0.9.1",
        "1..2.3",
        "a.b.c.d",
        "10.0.0.-1",
    ] {
        assert_eq!(ip(bad), None, "{bad}");
    }
    assert_eq!(read_hold_ip(&root.join("missing.txt")), None);
}

#[test]
fn stopping_the_desk_closes_its_port() {
    let desk = start(
        {
            let mut c = DeskConfig::new(Edition::E1011);
            c.bind = "127.0.0.1".into();
            c.port = 0;
            c
        },
        capture().0,
    )
    .unwrap();
    let addr = desk.addr();
    assert!(std::net::TcpStream::connect(addr).is_ok());
    desk.stop();
    assert!(std::net::TcpStream::connect(addr).is_err());
}

/// An older Launcher's problem report: refused at once (a clean `NO`, no hang), nothing kept.
#[test]
fn reports_and_log_uploads_are_refused() {
    use std::io::{Read, Write};
    for ed in [Edition::E1011, Edition::E1718] {
        let mut c = DeskConfig::new(ed);
        c.bind = "127.0.0.1".into();
        c.port = 0;
        let desk = start(c, capture().0).unwrap();
        let mut s = std::net::TcpStream::connect(desk.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(b"REPORT 100 5 0.4.1\n").unwrap();
        let mut got = String::new();
        s.read_to_string(&mut got).unwrap();
        assert!(got.starts_with("NO ") && got.ends_with('\n'), "{got:?}");
    }
}

/// An arcade guest's club through the real desk: refused for a seat nobody holds, in, out and dropped once it holds one.
#[test]
fn a_club_needs_a_held_seat() {
    use forza::online::club::{self, Club};
    use std::io::{Read, Write};
    let root = scratch("desk_club");
    let mut c = DeskConfig::new(Edition::E1718);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.club_db = Some(root.join("gamesvr.db"));
    let (log, _) = capture();
    let desk = start(c, log).unwrap();
    let (code, mine) = (
        "50123456789012345678",
        Club {
            aime: 1,
            ..Default::default()
        },
    );
    let err = club::send(desk.addr(), 2, 50, code, &mine).unwrap_err();
    assert!(err.contains("seat 2 is not taken"), "{err}");
    let mut seat = std::net::TcpStream::connect(desk.addr()).unwrap();
    seat.write_all(b"WANT SEAT 2\n").unwrap();
    let mut b = [0u8; 64];
    let n = seat.read(&mut b).unwrap();
    assert!(b[..n].starts_with(b"SEAT 2"));
    club::send(desk.addr(), 2, 50, code, &mine).unwrap();
    let (_, there) = club::fetch(desk.addr(), 50, code, "").unwrap().unwrap();
    assert_eq!(there.aime, 50);
    assert!(
        club::fetch(desk.addr(), 50, "50123456789012345679", "").is_err(),
        "another card's token"
    );
    club::release(desk.addr(), 50, code).unwrap();
    assert!(club::fetch(desk.addr(), 50, code, "").is_err(), "dropped");
}

/// det-math: a PC whose match engine math differs from a seat already held is refused (its engine would desync against
/// that seat's); the same token, and a reclaim, take a seat; a PC that sends none counts as the original math.
#[test]
fn seats_must_run_the_same_det_math() {
    use std::io::{Read, Write};
    let mut c = DeskConfig::new(Edition::E1718);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    let desk = start(c, capture().0).unwrap();
    let want = |line: &str| {
        let mut s = std::net::TcpStream::connect(desk.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(format!("{line}\n").as_bytes()).unwrap();
        let mut b = [0u8; 128];
        let n = s.read(&mut b).unwrap();
        (s, String::from_utf8_lossy(&b[..n]).trim_end().to_owned())
    };
    let (_host, got) = want("WANT DMATH 0123456789abcdef");
    assert_eq!(got, "SEAT 1 PROJECTOR 1");
    let (_same, got) = want("WANT DMATH 0123456789abcdef");
    assert_eq!(got, "SEAT 2 PROJECTOR 0");
    for (line, theirs) in [
        ("WANT DMATH 0123456789abcdef-off", "0123456789abcdef-off"),
        ("WANT DMATH fedcba9876543210", "fedcba9876543210"),
        ("WANT", "none"),
        ("WANT SEAT 5 TICKET ab12 DMATH none", "none"),
    ] {
        let (_, got) = want(line);
        assert_eq!(
            got,
            format!("ERR dmath 0123456789abcdef {theirs}"),
            "{line}"
        );
    }
    // the seat-desk client's own words reach the same answer
    let refused = forza::online::seat::claim_with(
        desk.addr(),
        None,
        None,
        Some("0123456789abcdef-off"),
        Duration::from_secs(5),
        forza::online::seat::Timing::default(),
        std::sync::Arc::new(|_| {}),
    )
    .err()
    .unwrap();
    assert_eq!(
        refused.detail,
        "ERR dmath 0123456789abcdef 0123456789abcdef-off"
    );
}
