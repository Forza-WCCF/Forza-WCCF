//! launcher::run end to end, without the game: a scratch kit in %TEMP% whose game folder holds the stand-in
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

/// What the run said (as shown, and the English its logs get), and whether the game is up; `lang` None: English.
#[derive(Default)]
struct Said {
    lines: Mutex<Vec<String>>,
    logged: Mutex<Vec<String>>,
    up: AtomicBool,
    lang: Option<forza_ui::Lang>,
}

/// A `Ui` that drops every line.
fn quiet() -> run::Lines<'static> {
    run::Lines(forza_ui::Lang::En, &|_, _| {})
}

impl Ui for Said {
    fn say(&self, line: &str) {
        self.say_logged(line, line);
    }
    fn say_logged(&self, shown: &str, en: &str) {
        println!("{shown}");
        self.lines.lock().unwrap().push(shown.to_owned());
        self.logged.lock().unwrap().push(en.to_owned());
    }
    fn lang(&self) -> forza_ui::Lang {
        self.lang.unwrap_or(forza_ui::Lang::En)
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
    built_copy("2010-11/source/build", exe, to);
}

/// A file of a staged kit (`target/kit/<from>/<exe>`), copied to `to`.
fn built_copy(from: &str, exe: &str, to: &Path) {
    let built = Path::new("../../target/kit").join(from).join(exe);
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

/// A card folder from an older version, with one picture in it: a start that makes the pictures wipes it.
fn old_pictures(kit: &Kit) -> PathBuf {
    let cards = kit.cache().join("cards");
    fs::create_dir_all(&cards).unwrap();
    fs::write(cards.join("source.txt"), "1 game").unwrap();
    fs::write(cards.join("7_f.jpg"), b"old").unwrap();
    cards
}

/// The pictures are made late in a start, after every refusal: a refused or stopped start leaves them as they were.
fn assert_pictures_untouched(cards: &Path, ui: &Said) {
    assert!(cards.join("7_f.jpg").is_file(), "{}", ui.text());
    assert!(!ui.text().contains("card pictures"), "{}", ui.text());
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
        logs_dir: env::temp_dir().join(format!("wccftest_logs_{tag}_{pid}")),
        network_ports: [0; 6],
        icc_pipe: format!(r"\\.\pipe\wccftest_icc_{tag}_{pid}"),
        jvs_pipe: format!(r"\\.\pipe\wccftest_jvs_{tag}_{pid}"),
        aime_pipe: format!(r"\\.\pipe\wccftest_aime_{tag}_{pid}"),
        aimedb_port: free_port(),
        gamesvr_port: free_port(),
        scenes: Names::scratch(&format!("wccftest_{tag}_{pid}")),
        maps: None,
        hook_imports: false, // fakegame.exe imports no WINMM.dll; a_program_without_the_hook_starts_nothing checks it
        shared,
        extra_watch: Default::default(),
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

/// Online on a scratch server named `localhost:DESK-PORT`: the cabinet starts, a stop is refused while the club card is in, closing seat 1's
/// window ends the run (nothing of it left in < 30 s), and the end says the player closed it.
#[test]
fn closing_seat_1_ends_the_run_and_the_card_rule_holds() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _seat) = scratch_kit(top.path());
    let shared = Shared::new();
    shared.set_switches(0, [0, 0], [CARD_BIT, 0]); // the club card is in when the key driver starts
    let p = params("close", shared);
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap(); // "the server answers"
    // written as the lobby lists it: a name, and the seat desk on a port of its own (not p.seat_port)
    let desk_port = free_port();
    let _desk = seat_desk(desk_port);
    let ui = Arc::new(Said::default());
    let kit_dir = kit.dir().to_path_buf();
    let (u, kd) = (ui.clone(), kit_dir.clone());
    let server = format!("localhost:{desk_port}");
    let t = thread::spawn(move || run::command(&Kit::at(kd), &p, &["remote".into(), server], &*u));

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
    assert!(said.contains("  5/6 seat 1 window up"), "{said}");
    assert!(said.contains(": seat 1"), "{said}");
    assert!(
        !said.contains("has no seat desk"),
        "the claim went to the desk's own port: {said}"
    );
    assert!(run::is_on(&kit, Kind::Run), "the run's lock is held");
    // the pictures command and step wait for the run to end: nothing is rebuilt under the game
    let cards = old_pictures(&kit);
    let other = Said::default();
    assert!(!run::make_pictures(&kit, &params("pics", Shared::new()), &other));
    assert!(other.text().contains("Close its window"), "{}", other.text());
    assert_pictures_untouched(&cards, &other);
    let running = fs::read_to_string(kit.data().join("running.json")).unwrap();
    assert!(
        running.contains("\"server\":\"127.0.0.1\""),
        "the name resolved: {running}"
    );
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
    assert!(said.contains("Game closed by the player."), "{said}");
    let logged = std::fs::read_to_string(kit.logs().join("run_launcher.txt")).unwrap_or_default();
    assert!(
        logged.contains("closed by the player"),
        "the run's lines are in run_launcher.txt too:\n{logged}"
    );
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
    // in Italian: the window's line is Italian, the log's English
    let it = Said {
        lang: Some(forza_ui::Lang::It),
        ..Said::default()
    };
    run::command(&kit, &params("unset", Shared::new()), &["local".into()], &it);
    assert!(!it.text().contains("Not set up yet"), "{}", it.text());
    assert!(it.text().contains("CONFIGURA"), "{}", it.text());
    assert!(it.logged.lock().unwrap().iter().any(|l| l.contains("Not set up yet")));
}

/// forza's own setup lines (forza::Line): the window shows them in the player's language, the log in English.
#[test]
fn a_setup_line_is_shown_translated_and_logged_in_english() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _) = scratch_kit(top.path());
    let it = Said {
        lang: Some(forza_ui::Lang::It),
        ..Said::default()
    };
    let nowhere = top.path().join("no game here");
    let rc = launcher::setup::game_folder(&kit, &nowhere.to_string_lossy(), false, &it);
    assert_eq!(rc, 2, "{}", it.text());
    assert!(it.text().contains("nessun file di gioco"), "{}", it.text());
    assert!(!it.text().contains("no WCCF"), "{}", it.text());
    assert!(
        it.logged
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("no WCCF 2010-11 game files in"))
    );
}

