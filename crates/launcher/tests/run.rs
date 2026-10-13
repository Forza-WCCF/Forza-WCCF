//! launcher::run end to end, without the game: a scratch kit in %TEMP% whose game folder holds the C stand-in
//! `fakegame.exe` (from `cargo xtask build`) under names of its own, the run given those names, free ports, scratch
//! scene names and pipes with this test's process id.  Nothing of the game runs; no game port, pipe or name is used.
//! Port of test_watch_late.py and test_setup_state.py's start decisions, plus the card rule and the Prefer-System32
//! refusal.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{env, mem, ptr, thread};

use forza::standins::io_board::{CARD_BIT, Shared};
use forza::standins::scenes::Names;
use launcher::Kit;
use launcher::run::{self, Kind, Params, Ui};

/// What the run said, and whether the game is up.
#[derive(Default)]
struct Said {
    lines: Mutex<Vec<String>>,
    up: AtomicBool,
}

impl Ui for Said {
    fn say(&self, line: &str) {
        println!("{line}");
        self.lines.lock().unwrap().push(line.to_owned());
    }
    fn running(&self, up: bool) {
        self.up.store(up, Ordering::Relaxed);
    }
}

impl Said {
    fn text(&self) -> String {
        self.lines.lock().unwrap().join("\n")
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A stand-in from the kit build, copied as `name`.
fn stand_in(exe: &str, to: &Path) {
    let built = Path::new("../../target/kit/2010-11/source/build").join(exe);
    assert!(
        built.is_file(),
        "{} is missing: run cargo xtask build first",
        built.display()
    );
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    fs::copy(&built, to).unwrap();
}

/// A kit set up for a game folder of stand-ins: `top\kit`, `top\game\extracted`, `top\game\seat1`.
fn scratch_kit(top: &Path) -> (Kit, PathBuf) {
    let kit = top.join("kit");
    let game = top.join(r"game\extracted");
    let seat = top.join(r"game\seat1");
    for d in [
        kit.join(r"data\cache"),
        kit.join("programs"),
        game.join(r"data\motion\launcherMotion\xscn"),
        seat.clone(),
    ] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(kit.join(r"data\cache\catalogue.tsv"), b"").unwrap();
    let settings = serde_json::json!({ "game": game.display().to_string() });
    fs::write(kit.join(r"data\settings.json"), settings.to_string()).unwrap();
    for dir in [kit.join("programs"), game.clone(), seat.clone()] {
        fs::write(dir.join("wccfk.dll"), b"this kit's hook").unwrap();
    }
    fs::write(seat.join(".wccf-kit"), b"").unwrap();
    stand_in("fakegame.exe", &game.join("standin_client.exe"));
    stand_in("fakegame.exe", &seat.join("standin_client.exe"));
    for n in ["standin_control.exe", "standin_match.exe"] {
        fs::write(game.join(n), b"never run").unwrap();
    }
    for dir in [&game, &seat] {
        fs::create_dir_all(dir.join("local")).unwrap();
        fs::write(
            dir.join(r"local\client_user_option.conf"),
            b"CONTROL_IP=127.0.0.1\r\nMATCH_IP=127.0.0.1\r\nSATELLITE_NO=1\r\n",
        )
        .unwrap();
    }
    (Kit::at(kit), seat)
}

fn params(tag: &str, shared: Arc<Shared>) -> Params {
    let pid = std::process::id();
    Params {
        programs: ["standin_client.exe", "standin_control.exe", "standin_match.exe"].map(String::from),
        // a window up 9 s (a close counts once it has been up 5 s), then closed by "the player"; it runs on without it
        seat_args: Some(
            ["version.dll", "wait:9000", "close", "wait:60000"]
                .map(String::from)
                .to_vec(),
        ),
        projector_args: Some(["version.dll", "wait:120000"].map(String::from).to_vec()),
        control_args: Vec::new(),
        window_wait: Duration::from_secs(120),
        match_engines: 4,
        control_port: free_port(),
        relay_port: free_port(),
        seat_port: free_port(),
        inbox_port: free_port(),
        network_ports: [0; 6],
        icc_pipe: format!(r"\\.\pipe\wccftest_icc_{tag}_{pid}"),
        jvs_pipe: format!(r"\\.\pipe\wccftest_jvs_{tag}_{pid}"),
        scenes: Names::scratch(&format!("wccftest_{tag}_{pid}")),
        maps: None,
        shared,
    }
}

/// A server's seat desk that gives seat 1 without the projector and answers its pings.
fn seat_desk(port: u16) -> TcpListener {
    let l = TcpListener::bind(("127.0.0.1", port)).unwrap();
    let desk = l.try_clone().unwrap();
    thread::spawn(move || {
        for conn in desk.incoming().flatten() {
            thread::spawn(move || {
                let mut out = conn.try_clone().unwrap();
                for line in BufReader::new(conn).lines() {
                    let Ok(line) = line else { return };
                    let reply = if line.starts_with("WANT") {
                        "SEAT 1 PROJECTOR 0\n"
                    } else {
                        "PONG\n"
                    };
                    if out.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    l
}

/// Programs running from `dir` (in the form Windows reports them).
fn running_from(dir: &Path) -> Vec<String> {
    let root = launcher::procs::same_form(dir) + "\\";
    launcher::procs::processes()
        .into_iter()
        .filter(|q| launcher::procs::normcase(&q.path).starts_with(&root))
        .map(|q| format!("{} {}", q.name, q.pid))
        .collect()
}

/// Online on a scratch server: the cabinet starts, a stop is refused while the club card is in, closing seat 1's
/// window ends the run (nothing of it left in < 30 s), and the end says the player closed it.
#[test]
fn closing_seat_1_ends_the_run_and_the_card_rule_holds() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _seat) = scratch_kit(top.path());
    let shared = Shared::new();
    shared.set_switches(0, [0, 0], [CARD_BIT, 0]); // the club card is in when the key driver starts
    let p = params("close", shared);
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap(); // "the server answers"
    let _desk = seat_desk(p.seat_port);
    let ui = Arc::new(Said::default());
    let kit_dir = kit.dir().to_path_buf();
    let (u, kd) = (ui.clone(), kit_dir.clone());
    let t = thread::spawn(move || run::command(&Kit::at(kd), &p, &["remote".into(), "127.0.0.1".into()], &*u));

    let t0 = Instant::now();
    while !ui.up.load(Ordering::Relaxed) {
        assert!(!t.is_finished(), "the start failed:\n{}", ui.text());
        assert!(
            t0.elapsed() < Duration::from_secs(120),
            "not up in 2 minutes:\n{}",
            ui.text()
        );
        thread::sleep(Duration::from_millis(100));
    }
    let said = ui.text();
    assert!(said.contains("  5/6 seat 1 (the player cabinet) - window up"), "{said}");
    assert!(said.contains("this PC is seat 1"), "{said}");
    assert!(run::is_on(&kit, Kind::Run), "the run's lock is held");
    let running = fs::read_to_string(kit.data().join("running.json")).unwrap();
    assert!(
        running.contains("\"started\"") && !running.contains("processes"),
        "a start stamp: {running}"
    );

    let (code, answer) = run::request(&kit, Kind::Run, "stop").expect("a run is on");
    assert_eq!(code, run::REFUSED, "{answer}");
    assert!(answer.contains("Your club card is IN the reader"), "{answer}");
    let (code, status) = run::request(&kit, Kind::Run, "status").unwrap();
    assert_eq!(code, run::OK);
    assert!(
        status.contains("standin_client.exe") && status.contains("club card: IN the reader"),
        "{status}"
    );

    let asked = Instant::now();
    let rc = t.join().unwrap();
    let said = ui.text();
    assert_eq!(rc, run::OK, "{said}");
    assert!(
        asked.elapsed() < Duration::from_secs(40),
        "{:?}\n{said}",
        asked.elapsed()
    );
    assert!(
        said.contains("A game window was closed - stopping the rest.") || said.contains("No game window is left"),
        "{said}"
    );
    assert!(said.contains("The game has ended: closed by the player."), "{said}");
    assert!(!run::is_on(&kit, Kind::Run), "the lock went with the run");
    assert!(
        !kit.data().join("running.json").exists(),
        "running.json goes with the run"
    );
    let left = running_from(top.path());
    assert!(left.is_empty(), "nothing of the run is left: {left:?}");
    let seat_log = fs::read_to_string(kit.logs().join("run_seat1.txt")).unwrap();
    assert!(seat_log.contains("its window was closed"), "{seat_log}");
}

/// test_setup_state.py: a game that is not set up stops the start; nothing is started.
#[test]
fn a_game_not_set_up_is_refused() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, seat) = scratch_kit(top.path());
    fs::remove_file(seat.join(".wccf-kit")).unwrap();
    let ui = Said::default();
    let rc = run::command(&kit, &params("unset", Shared::new()), &["local".into()], &ui);
    assert_eq!(rc, run::NOT_SET_UP, "{}", ui.text());
    assert!(ui.text().contains("Not set up yet"), "{}", ui.text());
    assert!(!run::is_on(&kit, Kind::Run));
}

/// test_setup_state.py's ensure_hook: a game whose wccfk.dll is not this kit's is set up once (setup.py; here there
/// is no Python, so that fails and the start stops, saying so); a missing catalogue is only a note.
#[test]
fn an_old_hook_is_set_up_first() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, seat) = scratch_kit(top.path());
    fs::write(seat.join("wccfk.dll"), b"an older kit's hook").unwrap();
    fs::remove_file(kit.cache().join("catalogue.tsv")).unwrap();
    let ui = Said::default();
    let rc = run::command(&kit, &params("hook", Shared::new()), &["local".into()], &ui);
    let said = ui.text();
    assert_eq!(rc, run::NOT_SET_UP, "{said}");
    assert!(said.contains("the card CATALOGUE is not built yet"), "{said}");
    assert!(said.contains("(wccfk.dll) is missing or not this kit's"), "{said}");
    assert!(said.contains("Setting the game up did not finish"), "{said}");
    assert!(!run::is_on(&kit, Kind::Run));
}

#[test]
fn nothing_running_says_so() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _) = scratch_kit(top.path());
    let p = params("idle", Shared::new());
    for (words, said) in [
        ("stop", "Nothing running."),
        ("status", "Nothing running."),
        ("show", "No server"),
    ] {
        let ui = Said::default();
        run::command(&kit, &p, &words.split(' ').map(String::from).collect::<Vec<_>>(), &ui);
        assert!(ui.text().contains(said), "{words}: {}", ui.text());
    }
}

