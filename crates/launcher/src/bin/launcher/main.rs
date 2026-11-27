//! Launcher.exe - the kit's one program, a window (2026-10-10; before, a console that ran play.py, and SETUP.exe beside
//! it for the rest).  Nothing it starts shows a console: the game's two windows are all a player sees.  The window is
//! eframe drawing forza's launcher screen (forza_ui::launcher, step 9.4): this file fills its state from the kit and
//! does what the player asks for there.
//!   PLAY         play on THIS PC or ONLINE on a server, the window's language, the game's text (English,
//!                Italian or Japanese) - then this process runs the game (launcher::run) and holds every process of it
//!                in one Job until it ends.  Once the game is up this window is minimized; closing a game window ends
//!                the run, and then Launcher.exe ends too.  Closing this window (or STOP THE GAME) asks the run to stop
//!                by the card rule: with the club card in or a match on, it asks first.  Launcher.exe opened again
//!                while the game runs offers STOP THE GAME, which asks the running Launcher.exe.
//!   SETUP        the game folder: set up / repair (launcher::setup) and undo; it is typed, picked, dropped on
//!                the window or on Launcher.exe, and remembered by setup in data\settings.json.  The card pictures
//!                (the game's or the high-quality pack) and, for 2017-18, skip the tutorial.
//!   HOST ARCADE    (2017-18, unless "shop_1718": false) this PC's arcade for players elsewhere: the server, its tunnel and its room
//!                in the lobby (the code shown here), then this PC's own seat; the arcade closes once that seat's run has ended and every guest has left.
//!   UPDATE       (update.rs, 2010-11 only): the kit's releases and test builds at UPDATE_URL.
//! The choices go where the kit already keeps them (choices.rs): data\panel.txt, which the in-game SETTINGS panel
//! writes too, and data\settings.json.  Setup's lines fill OUTPUT.
//!
//! The command line still works: "Launcher.exe GAME-FOLDER" / "Launcher.exe undo" (setup at once), and play.py's words
//! (local, server, remote ADDRESS [seat N], debug, stop, status, show, restart): from a console they print there and a
//! start holds the console until the run ends (Ctrl+C asks it to stop, by the card rule); from elsewhere (a shortcut)
//! they run in this window.

#![windows_subsystem = "windows"]

mod link;
mod online;
mod pack;
mod text;
mod update;

use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::os::windows::io::IntoRawHandle;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use std::{env, mem, ptr, thread};

use eframe::egui;
use egui::ViewportCommand;
use forza::online::club::{self, Left, NotReplaced};
use forza_ui::launcher::{self as screen, Action, ClubRow, Confirm, LauncherState, Tab};
use forza_ui::lobby::LobbyAction;
use forza_ui::theme;
use forza_ui::widgets::Tone;
use forza_ui::{Lang, text as ui_text};
use launcher::release::{check_due, files_differ, is_newer, keep_notes, kept_notes, parse};
use launcher::run::{self, Params};
use launcher::{Edition, Kit, choices};
use text::{t, tf};
use update::Release;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, GetConsoleMode,
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetStdHandle,
};
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, GetCurrentProcess, GetCurrentThreadId, SetPriorityClass,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, FindWindowW, HCBT_ACTIVATE, IDNO, IDYES, IsIconic, MB_ICONERROR, MB_ICONQUESTION, MB_OK,
    MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW, SW_RESTORE, SetDlgItemTextW, SetForegroundWindow,
    SetWindowsHookExW, ShowWindow, UnhookWindowsHookEx, WH_CBT,
};
use windows_sys::core::BOOL;

/// The message boxes' title (the window's own title bar draws the name itself).
const TITLE: &str = "Forza-WCCF";

enum Step {
    /// Setup of the game folder given (empty: the recorded one), or its undo (launcher::setup)
    Setup {
        folder: OsString,
        undo: bool,
    },
    /// The game's text: "en", "it" or "" for the original Japanese (launcher::setup::game_text)
    Text(&'static str),
    Update(Release),
    /// The run (launcher::run) with play.py's words; its exit code is the result
    Game(Vec<OsString>),
    /// The card pictures (launcher::run::make_pictures), after setup and when the pictures choice changes
    Pictures,
    /// DOWNLOAD the high-quality card pictures (pack::PACK), then chosen in data\settings.json for Step::Pictures
    Pack,
    /// HOST ARCADE (2017-18): the server with its arcade room, then this PC's seat on it; the server ends after the seat, once
    /// no guest is connected. `true`: HOST LAN ARCADE, the server for this network only (`server lan`), no room
    Host(bool),
}

/// What the worker threads tell the window; each change wakes it (`poke`).
struct Shared {
    releases: Vec<Release>,
    listed: bool, // `releases` is new: the list gets it
    status: Option<(Tone, String)>,
    update_note: Option<(Tone, String)>,
    available: Option<String>, // the tag of a newer release: the notice stays until UPDATE is opened or done
    progress: u32,             // of 1000
    steps: Option<(u32, u32)>, // the run's "N/M" step lines: N of M
    stage_secs: [f32; 6],      // the seconds a start's steps take on this PC: the bar's pace
    timed: [f32; 6],           // the start under way's steps, timed as their lines arrive
    began: Option<Instant>,    // the job's start
    stage_at: Option<Instant>, // the last step line
    log: Vec<String>,          // lines for OUTPUT, not shown yet
    busy: bool,                // a job runs
    updated: bool,             // an update has just finished: setup runs again
    failed: bool,              // the last job failed: OUTPUT opens, so its output shows
    setup_failed: bool,        // its Step::Setup failed: a "game" written before it stays as it was
    game: Option<u8>,          // a Game step has just ended with this exit code
    report_open: Option<bool>, // open the report dialog: Some(after a crash)
    report_done: Option<Result<(String, String), String>>, // where the report was saved and its issue link, or why not
    // HOST ARCADE: the arcade room's code, its lobby server and the flag set when that lobby loses it
    host_room: Option<(String, String, Arc<AtomicBool>)>,
    // HOST ARCADE: the guests connected through its tunnel now (vslink), for the host's "Players here" count
    host_guests: Option<Arc<AtomicUsize>>,
    // the LAN fold: the arcades the last search found (forza::online::lan)
    lan_found: Option<Vec<forza::online::lan::Found>>,
    // why the job's step failed or its start was refused (run::Ui::cause): the status line names it
    cause: Option<String>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    releases: Vec::new(),
    listed: false,
    status: None,
    update_note: None,
    available: None,
    progress: 0,
    steps: None,
    stage_secs: forza_ui::widgets::START_SECS,
    timed: [0.0; 6],
    began: None,
    stage_at: None,
    log: Vec::new(),
    busy: false,
    updated: false,
    failed: false,
    setup_failed: false,
    game: None,
    report_open: None,
    report_done: None,
    host_room: None,
    host_guests: None,
    lan_found: None,
    cause: None,
});

/// The game is up in this process (its run is supervised here): the window is minimized, closing it asks the run to stop.
static UP: AtomicBool = AtomicBool::new(false);
/// The game came up during the current job: when the run then ends well, Launcher.exe ends with it if the run was a
/// game start from the command line (a shortcut), and the window stays on its screen if the player started it there.
static WAS_UP: AtomicBool = AtomicBool::new(false);
/// A run is on in this kit folder (here or in another Launcher.exe): PLAY offers STOP THE GAME.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// The window was minimized for the game that is up (once per run, so the player can bring it back).
static MINIMIZED: AtomicBool = AtomicBool::new(false);
/// The window's context, so the workers can wake it.
static CTX: OnceLock<egui::Context> = OnceLock::new();

fn shared() -> MutexGuard<'static, Shared> {
    SHARED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The window looks again (also while it is minimized: eframe then runs only `App::logic`).
fn poke() {
    if let Some(c) = CTX.get() {
        c.request_repaint();
    }
}

fn say_tone(tone: Tone, status: impl Into<String>, progress: u32) {
    {
        let mut s = shared();
        s.status = Some((tone, status.into()));
        s.progress = progress;
    }
    poke();
}

fn say(status: impl Into<String>, progress: u32) {
    say_tone(Tone::Notice, status, progress);
}

fn refuse(status: impl Into<String>) {
    say_tone(Tone::Refused, status, 0);
}

fn log(line: &str) {
    log_pair(line, line);
}

/// A line into DETAILS as shown, and in English into data\logs\launcher.txt, which SAVE LOGS takes (support reads it):
/// setup and the card pictures happen before any run, whose logs were the only ones kept (a tester's 0.2.0 ZIP held
/// nothing but version.txt).
fn log_pair(shown: &str, en: &str) {
    shared().log.push(shown.to_owned());
    launcher::logs::append(&Kit::here().logs().join("launcher.txt"), en);
    poke();
}

// ---- setup and the game's text ---------------------------------------------------------------------------------

/// A step line moves the bar: never back.
fn stepped(line: &str) {
    if let Some((n, m)) = step(line) {
        let mut s = shared();
        s.progress = s.progress.max(n * 1000 / m);
        let before = s.steps;
        s.steps = Some(s.steps.filter(|&(d, t)| t == m && d >= n).unwrap_or((n, m)));
        if s.steps != before {
            // a local start's six steps are timed as their lines arrive: the bar of the next start creeps at this pace
            let took = s.stage_at.map_or(0.0, |a| a.elapsed().as_secs_f32());
            if m == 6 && took > 0.0 {
                s.timed[n.saturating_sub(1) as usize] = took;
                // all six, or nothing: a start that ended early leaves the last measured pace alone
                if s.timed.iter().all(|t| *t > 0.0) {
                    s.stage_secs = s.timed;
                    let _ = launcher::steps::write(&Kit::here(), s.timed);
                }
            }
            s.stage_at = Some(Instant::now());
            // a game start (6 steps): name what the player waits for
            if let Some(key) = (m == 6)
                .then(|| STAGES.get(s.steps.map_or(0, |(d, _)| d as usize)))
                .flatten()
            {
                s.status = Some((Tone::Notice, t(key)));
            }
        }
    }
}

/// The status line after step N of a game start (index N; 0 and 6 keep the line they have).
const STAGES: [&str; 6] = [
    "launcher.msg_starting",
    "launcher.stage_server",
    "launcher.stage_window",
    "launcher.stage_window",
    "launcher.stage_window",
    "launcher.stage_screen",
];

/// A setup or text step's lines into OUTPUT; Ok when it is done, else the status that says why.
fn setup_step(rc: i32) -> Result<(), String> {
    match rc {
        0 => Ok(()),
        launcher::setup::RUNNING => Err(t("launcher.msg_game_running")),
        _ => Err(failed_status("launcher.msg_failed", "launcher.msg_failed_why")),
    }
}

/// A failed job's status line: `why_key` with the cause the job's lines gave (`Shared::cause`), else `plain`.
fn failed_status(plain: &str, why_key: &str) -> String {
    with_cause(plain, why_key, shared().cause.as_deref())
}

fn with_cause(plain: &str, why_key: &str, cause: Option<&str>) -> String {
    match cause.map(str::trim).filter(|c| !c.is_empty()) {
        Some(why) => tf(why_key, &[("why", why)]),
        None => t(plain),
    }
}

/// "  4/6 cabinet (...)" -> 4 of 6.
fn step(line: &str) -> Option<(u32, u32)> {
    let (n, rest) = line.trim_start().split_once('/')?;
    let m: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let (n, m) = (n.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (n <= m && m > 0 && rest[m.to_string().len()..].starts_with(' ')).then_some((n, m))
}

/// The game's text follows the launcher language: "en", "it" or "" (the original Japanese). Hidden override, not in
/// the UI: env WCCF_GAME_TEXT or settings.json "game_text" = en | it | original.
fn game_text_for(lang: Lang, over: Option<&str>) -> &'static str {
    match over.map(str::to_ascii_lowercase).as_deref() {
        Some("en") => "en",
        Some("it") => "it",
        Some("original" | "ja") => "",
        _ => match lang {
            Lang::En => "en",
            Lang::It => "it",
            Lang::Ja => "",
        },
    }
}

/// The status line after a language switch: 2017-18's game text follows it only at the next PLAY.
fn language_note(edition: Edition, wanted: &str, now: &str) -> Option<String> {
    (edition == Edition::E1718 && wanted != now).then(|| t("launcher.msg_text_next_play"))
}

