//! The club card steps of a 2010-11 start and stop (port of `legacy/2010-11/code/card_steps.py` and the `cards\` modules
//! it runs): a finished manager transfer put away and a CLUB CARD switch ([`wallet`]), the bad endings cleared
//! ([`edit`]), the cards on the table following the club ([`formation`]) and data\club_view.txt ([`view`]), all read
//! with [`decode`].

pub mod decode;
pub mod edit;
pub mod formation;
mod schema;
pub mod view;
pub mod wallet;

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const USAGE: &str = "card_steps start SEAT            a finished manager transfer put away, the CLUB CARD panel's \"card=\" switch
                                 and \"card_fix=bad_endings\", each taken out of data\\panel.txt once done
card_steps start board SEAT      the same, then the cards on the table follow the club in the slot
card_steps keep SEAT             after a stop: the cards on the table kept with their club
card_steps view                  data\\club_view.txt written now";

/// Python's str.strip() for the ASCII a panel line holds.
fn strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii_whitespace() || matches!(c, '\x0b' | '\x1c'..='\x1f'))
}

/// A file read as Python's ascii/errors=replace reads it.
fn ascii(raw: &[u8]) -> String {
    raw.iter()
        .map(|&b| {
            if b.is_ascii() {
                char::from(b)
            } else {
                '\u{FFFD}'
            }
        })
        .collect()
}

/// The lines of a text with their ends, split at \r\n, \r or \n (Python's newline="").
fn lines_with_ends(s: &str) -> Vec<&str> {
    let (b, mut out, mut start, mut i) = (s.as_bytes(), Vec::new(), 0, 0);
    while i < b.len() {
        if b[i] == b'\n' || b[i] == b'\r' {
            i += if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            out.push(&s[start..i]);
            start = i;
        } else {
            i += 1;
        }
    }
    if start < b.len() {
        out.push(&s[start..]);
    }
    out
}

fn key_of(line: &str) -> Option<(String, &str)> {
    let line = line.split('#').next().unwrap_or("");
    let (name, value) = line.split_once('=')?;
    let name = strip(name);
    (!name.is_empty()).then(|| (name.to_lowercase(), strip(value)))
}

/// {name: value} from the SETTINGS panel's file: lines "name=value", "#" starts a note; empty if there is none.
pub fn read_panel(path: &Path) -> std::collections::HashMap<String, String> {
    let text = fs::read(path).map(|r| ascii(&r)).unwrap_or_default();
    text.lines()
        .filter_map(key_of)
        .map(|(k, v)| (k, v.to_owned()))
        .collect()
}

/// A request done taken out of the panel's file (only while nothing of the game runs: the panel is not writing).
pub fn drop_panel_line(name: &str, path: &Path) {
    let Ok(raw) = fs::read(path) else { return };
    let text = ascii(&raw);
    let lines = lines_with_ends(&text);
    let keep: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| key_of(l).is_none_or(|(k, _)| k != name))
        .collect();
    if keep.len() != lines.len() {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let out: String = keep
            .concat()
            .chars()
            .map(|c| if c.is_ascii() { c } else { '?' })
            .collect();
        if fs::write(&tmp, out).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }
}

