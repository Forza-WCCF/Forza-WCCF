//! A guest's own club in a 2017-18 arcade (decided 2026-10-27). Only the host's control talks to a game server,
//! so on JOIN the guest's Launcher sends its home club (every row of its card's Aime id in `data\save\gamesvr`) to
//! the host's seat desk, which imports it under the guest's arcade Aime id with fresh coach and card ids. The guest
//! pulls it back every [`POLL`] (so within seconds of each saved match) and on leave, and writes it home with its own
//! ids restored. A card whose home club sits under an older integer Aime id is read and written there: the arcade
//! only ever sees the hash.
//!
//! Desk words (TCP 20030, through the arcade tunnel or on the LAN), one request per connection:
//! `CLUB PUT seat aime token size` + size bytes of [`Club`] JSON -> `OK size` + [`Link`] JSON | `NO why`;
//! `CLUB GET aime token stamp` -> `SAME` | `OK size stamp` + [`Club`] JSON | `NO why`;
//! `CLUB DONE aime token` -> `OK 0` | `NO why`.
//! The host is a trust boundary: sizes and every record are checked, the first PUT for an Aime id binds its
//! [`token`] (a hash of the card's access code, which never leaves the guest), other tokens are refused, and an Aime
//! id that holds a club the desk did not import (the host's own, or one from before) is never read, replaced or
//! dropped.
//!
//! The guest's side keeps `club_out.json` (the marker: in which arcade since when, the id map, the home club's stamp)
//! beside its store while the club is out, and `club_last.json`, the last copy pulled. A marker found at the next
//! start means the session did not end cleanly: the home copy stays, the last pulled copy goes to `backups`, and the
//! player is told ([`leftover`]).

use super::gamesvr::{BLOCK, Store, TAIL, get_bits, next_id, now, put, put_bits, u32_at};
use crate::service::{Handle, Log};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// The biggest club a desk takes: 10 clubs of about 35 KB in hex, with room for purchases.
pub const MAX_BYTES: usize = 2 << 20;
/// How often the guest asks for its club while the run lasts.
pub const POLL: Duration = Duration::from_secs(10);
/// Guest clubs a host holds at once (one per seat): a guest cannot fill the host's store.
const MAX_GUESTS: usize = 8;
/// Backups of each kind kept in `backups` (oldest go first).
const KEEP_BACKUPS: usize = 30;
/// A stored request the copy carries: SAVE_DATA_SET 277, whole.
const SAVE_SET: u16 = 277;
const SAVE_SET_LEN: usize = 8572;
/// The ids inside a save block, 32 bits each (docs/research/ONLINE-2017-18.md, SYSTEM group): NETWORK_CARD_ID 185,
/// CARD_ID 263, NETWORK_COACH_ID 217. Stored clubs hold the cardId at both card fields and the coachId at 217.
const CARD_BITS: [usize; 2] = [185, 263];
const COACH_BIT: usize = 217;

/// Every row of one Aime id in `gamesvr.db`, as one value.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Club {
    pub aime: u32,
    pub coaches: Vec<Coach>,
    pub purchases: Vec<Purchase>,
}

/// A `coaches` row with its `saves`, `injustice` and `requests` rows.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Coach {
    pub coach: u32,
    pub card: u32,
    #[serde(with = "hex")]
    pub name1: Vec<u8>,
    #[serde(with = "hex")]
    pub name2: Vec<u8>,
    pub rec122: u8,
    pub play_count: u8,
    pub titles: u32,
    pub created: i64,
    pub updated: i64,
    pub save: Option<Save>,
    pub injustice: Option<u32>,
    pub requests: Vec<Request>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Save {
    #[serde(with = "hex")]
    pub block: Vec<u8>,
    pub version: u16,
    pub size: u16,
    pub kind: u8,
    pub updated: i64,
    #[serde(with = "hex")]
    pub tail: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub cmd: u16,
    #[serde(with = "hex")]
    pub payload: Vec<u8>,
    pub updated: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Purchase {
    pub at: i64,
    #[serde(with = "hex")]
    pub payload: Vec<u8>,
}

/// Bytes as lowercase hex in JSON.
mod hex {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::fmt::Write;

    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let mut t = String::with_capacity(b.len() * 2);
        for x in b {
            let _ = write!(t, "{x:02x}");
        }
        s.serialize_str(&t)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        if s.len() % 2 != 0 || !s.is_ascii() {
            return Err(D::Error::custom("not hex"));
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(D::Error::custom))
            .collect()
    }
}

/// The ids a copy swapped: (home, host) pairs. The guest keeps it while its club is out.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Link {
    pub coaches: Vec<(u32, u32)>,
    pub cards: Vec<(u32, u32)>,
}

/// Old id -> new id, per kind; a value not listed stays.
#[derive(Clone, Debug, Default)]
pub struct Swap {
    aimes: HashMap<u32, u32>,
    coaches: HashMap<u32, u32>,
    cards: HashMap<u32, u32>,
}

impl Swap {
    /// Home -> host ids.
    pub fn out(link: &Link, home_aime: u32, arcade_aime: u32) -> Swap {
        Swap {
            aimes: HashMap::from([(home_aime, arcade_aime)]),
            coaches: link.coaches.iter().copied().collect(),
            cards: link.cards.iter().copied().collect(),
        }
    }

    pub fn inverse(&self) -> Swap {
        let flip = |m: &HashMap<u32, u32>| m.iter().map(|(a, b)| (*b, *a)).collect();
        Swap {
            aimes: flip(&self.aimes),
            coaches: flip(&self.coaches),
            cards: flip(&self.cards),
        }
    }
}

fn swap_u32(b: &mut [u8], at: usize, ids: &HashMap<u32, u32>) {
    if let Some(&n) = ids.get(&u32_at(b, at)) {
        put(b, at, n);
    }
}

fn swap_bits(block: &mut [u8], bit: usize, ids: &HashMap<u32, u32>) {
    if let Some(&n) = ids.get(&get_bits(block, bit, 32)) {
        put_bits(block, bit, 32, n);
    }
}

fn swap_block(block: &mut [u8], s: &Swap) {
    for bit in CARD_BITS {
        swap_bits(block, bit, &s.cards);
    }
    swap_bits(block, COACH_BIT, &s.coaches);
}

impl Club {
    /// The club with its ids swapped: the Aime id, each coach's coach and card ids, the ids inside the save blocks,
    /// and in each stored 277 +4 Aime id, +8 coachId, +12 cardId, +28 its block, +3,224 the old cardId (a club moved
    /// to a new card); a purchase's +4 Aime id. Nothing else is touched (the 277's sums are not read back by the
    /// store, so they stay as the client wrote them).
    pub fn remap(&self, s: &Swap) -> Club {
        let mut c = self.clone();
        let one = |m: &HashMap<u32, u32>, v: u32| *m.get(&v).unwrap_or(&v);
        c.aime = one(&s.aimes, c.aime);
        for k in &mut c.coaches {
            k.coach = one(&s.coaches, k.coach);
            k.card = one(&s.cards, k.card);
            if let Some(save) = &mut k.save {
                swap_block(&mut save.block, s);
            }
            for r in k.requests.iter_mut().filter(|r| r.cmd == SAVE_SET) {
                let p = &mut r.payload;
                if p.len() != SAVE_SET_LEN {
                    continue;
                }
                swap_u32(p, 4, &s.aimes);
                swap_u32(p, 8, &s.coaches);
                swap_u32(p, 12, &s.cards);
                swap_u32(p, 3224, &s.cards);
                swap_block(&mut p[28..28 + BLOCK], s);
            }
        }
        for p in c.purchases.iter_mut().filter(|p| p.payload.len() >= 8) {
            swap_u32(&mut p.payload, 4, &s.aimes);
        }
        c
    }

