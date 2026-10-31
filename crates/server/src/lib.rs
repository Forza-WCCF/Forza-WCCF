//! The Forza-WCCF online server (step 10, piece I2): the seat desk and the match relay, ported from
//! `server/seat_desk.py` and `relay.py` (and the 2017-18 `_seat_broker.py`, `_relay.py`). Std threads
//! and blocking I/O; builds for Windows and Linux. Ports, timers and paths are parameters of each service's config
//! (the binary fills them from the Python's environment variables); tests bind `127.0.0.1:0`.

pub mod cli;
pub mod desk;
pub mod desync;
pub mod relay;
pub mod shoproom;

pub use forza::service::Log;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;
use time::{OffsetDateTime, UtcOffset};

/// Which edition's clients the server talks to. Only the seat desk and the relay differ (see [`desk::DeskConfig`],
/// [`relay::RelayConfig`]).
pub use forza::Edition;

static OFFSET: OnceLock<UtcOffset> = OnceLock::new();

/// Reads the PC's UTC offset while the process has one thread (Linux refuses it later); call first in `main`.
pub fn init_clock() {
    if let Ok(o) = UtcOffset::current_local_offset() {
        let _ = OFFSET.set(o);
    }
}

/// The PC's clock: [`init_clock`]'s offset, else the OS's for that instant (Windows), else UTC.
pub fn now() -> OffsetDateTime {
    let utc = OffsetDateTime::now_utc();
    let off = OFFSET
        .get()
        .copied()
        .or_else(|| UtcOffset::local_offset_at(utc).ok());
    off.map_or(utc, |o| utc.to_offset(o))
}

/// `YYYY-MM-DD HH:MM:SS` for log lines, `YYYY-MM-DD_HHMMSS` for upload names (`file_name`).
pub fn stamp(t: OffsetDateTime, file_name: bool) -> String {
    let (y, mo, d) = (t.year(), u8::from(t.month()), t.day());
    let (h, mi, s) = (t.hour(), t.minute(), t.second());
    if file_name {
        format!("{y:04}-{mo:02}-{d:02}_{h:02}{mi:02}{s:02}")
    } else {
        format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
    }
}

/// The server's log: each line stamped, printed, and appended to `file` (opened per line, failures ignored, as the
/// Python's `log()` did).
pub fn file_log(file: Option<PathBuf>) -> Log {
    Arc::new(move |msg| {
        let line = format!("{} {msg}", stamp(now(), false));
        let _ = writeln!(std::io::stdout(), "{line}");
        if let Some(p) = &file
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
        {
            let _ = writeln!(f, "{line}");
        }
    })
}

/// A running service. Dropping it (or [`Handle::stop`]) stops it and joins its threads; connection threads of the
/// TCP services end with their connections.
pub struct Handle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    tcp: bool,
    threads: Vec<JoinHandle<()>>,
    _ticker: Option<mpsc::Sender<()>>,
}

impl Handle {
    pub(crate) fn new(
        addr: SocketAddr,
        stop: Arc<AtomicBool>,
        tcp: bool,
        threads: Vec<JoinHandle<()>>,
        ticker: Option<mpsc::Sender<()>>,
    ) -> Self {
        Self {
            addr,
            stop,
            tcp,
            threads,
            _ticker: ticker,
        }
    }

    /// The address the service listens on (the real port when it was asked for port 0).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn stop(self) {}
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self._ticker.take();
        if self.tcp {
            // wakes the blocking accept
            let mut wake = self.addr;
            if wake.ip().is_unspecified() {
                wake.set_ip(std::net::Ipv4Addr::LOCALHOST.into());
            }
            let _ = TcpStream::connect(wake);
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Ends a connection after our last line. Closing with the PC's bytes still unread makes the OS send a reset, which
/// can discard the answer before the PC reads it (a refusal sent while the upload is still arriving). So: FIN after
/// the answer, then read off what the PC still sends (a little, briefly) until it hangs up.
pub(crate) fn hang_up(conn: TcpStream) {
    let _ = conn.shutdown(std::net::Shutdown::Write);
    let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = io::copy(&mut (&conn).take(1 << 20), &mut io::sink());
}
