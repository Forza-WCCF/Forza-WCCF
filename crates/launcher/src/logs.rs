//! data\logs at each start, as play.py's rotate_logs and archive_run do it: the last run's logs move to previous\, the
//! run before that is zipped into archive\run-<time of its last write>.zip (the newest zips kept up to a size, the
//! oldest go first), and a fresh hook\ folder is made.  Housekeeping never stops a start: what fails on a file another
//! program holds (an antivirus, a viewer) is said and stepped around.

use std::fs::{self, File};
use std::io;
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
    let mut zips: Vec<String> = fs::read_dir(archive)?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".zip"))
        .collect();
    zips.sort(); // the names sort by time
    let size = |n: &String| fs::metadata(archive.join(n)).map(|m| m.len());
    let mut total = 0;
    for z in &zips {
        total += size(z)?;
    }
    for z in &zips[..zips.len() - 1] {
        if total <= limit {
            break;
        }
        total -= size(z)?;
        fs::remove_file(archive.join(z))?;
    }
    Ok(true)
}

/// At a start: previous\ zipped into archive\, the last run's logs into previous\, and a fresh hook\ folder.  What it
/// had to step around, in the words to say; an error only if `logs` or its hook\ cannot be made.
pub fn rotate(logs: &Path) -> io::Result<Vec<String>> {
    let mut said = Vec::new();
    fs::create_dir_all(logs)?;
    let prev = logs.join("previous");
    if prev.is_dir() {
        if let Err(ex) = archive_run(&prev, &logs.join("archive"), ARCHIVE_LIMIT) {
            said.push(format!("  logs: the run before last not archived ({ex})"));
        }
        if let Err(ex) = fs::remove_dir_all(&prev) {
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
                    "  logs: last run's folder was busy ({ex}) - set aside as {name}"
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
