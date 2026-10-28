//! launcher::debug with the stand-ins `cargo xtask build` makes (target/kit/2010-11/source/build), copied under names
//! of their own into a scratch folder: nothing of the game runs, no game name or shared-memory name is used.
//! Port of legacy/2010-11/tests/test_crash_dump.py, plus a closed window, hidden windows and the shared memory.

use std::fs;
use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use launcher::debug::{Options, Role, maps, run};
use launcher::job::Job;
use launcher::kit::Edition;
use windows_sys::Win32::System::Memory::{FILE_MAP_READ, MapViewOfFile, UnmapViewOfFile};

/// A stand-in from the kit build, copied into `dir` as `name`.
fn stand_in(exe: &str, dir: &Path, name: &str) -> PathBuf {
    // tests run in crates/launcher
    let built = Path::new("../../target/kit/2010-11/source/build").join(exe);
    assert!(
        built.is_file(),
        "{} is missing: run cargo xtask build first",
        built.display()
    );
    fs::create_dir_all(dir).unwrap();
    fs::copy(&built, dir.join(name)).unwrap();
    dir.join(name)
}

fn options(exe: PathBuf, top: &Path, role: Role) -> Options {
    Options {
        role,
        exe,
        args: Vec::new(),
        timeout: Duration::from_secs(30),
        follow: true,
        hide: false,
        debug_heap: false,
        maps: None,
        dbg_file: top.join("data/logs/_dbg_client.txt"),
        dumps: top.join("data/dumps"),
        index: None,
        msgguard: Some(true),
        env: Vec::new(),
        stop: None,
    }
}

/// The log `run` writes, shared with the test thread.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Log {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// `run` on a thread of its own (it becomes the debugger); the job, the log and the thread.
fn start(opts: Options) -> (Arc<Job>, Log, thread::JoinHandle<i32>) {
    let job = Arc::new(Job::new().unwrap());
    let log = Log::default();
    let (j, mut l) = (job.clone(), log.clone());
    fs::create_dir_all(opts.dbg_file.parent().unwrap()).unwrap();
    let t = thread::spawn(move || run(&opts, &j, &mut l).unwrap());
    (job, log, t)
}

/// The pid from "launched NAME pid N ...", once the log has it.
fn launched_pid(log: &Log) -> u32 {
    let t = Instant::now();
    loop {
        if let Some(l) = log.text().lines().find(|l| l.starts_with("launched ")) {
            return l
                .split(" pid ")
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap()
                .parse()
                .unwrap();
        }
        assert!(t.elapsed() < Duration::from_secs(20), "not launched: {}", log.text());
        thread::sleep(Duration::from_millis(50));
    }
}

/// test_crash_dump.py: a write to address 0 in a 32-bit program, run as a client.
#[test]
fn a_crash_leaves_a_lean_and_a_full_snapshot() {
    let top = tempfile::tempdir().unwrap();
    let exe = stand_in("crashme.exe", &top.path().join("game"), "standin_crash.exe");
    let (job, log, t) = start(options(exe, top.path(), Role::Client));
    let code = t.join().unwrap();
    let out = log.text();
    assert!(
        out.contains("SECOND-chance / FATAL"),
        "the crash is reported as fatal: {out}"
    );
    assert!(
        out.contains("memory snapshot (fatal exception 0xC0000005"),
        "the report names the snapshots: {out}"
    );
    assert!(out.contains("(write of 0x00000000)"), "{out}");
    let dumps = top.path().join("data/dumps");
    let mut names: Vec<String> = fs::read_dir(&dumps)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let kinds: Vec<&str> = names.iter().map(|n| n.rsplit('_').next().unwrap()).collect();
    assert_eq!(
        kinds,
        ["full.dmp", "lean.dmp"],
        "data\\dumps has one lean and one full snapshot"
    );
    assert!(names[0].contains("_standin_crash_"), "{names:?}");
    for n in &names {
        assert_eq!(
            &fs::read(dumps.join(n)).unwrap()[..4],
            b"MDMP",
            "both are real minidumps"
        );
    }
    let sizes: Vec<u64> = names
        .iter()
        .map(|n| fs::metadata(dumps.join(n)).unwrap().len())
        .collect();
    assert!(
        sizes[0] > sizes[1],
        "the full one holds more than the lean one: {sizes:?}"
    );
    assert_eq!(code, 1, "the debugger ends with an error (the program crashed)");
    assert!(
        out.contains("  time the game was held per debug event: exception "),
        "{out}"
    );
    assert!(out.contains("  exception counts: 0xC0000005 x"), "{out}");
    assert!(job.wait_empty(Duration::from_secs(10)), "the job is empty");
}

