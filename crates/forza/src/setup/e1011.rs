//! 2010-11 (Rev D) setup, ported from `legacy/2010-11/code/setup.py` and its `helpers/hook.py`: make the player's own
//! copy of the game run on this PC, once; running it again checks everything and repairs what is missing.  It changes:
//!
//! - in `extracted\` the kit's hook `wccfk.dll`, and the three programs' import of WINMM.dll renamed to it, so the hook
//!   loads under its own name (this is the 2010-11 network guard: the hook blocks the IP-helper calls in-process);
//!   undo renames it back, which gives the exact original bytes.  `logowin.exe` stays the original (an older kit's
//!   stand-in goes, the original comes back); `local\` gets the projector's and the server's settings
//! - beside it `seat1\` (the player's cabinet: junctions and hard links to extracted\, its own patched
//!   client_Release.exe, 15 changes checked byte for byte, its own `local\` settings) and
//!   `misc\FlatPanelReader_Emulator\exe\FPR_Emu.exe`, the card-table helper the game starts
//! - in the kit `data\settings.json` "game", `data\logs`, `data\save`, the catalogue in `data\cache`, and an older kit's
//!   folders tidied (`tidy_old_layout`)
//!
//! The caller checks that no game program runs from the game's folders first (`forza::procs::game_processes`).
//! Messages go to `say`, word for word as setup.py printed them.  The settings files keep setup.py's name in their
//! header: undo and an earlier kit's files are recognised by those exact bytes.

use std::fs;
use std::io;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use super::{
    Error, Patch, Result, apply, imports, junction_target, load_settings, logowin_backup, make_dir,
    merge_settings, read, rename_import, replace, save_settings, sha,
};
use crate::{Edition, Line};

const CLIENT: &str = "client_Release.exe";
const PROGRAMS: [&str; 3] = [CLIENT, "control_Release.exe", "match_Release.exe"];
const ORIGINAL_CLIENT: &str = "aef9998d8a77b4dd7a0cff3753a11fc8689a6700e133265d6681ef28d3e9cea4";
const PATCHED_CLIENT: &str = "6243c394e9a57b33c23519326ef8a77b0992873a0033b5892a9155c5b827e20a";
const ORIGINAL_OTHERS: [(&str, &str); 2] = [
    (
        "control_Release.exe",
        "23b9553096bb195fc991a1fc141d719df3c8ae88460959a88cdfc18863a1376f",
    ),
    (
        "match_Release.exe",
        "d6a1d05d9b0d7365f4260a38ec4d25a55d996aabb83f436b59bde81ceddacb38",
    ),
];
const ORIGINAL_LOGOWIN: &str = "0bfb857af8627282bbeed9f3c977d6ec30f9e110a408e9683008f47637471326";
/// The import as the original Rev D programs name it (so renaming back is byte-exact), and the kit's hook in its place.
const SYS_DLL: &[u8] = b"WINMM.dll\0";
const HOOK_DLL: &[u8] = b"wccfk.dll\0";
const MARKER: &str = ".wccf-kit";
const JUNCTIONS: [&str; 9] = [
    "AI",
    "data",
    "lighting",
    "Microsoft.VC90.CRT",
    "Microsoft.VC90.MFC",
    "prog_data",
    "script",
    "shader",
    "update",
];

/// Seat 1's exe: the original exe with these 15 changes (file offsets; the VA in each `what`).
static PATCHES: [Patch; 15] = [
    Patch {
        at: 0x000b71,
        old: &[0x8d, 0x81, 0x40, 0xfc, 0xff, 0xff],
        new: &[0x31, 0xc0, 0x90, 0x90, 0x90, 0x90],
        what: "picture layout: no left offset; VA 0x00401771",
    },
    Patch {
        at: 0x000b8f,
        old: &[
            0xc7, 0x46, 0x44, 0xc0, 0x03, 0x00, 0x00, 0xe9, 0x36, 0xff, 0xff, 0xff,
        ],
        new: &[
            0x89, 0x4e, 0x44, 0x8b, 0x7e, 0x30, 0xe9, 0x37, 0xff, 0xff, 0xff, 0x90,
        ],
        what: "picture layout: full width; VA 0x0040178F",
    },
    Patch {
        at: 0x05996a,
        old: &[0x74, 0x1a],
        new: &[0x90, 0x90],
        what: "a main-board check jump (no effect found; kept: the tested exe); VA 0x0045A56A",
    },
    Patch {
        at: 0x0d85c9,
        old: &[0x01],
        new: &[0x00],
        what: "I/O board in JVS mode 0 (COM4); VA 0x004D91C9",
    },
    Patch {
        at: 0x0dfee0,
        old: &[0x6a, 0xff, 0x68],
        new: &[0xb0, 0x01, 0xc3],
        what: "keychip check passes (error 0949); VA 0x004E0AE0",
    },
    Patch {
        at: 0x0ed400,
        old: &[0x55, 0x8b, 0xec],
        new: &[0x32, 0xc0, 0xc3],
        what: "camera firmware check passes (error 0xC05); VA 0x004EE000",
    },
    Patch {
        at: 0x0ed480,
        old: &[0x53, 0x33, 0xdb],
        new: &[0x32, 0xc0, 0xc3],
        what: "camera board check passes (error 5801); VA 0x004EE080",
    },
    Patch {
        at: 0x0ed502,
        old: &[0x38],
        new: &[0xc3],
        what: "resolution check passes (error 0910); VA 0x004EE102",
    },
    Patch {
        at: 0x17002a,
        old: &[0xc7, 0x46, 0x04, 0xb7, 0x14, 0x00, 0x00],
        new: &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
        what: "main board error 5303 not raised; VA 0x00570C2A",
    },
    Patch {
        at: 0x170061,
        old: &[0xc7, 0x46, 0x04, 0x19, 0x00, 0x00, 0x00],
        new: &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
        what: "main board error 0025 not raised; VA 0x00570C61",
    },
    Patch {
        at: 0x170098,
        old: &[0xc7, 0x46, 0x04, 0x1a, 0x00, 0x00, 0x00],
        new: &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
        what: "main board error 0026 not raised; VA 0x00570C98",
    },
    Patch {
        at: 0x3a6466,
        old: &[0x8b, 0x80, 0xfc, 0x00, 0x00, 0x00],
        new: &[0x33, 0xc0, 0x90, 0x90, 0x90, 0x90],
        what: "board error read as 0 (no effect found; kept); VA 0x007A7066",
    },
    Patch {
        at: 0x587668,
        old: &[0x01, 0x00, 0x00, 0x00],
        new: &[0x02, 0x00, 0x00, 0x00],
        what: "smooth texture scaling; VA 0x00988668",
    },
    Patch {
        at: 0x5face8,
        old: &[0x20, 0x03],
        new: &[0xa0, 0x05],
        what: "screen width 800 -> 1440; VA 0x009FBCE8",
    },
    Patch {
        at: 0x5facec,
        old: &[0x58, 0x02],
        new: &[0x84, 0x03],
        what: "screen height 600 -> 900; VA 0x009FBCEC",
    },
];

