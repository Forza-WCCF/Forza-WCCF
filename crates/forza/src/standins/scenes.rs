//! Stand-in for the launcher's scene service, xscnWatch (2010–11): loads scene files on request. Port of
//! `legacy/2010-11/code/hardware/scenes.py` (step 10, piece B).
//!
//! What the original does (Launcher1011.exe, read 2026-10-04): xscnWatch::Watcher owns the mutex `xscnWatch::Mutex`
//! and a 0x5CC-byte board `xscnWatch::ComBuf`:
//!   dword 0   request counter (a requester adds 1 after filling a slot)
//!   dword 1   16 = number of request slots
//!   dword 2   32 = number of scene entries
//!   +0x0C     16 request slots x 12 bytes: {type, pid, scene number}   (type 1 = load)
//!   +0xCC     32 scene entries x 40 bytes: {scene number, state, 8 more dwords}   (state 2 = ready)
//! A requester (the match engine) takes the mutex (waits 0x21 ms), looks for its scene in the entries; if absent it
//! fills a free slot and adds 1 to the counter; once the entry's state is 2 it opens the mapping `%06ld.xscn` =
//! [4-byte size][the file]. The watcher, whenever the counter changed, handles every filled slot and clears it, then
//! rewrites the whole entry table. Files: `xscn/<number>_<name>.xscn`.
//!
//! This service reads requests under the mutex, loads OUTSIDE it (some files are 20–37 MB; requesters give up on the
//! mutex after 33 ms), marks a scene ready on the next pass, keeps every scene loaded until it stops, and logs each
//! request (repeats of one scene in one pass are summed). Requests of other types are logged and the scene stays
//! loaded. It waits up to 250 ms for the mutex (a requester that finds its scene not ready sleeps 16 ms BEFORE
//! releasing it, so four match engines keep it held almost all the time and a 4 ms wait starved).
//!
//! A board left open by requesters (they reopen it every pass) is adopted, not refused. [`spawn`] refuses only when
//! another copy holds the marker mutex.

use super::{Handle, Log};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, sleep};
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

pub const BOARD_SIZE: usize = 0x5CC;
pub const SLOTS: usize = 16;
pub const ENTRIES: usize = 32;
pub const SLOT_AT: usize = 0x0C;
pub const ENTRY_AT: usize = 0xCC;
pub const ENTRY_SIZE: usize = 40;
pub const READY: u32 = 2;

/// The names the service and its requesters share. [`Names::game`] has the game's; tests use their own.
#[derive(Clone, Debug)]
pub struct Names {
    pub mutex: String,
    pub board: String,
    /// Held while a service runs: a second copy is refused.
    pub marker: String,
    /// In front of `%06d.xscn` (the game's has none).
    pub mapping_prefix: String,
}

impl Names {
    pub fn game() -> Self {
        Self::with_prefix(
            "",
            "xscnWatch::Mutex",
            "xscnWatch::ComBuf",
            "WCCF::xscnWatchStandIn",
        )
    }

    /// Scratch names `<tag>_Mutex` and so on, for tests.
    pub fn scratch(tag: &str) -> Self {
        let (m, b, s) = (
            format!("{tag}_Mutex"),
            format!("{tag}_ComBuf"),
            format!("{tag}_StandIn"),
        );
        Self::with_prefix(&format!("{tag}_"), &m, &b, &s)
    }

