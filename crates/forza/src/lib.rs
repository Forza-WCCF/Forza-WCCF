//! Forza-WCCF's shared code. `ui`: the theme, the bespoke widgets and the screens of the step 9 design.
//! `online`: the client side of online play (seat lease, SEND LOGS) and the protocol the server shares (step 10 I1).
//! `standins`: the 2017–18 Aime reader, AiMeDB and game server stand-ins (step 10).
//! `panel_txt`: data\panel.txt, the SETTINGS page's choices for the next start and the view.
//! `keys`: keys.txt's keys and controller controls, shared by the key driver and the KEYS page.
//! `club_view`: data\club_view.txt, the CLUB CARD page's view of the club card (the run's helper writes it).
//! `cards`: the player's catalogue and the game's card table, read at runtime from the player's folders (step 9.4b).

#[cfg(feature = "ui")]
rust_i18n::i18n!("locales", fallback = "en");

#[cfg(feature = "ui")]
pub mod cards;
pub mod club_view;
pub mod keys;
pub mod online;
pub mod panel_txt;
pub mod standins;
#[cfg(feature = "ui")]
pub mod ui;
