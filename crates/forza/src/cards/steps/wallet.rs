//! Your other club cards, and the card in the slot (port of `legacy/2010-11/code/cards/wallet.py`). The slot is
//! data\save\seat1_club.bin, the card the card reader stand-in serves; the other cards wait in data\save\cards\, one
//! file each. A switch (CLUB CARD's "card=FILE" / "card=new", done at a start while nothing runs) is two renames inside
//! data\save: the card in the slot goes to cards\ under its club's name, then the chosen card into the slot. No card is
//! ever overwritten or deleted: a name already taken gets " (2)", " (3)" ... A finished manager transfer (the reader put
//! a blank card beside an expired one, the game moved the manager to it) is put away the same way.

use super::decode::{self as d, Decoded};
use super::view::{ascii_name, ordinal};
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

/// A rename that never replaces a file (Python's os.rename on Windows; std's rename would replace it).
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is already there", to.display()),
        ));
    }
    fs::rename(from, to)
}

/// (the club's name as the panel draws it, or None; a short summary) of a card file - read-only (wallet.club_label).
pub fn club_label(card: &Path) -> (Option<String>, String) {
    let (state, bad) = d::session(card, None);
    match state {
        "none" => return (None, "no file".into()),
        "unreadable" => return (None, "cannot be read".into()),
        "new" => return (None, "a new card - no club yet".into()),
        _ => {}
    }
    let Ok(c) = d::read_card(card) else {
        return (None, "cannot be read (CardError)".into());
    };
    let dd = Decoded::of(c.copy(d::game_choice(&c).2));
    let lg = d::league(&dd);
    let name =
        Some(ascii_name(&d::text(&dd.get("Club", 0, "CLUB_NAME")))).filter(|n| !n.is_empty());
    if used_up(card) {
        return (name, "transferred to a new card".into()); // short: YOUR CARDS has ~27 letters beside its button
    }
    let bad = bad
        .filter(|&b| b != 0)
        .map_or(String::new(), |b| format!(", {b} bad"));
    let summary = format!(
        "Div {} {}, W{} D{} L{}{bad}",
        lg.division,
        ordinal(lg.position_computed),
        lg.win,
        lg.draw,
        lg.lose
    );
    (name, summary)
}

/// A club name as a file name: no characters Windows refuses, no dots or spaces at the ends, 40 at most.
pub fn safe(name: &str) -> String {
    let bad = |c: char| {
        matches!(
            c,
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'..='\x1f'
        )
    };
    let s: String = name.chars().map(|c| if bad(c) { '_' } else { c }).collect();
    let ends = [' ', '.'];
    let s: String = s.trim_matches(ends).chars().take(40).collect();
    let s = s.trim_matches(ends);
    if s.is_empty() {
        "club card".into()
    } else {
        s.into()
    }
}

/// folder\BASE.bin, or BASE (2).bin ... - a name nothing has yet.
pub fn free_path(base: &str, folder: &Path) -> PathBuf {
    let mut p = folder.join(format!("{base}.bin"));
    let mut n = 2;
    while p.exists() {
        p = folder.join(format!("{base} ({n}).bin"));
        n += 1;
    }
    p
}

/// [(file name, club name or None, summary)] of the cards in `folder`, by file name.
pub fn wallet(folder: &Path) -> Vec<(String, Option<String>, String)> {
    let Ok(dir) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<String> = dir
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|f| f.to_lowercase().ends_with(".bin") && folder.join(f).is_file())
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|f| {
            let (name, summary) = club_label(&folder.join(&f));
            (f, name, summary)
        })
        .collect()
}

/// A card's own table goes with it: NAME.board beside NAME.bin, moved BEFORE its card; a folder already under the new
/// name (a leftover) is kept aside as " (2)" ..., never written over.
pub fn move_board(card_from: &Path, card_to: &Path) -> io::Result<()> {
    let (src, dst) = (
        card_from.with_extension("board"),
        card_to.with_extension("board"),
    );
    if !src.is_dir() {
        return Ok(());
    }
    if dst.exists() {
        let aside = |n: u32| PathBuf::from(format!("{} ({n})", dst.display()));
        let n = (2..).find(|&n| !aside(n).exists()).unwrap();
        rename_new(&dst, &aside(n))?;
    }
    rename_new(&src, &dst)
}

/// A card file's use counter (header bytes 5-6: 0xFFFF blank, low byte 1 expired, 0 used up), or None.
pub fn counter(card: &Path) -> Option<u16> {
    let mut raw = Vec::new();
    fs::File::open(card)
        .ok()?
        .take(7)
        .read_to_end(&mut raw)
        .ok()?;
    (raw.len() == 7).then(|| u16::from_le_bytes([raw[5], raw[6]]))
}