fn game_text(kit: &Kit) -> &'static str {
    let over = std::env::var("WCCF_GAME_TEXT")
        .ok()
        .or_else(|| choices::setting(&kit.settings_file(), "game_text"));
    game_text_for(text::lang(), over.as_deref())
}

/// Set in the environment of the Launcher.exe an update started: it sets up again before anything else.
const AFTER_UPDATE: &str = "WCCF_LAUNCHER_AFTER_UPDATE";

/// After an update: the setup again, the game text in the Launcher language, the card pictures.
fn after_update_steps(kit: &Kit, folder: &str) -> Vec<Step> {
    steps_after_update(kit.edition(), folder, game_text(kit))
}

fn steps_after_update(edition: Edition, folder: &str, lang: &'static str) -> Vec<Step> {
    let mut steps = vec![Step::Setup {
        folder: folder.into(),
        undo: false,
    }];
    // 2017-18: the game's text is rebuilt at the next start (refresh_old_text)
    if !lang.is_empty() && edition == Edition::E1011 {
        steps.push(Step::Text(lang));
    }
    steps.push(Step::Pictures); // last: a failure here must not skip the game's text
    steps
}

fn window_title(kit: &Kit) -> String {
    format!("Launcher - {TITLE} {}", kit.edition().name())
}

/// Another Launcher window of this edition to the front (a second start ends after it); not while a game runs: the
/// window stays minimized behind it.
// shortcut: found by title, so two installs of one edition both open bring the first; key it by kit path if needed
fn bring_to_front(kit: &Kit) {
    if run::is_on(kit, run::Kind::Run) {
        return;
    }
    let title: Vec<u16> = window_title(kit).encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-terminated title; the window handle is only passed back to Windows.
    unsafe {
        let w = FindWindowW(std::ptr::null(), title.as_ptr());
        if !w.is_null() {
            if IsIconic(w) != 0 {
                ShowWindow(w, SW_RESTORE);
            }
            SetForegroundWindow(w);
        }
    }
}

/// Starts the Launcher.exe the update put in place; false when it did not start.
fn restart_after_update(kit: &Kit) -> bool {
    std::process::Command::new(kit.dir().join(Kit::exe_name()))
        .current_dir(kit.dir())
        .env(AFTER_UPDATE, "1")
        .spawn()
        .is_ok()
}

fn panel_file(kit: &Kit) -> PathBuf {
    kit.data().join("panel.txt")
}

fn this_kit(kit: &Kit) -> String {
    kit.version().unwrap_or_else(|| t("launcher.msg_not_known"))
}

/// The steps one by one in a worker; the first that fails ends the job.  False when another job is on: nothing started.
fn start(steps: Vec<Step>) -> bool {
    {
        let mut s = shared();
        if s.busy {
            return false;
        }
        WAS_UP.store(false, Ordering::Relaxed);
        s.busy = true;
        s.progress = 0;
        // a game start shows the weighted bar and the clock from the first second
        s.steps = steps
            .iter()
            .any(|x| matches!(x, Step::Game(_) | Step::Host(_)))
            .then_some((0, 6));
        s.host_room = None;
        s.host_guests = None;
        s.began = Some(Instant::now());
        s.stage_at = s.began;
        s.timed = [0.0; 6];
        s.cause = None;
    }
    poke();
    thread::spawn(move || {
        let kit = Kit::here();
        let mut result = Ok(());
        let mut updated = None;
        let mut setup_failed = false;
        let mut game = None;
        for step in steps {
            result = match &step {
                Step::Setup { folder, undo } => {
                    let folder = folder.to_string_lossy();
                    log(&format!("> {} {folder}", if *undo { "undo" } else { "setup" }));
                    say(
                        t(if *undo {
                            "launcher.msg_undoing"
                        } else {
                            "launcher.msg_setting_up"
                        }),
                        0,
                    );
                    setup_step(launcher::setup::game_folder(&kit, &folder, *undo, &WindowUi))
                }
                Step::Text(lang) => {
                    log(&format!("> game text {lang}"));
                    say(t("launcher.msg_switching_text"), 0);
                    setup_step(launcher::setup::game_text(&kit, lang, &WindowUi))
                }
                Step::Update(r) => update::apply(&kit, r).map(|()| updated = Some(r.tag.clone())),
                Step::Pack => pack::PACK
                    .fetch(&kit.data().join("downloads"), |d| fs4::available_space(d).ok())
                    .and_then(|zip| {
                        let file = kit.settings_file();
                        choices::set_setting(&file, "card_pack", &zip.display().to_string())
                            .and_then(|()| choices::set_setting(&file, "card_pictures", "hq"))
                            .map_err(|e| write_failed(&file, &e))
                    }),
                Step::Pictures => {
                    say(t("launcher.msg_making_pictures"), 0);
                    run::make_pictures(&kit, &Params::of(&kit), &WindowUi)
                        .then_some(())
                        .ok_or_else(|| failed_status("launcher.msg_failed", "launcher.msg_failed_why"))
                }
                Step::Game(args) => {
                    let words: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
                    log(&format!("> {}", words.join(" ")));
                    let rc = run::command(&kit, &Params::of(&kit), &words, &WindowUi);
                    game = Some(u8::try_from(rc).unwrap_or(1));
                    // a crash: the report dialog, while the logs are this run's
                    if rc == run::WENT_WRONG {
                        shared().report_open = Some(true);
                    }
                    Ok(())
                }
                Step::Host(lan) => host_arcade(&kit, *lan).map(|rc| {
                    game = Some(u8::try_from(rc).unwrap_or(1));
                    if rc == run::WENT_WRONG {
                        shared().report_open = Some(true);
                    }
                }),
            };
            if result.is_err() {
                // a setup or undo writes "game" before it builds the catalogue, so a failure after that write is not Ready
                setup_failed = matches!(step, Step::Setup { .. });
                break;
            }
        }
        RUNNING.store(run::any_on(&kit), Ordering::Relaxed);
        let failed = result.is_err() || game.is_some_and(|rc| rc != 0);
        let status = match (&result, &updated, game) {
            (Err(e), _, _) => e.clone(),
            (Ok(()), Some(tag), _) => tf("launcher.msg_updated", &[("tag", tag)]),
            (Ok(()), None, Some(2)) => ui_text(text::lang(), "launcher.status_not_set_up", &[]),
            (Ok(()), None, Some(4)) => t("launcher.msg_unguarded"),
            (Ok(()), None, Some(5)) => t("launcher.msg_not_guarded"),
            (Ok(()), None, Some(rc)) if i32::from(rc) == run::WENT_WRONG => t("launcher.msg_went_wrong"),
            (Ok(()), None, Some(rc)) if rc != 0 => {
                failed_status("launcher.msg_start_failed", "launcher.msg_start_failed_why")
            }
            (Ok(()), None, _) => t("launcher.msg_done"),
        };
        log(&status);
        log("");
        {
            let mut s = shared();
            s.status = Some((if failed { Tone::Refused } else { Tone::Done }, status));
            s.progress = if failed { 0 } else { 1000 };
            s.updated |= updated.is_some(); // sticky: look() clears it, a job that ends first must not lose it
            if updated.is_some() {
                set_available(&Kit::here(), &mut s, None);
            }
            s.failed = failed;
            s.setup_failed = setup_failed;
            s.game = game;
            s.busy = false;
        }
        poke();
    });
    true
}

/// HOST ARCADE: the server on a thread of its own until its arcade room is made (or not), then this PC's seat on it as a
/// `remote 127.0.0.1` run would (first, so it gets the projector); when that run ends, the server is asked to stop, which
/// it does once no guest is connected; that closes the room. The seat's exit code, or why there is no room.
/// `lan`: HOST LAN ARCADE, `server lan` (no tunnel, no room, found by LAN searches).
fn host_arcade(kit: &Kit, lan: bool) -> Result<i32, String> {
    log(if lan {
        "> server lan, then remote 127.0.0.1 (HOST LAN ARCADE)"
    } else {
        "> server, then remote 127.0.0.1 (HOST ARCADE)"
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let kit = Kit::here();
        let p = Params::of(&kit);
        let mode = if lan { run::Mode::LanServer } else { run::Mode::Server };
        let Ok(r) = run::start(&kit, &p, mode, false, &ServerUi) else {
            return; // what went wrong is in OUTPUT
        };
        let _ = tx.send((
            if lan { Ok(None) } else { r.arcade_code().map(Some) },
            r.arcade_guests(),
        ));
        let _ = r.supervise();
    });
    let stop = |server: thread::JoinHandle<()>| {
        let _ = run::request(kit, run::Kind::Server, "force");
        let _ = server.join();
    };
    match rx.recv() {
        Ok((Ok(room), guests)) => {
            // shortcut: a LAN arcade counts no guests (they come by plain address, not the tunnel), so it closes once
            // this PC's seat has ended; count the desk's seats if LAN guests are cut off in practice
            {
                let mut s = shared();
                s.host_room = room;
                s.host_guests = guests.clone();
            }
            poke();
            let mode = run::Mode::Remote("127.0.0.1".into(), None);
            let rc = run::play(kit, &Params::of(kit), Some(mode), false, &WindowUi);
            // a plain stop: the arcade stays while guests are connected (run::rules::arcade_rule), until they have all left
            // (asked again then) or STOP THE GAME and "stop anyway"
            if let Some((code, why)) = run::request(kit, run::Kind::Server, "stop")
                && code != run::OK
            {
                // its guests= line is for the asker, never shown
                log(&run::rules::stop_question(&why, kit.edition()).0.join("\n"));
                say(t("launcher.msg_arcade_stays"), 0);
            }
            let connected = || guests.as_ref().map_or(0, |g| g.load(Ordering::Relaxed));
            while !server.is_finished() {
                // STOP THE GAME stays offered while the server is on, whatever its answer was
                RUNNING.store(run::is_on(kit, run::Kind::Server), Ordering::Relaxed);
                poke();
                thread::sleep(Duration::from_secs(3));
                // not while a seat runs here: its stop would share data\server.request/answer with this one
                if connected() == 0 && run::is_on(kit, run::Kind::Server) && !run::is_on(kit, run::Kind::Run) {
                    let _ = run::request(kit, run::Kind::Server, "stop");
                }
            }
            let _ = server.join();
            Ok(rc)
        }
        Ok((Err(why), _)) => {
            stop(server);
            Err(tf("launcher.msg_no_arcade_room", &[("error", &why)]))
        }
        Err(_) => {
            let _ = server.join();
            Ok(run::FAILED)
        }
    }
}

/// The HOST ARCADE server's lines: into OUTPUT and the status line (its start, a wait for the lobby), never the bar or
/// the window's game state (the seat's run has those).
struct ServerUi;

impl run::Ui for ServerUi {
    fn lang(&self) -> Lang {
        text::lang()
    }

    fn say(&self, line: &str) {
        self.say_logged(line, line);
    }

    fn say_logged(&self, shown: &str, en: &str) {
        log_pair(shown, en);
        if !shown.trim().is_empty() {
            shared().status = Some((Tone::Notice, shown.trim().to_owned()));
            poke();
        }
    }

    fn cause(&self, why: &str) {
        shared().cause = Some(why.to_owned());
    }
}

/// The run's lines go to OUTPUT (its "N/M" steps move the bar); its question - stop anyway, after the card rule
/// refused a stop - is asked in a message box in front of the game; the game up minimizes the window.
struct WindowUi;

impl run::Ui for WindowUi {
    fn lang(&self) -> Lang {
        text::lang()
    }

    fn updating_pictures(&self) {
        say(t("launcher.msg_updating_pictures"), 0);
    }

    /// Step 1 of a start is timed from here: the setup again and the card catalogue before it are not its time, or one
    /// start after an update would give step 1 most of the next bar's width.
    fn staged(&self) {
        shared().stage_at = Some(Instant::now());
    }

    fn say(&self, line: &str) {
        self.say_logged(line, line);
    }

    fn say_logged(&self, shown: &str, en: &str) {
        stepped(shown);
        log_pair(shown, en);
    }

    fn ask(&self, question: &str) -> bool {
        ask_box(question, TITLE, self.lang())
    }

    fn cause(&self, why: &str) {
        shared().cause = Some(why.to_owned());
    }

    fn report(&self) {
        shared().report_open = Some(false);
        poke();
    }

