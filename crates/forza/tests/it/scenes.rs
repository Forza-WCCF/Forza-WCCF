//! Replays `tests/rec/scenes.jsonl` (made by `legacy/2010-11/tests/rec_b_scenes.py` from the Python scene service
//! with a requester playing a match engine) against `forza::standins::scenes` and compares the board byte for byte,
//! the scene mappings and the log lines. Scratch names carry this process's pid; the game's names are never used.
#![cfg(windows)]

use crate::records;
use forza::standins::scenes::{BOARD_SIZE, Names, spawn};
use forza::standins::{Handle, Log};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
    UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenMutexW, ReleaseMutex, WaitForSingleObject,
};

const SYNCHRONIZE: u32 = 0x0010_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A match engine: opens (or, for adoption, creates) the mutex and board by name.
struct Requester {
    mutex: HANDLE,
    view: *mut u8,
    prefix: String,
}

impl Requester {
    fn new(names: &Names, create: bool) -> Self {
        let (m, b) = (wide(&names.mutex), wide(&names.board));
        // SAFETY: NUL-terminated names that outlive the calls; the handles are checked and live until the process ends.
        let (mutex, hb) = unsafe {
            if create {
                (
                    CreateMutexW(std::ptr::null(), 0, m.as_ptr()),
                    CreateFileMappingW(
                        INVALID_HANDLE_VALUE,
                        std::ptr::null(),
                        PAGE_READWRITE,
                        0,
                        BOARD_SIZE as u32,
                        b.as_ptr(),
                    ),
                )
            } else {
                (
                    OpenMutexW(SYNCHRONIZE, 0, m.as_ptr()),
                    OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, b.as_ptr()),
                )
            }
        };
        assert!(!mutex.is_null() && !hb.is_null(), "board not there");
        // SAFETY: a valid mapping handle.
        let view = unsafe { MapViewOfFile(hb, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
        Self {
            mutex,
            view: view.Value.cast(),
            prefix: names.mapping_prefix.clone(),
        }
    }

    fn locked<T>(&self, f: impl FnOnce() -> T) -> T {
        // SAFETY: a valid mutex handle; released below.
        let waited = unsafe { WaitForSingleObject(self.mutex, 5000) };
        assert!(matches!(waited, 0 | 0x80));
        let r = f();
        // SAFETY: we hold it.
        unsafe { ReleaseMutex(self.mutex) };
        r
    }

    fn write(&self, writes: &[(usize, u32)]) {
        self.locked(|| {
            for &(off, v) in writes {
                // SAFETY: offsets from the recording are dword-aligned and inside the board.
                unsafe { self.view.add(off).cast::<u32>().write_volatile(v) };
            }
        });
    }

    fn snap(&self) -> Vec<u8> {
        // SAFETY: the view is BOARD_SIZE bytes.
        self.locked(|| unsafe { std::slice::from_raw_parts(self.view, BOARD_SIZE).to_vec() })
    }

    /// The board once it stops changing (0.5 s, then two equal snapshots 0.3 s apart), as the recorder does.
    fn stable(&self) -> Vec<u8> {
        sleep(Duration::from_millis(500));
        let mut prev = self.snap();
        for _ in 0..60 {
            sleep(Duration::from_millis(300));
            let cur = self.snap();
            if cur == prev {
                return cur;
            }
            prev = cur;
        }
        panic!("board never settled");
    }

    fn mapping(&self, scene: u32) -> Option<(u32, String)> {
        let name = wide(&format!("{}{scene:06}.xscn", self.prefix));
        // SAFETY: NUL-terminated name; the view holds [u32 size][bytes] as the service wrote it.
        unsafe {
            let h = OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr());
            if h.is_null() {
                return None;
            }
            let v = MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            let p: *const u8 = v.Value.cast();
            let size = p.cast::<u32>().read_unaligned();
            let data = std::slice::from_raw_parts(p.add(4), size as usize);
            let sha: String = Sha256::digest(data)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            UnmapViewOfFile(v);
            CloseHandle(h);
            Some((size, sha))
        }
    }
}

fn content(seed: u64, size: u64) -> Vec<u8> {
    (0..size)
        .map(|i| (i * 7 + seed * 13 + (i >> 8)) as u8)
        .collect()
}

