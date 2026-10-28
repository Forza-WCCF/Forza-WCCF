//! Whole messages for the server (2026-10-11; 2017-18's control since 2026-10-28): control_Release only, the players' connections only (the ones it accepts
//! on port 20002). recv gives the server whole batches and nothing else ([`crate::wholemsg`] says why: a batch in pieces
//! crashed the server, cutting every player). A batch whose rest has not come yet: "nothing more right now"
//! (WSAEWOULDBLOCK); the server's own FUN_004123e0 simply waits on that, and as the hook has read everything that was
//! there, the socket's read event comes again with the rest. Bytes that cannot be a batch are skipped; nobody is
//! disconnected for them. WCCF_WHOLEMSG=0 turns it off.

use crate::log::log;
use crate::patch::{Real, exe_is, patch};
use crate::wholemsg::{MAX_BATCH, whole};
use forza_native::By;
use std::sync::{Arc, Mutex, PoisonError};
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, INVALID_SOCKET, SOCKADDR, SOCKADDR_IN, SOCKET, SOCKET_ERROR, WSAEWOULDBLOCK,
    WSAGetLastError, WSASetLastError, getsockname,
};

/// The players' port: CONTROL port in the settings.
const PORT: u16 = 20002;
const SOCKS: usize = 64;
/// Never full: an unfinished batch is under MAX_BATCH, and it is read only when no whole one is waiting.
const CAP: usize = MAX_BATCH * 2;

type Accept = unsafe extern "system" fn(SOCKET, *mut SOCKADDR, *mut i32) -> SOCKET;
type Recv = unsafe extern "system" fn(SOCKET, *mut u8, i32, i32) -> i32;
type CloseSocket = unsafe extern "system" fn(SOCKET) -> i32;
static REAL_ACCEPT: Real<Accept> = Real::new();
static REAL_RECV: Real<Recv> = Real::new();
static REAL_CLOSESOCKET: Real<CloseSocket> = Real::new();

struct Sock {
    who: String,
    /// read from the player, not given to the server yet
    buf: Vec<u8>,
    given: u64,
    waits: u64,
    skipped: u64,
    largest: usize,
}

static FOLLOWED: Mutex<Vec<(SOCKET, Arc<Mutex<Sock>>)>> = Mutex::new(Vec::new());

fn find(s: SOCKET) -> Option<Arc<Mutex<Sock>>> {
    let list = FOLLOWED.lock().unwrap_or_else(PoisonError::into_inner);
    list.iter().find(|(k, _)| *k == s).map(|(_, w)| w.clone())
}

pub fn install() {
    if !exe_is("control_Release.exe") {
        return;
    }
    if std::env::var("WCCF_WHOLEMSG").is_ok_and(|v| v == "0") {
        log!(
            "whole messages: OFF (WCCF_WHOLEMSG=0) - the server reads the players' batches as originally written"
        );
        return;
    }
    // 2017-18: only the control whose stitching code this was checked against
    #[cfg(not(feature = "e1011"))]
    for (off, want) in crate::wholemsg::CONTROL_1718 {
        // SAFETY: inside control's loaded image (both sites lie in its code)
        let got = unsafe { std::slice::from_raw_parts(crate::patch::exe().add(off), want.len()) };
        if got != want {
            log!(
                "whole messages: NOT installed - control's bytes at 0x{:X} differ from 2017-18's ({got:02X?}), another build",
                0x40_0000 + off
            );
            return;
        }
    }
    // ws2_32 ordinals: 3 closesocket, 1 accept, 16 recv, in this order, so a recv is followed only once the other two
    // are in place (a hook that cannot follow a connection reads it as before)
    let c = patch(
        &REAL_CLOSESOCKET,
        "ws2_32.dll",
        By::Ordinal(3),
        hook_closesocket,
    );
    let a = c && patch(&REAL_ACCEPT, "ws2_32.dll", By::Ordinal(1), hook_accept);
    let r = a && patch(&REAL_RECV, "ws2_32.dll", By::Ordinal(16), hook_recv);
    log!(
        "whole messages: {} (closesocket={} accept={} recv={}) - the server gets the players' batches on port {PORT} only whole",
        if r { "ON" } else { "NOT installed" },
        u8::from(c),
        u8::from(a),
        u8::from(r)
    );
}

fn ipv4(a: &SOCKADDR_IN) -> String {
    // SAFETY: an AF_INET address's union holds the four octets
    let o = unsafe { a.sin_addr.S_un.S_un_b };
    format!(
        "{}.{}.{}.{}:{}",
        o.s_b1,
        o.s_b2,
        o.s_b3,
        o.s_b4,
        u16::from_be(a.sin_port)
    )
}

