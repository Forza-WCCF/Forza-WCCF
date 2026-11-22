use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// One card on the table: FPR_Emu's slot (1-20), the card number, and where it lies: x -1..1 touchline to touchline,
/// y -1 (the far goal line) .. 1 (the own goal line); x past 1.16 is the bench strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub slot: i32,
    pub no: u32,
    pub x: f32,
    pub y: f32,
}

impl Placed {
    pub fn on_bench(&self) -> bool {
        self.x > ON_BENCH
    }
}

/// The board holds at most 16 cards: 11 on the pitch and 5 on the bench (the table itself may hold more rows).
pub const MAX_CARDS: usize = 16;
/// The pitch's slots: the first of the board's 16, the bench's boxes follow them.
pub(crate) const PITCH: usize = 11;
/// Bench cards lie at x 1.24, y on one of the game's five band spots.
pub const BENCH_X: f32 = 1.24;
pub const BENCH_Y: [f32; 5] = [-0.68, -0.34, 0.0, 0.34, 0.68];
pub(super) const ON_BENCH: f32 = 1.16;

/// The bench band whose spot is nearest `y`.
pub fn bench_band(y: f32) -> usize {
    // the first of two as near, as the C panel's loop
    (0..5)
        .min_by(|&a, &b| (BENCH_Y[a] - y).abs().total_cmp(&(BENCH_Y[b] - y).abs()))
        .unwrap_or(0)
}

// ---- the table file ----

/// The table file's text -> its cards, by FPR_Emu's rules: `#` starts a comment, commas count as spaces, a line is
/// "slot card x y" with slot 1-20 and card 1 or more; anything else is skipped, and so is a position that is not a
/// finite number (NaN or an infinity would put the card on no bench and no spot). The rows past the board's 16 are kept:
/// the board shows the first 16 and the rest stay in the file, as FPR_Emu's slots go up to 20.
pub fn parse_table(text: &[u8]) -> Vec<Placed> {
    // shortcut: "3x" is skipped where the C panel's sscanf read 3; take leading digits if a table with such rows turns up
    String::from_utf8_lossy(text)
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or("").replace(',', " ");
            let mut w = line.split_whitespace();
            let slot: i32 = w.next()?.parse().ok()?;
            let no: i64 = w.next()?.parse().ok()?;
            let x: f32 = w.next()?.parse().ok()?;
            let y: f32 = w.next()?.parse().ok()?;
            let finite = x.is_finite() && y.is_finite();
            ((1..=20).contains(&slot) && no >= 1 && finite).then_some(Placed {
                slot,
                no: u32::try_from(no).ok()?,
                x,
                y,
            })
        })
        .collect()
}

/// The table file as the C panel writes it: its first line, then "slot card x y" with three decimals.
pub fn table_text(cards: &[Placed]) -> String {
    let mut t = String::from(
        "# written by the in-game card board (wccfpanel.dll) - read by FPR_Emu.exe (table 0)\n",
    );
    for c in cards {
        t += &format!("{} {} {:.3} {:.3}\n", c.slot, c.no, c.x, c.y);
    }
    t
}

/// fpr_panel_state.json's "mirror" choice, read as the C panel reads it: the first `"mirror"`, its colon, then `t`.
pub fn mirror_of(state: &str) -> bool {
    state
        .find("\"mirror\"")
        .and_then(|i| state[i..].find(':').map(|j| i + j + 1))
        .is_some_and(|i| {
            state[i..]
                .trim_start_matches([' ', '\t', '\r', '\n'])
                .starts_with('t')
        })
}

/// fpr_panel_state.json as the C panel writes it: the table's cards with x unmirrored (the table holds the mirrored x
/// when "mirror" is on), and the mirror choice kept.
pub fn state_text(cards: &[Placed], mirror: bool) -> String {
    let mut t = String::from("{\n \"placed\": [");
    for (i, c) in cards.iter().enumerate() {
        let x = if mirror && c.x <= ON_BENCH { -c.x } else { c.x };
        t += &format!(
            "{}\n  {{\"slot\": {}, \"no\": {}, \"x\": {:.3}, \"y\": {:.3}}}",
            if i > 0 { "," } else { "" },
            c.slot,
            c.no,
            x,
            c.y
        );
    }
    t += &format!("\n ],\n \"mirror\": {mirror}\n}}\n");
    t
}

/// The table and the formation copy in the game's own folder, and when we last read or wrote the table.
pub struct TableFile {
    pub table: PathBuf,
    pub state: PathBuf,
    stamp: Option<SystemTime>,
    /// A write could not be swapped in: the next try writes again, and the file is not read back meanwhile.
    pub pending: bool,
    /// The table is swapped in but the formation copy failed: only the copy is tried again (`retry_copy`).
    pub copy_owed: bool,
    copy_try: Option<std::time::Instant>,
    /// When a failed write was last tried: the retry comes at most once a second, as the copy's does.
    write_try: Option<std::time::Instant>,
    /// The owed write's failure was logged: one line per failed streak, not one per frame.
    write_logged: bool,
}

