//! The reflect service's hot path (probe echoes and relayed datagrams) allocates nothing per datagram. Its own test
//! binary: the counting global allocator must not count the other tests.

use forza::online::vs::spawn_reflect;
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

const MAGIC: [u8; 8] = [0xF1, 0xC0, 0x80, 0x44, 0xB4, 0x9E, 0xDA, 0x3C];

#[test]
fn echoing_and_relaying_allocates_nothing() {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    let _svc = spawn_reflect(sock, std::sync::Arc::new(|_| {})).unwrap();
    let mk = || {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s
    };
    let (a, b) = (mk(), mk());
    let mut probe = [0u8; 52];
    probe[..8].copy_from_slice(&MAGIC);
    probe[8] = 1;
    // a datagram for b: type 2, b's address at +44, its port at +50
    let mut data = [7u8; 120];
    data[..8].copy_from_slice(&MAGIC);
    data[8..10].copy_from_slice(&2u16.to_le_bytes());
    data[44..48].copy_from_slice(&[127, 0, 0, 1]);
    data[50..52].copy_from_slice(&b.local_addr().unwrap().port().to_le_bytes());
    let mut buf = [0u8; 256];
    // one round: both probe (each answered), then a -> b relayed
    let mut round = || {
        for s in [&a, &b] {
            s.send_to(&probe, addr).unwrap();
            assert_eq!(s.recv(&mut buf).unwrap(), 58);
        }
        a.send_to(&data, addr).unwrap();
        let n = b.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], &data[..]);
        // under the service's 300 relayed datagrams per second and source
        std::thread::sleep(Duration::from_millis(4));
    };
    for _ in 0..50 {
        round(); // warm-up: the probe log lines, the maps' first entries
    }
    // the service answers before it reads the next datagram, so the counter is stable between rounds
    let before = ALLOCS.load(Ordering::SeqCst);
    for _ in 0..1000 {
        round();
    }
    assert_eq!(
        ALLOCS.load(Ordering::SeqCst),
        before,
        "allocations on the hot path"
    );
}
