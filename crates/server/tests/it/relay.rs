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

/// A hosted 2017-18 arcade (live 2026-10-27): every tunnelled seat reaches the relay from 127.0.0.1, each from its own
/// port. Seats 1 and 2 play; then seats 3 and 4 start a match of their own. Each seat gets only its opponent's
/// packets, all of them, labelled with the opponent's seat, before and after the second match starts.
#[test]
fn two_matches_in_one_arcade_stay_apart() {
    let relay = start(config(Edition::E1718, 30.0), capture().0).unwrap();
    let seats: Vec<_> = (0..4).map(|_| udp_client()).collect();
    let peer = |s: u8| [2, 1, 4, 3][usize::from(s) - 1];
    for (i, s) in seats.iter().enumerate() {
        send(
            s,
            &[b"WRL1R".as_slice(), &[i as u8 + 1]].concat(),
            relay.addr(),
        );
    }
    let mut buf = [0u8; 256];
    for round in 0..40u8 {
        // the second match kicks off at round 20
        let playing: &[u8] = if round < 20 { &[1, 2] } else { &[1, 2, 3, 4] };
        for &s in playing {
            let d = [b"WRL1D".as_slice(), &[peer(s), s, s, round]].concat();
            send(&seats[usize::from(s) - 1], &d, relay.addr());
        }
        for &s in playing {
            let sock = &seats[usize::from(s) - 1];
            sock.set_read_timeout(Some(LATE)).unwrap();
            let n = sock
                .recv(&mut buf)
                .unwrap_or_else(|e| panic!("seat {s} round {round}: {e}"));
            let p = peer(s);
            // F, the sender's seat, its address 127.0.0.1, then its payload (its seat and the round)
            assert_eq!(
                &buf[..n],
                [b"WRL1F".as_slice(), &[p, 127, 0, 0, 1, p, round]].concat(),
                "seat {s} round {round}"
            );
        }
    }
    for (i, s) in seats.iter().enumerate() {
        s.set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        assert!(
            s.recv(&mut buf).is_err(),
            "seat {}: a packet too many",
            i + 1
        );
    }
}

/// A 2017-18 frame-sync datagram (layout in `desync.rs`) carrying frames `f-5..=f` with values from `value`.
fn sync_packet(f: u16, value: impl Fn(u16) -> u32) -> Vec<u8> {
    let mut p = vec![0u8; 520];
    p[..8].copy_from_slice(&[0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C]);
    p[8] = 10;
    p[16..20].copy_from_slice(&520u32.to_le_bytes());
    p[54] = 6;
    let newest = i32::from(f) + 100;
    p[56..60].copy_from_slice(&newest.to_le_bytes());
    for k in 0..6 {
        let r = &mut p[64 + k * 76..64 + (k + 1) * 76];
        let g = f + k as u16 - 5;
        r[..4].copy_from_slice(&(newest - 5 + k as i32).to_le_bytes());
        r[8] = 5;
        r[9] = 64;
        r[12..14].copy_from_slice(&g.to_be_bytes());
        r[14..18].copy_from_slice(&value(g).to_be_bytes());
    }
    p
}

#[test]
fn the_desync_watch_logs_the_first_differing_frame_and_passes_every_packet() {
    let (log, lines) = capture();
    let relay = start(config(Edition::E1718, 30.0), log).unwrap();
    let seats: Vec<_> = (0..2).map(|_| udp_client()).collect();
    for (i, s) in seats.iter().enumerate() {
        send(
            s,
            &[b"WRL1R".as_slice(), &[i as u8 + 1]].concat(),
            relay.addr(),
        );
    }
    let mut buf = [0u8; 600];
    for f in (10..60u16).step_by(4) {
        for (s, peer) in [(1u8, 2u8), (2, 1)] {
            // seat 2's engine draws a different number from frame 40 on
            let p = sync_packet(f, |g| {
                u32::from(g) ^ if s == 2 && g >= 40 { 0x100 } else { 0 }
            });
            let d = [b"WRL1D".as_slice(), &[peer, s], &p].concat();
            send(&seats[usize::from(s) - 1], &d, relay.addr());
            let sock = &seats[usize::from(peer) - 1];
            sock.set_read_timeout(Some(LATE)).unwrap();
            let n = sock.recv(&mut buf).unwrap();
            assert_eq!(
                &buf[10..n],
                &p[..],
                "frame {f} seat {s}: the packet passes unchanged"
            );
        }
    }
    let text = wait_for(&lines, |t| t.contains("differs"));
    assert!(text.contains("desync watch: seats 1-2\n"), "{text}");
    assert!(
        text.contains(
            "desync watch: seats 1-2 frame 40 differs: seat 2 0x00000128, seat 1 0x00000028"
        ),
        "{text}"
    );
    assert_eq!(text.matches("differs").count(), 1, "{text}");
}

#[test]
fn the_test_stall_drops_game_packets_for_its_time() {
    let file = std::env::temp_dir().join(format!("relay_stall_{}.txt", std::process::id()));
    let mut c = config(Edition::E1718, 30.0);
    c.stall_file = Some(file.clone());
    let relay = start(c, capture().0).unwrap();
    let (a, b) = (udp_client(), udp_client());
    send(&a, b"WRL1R\x01", relay.addr());
    send(&b, b"WRL1R\x02", relay.addr());
    b.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut buf = [0u8; 64];
    let mut passes = |round: u8| {
        send(
            &a,
            &[b"WRL1D\x02\x01".as_slice(), &[round]].concat(),
            relay.addr(),
        );
        b.recv(&mut buf).is_ok_and(|n| buf[n - 1] == round)
    };
    assert!(passes(0));
    std::fs::write(&file, "1500").unwrap();
    std::thread::sleep(Duration::from_millis(150)); // the relay looks every 100 ms, when a packet comes
    assert!(!passes(1) && !passes(2), "a game packet passed the stall");
    assert!(!file.exists(), "the stall file is read once");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(passes(3));
}
