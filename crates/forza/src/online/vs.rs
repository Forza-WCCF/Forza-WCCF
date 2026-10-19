//! Direct VS over the internet (ROADMAP M3), server side: the entry matcher behind messages 243-246 and the UDP
//! reflect service. Layouts are field tables from `docs/research/ONLINE-2017-18.md` and the M3 notes; offsets marked
//! A there are assumptions until a live capture.

use crate::service::{Handle, Log};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];

/// What turns VS on: the reflect server address 202 hands out.
#[derive(Clone, Copy, Debug)]
pub struct VsConfig {
    pub public_ip: Ipv4Addr,
    pub reflect_port: u16,
    /// UDP port of the loopback reflect address (`127.0.0.1:<port>`) 202 gives to logins from a vslink-capable PC
    pub vslink_port: u16,
}

/// Entry window in seconds (answer 224 +8 and +12; the client needs >= 9).
pub const WINDOW_SECS: u32 = 40;
/// An entry waits this long after its 243 (the notes: about 60 s; there is no other cancel message).
const ENTRY_TTL: Duration = Duration::from_secs(60);
/// 243 request and 246 answer sizes.
pub const ENTRY_LEN: usize = 280;
pub const MATCH_LEN: usize = 248;

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// The matching round id for 224 +4: one per window.
pub fn round(unix_secs: i64) -> u32 {
    (unix_secs.max(0) as u64 / u64::from(WINDOW_SECS)) as u32 & 0x7FFF_FFFF
}

/// 202 body (16 bytes, slot echo already in): the reflect address.
pub fn fill_login(body: &mut [u8], cfg: &VsConfig) {
    body[8..12].copy_from_slice(&cfg.public_ip.octets());
    body[12..14].copy_from_slice(&cfg.reflect_port.to_le_bytes());
}

/// 202 body for a login from a PC with a live vslink hello: the local sidecar answers the reflect probe.
pub fn fill_login_loopback(body: &mut [u8], cfg: &VsConfig) {
    body[8..12].copy_from_slice(&Ipv4Addr::LOCALHOST.octets());
    body[12..14].copy_from_slice(&cfg.vslink_port.to_le_bytes());
}

/// 238 payload: the relay thresholds in GAME_CTRL bytes 12, 14, 15 (payload +28 u16 ms, +30 and +31 u8 percent). A
/// until a live capture.
pub fn fill_thresholds(body: &mut [u8]) {
    body[28..30].copy_from_slice(&200u16.to_le_bytes());
    body[30] = 20;
    body[31] = 50;
}

/// 224 body (92 bytes): round id and the entry window.
pub fn fill_round(body: &mut [u8], unix_secs: i64) {
    body[4..8].copy_from_slice(&round(unix_secs).to_le_bytes());
    body[8..12].copy_from_slice(&WINDOW_SECS.to_le_bytes());
    body[12..16].copy_from_slice(&WINDOW_SECS.to_le_bytes());
}

#[derive(Debug)]
struct Entry {
    req: [u8; ENTRY_LEN],
    at: Instant,
    /// Random per entry, never derived from the request: the lobby's room id; a pair shares the older entry's
    room: u64,
    /// The 246 for this seat once paired (slot not yet echoed)
    pair: Option<[u8; MATCH_LEN]>,
}

impl Entry {
    fn key(&self) -> (u32, u32, u32) {
        (
            u32_at(&self.req, 0),
            u32_at(&self.req, 44),
            u32_at(&self.req, 48),
        )
    }
    fn addr(&self) -> (u32, u16) {
        (u32_at(&self.req, 8), u16_at(&self.req, 12))
    }
    fn history(&self) -> impl Iterator<Item = u32> + '_ {
        (0..7)
            .map(|i| u32_at(&self.req, 172 + 4 * i))
            .filter(|c| *c != 0)
    }
}

/// How long a vslink hello and a token registration live (vslink repeats the hello every 60 s).
const LINK_TTL: Duration = Duration::from_secs(600);

/// What vslink told gmsvr: which PCs run it, and the EndpointId behind each token.
#[derive(Debug, Default)]
struct Links {
    capable: HashMap<IpAddr, Instant>,
    ids: HashMap<(u32, u16), ([u8; 32], Instant)>,
}

