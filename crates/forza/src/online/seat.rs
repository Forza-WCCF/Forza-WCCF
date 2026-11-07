//! This PC's seat on a server (port of `online/seat.py`): asks the server's seat desk for a seat, then holds it by
//! pinging every 20 s for as long as the run lasts. If the connection breaks it reconnects and re-claims the same
//! seat (A5). Dropping the [`Lease`] ends the lease and the seat is free again at once.

use super::{Reply, Want};
use crate::service::{Handle, Log};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// The Python's timers; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Connecting to the desk.
    pub connect: Duration,
    /// Waiting for the desk's answer to a `WANT`.
    pub answer: Duration,
    /// Waiting for a `PONG`.
    pub pong: Duration,
    /// Between pings (the desk waits 75 s).
    pub ping: Duration,
    /// Between reclaim tries after a break.
    pub retry: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(6),
            answer: Duration::from_secs(10),
            pong: Duration::from_secs(15),
            ping: Duration::from_secs(20),
            retry: Duration::from_secs(3),
        }
    }
}

/// Why no seat was given. `as_str` is the word `seat.py` wrote in its `error` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Nothing answered on the port: a server without a seat desk (the run then plays as before).
    NoBroker,
    /// The desk was reached but did not reply in time (the run stops rather than risk two PCs on one seat).
    NoAnswer,
    Full,
    Taken,
    Refused,
}

impl Fault {
    pub fn as_str(self) -> &'static str {
        match self {
            Fault::NoBroker => "no broker",
            Fault::NoAnswer => "no answer",
            Fault::Full => "full",
            Fault::Taken => "taken",
            Fault::Refused => "refused",
        }
    }
}

/// A refused claim: the kind, and the detail `seat.py` put in its file (the desk's line, or the I/O error text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatError {
    pub fault: Fault,
    pub detail: String,
}

impl SeatError {
    /// `(seat, projector)` to play with when the server has no seat desk (`play._seat_decision`): seat 1 with the
    /// projector, or the seat asked for without it. Every other fault is `None` and the run must stop, so two PCs
    /// never land on seat 1 (A4); the caller words the stop message from the fault and detail.
    pub fn no_desk_seat(&self, asked: Option<u8>) -> Option<(u8, bool)> {
        let seat = asked.filter(|&s| s != 0).unwrap_or(1);
        (self.fault == Fault::NoBroker).then_some((seat, seat == 1))
    }
}

/// A seat held on the desk until this is dropped (or `life` ends).
pub struct Lease {
    pub seat: u8,
    pub projector: bool,
    _handle: Handle,
}

/// The desk's answer is one line of a few bytes (`SEAT n PROJECTOR p`, `FULL`, `TAKEN n`, `ERR ...`): a desk that
/// sends more than this without a line end is not answered, it is broken.
const MAX_ANSWER: usize = 4096;

/// Sends one `WANT` line and reads the desk's reply line.
fn ask(conn: &mut TcpStream, line: &str, timeout: Duration) -> io::Result<String> {
    conn.set_read_timeout(Some(timeout))?;
    conn.write_all(line.as_bytes())?;
    let mut line = Vec::new();
    let mut b = [0u8; 256];
    while !line.contains(&b'\n') {
        match conn.read(&mut b)? {
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the broker closed the connection",
                ));
            }
            n => {
                line.extend_from_slice(&b[..n]);
                // the timeout is per read: without this, a desk that keeps sending holds the Launcher forever
                if line.len() > MAX_ANSWER {
                    return Err(io::Error::other("the broker's answer is too long"));
                }
            }
        }
    }
    let end = line.iter().position(|&c| c == b'\n').unwrap_or(line.len());
    Ok(String::from_utf8_lossy(&line[..end]).into_owned())
}

/// The `WANT` line, with `TICKET t` when the lobby reserved the seat (`RESERVE`, the desk's lobby words) and, last,
/// `DMATH d`: this PC's match engine state ([`dmath_token`]).
fn want_line(want: Want, ticket: Option<&str>, dmath: Option<&str>) -> String {
    let mut line = want.line().trim_end().to_owned();
    if let Some(t) = ticket {
        line += &format!(" TICKET {t}");
    }
    if let Some(d) = dmath.filter(|d| *d != NO_DMATH) {
        line += &format!(" DMATH {d}");
    }
    line + "\n"
}

