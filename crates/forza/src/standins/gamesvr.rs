//! Local stand-in for the 2017–18 game server ("gmsvr") that `control_Release.exe` relays club requests to.
//! Port of `_gamesvr.py`: 40-byte little-endian header (magic, command, sequence, status, size, connection id,
//! checksum, text), request N answered by N + 1 with an all-zero payload of the right size whose first u32 echoes
//! the cabinet's slot. The control program's framing: 0x40A970 build, 0x40ABE0 check, 0x40AB80 checksum.

use super::{Handle, Log, accept_loop, read_some};
use std::io::{self, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];
/// Response command and payload size (control.c response dispatcher 0x408C50 and the message table).
const RESP: &[(u16, usize)] = &[
    (202, 16),
    (224, 92),
    (226, 260),
    (228, 12),
    (234, 1092),
    (238, 0x5A54),
    (240, 12),
    (244, 12),
    (246, 248),
    (248, 12),
    (250, 504),
    (256, 1008),
    (258, 12),
    (260, 10168),
    (262, 2040),
    (264, 10080),
    (266, 200),
    (268, 33600),
    (272, 9292),
    (274, 1288),
    (276, 6176),
    (278, 76),
    (280, 16),
    (282, 8),
    (284, 8),
    (286, 704),
    (288, 12),
    (290, 12),
    (292, 20),
    (294, 8),
    (296, 16),
    (298, 16),
    (300, 8),
    (302, 8),
    (304, 8),
    (306, 1004),
    (308, 3332),
    (310, 8),
    (312, 3352),
    (314, 8),
    (316, 8),
    (318, 32),
];

fn checksum(h: &[u8]) -> u8 {
    h.iter().enumerate().fold(
        0u8,
        |s, (i, b)| if i == 24 { s } else { s.wrapping_add(*b) },
    )
}

fn header(cmd: u16, seq: u16, status: u32, size: usize, conn: u32) -> [u8; 40] {
    let mut h = [0u8; 40];
    h[..8].copy_from_slice(&MAGIC);
    h[8..10].copy_from_slice(&cmd.to_le_bytes());
    h[10..12].copy_from_slice(&seq.to_le_bytes());
    h[12..16].copy_from_slice(&status.to_le_bytes());
    h[16..20].copy_from_slice(&((40 + size) as u32).to_le_bytes());
    h[20..24].copy_from_slice(&conn.to_le_bytes());
    h[25..40].copy_from_slice(b"000000000000000");
    h[24] = checksum(&h);
    h
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// The response to one request: (command, payload), or `None` for a command the table does not know.
fn answer(cmd: u16, payload: &[u8], save_dir: &Path) -> Option<(u16, Vec<u8>)> {
    if cmd == 18 {
        // GMSVR_ECHO keep-alive
        let mut b = payload[..payload.len().min(4)].to_vec();
        b.resize(4, 0);
        return Some((18, b));
    }
    let rcmd = cmd.wrapping_add(1);
    let size = RESP.iter().find(|(c, _)| *c == rcmd)?.1;
    let mut body = vec![0u8; size];
    if size >= 4 && payload.len() >= 4 {
        body[..4].copy_from_slice(&payload[..4]);
    }
    if matches!(cmd, 307 | 311) {
        // GHOST_TEAM, BEST_ELEVEN: errCode 0 would install an all-zero team; 1 = none, the client plays a CPU team
        // (docs/research/ONLINE-2017-18.md)
        body[4..8].copy_from_slice(&1u32.to_le_bytes());
    }
    if cmd == 277 {
        // SAVE_DATA_SET: keep it
        let _ = std::fs::create_dir_all(save_dir);
        let aime = if payload.len() >= 8 {
            u32_at(payload, 4)
        } else {
            0
        };
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let _ = std::fs::write(
            save_dir.join(format!("save_set_{aime}_{secs}.bin")),
            payload,
        );
    }
    Some((rcmd, body))
}

/// Serves the game server stand-in on `listener` (use `127.0.0.1:0` in tests). Requests are logged, their payloads
/// kept in `save_dir`. Stops when the returned handle is dropped.
pub fn spawn(listener: TcpListener, save_dir: PathBuf, log: Log) -> io::Result<Handle> {
    std::fs::create_dir_all(&save_dir)?;
    let stop = Arc::new(AtomicBool::new(false));
    let t0 = Instant::now();
    let flag = stop.clone();
    accept_loop(listener, stop, move |mut conn, peer| {
        log(&format!("connected {peer}"));
        let mut buf: Vec<u8> = Vec::new();
        let mut tmp = vec![0u8; 65536];
        loop {
            match read_some(&mut conn, &mut tmp, &flag) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(e) => {
                    log(&format!("recv error {e}"));
                    break;
                }
            }
            while buf.len() >= 40 {
                if buf[..8] != MAGIC {
                    let skip = buf
                        .windows(8)
                        .skip(1)
                        .position(|w| w == MAGIC)
                        .map(|i| i + 1);
                    log(&format!(
                        "resync: dropped {} bytes",
                        skip.unwrap_or(buf.len())
                    ));
                    buf.drain(..skip.unwrap_or(buf.len()));
                    continue;
                }
                // ponytail: the Python hangs on a size below 40; floor it so a bad header cannot spin.
                let size = (u32_at(&buf, 16) as usize).max(40);
                if buf.len() < size {
                    break;
                }
                let (cmd, seq) = (
                    u16::from_le_bytes([buf[8], buf[9]]),
                    u16::from_le_bytes([buf[10], buf[11]]),
                );
                let (status, id) = (u32_at(&buf, 12), u32_at(&buf, 20));
                let payload = buf[40..size].to_vec();
                let text =
                    String::from_utf8_lossy(buf[25..40].split(|b| *b == 0).next().unwrap_or(&[]))
                        .into_owned();
                buf.drain(..size);
                let first: String = payload
                    .iter()
                    .take(48)
                    .map(|b| format!("{b:02x}"))
                    .collect();
                log(&format!(
                    "<- cmd {cmd} seq {seq} status {status} payload {} conn {id} text {text:?} first {first}",
                    payload.len()
                ));
                if !payload.is_empty() {
                    let name = format!("req_{cmd}_{}.bin", t0.elapsed().as_millis());
                    let _ = std::fs::write(save_dir.join(name), &payload);
                }
                match answer(cmd, &payload, &save_dir) {
                    Some((rcmd, body)) => {
                        let mut out = header(rcmd, seq, 1, body.len(), id).to_vec();
                        out.extend_from_slice(&body);
                        if conn.write_all(&out).is_err() {
                            return;
                        }
                        log(&format!("-> cmd {rcmd} payload {}", body.len()));
                    }
                    None => log(&format!("   (no answer for cmd {cmd})")),
                }
            }
        }
        log(&format!("closed {peer}"));
    })
}