fn logger() -> (Log, Arc<Mutex<Vec<String>>>) {
    let lines = Arc::new(Mutex::new(vec![]));
    let sink = lines.clone();
    (
        Arc::new(move |m| sink.lock().unwrap().push(m.to_owned())),
        lines,
    )
}

/// The lines since the last call, sorted, with the recorder's normalisation (the folder, the name prefix).
fn new_lines(lines: &Mutex<Vec<String>>, dir: &str, prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = lines
        .lock()
        .unwrap()
        .drain(..)
        .filter(|l| !l.starts_with("stopping;"))
        .map(|l| l.replace(prefix, "").replace(dir, "<dir>"))
        .collect();
    v.sort();
    v
}

fn json_list(v: &Value) -> Vec<Value> {
    v.as_array().unwrap().clone()
}

fn pairs(v: &Value) -> Vec<(usize, u32)> {
    json_list(v)
        .iter()
        .map(|p| {
            (
                p[0].as_u64().unwrap() as usize,
                p[1].as_u64().unwrap() as u32,
            )
        })
        .collect()
}

#[test]
fn replays_the_recorded_requester() {
    let recs = records("scenes");
    let mut i = 0;
    let mut scenarios = 0;
    while i < recs.len() {
        let s = &recs[i];
        assert_eq!(s["t"], "scenario");
        i += 1;
        let name = s["name"].as_str().unwrap();
        let tag = format!("FORZATEST{}_{name}", std::process::id());
        let names = Names::scratch(&tag);
        let tmp = std::env::temp_dir().join(format!("forza_scenes_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        for (file, v) in s["files"].as_object().unwrap() {
            std::fs::write(
                tmp.join(file),
                content(v[0].as_u64().unwrap(), v[1].as_u64().unwrap()),
            )
            .unwrap();
        }
        let dir = tmp.display().to_string();
        let (log, lines) = logger();
        let pre = pairs(&s["pre"]);
        let mut req = (!pre.is_empty()).then(|| Requester::new(&names, true));
        if let Some(r) = &req {
            r.write(&pre);
        }
        if name == "refused" {
            let first = spawn(&tmp, &names, log.clone()).expect("first copy");
            let err = spawn(&tmp, &names, log).err().expect("second copy refused");
            assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
            let r = &recs[i];
            i += 1;
            assert_eq!(r["exit"], 2);
            let got = new_lines(&lines, &dir, &names.mapping_prefix);
            // the recorded service only had the second copy's lines: its own "stand-in:" line and the refusal
            let want: Vec<String> = json_list(&r["log"])
                .iter()
                .map(|l| l.as_str().unwrap().to_owned())
                .collect();
            for l in &want {
                assert!(got.contains(l), "missing {l:?} in {got:?}");
            }
            drop(first);
            scenarios += 1;
            continue;
        }
        let svc: Handle = spawn(&tmp, &names, log).expect("spawn");
        req = Some(req.unwrap_or_else(|| Requester::new(&names, false)));
        let req = req.unwrap();
        while i < recs.len() && recs[i]["t"] == "step" {
            let r = &recs[i];
            i += 1;
            let step = r["name"].as_str().unwrap();
            let writes = pairs(&r["writes"]);
            if !writes.is_empty() {
                req.write(&writes);
            }
            let board = req.stable();
            let want: Vec<u8> = (0..r["board"].as_str().unwrap().len() / 2)
                .map(|k| {
                    u8::from_str_radix(&r["board"].as_str().unwrap()[k * 2..k * 2 + 2], 16).unwrap()
                })
                .collect();
            assert_eq!(board, want, "{name}/{step}: board");
            for (scene, want) in r["maps"].as_object().unwrap() {
                let got = req.mapping(scene.parse().unwrap());
                let want = want.as_array().map(|a| {
                    (
                        a[0].as_u64().unwrap() as u32,
                        a[1].as_str().unwrap().to_owned(),
                    )
                });
                assert_eq!(got, want, "{name}/{step}: mapping {scene}");
            }
            let want: Vec<String> = json_list(&r["log"])
                .iter()
                .map(|l| l.as_str().unwrap().to_owned())
                .collect();
            assert_eq!(
                new_lines(&lines, &dir, &names.mapping_prefix),
                want,
                "{name}/{step}: log"
            );
        }
        drop(svc);
        let _ = std::fs::remove_dir_all(&tmp);
        scenarios += 1;
    }
    assert_eq!(scenarios, 3);
}
