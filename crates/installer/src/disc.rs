//! WCCF RingEdge disc image -> decrypted game volume, read-only, nothing mounted.
//! Procedure: ringedge/revd-continuation/DECRYPTION.md (TrueCrypt 4.3a hidden volume at the game slot's tail,
//! AES-256 / LRW / RIPEMD-160 / 2000 iterations, 16-byte keyfile, empty password).
use crate::err::{InstallError as E, path};
use aes::Aes256;
use aes::cipher::{Array, BlockCipherDecrypt, KeyInit};
use anyhow::{Result, bail, ensure};
use ripemd::Ripemd160;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

pub struct Disc {
    pub name: &'static str,
    pub game_id: &'static str,
    pub edition: &'static str,
    pub iso_size: u64,
    pub slot_start: u64,
    pub slot_end: u64,
}

// Known layouts only; the 2017–2018 disc puts SBTR before the game slot.
pub const DISCS: [Disc; 2] = [
    Disc {
        name: "WCCF 2010–2011 (Rev D)",
        game_id: "SBWG",
        edition: "2010-2011",
        iso_size: 5_034_770_432,
        slot_start: 0x378000,
        slot_end: 0x10D5F8000,
    },
    Disc {
        name: "WCCF 2017–2018 v2.00.00 (DVR-5059A)",
        game_id: "SDDY",
        edition: "2017-2018",
        iso_size: 5_921_865_728,
        slot_start: 0x1FCB8000,
        slot_end: 0x160F38000,
    },
];

pub fn by_size(size: u64) -> Option<&'static Disc> {
    DISCS.iter().find(|d| d.iso_size == size)
}
const SEGMENT: u64 = 0x40000;
const HIDDEN_HEADER_OFFSET: u64 = 1536; // TrueCrypt: HEADER_SIZE + 2 * SECTOR_SIZE
const PROGRAM_VERSION: u16 = 0x0410; // the highest "required version" TrueCrypt 4.3a accepts

/// GF(2^128) LRW as TrueCrypt 4.3a computes it: tweak(i) = K * i, big-endian integers, x = 2, poly 0x87.
/// Multiplication is linear over XOR, so K * i = XOR of K * 2^j over the set bits j of i.
pub struct Lrw {
    aes: Aes256,
    kx: [u128; 64], // K * 2^j
}

fn double(v: u128) -> u128 {
    (v << 1) ^ if v >> 127 == 1 { 0x87 } else { 0 }
}

impl Lrw {
    fn new(lrw_key: &[u8], aes_key: &[u8]) -> Self {
        let mut v = u128::from_be_bytes(lrw_key.try_into().unwrap());
        let kx = std::array::from_fn(|_| {
            let k = v;
            v = double(v);
            k
        });
        Lrw {
            aes: Aes256::new_from_slice(aes_key).unwrap(),
            kx,
        }
    }

    fn tweak(&self, index: u64) -> u128 {
        (0..64)
            .filter(|j| index >> j & 1 == 1)
            .fold(0, |t, j| t ^ self.kx[j])
    }

    /// Decrypt whole 16-byte blocks; `first` is the LRW index of the first block.
    pub fn decrypt(&self, buf: &mut [u8], first: u64) {
        let mut index = first;
        let mut t = self.tweak(index);
        for chunk in buf.as_chunks_mut::<16>().0 {
            let x = u128::from_be_bytes(*chunk) ^ t;
            let mut block = Array::from(x.to_be_bytes());
            self.aes.decrypt_block(&mut block);
            chunk.copy_from_slice(&(u128::from_be_bytes(block.into()) ^ t).to_be_bytes());
            // ponytail: one block per AES call; batch with decrypt_blocks if extraction is too slow.
            let next = index + 1;
            t = (0..64)
                .filter(|j| (index ^ next) >> j & 1 == 1)
                .fold(t, |t, j| t ^ self.kx[j]);
            index = next;
        }
    }
}

/// TrueCrypt 4.3a KeyFilesApply with an empty password: the keyfile's CRC pool (64 bytes) becomes the password.
fn password_from_keyfile(data: &[u8]) -> [u8; 64] {
    let mut pool = [0u8; 64];
    let (mut crc, mut pos) = (0xFFFF_FFFFu32, 0);
    for &b in &data[..data.len().min(1 << 20)] {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                crc >> 1 ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
        for byte in crc.to_be_bytes() {
            pool[pos] = pool[pos].wrapping_add(byte);
            pos = (pos + 1) % 64;
        }
    }
    pool
}

/// The key file may be the raw 16 bytes or its 32 hex digits as text (how the public key lists print it).
fn key_candidates(raw: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![raw.to_vec()];
    let text: String = String::from_utf8_lossy(raw).split_whitespace().collect();
    if text.len() >= 2
        && text.len().is_multiple_of(2)
        && text.bytes().all(|b| b.is_ascii_hexdigit())
    {
        let bytes = (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap());
        out.push(bytes.collect());
    }
    out
}

