//! The 2017-18 control's Universal Championship day file, `game\log\<date>_uc.bin` (control 0x47D0D0): 3 match inits
//! (6,480 bytes each), the 3 match records (7,636 bytes each: semifinal 1, semifinal 2, final), then an MD5 of both.
//! The date is the control's business date, `YYYYMMDD` of the day before until 05:00 (0x47D800). A record's +8 counts
//! its teams (0..2), its team blocks (3,800 bytes) start at +36: +4 the CPU id (0 = an arcade player's team, 0x47DB20),
//! three 28-byte Shift-JIS names at +8, +36 and +64. The day's 4 Universal Cup winners fill the semifinals; empty places
//! get random CPU teams.

use std::path::{Path, PathBuf};

const RECORDS: usize = 3 * 6_480;
const RECORD: usize = 7_636;
const TEAM: usize = 3_800;

/// A team of the semifinals.
#[derive(Debug, PartialEq, Eq)]
pub struct Team {
    /// Its non-empty names, in the block's order.
    pub names: Vec<String>,
    /// An arcade player's team (CPU id 0), not a CPU team.
    pub player: bool,
}

/// The control's day file for `date` in `game`.
pub fn file(game: &Path, date: u32) -> PathBuf {
    game.join("log").join(format!("{date}_uc.bin"))
}

/// The control's business date at `now` (local time): the day before until 05:00.
pub fn date(now: time::OffsetDateTime) -> u32 {
    let d = (now - time::Duration::hours(5)).date();
    d.year().unsigned_abs() * 10_000 + u32::from(u8::from(d.month())) * 100 + u32::from(d.day())
}

/// The semifinals' teams in a day file's bytes (none from a short file).
pub fn teams(b: &[u8]) -> Vec<Team> {
    let u32_at = |at: usize| {
        b.get(at..at + 4)
            .map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap_or_default()))
    };
    let mut out = vec![];
    for m in 0..2 {
        let rec = RECORDS + m * RECORD;
        if b.len() < rec + RECORD {
            break;
        }
        for t in 0..(u32_at(rec + 8) as usize).min(2) {
            let blk = rec + 36 + t * TEAM;
            out.push(Team {
                names: [8, 36, 64]
                    .map(|n| super::gamesvr::name_text(&b[blk + n..blk + n + 28]))
                    .into_iter()
                    .filter(|n| !n.is_empty())
                    .collect(),
                player: u32_at(blk + 4) == 0,
            });
        }
    }
    out
}

/// The log's line for `date`: `championship: <date> teams: ...` (a player's team marked `[player]`), or why there is
/// none.
pub fn line(game: &Path, date: u32) -> String {
    let f = file(game, date);
    let Ok(b) = std::fs::read(&f) else {
        return format!(
            "championship: {date} no bracket ({} not there)",
            f.display()
        );
    };
    let teams: Vec<String> = teams(&b)
        .iter()
        .map(|t| {
            format!(
                "{}{}",
                t.names.join(" / "),
                if t.player { " [player]" } else { "" }
            )
        })
        .collect();
    if teams.is_empty() {
        return format!("championship: {date} teams: none");
    }
    format!("championship: {date} teams: {}", teams.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A day file with `teams` as (names, CPU id) in the semifinals' places, in order.
    fn day(teams: &[(&str, u32)]) -> Vec<u8> {
        let mut b = vec![0u8; RECORDS + 3 * RECORD + 16];
        for (i, (name, id)) in teams.iter().enumerate() {
            let rec = RECORDS + i / 2 * RECORD;
            b[rec + 8] += 1;
            let blk = rec + 36 + i % 2 * TEAM;
            b[blk + 4..blk + 8].copy_from_slice(&id.to_le_bytes());
            b[blk + 8..blk + 8 + name.len()].copy_from_slice(name.as_bytes());
        }
        b
    }

    #[test]
    fn the_day_file_gives_the_semifinal_teams_and_which_is_a_players() {
        let b = day(&[
            ("FC Madcaps", 0),
            ("CPU A", 950),
            ("CPU B", 951),
            ("CPU C", 952),
        ]);
        assert_eq!(b.len(), 42_348 + 16);
        let t = teams(&b);
        assert_eq!(t.len(), 4);
        assert_eq!(
            t[0],
            Team {
                names: vec!["FC Madcaps".into()],
                player: true
            }
        );
        assert!(t[1..].iter().all(|t| !t.player));
        assert_eq!(t[3].names, ["CPU C"]);
        // the second name field too, in its place
        let mut b = day(&[("A", 950)]);
        b[RECORDS + 36 + 36..][..5].copy_from_slice(b"Coach");
        assert_eq!(teams(&b)[0].names, ["A", "Coach"]);
        // the winner alone right after the cup's final; a cut file gives none
        assert_eq!(teams(&day(&[("FC Madcaps", 0)])).len(), 1);
        assert!(teams(&b[..RECORDS + 100]).is_empty());
    }

    #[test]
    fn the_line_names_the_teams_and_the_date_follows_the_05_00_roll() {
        let dir = std::env::temp_dir().join(format!("forza-uc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("log")).unwrap();
        assert!(line(&dir, 20261102).starts_with("championship: 20261102 no bracket ("));
        std::fs::write(
            file(&dir, 20261102),
            day(&[("FC Madcaps", 0), ("CPU A", 950)]),
        )
        .unwrap();
        assert_eq!(
            line(&dir, 20261102),
            "championship: 20261102 teams: FC Madcaps [player], CPU A"
        );
        std::fs::remove_dir_all(&dir).unwrap();
        let midnight = time::Date::from_calendar_date(2026, time::Month::November, 2)
            .unwrap()
            .midnight()
            .assume_utc();
        let at = |h| midnight + time::Duration::hours(h);
        assert_eq!(
            (date(at(0)), date(at(4)), date(at(5)), date(at(23))),
            (20261101, 20261101, 20261102, 20261102)
        );
    }
}
