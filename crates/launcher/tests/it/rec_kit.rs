//! Replays tests/rec/kit.jsonl, recorded once from the Python kit's kit_common.find_game, seat_dir and misc_dir
//! (legacy/2010-11/tests/rec_a_kit.py, which no check runs any more).

use std::fs;
use std::path::Path;

use launcher::kit::{find_game, misc_dir, seat_dir};
use serde_json::Value;

#[test]
fn find_game_as_recorded() {
    let rec = include_str!("rec/kit.jsonl");
    let mut lines = rec.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    for f in lines.next().unwrap()["tree"].as_array().unwrap() {
        let f = root.join(f.as_str().unwrap());
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, b"").unwrap();
    }
    let rel = |p: &Path| p.strip_prefix(root).unwrap().to_str().unwrap().to_owned();
    let mut n = 0;
    for case in lines {
        let input = case["input"].as_str().unwrap();
        let bare = input.trim_start_matches([' ', '"']);
        let arg = if input.is_empty() {
            String::new()
        } else {
            format!("{}{}", &input[..input.len() - bare.len()], root.join(bare).display())
        };
        let game = find_game(&arg);
        let got = |f: fn(&Path) -> std::path::PathBuf| game.as_deref().map(|g| rel(&f(g)));
        assert_eq!(game.as_deref().map(rel).as_deref(), case["game"].as_str(), "{input:?}");
        assert_eq!(got(seat_dir).as_deref(), case["seat"].as_str(), "{input:?}");
        assert_eq!(got(misc_dir).as_deref(), case["misc"].as_str(), "{input:?}");
        n += 1;
    }
    assert_eq!(n, 14);
}
