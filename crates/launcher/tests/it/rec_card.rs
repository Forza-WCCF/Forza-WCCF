//! Replays tests/rec/card.jsonl (legacy/2010-11/tests/rec_a_card.py: kit_common.card_session, play.py's club_session).

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use launcher::card::{club_session, session};
use serde_json::Value;

/// Every recorded card was last written at this time.
const WRITTEN: u64 = 1_791_500_000;

fn at(secs: &Value) -> Option<SystemTime> {
    secs.as_u64().map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s))
}

/// A card file from its record (None: no file), last written at WRITTEN.
fn put(path: &Path, raw: &Value) {
    let _ = fs::remove_file(path);
    if raw.is_null() {
        return;
    }
    let mut bytes = vec![0u8; usize::try_from(raw["size"].as_u64().unwrap()).unwrap()];
    for set in raw["set"].as_array().unwrap() {
        bytes[usize::try_from(set[0].as_u64().unwrap()).unwrap()] = u8::try_from(set[1].as_u64().unwrap()).unwrap();
    }
    fs::write(path, bytes).unwrap();
    let f = fs::File::options().write(true).open(path).unwrap();
    f.set_modified(at(&WRITTEN.into()).unwrap()).unwrap();
}

#[test]
fn sessions_as_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let (slot, new) = (dir.path().join("slot.bin"), dir.path().join("new.bin"));
    let mut cards = HashMap::new();
    let (mut singles, mut pairs) = (0, 0);
    for line in include_str!("rec/card.jsonl").lines() {
        let rec: Value = serde_json::from_str(line).unwrap();
        let since = at(&rec["since"]);
        let (state, bad) = if let Some(name) = rec["card"].as_str() {
            cards.insert(name.to_owned(), rec["raw"].clone());
            put(&slot, &rec["raw"]);
            singles += 1;
            session(&slot, since)
        } else {
            put(&slot, &cards[rec["slot"].as_str().unwrap()]);
            put(&new, &cards[rec["new"].as_str().unwrap()]);
            pairs += 1;
            club_session(&slot, &new, since)
        };
        assert_eq!(state.name(), rec["state"], "{line}");
        assert_eq!(bad.map(u64::from), rec["bad"].as_u64(), "{line}");
    }
    assert_eq!((singles, pairs), (64, 75));
}
