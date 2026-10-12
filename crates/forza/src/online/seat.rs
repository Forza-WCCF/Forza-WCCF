//! This PC's seat on a server (port of `online/seat.py`): asks the server's seat desk for a seat, then holds it by
//! pinging every 20 s for as long as the run lasts. If the connection breaks it reconnects and re-claims the same
//! seat (A5). Dropping the [`Lease`] ends the lease and the seat is free again at once.

use super::{Reply, Want};
use crate::standins::{Handle, Log};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
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

/// Sends one `WANT` line and reads the desk's reply line.
fn ask(conn: &mut TcpStream, want: Want, timeout: Duration) -> io::Result<String> {
    conn.set_read_timeout(Some(timeout))?;
    conn.write_all(want.line().as_bytes())?;
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
            n => line.extend_from_slice(&b[..n]),
        }
    }
    let end = line.iter().position(|&c| c == b'\n').unwrap_or(line.len());
    Ok(String::from_utf8_lossy(&line[..end]).into_owned())
}

fn connect(addr: SocketAddr, t: &Timing, want: Want) -> io::Result<(TcpStream, String)> {
    let mut c = TcpStream::connect_timeout(&addr, t.connect)?;
    let line = ask(&mut c, want, t.answer)?;
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
    let end = Instant::now() + life;
    let first = want.map_or(Want::Any, Want::Seat);
    let err = |fault, detail: String| SeatError { fault, detail };
    let mut conn = match TcpStream::connect_timeout(&addr, t.connect) {
        Ok(c) => c,
        Err(e) => return Err(err(Fault::NoBroker, e.to_string())),
    };
    let line = ask(&mut conn, first, t.answer).map_err(|e| err(Fault::NoAnswer, e.to_string()))?;
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
                match connect(addr, &t, Want::Seat(seat)) {
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
