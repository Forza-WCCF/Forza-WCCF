//! Stand-in for the 2010–11 player cabinet's JVS I/O board, served over a named pipe. Port of `io_board.py`.
//!
//! The game opens `\\.\mxjvs` (client `amJvstDriverSetup`); the winmm hook, started with
//! `MXHOOK_COM=mxjvs=\\.\pipe\wccf_jvs_seat1`, hands it this pipe (and drives the JVS sense line itself).
//!
//! JVS framing: `E0` (sync), node, length (= data bytes + 1), data..., checksum = (node + length + data) & FF.
//! After the sync byte, `E0` and `D0` are sent as `D0` then `byte - 1`. The game (master) sends to node `FF`
//! (broadcast) or to the board's address; the board answers to node `00`: STATUS (1 = normal, 2 = unknown command),
//! then for each command a REPORT (1 = normal) and its data.
//!
//! The switch and coin state and the game's outputs are shared with the key driver (`launcher::keys`) in memory
//! through [`Shared`]; the Python passed them through `_jvs_seat1_input.txt` and `_jvs_seat1_outputs.txt`.
//! Switch bits (p1/p2 first byte): 7 START, 6 SERVICE, 5 UP, 4 DOWN, 3 LEFT, 2 RIGHT, 1 BUTTON1, 0 BUTTON2; the second
//! byte holds buttons 3..10 from bit 7 down; `sys` bit 7 is TEST.

use super::{Handle, Log};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;
use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_NOWAIT, PIPE_READMODE_BYTE,
    PIPE_TYPE_BYTE, PeekNamedPipe,
};

const SYNC: u8 = 0xE0;
const MARK: u8 = 0xD0;
const BOARD_ID: &[u8] = b"SEGA ENTERPRISES,LTD.;I/O BD JVS;837-14572;Ver1.00;2005/10\0";
const FEATURES: &[u8] = &[
    0x01, 2, 13, 0, // switch inputs: 2 players, 13 switches each
    0x02, 2, 0, 0, // coin inputs: 2 slots
    0x03, 8, 10, 0, // analog inputs: 8 channels, 10 bits
    0x12, 20, 0, 0, // general-purpose outputs: 20
    0x00,
];
/// The card-in sensor: p2 first byte, bit 5.
pub const CARD_BIT: u8 = 0x20;

/// The switches and coin counters the board reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub sys: u8,
    /// Players 1 and 2, two bytes each.
    pub p: [[u8; 2]; 2],
    pub coin: [i64; 2],
}

/// What the key driver and the board share: the inputs the driver sets and the game's outputs the board keeps.
#[derive(Debug, Default)]
pub struct Shared {
    inputs: Mutex<Inputs>,
    outputs: Mutex<Option<Vec<u8>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    pub fn inputs(&self) -> Inputs {
        *lock(&self.inputs)
    }

    pub fn set_inputs(&self, i: Inputs) {
        *lock(&self.inputs) = i;
    }

    /// The driver's switches (the card bit included); the coin counters stay.
    pub fn set_switches(&self, sys: u8, p1: [u8; 2], p2: [u8; 2]) {
        let mut i = lock(&self.inputs);
        (i.sys, i.p) = (sys, [p1, p2]);
    }

    pub fn add_coin(&self, slot: usize, n: i64) {
        let mut i = lock(&self.inputs);
        i.coin[slot] = (i.coin[slot] + n).max(0);
    }

    /// The card is in the slot (what play's stop guard reads).
    pub fn card_in(&self) -> bool {
        lock(&self.inputs).p[1][0] & CARD_BIT != 0
    }

    /// The game's latest general-purpose output bytes (command 0x32), or `None` before the first.
    pub fn outputs(&self) -> Option<Vec<u8>> {
        lock(&self.outputs).clone()
    }
}

fn escape(body: &[u8]) -> Vec<u8> {
    let mut out = vec![SYNC];
    for &b in body {
        if b == SYNC || b == MARK {
            out.extend([MARK, b - 1]);
        } else {
            out.push(b);
        }
    }
    out
}

fn packet(node: u8, data: &[u8]) -> Vec<u8> {
    let length = (data.len() + 1) as u8;
    let total = data
        .iter()
        .fold(node.wrapping_add(length), |s, b| s.wrapping_add(*b));
    let mut body = vec![node, length];
    body.extend_from_slice(data);
    body.push(total);
    escape(&body)
}

fn hexs(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One request from the game.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub node: u8,
    pub data: Vec<u8>,
    /// The checksum matched (the board answers either way).
    pub ok: bool,
}

