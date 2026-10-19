//! Sans-IO core: loopback tokens, the seat map, the probe echo, `[seat]` framing and the routing rules. No sockets, no
//! clock, no randomness of its own: the caller passes random bits in.

use forza::online::vs::{PROBE_LEN, Reflect, reflect};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};

/// The game's peer port for satellite `s` is `SEAT_BASE + s`.
pub const SEAT_BASE: u16 = 20000;

/// An iroh EndpointId (the public key's 32 bytes).
pub type EndpointId = [u8; 32];

/// Seat of a game port (`20000 + s`), `None` outside 20000..=20255.
pub fn seat_of(port: u16) -> Option<u8> {
    port.checked_sub(SEAT_BASE)?.try_into().ok()
}

/// Token `127.r1.r2.r3:20000+seat` from 24 random bits (the top byte of `bits` is ignored). `None` for the oddities
/// 127.0.0.0, 127.0.0.1 (the real loopback address) and 127.255.255.255: draw again.
pub fn mint_token(bits: u32, seat: u8) -> Option<SocketAddrV4> {
    let b = bits & 0x00FF_FFFF;
    if matches!(b, 0 | 1 | 0x00FF_FFFF) {
        return None;
    }
    let [_, r1, r2, r3] = b.to_be_bytes();
    Some(SocketAddrV4::new(
        Ipv4Addr::new(127, r1, r2, r3),
        SEAT_BASE + u16::from(seat),
    ))
}

/// The opponent's token and EndpointId, from gmsvr's lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub token: SocketAddrV4,
    pub id: EndpointId,
}

impl Pair {
    /// Seat on the opponent's PC: the byte that goes in front of every datagram.
    pub fn remote_seat(&self) -> Option<u8> {
        seat_of(self.token.port())
    }
}

/// Per-seat state: our token (minted once per run) and the paired opponent.
#[derive(Default)]
pub struct Seats {
    tokens: HashMap<u8, SocketAddrV4>,
    pairs: HashMap<u8, Pair>,
}

impl Seats {
    pub fn token(&self, seat: u8) -> Option<SocketAddrV4> {
        self.tokens.get(&seat).copied()
    }

    /// The seat's token, minted from `rand` on first use.
    pub fn token_or_mint(&mut self, seat: u8, mut rand: impl FnMut() -> u32) -> SocketAddrV4 {
        *self.tokens.entry(seat).or_insert_with(|| {
            loop {
                if let Some(t) = mint_token(rand(), seat) {
                    break t;
                }
            }
        })
    }

    /// Replaces the seat's token (gmsvr refused it as a duplicate) and forgets its pair.
    pub fn remint(&mut self, seat: u8, mut rand: impl FnMut() -> u32) -> SocketAddrV4 {
        self.tokens.remove(&seat);
        self.pairs.remove(&seat);
        self.token_or_mint(seat, &mut rand)
    }

    pub fn set_pair(&mut self, seat: u8, pair: Pair) {
        self.pairs.insert(seat, pair);
    }

    pub fn pair(&self, seat: u8) -> Option<&Pair> {
        self.pairs.get(&seat)
    }

    /// Inbound gate: a connection from `remote` may feed seat `seat` only if it is the seat's paired id.
    pub fn accepts(&self, seat: u8, remote: &EndpointId) -> bool {
        self.pairs.get(&seat).is_some_and(|p| p.id == *remote)
    }

    /// A datagram on the reflect socket from `src`: for a type-1 probe from 127.0.0.1:20000+s, the seat and the echo
    /// with the public address (+52 ip, +56 port) set to the seat's token (minted on the first probe). Anything else
    /// (relay traffic, junk, a probe from elsewhere) is `None`; relay traffic is routed by seat, not by header.
    pub fn probe_echo(
        &mut self,
        pkt: &[u8],
        src: SocketAddrV4,
        rand: impl FnMut() -> u32,
    ) -> Option<(u8, [u8; PROBE_LEN])> {
        if !src.ip().is_loopback() {
            return None;
        }
        let seat = seat_of(src.port())?;
        let Reflect::Echo(mut echo) = reflect(pkt, src) else {
            return None;
        };
        let token = self.token_or_mint(seat, rand);
        echo[52..56].copy_from_slice(&token.ip().octets());
        echo[56..58].copy_from_slice(&token.port().to_le_bytes());
        Some((seat, echo))
    }
}

/// QUIC datagram: `[remote seat][game datagram]`.
pub fn encode(remote_seat: u8, game: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(game.len() + 1);
    v.push(remote_seat);
    v.extend_from_slice(game);
    v
}

/// Splits a QUIC datagram into the local seat it is for and the game datagram. `None` when empty.
pub fn decode(datagram: &[u8]) -> Option<(u8, &[u8])> {
    datagram.split_first().map(|(&s, g)| (s, g))
}

/// Which local socket an inbound game datagram is sent from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// The reflect socket (127.0.0.1:R): relay mode, header +50 (destination port) set.
    Reflect,
    /// The socket bound on the opponent's token.
    Peer,
}

