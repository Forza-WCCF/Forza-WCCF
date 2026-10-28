//! The match relay (`relay.py`, and 2017-18's `_relay.py`): passes each cabinet's game packets to the other seat over
//! UDP, so every player only talks to the server. Packets (binary, magic `WRL1`):
//! `R seat` (I am seat n), `D to from payload` (a game packet), `P token` (a ping, answered `Q token`); the relay
//! sends `F from ip(4) payload`. A seat not heard from for `forget` is forgotten. A live seat belongs to the address
//! that holds it (2010-11, A6): a claim from another address is rejected until the owner is quiet for `forget`.
//! 2017-18's relay has no such rule.
//!
//! Test switch (never on by default): with `FORZA_RELAY_STALL_FILE` set, a number of milliseconds written to that file
//! makes the relay drop every game packet (`D`) for that long, once; the file is removed when read. A one-PC stand-in
//! for a network stall in the middle of a match (`E:\wccf-builds\stall-tolerance-20261028\LIVE.md`).

use crate::{Edition, Handle, Log};
use std::collections::BTreeMap;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const MAGIC: &[u8; 4] = b"WRL1";

#[derive(Clone, Debug)]
pub struct RelayConfig {
    pub edition: Edition,
    /// `WCCF_BIND`, default `0.0.0.0`.
    pub bind: String,
    /// `WCCF_RELAY_PORT`, default 20040; 0 = any (tests).
    pub port: u16,
    /// `WCCF_RELAY_FORGET`, default 30 s.
    pub forget: Duration,
    /// How often it logs what passed and forgets quiet seats (the Python: 30 s).
    pub report: Duration,
    /// `FORZA_RELAY_STALL_FILE` (tests only): milliseconds written there drop all game packets that long.
    pub stall_file: Option<PathBuf>,
}

impl RelayConfig {
    pub fn new(edition: Edition) -> Self {
        Self {
            edition,
            bind: "0.0.0.0".into(),
            port: 20040,
            forget: Duration::from_secs(30),
            report: Duration::from_secs(30),
            stall_file: None,
        }
    }
}

struct Relay {
    cfg: RelayConfig,
    log: Log,
    seats: BTreeMap<u8, (SocketAddr, Instant)>,
    rejects: BTreeMap<u8, u32>,
}

impl Relay {
    /// Records `addr` as the owner of `seat` unless a different address holds it and was heard within `forget`.
    fn claim(&mut self, seat: u8, addr: SocketAddr, now: Instant) -> bool {
        let known = self.seats.get(&seat).copied();
        if self.cfg.edition == Edition::E1011
            && let Some((a, t)) = known
            && a != addr
            && now - t <= self.cfg.forget
        {
            *self.rejects.entry(seat).or_default() += 1;
            return false;
        }
        if known.is_none_or(|(a, _)| a != addr) {
            (self.log)(&format!("seat {seat} is at {}:{}", addr.ip(), addr.port()));
        }
        self.seats.insert(seat, (addr, now));
        true
    }
}