/// test_setup_state.py's ensure_hook: a game whose wccfk.dll is not this kit's is set up once (launcher::setup; the
/// stand-ins are no game it can set up, so that fails and the start stops, saying so); a missing catalogue is only a note.
#[test]
fn an_old_hook_is_set_up_first() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, seat) = scratch_kit(top.path());
    let cards = old_pictures(&kit);
    fs::write(seat.join("wccfk.dll"), b"an older kit's hook").unwrap();
    fs::remove_file(kit.cache().join("catalogue.tsv")).unwrap();
    let ui = Said::default();
    let rc = run::command(&kit, &params("hook", Shared::new()), &["local".into()], &ui);
    let said = ui.text();
    assert_eq!(rc, run::NOT_SET_UP, "{said}");
    assert!(said.contains("Card catalogue not built"), "{said}");
    assert!(said.contains("Updating the game's helper file (wccfk.dll)"), "{said}");
    assert!(said.contains("Setup did not finish"), "{said}");
    assert!(!run::is_on(&kit, Kind::Run));
    assert_pictures_untouched(&cards, &ui); // the repair comes first, and failed
}

/// The 2010-11 guard: with the kit's own wccfk.dll everywhere, a program that does not load it (fakegame.exe has no
/// WINMM.dll import to rename) is no reason to trust the hook copies: setup runs, and nothing starts.
#[test]
fn a_program_without_the_hook_starts_nothing() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _) = scratch_kit(top.path());
    let ui = Said::default();
    let mut p = params("hookimp", Shared::new());
    p.hook_imports = true;
    let rc = run::command(&kit, &p, &["local".into()], &ui);
    let said = ui.text();
    assert_eq!(rc, run::NOT_SET_UP, "{said}");
    assert!(said.contains("Updating the game's helper file (wccfk.dll)"), "{said}");
    assert!(!said.contains("1/6"), "{said}");
    assert!(!run::is_on(&kit, Kind::Run));
}

