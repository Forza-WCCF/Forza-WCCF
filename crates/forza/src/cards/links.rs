//! Player links, read at runtime from the player's own files (read-only; nothing derived is written). Layouts and rules
//! in `docs/research/LINKS-2010-11.md`:
//!
//! - **Affinity** 1-5 per pair of real players: `prog_data/player_data/partnership.bin` (129-byte records: two 64-byte
//!   Shift-JIS katakana names and the level; no header; one record covers both directions), keyed by the second
//!   katakana full name of `player_data.bin`; pairs not listed get the client's default from both birthdays (two small
//!   tables read from the player's own client program), +1 for the same nationality, -1 for a special card.
//! - **Link** 0-255 per pair of the club's 16 squad slots: the club card's CLUB_PLAYER_COOPERATION_INFO, 120 bytes
//!   from payload bit 3650, pair `k` of slots i < j as [`pair`]; each slot's card is Player\[i\]'s PLAYER_CARD_NO.

use crate::board::Card;
use std::collections::HashMap;
use std::path::Path;
use std::{fs, io};

/// Where a `player_data.bin` record keeps what the affinity rule reads, and where the client program keeps the default
/// rule's two tables (file offsets: the 60-day cycle's 12 types, the 12x12 base levels).
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    rec: usize,
    key: usize,
    birth: usize,
    nation: usize,
    special: usize,
    tables: Option<(usize, usize)>,
}

/// 2010-11: 0x328-byte records; the tables at VA 0xAED01C and 0xB02198.
pub const E1011: Layout = Layout {
    rec: 0x328,
    key: 0x177,
    birth: 0x112,
    nation: 0x124,
    special: 0x0B,
    tables: Some((0x6E_B21C, 0x70_0398)),
};
/// 2017-18: 0x620-byte records, the 2010-11 fields from +0x288 on (the second katakana name at +0x41F resolves 1,769
/// of the file's 1,810 names). The default rule's tables are not placed there yet: listed pairs only.
pub const E1718: Layout = Layout {
    rec: 0x620,
    key: 0x41F,
    birth: 0x3BA,
    nation: 0x3CC,
    special: 0x29F,
    tables: None,
};

const RECORD: usize = 129;
const NAME: usize = 64;

/// A 64-byte name field up to its NUL.
fn name(b: &[u8]) -> &[u8] {
    b.split(|&c| c == 0).next().unwrap_or(b)
}

/// One person (all their cards share one name and so one set of partners).
#[derive(Clone, Debug, Default)]
pub struct Person {
    /// The catalogue's name of their first card.
    pub name: String,
    pub cards: Vec<u32>,
    /// Listed partners (person index, level 1-5), best first.
    pub partners: Vec<(usize, u8)>,
}

