//! Saved formations (issue #89): ten named slots in `data\save\formations\` (`slot01.txt` .. `slot10.txt`), shared by
//! every club card. A slot is the site's lineup text ([`lineup::export_exact`]) plus a `# club NAME` comment naming the
//! club it was saved from, so loading one is an IMPORT LINEUP of its text, and the file pastes into db.wccf.it as it is.

use super::lineup::{self, Lineup};
use super::{BENCH_X, BENCH_Y, Placed};
use crate::Edition;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const SLOTS: usize = 10;
/// The longest name or club kept, in characters.
const MAX_NAME: usize = 40;
const CLUB: &str = "# club ";

/// A filled slot: its name, the club it was saved from ("" unknown), and the file's text.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub name: String,
    pub club: String,
    pub text: String,
}

/// The slots' folder in `data\save`.
pub fn dir(save: &Path) -> PathBuf {
    save.join("formations")
}

fn file(dir: &Path, i: usize) -> PathBuf {
    dir.join(format!("slot{:02}.txt", i + 1))
}

/// One line, at most [`MAX_NAME`] characters.
fn clean(s: &str) -> String {
    s.split(['\r', '\n'])
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(MAX_NAME)
        .collect()
}

/// A slot's file text: the table as [`lineup::export_exact`] writes it, its club in a comment after the header.
pub fn text(placed: &[Placed], edition: Edition, name: &str, club: &str) -> String {
    let t = lineup::export_exact(placed, edition, &clean(name));
    let club = clean(club);
    if club.is_empty() {
        return t;
    }
    match t.split_once('\n') {
        Some((head, rest)) => format!("{head}\n{CLUB}{club}\n{rest}"),
        None => t,
    }
}

/// A slot file's text read back: `None` when it holds no lineup for this edition.
pub fn slot_of(text: &str, edition: Edition) -> Option<Slot> {
    let l = lineup::parse(text, edition).ok()?;
    let club = text
        .lines()
        .find_map(|l| l.trim().strip_prefix(CLUB))
        .map(clean)
        .unwrap_or_default();
    Some(Slot {
        name: l.name.unwrap_or_default(),
        club,
        text: text.to_owned(),
    })
}

/// The ten slots; an empty, missing or unreadable file is an empty slot.
pub fn read(dir: &Path, edition: Edition) -> Vec<Option<Slot>> {
    (0..SLOTS)
        .map(|i| {
            let t = fs::read_to_string(file(dir, i)).ok()?;
            slot_of(&t, edition)
        })
        .collect()
}

/// Writes slot `i` (0-9): a temporary file, then renamed over the old one, so a failed write keeps the old slot.
pub fn write(dir: &Path, i: usize, text: &str) -> io::Result<()> {
    if i >= SLOTS {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no such slot"));
    }
    fs::create_dir_all(dir)?;
    let f = file(dir, i);
    let tmp = f.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, &f)
}

/// A slot's text as the site's export writes it (whole percents, no club comment): COPY. `None`: not a lineup.
pub fn site_text(text: &str, edition: Edition) -> Option<String> {
    let l = lineup::parse(text, edition).ok()?;
    Some(lineup::export(
        &placed(&l),
        edition,
        l.name.as_deref().unwrap_or(""),
    ))
}