    /// What a host takes from a guest; `Err` names the first thing wrong.
    pub fn check(&self) -> Result<(), String> {
        if self.aime == 0 {
            return Err("Aime id 0".into());
        }
        if self.coaches.len() > 32 || self.purchases.len() > 1000 {
            return Err("too many records".into());
        }
        let mut ids = std::collections::HashSet::new();
        for k in &self.coaches {
            if k.coach == 0 || k.card == 0 || !ids.insert((0, k.coach)) || !ids.insert((1, k.card))
            {
                return Err(format!(
                    "coach {} card {}: a zero or repeated id",
                    k.coach, k.card
                ));
            }
            if k.name1.len() > 56 || k.name2.len() > 56 {
                return Err(format!("coach {}: a name over 56 bytes", k.coach));
            }
            if let Some(s) = &k.save
                && (s.block.len() != BLOCK || ![0, SAVE_SET_LEN - TAIL].contains(&s.tail.len()))
            {
                return Err(format!("coach {}: a save of the wrong size", k.coach));
            }
            if k.requests.len() > 4 {
                return Err(format!("coach {}: too many requests", k.coach));
            }
            for r in &k.requests {
                if r.cmd != SAVE_SET || r.payload.len() != SAVE_SET_LEN {
                    return Err(format!(
                        "coach {}: request {} of {} bytes",
                        k.coach,
                        r.cmd,
                        r.payload.len()
                    ));
                }
                // a record keyed to another card
                if ![0, self.aime].contains(&u32_at(&r.payload, 4)) {
                    return Err(format!("coach {}: a save of another Aime id", k.coach));
                }
            }
        }
        for p in &self.purchases {
            if !(16..=64).contains(&p.payload.len()) {
                return Err(format!("a purchase of {} bytes", p.payload.len()));
            }
            if u32_at(&p.payload, 4) != self.aime {
                return Err("a purchase of another Aime id".into());
            }
        }
        Ok(())
    }

    /// A short hash of the club's JSON: equal stamps, equal clubs.
    pub fn stamp(&self) -> String {
        stamp_of(&serde_json::to_vec(self).unwrap_or_default())
    }
}