impl TableFile {
    /// `fpr_table0.txt` and `fpr_panel_state.json` in `dir` (the game's own folder: seat1\, or a test's).
    pub fn in_dir(dir: &Path) -> TableFile {
        TableFile {
            table: dir.join("fpr_table0.txt"),
            state: dir.join("fpr_panel_state.json"),
            stamp: None,
            pending: false,
            copy_owed: false,
            copy_try: None,
            write_try: None,
            write_logged: false,
        }
    }

    fn modified(&self) -> Option<SystemTime> {
        fs::metadata(&self.table).and_then(|m| m.modified()).ok()
    }

    /// Someone else wrote the table since we last read or wrote it (FPR_Emu's panel, a hand edit).
    pub fn changed(&self) -> bool {
        !self.pending && self.modified().is_some_and(|m| Some(m) != self.stamp)
    }

    /// The table's cards; `None` when it cannot be read (missing, or being swapped).
    pub fn read(&mut self) -> Option<Vec<Placed>> {
        let m = self.modified()?;
        let cards = parse_table(&fs::read(&self.table).ok()?);
        self.stamp = Some(m);
        Some(cards)
    }

    /// The table written whole (a temporary file, then one swap: FPR_Emu never sees half a file), then the formation
    /// copy. The temporary file gets a write time after the last one: FPR_Emu reads again only when the time
    /// changes, and two moves in one clock tick would look like one.
    pub fn write(&mut self, cards: &[Placed]) -> io::Result<()> {
        self.pending = true;
        let tmp = with_suffix(&self.table, ".board");
        let mut when = SystemTime::now();
        if let Some(last) = self.stamp
            && when <= last
        {
            when = last + Duration::from_millis(1);
        }
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(table_text(cards).as_bytes())?;
            f.set_modified(when)?;
        }
        swap_in(&tmp, &self.table, 4, 1)?;
        self.stamp = Some(when);
        self.pending = false;
        self.write_logged = false;
        // the copy is owed until it is written: the two files disagree meanwhile
        self.copy_owed = true;
        self.write_state(cards)?;
        self.copy_owed = false;
        Ok(())
    }

    /// The table written, the way the board does it: a failed write is retried at most once a second (as the copy is),
    /// and its error is returned only for the first failure of a streak, so the log gets one line.
    pub fn write_retry(&mut self, cards: &[Placed]) -> Option<io::Error> {
        let now = std::time::Instant::now();
        if self.pending
            && self
                .write_try
                .is_some_and(|t| now.duration_since(t) < Duration::from_secs(1))
        {
            return None;
        }
        self.write_try = Some(now);
        match self.write(cards) {
            Ok(()) => None,
            Err(e) => (!std::mem::replace(&mut self.write_logged, true)).then_some(e),
        }
    }

    /// The formation copy still owed: written again, at most once a second.
    pub fn retry_copy(&mut self, cards: &[Placed]) {
        let now = std::time::Instant::now();
        if !self.copy_owed
            || self
                .copy_try
                .is_some_and(|t| now.duration_since(t) < Duration::from_secs(1))
        {
            return;
        }
        self.copy_try = Some(now);
        self.copy_owed = self.write_state(cards).is_err();
    }

    fn write_state(&self, cards: &[Placed]) -> io::Result<()> {
        let mirror = self.read_mirror()?;
        let tmp = with_suffix(&self.state, ".board");
        fs::write(&tmp, state_text(cards, mirror))?;
        swap_in(&tmp, &self.state, 20, 2)
    }

    /// The state file's mirror choice: a file that is not there is off; one that cannot be read for a moment (a sharing
    /// violation) is an error, so the write waits for it rather than saving "mirror": false over it.
    fn read_mirror(&self) -> io::Result<bool> {
        match fs::read(&self.state) {
            Ok(b) => Ok(mirror_of(&String::from_utf8_lossy(&b))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

/// `tmp` over `to` (MoveFileEx with replace); another program (the reader, a scanner) may hold either this instant.
/// Contract: up to `tries` renames `ms` apart while the error is a sharing or lock violation or access denied; any other
/// error returns at once, and `tries` 0 is an error, never a silent Ok.
pub fn swap_in(tmp: &Path, to: &Path, tries: u32, ms: u64) -> io::Result<()> {
    let mut last = Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "swap_in: no tries",
    ));
    for n in 0..tries {
        if n > 0 {
            std::thread::sleep(Duration::from_millis(ms));
        }
        last = fs::rename(tmp, to);
        match &last {
            Err(e)
                if e.kind() == io::ErrorKind::PermissionDenied
                    || matches!(e.raw_os_error(), Some(32 | 33)) => {}
            _ => break,
        }
    }
    last
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(crate) fn p(slot: i32, no: u32, x: f32, y: f32) -> Placed {
        Placed { slot, no, x, y }
    }

    /// What the C panel's `write_table_locked` writes for these cards: its header line, then
    /// `"%d %d %.3f %.3f\n"` per card (the MSVC runtime's printf), no carriage returns.
    #[test]
    fn the_table_file_is_the_c_panels() {
        let cards = [
            p(1, 101, 0.0, 0.9),
            p(2, 2345, -0.6, 0.55),
            p(3, 7, 1.24, -0.68),
            p(16, 40000, 0.25, -0.45),
            p(5, 9, -0.0, 1.0),
        ];
        let want = b"# written by the in-game card board (wccfpanel.dll) - read by FPR_Emu.exe (table 0)\n\
1 101 0.000 0.900\n\
2 2345 -0.600 0.550\n\
3 7 1.240 -0.680\n\
16 40000 0.250 -0.450\n\
5 9 -0.000 1.000\n";
        assert_eq!(table_text(&cards).as_bytes(), want);
        assert_eq!(parse_table(want), cards);
        assert_eq!(
            table_text(&[]).as_bytes(),
            &b"# written by the in-game card board (wccfpanel.dll) - read by FPR_Emu.exe (table 0)\n"[..]
        );
    }

    /// FPR_Emu's reading rules (`parse_table`): comments, commas, bad slots and card numbers, short lines, positions
    /// that are not finite; and the rows past the board's 16 are kept, as the file's slots go up to 20.
    #[test]
    fn the_table_is_read_by_fpr_emus_rules() {
        let text = b"# a note\r\n1,101,0.1,0.2 # the keeper\r\n0 5 0 0\n21 5 0 0\n3 0 0 0\n4 5 0\n\n  7   8  -0.5  0.25\n";
        assert_eq!(
            parse_table(text),
            [p(1, 101, 0.1, 0.2), p(7, 8, -0.5, 0.25)]
        );
        // a NaN or an infinity would put the card on no bench and no spot: the row is skipped
        assert_eq!(
            parse_table(b"1 101 NaN 0.5\n2 102 inf -inf\n3 103 1e999 0\n4 104 0.1 -0.2\n"),
            [p(4, 104, 0.1, -0.2)]
        );
        // 20 rows: all of them are read, so the next save writes them back
        let many: String = (1..=20).map(|s| format!("{s} {s} 0 0\n")).collect();
        let rows = parse_table(many.as_bytes());
        assert_eq!(rows.len(), 20);
        assert_eq!(parse_table(table_text(&rows).as_bytes()), rows);
    }

    /// `write_panel_state_locked`: the mirror choice as the old file had it, a pitch card's x unmirrored, a bench
    /// card's x as it is; no cards gives an empty list.
    #[test]
    fn the_formation_copy_is_the_c_panels() {
        let cards = [p(1, 101, 0.6, 0.55), p(2, 7, 1.24, 0.34)];
        assert_eq!(
            state_text(&cards, false),
            "{\n \"placed\": [\n  {\"slot\": 1, \"no\": 101, \"x\": 0.600, \"y\": 0.550},\n  \
             {\"slot\": 2, \"no\": 7, \"x\": 1.240, \"y\": 0.340}\n ],\n \"mirror\": false\n}\n"
        );
        assert_eq!(
            state_text(&cards, true),
            "{\n \"placed\": [\n  {\"slot\": 1, \"no\": 101, \"x\": -0.600, \"y\": 0.550},\n  \
             {\"slot\": 2, \"no\": 7, \"x\": 1.240, \"y\": 0.340}\n ],\n \"mirror\": true\n}\n"
        );
        assert_eq!(
            state_text(&[], false),
            "{\n \"placed\": [\n ],\n \"mirror\": false\n}\n"
        );
        assert!(mirror_of("{\"placed\": [], \"mirror\":\n  true}"));
        assert!(!mirror_of("{\"mirror\": false}"));
        assert!(!mirror_of("{}"));
    }

    /// The table and its copy written beside each other, temporary files gone, the copy keeping the mirror; a
    /// second write in the same instant still gets a later write time.
    #[test]
    fn the_files_are_swapped_in_whole() {
        let d = tempfile::tempdir().unwrap();
        let mut f = TableFile::in_dir(d.path());
        fs::write(&f.state, "{\"placed\": [], \"mirror\": true}").unwrap();
        let cards = vec![p(1, 101, 0.5, 0.5)];
        f.write(&cards).unwrap();
        let first = fs::metadata(&f.table).unwrap().modified().unwrap();
        f.write(&cards).unwrap();
        let second = fs::metadata(&f.table).unwrap().modified().unwrap();
        assert!(second > first, "{first:?} then {second:?}");
        assert!(!f.changed());
        assert_eq!(fs::read_to_string(&f.table).unwrap(), table_text(&cards));
        assert_eq!(
            fs::read_to_string(&f.state).unwrap(),
            state_text(&cards, true)
        );
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 2);
        assert_eq!(f.read().unwrap(), cards);
    }

    /// A write that keeps failing is retried at most once a second and logged once per streak: a folder where the
    /// table goes can never be swapped in.
    #[test]
    fn a_failing_write_is_retried_once_a_second_and_logged_once() {
        let d = tempfile::tempdir().unwrap();
        let mut f = TableFile::in_dir(d.path());
        let one = [p(1, 101, 0.5, 0.5)];
        let two = [p(1, 102, 0.5, 0.5)];
        assert!(
            f.write_retry(&one).is_none(),
            "a write that goes in is no line"
        );
        assert_eq!(fs::read_to_string(&f.table).unwrap(), table_text(&one));
        fs::remove_file(&f.table).unwrap();
        fs::create_dir(&f.table).unwrap();
        assert!(f.write_retry(&two).is_some(), "the first failure is logged");
        assert!(f.pending);
        assert!(
            f.write_retry(&one).is_none(),
            "a retry within the second is skipped"
        );
        assert_eq!(
            fs::read_to_string(with_suffix(&f.table, ".board")).unwrap(),
            table_text(&two),
            "the skipped retry wrote nothing"
        );
    }

    /// `swap_in`'s contract: no tries is an error, an error that cannot clear (no `tmp`) returns at once, and there is
    /// no wait after the last try.
    #[test]
    fn swap_in_tries_only_what_can_clear() {
        let d = tempfile::tempdir().unwrap();
        let (tmp, to) = (d.path().join("a.tmp"), d.path().join("a"));
        fs::write(&tmp, "x").unwrap();
        assert_eq!(
            swap_in(&tmp, &to, 0, 0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(tmp.exists() && !to.exists());
        let t0 = std::time::Instant::now();
        let gone = d.path().join("gone.tmp");
        assert_eq!(
            swap_in(&gone, &to, 50, 20).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(
            t0.elapsed() < Duration::from_millis(500),
            "no retry: {:?}",
            t0.elapsed()
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let _held = fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&tmp)
                .unwrap(); // no delete sharing
            let t0 = std::time::Instant::now();
            assert!(swap_in(&tmp, &to, 1, 1000).is_err());
            assert!(
                t0.elapsed() < Duration::from_millis(500),
                "no wait after the last try: {:?}",
                t0.elapsed()
            );
        }
        swap_in(&tmp, &to, 1, 0).unwrap();
        assert!(to.exists() && !tmp.exists());
    }

    /// A state file that cannot be read for a moment (a sharing violation, a folder where it was) is not written over
    /// with "mirror": false from the session's first write: the copy waits, and is tried again once the file reads.
    #[test]
    fn a_state_file_that_cannot_be_read_is_not_written_over() {
        let d = tempfile::tempdir().unwrap();
        let mut f = TableFile::in_dir(d.path());
        let t = [p(1, 101, 0.5, 0.5)];
        fs::create_dir(&f.state).unwrap();
        assert!(f.write(&t).is_err(), "the copy waits for the file to read");
        assert!(!f.pending && f.copy_owed && f.state.is_dir());
        fs::remove_dir(&f.state).unwrap();
        fs::write(&f.state, "{\"placed\": [], \"mirror\": true}").unwrap();
        f.retry_copy(&t);
        assert!(!f.copy_owed);
        assert_eq!(fs::read_to_string(&f.state).unwrap(), state_text(&t, true));
        fs::remove_file(&f.state).unwrap();
        assert!(!f.read_mirror().unwrap(), "a missing file is off");
    }

    /// A failed formation copy leaves only the copy owed: the table is swapped in (not pending, edits seen), and
    /// `retry_copy` writes the copy alone.
    #[test]
    fn a_failed_copy_stays_owed() {
        let d = tempfile::tempdir().unwrap();
        let mut f = TableFile::in_dir(d.path());
        let t = [p(1, 101, 0.5, 0.5)];
        fs::create_dir(&f.state).unwrap(); // a folder where the copy goes: the swap fails
        assert!(f.write(&t).is_err());
        assert!(!f.pending && f.copy_owed);
        assert!(!f.changed());
        fs::remove_dir(&f.state).unwrap();
        f.retry_copy(&t);
        assert!(!f.copy_owed && f.state.is_file());
    }
}