pub fn deliver_from(game: &[u8]) -> Via {
    if game.get(50).is_some_and(|&b| b != 0) {
        Via::Reflect
    } else {
        Via::Peer
    }
}

/// The smaller EndpointId dials, the other accepts.
pub fn dials(own: &EndpointId, peer: &EndpointId) -> bool {
    own < peer
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];
    const LOCAL: Ipv4Addr = Ipv4Addr::LOCALHOST;

    fn probe(typ: u16) -> Vec<u8> {
        let mut p = vec![0u8; 52];
        p[..8].copy_from_slice(&MAGIC);
        p[8..10].copy_from_slice(&typ.to_le_bytes());
        p
    }

    #[test]
    fn token_range_and_oddities() {
        let t = mint_token(0xAB_01_02_03, 2).unwrap();
        assert_eq!(t, SocketAddrV4::new(Ipv4Addr::new(127, 1, 2, 3), 20002));
        for bad in [0, 1, 0xFF_FFFF, 0xAB00_0001] {
            assert_eq!(mint_token(bad, 0), None);
        }
    }

    #[test]
    fn seats_from_ports() {
        assert_eq!(seat_of(20003), Some(3));
        assert_eq!(seat_of(19999), None);
        assert_eq!(seat_of(20256), None);
    }

    #[test]
    fn token_is_minted_once_and_skips_oddities() {
        let mut s = Seats::default();
        let mut draws = [1u32, 0, 0x0A0B0C].into_iter();
        let t = s.token_or_mint(1, || draws.next().unwrap());
        assert_eq!(t.ip(), &Ipv4Addr::new(127, 10, 11, 12));
        assert_eq!(s.token_or_mint(1, || unreachable!()), t);
        assert_eq!(s.token(1), Some(t));
        assert_eq!(s.token(2), None);
        let t2 = s.remint(1, || 0x010203);
        assert_ne!(t2, t);
    }

    #[test]
    fn probe_gets_token_echo() {
        let mut s = Seats::default();
        let src = SocketAddrV4::new(LOCAL, 20001);
        let (seat, echo) = s.probe_echo(&probe(1), src, || 0x0A0B0C).unwrap();
        assert_eq!(seat, 1);
        assert_eq!(echo.len(), PROBE_LEN);
        assert_eq!(echo[..8], MAGIC);
        assert_eq!(echo[52..56], [127, 10, 11, 12]);
        assert_eq!(u16::from_le_bytes([echo[56], echo[57]]), 20001);
        // same token on the next probe (keep-alive)
        let (_, again) = s.probe_echo(&probe(1), src, || unreachable!()).unwrap();
        assert_eq!(again, echo);
    }

    #[test]
    fn non_probes_are_not_echoed() {
        let mut s = Seats::default();
        let src = SocketAddrV4::new(LOCAL, 20000);
        assert!(s.probe_echo(&probe(10), src, || 5).is_none());
        assert!(s.probe_echo(&[0; 52], src, || 5).is_none());
        assert!(
            s.probe_echo(&probe(1), SocketAddrV4::new(LOCAL, 5000), || 5)
                .is_none()
        );
        assert!(
            s.probe_echo(
                &probe(1),
                SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 2), 20000),
                || 5
            )
            .is_none()
        );
    }

    #[test]
    fn framing_roundtrip_and_remote_seat() {
        let pair = Pair {
            token: SocketAddrV4::new(Ipv4Addr::new(127, 4, 5, 6), 20002),
            id: [7; 32],
        };
        assert_eq!(pair.remote_seat(), Some(2));
        let d = encode(pair.remote_seat().unwrap(), b"abc");
        assert_eq!(d, [2, b'a', b'b', b'c']);
        assert_eq!(decode(&d), Some((2, &b"abc"[..])));
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[9]), Some((9, &[][..])));
    }

    #[test]
    fn deliver_from_header_byte_50() {
        let mut g = vec![0u8; 60];
        assert_eq!(deliver_from(&g), Via::Peer);
        g[50] = 0x20;
        assert_eq!(deliver_from(&g), Via::Reflect);
        assert_eq!(deliver_from(&g[..50]), Via::Peer);
    }

    #[test]
    fn smaller_id_dials() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        assert!(dials(&a, &b));
        assert!(!dials(&b, &a));
        assert!(!dials(&a, &a));
    }

    #[test]
    fn gate_accepts_only_the_paired_id() {
        let mut s = Seats::default();
        let id = [3u8; 32];
        assert!(!s.accepts(0, &id));
        s.set_pair(
            0,
            Pair {
                token: SocketAddrV4::new(Ipv4Addr::new(127, 1, 1, 1), 20000),
                id,
            },
        );
        assert!(s.accepts(0, &id));
        assert!(!s.accepts(0, &[4; 32]));
        assert!(!s.accepts(1, &id));
        assert_eq!(s.pair(0).unwrap().id, id);
    }
}