    fn running(&self, up: bool) {
        UP.store(up, Ordering::Relaxed);
        WAS_UP.fetch_or(up, Ordering::Relaxed);
        RUNNING.store(up, Ordering::Relaxed);
        if up {
            say(t("launcher.msg_running"), 1000);
        }
        poke();
    }
}

/// SAVE LOGS: the logs ZIP in the Downloads folder, said in the status line and shown selected in Explorer.
fn save_logs(kit: &Path, game: Option<&Path>) {
    let dir = forza::online::save_logs::downloads();
    match launcher::report::save_zip(&dir, kit, game, &forza::online::save_logs::Scrub::from_env(), None) {
        Ok(dest) => {
            say(tf("logs.saved", &[("path", &dest.display().to_string())]), 0);
            show_in_explorer(&dest);
        }
        Err(e) => refuse(tf("logs.save_failed", &[("why", &e.to_string())])),
    }
}

/// Explorer opened on the file's folder with the file selected.
fn show_in_explorer(file: &Path) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", file.display()))
        .spawn();
}

/// A network tool's error as the player reads it (`launcher::report::plain`); the tool's own words go to the log.
fn plain_error(raw: &str) -> String {
    launcher::logs::append(&Kit::here().logs().join("launcher.txt"), &format!("  error: {raw}"));
    t(launcher::report::plain(raw))
}

/// The setup recorded a game folder (data\settings.json "game"); it writes the key before it builds the catalogue, so
/// a setup that failed after it has one too: only a job whose Setup step worked (or had none: an update) re-reads it.
fn set_up(kit: &Kit) -> bool {
    choices::setting(&kit.settings_file(), "game").is_some_and(|g| !g.trim().is_empty())
}

/// A job ended this look (busy -> not): read the setup state again, unless its Step::Setup (setup or undo) failed. A
/// later step may fail after the setup worked (SET UP: the card pictures), and the setup state is still the new one.
fn reread_setup(busy: bool, now_busy: bool, setup_failed: bool) -> bool {
    busy && !now_busy && !setup_failed
}

/// The game folder setup recorded (its logs and seat 1's go in a report).
fn game_folder(kit: &Kit) -> Option<PathBuf> {
    let f = choices::setting(&kit.settings_file(), "game")?;
    launcher::kit::find_game(&f).or_else(|| Some(PathBuf::from(f)))
}

/// SAVE LOGS in the report dialog: the words and the logs saved in the Downloads folder, on a thread; the outcome comes
/// back in `report_done`.
fn send_report(words: String, crashed: bool) {
    thread::spawn(move || {
        let kit = Kit::here();
        let why = if crashed {
            "after a run that went wrong"
        } else {
            "opened by the player"
        };
        let dir = forza::online::save_logs::downloads();
        let done = match launcher::report::save(&kit, game_folder(&kit).as_deref(), &words, why, &dir) {
            Ok(zip) => {
                log(&format!("report: saved as {}", zip.display()));
                show_in_explorer(&zip);
                Ok((zip.display().to_string(), launcher::report::issue_url(&kit, why)))
            }
            Err(e) => {
                log(&format!("report: not saved ({e})"));
                Err(tf("logs.save_failed", &[("why", &e.to_string())]))
            }
        };
        shared().report_done = Some(done);
        poke();
    });
}

/// A game start was given on the command line (a shortcut's `play`, a dropped game folder is no start): the window it
/// opens is there for that run and ends with it; one the player starts in the window goes back to its screen instead.
fn starts_a_run(steps: &[Step]) -> bool {
    steps.iter().any(|s| matches!(s, Step::Game(_) | Step::Host(_)))
}

/// The window closes when the run it was opened for ends well. That job's end, good or bad, is the command line's last
/// say: a run the player starts later in the window ends on its screen.
fn closes_with_run(from_command: &mut bool, game: Option<u8>, failed: bool, was_up: bool) -> bool {
    let close = *from_command && was_up && game == Some(0);
    if game.is_some() || failed {
        *from_command = false;
    }
    close
}

/// A Windows message box with no owner; its answer.
fn message(text: &str, style: u32) -> i32 {
    message_titled(text, TITLE, style)
}

fn message_titled(text: &str, title: &str, style: u32) -> i32 {
    let (text, title) = (wide(text), wide(title));
    // SAFETY: NUL-terminated strings that live across the call; no owner window.
    unsafe { MessageBoxW(ptr::null_mut(), text.as_ptr(), title.as_ptr(), style) }
}

thread_local! {
    /// The Yes and No labels [`ask_box`] puts on its box while it is up on this thread.
    static BUTTONS: std::cell::RefCell<Option<[Vec<u16>; 2]>> = const { std::cell::RefCell::new(None) };
}

/// A yes/no question in front of the game (no owner: any thread may ask); true on yes. MessageBoxW labels its buttons
/// in the Windows language, so a CBT hook on this thread relabels them in the Launcher's (`lang`) as the box opens.
fn ask_box(question: &str, title: &str, lang: Lang) -> bool {
    BUTTONS.set(Some(
        ["launcher.ask_yes", "launcher.ask_no"].map(|k| wide(&ui_text(lang, k, &[]))),
    ));
    // SAFETY: a hook of this thread only, removed below; `relabel` reads this thread's BUTTONS.
    let hook = unsafe { SetWindowsHookExW(WH_CBT, Some(relabel), ptr::null_mut(), GetCurrentThreadId()) };
    let yes = message_titled(
        question,
        title,
        MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
    ) == IDYES;
    if !hook.is_null() {
        // SAFETY: the hook set above, on this thread.
        unsafe { UnhookWindowsHookEx(hook) };
    }
    BUTTONS.set(None);
    yes
}

