//! S0 load harness (Linux only, `#[ignore]`d): starts the built `forza-server all` binary on loopback, holds fake
//! shops on the seat desk (WANT, then PING every second), pumps datagrams through the relay between seat pairs, reads
//! the server's RSS and CPU from /proc and prints one JSON line.
//!
//! Run: `cargo test --release -p forza-server --test load -- --ignored --nocapture`
//!
//! Knobs (environment): `WCCF_LOAD_IDLE` idle seconds measured first (default 10), `WCCF_LOAD_SECS` load seconds
//! (10), `WCCF_LOAD_RATE` packets per second per side (60), `WCCF_LOAD_PAIRS` seat pairs (4), `WCCF_LOAD_SHOPS`
//! shops on the desk (8). The protocol has seats 1-8 and a seat belongs to one address, so one relay carries 4 pairs
//! at most; the harness refuses more.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, UdpSocket};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// (VmRSS, VmHWM) in KiB from /proc/PID/status.
fn rss(pid: u32) -> (u64, u64) {
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let get = |k: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(k))
            .and_then(|v| v.split_whitespace().next()?.parse().ok())
            .unwrap_or(0)
    };
    (get("VmRSS:"), get("VmHWM:"))
}

/// utime + stime in clock ticks (100 per second on Linux).
fn cpu_ticks(pid: u32) -> u64 {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let f: Vec<&str> = s.rsplit_once(')').unwrap().1.split_whitespace().collect();
    f[11].parse::<u64>().unwrap() + f[12].parse::<u64>().unwrap() // fields 14 and 15
}

fn shop(port: u16, stop: Arc<AtomicBool>, pongs: Arc<AtomicU64>) {
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut r = BufReader::new(c.try_clone().unwrap());
    let mut line = String::new();
    c.write_all(b"WANT\n").unwrap();
    r.read_line(&mut line).unwrap();
    assert!(line.starts_with("SEAT "), "desk said {line:?}");
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_secs(1));
        line.clear();
        if c.write_all(b"PING\n").is_ok()
            && r.read_line(&mut line).is_ok()
            && line.starts_with("PONG")
        {
            pongs.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// One cabinet: claims `seat`, sends `rate` packets/s to `peer`, counts what comes back until `stop`.
fn cabinet(
    relay: u16,
    seat: u8,
    peer: u8,
    rate: u64,
    stop: Arc<AtomicBool>,
    sent: Arc<AtomicU64>,
    got: Arc<AtomicU64>,
) {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.connect(("127.0.0.1", relay)).unwrap();
    s.send(&[b'W', b'R', b'L', b'1', b'R', seat]).unwrap();
    let mut pkt = vec![b'W', b'R', b'L', b'1', b'D', peer, seat];
    pkt.extend_from_slice(&[0xAB; 64]);
    let step = Duration::from_nanos(1_000_000_000 / rate.max(1));
    let (mut next, mut buf) = (Instant::now(), [0u8; 4200]);
    while !stop.load(Ordering::Relaxed) {
        if s.send(&pkt).is_ok() {
            sent.fetch_add(1, Ordering::Relaxed);
        }
        next += step;
        loop {
            let left = next.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            s.set_read_timeout(Some(left)).unwrap();
            if s.recv(&mut buf).is_ok_and(|n| buf[4] == b'F' && n > 10) {
                got.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

#[test]
#[ignore = "S0 load harness: run explicitly, see the module doc"]
fn load() {
    let (idle, secs, rate) = (
        knob("WCCF_LOAD_IDLE", 10),
        knob("WCCF_LOAD_SECS", 10),
        knob("WCCF_LOAD_RATE", 60),
    );
    let (pairs, shops) = (knob("WCCF_LOAD_PAIRS", 4) as u8, knob("WCCF_LOAD_SHOPS", 8));
    assert!(
        (1..=4).contains(&pairs),
        "seats 1-8 give at most 4 pairs per relay"
    );
    let (desk, relay, inbox) = (free_port(), free_port(), free_port());
    let dir = std::env::temp_dir().join(format!("forza_load_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_forza-server"))
        .args(["all", "100000"])
        .arg(&dir)
        .env("WCCF_BIND", "127.0.0.1")
        .env("WCCF_BROKER_PORT", desk.to_string())
        .env("WCCF_RELAY_PORT", relay.to_string())
        .env("WCCF_INBOX_PORT", inbox.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    thread::sleep(Duration::from_secs(2)); // start-up settles

    // idle: nothing connected
    let (t0, c0) = (Instant::now(), cpu_ticks(pid));
    thread::sleep(Duration::from_secs(idle));
    let idle_cpu = (cpu_ticks(pid) - c0) as f64 / 100.0 / t0.elapsed().as_secs_f64() * 100.0;
    let (idle_rss, _) = rss(pid);

    // load
    let stop = Arc::new(AtomicBool::new(false));
    let (sent, got, pongs) = (
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    );
    let mut threads = Vec::new();
    for _ in 0..shops {
        let (st, p) = (stop.clone(), pongs.clone());
        threads.push(thread::spawn(move || shop(desk, st, p)));
    }
    for i in 0..pairs {
        for (seat, peer) in [(2 * i + 1, 2 * i + 2), (2 * i + 2, 2 * i + 1)] {
            let (st, s, g) = (stop.clone(), sent.clone(), got.clone());
            threads.push(thread::spawn(move || {
                cabinet(relay, seat, peer, rate, st, s, g)
            }));
        }
    }
    let (t1, c1) = (Instant::now(), cpu_ticks(pid));
    thread::sleep(Duration::from_secs(secs));
    let load_cpu = (cpu_ticks(pid) - c1) as f64 / 100.0 / t1.elapsed().as_secs_f64() * 100.0;
    let (load_rss, hwm) = rss(pid);
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().unwrap();
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    let (sent, got) = (sent.load(Ordering::Relaxed), got.load(Ordering::Relaxed));
    println!(
        "{{\"pairs\":{pairs},\"shops\":{shops},\"rate_per_side\":{rate},\"idle_s\":{idle},\"load_s\":{secs},\
         \"idle_rss_kib\":{idle_rss},\"idle_cpu_pct\":{idle_cpu:.3},\"load_rss_kib\":{load_rss},\"vmhwm_kib\":{hwm},\
         \"load_cpu_pct\":{load_cpu:.3},\"sent\":{sent},\"relayed\":{got},\"pongs\":{}}}",
        pongs.load(Ordering::Relaxed)
    );
    assert!(
        got * 10 >= sent * 9,
        "relay lost more than 10 % of {sent} packets"
    );
}
