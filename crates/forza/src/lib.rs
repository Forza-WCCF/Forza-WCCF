//! Forza-WCCF's shared code. `ui`: the theme, the bespoke widgets and the screens of the step 9 design.
//! `standins`: the 2017–18 Aime reader, AiMeDB and game server stand-ins (step 10).

rust_i18n::i18n!("locales", fallback = "en");

pub mod standins;
pub mod ui;