fn run(cfg: RelayConfig, log: Log, sock: UdpSocket, stop: Arc<AtomicBool>) {
    let mut r = Relay {
        cfg,
        log,
        seats: BTreeMap::new(),
        rejects: BTreeMap::new(),
    };
    let (mut counts, mut drops) = (BTreeMap::<(u8, u8), u32>::new(), BTreeMap::<u8, u32>::new());
    let mut next_report = Instant::now() + r.cfg.report;
    let (mut stall_until, mut stall_look) = (Instant::now(), Instant::now());
    // no per-datagram allocation: receive and build the outgoing packet in these two stack buffers
    let (mut buf, mut out) = ([0u8; 4200], [0u8; 4208]);
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= next_report {
            if !(counts.is_empty() && drops.is_empty() && r.rejects.is_empty()) {
                let passed = counts
                    .iter()
                    .map(|((a, b), n)| format!("{a}->{b} {n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let dropped = drops
                    .iter()
                    .map(|(b, n)| format!("->{b} {n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let rejected = r
                    .rejects
                    .iter()
                    .map(|(b, n)| format!("seat {b} {n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                (r.log)(&format!(
                    "last 30 s: {}{}{}",
                    if passed.is_empty() {
                        "nothing passed"
                    } else {
                        &passed
                    },
                    if drops.is_empty() {
                        String::new()
                    } else {
                        format!("; dropped (seat not known): {dropped}")
                    },
                    if r.rejects.is_empty() {
                        String::new()
                    } else {
                        format!("; rejected (seat held by another address): {rejected}")
                    }
                ));
            }
            (counts, drops, r.rejects) = Default::default();
            next_report = now + r.cfg.report;
            let quiet: Vec<u8> = r
                .seats
                .iter()
                .filter(|(_, (_, t))| now - *t > r.cfg.forget)
                .map(|(s, _)| *s)
                .collect();
            for seat in quiet {
                let (a, _) = r.seats.remove(&seat).unwrap();
                (r.log)(&format!(
                    "seat {seat} ({}:{}) not heard from for {} s - forgotten",
                    a.ip(),
                    a.port(),
                    r.cfg.forget.as_secs()
                ));
            }
        }
        // errors (a read timeout, or on Windows a "port unreachable" echo or an oversize datagram) are ignored
        let Ok((n, addr)) = sock.recv_from(&mut buf) else {
            continue;
        };
        let (data, now) = (&buf[..n], Instant::now());
        if let Some(f) = r.cfg.stall_file.as_ref().filter(|_| now >= stall_look) {
            stall_look = now + Duration::from_millis(100);
            if let Some(ms) = take_stall(f) {
                stall_until = now + Duration::from_millis(ms);
                (r.log)(&format!(
                    "test stall: dropping every game packet for {ms} ms"
                ));
            }
        }
        if n < 6 || &data[..4] != MAGIC {
            continue;
        }
        match data[4] {
            b'P' => {
                // a ping from a cabinet's meter: its bytes back, as "Q"
                if n <= 64 {
                    out[..4].copy_from_slice(MAGIC);
                    out[4] = b'Q';
                    out[5..n].copy_from_slice(&data[5..]);
                    let _ = sock.send_to(&out[..n], addr);
                }
            }
            b'R' if (1..=8).contains(&data[5]) => {
                r.claim(data[5], addr, now);
            }
            b'D' if n >= 7 && (1..=8).contains(&data[5]) && (1..=8).contains(&data[6]) => {
                let (to, from) = (data[5], data[6]);
                if now < stall_until {
                    continue; // the test stall
                }
                if !r.claim(from, addr, now) {
                    continue; // not this seat's owner: drop the packet
                }
                let target = r.seats.get(&to).filter(|(_, t)| now - *t <= r.cfg.forget);
                let (Some((target, _)), SocketAddr::V4(src)) = (target, addr) else {
                    *drops.entry(to).or_default() += 1;
                    continue;
                };
                out[..4].copy_from_slice(MAGIC);
                out[4..6].copy_from_slice(&[b'F', from]);
                out[6..10].copy_from_slice(&src.ip().octets());
                out[10..n + 3].copy_from_slice(&data[7..]);
                match sock.send_to(&out[..n + 3], target) {
                    Ok(_) => *counts.entry((from, to)).or_default() += 1,
                    Err(_) => *drops.entry(to).or_default() += 1,
                }
            }
            _ => {}
        }
    }
    (r.log)("match relay: its time is up - stopping");
}

/// The test stall's milliseconds from `f`, which is removed; `None` while there is no file or no number in it yet.
fn take_stall(f: &Path) -> Option<u64> {
    let ms = std::fs::read_to_string(f).ok()?.trim().parse().ok()?;
    let _ = std::fs::remove_file(f);
    Some(ms)
}

/// Starts the relay. Err(io error) when the port cannot be bound.
pub fn start(cfg: RelayConfig, log: Log) -> io::Result<Handle> {
    let sock = UdpSocket::bind((cfg.bind.as_str(), cfg.port))?;
    sock.set_read_timeout(Some(Duration::from_millis(200)))?;
    let addr = sock.local_addr()?;
    log(&format!("match relay listening on UDP {}", addr.port()));
    let stop = Arc::new(AtomicBool::new(false));
    let t = thread::spawn({
        let stop = stop.clone();
        move || run(cfg, log, sock, stop)
    });
    Ok(Handle::new(addr, stop, false, vec![t], None))
}