/// The waiting 243 entries and the pairs made from them, plus the vslink registry.
#[derive(Debug, Default)]
pub struct Matcher {
    entries: Mutex<Vec<Entry>>,
    links: Mutex<Links>,
}

/// An address in 127/8 (`addr().0` holds the octets in memory order).
fn is_loopback(ip: u32) -> bool {
    ip.to_le_bytes()[0] == 127
}

/// A card id as 245 sends it (-1 for none) and 243 stores it (0).
fn card(c: u32) -> u32 {
    if c == u32::MAX { 0 } else { c }
}

fn compatible(a: &Entry, b: &Entry) -> bool {
    let (ca, cb) = (u32_at(&a.req, 44), u32_at(&b.req, 44));
    ca != 0
        && cb != 0
        && a.req[16] == b.req[16]
        && a.req[4..8] == b.req[4..8]
        && is_loopback(a.addr().0) == is_loopback(b.addr().0)
        && (a.addr().0 != b.addr().0 || u32_at(&a.req, 48) != u32_at(&b.req, 48))
        && !a.history().any(|c| c == cb)
        && !b.history().any(|c| c == ca)
}

/// Copies `src` into `dst` as a C string no longer than `dst` (NUL kept).
fn cstr(dst: &mut [u8], src: &[u8]) {
    let n = src
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(src.len())
        .min(dst.len() - 1);
    dst[..n].copy_from_slice(&src[..n]);
}

/// The 246 for the seat of `me`, from its partner `other`'s entry. `home` is the Home seat's card id.
fn build(me: &Entry, other: &Entry, seed: u32, home: u32) -> [u8; MATCH_LEN] {
    let (m, o) = (&me.req, &other.req);
    let mut r = [0u8; MATCH_LEN];
    r[8..12].copy_from_slice(&m[44..48]);
    r[12..16].copy_from_slice(&o[44..48]);
    r[16..20].copy_from_slice(&o[8..12]);
    r[20..24].copy_from_slice(&home.to_le_bytes());
    cstr(&mut r[28..56], &o[112..140]);
    r[56..60].copy_from_slice(&o[48..52]);
    cstr(&mut r[60..116], &o[56..112]);
    r[116..120].copy_from_slice(&seed.to_le_bytes());
    r[120..122].copy_from_slice(&o[12..14]);
    r[122] = 1;
    r[123] = o[15];
    // ponytail: the notes call +124..+147 a "rearranged" copy; the rearrangement is not known, so a straight copy
    r[124..148].copy_from_slice(&o[148..172]);
    cstr(&mut r[148..204], &o[200..256]);
    r[204..216].copy_from_slice(&o[256..268]);
    r[216..248].copy_from_slice(&o[16..48]);
    r
}

impl Matcher {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn links(&self) -> std::sync::MutexGuard<'_, Links> {
        self.links.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A 900: `ip` runs vslink.
    pub fn hello(&self, ip: IpAddr, now: Instant) {
        self.links().capable.insert(ip, now);
    }

    /// Whether `ip` said hello within [`LINK_TTL`].
    pub fn capable(&self, ip: IpAddr, now: Instant) -> bool {
        let mut l = self.links();
        l.capable.retain(|_, t| now.duration_since(*t) < LINK_TTL);
        l.capable.contains_key(&ip)
    }

    /// A 902: token (ip in memory order, port) -> EndpointId. First wins until the TTL; false on a collision.
    pub fn register(&self, ip: u32, port: u16, id: [u8; 32], now: Instant) -> bool {
        let mut l = self.links();
        l.ids.retain(|_, (_, t)| now.duration_since(*t) < LINK_TTL);
        l.ids.entry((ip, port)).or_insert((id, now)).0 == id
    }

    /// A 904: once the entry holding this token is paired, the opponent's token (ip, port) and EndpointId (zeros if
    /// it never registered).
    pub fn lookup(&self, ip: u32, port: u16, now: Instant) -> Option<([u8; 6], [u8; 32])> {
        let p = {
            let mut v = self.lock();
            v.retain(|x| now.duration_since(x.at) < ENTRY_TTL);
            v.iter().find(|x| x.addr() == (ip, port))?.pair?
        };
        let key = (u32_at(&p, 16), u16_at(&p, 120));
        let mut token = [0u8; 6];
        token[..4].copy_from_slice(&p[16..20]);
        token[4..].copy_from_slice(&p[120..122]);
        let mut l = self.links();
        l.ids.retain(|_, (_, t)| now.duration_since(*t) < LINK_TTL);
        Some((token, l.ids.get(&key).map_or([0; 32], |(id, _)| *id)))
    }

