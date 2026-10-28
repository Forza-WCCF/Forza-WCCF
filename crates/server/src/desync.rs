//! Desync watch (2017-18, logging only): reads the per-frame check values the two clients of a seat pair send each
//! other through the match relay and logs where they first differ, where one side stops sending new frames, and a
//! summary when the pair's traffic ends. It never changes, delays or drops a packet.
//!
//! What a peer packet holds (`client_Release.exe`, read in IDA; little-endian unless marked BE):
//! - 52-byte header (built by `0x4CDCA0`, parsed by `0x4C6270`): magic `F1 C0 80 44 B4 9E DA 3C`, `u16` kind at 8,
//!   `u32` total length at 16. Kind 10 is the frame sync (sent by `0x4C57C0`, read by `0x4C55E0`); 2-9 are
//!   handshake, keep-alive and acks.
//! - kind 10 body (`0x4C54E0`): `u16` sender side, `u16` record count (always 6), `i32` newest sync index, `i32`,
//!   then 6 records of 76 bytes for sync indexes newest-5..newest: `i32` index (0 or stale = no data), 72 data bytes.
//!   Each direction carries only the sender's own values, so the watch needs both directions (it has them: both
//!   seats' packets pass the host's relay).
//! - data (`0x4B14D0`): `i32` counter, channel id byte (5 = GAME_IO), length byte (64), the 64-byte GAME_IO message,
//!   `i16`.
//! - GAME_IO message: an MSB-first bit stream (writer `0x5D1B20`, reader `0x5D1D00`). Bytes 0-3 BE: high half a link
//!   counter, low half the match engine's frame number (`+2480` of its frame); bytes 4-7 BE: the engine's per-frame
//!   random value (`+2484`), the number the client compares with its own (`0x4B41E0`/`0x4B4690`, flag 0x2000).

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];
const HEADER: usize = 52;
const RECORDS: usize = 6;
const RECORD: usize = 76;
/// The whole frame-sync datagram: header, 12-byte body head, 6 records.
pub const SYNC_LEN: usize = HEADER + 12 + RECORDS * RECORD;
const GAME_IO: u8 = 5;

/// Frames kept per direction.
const KEEP: usize = 120;
/// A pause longer than this without a new frame is logged (the client gives up after 30 missing frames, ~0.5 s).
const GAP: Duration = Duration::from_millis(400);
/// Two sides' copies of one frame number are compared only when they arrived this close (a new match restarts the
/// frame numbers).
const SAME_MATCH: Duration = Duration::from_secs(2);
/// A pair with no frame-sync packet for this long has ended: summary, then forgotten.
const ENDED: Duration = Duration::from_secs(3);
/// Gap lines per pair; later gaps only count in the summary.
const MAX_GAP_LINES: u32 = 10;

/// The (frame, value) pairs in one frame-sync datagram, oldest first; `None` for empty records or other channels.
/// `None` overall when `p` is not a frame-sync datagram of the expected layout.
pub fn parse(p: &[u8]) -> Option<[Option<(u16, u32)>; RECORDS]> {
    // shortcut: the exact 2017-18 layout (one 72-byte field, 6 records); a different build logs "unexpected layout"
    if p.len() != SYNC_LEN
        || p[..8] != MAGIC
        || u16le(p, 8) != 10
        || u32le(p, 16) as usize != SYNC_LEN
    {
        return None;
    }
    let b = &p[HEADER..];
    if u16le(b, 2) as usize != RECORDS {
        return None;
    }
    let newest = u32le(b, 4) as i32;
    let mut out = [None; RECORDS];
    for (k, slot) in out.iter_mut().enumerate() {
        let r = &b[12 + k * RECORD..12 + (k + 1) * RECORD];
        let index = u32le(r, 0) as i32;
        let d = &r[4..];
        if index > 0
            && Some(index) == newest.checked_sub((RECORDS - 1 - k) as i32)
            && d[4] == GAME_IO
            && d[5] == 64
        {
            let m = &d[6..];
            *slot = Some((
                u16::from_be_bytes([m[2], m[3]]),
                u32::from_be_bytes([m[4], m[5], m[6], m[7]]),
            ));
        }
    }
    Some(out)
}

fn u16le(p: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([p[at], p[at + 1]])
}

fn u32le(p: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([p[at], p[at + 1], p[at + 2], p[at + 3]])
}