/// One admission per kit folder: while another thread (another Launcher window) holds it, a start and setup are refused
/// and leave nothing on; once it is free, setup gets past it (and fails on the stand-ins, as no game).
#[test]
fn a_start_and_setup_wait_their_turn() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _) = scratch_kit(top.path());
    let cards = old_pictures(&kit);
    let (held, free) = (std::sync::Barrier::new(2), std::sync::Barrier::new(2));
    thread::scope(|s| {
        s.spawn(|| {
            let admitted = run::admit(&kit).unwrap();
            held.wait();
            free.wait();
            drop(admitted);
        });
        held.wait();
        let ui = Said::default();
        let rc = run::command(&kit, &params("admit", Shared::new()), &["local".into()], &ui);
        let on = run::is_on(&kit, Kind::Run);
        let setup = launcher::setup::game_folder(&kit, "", true, &quiet());
        free.wait(); // before the assertions: a failed one must not leave the thread waiting
        assert_eq!(rc, run::REFUSED, "{}", ui.text());
        assert!(ui.text().contains("another Launcher window"), "{}", ui.text());
        assert_pictures_untouched(&cards, &ui);
        assert!(!on);
        assert_eq!(setup, launcher::setup::RUNNING);
    });
    assert_ne!(
        launcher::setup::game_folder(&kit, "", true, &quiet()),
        launcher::setup::RUNNING
    );
}

/// The pictures command and step (`make_pictures`) take SET UP's admission: refused beside another holder (a run that
/// is on: see closing_seat_1_ends_the_run_and_the_card_rule_holds); the folder stays as it was.
#[test]
fn the_pictures_wait_their_turn() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _) = scratch_kit(top.path());
    let cards = old_pictures(&kit);
    let p = params("pics", Shared::new());
    let ui = Said::default();
    {
        let _held = run::admit(&kit).unwrap();
        // the same thread may admit again (a start's own step): another thread is the other window
        thread::scope(|s| {
            s.spawn(|| assert!(!run::make_pictures(&kit, &p, &ui)));
        });
    }
    assert!(ui.text().contains("Close its window"), "{}", ui.text());
    assert_pictures_untouched(&cards, &ui);
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
    assert!(said.contains("Stopping."), "{said}");
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
/// the run with WENT_WRONG, and nothing is uploaded anywhere.
#[test]
fn a_crashed_projector_keeps_seat_1_and_is_reported() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _seat) = scratch_kit(top.path());
    let game = top.path().join(r"game\extracted");
    stand_in("crashme.exe", &game.join("standin_client.exe"));
    let p = params("proj", Shared::new());
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap(); // no seat desk: seat 1 + projector
    let (ui, t) = run_on_thread(&kit, p, &["remote", "127.0.0.1"]);
    wait_said(
        &ui,
        &t,
        "A game window crashed (see data\\logs\\run_projector.txt)",
        120,
    );
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(60));
    let said = ui.text();
    assert_eq!(rc, run::WENT_WRONG, "{said}");
    assert!(said.contains("has no seat desk: seat 1, projector."), "{said}");
    assert!(!said.contains("Logs sent"), "{said}");
    assert!(
        said.contains("Game ended with a problem: run_projector.txt: the game window went by itself"),
        "{said}"
    );
}

