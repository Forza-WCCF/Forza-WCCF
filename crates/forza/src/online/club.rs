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
//! id that holds a club the desk did not import is never read or dropped. Such a club is replaced (kept in `backups`
//! first) only when it is not the arcade's own: a club of one of the host's cards ([`serve`]'s `own`) refuses the
//! bring, so the host's card on a second PC plays the arcade's club as it is. The guest is a trust boundary too: a
//! copy-back that fails [`Club::check`], holds another Aime id or lost a club the guest brought is set aside in
//! `backups`, never written home.
//!
//! The guest's side keeps `club_out.json` (the marker: in which arcade since when, the id map, the home club's stamp)
//! beside its store while the club is out, and `club_last.json`, the last copy pulled. A marker found at the next
//! start means the session did not end cleanly: the home copy stays, the last pulled copy goes to `backups`, and the
//! player is told ([`leftover`]). Before that start the Launcher window reads what was left ([`left`]): when the
//! last copy never came home, the player may put it home ([`take`], the home club backed up first and `club_undo.json`
//! kept for [`undo`] until the next start). A guest who re-joins the same arcade first takes its newer copy home
//! ([`Guest::rejoin`]), so the desk sets aside no match saved there.
//!
//! Online on a server's own arcade (`forza-server all`, its game server beside the desk) the trip is the same, to
//! that server's store; there no seat is needed and the brought club replaces what the server kept for the card
//! ([`Store::import_public`]). Past [`MAX_PUBLIC_GUESTS`] the oldest copy whose card is not on the game server makes
//! room, so asking for copies keeps none of them: only players logged in there are never evicted.

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
/// Guest clubs a public server's store holds ([`Store::import_public`]); the oldest copy not in play makes room.
const MAX_PUBLIC_GUESTS: usize = 256;
/// Backups of each kind kept in `backups` (oldest go first).
const KEEP_BACKUPS: usize = 30;
/// Clubs the host set aside ([`set_aside`]) kept in `backups` per kind across all Aime ids: evictions a stranger can
/// drive stop filling the disk there.
const KEEP_SET_ASIDE: usize = 100;
/// Why a bring for an Aime id another token holds is refused.
const ANOTHER: &str = "another player's club for this card is in this arcade";
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

