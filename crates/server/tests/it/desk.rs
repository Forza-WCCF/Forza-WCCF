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
