//! The real player stats from the high-quality pack's `cards.db`, kept in the kit's cache beside the pictures
//! (`hq_stats.tsv`, one line per game card number) so the board shows them without opening a 28 MiB database.
//! Made when the pack is chosen ([`super::pictures::ensure`]); never part of the repository.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

/// The file's name in the pictures folder.
pub const FILE: &str = "hq_stats.tsv";

/// One card's printed values: position (GK DF MF FW), OFF DEF TEC POW SPD STA, TOTAL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hq {
    pub pos: [u8; 2],
    pub stats: [u8; 6],
    pub total: u8,
}

/// The pack's row per game card number, in the same order as the pictures' pick (name+stats link first).
const PICK: &str =
    "SELECT l.game_card_no, c.position, c.off, c.def, c.tec, c.pow, c.spe, c.sta, c.tot
    FROM game_link l JOIN cards c USING (ver, card)
    WHERE l.game = ?1 AND c.off IS NOT NULL AND c.tot IS NOT NULL
    ORDER BY l.game_card_no, l.method != 'name+stats', c.card LIKE '%EXTRA%', c.ver, c.card";

/// Writes `out`\hq_stats.tsv from the pack's database file for `edition` ("1011" or "1718"); returns the card count.
pub fn from_db(db: &Path, edition: &str, out: &Path) -> io::Result<usize> {
    let con = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(io::Error::other)?;
    let mut q = con.prepare(PICK).map_err(io::Error::other)?;
    let rows = q
        .query_map([edition], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1).unwrap_or_default(),
                [2, 3, 4, 5, 6, 7, 8].map(|i| r.get::<_, i64>(i).unwrap_or(0)),
            ))
        })
        .map_err(io::Error::other)?;
    let (mut text, mut n, mut last) = (String::new(), 0, None);
    for row in rows {
        let (no, pos, v) = row.map_err(io::Error::other)?;
        if last.replace(no) == Some(no) {
            continue; // a reprint: the first one wins
        }
        text.push_str(&format!(
            "{no}\t{pos}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            v[0], v[1], v[2], v[3], v[4], v[5], v[6]
        ));
        n += 1;
    }
    let tmp = out.join(format!("{FILE}.tmp"));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, out.join(FILE))?;
    Ok(n)
}

/// The cache file -> values by card number; empty when it is missing (no pack chosen).
pub fn load(path: &Path) -> HashMap<u32, Hq> {
    let Ok(text) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let n = |i: usize| f.get(i)?.parse::<u8>().ok();
            let p = f.get(1)?.as_bytes();
            Some((
                f.first()?.parse().ok()?,
                Hq {
                    pos: [*p.first()?, *p.get(1)?],
                    stats: [n(2)?, n(3)?, n(4)?, n(5)?, n(6)?, n(7)?],
                    total: n(8)?,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny synthetic cards.db: two printings of one game card (the name+stats one wins), one card without stats.
    #[test]
    fn the_stats_come_from_the_linked_card() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("cards.db");
        let con = rusqlite::Connection::open(&db).unwrap();
        con.execute_batch(
            "CREATE TABLE cards (ver INTEGER, card TEXT, position TEXT, off INTEGER, def INTEGER, tec INTEGER,
                                 pow INTEGER, spe INTEGER, sta INTEGER, tot INTEGER);
             CREATE TABLE game_link (ver INTEGER, card TEXT, game TEXT, game_card_no INTEGER, method TEXT);
             INSERT INTO cards VALUES (29, 'A01', 'MF', 1, 2, 3, 4, 5, 6, 21), (28, '240', 'FW', 19, 6, 17, 13, 18, 15, 88),
                                      (13, 'X', 'GK', NULL, NULL, NULL, NULL, NULL, NULL, NULL);
             INSERT INTO game_link VALUES (29, 'A01', '1718', 7049, 'name+season'), (28, '240', '1718', 7049, 'name+stats'),
                                          (13, 'X', '1718', 5, 'name+stats'), (28, '240', '1011', 9, 'name+stats');",
        )
        .unwrap();
        drop(con);
        assert_eq!(from_db(&db, "1718", d.path()).unwrap(), 1);
        let m = load(&d.path().join(FILE));
        assert_eq!(
            m.get(&7049),
            Some(&Hq {
                pos: *b"FW",
                stats: [19, 6, 17, 13, 18, 15],
                total: 88
            })
        );
        assert!(!m.contains_key(&5) && !m.contains_key(&9));
        assert!(load(&d.path().join("none.tsv")).is_empty());
    }
}