/// [`ask_box`]'s hook: the box about to be shown (`w`) gets this thread's labels on its Yes and No buttons.
unsafe extern "system" fn relabel(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if u32::try_from(code) == Ok(HCBT_ACTIVATE) {
        BUTTONS.with_borrow(|b| {
            if let Some([yes, no]) = b {
                // SAFETY: `w` is the window being activated; NUL-terminated labels that outlive the calls.
                unsafe {
                    SetDlgItemTextW(w as _, IDYES, yes.as_ptr());
                    SetDlgItemTextW(w as _, IDNO, no.as_ptr());
                }
            }
        });
    }
    // SAFETY: the next hook gets the call as it came.
    unsafe { CallNextHookEx(ptr::null_mut(), code, w, l) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// STOP THE GAME (clicked twice), or this window closed twice while the game runs: the running game is asked to stop by
/// the card rule, on a thread of its own - the run itself may be this process's job.
fn stop_game() {
    say(t("launcher.msg_stopping"), 0);
    thread::spawn(|| {
        let kit = Kit::here();
        run::command(&kit, &Params::of(&kit), &["stop".to_owned()], &WindowUi);
        RUNNING.store(run::any_on(&kit), Ordering::Relaxed);
        poke();
    });
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// data\update-check.txt: when the release list was last asked for (unix seconds).
fn check_file(kit: &Kit) -> PathBuf {
    kit.data().join("update-check.txt")
}

/// The UPDATE page is open.
static PAGE_SHOWN: AtomicBool = AtomicBool::new(false);

/// The newer release's notice, kept in data\update-available.txt so a restart within the day still shows it.
fn set_available(kit: &Kit, s: &mut Shared, tag: Option<String>) {
    let file = kit.data().join("update-available.txt");
    match &tag {
        Some(tag) => {
            let _ = std::fs::write(&file, tag);
        }
        None => {
            let _ = std::fs::remove_file(&file);
        }
    }
    s.available = tag;
}

/// At the start: the notice a check in the last day left, if it is still newer than this kit.
fn saved_available(kit: &Kit) -> Option<String> {
    let tag = std::fs::read_to_string(kit.data().join("update-available.txt")).ok()?;
    let tag = tag.trim();
    is_newer(tag, &this_kit(kit)).then(|| tag.to_owned())
}

/// The release list, asked in the background: at most once a day when the window opens, and every time the UPDATE page
/// is opened.  The edition's own releases only.
fn ask_updates() {
    static ASKING: AtomicBool = AtomicBool::new(false);
    if ASKING.swap(true, Ordering::Relaxed) {
        return;
    }
    shared().update_note = Some((Tone::Notice, t("launcher.msg_asking")));
    thread::spawn(|| {
        let kit = Kit::here();
        let found = update::fetch(kit.edition().name());
        let installed = this_kit(&kit);
        if let Ok(list) = &found {
            let _ = std::fs::write(check_file(&kit), unix_now().to_string());
            // the notes of this version and the newest, kept for UPDATE when GitHub does not answer
            let newer = update::newest(list, &installed).map(|r| r.tag.as_str());
            for r in list
                .iter()
                .filter(|r| parse(&r.tag) == parse(&installed) || Some(r.tag.as_str()) == newer)
            {
                keep_notes(&kit.data(), &r.tag, &r.notes);
            }
        }
        ASKING.store(false, Ordering::Relaxed);
        {
            let mut s = shared();
            match found {
                Ok(list) if !list.is_empty() => {
                    let newer = update::newest(&list, &installed).map(|r| r.tag.clone());
                    s.update_note = newer.is_none().then(|| (Tone::Notice, t("launcher.msg_pick")));
                    // the page is open: the player sees the release there
                    set_available(&kit, &mut s, newer.filter(|_| !PAGE_SHOWN.load(Ordering::Relaxed)));
                    s.releases = list;
                    s.listed = true;
                }
                Ok(_) => s.update_note = Some((Tone::Done, t("launcher.msg_none_listed"))),
                Err(e) => {
                    s.update_note = Some((
                        Tone::Refused,
                        tf("launcher.msg_no_answer", &[("error", &plain_error(&e))]),
                    ))
                }
            }
        }
        poke();
    });
}

// ---- the window -------------------------------------------------------------------------------------------------

/// The window: the screen's state, the kit, the releases behind its list.
struct App {
    st: LauncherState,
    releases: Vec<Release>,
    /// The release and language `st.notes` was made for.
    notes_for: Option<(String, Lang)>,
    online: online::Online,
    /// When the LAN fold's last search started.
    lan_at: Option<Instant>,
    /// The window was opened for a game start given on the command line (a shortcut's `play`): it ends with that run.
    run_from_command: bool,
    /// What a club still marked out left (2017-18, #434), and whether PLAY asked about its arcade copy yet.
    club: Option<Left>,
    club_asked: bool,
}

/// The LAN fold searches this often while it is open.
const LAN_EVERY: Duration = Duration::from_secs(4);

impl App {
    /// The state as the kit has it now; `folder` given (dropped on Launcher.exe) opens SETUP.
    fn new(kit: &Kit, folder: Option<&OsStr>, run_from_command: bool) -> Self {
        let ed = kit.edition();
        let panel = choices::read(&panel_file(kit));
        let settings = kit.settings_file();
        let online = choices::panel_value(&panel, "play_on").is_some_and(|v| v.eq_ignore_ascii_case("online"));
        let address = choices::panel_value(&panel, "server")
            .or_else(|| choices::setting(&settings, "server"))
            .unwrap_or_default();
        let game = folder.map_or_else(
            || choices::setting(&settings, "game").unwrap_or_default(),
            |f| f.to_string_lossy().into_owned(),
        );
        RUNNING.store(run::any_on(kit), Ordering::Relaxed);
        // the bar of a start creeps at the pace this PC's last start took; the estimate until one has timed it
        shared().stage_secs = launcher::steps::read(kit).unwrap_or(forza_ui::widgets::START_SECS);
        let set_up = set_up(kit);
        if !set_up {
            say(ui_text(text::lang(), "launcher.status_not_set_up", &[]), 0);
        } else if RUNNING.load(Ordering::Relaxed) {
            say(t("launcher.msg_running"), 0);
        } else if shared().status.is_none() {
            say_tone(Tone::Done, ui_text(text::lang(), "launcher.status_ready", &[]), 0);
        }
        let online_state = online::Online::new(&settings, &address, ed);
        let mut lobby = screen::lobby_for(ed);
        lobby.address = address;
        lobby.arcade_rooms = ed == Edition::E1718 && run::rules::arcade_1718_on(&kit.settings());
        // settings keys kept from before the arcade rename: players' settings.json already holds them
        lobby.list_arcade = kit.settings()["list_shop"].as_bool() == Some(true);
        lobby.arcade_name = choices::setting(&settings, "shop_name").unwrap_or_default();
        lobby.arcade_notice = choices::setting(&settings, "arcade_notice").unwrap_or_default();
        lobby.cup = launcher::conf::cup_choice(&kit.settings());
        let st = LauncherState {
            edition: ed,
            lang: text::lang(),
            online,
            lobby,
            rejoin: None,
            set_up,
            running: RUNNING.load(Ordering::Relaxed),
            cup_started: launcher::run::cup_started(),
            busy: false,
            steps: None,
            clock: (0.0, 0.0),
            // the bar creeps at this PC's measured pace; the estimate until a start has timed its six steps
            stage_secs: shared().stage_secs,
            progress: 0.0,
            status: None,
            server_notice: None,
            tab: if !set_up || folder.is_some() {
                Tab::Setup
            } else if online {
                Tab::Online
            } else {
                Tab::Play
            },
            folder: game,
            pictures_hq: pictures_hq(kit),
            projector: (kit.edition() == Edition::E1718).then(|| choices::projector_shown(&kit.settings())),
            cpu_championship: (kit.edition() == Edition::E1718).then(|| choices::cpu_championship_on(&kit.settings())),
            frame_cap: (kit.edition() == Edition::E1718).then(|| forza::kit::frame_cap(&kit.settings())),
            vsync: (kit.edition() == Edition::E1718).then(|| forza::kit::vsync(&kit.settings())),
            club_server: (kit.edition() == Edition::E1718)
                .then(|| choices::setting(&settings, "club_server").unwrap_or_default()),
            vs_link: (kit.edition() == Edition::E1718).then(|| kit.settings()["vs_link"].as_bool() != Some(false)),
            clean_up: ed == Edition::E1718,
            clean_status: None,
            club_row: None,
            updates: true,
            version: version(kit),
            update_available: saved_available(kit),
            update_status: None,
            releases: Vec::new(),
            selected: None,
            notes: None,
            output: Vec::new(),
            confirm: None,
            report: None,
            stop_armed: 0.0,
        };
        let mut app = App {
            st,
            releases: Vec::new(),
            notes_for: None,
            online: online_state,
            lan_at: None,
            run_from_command,
            club: None,
            club_asked: false,
        };
        app.club_look(kit);
        app.club_say();
        app
    }

    /// What a club still marked out left (#434), read again when the window opens, a job ends, after CLEAN UP and
    /// after the arcade copy row's button. Nothing while a run is on: a trip in progress keeps its marker too.
    fn club_look(&mut self, kit: &Kit) {
        self.club = None;
        self.st.club_row = None;
        if kit.edition() != Edition::E1718 || RUNNING.load(Ordering::Relaxed) || run::any_on(kit) {
            return;
        }
        let dir = kit.save().join("gamesvr");
        match club::left(&dir) {
            Ok(l) => self.club = l,
            Err(e) => log(&format!("arcade copy not offered: {e}")),
        }
        self.st.club_row = match &self.club {
            Some(Left::Differs(o)) => Some(ClubRow::Offer(tf(
                "launcher.club_offer_hint",
                &[("arcade", &date_time(o.pulled)), ("home", &date_time(o.home_at))],
            ))),
            _ if club::can_undo(&dir) => Some(ClubRow::Undo),
            _ => None,
        };
    }

    /// The club's line in the status line when it says nothing else: a copy on offer, or the club as the arcade sent it.
    fn club_say(&self) {
        let line = match &self.club {
            Some(Left::Differs(_)) => t("launcher.msg_club_offer"),
            Some(Left::Same { back }) => tf("launcher.club_up_to_date", &[("time", &date_time(*back))]),
            None => return,
        };
        let quiet = shared().status.as_ref().is_none_or(|(tone, _)| *tone == Tone::Done);
        if quiet {
            say(line, 0);
        }
    }

    /// USE THE ARCADE COPY (`take`) or PUT THE HOME CLUB BACK: refused as CLEAN UP is while a run is on or a game window
    /// is open; the result in the status line, the English and the paths in launcher.txt.
    fn club_act(&mut self, kit: &Kit, take: bool) {
        let game = game_folder(kit).unwrap_or_else(|| kit.dir().join("game"));
        let open = launcher::procs::game_processes(&game)
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case("client_release.exe") || launcher::procs::has_window(p.pid));
        if open || RUNNING.load(Ordering::Relaxed) || run::any_on(kit) {
            refuse(t("launcher.msg_game_running"));
            return;
        }
        let dir = kit.save().join("gamesvr");
        let done = if take {
            club::take(&dir).map(|tk| {
                log(&format!(
                    "arcade copy put home; the home club from before is {}",
                    tk.backup.display()
                ));
                match tk.after {
                    None => (Tone::Done, t("launcher.msg_club_arcade_used")),
                    Some(e) => {
                        log(&format!("arcade copy home, a later step failed: {e}"));
                        let path = tk.backup.display().to_string();
                        (
                            Tone::Notice,
                            tf("launcher.msg_club_arcade_used_partly", &[("path", &path)]),
                        )
                    }
                }
            })
        } else {
            club::undo(&dir).map(|()| {
                log("home club put back as it was before the arcade copy");
                (Tone::Done, t("launcher.msg_club_home_back"))
            })
        };
        match done {
            Ok((tone, line)) => say_tone(tone, line, 0),
            Err(NotReplaced::Changed(p)) => {
                log(&format!(
                    "club not replaced: it changed meanwhile; the copies are in {}",
                    p.display()
                ));
                refuse(tf("launcher.msg_club_changed", &[("path", &p.display().to_string())]));
            }
            Err(NotReplaced::Failed(e)) => {
                log(&format!("club not replaced: {e}"));
                refuse(tf("launcher.msg_club_not_replaced", &[("error", &e)]));
            }
        }
        self.club_look(kit);
    }

    /// PLAY with the arcade's copy on offer asks once in this window (#434): true when it asks now. Cancel, then PLAY
    /// again, starts as before, the home club kept.
    fn club_question(&mut self) -> bool {
        let Some(Left::Differs(o)) = &self.club else {
            return false;
        };
        if !asks_first(true, &mut self.club_asked) {
            return false;
        }
        let (home, arcade) = (date_time(o.home_at), date_time(o.pulled));
        let q = match o.plays {
            Some((h, a)) => tf(
                "launcher.club_offer_ask_plays",
                &[
                    ("home", &home),
                    ("arcade", &arcade),
                    ("home_n", &h.to_string()),
                    ("arcade_n", &a.to_string()),
                ],
            ),
            None => tf("launcher.club_offer_ask", &[("home", &home), ("arcade", &arcade)]),
        };
        self.ask(&q, "launcher.club_use_arcade", Action::ClubUseArcade);
        true
    }

    /// The release list with this folder's marks.
    fn list(&mut self, kit: &Kit) {
        let installed = this_kit(kit);
        self.st.releases = self
            .releases
            .iter()
            .map(|r| screen::Release {
                tag: r.tag.clone(),
                test: r.test,
                date: r.date.clone(),
                this: parse(&r.tag).is_some() && parse(&r.tag) == parse(&installed),
                newer: is_newer(&r.tag, &installed),
            })
            .collect();
    }

    /// The notes under UPDATE's list: the chosen release's (the list's copy, else the one kept in data\), else this
    /// version's kept copy, in the window's language. Made again only when the release or the language changes.
    fn notes(&mut self, kit: &Kit) {
        let chosen = self.st.selected.and_then(|i| self.releases.get(i));
        let tag = chosen.map_or_else(|| this_kit(kit), |r| r.tag.clone());
        let key = (tag, self.st.lang);
        if self.notes_for.as_ref() == Some(&key) {
            return;
        }
        let body = chosen
            .map(|r| r.notes.clone())
            .filter(|b| !b.trim().is_empty())
            .or_else(|| kept_notes(&kit.data(), &key.0));
        self.st.notes = body
            .map(|b| launcher::release::notes(&b, key.1.code()))
            .filter(|t| !t.is_empty());
        self.notes_for = Some(key);
    }

    /// The question for the player, in the window's language.
    fn ask(&mut self, question: &str, yes: &str, then: Action) {
        let yes = ui_text(self.st.lang, yes, &[]);
        self.st.confirm = Some(Confirm::new(question, yes, then));
    }

    /// The game folder box, or a message that it is empty.
    fn folder(&mut self) -> Option<OsString> {
        let f = self.st.folder.trim();
        if f.is_empty() {
            self.st.tab = Tab::Setup;
            refuse(t("launcher.msg_no_folder"));
            return None;
        }
        Some(f.into())
    }

    /// What the workers said since the last look; the window minimized for the game, back when its run ended badly or
    /// when the player started it here, closed only when a command line's run ended well.  Runs also while the window is
    /// minimized.
    fn look(&mut self, ctx: &egui::Context) {
        let ended;
        let (log, list, updated, failed, game, report_open, report_done) = {
            let mut s = shared();
            if reread_setup(self.st.busy, s.busy, s.setup_failed) {
                self.st.set_up = set_up(&Kit::here());
            }
            if self.st.busy && !s.busy && self.st.edition == Edition::E1718 {
                self.st.pictures_hq = pictures_hq(&Kit::here());
            }
            self.st.status.clone_from(&s.status);
            self.st.update_status.clone_from(&s.update_note);
            self.st.update_available.clone_from(&s.available);
            self.st.progress = s.progress as f32 / 1000.0;
            self.st.steps = s.steps.map(|(n, m)| (n as usize, m as usize));
            self.st.clock = s.began.zip(s.stage_at).map_or((0.0, 0.0), |(b, a)| {
                (a.elapsed().as_secs_f32(), b.elapsed().as_secs_f32())
            });
            self.st.stage_secs = s.stage_secs;
            ended = self.st.busy && !s.busy;
            self.st.busy = s.busy;
            s.setup_failed = false; // one look's: the next job says its own
            (
                mem::take(&mut s.log),
                mem::take(&mut s.listed).then(|| s.releases.clone()),
                mem::take(&mut s.updated),
                mem::take(&mut s.failed),
                s.game.take(),
                s.report_open.take(),
                s.report_done.take(),
            )
        };
        self.st.running = RUNNING.load(Ordering::Relaxed);
        // the cup time the run was started with, while it is on (its open time does not move with this window's clock)
        self.st.cup_started = launcher::run::cup_started().filter(|_| self.st.running);
        self.st.output.extend(log);
        let kit = Kit::here();
        if ended {
            self.club_look(&kit);
            self.club_say();
        }
        // HOST ARCADE: its code once the lobby gave it, gone with the room if the lobby loses it; the job over, the arcade is
        let (room, guests) = {
            let s = shared();
            (s.host_room.clone(), s.host_guests.clone())
        };
        let v = &mut self.st.lobby;
        v.host_code = None;
        // this PC's seat and its tunnel's guests: the desk's count for the room too (seat 1 and the JOINs); capped, as a
        // guest's reconnect can count twice until its old link times out
        let seats = usize::from(*forza::online::SEATS.end());
        v.host_players = guests.map(|g| (1 + g.load(Ordering::Relaxed)).min(seats) as u8);
        if let Some((code, desk, lost)) = room {
            v.desk = desk;
            if lost.load(Ordering::Relaxed) {
                v.notice = Some(forza_ui::lobby::Notice::RoomLost);
            } else {
                v.host_code = Some(code);
            }
        }
        if !self.st.busy {
            self.st.lobby.hosting = false;
            self.st.lobby.host_lan = None;
        }
        self.find_lan();
        // the VS room's countdown ran out, or an arcade code got this PC a seat: the run starts
        let go = self
            .online
            .look(&mut self.st, kit.edition().name(), &kit.settings_file());
        if !self.st.busy && !self.st.running {
            match go {
                Some(online::Go::Play) => self.play(&kit, Vec::new()),
                Some(online::Go::Remote(host)) => self.play(&kit, vec!["remote".into(), host.into()]),
                None => {}
            }
        }
        // the run this process held has ended well: Launcher.exe ends with it, unless the player started it here
        if closes_with_run(&mut self.run_from_command, game, failed, WAS_UP.load(Ordering::Relaxed)) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        let minimized = ctx.input(|i| i.viewport().minimized) == Some(true);
        if UP.load(Ordering::Relaxed) && !minimized && !MINIMIZED.swap(true, Ordering::Relaxed) {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true)); // the game's windows are in front; this one waits
        } else if game.is_some() && minimized {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false)); // the run ended badly: what it said is shown
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        if !UP.load(Ordering::Relaxed) {
            MINIMIZED.store(false, Ordering::Relaxed);
        }
        if failed {
            self.st.tab = Tab::Details;
        }
        if let Some(crashed) = report_open {
            if minimized {
                // asked from the game's panel, or after a crash: in front of the game
                ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            self.open_report(&kit, crashed);
        }
        if let (Some(done), Some(f)) = (report_done, self.st.report.as_mut()) {
            f.sending = false;
            f.result = Some(match done {
                Ok((path, url)) => {
                    f.sent = true;
                    f.issue_url = Some(url);
                    (Tone::Done, tf("logs.saved", &[("path", &path)]))
                }
                Err(why) => (Tone::Refused, why),
            });
        }
        if let Some(list) = list {
            self.st.selected = Some(update::reselect(
                &self.releases,
                self.st.selected,
                &list,
                &this_kit(&kit),
            ));
            self.releases = list;
            self.list(&kit);
        }
        // the UPDATE tab opened: the list is asked for again
        let shown = self.st.tab == Tab::Update;
        if PAGE_SHOWN.swap(shown, Ordering::Relaxed) != shown && shown {
            set_available(&kit, &mut shared(), None);
            ask_updates();
        }
        if updated {
            // this process still runs the old Launcher.exe's code (the setup, the text and the card pictures of the new
            // release would be made by it): the new one starts and does the setup itself (AFTER_UPDATE in main)
            if restart_after_update(&kit) {
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return;
            }
            self.st.version = version(&kit);
            self.list(&kit); // the "this version" mark moves
            // the new files need the setup again, and the game text follows the Launcher language: no question
            if !self.st.folder.trim().is_empty() && !self.set_up_after_update(&kit) {
                shared().updated = true; // another job holds the worker: try again at the next look
            }
        }
        // closing Launcher while the game runs is STOP THE GAME (armed by the first close, as by a first click), asked
        // of the run by the card rule; and never a kit left half copied, nor a start half done: the window stays while
        // a job runs
        if ctx.input(|i| i.viewport().close_requested()) {
            // a hosted arcade kept open for its guests is this window's run too, with no seat up (host_arcade)
            if UP.load(Ordering::Relaxed) || (self.st.lobby.hosting && RUNNING.load(Ordering::Relaxed)) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                if self.st.stop_click(ctx.input(|i| i.time)) {
                    stop_game();
                } else {
                    // closed from the taskbar while minimized: CLICK AGAIN is shown, not armed unseen
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
            } else if shared().busy {
                // the live flag: a job started in this look() is not in the sampled copy
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                say(t("launcher.msg_busy"), shared().progress);
            }
        }
    }

    /// The report dialog, unless one is open already (its words stay).
    fn open_report(&mut self, kit: &Kit, crashed: bool) {
        if self.st.report.is_some() {
            return;
        }
        self.st.report = Some(screen::ReportForm {
            crashed,
            contents: launcher::report::contents(kit, game_folder(kit).as_deref()),
            ..Default::default()
        });
    }

    /// CLEAN UP: the leftovers of a crash go after a backup (`launcher::cleanup`); one line in the window,
    /// one per item in DETAILS and launcher.txt.
    fn clean_up(&mut self, kit: &Kit) {
        let game = game_folder(kit).unwrap_or_else(|| kit.dir().join("game"));
        let procs = launcher::procs::game_processes(&game);
        let done = launcher::cleanup::clean(
            &kit.data(),
            RUNNING.load(Ordering::Relaxed) || run::any_on(kit),
            &procs,
            launcher::procs::has_window,
            launcher::procs::kill,
            &launcher::cleanup::today(),
        );
        let (tone, line) = match done {
            Err(e) => {
                log(&format!("clean-up: backup failed, nothing removed: {e}"));
                (
                    Tone::Refused,
                    tf("launcher.cleanup_failed", &[("what", &e.to_string())]),
                )
            }
            Ok(r) if r.refused => {
                log("clean-up: refused, the game is running");
                (Tone::Refused, t("launcher.msg_game_running"))
            }
            Ok(r) => {
                for l in &r.cleaned {
                    log(&format!("clean-up: {l}"));
                }
                for l in &r.failed {
                    log(&format!("clean-up: FAILED {l}"));
                }
                if let Some(b) = &r.backup {
                    log(&format!("clean-up: backup in {}", b.display()));
                }
                if !r.failed.is_empty() {
                    (
                        Tone::Refused,
                        tf("launcher.cleanup_failed", &[("what", &r.failed.join("; "))]),
                    )
                } else if r.cleaned.is_empty() {
                    (Tone::Done, t("launcher.cleanup_none"))
                } else {
                    let n = r.cleaned.len().to_string();
                    let path = r.backup.as_ref().map_or_else(String::new, |b| b.display().to_string());
                    (Tone::Done, tf("launcher.cleanup_done", &[("n", &n), ("path", &path)]))
                }
            }
        };
        self.st.clean_status = Some((tone, line));
        self.club_look(kit);
    }

    /// What the player asked for in the screen.
    fn act(&mut self, a: Action, ctx: &egui::Context, frame: &eframe::Frame) {
        let kit = Kit::here();
        match a {
            Action::Play => self.play(&kit, Vec::new()),
            Action::Stop => stop_game(),
            Action::SetUp => {
                if let Some(game) = self.folder() {
                    start(vec![
                        Step::Setup {
                            folder: game,
                            undo: false,
                        },
                        Step::Pictures,
                    ]);
                }
            }
            Action::Undo => {
                if self.folder().is_some() {
                    self.ask(&t("launcher.msg_undo_confirm"), "launcher.undo", Action::UndoNow);
                }
            }
            Action::UndoNow => {
                if let Some(game) = self.folder() {
                    // the undo refuses while English is on: the original files go back first
                    let mut steps = Vec::new();
                    if kit.text_changed() {
                        steps.push(Step::Text(""));
                    }
                    steps.push(Step::Setup {
                        folder: game,
                        undo: true,
                    });
                    start(steps);
                }
            }
            Action::Browse => {
                let picked = rfd::FileDialog::new()
                    .set_title(t("launcher.msg_browse_title"))
                    .set_parent(frame)
                    .pick_folder();
                if let Some(p) = picked {
                    self.st.folder = p.display().to_string();
                }
            }
            Action::SaveLogs => {
                let game = self.st.folder.trim();
                let game = (!game.is_empty()).then(|| Path::new(game));
                save_logs(kit.dir(), game);
            }
            Action::CleanUp => self.clean_up(&kit),
            Action::ClubUseArcade => self.club_act(&kit, true),
            Action::ClubUndo => self.club_act(&kit, false),
            Action::Pictures { choose } => self.pictures_pick(&kit, frame, choose),
            Action::PicturesAsk => {
                if self.st.pictures_hq {
                    self.ask(
                        &t("launcher.pictures_remove_ask"),
                        "launcher.pictures_remove",
                        Action::PicturesNow,
                    );
                } else {
                    let size = (pack::PACK.size >> 20).to_string();
                    self.ask(
                        &tf("launcher.pictures_download_ask", &[("size", &size)]),
                        "launcher.pictures_download",
                        Action::PicturesNow,
                    );
                }
            }
            Action::PicturesNow => self.pictures(&kit),
            Action::Projector => {
                // the projector in the run picks it up within a second
                let file = kit.settings_file();
                let on = self.st.projector != Some(false);
                if let Err(e) = choices::set_setting_bool(&file, choices::PROJECTOR, on) {
                    refuse(write_failed(&file, &e));
                }
            }
            Action::CpuChampionship => {
                let file = kit.settings_file();
                let on = self.st.cpu_championship == Some(true);
                if let Err(e) = choices::set_setting_bool(&file, choices::CPU_CHAMPIONSHIP, on) {
                    refuse(write_failed(&file, &e));
                }
            }
            Action::FramePacing => {
                // data\settings.json "frame_cap" and "vsync", which forza_panel.dll reads at the game's start
                let file = kit.settings_file();
                for (key, on) in [("frame_cap", self.st.frame_cap), ("vsync", self.st.vsync)] {
                    if let Some(on) = on
                        && let Err(e) = choices::set_setting_bool(&file, key, on)
                    {
                        refuse(write_failed(&file, &e));
                        break;
                    }
                }
            }
            Action::VsLink => {
                // data\settings.json "vs_link" (default on), which run::start_vslink reads
                let file = kit.settings_file();
                let on = self.st.vs_link != Some(false);
                if let Err(e) = choices::set_setting_bool(&file, "vs_link", on) {
                    refuse(write_failed(&file, &e));
                }
            }
            Action::Update(i) => {
                if let Some(r) = self.releases.get(i) {
                    if r.needs_installer {
                        // the game data changed: nothing is downloaded; the release page has the installer
                        refuse(tf("launcher.msg_needs_installer", &[("tag", &r.tag)]));
                        ctx.open_url(egui::OpenUrl::new_tab(&r.page));
                        return;
                    }
                    let key = if r.test {
                        "launcher.msg_confirm_test"
                    } else {
                        "launcher.msg_confirm_release"
                    };
                    let question = tf(key, &[("tag", &r.tag)]);
                    self.ask(&question, "launcher.update_button", Action::UpdateNow(r.tag.clone()));
                }
            }
            Action::UpdateNow(tag) => {
                // by tag: a list refreshed while the question was open may have moved the rows
                if let Some(r) = self.releases.iter().find(|r| r.tag == tag) {
                    start(vec![Step::Update(r.clone())]);
                }
            }
            Action::WhatsNew(i) => {
                // the chosen release's page, else (no list: offline) the releases page
                let page = i
                    .and_then(|i| self.releases.get(i))
                    .map_or(RELEASES_PAGE, |r| r.page.as_str());
                ctx.open_url(egui::OpenUrl::new_tab(page));
            }
            // #434: the club question before the lobby claims a seat or a room's countdown, or a hosted arcade opens; the
            // same button again after it
            Action::Lobby(a) if claims_a_start(&a) && self.club_question() => {}
            Action::Lobby(LobbyAction::HostArcade) => {
                if !self.st.busy && !self.st.running {
                    // the room goes on the club server the box names now (the run reads the setting), checked as PLAY does
                    if let Some(club) = &self.st.club_server {
                        let club = club.trim();
                        if !club.is_empty() && choices::parse_club_server(club).is_none() {
                            refuse(t("launcher.msg_bad_club"));
                            return;
                        }
                        let file = kit.settings_file();
                        if let Err(e) = choices::set_setting(&file, "club_server", club) {
                            refuse(write_failed(&file, &e));
                            return;
                        }
                    }
                    // the host dialog's switch (default off) and the listed room's name as typed, as the desk carries it
                    // (run::open_arcade_room reads both), the guests' notice (run's start_server_side) and the Universal
                    // Cup time (run's start_gamesvr)
                    let name = forza::online::lobby::arcade_name(&self.st.lobby.arcade_name);
                    let notice = self.st.lobby.arcade_notice.trim().to_owned();
                    let file = kit.settings_file();
                    // same kept settings keys as at start-up
                    if let Err(e) = choices::set_setting_bool(&file, "list_shop", self.st.lobby.list_arcade)
                        .and_then(|()| choices::set_setting(&file, "shop_name", &name))
                        .and_then(|()| choices::set_setting(&file, "arcade_notice", &notice))
                        .and_then(|()| launcher::conf::save_cup(&file, self.st.lobby.cup))
                    {
                        refuse(write_failed(&file, &e));
                        return;
                    }
                    self.st.lobby.arcade_name = name;
                    self.st.lobby.arcade_notice = notice;
                    self.st.lobby.hosting = true;
                    self.st.lobby.notice = None;
                    self.online.hosting();
                    start(vec![Step::Host(false)]);
                }
            }
            Action::Lobby(LobbyAction::HostLan) => {
                if !self.st.busy && !self.st.running {
                    // the cup the PLAY tab shows (run's start_gamesvr reads it)
                    let file = kit.settings_file();
                    if let Err(e) = launcher::conf::save_cup(&file, self.st.lobby.cup) {
                        refuse(write_failed(&file, &e));
                        return;
                    }
                    let ips = forza::online::lan::addresses()
                        .iter()
                        .map(ToString::to_string)
                        .collect();
                    self.st.lobby.host_lan = Some(ips);
                    self.st.lobby.hosting = true;
                    self.st.lobby.notice = None;
                    self.online.hosting();
                    start(vec![Step::Host(true)]);
                }
            }
            Action::Lobby(LobbyAction::JoinLan(address)) if !self.st.busy && !self.st.running => {
                if launcher::lobby::valid_server(&address) {
                    self.play(&kit, vec!["remote".into(), address.clone().into()]);
                    self.online.play_on(Some(address));
                } else {
                    refuse(t("launcher.msg_bad_address"));
                }
            }
            // LEAVE while hosting (clicked twice, as STOP THE GAME): the seat stops by the card rule; the server (and the
            // room) after it, never under its match (run::rules::stop_server_too, and host_arcade's stop once the seat's
            // run has ended)
            Action::Lobby(LobbyAction::Leave) if self.st.lobby.hosting => stop_game(),
            Action::Lobby(a) => self.online.act(a, &mut self.st, &kit.settings_file()),
            Action::Rejoin => self.online.rejoin(),
            Action::Report => self.open_report(&kit, false),
            Action::SendReport => {
                if let Some(f) = &mut self.st.report
                    && !f.sending
                {
                    f.sending = true;
                    f.result = None;
                    send_report(f.words.clone(), f.crashed);
                }
            }
            Action::Language => {
                let lang = self.st.lang;
                text::set(lang);
                let _ = choices::set_setting(&kit.settings_file(), "language", lang.code());
                if let Some(line) = language_note(kit.edition(), game_text(&kit), kit.game_language()) {
                    say(line, 0);
                }
            }
        }
    }

    /// After an update, when the new Launcher.exe could not be started: the same steps in this process.
    fn set_up_after_update(&self, kit: &Kit) -> bool {
        start(after_update_steps(kit, self.st.folder.trim()))
    }

    /// The LAN fold, open in ONLINE: a search every few seconds on a thread of its own, its answers into the list.
    fn find_lan(&mut self) {
        if let Some(found) = shared().lan_found.take() {
            self.st.lobby.lan = found
                .into_iter()
                .map(|f| forza_ui::lobby::LanArcade {
                    address: if f.desk_port == forza::online::SEAT_PORT {
                        f.ip.to_string()
                    } else {
                        format!("{}:{}", f.ip, f.desk_port)
                    },
                    name: f.name,
                })
                .collect();
        }
        let v = &self.st.lobby;
        if !(self.st.tab == Tab::Online && v.lan_open && !v.hosting)
            || self.lan_at.is_some_and(|t| t.elapsed() < LAN_EVERY)
        {
            return;
        }
        self.lan_at = Some(Instant::now());
        thread::spawn(|| {
            let lan = forza::online::lan::find(
                forza::online::lan::PORT,
                forza::online::lan::E1718,
                Duration::from_secs(1),
            );
            shared().lan_found = Some(lan.unwrap_or_default());
            poke();
        });
    }

    /// PLAY: the choices into data\panel.txt, then the run (`words` empty: what the choices say; else play.py's words,
    /// an arcade room's `remote ID`).
    fn play(&mut self, kit: &Kit, words: Vec<OsString>) {
        if self.club_question() {
            return;
        }
        let online = self.st.online;
        let mut server = self.st.lobby.address.trim().to_owned();
        // 2017-18: a club server alone is enough to play online (run::rules::vs_target)
        let club_only = self.st.club_server.as_ref().is_some_and(|c| !c.trim().is_empty());
        // a seat or VS room the lobby reserved: its server and ticket; else the Direct address, as before the lobby
        // (2017-18: a VS room's only, crates/server/README.md "VS rooms")
        let vs_only = kit.edition() == Edition::E1718;
        let seat = online.then(|| self.online.take_ticket(vs_only)).flatten();
        self.online.play_on(seat.as_ref().map(|(s, _)| s.clone()));
        if let Some((s, _)) = &seat {
            server.clone_from(s);
        } else if online && words.is_empty() && !launcher::lobby::valid_server(&server) && !club_only {
            refuse(t("launcher.msg_bad_address"));
            return;
        }
        if let Some(club) = &self.st.club_server {
            // the club server (run::start_gamesvr reads it): empty = this PC
            let club = club.trim();
            if !club.is_empty() && choices::parse_club_server(club).is_none() {
                refuse(t("launcher.msg_bad_club"));
                return;
            }
            let file = kit.settings_file();
            if let Err(e) = choices::set_setting(&file, "club_server", club) {
                refuse(write_failed(&file, &e));
                return;
            }
        }
        if kit.edition() == Edition::E1718 && !online {
            // the PLAY tab's Universal Cup, which run's start_gamesvr reads as for a host (one settings key for both)
            let file = kit.settings_file();
            if let Err(e) = launcher::conf::save_cup(&file, self.st.lobby.cup) {
                refuse(write_failed(&file, &e));
                return;
            }
        }
        let lang = game_text(kit);
        let path = panel_file(kit);
        let mut panel = match choices::read_ok(&path) {
            Ok(t) => t,
            // a file that cannot be read now would be saved over with the defaults: nothing starts
            Err(e) => {
                refuse(read_failed(&path, &e));
                return;
            }
        };
        panel = choices::set_panel_value(&panel, "play_on", Some(if online { "online" } else { "this_pc" }));
        if !server.is_empty() {
            panel = choices::set_panel_value(&panel, "server", Some(&server));
        }
        panel = choices::set_panel_value(&panel, "ticket", seat.as_ref().map(|(_, t)| t.as_str()));
        // a change only: play.py switches the game's text at the start (english.py), then takes the line out
        let change = (lang != kit.game_language()).then_some(match lang {
            "en" => "on",
            "it" => "it",
            _ => "off",
        });
        panel = choices::set_panel_value(&panel, "english", change);
        if let Err(e) = choices::write(&path, &panel) {
            refuse(write_failed(&path, &e));
            return;
        }
        say(t("launcher.msg_starting"), 0);
        start(vec![Step::Game(words)]);
    }

    /// The board's card pictures: the game's own, or the high-quality pack (data\\settings.json "card_pictures" and
    /// "card_pack"; made at setup and here, launcher::run::make_pictures).  The pack is asked for when none is known
    /// yet, or with "Choose pack".
    fn pictures_pick(&mut self, kit: &Kit, frame: &eframe::Frame, choose: bool) {
        let file = kit.settings_file();
        let hq = self.st.pictures_hq || choose;
        let known = choices::setting(&file, "card_pack").filter(|p| Path::new(p).is_file());
        let pack = if !hq {
            None
        } else if let Some(p) = known.filter(|_| !choose) {
            Some(p)
        } else if let Some(p) = rfd::FileDialog::new()
            .set_title(t("launcher.msg_pack_title"))
            .add_filter("ZIP", &["zip"])
            .set_parent(frame)
            .pick_file()
        {
            Some(p.display().to_string())
        } else {
            // no pack: the choice stays as it was
            self.st.pictures_hq = choices::setting(&file, "card_pictures").as_deref() == Some("hq");
            return;
        };
        self.st.pictures_hq = pack.is_some();
        let mut done = choices::set_setting(&file, "card_pictures", if pack.is_some() { "hq" } else { "game" });
        if let Some(p) = &pack {
            done = done.and_then(|()| choices::set_setting(&file, "card_pack", p));
        }
        match done {
            Ok(()) => {
                start(vec![Step::Pictures]);
            }
            Err(e) => refuse(write_failed(&file, &e)),
        }
    }

    /// 2017-18: the board's card pictures (data\\settings.json "card_pictures", made by launcher::run::make_pictures): REMOVE
    /// goes back to the game's own (the ZIP stays in data\\downloads), DOWNLOAD fetches the pack first.
    fn pictures(&mut self, kit: &Kit) {
        if self.st.pictures_hq {
            let file = kit.settings_file();
            match choices::set_setting(&file, "card_pictures", "game") {
                Ok(()) => {
                    start(vec![Step::Pictures]);
                }
                Err(e) => refuse(write_failed(&file, &e)),
            }
        } else {
            start(vec![Step::Pack, Step::Pictures]);
        }
    }
}