unsafe extern "system" fn hook_accept(ls: SOCKET, addr: *mut SOCKADDR, alen: *mut i32) -> SOCKET {
    // SAFETY: the game's own call, passed on; its address buffer as it gave it
    let s = unsafe { REAL_ACCEPT.get()(ls, addr, alen) };
    // SAFETY: an AF_INET name fits a SOCKADDR_IN
    let mut me: SOCKADDR_IN = unsafe { std::mem::zeroed() };
    let mut n = size_of::<SOCKADDR_IN>() as i32;
    if s == INVALID_SOCKET
        // SAFETY: the listening socket the game gave, and our own buffer
        || unsafe { getsockname(ls, (&raw mut me).cast(), &mut n) } != 0
        || me.sin_family != AF_INET
        || u16::from_be(me.sin_port) != PORT
    {
        return s;
    }
    // SAFETY: the game's address buffer, read only when it says it holds an AF_INET address
    let who = unsafe {
        match (addr.as_ref(), alen.as_ref()) {
            (Some(a), Some(&len))
                if len >= size_of::<SOCKADDR_IN>() as i32 && a.sa_family == AF_INET =>
            {
                ipv4(&*addr.cast::<SOCKADDR_IN>())
            }
            _ => "a player".into(),
        }
    };
    let mut list = FOLLOWED.lock().unwrap_or_else(PoisonError::into_inner);
    let mut buf = Vec::new();
    if list.len() < SOCKS && buf.try_reserve_exact(CAP).is_ok() {
        let sock = Sock {
            who,
            buf,
            given: 0,
            waits: 0,
            skipped: 0,
            largest: 0,
        };
        list.push((s, Arc::new(Mutex::new(sock))));
    } else {
        log!("whole messages: no room to follow one more connection - it is read as before");
    }
    s
}

unsafe extern "system" fn hook_recv(s: SOCKET, buf: *mut u8, len: i32, flags: i32) -> i32 {
    // the server's call: recv(s, b, 0x80000, 0)
    let w = if flags == 0 && len >= MAX_BATCH as i32 {
        find(s)
    } else {
        None
    };
    let Some(w) = w else {
        // SAFETY: the game's own call, passed on
        return unsafe { REAL_RECV.get()(s, buf, len, flags) };
    };
    let mut w = w.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        let (skip, n) = whole(&w.buf, len as usize);
        if skip > 0 {
            w.skipped += skip as u64;
            let head = |i: usize| {
                w.buf
                    .get(i..i + 4)
                    .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            };
            log!(
                "whole messages: {}: {skip} byte(s) that cannot be a message skipped (they began count {:?} total {:?}) - the player stays connected",
                w.who,
                head(0),
                head(4)
            );
            w.buf.drain(..skip);
        }
        if n > 0 {
            // SAFETY: the server's buffer holds len >= n bytes
            unsafe { std::ptr::copy_nonoverlapping(w.buf.as_ptr(), buf, n) };
            w.buf.drain(..n);
            w.given += 1;
            w.largest = w.largest.max(n);
            return n as i32;
        }
        let have = w.buf.len();
        let room = CAP - have;
        w.buf.reserve_exact(room);
        // SAFETY: reads into the spare capacity just reserved; only the bytes received are taken in
        let r = unsafe { REAL_RECV.get()(s, w.buf.as_mut_ptr().add(have), room as i32, 0) };
        if r > 0 {
            // SAFETY: recv wrote r bytes there
            unsafe { w.buf.set_len(have + r as usize) };
            continue;
        }
        if r == 0 {
            w.buf.clear(); // the player's side closed: an unfinished batch goes with it, as before
            return 0;
        }
        // SAFETY: plain winsock calls; the error goes back to the server as recv left it
        unsafe {
            let err = WSAGetLastError();
            if err == WSAEWOULDBLOCK && have > 0 {
                w.waits += 1; // a batch in pieces: held until its rest comes
            } else if err != WSAEWOULDBLOCK {
                w.buf.clear();
            }
            WSASetLastError(err);
        }
        return SOCKET_ERROR;
    }
}

unsafe extern "system" fn hook_closesocket(s: SOCKET) -> i32 {
    let gone = {
        let mut list = FOLLOWED.lock().unwrap_or_else(PoisonError::into_inner);
        list.iter()
            .position(|(k, _)| *k == s)
            .map(|i| list.swap_remove(i).1)
    };
    if let Some(w) = gone {
        let w = w.lock().unwrap_or_else(PoisonError::into_inner);
        log!(
            "whole messages: {} closed - {} times given whole batches, {} times a batch waited for its rest, {} byte(s) skipped, largest {} bytes",
            w.who,
            w.given,
            w.waits,
            w.skipped,
            w.largest
        );
    }
    // SAFETY: the game's own call, passed on
    unsafe { REAL_CLOSESOCKET.get()(s) }
}