#[derive(Clone, Copy, Debug)]
struct Facts {
    person: usize,
    /// The birthday's type (0-11) when the default tables are there.
    kind: Option<u8>,
    nation: u8,
    special: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Affinity {
    facts: HashMap<u32, Facts>,
    pub people: Vec<Person>,
    listed: HashMap<(usize, usize), u8>,
    base: Option<Vec<u8>>,
}

/// The client's birthday type (`sub_4E4050`): days from a fixed point into a 60-day cycle, the cycle's 12 types.
fn kind(types: &[u8], y: i32, m: i32, d: i32) -> u8 {
    let (n, y) = if m > 2 { (m - 3, y) } else { (m + 9, y - 1) };
    let n = n.clamp(0, 11);
    // the month table at 0xBD63C0 is the days before each month from March: (153 n + 2) / 5
    let days = y / 4 + d + 365 * y + y / 400 + (153 * n + 2) / 5 - y / 100 + 8;
    types[days.rem_euclid(60) as usize]
}

/// "Y/M/D" as the client's `atoi`s read it (`sub_4E40D0`): leading digits, 0 for none.
fn ymd(s: &[u8]) -> (i32, i32, i32) {
    let mut f = name(s).split(|&c| c == b'/').map(|p| {
        p.iter()
            .take_while(|c| c.is_ascii_digit())
            .fold(0i32, |v, c| {
                v.saturating_mul(10).saturating_add(i32::from(c - b'0'))
            })
    });
    (
        f.next().unwrap_or(0),
        f.next().unwrap_or(0),
        f.next().unwrap_or(0),
    )
}

/// The two default tables in the client program, when they read as the tables (types 0-11; levels 1-5, symmetric).
fn tables(l: Layout, exe: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let (t, b) = l.tables?;
    let types = exe.get(t..t + 60)?.to_vec();
    let base = exe.get(b..b + 144)?.to_vec();
    let ok = types.iter().all(|&v| v < 12)
        && base.iter().all(|v| (1..=5).contains(v))
        && (0..144).all(|k| base[k] == base[(k % 12) * 12 + k / 12]);
    ok.then_some((types, base))
}

impl Affinity {
    /// From the files' bytes: `cards` the catalogue (each card's `record` into `player_data`), `exe` the client program
    /// (the default rule; `None` or not matching: listed pairs only).
    pub fn parse(
        l: Layout,
        cards: &[Card],
        player_data: &[u8],
        partnership: &[u8],
        exe: Option<&[u8]>,
    ) -> Affinity {
        let tabs = exe.and_then(|e| tables(l, e));
        let mut a = Affinity::default();
        let mut person_of: HashMap<&[u8], usize> = HashMap::new();
        for c in cards {
            let Some(r) = c
                .record
                .and_then(|r| player_data.get(r as usize * l.rec..(r as usize + 1) * l.rec))
            else {
                continue;
            };
            let key = name(&r[l.key..l.key + NAME]);
            if key.is_empty() {
                continue;
            }
            let person = *person_of.entry(key).or_insert_with(|| {
                a.people.push(Person {
                    name: c.name.clone(),
                    ..Person::default()
                });
                a.people.len() - 1
            });
            a.people[person].cards.push(c.number);
            let (y, m, d) = ymd(&r[l.birth..l.birth + 16]);
            a.facts.insert(
                c.number,
                Facts {
                    person,
                    kind: tabs.as_ref().map(|(t, _)| kind(t, y, m, d)),
                    nation: r[l.nation],
                    special: r[l.special] > 3,
                },
            );
        }
        for rec in partnership.as_chunks::<RECORD>().0 {
            let (Some(&x), Some(&y)) = (
                person_of.get(name(&rec[..NAME])),
                person_of.get(name(&rec[NAME..2 * NAME])),
            ) else {
                continue; // a player without a card here
            };
            if x != y {
                a.listed.insert((x.min(y), x.max(y)), rec[128]);
            }
        }
        for (&(x, y), &v) in &a.listed {
            a.people[x].partners.push((y, v));
            a.people[y].partners.push((x, v));
        }
        for p in &mut a.people {
            p.partners.sort_by_key(|&(q, v)| (std::cmp::Reverse(v), q));
        }
        a.base = tabs.map(|(_, b)| b);
        a
    }

    /// From the game folder (`prog_data/player_data/`) and its client program, read-only.
    pub fn load(l: Layout, game: &Path, exe: &Path, cards: &[Card]) -> io::Result<Affinity> {
        let dir = game.join("prog_data").join("player_data");
        let players = fs::read(dir.join("player_data.bin"))?;
        let pairs = fs::read(dir.join("partnership.bin"))?;
        if players.len() % l.rec != 0 || pairs.len() % RECORD != 0 {
            return Err(io::Error::other(format!(
                "player_data.bin {} / partnership.bin {} bytes: not this edition's records",
                players.len(),
                pairs.len()
            )));
        }
        let exe = fs::read(exe).ok();
        Ok(Self::parse(l, cards, &players, &pairs, exe.as_deref()))
    }

    /// Listed pairs, and whether the default rule is there.
    pub fn counts(&self) -> (usize, bool) {
        (self.listed.len(), self.base.is_some())
    }

    /// The affinity of two cards, as the client's `sub_4E3EA0`: the listed level, else the default rule; `None` for a
    /// card not in the catalogue, the same player, or an unlisted pair without the default tables.
    pub fn level(&self, a: u32, b: u32) -> Option<u8> {
        let (x, y) = (self.facts.get(&a)?, self.facts.get(&b)?);
        if x.person == y.person {
            return None;
        }
        if let Some(&v) = self
            .listed
            .get(&(x.person.min(y.person), x.person.max(y.person)))
        {
            return Some(v);
        }
        let base = self.base.as_ref()?;
        let mut n = i32::from(base[usize::from(x.kind?) * 12 + usize::from(y.kind?)]);
        n += i32::from(x.nation == y.nation) - i32::from(x.special || y.special);
        Some(n.clamp(1, 5) as u8)
    }