/// Takes every complete packet out of `raw`; an incomplete one stays for the next read.
pub fn take_packets(raw: &mut Vec<u8>) -> Vec<Request> {
    let mut out = Vec::new();
    loop {
        let Some(s) = raw.iter().position(|b| *b == SYNC) else {
            raw.clear();
            return out;
        };
        raw.drain(..s);
        let (mut body, mut j, mut mark, mut stray) = (Vec::<u8>::new(), 1, false, false);
        while j < raw.len() {
            let b = raw[j];
            j += 1;
            if mark {
                body.push(b.wrapping_add(1));
                mark = false;
            } else if b == MARK {
                mark = true;
            } else if b == SYNC {
                stray = true;
                break;
            } else {
                body.push(b);
            }
            if body.len() >= 2 && body.len() > body[1] as usize + 1 {
                break;
            }
        }
        let complete = body.len() >= 2 && body.len() > body[1] as usize + 1;
        if !complete {
            if stray {
                // the Python waits here forever (the stray sync stops every later scan); start at the new packet
                raw.drain(..j - 1);
                continue;
            }
            return out;
        }
        raw.drain(..j);
        let (node, length) = (body[0], body[1]);
        let data = body.get(2..=length as usize).unwrap_or(&[]).to_vec();
        let ok = data
            .iter()
            .fold(node.wrapping_add(length), |s, b| s.wrapping_add(*b))
            == body[1 + length as usize];
        out.push(Request { node, data, ok });
    }
}

/// The board: JVS commands in, replies out.
pub struct Board {
    addr: Option<u8>,
    shared: Arc<Shared>,
    log: Log,
    last: Option<Vec<u8>>,
}

impl Board {
    pub fn new(shared: Arc<Shared>, log: Log) -> Self {
        Self {
            addr: None,
            shared,
            log,
            last: None,
        }
    }

    fn note_outputs(&self, outs: Vec<u8>) {
        let mut cur = lock(&self.shared.outputs);
        if cur.as_ref() == Some(&outs) {
            return;
        }
        let text = if outs.is_empty() {
            "(none)".to_string()
        } else {
            hexs(&outs)
        };
        (self.log)(&format!("outputs now: {text}"));
        *cur = Some(outs);
    }

    /// One request packet -> the reply packet (or `None`).
    pub fn handle(&mut self, node: u8, data: &[u8]) -> Option<Vec<u8>> {
        if node != 0xFF && Some(node) != self.addr {
            return None;
        }
        let (mut out, mut i, mut status) = (Vec::<u8>::new(), 0usize, 1u8);
        let len = data.len();
        while i < len {
            let c = data[i];
            match c {
                0xF0 if i + 1 < len => {
                    // reset
                    self.addr = None;
                    (self.log)("  reset");
                    return None;
                }
                0xF1 if i + 1 < len => {
                    self.addr = Some(data[i + 1]);
                    (self.log)(&format!("  address set to {}", data[i + 1]));
                    out.push(1);
                    i += 2;
                }
                0x10 => {
                    out.push(1);
                    out.extend_from_slice(BOARD_ID);
                    i += 1;
                }
                0x11 => {
                    out.extend([1, 0x13]);
                    i += 1;
                }
                0x12 => {
                    out.extend([1, 0x30]);
                    i += 1;
                }
                0x13 => {
                    out.extend([1, 0x10]);
                    i += 1;
                }
                0x14 => {
                    out.push(1);
                    out.extend_from_slice(FEATURES);
                    i += 1;
                }
                0x15 => {
                    // main board id: a text up to its NUL
                    let j = data[i + 1..]
                        .iter()
                        .position(|b| *b == 0)
                        .map_or(len, |p| i + 1 + p + 1);
                    let text = data[i + 1..j].to_vec();
                    let end = text.iter().rposition(|b| *b != 0).map_or(0, |p| p + 1);
                    (self.log)(&format!(
                        "  main board says: {:?}",
                        String::from_utf8_lossy(&text[..end])
                    ));
                    out.push(1);
                    i = j;
                }
                0x20 if i + 2 < len => {
                    // switches: players, bytes each
                    let (players, nbytes) = (data[i + 1] as usize, data[i + 2] as usize);
                    let inp = self.shared.inputs();
                    out.extend([1, inp.sys]);
                    for pl in 0..players {
                        let sw = if pl < 2 { inp.p[pl] } else { [0, 0] };
                        out.extend((0..nbytes).map(|k| sw.get(k).copied().unwrap_or(0)));
                    }
                    i += 3;
                }
                0x21 if i + 1 < len => {
                    // coins: slots
                    let inp = self.shared.inputs();
                    out.push(1);
                    for s in 0..data[i + 1] as usize {
                        let n = if s < 2 { inp.coin[s] } else { 0 };
                        out.extend([((n >> 8) & 0x3F) as u8, (n & 0xFF) as u8]);
                    }
                    i += 2;
                }
                0x22 if i + 1 < len => {
                    // analog: channels
                    out.push(1);
                    out.resize(out.len() + 2 * data[i + 1] as usize, 0);
                    i += 2;
                }
                0x2F => return self.last.clone(), // retransmit
                0x30 | 0x31 if i + 3 < len => {
                    // coin decrease / increase
                    let s = i64::from(data[i + 1]) - 1;
                    let n = (i64::from(data[i + 2]) << 8) | i64::from(data[i + 3]);
                    if (0..2).contains(&s) {
                        self.shared
                            .add_coin(s as usize, if c == 0x30 { -n } else { n });
                    }
                    out.push(1);
                    i += 4;
                }
                0x32 if i + 1 < len => {
                    // general-purpose output: n bytes
                    let n = data[i + 1] as usize;
                    let end = (i + 2 + n).min(len);
                    self.note_outputs(data[i + 2..end].to_vec());
                    out.push(1);
                    i += 2 + n;
                }
                0x37 | 0x38 if i + 1 < len => {
                    // analog / character outputs
                    out.push(1);
                    i += 2 + if c == 0x38 { data[i + 1] as usize } else { 0 };
                }
                _ => {
                    (self.log)(&format!(
                        "  UNKNOWN command {c:02x} (rest {})",
                        hexs(&data[i..])
                    ));
                    status = 2;
                    break;
                }
            }
        }
        out.insert(0, status);
        let reply = packet(0, &out);
        self.last = Some(reply.clone());
        Some(reply)
    }
}