/// The det-math token of a PC that sends none (an older build, the 2010-11 edition, a 2017-18 runtime without
/// dmath.dll): its engine runs the original math.
pub const NO_DMATH: &str = "none";

/// This 2017-18 runtime's match engine math, as the seat desk compares it between PCs (an engine with dmath.dll
/// desyncs against one without, or with another dmath.dll, from the first differing frame): [`NO_DMATH`] without
/// `game\dmath.dll`; else the first 16 hex digits of its SHA-256, with `-off` when the last engine start in the newer
/// `dmath.log` (game or seat folder, wherever the engine ran) did not patch all sites and hook atan2, `-tst` when it
/// ran a test mode of dmath.cfg (`foreign`, `frames`).
/// shortcut: no dmath.log yet (no match played since setup) counts as patched, since setup pinned the engine; a strict
/// check would refuse every first online match.
pub fn dmath_token(game: &Path, seat: &Path) -> String {
    use sha2::{Digest, Sha256};
    let Ok(dll) = std::fs::read(game.join("dmath.dll")) else {
        return NO_DMATH.into();
    };
    let hash: String = Sha256::digest(&dll)[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let newest = [game, seat]
        .iter()
        .map(|d| d.join("dmath.log"))
        .filter_map(|p| Some((std::fs::metadata(&p).and_then(|m| m.modified()).ok()?, p)))
        .max_by_key(|(t, _)| *t);
    let Some(text) = newest.and_then(|(_, p)| std::fs::read(p).ok()) else {
        return hash;
    };
    let text = String::from_utf8_lossy(&text);
    // engine starts only: the frame-log and its error lines also begin "det-math". Each record carries the DLL's
    // "yyyy-mm-dd hh:mm:ss pid=N " prefix (native/detmath/src/dll.rs `log`), and a record cut short by a kill runs
    // into the next one on the same line: every " pid=N " starts a record. The text before the first is the date, or
    // the whole line from the first dmath.dll, which wrote no prefix.
    let last = text
        .lines()
        .rev()
        .flat_map(|l| {
            let mut records: Vec<&str> = l.split(" pid=").collect();
            for r in &mut records[1..] {
                *r = r.split_once(' ').map_or(*r, |(_, m)| m);
            }
            records.into_iter().rev()
        })
        .find(|l| {
            ["det-math NOT applied", "det-math off"]
                .iter()
                .any(|s| l.starts_with(s))
                || l.starts_with("det-math ") && l.contains("/") && l.contains("atan2")
        });
    match last {
        // the DLL writes "det-math n/n" only when every site of its table was patched (140 today)
        Some(l) if !l.contains(", atan2 hooked") || l.starts_with("det-math NOT") => {
            format!("{hash}-off")
        }
        // only the normal mode ends at the CW value: foreign, frames or any later suffix must not pair with it (4
        // letters, as `-off`: the vslink hello holds 20, vs::HELLO_DMATH)
        Some(l)
            if l.rsplit_once(", CW ")
                .is_none_or(|(_, cw)| cw.contains([' ', ','])) =>
        {
            format!("{hash}-tst")
        }
        _ => hash,
    }
}

/// The first token in `held` (the desk's leases) that differs from `mine`: the claim must be refused, its engine would
/// desync against that seat's.
pub fn dmath_clash<'a>(mine: &str, held: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    held.into_iter().find(|h| *h != mine)
}

fn connect(addr: SocketAddr, t: &Timing, line: &str) -> io::Result<(TcpStream, String)> {
    let mut c = TcpStream::connect_timeout(&addr, t.connect)?;
    let line = ask(&mut c, line, t.answer)?;
    Ok((c, line))
}

/// Sleeps up to `d`, in short steps; false when `stop` is set or `end` passes first.
fn sleep_until(d: Duration, end: Instant, stop: &AtomicBool) -> bool {
    let until = Instant::now() + d;
    while Instant::now() < until {
        if stop.load(Ordering::Relaxed) || Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(20).min(until - Instant::now()));
    }
    !stop.load(Ordering::Relaxed) && Instant::now() < end
}

