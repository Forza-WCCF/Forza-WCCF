//! Replays tests/rec/logs.jsonl (legacy/2010-11/tests/rec_a_logs.py: play.py's archive_run and rotate_logs).  Zip names
//! and times are local times, so the replay checks only on a PC with the recorder's UTC offset at those times.

use std::fs::{self, File};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use launcher::logs::{archive_run, rotate};
use regex::Regex;
use serde_json::{Value, json};
use time::{OffsetDateTime, UtcOffset};
use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

/// xorshift32 bytes, as the recorder makes them.
fn data(seed: u32, n: usize) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn num<T: TryFrom<u64>>(v: &Value) -> T {
    T::try_from(v.as_u64().unwrap()).ok().unwrap()
}

/// The recorded files ([rel, size, seed, mtime]) under `base`.
fn make(base: &Path, files: &Value) {
    for f in files.as_array().unwrap() {
        let p = base.join(f[0].as_str().unwrap());
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, data(num(&f[2]), num(&f[1]))).unwrap();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(num(&f[3]));
        File::options().write(true).open(&p).unwrap().set_modified(at).unwrap();
    }
}

/// Every folder and file under `root` as the recorder lists them: [rel, null | "zip" | the text].
fn tree(root: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().to_str().unwrap().to_owned();
            if p.is_dir() {
                out.push(json!([rel, null]));
                todo.push(p);
            } else if rel.ends_with(".zip") {
                out.push(json!([rel, "zip"]));
            } else {
                let body: String = fs::read(&p).unwrap().into_iter().map(char::from).collect();
                out.push(json!([rel, body]));
            }
        }
    }
    out.sort_by_key(|v| v[0].as_str().unwrap().to_owned());
    out
}

/// The zips' names with the " (n)" of a second run in the same second dropped: which of the two stays follows the
/// fixed pruning (#356), where the recorded cases keep play.py's choice of it.
fn stamps(v: &Value) -> Vec<String> {
    let mut s: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            let n = n.as_str().unwrap();
            n.split_once(" (").map_or(n, |(stamp, _)| stamp).to_owned()
        })
        .collect();
    s.sort();
    s
}

fn archive_case(root: &Path, rec: &Value) {
    let arc = root.join("archive");
    let folder = root.join(rec["folder"].as_str().unwrap());
    if !rec["files"].is_null() {
        fs::create_dir_all(&folder).unwrap();
        make(&folder, &rec["files"]);
    }
    let names = |d: &Path| -> Vec<String> {
        let mut n: Vec<String> = fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        n.sort();
        n
    };
    let before = if arc.is_dir() { names(&arc) } else { Vec::new() };
    let ret = archive_run(&folder, &arc, num(&rec["limit"])).unwrap();
    assert_eq!(Value::from(ret), rec["ret"], "{rec}");
    let zips = arc.is_dir().then(|| names(&arc));
    // the same runs stay; of two zips of one second the newer one now stays, where play.py's name order kept the older
    assert_eq!(
        zips.as_ref().map(|z| stamps(&json!(z))),
        Some(stamps(&rec["zips"])),
        "{rec}"
    );
    let made: Vec<String> = zips
        .unwrap_or_default()
        .into_iter()
        .filter(|z| !before.contains(z))
        .collect();
    assert_eq!(json!(made), rec["made"], "{rec}");
    let mut entries = Vec::new();
    for m in &made {
        let mut z = zip::ZipArchive::new(File::open(arc.join(m)).unwrap()).unwrap();
        for i in 0..z.len() {
            let f = z.by_index(i).unwrap();
            let t = f.last_modified().unwrap();
            let when = [
                u16::from(t.month()),
                u16::from(t.day()),
                u16::from(t.hour()),
                u16::from(t.minute()),
            ];
            let when = [&[t.year()][..], &when, &[u16::from(t.second())]].concat();
            let method = if f.compression() == zip::CompressionMethod::Deflated {
                8
            } else {
                0
            };
            entries.push(json!([f.name().unwrap(), when, f.size(), f.crc32(), method]));
        }
    }
    assert_eq!(Value::from(entries), rec["entries"], "{rec}");
}

fn rotate_case(root: &Path, rec: &Value) {
    let logs = root.join(rec["case"].as_str().unwrap().replace(' ', "_")).join("logs");
    make(&logs, &rec["files"]);
    let held = rec["held"].as_str().map(|h| {
        // as the recorder's open(.., "rb"): no FILE_SHARE_DELETE
        File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(logs.join(h))
            .unwrap()
    });
    let said = rotate(&logs).unwrap();
    drop(held);
    let ex = Regex::new(r"\(.*\)( - |$)").unwrap();
    let now = Regex::new(r"previous-\d{8}-\d{6}").unwrap();
    let said: Vec<String> = said
        .iter()
        .map(|s| now.replace(&ex.replace(s, "({ex})$1"), "previous-{now}").into_owned())
        .collect();
    assert_eq!(json!(said), rec["said"], "{}", rec["case"]);
    // the other change: the run before last also stays as previous-2\ (play.py only zipped it)
    let older = |v: &Value| v[0].as_str().unwrap().starts_with("previous-2");
    let t: Vec<Value> = tree(&logs)
        .into_iter()
        .filter(|v| !older(v))
        .map(|v| {
            let p = v[0].as_str().unwrap();
            let p = if p.contains("20261001") {
                p.to_owned()
            } else {
                now.replace(p, "previous-{now}").into_owned()
            };
            json!([p, v[1]])
        })
        .collect();
    // the one change from play.py: a previous-<time> folder an earlier start set aside goes at the next start
    let set_aside = |v: &Value| v[0].as_str().unwrap().contains("previous-20261001");
    let want: Vec<Value> = rec["tree"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| !set_aside(v))
        .cloned()
        .collect();
    assert!(
        !t.iter().any(set_aside),
        "{}: the old previous-<time> folder is gone",
        rec["case"]
    );
    assert_eq!(Value::from(t), Value::from(want), "{}", rec["case"]);
}