    /// A 243. Port 0 is CANCEL: the seat's entry goes. Otherwise the whole request replaces the seat's entry.
    /// Returns true when an entry is now waiting.
    pub fn enter(&self, req: &[u8], now: Instant) -> bool {
        let Ok(req) = <[u8; ENTRY_LEN]>::try_from(req.get(..ENTRY_LEN).unwrap_or(&[])) else {
            return false;
        };
        let e = Entry {
            req,
            at: now,
            room: getrandom::u64().unwrap_or(0),
            pair: None,
        };
        let mut v = self.lock();
        v.retain(|x| x.key() != e.key() && now.duration_since(x.at) < ENTRY_TTL);
        if e.addr().1 == 0 {
            return false;
        }
        v.push(e);
        true
    }

    /// A 245 (20 bytes): the 246 body (slot not echoed) for the caller, or `None` while it waits. The first poll
    /// that finds a compatible waiting entry makes the pair, once; later polls of either seat return the same one.
    pub fn poll(&self, req: &[u8], now: Instant) -> Option<[u8; MATCH_LEN]> {
        if req.len() < 16 {
            return None;
        }
        let key = (u32_at(req, 0), card(u32_at(req, 8)), u32_at(req, 12));
        let mut v = self.lock();
        v.retain(|x| now.duration_since(x.at) < ENTRY_TTL);
        let me = v.iter().position(|x| x.key() == key)?;
        if let Some(p) = v[me].pair {
            return Some(p);
        }
        // the oldest compatible waiting entry
        let other = v
            .iter()
            .enumerate()
            .find(|(i, x)| *i != me && x.pair.is_none() && compatible(&v[me], x))?
            .0;
        let seed = getrandom::u32().unwrap_or(0x1234_5678);
        let hc = u32_at(&v[me.min(other)].req, 44); // the older entry is Home
        let (for_me, for_other) = (
            build(&v[me], &v[other], seed, hc),
            build(&v[other], &v[me], seed, hc),
        );
        v[me].pair = Some(for_me);
        v[other].pair = Some(for_other);
        let room = v[me.min(other)].room;
        (v[me].room, v[other].room) = (room, room);
        Some(for_me)
    }

    pub fn waiting(&self) -> usize {
        self.lock().len()
    }

    /// The lobby's VS rooms (ROADMAP "Lobby listing"): (random room id, players), a waiting entry 1, a pair 2 until
    /// its entries expire.
    pub fn rooms(&self, now: Instant) -> Vec<(u64, u8)> {
        let mut v = self.lock();
        v.retain(|x| now.duration_since(x.at) < ENTRY_TTL);
        let mut rooms: Vec<(u64, u8)> = Vec::new();
        for e in v.iter() {
            match rooms.iter_mut().find(|(r, _)| *r == e.room) {
                Some((_, n)) => *n += 1,
                None => rooms.push((e.room, 1)),
            }
        }
        rooms
    }
}

// ---- the reflect service ----

/// 58 bytes: the echo of a probe, carrying the sender's public address.
pub const PROBE_LEN: usize = 58;

/// What the reflect service does with one datagram.
#[derive(Debug, PartialEq)]
pub enum Reflect {
    Echo([u8; PROBE_LEN]),
    /// Forward the datagram unchanged to this address (header +44 ip, +50 port)
    Relay(SocketAddrV4),
    Ignore,
}

/// Classifies a datagram from `src`. A probe (type 1, >= 52 bytes) gets the echo. Any other type with a
/// destination port at +50 is relay traffic.
pub fn reflect(pkt: &[u8], src: SocketAddrV4) -> Reflect {
    if pkt.len() < 52 || pkt[..8] != MAGIC {
        return Reflect::Ignore;
    }
    if u16_at(pkt, 8) == 1 {
        let mut r = [0u8; PROBE_LEN];
        r[..10].copy_from_slice(&pkt[..10]);
        r[16..20].copy_from_slice(&(PROBE_LEN as u32).to_le_bytes());
        r[52..56].copy_from_slice(&src.ip().octets());
        r[56..58].copy_from_slice(&src.port().to_le_bytes());
        return Reflect::Echo(r);
    }
    // ponytail: the destination port's byte order (host order here, like the probe's) is unconfirmed by a capture
    let port = u16_at(pkt, 50);
    if port == 0 {
        return Reflect::Ignore;
    }
    Reflect::Relay(SocketAddrV4::new(
        Ipv4Addr::from(<[u8; 4]>::try_from(&pkt[44..48]).unwrap()),
        port,
    ))
}

