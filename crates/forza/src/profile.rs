//! The player's profiles (2017-18): free play and Create-a-Team. Each keeps its own save (access code, club, the game
//! server stand-in's store) and its own card table; settings.json "profile" names the active one.
//! - Free play's save stays `data\save`, as before profiles: never moved, copied or migrated.
//! - Create-a-Team's save is `data\profiles\create-a-team\save`: a first run makes its own access code there, so the
//!   game sees a new card and asks for a new club.
//! - The card table: FPR_Emu and the panel read it by fixed names in the seat folder, so a run's start copies the active
//!   profile's table in from `data\profiles\<profile>\board` and its end copies it back. A marker in the seat folder
//!   ([`MARKER`]) names the profile whose table is there; without it the table is free play's, as before profiles. A
//!   crash between the copies is put right at the next start: the table there goes to the profile the marker names.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// settings.json's key for the active profile.
pub const KEY: &str = "profile";
/// The seat folder's marker: the profile whose card table is there; empty while a switch is half done (the table there
/// was already copied to its profile).
pub const MARKER: &str = "fpr_board_profile.txt";
/// The card table's files in the seat folder: the table, the board's copy of it, the 2010-11 board's owner, and the
/// panel's RESET LINEUP and UNDO copies.
pub const BOARD: [&str; 5] = [
    "fpr_table0.txt",
    "fpr_panel_state.json",
    "fpr_board_owner.txt",
    "fpr_table0.before-match.txt",
    "fpr_table0.before-import.txt",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    #[default]
    Free,
    CreateATeam,
}

impl Profile {
    pub const ALL: [Profile; 2] = [Profile::Free, Profile::CreateATeam];

    /// The name in settings.json and in the folder names.
    pub fn code(self) -> &'static str {
        match self {
            Profile::Free => "free",
            Profile::CreateATeam => "create-a-team",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.code() == s)
    }

    /// settings.json's "profile": free play when missing or unknown.
    pub fn of(settings: &serde_json::Value) -> Self {
        settings[KEY]
            .as_str()
            .and_then(Self::from_code)
            .unwrap_or_default()
    }

    /// The profile's save folder in the kit's `data`.
    pub fn save(self, data: &Path) -> PathBuf {
        match self {
            Profile::Free => data.join("save"),
            Profile::CreateATeam => data.join("profiles").join(self.code()).join("save"),
        }
    }

    /// The profile's collection folder (`grants.jsonl`): Create-a-Team only, free play has none.
    pub fn collection(self, data: &Path) -> Option<PathBuf> {
        (self == Profile::CreateATeam)
            .then(|| data.join("profiles").join(self.code()).join("collection"))
    }

    /// The profile's card table while another profile's is in the seat folder.
    pub fn board(self, data: &Path) -> PathBuf {
        data.join("profiles").join(self.code()).join("board")
    }
}

/// The profile whose table the seat folder holds: free play without a marker, `None` while a switch is half done. A
/// marker naming no profile is an error: its table is nobody's to overwrite.
// shortcut: the marker is the only witness; a lost one (a seat folder copied from another install, a hand deletion)
// means free play, so a Create-a-Team table left there goes to free play's board (free play's own kept as `.prev`).
// Upgrade if a launcher path loses it.
fn held(seat: &Path) -> io::Result<Option<Profile>> {
    match fs::read_to_string(seat.join(MARKER)) {
        Ok(s) if s.trim().is_empty() => Ok(None),
        Ok(s) => Profile::from_code(s.trim()).map(Some).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} names no profile: {:?}", MARKER, s.trim()),
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Some(Profile::Free)),
        Err(e) => Err(e),
    }
}

/// What [`mirror`] keeps of a file it replaces or removes in `to`, when that file differs.
#[derive(Clone, Copy, PartialEq)]
enum Keep {
    /// Nothing: the seat folder, whose table was already copied to its profile.
    Nothing,
    /// A table with cards that an empty or missing one replaces (a half-written or lost seat table at a run's end).
    Cards,
    /// Every file (a switch: the table may be another profile's, a lost marker).
    All,
}