/// Run by `prefer_system32_refuses_the_start` in a copy of this test program started with the policy on: the run
/// refuses before anything else (here: before it finds no kit).
#[test]
fn probe_unguarded_start() {
    if env::var_os("FORZA_PROBE_RUN").is_some() {
        let ui = Said::default();
        let kit = Kit::at(env::temp_dir().join(format!("wccf-no-kit-{}", std::process::id())));
        let rc = run::command(&kit, &params("probe", Shared::new()), &["local".into()], &ui);
        let worded = ui.text().contains("Prefer System32 images");
        std::process::exit(if rc == run::UNGUARDED && worded { 7 } else { 8 });
    }
}

/// A Launcher started with "Prefer System32 images" on (as wslhost.exe starts programs) refuses to start the game,
/// saying why.
#[test]
fn prefer_system32_refuses_the_start() {
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, PROCESS_INFORMATION, STARTUPINFOEXW,
        UpdateProcThreadAttribute, WaitForSingleObject,
    };
    // PROCESS_CREATION_MITIGATION_POLICY_IMAGE_LOAD_PREFER_SYSTEM32_ALWAYS_ON (winnt.h: 1 << 60)
    let policy: u64 = 1 << 60;
    let exe = env::current_exe().unwrap();
    let line = format!("\"{}\" probe_unguarded_start --exact --test-threads 1", exe.display());
    let mut line: Vec<u16> = std::ffi::OsStr::new(&line).encode_wide().chain([0]).collect();
    let mut block: Vec<u16> = env::vars_os()
        .chain([("FORZA_PROBE_RUN".into(), "1".into())])
        .flat_map(|(k, v)| {
            let mut kv = k;
            kv.push("=");
            kv.push(v);
            kv.encode_wide().chain([0]).collect::<Vec<u16>>()
        })
        .chain([0])
        .collect();
    // SAFETY: the attribute list lives in `list` for the whole call; every pointer outlives CreateProcessW; the
    // handles it returns are closed once.
    unsafe {
        let mut size = 0;
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        let mut list = vec![0u8; size];
        let attrs: LPPROC_THREAD_ATTRIBUTE_LIST = list.as_mut_ptr().cast();
        assert!(InitializeProcThreadAttributeList(attrs, 1, 0, &mut size) != 0);
        let set = UpdateProcThreadAttribute(
            attrs,
            0,
            PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY as usize,
            (&raw const policy).cast(),
            size_of::<u64>(),
            ptr::null_mut(),
            ptr::null(),
        );
        assert!(set != 0);
        let mut si: STARTUPINFOEXW = mem::zeroed();
        si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        si.lpAttributeList = attrs;
        let mut pi: PROCESS_INFORMATION = mem::zeroed();
        let made = CreateProcessW(
            ptr::null(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            FALSE,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            block.as_mut_ptr().cast(),
            ptr::null(),
            &si.StartupInfo,
            &mut pi,
        );
        DeleteProcThreadAttributeList(attrs);
        assert!(made != 0, "{}", std::io::Error::last_os_error());
        WaitForSingleObject(pi.hProcess, INFINITE);
        let mut code = 0;
        GetExitCodeProcess(pi.hProcess, &mut code);
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
        assert_eq!(
            code, 7,
            "the run must refuse with Prefer System32 on, and say so (8 = it did not)"
        );
    }
}