/// Whether `p` starts like a frame-sync datagram (to report a layout the parser does not know, once).
fn looks_like_sync(p: &[u8]) -> bool {
    p.len() >= HEADER && p[..8] == MAGIC && u16le(p, 8) == 10
}

#[derive(Default)]
struct Dir {
    frames: VecDeque<(u16, u32, Instant)>,
    last_new: Option<Instant>,
}

impl Dir {
    fn find(&self, frame: u16, now: Instant) -> Option<(u32, Instant)> {
        self.frames
            .iter()
            .rev()
            .find(|(f, _, t)| *f == frame && now - *t <= SAME_MATCH)
            .map(|(_, v, t)| (*v, *t))
    }
}

struct Pair {
    start: Instant,
    last_packet: Instant,
    /// 0: from the lower seat to the higher, 1: back.
    dirs: [Dir; 2],
    compared: u64,
    mismatches: u64,
    first_mismatch: Option<u16>,
    gaps: u32,
    longest_gap: Option<(Duration, usize, Duration)>,
}

/// Per-pair state of the desync watch. Feed it every relayed data packet (`packet`) and call `tick` from the relay
/// loop; both cost one length check or one empty-map check while no match runs.
#[derive(Default)]
pub struct Watch {
    pairs: BTreeMap<(u8, u8), Pair>,
    odd_reported: bool,
}

impl Watch {
    /// A game packet relayed from seat `from` to seat `to`, received at `now`.
    pub fn packet(&mut self, from: u8, to: u8, p: &[u8], now: Instant, log: &dyn Fn(&str)) {
        if p.len() < HEADER {
            return;
        }
        let Some(records) = parse(p) else {
            if !self.odd_reported && looks_like_sync(p) {
                self.odd_reported = true;
                log(&format!(
                    "desync watch: a frame-sync packet {from}->{to} has an unexpected layout ({} bytes) - not watched",
                    p.len()
                ));
            }
            return;
        };
        let (a, b) = (from.min(to), from.max(to));
        let d = usize::from(from != a);
        let pair = self.pairs.entry((a, b)).or_insert_with(|| {
            log(&format!("desync watch: seats {a}-{b}"));
            Pair {
                start: now,
                last_packet: now,
                dirs: Default::default(),
                compared: 0,
                mismatches: 0,
                first_mismatch: None,
                gaps: 0,
                longest_gap: None,
            }
        });
        pair.last_packet = now;
        for (frame, value) in records.into_iter().flatten() {
            if pair.dirs[d].find(frame, now).is_some() {
                continue; // resent: each packet repeats the last 6 records
            }
            if let Some(last) = pair.dirs[d].last_new
                && now - last > GAP
            {
                let (len, at) = (now - last, last - pair.start);
                pair.gaps += 1;
                if pair.longest_gap.is_none_or(|(l, _, _)| len > l) {
                    pair.longest_gap = Some((len, d, at));
                }
                if pair.gaps <= MAX_GAP_LINES {
                    log(&format!(
                        "desync watch: seats {a}-{b} gap: {from}->{to} no new frame for {:.2} s from +{:.2} s{}",
                        len.as_secs_f64(),
                        at.as_secs_f64(),
                        if pair.gaps == MAX_GAP_LINES {
                            " (more gaps only in the summary)"
                        } else {
                            ""
                        }
                    ));
                }
            }
            pair.dirs[d].last_new = Some(now);
            if let Some((other, _)) = pair.dirs[1 - d].find(frame, now) {
                pair.compared += 1;
                if other != value {
                    pair.mismatches += 1;
                    if pair.first_mismatch.is_none() {
                        pair.first_mismatch = Some(frame);
                        log(&format!(
                            "desync watch: seats {a}-{b} frame {frame} differs: seat {from} {value:#010x}, seat {to} \
                             {other:#010x} (+{:.2} s)",
                            (now - pair.start).as_secs_f64()
                        ));
                    }
                }
            }
            let dir = &mut pair.dirs[d];
            if dir.frames.len() == KEEP {
                dir.frames.pop_front();
            }
            dir.frames.push_back((frame, value, now));
        }
    }