/// Run one card step, as `python card_steps.py ARGS` does in the kit whose data folder is `data` -> (exit code, what it
/// printed). Anything wrong is said and never stops the start: the game starts with the card that is in the slot.
pub fn run(data: &Path, args: &[&str]) -> (i32, String) {
    let save = data.join("save");
    let (card, panel) = (save.join("seat1_club.bin"), data.join("panel.txt"));
    let catalogue = data.join("cache").join("catalogue.tsv");
    let mut out = String::new();
    let mut say = |s: &str| {
        out.push_str(s);
        out.push('\n');
    };
    match args {
        ["start", rest @ .., seat] if rest.is_empty() || rest == ["board"] => {
            // a finished manager transfer put away first, then the CLUB CARD panel's "card=FILE" / "card=new"
            let what = match wallet::finish_transfer(
                &card,
                &save.join("seat1_club.new.bin"),
                &save.join("cards"),
            ) {
                Ok((_, what)) => what,
                Err(e) => format!(
                    "the manager transfer could not be finished now ({e}) - both cards stay where they are"
                ),
            };
            if !what.is_empty() {
                say(&format!("  club card: {what}"));
            }
            let want = read_panel(&panel).remove("card").unwrap_or_default();
            if !want.is_empty() {
                let what = match wallet::switch(&want, &card, &save.join("cards")) {
                    Ok((_, what)) => what,
                    Err(e) => {
                        format!("the switch could not be done ({e}) - the card in the slot stays")
                    }
                };
                say(&format!("  club card: {what}"));
                drop_panel_line("card", &panel); // only after the switch: one stopped half way finishes next start
            }
            let fix = read_panel(&panel)
                .remove("card_fix")
                .unwrap_or_default()
                .to_lowercase();
            if !fix.is_empty() {
                if fix == "bad_endings" {
                    let what = edit::clear_bad_endings(&card).unwrap_or_else(|e| {
                        format!("the bad endings were not cleared ({e}) - the card stays as it was")
                    });
                    say(&format!("  club card: {what}"));
                }
                drop_panel_line("card_fix", &panel);
            }
            if rest == ["board"] {
                match formation::sync(Path::new(seat), &card, &catalogue) {
                    Ok(said) => said.iter().for_each(|w| say(&format!("  table: {w}"))),
                    Err(e) => say(&format!("  table: left as it is ({e})")),
                }
            }
            (0, out)
        }
        ["view"] => {
            let built = view::build(
                &card,
                &catalogue,
                &save.join("backup"),
                &data.join("running.json"),
                &save.join("cards"),
            )
            .and_then(|lines| view::write(&lines, &data.join("club_view.txt")).map(|()| lines));
            match built {
                Ok(lines) => {
                    say(&format!(
                        "club card view written: {}",
                        lines.first().map_or("-", String::as_str)
                    ));
                    (0, out)
                }
                Err(e) => {
                    say(&format!("the club card view was not written: {e}"));
                    (1, out)
                }
            }
        }
        ["keep", seat] => {
            if let Err(e) = formation::keep(Path::new(seat), &save) {
                say(&format!(
                    "  (the cards on the table were not copied to their club: {e})"
                ));
            }
            (0, out)
        }
        _ => {
            let _ = writeln!(out, "{USAGE}");
            (2, out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("panel.txt");
        assert!(read_panel(&p).is_empty());
        fs::write(&p, "play_on=this_pc\r\n# a note\n Card = X.bin # chosen\rcard_fix=bad_endings\nodd\u{e9}=1").unwrap();
        let got = read_panel(&p);
        assert_eq!(got["card"], "X.bin");
        assert_eq!(got["play_on"], "this_pc");
        drop_panel_line("card", &p);
        assert_eq!(
            fs::read(&p).unwrap(),
            b"play_on=this_pc\r\n# a note\ncard_fix=bad_endings\nodd??=1"
        );
        drop_panel_line("nothing", &p);
        assert!(!tmp.path().join("panel.txt.tmp").exists());
    }

    /// Port of test_card_fix.py: a card fix that cannot be done is said, taken out of panel.txt, and the start goes on.
    #[test]
    fn a_failed_card_fix_never_stops_a_start() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        fs::create_dir_all(data.join("save")).unwrap();
        fs::write(data.join("save/seat1_club.bin"), b"not a card").unwrap();
        fs::write(
            data.join("panel.txt"),
            "play_on=this_pc\ncard_fix=bad_endings\n",
        )
        .unwrap();
        let (code, out) = run(data, &["start", "SEAT"]);
        assert_eq!(code, 0);
        assert!(out.contains("were not cleared"), "{out}");
        assert_eq!(
            fs::read_to_string(data.join("panel.txt")).unwrap(),
            "play_on=this_pc\n"
        );
        assert_eq!(run(data, &["start", "x", "SEAT"]).0, 2);
        let (code, out) = run(data, &["view"]);
        assert_eq!(code, 0, "{out}");
        assert!(
            out.starts_with("club card view written: state|unreadable|"),
            "{out}"
        );
        assert!(out.ends_with(
            "seat1_club.bin is 10 bytes; a kit card file is 4112 (or 4096 without its header)\n"
        ));
    }
}
