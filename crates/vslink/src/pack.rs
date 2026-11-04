//! lz4 frames for the arcade tunnel's streams, when both ends speak [`crate::arcade::ARCADE_ALPN_LZ4`]. Each TCP read
//! (at most [`MAX_RAW`] bytes) crosses at once as one frame, so nothing waits for more data:
//! `[kind u8][raw length u16 LE][stored length u16 LE][stored bytes]`, kind [`RAW`] (stored = raw) or [`LZ4`] (an lz4
//! block). Each block is compressed against the last [`HISTORY`] raw bytes of its direction, which both ends keep the
//! same, so small game writes still shrink (measured: `E:\wccf-builds\tunnel-compress-20261103\MEASURE.md`).
//! A frame that does not check out is an error, never a panic: the stream ends there.

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

/// The most raw bytes in one frame: the tunnel's read size.
pub const MAX_RAW: usize = 16 * 1024;
/// Raw bytes each block may refer back to (lz4's window).
pub const HISTORY: usize = 64 * 1024;
pub const RAW: u8 = 0;
pub const LZ4: u8 = 1;
const HEAD: usize = 5;

/// The last [`HISTORY`] raw bytes of a direction; kept up to twice that, so trimming moves memory once per 64 KiB.
#[derive(Default)]
struct History(Vec<u8>);

impl History {
    fn dict(&self) -> &[u8] {
        &self.0[self.0.len().saturating_sub(HISTORY)..]
    }

    fn push(&mut self, raw: &[u8]) {
        if self.0.len() + raw.len() > 2 * HISTORY {
            let cut = self.0.len().saturating_sub(HISTORY);
            self.0.drain(..cut);
        }
        self.0.extend_from_slice(raw);
    }
}

/// The sending end of a direction.
#[derive(Default)]
pub struct Packer(History);

impl Packer {
    /// `raw` (1 to [`MAX_RAW`] bytes) as one frame.
    pub fn pack(&mut self, raw: &[u8]) -> Vec<u8> {
        assert!(
            !raw.is_empty() && raw.len() <= MAX_RAW,
            "{} raw bytes",
            raw.len()
        );
        let block = lz4_flex::block::compress_with_dict(raw, self.0.dict());
        let (kind, stored) = if block.len() < raw.len() {
            (LZ4, &block[..])
        } else {
            (RAW, raw)
        };
        let mut f = Vec::with_capacity(HEAD + stored.len());
        f.push(kind);
        f.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        f.extend_from_slice(&(stored.len() as u16).to_le_bytes());
        f.extend_from_slice(stored);
        self.0.push(raw);
        f
    }
}

/// The receiving end of a direction.
#[derive(Default)]
pub struct Unpacker {
    hist: History,
    /// Frame bytes read so far, a frame cut short by the stream's end included
    pub wire: u64,
}

