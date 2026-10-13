//! The shared memory the game reads at start (debugger.py's make_all, as _shm_provider.py): the 3 `<id>_*.moai` from the
//! game's files with a size prefix, `<id>_status` byte[0]=1 and `<id>_moSuppl`; 2017-18 (`_debug_launch.py`) reads
//! its player and model data instead and waits for its version mapping.  An existing mapping too small for its payload
//! is reported, never overrun.

use std::io::{self, Write};
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::Path;
use std::{fs, mem, ptr};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_WRITE, MEMORY_BASIC_INFORMATION, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
    VirtualQuery,
};

use crate::job::wide;
use crate::kit::Edition;

/// The game's name for its mappings.
pub const GAME_ID: &str = "SBWG";

/// The three motion files, under the names the game opens them by, and where they are in the game folder.
const MOAI: [(&str, &str); 3] = [
    ("game_motion_data.moai", "AI/motion_data/game_motion_data.moai"),
    (
        "management_motion_data.moai",
        "data/motion/launcherMotion/management_motion_data.moai",
    ),
    (
        "scene_motion_data.moai",
        "data/motion/launcherMotion/scene_motion_data.moai",
    ),
];

/// SBWG_moSuppl, the motion-supplement tables (2026-10-04): the original launcher (FUN_00401490) reads these four
/// AI/motion_data files in THIS order (pointer table 0x0043c13c) and lays them out as four dword offsets from the start
/// of the block, then each file padded to a multiple of 4.  No size prefix.  A stand-in of four empty tables booted the
/// client, but every match engine crashed at the GAME phase (an empty table -> a null motion object).
const MOSUPPL: [&str; 4] = [
    "game_motion_table_moai.bin",
    "game_actionsetup_table_moai.bin",
    "game_movepack_table_moai.bin",
    "game_request_table_moai.bin",
];

/// SBWG_moSuppl exactly as the original launcher builds it, from the game folder's AI/motion_data.
pub fn mosuppl_bytes(game: &Path) -> io::Result<Vec<u8>> {
    let (mut header, mut body, mut off) = (Vec::new(), Vec::new(), 16u32);
    for name in MOSUPPL {
        let mut data = fs::read(game.join("AI/motion_data").join(name))?;
        header.extend_from_slice(&off.to_le_bytes());
        data.resize(data.len().next_multiple_of(4), 0);
        off += data.len() as u32;
        body.extend_from_slice(&data);
    }
    header.extend_from_slice(&body);
    Ok(header)
}

/// Makes (or opens) one named mapping of `size` bytes and writes `payload` at its start.  None if it cannot be made.
pub fn make(out: &mut dyn Write, full: &str, payload: &[u8], size: usize) -> Option<OwnedHandle> {
    let name = wide(full);
    // SAFETY: a pagefile-backed mapping with a live NUL-ended name; the handle is owned at once.
    let (h, existed) = unsafe {
        let h = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            (size as u64 >> 32) as u32,
            size as u32,
            name.as_ptr(),
        );
        let existed = GetLastError() == ERROR_ALREADY_EXISTS;
        if h.is_null() {
            let _ = writeln!(out, "  FAIL create {full} err={}", GetLastError());
            return None;
        }
        (OwnedHandle::from_raw_handle(h), existed)
    };
    use std::os::windows::io::AsRawHandle;
    // SAFETY: the view is checked, written only within the region VirtualQuery reports, and unmapped once.
    unsafe {
        let view = MapViewOfFile(h.as_raw_handle(), FILE_MAP_WRITE, 0, 0, 0);
        if !view.Value.is_null() && !payload.is_empty() {
            let mut mbi: MEMORY_BASIC_INFORMATION = mem::zeroed();
            VirtualQuery(view.Value, &mut mbi, size_of::<MEMORY_BASIC_INFORMATION>());
            if payload.len() > mbi.RegionSize {
                // an older, smaller copy is still held by another process
                let _ = writeln!(
                    out,
                    "  !! {full} already exists and is only {} bytes (need {}): NOT written - stop whatever holds it",
                    mbi.RegionSize,
                    payload.len()
                );
                UnmapViewOfFile(view);
                return Some(h);
            }
            ptr::copy_nonoverlapping(payload.as_ptr(), view.Value.cast::<u8>(), payload.len());
        }
        if !view.Value.is_null() {
            UnmapViewOfFile(view);
        }
    }
    let _ = writeln!(
        out,
        "  {} {full:<34} size={size}",
        if existed { "opened" } else { "made  " }
    );
    Some(h)
}

/// 2017-18's player and model data (its launcher, FUN_00403890, before it sets the version byte): `<id>_<file>` = a u32
/// size + prog_data/player_data/<file>; a missing file gets no mapping.  Without them the client's players have no
/// models.
const SDDY_DATA: [&str; 4] = [
    "player_data.bin",
    "partnership.bin",
    "model_data.bin",
    "player_data_diff.bin",
];

/// Every mapping the game reads, named `<id>_...` (the edition's game id for the game; tests use a name of their own).
/// 2017-18: its player data, then `<id>_ WCCF 17-18 Ver2.0` byte[0]=1 (control waits up to 60 s for it); no moai and no
/// moSuppl.  The handles keep them alive: hold them for the run.
pub fn make_all(
    out: &mut dyn Write,
    game: &Path,
    id: &str,
    with_mosuppl: bool,
    edition: Edition,
) -> io::Result<Vec<OwnedHandle>> {
    let mut handles = Vec::new();
    if edition == Edition::E1718 {
        for name in SDDY_DATA {
            let Ok(data) = fs::read(game.join("prog_data/player_data").join(name)) else {
                continue;
            };
            let mut payload = (data.len() as u32).to_le_bytes().to_vec();
            payload.extend_from_slice(&data);
            handles.extend(make(out, &format!("{id}_{name}"), &payload, data.len() + 4));
        }
        handles.extend(make(out, &format!("{id}_ WCCF 17-18 Ver2.0"), &[1], 8));
        return Ok(handles);
    }
    for (name, path) in MOAI {
        let data = fs::read(game.join(path))?;
        let mut payload = (data.len() as u32).to_le_bytes().to_vec();
        payload.extend_from_slice(&data);
        handles.extend(make(out, &format!("{id}_{name}"), &payload, data.len() + 4));
    }
    handles.extend(make(out, &format!("{id}_status"), &[1], 64));
    if with_mosuppl {
        let body = mosuppl_bytes(game)?;
        handles.extend(make(out, &format!("{id}_moSuppl"), &body, body.len()));
    }
    Ok(handles)
}
