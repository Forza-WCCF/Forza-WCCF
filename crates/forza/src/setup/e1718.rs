//! 2017-18 setup (`setup.py`): set up your copy of the decrypted game for Forza-WCCF, once; run it again to check and
//! repair.  Works in place on the game folder; the runtime is the folder that holds it:
//!
//! - `original\` the untouched client_Release.exe and control_Release.exe, kept on the first run; every later run
//!   patches from them, so running setup again never stacks a patch (a second run changes nothing)
//! - `game\` the server and projector: guarded programs (they import wccfnet.dll, which stubs the four IP-helper
//!   calls, instead of IPHLPAPI.DLL), the kit's winmm.dll / logowin.exe / ICMP.DLL stand-ins, the settings files
//! - `seat1\` the player's cabinet: junctions to game's folders, hard links to its files, its own patched
//!   client_Release.exe, the card-table helper
//! - `setup-hashes.json` the SHA-256 of what setup made; Launcher.exe starts nothing unless every file still matches
//!
//! It also writes the kit's `data\settings.json` "game".  The card catalogue (overlay\catalogue.tsv) is
//! `setup_catalogue`, a separate step like `setup.py`'s; the card pictures are the Launcher's.

use std::fs;
use std::path::{Path, PathBuf};

use super::{
    Error, Hash, Patch, Report, Result, apply, copy_in, logowin_backup, make_dir, make_junction,
    merge_settings, original, put, read, rename_import, replace, sha,
};
use crate::Edition;
#[cfg(feature = "ui")]
use crate::cards::catalogue::Output;

