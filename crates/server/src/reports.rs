//! Players' problem reports, taken by the seat desk's `REPORT` line ([`forza::online::report`] has the wire). The
//! desk port is public and unauthenticated, so: a ZIP of at most [`MAX_ZIP`] bytes, words of at most
//! [`MAX_NOTE_CHARS`] characters of valid UTF-8, `per_hour` reports an hour from one address, [`SLOTS`] at a time,
//! `timeout` per read and `deadline` for the whole upload, `keep` bytes in all (the oldest go). Names are the server's
//! own (`DATE_TIME_ID.zip` and `.txt`); the ZIP is checked for its first 4 bytes and kept as it came, never opened.

use crate::inbox::{clean_version, code, hourly_over, trim_folder};
use crate::{Edition, Log, now, stamp};
use forza::online::UploadReply;
use forza::online::report::{Head, MAX_NOTE_BYTES, MAX_NOTE_CHARS, MAX_ZIP};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Reports coming in at once; the next is told `NO busy`.
pub const SLOTS: usize = 4;

#[derive(Clone, Debug)]
pub struct ReportConfig {
    /// `<store>/reports` (`forza-server --reports`).
    pub folder: PathBuf,
    /// Reports an hour from one address (5).
    pub per_hour: usize,
    /// Bytes kept in all, `.zip` and `.txt` (1 GB).
    pub keep: u64,
    /// Longest wait for the next bytes (30 s).
    pub timeout: Duration,
    /// Longest whole upload after `GO` (3 min: 4 MB at 25 KB/s).
    pub deadline: Duration,
}

impl ReportConfig {
    pub fn new(folder: PathBuf) -> Self {
        Self {
            folder,
            per_hour: 5,
            keep: 1024 * 1024 * 1024,
            timeout: Duration::from_secs(30),
            deadline: Duration::from_secs(180),
        }
    }
}

#[derive(Debug)]
pub struct Reports {
    cfg: ReportConfig,
    recent: Mutex<HashMap<String, Vec<Instant>>>,
    active: AtomicUsize,
    trim_lock: Mutex<()>,
}

/// A slot taken while it lives.
struct Slot<'a>(&'a AtomicUsize);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Reports {
    pub fn new(cfg: ReportConfig) -> Self {
        Self {
            cfg,
            recent: Mutex::default(),
            active: AtomicUsize::new(0),
            trim_lock: Mutex::default(),
        }
    }

    pub fn folder(&self) -> &std::path::Path {
        &self.cfg.folder
    }

    /// One `REPORT` line `head` from `ip`; `rest` is what came after the line. Answers on `conn`.
    pub(crate) fn serve(
        &self,
        edition: Edition,
        log: &Log,
        head: &str,
        rest: Vec<u8>,
        conn: &TcpStream,
        ip: &str,
    ) -> io::Result<()> {
        let mut conn = conn;
        let no = |mut conn: &TcpStream, why: &str| {
            log(&format!("{ip}: report refused - {why}"));
            conn.write_all(UploadReply::No(why.into()).line().as_bytes())
        };
        let Some(Head { zip, note, version }) = Head::parse(head) else {
            return no(conn, "a bad request");
        };
        if zip == 0 || zip > MAX_ZIP {
            return no(conn, &format!("too big - {zip} bytes, at most {MAX_ZIP}"));
        }
        if note > MAX_NOTE_BYTES {
            return no(
                conn,
                &format!("the words are too long - at most {MAX_NOTE_CHARS} characters"),
            );
        }
        if self.active.fetch_add(1, Ordering::SeqCst) >= SLOTS {
            self.active.fetch_sub(1, Ordering::SeqCst);
            return no(conn, "busy - try again in a minute");
        }
        let _slot = Slot(&self.active);
        if hourly_over(&self.recent, ip, self.cfg.per_hour) {
            return no(conn, "too many from this address - try again in an hour");
        }
        conn.write_all(b"GO\n")?;
        conn.set_read_timeout(Some(self.cfg.timeout))?;
        conn.set_write_timeout(Some(self.cfg.timeout))?;
        let want = note + zip;
        let end = Instant::now() + self.cfg.deadline;
        let mut data = rest;
        data.truncate(want);
        while data.len() < want {
            if Instant::now() > end {
                return Err(io::Error::other(format!(
                    "{ip}: report too slow, {} of {want} bytes",
                    data.len()
                )));
            }
            let mut chunk = vec![0u8; (want - data.len()).min(65536)];
            match conn.read(&mut chunk)? {
                0 => {
                    return Err(io::Error::other(format!(
                        "{ip}: the PC hung up after {} of {want} bytes",
                        data.len()
                    )));
                }
                n => data.extend_from_slice(&chunk[..n]),
            }
        }
        let blob = data.split_off(note);
        let Ok(words) = String::from_utf8(data) else {
            return no(conn, "the words are not UTF-8");
        };
        if words.chars().count() > MAX_NOTE_CHARS {
            return no(
                conn,
                &format!("the words are too long - at most {MAX_NOTE_CHARS} characters"),
            );
        }
        if !blob.starts_with(b"PK\x03\x04") {
            return no(conn, "not a ZIP");
        }
        let id = code(6);
        let version = clean_version(&version);
        let when = now();
        let name = format!("{}_{id}", stamp(when, true));
        let dir = &self.cfg.folder;
        std::fs::create_dir_all(dir)?;
        let part = dir.join(format!("{name}.part"));
        std::fs::write(&part, &blob)?;
        std::fs::rename(&part, dir.join(format!("{name}.zip")))?;
        std::fs::write(
            dir.join(format!("{name}.txt")),
            format!(
                "id: {id}\nedition: {}\nversion: {version}\nreceived: {}\nzip: {} bytes\n\n{words}\n",
                edition.name(),
                stamp(when, false),
                blob.len()
            ),
        )?;
        trim_folder(
            "reports",
            dir,
            self.cfg.keep,
            &["txt"],
            &self.trim_lock,
            log,
        );
        conn.write_all(UploadReply::Ok(id.clone()).line().as_bytes())?;
        log(&format!(
            "report {id}: edition {}, version {version}, {} KB, from {ip}",
            edition.name(),
            blob.len() / 1024
        ));
        Ok(())
    }
}
