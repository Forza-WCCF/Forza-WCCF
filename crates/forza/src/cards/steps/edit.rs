//! The CLUB CARD panel's CLEAR BAD ENDINGS (port of `legacy/2010-11/code/cards/edit.py`'s card writing and
//! `clear_bad_endings`): the game counts a session cut before its locker-room save in USER_INJUSTICE_NUM (at 2 trade
//! rights are lost, from the 5th it costs money) and marks a session open in USER_INJUSTICE_FLAG; the WAN pair counts
//! the same for online play. All four go back to 0, in both copies with the checksum the game writes (sum + 1), the
//! changed card checked with the game's own rules and the old card copied into backup\ first.

use super::decode::{self as d, CardFile, Decoded};
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

/// Backups kept in backup\ (the card reader keeps the same number).
const BACKUP_KEEP: usize = 20;

const BAD_ENDING_FIELDS: [(&str, &str); 4] = [
    ("UserInfo", "USER_INJUSTICE_NUM"),
    ("UserInfo", "USER_INJUSTICE_FLAG"),
    ("UserInfo", "USER_WAN_INJUSTICE_NUM"),
    ("SystemInfo", "USER_WAN_INJUSTICE_FLAG"),
];

/// The copy the game would read; refuses the cards the game refuses (edit.good_copy).
pub fn good_copy(card: &CardFile) -> Result<(char, Vec<u8>), String> {
    if card.is_new() {
        return Err(
            "this is a NEW card (nothing on it yet) - put it in the game once, then edit".into(),
        );
    }
    match d::game_choice(card) {
        (_, _, Some(which), _) => Ok((which, card.copy(Some(which)).to_vec())),
        (_, _, None, verdict) => Err(format!("the game would not read this card: {verdict}")),
    }
}

/// The finished copy in both copy areas, with the checksum the game itself writes (edit.stamp_copy).
pub fn stamp_copy(card: &CardFile, copy: &[u8]) -> Vec<u8> {
    let mut copy = copy.to_vec();
    let s = d::dword_sum(&copy[..d::PAYLOAD_BYTES]).wrapping_add(1);
    copy[d::PAYLOAD_BYTES..d::COPY_BYTES].copy_from_slice(&s.to_le_bytes());
    let mut out = card.clone();
    for block in [8, 113] {
        out.body[block * 16..block * 16 + d::COPY_BYTES].copy_from_slice(&copy);
    }
    out.bytes()
}

/// The game's own acceptance rules over a finished file (edit.check).
pub fn check(raw: &[u8]) -> Result<&'static str, String> {
    let card = CardFile::from_bytes(raw).ok_or("not a card file")?;
    let (_, _, chosen, verdict) = d::game_choice(&card);
    let bad: Vec<String> = d::card_checks(&card)
        .into_iter()
        .filter(|c| !c.2)
        .map(|c| format!("{}: {}", c.0, c.1))
        .collect();
    if chosen.is_none() || !bad.is_empty() {
        let more = if bad.is_empty() {
            String::new()
        } else {
            format!("; {}", bad.join("; "))
        };
        return Err(format!(
            "the changed card would not be accepted ({verdict}{more})"
        ));
    }
    Ok(verdict)
}

/// (the card file with the bad-ending fields at 0, the old values in BAD_ENDING_FIELDS' order); None for the file when
/// all were 0 already (edit.cleared_bad_endings).
pub fn cleared_bad_endings(card: &CardFile) -> Result<(Option<Vec<u8>>, [u32; 4]), String> {
    let (_, mut copy) = good_copy(card)?;
    let vals = Decoded::of(&copy);
    let mut old = [0; 4];
    for (k, (group, name)) in BAD_ENDING_FIELDS.into_iter().enumerate() {
        let (.., count, bits, first) = d::field(group, 0, name);
        old[k] = vals.one(group, name);
        d::write_bits(&mut copy, first, count * bits, 0);
    }
    if old.iter().all(|&v| v == 0) {
        return Ok((None, old));
    }
    let raw = stamp_copy(card, &copy);
    check(&raw)?;
    Ok((Some(raw), old))
}

