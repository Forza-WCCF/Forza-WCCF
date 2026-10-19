//! What every service of the Launcher's stand-ins and of `forza-server` is built from: the log sink, the running
//! service's handle and the blocking accept and read loops (std threads, blocking I/O).

use std::io::{self, Read};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Where a service writes its log lines (the caller adds timestamps and files).
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// A running service. Dropping it (or [`Handle::stop`]) stops the service and joins its main thread; connection
/// threads notice the flag within 100 ms.
pub struct Handle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn new(stop: Arc<AtomicBool>, thread: JoinHandle<()>) -> Self {
        Self {
            stop,
            thread: Some(thread),
        }
    }

    pub fn stop(self) {}

    /// How the service's main thread ended, once it has - before the handle was dropped that is never planned, so the
    /// run's supervisor says it (and stops if the service was essential).  None while it runs.  Said once: the
    /// thread is taken.
    pub fn died(&mut self) -> Option<String> {
        if !self.thread.as_ref()?.is_finished() {
            return None;
        }
        Some(match self.thread.take()?.join() {
            Ok(()) => "its thread returned".to_owned(),
            Err(p) => {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                format!("its thread panicked: {msg}")
            }
        })
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Accepts connections until `stop` is set; each one runs `serve` on its own thread.
pub(crate) fn accept_loop(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    serve: impl Fn(TcpStream, SocketAddr) + Send + Sync + 'static,
) -> io::Result<Handle> {
    listener.set_nonblocking(true)?;
    let serve = Arc::new(serve);
    let flag = stop.clone();
    let thread = thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((conn, peer)) => {
                    let serve = serve.clone();
                    let _ = conn.set_nonblocking(false);
                    let _ = conn.set_read_timeout(Some(Duration::from_millis(100)));
                    thread::spawn(move || serve(conn, peer));
                }
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        }
    });
    Ok(Handle::new(stop, thread))
}

/// Reads at least one byte into `buf`, retrying on the 100 ms read timeout until `stop` is set.
/// `Ok(0)` means the peer closed or `stop` is set.
pub(crate) fn read_some(
    conn: &mut TcpStream,
    buf: &mut [u8],
    stop: &AtomicBool,
) -> io::Result<usize> {
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(0);
        }
        match conn.read(buf) {
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            r => return r,
        }
    }
}
