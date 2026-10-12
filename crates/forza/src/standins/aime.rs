//! Stand-in for the 2017–18 Aime card reader (the publisher's 837-15084 NFC reader, TN32MSEC003S, plus its LED
//! board), served over a named pipe. Port of `_aime_reader.py`.
//!
//! The client's aimelib 3.07.7 opens `\\.\COM2` (VA 0x00D8B0E0, synchronous); the winmm hook, started with
//! `MXHOOK_COM=COM2=PIPE_NAME`, hands it this pipe instead.
//!
//! Frames (client parser VA 0x00D87040; the protocol public reader emulators implement): `E0` then
//! `[len addr seq cmd plen payload.. sum]` from the game, `[len addr seq cmd status plen payload.. sum]` back;
//! `len` counts itself through the payload, `sum` is the byte sum of `len..payload`; after the `E0` every `E0`/`D0`
//! byte is sent as `D0, byte-1`. Address 0 is the NFC reader, commands from 0x80 go to the LED board at address 8.
//! One Mifare Aime card is always on the reader. Its 20-digit access code lives in the card file (a text file, made
//! with a random code the first time); block 2 bytes 6..15 carry it as BCD, as on a real card.

use super::{Handle, Log};
use std::collections::hash_map::RandomState;
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_NOWAIT, PIPE_READMODE_BYTE,
    PIPE_TYPE_BYTE, PeekNamedPipe,
};

const FW: &[u8] = b"TN32MSEC003S F/W Ver1.2"; // prog_data/firmware/TN32MSEC003S_V12.hex: no firmware update wanted
const HW: &[u8] = b"TN32MSEC003S H/W Ver3.0";
const LED_INFO: &[u8] = b"15084\xff\x10\x00\x12";

fn name(cmd: u8) -> &'static str {
    match cmd {
        0x30 => "fw version",
        0x32 => "hw version",
        0x40 => "radio on",
        0x41 => "radio off",
        0x42 => "poll",
        0x43 => "select tag",
        0x44 => "?44",
        0x50 => "set key B",
        0x51 => "auth B",
        0x52 => "read block",
        0x54 => "set key A",
        0x55 => "auth A",
        0x61 => "send hex",
        0x62 => "reset",
        0x71 => "felica",
        0x81 => "led color",
        0xF0 => "led info",
        0xF5 => "led reset",
        _ => "?",
    }
}

#[derive(Debug, PartialEq)]
struct Req {
    addr: u8,
    seq: u8,
    cmd: u8,
    payload: Vec<u8>,
    ok: bool,
}

fn escape(body: &[u8]) -> Vec<u8> {
    let mut out = vec![0xE0];
    for &b in body {
        if b == 0xE0 || b == 0xD0 {
            out.extend([0xD0, b - 1]);
        } else {
            out.push(b);
        }
    }
    out
}

fn response(req: &Req, status: u8, payload: &[u8]) -> Vec<u8> {
    let mut body = vec![
        6 + payload.len() as u8,
        req.addr,
        req.seq,
        req.cmd,
        status,
        payload.len() as u8,
    ];
    body.extend_from_slice(payload);
    body.push(body.iter().fold(0u8, |s, b| s.wrapping_add(*b)));
    escape(&body)
}

/// Bytes from the game -> (requests, bytes left over). Junk before an `E0` is dropped.
fn parse(stream: &[u8]) -> (Vec<Req>, Vec<u8>) {
    let (mut reqs, mut i) = (Vec::new(), 0);
    loop {
        let Some(start) = stream[i..].iter().position(|b| *b == 0xE0).map(|p| p + i) else {
            return (reqs, Vec::new());
        };
        let (mut body, mut j) = (Vec::<u8>::new(), start + 1);
        while j < stream.len() {
            let b = stream[j];
            if b == 0xE0 {
                break; // a new frame started: the old one was cut short
            }
            if b == 0xD0 {
                if j + 1 >= stream.len() {
                    return (reqs, stream[start..].to_vec());
                }
                body.push(stream[j + 1].wrapping_add(1));
                j += 2;
            } else {
                body.push(b);
                j += 1;
            }
            if !body.is_empty() && body.len() == body[0] as usize + 1 {
                break;
            }
        }
        if body.is_empty() || body.len() < body[0] as usize + 1 {
            if j >= stream.len() {
                return (reqs, stream[start..].to_vec());
            }
            i = j;
            continue;
        }
        let n = body[0] as usize;
        // ponytail: the Python raises on a frame shorter than addr/seq/cmd/plen; skip it here
        if n >= 5 {
            let sum = body[..n].iter().fold(0u8, |s, b| s.wrapping_add(*b));
            reqs.push(Req {
                addr: body[1],
                seq: body[2],
                cmd: body[3],
                payload: body[5..n].to_vec(),
                ok: sum == body[n],
            });
        }
        i = j;
    }
}

/// The card on the reader.
struct Card {
    code: String,
    uid: [u8; 4],
}

impl Card {
    fn load(path: Option<&Path>) -> Self {
        let from_file = path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| s.len() == 20 && s.bytes().all(|b| b.is_ascii_digit()));
        let code = from_file.unwrap_or_else(|| {
            // ponytail: RandomState is the std source of randomness; fine for a stand-in card number
            let r = RandomState::new().build_hasher().finish() % 10_000_000_000_000_000_000;
            let code = format!("5{r:019}");
            if let Some(p) = path {
                let _ = std::fs::create_dir_all(p.parent().unwrap_or(Path::new(".")));
                let tmp = p.with_extension("tmp");
                if std::fs::write(&tmp, format!("{code}\n")).is_ok() {
                    let _ = std::fs::rename(&tmp, p);
                }
            }
            code
        });
        let bcd = |i: usize| u8::from_str_radix(&code[i..i + 2], 16).unwrap_or(0);
        let uid = [bcd(12), bcd(14), bcd(16), bcd(18)]; // 4-byte Mifare UID, stable per card
        Self { code, uid }
    }

    fn block(&self, n: u8) -> Vec<u8> {
        let mut b = vec![0u8; 16];
        match n {
            0 => {
                b[..4].copy_from_slice(&self.uid);
                b[4] = self.uid.iter().fold(0, |x, y| x ^ y);
            }
            2 => {
                for i in 0..10 {
                    b[6 + i] = u8::from_str_radix(&self.code[i * 2..i * 2 + 2], 16).unwrap_or(0);
                }
            }
            _ => {}
        }
        b
    }
}