// ---- failure paths (stability first: a crash, a hang, a restart mid-start, a stand-in that cannot start) ----------

/// `words` on a thread: what the run says, and its exit code when it ends.
fn run_on_thread(kit: &Kit, p: Params, words: &[&str]) -> (Arc<Said>, thread::JoinHandle<i32>) {
    let ui = Arc::new(Said::default());
    let (u, kd) = (ui.clone(), kit.dir().to_path_buf());
    let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
    let t = thread::spawn(move || run::command(&Kit::at(kd), &p, &words, &*u));
    (ui, t)
}

/// Waits for a line of the run; panics with what it said if the run ends first.
fn wait_said(ui: &Said, t: &thread::JoinHandle<i32>, line: &str, secs: u64) {
    let t0 = Instant::now();
    while !ui.text().contains(line) {
        assert!(!t.is_finished(), "ended before {line:?}:\n{}", ui.text());
        assert!(
            t0.elapsed() < Duration::from_secs(secs),
            "no {line:?} in {secs} s:\n{}",
            ui.text()
        );
        thread::sleep(Duration::from_millis(100));
    }
}

/// After a run: its code, nothing of it left, the lock and running.json gone.
fn ended_clean(kit: &Kit, top: &Path, ui: &Said, t: thread::JoinHandle<i32>, limit: Duration) -> i32 {
    let t0 = Instant::now();
    let rc = t.join().unwrap();
    assert!(t0.elapsed() < limit, "took {:?}:\n{}", t0.elapsed(), ui.text());
    let left = running_from(top);
    assert!(left.is_empty(), "nothing of the run is left: {left:?}\n{}", ui.text());
    assert!(!run::is_on(kit, Kind::Run), "the lock went with the run");
    assert!(
        !kit.data().join("running.json").exists(),
        "running.json goes with the run"
    );
    rc
}