#[test]
fn logs_as_recorded() {
    let mut lines = include_str!("../rec/logs.jsonl")
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap());
    let offset = lines.next().unwrap()["utc_offset"].as_i64().unwrap();
    let at = OffsetDateTime::from_unix_timestamp(1_791_500_001).unwrap();
    let here = UtcOffset::local_offset_at(at).map_or(0, |o| i64::from(o.whole_seconds()));
    if here != offset {
        eprintln!("recorded at UTC{offset:+}s, this PC is UTC{here:+}s at those times: not replayed");
        return;
    }
    let root = crate::scratch();
    let (mut archives, mut rotations) = (0, 0);
    for rec in lines {
        if rec["op"] == "archive" {
            archive_case(&root.path().join("archive_run"), &rec);
            archives += 1;
        } else {
            rotate_case(&root.path().join("rotate"), &rec);
            rotations += 1;
        }
    }
    assert_eq!((archives, rotations), (8, 6));
}

#[test]
fn append_makes_the_folder_and_adds_timed_lines() {
    let d = crate::scratch();
    let f = d.path().join("logs").join("launcher.txt");
    launcher::logs::append(&f, "one");
    launcher::logs::append(&f, "two");
    let text = fs::read_to_string(&f).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].ends_with("  one") && lines[0].len() == 13, "{text}");
    assert!(lines[1].ends_with("  two") && lines[1].as_bytes()[2] == b':', "{text}");
}

#[test]
fn rotate_keeps_the_last_three_runs() {
    let d = crate::scratch();
    let logs = d.path().join("logs");
    for run in 1..=5 {
        rotate(&logs).unwrap();
        fs::write(logs.join("run.txt"), format!("run {run}")).unwrap();
    }
    fs::create_dir_all(logs.join("previous-20261001-000000")).unwrap(); // set aside by an earlier start
    rotate(&logs).unwrap();
    let run = |dir: &str| fs::read_to_string(logs.join(dir).join("run.txt")).unwrap();
    assert_eq!(
        (run("previous"), run("previous-2"), run("previous-3")),
        ("run 5".into(), "run 4".into(), "run 3".into())
    );
    assert!(!logs.join("previous-20261001-000000").exists());
    let zips = fs::read_dir(logs.join("archive")).unwrap().count();
    assert!(zips >= 1, "every run that left previous\\ is zipped");
}

/// The archive: the oldest run goes by its last write, so the " (2)" of two runs in one second outlives the plain
/// name it sorts before (#356). Runs everywhere: no recorded times, no PC clock offset.
#[test]
fn the_archive_prunes_by_time_not_by_name() {
    let d = crate::scratch();
    let arc = d.path().join("logs").join("archive");
    fs::create_dir_all(&arc).unwrap();
    let old = arc.join("run-2026-10-09_00-53-21.zip");
    let newer = arc.join("run-2026-10-09_00-53-21 (2).zip");
    // two zips of one second, 60 KB each: over the limit below together with the run made now
    for (p, seed) in [(&old, 1u32), (&newer, 2u32)] {
        fs::write(p, data(seed, 60_000)).unwrap();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_500_000 + u64::from(seed));
        File::options().write(true).open(p).unwrap().set_modified(at).unwrap();
    }
    let run = d.path().join("run");
    fs::create_dir_all(&run).unwrap();
    fs::write(run.join("run_seat1.txt"), "a run").unwrap();
    archive_run(&run, &arc, 100_000).unwrap();
    assert!(newer.exists(), "the newer of two runs in one second stays");
    assert!(!old.exists(), "the older of the two goes");
    assert_eq!(fs::read_dir(&arc).unwrap().count(), 2, "the run made now stays as the newest");
}

/// A run whose archive could not be made is set aside, not deleted (#356): previous-2\ cannot go (a file in it is
/// held), so previous\ has nowhere to move, and play.py's path dropped it with no copy in the archive.
#[test]
fn a_run_whose_archive_failed_is_set_aside() {
    let d = crate::scratch();
    let logs = d.path().join("logs");
    fs::create_dir_all(logs.join("previous")).unwrap();
    fs::write(logs.join("previous").join("run_seat1.txt"), "the run before last").unwrap();
    // the archive cannot be made: a file is in its place
    fs::write(logs.join("archive"), "not a folder").unwrap();
    fs::create_dir_all(logs.join("previous-2")).unwrap();
    let held = logs.join("previous-2").join("mxhook_1.log");
    fs::write(&held, "held").unwrap();
    let open = File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&held)
        .unwrap();
    let said = rotate(&logs).unwrap();
    drop(open);
    assert!(
        said.iter().any(|s| s.contains("not archived")),
        "the start says the run was not archived: {said:?}"
    );
    let kept = fs::read_dir(&logs).unwrap().flatten().any(|e| {
        fs::read_to_string(e.path().join("run_seat1.txt"))
            .is_ok_and(|t| t.contains("the run before last"))
    });
    assert!(kept, "the run whose archive failed is kept, not deleted: {said:?}");
}

#[test]
fn run_log_clock_has_milliseconds() {
    let t = OffsetDateTime::from_unix_timestamp_nanos(1_791_500_001_042_000_000).unwrap();
    assert_eq!(launcher::logs::clock(t), "22:53:21.042");
}