    /// Ends pairs whose traffic stopped `ENDED` ago, with a summary line each.
    pub fn tick(&mut self, now: Instant, log: &dyn Fn(&str)) {
        if self.pairs.is_empty() {
            return;
        }
        self.pairs.retain(|&(a, b), p| {
            if now - p.last_packet <= ENDED {
                return true;
            }
            let name = |d: usize| if d == 0 { format!("{a}->{b}") } else { format!("{b}->{a}") };
            let mism = match p.first_mismatch {
                Some(f) => format!("{} mismatches (first at frame {f})", p.mismatches),
                None => "0 mismatches".into(),
            };
            let gap = match p.longest_gap {
                Some((len, d, at)) => format!(
                    "longest gap {:.2} s ({} at +{:.2} s, {} gaps)",
                    len.as_secs_f64(),
                    name(d),
                    at.as_secs_f64(),
                    p.gaps
                ),
                None => "no gap over 0.4 s".into(),
            };
            let last = |d: usize| match p.dirs[d].last_new {
                Some(t) => format!("{} +{:.2} s", name(d), (t - p.start).as_secs_f64()),
                None => format!("{} none", name(d)),
            };
            log(&format!(
                "desync watch: seats {a}-{b} ended after {:.1} s: {} frames compared, {mism}, {gap}; last new frame {}, {}",
                (p.last_packet - p.start).as_secs_f64(),
                p.compared,
                last(0),
                last(1)
            ));
            false
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A frame-sync datagram with `newest` as the newest sync index and the given (frame, value) per record.
    pub(crate) fn sync_packet(newest: i32, frames: [Option<(u16, u32)>; RECORDS]) -> Vec<u8> {
        let mut p = vec![0u8; SYNC_LEN];
        p[..8].copy_from_slice(&MAGIC);
        p[8..10].copy_from_slice(&10u16.to_le_bytes());
        p[16..20].copy_from_slice(&(SYNC_LEN as u32).to_le_bytes());
        let b = &mut p[HEADER..];
        b[2..4].copy_from_slice(&(RECORDS as u16).to_le_bytes());
        b[4..8].copy_from_slice(&newest.to_le_bytes());
        for (k, f) in frames.iter().enumerate() {
            let r = &mut b[12 + k * RECORD..12 + (k + 1) * RECORD];
            r[..4].copy_from_slice(&(newest - (RECORDS - 1 - k) as i32).to_le_bytes());
            if let Some((frame, value)) = f {
                r[8] = GAME_IO;
                r[9] = 64;
                r[10..12].copy_from_slice(&7u16.to_be_bytes()); // link counter, ignored
                r[12..14].copy_from_slice(&frame.to_be_bytes());
                r[14..18].copy_from_slice(&value.to_be_bytes());
            }
        }
        p
    }

    /// The packet a side sends at frame `f`: frames f-5..=f, value from `value`.
    fn at(f: u16, value: impl Fn(u16) -> u32) -> Vec<u8> {
        let mut fr = [None; RECORDS];
        for (k, s) in fr.iter_mut().enumerate() {
            let g = f + k as u16;
            *s = (g >= 5).then(|| (g - 5, value(g - 5)));
        }
        sync_packet(i32::from(f) + 1, fr)
    }

    #[test]
    fn parses_a_sync_packet() {
        let p = sync_packet(
            10,
            [
                None,
                Some((100, 1)),
                None,
                None,
                Some((103, 0xdead_beef)),
                Some((104, 5)),
            ],
        );
        assert_eq!(
            parse(&p).unwrap(),
            [
                None,
                Some((100, 1)),
                None,
                None,
                Some((103, 0xdead_beef)),
                Some((104, 5))
            ]
        );
    }

    #[test]
    fn rejects_short_garbage_and_odd_records() {
        let p = sync_packet(10, [Some((1, 1)); RECORDS]);
        assert!(parse(&p[..SYNC_LEN - 1]).is_none());
        assert!(parse(&[]).is_none());
        assert!(parse(&[0x55; SYNC_LEN]).is_none());
        let mut q = p.clone();
        q[8] = 4; // an ack, not frame sync
        assert!(parse(&q).is_none());
        let mut q = p.clone();
        q[16] ^= 1; // length field disagrees
        assert!(parse(&q).is_none());
        // a stale record (index not newest-5+k), another channel, a zero index: skipped, the rest kept
        let mut q = p.clone();
        q[HEADER + 12] = 99;
        q[HEADER + 12 + RECORD + 8] = 6;
        q[HEADER + 12 + 2 * RECORD..HEADER + 12 + 2 * RECORD + 4].fill(0);
        let r = parse(&q).unwrap();
        assert_eq!(r[..3], [None, None, None]);
        assert_eq!(r[3], Some((1, 1)));
        // early in a match the oldest indexes are below 1: no data
        let early = sync_packet(3, [Some((1, 1)); RECORDS]);
        assert_eq!(parse(&early).unwrap().iter().flatten().count(), 3);
    }

    /// Two seats sending frames 16 ms apart, a packet every 4 frames up to `frames`, seat 2's values from `v2`; with
    /// `pause` = (frame, length), seat 2's packets after that frame arrive `length` late. Returns the log.
    fn run(frames: u16, v2: impl Fn(u16) -> u32, pause: Option<(u16, Duration)>) -> Vec<String> {
        let lines = RefCell::new(Vec::new());
        let log = |s: &str| lines.borrow_mut().push(s.to_string());
        let mut w = Watch::default();
        let t0 = Instant::now();
        let mut late = Duration::ZERO;
        let v1 = |f: u16| u32::from(f).wrapping_mul(2_654_435_761);
        for f in (0..frames).step_by(4) {
            let now = t0 + Duration::from_millis(16 * u64::from(f));
            w.packet(1, 2, &at(f, v1), now, &log);
            w.packet(2, 1, &at(f, &v2), now + late, &log);
            if let Some((pf, len)) = pause
                && f == pf
            {
                late = len; // seat 2's packets after this one arrive `len` late
            }
            w.tick(now, &log);
        }
        w.tick(t0 + Duration::from_secs(60), &log);
        lines.into_inner()
    }

    #[test]
    fn agreeing_streams_log_only_the_start_and_summary() {
        let v = |f: u16| u32::from(f).wrapping_mul(2_654_435_761);
        let l = run(600, v, None);
        assert_eq!(l.len(), 2, "{l:?}");
        assert_eq!(l[0], "desync watch: seats 1-2");
        assert!(
            l[1].starts_with("desync watch: seats 1-2 ended after"),
            "{l:?}"
        );
        assert!(
            l[1].contains("597 frames compared, 0 mismatches, no gap over 0.4 s"),
            "{l:?}"
        );
    }

    #[test]
    fn diverging_streams_log_the_first_differing_frame_once() {
        let v = |f: u16| {
            let x = u32::from(f).wrapping_mul(2_654_435_761);
            if f >= 300 { x ^ 1 } else { x }
        };
        let l = run(600, v, None);
        let diff: Vec<_> = l.iter().filter(|s| s.contains("differs")).collect();
        assert_eq!(diff.len(), 1, "{l:?}");
        assert!(
            diff[0].starts_with("desync watch: seats 1-2 frame 300 differs: seat 2 0x"),
            "{l:?}"
        );
        assert!(
            l.last()
                .unwrap()
                .contains("297 mismatches (first at frame 300)"),
            "{l:?}"
        );
    }

    #[test]
    fn a_pause_logs_a_gap_with_direction_and_length() {
        let v = |f: u16| u32::from(f).wrapping_mul(2_654_435_761);
        let l = run(600, v, Some((200, Duration::from_millis(600))));
        let gaps: Vec<_> = l.iter().filter(|s| s.contains(" gap: ")).collect();
        assert_eq!(gaps.len(), 1, "{l:?}");
        // seat 2's last new frame came at frame 200 (3.2 s); the next 0.6 s + one packet interval later
        assert!(
            gaps[0].starts_with(
                "desync watch: seats 1-2 gap: 2->1 no new frame for 0.66 s from +3.20 s"
            ),
            "{l:?}"
        );
        assert!(
            l.last()
                .unwrap()
                .contains("longest gap 0.66 s (2->1 at +3.20 s, 1 gaps)"),
            "{l:?}"
        );
        assert!(l.last().unwrap().contains("0 mismatches"), "{l:?}");
    }

    #[test]
    fn no_traffic_costs_nothing_and_other_packets_are_ignored() {
        let lines = RefCell::new(Vec::<String>::new());
        let log = |s: &str| lines.borrow_mut().push(s.to_string());
        let mut w = Watch::default();
        let now = Instant::now();
        w.tick(now, &log);
        w.packet(1, 2, b"hello", now, &log);
        w.packet(1, 2, &[0u8; 600], now, &log);
        assert!(w.pairs.is_empty() && lines.borrow().is_empty());
        // a frame-sync header with a body the parser does not know: one line, once
        let mut odd = sync_packet(10, [Some((1, 1)); RECORDS]);
        odd.extend_from_slice(&[0; 76]);
        w.packet(1, 2, &odd, now, &log);
        w.packet(1, 2, &odd, now, &log);
        assert_eq!(lines.borrow().len(), 1);
        assert!(lines.borrow()[0].contains("unexpected layout (596 bytes)"));
    }
}
