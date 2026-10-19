//! The relay's hot path allocates nothing per datagram. This file has its own counting global allocator, so it is
//! its own test binary and keeps the other tests unaffected.

use forza_server::Edition;
use forza_server::relay::{RelayConfig, start};
use std::alloc::{GlobalAlloc, Layout, System};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Counting;
// SAFETY: every method forwards to `System` unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwards the caller's contract to the system allocator unchanged.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: forwards the caller's contract to the system allocator unchanged.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwards the caller's contract to the system allocator unchanged.
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

#[test]
fn relaying_datagrams_and_pings_allocates_nothing() {
    let mut c = RelayConfig::new(Edition::E1011);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    let relay = start(c, std::sync::Arc::new(|_| {})).unwrap();
    let mk = || {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s
    };
    let (a, b) = (mk(), mk());
    for (s, seat) in [(&a, 1u8), (&b, 2)] {
        s.send_to(&[b'W', b'R', b'L', b'1', b'R', seat], relay.addr())
            .unwrap();
    }
    let d = [b'W', b'R', b'L', b'1', b'D', 2, 1, 9, 9, 9, 9, 9, 9, 9, 9];
    let ping = [b'W', b'R', b'L', b'1', b'P', 1, 2, 3];
    let mut buf = [0u8; 64];
    // one round: a datagram a -> b, and a ping answered
    let mut round = || {
        a.send_to(&d, relay.addr()).unwrap();
        let n = b.recv(&mut buf).unwrap();
        assert_eq!(
            &buf[..n],
            [
                b'W', b'R', b'L', b'1', b'F', 1, 127, 0, 0, 1, 9, 9, 9, 9, 9, 9, 9, 9
            ]
        );
        a.send_to(&ping, relay.addr()).unwrap();
        let n = a.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], [b'W', b'R', b'L', b'1', b'Q', 1, 2, 3]);
    };
    for _ in 0..50 {
        round(); // warm-up: the seat log line, the count map entry
    }
    // the relay answers a round only after finishing its work, so the counter is stable between rounds
    let before = ALLOCS.load(Ordering::SeqCst);
    for _ in 0..2000 {
        round();
    }
    // the test thread itself allocates nothing in `round`; any growth is the relay's
    assert_eq!(
        ALLOCS.load(Ordering::SeqCst),
        before,
        "allocations on the hot path"
    );
}
