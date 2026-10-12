//! Stand-ins for the cabinet's background services, for 2010–11 on a normal PC. Port of `network.py`.
//!
//! The launcher and the game talk to them over TCP 40100–40107 (see [`SERVICES`]): the master (40100), the
//! installer (40102/40103) and the keychip (40106/40107) speak PCP, a '>' greeting, then one line per request,
//! `key=value&key=value`, where `?` asks for a value; the answer is the same form, then the prompt '>' again
//! (without it the launcher spins). The first port of a pair is text, the second binary (only logged). amNetwork on
//! 40104 answers `request=...` lines with `response=...&result=0&...` and no trailing prompt (the prompt would keep
//! the game's receive open and its boot state machine would never pass "GET DHCP STATUS"). Values marked as guesses
//! in `network.py` stay guesses. Everything is bytes (the Python decoded as latin-1, which is the identity).

use super::{Handle, Log, accept_loop, read_some};
use std::collections::HashMap;
use std::io::{self, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Which protocol a port speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Master, installer and keychip text ports: answers keep the '>' prompt.
    Text,
    /// amNetwork: answers have no prompt.
    Amnet,
    /// Binary ports: nothing is sent; what arrives is logged.
    Binary,
}

/// The game's ports, their names (used in the log) and protocols. Callers bind these on 127.0.0.1; tests pass
/// their own listeners to [`spawn`].
pub const SERVICES: [(u16, &str, Kind); 6] = [
    (40100, "master", Kind::Text),
    (40102, "installer", Kind::Text),
    (40103, "installer-bin", Kind::Binary),
    (40104, "amnetwork", Kind::Amnet),
    (40106, "keychip", Kind::Text),
    (40107, "keychip-bin", Kind::Binary),
];

/// Answers to `key=?` (and to any key listed here that is not in `EXTRA`). DOC/DISC/GUESS notes: `network.py`.
const ANSWERS: [(&str, &str); 16] = [
    ("keychip.status", "available"),
    ("keychip.version", "0104"),
    ("keychip.appboot.gameid", "SBWG"),
    ("keychip.appboot.platformid", "AAL"),
    ("keychip.appboot.region", "1"),
    ("keychip.appboot.modeltype", "01"),
    ("keychip.appboot.networkaddr", "192.168.46.0"),
    ("keychip.appboot.systemflag", "00"),
    ("keychip.appboot.formattype", "1"),
    ("keychip.appboot.dvdflag", "00"),
    ("keychip.billing.keyid", "A72E-0123456"),
    ("keychip.billing.mainid", "A72E-0123456"),
    ("keychip.billing.playcount", "0"),
    ("keychip.billing.playlimit", "1024"),
    ("keychip.billing.nearfull", "0"),
    ("mxmaster.foreground.getcount", "1"),
];
/// Fields some answers carry after them; the master's first keyword echoes the question, then `count`.
const EXTRA: [(&str, &str); 1] = [("mxmaster.foreground.getcount", "count=1")];
/// amNetwork: request -> the field after `result=0`.
const AMNET_FIELDS: [(&str, &str); 9] = [
    ("query_dhcp_status", "dhcp_status=3"),
    ("update_dhcp_status", ""),
    ("query_nic_status", "status=3"),
    ("query_ip_address", "ip_address=192.168.46.46"),
    ("query_subnetmask", "subnetmask=255.255.255.0"),
    ("query_gateway", "gateway=192.168.46.254"),
    ("query_primary_dns", "primary_dns=192.168.46.254"),
    ("query_secondary_dns", "secondary_dns=192.168.46.254"),
    ("query_mac_address", "mac_address=0123456789AB"),
];

fn lookup(table: &[(&'static str, &'static str)], k: &[u8]) -> Option<&'static str> {
    table
        .iter()
        .find(|(n, _)| n.as_bytes() == k)
        .map(|(_, v)| *v)
}

/// Python's `str.strip()` on latin-1 text: ASCII whitespace, 0x1c–0x1f, 0x85 and 0xa0.
fn strip(mut s: &[u8]) -> &[u8] {
    let ws = |b: u8| matches!(b, 9..=13 | 0x1c..=0x20 | 0x85 | 0xa0);
    while let [first, rest @ ..] = s {
        if !ws(*first) {
            break;
        }
        s = rest;
    }
    while let [rest @ .., last] = s {
        if !ws(*last) {
            break;
        }
        s = rest;
    }
    s
}

