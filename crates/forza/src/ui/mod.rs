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