/// SAVE LOGS from the panel: data\savelogs.request taken by the run, the ZIP written to `logs_dir`, and
/// data\savelogs.result answers "ok PATH" for the panel.
#[test]
fn save_logs_request_answers_the_saved_path() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _seat) = scratch_kit(top.path());
    let mut p = params("savelogs", Shared::new());
    p.logs_dir = top.path().join("Downloads");
    let dir = p.logs_dir.clone();
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap();
    let _desk = seat_desk(p.seat_port);
    let (ui, t) = run_on_thread(&kit, p, &["remote", "127.0.0.1"]);
    wait_said(&ui, &t, "  5/6 seat 1 window up", 120);
    fs::write(kit.data().join("savelogs.request"), b"save\n").unwrap();
    wait_said(&ui, &t, "Logs saved: ", 30);
    let answer = fs::read_to_string(kit.data().join("savelogs.result")).unwrap();
    let path = Path::new(answer.trim().strip_prefix("ok ").expect(&answer));
    assert_eq!(path.parent(), Some(dir.as_path()), "{answer}");
    assert_eq!(&fs::read(path).unwrap()[..2], b"PK", "{answer}");
    assert!(!kit.data().join("savelogs.request").exists(), "the request was taken");
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(60));
    assert_eq!(rc, run::OK, "{}", ui.text());
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
    wait_said(&ui, &t, "  5/6 seat 1 window up", 120);
    fs::write(kit.data().join("restart.request"), b"RESTART NOW - a test\n").unwrap();
    wait_said(&ui, &t, "Seat 1 is back.", 120);
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(60));
    let said = ui.text();
    assert_eq!(rc, run::OK, "{said}");
    assert!(said.contains("Restarting seat 1 only"), "{said}");
    assert_eq!(said.matches("  5/6 seat 1").count(), 2, "{said}");
    assert!(said.contains("Game closed by the player."), "{said}");
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
    assert!(said.contains("FAILED: the card reader did not start"), "{said}");
    // the reason outlives the window: a failed start once left no trace in the log (run 5a, 2026-10-16)
    let logged = std::fs::read_to_string(kit.logs().join("run_launcher.txt")).unwrap_or_default();
    assert!(logged.contains("FAILED: the card reader did not start"), "{logged}");
}

/// A stand-in thread that panics: the supervisor says it in the run log; an essential one stops the run (nothing left,
/// exit 1), one the game can do without leaves the run going until the player closes seat 1.
fn run_with_panicking_stand_in(tag: &str, essential: bool) -> (String, i32) {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _seat) = scratch_kit(top.path());
    let p = params(tag, Shared::new());
    let thread = thread::spawn(|| panic!("boom"));
    let handle = forza::standins::Handle::new(Arc::new(AtomicBool::new(false)), thread);
    p.extra_watch
        .lock()
        .unwrap()
        .push(("the test stand-in".into(), essential, handle));
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap();
    let _desk = seat_desk(p.seat_port);
    let (ui, t) = run_on_thread(&kit, p, &["remote", "127.0.0.1"]);
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(120));
    (ui.text(), rc)
}

#[test]
fn an_essential_stand_in_that_panics_stops_the_run() {
    let (said, rc) = run_with_panicking_stand_in("panic1", true);
    assert_eq!(rc, run::FAILED, "{said}");
    assert!(
        said.contains(
            "the test stand-in stopped (its thread panicked: boom). The game cannot continue without it - stopping."
        ),
        "{said}"
    );
    assert!(said.contains("Stopped: the test stand-in ended."), "{said}");
}

#[test]
fn a_stand_in_the_game_can_do_without_is_said_and_the_run_goes_on() {
    let (said, rc) = run_with_panicking_stand_in("panic2", false);
    assert_eq!(rc, run::OK, "{said}");
    assert!(
        said.contains("the test stand-in stopped (its thread panicked: boom). The game continues without it."),
        "{said}"
    );
    assert!(said.contains("Game closed by the player."), "{said}");
}

