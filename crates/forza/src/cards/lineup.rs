//! The db.wccf.it lineup text (the site's board Export/Import, `board.js` `exportText`/`importText`) <-> the card table.
//! Pure: no files, no clipboard, no strings for the player. [`parse`] reads the text, [`table`] turns the result into the
//! cards for the table with the edition's catalogue, [`export`] writes a table back as the site's text.
//!
//! ```text
//! # WCCF DB lineup
//! game 1718                 0203 | 0506 | 1011 | 1718
//! name My team
//! P 1234 50 88 K            starter: card number, x y in percent (y 0 = opponent goal), optional K = key player
//! S 2345                    substitute, in bench order
//! S "13/GES 3 "             a card with no number in that game: the site's ver/card, quoted when it has spaces
//! ```
//! A bare list of numbers (first 11 start, the next 5 sit on the bench) is read too. Numbers are the game's own card
//! numbers (the catalogue's `card_no`); `ver/card` refs have none and are skipped.

use super::{BENCH_X, BENCH_Y, MAX_CARDS, Placed, add, round3};
use crate::Edition;
use crate::board::Card;
use regex::Regex;
use std::sync::LazyLock;

const PITCH: usize = 11;
const BENCH: usize = MAX_CARDS - PITCH;

/// One card of the lineup: its game card number, where the text puts it, and the text to name it by when it is skipped.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub no: u32,
    /// Table coordinates (x -1..1 left to right, y -1 far goal .. 1 own goal); `None` = the text gave none (a bare list,
    /// or a short line), so [`table`] takes the spot [`add`] gives that card's position. Always `None` on the bench.
    pub at: Option<(f32, f32)>,
    /// The line's comment (the site writes `SHORT · version card`), else the ref.
    pub label: String,
}

/// An entry that did not come through, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Skipped {
    pub label: String,
    pub why: Skip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// A `ver/card` ref: the card has no number in this game.
    NoGameNumber,
    /// A line that is not a lineup entry.
    BadLine,
    /// The same card, or another card of the same player, was already in.
    Duplicate,
    /// A number this edition's catalogue does not have.
    NotInCatalogue,
    /// More than 11 on the pitch and 5 on the bench.
    NoRoom,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lineup {
    /// The `name` line, if any.
    pub name: Option<String>,
    /// At most 11, in file order.
    pub pitch: Vec<Entry>,
    /// At most 5, in bench order (subs and starters past the 11th, in file order).
    pub bench: Vec<Entry>,
    /// The card number of the starter marked `K`, as given; the table has no key player.
    pub kp: Option<u32>,
    pub skipped: Vec<Skipped>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineupError {
    /// The lineup's `game` (as written) is not this edition's.
    WrongEdition(String),
    /// Nothing to import.
    Empty,
}

/// What [`table`] makes: the new table, with what to tell the player.
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub placed: Vec<Placed>,
    pub name: Option<String>,
    /// The key player's card number if that card is on the table.
    pub kp: Option<u32>,
    pub skipped: Vec<Skipped>,
}

fn game(edition: Edition) -> &'static str {
    match edition {
        Edition::E1011 => "1011",
        Edition::E1718 => "1718",
    }
}

/// The site's tokens: a quoted string, a `#` comment to the end, or a run of anything but space and comma.
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r##""(?:[^"\\]|\\.)*"|#.*|[^\s,]+"##).unwrap());

enum Tok {
    Word(String),
    Quoted(String),
}

impl Tok {
    fn text(&self) -> &str {
        match self {
            Tok::Word(s) | Tok::Quoted(s) => s,
        }
    }
}

/// A line's tokens before the comment, and the comment's text.
fn tokens(line: &str) -> (Vec<Tok>, String) {
    let mut out = Vec::new();
    for m in TOKEN.find_iter(line) {
        let s = m.as_str();
        if let Some(c) = s.strip_prefix('#') {
            return (out, c.trim().to_string());
        }
        out.push(match s.strip_prefix('"') {
            // shortcut: only \" and \\ are unescaped; the site's refs hold no other escapes
            Some(q) => Tok::Quoted(
                q.strip_suffix('"')
                    .unwrap_or(q)
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\"),
            ),
            None => Tok::Word(s.to_string()),
        });
    }
    (out, String::new())
}

/// A ref token as a game card number: digits, 1 or more.
fn number(t: &Tok) -> Option<u32> {
    match t {
        Tok::Word(w) if w.bytes().all(|b| b.is_ascii_digit()) => w.parse().ok().filter(|&n| n >= 1),
        _ => None,
    }
}