/// The downloaded card pictures are chosen.
/// Whether a question about something on offer is asked now: once, the first time (`asked` remembers it).
fn asks_first(offer: bool, asked: &mut bool) -> bool {
    let ask = offer && !*asked;
    *asked |= ask;
    ask
}

/// A lobby choice that claims a seat or a room (whose countdown starts the run), or opens a hosted arcade (HOST
/// ARCADE, on the internet or the LAN): asked about the club first (#434).
fn claims_a_start(a: &LobbyAction) -> bool {
    matches!(
        a,
        LobbyAction::Join(_)
            | LobbyAction::JoinCode(_)
            | LobbyAction::QuickMatch
            | LobbyAction::HostVs
            | LobbyAction::HostArcade
            | LobbyAction::HostLan
    )
}

fn date_time(unix: i64) -> String {
    launcher::logs::date_time(unix)
}

fn pictures_hq(kit: &Kit) -> bool {
    choices::setting(&kit.settings_file(), "card_pictures").as_deref() == Some("hq")
}

fn write_failed(path: &Path, e: &std::io::Error) -> String {
    tf(
        "launcher.msg_write_failed",
        &[("path", &path.display().to_string()), ("error", &e.to_string())],
    )
}

/// A file the Launcher writes back whole could not be read: it is left as it is.
fn read_failed(path: &Path, e: &std::io::Error) -> String {
    tf(
        "launcher.msg_read_failed",
        &[("path", &path.display().to_string()), ("error", &e.to_string())],
    )
}