/// A card the game will not play again: counter low byte 0, its manager moved to a new card - kept as a record.
pub fn used_up(card: &Path) -> bool {
    counter(card).is_some_and(|c| c != 0xFFFF && c & 0xFF == 0)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// After the game's manager transfer: the old card (used up, its club still on it) goes to your cards as
/// "CLUB - transferred", its table with it, and the new card into the slot -> (done, what to say); (false, "") when
/// there is nothing to finish. Either order of a stop between the two renames is finished at the next start.
pub fn finish_transfer(slot: &Path, new: &Path, folder: &Path) -> io::Result<(bool, String)> {
    let (old_c, new_c) = (counter(slot), counter(new));
    if new_c.is_none_or(|c| c == 0xFFFF) {
        return Ok((false, String::new()));
    }
    if old_c.is_none() && !slot.exists() {
        // stopped between the two renames: the second one now
        rename_new(new, slot)?;
        return Ok((
            true,
            "manager transfer finished: the new club is in the slot".into(),
        ));
    }
    if old_c.is_none_or(|c| c & 0xFF != 0) {
        return Ok((false, String::new()));
    }
    if matches!(d::session(new, None).0, "unreadable" | "none") {
        return Ok((
            false,
            "the new card from the transfer cannot be read - both cards stay where they are".into(),
        ));
    }
    fs::create_dir_all(folder)?;
    let (name, _) = club_label(slot);
    let base: String = safe(name.as_deref().unwrap_or("club card"))
        .chars()
        .take(26)
        .collect();
    let dst = free_path(&format!("{base} - transferred"), folder);
    move_board(slot, &dst)?; // the old club's table goes with it
    rename_new(slot, &dst)?;
    rename_new(new, slot)?; // the new club starts with no table of its own (formation)
    let (new_name, _) = club_label(slot);
    Ok((
        true,
        format!(
            "manager transfer finished: {} went to your cards ({}, the game will not play it again); {} is in the slot",
            name.as_deref().unwrap_or("the old club"),
            file_name(&dst),
            new_name.as_deref().unwrap_or("the new club")
        ),
    ))
}

/// Carry out "card=FILE" or "card=new" (nothing of the game may run) -> (done, what to say). A bad request leaves the
/// slot as it is.
pub fn switch(request: &str, slot: &Path, folder: &Path) -> io::Result<(bool, String)> {
    let req = request.trim();
    let target = if req.eq_ignore_ascii_case("new") {
        None
    } else {
        let not_a_name = req.contains(['/', '\\']) || req.chars().nth(1) == Some(':');
        if not_a_name || !req.to_lowercase().ends_with(".bin") {
            return Ok((
                false,
                format!("\"{req}\" is not one of your cards - the card in the slot stays"),
            ));
        }
        let target = folder.join(req);
        if !target.is_file() {
            return Ok((
                false,
                format!("{req} is not in your cards (any more) - the card in the slot stays"),
            ));
        }
        if matches!(d::session(&target, None).0, "unreadable" | "none") {
            return Ok((
                false,
                format!(
                    "{req} cannot be read - it stays in your cards, the card in the slot stays"
                ),
            ));
        }
        if used_up(&target) {
            return Ok((
                false,
                format!(
                    "{req} is transferred - the game will not play it again; it stays in your cards, the card in \
                     the slot stays"
                ),
            ));
        }
        Some(target)
    };
    fs::create_dir_all(folder)?;
    let mut put_away = String::new();
    if slot.is_file() {
        // 1: the card in the slot goes to your cards, by club name
        let (name, _) = club_label(slot);
        let dst = free_path(&safe(name.as_deref().unwrap_or("club card")), folder);
        move_board(slot, &dst)?; // its cards on the table go with it
        rename_new(slot, &dst)?;
        put_away = format!(
            "{} went to your cards ({}); ",
            name.as_deref().unwrap_or("the old card"),
            file_name(&dst)
        );
    }
    let Some(target) = target else {
        return Ok((
            true,
            format!("{put_away}a NEW card is in the slot - put it in, and the game makes a club"),
        ));
    };
    move_board(&target, slot)?;
    rename_new(&target, slot)?; // 2: the chosen card into the slot
    let (name, _) = club_label(slot);
    Ok((
        true,
        format!(
            "{put_away}{} is in the slot",
            name.as_deref().unwrap_or(req)
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::super::decode::tests::{card_with, set};
    use super::super::view;
    use super::*;

    /// A card file the game accepts, with this use counter and contract.
    fn card(path: &Path, counter: u16, term: u32, club: &str) {
        let mut copy = vec![0u8; d::COPY_BYTES];
        set(&mut copy, "Coach", 0, "COACH_LAST_TERM", &[term]);
        set(
            &mut copy,
            "Club",
            0,
            "CLUB_NAME",
            &club.bytes().map(u32::from).collect::<Vec<_>>(),
        );
        fs::write(path, card_with(counter, &copy)).unwrap();
    }

    /// Port of test_wallet_transferred.py: a used-up card is listed as transferred, a switch to it is refused, and an
    /// ended contract says so.
    #[test]
    fn transferred_cards() {
        let tmp = tempfile::tempdir().unwrap();
        let cards = tmp.path().join("cards");
        fs::create_dir(&cards).unwrap();
        let (slot, dead, other) = (
            tmp.path().join("seat1_club.bin"),
            cards.join("OLD - transferred.bin"),
            cards.join("OTHER.bin"),
        );
        card(&slot, 0xFFF0, 50, "SLOT");
        card(&dead, 0x0000, 0, "OLD");
        card(&other, 0xFF01, 0, "OTHER"); // contract ended, not transferred yet: still playable
        assert!(used_up(&dead) && !used_up(&other) && !used_up(&slot));
        let labels: Vec<_> = wallet(&cards).into_iter().map(|w| (w.0, w.2)).collect();
        assert_eq!(
            labels[0],
            (
                "OLD - transferred.bin".into(),
                "transferred to a new card".into()
            )
        );
        assert!(!labels[1].1.starts_with("transferred"), "{labels:?}");
        let none = tmp.path().join("none");
        let build = |card: &Path| view::build(card, &none, &none, &none, &cards).unwrap();
        let wal: Vec<_> = build(&slot)
            .into_iter()
            .filter(|l| l.starts_with("wallet|"))
            .collect();
        assert!(
            wal[0].starts_with("wallet|OLD - transferred.bin|") && wal[0].ends_with("|transferred")
        );
        assert!(!wal[1].ends_with("|transferred"), "{wal:?}");
        let (done, what) = switch("OLD - transferred.bin", &slot, &cards).unwrap();
        assert!(
            !done && what.contains("transferred") && dead.exists() && slot.exists(),
            "{what}"
        );
        for (path, want) in [
            (&other, "ended - put the card in"),
            (&dead, "transferred - the game will not play"),
            (&slot, "50 matches left"),
        ] {
            let rows: Vec<_> = build(path)
                .into_iter()
                .filter(|l| l.starts_with("row|CONTRACT|"))
                .collect();
            assert!(rows[0].contains(want), "{rows:?}");
        }
        let (done, what) = switch("OTHER.bin", &slot, &cards).unwrap(); // an ended contract still switches
        assert!(done, "{what}");
        assert_eq!(
            what,
            "SLOT went to your cards (SLOT.bin); OTHER is in the slot"
        );
    }

    #[test]
    fn switch_and_transfer_never_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let (slot, new, cards) = (
            tmp.path().join("seat1_club.bin"),
            tmp.path().join("seat1_club.new.bin"),
            tmp.path().join("cards"),
        );
        assert!(!switch("../x.bin", &slot, &cards).unwrap().0);
        assert!(!switch("x.txt", &slot, &cards).unwrap().0);
        assert!(
            switch("gone.bin", &slot, &cards)
                .unwrap()
                .1
                .contains("not in your cards")
        );
        card(&slot, 0x0000, 0, "OLD"); // used up by the transfer
        fs::create_dir(slot.with_extension("board")).unwrap();
        assert_eq!(
            finish_transfer(&slot, &new, &cards).unwrap(),
            (false, String::new())
        );
        card(&new, 0xFFF0, 50, "");
        let (done, what) = finish_transfer(&slot, &new, &cards).unwrap();
        assert!(done, "{what}");
        assert!(cards.join("OLD - transferred.bin").is_file());
        assert!(cards.join("OLD - transferred.board").is_dir());
        assert!(!new.exists() && slot.is_file());
        // a new card in the slot: the old one goes away under its club name, (2) when taken
        fs::write(cards.join("club card.bin"), b"x").unwrap();
        let (done, what) = switch("new", &slot, &cards).unwrap();
        assert!(
            done && !slot.exists() && cards.join("club card (2).bin").is_file(),
            "{what}"
        );
        assert_eq!(safe(" .a<b>c. "), "a_b_c");
        assert_eq!(safe(".."), "club card");
    }
}
