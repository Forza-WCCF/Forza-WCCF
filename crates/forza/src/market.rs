//! Create-a-Team's market requests (PLAN.md 3.1, 3.4): the installation's key `data\market.key` and the signed desk
//! words `MARKET JOIN`, `MARKET LEAVE` and `MARKET RESET`. The Launcher builds and sends them; the server
//! (`forza-server`, `crates/server/src/market.rs`) reads them with [`Request::open`], so the format lives here once.
//!
//! A request is one line and a body: `MARKET VERB size sig` + `size` bytes. The body's first line is
//! `VERB key at rules [name]` (the public key in hex, `at` in Unix milliseconds, the economy table's `RULES`, and on
//! JOIN the club name, cleaned as arcade names are), then on JOIN the grant log `grants.jsonl` as it is on disk. `sig`
//! is the ed25519 signature of the whole body in hex. Answers: `OK ...` or `NO reason`, one line.
//!
//! The key is made on first use and lives outside the profile folder, so a profile reset keeps it (PLAN.md 3.6).

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// The key's file in the Launcher's `data` folder, beside `vslink.key`.
pub const KEY_FILE: &str = "market.key";
/// The biggest body a desk reads: the grant log of years of play (a line is about 200 bytes) and its header line.
pub const MAX_BODY: usize = 2 << 20;
/// The body's first line at most: the words and a 24-character name in 4-byte characters.
pub const MAX_HEAD: usize = 256;
/// What every `MARKET` word answers on a desk without the market's value table.
pub const NOT_SET_UP: &str = "the market is not set up on this server";

/// What a request asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    /// Opt in, or send the grant log again: the server checks it and keeps it.
    Join,
    /// Leave the market: the server deletes the name, the log and the ledger rows.
    Leave,
    /// A profile reset: the server starts a new history for the same key.
    Reset,
}

impl Verb {
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Join => "JOIN",
            Self::Leave => "LEAVE",
            Self::Reset => "RESET",
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        [Self::Join, Self::Leave, Self::Reset]
            .into_iter()
            .find(|v| v.word() == word)
    }
}

/// One market request, as signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub verb: Verb,
    /// The installation's public key.
    pub key: [u8; 32],
    /// When the Launcher made it, Unix milliseconds: the server takes each key's requests in rising order only.
    pub at: u64,
    /// The Launcher's economy table ([`crate::collection::RULES`]).
    pub rules: u32,
    /// JOIN: the club name, cleaned ([`crate::online::lobby::arcade_name`]); else empty.
    pub name: String,
    /// JOIN: `grants.jsonl` as it is; else empty.
    pub log: String,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let mut out = [0u8; N];
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    for (o, pair) in out.iter_mut().zip(s.as_bytes().chunks(2)) {
        *o = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

impl Request {
    /// The bytes to send: the request line and the body, signed with `key` (whose public half the body names).
    #[must_use]
    pub fn signed(&self, key: &SigningKey) -> Vec<u8> {
        let name = crate::online::lobby::arcade_name(&self.name);
        let mut body = format!(
            "{} {} {} {}",
            self.verb.word(),
            hex(key.verifying_key().as_bytes()),
            self.at,
            self.rules
        );
        if !name.is_empty() {
            body.push(' ');
            body.push_str(&name);
        }
        body.push('\n');
        body.push_str(&self.log);
        let sig = key.sign(body.as_bytes());
        let mut out = format!(
            "MARKET {} {} {}\n",
            self.verb.word(),
            body.len(),
            hex(&sig.to_bytes())
        )
        .into_bytes();
        out.extend_from_slice(body.as_bytes());
        out
    }

    /// The request line's words after `MARKET`: the verb, the body's size (at most [`MAX_BODY`]) and the signature.
    /// Read before the body, so a bad line is refused unread.
    pub fn head(words: &[&str]) -> Result<(Verb, usize, [u8; 64]), String> {
        let [verb, size, sig] = words else {
            return Err("say MARKET JOIN|LEAVE|RESET size signature".into());
        };
        let verb = Verb::parse(verb).ok_or("an unknown market word")?;
        let size = size
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_BODY).contains(n))
            .ok_or_else(|| format!("a request of 1 to {MAX_BODY} bytes"))?;
        let sig = unhex::<64>(sig).ok_or("the signature is not 128 hex digits")?;
        Ok((verb, size, sig))
    }

    /// The body of a request whose line said `verb` and `sig`: the signature checked against the key the body names,
    /// the words parsed and the name cleaned again (the PC is not trusted to have done it).
    pub fn open(verb: Verb, sig: &[u8; 64], body: &[u8]) -> Result<Self, String> {
        let end = body
            .iter()
            .take(MAX_HEAD)
            .position(|&b| b == b'\n')
            .ok_or("the request's first line is missing or too long")?;
        let (head, rest) = body.split_at(end);
        let head =
            std::str::from_utf8(head).map_err(|_| "the request's first line is not UTF-8")?;
        let words: Vec<&str> = head.split_whitespace().collect();
        let [word, key, at, rules, name @ ..] = words.as_slice() else {
            return Err("the request's first line is short".into());
        };
        if Verb::parse(word) != Some(verb) {
            return Err("the request's line and body name different words".into());
        }
        let key = unhex::<32>(key).ok_or("the key is not 64 hex digits")?;
        let public = VerifyingKey::from_bytes(&key)
            .ok()
            .filter(|k| !k.is_weak())
            .ok_or("not an ed25519 key")?;
        public
            .verify_strict(body, &Signature::from_bytes(sig))
            .map_err(|_| "the signature does not match")?;
        let at = at.parse().map_err(|_| "the time is not a number")?;
        let rules = rules.parse().map_err(|_| "the rules are not a number")?;
        let log = std::str::from_utf8(rest.get(1..).unwrap_or_default())
            .map_err(|_| "the grant log is not UTF-8")?;
        Ok(Self {
            verb,
            key,
            at,
            rules,
            name: crate::online::lobby::arcade_name(&name.join(" ")),
            log: log.to_owned(),
        })
    }
}