/// [`BOARD`]'s files from `from` to `to`, a file `from` lacks removed from `to`; what it replaces is kept first as
/// [`Keep`] says ([`keep_aside`]). A run's end ([`Keep::Cards`]) removes nothing: the panel never deletes these files, so
/// one the seat lacks was lost, and the profile's copy (RESET LINEUP's kept table, the panel state) stays.
fn mirror(from: &Path, to: &Path, keep: Keep) -> io::Result<()> {
    fs::create_dir_all(to)?;
    let blank = |f: &[u8]| f.iter().all(u8::is_ascii_whitespace);
    for n in BOARD {
        let (a, b) = (from.join(n), to.join(n));
        if keep != Keep::Nothing && b.is_file() {
            let (new, old) = (fs::read(&a).ok(), fs::read(&b)?);
            let emptied = n == BOARD[0] && !blank(&old) && new.as_deref().is_none_or(blank);
            if new.as_ref() != Some(&old) && (keep == Keep::All || emptied) {
                keep_aside(to, n)?;
            }
        }
        if a.is_file() {
            fs::copy(&a, &b)?;
        } else if b.is_file() && keep != Keep::Cards {
            fs::remove_file(&b)?;
        }
    }
    Ok(())
}

/// `dir`'s file `n` moved to `<n>.prev`, or `<n>.prev2`, `<n>.prev3`... when those are taken: a kept copy never
/// replaces an older one. They come only from a lost marker, a crash before the copy back or an emptied table, so few.
fn keep_aside(dir: &Path, n: &str) -> io::Result<()> {
    let free = (1..)
        .map(|i| match i {
            1 => dir.join(format!("{n}.prev")),
            i => dir.join(format!("{n}.prev{i}")),
        })
        .find(|p| !p.exists())
        .expect("a free name");
    fs::rename(dir.join(n), free)
}

/// The marker through a temporary file and one rename: a power cut never leaves a torn name that refuses every start.
fn mark(seat: &Path, code: &str) -> io::Result<()> {
    crate::kit::write_atomic(&seat.join(MARKER), code.as_bytes())
}

/// A run's start: the seat folder gets `p`'s card table. The table there is first copied to the profile it belongs to;
/// only then is it replaced. Each step is safe to repeat, so a start that stops halfway is finished by the next.
/// A seat folder with no table (rebuilt by UNDO SETUP and SET UP, or a new game folder) holds nobody's table: it is
/// never copied over a profile's board, and `p`'s saved table, when it has one, is copied in.
pub fn board_in(data: &Path, seat: &Path, p: Profile) -> io::Result<()> {
    let held = held(seat)?;
    let empty = !seat.join(BOARD[0]).is_file();
    if held == Some(p) {
        // free play that never switched has no board folder: nothing moves, nothing new on disk
        if empty && p.board(data).join(BOARD[0]).is_file() {
            mirror(&p.board(data), seat, Keep::Nothing)?;
            mark(seat, p.code())?;
        }
        return Ok(());
    }
    if let Some(h) = held {
        if !empty {
            // a lost marker names free play for another profile's table: free play's own table stays as `.prev`
            mirror(seat, &h.board(data), Keep::All)?;
        }
        mark(seat, "")?;
    }
    mirror(&p.board(data), seat, Keep::Nothing)?;
    mark(seat, p.code())
}

/// RESET PROFILE (`PLAN.md` 1.5): Create-a-Team's whole folder (collection, save, table) moved in one rename to
/// `data\profiles\archive\create-a-team-<stamp>` (`-2`, `-3`... when taken), the archive returned. Nothing is deleted
/// and free play's save and table are never touched. The seat folder's table, when it is Create-a-Team's, goes to
/// its board first (the old profile is whole after that step too) and leaves the seat folder empty, so the rename is the one step that changes the
/// profile: a reset stopped anywhere leaves the old profile whole or the new one empty. The next start makes a new
/// access code (a new club) and finds the table empty; the next look at the collection offers the first start.
/// `data\market.key` and the welcome mark live outside the folder and stay.
pub fn reset(data: &Path, seat: Option<&Path>, stamp: &str) -> io::Result<PathBuf> {
    reset_with(data, seat, stamp, &mut |_| Ok(()))
}

