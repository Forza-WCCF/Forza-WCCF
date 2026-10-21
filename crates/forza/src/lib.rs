//! Forza-WCCF's shared code. `ui`: the theme, the bespoke widgets and the screens of the step 9 design.
//! `online`: the client side of online play (seat lease, SEND LOGS), the protocol the server shares (step 10 I1), and
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

#[cfg(feature = "ui")]
rust_i18n::i18n!("locales", fallback = "en");
// rustc does not list the macro's yml as an input, so cargo and sccache reused a build with old text (2026-10-20);
// include_str! puts it in the dep-info. Add each new locale file here.
#[cfg(feature = "ui")]
const _: &str = include_str!("../locales/ui.yml");

#[cfg(feature = "ui")]
pub mod cards;
pub mod club_view;
pub mod curl;
pub mod keys;
pub mod layout;
pub mod online;
pub mod panel_txt;
pub mod pause;
pub mod service;
/// The install steps need the card catalogue (`cards`), so the `ui` feature too.
#[cfg(feature = "ui")]
pub mod setup;
pub mod standins;
#[cfg(feature = "ui")]
pub mod text;
#[cfg(feature = "ui")]
pub mod ui;
