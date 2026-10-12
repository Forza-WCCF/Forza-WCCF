//! Forza-WCCF's shared code. `ui`: the theme, the bespoke widgets and the screens of the step 9 design.
//! `online`: the client side of online play (seat lease, SEND LOGS) and the protocol the server shares (step 10 I1).
//! `standins`: the 2017–18 Aime reader, AiMeDB and game server stand-ins (step 10).

#[cfg(feature = "ui")]
rust_i18n::i18n!("locales", fallback = "en");

pub mod online;
pub mod standins;
#[cfg(feature = "ui")]
pub mod ui;