/// `key=value` split at the first '='; no '=' is an empty value. Both stripped.
fn pair(part: &[u8]) -> (&[u8], &[u8]) {
    match part.iter().position(|&b| b == b'=') {
        Some(i) => (strip(&part[..i]), strip(&part[i + 1..])),
        None => (strip(part), b""),
    }
}

/// The PCP answer for one request line (master, installer, keychip).
pub fn answer(line: &[u8]) -> Vec<u8> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut keychip = false;
    for part in line.split(|&b| b == b'&') {
        let (k, v) = pair(part);
        keychip |= k.starts_with(b"keychip.");
        let known = lookup(&ANSWERS, k);
        let value: &[u8] = if v == b"?" || (known.is_some() && lookup(&EXTRA, k).is_none()) {
            known.unwrap_or("0").as_bytes()
        } else {
            v
        };
        out.push([k, b"=", value].concat());
        if let Some(e) = lookup(&EXTRA, k) {
            out.push(e.as_bytes().to_vec());
        }
    }
    let mut reply = out.join(&b'&');
    // The game's amDongle keychip client needs result=0 to treat a reply as complete.
    if keychip && !reply.windows(7).any(|w| w == b"result=") {
        reply.extend_from_slice(b"&result=0");
    }
    reply
}

/// The amNetwork answer for one request line.
pub fn amnet_answer(line: &[u8]) -> Vec<u8> {
    let kv: HashMap<&[u8], &[u8]> = line.split(|&b| b == b'&').map(pair).collect();
    let cmd = kv.get(&b"request"[..]).copied().unwrap_or(b"");
    let mut reply = [b"response=", cmd, b"&result=0"].concat();
    if let Some(f) = lookup(&AMNET_FIELDS, cmd)
        && !f.is_empty()
    {
        reply.push(b'&');
        reply.extend_from_slice(f.as_bytes());
    }
    if let Some(i) = kv.get(&b"if"[..]) {
        reply.extend_from_slice(b"&if=");
        reply.extend_from_slice(i);
    }
    reply
}

/// Latin-1 text of bytes for the log.
fn text(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// Serves one port of `kind` on `listener` (use `127.0.0.1:0` in tests); `name` heads its log lines. Stops when the
/// returned handle is dropped.
pub fn spawn(
    listener: TcpListener,
    kind: Kind,
    name: &'static str,
    log: Log,
) -> io::Result<Handle> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    accept_loop(listener, stop, move |conn, peer| {
        log(&format!("{name:<13} {} connected", peer.port()));
        let _ = client(conn, kind, name, &log, &flag);
        if kind == Kind::Binary {
            log(&format!("{name:<13} {} closed", peer.port()));
        }
    })
}

fn client(
    mut conn: TcpStream,
    kind: Kind,
    name: &str,
    log: &Log,
    stop: &AtomicBool,
) -> io::Result<()> {
    if kind != Kind::Binary {
        conn.write_all(b">")?;
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = read_some(&mut conn, &mut tmp, stop)?;
        if n == 0 {
            return Ok(());
        }
        if kind == Kind::Binary {
            let hex: String = tmp[..n.min(64)]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            log(&format!("{name:<13} got {n} bytes: {hex}"));
            continue;
        }
        buf.extend_from_slice(&tmp[..n]);
        while let Some(i) = buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=i).collect();
            let mut line = &raw[..raw.len() - 1];
            while let [rest @ .., b'\r'] = line {
                line = rest;
            }
            if line.is_empty() {
                continue;
            }
            let mut reply = if kind == Kind::Amnet {
                amnet_answer(line)
            } else {
                answer(line)
            };
            log(&format!("{name:<13} ask  {}", text(line)));
            log(&format!("{name:<13} say  {}", text(&reply)));
            reply.extend_from_slice(if kind == Kind::Amnet {
                b"\r\n"
            } else {
                b"\r\n>"
            });
            conn.write_all(&reply)?;
        }
    }
}
