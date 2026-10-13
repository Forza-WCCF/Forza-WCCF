//! The log inbox (`log_inbox.py`): keeps the ZIPs players send with SEND LOGS. One upload per connection:
//! `WCCFLOGS1 SIZE VERSION\n` + SIZE bytes, answered `OK CODE\n` or `NO reason\n`. Kept as
//! `FOLDER\DATE_TIME_CODE_ADDRESS_VERSION.zip`. An open port must not fill the disk: 8 MB an upload, `per_hour`
//! uploads an hour from one address, 4 at a time, `keep` bytes in all (the oldest go). It never opens what it keeps.

use crate::{Handle, Log, now, stamp};
use forza::online::{MAX_UPLOAD, UploadHead, UploadReply};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// No 0/O, 1/I/L: read out loud or typed from a screen.
const CODE_CHARS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const SLOTS: usize = 4;

#[derive(Clone, Debug)]
pub struct InboxConfig {
    /// `WCCF_BIND`, default `0.0.0.0`.
    pub bind: String,
    /// `WCCF_INBOX_PORT`, default 20050; 0 = any (tests).
    pub port: u16,
    pub folder: PathBuf,
    /// `WCCF_INBOX_PER_HOUR`, default 6.
    pub per_hour: usize,
    /// `WCCF_INBOX_KEEP`, default 1 GB.
    pub keep: u64,
    /// Longest wait for the next bytes of an upload (the Python: 60 s).
    pub timeout: Duration,
}

impl InboxConfig {
    pub fn new(folder: PathBuf) -> Self {
        Self {
            bind: "0.0.0.0".into(),
            port: forza::online::INBOX_PORT,
            folder,
            per_hour: 6,
            keep: 1024 * 1024 * 1024,
            timeout: Duration::from_secs(60),
        }
    }
}

struct Shared {
    cfg: InboxConfig,
    log: Log,
    recent: Mutex<HashMap<String, Vec<Instant>>>,
    active: AtomicUsize,
}

/// The oldest uploads go while the folder holds more than `keep` bytes.
fn trim(sh: &Shared) {
    let Ok(dir) = std::fs::read_dir(&sh.cfg.folder) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = dir
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".zip"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    files.sort();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    for (_, size, path) in files {
        if total <= sh.cfg.keep {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
            (sh.log)(&format!(
                "inbox full: removed the oldest, {}",
                path.file_name().unwrap().to_string_lossy()
            ));
        }
    }
}

fn code() -> String {
    let mut out = String::new();
    while out.len() < 5 {
        let mut b = [0u8; 1];
        getrandom::fill(&mut b).expect("the OS random source");
        // 248 = 8 * 31: no modulo bias
        if b[0] < 248 {
            out.push(CODE_CHARS[usize::from(b[0]) % CODE_CHARS.len()] as char);
        }
    }
    out
}

/// A name part that is safe in a file name: `[0-9A-Za-z.-]`, anything else (each byte of it) `_`, 24 characters.
fn clean_version(v: &str) -> String {
    v.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(24)
        .collect()
}

fn serve(sh: &Shared, mut conn: TcpStream, ip: &str) {
    let no = |conn: &mut TcpStream, why: String| {
        let _ = conn.write_all(UploadReply::No(why.clone()).line().as_bytes());
        (sh.log)(&format!("{ip}: refused - {why}"));
    };
    if let Err(e) = upload(sh, &mut conn, ip, no) {
        (sh.log)(&format!("{ip}: upload failed - {e}"));
    }
}