/// GitHub's page of every release: VIEW ON GITHUB before the list has come.
const RELEASES_PAGE: &str = "https://github.com/Forza-WCCF/Forza-WCCF/releases";

/// This folder's version as the window shows it ("kit-7.0.0" -> "7.0.0").
fn version(kit: &Kit) -> String {
    kit.release().unwrap_or_else(|| t("launcher.msg_not_known"))
}

/// Gamepad presses since the window last looked, as the lobby's keys (`forza_ui::lobby::pad_key`).
static PAD_KEYS: Mutex<Vec<egui::Key>> = Mutex::new(Vec::new());

/// Reads every controller (winmm, as the keys driver does) every 30 ms; a control newly pressed that the lobby knows
/// becomes a key for the window's next frame.
fn pad_thread() {
    std::thread::spawn(|| {
        let pads = launcher::keys::winmm_pads(Arc::new(log));
        let mut was = std::collections::BTreeSet::new();
        loop {
            let now = pads.poll();
            let new: Vec<egui::Key> = now
                .difference(&was)
                .filter_map(|c| forza_ui::lobby::pad_key(*c))
                .collect();
            if !new.is_empty() {
                PAD_KEYS.lock().unwrap_or_else(PoisonError::into_inner).extend(new);
                poke();
            }
            was = now;
            std::thread::sleep(Duration::from_millis(30));
        }
    });
}

impl eframe::App for App {
    /// The gamepad's keys, only while the window has the focus and the lobby shows.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        let keys = mem::take(&mut *PAD_KEYS.lock().unwrap_or_else(PoisonError::into_inner));
        if !raw.focused || self.st.tab != Tab::Online {
            return;
        }
        for key in keys {
            for pressed in [true, false] {
                raw.events.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.look(ctx);
        if self.st.projector.is_some() && ctx.input(|i| i.events.contains(&egui::Event::WindowFocused(true))) {
            // the in-game SETTINGS may have changed the projector while this window was down
            let s = Kit::here().settings();
            self.st.projector = Some(choices::projector_shown(&s));
            self.st.cpu_championship = Some(choices::cpu_championship_on(&s));
        }
        if self.st.tab == Tab::Update {
            self.notes(&Kit::here());
        }
        // a safety net for anything that changed without a poke
        ctx.request_repaint_after(Duration::from_millis(if self.st.busy { 250 } else { 1000 }));
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // a folder dropped on the window: the game folder
        let dropped = ui.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(p) = dropped.filter(|_| !self.st.busy) {
            self.st.folder = p.display().to_string();
            self.st.tab = Tab::Setup;
        }
        if let Some(a) = screen::show(ui, &mut self.st) {
            let ctx = ui.ctx().clone();
            self.act(a, &ctx, frame);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::NIGHT.to_normalized_gamma_f32()
    }
}

/// What a panic said.
fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into())
}