/// Percent on the site's board -> a table coordinate (inverse of `spot_of`).
fn from_percent(v: f32) -> f32 {
    round3((v / 50.0 - 1.0).clamp(-1.0, 1.0))
}

/// The lineup text -> its cards. Fails when a `game` line names another edition, or no entry is left to import
/// (entries that fail are in `skipped`). Entries are read as the site's import does, so a bare list works and a
/// starter line with no valid x y is auto-placed.
pub fn parse(text: &str, edition: Edition) -> Result<Lineup, LineupError> {
    let lines: Vec<(String, Vec<Tok>, String)> = text
        .lines()
        .map(|l| {
            let (t, c) = tokens(l);
            (l.trim().to_string(), t, c)
        })
        .filter(|(_, t, _)| !t.is_empty())
        .collect();
    for (_, t, _) in &lines {
        if t[0].text() == "game" {
            let g = t.get(1).map_or("", Tok::text);
            if g != game(edition) {
                return Err(LineupError::WrongEdition(g.to_string()));
            }
        }
    }
    let keyed = |t: &[Tok]| matches!(t[0].text(), "P" | "S" | "game" | "name");
    let keyed_text = lines.iter().any(|(_, t, _)| keyed(t));
    let mut l = Lineup::default();
    let mut seen = Vec::new();
    // (kind, ref, x y, K, comment)
    let mut entries = Vec::new();
    for (raw, t, c) in &lines {
        if !keyed_text {
            entries.extend(t.iter().map(|r| ('P', r, None, false, c)));
        } else if t[0].text() == "name" {
            let n = raw.strip_prefix("name").unwrap_or("").trim();
            if !n.is_empty() {
                l.name = Some(n.to_string());
            }
        } else if t[0].text() == "game" {
        } else if matches!(t[0].text(), "P" | "S") && t.len() > 1 {
            let xy = |i: usize| {
                t.get(i)
                    .and_then(|v| v.text().parse::<f32>().ok())
                    .filter(|v| v.is_finite())
            };
            let at = xy(2)
                .zip(xy(3))
                .map(|(x, y)| (from_percent(x), from_percent(y)));
            let kp = t[0].text() == "P" && t.get(4).is_some_and(|k| k.text() == "K");
            entries.push((t[0].text().chars().next().unwrap_or('P'), &t[1], at, kp, c));
        } else {
            l.skipped.push(Skipped {
                label: raw.clone(),
                why: Skip::BadLine,
            });
        }
    }
    // As the site's `addTo`: a starter past the 11th takes the next free bench box, so a bare list needs no split.
    for (kind, r, at, kp, c) in entries {
        let label = if c.is_empty() {
            r.text().to_string()
        } else {
            c.clone()
        };
        let Some(no) = number(r) else {
            let why = if matches!(r, Tok::Quoted(_)) || r.text().contains('/') {
                Skip::NoGameNumber
            } else {
                Skip::BadLine
            };
            l.skipped.push(Skipped { label, why });
            continue;
        };
        if kp && l.kp.is_none() {
            l.kp = Some(no);
        }
        if seen.contains(&no) {
            l.skipped.push(Skipped {
                label,
                why: Skip::Duplicate,
            });
            continue;
        }
        seen.push(no);
        let e = Entry {
            no,
            at: if kind == 'P' { at } else { None },
            label,
        };
        if kind == 'S' {
            if l.bench.len() < BENCH {
                l.bench.push(e);
            } else {
                l.skipped.push(Skipped {
                    label: e.label,
                    why: Skip::NoRoom,
                });
            }
        } else if l.pitch.len() < PITCH {
            l.pitch.push(e);
        } else if l.bench.len() < BENCH {
            l.bench.push(Entry { at: None, ..e });
        } else {
            l.skipped.push(Skipped {
                label: e.label,
                why: Skip::NoRoom,
            });
        }
    }
    if l.pitch.is_empty() && l.bench.is_empty() {
        return Err(LineupError::Empty);
    }
    Ok(l)
}

