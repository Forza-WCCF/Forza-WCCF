//! The match relay and the ping meter, as the C panel's (`wccfpanel.c`: `relay_*`, `ping_*`; both editions' sources
//! agree byte for byte in these parts, down to the import slots).
//!
//! A match between two players: the cabinets send their moves to each other over UDP to port 20000 + the other's
//! seat. Home routers drop that traffic and the game falls back to a CPU match. When the run sets `WCCF_RELAY=ADDRESS:PORT`
//! and `WCCF_SEAT=n`, the game's own `sendto` / `recvfrom` import slots come here: a packet for another player (a
//! public address, port 20001-20008) goes to the server's relay labelled with both seats, and what the relay passes on
//! arrives on the same socket and is handed to the game as if straight from that player. Everything else (127.x, the
//! server's TCP, other ports) passes untouched. 2017-18 also routes 127.x (another seat of the arcade may have reached
//! the control from the host's own loopback, so the control names 127.0.0.1 as its address), except to its own seat;
//! ports 20003-20006 (also the client's `MATCH_PORT` and `CLIENT_PORT1..3` defaults, whose loopback use is not known)
//! only from the game's peer socket, the one bound to 20000 + its seat.
//! Every 5 s each socket that played tells the relay "I am seat n".
//!
//! The ping meter asks the relay for an echo every 2 s on a socket of its own; [`ping_ms`] is the last answer.
//!
//! Protocol bytes (all to the relay, UDP): `"WRL1"` then a kind byte. `R seat` = hello; `D to from payload` = data out;
//! `F from ip4 payload` = data in; `P seq4` echo, answered `Q seq4`.

use std::ffi::c_void;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU32, AtomicUsize,
    Ordering::{Acquire, Relaxed, Release},
};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::core::{PCSTR, w};

const MAGIC: &[u8; 4] = b"WRL1";
const AF_INET: u16 = 2;
const SOCKET_ERROR: i32 = -1;
const WSAEWOULDBLOCK: i32 = 10035;
const WSAEMSGSIZE: i32 = 10040;
const MSG_PEEK: i32 = 2;
/// `SIO_UDP_CONNRESET` = `_WSAIOW(IOC_VENDOR, 12)`.
const SIO_UDP_CONNRESET: u32 = 0x9800_000C;
/// The preferred image base of `client_Release.exe`, in both editions (2017-18 is ASLR, so it may load elsewhere).
const PREFERRED_BASE: usize = 0x0040_0000;

/// The import slots in `client_Release.exe` (WS2_32 #20 `sendto`, #17 `recvfrom`) as offsets from the image base,
/// read from each edition's PE import table: (`sendto`, `recvfrom`). 2010-11 has `recvfrom` first, 2017-18 `sendto`.
const fn slot_rvas(e1718: bool) -> (usize, usize) {
    if e1718 {
        (0x011A_9D88 - PREFERRED_BASE, 0x011A_9D8C - PREFERRED_BASE)
    } else {
        (0x0097_E8FC - PREFERRED_BASE, 0x0097_E8F8 - PREFERRED_BASE)
    }
}

#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSAGetLastError() -> i32;
    fn WSASetLastError(e: i32);
    fn getsockname(s: usize, name: *mut c_void, namelen: *mut i32) -> i32;
    fn WSAIoctl(
        s: usize,
        code: u32,
        inb: *const c_void,
        inlen: u32,
        outb: *mut c_void,
        outlen: u32,
        ret: *mut u32,
        ov: *mut c_void,
        cb: *mut c_void,
    ) -> i32;
}

/// `struct sockaddr_in`: port and address in network order.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug)]
struct SockaddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}
const SA_LEN: usize = size_of::<SockaddrIn>();

impl SockaddrIn {
    const NONE: SockaddrIn = SockaddrIn {
        family: 0,
        port: 0,
        addr: [0; 4],
        zero: [0; 8],
    };
    fn new(addr: [u8; 4], port: u16) -> Self {
        SockaddrIn {
            family: AF_INET,
            port: port.to_be(),
            addr,
            zero: [0; 8],
        }
    }
}

type SendtoFn = unsafe extern "system" fn(usize, *const u8, i32, i32, *const c_void, i32) -> i32;
type RecvfromFn = unsafe extern "system" fn(usize, *mut u8, i32, i32, *mut c_void, *mut i32) -> i32;

static REAL_SENDTO: AtomicUsize = AtomicUsize::new(0);
static REAL_RECVFROM: AtomicUsize = AtomicUsize::new(0);
static RELAY: Mutex<SockaddrIn> = Mutex::new(SockaddrIn::NONE);
static RELAY_ON: AtomicBool = AtomicBool::new(false);
static RELAY_TEST: AtomicBool = AtomicBool::new(false);
/// 2017-18: a packet for another seat at 127.x goes to the relay too ([`peer_seat`]).
static RELAY_LOOPBACK: AtomicBool = AtomicBool::new(false);
static MY_SEAT: AtomicU32 = AtomicU32::new(0);
/// Per seat: the address the game itself sends to (its source, back).
static PEER: Mutex<[SockaddrIn; 9]> = Mutex::new([SockaddrIn::NONE; 9]);
/// The sockets that played: "I am seat n" every 5 s.
static RSOCK: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static RL_OUT: AtomicI32 = AtomicI32::new(0);
static RL_IN: AtomicI32 = AtomicI32::new(0);
static RL_SAID_OUT: AtomicBool = AtomicBool::new(false);
static RL_SAID_IN: AtomicBool = AtomicBool::new(false);
static LOG: OnceLock<fn(&str)> = OnceLock::new();