fn stamp_of(json: &[u8]) -> String {
    Sha256::digest(json)[..12]
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// The token that owns a card's club in an arcade: SHA-256 of the access code with a fixed prefix, so the host never
/// learns the code (which would let it play as that card elsewhere).
pub fn token(code: &str) -> String {
    let h = Sha256::digest(format!("forza-wccf club\0{code}").as_bytes());
    h.iter().map(|x| format!("{x:02x}")).collect()
}

fn db_err(e: rusqlite::Error) -> String {
    format!("store: {e}")
}

/// Every row of `aime`, coaches by id, purchases in the order made.
pub fn export(db: &Connection, aime: u32) -> rusqlite::Result<Club> {
    let mut coaches: Vec<Coach> = db
        .prepare(
            "SELECT coach_id, card_id, name1, name2, rec122, play_count, titles, created, updated FROM coaches
             WHERE aime_id = ?1 ORDER BY coach_id",
        )?
        .query_map([aime], |r| {
            Ok(Coach {
                coach: r.get(0)?,
                card: r.get(1)?,
                name1: r.get(2)?,
                name2: r.get(3)?,
                rec122: r.get(4)?,
                play_count: r.get(5)?,
                titles: r.get(6)?,
                created: r.get(7)?,
                updated: r.get(8)?,
                save: None,
                injustice: None,
                requests: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    for k in &mut coaches {
        let key = params![aime, k.coach];
        k.save = db
            .query_row(
                "SELECT block, version, size, kind, updated, tail FROM saves WHERE aime_id = ?1 AND coach_id = ?2",
                key,
                |r| {
                    Ok(Save {
                        block: r.get(0)?,
                        version: r.get(1)?,
                        size: r.get(2)?,
                        kind: r.get(3)?,
                        updated: r.get(4)?,
                        tail: r.get(5)?,
                    })
                },
            )
            .optional()?;
        k.injustice = db
            .query_row(
                "SELECT count FROM injustice WHERE aime_id = ?1 AND coach_id = ?2",
                key,
                |r| r.get(0),
            )
            .optional()?;
        k.requests = db
            .prepare("SELECT cmd, payload, updated FROM requests WHERE aime_id = ?1 AND coach_id = ?2 ORDER BY cmd")?
            .query_map(key, |r| {
                Ok(Request {
                    cmd: r.get(0)?,
                    payload: r.get(1)?,
                    updated: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
    }
    let purchases = db
        .prepare("SELECT at, payload FROM purchases WHERE aime_id = ?1 ORDER BY rowid")?
        .query_map([aime], |r| {
            Ok(Purchase {
                at: r.get(0)?,
                payload: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Club {
        aime,
        coaches,
        purchases,
    })
}

/// Deletes every row of `aime` (the caller's transaction).
fn delete(db: &Connection, aime: u32) -> rusqlite::Result<()> {
    // saves refer to coaches (foreign keys are on in the bundled SQLite): coaches go last
    for t in ["saves", "injustice", "requests", "purchases", "coaches"] {
        db.execute(&format!("DELETE FROM {t} WHERE aime_id = ?1"), [aime])?;
    }
    Ok(())
}

/// Writes every row of `c` under `c.aime` (the caller's transaction, after [`delete`]).
fn insert(db: &Connection, c: &Club) -> rusqlite::Result<()> {
    for k in &c.coaches {
        db.execute(
            "INSERT INTO coaches (aime_id, coach_id, card_id, name1, name2, rec122, play_count, titles, created, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![c.aime, k.coach, k.card, k.name1, k.name2, k.rec122, k.play_count, k.titles, k.created, k.updated],
        )?;
        if let Some(s) = &k.save {
            db.execute(
                "INSERT INTO saves VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    c.aime, k.coach, s.block, s.version, s.size, s.kind, s.updated, s.tail
                ],
            )?;
        }
        if let Some(n) = k.injustice {
            db.execute(
                "INSERT INTO injustice VALUES (?1, ?2, ?3)",
                params![c.aime, k.coach, n],
            )?;
        }
        for r in &k.requests {
            db.execute(
                "INSERT INTO requests VALUES (?1, ?2, ?3, ?4, ?5)",
                params![c.aime, k.coach, r.cmd, r.payload, r.updated],
            )?;
        }
    }
    for p in &c.purchases {
        db.execute(
            "INSERT INTO purchases (aime_id, at, payload) VALUES (?1, ?2, ?3)",
            params![c.aime, p.at, p.payload],
        )?;
    }
    Ok(())
}

/// The token that owns `aime`'s imported club, if the desk imported one.
fn guest_token(db: &Connection, aime: u32) -> rusqlite::Result<Option<String>> {
    db.query_row("SELECT token FROM guests WHERE aime_id = ?1", [aime], |r| {
        r.get(0)
    })
    .optional()
}

/// Whether `aime` has any row at all.
fn has_rows(db: &Connection, aime: u32) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS (SELECT 1 FROM coaches WHERE aime_id = ?1) OR EXISTS (SELECT 1 FROM saves WHERE aime_id = ?1)
         OR EXISTS (SELECT 1 FROM requests WHERE aime_id = ?1) OR EXISTS (SELECT 1 FROM purchases WHERE aime_id = ?1)
         OR EXISTS (SELECT 1 FROM injustice WHERE aime_id = ?1)",
        [aime],
        |r| r.get(0),
    )
}

/// The rows of `aime` the desk imported for the guest holding `token`; `Err` for any other Aime id or token.
fn owned(db: &Connection, aime: u32, token: &str) -> Result<(), String> {
    match guest_token(db, aime).map_err(db_err)? {
        Some(t) if t == token => Ok(()),
        Some(_) => Err("another player's club for this card is in this arcade".into()),
        None => Err("no club of this card was brought here".into()),
    }
}

impl Store {
    /// Host: imports a guest's home club under its arcade Aime id `aime` with fresh coach and card ids; the map goes
    /// back to the guest. A copy of the same token replaces the old one (a rejoin); any other club under `aime` is
    /// refused, never touched.
    pub fn import_guest(
        &mut self,
        club: &Club,
        aime: u32,
        seat: u8,
        token: &str,
    ) -> Result<Link, String> {
        club.check()?;
        let tx = self.0.transaction().map_err(db_err)?;
        match guest_token(&tx, aime).map_err(db_err)? {
            Some(t) if t != token => {
                return Err("another player's club for this card is in this arcade".into());
            }
            Some(_) => {}
            None if has_rows(&tx, aime).map_err(db_err)? => {
                return Err("this arcade already keeps a club for this card".into());
            }
            None => {
                let n: u32 = tx
                    .query_row("SELECT count(*) FROM guests", [], |r| r.get(0))
                    .map_err(db_err)?;
                if n as usize >= MAX_GUESTS {
                    return Err(format!(
                        "this arcade holds {MAX_GUESTS} guests' clubs already"
                    ));
                }
            }
        }
        delete(&tx, aime).map_err(db_err)?;
        let mut link = Link::default();
        for k in &club.coaches {
            link.coaches
                .push((k.coach, next_id(&tx, "coach").map_err(db_err)?));
            link.cards
                .push((k.card, next_id(&tx, "card").map_err(db_err)?));
        }
        let swap = Swap::out(&link, club.aime, aime);
        let host = club.remap(&swap);
        // the copy must come back byte for byte: refused when a field already held one of the new ids
        if host.remap(&swap.inverse()) != *club {
            return Err("an id field the copy cannot restore".into());
        }
        insert(&tx, &host).map_err(db_err)?;
        tx.execute(
            "INSERT OR REPLACE INTO guests VALUES (?1, ?2, ?3, ?4)",
            params![aime, token, seat, now()],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(link)
    }

    /// Host: the imported club of `aime`, for the guest holding `token`.
    pub fn guest_club(&self, aime: u32, token: &str) -> Result<Club, String> {
        // one snapshot: a save the stand-in commits meanwhile is in the copy whole or not at all
        let tx =
            rusqlite::Transaction::new_unchecked(&self.0, rusqlite::TransactionBehavior::Deferred)
                .map_err(db_err)?;
        owned(&tx, aime, token)?;
        export(&tx, aime).map_err(db_err)
    }

    /// Host: drops the imported club of `aime` once its guest has it home.
    pub fn drop_guest(&mut self, aime: u32, token: &str) -> Result<(), String> {
        let tx = self.0.transaction().map_err(db_err)?;
        owned(&tx, aime, token)?;
        delete(&tx, aime).map_err(db_err)?;
        tx.execute("DELETE FROM guests WHERE aime_id = ?1", [aime])
            .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    /// Host start: drops every guest club a crash left behind; returns how many.
    pub fn purge_guests(&mut self) -> rusqlite::Result<usize> {
        let tx = self.0.transaction()?;
        let aimes: Vec<u32> = tx
            .prepare("SELECT aime_id FROM guests")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for &a in &aimes {
            delete(&tx, a)?;
        }
        tx.execute("DELETE FROM guests", [])?;
        tx.commit()?;
        Ok(aimes.len())
    }

    /// Guest: every row of `aime` in the home store.
    pub fn club(&self, aime: u32) -> rusqlite::Result<Club> {
        export(&self.0, aime)
    }

    /// Guest: writes the host's copy `host` home under `home_aime` with the home ids (`link`, which gains fresh home
    /// ids for clubs or cards made in the arcade), in one transaction. `expect` is the home club's stamp at the last
    /// copy: a home club changed since is never overwritten (`Ok(None)`). Returns the new home stamp.
    pub fn write_home(
        &mut self,
        host: &Club,
        link: &mut Link,
        home_aime: u32,
        expect: &str,
    ) -> Result<Option<String>, String> {
        let tx = self.0.transaction().map_err(db_err)?;
        if export(&tx, home_aime).map_err(db_err)?.stamp() != expect {
            return Ok(None);
        }
        let mut next = link.clone();
        for k in &host.coaches {
            if !next.coaches.iter().any(|p| p.1 == k.coach) {
                next.coaches
                    .push((next_id(&tx, "coach").map_err(db_err)?, k.coach));
            }
            if !next.cards.iter().any(|p| p.1 == k.card) {
                next.cards
                    .push((next_id(&tx, "card").map_err(db_err)?, k.card));
            }
        }
        let home = host.remap(&Swap::out(&next, home_aime, host.aime).inverse());
        delete(&tx, home_aime).map_err(db_err)?;
        insert(&tx, &home).map_err(db_err)?;
        let stamp = export(&tx, home_aime).map_err(db_err)?.stamp();
        tx.commit().map_err(db_err)?;
        *link = next;
        Ok(Some(stamp))
    }
}

// ---- the host's desk ----

/// The words of one `CLUB` line.
#[derive(Debug, PartialEq, Eq)]
enum Ask {
    Put {
        seat: u8,
        aime: u32,
        token: String,
        size: usize,
    },
    Get {
        aime: u32,
        token: String,
        stamp: String,
    },
    Done {
        aime: u32,
        token: String,
    },
}

impl Ask {
    fn parse(line: &str) -> Option<Ask> {
        let aime = |a: &str| a.parse::<u32>().ok().filter(|a| *a != 0);
        let token = |t: &str| {
            (t.len() == 64
                && t.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            .then(|| t.to_owned())
        };
        match line.split_whitespace().collect::<Vec<_>>()[..] {
            ["CLUB", "PUT", seat, a, t, size] => Some(Ask::Put {
                seat: seat.parse().ok().filter(|s| super::SEATS.contains(s))?,
                aime: aime(a)?,
                token: token(t)?,
                size: size.parse().ok().filter(|n| (1..=MAX_BYTES).contains(n))?,
            }),
            ["CLUB", "GET", a, t, stamp] if stamp.len() <= 32 => Some(Ask::Get {
                aime: aime(a)?,
                token: token(t)?,
                stamp: stamp.to_owned(),
            }),
            ["CLUB", "DONE", a, t] => Some(Ask::Done {
                aime: aime(a)?,
                token: token(t)?,
            }),
            _ => None,
        }
    }
}

/// Host: answers one `CLUB` request. `head` is the line, `rest` what came after it, `held` says whether a seat is
/// leased on the desk now. Returns the line for the desk's log.
pub fn serve(
    db: &Path,
    head: &str,
    rest: Vec<u8>,
    conn: &mut TcpStream,
    held: &dyn Fn(u8) -> bool,
) -> io::Result<String> {
    fn no(conn: &mut TcpStream, why: &str) -> io::Result<String> {
        conn.write_all(format!("NO {why}\n").as_bytes())?;
        Ok(format!("club refused - {why}"))
    }
    let Some(ask) = Ask::parse(head) else {
        return no(conn, "a bad request");
    };
    let mut store = match Store::open(db) {
        Ok(s) => s,
        Err(e) => return no(conn, &format!("store: {e}")),
    };
    let (reply, body, said) = match ask {
        Ask::Put {
            seat,
            aime,
            token,
            size,
        } => {
            // read first: a refusal with the upload unread would reset the connection before the answer arrives
            let body = read_body(conn, rest, size)?;
            if !held(seat) {
                return no(conn, &format!("seat {seat} is not taken"));
            }
            let club: Club = match serde_json::from_slice(&body) {
                Ok(c) => c,
                Err(e) => return no(conn, &format!("not a club ({e})")),
            };
            match store.import_guest(&club, aime, seat, &token) {
                Ok(link) => {
                    let json = serde_json::to_vec(&link).unwrap_or_default();
                    let said = format!(
                        "club in: seat {seat}, Aime id {aime}, {} clubs",
                        club.coaches.len()
                    );
                    (format!("OK {}\n", json.len()), json, said)
                }
                Err(why) => return no(conn, &why),
            }
        }
        Ask::Get { aime, token, stamp } => match store.guest_club(aime, &token) {
            Ok(club) => {
                let json = serde_json::to_vec(&club).unwrap_or_default();
                let now = stamp_of(&json);
                if now == stamp {
                    ("SAME\n".to_owned(), Vec::new(), String::new())
                } else {
                    let said = format!("club out: Aime id {aime}, {} clubs", club.coaches.len());
                    (format!("OK {} {now}\n", json.len()), json, said)
                }
            }
            Err(why) => return no(conn, &why),
        },
        Ask::Done { aime, token } => match store.drop_guest(aime, &token) {
            Ok(()) => (
                "OK 0\n".to_owned(),
                Vec::new(),
                format!("club home: Aime id {aime}, dropped here"),
            ),
            Err(why) => return no(conn, &why),
        },
    };
    conn.write_all(reply.as_bytes())?;
    conn.write_all(&body)?;
    Ok(said)
}

/// Exactly `size` bytes: `rest` and then the connection, within 60 s.
fn read_body(conn: &mut TcpStream, mut data: Vec<u8>, size: usize) -> io::Result<Vec<u8>> {
    let end = Instant::now() + Duration::from_secs(60);
    conn.set_read_timeout(Some(Duration::from_secs(30)))?;
    data.truncate(size);
    while data.len() < size {
        if Instant::now() > end {
            return Err(io::Error::other("club upload too slow"));
        }
        let mut chunk = vec![0u8; (size - data.len()).min(65536)];
        match conn.read(&mut chunk)? {
            0 => return Err(io::Error::other("the PC hung up mid-club")),
            n => data.extend_from_slice(&chunk[..n]),
        }
    }
    Ok(data)
}

// ---- the guest ----

/// One request to the desk at `desk`: the reply's line and its body (`OK size ...` is followed by size bytes).
fn ask(desk: SocketAddr, head: &str, body: &[u8]) -> Result<(String, Vec<u8>), String> {
    let mut c =
        TcpStream::connect_timeout(&desk, Duration::from_secs(6)).map_err(|e| e.to_string())?;
    c.set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    c.write_all(head.as_bytes())
        .and_then(|()| c.write_all(body))
        .map_err(|e| e.to_string())?;
    let mut all = Vec::new();
    c.take((MAX_BYTES + 256) as u64)
        .read_to_end(&mut all)
        .map_err(|e| e.to_string())?;
    let at = all.iter().position(|&b| b == b'\n').ok_or("no answer")?;
    let line = String::from_utf8_lossy(&all[..at]).into_owned();
    let rest = all.split_off(at + 1);
    match line.split_whitespace().collect::<Vec<_>>()[..] {
        ["OK", n, ..] if n.parse() == Ok(rest.len()) => Ok((line, rest)),
        ["SAME"] => Ok((line, rest)),
        _ => Err(line.strip_prefix("NO ").unwrap_or(&line).to_owned()),
    }
}

/// Sends the home club; the host's id map comes back.
pub fn send(
    desk: SocketAddr,
    seat: u8,
    aime: u32,
    code: &str,
    club: &Club,
) -> Result<Link, String> {
    let json = serde_json::to_vec(club).map_err(|e| e.to_string())?;
    let head = format!("CLUB PUT {seat} {aime} {} {}\n", token(code), json.len());
    let (_, body) = ask(desk, &head, &json)?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

/// The host's copy and its stamp, or `None` when it is still `stamp`.
pub fn fetch(
    desk: SocketAddr,
    aime: u32,
    code: &str,
    stamp: &str,
) -> Result<Option<(String, Club)>, String> {
    let stamp = if stamp.is_empty() { "-" } else { stamp };
    let (line, body) = ask(
        desk,
        &format!("CLUB GET {aime} {} {stamp}\n", token(code)),
        &[],
    )?;
    match line.split_whitespace().collect::<Vec<_>>()[..] {
        ["OK", _, now] => Ok(Some((
            now.to_owned(),
            serde_json::from_slice(&body).map_err(|e| e.to_string())?,
        ))),
        _ => Ok(None),
    }
}

/// Tells the host the club is home; it drops its copy.
pub fn release(desk: SocketAddr, aime: u32, code: &str) -> Result<(), String> {
    ask(desk, &format!("CLUB DONE {aime} {}\n", token(code)), &[]).map(|_| ())
}

/// `club_out.json`: the club is out, in which arcade since when, and what copying it back needs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Out {
    #[serde(rename = "shop")] // on-disk key from before the arcade rename
    pub arcade: String,
    pub since: i64,
    pub seat: u8,
    pub home_aime: u32,
    #[serde(rename = "shop_aime")] // on-disk key from before the arcade rename
    pub arcade_aime: u32,
    pub link: Link,
    /// The home club's stamp after the last copy (in or back).
    pub home_stamp: String,
    /// The host's stamp of the copy last pulled, and of the one last written home.
    pub host_stamp: String,
    pub back_stamp: String,
    /// When the last copy-back was written home (0: none yet).
    pub back: i64,
}

fn out_file(dir: &Path) -> PathBuf {
    dir.join("club_out.json")
}

fn last_file(dir: &Path) -> PathBuf {
    dir.join("club_last.json")
}

/// Writes `bytes` to `path` through a temporary file and a rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

fn time_name() -> String {
    let t = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// Writes `bytes` to `dir\backups\<what>-<time>.json` and keeps the newest [`KEEP_BACKUPS`] of that kind there.
fn backup(dir: &Path, what: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let b = dir.join("backups");
    std::fs::create_dir_all(&b)?;
    let mut path = b.join(format!("{what}-{}.json", time_name()));
    for n in 2.. {
        if !path.exists() {
            break;
        }
        path = b.join(format!("{what}-{}-{n}.json", time_name()));
    }
    write_atomic(&path, bytes)?;
    let mut all: Vec<_> = std::fs::read_dir(&b)?
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.ends_with(".json") && name.strip_prefix(what).is_some_and(|r| r.starts_with('-'))
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    all.sort();
    for (_, old) in all.iter().rev().skip(KEEP_BACKUPS) {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

/// At a start: a club still marked out means the last arcade session ended without bringing it home. The home copy
/// stays; the last copy pulled from the host (if any) goes to `backups`, the marker goes. `Some(path)` (the backup,
/// or the store folder when no copy came) when the player must be told.
pub fn leftover(dir: &Path) -> io::Result<Option<PathBuf>> {
    let out = out_file(dir);
    if !out.exists() {
        return Ok(None);
    }
    let last = last_file(dir);
    let kept = match std::fs::read(&last) {
        // file prefix kept from before the arcade rename: players' backups already carry it
        Ok(b) => backup(dir, "club-shop", &b)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => dir.to_path_buf(),
        // unreadable now: keep the marker and the copy for the next start
        Err(e) => return Err(e),
    };
    let _ = std::fs::remove_file(&last);
    std::fs::remove_file(&out)?;
    Ok(Some(kept))
}

/// The club as one more step settles it: the club came home in effect (the home copy is the host's last), so the marker
/// goes as after a trip that ended at home. For a trip whose last copy-back found no host ([`Progress::gone`]) but
/// whose earlier copies already brought every saved match home.
pub fn settle(dir: &Path) -> io::Result<()> {
    let _ = std::fs::remove_file(last_file(dir));
    match std::fs::remove_file(out_file(dir)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// How a trip went, for the run's end: read after the [`Trip`] is dropped (its last copy-back done).
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    /// The last copy-back that left the home club matching the host's.
    pub last_ok: Option<Instant>,
    /// The last copy-back (at the end) brought it home.
    pub home: bool,
    /// The last two copy-backs found the host's PC up but its server gone (refused or reset): the host closed the
    /// arcade, not a desk that restarts.
    pub gone: bool,
    gone_once: bool,
}

/// A copy-back error that says the host's server is gone while its PC answers: refused or reset (Windows errors).
fn server_gone(e: &str) -> bool {
    e.contains("(os error 10061)") || e.contains("(os error 10054)")
}

/// The guest's card for an arcade.
pub struct Guest {
    /// The seat desk (through the tunnel: 127.0.0.1 and the tunnel's port).
    pub desk: SocketAddr,
    pub seat: u8,
    /// The card's access code (`data\save\seat1_aime.txt`).
    pub code: String,
    /// Its Aime id at home (`aimedb.json`; an older card keeps its integer id there) and in the arcade (the hash).
    pub home_aime: u32,
    pub arcade_aime: u32,
    /// The home store's folder, `data\save\gamesvr`.
    pub dir: PathBuf,
    /// Which arcade, for the marker.
    pub arcade: String,
    /// Between copy-backs ([`POLL`]; tests shorten it).
    pub poll: Duration,
}

/// The club out in an arcade: pulls it home every [`POLL`] and once more when dropped (the run's end), then tells the
/// host; the marker goes only after that last copy is home.
pub struct Trip {
    _handle: Handle,
    progress: Arc<Mutex<Progress>>,
}

impl Trip {
    /// How the trip goes; final once the trip is dropped.
    pub fn progress(&self) -> Arc<Mutex<Progress>> {
        self.progress.clone()
    }
}

impl Guest {
    /// JOIN: backs the home club up, sends it, marks it out, and starts the copy-back. `Err`: the club stays home
    /// (nothing marked), the player plays with what the arcade has for the card.
    pub fn bring(self, log: Log) -> Result<Trip, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let store = Store::open(&self.dir.join("gamesvr.db")).map_err(db_err)?;
        let club = store.club(self.home_aime).map_err(db_err)?;
        let json = serde_json::to_vec(&club).map_err(|e| e.to_string())?;
        let saved = backup(&self.dir, &format!("club-{}", self.home_aime), &json)
            .map_err(|e| e.to_string())?;
        log(&format!("home club backed up: {}", saved.display()));
        let link = send(self.desk, self.seat, self.arcade_aime, &self.code, &club)?;
        let out = Out {
            arcade: self.arcade.clone(),
            since: now(),
            seat: self.seat,
            home_aime: self.home_aime,
            arcade_aime: self.arcade_aime,
            link,
            home_stamp: club.stamp(),
            host_stamp: String::new(),
            back_stamp: String::new(),
            back: 0,
        };
        self.mark(&out).map_err(|e| e.to_string())?;
        log(&format!(
            "club in the arcade: {} clubs, Aime id {} at home, {} here",
            club.coaches.len(),
            self.home_aime,
            self.arcade_aime
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let progress = Arc::new(Mutex::new(Progress::default()));
        let p = progress.clone();
        let thread = thread::Builder::new()
            .name("club-trip".into())
            .spawn(move || self.trip(out, store, &flag, &log, &p))
            .map_err(|e| e.to_string())?;
        Ok(Trip {
            _handle: Handle::new(stop, thread),
            progress,
        })
    }

    fn mark(&self, out: &Out) -> io::Result<()> {
        write_atomic(
            &out_file(&self.dir),
            &serde_json::to_vec_pretty(out).map_err(io::Error::other)?,
        )
    }

    fn trip(
        self,
        mut out: Out,
        mut store: Store,
        stop: &AtomicBool,
        log: &Log,
        progress: &Mutex<Progress>,
    ) {
        loop {
            let until = Instant::now() + self.poll;
            while Instant::now() < until && !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
            }
            let last = stop.load(Ordering::Relaxed);
            let home = self.pull(&mut out, &mut store, last, log, progress);
            let mut p = progress.lock().unwrap_or_else(PoisonError::into_inner);
            if home {
                p.last_ok = Some(Instant::now());
            }
            if last {
                p.home = home;
                if home {
                    match release(self.desk, self.arcade_aime, &self.code) {
                        Ok(()) => log("club home; the arcade dropped its copy"),
                        Err(e) => log(&format!(
                            "club home; the arcade keeps its copy until it restarts ({e})"
                        )),
                    }
                    let _ = std::fs::remove_file(last_file(&self.dir));
                    let _ = std::fs::remove_file(out_file(&self.dir));
                } else {
                    let ago = p.last_ok.map_or("none".into(), |t| {
                        format!("{} s ago", t.elapsed().as_secs())
                    });
                    log(&format!(
                        "club not brought home at the end (last good copy-back: {ago}): the marker stays unless the \
                         Launcher finds the club safe, else the next start keeps the home copy"
                    ));
                }
                return;
            }
        }
    }

    /// One copy-back; true when the home club now matches the host's (or nothing changed). `all`: ask even when
    /// the stamp is the same (the last one).
    fn pull(
        &self,
        out: &mut Out,
        store: &mut Store,
        all: bool,
        log: &Log,
        progress: &Mutex<Progress>,
    ) -> bool {
        let since = if all { "" } else { out.host_stamp.as_str() };
        let got = fetch(self.desk, self.arcade_aime, &self.code, since);
        {
            let mut p = progress.lock().unwrap_or_else(PoisonError::into_inner);
            let now = got.as_ref().is_err_and(|e| server_gone(e));
            p.gone = now && p.gone_once;
            p.gone_once = now;
        }
        let (stamp, host) = match got {
            Ok(Some(got)) => got,
            Ok(None) => return true,
            Err(e) => {
                log(&format!("club copy-back failed: {e}"));
                return false;
            }
        };
        if stamp == out.back_stamp {
            return true;
        }
        let pulled = serde_json::to_vec(&(&out.link, &host)).unwrap_or_default();
        if let Err(e) = write_atomic(&last_file(&self.dir), &pulled) {
            log(&format!(
                "club copy-back: cannot keep the pulled copy ({e})"
            ));
        }
        // the stamp moves only once this copy is home or set aside, so a failed write is tried again next poll
        match store.write_home(&host, &mut out.link, self.home_aime, &out.home_stamp) {
            Ok(Some(home)) => {
                out.home_stamp = home;
                out.host_stamp = stamp.clone();
                out.back_stamp = stamp;
                out.back = now();
                if let Err(e) = self.mark(out) {
                    log(&format!("club copy-back: marker not updated ({e})"));
                }
                log(&format!("club copied home: {} clubs", host.coaches.len()));
                true
            }
            Ok(None) => {
                out.host_stamp = stamp;
                let _ = self.mark(out);
                // same kept "club-shop" prefix as above
                match backup(&self.dir, "club-shop", &pulled) {
                    Ok(p) => log(&format!(
                        "club copy-back: the home club changed meanwhile, so it stays; the arcade's copy is {}",
                        p.display()
                    )),
                    Err(e) => log(&format!(
                        "club copy-back: the home club changed meanwhile; backup failed ({e})"
                    )),
                }
                false
            }
            Err(e) => {
                log(&format!("club copy-back failed: {e}"));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `club_out.json` written before the arcade rename still loads, and the keys on disk stay the same.
    #[test]
    fn club_out_keeps_its_old_keys() {
        let old = r#"{"shop":"Sala Roma","since":1,"seat":2,"home_aime":3,"shop_aime":4,
            "link":{"coaches":[],"cards":[]},"home_stamp":"a","host_stamp":"b","back_stamp":"c","back":0}"#;
        let out: Out = serde_json::from_str(old).unwrap();
        assert_eq!((out.arcade.as_str(), out.arcade_aime), ("Sala Roma", 4));
        let json = serde_json::to_value(&out).unwrap();
        assert!(
            json.get("shop").is_some() && json.get("shop_aime").is_some(),
            "{json}"
        );
    }

    #[test]
    fn ask_lines() {
        let t = "ab".repeat(32);
        assert_eq!(
            Ask::parse(&format!("CLUB PUT 2 952833657 {t} 100")),
            Some(Ask::Put {
                seat: 2,
                aime: 952833657,
                token: t.clone(),
                size: 100
            })
        );
        assert_eq!(
            Ask::parse(&format!("CLUB GET 7 {t} -")),
            Some(Ask::Get {
                aime: 7,
                token: t.clone(),
                stamp: "-".into()
            })
        );
        for bad in [
            format!("CLUB PUT 9 5 {t} 100"),
            format!("CLUB PUT 2 0 {t} 100"),
            format!("CLUB PUT 2 5 {t} 0"),
            format!("CLUB PUT 2 5 {t} {}", MAX_BYTES + 1),
            format!("CLUB PUT 2 5 {} 100", "AB".repeat(32)),
            format!("CLUB PUT 2 5 {} 100", "ab".repeat(31)),
            "CLUB DONE 5".into(),
            "CLUB".into(),
        ] {
            assert_eq!(Ask::parse(&bad), None, "{bad}");
        }
    }

    use std::net::TcpListener;
    use std::sync::Mutex;

    const CODE: &str = "50123456789012345678";

    fn sum32(b: &[u8]) -> u32 {
        b.as_chunks::<4>()
            .0
            .iter()
            .fold(0u32, |s, w| s.wrapping_add(u32::from_le_bytes(*w)))
    }

    /// A SAVE_DATA_SET 277 as the client sends it: ids, kind 2, `block`, the revision and the block's sum.
    fn save_277(aime: u32, coach: u32, card: u32, block: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; SAVE_SET_LEN];
        for (i, b) in p.iter_mut().enumerate().skip(TAIL) {
            *b = (i * 13 + 5) as u8;
        }
        put(&mut p, 4, aime);
        put(&mut p, 8, coach);
        put(&mut p, 12, card);
        p[24] = 2;
        p[28..28 + BLOCK].copy_from_slice(block);
        put(&mut p, 3100, 12);
        p[3102..3104].copy_from_slice(&(BLOCK as u16).to_le_bytes());
        put(&mut p, 3224, 0);
        put(&mut p, 5236, sum32(block));
        put(&mut p, 5940, 47565);
        p
    }

    /// A saved club as the store keeps it: the ids inside its block too.
    fn coach(aime: u32, coach: u32, card: u32, seed: u8) -> Coach {
        let mut block: Vec<u8> = (0..BLOCK)
            .map(|i| (i as u8).wrapping_mul(seed) ^ seed)
            .collect();
        for bit in CARD_BITS {
            put_bits(&mut block, bit, 32, card);
        }
        put_bits(&mut block, COACH_BIT, 32, coach);
        let req = save_277(aime, coach, card, &block);
        Coach {
            coach,
            card,
            name1: b"MANAGER".to_vec(),
            name2: b"CLUB".to_vec(),
            rec122: 90,
            play_count: 7,
            titles: 16,
            created: 100,
            updated: 200,
            save: Some(Save {
                block,
                version: 12,
                size: BLOCK as u16,
                kind: 2,
                updated: 200,
                tail: req[TAIL..].to_vec(),
            }),
            injustice: Some(1),
            requests: vec![Request {
                cmd: SAVE_SET,
                payload: req,
                updated: 200,
            }],
        }
    }

    fn purchase(aime: u32) -> Purchase {
        let mut payload = vec![0u8; 20];
        put(&mut payload, 4, aime);
        put(&mut payload, 8, 1);
        put(&mut payload, 12, 2);
        Purchase { at: 150, payload }
    }

    fn club(aime: u32, ids: &[(u32, u32)]) -> Club {
        Club {
            aime,
            coaches: ids
                .iter()
                .enumerate()
                .map(|(i, &(k, c))| coach(aime, k, c, i as u8 + 3))
                .collect(),
            purchases: vec![purchase(aime)],
        }
    }

    /// A store at `path` holding `c`, its counters past `c`'s ids.
    fn store_with(path: &Path, c: &Club) -> Store {
        let st = Store::open(path).unwrap();
        insert(&st.0, c).unwrap();
        let top = c
            .coaches
            .iter()
            .map(|k| k.coach.max(k.card))
            .max()
            .unwrap_or(0);
        st.0.execute("INSERT INTO ids VALUES ('coach', ?1), ('card', ?1)", [top])
            .unwrap();
        st
    }

    /// An old card at home (integer Aime id 1, from before the hash) brings two clubs to a host whose own club holds
    /// the same coach and card ids; a match is saved there through the stand-in; the copy back restores the home
    /// ids and every other byte, with the match's changes.
    #[test]
    fn round_trip_restores_the_ids_and_every_byte() {
        let d = tempfile::tempdir().unwrap();
        let (home_aime, arcade_aime) = (1, crate::standins::aimedb::hash_id(CODE) as u32);
        let sent = club(home_aime, &[(1, 1), (2, 3)]);
        let mut home = store_with(&d.path().join("home.db"), &sent);
        let host = store_with(
            &d.path().join("host.db"),
            &club(9, &[(1, 1), (2, 2), (3, 3)]),
        );
        let host_own = host.club(9).unwrap();
        let out = home.club(home_aime).unwrap();
        assert_eq!(out, sent);
        let host = Mutex::new(host);
        let link = host
            .lock()
            .unwrap()
            .import_guest(&out, arcade_aime, 2, &token(CODE))
            .unwrap();
        assert_eq!(link.coaches, [(1, 4), (2, 5)]);
        assert_eq!(link.cards, [(1, 4), (3, 5)]);

        // in the arcade: host ids everywhere the store and the client read them
        let there = host
            .lock()
            .unwrap()
            .guest_club(arcade_aime, &token(CODE))
            .unwrap();
        assert_eq!(there.aime, arcade_aime);
        for (k, (coach, card)) in there.coaches.iter().zip([(4, 4), (5, 5)]) {
            assert_eq!((k.coach, k.card), (coach, card));
            let b = &k.save.as_ref().unwrap().block;
            assert_eq!(
                [
                    get_bits(b, 185, 32),
                    get_bits(b, 263, 32),
                    get_bits(b, 217, 32)
                ],
                [card, card, coach]
            );
            let p = &k.requests[0].payload;
            assert_eq!(
                [u32_at(p, 4), u32_at(p, 8), u32_at(p, 12)],
                [arcade_aime, coach, card]
            );
        }
        assert_eq!(u32_at(&there.purchases[0].payload, 4), arcade_aime);
        assert_eq!(
            host.lock().unwrap().club(9).unwrap(),
            host_own,
            "the host's own club is untouched"
        );

        // nothing played: the copy back is the club sent, byte for byte
        let mut back = link.clone();
        let stamp = home
            .write_home(&there, &mut back, home_aime, &out.stamp())
            .unwrap()
            .unwrap();
        assert_eq!(home.club(home_aime).unwrap(), sent);
        assert_eq!(back, link);

        // a match saved in the arcade by the stand-in (277 with the arcade's ids, one byte of the block changed)
        let k = &there.coaches[1];
        let mut block = k.save.as_ref().unwrap().block.clone();
        block[1000] ^= 0x5A;
        let (_, refused) =
            super::super::gamesvr::ask_for_test(&host, 277, &save_277(arcade_aime, 5, 5, &block));
        assert_eq!(refused, None);
        let after = host
            .lock()
            .unwrap()
            .guest_club(arcade_aime, &token(CODE))
            .unwrap();
        home.write_home(&after, &mut back, home_aime, &stamp)
            .unwrap()
            .unwrap();
        let got = home.club(home_aime).unwrap();
        assert_eq!(
            got.coaches[0], sent.coaches[0],
            "the club that did not play comes back as it went"
        );
        let (g, s) = (&got.coaches[1], &sent.coaches[1]);
        assert_eq!((g.coach, g.card, g.play_count), (2, 3, s.play_count + 1));
        let mut want = s.save.as_ref().unwrap().block.clone();
        want[1000] ^= 0x5A;
        assert_eq!(
            g.save.as_ref().unwrap().block,
            want,
            "the home ids back in the block, the match's byte kept"
        );
        let p = &g.requests[0].payload;
        assert_eq!(
            [u32_at(p, 4), u32_at(p, 8), u32_at(p, 12)],
            [home_aime, 2, 3]
        );
        assert_eq!(p[28..28 + BLOCK], want[..]);
        assert_eq!(got.purchases, sent.purchases);
        assert_eq!(host.lock().unwrap().club(9).unwrap(), host_own);
    }

    /// A club made in the arcade gets fresh home ids; the next copy back keeps them.
    #[test]
    fn a_club_made_in_the_arcade_gets_home_ids() {
        let d = tempfile::tempdir().unwrap();
        let mut home = store_with(&d.path().join("home.db"), &club(1, &[(1, 1)]));
        let mut host = store_with(&d.path().join("host.db"), &club(9, &[(1, 1)]));
        let link = host
            .import_guest(&home.club(1).unwrap(), 50, 3, &token(CODE))
            .unwrap();
        // made in the arcade: coach 7 / card 8, as COACH_LIST_ADD would
        insert(
            &host.0,
            &Club {
                aime: 50,
                coaches: vec![coach(50, 7, 8, 9)],
                purchases: vec![],
            },
        )
        .unwrap();
        let there = host.guest_club(50, &token(CODE)).unwrap();
        let mut back = link;
        let stamp = home
            .write_home(&there, &mut back, 1, &home.club(1).unwrap().stamp())
            .unwrap()
            .unwrap();
        let got = home.club(1).unwrap();
        let new = &got.coaches[1];
        assert_eq!((new.coach, new.card), (2, 2), "the home store's next ids");
        assert_eq!(get_bits(&new.save.as_ref().unwrap().block, 217, 32), 2);
        let again = back.clone();
        home.write_home(&there, &mut back, 1, &stamp)
            .unwrap()
            .unwrap();
        assert_eq!(back, again);
        assert_eq!(home.club(1).unwrap(), got);
    }

    /// A home club that changed since the copy is never overwritten.
    #[test]
    fn a_newer_home_club_is_kept() {
        let d = tempfile::tempdir().unwrap();
        let mut home = store_with(&d.path().join("home.db"), &club(1, &[(1, 1)]));
        let mut host = Store::open(&d.path().join("host.db")).unwrap();
        let stamp = home.club(1).unwrap().stamp();
        let mut link = host
            .import_guest(&home.club(1).unwrap(), 50, 3, &token(CODE))
            .unwrap();
        home.0
            .execute("UPDATE coaches SET play_count = 99", [])
            .unwrap();
        let mine = home.club(1).unwrap();
        let there = host.guest_club(50, &token(CODE)).unwrap();
        assert_eq!(home.write_home(&there, &mut link, 1, &stamp).unwrap(), None);
        assert_eq!(home.club(1).unwrap(), mine);
    }

    #[test]
    fn a_guest_cannot_touch_the_host_or_another_guest() {
        let d = tempfile::tempdir().unwrap();
        let mut host = store_with(&d.path().join("host.db"), &club(9, &[(1, 1)]));
        let own = host.club(9).unwrap();
        let (a, b) = (token(CODE), token("50123456789012345679"));
        let mine = club(1, &[(1, 1)]);
        // the host's own club (or any club the desk did not import) is never replaced, read or dropped
        assert!(
            host.import_guest(&mine, 9, 2, &a)
                .unwrap_err()
                .contains("already keeps")
        );
        assert!(host.guest_club(9, &a).is_err());
        assert!(host.drop_guest(9, &a).is_err());
        assert_eq!(host.club(9).unwrap(), own);
        // the first guest owns its Aime id here; another token is refused
        host.import_guest(&mine, 100, 2, &a).unwrap();
        assert!(
            host.import_guest(&mine, 100, 3, &b)
                .unwrap_err()
                .contains("another player")
        );
        assert!(host.guest_club(100, &b).is_err());
        assert!(host.drop_guest(100, &b).is_err());
        assert_eq!(host.guest_club(100, &a).unwrap().coaches.len(), 1);
        // the same guest again replaces its copy
        host.import_guest(&club(1, &[(1, 1), (2, 2)]), 100, 2, &a)
            .unwrap();
        assert_eq!(host.guest_club(100, &a).unwrap().coaches.len(), 2);
        // malformed or foreign records
        let bad = |f: &dyn Fn(&mut Club)| {
            let mut c = club(1, &[(1, 1), (2, 2)]);
            f(&mut c);
            c
        };
        for (what, c) in [
            (
                "short block",
                bad(&|c| {
                    c.coaches[0].save.as_mut().unwrap().block.pop();
                }),
            ),
            (
                "odd tail",
                bad(&|c| c.coaches[0].save.as_mut().unwrap().tail.push(0)),
            ),
            (
                "other command",
                bad(&|c| c.coaches[0].requests[0].cmd = 309),
            ),
            (
                "short request",
                bad(&|c| c.coaches[0].requests[0].payload.truncate(100)),
            ),
            (
                "another card's save",
                bad(&|c| put(&mut c.coaches[0].requests[0].payload, 4, 77)),
            ),
            (
                "another card's purchase",
                bad(&|c| put(&mut c.purchases[0].payload, 4, 77)),
            ),
            ("repeated coach", bad(&|c| c.coaches[1].coach = 1)),
            ("zero card", bad(&|c| c.coaches[1].card = 0)),
            ("long name", bad(&|c| c.coaches[0].name1 = vec![b'x'; 57])),
            ("Aime id 0", bad(&|c| c.aime = 0)),
        ] {
            assert!(host.import_guest(&c, 200, 2, &a).is_err(), "{what}");
        }
        assert!(host.guest_club(200, &a).is_err(), "nothing imported");
        // one club per seat at most: a guest cannot fill the host's store
        for aime in 101..108 {
            host.import_guest(&club(1, &[(1, 1)]), aime, 2, &a).unwrap();
        }
        assert!(
            host.import_guest(&club(1, &[(1, 1)]), 108, 2, &a)
                .unwrap_err()
                .contains("8 guests")
        );
        host.import_guest(&club(1, &[(1, 1)]), 107, 2, &a).unwrap();
        // the purge at the host's next start drops guests only
        assert_eq!(host.purge_guests().unwrap(), 8);
        assert!(!has_rows(&host.0, 100).unwrap());
        assert_eq!(host.club(9).unwrap(), own);
    }

    /// The host's desk for tests: `serve` on every connection, seat 2 taken; `down` closes connections unanswered;
    /// `closed`: the next connection closes the desk itself (later ones are refused, as after the host's Stop).
    fn desk(db: PathBuf) -> (SocketAddr, Arc<AtomicBool>, Arc<AtomicBool>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let (at, down, closed) = (
            l.local_addr().unwrap(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let (off, gone) = (down.clone(), closed.clone());
        thread::spawn(move || {
            for c in l.incoming() {
                let mut c = c.unwrap();
                if gone.load(Ordering::Relaxed) {
                    return;
                }
                if off.load(Ordering::Relaxed) {
                    continue;
                }
                let mut buf = Vec::new();
                while !buf.contains(&b'\n') {
                    let mut b = [0u8; 256];
                    match c.read(&mut b) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&b[..n]),
                    }
                }
                let Some(at) = buf.iter().position(|&b| b == b'\n') else {
                    continue;
                };
                let rest = buf.split_off(at + 1);
                let line = String::from_utf8_lossy(&buf).trim().to_owned();
                let _ = serve(&db, &line, rest, &mut c, &|s| s == 2);
                let _ = c.shutdown(std::net::Shutdown::Write);
            }
        });
        (at, down, closed)
    }

    fn guest(desk: SocketAddr, dir: &Path, seat: u8) -> Guest {
        Guest {
            desk,
            seat,
            code: CODE.into(),
            home_aime: 1,
            arcade_aime: 50,
            dir: dir.to_path_buf(),
            arcade: "test arcade".into(),
            poll: Duration::from_millis(200),
        }
    }

    fn wait(what: &str, ok: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !ok() {
            assert!(Instant::now() < end, "{what}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// The whole trip over the desk words: backup, copy in, a save copied home while playing, home at the end and
    /// the host's copy dropped; a seat not taken is refused.
    #[test]
    fn a_trip_over_the_desk() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("gamesvr");
        std::fs::create_dir_all(&dir).unwrap();
        let sent = club(1, &[(1, 1)]);
        drop(store_with(&dir.join("gamesvr.db"), &sent));
        let host_db = d.path().join("host.db");
        drop(store_with(&host_db, &club(9, &[(1, 1)])));
        let (at, _down, _) = desk(host_db.clone());
        let log: Log = Arc::new(|_: &str| {});
        assert!(
            guest(at, &dir, 3)
                .bring(log.clone())
                .err()
                .unwrap()
                .contains("seat 3 is not taken")
        );
        assert!(!out_file(&dir).exists());

        let trip = guest(at, &dir, 2).bring(log).unwrap();
        assert!(out_file(&dir).exists(), "marked out");
        let backups = std::fs::read_dir(dir.join("backups")).unwrap().count();
        assert_eq!(
            backups, 2,
            "the home club backed up first (the refused try made one too)"
        );
        // a save in the arcade comes home within a poll
        let host = Mutex::new(Store::open(&host_db).unwrap());
        let there = host.lock().unwrap().guest_club(50, &token(CODE)).unwrap();
        let mut block = there.coaches[0].save.as_ref().unwrap().block.clone();
        block[2000] ^= 1;
        let (k, c) = (there.coaches[0].coach, there.coaches[0].card);
        assert_eq!(
            super::super::gamesvr::ask_for_test(&host, 277, &save_277(50, k, c, &block)).1,
            None
        );
        let home = Store::open(&dir.join("gamesvr.db")).unwrap();
        wait("the save copied home", || {
            home.club(1).unwrap().coaches[0].play_count == 8
        });
        drop(trip);
        assert!(
            !out_file(&dir).exists() && !last_file(&dir).exists(),
            "home: marker gone"
        );
        assert!(
            !has_rows(&host.lock().unwrap().0, 50).unwrap(),
            "the host dropped its copy"
        );
        let got = home.club(1).unwrap();
        assert_eq!(
            get_bits(&got.coaches[0].save.as_ref().unwrap().block, 217, 32),
            1
        );
        assert_eq!(
            got.coaches[0].save.as_ref().unwrap().block[2000],
            sent.coaches[0].save.as_ref().unwrap().block[2000] ^ 1
        );
        assert_eq!(leftover(&dir).unwrap(), None);
    }

    /// The host dies mid-session: the club stays marked out; the next start keeps the home copy, puts the last
    /// pulled copy in backups and says so once.
    #[test]
    fn a_host_crash_keeps_the_home_copy() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("gamesvr");
        std::fs::create_dir_all(&dir).unwrap();
        drop(store_with(&dir.join("gamesvr.db"), &club(1, &[(1, 1)])));
        let host_db = d.path().join("host.db");
        let (at, down, _) = desk(host_db.clone());
        let trip = guest(at, &dir, 2).bring(Arc::new(|_: &str| {})).unwrap();
        wait("a first copy pulled", || last_file(&dir).exists());
        let home = Store::open(&dir.join("gamesvr.db")).unwrap();
        let before = home.club(1).unwrap();
        down.store(true, Ordering::Relaxed);
        let progress = trip.progress();
        drop(trip);
        let p = *progress.lock().unwrap();
        assert!(!p.home && p.last_ok.is_some(), "{p:?}");
        assert!(out_file(&dir).exists(), "not home: still marked out");
        assert!(
            has_rows(&Store::open(&host_db).unwrap().0, 50).unwrap(),
            "the host still holds it (purged at its start)"
        );
        let kept = leftover(&dir).unwrap().expect("told");
        assert!(
            kept.starts_with(dir.join("backups")) && kept.is_file(),
            "{kept:?}"
        );
        let (link, host): (Link, Club) =
            serde_json::from_slice(&std::fs::read(&kept).unwrap()).unwrap();
        assert_eq!((link.coaches.len(), host.aime), (1, 50));
        assert!(!out_file(&dir).exists());
        assert_eq!(home.club(1).unwrap(), before, "the home copy is kept");
        assert_eq!(leftover(&dir).unwrap(), None, "said once");
    }

    /// The host closes its arcade (its desk gone, the PC still up): the last copy-back says so, and once the run
    /// finds the club safe, `settle` takes the marker away so the next start says nothing.
    #[test]
    fn a_closed_arcade_is_told_and_settled() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("gamesvr");
        std::fs::create_dir_all(&dir).unwrap();
        drop(store_with(&dir.join("gamesvr.db"), &club(1, &[(1, 1)])));
        let (addr, _down, closed) = desk(d.path().join("host.db"));
        let trip = guest(addr, &dir, 2).bring(Arc::new(|_: &str| {})).unwrap();
        wait("a first copy pulled", || last_file(&dir).exists());
        let progress = trip.progress();
        wait("a good copy-back", || {
            progress.lock().unwrap().last_ok.is_some()
        });
        closed.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(addr); // wakes the desk, which then closes its listener
        wait("a refused copy-back", || progress.lock().unwrap().gone_once);
        drop(trip);
        let p = *progress.lock().unwrap();
        assert!(!p.home && p.gone, "{p:?}");
        assert!(out_file(&dir).exists());
        settle(&dir).unwrap();
        assert!(!out_file(&dir).exists() && !last_file(&dir).exists());
        settle(&dir).unwrap();
        assert_eq!(
            leftover(&dir).unwrap(),
            None,
            "nothing to tell at the next start"
        );
    }

    #[test]
    fn an_old_card_is_found_under_its_home_id() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("aimedb.json");
        std::fs::write(&path, format!("{{\"{CODE}\": 1}}")).unwrap();
        let db = crate::standins::aimedb::Db::open(path).unwrap();
        assert_eq!(db.home_id(CODE), 1);
        let other = "50123456789012345679";
        assert_eq!(db.home_id(other), crate::standins::aimedb::hash_id(other));
        assert_eq!(db.len(), 1, "read only");
    }

    #[test]
    fn token_hides_the_code() {
        let code = "50123456789012345678";
        assert_eq!(token(code).len(), 64);
        assert!(!token(code).contains(code));
        assert_ne!(token(code), token("50123456789012345679"));
    }
}
