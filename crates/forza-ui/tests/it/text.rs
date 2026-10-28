//! Every text key the screens use is in `locales/ui.yml` with English, Italian and Japanese (rust-i18n falls back to
//! English silently, so a missing translation would not show otherwise). Launcher.exe's keys are checked here too.

use std::collections::HashMap;

#[test]
fn every_key_in_three_languages() {
    let yml = std::fs::read_to_string("locales/ui.yml").unwrap();
    let table: HashMap<&str, &str> = yml.lines().filter_map(|l| l.split_once(": {")).collect();
    let mut missing = vec![];
    // the screens, and Launcher.exe's messages (forza_ui::text, also from the run)
    let files = [
        "src",
        "../launcher/src",
        "../launcher/src/bin/launcher",
        "../launcher/src/run",
        "../installer/src",
    ]
    .iter()
    .flat_map(|d| std::fs::read_dir(d).unwrap())
    .map(|e| e.unwrap().path())
    .filter(|p| p.is_file());
    for path in files {
        let src = std::fs::read_to_string(path).unwrap();
        for piece in src.split('"').skip(1).step_by(2) {
            let Some((section, rest)) = piece.split_once('.') else {
                continue;
            };
            let known = [
                "common",
                "launcher",
                "installer",
                "cab",
                "panel",
                "board",
                "cat",
                "keys",
                "settings",
                "club",
                "deal",
                "close",
                "lobby",
                "pause",
                "logs",
            ];
            if !known.contains(&section)
                || rest.is_empty()
                || ["txt", "exe", "json", "example", "test"].contains(&rest) // file and host names: "panel.txt", "lobby.test"
                || !rest
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                continue;
            }
            match table.get(piece) {
                Some(v) if ["en: ", "it: ", "ja: "].iter().all(|l| v.contains(l)) => {}
                _ => missing.push(piece.to_owned()),
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
    assert!(wrong.is_empty(), "placeholders differ from the English: {wrong:?}");
}