fn say(msg: &str) {
    if let Some(f) = LOG.get() {
        f(msg);
    }
}

/// C's `atoi`: leading blanks, a sign, then digits up to the first other character.
fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let n = s.bytes().take_while(u8::is_ascii_digit).fold(0i32, |a, b| {
        a.wrapping_mul(10).wrapping_add(i32::from(b - b'0'))
    });
    if neg { -n } else { n }
}

/// Why `WCCF_RELAY` is not usable.
#[derive(Debug, PartialEq)]
enum Bad {
    /// no colon
    Form,
    /// the address and the port as written
    Address(String, String),
}

/// `ADDRESS:PORT` as `relay_install` and `ping_install` read it (`inet_addr`, `atoi`, port 1-65535).
fn parse_relay(v: &str) -> Result<([u8; 4], u16), Bad> {
    let (host, port) = v.split_once(':').ok_or(Bad::Form)?;
    let n = atoi(port);
    match host.parse::<Ipv4Addr>() {
        // INADDR_NONE is 255.255.255.255
        Ok(ip) if ip != Ipv4Addr::BROADCAST && (1..=65535).contains(&n) => {
            Ok((ip.octets(), n as u16))
        }
        _ => Err(Bad::Address(host.to_owned(), port.to_owned())),
    }
}

/// `relay_peer`: the seat of the cabinet a packet to `a` is for, if it is one (port 20001-20008, not 0.0.0.0 or
/// broadcast, not 127.x except 127.0.0.2 in the test). `loopback` (2017-18: this cabinet's seat): 127.x is another seat
/// too, unless it is this seat's own port; 20003-20006 (also the client's `MATCH_PORT` and `CLIENT_PORT1..3` defaults)
/// only when `from_game_port`, the sending socket being the game's peer socket (bound to 20000 + this seat).
fn peer_seat(
    a: &SockaddrIn,
    test: bool,
    loopback: Option<usize>,
    from_game_port: bool,
) -> Option<usize> {
    if a.family != AF_INET {
        return None;
    }
    let port = u16::from_be(a.port);
    let ip = u32::from_be_bytes(a.addr);
    if !(20001..=20008).contains(&port) || ip == 0 || ip == 0xFFFF_FFFF {
        return None;
    }
    let seat = usize::from(port) - 20000;
    // seats 3-6 share their ports with MATCH_PORT and CLIENT_PORT1..3, whose loopback use is not known: only the game's
    // peer socket sends to them through the relay, so an arcade's second match (seats 3 and 4) plays too (live 2026-10-27)
    let arcade =
        loopback.is_some_and(|own| seat != own && (!(3..=6).contains(&seat) || from_game_port));
    if (ip >> 24) == 127 && !arcade && !(test && ip == 0x7F00_0002) {
        return None;
    }
    Some(seat)
}

/// `relay_peer` on the game's `sendto` arguments.
///
/// # Safety
/// `to` is null or points at `tolen` readable bytes.
unsafe fn relay_peer(s: usize, to: *const c_void, tolen: i32) -> Option<(usize, SockaddrIn)> {
    if to.is_null() || tolen < SA_LEN as i32 {
        return None;
    }
    // SAFETY: `tolen` bytes are readable, and a `sockaddr_in` is plain bytes (read unaligned).
    let a = unsafe { to.cast::<SockaddrIn>().read_unaligned() };
    let loopback = RELAY_LOOPBACK
        .load(Relaxed)
        .then(|| MY_SEAT.load(Relaxed) as usize);
    // asked only for 127.x:20003-20006 (a second match's packets), so other packets cost no extra call
    let port = u16::from_be(a.port);
    let from_game_port = loopback.is_some_and(|own| {
        a.addr[0] == 127
            && (20003..=20006).contains(&port)
            && bound_port(s) == Some(20000 + own as u16)
    });
    peer_seat(&a, RELAY_TEST.load(Relaxed), loopback, from_game_port).map(|s| (s, a))
}

/// The local port socket `s` is bound to, if it is an IPv4 one.
fn bound_port(s: usize) -> Option<u16> {
    let mut a = SockaddrIn::NONE;
    let mut len = SA_LEN as i32;
    // SAFETY: `a` is ours and `len` its size; a bad socket only makes the call fail.
    let r = unsafe { getsockname(s, (&raw mut a).cast(), &raw mut len) };
    (r == 0 && a.family == AF_INET).then(|| u16::from_be(a.port))
}

/// The "I am seat n" packet (`relay_hello`).
fn frame_hello(my_seat: u8) -> [u8; 6] {
    let mut p = [0u8; 6];
    p[..4].copy_from_slice(MAGIC);
    p[4] = b'R';
    p[5] = my_seat;
    p
}

/// The data packet to the relay (`relay_sendto`): magic, `D`, the other's seat, mine, the payload. Returns its length.
fn frame_d(out: &mut [u8], seat: u8, my_seat: u8, payload: &[u8]) -> usize {
    out[..4].copy_from_slice(MAGIC);
    out[4] = b'D';
    out[5] = seat;
    out[6] = my_seat;
    out[7..7 + payload.len()].copy_from_slice(payload);
    7 + payload.len()
}

