//! LAN shops (2017-18): how a guest's Launcher finds the shops on its own network, with no internet. The guest sends
//! a query to UDP [`PORT`] as a broadcast on every IPv4 network it is on; a shop's `forza-server all --lan NAME`
//! answers the asker alone. Nothing listens on the guest's side but its own query's socket.
//!
//! Every packet has one fixed layout (a parser's trust boundary: any other length or byte is not a packet):
//! - query: `FWLN`, version 1, kind 0, edition (1 = 2010-11, 2 = 2017-18): 7 bytes;
//! - answer: `FWLN`, version 1, kind 1, edition, the seat desk's TCP port (big-endian u16), the name's length n
//!   (at most [`NAME_MAX`]) and n bytes of UTF-8 without control characters: 10 + n bytes.

use crate::service::{Handle, Log};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// The UDP port a LAN shop answers on (not one of the game's: 20000-20008, 20030-20032, 20040, 20050, 20060).
pub const PORT: u16 = 20035;
/// A shop name's most bytes on the wire (longer names are cut at a character).
pub const NAME_MAX: usize = 32;
/// The edition byte of a 2017-18 shop.
pub const E1718: u8 = 2;

const VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Packet {
    Query {
        edition: u8,
    },
    Answer {
        edition: u8,
        desk_port: u16,
        name: String,
    },
}

impl Packet {
    pub fn bytes(&self) -> Vec<u8> {
        let mut b = b"FWLN".to_vec();
        match self {
            Packet::Query { edition } => b.extend([VERSION, 0, *edition]),
            Packet::Answer {
                edition,
                desk_port,
                name,
            } => {
                let mut end = name.len().min(NAME_MAX);
                while !name.is_char_boundary(end) {
                    end -= 1;
                }
                b.extend([VERSION, 1, *edition]);
                b.extend(desk_port.to_be_bytes());
                b.push(u8::try_from(end).unwrap_or_default()); // end <= NAME_MAX
                b.extend(&name.as_bytes()[..end]);
            }
        }
        b
    }

    /// A packet, or None for anything else (other magic, version, kind or length, a bad name).
    pub fn parse(b: &[u8]) -> Option<Packet> {
        let [b'F', b'W', b'L', b'N', VERSION, kind, edition, rest @ ..] = b else {
            return None;
        };
        match (kind, rest) {
            (0, []) => Some(Packet::Query { edition: *edition }),
            (1, [hi, lo, n, name @ ..])
                if usize::from(*n) == name.len() && name.len() <= NAME_MAX =>
            {
                let name = std::str::from_utf8(name)
                    .ok()
                    .filter(|s| !s.chars().any(char::is_control))?;
                Some(Packet::Answer {
                    edition: *edition,
                    desk_port: u16::from_be_bytes([*hi, *lo]),
                    name: name.to_owned(),
                })
            }
            _ => None,
        }
    }
}

/// A shop that answered: its address as the guest saw it, its seat desk's port and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub ip: Ipv4Addr,
    pub desk_port: u16,
    pub name: String,
}

/// Answers every query for `edition` on UDP `bind:port` with this shop's name and desk port, until the handle drops.
/// Only askers on a private, link-local or loopback address get an answer (never one from the internet).
pub fn serve(
    bind: &str,
    port: u16,
    edition: u8,
    desk_port: u16,
    name: &str,
    log: Log,
) -> io::Result<Handle> {
    let sock = UdpSocket::bind((bind, port))?;
    sock.set_read_timeout(Some(Duration::from_millis(100)))?;
    let answer = Packet::Answer {
        edition,
        desk_port,
        name: name.to_owned(),
    }
    .bytes();
    log(&format!(
        "LAN shop \"{name}\": answering searches on UDP {port}"
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = thread::spawn(move || {
        let mut buf = [0u8; 64];
        while !flag.load(Ordering::Relaxed) {
            if let Ok((n, from)) = sock.recv_from(&mut buf)
                && local(from.ip())
                && Packet::parse(&buf[..n]) == Some(Packet::Query { edition })
            {
                let _ = sock.send_to(&answer, from);
            }
        }
    });
    Ok(Handle::new(stop, thread))
}

fn local(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v) if v.is_private() || v.is_link_local() || v.is_loopback())
}