/// `server` mode: the control program is a stand-in (the test's own listener holds the control port), `forza-server all`
/// from the kit build is the seat desk and relay; it answers a WANT on the test's port, and stop ends the run
/// with nothing left.
#[test]
fn server_mode_runs_forza_server_and_stop_ends_it() {
    let top = tempfile::Builder::new().prefix("wccf-run-").tempdir().unwrap();
    let (kit, _seat) = scratch_kit(top.path());
    let built = Path::new("../../target/kit/2010-11/programs/forza-server.exe");
    assert!(
        built.is_file(),
        "{} is missing: run cargo xtask build first",
        built.display()
    );
    fs::copy(built, kit.programs().join("forza-server.exe")).unwrap();
    let game = top.path().join(r"game\extracted");
    stand_in("fakegame.exe", &game.join("standin_control.exe"));
    fs::write(game.join(r"local\ctrl_user_option.conf"), b"WAIT_PROJECTOR=1\r\n").unwrap();
    let mut p = params("server", Shared::new());
    p.control_args = ["version.dll", "wait:120000"].map(String::from).to_vec();
    p.match_engines = 0;
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap();
    let seat_port = p.seat_port;
    let (ui, t) = run_on_thread(&kit, p, &["server"]);
    wait_said(&ui, &t, "Server running.", 90);

    let mut conn = std::net::TcpStream::connect(("127.0.0.1", seat_port)).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    conn.write_all(b"WANT\n").unwrap();
    let mut reply = String::new();
    BufReader::new(conn).read_line(&mut reply).unwrap();
    assert!(reply.starts_with("SEAT 1 PROJECTOR"), "{reply:?}\n{}", ui.text());
    let desk_log = fs::read_to_string(kit.logs().join("seat_broker.txt")).unwrap();
    assert!(desk_log.contains("listening on TCP"), "{desk_log}");

    let (code, answer) = run::request(&kit, Kind::Server, "stop").expect("a server run is on");
    assert_eq!(code, run::OK, "{answer}\n{}", ui.text());
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(60));
    assert_eq!(rc, run::OK, "{}", ui.text());
}

// ---- 2017-18 ---------------------------------------------------------------------------------------------------

/// fakegame_iphlp.exe with its IPHLPAPI.DLL import renamed to wccfnet.dll, as the 2017-18 setup makes the game's guarded
/// copies (the same bytes: the name and one more NUL in its place).
fn guarded_copy(to: &Path) {
    stand_in("fakegame_iphlp.exe", to);
    let mut exe = fs::read(to).unwrap();
    let old = b"iphlpapi.dll\0";
    let at: Vec<usize> = exe
        .windows(old.len())
        .enumerate()
        .filter(|(_, w)| w.eq_ignore_ascii_case(old))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 1, "one import name");
    exe[at[0]..at[0] + old.len()].copy_from_slice(b"wccfnet.dll\0\0");
    fs::write(to, exe).unwrap();
}

/// The import check on built programs: the stand-in as built imports IPHLPAPI.DLL, the plain fakegame imports no
/// guard, and only the renamed copy passes.
#[test]
fn the_guard_reads_the_import_table() {
    let dir = tempfile::tempdir().unwrap();
    let (plain, iphlp, guarded) = (
        dir.path().join("plain.exe"),
        dir.path().join("iphlp.exe"),
        dir.path().join("guarded.exe"),
    );
    stand_in("fakegame.exe", &plain);
    stand_in("fakegame_iphlp.exe", &iphlp);
    guarded_copy(&guarded);
    let check = |p: &Path| launcher::guard::imports(&fs::read(p).unwrap());
    let why = check(&iphlp).unwrap_err();
    assert!(why.contains("still imports IPHLPAPI.DLL"), "{why}");
    let why = check(&plain).unwrap_err();
    assert!(why.contains("does not import wccfnet.dll"), "{why}");
    check(&guarded).unwrap();
}