/// A stand-in that hangs: the test program again, no window, asleep for 10 minutes.
#[test]
#[ignore = "a stand-in started by a_seat_that_never_shows_its_window_is_given_up"]
fn hang_stand_in() {
    thread::sleep(Duration::from_secs(600));
}

/// A server's log inbox that takes one upload and answers code R042; what it got.
fn log_inbox(port: u16) -> thread::JoinHandle<Vec<u8>> {
    let l = TcpListener::bind(("127.0.0.1", port)).unwrap();
    thread::spawn(move || {
        let (conn, _) = l.accept().unwrap();
        let mut out = conn.try_clone().unwrap();
        let mut r = BufReader::new(conn);
        let mut head = String::new();
        r.read_line(&mut head).unwrap();
        let size: usize = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        let mut blob = vec![0; size];
        std::io::Read::read_exact(&mut r, &mut blob).unwrap();
        out.write_all(b"OK R042\n").unwrap();
        blob
    })
}

/// Seat 1 crashes as it starts: the start fails, says why, and stops everything it started.
#[test]
fn a_seat_that_crashes_at_the_start_stops_everything() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, seat) = scratch_kit(top.path());
    stand_in("crashme.exe", &seat.join("standin_client.exe"));
    let p = params("crash", Shared::new());
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap();
    let _desk = seat_desk(p.seat_port);
    let (ui, t) = run_on_thread(&kit, p, &["remote", "127.0.0.1"]);
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(90));
    let said = ui.text();
    assert_eq!(rc, run::FAILED, "{said}");
    assert!(
        said.contains("FAILED: seat 1's window did not come up - see data\\logs\\run_seat1.txt"),
        "{said}"
    );
    assert!(said.contains("Stopping what was started ..."), "{said}");
    let log = fs::read_to_string(kit.logs().join("run_seat1.txt")).unwrap();
    assert!(
        log.contains("SECOND-chance / FATAL"),
        "the crash is in seat 1's log: {log}"
    );
}