/// The installation's market key from `path` (`data\market.key`), made and saved there on first use.
pub fn load_key(path: &Path) -> io::Result<SigningKey> {
    match std::fs::read(path) {
        Ok(b) => <[u8; 32]>::try_from(b)
            .map(|b| SigningKey::from_bytes(&b))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: not 32 bytes", path.display()),
                )
            }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut b = [0u8; 32];
            getrandom::fill(&mut b).map_err(|e| io::Error::other(e.to_string()))?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, b)?;
            Ok(SigningKey::from_bytes(&b))
        }
        Err(e) => Err(e),
    }
}

/// Sends `request` ([`Request::signed`]) to the desk at `desk`: `Ok` with the words after `OK`, `Err` with the reason
/// after `NO` (or what went wrong on the way).
pub fn send(desk: SocketAddr, request: &[u8]) -> Result<String, String> {
    let mut c =
        TcpStream::connect_timeout(&desk, Duration::from_secs(6)).map_err(|e| e.to_string())?;
    c.set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    c.write_all(request).map_err(|e| e.to_string())?;
    let mut got = Vec::new();
    c.take(1024)
        .read_to_end(&mut got)
        .map_err(|e| e.to_string())?;
    let got = String::from_utf8_lossy(&got);
    let line = got.lines().next().unwrap_or_default();
    match line.split_once(' ') {
        Some(("OK", rest)) => Ok(rest.to_owned()),
        Some(("NO", why)) => Err(why.to_owned()),
        _ if line == "OK" => Ok(String::new()),
        _ => Err(format!("the desk answered {line:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn join() -> Request {
        Request {
            verb: Verb::Join,
            key: *key(7).verifying_key().as_bytes(),
            at: 1_762_000_000_000,
            rules: 1,
            name: "  Real\u{202E} Club\tTokyo ".into(),
            log: "{\"seq\":1}\n{\"seq\":2}\n".into(),
        }
    }

    /// Splits sent bytes into the line's words after `MARKET` and the body.
    fn split(sent: &[u8]) -> (Vec<String>, Vec<u8>) {
        let nl = sent.iter().position(|&b| b == b'\n').unwrap();
        let line = std::str::from_utf8(&sent[..nl]).unwrap();
        let words = line.split(' ').skip(1).map(String::from).collect();
        (words, sent[nl + 1..].to_vec())
    }

    fn open(sent: &[u8]) -> Result<Request, String> {
        let (words, body) = split(sent);
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let (verb, size, sig) = Request::head(&words)?;
        assert_eq!(size, body.len());
        Request::open(verb, &sig, &body)
    }

    /// Acceptance 3.1 (a): a signed log verifies, and fails with any one byte of the body changed.
    #[test]
    fn a_signed_log_verifies_and_fails_with_one_byte_changed() {
        let sent = join().signed(&key(7));
        let got = open(&sent).unwrap();
        assert_eq!(got.log, join().log);
        assert_eq!(got.name, "Real Club Tokyo", "cleaned as arcade names are");
        assert_eq!(got.key, join().key);
        let body_at = sent.iter().position(|&b| b == b'\n').unwrap() + 1;
        for i in body_at..sent.len() {
            let mut bad = sent.clone();
            bad[i] ^= 0x01;
            assert!(open(&bad).is_err(), "byte {i} changed and still verified");
        }
    }

    #[test]
    fn another_key_or_another_word_is_refused() {
        // signed by key 8, naming key 7's public half
        let mut sent = join().signed(&key(8));
        let k8 = hex(key(8).verifying_key().as_bytes());
        let k7 = hex(key(7).verifying_key().as_bytes());
        let s = String::from_utf8(sent.clone())
            .unwrap()
            .replacen(&k8, &k7, 1);
        sent = s.into_bytes();
        assert_eq!(open(&sent).unwrap_err(), "the signature does not match");
        // a LEAVE line over a JOIN body
        let sent = join().signed(&key(7));
        let s = String::from_utf8(sent)
            .unwrap()
            .replacen("MARKET JOIN", "MARKET LEAVE", 1);
        assert!(open(s.as_bytes()).unwrap_err().contains("different words"));
    }

    #[test]
    fn bad_lines_are_refused_before_the_body() {
        let sig = "ab".repeat(64);
        for words in [
            vec!["JOIN"],
            vec!["SELL", "10", sig.as_str()],
            vec!["JOIN", "0", sig.as_str()],
            vec!["JOIN", "99999999", sig.as_str()],
            vec!["JOIN", "-1", sig.as_str()],
            vec!["JOIN", "10", "zz"],
        ] {
            assert!(Request::head(&words).is_err(), "{words:?}");
        }
        // a body without a first line, or with a short one
        let s = [0u8; 64];
        assert!(Request::open(Verb::Join, &s, &[b'x'; 300]).is_err());
        assert!(Request::open(Verb::Join, &s, b"JOIN abc\n").is_err());
        assert!(Request::open(Verb::Join, &s, b"\xff\xfe\n").is_err());
    }

    #[test]
    fn the_key_is_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join(KEY_FILE);
        let a = load_key(&path).unwrap();
        let b = load_key(&path).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes());
        std::fs::write(&path, b"short").unwrap();
        assert!(load_key(&path).is_err());
    }
}