/// (status, data); a `None` status means the board does not answer.
fn answer(card: &Card, req: &Req) -> (Option<u8>, Vec<u8>) {
    match req.cmd {
        0x30 => (Some(0), FW.to_vec()),
        0x32 => (Some(0), HW.to_vec()),
        0x42 => (Some(0), [&[1, 0x10, 4][..], &card.uid].concat()), // one Mifare card: count, type, id length, UID
        0x52 => (
            Some(0),
            card.block(req.payload.get(4).copied().unwrap_or(0)),
        ), // payload: UID(4) block(1)
        0xF0 => (Some(0), LED_INFO.to_vec()),
        0x81 => (None, Vec::new()), // LED colour
        _ => (Some(0), Vec::new()), // radio on/off, select, keys, auth, reset, hex data
    }
}

fn hexs(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Creates the named pipe `pipe_name` (`\\.\pipe\...`, one instance) and serves the reader on it until the returned
/// handle is dropped. Fails when the pipe cannot be made (another reader running). `card_file` holds the card's
/// 20-digit access code; it is made with a random code when missing.
pub fn spawn(pipe_name: &str, card_file: Option<&Path>, log: Log) -> io::Result<Handle> {
    let wide: Vec<u16> = pipe_name.encode_utf16().chain([0]).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call; no security attributes are passed.
    let raw = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT, // no-wait: every call returns, so `stop` is seen
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a valid pipe handle we own; the File closes it.
    let mut pipe = File::from(unsafe { OwnedHandle::from_raw_handle(raw) });
    let card = Card::load(card_file);
    log(&format!(
        "Aime reader stand-in on {pipe_name}; card access code {} on the reader",
        card.code
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = thread::spawn(move || {
        let h = pipe.as_raw_handle();
        let mut buf = [0u8; 4096];
        while !flag.load(Ordering::Relaxed) {
            // SAFETY: `h` is the pipe handle `pipe` owns; a null overlapped is allowed for a non-overlapped pipe.
            let connected = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0
                // SAFETY: reads the calling thread's last error.
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if !connected {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            log("game opened the reader port");
            let (mut pending, mut last_poll_log) = (Vec::new(), None::<Instant>);
            while !flag.load(Ordering::Relaxed) {
                let mut avail = 0u32;
                // SAFETY: `h` is valid; only the bytes-available out-pointer is given.
                let alive = unsafe {
                    PeekNamedPipe(
                        h,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut avail,
                        std::ptr::null_mut(),
                    )
                };
                if alive == 0 {
                    log("game closed the port");
                    break;
                }
                if avail == 0 {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                let Ok(n) = pipe.read(&mut buf[..(avail as usize).min(4096)]) else {
                    continue;
                };
                pending.extend_from_slice(&buf[..n]);
                let (reqs, rest) = parse(&pending);
                pending = rest;
                for r in reqs {
                    let (status, data) = answer(&card, &r);
                    let quiet = matches!(r.cmd, 0x42 | 0x81)
                        && last_poll_log.is_some_and(|t| t.elapsed().as_secs() < 5);
                    if !quiet {
                        log(&format!(
                            "<- addr {} seq {:3} cmd {:02x} {:<10} [{}]{}  -> {}",
                            r.addr,
                            r.seq,
                            r.cmd,
                            name(r.cmd),
                            hexs(&r.payload),
                            if r.ok { "" } else { " SUM WRONG" },
                            status.map_or("no answer".to_string(), |s| format!(
                                "st {s} [{}]",
                                hexs(&data)
                            ))
                        ));
                        if matches!(r.cmd, 0x42 | 0x81) {
                            last_poll_log = Some(Instant::now());
                        }
                    }
                    if let Some(s) = status {
                        let _ = pipe.write_all(&response(&r, s, &data));
                    }
                }
            }
            // SAFETY: `h` is the pipe handle `pipe` owns.
            unsafe { DisconnectNamedPipe(h) };
        }
        log("stopping");
    });
    Ok(Handle::new(stop, thread))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of `_aime_reader.py`'s selftest: frame round trip incl. escaped bytes and a split read.
    #[test]
    fn frame_round_trip() {
        let body = [5 + 3, 0, 0xD0, 0x42, 3, 0xE0, 0x01, 0xD0];
        let sum = body.iter().fold(0u8, |s, b| s.wrapping_add(*b));
        let raw = escape(&[&body[..], &[sum]].concat());
        let (reqs, left) = parse(&raw[..4]);
        assert!(reqs.is_empty() && left == raw[..4]);
        let (reqs, left) = parse(&[&left[..], &raw[4..]].concat());
        assert!(left.is_empty() && reqs.len() == 1 && reqs[0].ok);
        assert_eq!((reqs[0].cmd, reqs[0].seq), (0x42, 0xD0));
        assert_eq!(reqs[0].payload, [0xE0, 0x01, 0xD0]);
        let r = response(&reqs[0], 0, &[0xE0]);
        assert!(r[0] == 0xE0 && r[1..].windows(2).any(|w| w == [0xD0, 0xDF]));
    }
}