/// The primary screen's work area (the screen less the taskbar) in points, where the window opens.
fn work_area() -> Option<egui::Rect> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, LOGPIXELSY, ReleaseDC};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETWORKAREA, SystemParametersInfoW};
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: SPI_GETWORKAREA fills the RECT given; the screen DC is released right after its one query
    let dpi = unsafe {
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut r).cast(), 0) == 0 {
            return None;
        }
        let dc = GetDC(ptr::null_mut());
        let dpi = GetDeviceCaps(dc, LOGPIXELSY as _);
        ReleaseDC(ptr::null_mut(), dc);
        dpi
    };
    // per-monitor aware (the manifest): pixels at the system DPI, the primary screen's
    let s = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
    let rect = egui::Rect::from_min_max(
        egui::pos2(r.left as f32, r.top as f32) / s,
        egui::pos2(r.right as f32, r.bottom as f32) / s,
    );
    (rect.height() > 0.0).then_some(rect)
}

/// The window, until it is closed (or gone because the game is up and then ended well).
fn window(kit: &Kit, first: Vec<Step>, folder: Option<&OsStr>) {
    let own = env!("FORZA_VERSION");
    log(&format!("Launcher.exe {own}, files {}", this_kit(kit)));
    if files_differ(own, &this_kit(kit)) {
        say(
            tf(
                "launcher.msg_launcher_differs",
                &[("own", own), ("files", &version(kit))],
            ),
            0,
        );
    }
    let last = std::fs::read_to_string(check_file(kit))
        .ok()
        .and_then(|t| t.trim().parse().ok());
    shared().available = saved_available(kit);
    if check_due(unix_now(), last) {
        ask_updates();
    }
    // a game start given on the command line: this window is there for that run alone
    let run_from_command = starts_a_run(&first);
    if !first.is_empty() {
        start(first);
    }
    pad_thread();
    let title = window_title(kit);
    let fonts = theme::font_dir();
    // OpenGL first, wgpu once if it cannot open the window (as the installer: wgpu flashed black on an RTX 2060 and
    // never opened on a GeForce 920MX laptop). FORZA_RENDERER=glow|wgpu forces one.
    let forced = match std::env::var("FORZA_RENDERER").as_deref() {
        Ok("glow") => Some(eframe::Renderer::Glow),
        Ok("wgpu") => Some(eframe::Renderer::Wgpu),
        _ => None,
    };
    let mut tries = forced.map_or(vec![eframe::Renderer::Glow, eframe::Renderer::Wgpu], |r| vec![r]);
    let work = work_area();
    let size = screen::window_size(work.map(|w| w.height()));
    let mut ran = Ok(());
    while !tries.is_empty() {
        let renderer = tries.remove(0);
        let mut options = eframe::NativeOptions {
            renderer,
            viewport: egui::ViewportBuilder::default()
                .with_title(&title)
                .with_inner_size(size)
                .with_resizable(false)
                .with_maximize_button(false)
                .with_decorations(false)
                .with_drag_and_drop(true)
                .with_icon(
                    eframe::icon_data::from_png_bytes(include_bytes!("../../../../../assets/logo/forza-wccf-256.png"))
                        .expect("the logo is a PNG"),
                ),
            ..Default::default()
        };
        // shortened: centred in the work area, so none of it is under the taskbar or off the screen
        if let Some(w) = work
            && size.y < screen::SIZE.y
        {
            options.viewport = options.viewport.with_position(w.center() - size / 2.0);
        }
        let fonts = &fonts;
        // a renderer that panics (a driver wgpu cannot use) is tried like one that fails: the next one, or the message
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            eframe::run_native(
                &title,
                options,
                Box::new(move |cc| {
                    let _ = CTX.set(cc.egui_ctx.clone());
                    log(&format!("window: {renderer:?} renderer"));
                    let app = App::new(kit, folder, run_from_command);
                    if let Err(e) = theme::install(&cc.egui_ctx, app.st.edition, fonts) {
                        return Err(tf("launcher.msg_no_fonts", &[("error", &e.to_string())]).into());
                    }
                    Ok(Box::new(app))
                }),
            )
        }));
        ran = match run {
            Ok(r) => r,
            Err(p) => {
                let why = panic_text(&*p);
                log(&format!("window: {renderer:?} renderer crashed ({why})"));
                if !tries.is_empty() {
                    continue;
                }
                Err(eframe::Error::AppCreation(
                    tf("launcher.msg_no_window", &[("error", &why)]).into(),
                ))
            }
        };
        match &ran {
            // the window never opened with this renderer: the next one, and the error in the log
            Err(e) if !matches!(e, eframe::Error::AppCreation(_)) && !tries.is_empty() => {
                log(&format!("window: {renderer:?} renderer failed ({e}), trying the next"));
            }
            _ => break,
        }
    }
    if let Err(e) = ran {
        let text = match e {
            eframe::Error::AppCreation(e) => e.to_string(),
            e => tf("launcher.msg_no_window", &[("error", &e.to_string())]),
        };
        message(&text, MB_OK | MB_ICONERROR);
    }
}

// ---- from a console ---------------------------------------------------------------------------------------------

/// A standard handle that a file or a pipe from the parent stands behind (`Launcher.exe status > out.txt`).
fn redirected(which: u32) -> bool {
    // SAFETY: plain Win32 calls on this process's own standard handle.
    unsafe {
        let h = GetStdHandle(which);
        !h.is_null() && h != INVALID_HANDLE_VALUE && matches!(GetFileType(h), FILE_TYPE_DISK | FILE_TYPE_PIPE)
    }
}

/// Started from a console (cmd, PowerShell): its output goes there, unless the parent redirected it to a file or a
/// pipe - then it goes there and the console (if any) only serves Ctrl+C.  A program built as a window has no console of
/// its own, so it joins the one it was started from; false if there is neither a console nor a redirection.
fn attach_console() -> bool {
    let (out, input) = (redirected(STD_OUTPUT_HANDLE), redirected(STD_INPUT_HANDLE));
    // SAFETY: plain Win32 calls; each console handle is opened once and handed to Windows as this process's own.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return out;
        }
        let open = |name: &str| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(name)
                .ok()
                .map(|f| f.into_raw_handle())
        };
        if !out && let Some(h) = open("CONOUT$") {
            SetStdHandle(STD_OUTPUT_HANDLE, h);
            if !redirected(STD_ERROR_HANDLE) {
                SetStdHandle(STD_ERROR_HANDLE, h);
            }
        }
        if !input && let Some(h) = open("CONIN$") {
            SetStdHandle(STD_INPUT_HANDLE, h);
        }
    }
    true
}

/// The kit folder, for the console's Ctrl+C handler.
static CONSOLE_KIT: OnceLock<PathBuf> = OnceLock::new();

/// Ctrl+C, Ctrl+Break or the console closed: the run is asked to stop by the card rule (it says what it does).  A closed
/// console ends this process a few seconds later anyway, and the Job ends the run with it.
unsafe extern "system" fn on_ctrl(event: u32) -> BOOL {
    if let Some(dir) = CONSOLE_KIT.get() {
        run::ask_stop(&Kit::at(dir.clone()));
    }
    i32::from(event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT || event == CTRL_CLOSE_EVENT)
}

/// The console's lines and its question: answered only by someone at a real console keyboard (never by a pipe or a
/// file - those read as no).
struct ConsoleUi;

impl run::Ui for ConsoleUi {
    fn lang(&self) -> Lang {
        text::lang()
    }

    fn say(&self, line: &str) {
        println!("{line}");
    }

    fn ask(&self, question: &str) -> bool {
        let mut mode = 0;
        // SAFETY: this process's own handles; a missing one makes the call fail.  Both must be the console.
        if unsafe {
            GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode) == 0
                || GetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), &mut mode) == 0
        } {
            return false;
        }
        print!("{question} Type Y and press Enter (Enter alone keeps the game running): ");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
    }
}

/// The run in the console Launcher.exe was started from: a start holds the console until the run ends.
fn console(kit: &Kit, args: &[OsString]) -> ExitCode {
    let _ = CONSOLE_KIT.set(kit.dir().to_path_buf());
    // SAFETY: `on_ctrl` is a valid handler for the life of the process.
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1) };
    println!();
    let words: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let rc = run::command(kit, &Params::of(kit), &words, &ConsoleUi);
    ExitCode::from(u8::try_from(rc).unwrap_or(1))
}

/// `Launcher.exe server ARGS`: the online server (`forza-server ARGS`, the 2017-18 kit has no forza-server.exe).
/// `server`, `server lan` and `server debug` stay play.py's words (the HOST mode).
fn server_args(args: &[OsString]) -> Option<&[OsString]> {
    let word = |i: usize, w: &str| args.get(i).is_some_and(|a| a.eq_ignore_ascii_case(w));
    (word(0, "server") && args.len() > 1 && !word(1, "lan") && !word(1, "debug")).then(|| &args[1..])
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    // the run's child that switches QuickEdit off in the server's console (it changes its own console): nothing else
    if args.first().is_some_and(|a| a == launcher::helper::console::ARG) {
        return ExitCode::from(u8::try_from(launcher::helper::console::child_main(&args[1..])).unwrap_or(1));
    }
    if let Some(rest) = server_args(&args) {
        // its log goes to the file the run redirects it to; from a console (--help) it prints there
        if !redirected(STD_OUTPUT_HANDLE) {
            attach_console();
        }
        // background load on the host (builds, disk tools) must not stall the relay and tunnel of a VS match; never
        // HIGH: the game processes run at above normal too (debug::Options::above_normal)
        // SAFETY: plain call on this process's pseudo handle.
        let raised = unsafe { SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS) } != 0;
        println!(
            "{} this server: above-normal priority {}",
            forza_server::stamp(forza_server::now(), false),
            if raised { "yes" } else { "NO" }
        );
        forza_server::cli::main(std::iter::once(OsString::from("Launcher.exe server")).chain(rest.iter().cloned()));
    }
    let kit = Kit::here();
    let first = args.first().map(|a| a.to_string_lossy().to_ascii_lowercase());
    update::tidy(&kit);
    text::choose(choices::setting(&kit.settings_file(), "language").as_deref());
    // a plain start (no arguments): first run, the kit folder gets the installer's name for its edition (link.rs)
    if args.is_empty() {
        match link::first_run(kit.dir(), kit.edition()) {
            link::Outcome::Restart(to) if link::restart(&to) => return ExitCode::SUCCESS,
            // 2017-18 starts its game through ASCII junctions (launcher::alias) and says so itself when it cannot
            link::Outcome::InPlace { warn: Some(msg) } if kit.edition() == Edition::E1011 => {
                log(&msg);
                say(msg, 0);
            }
            _ => {}
        }
    }
    // dropped on Launcher.exe (or given): a game folder to set up
    let folder = args.first().filter(|a| args.len() == 1 && Path::new(a).is_dir());
    let step = match first.as_deref() {
        None if env::var_os(AFTER_UPDATE).is_some() => {
            let folder = choices::setting(&kit.settings_file(), "game").unwrap_or_default();
            if folder.trim().is_empty() {
                Vec::new()
            } else {
                after_update_steps(&kit, folder.trim())
            }
        }
        None => Vec::new(),
        Some(_) if folder.is_some() => vec![
            Step::Setup {
                folder: args[0].clone(),
                undo: false,
            },
            Step::Pictures,
        ],
        // the undo refuses while the game text is changed: the original files go back first (as UNDO SETUP)
        Some("undo") => {
            let mut steps = Vec::new();
            if kit.text_changed() {
                steps.push(Step::Text(""));
            }
            steps.push(Step::Setup {
                folder: args.get(1).cloned().unwrap_or_default(),
                undo: true,
            });
            steps
        }
        Some("restart") => {
            say(t("launcher.msg_restarting"), 0);
            vec![Step::Game(args.clone())]
        }
        Some(_) if attach_console() => return console(&kit, &args),
        Some(_) => vec![Step::Game(args.clone())],
    };
    // one window per kit folder: a second start shows the first and ends; the Launcher an update started waits
    // for the old one to close
    let after_update = env::var_os(AFTER_UPDATE).is_some();
    let window_lock = run::hold_window(&kit, if after_update { 30_000 } else { 0 });
    if window_lock.is_none() && !after_update {
        bring_to_front(&kit);
        return ExitCode::SUCCESS;
    }
    window(&kit, step, folder.map(OsString::as_os_str));
    ExitCode::SUCCESS
}