/// The lineup as the table the panel writes: pitch cards at their spots (auto-placed ones where [`add`] puts them), the
/// bench on its band boxes in order, slots 1.. in that order. Cards the catalogue lacks, and a second card of a player
/// already in, are skipped after the lineup's own skips. `Err(Empty)` when no card is left.
pub fn table(l: &Lineup, catalogue: &[Card]) -> Result<Import, LineupError> {
    let mut placed: Vec<Placed> = Vec::new();
    let mut taken: Vec<&Card> = Vec::new();
    let mut skipped = l.skipped.clone();
    let mut benched = 0;
    for (e, bench) in l
        .pitch
        .iter()
        .map(|e| (e, false))
        .chain(l.bench.iter().map(|e| (e, true)))
    {
        let skip = |why| Skipped {
            label: e.label.clone(),
            why,
        };
        let Some(card) = catalogue.iter().find(|c| c.number == e.no) else {
            skipped.push(skip(Skip::NotInCatalogue));
            continue;
        };
        if taken.iter().any(|c| c.same_player(card)) {
            skipped.push(skip(Skip::Duplicate));
            continue;
        }
        let slot = (1..=MAX_CARDS as i32)
            .find(|s| !placed.iter().any(|p| p.slot == *s))
            .unwrap_or(1);
        if bench {
            let Some(&y) = BENCH_Y.get(benched) else {
                skipped.push(skip(Skip::NoRoom));
                continue;
            };
            placed.push(Placed {
                slot,
                no: e.no,
                x: BENCH_X,
                y,
            });
            benched += 1;
        } else if placed.iter().filter(|p| !p.on_bench()).count() >= PITCH {
            skipped.push(skip(Skip::NoRoom));
            continue;
        } else if let Some((x, y)) = e.at {
            placed.push(Placed {
                slot,
                no: e.no,
                x,
                y,
            });
        } else if add(&mut placed, e.no, card.pos).is_err() {
            skipped.push(skip(Skip::NoRoom));
            continue;
        }
        taken.push(card);
    }
    if placed.is_empty() {
        return Err(LineupError::Empty);
    }
    Ok(Import {
        kp: l.kp.filter(|k| placed.iter().any(|p| p.no == *k)),
        name: l.name.clone(),
        placed,
        skipped,
    })
}

