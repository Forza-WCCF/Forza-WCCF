//! Stand-in for the original AiMeDB (Aime card -> Aime ID), for 2017–18 on one PC. Port of `_aimedb.py`.
//!
//! Packets are AES-128-ECB with the 16-byte key `KEY` (client VA 0x00D8AC30), with a 32-byte little-endian
//! header: u16 magic 0xA13E, u16 version, u16 command, u16 length (whole packet), u16 status (1 = ok),
//! char game_id[6], u32 store_id, char keychip_id[12]. Commands follow the public Artemis server: 0x04 lookup
//! (access code) -> 0x06, 0x05 register -> 0x06, 0x01 FeliCa lookup -> 0x03, 0x09 log -> 0x0A, 0x0B campaign ->
//! 0x0C, 0x0D touch -> 0x0E, 0x0F lookup2 -> 0x10, 0x11 FeliCa lookup2 -> 0x12, 0x13 log2 -> 0x14, 0x64 hello ->
//! 0x65, 0x66 goodbye. The database (JSON, written atomically) maps access codes to Aime IDs; an unknown card gets
//! a new ID on lookup, [`hash_id`] of its access code, so a card always has one club owner and two homes on one game
//! server never share one. Cards mapped before the hash (the Python counted from 1, so every PC's first card is 1)
//! keep their ID on this PC, where their clubs are; online ([`spawn_with`] `online`) every card answers its hash.