/// [`reset`] with `step` called before each of its steps (0: park the seat's table, 1: empty the marker, 2: empty the
/// seat's table, 3: make the archive folder, 4: the rename): a test makes one fail.
fn reset_with(
    data: &Path,
    seat: Option<&Path>,
    stamp: &str,
    step: &mut dyn FnMut(u8) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let p = Profile::CreateATeam;
    let from = data.join("profiles").join(p.code());
    if !from.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} does not exist", from.display()),
        ));
    }
    if let Some(seat) = seat
        && held(seat)? == Some(p)
    {
        // what a switch away does (`board_in`): the board then holds the whole table and the marker says so
        step(0)?;
        // every file the seat holds, not only with the table there: the emptying below deletes them all
        if BOARD.iter().any(|n| seat.join(n).is_file()) {
            mirror(seat, &p.board(data), Keep::All)?;
        }
        step(1)?;
        mark(seat, "")?;
        // the seat empty too: a game started without Launcher must not read the archived club's table
        step(2)?;
        for n in BOARD {
            let f = seat.join(n);
            if f.is_file() {
                fs::remove_file(f)?;
            }
        }
    }
    step(3)?;
    let archive = data.join("profiles").join("archive");
    fs::create_dir_all(&archive)?;
    let to = (1..)
        .map(|i| match i {
            1 => archive.join(format!("{}-{stamp}", p.code())),
            i => archive.join(format!("{}-{stamp}-{i}", p.code())),
        })
        .find(|p| !p.exists())
        .expect("a free name");
    step(4)?;
    fs::rename(&from, &to)?;
    Ok(to)
}

