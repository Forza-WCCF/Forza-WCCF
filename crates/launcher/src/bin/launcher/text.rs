//! Launcher.exe's words, from forza's `locales/ui.yml` (the window's own source too): English, Italian or Japanese -
//! the one picked in the window (data\settings.json "language"), else Windows' display language.  The kit's scripts
//! (their output in the window) stay in English.

use std::sync::atomic::{AtomicUsize, Ordering};

use forza_ui::Lang;
use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;

static CHOSEN: AtomicUsize = AtomicUsize::new(0);

/// The words picked: WCCF_KIT_LANG=en|it|ja (to check a translation on any PC), the one saved in data\settings.json,
/// Windows' display language (LANG_ITALIAN 0x10, LANG_JAPANESE 0x11), English.  The window's list changes it later.
pub fn choose(saved: Option<&str>) {
    let env = std::env::var("WCCF_KIT_LANG").ok();
    // SAFETY: a plain query without arguments.
    let windows = match unsafe { GetUserDefaultUILanguage() } & 0x3ff {
        0x10 => Lang::It,
        0x11 => Lang::Ja,
        _ => Lang::En,
    };
    set([env.as_deref(), saved]
        .into_iter()
        .flatten()
        .find_map(of)
        .unwrap_or(windows));
}

/// The language of a code ("en", "it", "ja").
pub fn of(code: &str) -> Option<Lang> {
    Lang::ALL.into_iter().find(|l| l.code() == code)
}

pub fn set(lang: Lang) {
    CHOSEN.store(
        Lang::ALL.iter().position(|&l| l == lang).unwrap_or(0),
        Ordering::Relaxed,
    );
}

pub fn lang() -> Lang {
    Lang::ALL[CHOSEN.load(Ordering::Relaxed).min(Lang::ALL.len() - 1)]
}

/// `key` (ui.yml's full key, so forza's text test finds it) in the chosen language.
pub fn t(key: &str) -> String {
    tf(key, &[])
}

/// `key` in the chosen language, each `%{name}` filled.
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    forza_ui::text(lang(), key, args)
}