/// The projector's (seat 0) or player cabinet's (seat 1) client_user_option.conf.
fn conf_client(seat: u8) -> String {
    let (what, extra) = if seat == 0 {
        ("the PROJECTOR (seat 0)", "")
    } else {
        ("the PLAYER CABINET (seat 1)", "MANAGEMENT_0910=0\r\n")
    };
    format!(
        "# WCCF 2010-11 - {what} - written by Forza-WCCF setup.\r\n\
         # DHCP=0 skips the network self-test; FPR_MODE=2 = the card-table helper; FULL_SCREEN_MODE=0 = a window.\r\n\
         # SATELLITE_NO = the seat (0 projector, 1-8 player cabinets), set here because the game would otherwise\r\n\
         # read it from its command line, which breaks on folder names with spaces.\r\n\
         IS_RINGEDGE={seat}\r\nALLNET_AUTH=0\r\nDHCP=0\r\nCONTROL_IP=127.0.0.1\r\nCONTROL_PORT=20002\r\n\
         MATCH_IP=127.0.0.1\r\nMATCH_PORT=20003\r\nFPR_MODE=2\r\nFULL_SCREEN_MODE=0\r\n{extra}SATELLITE_NO={seat}\r\n"
    )
}

const CONF_CONTROL: &str = "# WCCF 2010-11 - the SERVER (control) - written by Forza-WCCF setup: everything on this PC.\r\n\
    # CLIENT_MAX must stay 9 (the game's own default): fewer made the server crash.\r\n\
    # FIX_REST_ENTRY_TIME = seconds the entry window stays open each cycle (the game's own: 40). Longer, so\r\n\
    # seat 1 can still enter the first match after starting and the projector does not wait a whole\r\n\
    # 12-minute cycle on \"Now Synchronizing\".\r\n\
    DHCP=0\r\nIS_RINGEDGE=0\r\nALLNET_AUTH=0\r\nACCEPT_ALL_IP=1\r\nCLIENT_MAX=9\r\nACCEPT_IP_0=127.0.0.1\r\n\
    CONTROL_PORT=20002\r\nMATCH_PORT=20003\r\nGAME_SVR_IP=127.0.0.1\r\nFIX_REST_ENTRY_TIME=120\r\n";

/// The header words earlier releases wrote; the files they made are still recognised ([`before_rename`]).
const OLD_HEADER: &str = "written by the kit's setup.py";
const NEW_HEADER: &str = "written by Forza-WCCF setup";

/// `text` as an earlier release wrote it (same file, old header words).
fn before_rename(text: &str) -> String {
    text.replace(NEW_HEADER, OLD_HEADER)
}

/// Byte for byte what an earlier kit wrote: replaced without keeping a copy (any other text is a hand edit and is kept
/// as .before-kit).  The first kit's server settings, before FIX_REST_ENTRY_TIME.
const EARLIER_KIT_CONFS: [&str; 1] = [
    "# WCCF 2010-11 - the SERVER (control) - written by the kit's setup.py: everything on this PC.\r\n\
     # CLIENT_MAX must stay 9 (the game's own default): fewer made the server crash.\r\n\
     DHCP=0\r\nIS_RINGEDGE=0\r\nALLNET_AUTH=0\r\nACCEPT_ALL_IP=1\r\nCLIENT_MAX=9\r\nACCEPT_IP_0=127.0.0.1\r\n\
     CONTROL_PORT=20002\r\nMATCH_PORT=20003\r\nGAME_SVR_IP=127.0.0.1\r\n",
];