    /// A card's listed partners (each a person with all their cards), best first.
    pub fn partners(&self, card: u32) -> impl Iterator<Item = (&Person, u8)> {
        self.facts
            .get(&card)
            .map(|f| self.people[f.person].partners.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|&(p, v)| (&self.people[p], v))
    }
}

// ---- the club card's links ----

/// The pair index of squad slots i < j (0-15) in the club's 120-pair arrays, as the client's readers compute it.
pub fn pair(i: usize, j: usize) -> usize {
    15 * i - i * (i + 1) / 2 + j - 1
}

const CARD_BYTES: usize = 16 + 256 * 16;
const COPIES: [usize; 2] = [16 + 8 * 16, 16 + 8 * 16 + 0x690];
const PAYLOAD: usize = 0x68C;
/// CLUB_PLAYER_COOPERATION_INFO, 120 x 8 bits.
const LINK_BIT: usize = 3650;
/// Player\[i\] from this bit, 312 bits each; PLAYER_CARD_NO its first 14.
const PLAYER_BIT: usize = 6688;

fn bits(buf: &[u8], start: usize, width: usize) -> u32 {
    (start..start + width).fold(0, |v, i| {
        (v << 1) | u32::from((buf[i >> 3] >> (7 - (i & 7))) & 1)
    })
}

/// The club's 16 squad cards (0 = empty slot) and the link of each pair of slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClubLinks {
    pub cards: [u32; 16],
    pub links: [u8; 120],
}

impl ClubLinks {
    /// A kit card file's bytes, read from the copy the game reads (copy A when its checksum is good, else B); `None`
    /// for a new card, a wrong size or no good copy.
    pub fn parse(raw: &[u8]) -> Option<ClubLinks> {
        if raw.len() != CARD_BYTES || u16::from_le_bytes([raw[5], raw[6]]) == 0xFFFF {
            return None;
        }
        let word = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let copy = COPIES.iter().map(|&s| &raw[s..s + 0x690]).find(|c| {
            let total = (0..PAYLOAD)
                .step_by(4)
                .fold(0u32, |t, i| t.wrapping_add(word(c, i)));
            let stored = word(c, PAYLOAD);
            (total != 0 || stored != 0) && (stored == total || stored == total.wrapping_add(1))
        })?;
        Some(ClubLinks {
            cards: std::array::from_fn(|i| bits(copy, PLAYER_BIT + 312 * i, 14)),
            links: std::array::from_fn(|k| bits(copy, LINK_BIT + 8 * k, 8) as u8),
        })
    }

    pub fn read(path: &Path) -> Option<ClubLinks> {
        Self::parse(&fs::read(path).ok()?)
    }