/// A packet from the relay (`relay_recvfrom`): magic, `F`, the sender's seat 1-8, its address (4 bytes), the payload.
/// `None`: the relay's own, not the game's.
fn parse_f(p: &[u8]) -> Option<(usize, [u8; 4], &[u8])> {
    if p.len() < 10 || &p[..4] != MAGIC || p[4] != b'F' || !(1..=8).contains(&p[5]) {
        return None;
    }
    Some((usize::from(p[5]), [p[6], p[7], p[8], p[9]], &p[10..]))
}

fn real_sendto() -> SendtoFn {
    // SAFETY: set by `relay_install` or `relay_selftest` from ws2_32's `sendto` before any hook can run.
    unsafe { std::mem::transmute::<usize, SendtoFn>(REAL_SENDTO.load(Acquire)) }
}

fn real_recvfrom() -> RecvfromFn {
    // SAFETY: as `real_sendto`, for `recvfrom`.
    unsafe { std::mem::transmute::<usize, RecvfromFn>(REAL_RECVFROM.load(Acquire)) }
}

fn relay_addr() -> SockaddrIn {
    *RELAY.lock().unwrap_or_else(|e| e.into_inner())
}

/// `relay_hello`: "I am seat n" from this socket.
fn relay_hello(s: usize) {
    let p = frame_hello(MY_SEAT.load(Relaxed) as u8);
    let r = relay_addr();
    // SAFETY: the real `sendto` with a valid buffer and a 16-byte `sockaddr_in`.
    unsafe { real_sendto()(s, p.as_ptr(), 6, 0, (&raw const r).cast(), SA_LEN as i32) };
}

/// `relay_keep`: the game's thread, a socket that sends to another player.
fn relay_keep(s: usize) {
    {
        let mut v = RSOCK.lock().unwrap_or_else(|e| e.into_inner());
        if v.contains(&s) {
            return;
        }
        if v.len() < 8 {
            v.push(s);
        }
    }
    relay_hello(s);
}

/// `relay_sendto`: the game's `sendto`. A packet for another player goes to the relay, the rest to the real one.
///
/// # Safety
/// Called by the game with `sendto`'s contract: `buf` has `len` bytes, `to` has `tolen`.
unsafe extern "system" fn relay_sendto(
    s: usize,
    buf: *const u8,
    len: i32,
    flags: i32,
    to: *const c_void,
    tolen: i32,
) -> i32 {
    let peer = if RELAY_ON.load(Acquire) && (0..=4096).contains(&len) {
        // SAFETY: `to` and `tolen` are the caller's.
        unsafe { relay_peer(s, to, tolen) }
    } else {
        None
    };
    let Some((seat, addr)) = peer else {
        // SAFETY: the caller's arguments, unchanged.
        return unsafe { real_sendto()(s, buf, len, flags, to, tolen) };
    };
    let was = std::mem::replace(
        &mut PEER.lock().unwrap_or_else(|e| e.into_inner())[seat],
        addr,
    );
    // the game leaves sin_zero unset in one of its send paths (client 0x4CDB40): only the address and port count
    if (was.family, was.addr, was.port) != (addr.family, addr.addr, addr.port) {
        // the opponent's address as the game took it from the control (ENEMY_INFO)
        say(&format!(
            "relay: seat {seat}'s cabinet is at {}:{} for the game",
            Ipv4Addr::from(addr.addr),
            u16::from_be(addr.port)
        ));
    }
    relay_keep(s);
    let mut pkt = [0u8; 4200];
    // SAFETY: `buf` has `len` readable bytes (len is 0..=4096).
    let payload = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    let n = frame_d(&mut pkt, seat as u8, MY_SEAT.load(Relaxed) as u8, payload);
    let r = relay_addr();
    // SAFETY: the real `sendto` with our packet and a 16-byte `sockaddr_in`.
    let sent = unsafe {
        real_sendto()(
            s,
            pkt.as_ptr(),
            n as i32,
            flags,
            (&raw const r).cast(),
            SA_LEN as i32,
        )
    };
    if sent == SOCKET_ERROR {
        return sent;
    }
    RL_OUT.fetch_add(1, Relaxed);
    if !RL_SAID_OUT.swap(true, Relaxed) {
        say(&format!(
            "relay: the first packet for seat {seat}'s cabinet goes through the server's relay"
        ));
    }
    len
}