const PROBED_FOR: Duration = Duration::from_secs(60);
/// Relay packets per second per source (lock-step play needs ~60 plus pings).
const RELAY_RATE: u32 = 300;

/// Serves the reflect echo and relay on `sock`. Stops when the handle is dropped.
pub fn spawn_reflect(sock: UdpSocket, log: Log) -> io::Result<Handle> {
    sock.set_read_timeout(Some(Duration::from_millis(100)))?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        let mut probed: HashMap<SocketAddrV4, Instant> = HashMap::new();
        let mut rate: HashMap<SocketAddrV4, (Instant, u32)> = HashMap::new();
        while !flag.load(std::sync::atomic::Ordering::Relaxed) {
            let Ok((n, SocketAddr::V4(src))) = sock.recv_from(&mut buf) else {
                continue;
            };
            let now = Instant::now();
            match reflect(&buf[..n], src) {
                Reflect::Echo(r) => {
                    if probed.insert(src, now).is_none() {
                        log(&format!("reflect: probe from {src}"));
                    }
                    let _ = sock.send_to(&r, src);
                    probed.retain(|_, t| now.duration_since(*t) < PROBED_FOR);
                    rate.retain(|_, (t, _)| now.duration_since(*t) < Duration::from_secs(1));
                }
                Reflect::Relay(dst) => {
                    let fresh = |a: &SocketAddrV4| {
                        probed
                            .get(a)
                            .is_some_and(|t| now.duration_since(*t) < PROBED_FOR)
                    };
                    let r = rate.entry(src).or_insert((now, 0));
                    if now.duration_since(r.0) >= Duration::from_secs(1) {
                        *r = (now, 0);
                    }
                    r.1 += 1;
                    if r.1 <= RELAY_RATE && fresh(&src) && fresh(&dst) {
                        let _ = sock.send_to(&buf[..n], dst);
                    }
                }
                Reflect::Ignore => {}
            }
        }
    });
    Ok(Handle::new(stop, thread))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(slot: u32, seq: u32, ip: [u8; 4], port: u16, card: u32, shop: u32) -> Vec<u8> {
        let mut r = vec![0u8; ENTRY_LEN];
        r[0..4].copy_from_slice(&slot.to_le_bytes());
        r[4..8].copy_from_slice(&seq.to_le_bytes());
        r[8..12].copy_from_slice(&ip);
        r[12..14].copy_from_slice(&port.to_le_bytes());
        r[17] = slot as u8;
        r[44..48].copy_from_slice(&card.to_le_bytes());
        r[48..52].copy_from_slice(&shop.to_le_bytes());
        r[56..61].copy_from_slice(b"club\0");
        r[200..205].copy_from_slice(b"coach");
        r[112..116].copy_from_slice(b"shop");
        r
    }

    fn poll_req(slot: u32, card: u32, shop: u32) -> Vec<u8> {
        let mut r = vec![0u8; 20];
        r[0..4].copy_from_slice(&slot.to_le_bytes());
        r[8..12].copy_from_slice(&card.to_le_bytes());
        r[12..16].copy_from_slice(&shop.to_le_bytes());
        r
    }

    #[test]
    fn login_and_round_layouts() {
        let mut b = [0u8; 16];
        fill_login(
            &mut b,
            &VsConfig {
                public_ip: Ipv4Addr::new(1, 2, 3, 4),
                reflect_port: 0x1234,
                vslink_port: 20090,
            },
        );
        assert_eq!(b[8..14], [1, 2, 3, 4, 0x34, 0x12]);
        let mut r = [0u8; 92];
        fill_round(&mut r, 400);
        assert_eq!((u32_at(&r, 4), u32_at(&r, 8), u32_at(&r, 12)), (10, 40, 40));
        let mut c = vec![0u8; 0x5A54];
        fill_thresholds(&mut c);
        assert_eq!((u16_at(&c, 28), c[30], c[31]), (200, 20, 50));
    }

    #[test]
    fn pair_is_mirrored_stable_and_once() {
        let m = Matcher::default();
        let t = Instant::now();
        let a = entry(1, 7, [9, 9, 9, 1], 20001, 111, 5);
        let b = entry(2, 7, [9, 9, 9, 2], 20002, 222, 6);
        assert!(m.enter(&a, t) && m.enter(&b, t));
        assert_eq!(m.poll(&poll_req(1, 111, 5), t).map(|_| ()), Some(()));
        let pa = m.poll(&poll_req(1, 111, 5), t).unwrap();
        let pb = m.poll(&poll_req(2, 222, 6), t).unwrap();
        assert_eq!((u32_at(&pa, 12), u32_at(&pb, 12)), (222, 111));
        assert_eq!(pa[16..20], [9, 9, 9, 2]);
        assert_eq!(pb[16..20], [9, 9, 9, 1]);
        assert_eq!((u16_at(&pa, 120), u16_at(&pb, 120)), (20002, 20001));
        assert_eq!(u32_at(&pa, 116), u32_at(&pb, 116));
        assert_eq!((pa[122], pb[122]), (1, 1));
        assert_eq!(&pa[60..64], b"club");
        assert_eq!(&pa[148..153], b"coach");
        // the waiting seat (a) is Home: +20 equals +8 only for it
        assert_eq!(u32_at(&pa, 20), u32_at(&pa, 8));
        assert_ne!(u32_at(&pb, 20), u32_at(&pb, 8));
        assert_eq!(pa[216..248], b[16..48]);
        assert_eq!(pb[216..248], a[16..48]);
    }

    #[test]
    fn rooms_a_waiting_entry_is_one_a_pair_is_two() {
        let m = Matcher::default();
        let t = Instant::now();
        m.enter(&entry(1, 7, [9, 9, 9, 1], 20001, 111, 5), t);
        assert_eq!(m.rooms(t).iter().map(|r| r.1).collect::<Vec<_>>(), [1]);
        m.enter(&entry(2, 7, [9, 9, 9, 2], 20002, 222, 6), t);
        assert_eq!(m.rooms(t).len(), 2);
        m.poll(&poll_req(1, 111, 5), t).unwrap();
        m.enter(&entry(3, 7, [9, 9, 9, 3], 20003, 333, 7), t);
        let mut n: Vec<u8> = m.rooms(t).iter().map(|r| r.1).collect();
        n.sort();
        assert_eq!(n, [1, 2]);
        assert!(m.rooms(t + ENTRY_TTL).is_empty());
    }

    #[test]
    fn no_partner_waits_cancel_and_expiry() {
        let m = Matcher::default();
        let t = Instant::now();
        assert!(m.enter(&entry(1, 7, [9, 9, 9, 1], 20001, 111, 5), t));
        assert!(m.poll(&poll_req(1, 111, 5), t).is_none());
        // same cabinet, other round, other kbn do not pair
        m.enter(&entry(2, 7, [9, 9, 9, 1], 20002, 222, 5), t);
        m.enter(&entry(3, 8, [9, 9, 9, 3], 20003, 333, 7), t);
        let mut k = entry(4, 7, [9, 9, 9, 4], 20004, 444, 8);
        k[16] = 1;
        m.enter(&k, t);
        assert!(m.poll(&poll_req(1, 111, 5), t).is_none());
        // history blocks a rematch
        let mut h = entry(5, 7, [9, 9, 9, 5], 20005, 555, 9);
        h[172..176].copy_from_slice(&111u32.to_le_bytes());
        m.enter(&h, t);
        assert!(m.poll(&poll_req(1, 111, 5), t).is_none());
        // cancel (port 0) removes; expiry drops
        assert_eq!(m.waiting(), 5);
        assert!(!m.enter(&entry(5, 7, [9, 9, 9, 5], 0, 555, 9), t));
        assert_eq!(m.waiting(), 4);
        assert!(m.poll(&poll_req(1, 111, 5), t + ENTRY_TTL).is_none());
        assert_eq!(m.waiting(), 0);
    }

    #[test]
    fn probe_echo_and_relay() {
        let src = SocketAddrV4::new(Ipv4Addr::new(5, 6, 7, 8), 20003);
        let mut p = [0u8; 58];
        p[..8].copy_from_slice(&MAGIC);
        p[8] = 1;
        p[16] = 58;
        let Reflect::Echo(r) = reflect(&p, src) else {
            panic!()
        };
        assert_eq!((r[..8] == MAGIC, r[8], r[16]), (true, 1, 58));
        assert_eq!(
            (r[52..56].to_vec(), u16_at(&r, 56)),
            (vec![5, 6, 7, 8], 20003)
        );
        assert_eq!(reflect(&p[..51], src), Reflect::Ignore);
        p[8] = 2;
        assert_eq!(reflect(&p, src), Reflect::Ignore);
        p[44..48].copy_from_slice(&[1, 1, 1, 1]);
        p[50..52].copy_from_slice(&99u16.to_le_bytes());
        assert_eq!(
            reflect(&p, src),
            Reflect::Relay(SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 99))
        );
    }

    fn paired(m: &Matcher, t: Instant, a_ip: [u8; 4], b_ip: [u8; 4]) {
        assert!(m.enter(&entry(1, 7, a_ip, 20001, 111, 5), t));
        assert!(m.enter(&entry(2, 7, b_ip, 20002, 222, 6), t));
    }

    #[test]
    fn vslink_register_collision_and_ttl() {
        let m = Matcher::default();
        let t = Instant::now();
        let ip = IpAddr::from([1, 2, 3, 4]);
        assert!(!m.capable(ip, t));
        m.hello(ip, t);
        assert!(m.capable(ip, t) && !m.capable([1, 2, 3, 5].into(), t));
        assert!(!m.capable(ip, t + LINK_TTL));
        let tok = u32::from_le_bytes([127, 1, 2, 3]);
        assert!(m.register(tok, 20001, [7; 32], t));
        assert!(m.register(tok, 20001, [7; 32], t), "same id again is fine");
        assert!(!m.register(tok, 20001, [8; 32], t), "first wins");
        assert!(
            m.register(tok, 20001, [8; 32], t + LINK_TTL),
            "expired token is free"
        );
    }

    #[test]
    fn vslink_lookup_before_and_after_pairing() {
        let m = Matcher::default();
        let t = Instant::now();
        let (ta, tb) = ([127, 1, 1, 1], [127, 2, 2, 2]);
        let (ka, kb) = (u32::from_le_bytes(ta), u32::from_le_bytes(tb));
        assert!(m.register(ka, 20001, [0xAA; 32], t));
        assert!(m.enter(&entry(1, 7, ta, 20001, 111, 5), t));
        assert_eq!(m.lookup(ka, 20001, t), None, "unpaired");
        assert!(m.enter(&entry(2, 7, tb, 20002, 222, 6), t));
        assert_eq!(m.lookup(ka, 20001, t), None, "not polled yet");
        m.poll(&poll_req(1, 111, 5), t).unwrap();
        // b has not registered: zeros; a is known to b
        let (tok, id) = m.lookup(ka, 20001, t).unwrap();
        assert_eq!(
            (tok[..4].to_vec(), u16_at(&tok, 4), id),
            (tb.to_vec(), 20002, [0; 32])
        );
        assert!(m.register(kb, 20002, [0xBB; 32], t));
        assert_eq!(m.lookup(ka, 20001, t).unwrap().1, [0xBB; 32]);
        let (tok, id) = m.lookup(kb, 20002, t).unwrap();
        assert_eq!(
            (tok[..4].to_vec(), u16_at(&tok, 4), id),
            (ta.to_vec(), 20001, [0xAA; 32])
        );
        assert_eq!(m.lookup(ka, 20009, t), None, "unknown token");
        // registrations expire (the entries, at 60 s, are gone by then too)
        assert_eq!(m.lookup(ka, 20001, t + LINK_TTL), None);
    }

    #[test]
    fn loopback_and_public_never_pair() {
        let t = Instant::now();
        for (a, b, pairs) in [
            ([127, 1, 1, 1], [9, 9, 9, 2], false),
            ([9, 9, 9, 2], [127, 1, 1, 1], false),
            ([127, 1, 1, 1], [127, 2, 2, 2], true),
            ([9, 9, 9, 1], [9, 9, 9, 2], true),
        ] {
            let m = Matcher::default();
            paired(&m, t, a, b);
            assert_eq!(
                m.poll(&poll_req(1, 111, 5), t).is_some(),
                pairs,
                "{a:?} {b:?}"
            );
        }
    }
}
