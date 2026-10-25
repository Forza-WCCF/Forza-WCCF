//! sddy_maps.exe GAME_DIR: the shared memory the 2017-18 Launcher makes before it starts control, for the Linux shop (run
//! under Wine by `wccf-linux-server --edition 2017-18`, which has no Launcher). Same layout as the Launcher's
//! (crates/launcher/src/debug/maps.rs `make_all`, 2017-18): `SDDY_<file>` = a u32 size + `prog_data/player_data/<file>`
//! for each file that exists, then `SDDY_ WCCF 17-18 Ver2.0` with byte 0 = 1, which control waits up to 60 s for. Prints one
//! line per mapping, then `holding`, and keeps them until it is stopped. Not shipped to players.
use std::path::Path;
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingA, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE,
};
use windows_sys::Win32::System::Threading::{INFINITE, Sleep};

const ID: &str = "SDDY";
const DATA: [&str; 4] = [
    "player_data.bin",
    "partnership.bin",
    "model_data.bin",
    "player_data_diff.bin",
];

/// Every mapping to make, as (name, payload, size).
fn plan(game: &Path) -> Vec<(String, Vec<u8>, usize)> {
    let mut out = Vec::new();
    for name in DATA {
        let Ok(data) = std::fs::read(game.join("prog_data/player_data").join(name)) else {
            continue;
        };
        let mut payload = (data.len() as u32).to_le_bytes().to_vec();
        payload.extend_from_slice(&data);
        out.push((format!("{ID}_{name}"), payload, data.len() + 4));
    }
    out.push((format!("{ID}_ WCCF 17-18 Ver2.0"), vec![1], 8));
    out
}

/// Makes (or opens) the named mapping and writes `payload` at its start. The handle stays open for the process's life.
fn make(name: &str, payload: &[u8], size: usize) -> bool {
    let cname = format!("{name}\0");
    // SAFETY: a pagefile-backed mapping with a NUL-ended name; the view is `size` >= payload.len() bytes
    unsafe {
        let h = CreateFileMappingA(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            0,
            size as u32,
            cname.as_ptr(),
        );
        let existed = GetLastError() == ERROR_ALREADY_EXISTS;
        if h.is_null() {
            println!("  FAIL create {name} err={}", GetLastError());
            return false;
        }
        let view = MapViewOfFile(h, FILE_MAP_WRITE, 0, 0, size);
        if view.Value.is_null() {
            println!("  FAIL map {name} err={}", GetLastError());
            return false;
        }
        std::ptr::copy_nonoverlapping(payload.as_ptr(), view.Value.cast::<u8>(), payload.len());
        println!(
            "  {} {name:<34} size={size}",
            if existed { "opened" } else { "made  " }
        );
    }
    true
}

fn main() {
    let Some(game) = std::env::args().nth(1) else {
        eprintln!("usage: sddy_maps.exe GAME_DIR");
        std::process::exit(2);
    };
    let mut ok = true;
    for (name, payload, size) in plan(Path::new(&game)) {
        ok &= make(&name, &payload, size);
    }
    if !ok {
        std::process::exit(1);
    }
    println!("holding");
    loop {
        // SAFETY: plain call
        unsafe { Sleep(INFINITE) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{FILE_MAP_READ, OpenFileMappingA};

    #[test]
    fn plan_matches_the_launchers_layout() {
        let game = std::env::temp_dir().join(format!("sddy-plan-{}", std::process::id()));
        let dir = game.join("prog_data/player_data");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("partnership.bin"), [7, 8, 9]).unwrap();
        std::fs::write(dir.join("player_data.bin"), [1; 5]).unwrap();
        let plan = plan(&game);
        std::fs::remove_dir_all(&game).unwrap();
        // data files in the Launcher's order, missing ones skipped, the version mapping last
        let names: Vec<_> = plan.iter().map(|p| p.0.as_str()).collect();
        assert_eq!(
            names,
            [
                "SDDY_player_data.bin",
                "SDDY_partnership.bin",
                "SDDY_ WCCF 17-18 Ver2.0"
            ]
        );
        assert_eq!(plan[1].1, [3, 0, 0, 0, 7, 8, 9]);
        assert_eq!(plan[1].2, 7);
        assert_eq!((plan[2].1.as_slice(), plan[2].2), (&[1u8][..], 8));
    }

    #[test]
    fn make_is_readable_by_name() {
        let name = format!("sddy_maps_test_{}", std::process::id());
        assert!(make(&name, &[4, 0, 0, 0, 42], 16));
        let cname = format!("{name}\0");
        // SAFETY: opens the mapping made above by its NUL-ended name and reads inside its 16 bytes
        let seen = unsafe {
            let h = OpenFileMappingA(FILE_MAP_READ, 0, cname.as_ptr());
            assert!(!h.is_null());
            let v = MapViewOfFile(h, FILE_MAP_READ, 0, 0, 16);
            std::slice::from_raw_parts(v.Value.cast::<u8>(), 5).to_vec()
        };
        assert_eq!(seen, [4, 0, 0, 0, 42]);
    }
}