/// A run's end: the seat folder's table copied back to `p`'s board folder when it is `p`'s. Free play's table is never
/// copied while no switch was ever made (no marker): it stays in the seat folder only, as before profiles.
pub fn board_out(data: &Path, seat: &Path, p: Profile) -> io::Result<()> {
    if seat.join(MARKER).is_file() && held(seat)? == Some(p) {
        mirror(seat, &p.board(data), Keep::Cards)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(seat: &Path) -> Option<String> {
        fs::read_to_string(seat.join("fpr_table0.txt")).ok()
    }

    #[test]
    fn the_profile_comes_from_settings_and_free_play_is_the_default() {
        use serde_json::json;
        assert_eq!(Profile::of(&json!({})), Profile::Free);
        assert_eq!(Profile::of(&json!({"profile": "free"})), Profile::Free);
        assert_eq!(
            Profile::of(&json!({"profile": "create-a-team"})),
            Profile::CreateATeam
        );
        assert_eq!(Profile::of(&json!({"profile": "career"})), Profile::Free);
        assert_eq!(Profile::of(&json!({"profile": 1})), Profile::Free);
    }

    /// (c) A run that ends without the copy back (a crash): the next start still gives each profile its own table, and
    /// free play's first table is never lost.
    #[test]
    fn a_crashed_run_leaves_each_profile_its_own_table() {
        let d = tempfile::tempdir().unwrap();
        let (data, seat) = (d.path().join("data"), d.path().join("seat1"));
        fs::create_dir_all(&seat).unwrap();
        fs::write(seat.join("fpr_table0.txt"), "1 100 0 0\r\n").unwrap();
        fs::write(seat.join("fpr_panel_state.json"), "{\"free\":1}").unwrap();
        fs::write(seat.join("fpr_table0.before-match.txt"), "free kept").unwrap();
        // free play with no switch yet: nothing moves, nothing new on disk
        board_in(&data, &seat, Profile::Free).unwrap();
        board_out(&data, &seat, Profile::Free).unwrap();
        assert!(!seat.join(MARKER).exists() && !data.exists());
        // Create-a-Team's first start: an empty table, free play's kept in its board folder
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        assert_eq!(table(&seat), None);
        assert!(!seat.join("fpr_table0.before-match.txt").exists());
        assert_eq!(
            fs::read_to_string(Profile::Free.board(&data).join("fpr_table0.txt")).unwrap(),
            "1 100 0 0\r\n"
        );
        // its run lays a table, then crashes: no copy back
        fs::write(seat.join("fpr_table0.txt"), "1 200 0 0\r\n").unwrap();
        // free play's next start puts its own table back and keeps Create-a-Team's
        board_in(&data, &seat, Profile::Free).unwrap();
        assert_eq!(table(&seat).as_deref(), Some("1 100 0 0\r\n"));
        assert_eq!(
            fs::read_to_string(seat.join("fpr_panel_state.json")).unwrap(),
            "{\"free\":1}"
        );
        assert_eq!(
            fs::read_to_string(seat.join("fpr_table0.before-match.txt")).unwrap(),
            "free kept"
        );
        assert_eq!(fs::read_to_string(seat.join(MARKER)).unwrap(), "free");
        // free play's run changes its table and crashes too; Create-a-Team gets its 200 back
        fs::write(seat.join("fpr_table0.txt"), "1 101 0 0\r\n").unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        assert_eq!(table(&seat).as_deref(), Some("1 200 0 0\r\n"));
        assert!(!seat.join("fpr_panel_state.json").exists());
        // a clean end copies back; the next free start finds free play's latest
        board_out(&data, &seat, Profile::CreateATeam).unwrap();
        board_in(&data, &seat, Profile::Free).unwrap();
        assert_eq!(table(&seat).as_deref(), Some("1 101 0 0\r\n"));
    }

    /// A switch stopped after the table went to its profile (marker empty): the next start only copies the new table in.
    #[test]
    fn a_half_done_switch_is_finished_by_the_next_start() {
        let d = tempfile::tempdir().unwrap();
        let (data, seat) = (d.path().join("data"), d.path().join("seat1"));
        for p in Profile::ALL {
            fs::create_dir_all(p.board(&data)).unwrap();
        }
        fs::create_dir_all(&seat).unwrap();
        fs::write(
            Profile::CreateATeam.board(&data).join("fpr_table0.txt"),
            "cat",
        )
        .unwrap();
        fs::write(Profile::Free.board(&data).join("fpr_table0.txt"), "free").unwrap();
        fs::write(seat.join("fpr_table0.txt"), "half copied").unwrap();
        fs::write(seat.join(MARKER), "").unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        assert_eq!(table(&seat).as_deref(), Some("cat"));
        assert_eq!(
            fs::read_to_string(Profile::Free.board(&data).join("fpr_table0.txt")).unwrap(),
            "free",
            "the half-copied table overwrote no profile's"
        );
        // a marker naming no profile: the table stays where it is
        fs::write(seat.join(MARKER), "career").unwrap();
        assert!(board_in(&data, &seat, Profile::Free).is_err());
        assert_eq!(table(&seat).as_deref(), Some("cat"));
    }
    fn board(data: &Path, p: Profile) -> Option<String> {
        fs::read_to_string(p.board(data).join("fpr_table0.txt")).ok()
    }

    /// A seat folder with a marker but no table (a table deleted by hand, a rebuilt seat that kept the marker): no
    /// profile's board is emptied, and the marker's own profile gets its saved table back.
    #[test]
    fn an_empty_seat_with_a_marker_empties_no_board() {
        let d = tempfile::tempdir().unwrap();
        let (data, seat) = (d.path().join("data"), d.path().join("seat1"));
        for p in Profile::ALL {
            fs::create_dir_all(p.board(&data)).unwrap();
            fs::write(p.board(&data).join("fpr_table0.txt"), p.code()).unwrap();
        }
        let kept = Profile::Free
            .board(&data)
            .join("fpr_table0.before-match.txt");
        fs::write(&kept, "last match").unwrap();
        fs::create_dir_all(&seat).unwrap();
        fs::write(seat.join(MARKER), "free").unwrap();
        board_in(&data, &seat, Profile::Free).unwrap();
        assert_eq!(
            table(&seat).as_deref(),
            Some("free"),
            "free play's table is copied in"
        );
        assert_eq!(
            fs::read_to_string(seat.join("fpr_table0.before-match.txt")).unwrap(),
            "last match"
        );
        fs::remove_file(seat.join("fpr_table0.txt")).unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        assert_eq!(table(&seat).as_deref(), Some("create-a-team"));
        assert_eq!(
            board(&data, Profile::Free).as_deref(),
            Some("free"),
            "free play's board kept its table"
        );
        assert_eq!(fs::read_to_string(&kept).unwrap(), "last match");
    }

    /// UNDO SETUP and SET UP after a past switch: the seat folder comes back with no table and no marker. Neither
    /// profile's board is emptied, and free play's table comes back; a free-only player's empty seat stays as it is.
    #[test]
    fn a_rebuilt_seat_after_a_switch_gets_the_profiles_table_back() {
        let d = tempfile::tempdir().unwrap();
        let (data, seat) = (d.path().join("data"), d.path().join("seat1"));
        fs::create_dir_all(&seat).unwrap();
        board_in(&data, &seat, Profile::Free).unwrap();
        assert!(
            !data.exists() && fs::read_dir(&seat).unwrap().next().is_none(),
            "nothing new on disk"
        );
        fs::create_dir_all(Profile::Free.board(&data)).unwrap();
        fs::write(Profile::Free.board(&data).join("fpr_table0.txt"), "free").unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        assert_eq!(
            board(&data, Profile::Free).as_deref(),
            Some("free"),
            "the empty seat overwrote nothing"
        );
        fs::write(seat.join("fpr_table0.txt"), "cat").unwrap();
        board_out(&data, &seat, Profile::CreateATeam).unwrap();
        // the seat folder is rebuilt
        fs::remove_dir_all(&seat).unwrap();
        fs::create_dir_all(&seat).unwrap();
        board_in(&data, &seat, Profile::Free).unwrap();
        assert_eq!(table(&seat).as_deref(), Some("free"));
        assert_eq!(fs::read_to_string(seat.join(MARKER)).unwrap(), "free");
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        assert_eq!(table(&seat).as_deref(), Some("cat"));
        assert_eq!(board(&data, Profile::Free).as_deref(), Some("free"));
    }

    /// A marker write that fails partway leaves the old marker whole and no temporary file a later start trips on.
    #[test]
    fn a_failed_marker_write_leaves_the_old_marker_whole() {
        let d = tempfile::tempdir().unwrap();
        let (data, seat) = (d.path().join("data"), d.path().join("seat1"));
        fs::create_dir_all(&seat).unwrap();
        fs::write(seat.join("fpr_table0.txt"), "free").unwrap();
        fs::write(seat.join(MARKER), "free").unwrap();
        let tmp = seat.join(MARKER).with_extension("tmp");
        fs::create_dir_all(&tmp).unwrap(); // the temporary file cannot be made
        assert!(board_in(&data, &seat, Profile::CreateATeam).is_err());
        assert_eq!(fs::read_to_string(seat.join(MARKER)).unwrap(), "free");
        fs::remove_dir(&tmp).unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        assert_eq!(
            fs::read_to_string(seat.join(MARKER)).unwrap(),
            "create-a-team"
        );
        assert!(!tmp.exists());
    }

    /// A lost marker (a hand deletion) makes Create-a-Team's table look like free play's: the switch that follows copies
    /// it over free play's board, and free play's own table stays beside it as `.prev`.
    #[test]
    fn a_lost_marker_keeps_free_plays_table_as_prev() {
        let d = tempfile::tempdir().unwrap();
        let (data, seat) = (d.path().join("data"), d.path().join("seat1"));
        fs::create_dir_all(&seat).unwrap();
        fs::write(seat.join("fpr_table0.txt"), "free").unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        fs::write(seat.join("fpr_table0.txt"), "cat").unwrap();
        fs::remove_file(seat.join(MARKER)).unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        let free = Profile::Free.board(&data);
        assert_eq!(
            fs::read_to_string(free.join("fpr_table0.txt")).unwrap(),
            "cat"
        );
        assert_eq!(
            fs::read_to_string(free.join("fpr_table0.txt.prev")).unwrap(),
            "free"
        );
        // an unchanged table makes no `.prev`
        fs::remove_file(free.join("fpr_table0.txt.prev")).unwrap();
        board_in(&data, &seat, Profile::Free).unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        assert!(!free.join("fpr_table0.txt.prev").exists());
    }

    /// #234 review: a second lost marker keeps its own copy beside the first; neither replaces the other.
    #[test]
    fn a_second_lost_marker_keeps_the_first_prev() {
        let d = tempfile::tempdir().unwrap();
        let (data, seat) = (d.path().join("data"), d.path().join("seat1"));
        let free = Profile::Free.board(&data);
        fs::create_dir_all(&seat).unwrap();
        fs::write(seat.join("fpr_table0.txt"), "free").unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        for cat in ["cat 1", "cat 2"] {
            fs::write(seat.join("fpr_table0.txt"), cat).unwrap();
            fs::remove_file(seat.join(MARKER)).unwrap();
            board_in(&data, &seat, Profile::CreateATeam).unwrap();
        }
        let read = |n: &str| fs::read_to_string(free.join(n)).unwrap();
        assert_eq!(read("fpr_table0.txt"), "cat 2");
        assert_eq!(read("fpr_table0.txt.prev"), "free", "the first copy stays");
        assert_eq!(read("fpr_table0.txt.prev2"), "cat 1");
    }

    /// #234 review: a run that ends with the seat table emptied or gone (a half-written file, a hand deletion) copies it
    /// back, and the board's table with cards is kept as `.prev`; a table replaced by another with cards keeps nothing.
    #[test]
    fn an_emptied_seat_table_keeps_the_boards_cards() {
        let d = tempfile::tempdir().unwrap();
        let (data, seat) = (d.path().join("data"), d.path().join("seat1"));
        let cat = Profile::CreateATeam.board(&data);
        fs::create_dir_all(&cat).unwrap();
        fs::write(cat.join("fpr_table0.txt"), "1 200 0 0\r\n").unwrap();
        fs::create_dir_all(&seat).unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        fs::write(seat.join("fpr_table0.txt"), "1 201 0 0\r\n").unwrap();
        board_out(&data, &seat, Profile::CreateATeam).unwrap();
        assert!(
            !cat.join("fpr_table0.txt.prev").exists(),
            "a table with cards keeps nothing"
        );
        fs::write(seat.join("fpr_table0.txt"), "\r\n").unwrap();
        board_out(&data, &seat, Profile::CreateATeam).unwrap();
        assert_eq!(
            fs::read_to_string(cat.join("fpr_table0.txt.prev")).unwrap(),
            "1 201 0 0\r\n"
        );
        fs::remove_file(seat.join("fpr_table0.txt")).unwrap();
        fs::write(cat.join("fpr_table0.txt"), "1 202 0 0\r\n").unwrap();
        board_out(&data, &seat, Profile::CreateATeam).unwrap();
        assert!(!cat.join("fpr_table0.txt").exists());
        assert_eq!(
            fs::read_to_string(cat.join("fpr_table0.txt.prev2")).unwrap(),
            "1 202 0 0\r\n"
        );
    }

    /// Review 5: a run's end whose seat lacks the panel's other files leaves the profile's copies where they are.
    #[test]
    fn a_run_end_keeps_the_profiles_other_board_files() {
        let d = tempfile::tempdir().unwrap();
        let (data, seat) = (d.path().join("data"), d.path().join("seat1"));
        let cat = Profile::CreateATeam.board(&data);
        fs::create_dir_all(&cat).unwrap();
        for n in BOARD {
            fs::write(cat.join(n), n).unwrap();
        }
        fs::create_dir_all(&seat).unwrap();
        board_in(&data, &seat, Profile::CreateATeam).unwrap();
        for n in &BOARD[1..] {
            fs::remove_file(seat.join(n)).unwrap();
        }
        board_out(&data, &seat, Profile::CreateATeam).unwrap();
        for n in BOARD {
            assert_eq!(fs::read_to_string(cat.join(n)).unwrap(), n);
        }
    }

    /// Every file under `dir` by its path there, with its bytes; empty when `dir` is missing.
    fn tree(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            let Ok(list) = fs::read_dir(&d) else { continue };
            for e in list {
                let p = e.unwrap().path();
                if p.is_dir() {
                    todo.push(p);
                } else {
                    let k = p.strip_prefix(dir).unwrap().to_path_buf();
                    out.insert(k, fs::read(&p).unwrap());
                }
            }
        }
        out
    }

    /// Everything in `data` but Create-a-Team's folder and the archive: free play's save and table, settings.json,
    /// the market key and the welcome mark.
    fn outside(data: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut t = tree(data);
        let profiles = Path::new("profiles");
        t.retain(|k, _| {
            !k.starts_with(profiles.join(Profile::CreateATeam.code()))
                && !k.starts_with(profiles.join("archive"))
        });
        t
    }

    /// An installation that played both profiles: free play's save and table, Create-a-Team's save, collection (a
    /// damaged log) and board, the market key and the welcome mark; the seat holds Create-a-Team's table, newer than
    /// its board (a run that crashed before the copy back).
    fn played(d: &Path) -> (PathBuf, PathBuf) {
        let (data, seat) = (d.join("data"), d.join("seat1"));
        let cat = Profile::CreateATeam;
        fs::create_dir_all(Profile::Free.save(&data)).unwrap();
        fs::write(
            Profile::Free.save(&data).join("seat1_aime.txt"),
            "free code",
        )
        .unwrap();
        fs::write(data.join("settings.json"), r#"{"profile":"create-a-team"}"#).unwrap();
        fs::write(data.join("market.key"), "key").unwrap();
        fs::write(data.join("market.welcome"), "1").unwrap();
        fs::create_dir_all(&seat).unwrap();
        fs::write(seat.join("fpr_table0.txt"), "free table").unwrap();
        board_in(&data, &seat, cat).unwrap();
        fs::create_dir_all(cat.save(&data).join("gamesvr")).unwrap();
        fs::write(cat.save(&data).join("seat1_aime.txt"), "cat code").unwrap();
        fs::write(cat.save(&data).join("gamesvr").join("gamesvr.db"), "club").unwrap();
        let coll = cat.collection(&data).unwrap();
        fs::create_dir_all(&coll).unwrap();
        fs::write(
            coll.join("grants.jsonl"),
            "{\"seq\":1}\n{broken\n{\"seq\":3}\n",
        )
        .unwrap();
        fs::write(seat.join("fpr_table0.txt"), "cat 1").unwrap();
        fs::write(seat.join("fpr_panel_state.json"), "{}").unwrap();
        board_out(&data, &seat, cat).unwrap();
        fs::write(seat.join("fpr_table0.txt"), "cat 2").unwrap();
        (data, seat)
    }

    /// The table files in the seat folder, the marker left out.
    fn seat_table(seat: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut t = tree(seat);
        t.remove(Path::new(MARKER));
        t
    }

    /// 1.5 (1), (2): a reset moves Create-a-Team's collection (the damaged log byte-identical), save and table, the
    /// seat's newest included, into the archive; free play's save, table and settings and the market key and welcome
    /// mark stay byte-identical; the next start has a new save and an empty table.
    #[test]
    fn a_reset_archives_create_a_team_and_leaves_free_play_alone() {
        let d = tempfile::tempdir().unwrap();
        let (data, seat) = played(d.path());
        let cat = Profile::CreateATeam;
        let home = data.join("profiles").join(cat.code());
        let old = tree(&home);
        let free = outside(&data);
        let to = reset(&data, Some(&seat), "20261118-120000").unwrap();
        // the seat holds no table of the archived club, even for a game started without Launcher
        assert!(BOARD.iter().all(|n| !seat.join(n).exists()), "seat emptied");
        assert_eq!(
            to,
            data.join("profiles")
                .join("archive")
                .join("create-a-team-20261118-120000")
        );
        assert!(!home.exists());
        let got = tree(&to);
        let at = |k: &str| got.get(Path::new(k)).cloned();
        assert_eq!(
            at("collection/grants.jsonl"),
            old.get(Path::new("collection/grants.jsonl")).cloned()
        );
        assert_eq!(at("save/seat1_aime.txt").as_deref(), Some(&b"cat code"[..]));
        assert_eq!(at("save/gamesvr/gamesvr.db").as_deref(), Some(&b"club"[..]));
        assert_eq!(at("board/fpr_table0.txt").as_deref(), Some(&b"cat 2"[..]));
        assert_eq!(
            at("board/fpr_table0.txt.prev").as_deref(),
            Some(&b"cat 1"[..])
        );
        assert!(
            old.values().all(|v| got.values().any(|g| g == v)),
            "nothing lost"
        );
        assert_eq!(outside(&data), free);
        // the next start: an empty table for Create-a-Team, free play's own back for free play
        board_in(&data, &seat, cat).unwrap();
        assert_eq!(table(&seat), None);
        assert!(!cat.save(&data).exists());
        board_in(&data, &seat, Profile::Free).unwrap();
        assert_eq!(table(&seat).as_deref(), Some("free table"));
        // a second reset in the same second gets its own folder
        fs::create_dir_all(cat.collection(&data).unwrap()).unwrap();
        let again = reset(&data, Some(&seat), "20261118-120000").unwrap();
        assert!(again.ends_with("create-a-team-20261118-120000-2"));
        assert_eq!(tree(&to), got);
        // with free play's table in the seat the seat is not touched; with no Create-a-Team folder nothing moves
        let before = tree(&seat);
        fs::create_dir_all(cat.collection(&data).unwrap()).unwrap();
        reset(&data, Some(&seat), "x").unwrap();
        assert_eq!(tree(&seat), before);
        assert!(reset(&data, Some(&seat), "y").is_err());
        assert_eq!(outside(&data), free);
    }

    /// 1.5 (3): a reset stopped before any of its steps (an error, or a crash there: nothing is undone either way)
    /// leaves the old profile whole: every file still in its folder, the next start shows the seat's table, and free
    /// play's files are byte-identical. A reset after it goes through.
    #[test]
    fn a_reset_stopped_at_any_step_leaves_the_old_profile_whole() {
        let cat = Profile::CreateATeam;
        for fail in 0..5u8 {
            let d = tempfile::tempdir().unwrap();
            let (data, seat) = played(d.path());
            let home = data.join("profiles").join(cat.code());
            let (old, free, shown) = (tree(&home), outside(&data), seat_table(&seat));
            let r = reset_with(&data, Some(&seat), "s", &mut |s| {
                if s == fail {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            });
            assert!(r.is_err(), "step {fail}");
            let now = tree(&home);
            for (k, v) in &old {
                if !k.starts_with("board") {
                    assert_eq!(now.get(k), Some(v), "step {fail}: {k:?}");
                }
            }
            assert!(
                old.values().all(|v| now.values().any(|g| g == v)),
                "step {fail}: nothing lost"
            );
            assert!(
                !data
                    .join("profiles")
                    .join("archive")
                    .join("create-a-team-s")
                    .exists()
            );
            board_in(&data, &seat, cat).unwrap();
            assert_eq!(
                seat_table(&seat),
                shown,
                "step {fail}: the table the next start shows"
            );
            assert_eq!(outside(&data), free, "step {fail}");
            let to = reset(&data, Some(&seat), "s").unwrap();
            assert_eq!(
                fs::read(to.join("collection").join("grants.jsonl")).ok(),
                old.get(Path::new("collection/grants.jsonl")).cloned()
            );
            board_in(&data, &seat, cat).unwrap();
            assert_eq!(
                table(&seat),
                None,
                "step {fail}: the new profile's empty table"
            );
        }
        // review 7 m1: a seat whose table file is gone (quarantined, half deleted) still has its other files parked
        // before the seat is emptied: stopped at any step (5: not stopped), the panel's UNDO copy is in the profile,
        // its archive or the seat
        for fail in 0..6u8 {
            let d = tempfile::tempdir().unwrap();
            let (data, seat) = played(d.path());
            fs::remove_file(seat.join(BOARD[0])).unwrap();
            fs::write(seat.join(BOARD[4]), "undo copy").unwrap();
            let r = reset_with(&data, Some(&seat), "s", &mut |s| {
                if s == fail {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            });
            assert_eq!(r.is_err(), fail < 5, "step {fail}");
            let kept = [tree(&data.join("profiles")), tree(&seat)];
            assert!(
                kept.iter().any(|t| t.values().any(|v| v == b"undo copy")),
                "step {fail}: the UNDO copy is kept"
            );
        }
    }
}