fn upload(
    sh: &Shared,
    conn: &mut TcpStream,
    ip: &str,
    no: impl Fn(&mut TcpStream, String),
) -> io::Result<()> {
    conn.set_read_timeout(Some(sh.cfg.timeout))?;
    conn.set_write_timeout(Some(sh.cfg.timeout))?;
    let mut buf = Vec::new();
    while !buf.contains(&b'\n') {
        if buf.len() > 120 {
            no(conn, "a bad request".into());
            return Ok(());
        }
        let mut chunk = [0u8; 256];
        match conn.read(&mut chunk)? {
            0 => return Ok(()),
            n => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let at = buf.iter().position(|&b| b == b'\n').unwrap();
    // every byte over 127 is one U+FFFD, as the Python's ASCII decode with "replace" made it
    let head: String = buf[..at]
        .iter()
        .map(|&b| if b < 128 { b as char } else { '\u{fffd}' })
        .collect();
    let rest = &buf[at + 1..];
    let Some(UploadHead { size, version }) = UploadHead::parse(&head) else {
        // digits too long for a number are "too big", anything else a bad request
        match head.split_whitespace().collect::<Vec<_>>()[..] {
            ["WCCFLOGS1", d, _] if !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) => {
                let n = d.trim_start_matches('0');
                no(
                    conn,
                    format!(
                        "too big - {} bytes, at most {MAX_UPLOAD}",
                        if n.is_empty() { "0" } else { n }
                    ),
                );
            }
            _ => no(conn, "a bad request".into()),
        }
        return Ok(());
    };
    let version = clean_version(&version);
    if size == 0 || size > MAX_UPLOAD {
        no(
            conn,
            format!("too big - {size} bytes, at most {MAX_UPLOAD}"),
        );
        return Ok(());
    }
    {
        let now = Instant::now();
        let mut recent = sh.recent.lock().unwrap();
        let times = recent.entry(ip.to_string()).or_default();
        times.retain(|t| now.duration_since(*t) < Duration::from_secs(3600));
        if times.len() >= sh.cfg.per_hour {
            drop(recent);
            no(
                conn,
                "too many from this address - try again in an hour".into(),
            );
            return Ok(());
        }
        times.push(now);
    }
    let mut data = rest[..rest.len().min(size)].to_vec();
    while data.len() < size {
        let mut chunk = vec![0u8; (size - data.len()).min(65536)];
        match conn.read(&mut chunk)? {
            0 => {
                return Err(io::Error::other(format!(
                    "the PC hung up after {} of {size} bytes",
                    data.len()
                )));
            }
            n => data.extend_from_slice(&chunk[..n]),
        }
    }
    if !data.starts_with(b"PK\x03\x04") {
        no(conn, "not a ZIP".into());
        return Ok(());
    }
    let code = code();
    let name = format!("{}_{code}_{ip}_{version}.zip", stamp(now(), true));
    std::fs::create_dir_all(&sh.cfg.folder)?;
    let path = sh.cfg.folder.join(&name);
    let part = sh.cfg.folder.join(format!("{name}.part"));
    std::fs::write(&part, &data)?;
    std::fs::rename(&part, &path)?;
    trim(sh); // before the answer: what the PC hears is what the folder holds
    conn.write_all(UploadReply::Ok(code.clone()).line().as_bytes())?;
    (sh.log)(&format!(
        "{ip}: logs kept, code {code}, kit {version}, {} KB -> {name}",
        data.len() / 1024
    ));
    Ok(())
}

/// Starts the inbox. Err(io error) when the port cannot be bound.
pub fn start(cfg: InboxConfig, log: Log) -> io::Result<Handle> {
    let listener = TcpListener::bind((cfg.bind.as_str(), cfg.port))?;
    let addr = listener.local_addr()?;
    log(&format!(
        "log inbox listening on TCP {}: players' SEND LOGS kept in {}",
        addr.port(),
        cfg.folder.display()
    ));
    let sh = Arc::new(Shared {
        cfg,
        log,
        recent: Mutex::default(),
        active: AtomicUsize::new(0),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let t = thread::spawn(move || {
        for conn in listener.incoming() {
            if stopped.load(Ordering::Relaxed) {
                break;
            }
            let Ok(mut conn) = conn else { continue };
            let Ok(peer) = conn.peer_addr() else { continue };
            let ip = peer.ip().to_string();
            if sh.active.fetch_add(1, Ordering::SeqCst) >= SLOTS {
                sh.active.fetch_sub(1, Ordering::SeqCst);
                let _ = conn.write_all(b"NO busy - try again in a minute\n");
                (sh.log)(&format!("{ip}: refused - 4 uploads already coming in"));
                continue;
            }
            let sh = sh.clone();
            thread::spawn(move || {
                serve(&sh, conn, &ip);
                sh.active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    Ok(Handle::new(addr, stop, true, vec![t], None))
}
