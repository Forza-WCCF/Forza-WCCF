//! data\logs at each start, as play.py's rotate_logs and archive_run do it: the last run's logs move to previous\, the
//! run before that is zipped into archive\run-<time of its last write>.zip (the newest zips kept up to a size, the
//! oldest go first), and a fresh hook\ folder is made.  Housekeeping never stops a start: what fails on a file another
//! program holds (an antivirus, a viewer) is said and stepped around.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use time::{OffsetDateTime, UtcOffset};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

/// data\logs\archive: zipped runs kept up to this many bytes; the newest stays, however big.
pub const ARCHIVE_LIMIT: u64 = 200 * 1024 * 1024;

/// A time on this PC's clock (UTC if Windows cannot say the offset).
pub(crate) fn local(t: SystemTime) -> OffsetDateTime {
    let utc = OffsetDateTime::from(t);
    UtcOffset::local_offset_at(utc).map_or(utc, |o| utc.to_offset(o))
}

/// `t` as the run logs' lines start: `hh:mm:ss.mmm`.
pub fn clock(t: OffsetDateTime) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

/// One "hh:mm:ss  line" added to `file`, its folder made first. A write that fails is skipped: logs never stop a step.
pub fn append(file: &Path, line: &str) {
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(file) {
        let t = local(SystemTime::now());
        let _ = writeln!(f, "{:02}:{:02}:{:02}  {line}", t.hour(), t.minute(), t.second());
    }
}

/// The line [`session_log`] leaves between two sessions of one run (a cabinet restart): [`last_session`] reads what
/// follows the last one, the session that is running.
pub const RESTART: &str = "--- the cabinet restarted; the lines above are the session before ---";

/// This session's log at `path`, made when it is not there: a log that is there is appended to, so a cabinet restart
/// keeps the session before it (the logs a player then saves with SAVE LOGS), set off by [`RESTART`].
pub fn session_log(path: &Path) -> io::Result<File> {
    let mut f = open_log(path)?;
    if f.metadata()?.len() > 0 {
        writeln!(f, "\n{RESTART}")?;
    }
    Ok(f)
}

/// `path` opened to add this session's lines to, with no [`RESTART`] line: a log opened again inside one session (a text
/// switch done twice, a start that skipped rotation) did not see the cabinet restart.
pub fn open_log(path: &Path) -> io::Result<File> {
    fs::OpenOptions::new().create(true).append(true).open(path)
}

/// `text` from after the last [`RESTART`] line: the session that is running (all of it when there is none).
pub fn last_session(text: &str) -> &str {
    text.rsplit_once(RESTART).map_or(text, |(_, running)| running)
}

/// Every file under `dir` in os.walk's order: a folder's files, then its folders one by one; none if it is missing.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut dirs = Vec::new();
    for e in entries.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            dirs.push(e.path());
        } else {
            out.push(e.path());
        }
    }
    for d in dirs {
        walk(&d, out);
    }
}

fn modified(f: &Path) -> io::Result<SystemTime> {
    fs::metadata(f)?.modified()
}

/// A run's log folder -> archive\run-<local time of its last write>.zip (" (2)", " (3)" ... if taken), then the oldest
/// zips go until the archive fits in `limit` (the newest stays).  True if there was something to zip.
pub fn archive_run(folder: &Path, archive: &Path, limit: u64) -> io::Result<bool> {
    let mut files = Vec::new();
    walk(folder, &mut files);
    if files.is_empty() {
        return Ok(false);
    }
    fs::create_dir_all(archive)?;
    let times = files.iter().map(|f| modified(f)).collect::<io::Result<Vec<_>>>()?;
    let t = local(times.into_iter().max().unwrap_or(SystemTime::UNIX_EPOCH));
    let stamp = format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    );
    let mut out = archive.join(format!("run-{stamp}.zip"));
    for n in 2.. {
        if !out.exists() {
            break;
        }
        out = archive.join(format!("run-{stamp} ({n}).zip"));
    }
    let mut tmp = out.as_os_str().to_owned();
    tmp.push(".tmp");
    let mut z = ZipWriter::new(File::create(&tmp)?);
    for f in &files {
        let rel = f.strip_prefix(folder).map_err(io::Error::other)?;
        let name: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect();
        let t = local(modified(f)?);
        let when = DateTime::from_date_and_time(
            u16::try_from(t.year()).unwrap_or(1980),
            t.month().into(),
            t.day(),
            t.hour(),
            t.minute(),
            t.second(),
        )
        .unwrap_or_default();
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(when);
        z.start_file(name.join("/"), opts)?;
        io::copy(&mut File::open(f)?, &mut z)?;
    }
    z.finish()?;
    fs::rename(&tmp, &out)?;
    let mut zips: Vec<(String, SystemTime)> = fs::read_dir(archive)?
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().into_string().ok()?;
            let t = e.metadata().ok()?.modified().ok()?;
            n.ends_with(".zip").then_some((n, t))
        })
        .collect();
    // oldest first by last write: two runs in one second are "run-<t>.zip" and "run-<t> (2).zip", and their names
    // alone put the newer one first (a space sorts before the dot), so it was the newer that went
    zips.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let size = |n: &String| fs::metadata(archive.join(n)).map(|m| m.len());
    let mut total = 0;
    for (n, _) in &zips {
        total += size(n)?;
    }
    for (n, _) in &zips[..zips.len() - 1] {
        if total <= limit {
            break;
        }
        total -= size(n)?;
        fs::remove_file(archive.join(n))?;
    }
    Ok(true)
}

/// The two runs kept as folders behind previous\, newest first (SAVE LOGS sends all three).
pub const OLDER: [&str; 2] = ["previous-2", "previous-3"];