    fn with_prefix(prefix: &str, mutex: &str, board: &str, marker: &str) -> Self {
        Self {
            mutex: mutex.into(),
            board: board.into(),
            marker: marker.into(),
            mapping_prefix: prefix.into(),
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// `{scene number: file path}` from the leading digits of each `*.xscn` file name (the later name wins a repeat).
pub fn scene_index(dir: &Path) -> io::Result<BTreeMap<u32, PathBuf>> {
    let mut names: Vec<String> = fs::read_dir(dir)?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    let mut out = BTreeMap::new();
    for name in names {
        let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
        if name.to_lowercase().ends_with(".xscn")
            && let Ok(n) = digits.parse()
        {
            out.insert(n, dir.join(&name));
        }
    }
    Ok(out)
}

/// A kernel handle owned by the service thread.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Create* call and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// The 0x5CC-byte board, read and written as dwords through a mapped view.
struct Board(*mut u32);

impl Board {
    fn get(&self, off: usize) -> u32 {
        // SAFETY: `off` is a multiple of 4 inside the 0x5CC-byte view, which stays mapped while the service runs.
        unsafe { self.0.add(off / 4).read_volatile() }
    }

    fn put(&self, off: usize, v: u32) {
        // SAFETY: as in `get`.
        unsafe { self.0.add(off / 4).write_volatile(v) }
    }

    fn zero(&self, off: usize, bytes: usize) {
        (0..bytes / 4).for_each(|i| self.put(off + 4 * i, 0));
    }
}

impl Drop for Board {
    fn drop(&mut self) {
        // SAFETY: the view was mapped by `MapViewOfFile` and is unmapped once.
        unsafe {
            UnmapViewOfFile(
                windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.0.cast(),
                },
            )
        };
    }
}

fn last_error() -> u32 {
    // SAFETY: plain thread-local read.
    unsafe { GetLastError() }
}

fn lock(mutex: &Owned, ms: u32) -> bool {
    // SAFETY: a valid mutex handle.
    let r = unsafe { WaitForSingleObject(mutex.0, ms) };
    matches!(r, WAIT_OBJECT_0 | WAIT_ABANDONED)
}

fn unlock(mutex: &Owned) {
    // SAFETY: called after a successful `lock`.
    unsafe { ReleaseMutex(mutex.0) };
}

/// Starts the service on a thread and returns once the board is ready. Dropping the handle stops it and unloads the
/// scenes. Fails when the folder cannot be read, when another copy runs (`AlreadyExists`), or when the mutex or board
/// cannot be made or taken within 5 s.
pub fn spawn(xscn_dir: &Path, names: &Names, log: Log) -> io::Result<Handle> {
    let index = scene_index(xscn_dir)?;
    log(&format!(
        "xscnWatch stand-in: {} scenes in {} ({})",
        index.len(),
        xscn_dir.display(),
        index
            .keys()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let (flag, names) = (stop.clone(), names.clone());
    let (tx, rx) = mpsc::channel();
    let thread = thread::spawn(move || match open(&names, &log) {
        Ok(svc) => {
            let _ = tx.send(Ok(()));
            run(svc, &index, &names, &flag, &log);
        }
        Err(e) => {
            let _ = tx.send(Err(e));
        }
    });
    let handle = Handle::new(stop, thread);
    rx.recv()
        .map_err(|_| io::Error::other("the scene service thread ended"))??;
    Ok(handle)
}

struct Service {
    _marker: Owned,
    mutex: Owned,
    _hboard: Owned,
    board: Board,
}

fn open(names: &Names, log: &Log) -> io::Result<Service> {
    let (mw, bw, kw) = (wide(&names.mutex), wide(&names.board), wide(&names.marker));
    // SAFETY: the names are NUL-terminated and outlive the calls; no security attributes.
    let marker = unsafe { CreateMutexW(std::ptr::null(), 0, kw.as_ptr()) };
    if marker.is_null() || last_error() == ERROR_ALREADY_EXISTS {
        if !marker.is_null() {
            drop(Owned(marker));
        }
        log("REFUSED: another copy of this stand-in is running");
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "another scene service is running",
        ));
    }
    let marker = Owned(marker);
    // SAFETY: as above.
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mw.as_ptr()) };
    // SAFETY: as above; a pagefile-backed mapping of BOARD_SIZE bytes.
    let hboard = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            0,
            BOARD_SIZE as u32,
            bw.as_ptr(),
        )
    };
    let adopted = last_error() == ERROR_ALREADY_EXISTS;
    if mutex.is_null() || hboard.is_null() {
        let e = io::Error::last_os_error();
        log(&format!(
            "could not create the mutex or the board (err {})",
            e.raw_os_error().unwrap_or(0)
        ));
        return Err(e);
    }
    let (mutex, hboard) = (Owned(mutex), Owned(hboard));
    // SAFETY: a valid mapping handle; the whole mapping.
    let view = unsafe { MapViewOfFile(hboard.0, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
    if view.Value.is_null() {
        return Err(io::Error::last_os_error());
    }
    let board = Board(view.Value.cast());
    if !lock(&mutex, 5000) {
        log("could not take xscnWatch::Mutex within 5 s - something holds it");
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the board's mutex is held",
        ));
    }
    let counter_was = if adopted { board.get(0) } else { 0 };
    if adopted {
        board.zero(ENTRY_AT, ENTRIES * ENTRY_SIZE); // stale entries: requesters will ask again
    } else {
        board.zero(0, BOARD_SIZE);
    }
    board.put(4, SLOTS as u32);
    board.put(8, ENTRIES as u32);
    unlock(&mutex);
    log(&format!(
        "board ready ({}): xscnWatch::Mutex + xscnWatch::ComBuf (0x5CC) = [counter {counter_was}, 16 slots, 32 entries]",
        if adopted {
            "ADOPTED - requesters had it open"
        } else {
            "new"
        }
    ));
    Ok(Service {
        _marker: marker,
        mutex,
        _hboard: hboard,
        board,
    })
}

