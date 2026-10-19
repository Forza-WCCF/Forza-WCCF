//! Every club card keeps its own cards on the table (port of `legacy/2010-11/code/cards/formation.py`). The table is
//! what the card board shows and FPR_Emu hands the game: fpr_table0.txt and the board's copy, fpr_panel_state.json, in
//! the game's seat folder. Each card FILE keeps its table beside it in a folder of the same name (seat1_club.board for
//! the slot, cards\NAME.board for NAME.bin), with id.txt, a random id; fpr_board_owner.txt in the seat folder names the
//! board that is out.
//! - at every start: the table out is copied back to its own board, then the slot card's board comes out; a card with
//!   none yet gets its registered players laid out as the board's "+ CARD" would (4-4-2, the rest on the bench). A table
//!   out that belongs to no board becomes the slot card's when its registered players are 11 of its cards or more (all,
//!   for fewer), else it is kept in data\save\boards_unknown - never lost
//! - at every stop the table out is copied back to its board
//! - no club in the slot: the table is copied back and stays out, owned by nobody
//!
//! Nothing is ever deleted.

use super::decode::{self as d, Decoded};
use super::view::{ascii_name, load_names};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const FILES: [&str; 2] = ["fpr_table0.txt", "fpr_panel_state.json"];
const OWNER: &str = "fpr_board_owner.txt";
const ID: &str = "id.txt";
const UNKNOWN: &str = "boards_unknown";
/// A card's registered players among a table's cards that make an unowned table its own.
const MATCH: usize = 11;
/// Where the board's "+ CARD" puts a card (x -1..1 touchline to touchline, y -1 the far goal line .. 1 the own one).
const SPOTS: [(&str, f64, f64); 11] = [
    ("GK", 0.0, 0.9),
    ("DF", -0.6, 0.55),
    ("DF", -0.2, 0.6),
    ("DF", 0.2, 0.6),
    ("DF", 0.6, 0.55),
    ("MF", -0.6, 0.1),
    ("MF", -0.2, 0.15),
    ("MF", 0.2, 0.15),
    ("MF", 0.6, 0.1),
    ("FW", -0.25, -0.45),
    ("FW", 0.25, -0.45),
];
const BENCH: [(f64, f64); 5] = [
    (1.24, -0.68),
    (1.24, -0.34),
    (1.24, 0.0),
    (1.24, 0.34),
    (1.24, 0.68),
];

/// A squad: its registered players (card number, position) in the card's order.
pub type Squad = Vec<(u32, String)>;
/// A card on the table: (slot, card, x, y).
pub type Placed = (i64, u32, f64, f64);

/// A card file's own board folder: NAME.bin -> NAME.board.
pub fn board_of(card: &Path) -> PathBuf {
    card.with_extension("board")
}

fn read_trim(p: &Path) -> Option<String> {
    let raw = fs::read(p).ok()?;
    let s: String = raw
        .iter()
        .map(|&b| {
            if b.is_ascii() {
                char::from(b)
            } else {
                '\u{FFFD}'
            }
        })
        .collect();
    Some(s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn board_id(folder: &Path) -> Option<String> {
    read_trim(&folder.join(ID))
}

/// {id: folder} of every card's board in data\save (the slot's and YOUR CARDS').
fn all_boards(save: &Path) -> HashMap<String, PathBuf> {
    let mut found = vec![save.join("seat1_club.board")];
    if let Ok(dir) = fs::read_dir(save.join("cards")) {
        found.extend(
            dir.filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .to_lowercase()
                        .ends_with(".board")
                })
                .map(|e| e.path()),
        );
    }
    found
        .into_iter()
        .filter(|f| f.is_dir())
        .filter_map(|f| Some((board_id(&f)?, f)))
        .collect()
}

