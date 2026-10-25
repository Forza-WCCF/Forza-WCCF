//! Replays `rec/relay.jsonl` (recorded against the Python relays by `rec_i2_relay.py`: R and D packets, pings, junk,
//! a stranger's claim of a live seat, forgetting, 2017-18) against `forza_server::relay`, and ports `test_relay.py`'s
//! ownership rule (the recorded `hijack` scenario) with the 30-second report's wording.

use crate::common::*;
use forza_server::Edition;
use forza_server::relay::{RelayConfig, start};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// On a busy Windows PC loopback datagrams from different sockets reach the relay out of order, the first datagram a
/// socket sends after a few quiet seconds can vanish, and threads stall for a few hundred ms. So each socket sends a
/// junk datagram (the relay ignores it) before every real one, one packet at a time, and the recorded clock (the
/// `forget` time and the sleeps) is scaled with its ratios kept.
const PAUSE: Duration = Duration::from_millis(10);
const SLOW: f64 = 3.0;
const LATE: Duration = Duration::from_secs(5);

/// Sends one real datagram after a junk one (see above), then pauses.
fn send(s: &std::net::UdpSocket, p: &[u8], to: std::net::SocketAddr) {
    s.send_to(b"x", to).unwrap();
    std::thread::sleep(PAUSE);
    s.send_to(p, to).unwrap();
    std::thread::sleep(PAUSE);
}

/// Waits up to 10 s for a logged line that satisfies `ok`.
fn wait_for(lines: &std::sync::Mutex<Vec<String>>, ok: impl Fn(&str) -> bool) -> String {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let text = lines.lock().unwrap().join("\n");
        if ok(&text) || Instant::now() > end {
            return text;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn config(edition: Edition, forget: f64) -> RelayConfig {
    let mut c = RelayConfig::new(edition);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.forget = Duration::from_secs_f64(forget);
    c
}

#[test]
fn the_relay_passes_packets_as_the_python_relay_did() {
    let recs = records("relay");
    assert_eq!(recs.len(), 4);
    for rec in &recs {
        let name = rec["name"].as_str().unwrap();
        let ed = if rec["cfg"]["edition"] == "2017-18" {
            Edition::E1718
        } else {
            Edition::E1011
        };
        let relay = start(
            config(ed, rec["cfg"]["forget"].as_f64().unwrap() * SLOW),
            capture().0,
        )
        .unwrap();
        let mut seats = HashMap::new();
        for (i, op) in rec["ops"].as_array().unwrap().iter().enumerate() {
            let id = op["c"].as_str().unwrap_or("").to_string();
            match op["op"].as_str().unwrap() {
                "uopen" => {
                    seats.insert(id, udp_client());
                }
                "usend" => send(
                    &seats[&id],
                    &unhex(op["hex"].as_str().unwrap()),
                    relay.addr(),
                ),
                "sleep" => {
                    std::thread::sleep(Duration::from_secs_f64(op["s"].as_f64().unwrap() * SLOW))
                }
                "urecv" => {
                    let s = &seats[&id];
                    let want: Vec<&str> = op["got"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|g| g.as_str().unwrap())
                        .collect();
                    let wait = Duration::from_secs_f64(op["wait"].as_f64().unwrap());
                    let (mut got, mut buf) = (Vec::new(), [0u8; 65536]);
                    // wait up to 5 s for the packets that must come, then `wait` for any that must not
                    loop {
                        let t = if got.len() < want.len() { LATE } else { wait };
                        s.set_read_timeout(Some(t)).unwrap();
                        let Ok(n) = s.recv(&mut buf) else { break };
                        got.push(hex(&buf[..n]));
                    }
                    assert_eq!(got, want, "{name} op {i}: seat {id}");
                }
                other => panic!("{name}: unknown op {other}"),
            }
        }
    }
}

#[test]
fn the_report_says_what_passed_was_dropped_and_rejected() {
    let mut c = config(Edition::E1011, 2.0);
    c.report = Duration::from_secs(1);
    let (log, lines) = capture();
    let relay = start(c, log).unwrap();
    let (a, b, stranger) = (udp_client(), udp_client(), udp_client());
    let send = |s: &std::net::UdpSocket, p: &[u8]| send(s, p, relay.addr());
    send(&a, b"WRL1R\x03");
    send(&b, b"WRL1R\x04");
    send(&a, b"WRL1D\x04\x03x");
    send(&a, b"WRL1D\x06\x03x"); // seat 6 unknown: dropped
    send(&stranger, b"WRL1R\x03"); // seat 3 is live: rejected, twice
    send(&stranger, b"WRL1D\x04\x03x");
    let text = wait_for(&lines, |t| t.contains("last 30 s"));
    assert!(text.contains("seat 3 is at 127.0.0.1:"), "{text}");
    assert!(
        text.contains("last 30 s: 3->4 1; dropped (seat not known): ->6 1; rejected (seat held by another address): seat 3 2"),
        "{text}"
    );
    // nobody refreshes: both seats are forgotten at the first report after `forget`
    let text = wait_for(&lines, |t| t.contains("forgotten"));
    assert!(
        text.contains("seat 3 (127.0.0.1:") && text.contains("not heard from for 2 s - forgotten"),
        "{text}"
    );
}