/// A save block's tag for the logs: 8 hex digits of its SHA-256 with the ids a copy swaps zeroed, so a club reads the
/// same at home and in an arcade, and any change to its squad, lineup or record shows as a new tag.
pub fn team_tag(block: &[u8]) -> String {
    let mut b = block.to_vec();
    if b.len() >= BLOCK {
        for bit in CARD_BITS.into_iter().chain([COACH_BIT]) {
            put_bits(&mut b, bit, 32, 0);
        }
    }
    Sha256::digest(&b)[..4]
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
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

    /// One short log line: the card count, then each club card's id, coach id, play count and [`team_tag`].
    pub fn summary(&self) -> String {
        let mut s = format!("aime {}: {} card(s)", self.aime, self.coaches.len());
        for k in &self.coaches {
            let team = k.save.as_ref().map_or("-".into(), |v| team_tag(&v.block));
            s += &format!(
                " [card {} coach {} plays {} team {team}]",
                k.card, k.coach, k.play_count
            );
        }
        s
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

/// Writes every row of `aime` to `backups\<what>-<aime>-<time>.json` beside the store; [`KEEP_SET_ASIDE`] of `what`.
fn set_aside(tx: &Connection, aime: u32, what: &str) -> Result<(), String> {
    let old = serde_json::to_vec(&export(tx, aime).map_err(db_err)?).map_err(|e| e.to_string())?;
    // a store in memory (tests) has nothing on disk to keep the club beside
    let Some(dir) = tx
        .path()
        .filter(|p| !p.is_empty())
        .and_then(|p| Path::new(p).parent())
    else {
        return Ok(());
    };
    backup(dir, &format!("{what}-{aime}"), &old).map_err(|e| format!("backup: {e}"))?;
    let kind = format!("{what}-");
    prune(&dir.join("backups"), KEEP_SET_ASIDE, |name| {
        name.starts_with(&kind)
    })
    .map_err(|e| format!("backup: {e}"))
}

/// The desk's answer for a card whose club it does not hold (never brought, or the arcade restarted).
const NOT_BROUGHT: &str = "no club of this card was brought here";

/// The rows of `aime` the desk imported for the guest holding `token`; `Err` for any other Aime id or token.
fn owned(db: &Connection, aime: u32, token: &str) -> Result<(), String> {
    match guest_token(db, aime).map_err(db_err)? {
        Some(t) if t == token => Ok(()),
        Some(_) => Err("another player's club for this card is in this arcade".into()),
        None => Err(NOT_BROUGHT.into()),
    }
}

impl Store {
    /// Host: imports a guest's home club under its arcade Aime id `aime` with fresh coach and card ids; the map goes
    /// back to the guest. A copy of the same token replaces the old one (a rejoin); a club the arcade kept for that
    /// card (a copy that never went home, a club made there) is replaced too, kept in `backups` beside the store first:
    /// refusing it left the player on a club that stopped changing (2026-11-02). Another token's club is refused.
    pub fn import_guest(
        &mut self,
        club: &Club,
        aime: u32,
        seat: u8,
        token: &str,
    ) -> Result<Link, String> {
        self.import(club, aime, seat, token, None)
    }

    /// A public server's desk beside its own game server (`forza-server all`): no host club to keep, so the
    /// brought club replaces whatever the server kept for the card (a copy that never went home, a club made
    /// there), and past [`MAX_PUBLIC_GUESTS`] the oldest guest copy whose card is not `playing` on the game server
    /// makes room. Another token's copy is still refused.
    pub fn import_public(
        &mut self,
        club: &Club,
        aime: u32,
        seat: u8,
        token: &str,
        playing: &dyn Fn(u32) -> bool,
    ) -> Result<Link, String> {
        self.import(club, aime, seat, token, Some(playing))
    }

    fn import(
        &mut self,
        club: &Club,
        aime: u32,
        seat: u8,
        token: &str,
        public: Option<&dyn Fn(u32) -> bool>,
    ) -> Result<Link, String> {
        club.check()?;
        let tx = self.0.transaction().map_err(db_err)?;
        match guest_token(&tx, aime).map_err(db_err)? {
            Some(t) if t != token => return Err(ANOTHER.into()),
            // a rejoin: the arcade's copy may be newer than the home club brought again (a Launcher killed mid-run)
            Some(_) => set_aside(&tx, aime, "rejoined")?,
            None => {
                if has_rows(&tx, aime).map_err(db_err)? {
                    // the club the arcade kept for this card is replaced: kept in `backups` beside the store first
                    set_aside(&tx, aime, "replaced")?;
                }
                let n: u32 = tx
                    .query_row("SELECT count(*) FROM guests", [], |r| r.get(0))
                    .map_err(db_err)?;
                if let Some(playing) = public
                    && n as usize >= MAX_PUBLIC_GUESTS
                {
                    let by_age: Vec<u32> = tx
                        .prepare("SELECT aime_id FROM guests ORDER BY since, aime_id")
                        .and_then(|mut q| q.query_map([], |r| r.get(0))?.collect())
                        .map_err(db_err)?;
                    let Some(oldest) = by_age.into_iter().find(|&a| !playing(a)) else {
                        return Err(format!(
                            "this server holds {MAX_PUBLIC_GUESTS} clubs in play already"
                        ));
                    };
                    set_aside(&tx, oldest, "evicted")?;
                    delete(&tx, oldest).map_err(db_err)?;
                    tx.execute("DELETE FROM guests WHERE aime_id = ?1", [oldest])
                        .map_err(db_err)?;
                } else if public.is_none() && n as usize >= MAX_GUESTS {
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

    /// Guest: `club` (home ids) replaces the home rows of `club.aime` in one transaction, unless the home club's
    /// stamp is no longer `expect` (false: nothing written).
    pub fn put_home(&mut self, club: &Club, expect: &str) -> Result<bool, String> {
        let tx = self.0.transaction().map_err(db_err)?;
        if export(&tx, club.aime).map_err(db_err)?.stamp() != expect {
            return Ok(false);
        }
        delete(&tx, club.aime).map_err(db_err)?;
        insert(&tx, club).map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(true)
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
/// leased on the desk now, `own` whether an Aime id is one of the host's own cards (`aimedb::Db::holds`); `public`:
/// a public server's store, which needs no seat and has no own cards, with whether a card is on its game server now
/// ([`Store::import_public`]). A PUT is refused from its line when it can be, before its upload is read.
/// Returns the line for the desk's log.
pub fn serve(
    db: &Path,
    head: &str,
    rest: Vec<u8>,
    conn: &mut TcpStream,
    held: &dyn Fn(u8) -> bool,
    own: &dyn Fn(u32) -> bool,
    public: Option<&dyn Fn(u32) -> bool>,
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
            // refused from the line, the upload unread: the desk's hang-up reads it off, so no reset loses the answer
            if public.is_none() && !held(seat) {
                return no(conn, &format!("seat {seat} is not taken"));
            }
            // the arcade's own club for one of its cards (its host's card on a second PC, or a guest naming that
            // id) stays: the player plays it here as it is, and the next start's purge never sees it
            let kept = || -> rusqlite::Result<bool> {
                Ok(guest_token(&store.0, aime)?.is_none() && has_rows(&store.0, aime)?)
            };
            if public.is_none() && own(aime) && kept().unwrap_or(true) {
                return no(conn, "this card's own club is in this arcade");
            }
            if guest_token(&store.0, aime)
                .ok()
                .flatten()
                .is_some_and(|t| t != token)
            {
                return no(conn, ANOTHER);
            }
            let body = read_body(conn, rest, size)?;
            let club: Club = match serde_json::from_slice(&body) {
                Ok(c) => c,
                Err(e) => return no(conn, &format!("not a club ({e})")),
            };
            let imported = if let Some(playing) = public {
                store.import_public(&club, aime, seat, &token, playing)
            } else {
                store.import_guest(&club, aime, seat, &token)
            };
            match imported {
                Ok(link) => {
                    let json = serde_json::to_vec(&link).unwrap_or_default();
                    let said = format!(
                        "club in: seat {seat}, Aime id {aime}, {} clubs; home {}",
                        club.coaches.len(),
                        club.summary()
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
                    let said = format!(
                        "club out: Aime id {aime}, {} clubs; {}",
                        club.coaches.len(),
                        club.summary()
                    );
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
    prune(&b, KEEP_BACKUPS, |name| {
        name.strip_prefix(what).is_some_and(|r| r.starts_with('-'))
    })?;
    Ok(path)
}

/// Keeps the newest `keep` of the `.json` files in `b` whose name passes `kind`.
fn prune(b: &Path, keep: usize, kind: impl Fn(&str) -> bool) -> io::Result<()> {
    let mut all: Vec<_> = std::fs::read_dir(b)?
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.ends_with(".json") && kind(&name)
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    all.sort();
    for (_, old) in all.iter().rev().skip(keep) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

/// CLEAR BAD ENDINGS (2017-18), at a restart asked by the panel: the improper-end counters (INJUSTICE_COUNT 289; the
/// client writes count - 1 to save parameter 73) of this PC's clubs back to 0, `gamesvr.db` copied to `backups\` first.
/// Arcade guests' clubs are left alone. -> what was done, in words.
pub fn clear_bad_endings(dir: &Path) -> Result<String, String> {
    if out_file(dir).exists() {
        return Ok(
            "the club is out in an arcade - its bad endings stay (clear them at home)".into(),
        );
    }
    let path = dir.join("gamesvr.db");
    if !path.exists() {
        return Ok("no club store - nothing to clear".into());
    }
    const HOME: &str =
        "FROM injustice WHERE count > 0 AND aime_id NOT IN (SELECT aime_id FROM guests)";
    let s = Store::open(&path).map_err(db_err)?;
    let n: u32 =
        s.0.query_row(&format!("SELECT count(*) {HOME}"), [], |r| r.get(0))
            .map_err(db_err)?;
    if n == 0 {
        return Ok("no bad endings to clear".into());
    }
    let b = dir.join("backups");
    std::fs::create_dir_all(&b).map_err(|e| e.to_string())?;
    let mut copy = b.join(format!("bad-endings-{}.db", time_name()));
    for k in 2.. {
        if !copy.exists() {
            break;
        }
        copy = b.join(format!("bad-endings-{}-{k}.db", time_name()));
    }
    // a consistent copy of the store, its WAL included
    s.0.execute("VACUUM INTO ?1", [copy.to_string_lossy()])
        .map_err(db_err)?;
    s.0.execute(
        &format!("UPDATE injustice SET count = 0 WHERE rowid IN (SELECT rowid {HOME})"),
        [],
    )
    .map_err(db_err)?;
    Ok(format!(
        "bad endings cleared ({n} club(s) back to 0; the old store is in {})",
        copy.display()
    ))
}

/// At a start: a club still marked out means the last arcade session ended without bringing it home. The home copy
/// stays; the last copy pulled from the host (if any) goes to `backups`, the marker goes, and so does an undo of
/// [`take`] (a start ends it). `Some((path, home))` when the player must be told: the backup (or the store folder
/// when no copy came), and `home` = `Some(time)` when that copy was already home (`time`: when the club last came
/// home, else when it left), `None` when it differed or could not be read.
pub fn leftover(dir: &Path) -> io::Result<Option<(PathBuf, Option<i64>)>> {
    match std::fs::remove_file(undo_file(dir)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    settle(dir)
}

/// [`leftover`] without the undo's end.
fn settle(dir: &Path) -> io::Result<Option<(PathBuf, Option<i64>)>> {
    let out = out_file(dir);
    if !out.exists() {
        return Ok(None);
    }
    let home = match read_left(dir) {
        Ok(Some(l)) => l.home_at(),
        _ => None,
    };
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
    Ok(Some((kept, home)))
}

/// What became of a host copy offered home ([`Guest::take_copy`]).
enum Took {
    Home,
    /// Refused or the home club changed meanwhile: set aside, the home club stays.
    Kept,
    Failed(String),
}

/// What [`Guest::rejoin`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Rejoin {
    /// The arcade's newer copy came home: the marker and the last copy went as at a start.
    Pulled,
    /// Nothing to take: [`leftover`] settles the marker as at any start.
    Skip,
    /// The arcade's copy could not be asked for or written home: the marker stays and no bring follows (it would
    /// send the older home club over the arcade's copy).
    Failed(String),
}

/// What a club still marked out left (read only, [`left`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Left {
    /// The arcade's last copy is the home club (or no copy came): `back` is when it last came home, else when it left.
    Same { back: i64 },
    /// The arcade's last copy is sound and differs from the home club: [`take`] can put it home.
    Differs(Offer),
}

impl Left {
    fn home_at(&self) -> Option<i64> {
        match self {
            Left::Same { back } => Some(*back),
            Left::Differs(_) => None,
        }
    }
}

/// The arcade's copy on offer, for the question: unix times on this PC's clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub arcade: String,
    /// When the copy was pulled (`club_last.json` written).
    pub pulled: i64,
    /// When the home club last came home, else when it left.
    pub home_at: i64,
    /// The play totals (home, arcade copy) over each copy's cards, only when they differ and no card is at the cap.
    pub plays: Option<(u32, u32)>,
}

/// `club_undo.json`: what [`undo`] puts back after [`take`].
#[derive(Serialize, Deserialize)]
struct Undo {
    home_aime: u32,
    /// The home club from before, a file name in `backups`.
    backup: String,
    /// The home club's stamp right after [`take`]: a club changed since is never replaced.
    stamp: String,
}

fn undo_file(dir: &Path) -> PathBuf {
    dir.join("club_undo.json")
}

/// Whether [`undo`] has something to put back.
pub fn can_undo(dir: &Path) -> bool {
    undo_file(dir).is_file()
}

/// A file read whole, refused past `cap` bytes.
fn read_capped(path: &Path, cap: usize) -> io::Result<Vec<u8>> {
    let mut b = Vec::new();
    std::fs::File::open(path)?
        .take(cap as u64 + 1)
        .read_to_end(&mut b)?;
    if b.len() > cap {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is over {cap} bytes", path.display()),
        ));
    }
    Ok(b)
}

/// The marker and the last pulled copy, checked as a copy-back is ([`sound`]). `Ok(None)`: no marker.
struct Pending {
    out: Out,
    /// The pulled copy and when it was written; `None` when no copy came.
    copy: Option<(Club, i64)>,
}

impl Pending {
    fn home_at(&self) -> Option<i64> {
        self.left(None).home_at()
    }

    fn left(&self, plays: Option<(u32, u32)>) -> Left {
        let back = if self.out.back != 0 {
            self.out.back
        } else {
            self.out.since
        };
        match &self.copy {
            Some((host, pulled)) if host.stamp() != self.out.back_stamp => Left::Differs(Offer {
                arcade: self.out.arcade.clone(),
                pulled: *pulled,
                home_at: back,
                plays,
            }),
            _ => Left::Same { back },
        }
    }
}

/// `Err` of kind `InvalidData`: a marker or copy that fails the checks; any other kind: it could not be read.
fn read_left(dir: &Path) -> io::Result<Option<Pending>> {
    let bad = |what: &str, e: &dyn std::fmt::Display| {
        io::Error::new(io::ErrorKind::InvalidData, format!("{what}: {e}"))
    };
    let out = match read_capped(&out_file(dir), 64 << 10) {
        Ok(b) => serde_json::from_slice::<Out>(&b).map_err(|e| bad("club_out.json", &e))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io::Error::new(e.kind(), format!("club_out.json: {e}"))),
    };
    let last = last_file(dir);
    let copy = match read_capped(&last, MAX_BYTES + (64 << 10)) {
        Ok(b) => {
            let (_, host): (Link, Club) =
                serde_json::from_slice(&b).map_err(|e| bad("club_last.json", &e))?;
            sound(out.arcade_aime, &out.link, &host).map_err(|e| bad("club_last.json", &e))?;
            let pulled = std::fs::metadata(&last)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            Some((host, pulled))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(io::Error::new(e.kind(), format!("club_last.json: {e}"))),
    };
    Ok(Some(Pending { out, copy }))
}

/// The club's play total, `None` when a card is at its cap (255) and the total says nothing.
fn plays(c: &Club) -> Option<u32> {
    c.coaches.iter().try_fold(0u32, |n, k| {
        (k.play_count < u8::MAX).then(|| n + u32::from(k.play_count))
    })
}

/// Read only, before PLAY: what a club still marked out left in `dir` (`data\save\gamesvr`). `Ok(None)`: no
/// marker; `Err`: a marker or copy that cannot be read or fails the checks, which is never offered (the next start
/// handles it as [`leftover`] always did). Not while a run is on: a trip in progress keeps a marker too.
pub fn left(dir: &Path) -> Result<Option<Left>, String> {
    let Some(p) = read_left(dir).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let mut left = p.left(None);
    if let (Left::Differs(o), Some((host, _))) = (&mut left, &p.copy) {
        let db = dir.join("gamesvr.db");
        let home = db
            .is_file()
            .then(|| Store::open(&db).and_then(|s| s.club(p.out.home_aime)).ok())
            .flatten();
        o.plays = home
            .and_then(|h| plays(&h).zip(plays(host)))
            .filter(|(a, b)| a != b);
    }
    Ok(Some(left))
}

/// Why [`take`] or [`undo`] replaced nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum NotReplaced {
    /// The home club changed in between: the copies are in this folder.
    Changed(PathBuf),
    Failed(String),
}

impl From<String> for NotReplaced {
    fn from(e: String) -> Self {
        NotReplaced::Failed(e)
    }
}

fn io_err(e: io::Error) -> NotReplaced {
    NotReplaced::Failed(e.to_string())
}

/// What [`take`] did once the copy was home.
#[derive(Debug)]
pub struct Taken {
    /// The home club from before, in `backups`.
    pub backup: PathBuf,
    /// A step after the copy went home that failed (marker, undo file, settle): the copy is home all the same.
    pub after: Option<String>,
}

/// USE THE ARCADE COPY: the copy [`left`] offers goes home after the home club is backed up; the marker and the copy
/// then go as at a start ([`leftover`]), and [`undo`] can put the home club back until the next start. `Err` only
/// when nothing was replaced.
pub fn take(dir: &Path) -> Result<Taken, NotReplaced> {
    let p = read_left(dir)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no club is out".to_owned())?;
    let (host, _) = match (&p.copy, p.left(None)) {
        (Some(c), Left::Differs(_)) => c,
        _ => return Err("the arcade's last copy is already home".to_owned().into()),
    };
    let mut out = p.out.clone();
    let mut store = Store::open(&dir.join("gamesvr.db")).map_err(db_err)?;
    let home = store.club(out.home_aime).map_err(db_err)?;
    let json = serde_json::to_vec(&home).map_err(|e| e.to_string())?;
    let saved = backup(dir, &format!("club-{}", out.home_aime), &json).map_err(io_err)?;
    let Some(stamp) = store.write_home(host, &mut out.link, out.home_aime, &home.stamp())? else {
        // both copies in `backups` for the player; the marker stays for the next start
        if let Ok(b) = std::fs::read(last_file(dir)) {
            let _ = backup(dir, "club-shop", &b);
        }
        return Err(NotReplaced::Changed(dir.join("backups")));
    };
    // the copy is home: each later step is tried, a failure is told but replaces nothing back
    let mut failed = Vec::new();
    // a stop right here must not offer the same copy again
    out.home_stamp.clone_from(&stamp);
    out.host_stamp = host.stamp();
    out.back_stamp.clone_from(&out.host_stamp);
    out.back = now();
    if let Err(e) = serde_json::to_vec_pretty(&out)
        .map_err(io::Error::other)
        .and_then(|b| write_atomic(&out_file(dir), &b))
    {
        failed.push(format!("club_out.json: {e}"));
    }
    let undo = Undo {
        home_aime: out.home_aime,
        backup: saved
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        stamp,
    };
    if let Err(e) = serde_json::to_vec_pretty(&undo)
        .map_err(io::Error::other)
        .and_then(|b| write_atomic(&undo_file(dir), &b))
    {
        failed.push(format!("club_undo.json: {e}"));
    }
    if let Err(e) = settle(dir) {
        failed.push(e.to_string());
    }
    Ok(Taken {
        backup: saved,
        after: (!failed.is_empty()).then(|| failed.join("; ")),
    })
}

/// PUT THE HOME CLUB BACK: the home club from before [`take`], while the club is still the one it put home. The
/// current club is backed up first; a club changed since is left as it is and the undo goes.
pub fn undo(dir: &Path) -> Result<(), NotReplaced> {
    let u: Undo = serde_json::from_slice(&read_capped(&undo_file(dir), 64 << 10).map_err(io_err)?)
        .map_err(|e| format!("club_undo.json: {e}"))?;
    let b = dir.join("backups");
    // a plain name in `backups`, nothing else
    if Path::new(&u.backup).file_name() != Some(std::ffi::OsStr::new(&u.backup)) {
        return Err(format!("club_undo.json: a bad backup name {:?}", u.backup).into());
    }
    let old: Club =
        serde_json::from_slice(&read_capped(&b.join(&u.backup), MAX_BYTES).map_err(io_err)?)
            .map_err(|e| format!("{}: {e}", u.backup))?;
    old.check()?;
    if old.aime != u.home_aime {
        return Err(format!("{}: the club of Aime id {}", u.backup, old.aime).into());
    }
    let mut store = Store::open(&dir.join("gamesvr.db")).map_err(db_err)?;
    let current = store.club(u.home_aime).map_err(db_err)?;
    if current.stamp() != u.stamp {
        let _ = std::fs::remove_file(undo_file(dir));
        return Err(NotReplaced::Changed(b));
    }
    let json = serde_json::to_vec(&current).map_err(|e| e.to_string())?;
    backup(dir, &format!("club-{}", u.home_aime), &json).map_err(io_err)?;
    if !store.put_home(&old, &u.stamp)? {
        let _ = std::fs::remove_file(undo_file(dir));
        return Err(NotReplaced::Changed(b));
    }
    std::fs::remove_file(undo_file(dir)).map_err(io_err)
}

/// How a trip went, for the run's end: read after the [`Trip`] is dropped (dropping it joins the trip's thread, so
/// its last copy-back is done; `a_trip_over_the_desk` checks the marker is gone right after the drop).
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

/// A copy-back error that says the host's server is gone while its PC answers: refused or reset (Windows' 10061 and
/// 10054, Linux's 111 and 104: [`ask`] keeps the OS error's text, which is the only place the code shows).
fn server_gone(e: &str) -> bool {
    ["10061", "10054", "111", "104"]
        .iter()
        .any(|code| e.contains(&format!("(os error {code})")))
}

/// Whether the host's copy `host` may be written home: a sound club under the arcade's Aime id that still holds
/// every club the guest brought or made there (`link`; nothing deletes a club in the game).
fn sound(arcade_aime: u32, link: &Link, host: &Club) -> Result<(), String> {
    host.check()?;
    if host.aime != arcade_aime {
        return Err(format!("the copy is of Aime id {}", host.aime));
    }
    match link
        .coaches
        .iter()
        .find(|(_, there)| !host.coaches.iter().any(|k| k.coach == *there))
    {
        Some((_, lost)) => Err(format!("coach {lost} is missing from the copy")),
        None => Ok(()),
    }
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
        log(&format!("club copy-in, home: {}", club.summary()));
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

    /// Re-join (#437), before [`leftover`] and [`Guest::bring`]: a club still marked out in this same arcade, its last
    /// copy home, takes the arcade's newer copy first (the home club backed up), so the bring carries every match
    /// saved there and the desk sets none aside. [`Rejoin::Skip`] leaves all to [`leftover`]: another arcade, a
    /// differing copy the player is asked about ([`left`]), nothing newer, a copy the desk no longer holds or that
    /// fails the checks.
    pub fn rejoin(&self, log: &Log) -> Rejoin {
        let p = match read_left(&self.dir) {
            Ok(Some(p)) => p,
            Ok(None) => return Rejoin::Skip,
            // a marker that fails the checks is leftover's; one that cannot be read keeps it, and no bring follows
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                log(&format!("re-join: the arcade's copy not asked for ({e})"));
                return Rejoin::Skip;
            }
            Err(e) => {
                log(&format!("re-join: the marker could not be read ({e})"));
                return Rejoin::Failed(format!("re-join: {e}"));
            }
        };
        let mut out = p.out.clone();
        if (out.arcade.as_str(), out.arcade_aime, out.home_aime)
            != (self.arcade.as_str(), self.arcade_aime, self.home_aime)
            || matches!(p.left(None), Left::Differs(_))
        {
            return Rejoin::Skip;
        }
        let (stamp, host) = match fetch(self.desk, self.arcade_aime, &self.code, "") {
            Ok(Some(got)) => got,
            Ok(None) => return Rejoin::Skip,
            // the arcade restarted: its copy is gone, the bring makes a new one
            Err(e) if e == NOT_BROUGHT => return Rejoin::Skip,
            Err(e) => {
                log(&format!("re-join: the arcade's copy not asked for ({e})"));
                return Rejoin::Failed(format!("re-join: {e}"));
            }
        };
        if stamp == out.back_stamp {
            return Rejoin::Skip;
        }
        let saved = Store::open(&self.dir.join("gamesvr.db"))
            .and_then(|s| Ok((s.club(self.home_aime)?, s)))
            .map_err(db_err)
            .and_then(|(home, s)| {
                let json = serde_json::to_vec(&home).map_err(|e| e.to_string())?;
                backup(&self.dir, &format!("club-{}", self.home_aime), &json)
                    .map(|path| (path, s))
                    .map_err(|e| e.to_string())
            });
        let mut store = match saved {
            Ok((path, s)) => {
                log(&format!("re-join: home club backed up: {}", path.display()));
                s
            }
            Err(e) => {
                log(&format!("re-join: the home club not backed up ({e})"));
                return Rejoin::Failed(format!("re-join: {e}"));
            }
        };
        match self.take_copy(&mut out, &mut store, stamp, host, log) {
            Took::Home => {}
            Took::Kept => return Rejoin::Skip,
            Took::Failed(e) => return Rejoin::Failed(format!("re-join: {e}")),
        }
        log("re-join: the arcade's newer copy of the club is home");
        if let Err(e) = settle(&self.dir) {
            log(&format!("re-join: the marker stays ({e})"));
        }
        Rejoin::Pulled
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
                        "club NOT brought home at the end (last good copy-back: {ago}): the marker stays, the next \
                         start keeps the home copy"
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
            // unchanged: home only if the copy last seen went home (not a refused or set-aside one)
            Ok(None) => return out.host_stamp == out.back_stamp,
            Err(e) => {
                log(&format!("club copy-back failed: {e}"));
                return false;
            }
        };
        if stamp == out.back_stamp {
            return true;
        }
        matches!(self.take_copy(out, store, stamp, host, log), Took::Home)
    }

    /// The host's copy `host` (stamp `stamp`, newer than the last one home) written home when sound and the home
    /// club did not change meanwhile; else set aside in `backups`.
    fn take_copy(
        &self,
        out: &mut Out,
        store: &mut Store,
        stamp: String,
        host: Club,
        log: &Log,
    ) -> Took {
        let pulled = serde_json::to_vec(&(&out.link, &host)).unwrap_or_default();
        if let Err(why) = sound(self.arcade_aime, &out.link, &host) {
            // asked again only once the host's copy changes; the home club and the last good copy stay
            out.host_stamp = stamp;
            let _ = self.mark(out);
            match backup(&self.dir, "club-refused", &pulled) {
                Ok(p) => log(&format!(
                    "club copy-back refused ({why}): the home club stays; the arcade's copy is {}",
                    p.display()
                )),
                Err(e) => log(&format!(
                    "club copy-back refused ({why}): the home club stays; backup failed ({e})"
                )),
            }
            return Took::Kept;
        }
        if let Err(e) = write_atomic(&last_file(&self.dir), &pulled) {
            log(&format!(
                "club copy-back: cannot keep the pulled copy ({e})"
            ));
        }
        if let Ok(home) = store.club(self.home_aime) {
            log(&format!("club copy-back, home before: {}", home.summary()));
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
                if let Ok(home) = store.club(self.home_aime) {
                    log(&format!("club copy-back, home after: {}", home.summary()));
                }
                Took::Home
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
                Took::Kept
            }
            Err(e) => {
                log(&format!("club copy-back failed: {e}"));
                Took::Failed(e)
            }
        }
    }
}

#[cfg(test)]
mod tests;