fn bad(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

impl Unpacker {
    /// The next frame's raw bytes; `None` when `r` ends between frames. A short, oversized, cut or undecodable frame
    /// is an error.
    pub async fn next<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> io::Result<Option<Vec<u8>>> {
        let mut head = [0u8; HEAD];
        let n = r.read(&mut head).await?;
        if n == 0 {
            return Ok(None);
        }
        self.wire += n as u64;
        self.fill(r, &mut head[n..]).await?;
        let raw_len = u16::from_le_bytes([head[1], head[2]]) as usize;
        let stored_len = u16::from_le_bytes([head[3], head[4]]) as usize;
        if raw_len == 0 || raw_len > MAX_RAW || stored_len > MAX_RAW {
            return Err(bad(format!(
                "frame of {raw_len} raw, {stored_len} stored bytes"
            )));
        }
        let mut stored = vec![0u8; stored_len];
        self.fill(r, &mut stored).await?;
        let raw = match head[0] {
            RAW if stored_len == raw_len => stored,
            LZ4 => {
                let mut raw = vec![0u8; raw_len];
                match lz4_flex::block::decompress_into_with_dict(
                    &stored,
                    &mut raw,
                    self.hist.dict(),
                ) {
                    Ok(n) if n == raw_len => raw,
                    Ok(n) => return Err(bad(format!("block of {n} bytes, {raw_len} announced"))),
                    Err(e) => return Err(bad(format!("block: {e}"))),
                }
            }
            k => {
                return Err(bad(format!(
                    "frame kind {k}, {raw_len} raw, {stored_len} stored bytes"
                )));
            }
        };
        self.hist.push(&raw);
        Ok(Some(raw))
    }

    /// Fills `buf`, counting each byte into [`Unpacker::wire`]. A stream that ends first is a cut frame, not a clean
    /// end: `read_exact` would report it as `UnexpectedEof`, which callers take for the other side hanging up.
    async fn fill<R: AsyncRead + Unpin>(&mut self, r: &mut R, buf: &mut [u8]) -> io::Result<()> {
        let mut got = 0;
        while got < buf.len() {
            match r.read(&mut buf[got..]).await {
                Ok(0) => return Err(bad(format!("frame cut after {got} of {} bytes", buf.len()))),
                Ok(n) => {
                    got += n;
                    self.wire += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// Bytes as the log shows them: `950 B`, `41 KB`, `1.79 MB` (powers of 1000).
pub fn size(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{} KB", (n + 500) / 1_000),
        _ => format!("{:.2} MB", n as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn unpack_all(wire: &[u8]) -> io::Result<Vec<u8>> {
        let mut u = Unpacker::default();
        let mut r = wire;
        let mut out = Vec::new();
        while let Some(raw) = u.next(&mut r).await? {
            out.extend_from_slice(&raw);
        }
        assert_eq!(u.wire, wire.len() as u64);
        Ok(out)
    }

    /// Payloads of the real sizes: the 261 feed's 125 answers of 2,080 bytes, a 76 KB club in hex JSON cut into
    /// 16 KiB reads, noise that cannot shrink, and 1-byte writes.
    #[tokio::test]
    async fn round_trip_on_real_sizes() {
        let mut feed = Vec::new();
        for i in 0..125u32 {
            let mut answer = vec![0u8; 2080];
            answer[..8].copy_from_slice(&[0xf1, 0xc0, 0x80, 0x44, 0xb4, 0x9e, 0xda, 0x3c]);
            answer[44..48].copy_from_slice(&2032u32.min(0x3DA38 - i * 2032).to_le_bytes());
            feed.push(answer);
        }
        let hex: String = (0..38_000u32)
            .map(|i| format!("{:02x}", if i % 37 == 0 { i % 251 } else { 0 }))
            .collect();
        let club = format!("{{\"aime\":1,\"coaches\":[{{\"save\":{{\"block\":\"{hex}\"}}}}]}}")
            .into_bytes();
        let mut seed = 0x1234_5678u32;
        let noise: Vec<u8> = (0..40_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let cases: Vec<(&str, Vec<Vec<u8>>)> = vec![
            ("261 feed", feed),
            ("club", club.chunks(MAX_RAW).map(<[u8]>::to_vec).collect()),
            ("noise", noise.chunks(MAX_RAW).map(<[u8]>::to_vec).collect()),
            ("single bytes", (0..300u32).map(|i| vec![i as u8]).collect()),
        ];
        for (name, reads) in cases {
            let mut p = Packer::default();
            let wire: Vec<u8> = reads.iter().flat_map(|r| p.pack(r)).collect();
            let raw: Vec<u8> = reads.concat();
            assert_eq!(unpack_all(&wire).await.unwrap(), raw, "{name}");
            eprintln!(
                "{name}: {} -> {}",
                size(raw.len() as u64),
                size(wire.len() as u64)
            );
            match name {
                "261 feed" => assert!(wire.len() < raw.len() / 20, "{name}: {}", wire.len()),
                "noise" => assert!(
                    wire.len() <= raw.len() + 5 * reads.len(),
                    "{name}: raw frames"
                ),
                _ => {}
            }
        }
    }

    /// Every way a frame can be wrong ends the stream with an error: no panic, no oversized buffer.
    #[tokio::test]
    async fn a_corrupt_frame_is_an_error() {
        let mut p = Packer::default();
        let good = p.pack(&[0u8; 4000]);
        assert_eq!(good[0], LZ4);
        let mut flipped = good.clone();
        for b in &mut flipped[HEAD..] {
            *b ^= 0x5a;
        }
        let mut long = good.clone();
        long[1..3].copy_from_slice(&5000u16.to_le_bytes());
        let mut raw_mismatch = good.clone();
        raw_mismatch[0] = RAW;
        let mut kind = good.clone();
        kind[0] = 9;
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("flipped block", flipped),
            ("raw length longer than the block", long),
            ("raw frame of the wrong length", raw_mismatch),
            ("unknown kind", kind),
            ("oversized", vec![RAW, 0xff, 0xff, 0xff, 0xff]),
            ("empty frame", vec![RAW, 0, 0, 0, 0]),
            ("cut header", good[..3].to_vec()),
            ("cut body", good[..good.len() - 1].to_vec()),
        ];
        for (name, wire) in cases {
            match unpack_all(&wire).await {
                Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData, "{name}: {e}"),
                Ok(_) => panic!("{name}: ended cleanly"),
            }
        }
        assert_eq!(unpack_all(&good).await.unwrap(), vec![0u8; 4000]);
        // every truncation of a frame sequence too: a cut between frames is a clean end, one inside a frame is not
        let mut p = Packer::default();
        let wire: Vec<u8> = [&b"abc"[..], &[7u8; 900], b"abcabcabc"]
            .iter()
            .flat_map(|r| p.pack(r))
            .collect();
        for cut in 1..wire.len() {
            let mut u = Unpacker::default();
            let mut r = &wire[..cut];
            loop {
                match u.next(&mut r).await {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(e) => {
                        assert_eq!(e.kind(), io::ErrorKind::InvalidData, "cut {cut}: {e}");
                        break;
                    }
                }
            }
            assert_eq!(
                u.wire, cut as u64,
                "cut {cut}: the bytes of a cut frame count too"
            );
        }
    }

    #[test]
    fn sizes_read_like_the_log() {
        assert_eq!(size(87), "87 B");
        assert_eq!(size(41_234), "41 KB");
        assert_eq!(size(1_792_094), "1.79 MB");
    }
}