/// A lineup's cards as a table with no catalogue: pitch entries at their spots (the middle when the text gave none),
/// the bench on its band boxes in order.
fn placed(l: &Lineup) -> Vec<Placed> {
    let pitch = l.pitch.iter().map(|e| (e.no, e.at.unwrap_or((0.0, 0.0))));
    let bench = l
        .bench
        .iter()
        .zip(BENCH_Y)
        .map(|(e, y)| (e.no, (BENCH_X, y)));
    pitch
        .chain(bench)
        .enumerate()
        .map(|(i, (no, (x, y)))| Placed {
            slot: i as i32 + 1,
            no,
            x,
            y,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Card;

    /// A full table as the panel keeps it: 11 on the pitch (not in slot order), 5 on the bench by band.
    fn sixteen() -> Vec<Placed> {
        let mut t: Vec<Placed> = (0..11)
            .map(|i| Placed {
                slot: 11 - i,
                no: 100 + i as u32,
                x: [
                    -0.6, -0.2, 0.2, 0.6, 0.0, -0.613, 0.247, 0.999, -1.0, 0.123, -0.25,
                ][i as usize],
                y: [
                    0.55, 0.6, 0.6, 0.55, 0.9, 0.1, 0.153, -0.45, 1.0, -0.871, -0.45,
                ][i as usize],
            })
            .collect();
        for (b, y) in [3, 0, 4, 1, 2].into_iter().map(|b| (b, BENCH_Y[b])) {
            t.push(Placed {
                slot: 12 + b as i32,
                no: 200 + b as u32,
                x: BENCH_X,
                y,
            });
        }
        t
    }

    fn cat(t: &[Placed]) -> Vec<Card> {
        t.iter().map(|p| super::super::unknown(p.no, 0.0)).collect()
    }

    #[test]
    fn board_to_slot_file_to_board_keeps_all_sixteen_and_their_spots() {
        let dir = tempfile::tempdir().unwrap();
        let old = sixteen();
        let c = cat(&old);
        write(
            dir.path(),
            3,
            &text(&old, Edition::E1718, "Attack\nx", "FC ROMA"),
        )
        .unwrap();
        let slots = read(dir.path(), Edition::E1718);
        assert_eq!(slots.iter().filter(|s| s.is_some()).count(), 1);
        let s = slots[3].clone().unwrap();
        assert_eq!((s.name.as_str(), s.club.as_str()), ("Attack", "FC ROMA"));
        let imp = lineup::table(&lineup::parse(&s.text, Edition::E1718).unwrap(), &c).unwrap();
        assert!(imp.skipped.is_empty());
        let (a, b) = (
            super::super::board(&old, &c),
            super::super::board(&imp.placed, &c),
        );
        for i in 0..16 {
            assert_eq!(a.number(i), b.number(i), "slot {i}");
        }
        assert_eq!(a.spots, b.spots);
        for p in &old {
            let q = imp.placed.iter().find(|q| q.no == p.no).unwrap();
            assert_eq!((p.x, p.y), (q.x, q.y), "card {}", p.no);
        }
    }

    #[test]
    fn copy_text_parses_to_the_same_lineup() {
        let old = sixteen();
        let saved = text(&old, Edition::E1718, "Attack", "FC ROMA");
        let copy = site_text(&saved, Edition::E1718).unwrap();
        assert!(copy.starts_with("# WCCF DB lineup\ngame 1718\nname Attack\nP 100 "));
        assert!(!copy.contains("club"));
        let (a, b) = (
            lineup::parse(&saved, Edition::E1718).unwrap(),
            lineup::parse(&copy, Edition::E1718).unwrap(),
        );
        let nos = |l: &Lineup| {
            (
                l.pitch.iter().map(|e| e.no).collect::<Vec<_>>(),
                l.bench.iter().map(|e| e.no).collect::<Vec<_>>(),
            )
        };
        assert_eq!(nos(&a), nos(&b));
        assert_eq!(
            b.bench.iter().map(|e| e.no).collect::<Vec<_>>(),
            [200, 201, 202, 203, 204]
        );
        // whole percents: within half a percent (0.01 of the table) of the exact spots
        for (x, y) in a.pitch.iter().zip(&b.pitch) {
            let ((ax, ay), (bx, by)) = (x.at.unwrap(), y.at.unwrap());
            assert!((ax - bx).abs() <= 0.0101 && (ay - by).abs() <= 0.0101);
        }
        assert_eq!(copy, lineup::export(&placed(&b), Edition::E1718, "Attack"));
    }

    #[test]
    fn empty_and_foreign_files_are_empty_slots() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(file(dir.path(), 0), "").unwrap();
        fs::write(file(dir.path(), 1), "game 1011\nP 5 50 50\n").unwrap();
        fs::write(file(dir.path(), 2), "P 5 50 50\n").unwrap();
        let s = read(dir.path(), Edition::E1718);
        assert_eq!(s.len(), SLOTS);
        assert!(s[0].is_none() && s[1].is_none());
        assert_eq!(
            s[2].as_ref().map(|s| (s.name.as_str(), s.club.as_str())),
            Some(("", ""))
        );
        assert!(write(dir.path(), SLOTS, "x").is_err());
    }
}