/// The table as the site's text: `game`, `name` (left out when empty), `P no x y` (percent, whole numbers, the first 11
/// on the pitch in slot order) and `S no` (the bench by band, the first 5). No key player and no comments: the table
/// has neither.
pub fn export(placed: &[Placed], edition: Edition, name: &str) -> String {
    let mut pitch: Vec<&Placed> = placed.iter().filter(|p| !p.on_bench()).collect();
    pitch.sort_by_key(|p| p.slot);
    let mut bench: Vec<&Placed> = placed.iter().filter(|p| p.on_bench()).collect();
    bench.sort_by_key(|p| (super::bench_band(p.y), p.slot));
    let pct = |v: f32| ((v + 1.0) * 50.0).round().clamp(0.0, 100.0);
    let mut t = format!("# WCCF DB lineup\ngame {}\n", game(edition));
    if !name.is_empty() {
        t += &format!("name {name}\n");
    }
    for p in pitch.iter().take(PITCH) {
        t += &format!("P {} {} {}\n", p.no, pct(p.x), pct(p.y));
    }
    for p in bench.iter().take(BENCH) {
        t += &format!("S {}\n", p.no);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Position;

    const SITE: &str = "# WCCF DB lineup\ngame 1718\nname My team\nP 1234 50 88 K\nP 2001 30 70\nS 2345\nS \"13/GES 3 \"\n";

    fn cat(nos: &[u32]) -> Vec<Card> {
        nos.iter().map(|&n| super::super::unknown(n, 0.0)).collect()
    }

    fn why(l: &Lineup) -> Vec<(&str, Skip)> {
        l.skipped
            .iter()
            .map(|s| (s.label.as_str(), s.why))
            .collect()
    }

    #[test]
    fn the_sites_sample_reads_and_comes_back() {
        let l = parse(SITE, Edition::E1718).unwrap();
        assert_eq!(l.name.as_deref(), Some("My team"));
        assert_eq!(l.kp, Some(1234));
        let at = |e: &Entry| (e.no, e.at);
        assert_eq!(
            l.pitch.iter().map(at).collect::<Vec<_>>(),
            [(1234, Some((0.0, 0.76))), (2001, Some((-0.4, 0.4)))]
        );
        assert_eq!(l.bench.iter().map(|e| e.no).collect::<Vec<_>>(), [2345]);
        assert_eq!(why(&l), [("13/GES 3 ", Skip::NoGameNumber)]);
        let i = table(&l, &cat(&[1234, 2001, 2345])).unwrap();
        assert_eq!(
            i.placed,
            [
                Placed {
                    slot: 1,
                    no: 1234,
                    x: 0.0,
                    y: 0.76
                },
                Placed {
                    slot: 2,
                    no: 2001,
                    x: -0.4,
                    y: 0.4
                },
                Placed {
                    slot: 3,
                    no: 2345,
                    x: BENCH_X,
                    y: BENCH_Y[0]
                },
            ]
        );
        assert_eq!((i.kp, i.name.as_deref()), (Some(1234), Some("My team")));
        // the same text back, minus the key player and the comments the table cannot know
        let back = export(&i.placed, Edition::E1718, "My team");
        assert_eq!(
            back,
            "# WCCF DB lineup\ngame 1718\nname My team\nP 1234 50 88\nP 2001 30 70\nS 2345\n"
        );
        let again = parse(&back, Edition::E1718).unwrap();
        assert_eq!(
            again.pitch,
            l.pitch
                .iter()
                .map(|e| Entry {
                    label: e.no.to_string(),
                    ..e.clone()
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_ver_card_ref_is_skipped_under_its_comment() {
        let t = "game 1011\nS \"13/GES 3 \"   # GESSNER · 13 GES 3\nP 13/A01 50 50 # ALPHA · 13 A01\nP 7 50 90\n";
        let l = parse(t, Edition::E1011).unwrap();
        assert_eq!(
            why(&l),
            [
                ("GESSNER · 13 GES 3", Skip::NoGameNumber),
                ("ALPHA · 13 A01", Skip::NoGameNumber)
            ]
        );
        assert_eq!(l.pitch.len(), 1);
    }

    #[test]
    fn another_edition_is_refused() {
        for g in ["0203", "0506", "1011", "9999"] {
            assert_eq!(
                parse(&format!("game {g}\nP 5 50 50\n"), Edition::E1718),
                Err(LineupError::WrongEdition(g.into()))
            );
        }
        assert!(parse("game 1718\nP 5 50 50\n", Edition::E1718).is_ok());
        // a game line after the cards counts too
        assert_eq!(
            parse("P 5 50 50\ngame 0506\n", Edition::E1718),
            Err(LineupError::WrongEdition("0506".into()))
        );
    }

    #[test]
    fn a_bare_list_starts_eleven_and_benches_five_auto_placed() {
        let l = parse(
            "1 2 3 4, 5 6 7 8 9 10 11\n12,13 14\r\n15 16 17 18",
            Edition::E1011,
        )
        .unwrap();
        assert_eq!((l.pitch.len(), l.bench.len()), (11, 5));
        assert_eq!(l.pitch.iter().filter(|e| e.at.is_some()).count(), 0);
        assert_eq!(why(&l), [("17", Skip::NoRoom), ("18", Skip::NoRoom)]);
        let mut c = cat(&(1..=18).collect::<Vec<_>>());
        for (i, k) in c.iter_mut().enumerate() {
            k.pos = [
                Position::Gk,
                Position::Df,
                Position::Df,
                Position::Df,
                Position::Df,
                Position::Mf,
                Position::Mf,
                Position::Mf,
                Position::Mf,
                Position::Fw,
                Position::Fw,
            ][i.min(10)];
        }
        let i = table(&l, &c).unwrap();
        assert_eq!(i.placed.len(), 16);
        assert_eq!((i.placed[0].x, i.placed[0].y), (0.0, 0.9)); // the GK on the 4-4-2 GK spot
        assert!(i.placed[11..].iter().all(Placed::on_bench));
        assert_eq!(
            i.placed.iter().map(|p| p.slot).collect::<Vec<_>>(),
            (1..=16).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_skipped_ref_in_a_bare_list_leaves_its_pitch_place_to_the_next() {
        let l = parse("1 13/A01 2 3 4 5 6 7 8 9 10 11 12", Edition::E1718).unwrap();
        assert_eq!((l.pitch.len(), l.bench.len()), (11, 1));
        assert_eq!(l.bench[0].no, 12);
    }

    #[test]
    fn name_lines_keep_the_last_non_empty_name() {
        let l = parse("name A\nname\nP 1 1 1\n", Edition::E1718).unwrap();
        assert_eq!(l.name.as_deref(), Some("A"));
    }

    #[test]
    fn twelve_starters_and_six_subs() {
        let mut t = String::from("game 1718\n");
        for n in 1..=12 {
            t += &format!("P {n} 50 {}\n", n * 7);
        }
        for n in 21..=26 {
            t += &format!("S {n}\n");
        }
        let l = parse(&t, Edition::E1718).unwrap();
        // as the site: the 12th starter takes the first free bench box, the subs after it
        assert_eq!(l.pitch.len(), 11);
        assert_eq!(
            l.bench.iter().map(|e| e.no).collect::<Vec<_>>(),
            [12, 21, 22, 23, 24]
        );
        assert_eq!(why(&l), [("25", Skip::NoRoom), ("26", Skip::NoRoom)]);
        // with 3 subs, the 12th starter takes the next free box
        let l = parse("P 1 1 1\nP 2 1 1\nP 3 1 1\nP 4 1 1\nP 5 1 1\nP 6 1 1\nP 7 1 1\nP 8 1 1\nP 9 1 1\nP 10 1 1\nP 11 1 1\nP 12 1 1\nS 21\nS 22\nS 23\n", Edition::E1718).unwrap();
        assert_eq!(
            l.bench.iter().map(|e| e.no).collect::<Vec<_>>(),
            [12, 21, 22, 23]
        );
        assert!(l.skipped.is_empty());
    }

    #[test]
    fn duplicates_garbage_and_two_goalkeepers() {
        let t = "# c\nP 5 50 90\nP 5 20 20\nP 6 60 90\nP\nhello world\nP abc 1 1\nP 9 x y\nS 0\n\n";
        let l = parse(t, Edition::E1718).unwrap();
        assert_eq!(
            why(&l),
            [
                ("P", Skip::BadLine),
                ("hello world", Skip::BadLine),
                ("5", Skip::Duplicate),
                ("abc", Skip::BadLine),
                ("0", Skip::BadLine)
            ]
        );
        assert_eq!(l.pitch.iter().map(|e| e.no).collect::<Vec<_>>(), [5, 6, 9]);
        assert_eq!(l.pitch[2].at, None);
        // numbers 5 and 6 are the same player; 8 is unknown; both goalkeepers stay as given
        let mut c = cat(&[5, 6, 9, 10]);
        c[1].name = c[0].name.clone();
        let l = parse(
            "P 5 50 95\nP 6 50 90\nP 10 50 90\nP 8 1 1\nP 11 2 2\n",
            Edition::E1718,
        )
        .unwrap();
        let i = table(&l, &c).unwrap();
        assert_eq!(i.placed.iter().map(|p| p.no).collect::<Vec<_>>(), [5, 10]);
        assert_eq!(
            why_of(&i),
            [
                ("6", Skip::Duplicate),
                ("8", Skip::NotInCatalogue),
                ("11", Skip::NotInCatalogue)
            ]
        );
        assert_eq!(table(&l, &[]), Err(LineupError::Empty));
    }

    fn why_of(i: &Import) -> Vec<(&str, Skip)> {
        i.skipped
            .iter()
            .map(|s| (s.label.as_str(), s.why))
            .collect()
    }

    #[test]
    fn coordinates_clamp_round_and_invert() {
        let l = parse(
            "P 1 0 100\nP 2 150 -20\nP 3 33.3 66.6\nP 4 96 4\n",
            Edition::E1718,
        )
        .unwrap();
        let at: Vec<_> = l.pitch.iter().map(|e| e.at.unwrap()).collect();
        assert_eq!(
            at,
            [(-1.0, 1.0), (1.0, -1.0), (-0.334, 0.332), (0.92, -0.92)]
        );
        let placed: Vec<Placed> = l
            .pitch
            .iter()
            .enumerate()
            .map(|(i, e)| Placed {
                slot: i as i32 + 1,
                no: e.no,
                x: e.at.unwrap().0,
                y: e.at.unwrap().1,
            })
            .collect();
        assert_eq!(
            export(&placed, Edition::E1718, ""),
            "# WCCF DB lineup\ngame 1718\nP 1 0 100\nP 2 100 0\nP 3 33 67\nP 4 96 4\n"
        );
    }

    #[test]
    fn crlf_and_empty() {
        let l = parse(&SITE.replace('\n', "\r\n"), Edition::E1718).unwrap();
        assert_eq!(
            (l.pitch.len(), l.bench.len(), l.name.as_deref()),
            (2, 1, Some("My team"))
        );
        for t in [
            "",
            "  \n# only a comment\n",
            "game 1718\nname x\n",
            "S \"1/A\"\n",
        ] {
            let r = parse(t, Edition::E1718);
            assert_eq!(r, Err(LineupError::Empty), "{t:?}");
        }
    }

    #[test]
    fn export_puts_the_bench_by_band_and_caps_the_counts() {
        let mut placed: Vec<Placed> = (1..=13)
            .map(|n| Placed {
                slot: n,
                no: n as u32,
                x: 0.0,
                y: 0.0,
            })
            .collect();
        placed.push(Placed {
            slot: 14,
            no: 90,
            x: BENCH_X,
            y: BENCH_Y[3],
        });
        placed.push(Placed {
            slot: 15,
            no: 91,
            x: BENCH_X,
            y: BENCH_Y[1],
        });
        let t = export(&placed, Edition::E1011, "");
        assert_eq!(t.matches("\nP ").count(), 11);
        assert!(t.starts_with("# WCCF DB lineup\ngame 1011\nP 1 50 50\n"));
        assert!(t.ends_with("S 91\nS 90\n"));
    }
}
