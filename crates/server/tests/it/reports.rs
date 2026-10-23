//! `REPORT` on the seat desk (`forza_server::reports`): a good report kept and its id returned, and the caps of a
//! public, unauthenticated input: too big, too long, not UTF-8, too many from one address, the folder's total.

use crate::common::*;
use forza::online::report::{self, MAX_ZIP};
use forza::online::send_logs::SendError;
use forza_server::Edition;
use forza_server::desk::{DeskConfig, start};
use forza_server::reports::{ReportConfig, Reports};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn desk(
    folder: &Path,
    tune: impl FnOnce(&mut ReportConfig),
) -> (forza_server::Handle, Arc<std::sync::Mutex<Vec<String>>>) {
    let mut r = ReportConfig::new(folder.to_path_buf());
    tune(&mut r);
    let mut c = DeskConfig::new(Edition::E1718);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.reports = Some(Arc::new(Reports::new(r)));
    let (log, lines) = capture();
    (start(c, log).unwrap(), lines)
}

fn zip(n: usize) -> Vec<u8> {
    let mut z = b"PK\x03\x04".to_vec();
    z.resize(n, b'z');
    z
}

fn send(at: SocketAddr, z: &[u8], note: &str) -> Result<String, SendError> {
    report::send(at, z, note, "0.3.2", Duration::from_secs(10))
}

/// The answer to a bare first line.
fn answer(at: SocketAddr, line: &[u8]) -> String {
    let mut c = TcpStream::connect(at).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    c.write_all(line).unwrap();
    let mut got = String::new();
    let _ = c.read_to_string(&mut got);
    got
}

fn files(folder: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(folder)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn a_good_report_is_kept_and_its_id_returned() {
    let root = scratch("report_good");
    let (h, log) = desk(&root, |_| {});
    let z = zip(5000);
    let id = send(h.addr(), &z, "La partita si è bloccata: 試合").unwrap();
    assert!(
        id.len() == 6 && id.bytes().all(|b| b.is_ascii_alphanumeric()),
        "{id}"
    );
    let f = files(&root);
    assert_eq!(f.len(), 2, "{f:?}");
    let zip_file = f.iter().find(|p| p.extension().unwrap() == "zip").unwrap();
    assert!(
        zip_file
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(&format!("_{id}.zip"))
    );
    assert_eq!(std::fs::read(zip_file).unwrap(), z);
    let txt = std::fs::read_to_string(zip_file.with_extension("txt")).unwrap();
    assert!(
        txt.contains(&format!("id: {id}")) && txt.contains("edition: 2017-18"),
        "{txt}"
    );
    assert!(
        txt.contains("version: 0.3.2") && txt.ends_with("La partita si è bloccata: 試合\n"),
        "{txt}"
    );
    let log = log.lock().unwrap().join("\n");
    assert!(
        log.contains(&format!(
            "report {id}: edition 2017-18, version 0.3.2, 4 KB, from 127.0.0.1"
        )),
        "{log}"
    );
}

#[test]
fn too_big_too_long_or_not_utf8_is_refused() {
    let root = scratch("report_caps");
    let (h, _) = desk(&root, |_| {});
    let big = format!("REPORT {} 0 v\n", MAX_ZIP + 1);
    assert!(answer(h.addr(), big.as_bytes()).starts_with("NO too big"));
    assert!(answer(h.addr(), b"REPORT 0 0 v\n").starts_with("NO too big"));
    assert!(answer(h.addr(), b"REPORT 10 9000 v\n").starts_with("NO the words are too long"));
    assert!(answer(h.addr(), b"REPORT ten 0 v\n").starts_with("NO a bad request"));
    // 2,001 characters in fewer bytes than the byte cap
    let long = "a".repeat(2001);
    assert!(
        matches!(send(h.addr(), &zip(100), &long), Err(SendError::Refused(w)) if w.contains("too long"))
    );
    // not UTF-8, not a ZIP: refused after the payload
    let mut c = TcpStream::connect(h.addr()).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    c.write_all(b"REPORT 4 2 v\n\xff\xfePK\x03\x04").unwrap();
    let mut got = String::new();
    let _ = c.read_to_string(&mut got);
    assert_eq!(got, "GO\nNO the words are not UTF-8\n");
    assert!(matches!(send(h.addr(), b"ABCD", "x"), Err(SendError::Refused(w)) if w == "not a ZIP"));
    assert!(files(&root).is_empty());
}

#[test]
fn too_many_from_one_address_are_refused() {
    let root = scratch("report_rate");
    let (h, _) = desk(&root, |r| r.per_hour = 2);
    assert!(send(h.addr(), &zip(100), "1").is_ok());
    assert!(send(h.addr(), &zip(100), "2").is_ok());
    let third = send(h.addr(), &zip(100), "3");
    assert!(
        matches!(&third, Err(SendError::Refused(w)) if w.starts_with("too many")),
        "{third:?}"
    );
    assert_eq!(files(&root).len(), 4);
}

#[test]
fn the_oldest_go_to_keep_the_total() {
    let root = scratch("report_prune");
    let keep = 2500;
    let (h, log) = desk(&root, |r| r.keep = keep);
    let mut ids = vec![];
    for i in 0..4 {
        ids.push(send(h.addr(), &zip(1000), &format!("report {i}")).unwrap());
        let total: u64 = files(&root)
            .iter()
            .map(|p| p.metadata().unwrap().len())
            .sum();
        assert!(total <= keep, "{total} > {keep}");
        std::thread::sleep(Duration::from_millis(50));
    }
    let left: Vec<String> = files(&root)
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(
        left.iter().any(|n| n.contains(&ids[3])),
        "the newest stays: {left:?}"
    );
    assert!(
        !left.iter().any(|n| n.contains(&ids[0])),
        "the oldest went: {left:?}"
    );
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("reports full: removed the oldest"))
    );
}

#[test]
fn a_desk_without_a_folder_keeps_no_report() {
    let mut c = DeskConfig::new(Edition::E1011);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    let (log, _) = capture();
    let h = start(c, log).unwrap();
    assert!(matches!(
        send(h.addr(), &zip(100), "x"),
        Err(SendError::Refused(_))
    ));
}