fn run(svc: Service, index: &BTreeMap<u32, PathBuf>, names: &Names, stop: &AtomicBool, log: &Log) {
    let Service { mutex, board, .. } = &svc;
    let mut loaded: BTreeMap<u32, Owned> = BTreeMap::new(); // kept open = the scene stays available
    let mut pending: Vec<(u32, u32, u32)> = vec![]; // (type, pid, scene), handled outside the mutex
    let mut last_counter = None; // None: read the slots on the first pass whatever the counter says
    while !stop.load(Ordering::Relaxed) {
        if lock(mutex, 250) {
            let counter = board.get(0);
            if last_counter != Some(counter) {
                last_counter = Some(counter);
                for i in 0..SLOTS {
                    let off = SLOT_AT + 12 * i;
                    let (typ, pid, scene) =
                        (board.get(off), board.get(off + 4), board.get(off + 8));
                    if typ != 0 && pid != 0 && 0 < scene && scene < 0x8000_0000 {
                        pending.push((typ, pid, scene));
                    }
                    board.zero(off, 12);
                }
            }
            board.zero(ENTRY_AT, ENTRIES * ENTRY_SIZE);
            for (i, scene) in loaded.keys().take(ENTRIES).enumerate() {
                board.put(ENTRY_AT + ENTRY_SIZE * i, *scene);
                board.put(ENTRY_AT + ENTRY_SIZE * i + 4, READY);
            }
            unlock(mutex);
        }
        let mut repeats: BTreeMap<u32, (u32, Vec<u32>)> = BTreeMap::new();
        for (typ, pid, scene) in pending.drain(..) {
            let seen = loaded.contains_key(&scene);
            if typ != 1 {
                log(&format!(
                    "request type {typ} for scene {scene} from pid {pid} - logged only (scene stays loaded)"
                ));
            } else if seen {
                let r = repeats.entry(scene).or_default();
                r.0 += 1;
                if !r.1.contains(&pid) {
                    r.1.push(pid);
                }
            } else if let Some(path) = index.get(&scene) {
                match load(path, scene, names) {
                    Ok((h, len, name)) => {
                        let file = path.file_name().unwrap_or_default().to_string_lossy();
                        log(&format!(
                            "scene {scene} loaded for pid {pid}: {file}, {len} bytes -> mapping {name}, marked ready"
                        ));
                        loaded.insert(scene, h);
                    }
                    Err((name, err)) => log(&format!(
                        "scene {scene}: could not make mapping {name} (err {err})"
                    )),
                }
            } else {
                log(&format!(
                    "scene {scene} asked by pid {pid} - NO FILE for it in xscn/ (not served)"
                ));
            }
        }
        for (scene, (n, mut pids)) in repeats {
            pids.sort_unstable();
            let pids: Vec<String> = pids.iter().map(u32::to_string).collect();
            log(&format!(
                "scene {scene} asked again {n} time(s) by pid {} - already loaded",
                pids.join(", ")
            ));
        }
        sleep(Duration::from_millis(5));
    }
    log(&format!(
        "stopping; {} scene(s) were loaded: {:?}",
        loaded.len(),
        loaded.keys().collect::<Vec<_>>()
    ));
}

/// Reads the file into the mapping `<prefix>%06d.xscn` = [4-byte size][file]; the mapping lives as long as the handle.
fn load(path: &Path, scene: u32, names: &Names) -> Result<(Owned, usize, String), (String, u32)> {
    let name = format!("{}{scene:06}.xscn", names.mapping_prefix);
    let data = fs::read(path).map_err(|e| (name.clone(), e.raw_os_error().unwrap_or(0) as u32))?;
    let total = data.len() as u64 + 4;
    let w = wide(&name);
    // SAFETY: `w` is NUL-terminated; a pagefile-backed mapping of `total` bytes.
    let h = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            (total >> 32) as u32,
            total as u32,
            w.as_ptr(),
        )
    };
    if h.is_null() {
        return Err((name, last_error()));
    }
    let h = Owned(h);
    // SAFETY: a valid mapping handle; the whole mapping.
    let v = unsafe { MapViewOfFile(h.0, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
    if v.Value.is_null() {
        return Err((name, last_error()));
    }
    // SAFETY: the view is `total` bytes, writable, and not aliased; it is unmapped right after.
    unsafe {
        let p: *mut u8 = v.Value.cast();
        p.copy_from_nonoverlapping((data.len() as u32).to_le_bytes().as_ptr(), 4);
        p.add(4).copy_from_nonoverlapping(data.as_ptr(), data.len());
        UnmapViewOfFile(v);
    }
    Ok((h, data.len(), name))
}