/// A client whose window is closed runs on without it, as the game does: the debugger ends it by its own handle.
/// Children watched from outside (no follow) on the way.
#[test]
fn a_closed_window_ends_the_program() {
    let top = tempfile::tempdir().unwrap();
    let exe = stand_in("fakegame.exe", &top.path().join("game"), "standin_window.exe");
    let mut opts = options(exe, top.path(), Role::Client);
    // any DLL for its panel slot; the window shows at once, closes after ~8.5 s (a window must have been up 5 s), and
    // the program runs on for a minute
    opts.args = ["version.dll", "wait:6000", "close", "wait:60000"]
        .map(String::from)
        .to_vec();
    opts.follow = false;
    let t0 = Instant::now();
    let (job, log, t) = start(opts);
    let code = t.join().unwrap();
    let out = log.text();
    assert!(
        out.contains("its window was closed - ending standin_window.exe (the program never quits by itself)"),
        "{out}"
    );
    assert!(out.contains("  children at the end: none"), "{out}");
    assert_eq!(code, 0);
    assert!(job.wait_empty(Duration::from_secs(10)), "the job is empty: {out}");
    assert!(t0.elapsed() < Duration::from_secs(30), "{:?}", t0.elapsed());
}

/// The stand-in for `hide`: this test program again, showing a window the way a program shows its first one
/// (`SW_SHOWDEFAULT`: what the starter asked for), for 3 s.  Runs only when a test starts it (`--ignored`).
#[test]
#[ignore = "a stand-in started by hide_starts_the_windows_hidden"]
fn window_stand_in() {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, RegisterClassW, SW_SHOWDEFAULT, ShowWindow, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };
    let class: Vec<u16> = "wccf-debug-stand-in\0".encode_utf16().collect();
    // SAFETY: zeroed WNDCLASSW plus the fields set is a valid class; the strings outlive the calls.
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: GetModuleHandleW(std::ptr::null()),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let w = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            -32000,
            -32000,
            200,
            100,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            wc.hInstance,
            std::ptr::null(),
        );
        assert!(!w.is_null());
        ShowWindow(w, SW_SHOWDEFAULT);
    }
    thread::sleep(Duration::from_secs(3));
}

/// `hide` (the server's start): the program's window exists but is not visible; without it the same window shows.
#[test]
fn hide_starts_the_windows_hidden() {
    for hide in [true, false] {
        let top = tempfile::tempdir().unwrap();
        let mut opts = options(std::env::current_exe().unwrap(), top.path(), Role::Control);
        opts.args = ["debug::window_stand_in", "--exact", "--ignored", "--nocapture"]
            .map(String::from)
            .to_vec();
        opts.hide = hide;
        let (job, log, t) = start(opts);
        let pid = launched_pid(&log);
        let since = Instant::now();
        let mut wins = Vec::new();
        while wins.is_empty() && since.elapsed() < Duration::from_secs(10) {
            thread::sleep(Duration::from_millis(100));
            wins = launcher::procs::windows(pid)
                .into_iter()
                .filter(|w| w.class == "wccf-debug-stand-in")
                .collect();
        }
        thread::sleep(Duration::from_millis(300));
        let visible = launcher::procs::windows(pid)
            .iter()
            .any(|w| w.class == "wccf-debug-stand-in" && w.visible);
        let code = t.join().unwrap();
        let out = log.text();
        assert!(!wins.is_empty(), "no window at all: {out}");
        assert_eq!(visible, !hide, "hide={hide}: {out}");
        assert_eq!(out.contains("; its windows hidden"), hide, "{out}");
        assert!(out.contains("process exited, exit code 0x00000000"), "{out}");
        assert_eq!(code, 0);
        assert!(job.wait_empty(Duration::from_secs(10)));
    }
}