/// (name, squad) of a card file, or Err(why): "none", "new", "unreadable", "refused".
pub fn club_of(
    card: &Path,
    names: &HashMap<u32, (String, String)>,
) -> Result<(String, Squad), &'static str> {
    let state = d::session(card, None).0;
    if matches!(state, "none" | "unreadable" | "new") {
        return Err(state);
    }
    let c = d::read_card(card).map_err(|_| "unreadable")?;
    if c.is_new() {
        return Err("new");
    }
    let chosen = d::game_choice(&c).2.ok_or("refused")?;
    let dd = Decoded::of(c.copy(Some(chosen)));
    let squad = (0..16)
        .map(|i| dd.get("Player", i, "PLAYER_CARD_NO")[0])
        .filter(|&no| no != 0)
        .map(|no| {
            let pos = names.get(&no).map_or("", |n| n.1.as_str());
            (
                no,
                if pos.is_empty() {
                    "?".into()
                } else {
                    pos.to_uppercase()
                },
            )
        })
        .collect();
    let name = ascii_name(&d::text(&dd.get("Club", 0, "CLUB_NAME")));
    Ok((
        if name.is_empty() {
            "the club".into()
        } else {
            name
        },
        squad,
    ))
}

/// The cards out on the table, as FPR_Emu reads them: "#" starts a note, commas or spaces.
pub fn table(seat: &Path) -> Vec<Placed> {
    let Ok(raw) = fs::read(seat.join(FILES[0])) else {
        return Vec::new();
    };
    let text: String = raw
        .iter()
        .map(|&b| {
            if b.is_ascii() {
                char::from(b)
            } else {
                '\u{FFFD}'
            }
        })
        .collect();
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").replace(',', " ");
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 4 {
            continue;
        }
        let (Ok(slot), Ok(no), Ok(x), Ok(y)) = (
            parts[0].parse::<i64>(),
            parts[1].parse::<i64>(),
            parts[2].parse::<f64>(),
            parts[3].parse::<f64>(),
        ) else {
            continue;
        };
        if (1..=20).contains(&slot) && no >= 1 {
            out.push((slot, u32::try_from(no).unwrap_or(u32::MAX), x, y));
        }
    }
    out
}

fn owner(seat: &Path) -> Option<String> {
    read_trim(&seat.join(OWNER))
}

fn boards_tmp(path: &Path) -> PathBuf {
    let mut t = path.as_os_str().to_owned();
    t.push(".boards");
    t.into()
}

/// A file whole, in one step, with a new write time (FPR_Emu and the board read again when it changes).
fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    fs::write(boards_tmp(path), data)?;
    fs::rename(boards_tmp(path), path)
}

fn copy_table(src: &Path, dst: &Path) -> io::Result<()> {
    for n in FILES {
        if src.join(n).exists() {
            write(&dst.join(n), &fs::read(src.join(n))?)?;
        }
    }
    Ok(())
}

/// path, or "path (2)", "path (3)" ... - a name nothing has yet.
fn free(path: &Path) -> PathBuf {
    let numbered = |n: u32| PathBuf::from(format!("{} ({n})", path.display()));
    if !path.exists() {
        return path.into();
    }
    (2..).map(numbered).find(|p| !p.exists()).unwrap()
}

fn set_owner(seat: &Path, key: Option<&str>) -> io::Result<()> {
    match key {
        Some(k) => write(&seat.join(OWNER), format!("{k}\n").as_bytes()),
        None if seat.join(OWNER).exists() => {
            fs::rename(seat.join(OWNER), seat.join(format!("{OWNER}.before")))
        } // never deleted
        None => Ok(()),
    }
}

/// The table out copied back to the board it came from, wherever its card is now -> that folder.
fn save_live(seat: &Path, save: &Path) -> io::Result<Option<PathBuf>> {
    let folder = all_boards(save).remove(&owner(seat).unwrap_or_default());
    match folder {
        Some(f) if seat.join(FILES[0]).exists() => {
            copy_table(seat, &f)?;
            Ok(Some(f))
        }
        _ => Ok(None),
    }
}