/// `relay_recvfrom`: the game's `recvfrom`. The relay's packets become the other player's, from the address the game
/// sends to it; anything else is what the real one returned.
///
/// # Safety
/// Called by the game with `recvfrom`'s contract: `buf` has `len` bytes, `from`/`fromlen` are null or valid.
unsafe extern "system" fn relay_recvfrom(
    s: usize,
    buf: *mut u8,
    len: i32,
    flags: i32,
    from: *mut c_void,
    fromlen: *mut i32,
) -> i32 {
    if !RELAY_ON.load(Acquire) || flags & MSG_PEEK != 0 {
        // SAFETY: the caller's arguments, unchanged.
        return unsafe { real_recvfrom()(s, buf, len, flags, from, fromlen) };
    }
    let relay = relay_addr();
    let mut tmp = [0u8; 4200];
    loop {
        let mut src = SockaddrIn::NONE;
        let mut sl = SA_LEN as i32;
        // SAFETY: `tmp` and `src` are ours, sized as passed.
        let r = unsafe {
            real_recvfrom()(
                s,
                tmp.as_mut_ptr(),
                tmp.len() as i32,
                flags,
                (&raw mut src).cast(),
                &raw mut sl,
            )
        };
        if r == SOCKET_ERROR {
            return r;
        }
        if src.family == AF_INET && src.addr == relay.addr && src.port == relay.port {
            let Some((seat, ip, payload)) = parse_f(&tmp[..r as usize]) else {
                continue; // the relay's, not the game's: read the next one
            };
            let n = payload.len().min(len.max(0) as usize);
            // SAFETY: `buf` has `len` writable bytes, `n <= len`.
            unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), buf, n) };
            // SAFETY: `fromlen` is null or valid for the caller.
            if !from.is_null() && !fromlen.is_null() && unsafe { *fromlen } >= SA_LEN as i32 {
                let known = PEER.lock().unwrap_or_else(|e| e.into_inner())[seat];
                let f = if known.family == AF_INET {
                    known
                } else {
                    SockaddrIn::new(ip, 20000 + seat as u16)
                };
                // SAFETY: `from` has at least `*fromlen >= 16` writable bytes.
                unsafe {
                    from.cast::<SockaddrIn>().write_unaligned(f);
                    *fromlen = SA_LEN as i32;
                }
            }
            RL_IN.fetch_add(1, Relaxed);
            if !RL_SAID_IN.swap(true, Relaxed) {
                say(&format!(
                    "relay: the first packet from seat {seat}'s cabinet came through the server's relay"
                ));
            }
            return n as i32;
        }
        if r > len {
            // what the game's own recvfrom would have said: too big, cut
            // SAFETY: `buf` has `len` writable bytes and `len < r <= tmp.len()`.
            unsafe {
                std::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, len.max(0) as usize);
                WSASetLastError(WSAEMSGSIZE);
            }
            return SOCKET_ERROR;
        }
        // SAFETY: `r <= len`, so `buf` holds it.
        unsafe { std::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, r as usize) };
        // SAFETY: `fromlen` is null or valid for the caller.
        if !from.is_null() && !fromlen.is_null() {
            // SAFETY: as above; at most `*fromlen` bytes are written.
            unsafe {
                let k = (*fromlen).clamp(0, sl) as usize;
                std::ptr::copy_nonoverlapping((&raw const src).cast::<u8>(), from.cast::<u8>(), k);
                *fromlen = sl;
            }
        }
        return r;
    }
}

/// `relay_watch`: every 5 s "I am seat n" from each socket that played; every minute the counts.
///
/// ponytail: runs for the process's life, as the C's (the real game never unloads the panel); no stop flag.
fn relay_watch() {
    // SAFETY: GetTickCount takes nothing.
    let mut next = unsafe { GetTickCount() }.wrapping_add(60000);
    let (mut o0, mut i0) = (0, 0);
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let socks = RSOCK.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for s in socks {
            relay_hello(s);
        }
        // SAFETY: GetTickCount takes nothing.
        let now = unsafe { GetTickCount() };
        if now.wrapping_sub(next) as i32 >= 0 {
            let (o, i) = (RL_OUT.load(Relaxed), RL_IN.load(Relaxed));
            if o != o0 || i != i0 {
                say(&format!(
                    "relay: last minute {} packets out, {} in, through the server",
                    o - o0,
                    i - i0
                ));
            }
            (o0, i0) = (o, i);
            next = now.wrapping_add(60000);
        }
    }
}

/// `relay_install`: seat n's cabinet on a server (the run set `WCCF_RELAY` and `WCCF_SEAT`). Both editions: the
/// program must be `client_Release.exe` and the two slots must hold ws2_32's own `sendto` / `recvfrom`.
pub fn relay_install(e1718: bool, log: fn(&str)) {
    let _ = LOG.set(log);
    let Ok(v) = std::env::var("WCCF_RELAY") else {
        return;
    }; // this PC alone: no relay
    let seat = std::env::var("WCCF_SEAT").map_or(0, |s| atoi(&s));
    if !(1..=8).contains(&seat) {
        return say("relay: off - no seat (WCCF_SEAT)");
    }
    let (ip, port) = match parse_relay(&v) {
        Ok(x) => x,
        Err(Bad::Form) => {
            return say(&format!(
                "relay: off - WCCF_RELAY is not ADDRESS:PORT ({v})"
            ));
        }
        Err(Bad::Address(h, p)) => return say(&format!("relay: off - not an address: {h}:{p}")),
    };
    let mut exe = [0u16; 260];
    // SAFETY: a buffer of our own.
    let n = unsafe { GetModuleFileNameW(None, &mut exe) } as usize;
    let path = String::from_utf16_lossy(&exe[..n]);
    let nm = path.rsplit('\\').next().unwrap_or(&path);
    // SAFETY: plain module lookups.
    let (me, ws) = unsafe { (GetModuleHandleW(None), GetModuleHandleW(w!("ws2_32.dll"))) };
    let (Ok(me), Ok(ws)) = (me, ws) else {
        return say(&format!("relay: off - not the game ({nm})"));
    };
    if !nm.eq_ignore_ascii_case("client_Release.exe") {
        return say(&format!("relay: off - not the game ({nm})"));
    }
    // SAFETY: names are NUL-terminated literals.
    let (st, rf) = unsafe {
        (
            GetProcAddress(ws, PCSTR(c"sendto".as_ptr().cast())),
            GetProcAddress(ws, PCSTR(c"recvfrom".as_ptr().cast())),
        )
    };
    let (Some(st), Some(rf)) = (st, rf) else {
        return say("relay: off - the game's sendto / recvfrom imports are not the ones read");
    };
    let (st, rf) = (st as usize, rf as usize);
    let (rs, rr) = slot_rvas(e1718);
    let (slot_sendto, slot_recvfrom) = (me.0 as usize + rs, me.0 as usize + rr);
    // SAFETY: reading the program's own import table; the slots are mapped in the game (the test above holds).
    let held = unsafe {
        (
            *(slot_sendto as *const usize),
            *(slot_recvfrom as *const usize),
        )
    };
    if held != (st, rf) {
        return say("relay: off - the game's sendto / recvfrom imports are not the ones read");
    }
    REAL_SENDTO.store(st, Release);
    REAL_RECVFROM.store(rf, Release);
    *RELAY.lock().unwrap_or_else(|e| e.into_inner()) = SockaddrIn::new(ip, port);
    MY_SEAT.store(seat as u32, Relaxed);
    RELAY_LOOPBACK.store(e1718, Relaxed);
    let mut old = PAGE_PROTECTION_FLAGS(0);
    let lo = slot_sendto.min(slot_recvfrom) as *const c_void; // the two slots are adjacent
    // SAFETY: the two adjacent slots, 8 bytes.
    let opened = unsafe { VirtualProtect(lo, 8, PAGE_READWRITE, &mut old) };
    if let Err(e) = opened {
        return say(&format!(
            "relay: off - the import slots could not be opened (error {})",
            e.code().0 as u32 & 0xFFFF
        ));
    }
    RELAY_ON.store(true, Release); // before the slots change: the hooks must find it on
    // SAFETY: the slots are writable now; the hooks have the real functions' signatures.
    unsafe {
        *(slot_sendto as *mut usize) = relay_sendto as *const () as usize;
        *(slot_recvfrom as *mut usize) = relay_recvfrom as *const () as usize;
        let _ = VirtualProtect(lo, 8, old, &mut old);
    }
    std::thread::spawn(relay_watch);
    say(&format!(
        "relay: on - seat {seat}; a match against another player goes through {}:{port}",
        Ipv4Addr::from(ip)
    ));
}