/// Seat 1 hangs without a window: given up after its deadline, and ended with everything else.
#[test]
fn a_seat_that_never_shows_its_window_is_given_up() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, seat) = scratch_kit(top.path());
    fs::copy(env::current_exe().unwrap(), seat.join("standin_client.exe")).unwrap();
    let mut p = params("hang", Shared::new());
    p.seat_args = Some(["hang_stand_in", "--exact", "--ignored"].map(String::from).to_vec());
    p.window_wait = Duration::from_secs(8);
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap();
    let _desk = seat_desk(p.seat_port);
    let (ui, t) = run_on_thread(&kit, p, &["remote", "127.0.0.1"]);
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(90));
    let said = ui.text();
    assert_eq!(rc, run::FAILED, "{said}");
    assert!(said.contains("FAILED: seat 1's window did not come up"), "{said}");
}

/// The projector crashes (a server without a seat desk gives seat 1 the projector): seat 1 plays on; closing it ends
/// the run, and the run that went wrong is reported to the server's log inbox by itself.
#[test]
fn a_crashed_projector_keeps_seat_1_and_is_reported() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _seat) = scratch_kit(top.path());
    let game = top.path().join(r"game\extracted");
    stand_in("crashme.exe", &game.join("standin_client.exe"));
    let p = params("proj", Shared::new());
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap(); // no seat desk: seat 1 + projector
    let inbox = log_inbox(p.inbox_port);
    let (ui, t) = run_on_thread(&kit, p, &["remote", "127.0.0.1"]);
    wait_said(
        &ui,
        &t,
        "A game window went by itself, not closed (a crash - see data\\logs\\run_projector.txt)",
        120,
    );
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(60));
    let said = ui.text();
    assert_eq!(rc, run::OK, "{said}");
    assert!(said.contains("has no seat desk - seat 1 with the projector"), "{said}");
    assert!(
        said.contains("its logs went to the server at 127.0.0.1 by themselves: code R042"),
        "{said}"
    );
    assert!(
        said.contains("it went wrong: run_projector.txt: the game window went by itself"),
        "{said}"
    );
    let blob = inbox.join().unwrap();
    assert_eq!(&blob[..2], b"PK", "a ZIP reached the inbox");
}

/// RESTART NOW written while the start is still on (the overlay's wait): taken once the run is up, seat 1's cabinet
/// alone restarts (THIS PC... here ONLINE on the same server), and the run then ends as usual.
#[test]
fn restart_now_mid_start_restarts_the_cabinet() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _seat) = scratch_kit(top.path());
    fs::write(kit.data().join("panel.txt"), b"play_on=online\nserver=127.0.0.1\n").unwrap();
    let p = params("restart", Shared::new());
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap();
    let _desk = seat_desk(p.seat_port);
    let (ui, t) = run_on_thread(&kit, p, &["remote", "127.0.0.1"]);
    wait_said(&ui, &t, "  5/6 seat 1 (the player cabinet) - window up", 120);
    fs::write(kit.data().join("restart.request"), b"RESTART NOW - a test\n").unwrap();
    wait_said(&ui, &t, "Seat 1's cabinet is back; the projector was not touched.", 120);
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(60));
    let said = ui.text();
    assert_eq!(rc, run::OK, "{said}");
    assert!(said.contains("Restarting seat 1's cabinet only"), "{said}");
    assert_eq!(said.matches("  5/6 seat 1").count(), 2, "{said}");
    assert!(said.contains("The game has ended: closed by the player."), "{said}");
    assert!(!kit.data().join("restart.request").exists(), "the request was taken");
}

/// The card reader's pipe is taken (another reader runs): the start stops, says which stand-in, and leaves nothing.
#[test]
fn a_stand_in_that_cannot_start_stops_the_start() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _seat) = scratch_kit(top.path());
    let p = params("taken", Shared::new());
    let other = forza::standins::io_board::spawn(&p.icc_pipe, Shared::new(), Arc::new(|_: &str| {})).unwrap();
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap();
    let _desk = seat_desk(p.seat_port);
    let (ui, t) = run_on_thread(&kit, p, &["remote", "127.0.0.1"]);
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(60));
    drop(other);
    let said = ui.text();
    assert_eq!(rc, run::FAILED, "{said}");
    assert!(
        said.contains("FAILED: the card reader stand-in did not start"),
        "{said}"
    );
}
