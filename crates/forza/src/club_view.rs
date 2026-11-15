//! data\club_view.txt, the CLUB CARD page's view of the club card, as `wccfpanel.c` `club_read` reads it. The run's
//! helper writes it (`card_steps.py view`, `legacy/2010-11/code/cards/view.py`) whenever the card, its backups or the
//! other cards change; the panel only reads it. ASCII lines "kind|field|...", "#" a note:
//!
//! - `state|ok` / `state|none|why` / `state|new|why` / `state|unreadable|why`
//! - `head|TITLE` a section; `row|LABEL|VALUE|COLOUR` (w white, g green, y yellow, r red, o orange, d dim)
//! - `squad|NO|NAME|POS|APPS|GOALS|ASSISTS|CONDITION|INJURY`
//! - `session|open/closed/cut/new/none/unreadable`, `backup|WHEN`
//! - `wallet|FILE|CLUB|SUMMARY[|transferred]`: the other cards (YOUR CARDS); a transferred one cannot be played.

/// One line of the left column: a section heading, or a row with its colour letter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Head(String),
    Row {
        label: String,
        value: String,
        colour: char,
    },
}

/// Another club card in YOUR CARDS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wallet {
    /// Its file in data\save\cards, as panel.txt's card= names it.
    pub file: String,
    pub club: String,
    pub summary: String,
    /// Used up by a manager transfer: the game will not play it again.
    pub transferred: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClubView {
    /// "ok", "none", "new", "unreadable"; "noview" while there is no file (wccfpanel.c's word).
    pub state: String,
    pub reason: String,
    pub lines: Vec<Line>,
    /// NO NAME POS APPS GOALS ASSISTS CONDITION INJURY ("-" none).
    pub squad: Vec<[String; 8]>,
    pub backups: Vec<String>,
    /// The slot's card session ("open", "cut", ...); "" not said.
    pub session: String,
    pub wallet: Vec<Wallet>,
}

impl Default for ClubView {
    fn default() -> Self {
        ClubView {
            state: "noview".into(),
            reason: String::new(),
            lines: Vec::new(),
            squad: Vec::new(),
            backups: Vec::new(),
            session: String::new(),
            wallet: Vec::new(),
        }
    }
}

/// The C panel's caps: 48 lines, 16 players, 8 backups, 16 other cards; fields cut as its buffers cut them.
fn cut(s: &str, n: usize) -> String {
    s.chars().take(n - 1).collect()
}

impl ClubView {
    pub fn parse(raw: &[u8]) -> ClubView {
        let text = String::from_utf8_lossy(&raw[..raw.len().min(65536)]);
        let mut v = ClubView {
            state: "unreadable".into(),
            reason: "the view file is empty".into(),
            ..ClubView::default()
        };
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.splitn(10, '|').collect();
            match (f[0], f.len()) {
                ("state", 2..) => {
                    v.state = cut(f[1], 16);
                    v.reason = f.get(2).map_or_else(String::new, |r| cut(r, 128));
                }
                ("head", 2..) if v.lines.len() < 48 => v.lines.push(Line::Head(cut(f[1], 32))),
                ("row", 3..) if v.lines.len() < 48 => v.lines.push(Line::Row {
                    label: cut(f[1], 32),
                    value: cut(f[2], 128),
                    colour: f.get(3).and_then(|c| c.chars().next()).unwrap_or('w'),
                }),
                ("squad", 9..) if v.squad.len() < 16 => {
                    v.squad.push(std::array::from_fn(|k| cut(f[k + 1], 28)));
                }
                ("backup", 2..) if v.backups.len() < 8 => v.backups.push(cut(f[1], 24)),
                ("session", 2..) => v.session = cut(f[1], 16),
                ("wallet", 4..) if v.wallet.len() < 16 => v.wallet.push(Wallet {
                    file: cut(f[1], 64),
                    club: cut(f[2], 40),
                    summary: cut(f[3], 48),
                    transferred: f.get(4) == Some(&"transferred"),
                }),
                _ => {}
            }
        }
        v
    }

    /// The BAD ENDINGS row's number (0 when there is none).
    pub fn bad_endings(&self) -> u32 {
        self.lines
            .iter()
            .find_map(|l| match l {
                Line::Row { label, value, .. } if label == "BAD ENDINGS" => Some(
                    value
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                        .parse()
                        .unwrap_or(0),
                ),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// A row's value by its label ("CLUB", "MANAGER").
    pub fn row(&self, want: &str) -> Option<&str> {
        self.lines.iter().find_map(|l| match l {
            Line::Row { label, value, .. } if label == want => Some(value.as_str()),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_view_as_the_c_panel() {
        let raw = b"# a note\r\nstate|ok\r\nhead|CLUB\nrow|CLUB|KITAKAZE  (founded 2026-10-01)|w\n\
                    row|BAD ENDINGS|2 - trade rights lost; 5th costs money|r\nrow|SALARY|$1|\n\
                    squad|7|NAME|MF|3|1|0|good|-\nsquad|too|few\nsession|cut\nbackup|2026-10-12 15:04\n\
                    wallet|a.bin|PORTO|W1 D0 L0\nwallet|b.bin|NORD|W0 D0 L2|transferred\nnonsense|x\n";
        let v = ClubView::parse(raw);
        assert_eq!(v.state, "ok");
        assert_eq!(v.lines.len(), 4);
        assert_eq!(v.lines[0], Line::Head("CLUB".into()));
        assert_eq!(v.row("CLUB"), Some("KITAKAZE  (founded 2026-10-01)"));
        // an empty colour field: the C panel takes its first character, a zero - white
        assert!(matches!(&v.lines[3], Line::Row { colour: 'w', .. }));
        assert_eq!(v.bad_endings(), 2);
        assert_eq!(v.squad.len(), 1);
        assert_eq!(v.squad[0][2], "MF");
        assert_eq!(v.session, "cut");
        assert_eq!(v.backups, ["2026-10-12 15:04"]);
        assert_eq!(v.wallet.len(), 2);
        assert!(!v.wallet[0].transferred && v.wallet[1].transferred);
        assert_eq!(v.wallet[1].file, "b.bin");

        let none = ClubView::parse(b"state|none|no club card in the slot yet\nsession|none\n");
        assert_eq!(
            (none.state.as_str(), none.reason.as_str()),
            ("none", "no club card in the slot yet")
        );
        assert_eq!(ClubView::parse(b"").state, "unreadable");
        assert_eq!(ClubView::default().state, "noview");
    }
}