fn sha(path: &Path) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(fs::read(path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A 2017-18 runtime as setup leaves it, of stand-ins: `top\kit` (overlay\, data\; no settings - the
/// game is found beside the kit), `top\game`, `top\seat1` and `top\setup-hashes.json`.
fn scratch_kit_1718(top: &Path) -> Kit {
    let (kit, game, seat) = (top.join("kit"), top.join("game"), top.join("seat1"));
    for d in [kit.join("overlay"), kit.join("data"), seat.clone()] {
        fs::create_dir_all(d).unwrap();
    }
    guarded_copy(&game.join("standin_client.exe"));
    guarded_copy(&game.join("standin_control.exe")); // checked, never run: the tests play on a "server"
    guarded_copy(&seat.join("standin_client.exe"));
    for dir in [&game, &seat] {
        fs::write(dir.join("standin_match.exe"), b"never run").unwrap();
        fs::write(dir.join("dmath.dll"), b"the det-math stand-in").unwrap();
    }
    built_copy("2017-18/bin", "wccfnet.dll", &game.join("wccfnet.dll"));
    built_copy("2017-18/bin", "wccfnet.dll", &seat.join("wccfnet.dll"));
    // no panel (overlay\forza_panel.dll): the run says so and plays on
    for dir in [&game, &seat] {
        fs::create_dir_all(dir.join("local")).unwrap();
        fs::write(
            dir.join(r"local\client_user_option.conf"),
            b"CONTROL_IP=127.0.0.1\r\nMATCH_IP=127.0.0.1\r\nSATELLITE_NO=1\r\n",
        )
        .unwrap();
    }
    let hashes = serde_json::json!({
        "standin_client.exe": {"original": "-", "guarded": sha(&game.join("standin_client.exe"))},
        "standin_control.exe": {"original": "-", "guarded": sha(&game.join("standin_control.exe"))},
        "wccfnet.dll": sha(&game.join("wccfnet.dll")),
        "seat1/standin_client.exe": sha(&seat.join("standin_client.exe")),
        "standin_match.exe": {"original": "-", "guarded": sha(&game.join("standin_match.exe"))},
        "dmath.dll": sha(&game.join("dmath.dll")),
    });
    fs::write(top.join("setup-hashes.json"), hashes.to_string()).unwrap();
    let kit = Kit::at(kit);
    assert_eq!(kit.edition(), launcher::Edition::E1718);
    kit
}

/// A changed guarded program, a record that is gone, or a program that imports IPHLPAPI.DLL: the start is refused
/// before anything of the run exists - no lock, no logs, nothing running.
#[test]
fn a_2017_18_kit_that_is_not_guarded_starts_nothing() {
    let top = tempfile::Builder::new().prefix("wccf-1718-").tempdir().unwrap();
    let kit = scratch_kit_1718(top.path());
    let (game, seat) = (top.path().join("game"), top.path().join("seat1"));
    let refused = |what: &str, part: &str| {
        let ui = Said::default();
        let rc = run::command(&kit, &params("unguarded", Shared::new()), &["local".into()], &ui);
        let said = ui.text();
        assert_eq!(rc, run::NOT_GUARDED, "{what}: {said}");
        assert!(
            said.contains("the network guard is not in place") && said.contains(part),
            "{what}: {said}"
        );
        assert!(!run::is_on(&kit, Kind::Run));
        assert!(!kit.logs().exists(), "{what}: no logs");
        assert!(running_from(top.path()).is_empty());
    };
    fs::write(seat.join("standin_client.exe"), b"not the guarded copy").unwrap();
    refused("a wrong hash", "is not the file Setup made");
    guarded_copy(&seat.join("standin_client.exe"));

    fs::write(game.join("wccfnet.dll"), b"no guard").unwrap();
    refused("a changed wccfnet.dll", "wccfnet.dll is not the file Setup made");
    built_copy("2017-18/bin", "wccfnet.dll", &game.join("wccfnet.dll"));

    // a runtime set up before 0.6.0: its programs still load ICMP.DLL, so it is checked while the record names it
    let record_at = top.path().join("setup-hashes.json");
    let fresh = fs::read_to_string(&record_at).unwrap();
    let mut old: serde_json::Value = serde_json::from_str(&fresh).unwrap();
    for dir in [&game, &seat] {
        fs::write(dir.join("ICMP.DLL"), b"the old stand-in").unwrap();
    }
    old["ICMP.DLL"] = sha(&game.join("ICMP.DLL")).into();
    fs::write(&record_at, old.to_string()).unwrap();
    fs::write(seat.join("ICMP.DLL"), b"pings for real").unwrap();
    refused(
        "a changed ICMP.DLL in an old runtime",
        "ICMP.DLL is not the file Setup made",
    );
    fs::write(&record_at, fresh).unwrap(); // a new record does not name it: nothing loads it

    fs::write(seat.join("dmath.dll"), b"another det-math").unwrap();
    refused("a changed dmath.dll in seat 1", "dmath.dll is not the file Setup made");
    fs::write(seat.join("dmath.dll"), b"the det-math stand-in").unwrap();
    fs::write(game.join("standin_match.exe"), b"another engine").unwrap();
    refused("a changed match engine", "standin_match.exe is not the file Setup made");
    fs::write(game.join("standin_match.exe"), b"never run").unwrap();

    stand_in("fakegame_iphlp.exe", &game.join("standin_control.exe"));
    let mut record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(top.path().join("setup-hashes.json")).unwrap()).unwrap();
    record["standin_control.exe"]["guarded"] = sha(&game.join("standin_control.exe")).into();
    fs::write(top.path().join("setup-hashes.json"), record.to_string()).unwrap();
    refused("an unguarded control recorded as guarded", "still imports IPHLPAPI.DLL");

    fs::remove_file(top.path().join("setup-hashes.json")).unwrap();
    refused("no record", "setup-hashes.json cannot be read");
}

/// 2017-18 on a scratch server: the guard passes, the Aime reader, AiMeDB, the network and I/O board stand-ins come
/// up, seat 1 starts; closing its window ends the run with nothing left.
#[test]
fn closing_seat_1_ends_the_2017_18_run() {
    let top = tempfile::Builder::new().prefix("wccf-1718-").tempdir().unwrap();
    let kit = scratch_kit_1718(top.path());
    let p = params("close1718", Shared::new());
    let _control = TcpListener::bind(("127.0.0.1", p.control_port)).unwrap(); // "the server answers"
    let _desk = seat_desk(p.seat_port);
    // no seat in a shared shop with data\settings.json "shop_1718": false; on when the key is missing
    fs::write(kit.settings_file(), r#"{"shop_1718": false}"#).unwrap();
    let ui = Said::default();
    let rc = run::command(&kit, &p, &["remote".into(), "127.0.0.1".into()], &ui);
    assert_eq!(rc, run::REFUSED, "{}", ui.text());
    assert!(
        ui.text().contains("The shared arcade is off on this PC"),
        "{}",
        ui.text()
    );
    assert!(!kit.logs().exists(), "refused before anything of the run exists");
    fs::write(kit.settings_file(), "{}").unwrap();
    let aimedb = p.aimedb_port;
    let (ui, t) = run_on_thread(&kit, p, &["remote", "127.0.0.1"]);
    wait_said(&ui, &t, "  6/6 ", 120);
    let said = ui.text();
    assert!(said.contains("Starting WCCF 2017-18 from"), "{said}");
    assert!(
        said.contains("cabinet (card file data\\save\\seat1_aime.txt)"),
        "{said}"
    );
    assert!(
        said.contains("6/6 panels did not load (forza_panel.dll is missing)"),
        "no panel in the scratch kit, and the run says so: {said}"
    );
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", aimedb)).is_ok(),
        "AiMeDB listens"
    );
    let rc = ended_clean(&kit, top.path(), &ui, t, Duration::from_secs(60));
    let said = ui.text();
    assert_eq!(rc, run::OK, "{said}");
    assert!(said.contains("Game closed by the player."), "{said}");
    let aime = fs::read_to_string(kit.save().join("seat1_aime.txt")).unwrap();
    assert!(aime.trim().len() >= 20, "the Aime card was made: {aime}");
}
