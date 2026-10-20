//! Replays `rec/inbox.jsonl` (recorded against the Python log inbox by `rec_i2_inbox.py`) against
//! `forza_server::inbox`, and ports `test_send_logs.py`'s server-side cases: a PC that hangs up half way, the quota
//! per address, eviction of the oldest, 4 uploads at a time. The random code is compared as `<CODE>`.

use crate::common::*;
use forza_server::inbox::{InboxConfig, start};
use std::time::Duration;

fn ls(folder: &std::path::Path) -> (Vec<String>, Vec<u64>) {
    let mut found: Vec<(String, u64)> = std::fs::read_dir(folder)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    // YYYY-MM-DD_HHMMSS_CODE_ -> TS_<CODE>_
                    let n = if n.len() > 24
                        && n.as_bytes()[10] == b'_'
                        && n.as_bytes()[17] == b'_'
                        && n.as_bytes()[23] == b'_'
                    {
                        format!("TS_<CODE>_{}", &n[24..])
                    } else {
                        n
                    };
                    (n, e.metadata().unwrap().len())
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found.into_iter().unzip()
}

#[test]
fn the_inbox_keeps_and_refuses_as_the_python_inbox_did() {
    let recs = records("inbox");
    assert_eq!(recs.len(), 7);
    for rec in &recs {
        let name = rec["name"].as_str().unwrap();
        let root = scratch(&format!("inbox_{name}"));
        let mut c = InboxConfig::new(root.join("keep"));
        c.bind = "127.0.0.1".into();
        c.port = 0;
        c.per_hour = rec["cfg"]["per_hour"].as_u64().unwrap() as usize;
        c.keep = rec["cfg"]["keep"].as_u64().unwrap();
        let (log, lines) = capture();
        let inbox = start(c, log).unwrap();
        let mut tcp = Tcp::new(inbox.addr());
        for (i, op) in rec["ops"].as_array().unwrap().iter().enumerate() {
            if tcp.run(op, name, i) {
                continue;
            }
            match op["op"].as_str().unwrap() {
                "ls" => {
                    let (names, sizes) = ls(&root.join("keep"));
                    let want_names: Vec<&str> = op["names"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|n| n.as_str().unwrap())
                        .collect();
                    let want_sizes: Vec<u64> = op["sizes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|n| n.as_u64().unwrap())
                        .collect();
                    assert_eq!(
                        (names, sizes),
                        (
                            want_names.into_iter().map(String::from).collect(),
                            want_sizes
                        ),
                        "{name} op {i}"
                    );
                }
                "log" => {
                    let text = lines.lock().unwrap().join("\n");
                    for (sub, found) in op["has"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .zip(op["found"].as_array().unwrap())
                    {
                        assert_eq!(
                            text.contains(sub.as_str().unwrap()),
                            found.as_bool().unwrap(),
                            "{name} op {i}: log {sub}"
                        );
                    }
                }
                other => panic!("{name}: unknown op {other}"),
            }
        }
        drop(tcp);
        drop(inbox);
        let leftovers = std::fs::read_dir(root.join("keep")).map(|d| {
            d.filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
                .count()
        });
        assert_eq!(leftovers.unwrap_or(0), 0, "{name}: no half files");
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn a_silent_pc_is_dropped_after_the_timeout() {
    let root = scratch("inbox_timeout");
    let mut c = InboxConfig::new(root.join("keep"));
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.timeout = Duration::from_millis(300);
    let (log, lines) = capture();
    let inbox = start(c, log).unwrap();
    let mut tcp = Tcp::new(inbox.addr());
    let op = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
    tcp.run(&op(r#"{"op":"open","c":"a"}"#), "timeout", 0);
    tcp.run(
        &op(r#"{"op":"send","c":"a","hex":"574343464c4f475331203130302035"}"#),
        "timeout",
        1,
    ); // "WCCFLOGS1 100 5" and then nothing
    tcp.run(
        &op(r#"{"op":"recv","c":"a","wait":1.5,"got":"","eof":true}"#),
        "timeout",
        2,
    );
    assert!(
        lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("upload failed")),
        "{:?}",
        lines.lock().unwrap()
    );
}
