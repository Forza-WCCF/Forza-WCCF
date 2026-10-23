//! Every text key the screens use is in `locales/ui.yml` with English, Italian and Japanese (rust-i18n falls back to
//! English silently, so a missing translation would not show otherwise). Launcher.exe's keys are checked here too.

use std::collections::HashMap;

#[test]
fn every_key_in_three_languages() {
    let yml = std::fs::read_to_string("locales/ui.yml").unwrap();
    let table: HashMap<&str, &str> = yml.lines().filter_map(|l| l.split_once(": {")).collect();
    let mut missing = vec![];
    // the screens, and Launcher.exe's messages (forza_ui::text)
    let files = ["src", "../launcher/src/bin/launcher", "../installer/src"]
        .iter()
        .flat_map(|d| std::fs::read_dir(d).unwrap());
    for e in files {
        let src = std::fs::read_to_string(e.unwrap().path()).unwrap();
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
                || ["txt", "exe"].contains(&rest) // file names: "panel.txt", "launcher.exe"
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