struct Header {
    hidden_size: u64,
    lrw: Lrw,
}

fn read_header(raw: &[u8; 512], keyfile: &[u8]) -> Option<Result<Header>> {
    let mut dk = [0u8; 64];
    pbkdf2::pbkdf2_hmac::<Ripemd160>(&password_from_keyfile(keyfile), &raw[..64], 2000, &mut dk);
    let mut h = *raw;
    Lrw::new(&dk[..16], &dk[32..]).decrypt(&mut h[64..], 1);
    let be16 = |o: usize| u16::from_be_bytes(h[o..o + 2].try_into().unwrap());
    let be32 = |o: usize| u32::from_be_bytes(h[o..o + 4].try_into().unwrap());
    if &h[64..68] != b"TRUE" || be32(72) != crc32fast::hash(&h[256..]) {
        return None;
    }
    if be16(70) > PROGRAM_VERSION {
        return Some(Err(E::NewerTrueCrypt.into()));
    }
    let hidden_size = u64::from_be_bytes(h[92..100].try_into().unwrap());
    Some(Ok(Header {
        hidden_size,
        lrw: Lrw::new(&h[256..272], &h[288..320]),
    }))
}

/// Entry 0 of the slot's CRC table covers the metadata block minus its own four bytes (mxinstaller 0x401b20).
/// Returns the per-block CRC table (entry i = CRC-32 of data block i).
fn check_metadata(f: &mut File, disc: &Disc) -> Result<Vec<u32>> {
    let n = ((disc.slot_end - disc.slot_start) / SEGMENT) as usize;
    let size = ((512 + 4 * n as u64).div_ceil(SEGMENT) * SEGMENT) as usize;
    let mut block = vec![0u8; size];
    f.seek(SeekFrom::Start(disc.slot_start))?;
    f.read_exact(&mut block)?;
    let mut h = crc32fast::Hasher::new();
    h.update(&block[..512]);
    h.update(&block[516..]);
    let table: Vec<u32> = block[512..512 + 4 * n]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    ensure!(
        h.finalize() == table[0],
        E::ChecksumTable {
            name: disc.name.to_string()
        }
    );
    Ok(table)
}

/// CRC-32 of every data block of the game slot against the disc's table; `progress` gets bytes read so far.
pub fn verify_blocks(
    iso: &Path,
    disc: &Disc,
    table: &[u32],
    mut progress: impl FnMut(u64),
    mut keep_going: impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut f = open_iso(iso)?;
    ensure!(
        f.metadata()?.len() == disc.iso_size
            && table.len() as u64 == (disc.slot_end - disc.slot_start) / SEGMENT,
        E::TableMismatch
    );
    let mut block = vec![0u8; SEGMENT as usize];
    for (i, &want) in table.iter().enumerate().skip(1) {
        keep_going()?;
        f.seek(SeekFrom::Start(disc.slot_start + i as u64 * SEGMENT))?;
        f.read_exact(&mut block).map_err(|_| E::ShortRead)?;
        ensure!(
            crc32fast::hash(&block) == want,
            E::BlockCorrupt { block: i }
        );
        progress(i as u64 * SEGMENT);
    }
    Ok(())
}

fn open_iso(iso: &Path) -> Result<File> {
    Ok(File::open(iso).map_err(|e| E::CannotOpen {
        path: path(iso),
        detail: e.to_string(),
    })?)
}

/// Returns the decrypted game volume (an NTFS image, read on demand) and the slot's block CRC table.
pub fn open(iso: &Path, keyfile: &[u8]) -> Result<(Volume, Vec<u32>)> {
    let mut f = open_iso(iso)?;
    let len = f.metadata()?.len();
    let disc = by_size(len).ok_or_else(|| E::UnsupportedSize {
        path: path(iso),
        len,
    })?;
    let table = check_metadata(&mut f, disc)?;
    let mut raw = [0u8; 512];
    f.seek(SeekFrom::Start(disc.slot_end - HIDDEN_HEADER_OFFSET))?;
    f.read_exact(&mut raw)?;
    let Some(header) = key_candidates(keyfile)
        .iter()
        .find_map(|k| read_header(&raw, k))
    else {
        bail!(E::WrongKey);
    };
    let header = header?;
    let size = header.hidden_size;
    ensure!(
        size > 0
            && size % 512 == 0
            && size <= disc.slot_end - disc.slot_start - HIDDEN_HEADER_OFFSET - 512,
        E::BadVolumeSize
    );
    let base = disc.slot_end - HIDDEN_HEADER_OFFSET - size;
    let vol = Volume {
        f,
        disc,
        base,
        size,
        lrw: header.lrw,
        pos: 0,
        cache_at: u64::MAX,
        cache: Vec::new(),
    };
    Ok((vol, table))
}