/// A new board for a card from the table out (a folder there without an id is kept aside first) -> its id.
fn make_board(folder: &Path, seat: &Path) -> io::Result<String> {
    if folder.is_dir() {
        let aside = free(&PathBuf::from(format!("{} old", folder.display())));
        fs::rename(folder, aside)?;
    }
    fs::create_dir_all(folder.parent().unwrap_or(Path::new(".")))?;
    fs::create_dir(folder)?;
    copy_table(seat, folder)?;
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|e| io::Error::other(e.to_string()))?;
    let key: String = b.iter().map(|x| format!("{x:02x}")).collect();
    write(&folder.join(ID), format!("{key}\n").as_bytes())?;
    Ok(key)
}

/// The table out kept in data\save\boards_unknown under the time (never over another) -> the folder.
fn put_aside(seat: &Path, save: &Path) -> io::Result<PathBuf> {
    let utc = time::OffsetDateTime::now_utc();
    let t = time::UtcOffset::local_offset_at(utc).map_or(utc, |o| utc.to_offset(o));
    let when = format!(
        "{:04}-{:02}-{:02} {:02}{:02}{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    );
    let folder = free(&save.join(UNKNOWN).join(when));
    fs::create_dir_all(&folder)?;
    copy_table(seat, &folder)?;
    Ok(folder)
}

/// A card's registered players are this table's cards: 11 of them or more (all of them, for a smaller table).
fn covers(live: &[Placed], squad: &Squad) -> bool {
    let cards: HashSet<u32> = live.iter().map(|c| c.1).collect();
    let n = squad
        .iter()
        .map(|s| s.0)
        .collect::<HashSet<_>>()
        .intersection(&cards)
        .count();
    n > 0 && n >= MATCH.min(cards.len())
}

/// The squad laid out as "+ CARD" would: each player on his own line's free spot (the card's order), the spots left
/// over filled by the players left over, the rest on the bench; slot = card order.
pub fn layout(squad: &Squad) -> Vec<Placed> {
    let mut spots = SPOTS.to_vec();
    let (mut placed, mut left) = (Vec::new(), Vec::new());
    for (i, (no, pos)) in squad.iter().take(16).enumerate() {
        match spots.iter().position(|sp| sp.0 == pos) {
            Some(k) => {
                let sp = spots.remove(k);
                placed.push((i as i64 + 1, *no, sp.1, sp.2));
            }
            None => left.push((i, *no)),
        }
    }
    spots.sort_by_key(|sp| {
        ["DF", "MF", "FW", "GK"]
            .iter()
            .position(|&p| p == sp.0)
            .unwrap_or(4)
    });
    let (mut spots, mut bench) = (spots.into_iter(), BENCH.into_iter());
    for (i, no) in left {
        if let Some(sp) = spots.next() {
            placed.push((i as i64 + 1, no, sp.1, sp.2));
        } else if let Some((x, y)) = bench.next() {
            placed.push((i as i64 + 1, no, x, y));
        }
    }
    placed.sort_by(|a, b| a.partial_cmp(b).unwrap());
    placed
}

/// Python's round(v, 3) for the board's JSON.
fn round3(v: f64) -> f64 {
    format!("{v:.3}").parse().unwrap()
}

/// The squad's own lay-out written as the table out (the table file and the board's copy, which agree).
fn lay_out(seat: &Path, name: &str, squad: &Squad) -> io::Result<usize> {
    let cards = layout(squad);
    let mut text = format!(
        "# laid out by the kit (formation.py) for {name} - read by FPR_Emu.exe (table 0)\n"
    );
    for (s, n, x, y) in &cards {
        text += &format!("{s} {n} {x:.3} {y:.3}\n");
    }
    write(&seat.join(FILES[0]), text.as_bytes())?;
    let placed: Vec<_> = cards
        .iter()
        .map(
            |&(s, n, x, y)| serde_json::json!({"slot": s, "no": n, "x": round3(x), "y": round3(y)}),
        )
        .collect();
    let state = serde_json::json!({"placed": placed, "mirror": false});
    let mut out = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
    serde::Serialize::serialize(
        &state,
        &mut serde_json::Serializer::with_formatter(&mut out, fmt),
    )
    .map_err(io::Error::other)?;
    out.push(b'\n');
    write(&seat.join(FILES[1]), &out)?;
    Ok(cards.len())
}

/// At a start, before anything reads the table: the slot card's cards out -> what it did, said (formation.sync).
pub fn sync(seat: &Path, card: &Path, catalogue: &Path) -> io::Result<Vec<String>> {
    let save = card.parent().unwrap_or(Path::new("."));
    let mut said = Vec::new();
    let saved = save_live(seat, save)?; // the table out goes back to its own board first
    let Ok((name, squad)) = club_of(card, &load_names(catalogue)) else {
        // no club in the slot: the table stays out, owned by nobody
        if owner(seat).is_some() {
            set_owner(seat, None)?;
            let kept = if saved.is_some() {
                " (and are kept with their card)"
            } else {
                ""
            };
            said.push(format!(
                "no club in the slot: the cards on the table stay out{kept}"
            ));
        }
        return Ok(said);
    };
    let folder = board_of(card);
    let key = board_id(&folder);
    let out = seat.join(FILES[0]).exists();
    if key.is_some() && owner(seat) == key && out {
        return Ok(said); // its own board is out (and was just copied back)
    }
    let live = table(seat);
    if out && saved.is_none() {
        // a table out that belongs to no board here
        if !live.is_empty() && key.is_none() && covers(&live, &squad) {
            set_owner(seat, Some(&make_board(&folder, seat)?))?;
            said.push(format!(
                "the cards on the table are {name}'s - kept with its card from now on"
            ));
            return Ok(said);
        }
        let aside = put_aside(seat, save)?;
        said.push(format!(
            "cards on the table that belong to no club card here kept in data\\save\\{UNKNOWN}\\{}",
            aside.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    if let Some(key) = key {
        copy_table(&folder, seat)?;
        set_owner(seat, Some(&key))?;
        said.push(format!("{name}'s own cards are on the table"));
    } else {
        let n = lay_out(seat, &name, &squad)?;
        set_owner(seat, Some(&make_board(&folder, seat)?))?;
        said.push(format!(
            "{name} had no cards kept yet: its {n} registered players laid out (4-4-2, the rest on the bench)"
        ));
    }
    Ok(said)
}

/// At a stop: the table out copied back to its board -> the folder.
pub fn keep(seat: &Path, save: &Path) -> io::Result<Option<PathBuf>> {
    save_live(seat, save)
}

#[cfg(test)]
mod tests {
    use super::super::decode::tests::{card_with, set};
    use super::*;

    fn squad(pos: &[&str]) -> Squad {
        pos.iter()
            .enumerate()
            .map(|(i, p)| (100 + i as u32, (*p).to_owned()))
            .collect()
    }

    #[test]
    fn a_squad_laid_out_as_plus_card_would() {
        let s = squad(&[
            "GK", "DF", "DF", "DF", "DF", "DF", "MF", "MF", "FW", "FW", "FW", "GK", "MF",
        ]);
        let l = layout(&s);
        assert_eq!(l.len(), 13);
        assert_eq!(l[0], (1, 100, 0.0, 0.9));
        assert_eq!(
            l[5],
            (6, 105, 0.6, 0.1),
            "the fifth DF takes the spot left over"
        );
        assert_eq!(
            l[10],
            (11, 110, 1.24, -0.68),
            "the third FW: no spot left, the bench"
        );
        assert_eq!(l[11], (12, 111, 1.24, -0.34), "the second GK too");
        assert_eq!(
            l[12],
            (13, 112, 0.2, 0.15),
            "a MF placed on his own line in card order"
        );
        assert_eq!(round3(-0.25), -0.25);
    }

    #[test]
    fn sync_lays_out_keeps_and_follows_the_slot() {
        let tmp = tempfile::tempdir().unwrap();
        let (seat, save) = (tmp.path().join("seat"), tmp.path().join("save"));
        fs::create_dir_all(&seat).unwrap();
        fs::create_dir_all(save.join("cards")).unwrap();
        let card = save.join("seat1_club.bin");
        let none = tmp.path().join("none.tsv");
        assert!(
            sync(&seat, &card, &none).unwrap().is_empty(),
            "no club, no owner: nothing said"
        );
        let mut copy = vec![0u8; d::COPY_BYTES];
        set(&mut copy, "Club", 0, "CLUB_NAME", &b"ROSSO".map(u32::from));
        for i in 0..3 {
            set(&mut copy, "Player", i, "PLAYER_CARD_NO", &[200 + i as u32]);
        }
        fs::write(&card, card_with(0xFFF0, &copy)).unwrap();
        let said = sync(&seat, &card, &none).unwrap();
        assert_eq!(
            said,
            [
                "ROSSO had no cards kept yet: its 3 registered players laid out (4-4-2, the rest on the bench)"
            ]
        );
        let t = fs::read_to_string(seat.join(FILES[0])).unwrap();
        assert!(t.starts_with("# laid out by the kit (formation.py) for ROSSO"));
        assert!(
            t.ends_with("1 200 -0.600 0.550\n2 201 -0.200 0.600\n3 202 0.200 0.600\n"),
            "no positions known: DF spots first; {t}"
        );
        let j = fs::read_to_string(seat.join(FILES[1])).unwrap();
        assert!(j.starts_with("{\n \"placed\": [\n  {\n   \"slot\": 1,\n   \"no\": 200,\n   \"x\": -0.6,\n   \"y\": 0.55\n  },"), "{j}");
        assert!(j.ends_with("\n ],\n \"mirror\": false\n}\n"), "{j}");
        let key = board_id(&board_of(&card)).unwrap();
        assert_eq!(owner(&seat).as_deref(), Some(key.as_str()));
        assert_eq!(key.len(), 16);
        // the player moves a card; a stop keeps it with the club, the next start says nothing
        fs::write(seat.join(FILES[0]), "1 200 0.1 0.9\n").unwrap();
        assert_eq!(keep(&seat, &save).unwrap(), Some(board_of(&card)));
        assert_eq!(
            fs::read_to_string(board_of(&card).join(FILES[0])).unwrap(),
            "1 200 0.1 0.9\n"
        );
        assert!(sync(&seat, &card, &none).unwrap().is_empty());
        // a table nobody owns, not this club's: put aside
        fs::write(seat.join(OWNER), "someone\n").unwrap();
        fs::write(seat.join(FILES[0]), "1 999 0 0\n").unwrap();
        let said = sync(&seat, &card, &none).unwrap();
        assert!(said[0].starts_with("cards on the table that belong to no club card here kept in data\\save\\boards_unknown\\"));
        assert_eq!(said[1], "ROSSO's own cards are on the table");
        assert_eq!(
            fs::read_to_string(seat.join(FILES[0])).unwrap(),
            "1 200 0.1 0.9\n"
        );
        // the card leaves the slot: the table stays out, owned by nobody (the owner file kept aside)
        fs::rename(&card, save.join("cards/ROSSO.bin")).unwrap();
        fs::rename(board_of(&card), save.join("cards/ROSSO.board")).unwrap();
        assert_eq!(
            sync(&seat, &card, &none).unwrap(),
            ["no club in the slot: the cards on the table stay out (and are kept with their card)"]
        );
        assert!(seat.join("fpr_board_owner.txt.before").exists() && !seat.join(OWNER).exists());
    }

    #[test]
    fn a_table_of_the_clubs_own_players_becomes_its_board() {
        let s = squad(&["GK"; 12]);
        let live: Vec<Placed> = (0..11).map(|i| (i + 1, 100 + i as u32, 0.0, 0.0)).collect();
        assert!(covers(&live, &s));
        assert!(!covers(
            &live[..1]
                .iter()
                .map(|c| (c.0, 5, c.2, c.3))
                .collect::<Vec<_>>(),
            &s
        ));
        assert!(covers(&live[..3], &s), "all of a smaller table");
        assert_eq!(table(Path::new("/nonexistent")), Vec::<Placed>::new());
    }
}