/// Pings every `t.ping` until the connection breaks, `end` passes or `stop` is set.
fn hold(conn: &mut TcpStream, t: &Timing, end: Instant, stop: &AtomicBool) {
    while sleep_until(t.ping, end, stop) {
        let _ = conn.set_read_timeout(Some(t.pong));
        if conn.write_all(b"PING\n").is_err() {
            return;
        }
        let mut b = [0u8; 64];
        match conn.read(&mut b) {
            Ok(n) if b[..n].windows(4).any(|w| w == b"PONG") => {}
            _ => return,
        }
    }
}

/// Claims a seat on the desk at `addr` (port [`super::SEAT_PORT`]) and returns once the desk has answered; the lease then
/// holds the seat on its own thread for `life`. `want` is the seat asked for, or `None` for the lowest free one.
/// Blocks for at most `t.connect + t.answer`; call it from a worker thread.
pub fn claim(
    addr: SocketAddr,
    want: Option<u8>,
    life: Duration,
    t: Timing,
    log: Log,
) -> Result<Lease, SeatError> {
    claim_with(addr, want, None, None, life, t, log)
}

/// [`claim`] for a seat the lobby reserved: `WANT ... TICKET t`. The desk spends the ticket on the first claim, so
/// a reclaim after a break asks for the seat by number, as before. `dmath`: this PC's [`dmath_token`], on every claim.
pub fn claim_with(
    addr: SocketAddr,
    want: Option<u8>,
    ticket: Option<&str>,
    dmath: Option<&str>,
    life: Duration,
    t: Timing,
    log: Log,
) -> Result<Lease, SeatError> {
    let end = Instant::now() + life;
    let first = want.map_or(Want::Any, Want::Seat);
    let err = |fault, detail: String| SeatError { fault, detail };
    let mut conn = match TcpStream::connect_timeout(&addr, t.connect) {
        Ok(c) => c,
        Err(e) => return Err(err(Fault::NoBroker, e.to_string())),
    };
    let line = ask(&mut conn, &want_line(first, ticket, dmath), t.answer)
        .map_err(|e| err(Fault::NoAnswer, e.to_string()))?;
    let (seat, projector) = match Reply::parse(&line) {
        Reply::Seat { seat, projector } => (seat, projector),
        r => {
            let fault = match r {
                Reply::Full => Fault::Full,
                Reply::Taken(_) => Fault::Taken,
                _ => Fault::Refused,
            };
            return Err(err(
                fault,
                line.split_whitespace().collect::<Vec<_>>().join(" "),
            ));
        }
    };
    log(&format!(
        "seat {seat} held (projector {})",
        u8::from(projector)
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let again = want_line(Want::Seat(seat), None, dmath);
    let thread = thread::spawn(move || {
        let mut conn = conn;
        // Holds the seat for the life of the run; reconnects and reclaims the SAME seat after any break (A5).
        while Instant::now() < end && !flag.load(Ordering::Relaxed) {
            hold(&mut conn, &t, end, &flag);
            let _ = conn.shutdown(Shutdown::Both);
            if Instant::now() >= end || flag.load(Ordering::Relaxed) {
                break;
            }
            log(&format!(
                "the seat desk connection broke - reclaiming seat {seat}"
            ));
            // retry through a blip, a desk restart, or the old lease not yet reaped (TAKEN until the desk frees it)
            let mut next = None;
            while next.is_none() && Instant::now() < end && !flag.load(Ordering::Relaxed) {
                match connect(addr, &t, &again) {
                    Ok((c, line))
                        if line.starts_with("SEAT ") && line.split_whitespace().count() == 4 =>
                    {
                        log(&format!("reclaimed seat {seat}"));
                        next = Some(c);
                    }
                    _ => {
                        sleep_until(t.retry, end, &flag);
                    }
                }
            }
            match next {
                Some(c) => conn = c,
                None => break,
            }
        }
        let _ = conn.shutdown(Shutdown::Both);
    });
    Ok(Lease {
        seat,
        projector,
        _handle: Handle::new(stop, thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(fault: Fault) -> SeatError {
        SeatError {
            fault,
            detail: "x".into(),
        }
    }

    /// `test_take_seat.py`: only "no broker" falls back to a seat; full, taken, refused and no answer stop.
    #[test]
    fn ticket_rides_on_the_want_line() {
        assert_eq!(
            want_line(Want::Any, Some("ab12"), None),
            "WANT TICKET ab12\n"
        );
        assert_eq!(
            want_line(Want::Seat(3), Some("ab12"), Some("00ff")),
            "WANT SEAT 3 TICKET ab12 DMATH 00ff\n"
        );
        assert_eq!(want_line(Want::Any, None, Some(NO_DMATH)), "WANT\n");
    }

    #[test]
    fn dmath_tokens_must_match() {
        assert_eq!(dmath_clash("ab", ["ab", "ab"]), None);
        assert_eq!(dmath_clash("ab", []), None);
        assert_eq!(dmath_clash("ab", ["ab", "ab-off"]), Some("ab-off"));
        assert_eq!(dmath_clash(NO_DMATH, ["ab"]), Some("ab"));
    }

    /// The token: none without dmath.dll, the DLL's hash when the last engine start patched everything, `-off` when
    /// it did not.
    #[test]
    fn dmath_token_reads_the_dll_and_the_last_engine_start() {
        let root = std::env::temp_dir().join(format!("forza_dmath_token_{}", std::process::id()));
        let (game, seat) = (root.join("game"), root.join("seat1"));
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(&seat).unwrap();
        assert_eq!(dmath_token(&game, &seat), NO_DMATH);
        std::fs::write(game.join("dmath.dll"), b"dll").unwrap();
        let patched = dmath_token(&game, &seat);
        assert_eq!(patched.len(), 16, "{patched}");
        // the shape of native/detmath/src/dll.rs `log`: "{date} {time} pid={pid} {text}\r\n"
        let at = |text: &str| format!("2026-11-06 22:44:17 pid=21684 {text}\r\n");
        let ok = at("det-math 140/140, atan2 hooked, MXCSR 0x1F80, CW 0x027F");
        std::fs::write(
            seat.join("dmath.log"),
            format!(
                "{ok}{}",
                at("det-math frame log on the sim thread: MXCSR 0x1F80")
            ),
        )
        .unwrap();
        assert_eq!(dmath_token(&game, &seat), patched);
        let off = format!("{patched}-off");
        for bad in [
            "det-math NOT applied, nothing patched: not the pinned original",
            "det-math off (dmath.cfg): nothing patched",
            "det-math 140/140, atan2 NOT hooked, MXCSR 0x1F80, CW 0x027F",
        ] {
            std::fs::write(seat.join("dmath.log"), format!("{ok}{}", at(bad))).unwrap();
            assert_eq!(dmath_token(&game, &seat), off, "{bad}");
        }
        // a later good start counts again
        std::fs::write(
            seat.join("dmath.log"),
            format!("{}{ok}", at("det-math NOT applied, nothing patched: x")),
        )
        .unwrap();
        assert_eq!(dmath_token(&game, &seat), patched);
        std::fs::write(game.join("dmath.dll"), b"another dll").unwrap();
        assert_ne!(dmath_token(&game, &seat), patched);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The token for a seat dmath.log holding `log`, and the plain hash of the same DLL.
    fn token_with_log(name: &str, log: &str) -> (String, String) {
        let root = std::env::temp_dir().join(format!("forza_dmath_{name}_{}", std::process::id()));
        let (game, seat) = (root.join("game"), root.join("seat1"));
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(&seat).unwrap();
        std::fs::write(game.join("dmath.dll"), b"dll").unwrap();
        let hash = dmath_token(&game, &seat);
        std::fs::write(seat.join("dmath.log"), log).unwrap();
        let token = dmath_token(&game, &seat);
        let _ = std::fs::remove_dir_all(&root);
        (token, hash)
    }

    const DMATH_OK: &str = "det-math 140/140, atan2 hooked, MXCSR 0x1F80, CW 0x027F";

    /// dmath.cfg `foreign` and `frames` (native/detmath/src/dll.rs `patch`) never pair with the normal mode.
    #[test]
    fn dmath_test_modes_never_pair_with_the_normal_hash() {
        for (i, suffix) in [
            ", FOREIGN estimate (test)",
            ", frame log on",
            ", FOREIGN estimate (test), frame log on",
            ", something later",
        ]
        .iter()
        .enumerate()
        {
            let log = format!("2026-11-06 22:44:17 pid=21684 {DMATH_OK}{suffix}\r\n");
            let (token, hash) = token_with_log(&format!("mode{i}"), &log);
            assert_eq!(token, format!("{hash}-tst"), "{suffix}");
        }
    }

    /// A start line cut short by a kill, with the next start's line written onto it, reads as that next start.
    #[test]
    fn dmath_truncated_line_reads_as_the_start_written_onto_it() {
        let cut = "2026-11-06 22:44:17 pid=21684 det-math 140/140, atan2 hook";
        let ok = format!("2026-11-06 22:40:00 pid=1 {DMATH_OK}\r\n");
        let off = "2026-11-07 10:00:00 pid=999 det-math off (dmath.cfg): nothing patched\r\n";
        let (token, hash) = token_with_log("cut_off", &format!("{ok}{cut}{off}"));
        assert_eq!(token, format!("{hash}-off"));
        // cut short and followed by a frame-log line: the cut start is the newest, and it is not a whole patch line
        let frame =
            "2026-11-07 10:00:00 pid=21684 det-math frame log on the sim thread: MXCSR 0x1F80\r\n";
        let (token, hash) = token_with_log("cut_frame", &format!("{ok}{cut}{frame}"));
        assert_eq!(token, format!("{hash}-off"));
        let (token, hash) = token_with_log("cut_ok", &format!("{cut}{ok}"));
        assert_eq!(token, hash);
    }

    /// The first dmath.dll (before commit 2c39a13e) wrote its lines with no date and pid prefix.
    #[test]
    fn dmath_reads_a_line_without_the_prefix() {
        let (token, hash) = token_with_log("bare_ok", &format!("{DMATH_OK}\r\n"));
        assert_eq!(token, hash);
        let log = format!("{DMATH_OK}\r\ndet-math off (dmath.cfg): nothing patched\r\n");
        let (token, hash) = token_with_log("bare_off", &log);
        assert_eq!(token, format!("{hash}-off"));
    }

    /// A desk that keeps sending without a line end is refused at once, not read until its own timeout (and not grown
    /// in memory): the words are a line, and this is the first call of an online start.
    #[test]
    fn an_endless_answer_is_refused() {
        let desk = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let at = desk.local_addr().unwrap();
        std::thread::spawn(move || {
            let Ok((mut c, _)) = desk.accept() else {
                return;
            };
            let junk = [b'a'; 4096];
            for _ in 0..256 {
                // 1 MB with no line end, then the socket stays open
                if c.write_all(&junk).is_err() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(30));
        });
        let t = Timing {
            answer: Duration::from_secs(10),
            ..Timing::default()
        };
        let started = Instant::now();
        let fault = match claim(at, None, Duration::from_secs(60), t, Arc::new(|_| {})) {
            Ok(lease) => panic!("a seat {} from a desk that sent no line", lease.seat),
            Err(e) => e.fault,
        };
        assert_eq!(fault, Fault::NoAnswer);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn only_no_broker_falls_back() {
        assert_eq!(err(Fault::NoBroker).no_desk_seat(None), Some((1, true)));
        assert_eq!(err(Fault::NoBroker).no_desk_seat(Some(5)), Some((5, false)));
        assert_eq!(err(Fault::NoBroker).no_desk_seat(Some(1)), Some((1, true)));
        for f in [Fault::NoAnswer, Fault::Full, Fault::Taken, Fault::Refused] {
            assert_eq!(err(f).no_desk_seat(Some(2)), None, "{f:?}");
        }
        let words: Vec<_> = [
            Fault::NoBroker,
            Fault::NoAnswer,
            Fault::Full,
            Fault::Taken,
            Fault::Refused,
        ]
        .map(Fault::as_str)
        .into();
        assert_eq!(
            words,
            ["no broker", "no answer", "full", "taken", "refused"]
        );
    }
}