const CHUNK: u64 = 64 * 1024; // multiple of 512

/// Read + Seek over the decrypted volume: sector s (counted from the volume start) uses LRW index (s << 5) | 1.
pub struct Volume {
    f: File,
    pub disc: &'static Disc,
    base: u64,
    pub size: u64,
    lrw: Lrw,
    pos: u64,
    cache_at: u64,
    cache: Vec<u8>,
}

impl Read for Volume {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.size || out.is_empty() {
            return Ok(0);
        }
        let at = self.pos / CHUNK * CHUNK;
        if self.cache_at != at {
            self.cache.resize((CHUNK).min(self.size - at) as usize, 0);
            self.f.seek(SeekFrom::Start(self.base + at))?;
            self.f.read_exact(&mut self.cache)?;
            self.lrw.decrypt(&mut self.cache, (at / 512) << 5 | 1);
            self.cache_at = at;
        }
        let start = (self.pos - at) as usize;
        let n = out.len().min(self.cache.len() - start);
        out[..n].copy_from_slice(&self.cache[start..start + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Volume {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let new = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self.size.checked_add_signed(d),
        };
        self.pos =
            new.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// Bitwise reference: K * index, schoolbook, as GfMul128Basic does it.
    fn gf_mul(k: u128, index: u64) -> u128 {
        let (mut a, mut p) = (k, 0u128);
        for i in 0..128 {
            if i < 64 && index >> i & 1 == 1 {
                p ^= a;
            }
            a = double(a);
        }
        p
    }

    #[test]
    fn lrw_tweak_table_and_stepping_match_schoolbook() {
        let key: [u8; 16] = *b"0123456789abcdef";
        let lrw = Lrw::new(&key, &[7u8; 32]);
        let k = u128::from_be_bytes(key);
        for i in [0, 1, 2, 31, 32, 0xFFFF, 0x1_0000, 0xFFFF_FFFF, u64::MAX] {
            assert_eq!(lrw.tweak(i), gf_mul(k, i));
        }
        // incremental stepping inside decrypt() must agree with the direct tweak across a carry boundary
        let mut buf = vec![0u8; 16 * 40];
        let mut whole = buf.clone();
        lrw.decrypt(&mut whole, 0xFFF0);
        for (n, c) in buf.as_chunks_mut::<16>().0.iter_mut().enumerate() {
            lrw.decrypt(c, 0xFFF0 + n as u64);
        }
        assert_eq!(buf, whole);
    }

    #[test]
    fn supported_layouts_select_the_game_slot() {
        let old = by_size(5_034_770_432).unwrap();
        assert_eq!(
            (old.game_id, old.slot_start, old.slot_end),
            ("SBWG", 0x378000, 0x10D5F8000)
        );
        let new = by_size(5_921_865_728).unwrap();
        assert_eq!(
            (new.game_id, new.slot_start, new.slot_end),
            ("SDDY", 0x1FCB8000, 0x160F38000)
        );
        assert_eq!((new.slot_end - new.slot_start) / SEGMENT, 20_554);
        assert!(by_size(0).is_none());
        assert!(by_size(new.iso_size - 2048).is_none());
    }

    /// Needs either real disc and its key: WCCF_ISO=... WCCF_KEY=... cargo test -- --ignored
    /// First-MiB hashes were independently recovered with the TrueCrypt 4.3a C extractor.
    #[test]
    #[ignore]
    fn first_mib_matches_known_decryption() {
        let key = std::fs::read(std::env::var("WCCF_KEY").unwrap()).unwrap();
        let (mut vol, _) = open(Path::new(&std::env::var("WCCF_ISO").unwrap()), &key).unwrap();
        let (size, expected) = match vol.disc.game_id {
            "SBWG" => (
                4_001_366_016,
                "18910a50c7d76ed0905b3ad075294c9db70592674e8a5edbb5dbe601a6f10d08",
            ),
            "SDDY" => (
                4_870_635_520,
                "977fe11057720e6a0f95a6be03ab988c3ec3949d6491dfcebacd86e14846a2e0",
            ),
            _ => unreachable!(),
        };
        assert_eq!(vol.size, size);
        let mut mib = vec![0u8; 1 << 20];
        vol.read_exact(&mut mib).unwrap();
        let digest = Sha256::digest(&mib);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, expected);
        // The same key exported as text must unlock the same layout too.
        let raw_key = key_candidates(&key)
            .into_iter()
            .find(|k| k.len() == 16)
            .unwrap();
        let hex_key: String = raw_key.iter().map(|b| format!("{b:02x}")).collect();
        let (mut text_vol, _) = open(
            Path::new(&std::env::var("WCCF_ISO").unwrap()),
            hex_key.as_bytes(),
        )
        .unwrap();
        text_vol.read_exact(&mut mib).unwrap();
        assert_eq!(Sha256::digest(&mib), digest);
    }
}
