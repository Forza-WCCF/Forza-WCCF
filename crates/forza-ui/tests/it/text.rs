//! Every text key the screens use is in `locales/ui.yml` with English, Italian and Japanese (rust-i18n falls back to
//! English silently, so a missing translation would not show otherwise). Launcher.exe's keys are checked here too.

use std::collections::HashMap;

#[test]
fn every_key_in_three_languages() {
    let yml = std::fs::read_to_string("locales/ui.yml").unwrap();
    let table: HashMap<&str, &str> = yml.lines().filter_map(|l| l.split_once(": {")).collect();
    // every section `ui.yml` has ("launcher", "form", ...): a quoted "section.key" in the sources must be in it
    let sections: Vec<&str> = table
        .keys()
        .filter_map(|k| k.split_once('.'))
        .map(|(s, _)| s)
        .collect();
    let mut missing = vec![];
    // the crates that word text with forza_ui::text or tr!, every folder under their src: the screens, Launcher.exe,
    // the panel and forza's setup, game text, card picture and online lines
    let mut files = vec![];
    let mut dirs: Vec<std::path::PathBuf> = [
        "src",
        "../launcher/src",
        "../installer/src",
        "../panel/src",
        "../forza/src",
    ]
    .map(Into::into)
    .into();
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    for path in files {
        let src = std::fs::read_to_string(&path).unwrap();
        for piece in src.split('"').skip(1).step_by(2) {
            let Some((section, rest)) = piece.split_once('.') else {
                continue;
            };
            if !sections.contains(&section)
                || rest.is_empty()
                // file and host names: "panel.txt", "lobby.test", "club.bin", "report.request"
                || ["txt", "exe", "json", "py", "example", "test", "bin", "request"].contains(&rest)
                || !rest
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                continue;
            }
            match table.get(piece) {
                Some(v) if ["en: ", "it: ", "ja: "].iter().all(|l| v.contains(l)) => {}
                _ => missing.push(format!("{} in {}", piece, path.display())),
            }
        }
    }
    assert!(
        missing.is_empty(),
        "not in locales/ui.yml in en, it and ja: {missing:?}"
    );
}

/// Each key's Italian and Japanese use the same `%{name}` placeholders as its English (a lost one shows an empty gap,
/// a misspelt one shows `%{name}` to the player).
#[test]
fn placeholders_match_english() {
    let yml = std::fs::read_to_string("locales/ui.yml").unwrap();
    // `key: {en: "...", it: "...", ja: "..."}`: each language's part runs to the next one's label
    let part = |line: &str, lang: &str| -> Vec<String> {
        let from = line.find(&format!("{lang}: \"")).unwrap_or(line.len());
        let to = ["en", "it", "ja"]
            .iter()
            .filter_map(|l| line.find(&format!(", {l}: \"")))
            .filter(|&i| i > from)
            .min()
            .unwrap_or(line.len());
        let mut v: Vec<String> = line[from..to]
            .split("%{")
            .skip(1)
            .filter_map(|p| p.split_once('}').map(|(n, _)| n.to_owned()))
            .collect();
        v.sort();
        v
    };
    let mut wrong = vec![];
    for line in yml.lines().filter(|l| l.contains(": {en: \"")) {
        let key = line.split(':').next().unwrap_or_default();
        for lang in ["it", "ja"] {
            if part(line, lang) != part(line, "en") {
                wrong.push(format!("{key}.{lang}"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "placeholders differ from the English: {wrong:?}"
    );
}

/// A language code from a setting or `WCCF_KIT_LANG` reads in any case and with spaces around it (#306).
#[test]
fn language_codes() {
    use forza_ui::Lang;
    assert_eq!(Lang::from_code(" IT "), Some(Lang::It));
    assert_eq!(Lang::from_code("Ja"), Some(Lang::Ja));
    assert_eq!(Lang::from_code("xx"), None);
}

/// #375 text pass: the Italian slips fixed, the market's key and fork refusals name the file to put back (and the key's
/// 7 days), START's hint names DEAL as the step that locks, and IMPORT LINEUP's count blames no game.
#[test]
fn create_a_team_lines_name_what_the_player_needs() {
    use forza_ui::{Lang, text};
    let t = |lang, k| text(lang, k, &[("n", "3")]);
    assert!(t(Lang::It, "collection.buy_pool").contains("troppo pochi"));
    assert_eq!(t(Lang::It, "collection.back"), "‹ PARTITA");
    for lang in Lang::ALL {
        let key = t(lang, "collection.market_why_another_key");
        assert!(
            key.contains(r"data\market.key") && key.contains('7'),
            "{lang:?}: {key}"
        );
        let fork = t(lang, "collection.market_why_fork");
        assert!(
            fork.contains(r"collection\grants.jsonl"),
            "{lang:?}: {fork}"
        );
        let deal = t(lang, "collection.deal");
        assert!(
            t(lang, "collection.choose_hint").contains(&deal),
            "{lang:?}"
        );
        assert!(!t(lang, "cat.ask_skipped").is_empty());
    }
    assert!(!t(Lang::En, "cat.ask_skipped").contains("game"));
}