/// A scratch folder under `%TEMP%` named `wccf-…`, as `launcher::scratch` in the lib: one Defender exclusion of
/// `%TEMP%\wccf-*` covers it.
#[cfg(test)]
fn scratch() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("wccf-").tempdir().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both host starts (HOST ARCADE, HOST LAN ARCADE) clear the desk an earlier run played on right before the run
    /// starts (#419): without it HOST LAN ARCADE served its guests that desk's notice.
    #[test]
    fn each_host_start_clears_the_last_desk() {
        let code = include_str!("main.rs").split("#[cfg(test)]").next().unwrap_or_default();
        let lines: Vec<&str> = code.lines().map(str::trim).collect();
        let starts: Vec<usize> = (0..lines.len())
            .filter(|&i| lines[i].starts_with("start(vec![Step::Host("))
            .collect();
        assert_eq!(starts.len(), 2, "HOST ARCADE and HOST LAN ARCADE");
        for i in starts {
            // the code line before it: blank lines and comments between them do not count
            let before = lines[..i].iter().rev().find(|l| !l.is_empty() && !l.starts_with("//"));
            assert_eq!(before, Some(&"self.online.hosting();"), "before `{}`", lines[i]);
        }
    }

    /// The title bar and the UPDATE tab show `version(kit)`; the panel's bar shows `Kit::release` (`panel` `Files::status`).
    #[test]
    fn the_version_is_the_panels() {
        let dir = scratch();
        std::fs::write(dir.path().join("version.txt"), "kit-0.6.0\n").unwrap();
        let kit = Kit::at(dir.path());
        assert_eq!(version(&kit), "0.6.0");
        assert_eq!(Some(version(&kit)), kit.release());
    }

    /// A session the player started in the window ends on its own screen (2026-11-06: the window closed after a joined
    /// arcade's game ended); one given on the command line ends the window it opened, as a shortcut always did.
    #[test]
    fn the_window_stays_after_a_run_it_started() {
        let closes =
            |from_command: bool, game, failed, was_up| closes_with_run(&mut { from_command }, game, failed, was_up);
        // PLAY or JOIN in the window: the game came up and ended well
        assert!(!closes(false, Some(0), false, true));
        // a shortcut's `play` (no console): the window was opened for that run alone
        assert!(closes(true, Some(0), false, true));
        // the game never came up, or the run went wrong: the window stays with what it said
        assert!(!closes(true, Some(5), true, true));
        assert!(!closes(true, Some(0), false, false));
        // the shortcut's run failed, the window came back: a later PLAY or JOIN that ends well keeps it
        for first in [(Some(5), true), (None, true)] {
            let mut from_command = true;
            assert!(!closes_with_run(&mut from_command, first.0, first.1, true));
            assert!(!closes_with_run(&mut from_command, Some(0), false, true));
        }
        // looks before the shortcut's job ends keep its close
        let mut from_command = true;
        assert!(!closes_with_run(&mut from_command, None, false, false));
        assert!(closes_with_run(&mut from_command, Some(0), false, true));
    }

    /// Only a command line's game start keeps the window for its run: a dropped folder or `undo` are no start.
    #[test]
    fn a_game_start_comes_from_the_command_line() {
        let pics = || vec![Step::Pictures];
        assert!(!starts_a_run(&[]));
        assert!(!starts_a_run(&pics()));
        assert!(!starts_a_run(&[Step::Setup {
            folder: "C:\\game".into(),
            undo: false,
        }]));
        assert!(starts_a_run(&[Step::Game(vec!["play".into()])]));
        assert!(starts_a_run(&[Step::Host(true)]));
    }

    #[test]
    fn play_py_steps() {
        assert_eq!(
            step("  4/6 cabinet (card file data\\save\\seat1_club.bin)"),
            Some((4, 6))
        );
        assert_eq!(step("  1/2 scene service"), Some((1, 2)));
        assert_eq!(step("  6/6 panels loaded"), Some((6, 6)));
        assert_eq!(step("     seat desk: TCP 20030"), None);
        assert_eq!(step("see data\\logs\\run_seat1.txt"), None);
        assert_eq!(step("  7/6 x"), None);
        assert_eq!(step("  3/6"), None);
    }

    #[test]
    fn messages_filled_in_each_language() {
        let _globals = text::TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        for lang in Lang::ALL {
            text::set(lang);
            let s = tf("launcher.msg_write_failed", &[("path", "P"), ("error", "E")]);
            assert!(s.contains('P') && s.contains('E') && !s.contains("%{"), "{s}");
            assert!(
                !t("launcher.msg_done").starts_with("launcher."),
                "{lang:?}: the key, not its text"
            );
        }
        text::set(Lang::En);
    }

    #[test]
    fn server_words_stay_host_mode() {
        let a = |s: &str| s.split(' ').map(OsString::from).collect::<Vec<_>>();
        for host in [
            "server",
            "server lan",
            "Server LAN",
            "server debug",
            "server lan debug",
            "local",
        ] {
            assert!(server_args(&a(host)).is_none(), "{host}");
        }
        let all = a("server --edition 2017-18 all");
        assert_eq!(server_args(&all), Some(&all[1..]));
        assert!(server_args(&a("SERVER --help")).is_some());
    }

    #[test]
    fn a_job_is_not_started_over_a_busy_one() {
        let _globals = text::TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        shared().busy = true;
        assert!(!start(vec![Step::Pictures]), "a busy worker takes no second job");
        shared().busy = false;
    }

    #[test]
    fn a_setup_that_worked_is_ready_even_when_a_later_step_fails() {
        // SET UP: the setup worked, the card pictures after it failed
        assert!(reread_setup(true, false, false));
        // a Setup step that failed: its half-written "game" stays as it was
        assert!(!reread_setup(true, false, true));
        // no job ended
        assert!(!reread_setup(false, false, false));
        assert!(!reread_setup(true, true, false));
    }

    #[test]
    fn after_an_update_the_setup_runs_first_and_the_pictures_last() {
        let steps = steps_after_update(Edition::E1011, "D:\\game", "it");
        assert!(matches!(
            steps.as_slice(),
            [Step::Setup { undo: false, .. }, Step::Text("it"), Step::Pictures]
        ));
        // 2017-18 builds the text at the start; the original Japanese needs none
        let steps = steps_after_update(Edition::E1718, "D:\\game", "it");
        assert!(matches!(steps.as_slice(), [Step::Setup { .. }, Step::Pictures]));
        assert_eq!(steps_after_update(Edition::E1011, "D:\\game", "").len(), 2);
    }

    /// #434: with the arcade's copy on offer the first PLAY asks and starts nothing; the next PLAY starts (the run's
    /// `club::leftover` then keeps the home club, `a_declined_copy_leaves_the_home_club`).
    #[test]
    fn the_first_play_with_an_arcade_copy_on_offer_asks() {
        let mut asked = false;
        assert!(asks_first(true, &mut asked), "the first PLAY asks");
        assert!(!asks_first(true, &mut asked), "the next one starts");
        let mut none = false;
        assert!(!asks_first(false, &mut none) && !none, "nothing on offer: no question");
        let code = include_str!("main.rs").split("#[cfg(test)]").next().unwrap_or_default();
        let play = code.find("fn play(&mut self").unwrap();
        assert!(
            code[play..].find("if self.club_question()").unwrap() < code[play..].find("take_ticket").unwrap(),
            "asked before the ticket is taken"
        );
    }

    /// #434: a coded JOIN (or any lobby choice that claims a seat or a room, or HOST ARCADE) asks before the lobby or the
    /// host sees it, so nothing is claimed or opened while the question is open and `held.played` is never set for a
    /// start the question stops.
    #[test]
    fn a_lobby_start_asks_before_it_claims() {
        for a in [
            LobbyAction::JoinCode("ABCDEF".into()),
            LobbyAction::Join("r1".into()),
            LobbyAction::QuickMatch,
            LobbyAction::HostVs,
            LobbyAction::HostArcade,
            LobbyAction::HostLan,
        ] {
            assert!(claims_a_start(&a), "{a:?}");
        }
        for a in [
            LobbyAction::Ready,
            LobbyAction::Leave,
            LobbyAction::Refresh,
            LobbyAction::Connect,
        ] {
            assert!(!claims_a_start(&a), "{a:?}");
        }
        let code = include_str!("main.rs").split("#[cfg(test)]").next().unwrap_or_default();
        let act = code.find("fn act(&mut self").unwrap();
        let gate = code[act..]
            .find("claims_a_start(&a) && self.club_question()")
            .expect("the gate");
        let lobby = code[act..].find("self.online.act(a,").unwrap();
        assert!(gate < lobby, "asked before the lobby claims");
        // HOST ARCADE (internet or LAN) asks before it opens the room (#434)
        for host in [
            "Action::Lobby(LobbyAction::HostArcade) =>",
            "Action::Lobby(LobbyAction::HostLan) =>",
        ] {
            assert!(gate < code[act..].find(host).unwrap(), "asked before {host}");
        }
    }

    #[test]
    fn a_language_switch_says_when_the_game_text_follows() {
        let _globals = text::TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(
            language_note(Edition::E1718, "it", "en"),
            Some(t("launcher.msg_text_next_play"))
        );
        assert_eq!(
            language_note(Edition::E1718, "it", "it"),
            None,
            "the game has it already"
        );
        assert_eq!(language_note(Edition::E1011, "it", "en"), None);
    }

    /// A failed setup or start names its cause in the status line (#438), not only "See DETAILS".
    #[test]
    fn a_failure_names_its_cause() {
        let _globals = text::TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (plain, why) = ("launcher.msg_start_failed", "launcher.msg_start_failed_why");
        let said = with_cause(plain, why, Some("Port 8080 is taken."));
        assert_eq!(said, tf(why, &[("why", "Port 8080 is taken.")]));
        assert!(
            said.contains("Port 8080 is taken.") && !said.contains("DETAILS"),
            "{said}"
        );
        assert_eq!(with_cause(plain, why, None), t(plain));
        assert_eq!(
            with_cause(plain, why, Some("  ")),
            t(plain),
            "an empty cause says nothing"
        );
    }

    /// "Stop anyway?" reads Yes and No in the Launcher's language (#438), not in the Windows one.
    #[test]
    fn the_stop_question_answers_in_the_launchers_language() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetDlgItemTextW, PostMessageW, WM_COMMAND};
        let title = format!("wccf-ask-test-{}", std::process::id());
        let asked = {
            let title = title.clone();
            thread::spawn(move || ask_box("Stop anyway?", &title, Lang::It))
        };
        let (class, name) = (wide("#32770"), wide(&title));
        let want = ui_text(Lang::It, "launcher.ask_yes", &[]);
        let deadline = Instant::now() + Duration::from_secs(10);
        let label = |dlg, id| {
            let mut buf = [0u16; 64];
            // SAFETY: a buffer of its stated length; the box is up until the answer below.
            let n = unsafe { GetDlgItemTextW(dlg, id, buf.as_mut_ptr(), 64) };
            String::from_utf16_lossy(&buf[..n as usize])
        };
        let (yes, no) = loop {
            // SAFETY: NUL-terminated strings.
            let dlg = unsafe { FindWindowW(class.as_ptr(), name.as_ptr()) };
            if !dlg.is_null() {
                // the hook relabels at activation: wait for it
                let yes = label(dlg, IDYES);
                if yes == want || Instant::now() > deadline {
                    let no = label(dlg, IDNO);
                    // SAFETY: a plain message to the box; IDNO answers it.
                    unsafe { PostMessageW(dlg, WM_COMMAND, IDNO as usize, 0) };
                    break (yes, no);
                }
            }
            assert!(Instant::now() < deadline, "the question's box never opened");
            thread::sleep(Duration::from_millis(20));
        };
        assert!(!asked.join().unwrap(), "No is no");
        assert_eq!(yes, want);
        assert_eq!(no, ui_text(Lang::It, "launcher.ask_no", &[]));
    }

    #[test]
    fn game_text_follows_language() {
        assert_eq!(game_text_for(Lang::En, None), "en");
        assert_eq!(game_text_for(Lang::It, None), "it");
        assert_eq!(game_text_for(Lang::Ja, None), "");
        assert_eq!(game_text_for(Lang::En, Some("original")), "");
        assert_eq!(game_text_for(Lang::Ja, Some("en")), "en");
    }
}
