//! Forza-WCCF's shared code; the egui screens are `forza-ui`. `board`: the player cards and the card table as data.
//! `online`: the client side of online play (seat lease, SAVE LOGS), the protocol the server shares (step 10 I1), and
//! the 2017–18 game server and VS matcher that both the Launcher's stand-in and `forza-server` run.
//! `standins`: the cabinet stand-ins on the player's PC: the 2017–18 Aime reader and AiMeDB, the 2010–11 card reader,
//! network, I/O board and scenes (step 10). `service`: the log sink, handle and accept loop every service shares.
//! `panel_txt`: data\panel.txt, the SETTINGS page's choices for the next start and the view.
//! `keys`: keys.txt's keys and controller controls, shared by the key driver and the KEYS page.
//! `pause`: the rules of the in-game panel's capped pause (where it is allowed, the cap and the gap).
//! `club_view`: data\club_view.txt, the CLUB CARD page's view of the club card (the run's helper writes it).
//! `text`: the game text in English or Italian (`english.py` of both editions).
//! `layout`: the folder names of each edition's kit.
//! `setup`: the install steps (step 11): the guarded, patched copies of the game and the seat folder.
//! `cards`: the player's catalogue and the game's card table, read at runtime from the player's folders (step 9.4b).

/// The two editions; the one type the theme, the launcher, the server and the panel share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edition {
    E1011,
    E1718,
}

impl Edition {
    /// "2010-11" or "2017-18": the run's words and `forza-server --edition`.
    pub fn name(self) -> &'static str {
        match self {
            Self::E1011 => "2010-11",
            Self::E1718 => "2017-18",
        }
    }

    /// The folder name under `Games\Forza-WCCF` (the installer's `disc.rs` names).
    pub fn folder(self) -> &'static str {
        match self {
            Self::E1011 => "2010-2011",
            Self::E1718 => "2017-2018",
        }
    }

    /// The badge text; an en dash, as the project writes it.
    pub fn label(self) -> &'static str {
        match self {
            Self::E1011 => "2010–11",
            Self::E1718 => "2017–18",
        }
    }

    /// The game's id: the keychip's answer and the shared memory's name prefix.
    pub fn game_id(self) -> &'static str {
        match self {
            Self::E1011 => "SBWG",
            Self::E1718 => "SDDY",
        }
    }
}

/// A line setup, the game text and the card pictures tell the player: a `locales/ui.yml` key of forza-ui and its
/// `%{name}` arguments, which may be lines too.  forza does not word it (forza-ui needs forza): the caller shows it in
/// the player's language and logs the English (`forza_ui::line`).  `Raw`: words the same in every language (a path, an
/// error from Windows or the original files).
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Raw(String),
    Key(&'static str, Vec<(&'static str, Line)>),
}

impl Line {
    pub fn key(key: &'static str) -> Self {
        Self::Key(key, Vec::new())
    }

    pub fn raw(words: impl std::fmt::Display) -> Self {
        Self::Raw(words.to_string())
    }

    /// `%{name}` filled with `value` as it is.
    #[must_use]
    pub fn arg(self, name: &'static str, value: impl std::fmt::Display) -> Self {
        self.part(name, Self::Raw(value.to_string()))
    }

    /// `%{name}` filled with another line.
    #[must_use]
    pub fn part(self, name: &'static str, value: Line) -> Self {
        match self {
            Self::Key(k, mut args) => {
                args.push((name, value));
                Self::Key(k, args)
            }
            raw => raw,
        }
    }
}

/// Where a step's lines go: a lead the same in every language ("  ok    ", "  NOTE  ", "  FAIL  ", an indent), then
/// the line.
pub type Say<'a> = &'a dyn Fn(&str, &Line);

pub mod board;
pub mod card;
#[cfg(feature = "full")]
pub mod cards;
pub mod club_view;
pub mod curl;
pub mod keys;
pub mod kit;
pub mod layout;
pub mod online;
pub mod panel_txt;
pub mod pause;
/// Windows only: the game's processes and windows (the Launcher's run, the panel).
#[cfg(windows)]
pub mod procs;
pub mod service;
/// The install steps need the card catalogue (`cards`), so the `full` feature too.
#[cfg(feature = "full")]
pub mod setup;
/// Windows only: the game's start volumes (the Launcher's helper, the panel's mute).
#[cfg(windows)]
pub mod sound;
pub mod standins;
#[cfg(feature = "full")]
pub mod text;