/// `YYYYMMDD-HHMMSS` on this PC's clock (time.strftime).
fn stamp() -> String {
    let utc = time::OffsetDateTime::now_utc();
    let t = time::UtcOffset::local_offset_at(utc).map_or(utc, |o| utc.to_offset(o));
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// The reader's own way of saving a card: the old card copied into backup\ first (the newest BACKUP_KEEP kept), then a
/// temporary file flushed to the disk and one replace (edit.save) -> the backup.
pub fn save(path: &Path, raw: &[u8]) -> Result<std::path::PathBuf, String> {
    let abs = std::path::absolute(path).map_err(|e| e.to_string())?;
    let folder = abs.parent().unwrap_or(Path::new(".")).join("backup");
    let base = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let dst = folder.join(format!("{base}.{}.bak", stamp()));
    let backup = || -> io::Result<()> {
        fs::create_dir_all(&folder)?;
        fs::write(&dst, fs::read(path)?)?;
        let mut old: Vec<String> = fs::read_dir(&folder)?
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|f| f.starts_with(&format!("{base}.")) && f.ends_with(".bak"))
            .collect();
        old.sort();
        for f in &old[..old.len().saturating_sub(BACKUP_KEEP)] {
            fs::remove_file(folder.join(f))?;
        }
        Ok(())
    };
    backup()
        .map_err(|e| format!("the old card could not be backed up ({e}) - nothing was written"))?;
    let tmp = path.with_file_name(format!("{base}.tmp"));
    let mut f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    f.write_all(raw)
        .and_then(|()| f.sync_all())
        .map_err(|e| e.to_string())?;
    drop(f);
    for attempt in 0..50 {
        match fs::rename(&tmp, path) {
            Ok(()) => return Ok(dst),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && attempt < 49 => {
                sleep(Duration::from_millis(20));
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                return Err(
                    "the card file stayed busy for a second - nothing was written (is the game up?)".into(),
                );
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    unreachable!()
}

/// The card at `path` with its bad endings back to 0 (the old card kept in backup\ first) -> what was done, in words.
pub fn clear_bad_endings(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Ok("no card in the slot - nothing to clear".into());
    }
    let raw = fs::read(path).map_err(|e| e.to_string())?;
    let card = CardFile::from_bytes(&raw).ok_or_else(|| {
        format!(
            "{} is {} bytes; a card file is {} ({} without its header)",
            path.display(),
            raw.len(),
            d::CARD_BYTES,
            d::RAW_BYTES
        )
    })?;
    let (Some(raw), old) = cleared_bad_endings(&card)? else {
        return Ok("no bad endings to clear".into());
    };
    save(path, &raw)?;
    let wan = if old[2] == 0 {
        String::new()
    } else {
        format!(", online {}", old[2])
    };
    Ok(format!(
        "bad endings cleared ({}{wan} back to 0; the old card is in data\\save\\backup)",
        old[0]
    ))
}

#[cfg(test)]
mod tests {
    use super::super::decode::tests::{card_with, set};
    use super::*;

    /// Port of edit.py's self-test (the packing, the checksum and the acceptance check) and its bad-endings part.
    #[test]
    fn self_test() {
        let raw = card_with(0x0010, &[0u8; d::COPY_BYTES]);
        assert!(check(&raw).is_ok(), "a fresh copy passes the game's checks");
        let card = CardFile::from_bytes(&raw).unwrap();
        let (_, mut copy) = good_copy(&card).unwrap();
        let base = copy.clone();
        set(&mut copy, "Club", 0, "CLUB_GET_PRIZE", &[12345]);
        assert_eq!(
            Decoded::of(&copy).one("Club", "CLUB_GET_PRIZE"),
            12345,
            "a written field comes back"
        );
        let (.., bits, first) = d::field("Club", 0, "CLUB_GET_PRIZE");
        assert!(
            (0..copy.len())
                .filter(|&i| copy[i] != base[i])
                .all(|i| first / 8 <= i && i <= (first + bits - 1) / 8),
            "only the field's own bytes change"
        );
        set(&mut copy, "Club", 0, "CLUB_GET_PRIZE", &[0]);
        assert_eq!(copy, base, "writing the old value back restores the bytes");
        set(
            &mut copy,
            "Club",
            0,
            "CLUB_NAME",
            &b"TEST CLUB".map(u32::from),
        );
        assert_eq!(
            d::text(&Decoded::of(&copy).get("Club", 0, "CLUB_NAME")),
            "TEST CLUB"
        );
        let raw2 = stamp_copy(&card, &copy);
        let c2 = CardFile::from_bytes(&raw2).unwrap();
        assert_eq!(
            c2.copy(Some('A')),
            c2.copy(Some('B')),
            "both copies carry the change"
        );
        assert!(check(&raw2).is_ok(), "the changed card is accepted");
        assert!(good_copy(&CardFile::from_bytes(&card_with(0xFFFF, &copy)).unwrap()).is_err());

        let mut copy = vec![0u8; d::COPY_BYTES]; // bad endings: 2 counted, a session left open
        set(&mut copy, "UserInfo", 0, "USER_INJUSTICE_NUM", &[2]);
        set(&mut copy, "UserInfo", 0, "USER_INJUSTICE_FLAG", &[1]);
        let c4 = CardFile::from_bytes(&card_with(0x0010, &copy)).unwrap();
        let (raw4, old4) = cleared_bad_endings(&c4).unwrap();
        assert_eq!(old4, [2, 1, 0, 0]);
        let after = Decoded::of(
            &good_copy(&CardFile::from_bytes(&raw4.unwrap()).unwrap())
                .unwrap()
                .1,
        );
        let before = Decoded::of(&good_copy(&c4).unwrap().1);
        let changed: Vec<_> = d::LAYOUT
            .iter()
            .filter(|f| after.get(f.0, f.1, f.2) != before.get(f.0, f.1, f.2))
            .map(|f| f.2)
            .collect();
        assert_eq!(
            changed,
            ["USER_INJUSTICE_FLAG", "USER_INJUSTICE_NUM"],
            "the rest is unchanged"
        );
        assert!(
            cleared_bad_endings(&card).unwrap().0.is_none(),
            "a card without bad endings is left alone"
        );
    }

    #[test]
    fn clear_saves_with_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("seat1_club.bin");
        assert_eq!(
            clear_bad_endings(&p).unwrap(),
            "no card in the slot - nothing to clear"
        );
        let mut copy = vec![0u8; d::COPY_BYTES];
        set(&mut copy, "UserInfo", 0, "USER_INJUSTICE_NUM", &[3]);
        set(&mut copy, "UserInfo", 0, "USER_WAN_INJUSTICE_NUM", &[1]);
        let raw = card_with(0x0010, &copy);
        fs::write(&p, &raw).unwrap();
        assert_eq!(
            clear_bad_endings(&p).unwrap(),
            "bad endings cleared (3, online 1 back to 0; the old card is in data\\save\\backup)"
        );
        let baks: Vec<_> = fs::read_dir(dir.path().join("backup")).unwrap().collect();
        assert_eq!(baks.len(), 1);
        assert_eq!(fs::read(baks[0].as_ref().unwrap().path()).unwrap(), raw);
        assert_eq!(clear_bad_endings(&p).unwrap(), "no bad endings to clear");
    }
}