/// The folders a kit before 6 used (the 2026-10-11 folder move): the old copies of the files this kit keeps in a folder
/// now (`Some`), or those files by name.  scripts\: every file name kits 5 to 5.5.1 shipped there.
const OLD_LAYOUT: [(&str, Option<&str>, &[&str]); 4] = [
    (
        "scripts",
        None,
        &[
            "_debug_launch.py",
            "_icc_reader.py",
            "_jvs_board.py",
            "_keys_seat1.py",
            "_kit_helper.py",
            "_log_inbox.py",
            "_no_quickedit.py",
            "_relay.py",
            "_ringedge_services.py",
            "_seat_broker.py",
            "_seat_lease.py",
            "_xscn_watch.py",
            "boards.py",
            "cards.py",
            "club_card_schema.py",
            "club_view.py",
            "club_wallet.py",
            "codes_inferred.py",
            "decode_club_card.py",
            "edit_club_card.py",
            "english.py",
            "kit_common.py",
            "play.py",
            "send_logs.py",
            "setup.py",
            "sound_defaults.py",
            "ys_lzw.py",
        ],
    ),
    ("english", Some("languages/english"), &[]),
    (
        "bin",
        None,
        &[
            "winmm.dll",
            "winmm_orig.dll",
            "wccfk.dll",
            "FPR_Emu.exe",
            "logowin.exe",
        ],
    ),
    (
        "overlay",
        None,
        &["wccfpanel.dll", "skin.tex", "inject.exe"],
    ),
];

/// `name` is `prefix`, one or more ASCII digits, `suffix` (`name` lower case).
fn numbered(name: &str, prefix: &str, suffix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|r| r.strip_suffix(suffix))
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// The game's own files seat 1 does not share with extracted\ (its run state, the exe it has its own copy of, the hook).
fn no_link(name: &str) -> bool {
    let n = name.to_lowercase();
    numbered(&n, "busram", ".bin")
        || numbered(&n, "eeprom", ".bin")
        || [
            "logowin_messages.txt",
            "client_release.exe",
            "wccfk.dll",
            "winmm.dll",
            "mxsram.bin",
        ]
        .contains(&n.as_str())
        || [".log", ".bmp", ".tmp"].iter().any(|e| n.ends_with(e))
}

/// Seat 1's formation and cabinet settings, kept in data\save\seat1_backup by undo.
fn seat_data(name: &str) -> bool {
    let n = name.to_lowercase();
    numbered(&n, "busram", ".bin")
        || numbered(&n, "eeprom", ".bin")
        || n == "mxsram.bin"
        || n == "fpr_panel_state.json"
        || n.strip_prefix("fpr_table")
            .and_then(|r| r.strip_suffix(".txt"))
            .is_some_and(|d| d.len() == 1 && d.bytes().all(|b| b.is_ascii_digit()))
}

/// What an older kit wrote beside its scripts while it ran: the key driver's and I/O board's files, server debug logs.
fn old_runtime(name: &str) -> bool {
    let word = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
    let n = name.to_lowercase();
    n.strip_suffix(".txt").is_some_and(|s| {
        s == "keys_crash"
            || s.strip_prefix("_jvs_seat1_").is_some_and(word)
            || s.strip_prefix("_dbg_").is_some_and(word)
    })
}

/// `text` without the lines play.py sets at each start (the server's address, the seat, whether to wait for a
/// projector): a file that differs from ours only there is ours, as play.py left it.
fn without_runtime_lines(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in text.split_inclusive(|&b| b == b'\n') {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        let keyed = [
            "CONTROL_IP=",
            "MATCH_IP=",
            "SATELLITE_NO=",
            "WAIT_PROJECTOR=",
        ]
        .iter()
        .any(|k| body.starts_with(k.as_bytes()));
        // the value runs to the line end; a CR only right before its LF
        let cr = body.iter().position(|&b| b == b'\r');
        let whole = match cr {
            None => true,
            Some(i) => i + 1 == body.len() && line.ends_with(b"\n"),
        };
        if !(keyed && whole) {
            out.extend_from_slice(line);
        }
    }
    out
}

/// What setup and undo tell the player, line by line.
struct Say<'a>(crate::Say<'a>);

impl Say<'_> {
    fn ok(&self, line: Line) {
        (self.0)("  ok    ", &line);
    }
    fn note(&self, line: Line) {
        (self.0)("  NOTE  ", &line);
    }
    fn line(&self, line: Line) {
        (self.0)("", &line);
    }
}