/// At a start: previous\ zipped into archive\ and moved to previous-2\ (previous-2\ to previous-3\, the old
/// previous-3\ gone), the last run's logs into previous\, and a fresh hook\ folder.  What it had to step around, in
/// the words to say; an error only if `logs` or its hook\ cannot be made.  A previous-<time> folder set aside by an
/// earlier start (its archive failed then) is zipped here and goes once it is in the archive; one still unarchived
/// stays for SAVE LOGS.
pub fn rotate(logs: &Path) -> io::Result<Vec<String>> {
    let mut said = Vec::new();
    fs::create_dir_all(logs)?;
    let archive = logs.join("archive");
    for e in fs::read_dir(logs)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with("previous-") && !OLDER.contains(&n.as_str()) && e.file_type().is_ok_and(|t| t.is_dir()) {
            // a run an earlier start set aside (its archive failed then) goes once it is in the archive, not before:
            // the start tries again, and a folder that still cannot be archived stays for SAVE LOGS
            if archive_run(&e.path(), &archive, ARCHIVE_LIMIT).is_ok() {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
    let prev = logs.join("previous");
    if prev.is_dir() {
        let archived = archive_run(&prev, &archive, ARCHIVE_LIMIT);
        if let Err(ex) = &archived {
            said.push(format!("  logs: the run before last not archived ({ex})"));
        }
        let _ = fs::remove_dir_all(logs.join(OLDER[1]));
        let _ = fs::rename(logs.join(OLDER[0]), logs.join(OLDER[1]));
        // a run that cannot move (previous-2\ still there, a file held) goes, as before, once it is in the archive;
        // with none it is set aside below instead of deleted
        let moved = fs::rename(&prev, logs.join(OLDER[0])).or_else(|ex| {
            if archived.is_ok() {
                fs::remove_dir_all(&prev)
            } else {
                Err(ex)
            }
        });
        if let Err(ex) = moved {
            // a file in it is held open: set it aside so a fresh one can be made
            let t = local(SystemTime::now());
            let name = format!(
                "previous-{:04}{:02}{:02}-{:02}{:02}{:02}",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            );
            if fs::rename(&prev, logs.join(&name)).is_ok() {
                said.push(format!(
                    "  logs: last run's folder could not move ({ex}) - set aside as {name}"
                ));
            } else {
                said.push(format!(
                    "  logs: last run's folder is busy ({ex}) - starting without rotating this run's logs"
                ));
                return Ok(said);
            }
        }
    }
    let moved = (|| -> io::Result<()> {
        let names: Vec<_> = fs::read_dir(logs)?
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n != "archive" && !n.to_string_lossy().starts_with("previous"))
            .collect();
        if !names.is_empty() {
            fs::create_dir_all(&prev)?;
        }
        for n in names {
            let (from, to) = (logs.join(&n), prev.join(&n));
            // as shutil.move: a file that cannot be renamed (held open) is copied, and stays if it cannot be removed
            if fs::rename(&from, &to).is_err() && from.is_file() && fs::copy(&from, &to).is_ok() {
                let _ = fs::remove_file(&from);
            }
        }
        Ok(())
    })();
    if let Err(ex) = moved {
        said.push(format!(
            "  logs: could not rotate this run's logs ({ex}) - starting anyway"
        ));
    }
    fs::create_dir_all(logs.join("hook"))?;
    Ok(said)
}

/// A run log whose every line starts with the wall clock ([`clock`]): the debugger's own lines keep their `t+`
/// seconds after it, so lines of logs started at different times line up.
pub struct Stamped<W: Write> {
    out: W,
    at_start: bool,
    now: fn() -> String,
}

impl<W: Write> Stamped<W> {
    pub fn new(out: W) -> Self {
        Self::with_clock(out, || clock(local(SystemTime::now())))
    }

    fn with_clock(out: W, now: fn() -> String) -> Self {
        Self {
            out,
            at_start: true,
            now,
        }
    }
}

impl<W: Write> Write for Stamped<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for piece in buf.split_inclusive(|&b| b == b'\n') {
            if self.at_start {
                self.out.write_all(format!("{} ", (self.now)()).as_bytes())?;
            }
            self.out.write_all(piece)?;
            self.at_start = piece.ends_with(b"\n");
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::{Stamped, last_session, session_log};
    use std::io::Write;

    #[test]
    fn every_line_of_a_stamped_log_starts_with_the_clock() {
        let mut s = Stamped::with_clock(Vec::new(), || "09:05:07.042".into());
        write!(s, "  t+  1.0s process ").unwrap();
        writeln!(s, "started").unwrap();
        write!(s, "a\nb\n").unwrap();
        assert_eq!(
            String::from_utf8(s.out).unwrap(),
            "09:05:07.042   t+  1.0s process started\n09:05:07.042 a\n09:05:07.042 b\n"
        );
    }

    /// A cabinet restart reopens the run's logs: the session before it stays (SAVE LOGS sends all), and what the run
    /// reads of a log (its seat's window, a crash) is the session that is running.
    #[test]
    fn a_restarted_cabinet_keeps_the_session_before_it() {
        let d = crate::scratch();
        let f = d.path().join("run_seat1.txt");
        writeln!(Stamped::new(session_log(&f).unwrap()), "launched control pid 4242").unwrap();
        writeln!(Stamped::new(session_log(&f).unwrap()), "launched control pid 5151").unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(
            text.contains("launched control pid 4242"),
            "the session before the restart is kept: {text}"
        );
        assert!(
            !last_session(&text).contains("4242"),
            "only the running session is read: {text}"
        );
        assert!(last_session(&text).contains("5151"), "{text}");
    }
}