    /// Each pair of squad cards' link, smaller card number first.
    pub fn by_cards(&self) -> HashMap<(u32, u32), u8> {
        let mut m = HashMap::new();
        for i in 0..16 {
            for j in i + 1..16 {
                let (a, b) = (self.cards[i], self.cards[j]);
                if a > 0 && b > 0 && a != b {
                    m.insert((a.min(b), a.max(b)), self.links[pair(i, j)]);
                }
            }
        }
        m
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::board::{Aptitude, Position, Rarity};

    pub(crate) fn card(number: u32, record: u32, name: &str) -> Card {
        Card {
            number,
            record: Some(record),
            name: name.into(),
            short: String::new(),
            club: String::new(),
            country: String::new(),
            season: String::new(),
            born: String::new(),
            pos: Position::Mf,
            rarity: Rarity::Regular,
            code: String::new(),
            secret: false,
            stats: [0; 6],
            total: 0,
            aptitude: Aptitude::default(),
        }
    }

    /// A synthetic 2010-11 record: katakana key, birthday, nationality, rarity byte.
    fn record(key: &str, birth: &str, nation: u8, special: u8) -> Vec<u8> {
        let mut r = vec![0u8; 0x328];
        let k = key.as_bytes(); // the reader compares bytes; any encoding does for the test
        r[0x177..0x177 + k.len()].copy_from_slice(k);
        r[0x112..0x112 + birth.len()].copy_from_slice(birth.as_bytes());
        r[0x124] = nation;
        r[0x0B] = special;
        r
    }

    fn pair_rec(a: &str, b: &str, v: u8) -> Vec<u8> {
        let mut r = vec![0u8; RECORD];
        r[..a.len()].copy_from_slice(a.as_bytes());
        r[64..64 + b.len()].copy_from_slice(b.as_bytes());
        r[128] = v;
        r
    }

    /// A made-up client program with the two tables: types = day % 12, base = 1 + (i + j) % 5.
    fn exe() -> Vec<u8> {
        let mut e = vec![0u8; 0x70_0398 + 144];
        for k in 0..60 {
            e[0x6E_B21C + k] = (k % 12) as u8;
        }
        for i in 0..12 {
            for j in 0..12 {
                e[0x70_0398 + i * 12 + j] = 1 + ((i + j) % 5) as u8;
            }
        }
        e
    }

    #[test]
    fn affinity_from_the_file_and_the_default_rule() {
        // A: two cards of one player; B listed with A at 5 (written B, A); C unlisted; D a special card
        let recs = [
            record("AAA", "1980/1/1", 7, 1),
            record("BBB", "1985/6/15", 7, 1),
            record("CCC", "1990/12/31", 9, 1),
            record("AAA", "1980/1/1", 7, 2),
            record("DDD", "1985/6/15", 7, 5),
        ];
        let pd: Vec<u8> = recs.concat();
        let pairs = [pair_rec("BBB", "AAA", 5), pair_rec("AAA", "ZZZ", 2)].concat();
        let cards = [
            card(10, 0, "A one"),
            card(11, 1, "B"),
            card(12, 2, "C"),
            card(13, 3, "A two"),
            card(14, 4, "D"),
        ];
        let a = Affinity::parse(E1011, &cards, &pd, &pairs, Some(&exe()));
        assert_eq!(a.counts(), (1, true)); // the pair with a player without a card is left out
        assert_eq!(a.level(10, 11), Some(5));
        assert_eq!(a.level(11, 13), Some(5)); // every card of the player
        assert_eq!(a.level(10, 13), None); // the same player
        assert_eq!(a.level(10, 99), None);
        // the default: base[type A][type C], other nationality
        let t = exe();
        let ka = kind(&t[0x6E_B21C..0x6E_B21C + 60], 1980, 1, 1);
        let kc = kind(&t[0x6E_B21C..0x6E_B21C + 60], 1990, 12, 31);
        let base = 1 + (usize::from(ka) + usize::from(kc)) % 5;
        assert_eq!(a.level(10, 12), Some(base as u8));
        // B and D: same birthday, same nationality (+1), D special (-1)
        let kb = usize::from(kind(&t[0x6E_B21C..0x6E_B21C + 60], 1985, 6, 15));
        assert_eq!(a.level(11, 14), Some(1 + ((kb * 2) % 5) as u8));
        let p: Vec<(&str, u8)> = a.partners(13).map(|(p, v)| (p.name.as_str(), v)).collect();
        assert_eq!(p, [("B", 5)]);
        assert_eq!(a.partners(11).next().unwrap().0.cards, [10, 13]);
        // no program, or one whose bytes are not the tables: listed pairs only
        let b = Affinity::parse(E1011, &cards, &pd, &pairs, Some(&[0xFF; 16]));
        assert_eq!((b.level(10, 11), b.level(10, 12)), (Some(5), None));
    }

    /// The day count of `sub_4E4050`: a day later is the next of the 60, the cycle repeats every 60 days, and March
    /// starts where February ends (1 March 2000 = 29 February 2000 + 1).
    #[test]
    fn birthday_types_follow_the_days() {
        let t: Vec<u8> = (0..60).collect();
        let k = |y, m, d| kind(&t, y, m, d);
        assert_eq!((k(2000, 1, 2) + 59) % 60, k(2000, 1, 1));
        assert_eq!(k(2000, 3, 1), (k(2000, 2, 29) + 1) % 60);
        assert_eq!(k(2001, 3, 1), (k(2001, 2, 28) + 1) % 60);
        assert_eq!(ymd(b"1979/10/2\0\0"), (1979, 10, 2));
        assert_eq!(ymd(b"x"), (0, 0, 0));
    }

    #[test]
    fn pair_order_is_i_below_j() {
        let mut k = 0;
        for i in 0..16 {
            for j in i + 1..16 {
                assert_eq!(pair(i, j), k);
                k += 1;
            }
        }
        assert_eq!(k, 120);
    }

    fn put(buf: &mut [u8], start: usize, width: usize, v: u32) {
        for b in 0..width {
            let i = start + b;
            let bit = (v >> (width - 1 - b)) & 1;
            buf[i >> 3] = (buf[i >> 3] & !(0x80 >> (i & 7))) | ((bit as u8) << (7 - (i & 7)));
        }
    }

    /// A synthetic card file: copy `which` good (stored = sum + 1), the other zeroed or broken.
    pub(crate) fn club_file(
        cards: &[u32; 16],
        links: &[(usize, usize, u8)],
        good_a: bool,
    ) -> Vec<u8> {
        let mut raw = vec![0u8; CARD_BYTES];
        let mut copy = vec![0u8; 0x690];
        for (i, &c) in cards.iter().enumerate() {
            put(&mut copy, PLAYER_BIT + 312 * i, 14, c);
        }
        for &(i, j, v) in links {
            put(&mut copy, LINK_BIT + 8 * pair(i, j), 8, u32::from(v));
        }
        let sum = (0..PAYLOAD).step_by(4).fold(0u32, |t, i| {
            t.wrapping_add(u32::from_le_bytes(copy[i..i + 4].try_into().unwrap()))
        });
        copy[PAYLOAD..PAYLOAD + 4].copy_from_slice(&sum.wrapping_add(1).to_le_bytes());
        raw[COPIES[1]..COPIES[1] + 0x690].copy_from_slice(&copy);
        if good_a {
            raw[COPIES[0]..COPIES[0] + 0x690].copy_from_slice(&copy);
        } else {
            copy[0] ^= 0x55; // a bad checksum
            raw[COPIES[0]..COPIES[0] + 0x690].copy_from_slice(&copy);
        }
        raw
    }

    #[test]
    fn club_links_from_the_copy_the_game_reads() {
        let mut cards = [0u32; 16];
        for (i, c) in cards.iter_mut().enumerate().take(15) {
            *c = 100 + i as u32 * 3; // slot 15 empty
        }
        cards[0] = 16383; // all 14 bits
        let links = [
            (0, 1, 255),
            (0, 15, 9),
            (3, 9, 150),
            (14, 15, 7),
            (13, 14, 201),
        ];
        for good_a in [true, false] {
            let c = ClubLinks::parse(&club_file(&cards, &links, good_a)).expect("a good copy");
            assert_eq!(c.cards, cards);
            assert_eq!(c.links[pair(0, 1)], 255);
            assert_eq!(c.links[pair(3, 9)], 150);
            assert_eq!(c.links[pair(13, 14)], 201);
            assert_eq!(c.links.iter().filter(|&&v| v > 0).count(), 5);
            let m = c.by_cards();
            assert_eq!(m.len(), 105); // pairs of the 15 filled slots
            assert_eq!(m[&(cards[1], cards[0])], 255);
            assert_eq!(m[&(cards[13], cards[14])], 201);
        }
        let mut new = club_file(&cards, &links, true);
        new[5] = 0xFF;
        new[6] = 0xFF;
        assert_eq!(ClubLinks::parse(&new), None);
        assert_eq!(ClubLinks::parse(&[0; 100]), None);
        let mut broken = club_file(&cards, &links, false);
        broken[COPIES[1]] ^= 0x40;
        assert_eq!(ClubLinks::parse(&broken), None);
    }

    /// Local only, against the player's own files (counts, never names): `FORZA_LINKS_GAME` the game folder (its
    /// client_Release.exe beside prog_data), `FORZA_LINKS_CATALOGUE` its catalogue.tsv, `FORZA_LINKS_CLUB` a club card
    /// file (optional). Every squad pair's link must stay under its affinity's growth ceiling (`sub_4FFCB0`), which a
    /// wrong pair order or slot mapping breaks.
    #[test]
    #[ignore = "needs the player's own files"]
    fn the_players_own_files() {
        let var = |n| std::path::PathBuf::from(std::env::var_os(n).expect(n));
        let game = var("FORZA_LINKS_GAME");
        let cat = crate::cards::load_catalogue(&var("FORZA_LINKS_CATALOGUE")).unwrap();
        let l = if fs::metadata(game.join("prog_data/player_data/player_data.bin"))
            .unwrap()
            .len()
            % 0x620
            == 0
        {
            E1718
        } else {
            E1011
        };
        let a = Affinity::load(l, &game, &game.join("client_Release.exe"), &cat).unwrap();
        let with = a.people.iter().filter(|p| !p.partners.is_empty()).count();
        println!(
            "catalogue {} cards, {} players, {} with partners; pairs, default rule: {:?}",
            cat.len(),
            a.people.len(),
            with,
            a.counts()
        );
        let Some(club) = std::env::var_os("FORZA_LINKS_CLUB") else {
            return;
        };
        let c = ClubLinks::read(Path::new(&club)).expect("a readable club card");
        let cap = |v: u8| [0, 50, 100, 150, 200, 253][usize::from(v)];
        let (mut over, mut stages) = (0, [0; 4]);
        for i in 0..16 {
            for j in i + 1..16 {
                let v = c.links[pair(i, j)];
                if let Some(af) = a.level(c.cards[i], c.cards[j])
                    && v > cap(af)
                {
                    over += 1;
                }
                stages[match v {
                    0..150 => 0,
                    150..200 => 1,
                    200..255 => 2,
                    255 => 3,
                }] += 1;
            }
        }
        println!(
            "squad {} cards; links by stage (none, thin, thick, gold) {stages:?}; over the ceiling {over}",
            c.cards.iter().filter(|&&n| n > 0).count()
        );
        assert_eq!(over, 0);
    }
}