/// Creates the named pipe `pipe_name` (`\\.\pipe\...`, one instance) and serves the board on it until the returned
/// handle is dropped. Fails when the pipe cannot be made (another board running).
pub fn spawn(pipe_name: &str, shared: Arc<Shared>, log: Log) -> io::Result<Handle> {
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
    log(&format!("JVS I/O board stand-in on {pipe_name}"));
    let mut board = Board::new(shared, log.clone());
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = thread::spawn(move || {
        let h = pipe.as_raw_handle();
        let mut buf = [0u8; 4096];
        let mut polls = 0u64;
        while !flag.load(Ordering::Relaxed) {
            // SAFETY: `h` is the pipe handle `pipe` owns; a null overlapped is allowed for a non-overlapped pipe.
            let connected = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0
                // SAFETY: reads the calling thread's last error.
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if !connected {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            log("game connected");
            let mut raw = Vec::new();
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
                raw.extend_from_slice(&buf[..n]);
                for req in take_packets(&mut raw) {
                    let reply = board.handle(req.node, &req.data);
                    let routine = matches!(req.data.first(), Some(0x20 | 0x21)) && req.ok;
                    if routine {
                        polls += 1;
                        if polls % 600 == 1 {
                            log(&format!(
                                "<- node {:02x} {} (input polls so far: {polls})",
                                req.node,
                                hexs(&req.data)
                            ));
                        }
                    } else {
                        log(&format!(
                            "<- node {:02x} {}{}",
                            req.node,
                            hexs(&req.data),
                            if req.ok { "" } else { "  CHECKSUM WRONG" }
                        ));
                    }
                    if let Some(reply) = reply {
                        let _ = pipe.write_all(&reply);
                        if !routine {
                            log(&format!("-> {}", hexs(&reply)));
                        }
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

    #[test]
    fn packets_split_and_escaped() {
        let reply = packet(0, &[1, 0xE0, 0xD0]);
        assert!(
            reply.windows(2).any(|w| w == [MARK, 0xDF])
                && reply.windows(2).any(|w| w == [MARK, 0xCF])
        );
        let mut raw = reply[..3].to_vec();
        assert!(take_packets(&mut raw).is_empty());
        raw.extend_from_slice(&reply[3..]);
        let got = take_packets(&mut raw);
        assert!(raw.is_empty() && got.len() == 1 && got[0].ok);
        assert_eq!(
            (got[0].node, got[0].data.as_slice()),
            (0, &[1, 0xE0, 0xD0][..])
        );
    }

    #[test]
    fn stray_sync_resyncs() {
        let good = packet(0xFF, &[0x10]);
        let mut raw = [&[SYNC, 0xFF, 5][..], &good].concat();
        let got = take_packets(&mut raw);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].data, [0x10]);
    }
}