/// make_all on made-up motion files under a name of the test's own: every mapping holds its payload; a mapping that
/// already exists too small is reported and left alone.
#[test]
fn the_shared_memory_holds_the_motion_files() {
    let game = tempfile::tempdir().unwrap();
    let files = [
        ("AI/motion_data/game_motion_data.moai", 5),
        ("data/motion/launcherMotion/management_motion_data.moai", 0),
        ("data/motion/launcherMotion/scene_motion_data.moai", 9),
        ("AI/motion_data/game_motion_table_moai.bin", 3),
        ("AI/motion_data/game_actionsetup_table_moai.bin", 4),
        ("AI/motion_data/game_movepack_table_moai.bin", 1),
        ("AI/motion_data/game_request_table_moai.bin", 6),
    ];
    for (i, (f, n)) in files.iter().enumerate() {
        let p = game.path().join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![i as u8 + 1; *n]).unwrap();
    }
    let id = format!("wccftest_{}", std::process::id());
    let mut out = Vec::new();
    let handles = maps::make_all(&mut out, game.path(), &id, true, Edition::E1011).unwrap();
    assert_eq!(handles.len(), 5);
    let view = |i: usize, n: usize| {
        // SAFETY: a live mapping handle; the view is read within its size and unmapped.
        unsafe {
            let v = MapViewOfFile(handles[i].as_raw_handle(), FILE_MAP_READ, 0, 0, 0);
            assert!(!v.Value.is_null());
            let b = std::slice::from_raw_parts(v.Value.cast::<u8>(), n).to_vec();
            UnmapViewOfFile(v);
            b
        }
    };
    assert_eq!(view(0, 9), [5, 0, 0, 0, 1, 1, 1, 1, 1]);
    assert_eq!(view(1, 4), [0, 0, 0, 0]);
    assert_eq!(view(3, 2), [1, 0]);
    assert_eq!(view(4, 16 + 4 + 4 + 4 + 8), maps::mosuppl_bytes(game.path()).unwrap());
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains(&format!("  made   {id}_status")), "{text}");

    // already there and too small (another process holds an older copy): reported, not written
    let mut out = Vec::new();
    let small = maps::make(&mut out, &format!("{id}_small"), &[], 16).unwrap();
    let again = maps::make(&mut out, &format!("{id}_small"), &[7; 5000], 5000);
    let text = String::from_utf8(out).unwrap();
    assert!(again.is_some());
    assert!(
        text.contains("already exists and is only 4096 bytes (need 5000): NOT written"),
        "{text}"
    );
    drop(small);
}

/// 2017-18's mappings: the player data files there are (a size, then the file) and the version byte, nothing of
/// 2010-11's.
#[test]
fn the_2017_18_shared_memory_holds_the_player_data() {
    let game = tempfile::tempdir().unwrap();
    let dir = game.path().join("prog_data/player_data");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("player_data.bin"), [7, 8, 9]).unwrap();
    fs::write(dir.join("model_data.bin"), [4]).unwrap();
    let id = format!("wccftest1718_{}", std::process::id());
    let mut out = Vec::new();
    let handles = maps::make_all(&mut out, game.path(), &id, true, Edition::E1718).unwrap();
    assert_eq!(handles.len(), 3, "two files and the version");
    let view = |i: usize, n: usize| {
        // SAFETY: a live mapping handle; the view is read within its size and unmapped.
        unsafe {
            let v = MapViewOfFile(handles[i].as_raw_handle(), FILE_MAP_READ, 0, 0, 0);
            assert!(!v.Value.is_null());
            let b = std::slice::from_raw_parts(v.Value.cast::<u8>(), n).to_vec();
            UnmapViewOfFile(v);
            b
        }
    };
    assert_eq!(view(0, 7), [3, 0, 0, 0, 7, 8, 9]);
    assert_eq!(view(1, 5), [1, 0, 0, 0, 4]);
    assert_eq!(view(2, 1), [1]);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains(&format!("{id}_ WCCF 17-18 Ver2.0")), "{text}");
    assert!(!text.contains("moai") && !text.contains("status"), "{text}");
}
