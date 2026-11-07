//! Runs natively on Windows to check a Linux endpoint, without launching a game.
// No call that can panic (tests may, crates/clippy.toml)
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::string_slice
)]
use std::{
    io::{self, BufRead, BufReader, Write},
    net::{SocketAddr, TcpStream, UdpSocket},
    time::Duration,
};
fn connect(addr: SocketAddr) -> io::Result<TcpStream> {
    let c = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    c.set_read_timeout(Some(Duration::from_secs(3)))?;
    c.set_write_timeout(Some(Duration::from_secs(3)))?;
    Ok(c)
}
fn request(c: &mut TcpStream, text: &str) -> io::Result<String> {
    c.write_all(text.as_bytes())?;
    let mut out = String::new();
    BufReader::new(c).read_line(&mut out)?;
    Ok(out)
}
fn main() -> io::Result<()> {
    let host = std::env::args()
        .nth(1)
        .ok_or_else(|| io::Error::other("usage: check-network HOST"))?;
    let address = |port| {
        format!("{host}:{port}")
            .parse::<SocketAddr>()
            .map_err(io::Error::other)
    };
    drop(connect(address(20002)?)?);
    println!("PASS: native Windows -> {host}:20002 controller TCP");
    let mut a = connect(address(20030)?)?;
    let mut b = connect(address(20030)?)?;
    let seat_a = request(&mut a, "WANT\n")?;
    let seat_b = request(&mut b, "WANT\n")?;
    assert!(seat_a.starts_with("SEAT ") && seat_b.starts_with("SEAT "));
    assert_ne!(
        seat_a.split_whitespace().nth(1),
        seat_b.split_whitespace().nth(1)
    );
    assert_eq!(request(&mut a, "PING\n")?, "PONG\n");
    assert_eq!(request(&mut b, "PING\n")?, "PONG\n");
    let seat = |answer: &str| {
        answer
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u8>().ok())
            .ok_or_else(|| io::Error::other(format!("no seat in {answer:?}")))
    };
    let (n, m) = (seat(&seat_a)?, seat(&seat_b)?);
    let mut taken = connect(address(20030)?)?;
    assert_eq!(
        request(&mut taken, &format!("WANT SEAT {n}\n"))?,
        format!("TAKEN {n}\n")
    );
    println!("PASS: seat broker leases distinct seats, refuses collisions, answers PING");
    let u = UdpSocket::bind("0.0.0.0:0")?;
    let v = UdpSocket::bind("0.0.0.0:0")?;
    u.connect(address(20040)?)?;
    v.connect(address(20040)?)?;
    u.set_read_timeout(Some(Duration::from_secs(3)))?;
    v.set_read_timeout(Some(Duration::from_secs(3)))?;
    u.send(&[b'W', b'R', b'L', b'1', b'R', n])?;
    v.send(&[b'W', b'R', b'L', b'1', b'R', m])?;
    std::thread::sleep(Duration::from_millis(100));
    let packet = |to, from| {
        [
            &[b'W', b'R', b'L', b'1', b'D', to, from],
            &b"windows-to-wsl"[..],
        ]
        .concat()
    };
    u.send(&packet(m, n))?;
    let mut buf = [0u8; 128];
    let got = v.recv(&mut buf)?;
    assert_eq!(&buf[..6], &[b'W', b'R', b'L', b'1', b'F', n]);
    assert_eq!(buf.get(10..got), Some(&b"windows-to-wsl"[..]));
    v.send(&packet(n, m))?;
    let got = u.recv(&mut buf)?;
    assert_eq!(&buf[..6], &[b'W', b'R', b'L', b'1', b'F', m]);
    assert_eq!(buf.get(10..got), Some(&b"windows-to-wsl"[..]));
    println!("PASS: native Windows <-> {host} UDP 20040, both relay directions");
    Ok(())
}