fn exists(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

fn names(dir: &Path) -> Result<Vec<String>> {
    let mut v = fs::read_dir(dir)
        .and_then(|r| {
            r.map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect::<io::Result<Vec<_>>>()
        })
        .map_err(|e| Error(format!("{}: {e}", dir.display())))?;
    v.sort();
    Ok(v)
}

/// The program with the hook import renamed back to WINMM.dll if setup renamed it: the original bytes either way.
fn normalized(data: Vec<u8>) -> Vec<u8> {
    if imports(&data, HOOK_DLL) {
        rename_import(&data, HOOK_DLL, SYS_DLL).unwrap_or(data)
    } else {
        data
    }
}

/// SHA-256 of a game program as the original: the same whether or not setup has renamed its import yet.
fn norm_sha(path: &Path) -> Result<String> {
    Ok(sha(&normalized(read(path)?)))
}

fn rt(game: &Path) -> &Path {
    game.parent().unwrap_or(game)
}

fn english_backup(kit: &Path) -> PathBuf {
    kit.join("data").join("english_backup")
}

/// The game folder (the one with the three programs) from a path to it or to the folder above it.
pub fn find_game(path: &str) -> Option<PathBuf> {
    super::find_game(path, &PROGRAMS, "extracted")
}

/// The original client exe: extracted\'s own or, while English is on, the copy in the English backup.
fn original_client(kit: &Path, game: &Path) -> Result<PathBuf> {
    let (p, b) = (game.join(CLIENT), english_backup(kit).join(CLIENT));
    if norm_sha(&p)? != ORIGINAL_CLIENT && b.is_file() && norm_sha(&b)? == ORIGINAL_CLIENT {
        return Ok(b);
    }
    Ok(p)
}

/// The SHA-256 of the English file the game text put at `rel` (its manifest), when English is on.
fn english_copy(kit: &Path, rel: &str) -> Option<String> {
    let m: serde_json::Value =
        serde_json::from_slice(&fs::read(english_backup(kit).join("manifest.json")).ok()?).ok()?;
    Some(m.get(rel)?.get("english")?.as_str()?.to_lowercase())
}

/// The original client exe with the 15 changes, checked before and after.
fn patched_client(kit: &Path, game: &Path) -> Result<Vec<u8>> {
    let data = normalized(read(&original_client(kit, game)?)?);
    let d = apply(&data, &PATCHES).map_err(|e| {
        Error(format!(
            "client_Release.exe is not the original Rev D bytes: {e}"
        ))
    })?;
    if sha(&d) != PATCHED_CLIENT {
        return Err(Error(
            "the patched exe does not come out as the tested one - not written".into(),
        ));
    }
    Ok(d)
}

/// Points the program at the kit's hook by its own name; a program already renamed is left alone.
fn rename_hook_import(path: &Path, what: &Line, say: &Say) -> Result<()> {
    let data = read(path)?;
    if imports(&data, HOOK_DLL) {
        return Ok(());
    }
    let g = rename_import(&data, SYS_DLL, HOOK_DLL).map_err(|e| {
        Error(format!(
            "{}: could not point it at Forza-WCCF's hook ({e})",
            path.display()
        ))
    })?;
    replace(path, &g)?;
    say.ok(Line::key("setup.uses_hook").part("what", what.clone()));
    Ok(())
}

fn copy_if_different(src: &Path, dst: &Path, what: &Line, say: &Say) -> Result<()> {
    let data = read(src)?;
    if dst.is_file() && sha(&read(dst)?) == sha(&data) {
        say.ok(Line::key("setup.in_place").part("what", what.clone()));
        return Ok(());
    }
    replace(dst, &data)?;
    if read(dst)? != data {
        return Err(Error(format!("{}: the copy does not match", dst.display())));
    }
    say.ok(Line::key("setup.put_in_place").part("what", what.clone()));
    Ok(())
}

fn write_text(path: &Path, text: &str, what: &Line, say: &Say) -> Result<()> {
    let have = path.is_file().then(|| read(path)).transpose()?;
    if let Some(have) = have {
        if have == text.as_bytes() {
            say.ok(Line::key("setup.in_place").part("what", what.clone()));
            return Ok(());
        }
        if without_runtime_lines(&have) == without_runtime_lines(text.as_bytes()) {
            say.ok(Line::key("setup.in_place_set").part("what", what.clone()));
            return Ok(());
        }
        if have != before_rename(text).as_bytes()
            && !EARLIER_KIT_CONFS.iter().any(|c| c.as_bytes() == have)
        {
            let name = format!(
                "{}.before-kit",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            fs::copy(path, path.with_file_name(&name))
                .map_err(|e| Error(format!("{}: {e}", path.display())))?;
            say.note(
                Line::key("setup.old_kept")
                    .part("what", what.clone())
                    .arg("name", &name),
            );
        }
    }
    replace(path, text.as_bytes())?;
    say.ok(Line::key("setup.written").part("what", what.clone()));
    Ok(())
}

/// Whether `link` is a folder link to `target` (paths compared as Windows does: absolute, any case).
fn links_to(link: &Path, target: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy();
        let s = s
            .strip_prefix(r"\\?\")
            .or_else(|| s.strip_prefix(r"\??\"))
            .unwrap_or(&s);
        std::path::absolute(s).map(|p| p.to_string_lossy().to_lowercase())
    };
    junction_target(link).is_some_and(|t| norm(&t).ok() == norm(target).ok())
}

fn junction(link: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    let made = junction::create(target, link);
    #[cfg(not(windows))]
    let made: io::Result<()> = Err(io::Error::other("folder links need Windows"));
    if made.is_err() || !links_to(link, target) {
        return Err(Error(format!(
            "could not link {} to {} (is the drive NTFS?)",
            link.display(),
            target.display()
        )));
    }
    Ok(())
}

/// Checks the game folder holds the untouched Rev D client and that Windows has what the game needs.
fn check_game(kit: &Path, game: &Path, say: &Say) -> Result<()> {
    let orig = original_client(kit, game)?;
    let h = norm_sha(&orig)?;
    if orig != game.join(CLIENT) {
        say.ok(Line::key("setup.check_english_backup"));
    }
    if h == PATCHED_CLIENT {
        return Err(Error(
            "the game folder's client_Release.exe is already PATCHED - setup needs the unchanged exe there (only \
             seat1\\ gets the patched copy). Put your own unchanged client_Release.exe back first."
                .into(),
        ));
    }
    if h != ORIGINAL_CLIENT {
        return Err(Error(format!(
            "the game folder's client_Release.exe is not Rev D (SHA-256 {}...) - Forza-WCCF 2010-11 is for Rev D only",
            h[..16].to_uppercase()
        )));
    }
    say.ok(Line::key("setup.client_rev_d"));
    for (n, want) in ORIGINAL_OTHERS {
        if norm_sha(&game.join(n))? != want {
            say.note(Line::key("setup.not_rev_d").arg("file", n));
        }
    }
    let wow = std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("SysWOW64");
    if !wow.join("winmm.dll").is_file() {
        return Err(Error(format!(
            "no {} - this needs 64-bit Windows 10 or 11",
            wow.join("winmm.dll").display()
        )));
    }
    if !wow.join("d3dx9_40.dll").is_file() {
        say.note(Line::key("setup.no_directx"));
    }
    for (p, what) in [
        (game, "setup.game_path_letters"),
        (kit, "setup.kit_path_letters"),
    ] {
        if !p.to_string_lossy().is_ascii() {
            say.note(Line::key(what));
        }
    }
    Ok(())
}

fn setup_extracted(kit: &Path, game: &Path, say: &Say) -> Result<()> {
    let bin = kit.join(Edition::E1011.layout().bin);
    say.line(Line::key("setup.in_folder").arg("folder", game.display()));
    copy_if_different(
        &bin.join("wccfk.dll"),
        &game.join("wccfk.dll"),
        &Line::key("setup.what_hook"),
        say,
    )?;
    for n in PROGRAMS {
        rename_hook_import(&game.join(n), &Line::raw(n), say)?;
    }
    // a kit before this one put a fake winmm.dll (a Windows name) here
    for old in ["winmm.dll", "winmm_orig.dll"] {
        let p = game.join(old);
        if p.is_file() {
            fs::remove_file(&p)?;
            say.ok(Line::key("setup.removed_old").arg("file", old));
        }
    }
    // the hook answers the server's start of logowin.exe itself, so the original stays; a kit before 2026-10-09 put a
    // stand-in there and kept the original under the backup name: the original goes back
    let (lw, backup) = (game.join("logowin.exe"), game.join(logowin_backup()));
    if backup.is_file() && sha(&read(&backup)?) == ORIGINAL_LOGOWIN {
        if lw.is_file() {
            fs::remove_file(&lw)?;
        }
        fs::rename(&backup, &lw)?;
        say.ok(Line::key("setup.logowin_back"));
    } else if lw.is_file() && sha(&read(&lw)?) == ORIGINAL_LOGOWIN {
        say.ok(Line::key("setup.logowin_original"));
    } else {
        say.note(Line::key("setup.logowin_not_original"));
    }
    write_text(
        &game.join("local").join("client_user_option.conf"),
        &conf_client(0),
        &Line::key("setup.what_projector"),
        say,
    )?;
    write_text(
        &game.join("local").join("ctrl_user_option.conf"),
        CONF_CONTROL,
        &Line::key("setup.what_server"),
        say,
    )
}

fn setup_seat(kit: &Path, game: &Path, say: &Say) -> Result<()> {
    let bin = kit.join(Edition::E1011.layout().bin);
    let seat = rt(game).join("seat1");
    say.line(Line::key("setup.in_folder").arg("folder", seat.display()));
    if seat.is_dir() && !seat.join(MARKER).is_file() {
        return Err(Error(format!(
            "there is already a folder {} that Forza-WCCF did not make - move it away first",
            seat.display()
        )));
    }
    make_dir(&seat.join("local"))?;
    fs::write(
        seat.join(MARKER),
        "made by Forza-WCCF setup; \"Launcher.exe undo\" removes this folder\r\n",
    )?;
    let mut made = 0;
    for n in JUNCTIONS {
        let (link, target) = (seat.join(n), game.join(n));
        if links_to(&link, &target) {
            continue;
        }
        if exists(&link) {
            if junction_target(&link).is_none() {
                return Err(Error(format!(
                    "{} is in the way (not a link)",
                    link.display()
                )));
            }
            fs::remove_dir(&link)?; // an old link only
        }
        junction(&link, &target)?;
        made += 1;
    }
    say.ok(Line::key("setup.folder_links")
        .arg("count", JUNCTIONS.len())
        .arg("new", made));
    let (mut linked, mut new) = (0, 0);
    for n in names(game)? {
        let (src, dst) = (game.join(&n), seat.join(&n));
        if !src.is_file() || no_link(&n) {
            continue;
        }
        linked += 1;
        if dst.exists() && same_file::is_same_file(&src, &dst).unwrap_or(false) {
            continue;
        }
        if dst.exists() {
            fs::remove_file(&dst)?;
        }
        fs::hard_link(&src, &dst).map_err(|e| {
            Error(format!(
                "could not link {n} ({e}) - the game must be on an NTFS drive"
            ))
        })?;
        new += 1;
    }
    say.ok(Line::key("setup.file_links")
        .arg("count", linked)
        .arg("new", new));
    let exe = seat.join(CLIENT);
    let have = exe.is_file().then(|| norm_sha(&exe)).transpose()?;
    if have.as_deref() == Some(PATCHED_CLIENT) {
        say.ok(Line::key("setup.seat_client_in_place"));
    } else if have.is_some() && have == english_copy(kit, &format!("seat1{MAIN_SEPARATOR}{CLIENT}"))
    {
        say.ok(Line::key("setup.seat_client_english"));
    } else {
        replace(&exe, &patched_client(kit, game)?)?;
        if sha(&read(&exe)?) != PATCHED_CLIENT {
            return Err(Error("seat 1's exe did not write correctly".into()));
        }
        say.ok(Line::key("setup.seat_client_made"));
    }
    rename_hook_import(&exe, &Line::key("setup.what_seat_client"), say)?;
    copy_if_different(
        &bin.join("wccfk.dll"),
        &seat.join("wccfk.dll"),
        &Line::key("setup.what_hook"),
        say,
    )?;
    for old in ["winmm.dll", "winmm_orig.dll"] {
        if seat.join(old).is_file() {
            fs::remove_file(seat.join(old))?;
        }
    }
    write_text(
        &seat.join("local").join("client_user_option.conf"),
        &conf_client(1),
        &Line::key("setup.what_seat"),
        say,
    )?;
    let table = Path::new("local").join("SBWG_Table.dat");
    copy_if_different(
        &game.join(&table),
        &seat.join(&table),
        &Line::raw("local\\SBWG_Table.dat"),
        say,
    )?;
    let misc = rt(game).join("misc");
    say.line(Line::key("setup.in_folder").arg("folder", misc.display()));
    let helper = misc.join("FlatPanelReader_Emulator").join("exe");
    make_dir(&helper)?;
    copy_if_different(
        &bin.join("FPR_Emu.exe"),
        &helper.join("FPR_Emu.exe"),
        &Line::raw("FPR_Emu.exe"),
        say,
    )
}

/// An older kit's overlay\ catalogue and card pictures moved into data\cache\, its own files and then its empty
/// folders removed.  Housekeeping: a file that cannot go just stays, and is said.
fn tidy_old_layout(kit: &Path, say: &Say) -> Result<()> {
    let (old, cache) = (kit.join("overlay"), kit.join(Edition::E1011.layout().cache));
    for name in ["catalogue.tsv", "cards"] {
        let (src, dst) = (old.join(name), cache.join(name));
        let moved = || -> io::Result<()> {
            if !src.exists() {
                return Ok(());
            }
            if dst.exists() {
                // already made in the new place: the old copy is spare
                if src.is_dir() {
                    fs::remove_dir_all(&src)
                } else {
                    fs::remove_file(&src)
                }
            } else {
                fs::create_dir_all(&cache)?;
                fs::rename(&src, &dst)?;
                say.ok(Line::key("setup.moved_to_cache").arg("name", name));
                Ok(())
            }
        };
        if let Err(e) = moved() {
            say.note(
                Line::key("setup.not_moved")
                    .arg("name", name)
                    .arg("error", e),
            );
        }
    }
    for (folder, now, files) in OLD_LAYOUT {
        let d = kit.join(folder);
        if !d.is_dir() {
            continue;
        }
        let here = names(&d)?;
        let mut files: Vec<String> = match now {
            Some(now) => here
                .iter()
                .filter(|n| kit.join(now).join(n).is_file())
                .cloned()
                .collect(),
            None => files.iter().map(|n| (*n).to_owned()).collect(),
        };
        files.extend(here.into_iter().filter(|n| old_runtime(n)));
        let _ = fs::remove_dir_all(d.join("__pycache__"));
        for n in files {
            match fs::remove_file(d.join(&n)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => say.note(
                    Line::key("setup.old_file_stays")
                        .arg("file", format!("{folder}\\{n}"))
                        .arg("error", e),
                ),
                _ => {}
            }
        }
        if fs::remove_dir(&d).is_ok() {
            say.ok(Line::key("setup.old_folder_removed").arg("folder", folder));
        } else {
            say.note(Line::key("setup.old_folder_kept").arg("folder", folder));
        }
    }
    Ok(())
}

/// The catalogue and card pictures in data\cache, made from the player's game unless they are there (or `force`).
fn setup_cards(kit: &Path, game: &Path, force: bool, say: &Say) -> Result<()> {
    use crate::cards::catalogue;
    say.line(Line::key("setup.in_cache"));
    let cache = kit.join(Edition::E1011.layout().cache);
    make_dir(&cache)?;
    let have = fs::read_dir(cache.join("cards")).map_or(0, |r| {
        r.filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.file_name().to_string_lossy().ends_with(".png"))
        })
        .count()
    });
    if !force && cache.join("catalogue.tsv").is_file() && have >= catalogue::RECORDS {
        say.ok(Line::key("setup.cards_there").arg("count", have));
        return Ok(());
    }
    (say.0)("  ", &Line::key("setup.making_cards"));
    if catalogue::run(Edition::E1011, game, &cache, catalogue::Output::All, say.0) != 0 {
        return Err(Error(
            "the card catalogue could not be made (see above)".into(),
        ));
    }
    say.ok(Line::key("setup.cards_made"));
    Ok(())
}

/// The whole setup, as `setup.py [GAME] [cards]` ran it: `force_cards` makes the catalogue again.
pub fn run(kit: &Path, game: &Path, force_cards: bool, say: crate::Say) -> Result<()> {
    let say = &Say(say);
    check_game(kit, game, say)?;
    setup_extracted(kit, game, say)?;
    setup_seat(kit, game, say)?;
    let data = kit.join("data");
    make_dir(&data.join("logs"))?;
    make_dir(&data.join("save"))?;
    merge_settings(&data.join("settings.json"), "game", &game.to_string_lossy())?;
    say.line(Line::key("setup.in_kit"));
    tidy_old_layout(kit, say)?;
    setup_cards(kit, game, force_cards, say)
}

/// Whether `dir` holds a folder link anywhere below it (a folder link itself is not walked into).
fn holds_link(dir: &Path) -> io::Result<bool> {
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if junction_target(&p).is_some() || (p.symlink_metadata()?.is_dir() && holds_link(&p)?) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The game folder put back as it was before setup, apart from the files the game itself writes when it runs.
pub fn undo(kit: &Path, game: &Path, say: crate::Say) -> Result<()> {
    let say = &Say(say);
    if english_backup(kit).join("manifest.json").is_file() {
        return Err(Error("The game text is not the original. Press UNDO SETUP in Launcher.exe: it puts the original text back first".into()));
    }
    let seat = rt(game).join("seat1");
    if seat.is_dir() {
        if !seat.join(MARKER).is_file() {
            return Err(Error(format!(
                "{} was not made by Forza-WCCF - left alone",
                seat.display()
            )));
        }
        let keep = kit.join("data").join("save").join("seat1_backup");
        for n in names(&seat)? {
            if seat_data(&n) && seat.join(&n).is_file() {
                make_dir(&keep)?;
                fs::copy(seat.join(&n), keep.join(&n))?;
            }
        }
        if keep.is_dir() {
            say.ok(Line::key("setup.seat_backup"));
        }
        for n in names(&seat)? {
            if junction_target(&seat.join(&n)).is_some() {
                fs::remove_dir(seat.join(&n))?; // the link only; extracted\'s folder is untouched
            }
        }
        for n in names(&seat)? {
            let p = seat.join(&n);
            if p.is_dir() {
                if holds_link(&p)? {
                    return Err(Error(format!(
                        "unexpected link inside {} - stopped",
                        p.display()
                    )));
                }
                fs::remove_dir_all(&p)?;
            } else {
                fs::remove_file(&p)?; // a file link: removes the link, never extracted\'s file
            }
        }
        fs::remove_dir(&seat)?;
        say.ok(Line::key("setup.seat_removed"));
    }
    let misc = rt(game).join("misc");
    let exe_dir = misc.join("FlatPanelReader_Emulator").join("exe");
    if exe_dir.join("FPR_Emu.exe").is_file() {
        fs::remove_file(exe_dir.join("FPR_Emu.exe"))?;
        for d in [
            exe_dir.as_path(),
            &misc.join("FlatPanelReader_Emulator"),
            &misc,
        ] {
            if d.is_dir() && fs::read_dir(d)?.next().is_none() {
                fs::remove_dir(d)?;
            }
        }
        say.ok(Line::key("setup.fpr_removed"));
    }
    for n in PROGRAMS {
        let p = game.join(n);
        if p.is_file() {
            let data = read(&p)?;
            if imports(&data, HOOK_DLL) {
                // a pure swap: this restores the exact original bytes
                replace(&p, &rename_import(&data, HOOK_DLL, SYS_DLL)?)?;
                say.ok(Line::key("setup.import_restored").arg("file", format!("extracted\\{n}")));
            }
        }
    }
    for n in ["wccfk.dll", "winmm.dll", "winmm_orig.dll"] {
        let p = game.join(n);
        if p.is_file() {
            if n != "winmm_orig.dll" && !read(&p)?.windows(15).any(|w| w == b"mxhook attached") {
                say.note(Line::key("setup.not_our_hook").arg("file", format!("extracted\\{n}")));
                continue;
            }
            fs::remove_file(&p)?;
            say.ok(Line::key("setup.removed").arg("file", format!("extracted\\{n}")));
        }
    }
    let (lw, backup) = (game.join("logowin.exe"), game.join(logowin_backup()));
    if backup.is_file() && sha(&read(&backup)?) == ORIGINAL_LOGOWIN {
        if lw.is_file() {
            fs::remove_file(&lw)?;
        }
        fs::rename(&backup, &lw)?;
        say.ok(Line::key("setup.logowin_back"));
    }
    for (n, text) in [
        ("client_user_option.conf", conf_client(0)),
        ("ctrl_user_option.conf", CONF_CONTROL.to_owned()),
    ] {
        let p = game.join("local").join(n);
        if p.is_file() {
            let have = read(&p)?;
            if have == text.as_bytes() || have == before_rename(&text).as_bytes() {
                fs::remove_file(&p)?;
                say.ok(Line::key("setup.removed").arg("file", format!("extracted\\local\\{n}")));
            } else {
                say.note(
                    Line::key("setup.changed_left").arg("file", format!("extracted\\local\\{n}")),
                );
            }
        }
    }
    let settings = kit.join("data").join("settings.json");
    let mut s = load_settings(&settings);
    s.remove("game");
    save_settings(&settings, s)?;
    say.line(Line::key("setup.undone_1011"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::pe;

    #[test]
    fn the_hook_import_swaps_both_ways_byte_for_byte() {
        let exe = pe(&["KERNEL32.dll", "WINMM.dll", "USER32.dll"], b"");
        assert!(!imports(&exe, HOOK_DLL) && imports(&exe, SYS_DLL));
        let hooked = rename_import(&exe, SYS_DLL, HOOK_DLL).unwrap();
        assert_eq!(&hooked[0x300 + 0x20..][..10], HOOK_DLL);
        let name = 0x300 + 0x20..0x300 + 0x20 + 9;
        assert!(
            (0..exe.len()).all(|i| exe[i] == hooked[i] || name.contains(&i)),
            "only the name changes"
        );
        assert!(imports(&hooked, HOOK_DLL) && !imports(&hooked, SYS_DLL));
        assert_eq!(normalized(hooked.clone()), exe);
        assert_eq!(normalized(exe.clone()), exe);
        assert_eq!(rename_import(&hooked, HOOK_DLL, SYS_DLL).unwrap(), exe);
        // the case of the import does not matter for finding it; a program without it is refused
        let lower = pe(&["winmm.dll"], b"");
        assert!(imports(&lower, SYS_DLL));
        assert!(rename_import(&pe(&["KERNEL32.dll"], b""), SYS_DLL, HOOK_DLL).is_err());
        assert!(!imports(b"not a program", HOOK_DLL));
    }

    #[test]
    fn settings_written_with_the_old_header_words_are_still_ours() {
        assert!(CONF_CONTROL.contains(NEW_HEADER) && conf_client(1).contains(NEW_HEADER));
        let old = before_rename(CONF_CONTROL);
        assert!(old.contains("the kit's setup.py") && old != CONF_CONTROL);
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("c.conf");
        fs::write(&p, &old).unwrap();
        write_text(&p, CONF_CONTROL, &Line::raw("x"), &Say(&|_, _| {})).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), CONF_CONTROL);
        assert!(!t.path().join("c.conf.before-kit").exists());
    }

    #[test]
    fn an_older_kits_layout_is_tidied_and_a_second_run_changes_nothing() {
        let t = tempfile::tempdir().unwrap();
        let k = t.path();
        let touch = |rel: &str| {
            let p = k.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"x").unwrap();
        };
        for rel in [
            "scripts/play.py", // the old copy of a script
            "scripts/__pycache__/play.cpython-313.pyc",
            "scripts/_jvs_seat1_input.txt", // written beside the scripts while the kit ran
            "scripts/_dbg_control.txt",
            "languages/english/screen_text.tsv",
            "english/screen_text.tsv", // the old copy of languages\\english\\
            "bin/winmm.dll",           // the old fake hook
            "overlay/wccfpanel.dll",
            "overlay/skin.tex",
            "overlay/catalogue.tsv",
            "overlay/cards/1.png",
            "overlay/mine.txt",    // not the kit's: stays
            "programs/client.exe", // the new layout: untouched
        ] {
            touch(rel);
        }
        let say = |_: &str, _: &Line| {};
        tidy_old_layout(k, &Say(&say)).unwrap();
        for gone in ["scripts", "bin", "english"] {
            assert!(!k.join(gone).exists(), "{gone} removed");
        }
        assert!(k.join("data/cache/catalogue.tsv").is_file());
        assert!(k.join("data/cache/cards/1.png").is_file());
        assert_eq!(names(&k.join("overlay")).unwrap(), ["mine.txt"]);
        assert!(
            k.join("languages/english/screen_text.tsv").is_file()
                && k.join("programs/client.exe").is_file()
        );
        tidy_old_layout(k, &Say(&say)).unwrap();
        assert_eq!(names(&k.join("overlay")).unwrap(), ["mine.txt"]);
        assert!(k.join("data/cache/cards/1.png").is_file());
    }

    #[test]
    fn names_as_setup_py_matched_them() {
        assert!(no_link("busram12.bin") && no_link("Client_Release.EXE") && no_link("x.log"));
        assert!(!no_link("busram.bin") && !no_link("cg.dll") && !no_link("logowin.exe"));
        assert!(
            seat_data("fpr_table1.txt")
                && !seat_data("fpr_table12.txt")
                && seat_data("EEPROM0.bin")
        );
        assert!(
            old_runtime("_dbg_server.txt")
                && old_runtime("keys_crash.txt")
                && !old_runtime("_dbg_.txt")
        );
        let t = b"A=1\r\nCONTROL_IP=1.2.3.4\r\nSATELLITE_NO=3\nB=2\r\nMATCH_IP=x";
        assert_eq!(without_runtime_lines(t), b"A=1\r\nB=2\r\n");
        assert_eq!(
            without_runtime_lines(b"MATCH_IP=x\ry\n"),
            b"MATCH_IP=x\ry\n"
        );
    }

    #[test]
    fn the_patches_keep_their_length() {
        assert!(PATCHES.iter().all(|p| p.old.len() == p.new.len()));
        assert!(conf_client(1).contains("MANAGEMENT_0910=0\r\nSATELLITE_NO=1\r\n"));
    }
}
