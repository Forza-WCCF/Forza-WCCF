//! Shared by the replay tests: the recordings (`tests/rec/*.jsonl`, made by `legacy/2010-11/tests/rec_i2_*.py` against
//! the Python server pieces) and the interpreter of their TCP and UDP operations.
#![allow(dead_code)]

use forza_server::Log;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;

/// The package folder at run time first: nextest's `--workspace-remap` sets it for an archive built on another machine.
pub fn manifest_dir() -> PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map_or_else(|| env!("CARGO_MANIFEST_DIR").into(), PathBuf::from)
}

/// The built `forza-server`: at run time first (cargo test and nextest set it; an archive's copy is elsewhere).
pub fn server_exe() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_forza-server")
        .map_or_else(|| env!("CARGO_BIN_EXE_forza-server").into(), PathBuf::from)
}

pub fn records(name: &str) -> Vec<Value> {
    let path = manifest_dir()
        .join("tests/rec")
        .join(format!("{name}.jsonl"));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

pub fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn scratch(what: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("forza_server_{what}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A log that keeps its lines for the tests to search.
pub fn capture() -> (Log, Arc<Mutex<Vec<String>>>) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    (
        Arc::new(move |m| sink.lock().unwrap().push(m.to_string())),
        lines,
    )
}

/// The inbox's answer with its random code replaced.
pub fn norm_ok(got: &[u8]) -> Vec<u8> {
    let alphabet = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    if got.len() == 9
        && got.starts_with(b"OK ")
        && got.ends_with(b"\n")
        && got[3..8].iter().all(|b| alphabet.contains(b))
    {
        b"OK <CODE>\n".to_vec()
    } else {
        got.to_vec()
    }
}

/// Runs the TCP operations of a recording against a server, one client socket per id. `other` gets the operations
/// this does not know (`ctl`, `fw`, `log`, `ls`).
pub struct Tcp {
    pub addr: SocketAddr,
    conns: HashMap<String, TcpStream>,
}

impl Tcp {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            conns: HashMap::new(),
        }
    }

    /// Returns false for an operation it does not know.
    pub fn run(&mut self, op: &Value, name: &str, i: usize) -> bool {
        let id = op["c"].as_str().unwrap_or("").to_string();
        match op["op"].as_str().unwrap() {
            "open" => {
                let c = TcpStream::connect(self.addr).unwrap();
                self.conns.insert(id, c);
            }
            "send" => {
                let c = self.conns.get_mut(&id).unwrap();
                // a server that already hung up may refuse the rest; the recorded reply says what was expected
                let _ = c.write_all(&unhex(op["hex"].as_str().unwrap()));
            }
            "shut" => {
                let _ = self.conns[&id].shutdown(Shutdown::Write);
            }
            "close" => {
                self.conns.remove(&id);
            }
            "sleep" => sleep(Duration::from_secs_f64(op["s"].as_f64().unwrap())),
            "recv" => {
                let c = self.conns.get_mut(&id).unwrap();
                let want = unhex(op["got"].as_str().unwrap());
                let wait = Duration::from_secs_f64(op["wait"].as_f64().unwrap());
                // a slow first answer (a cold start on Windows) is patient; once bytes flow, quiet for `wait` ends it
                let first = if want.is_empty() && !op["eof"].as_bool().unwrap() {
                    wait
                } else {
                    Duration::from_secs(5)
                };
                let (got, eof) = read_all(c, first, wait);
                assert_eq!(
                    (hex(&norm_ok(&got)), eof),
                    (
                        hex(&norm_ok(&unhex(op["got"].as_str().unwrap()))),
                        op["eof"].as_bool().unwrap()
                    ),
                    "{name} op {i}: {id} read {:?}",
                    String::from_utf8_lossy(&got)
                );
            }
            _ => return false,
        }
        true
    }
}

/// Everything the server sends until it is quiet for `wait` or hangs up (true).
pub fn read_all(c: &mut TcpStream, first: Duration, wait: Duration) -> (Vec<u8>, bool) {
    let (mut got, mut buf) = (Vec::new(), [0u8; 65536]);
    loop {
        c.set_read_timeout(Some(if got.is_empty() { first } else { wait }))
            .unwrap();
        match c.read(&mut buf) {
            Ok(0) => return (got, true),
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return (got, false);
            }
            Err(_) => return (got, true),
        }
    }
}

pub fn udp_client() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").unwrap()
}