/// Asks every IPv4 network this PC is on (and this PC itself) for `edition`'s shops on UDP `port`, and lists the
/// answers that came within `wait`, one per address.
pub fn find(port: u16, edition: u8, wait: Duration) -> io::Result<Vec<Found>> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    sock.set_broadcast(true)?;
    let query = Packet::Query { edition }.bytes();
    // 255.255.255.255 leaves by one interface only on Windows: each network's own broadcast address too
    let mut to = vec![Ipv4Addr::BROADCAST, Ipv4Addr::LOCALHOST];
    to.extend(
        networks()
            .filter(|n| n.prefix_len() < 31)
            .map(|n| n.broadcast()),
    );
    for ip in to {
        let _ = sock.send_to(&query, (ip, port));
    }
    let end = Instant::now() + wait;
    let mut found: Vec<Found> = Vec::new();
    let mut buf = [0u8; 64];
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        sock.set_read_timeout(Some(left))?;
        match sock.recv_from(&mut buf) {
            Ok((n, SocketAddr::V4(from))) => {
                if let Some(Packet::Answer {
                    edition: e,
                    desk_port,
                    name,
                }) = Packet::parse(&buf[..n])
                    && e == edition
                    && !found.iter().any(|f| f.ip == *from.ip())
                {
                    found.push(Found {
                        ip: *from.ip(),
                        desk_port,
                        name,
                    });
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            // Windows reports an ICMP "port unreachable" for an earlier send as an error here: not the end
            _ => {}
        }
    }
    Ok(found)
}

/// This PC's IPv4 addresses that others on its network can reach: the private ones of every interface that is up,
/// else any that is not loopback.
pub fn addresses() -> Vec<Ipv4Addr> {
    let all: Vec<Ipv4Addr> = networks().map(|n| n.addr()).collect();
    let private: Vec<Ipv4Addr> = all.iter().copied().filter(Ipv4Addr::is_private).collect();
    if private.is_empty() { all } else { private }
}

/// The private network this PC reaches others through: its default route's interface, else the first private one
/// that is up; its address and prefix length. `None` when no private IPv4 network is up (the tablet board refuses to
/// listen).
pub fn private_net() -> Option<(Ipv4Addr, u8)> {
    let default = netdev::get_default_interface()
        .ok()
        .into_iter()
        .flat_map(|i| i.ipv4);
    default
        .chain(networks())
        .find(|n| n.addr().is_private())
        .map(|n| (n.addr(), n.prefix_len()))
}

fn networks() -> impl Iterator<Item = netdev::ipnet::Ipv4Net> {
    netdev::get_interfaces()
        .into_iter()
        .filter(|i| i.is_up() && !i.is_loopback())
        .flat_map(|i| i.ipv4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_round_trip_and_bad_input_is_none() {
        let q = Packet::Query { edition: E1718 };
        assert_eq!(q.bytes(), b"FWLN\x01\x00\x02");
        assert_eq!(Packet::parse(&q.bytes()), Some(q));
        let a = Packet::Answer {
            edition: E1718,
            desk_port: 20030,
            name: "PABLO-PC".into(),
        };
        let b = a.bytes();
        assert_eq!(b.len(), 10 + 8);
        assert_eq!(Packet::parse(&b), Some(a));
        // every cut, every extra byte, and every single-byte change of the header is no packet
        for n in 0..b.len() {
            assert_eq!(Packet::parse(&b[..n]), None, "cut at {n}");
        }
        assert_eq!(Packet::parse(&[b.as_slice(), b"x"].concat()), None);
        for i in [0, 3, 4, 5] {
            let mut x = b.clone();
            x[i] ^= 0x40;
            assert_eq!(Packet::parse(&x), None, "byte {i}");
        }
        // a name length that lies, bad UTF-8, a control character, over NAME_MAX
        assert_eq!(Packet::parse(b"FWLN\x01\x01\x02\x4e\x3e\x05ab"), None);
        assert_eq!(Packet::parse(b"FWLN\x01\x01\x02\x4e\x3e\x02\xff\xfe"), None);
        assert_eq!(Packet::parse(b"FWLN\x01\x01\x02\x4e\x3e\x02a\n"), None);
        let long = [b"FWLN\x01\x01\x02\x4e\x3e\x21".as_slice(), &[b'a'; 33]].concat();
        assert_eq!(Packet::parse(&long), None);
        // a long name is cut at a character, and still parses
        let a = Packet::Answer {
            edition: E1718,
            desk_port: 1,
            name: "é".repeat(20),
        };
        assert_eq!(
            Packet::parse(&a.bytes()),
            Some(Packet::Answer {
                edition: E1718,
                desk_port: 1,
                name: "é".repeat(16),
            })
        );
        // no input crashes the parser
        for len in 0u8..40 {
            for seed in 0..64u8 {
                let junk: Vec<u8> = (0..len).map(|i| i.wrapping_mul(31) ^ seed).collect();
                let _ = Packet::parse(&junk);
            }
        }
    }

    #[test]
    fn a_shop_on_this_pc_is_found_and_another_edition_is_not() {
        let log: Log = Arc::new(|_| {});
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let _shop = serve("127.0.0.1", port, E1718, 20030, "HOST-PC", log).unwrap();
        let found = find(port, E1718, Duration::from_millis(500)).unwrap();
        assert_eq!(
            found,
            vec![Found {
                ip: Ipv4Addr::LOCALHOST,
                desk_port: 20030,
                name: "HOST-PC".into()
            }]
        );
        assert!(
            find(port, 1, Duration::from_millis(300))
                .unwrap()
                .is_empty()
        );
    }
}
