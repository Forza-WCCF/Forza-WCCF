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
// means free play, so a Create-a-Team table left there goes to free play's board. Upgrade if a launcher path loses it.
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

/// [`BOARD`]'s files from `from` to `to`, a file `from` lacks removed from `to`.
fn mirror(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for n in BOARD {
        let (a, b) = (from.join(n), to.join(n));
        if a.is_file() {
            fs::copy(&a, &b)?;
        } else if b.is_file() {
            fs::remove_file(&b)?;
        }
    }
    Ok(())
}

/// The marker through a temporary file and one rename: a power cut never leaves a torn name that refuses every start.
fn mark(seat: &Path, code: &str) -> io::Result<()> {
    let (marker, tmp) = (seat.join(MARKER), seat.join(format!("{MARKER}.tmp")));
    fs::write(&tmp, code)?;
    fs::rename(&tmp, &marker)
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
            mirror(&p.board(data), seat)?;
            mark(seat, p.code())?;
        }
        return Ok(());
    }
    if let Some(h) = held {
        if !empty {
            mirror(seat, &h.board(data))?;
        }
        mark(seat, "")?;
    }
    mirror(&p.board(data), seat)?;
    mark(seat, p.code())
}

/// A run's end: the seat folder's table copied back to `p`'s board folder when it is `p`'s. Free play's table is never
/// copied while no switch was ever made (no marker): it stays in the seat folder only, as before profiles.
pub fn board_out(data: &Path, seat: &Path, p: Profile) -> io::Result<()> {
    if seat.join(MARKER).is_file() && held(seat)? == Some(p) {
        mirror(seat, &p.board(data))?;
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
        let tmp = seat.join(format!("{MARKER}.tmp"));
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
}