use super::{Handle, Log, accept_loop, read_some};
use aes::Aes128;
use aes::cipher::{Array, BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The client's key (VA 0x00D8AC30): "Copyright(C)" plus the publisher's four capitals.
const KEY: &[u8; 16] = &[
    0x43, 0x6F, 0x70, 0x79, 0x72, 0x69, 0x67, 0x68, 0x74, 0x28, 0x43, 0x29, 0x53, 0x45, 0x47, 0x41,
];
const MAGIC: u16 = 0xA13E;
const HDR: usize = 32;

/// AES-128-ECB over whole 16-byte blocks.
pub struct Aes(Aes128);

impl Aes {
    pub fn new(key: &[u8; 16]) -> Self {
        Self(Aes128::new(&Array::from(*key)))
    }

    pub fn encrypt(&self, data: &mut [u8]) {
        for c in data.as_chunks_mut::<16>().0 {
            let mut b = Array::from(*c);
            self.0.encrypt_block(&mut b);
            *c = b.into();
        }
    }

    pub fn decrypt(&self, data: &mut [u8]) {
        for c in data.as_chunks_mut::<16>().0 {
            let mut b = Array::from(*c);
            self.0.decrypt_block(&mut b);
            *c = b.into();
        }
    }
}

/// A card's Aime ID from its access code: 31 bits of its SHA-256, never 0.
/// ponytail: two cards collide with probability ~n^2/2^32 for n cards (n = 1000 gives about 1 in 4300); past that,
/// hand out IDs from the game server instead.
/// shortcut: hashed IDs share the integer space with the stores' older IDs (counted from 1), unchecked; a hash
/// below 1,000 is about 1 in 2 million per card. Upgrade with the game-server IDs above, or when arcades pass a few
/// hundred cards. The range cannot move: VS clubs on the shared server already live under these IDs.
pub fn hash_id(code: &str) -> i32 {
    let h = Sha256::digest(code.as_bytes());
    (u32::from_be_bytes([h[0], h[1], h[2], h[3]]) & 0x7FFF_FFFF).max(1) as i32
}

/// Access code (20 hex digits as text) -> Aime ID, kept in a JSON file.
pub struct Db {
    path: PathBuf,
    cards: Mutex<BTreeMap<String, i32>>,
    /// Online: every card answers [`hash_id`], a mapping from before the hash only stays in the file.
    pub online: bool,
}

impl Db {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let cards = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).map_err(io::Error::other)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path,
            cards: Mutex::new(cards),
            online: false,
        })
    }

    pub fn len(&self) -> usize {
        self.cards.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The card's Aime id in this PC's own store (its mapping from before the hash, else the hash), read only: a
    /// arcade guest's home club is found and written back under it (online::club).
    pub fn home_id(&self, code: &str) -> i32 {
        self.cards
            .lock()
            .unwrap()
            .get(code)
            .copied()
            .unwrap_or_else(|| hash_id(code))
    }

    pub fn aime_id(&self, code: &str) -> i32 {
        let mut cards = self.cards.lock().unwrap();
        if !cards.contains_key(code) {
            cards.insert(code.to_string(), hash_id(code));
            let tmp = self.path.with_extension("json.tmp");
            let saved = (|| {
                let mut f = std::fs::File::create(&tmp)?;
                f.write_all(&serde_json::to_vec_pretty(&*cards).map_err(io::Error::other)?)?;
                f.sync_all()?;
                std::fs::rename(&tmp, &self.path)
            })();
            if let Err(e) = saved {
                // keep serving: the ID lives in memory until the next successful save
                eprintln!("aimedb: cannot save {}: {e}", self.path.display());
            }
        }
        if self.online {
            hash_id(code)
        } else {
            cards[code]
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Builds a reply with the request's header fields, status 1, `length` bytes in all.
fn reply(req: &[u8], cmd: u16, length: usize, body: &[u8]) -> Vec<u8> {
    let mut p = req[..HDR].to_vec();
    p[0..2].copy_from_slice(&MAGIC.to_le_bytes());
    p[4..6].copy_from_slice(&cmd.to_le_bytes());
    p[6..8].copy_from_slice(&(length as u16).to_le_bytes());
    p[8..10].copy_from_slice(&1u16.to_le_bytes());
    p.extend_from_slice(body);
    p.resize(length, 0);
    p
}

fn id_body(aid: i32) -> Vec<u8> {
    let mut b = aid.to_le_bytes().to_vec();
    b.push(0);
    b
}

/// The decrypted reply to one decrypted request, or `None` for goodbye.
fn handle(data: &[u8], db: &Db, log: &Log) -> Option<Vec<u8>> {
    let cmd = u16::from_le_bytes([data[4], data[5]]);
    // access code: 10 bytes BCD at 0x20 (a short packet gives a short slice, as in the Python)
    let at = |from: usize, n: usize| -> Vec<u8> {
        data[from.min(data.len())..(from + n).min(data.len())].to_vec()
    };
    Some(match cmd {
        0x04 | 0x05 | 0x0F => {
            let code = hex(&at(0x20, 10));
            let aid = db.aime_id(&code);
            log(&format!("   access code {code} -> Aime ID {aid}"));
            if cmd == 0x0F {
                reply(data, 0x10, 0x130, &id_body(aid))
            } else {
                reply(data, 0x06, 0x30, &id_body(aid))
            }
        }
        0x01 | 0x11 => {
            // FeliCa: IDm at 0x20 -> its own access code
            let idm = at(0x20, 8);
            let n = idm.iter().fold(0u64, |n, b| n << 8 | *b as u64);
            let code = format!("{n:020}");
            let aid = db.aime_id(&code);
            log(&format!(
                "   FeliCa IDm {} -> access code {code}, Aime ID {aid}",
                hex(&idm)
            ));
            let bcd: Vec<u8> = (0..code.len() / 2)
                .map(|i| u8::from_str_radix(&code[i * 2..i * 2 + 2], 16).unwrap_or(0))
                .collect();
            if cmd == 0x01 {
                reply(data, 0x03, 0x30, &[&0u32.to_le_bytes()[..], &bcd].concat())
            } else {
                let mut b = id_body(aid);
                b.resize(0x2C, 0);
                b.extend_from_slice(&bcd);
                reply(data, 0x12, 0x140, &b)
            }
        }
        0x0B => reply(data, 0x0C, 0x200, &[]),
        0x0D => reply(data, 0x0E, 0x50, &[0x6F, 0, 0, 0, 1, 0, 0, 0]),
        0x09 | 0x13 => reply(data, cmd + 1, 0x20, &[]),
        0x64 => reply(data, 0x65, 0x20, &[]),
        0x66 => return None,
        _ => {
            log(&format!(
                "   UNKNOWN command 0x{cmd:02X}: answered header-only as 0x{:02X}",
                cmd.wrapping_add(1)
            ));
            reply(data, cmd.wrapping_add(1), 0x20, &[])
        }
    })
}

/// Serves AiMeDB on `listener` (use `127.0.0.1:0` in tests) with the database at `db_path`. Every packet is logged
/// decrypted. Stops when the returned handle is dropped.
pub fn spawn(listener: TcpListener, db_path: PathBuf, log: Log) -> io::Result<Handle> {
    spawn_with(listener, db_path, log, false)
}

/// [`spawn`] for a run on a shared game server when `online`: every card answers its [`hash_id`].
pub fn spawn_with(
    listener: TcpListener,
    db_path: PathBuf,
    log: Log,
    online: bool,
) -> io::Result<Handle> {
    let mut db = Db::open(db_path)?;
    db.online = online;
    let db = Arc::new(db);
    let aes = Arc::new(Aes::new(KEY));
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    accept_loop(listener, stop, move |mut conn, peer| {
        log(&format!("connection from {peer}"));
        let mut buf: Vec<u8> = Vec::new();
        let mut last = Instant::now();
        // Fills `buf` to `n` bytes; false when the peer closed, the idle limit (30 s, as before) passed or we stop.
        let fill = |conn: &mut TcpStream,
                    last: &mut Instant,
                    buf: &mut Vec<u8>,
                    n: usize,
                    closed: &str|
         -> bool {
            let mut tmp = [0u8; 4096];
            while buf.len() < n {
                if last.elapsed() > Duration::from_secs(30) {
                    log("connection error: timed out");
                    return false;
                }
                match read_some(conn, &mut tmp, &flag) {
                    Ok(0) => {
                        if !closed.is_empty() {
                            log(closed);
                        }
                        return false;
                    }
                    Ok(k) => {
                        buf.extend_from_slice(&tmp[..k]);
                        *last = Instant::now();
                    }
                    Err(e) => {
                        log(&format!("connection error: {e}"));
                        return false;
                    }
                }
            }
            true
        };
        loop {
            if !fill(&mut conn, &mut last, &mut buf, 16, "closed by the game") {
                return;
            }
            let mut head: [u8; 16] = buf[..16].try_into().unwrap();
            aes.decrypt(&mut head);
            let length = u16::from_le_bytes([head[6], head[7]]) as usize;
            if u16::from_le_bytes([head[0], head[1]]) != MAGIC
                || length < 32
                || !length.is_multiple_of(16)
            {
                log(&format!(
                    "bad packet start {} (decrypted {}) - closing",
                    hex(&buf[..16]),
                    hex(&head)
                ));
                return;
            }
            if !fill(&mut conn, &mut last, &mut buf, length, "") {
                return;
            }
            let mut data: Vec<u8> = buf.drain(..length).collect();
            aes.decrypt(&mut data);
            let (cmd, ver) = (
                u16::from_le_bytes([data[4], data[5]]),
                u16::from_le_bytes([data[2], data[3]]),
            );
            log(&format!(
                "<- cmd 0x{ver_cmd:02X} ver 0x{ver:04X} len 0x{length:X} game {:?} store {} keychip {:?} body {}",
                String::from_utf8_lossy(&data[10..16]),
                u32::from_le_bytes(data[16..20].try_into().unwrap()),
                String::from_utf8_lossy(&data[20..32]),
                hex(&data[32..]),
                ver_cmd = cmd,
            ));
            let Some(mut out) = handle(&data, &db, &log) else {
                log("   goodbye");
                return;
            };
            log(&format!(
                "-> cmd 0x{:02X} len 0x{:X}",
                u16::from_le_bytes([out[4], out[5]]),
                out.len()
            ));
            aes.encrypt(&mut out);
            if conn.write_all(&out).is_err() {
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIPS-197 C.1 known answer (port of `_aimedb.py`'s selftest).
    #[test]
    fn aes_known_answer() {
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let pt: [u8; 16] = std::array::from_fn(|i| (i as u8) * 0x11);
        let mut b = pt;
        Aes::new(&key).encrypt(&mut b);
        assert_eq!(hex(&b), "69c4e0d86a7b0430d8cdb78070b4c55a");
        Aes::new(&key).decrypt(&mut b);
        assert_eq!(b, pt);
    }

    #[test]
    fn aime_id_is_stable_per_card() {
        let dir = std::env::temp_dir().join(format!("aimedb_id_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = ("11112222333344445555", "11112222333344445556");
        let id = |f: &str, c: &str| Db::open(dir.join(f)).unwrap().aime_id(c);
        assert_eq!(id("one.json", a), id("two.json", a));
        assert_ne!(id("one.json", a), id("one.json", b));
        assert!(id("one.json", a) > 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 31 bits, never 0, pinned, distinct for distinct codes (10,000 sequential codes, as a run of new cards).
    #[test]
    fn hash_id_is_31_bit_stable_and_distinct() {
        // pinned: a changed hash would give every card a new ID and orphan its club on a shared server
        assert_eq!(hash_id("50000000000000000001"), 1_510_114_823);
        let ids: std::collections::BTreeSet<i32> = (0..10_000u64)
            .map(|n| hash_id(&format!("5{n:019}")))
            .collect();
        assert_eq!(ids.len(), 10_000);
        assert!(ids.iter().all(|&i| i > 0), "positive: below 2^31");
    }

    /// A store from before the hash (the Python's ID 1): this PC keeps 1, so its club still loads; online the card
    /// answers its hash, so two homes' first cards never share a club on one server. The file keeps 1 either way.
    #[test]
    fn legacy_id_1_stays_here_and_online_answers_the_hash() {
        let dir = std::env::temp_dir().join(format!("aimedb_legacy_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (path, code) = (dir.join("aimedb.json"), "50123456789012345678");
        std::fs::write(&path, format!("{{\"{code}\": 1}}")).unwrap();
        assert_eq!(Db::open(path.clone()).unwrap().aime_id(code), 1);
        let mut online = Db::open(path.clone()).unwrap();
        online.online = true;
        assert_eq!(online.aime_id(code), hash_id(code));
        assert_ne!(hash_id(code), 1);
        assert_eq!(
            Db::open(path).unwrap().aime_id(code),
            1,
            "the store is not rewritten"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