/// The decrypted 2017-18 game's two programs and the SHA-256 each must have (every patch checks its own bytes as well).
const PINNED: [(&str, &str); 2] = [
    (
        "client_Release.exe",
        "8b55456f04ad4766fa1226738951954ab77f77ae2e4d7d0b57b88544a283e5ca",
    ),
    (
        "control_Release.exe",
        "0539087f72a238fab5a69f0f294205ff186028c7b9d3b9a1f95973fc19f27c5f",
    ),
];
const OLD: &[u8] = b"IPHLPAPI.DLL\0";
const NEW: &[u8] = b"wccfnet.dll\0\0";
/// The card-table helper as the client names it, relative to seat 1's folder.
const FPR_EMU: &str = "FlatPanelReader_Emulator/FPR_Emu.exe";
const CONF_CLIENT: &str = "local/client_user_option.conf";
const JUNCTION_DIRS: [&str; 9] = [
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

fn client_keys(seat: u8) -> String {
    format!(
        "IS_RINGEDGE={seat}\r\nALLNET_AUTH=0\r\nDHCP=0\r\nCONTROL_IP=127.0.0.1\r\nCONTROL_PORT=20002\r\n\
         MATCH_IP=127.0.0.1\r\nMATCH_PORT=20003\r\nFPR_MODE=2\r\nFULL_SCREEN_MODE=0\r\nWAIT_PROJECTOR=1\r\n\
         SATELLITE_NO={seat}\r\n"
    )
}

/// Online through the local game server stand-in; "127.0.0.1" itself would mean "use the address ALL.Net gave", so the
/// stand-in is named localhost.
const CONF_CONTROL: &str = "DHCP=0\r\nIS_RINGEDGE=0\r\nALLNET_AUTH=0\r\nACCEPT_ALL_IP=1\r\nCLIENT_MAX=9\r\n\
    ACCEPT_IP_0=127.0.0.1\r\nCONTROL_PORT=20002\r\nMATCH_PORT=20003\r\nFIX_REST_ENTRY_TIME=120\r\n\
    FORCE_ONLINE=1\r\nGAME_SVR_IP=localhost\r\nGAME_SVR_PORT=20050\r\nWAIT_PROJECTOR=1\r\n";

const ALLNET_AUTHORIZED: Patch = Patch {
    at: 0x877b70,
    old: &[0x56, 0xe8, 0x4a, 0xee, 0xff, 0xff],
    new: &[0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3],
    what: "ALL.Net authorized (VA 0x00C78770 returns 1)",
};

const ALLNET_BILLING_IDLE: Patch = Patch {
    at: 0xf1c7,
    old: &[0x8b, 0xb6, 0xf8, 0x00, 0x00, 0x00],
    new: &[0xbe, 0x14, 0x00, 0x00, 0x00, 0x90],
    what: "ALL.Net billing phase reads IDLE (20) in the online test",
};

/// Seat 1's client_Release.exe (the 2010-11 kit's patches as each proved needed).
static SEAT: [Patch; 15] = [
    Patch {
        at: 0x1db8a0,
        old: &[0x6a, 0xff, 0x68],
        new: &[0xb0, 0x01, 0xc3],
        what: "keychip check passes (error 0949); VA 0x005DC4A0, 2010-11 offset 0x004E0AE0",
    },
    Patch {
        at: 0x1ce5b1,
        old: &[0x01],
        new: &[0x00],
        what: "I/O board in JVS mode 0 (COM4); VA 0x005CF1B1 push 1 -> push 0, 2010-11 offset 0x004D91C9",
    },
    Patch {
        at: 0x1f3820,
        old: &[0x80, 0x3d, 0x6e],
        new: &[0x32, 0xc0, 0xc3],
        what: "resolution check passes (error 0910); VA 0x005F4420, 2010-11 offset 0x004EE102",
    },
    Patch {
        at: 0x1f3720,
        old: &[0x55, 0x8b, 0xec],
        new: &[0x32, 0xc0, 0xc3],
        what: "camera firmware check passes (error 0xC05); VA 0x005F4320, 2010-11 offset 0x004EE000",
    },
    Patch {
        at: 0x1f37a0,
        old: &[0x80, 0x3d, 0x6e],
        new: &[0x32, 0xc0, 0xc3],
        what: "camera board check passes (error 5801); VA 0x005F43A0, 2010-11 offset 0x004EE080",
    },
    Patch {
        at: 0x376f06,
        old: &[0xc7, 0x46, 0x04, 0xb7, 0x14, 0x00, 0x00],
        new: &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
        what: "main board error 5303 not raised; 2010-11 offset 0x00570C2A",
    },
    Patch {
        at: 0x376f3d,
        old: &[0xc7, 0x46, 0x04, 0x19, 0x00, 0x00, 0x00],
        new: &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
        what: "main board error 0025 not raised; 2010-11 offset 0x00570C61",
    },
    Patch {
        at: 0x376f74,
        old: &[0xc7, 0x46, 0x04, 0x1a, 0x00, 0x00, 0x00],
        new: &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
        what: "main board error 0026 not raised; 2010-11 offset 0x00570C98",
    },
    Patch {
        at: 0xdd7e90,
        old: &[0x01, 0x00, 0x00, 0x00],
        new: &[0x02, 0x00, 0x00, 0x00],
        what: "smooth texture scaling; 2010-11 offset 0x00988668",
    },
    Patch {
        at: 0xf93088,
        old: &[0x20, 0x03],
        new: &[0xa0, 0x05],
        what: "screen width 800 -> 1440; 2010-11 offset 0x009FBCE8",
    },
    Patch {
        at: 0xf9308c,
        old: &[0x58, 0x02],
        new: &[0x84, 0x03],
        what: "screen height 600 -> 900; 2010-11 offset 0x009FBCEC",
    },
    Patch {
        at: 0x150e,
        old: &[
            0x8b, 0x51, 0x3c, 0x8b, 0x71, 0x38, 0x8b, 0x79, 0x34, 0x8b, 0x49, 0x30,
        ],
        new: &[
            0x8b, 0x50, 0x30, 0x8b, 0x70, 0x2c, 0x33, 0xff, 0x33, 0xc9, 0x90, 0x90,
        ],
        what: "picture fills the back buffer",
    },
    ALLNET_AUTHORIZED,
    ALLNET_BILLING_IDLE,
    Patch {
        at: 0xe2c250,
        old: &[
            0x61, 0x69, 0x6d, 0x65, 0x2e, 0x6e, 0x61, 0x6f, 0x6d, 0x69, 0x6e, 0x65, 0x74, 0x2e,
            0x6a, 0x70, 0x00,
        ],
        new: &[
            0x6c, 0x6f, 0x63, 0x61, 0x6c, 0x68, 0x6f, 0x73, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00,
        ],
        what: "AiMeDB host aime.naominet.jp -> localhost",
    },
];

/// The projector (game\\client_Release.exe, its own copy: seat 1 has another): the two ALL.Net answers, so its corner
/// status icon agrees with seat 1's, and its title movie counted as over.
static PROJECTOR: [Patch; 3] = [
    ALLNET_AUTHORIZED,
    ALLNET_BILLING_IDLE,
    // Stopgap: the title movie stalls in Sofdec PREP, so the projector never sent GAME_READY. sub_88A230: call
    // sub_A0AD20 / test al,al / jz (VA 0x0088A28D); the jz goes, so it loads the match, plays movie 2 and sends
    // GAME_READY. The stalled player is stopped by the next round's sub_A0AAD0.
    Patch {
        at: 0x48968d,
        old: &[0xe8, 0x8e, 0x0a, 0x18, 0x00, 0x84, 0xc0, 0x74, 0x1a],
        new: &[0xe8, 0x8e, 0x0a, 0x18, 0x00, 0x84, 0xc0, 0x90, 0x90],
        what: "projector title movie counted as over (VA 0x0088A294 jz -> nop nop)",
    },
];

/// The server (control_Release.exe): its ALL.Net library's "network up" check answers yes, so it connects to the local
/// game server stand-in (FORCE_ONLINE=1, GAME_SVR_IP=localhost); its GAME_READY fallback comes after 15 s, not 100 s.
static CONTROL: [Patch; 2] = [
    Patch {
        at: 0xb67c0,
        old: &[0x56, 0xe8, 0xaa, 0xf2, 0xff, 0xff],
        new: &[0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3],
        what: "network up (FUN_004B73C0 returns 1)",
    },
    // Stopgap: in GAME the control waits for the projector's GAME_READY, else sends its own once the counter at
    // +0x90 passes 100 (seconds). VA 0x00432045: cmp dword [eax+90h], 64h -> 0Fh.
    Patch {
        at: 0x31445,
        old: &[0x83, 0xb8, 0x90, 0x00, 0x00, 0x00, 0x64],
        new: &[0x83, 0xb8, 0x90, 0x00, 0x00, 0x00, 0x0f],
        what: "GAME_READY fallback after 15 s, not 100 s (VA 0x0043204B)",
    },
];

/// What setup checks and changes; [`Spec::game`] is the real game's, tests give their own.
pub struct Spec {
    /// (program, SHA-256) of client_Release.exe and control_Release.exe
    pub pinned: [(&'static str, &'static str); 2],
    pub seat: &'static [Patch],
    pub projector: &'static [Patch],
    pub control: &'static [Patch],
    /// Windows' 32-bit winmm.dll, kept beside the game as winmm_orig.dll
    pub winmm: PathBuf,
}

impl Spec {
    pub fn game() -> Self {
        let root = std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        Self {
            pinned: PINNED,
            seat: &SEAT,
            projector: &PROJECTOR,
            control: &CONTROL,
            winmm: root.join("SysWOW64").join("winmm.dll"),
        }
    }
}

/// The game folder (the one with client_Release.exe and control_Release.exe) from a path to it or to the runtime above it.
pub fn find_game(path: &str) -> Option<PathBuf> {
    super::find_game(path, &[PINNED[0].0, PINNED[1].0], "game")
}

const SEAT_MARK: &str = ".forza-seat";

/// game\ (server + projector, guarded) and seat1\; returns what setup-hashes.json holds.
pub fn setup_game(kit: &Path, game: &Path, spec: &Spec) -> Result<Report> {
    let lay = Edition::E1718.layout();
    let rt = game.parent().unwrap_or(game);
    let seat = rt.join("seat1");
    let bin = kit.join(lay.bin);
    let mut report = Report::default();
    for (i, (exe, pinned)) in spec.pinned.iter().enumerate() {
        let orig = original(
            exe,
            &game.join(exe),
            &rt.join("original").join(exe),
            pinned,
            "2017-18",
        )?;
        let g = rename_import(&orig, OLD, NEW)?;
        // the projector's status icon needs the same ALL.Net answers as seat 1
        let g = apply(&g, if i == 1 { spec.control } else { spec.projector })?;
        if fs::read(game.join(exe)).ok().as_deref() != Some(&g) {
            // replace, not write: a hard link at the path is swapped out, never written through
            replace(&game.join(exe), &g)?;
        }
        report
            .0
            .push(((*exe).into(), Hash::Pair(sha(&orig), sha(&g))));
    }
    // older kits put netguard.dll here (and setup copied it into seat1\); nothing loads it now
    for old in [game.join("netguard.dll"), seat.join("netguard.dll")] {
        if old.is_file() {
            fs::remove_file(old)?;
        }
    }
    for (dll, built) in [
        ("wccfnet.dll", bin.join("wccfnet.dll")),
        ("ICMP.DLL", bin.join("ICMP.DLL")),
    ] {
        copy_in(&built, &game.join(dll), "the network guard")?;
        report
            .0
            .push((dll.into(), Hash::One(sha(&read(&game.join(dll))?))));
    }
    copy_in(
        &spec.winmm,
        &game.join("winmm_orig.dll"),
        "Windows' 32-bit winmm.dll",
    )?;
    copy_in(
        &bin.join("winmm.dll"),
        &game.join("winmm.dll"),
        "the cabinet stand-in",
    )?;
    if !game.join(logowin_backup()).exists() {
        fs::rename(game.join("logowin.exe"), game.join(logowin_backup()))?;
    }
    copy_in(
        &bin.join("logowin.exe"),
        &game.join("logowin.exe"),
        "the logo stand-in",
    )?;
    make_dir(&game.join("local"))?;
    put(&game.join(CONF_CLIENT), client_keys(0).as_bytes())?;
    put(
        &game.join("local/ctrl_user_option.conf"),
        CONF_CONTROL.as_bytes(),
    )?;

    // seat 1: junctions to game\'s folders, hard links to its files, its own exe and settings
    // a seat1\ that setup did not make holds someone else's files and saves: refuse it
    if seat.exists() && !seat.join(SEAT_MARK).is_file() && !rt.join("setup-hashes.json").is_file() {
        return Err(Error(format!(
            "{} exists and was not made by setup; move it away first",
            seat.display()
        )));
    }
    make_dir(&seat.join("local"))?;
    fs::write(seat.join(SEAT_MARK), b"made by Forza-WCCF setup\n")?;
    for n in JUNCTION_DIRS {
        make_junction(&seat.join(n), &game.join(n))?;
    }
    for entry in fs::read_dir(game)? {
        let name = entry?.file_name();
        let low = name.to_string_lossy().to_lowercase();
        let (src, dst) = (game.join(&name), seat.join(&name));
        if !src.is_file()
            || low == "client_release.exe"
            || [".log", ".bin", ".bmp", ".tmp", ".txt"]
                .iter()
                .any(|e| low.ends_with(e))
            || dst.exists()
        {
            continue;
        }
        fs::hard_link(&src, &dst)?;
    }
    let client = spec.pinned[0].0;
    let want = apply(
        &rename_import(&read(&rt.join("original").join(client))?, OLD, NEW)?,
        spec.seat,
    )?;
    let seat_exe = seat.join(client);
    if fs::read(&seat_exe).ok().as_deref() != Some(&want) {
        replace(&seat_exe, &want)?;
    }
    report
        .0
        .push((format!("seat1/{client}"), Hash::One(sha(&want))));
    put(&seat.join(CONF_CLIENT), client_keys(1).as_bytes())?;
    let table = Path::new("local/SDDY_Table.dat");
    fs::copy(game.join(table), seat.join(table))?;
    // the 2017-18 client starts the card-table helper from its own folder; with none there the game reads no cards
    let named = format!("./{FPR_EMU}");
    if !want.windows(named.len()).any(|w| w == named.as_bytes()) {
        return Err(Error(format!("seat 1's client no longer names {FPR_EMU}")));
    }
    let helper = seat.join(FPR_EMU);
    make_dir(helper.parent().unwrap_or(&helper))?;
    copy_in(&bin.join("FPR_Emu.exe"), &helper, "the card-table helper")?;
    fs::write(rt.join("setup-hashes.json"), report.json())?;
    Ok(report)
}

/// The whole setup as the command line runs it: the game, the log and save folders, the kit's settings.
pub fn run(kit: &Path, game: &Path, spec: &Spec) -> Result<Report> {
    let report = setup_game(kit, game, spec)?;
    let data = kit.join("data");
    make_dir(&data.join("logs"))?;
    make_dir(&data.join("save"))?;
    merge_settings(&data.join("settings.json"), "game", &game.to_string_lossy())?;
    Ok(report)
}

/// `overlay\catalogue.tsv`, for the board, made from the game unless it is there and `force` is off. `say` hears the
/// progress lines. The card pictures are the Launcher's (`forza::cards::pictures`).
#[cfg(feature = "ui")]
pub fn setup_catalogue(kit: &Path, game: &Path, force: bool, say: &dyn Fn(&str)) -> Result<()> {
    let overlay = kit.join(Edition::E1718.layout().overlay);
    if overlay.join("catalogue.tsv").is_file() && !force {
        say("  ok    card catalogue made");
        return Ok(());
    }
    match crate::cards::catalogue::run(Edition::E1718, game, &overlay, Output::CatalogueOnly, say) {
        0 => Ok(()),
        _ => Err(Error(
            "the card catalogue could not be made from this game".into(),
        )),
    }
}

/// The game folder as it was: the original programs and logo back, the kit's files, seat1\ and the record removed.
pub fn undo(kit: &Path, game: &Path, spec: &Spec) -> Result<()> {
    let rt = game.parent().unwrap_or(game);
    // every backup is checked before anything changes, then swapped in whole
    let mut backups = Vec::new();
    for (exe, pinned) in spec.pinned {
        let from = rt.join("original").join(exe);
        let data = read(&from)?;
        if sha(&data) != pinned {
            return Err(Error(format!(
                "{} is not the original {exe} (SHA-256 {}, expected {pinned}); undo changed nothing",
                from.display(),
                sha(&data)
            )));
        }
        backups.push((exe, data));
    }
    for (exe, data) in backups {
        replace(&game.join(exe), &data)?;
    }
    let backup = game.join(logowin_backup());
    if backup.is_file() {
        fs::rename(&backup, game.join("logowin.exe"))?;
    }
    for f in [
        "winmm.dll",
        "winmm_orig.dll",
        "wccfnet.dll",
        "netguard.dll",
        "ICMP.DLL",
    ] {
        if game.join(f).is_file() {
            fs::remove_file(game.join(f))?;
        }
    }
    let seat = rt.join("seat1");
    // only a seat1\ that setup made (it has the mark, or setup's record is still here)
    if seat.join(SEAT_MARK).is_file() || rt.join("setup-hashes.json").is_file() {
        for n in JUNCTION_DIRS {
            // a junction goes with remove_dir; remove_dir_all would walk into game\
            if seat.join(n).symlink_metadata().is_ok() {
                fs::remove_dir(seat.join(n))?;
            }
        }
        let _ = fs::remove_dir_all(&seat);
    }
    if rt.join("setup-hashes.json").is_file() {
        fs::remove_file(rt.join("setup-hashes.json"))?;
    }
    let settings = kit.join("data/settings.json");
    let mut s = super::load_settings(&settings);
    s.remove("game");
    super::save_settings(&settings, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::pe;

    #[test]
    fn the_import_is_renamed_once() {
        let exe = pe(&["KERNEL32.dll", "iphlpapi.dll"], b"");
        let g = rename_import(&exe, OLD, NEW).unwrap();
        let diff = exe.iter().zip(&g).filter(|(a, b)| a != b).count();
        assert_eq!(&g[0x300 + 0x20..][..13], NEW);
        assert!((10..=13).contains(&diff), "{diff}");
        assert!(
            rename_import(&pe(&["KERNEL32.dll"], b""), OLD, NEW)
                .unwrap_err()
                .0
                .contains("0 IPHLPAPI.DLL")
        );
        let two = pe(&["IPHLPAPI.DLL", "iphlpapi.dll"], b"");
        assert!(
            rename_import(&two, OLD, NEW)
                .unwrap_err()
                .0
                .contains("2 IPHLPAPI.DLL")
        );
        assert!(
            rename_import(&exe[..100], OLD, NEW).is_err(),
            "cut short is an error, not a panic"
        );
    }

    #[test]
    fn a_patch_needs_its_original_bytes() {
        let p = [Patch {
            at: 2,
            old: &[3, 4],
            new: &[9, 9],
            what: "x",
        }];
        assert_eq!(apply(&[1, 2, 3, 4, 5], &p).unwrap(), [1, 2, 9, 9, 5]);
        assert!(
            apply(&[1, 2, 3, 5, 5], &p)
                .unwrap_err()
                .0
                .contains("0x000002")
        );
        assert!(apply(&[1, 2, 3], &p).is_err());
        assert!(
            SEAT.iter()
                .chain(&PROJECTOR)
                .chain(&CONTROL)
                .all(|p| p.old.len() == p.new.len())
        );
    }

    #[test]
    fn the_two_gameready_stopgaps() {
        // a buffer with the original bytes at both places: each changes one or two bytes, the rest stays
        let stop = [&PROJECTOR[2], &CONTROL[1]];
        for p in stop {
            let mut buf = vec![0xccu8; p.at + 16];
            buf[p.at..p.at + p.old.len()].copy_from_slice(p.old);
            let out = apply(&buf, std::slice::from_ref(p)).unwrap();
            let changed: Vec<usize> = (0..buf.len()).filter(|&i| buf[i] != out[i]).collect();
            assert!(!changed.is_empty() && changed.len() <= 2, "{}", p.what);
            buf[p.at + p.old.len() - 1] ^= 0x01; // one wrong original byte: refused
            assert!(apply(&buf, std::slice::from_ref(p)).is_err(), "{}", p.what);
        }
        assert_eq!(PROJECTOR[2].new[7..], [0x90, 0x90]); // jz rel8 -> two nops
        assert_eq!(CONTROL[1].new[6], 15); // seconds
        // only the projector's copy loses its title movie; seat 1 keeps the original jz
        assert!(SEAT.iter().all(|p| p.at != PROJECTOR[2].at));
    }

    static T_SEAT: [Patch; 1] = [Patch {
        at: 0x900,
        old: &[0, 0],
        new: &[7, 7],
        what: "seat",
    }];
    static T_PROJ: [Patch; 1] = [Patch {
        at: 0x910,
        old: &[0],
        new: &[8],
        what: "projector",
    }];
    static T_CTRL: [Patch; 1] = [Patch {
        at: 0x920,
        old: &[0],
        new: &[9],
        what: "control",
    }];

    fn tree(root: &Path) -> Vec<(String, String)> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
            for e in fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if p.is_dir() {
                    out.push((rel.clone(), "dir".into()));
                    if !p.symlink_metadata().unwrap().file_type().is_symlink()
                        && !rel.starts_with("seat1/")
                    {
                        walk(&p, root, out);
                    }
                } else {
                    out.push((rel, sha(&fs::read(&p).unwrap())));
                }
            }
        }
        let mut out = vec![];
        walk(root, root, &mut out);
        out.sort();
        out
    }

    #[test]
    fn setup_makes_the_runtime_twice_and_undo_restores_it() {
        let top = tempfile::tempdir().unwrap();
        let (kit, rt) = (top.path().join("kit"), top.path().join("rt"));
        let game = rt.join("game");
        for d in ["bin", "overlay", "data"] {
            fs::create_dir_all(kit.join(d)).unwrap();
        }
        for (d, n) in [
            ("bin", "wccfnet.dll"),
            ("bin", "ICMP.DLL"),
            ("bin", "winmm.dll"),
            ("bin", "logowin.exe"),
            ("bin", "FPR_Emu.exe"),
        ] {
            fs::write(kit.join(d).join(n), format!("kit {n}")).unwrap();
        }
        fs::write(kit.join("data/settings.json"), "{\"keep\": 1}").unwrap();
        fs::create_dir_all(game.join("local")).unwrap();
        for d in JUNCTION_DIRS {
            fs::create_dir_all(game.join(d)).unwrap();
        }
        let client = pe(&["IPHLPAPI.DLL"], format!("./{FPR_EMU}").as_bytes());
        let control = pe(&["iphlpapi.dll"], b"control");
        fs::write(game.join("client_Release.exe"), &client).unwrap();
        fs::write(game.join("control_Release.exe"), &control).unwrap();
        fs::write(game.join("logowin.exe"), "their logo").unwrap();
        fs::write(game.join("other.dll"), "other").unwrap();
        fs::write(game.join("notes.txt"), "text").unwrap();
        fs::write(game.join("local/SDDY_Table.dat"), "table").unwrap();
        fs::write(top.path().join("winmm.dll"), "windows winmm").unwrap();
        // the pinned hashes are static in the real spec; a test leaks its own
        let leak = |b: &[u8]| -> &'static str { Box::leak(sha(b).into_boxed_str()) };
        let spec = Spec {
            pinned: [
                ("client_Release.exe", leak(&client)),
                ("control_Release.exe", leak(&control)),
            ],
            seat: &T_SEAT,
            projector: &T_PROJ,
            control: &T_CTRL,
            winmm: top.path().join("winmm.dll"),
        };
        let before = tree(&rt);
        // an older kit's netguard.dll: setup deletes it (undo deletes it too, so it stays out of `before`)
        fs::write(game.join("netguard.dll"), "old kit netguard").unwrap();

        let report = run(&kit, &game, &spec).unwrap();
        assert_eq!(report.0.len(), 5);
        let record = fs::read_to_string(rt.join("setup-hashes.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&record).unwrap();
        assert_eq!(json["client_Release.exe"]["original"], sha(&client));
        assert_eq!(json["wccfnet.dll"], sha(b"kit wccfnet.dll"));
        assert_eq!(json["ICMP.DLL"], sha(b"kit ICMP.DLL"));
        assert_eq!(
            fs::read(rt.join("seat1/ICMP.DLL")).unwrap(),
            b"kit ICMP.DLL"
        );
        assert!(
            record.ends_with("}\r\n") && record.starts_with("{\r\n  \"client_Release.exe\": {\r\n")
        );
        let seat = rt.join("seat1");
        let seat_exe = fs::read(seat.join("client_Release.exe")).unwrap();
        assert_eq!(json["seat1/client_Release.exe"], sha(&seat_exe));
        assert_eq!(&seat_exe[0x900..0x902], [7, 7]);
        assert_eq!(fs::read(game.join("client_Release.exe")).unwrap()[0x910], 8);
        assert_eq!(
            fs::read(game.join("control_Release.exe")).unwrap()[0x920],
            9
        );
        assert_eq!(
            fs::read(rt.join("original/client_Release.exe")).unwrap(),
            client
        );
        assert_eq!(
            fs::read(game.join("winmm_orig.dll")).unwrap(),
            b"windows winmm"
        );
        assert_eq!(
            fs::read(game.join(logowin_backup())).unwrap(),
            b"their logo"
        );
        assert_eq!(
            fs::read(game.join("logowin.exe")).unwrap(),
            b"kit logowin.exe"
        );
        assert_eq!(fs::read(seat.join("other.dll")).unwrap(), b"other");
        assert!(!game.join("netguard.dll").exists() && !seat.join("netguard.dll").exists());
        assert!(
            !seat.join("notes.txt").exists(),
            "text files are not linked"
        );
        assert_eq!(fs::read(seat.join(FPR_EMU)).unwrap(), b"kit FPR_Emu.exe");
        assert_eq!(
            fs::read(seat.join("local/SDDY_Table.dat")).unwrap(),
            b"table"
        );
        assert!(
            fs::read_to_string(seat.join(CONF_CLIENT))
                .unwrap()
                .contains("SATELLITE_NO=1")
        );
        #[cfg(windows)]
        assert_eq!(
            junction::get_target(seat.join("AI")).unwrap(),
            game.join("AI")
        );
        let s: serde_json::Value =
            serde_json::from_slice(&fs::read(kit.join("data/settings.json")).unwrap()).unwrap();
        assert_eq!(
            (s["keep"].as_i64(), s["game"].as_str()),
            (Some(1), game.to_str())
        );

        let again = tree(&rt);
        run(&kit, &game, &spec).unwrap();
        assert_eq!(tree(&rt), again, "a second run changes nothing");

        undo(&kit, &game, &spec).unwrap();
        assert_eq!(fs::read(game.join("logowin.exe")).unwrap(), b"their logo");
        assert_eq!(fs::read(game.join("client_Release.exe")).unwrap(), client);
        assert!(
            !seat.exists()
                && !rt.join("setup-hashes.json").exists()
                && !game.join("wccfnet.dll").exists()
                && !game.join("ICMP.DLL").exists()
        );
        assert!(
            game.join("AI").is_dir(),
            "undo does not walk into game\\ through the links"
        );
        let s: serde_json::Value =
            serde_json::from_slice(&fs::read(kit.join("data/settings.json")).unwrap()).unwrap();
        assert!(s.get("game").is_none() && s["keep"] == 1);
        let after: Vec<_> = tree(&rt)
            .into_iter()
            // undo keeps original\ and the two settings files it wrote in game\local, as setup.py's undo does
            .filter(|(p, _)| !p.starts_with("original") && !p.ends_with("_user_option.conf"))
            .collect();
        assert_eq!(after, before);

        // a damaged backup stops the undo before it changes anything
        run(&kit, &game, &spec).unwrap();
        let patched = fs::read(game.join("client_Release.exe")).unwrap();
        let backup = rt.join("original/client_Release.exe");
        fs::write(&backup, b"damaged").unwrap();
        assert!(undo(&kit, &game, &spec).is_err());
        assert_eq!(fs::read(game.join("client_Release.exe")).unwrap(), patched);
        fs::write(&backup, &client).unwrap();
        undo(&kit, &game, &spec).unwrap();
        // a seat1 that setup did not make is neither used nor deleted
        fs::create_dir(&seat).unwrap();
        fs::write(seat.join("save.dat"), b"theirs").unwrap();
        assert!(run(&kit, &game, &spec).is_err());
        undo(&kit, &game, &spec).unwrap();
        assert_eq!(fs::read(seat.join("save.dat")).unwrap(), b"theirs");
    }

    #[test]
    fn a_program_that_is_not_the_pinned_original_is_refused() {
        let top = tempfile::tempdir().unwrap();
        let exe = top.path().join("client_Release.exe");
        fs::write(&exe, "a patched one").unwrap();
        let err = original(
            "client_Release.exe",
            &exe,
            &top.path().join("original/client_Release.exe"),
            &sha(b"x"),
            "2017-18",
        )
        .unwrap_err();
        assert!(
            err.0.contains("is not the original WCCF 2017-18 program"),
            "{err}"
        );
        assert!(
            !top.path().join("original").exists(),
            "nothing is kept for a refused program"
        );
    }
}