fn rt_check(log: fn(&str), name: &str, ok: bool, detail: &str) {
    let sep = if detail.is_empty() { "" } else { "  - " };
    log(&format!(
        "relay test: {}  {name}{sep}{detail}",
        if ok { "PASS" } else { "FAIL" }
    ));
}

/// `rt_recv`: up to 2 s for a datagram through the hook. `-1` an error, `-2` nothing came.
fn rt_recv(s: usize, buf: &mut [u8], from: &mut SockaddrIn) -> i32 {
    for _ in 0..200 {
        let mut fl = SA_LEN as i32;
        // SAFETY: our buffers, sized as passed.
        let r = unsafe {
            relay_recvfrom(
                s,
                buf.as_mut_ptr(),
                buf.len() as i32,
                0,
                (&raw mut *from).cast(),
                &raw mut fl,
            )
        };
        if r != SOCKET_ERROR {
            return r;
        }
        // SAFETY: reads the thread's last socket error.
        if unsafe { WSAGetLastError() } != WSAEWOULDBLOCK {
            return -1;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    -2
}

fn send_via_hook(s: usize, data: &[u8], to: &SockaddrIn) -> i32 {
    // SAFETY: our buffers, sized as passed.
    unsafe {
        relay_sendto(
            s,
            data.as_ptr(),
            data.len() as i32,
            0,
            (&raw const *to).cast(),
            SA_LEN as i32,
        )
    }
}

fn show(r: i32, f: &SockaddrIn) -> String {
    format!(
        "{r} bytes from {}:{}",
        Ipv4Addr::from(f.addr),
        u16::from_be(f.port)
    )
}

/// `relay_selftest` (`WCCFPANEL_RELAYTEST=1`, the tests): two sockets on this PC play seats 1 and 2 through a relay on
/// 127.0.0.1:20941 (the test starts `forza-server relay 30` there, `WCCF_RELAY_PORT=20941`); 127.0.0.2 stands for
/// "another player's address".
pub fn relay_selftest(log: fn(&str)) {
    use std::os::windows::io::AsRawSocket;
    let _ = LOG.set(log);
    let (Ok(ua), Ok(ub)) = (
        UdpSocket::bind("127.0.0.1:0"),
        UdpSocket::bind("127.0.0.1:0"),
    ) else {
        return log("relay test: FAIL  no sockets");
    };
    let _ = (ua.set_nonblocking(true), ub.set_nonblocking(true));
    let (a, b) = (ua.as_raw_socket() as usize, ub.as_raw_socket() as usize);
    let sock = |u: &UdpSocket| match u.local_addr() {
        Ok(SocketAddr::V4(v)) => SockaddrIn::new(v.ip().octets(), v.port()),
        _ => SockaddrIn::NONE,
    };
    let (la, lb) = (sock(&ua), sock(&ub));
    // SAFETY: plain module lookups; ws2_32 is loaded (the sockets above).
    let (st, rf) = unsafe {
        let ws = GetModuleHandleW(w!("ws2_32.dll")).unwrap_or_default();
        (
            GetProcAddress(ws, PCSTR(c"sendto".as_ptr().cast())),
            GetProcAddress(ws, PCSTR(c"recvfrom".as_ptr().cast())),
        )
    };
    let (Some(st), Some(rf)) = (st, rf) else {
        return log("relay test: FAIL  no ws2_32");
    };
    REAL_SENDTO.store(st as usize, Release);
    REAL_RECVFROM.store(rf as usize, Release);
    *RELAY.lock().unwrap_or_else(|e| e.into_inner()) = SockaddrIn::new([127, 0, 0, 1], 20941);
    RELAY_ON.store(true, Release);
    RELAY_TEST.store(true, Release);
    RSOCK.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *PEER.lock().unwrap_or_else(|e| e.into_inner()) = [SockaddrIn::NONE; 9];
    let (mut buf, mut from) = ([0u8; 256], SockaddrIn::NONE);
    let other = |port| SockaddrIn::new([127, 0, 0, 2], port);

    MY_SEAT.store(1, Relaxed); // seat 1 -> seat 2 (2 not known yet)
    let r = send_via_hook(a, b"first-to-2", &other(20002));
    rt_check(
        log,
        "seat 1's packet for seat 2 is taken (as sent)",
        r == 10,
        "",
    );
    std::thread::sleep(Duration::from_millis(200));
    MY_SEAT.store(2, Relaxed); // seat 2 -> seat 1
    send_via_hook(b, b"hello-1", &other(20001));
    let r = rt_recv(a, &mut buf, &mut from);
    let d = show(r, &from);
    rt_check(
        log,
        "seat 1 gets seat 2's packet through the relay",
        r == 7 && &buf[..7] == b"hello-1",
        &d,
    );
    rt_check(
        log,
        "... as if from the address seat 1 sends to (127.0.0.2:20002)",
        r == 7 && from.addr == [127, 0, 0, 2] && u16::from_be(from.port) == 20002,
        &d,
    );
    MY_SEAT.store(1, Relaxed); // and back: seat 2 is known now
    send_via_hook(a, b"hello-2", &other(20002));
    let r = rt_recv(b, &mut buf, &mut from);
    let d = show(r, &from);
    rt_check(
        log,
        "seat 2 gets seat 1's reply through the relay, from 127.0.0.2:20001",
        r == 7 && &buf[..7] == b"hello-2" && u16::from_be(from.port) == 20001,
        &d,
    );
    rt_check(
        log,
        "the first packet (before seat 2 was known) was dropped, not delivered late",
        rt_recv(b, &mut buf, &mut from) == -2,
        "",
    );
    RELAY_LOOPBACK.store(true, Relaxed); // 2017-18's arcade: seat 2 named as 127.0.0.1 by the control
    send_via_hook(a, b"loop-2", &SockaddrIn::new([127, 0, 0, 1], 20002));
    let r = rt_recv(b, &mut buf, &mut from);
    let d = show(r, &from);
    rt_check(
        log,
        "2017-18: seat 1's packet for seat 2 at 127.0.0.1:20002 goes through the relay",
        r == 6 && &buf[..6] == b"loop-2",
        &d,
    );
    RELAY_LOOPBACK.store(false, Relaxed);
    send_via_hook(a, b"direct", &lb); // 127.0.0.1, not a player port: direct
    let r = rt_recv(b, &mut buf, &mut from);
    let d = format!("{r} bytes from port {}", u16::from_be(from.port));
    rt_check(
        log,
        "a packet for this PC's own channel (127.0.0.1) goes direct, untouched",
        r == 6 && &buf[..6] == b"direct" && from.port == la.port,
        &d,
    );
    send_via_hook(a, b"big", &lb);
    std::thread::sleep(Duration::from_millis(50));
    let (mut tiny, mut fl) = ([0u8; 2], SA_LEN as i32);
    // SAFETY: our buffers, sized as passed.
    let (r, e) = unsafe {
        let r = relay_recvfrom(
            b,
            tiny.as_mut_ptr(),
            2,
            0,
            (&raw mut from).cast(),
            &raw mut fl,
        );
        (r, WSAGetLastError())
    };
    rt_check(
        log,
        "a datagram bigger than the buffer: WSAEMSGSIZE, like Windows' own",
        r == SOCKET_ERROR && e == WSAEMSGSIZE,
        "",
    );
    RELAY_ON.store(false, Release);
    let r = send_via_hook(a, b"off", &lb);
    rt_check(log, "switched off: everything goes direct", r == 3, "");
    drop((ua, ub));
    RELAY_TEST.store(false, Relaxed);
    MY_SEAT.store(0, Relaxed);
    RSOCK.lock().unwrap_or_else(|e| e.into_inner()).clear();
    log("relay test: done");
}

// ---------------------------------------------------------------- the ping meter

static PING_MS: AtomicI32 = AtomicI32::new(-1);
static PING_TICK: AtomicU32 = AtomicU32::new(0);
static PING_ON: AtomicBool = AtomicBool::new(false);

/// `ping_once`: one echo, the round trip in ms or -1 (nothing within 1.5 s).
fn ping_once(s: &UdpSocket, to: SocketAddr, seq: u32) -> i64 {
    let mut p = [0u8; 9];
    p[..4].copy_from_slice(MAGIC);
    p[4] = b'P';
    p[5..].copy_from_slice(&seq.to_ne_bytes());
    let t0 = Instant::now();
    if s.send_to(&p, to).is_err() {
        return -1;
    }
    let mut r = [0u8; 64];
    for _ in 0..8 {
        // the answer to THIS echo; a late one to an earlier echo is skipped
        let Ok(n) = s.recv(&mut r) else { return -1 }; // the 1.5 s wait ran out
        if n == 9 && r[..4] == *MAGIC && r[4] == b'Q' && r[5..9] == p[5..] {
            return t0.elapsed().as_millis() as i64;
        }
    }
    -1
}

/// `ping_watch`: an echo every 2 s, a log line every minute.
///
/// ponytail: runs for the process's life, as the C's (the real game never unloads the panel); no stop flag.
fn ping_watch(to: SocketAddr, log: fn(&str)) {
    let s = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            return log(&format!(
                "ping: off - no socket (error {})",
                e.raw_os_error().unwrap_or(0)
            ));
        }
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(1500)));
    {
        use std::os::windows::io::AsRawSocket;
        let (no, mut br) = (0u32, 0u32);
        // a closed port: no answer, not an error
        // SAFETY: valid in/out buffers of the sizes passed; no overlapped I/O.
        unsafe {
            WSAIoctl(
                s.as_raw_socket() as usize,
                SIO_UDP_CONNRESET,
                (&raw const no).cast(),
                4,
                std::ptr::null_mut(),
                0,
                &mut br,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
    }
    PING_ON.store(true, Release);
    // SAFETY: GetTickCount takes nothing.
    let tick = || unsafe { GetTickCount() };
    let mut next = tick().wrapping_add(60000);
    let (mut seq, mut said) = (0u32, false);
    let (mut got, mut lost, mut mn, mut mx, mut sum) = (0i64, 0i64, 0i64, 0i64, 0i64);
    loop {
        let t = tick();
        seq += 1;
        let ms = ping_once(&s, to, seq);
        if ms >= 0 {
            PING_MS.store(ms as i32, Release);
            PING_TICK.store(tick(), Release);
            if !said {
                said = true;
                log(&format!("ping: the server's relay answers - {ms} ms"));
            }
            if got == 0 || ms < mn {
                mn = ms;
            }
            mx = mx.max(ms);
            sum += ms;
            got += 1;
        } else {
            lost += 1;
        }
        if tick().wrapping_sub(next) as i32 >= 0 {
            if got > 0 {
                log(&format!(
                    "ping: last minute {got} answers, {lost} lost, {mn} / {} / {mx} ms (min / average / max)",
                    sum / got
                ));
            } else {
                log(&format!(
                    "ping: last minute no answer from the server's relay ({lost} asked)"
                ));
            }
            (got, lost, sum, mn, mx) = (0, 0, 0, 0, 0);
            next = tick().wrapping_add(60000);
        }
        let t = tick().wrapping_sub(t);
        if t < 2000 {
            std::thread::sleep(Duration::from_millis(u64::from(2000 - t)));
        }
    }
}

/// `ping_install`: a cabinet playing online (the run set `WCCF_RELAY=ADDRESS:PORT`); starts the ping thread.
pub fn ping_install(log: fn(&str)) {
    let Ok(v) = std::env::var("WCCF_RELAY") else {
        return;
    }; // this PC alone: no meter
    let (ip, port) = match parse_relay(&v) {
        Ok(x) => x,
        Err(Bad::Form) => return log(&format!("ping: off - WCCF_RELAY is not ADDRESS:PORT ({v})")),
        Err(Bad::Address(h, p)) => return log(&format!("ping: off - not an address: {h}:{p}")),
    };
    let to = SocketAddr::from((ip, port));
    std::thread::spawn(move || ping_watch(to, log));
    log(&format!(
        "ping: on - asking {}:{port} every 2 s, shown top right above the game",
        Ipv4Addr::from(ip)
    ));
}

/// The meter's number (`ping_draw`): `None` off (this PC, not online); `Some(-1)` no answer for 6 s (or yet);
/// `Some(ms)` the last round trip.
pub fn ping_ms() -> Option<i32> {
    if !PING_ON.load(Acquire) {
        return None;
    }
    let ms = PING_MS.load(Acquire);
    // SAFETY: GetTickCount takes nothing.
    let age = unsafe { GetTickCount() }.wrapping_sub(PING_TICK.load(Acquire));
    Some(if ms >= 0 && age > 6000 { -1 } else { ms })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slot RVAs from each edition's PE import table; a relocated 2017-18 image moves them with its base.
    #[test]
    fn slot_table() {
        assert_eq!(slot_rvas(false), (0x57_E8FC, 0x57_E8F8));
        assert_eq!(slot_rvas(true), (0xDA_9D88, 0xDA_9D8C));
        // at the preferred base the RVAs give back the original virtual addresses
        assert_eq!(PREFERRED_BASE + slot_rvas(false).0, 0x97_E8FC);
        assert_eq!(PREFERRED_BASE + slot_rvas(true).1, 0x11A_9D8C);
        // moved by ASLR to 0x1000_0000
        assert_eq!(0x1000_0000 + slot_rvas(true).0, 0x10DA_9D88);
    }

    fn sa(ip: [u8; 4], port: u16) -> SockaddrIn {
        SockaddrIn::new(ip, port)
    }

    #[test]
    fn peer_rules() {
        assert_eq!(
            peer_seat(&sa([203, 0, 113, 5], 20001), false, None, false),
            Some(1)
        );
        assert_eq!(
            peer_seat(&sa([203, 0, 113, 5], 20008), false, None, false),
            Some(8)
        );
        assert_eq!(
            peer_seat(&sa([203, 0, 113, 5], 20000), false, None, false),
            None
        );
        assert_eq!(
            peer_seat(&sa([203, 0, 113, 5], 20009), false, None, false),
            None
        );
        assert_eq!(
            peer_seat(&sa([0, 0, 0, 0], 20002), false, None, false),
            None
        );
        assert_eq!(
            peer_seat(&sa([255, 255, 255, 255], 20002), false, None, false),
            None
        );
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 1], 20002), false, None, false),
            None
        );
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 2], 20002), false, None, false),
            None
        );
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 2], 20002), true, None, false),
            Some(2)
        );
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 1], 20002), true, None, false),
            None
        );
        let mut v6 = sa([1, 2, 3, 4], 20002);
        v6.family = 23;
        assert_eq!(peer_seat(&v6, false, None, false), None);
        assert_eq!(peer_seat(&v6, false, Some(1), false), None);
    }

    /// 2017-18 in a shared arcade (seat 2 here): another seat at 127.x goes to the relay, its own seat and the
    /// MATCH_PORT and CLIENT_PORT1..3 ports stay direct; other addresses as before.
    #[test]
    fn loopback_peers_in_the_1718_arcade() {
        let mine = Some(2);
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 1], 20001), false, mine, false),
            Some(1)
        );
        assert_eq!(
            peer_seat(&sa([127, 1, 2, 3], 20008), false, mine, false),
            Some(8)
        );
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 1], 20007), false, mine, false),
            Some(7)
        );
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 1], 20002), false, mine, false),
            None
        );
        for port in [20003, 20004, 20005, 20006] {
            assert_eq!(
                peer_seat(&sa([127, 0, 0, 1], port), false, mine, false),
                None
            );
            assert_eq!(
                peer_seat(&sa([192, 168, 56, 1], port), false, mine, false),
                Some(usize::from(port) - 20000)
            );
        }
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 1], 20000), false, mine, false),
            None
        );
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 1], 20009), false, mine, false),
            None
        );
        assert_eq!(
            peer_seat(&sa([192, 168, 56, 1], 20001), false, mine, false),
            Some(1)
        );
        assert_eq!(
            peer_seat(&sa([10, 0, 2, 2], 20001), false, mine, false),
            Some(1)
        );
        assert_eq!(
            peer_seat(&sa([0, 0, 0, 0], 20001), false, mine, false),
            None
        );
    }

    /// A hosted arcade's matches (live 2026-10-27: seats 1-2 and 3-4; the relay takes seats up to 8), every opponent at
    /// 127.0.0.1 through the tunnel. Each seat's packets from the game's peer socket reach its opponent through the relay; seats 3-6 from
    /// another socket (MATCH_PORT, CLIENT_PORT1..3) stay direct.
    #[test]
    fn a_second_match_in_the_arcade_goes_through_the_relay() {
        for (me, opponent) in [
            (1, 2),
            (2, 1),
            (3, 4),
            (4, 3),
            (5, 6),
            (6, 5),
            (7, 8),
            (8, 7),
        ] {
            let to = sa([127, 0, 0, 1], 20000 + opponent as u16);
            assert_eq!(
                peer_seat(&to, false, Some(me), true),
                Some(opponent),
                "seat {me}"
            );
        }
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 1], 20003), false, Some(1), false),
            None
        );
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 1], 20003), false, Some(3), true),
            None
        );
        assert_eq!(
            peer_seat(&sa([127, 0, 0, 1], 20004), false, None, true),
            None
        );
    }

    #[test]
    fn framing() {
        assert_eq!(frame_hello(3), *b"WRL1R\x03");
        let mut out = [0u8; 64];
        let n = frame_d(&mut out, 2, 1, b"abc");
        assert_eq!(&out[..n], b"WRL1D\x02\x01abc");
        let f = b"WRL1F\x02\x7f\x00\x00\x02xyz";
        assert_eq!(parse_f(f), Some((2, [127, 0, 0, 2], &b"xyz"[..])));
        assert_eq!(
            parse_f(b"WRL1F\x02\x7f\x00\x00\x02"),
            Some((2, [127, 0, 0, 2], &b""[..]))
        );
        assert!(parse_f(b"WRL1F\x02\x7f\x00\x00").is_none()); // short
        assert!(parse_f(b"WRL1F\x00\x7f\x00\x00\x02").is_none()); // seat 0
        assert!(parse_f(b"WRL1F\x09\x7f\x00\x00\x02").is_none()); // seat 9
        assert!(parse_f(b"WRL1Q\x02\x7f\x00\x00\x02").is_none()); // not F
        assert!(parse_f(b"XRL1F\x02\x7f\x00\x00\x02").is_none()); // magic
    }

    #[test]
    fn relay_variable() {
        assert_eq!(parse_relay("10.0.0.1:20040"), Ok(([10, 0, 0, 1], 20040)));
        assert_eq!(parse_relay("10.0.0.1"), Err(Bad::Form));
        assert!(matches!(parse_relay("10.0.0.1:0"), Err(Bad::Address(..))));
        assert!(matches!(
            parse_relay("10.0.0.1:65536"),
            Err(Bad::Address(..))
        ));
        assert!(matches!(parse_relay("host:20040"), Err(Bad::Address(..))));
        assert!(matches!(
            parse_relay("255.255.255.255:1"),
            Err(Bad::Address(..))
        ));
        assert_eq!(atoi(" 12ab"), 12);
        assert_eq!(atoi("x"), 0);
    }

    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Needs `forza-server relay 30` with `WCCF_RELAY_PORT=20941` running: run with `--ignored`.
    #[test]
    #[ignore]
    fn selftest_against_a_relay() {
        relay_selftest(|m| {
            println!("{m}");
            LINES.lock().unwrap().push(m.to_owned());
        });
        let l = LINES.lock().unwrap();
        assert_eq!(l.iter().filter(|x| x.contains("PASS")).count(), 9, "{l:?}");
        assert!(!l.iter().any(|x| x.contains("FAIL")), "{l:?}");
        assert_eq!(l.last().map(String::as_str), Some("relay test: done"));
    }
}
