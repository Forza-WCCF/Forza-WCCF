//! The step 9 design: [`theme`] (fonts and the one `egui::Style`), [`widgets`] and [`board`] (painted widgets), and the
//! screens as functions over sample data: [`launcher`], [`installer`], [`panel`].

/// UI text in one language: `tr!(lang, "key")`, `tr!(lang, "key", n = 3)`. English is the source and the fallback.
macro_rules! tr {
    ($lang:expr, $key:expr) => {
        rust_i18n::t!($key, locale = $lang.code()).into_owned()
    };
    ($lang:expr, $key:expr, $($args:tt)*) => {
        rust_i18n::t!($key, locale = $lang.code(), $($args)*).into_owned()
    };
}

pub mod board;
pub mod installer;
pub mod launcher;
pub mod panel;
pub mod sample;
pub mod theme;
pub mod widgets;

/// The three first-class languages, in the project's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    En,
    It,
    Ja,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::En, Lang::It, Lang::Ja];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::It => "it",
            Lang::Ja => "ja",
        }
    }

    /// The language's own name, as a language picker shows it.
    pub fn native(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::It => "Italiano",
            Lang::Ja => "日本語",
        }
    }
}

/// One UI string for a program that draws its own screen (the installer builds messages outside `forza::ui`).
pub fn text(lang: Lang, key: &str) -> String {
    rust_i18n::t!(key, locale = lang.code()).into_owned()
}

impl Lang {
    /// A language code: `en`, `it` or `ja`.
    pub fn from_code(code: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|l| l.code() == code.trim())
    }

    /// The program's language: `WCCF_KIT_LANG=en|it|ja` (to check a translation on any PC), else Windows' display
    /// language, else English.
    pub fn detect() -> Lang {
        if let Some(l) = std::env::var("WCCF_KIT_LANG")
            .ok()
            .and_then(|c| Lang::from_code(&c))
        {
            return l;
        }
        #[cfg(windows)]
        // SAFETY: a plain query without arguments.
        match unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff {
            0x10 => return Lang::It, // LANG_ITALIAN
            0x11 => return Lang::Ja, // LANG_JAPANESE
            _ => {}
        }
        Lang::En
    }
}
