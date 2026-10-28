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
//!   HOST SHOP    (2017-18, unless "shop_1718": false) this PC's shop for players elsewhere: the server, its tunnel and its room
//!                in the lobby (the code shown here), then this PC's own seat; the shop closes once that seat's run has ended and every guest has left.
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
mod text;
mod update;

use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::os::windows::io::IntoRawHandle;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use std::{env, mem, ptr, thread};

use eframe::egui;
use egui::ViewportCommand;
use forza_ui::launcher::{self as screen, Action, Confirm, LauncherState, Tab};
use forza_ui::lobby::LobbyAction;
use forza_ui::theme;
use forza_ui::widgets::Tone;
use forza_ui::{Lang, text as ui_text};
use launcher::release::{check_due, files_differ, is_newer, parse};
use launcher::run::{self, Params};
use launcher::{Edition, Kit, choices};
use text::{t, tf};
use update::Release;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, GetConsoleMode,
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetStdHandle,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW,
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
    /// HOST SHOP (2017-18): the server with its shop room, then this PC's seat on it; the server ends after the seat, once
    /// no guest is connected. `true`: HOST LAN SHOP, the server for this network only (`server lan`), no room
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
    began: Option<Instant>,    // the job's start
    stage_at: Option<Instant>, // the last step line
    log: Vec<String>,          // lines for OUTPUT, not shown yet
    busy: bool,                // a job runs
    updated: bool,             // an update has just finished: setup runs again
    failed: bool,              // the last job failed: OUTPUT opens, so its output shows
    game: Option<u8>,          // a Game step has just ended with this exit code
    report_open: Option<bool>, // open the report dialog: Some(after a crash)
    report_done: Option<Result<(String, String), String>>, // where the report was saved and its issue link, or why not
    // HOST SHOP: the shop room's code, its lobby server and the flag set when that lobby loses it
    host_room: Option<(String, String, Arc<AtomicBool>)>,
    // the LAN fold: the shops the last search found (forza::online::lan)
    lan_found: Option<Vec<forza::online::lan::Found>>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    releases: Vec::new(),
    listed: false,
    status: None,
    update_note: None,
    available: None,
    progress: 0,
    steps: None,
    began: None,
    stage_at: None,
    log: Vec::new(),
    busy: false,
    updated: false,
    failed: false,
    game: None,
    report_open: None,
    report_done: None,
    host_room: None,
    lan_found: None,
});

/// The game is up in this process (its run is supervised here): the window is minimized, closing it asks the run to stop.
static UP: AtomicBool = AtomicBool::new(false);
/// The game came up during the current job: when the run then ends well, Launcher.exe ends with it.
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
        _ => Err(t("launcher.msg_failed")),
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
        s.began = Some(Instant::now());
        s.stage_at = s.began;
    }
    poke();
    thread::spawn(move || {
        let kit = Kit::here();
        let mut result = Ok(());
        let mut updated = None;
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
                Step::Pictures => {
                    say(t("launcher.msg_making_pictures"), 0);
                    run::make_pictures(&kit, &Params::of(&kit), &WindowUi)
                        .then_some(())
                        .ok_or_else(|| t("launcher.msg_failed"))
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
                Step::Host(lan) => host_shop(&kit, *lan).map(|rc| {
                    game = Some(u8::try_from(rc).unwrap_or(1));
                    if rc == run::WENT_WRONG {
                        shared().report_open = Some(true);
                    }
                }),
            };
            if result.is_err() {
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
            (Ok(()), None, Some(rc)) if rc != 0 => t("launcher.msg_start_failed"),
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
            s.game = game;
            s.busy = false;
        }
        poke();
    });
    true
}

/// HOST SHOP: the server on a thread of its own until its shop room is made (or not), then this PC's seat on it as a
/// `remote 127.0.0.1` run would (first, so it gets the projector); when that run ends, the server is asked to stop, which
/// it does once no guest is connected; that closes the room. The seat's exit code, or why there is no room.
/// `lan`: HOST LAN SHOP, `server lan` (no tunnel, no room, found by LAN searches).
fn host_shop(kit: &Kit, lan: bool) -> Result<i32, String> {
    log(if lan {
        "> server lan, then remote 127.0.0.1 (HOST LAN SHOP)"
    } else {
        "> server, then remote 127.0.0.1 (HOST SHOP)"
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let kit = Kit::here();
        let p = Params::of(&kit);
        let mode = if lan { run::Mode::LanServer } else { run::Mode::Server };
        let Ok(r) = run::start(&kit, &p, mode, false, &ServerUi) else {
            return; // what went wrong is in OUTPUT
        };
        let _ = tx.send((if lan { Ok(None) } else { r.shop_code().map(Some) }, r.shop_guests()));
        let _ = r.supervise();
    });
    let stop = |server: thread::JoinHandle<()>| {
        let _ = run::request(kit, run::Kind::Server, "force");
        let _ = server.join();
    };
    match rx.recv() {
        Ok((Ok(room), guests)) => {
            // shortcut: a LAN shop counts no guests (they come by plain address, not the tunnel), so it closes once
            // this PC's seat has ended; count the desk's seats if LAN guests are cut off in practice
            shared().host_room = room;
            poke();
            let mode = run::Mode::Remote("127.0.0.1".into(), None);
            let rc = run::play(kit, &Params::of(kit), Some(mode), false, &WindowUi);
            // a plain stop: the shop stays while guests are connected (run::rules::shop_rule), until they have all left
            // (asked again then) or STOP THE GAME and "stop anyway"
            if let Some((code, why)) = run::request(kit, run::Kind::Server, "stop")
                && code != run::OK
            {
                log(&why);
                say(t("launcher.msg_shop_stays"), 0);
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
            Err(tf("launcher.msg_no_shop_room", &[("error", &why)]))
        }
        Err(_) => {
            let _ = server.join();
            Ok(run::FAILED)
        }
    }
}

/// The HOST SHOP server's lines: into OUTPUT and the status line (its start, a wait for the lobby), never the bar or
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

    fn say(&self, line: &str) {
        self.say_logged(line, line);
    }

    fn say_logged(&self, shown: &str, en: &str) {
        stepped(shown);
        log_pair(shown, en);
    }

    fn ask(&self, _question: &str) -> bool {
        // no owner (any thread may ask), kept in front of the game
        message(
            &t(if Kit::here().edition() == Edition::E1718 {
                "launcher.msg_stop_anyway_1718"
            } else {
                "launcher.msg_stop_anyway"
            }),
            MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
        ) == IDYES
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
    match forza::online::save_logs::save(&dir, kit, game, &forza::online::save_logs::Scrub::from_env(), None) {
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

/// A Windows message box with no owner; its answer.
fn message(text: &str, style: u32) -> i32 {
    let (text, title) = (wide(text), wide(TITLE));
    // SAFETY: NUL-terminated strings that live across the call; no owner window.
    unsafe { MessageBoxW(ptr::null_mut(), text.as_ptr(), title.as_ptr(), style) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// STOP THE GAME (clicked twice), or this window closed while the game runs: the running game is asked to stop by the
/// card rule, on a thread of its own - the run itself may be this process's job.
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
        if found.is_ok() {
            let _ = std::fs::write(check_file(&kit), unix_now().to_string());
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
    online: online::Online,
    /// When the LAN fold's last search started.
    lan_at: Option<Instant>,
}

/// The LAN fold searches this often while it is open.
const LAN_EVERY: Duration = Duration::from_secs(4);

impl App {
    /// The state as the kit has it now; `folder` given (dropped on Launcher.exe) opens SETUP.
    fn new(kit: &Kit, folder: Option<&OsStr>) -> Self {
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
        let set_up = !game.is_empty();
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
        lobby.shop_rooms = ed == Edition::E1718 && run::rules::shop_1718_on(&kit.settings());
        lobby.list_shop = kit.settings()["list_shop"].as_bool() == Some(true);
        let st = LauncherState {
            edition: ed,
            lang: text::lang(),
            online,
            lobby,
            rejoin: None,
            set_up,
            running: RUNNING.load(Ordering::Relaxed),
            busy: false,
            steps: None,
            clock: (0.0, 0.0),
            progress: 0.0,
            status: None,
            tab: if !set_up || folder.is_some() {
                Tab::Setup
            } else if online {
                Tab::Online
            } else {
                Tab::Play
            },
            folder: game,
            pictures_hq: choices::setting(&settings, "card_pictures").as_deref() == Some("hq"),
            skip_tutorial: (kit.edition() == Edition::E1718)
                .then(|| kit.settings()["skip_tutorial"].as_bool() == Some(true)),
            club_server: (kit.edition() == Edition::E1718)
                .then(|| choices::setting(&settings, "club_server").unwrap_or_default()),
            vs_link: (kit.edition() == Edition::E1718).then(|| kit.settings()["vs_link"].as_bool() != Some(false)),
            updates: true,
            version: version(kit),
            update_available: saved_available(kit),
            update_status: None,
            releases: Vec::new(),
            selected: None,
            output: Vec::new(),
            confirm: None,
            report: None,
            stop_armed: 0.0,
        };
        App {
            st,
            releases: Vec::new(),
            online: online_state,
            lan_at: None,
        }
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
                newer: !r.test && is_newer(&r.tag, &installed),
            })
            .collect();
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

    /// What the workers said since the last look; the window minimized for the game, back when its run ended badly,
    /// closed when it ended well.  Runs also while the window is minimized.
    fn look(&mut self, ctx: &egui::Context) {
        let (log, list, updated, failed, game, report_open, report_done) = {
            let mut s = shared();
            self.st.status.clone_from(&s.status);
            self.st.update_status.clone_from(&s.update_note);
            self.st.update_available.clone_from(&s.available);
            self.st.progress = s.progress as f32 / 1000.0;
            self.st.steps = s.steps.map(|(n, m)| (n as usize, m as usize));
            self.st.clock = s.began.zip(s.stage_at).map_or((0.0, 0.0), |(b, a)| {
                (a.elapsed().as_secs_f32(), b.elapsed().as_secs_f32())
            });
            self.st.busy = s.busy;
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
        self.st.output.extend(log);
        let kit = Kit::here();
        // HOST SHOP: its code once the lobby gave it, gone with the room if the lobby loses it; the job over, the shop is
        let room = shared().host_room.clone();
        let v = &mut self.st.lobby;
        v.host_code = None;
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
        // the VS room's countdown ran out, or a shop code got this PC a seat: the run starts
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
        // the run this process held has ended well: Launcher.exe ends with it
        if game == Some(0) && WAS_UP.load(Ordering::Relaxed) {
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
            self.st.selected = Some(list.iter().position(|r| !r.test).unwrap_or(0));
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
        // closing Launcher while the game runs is a quit: asked of the run, by the card rule; and never a kit left
        // half copied, nor a start half done: the window stays while a job runs
        if ctx.input(|i| i.viewport().close_requested()) {
            if UP.load(Ordering::Relaxed) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                stop_game();
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
            Action::Pictures { choose } => self.pictures(&kit, frame, choose),
            Action::SkipTutorial => {
                let file = kit.settings_file();
                let on = self.st.skip_tutorial == Some(true);
                if let Err(e) = choices::set_setting_bool(&file, "skip_tutorial", on) {
                    refuse(write_failed(&file, &e));
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
                    self.ask(&question, "launcher.update_button", Action::UpdateNow(i));
                }
            }
            Action::UpdateNow(i) => {
                if let Some(r) = self.releases.get(i) {
                    start(vec![Step::Update(r.clone())]);
                }
            }
            Action::WhatsNew(i) => {
                if let Some(r) = self.releases.get(i) {
                    ctx.open_url(egui::OpenUrl::new_tab(&r.page));
                }
            }
            Action::Lobby(LobbyAction::HostShop) => {
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
                    self.st.lobby.hosting = true;
                    self.st.lobby.notice = None;
                    start(vec![Step::Host(false)]);
                }
            }
            Action::Lobby(LobbyAction::ListShop(on)) => {
                // data\settings.json "list_shop" (default off), which run::open_shop_room reads
                let file = kit.settings_file();
                if let Err(e) = choices::set_setting_bool(&file, "list_shop", on) {
                    refuse(write_failed(&file, &e));
                }
            }
            Action::Lobby(LobbyAction::HostLan) => {
                if !self.st.busy && !self.st.running {
                    let ips = forza::online::lan::addresses()
                        .iter()
                        .map(ToString::to_string)
                        .collect();
                    self.st.lobby.host_lan = Some(ips);
                    self.st.lobby.hosting = true;
                    self.st.lobby.notice = None;
                    start(vec![Step::Host(true)]);
                }
            }
            Action::Lobby(LobbyAction::JoinLan(address)) if !self.st.busy && !self.st.running => {
                if launcher::lobby::valid_server(&address) {
                    self.play(&kit, vec!["remote".into(), address.into()]);
                } else {
                    refuse(t("launcher.msg_bad_address"));
                }
            }
            // LEAVE while hosting: the seat stops by the card rule; the server (and the room) after it, never under its
            // match (run::rules::stop_server_too, and host_shop's stop once the seat's run has ended)
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
                .map(|f| forza_ui::lobby::LanShop {
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
    /// a shop room's `remote ID`).
    fn play(&mut self, kit: &Kit, words: Vec<OsString>) {
        let online = self.st.online;
        let mut server = self.st.lobby.address.trim().to_owned();
        // 2017-18: a club server alone is enough to play online (run::rules::vs_target)
        let club_only = self.st.club_server.as_ref().is_some_and(|c| !c.trim().is_empty());
        // a seat or VS room the lobby reserved: its server and ticket; else the Direct address, as before the lobby
        // (2017-18: a VS room's only, crates/server/README.md "VS rooms")
        let vs_only = kit.edition() == Edition::E1718;
        let seat = online.then(|| self.online.take_ticket(vs_only)).flatten();
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
        let lang = game_text(kit);
        let path = panel_file(kit);
        let mut panel = choices::read(&path);
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
    fn pictures(&mut self, kit: &Kit, frame: &eframe::Frame, choose: bool) {
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
}

fn write_failed(path: &Path, e: &std::io::Error) -> String {
    tf(
        "launcher.msg_write_failed",
        &[("path", &path.display().to_string()), ("error", &e.to_string())],
    )
}

/// This folder's version as the window shows it ("kit-7.0.0" -> "7.0.0").
fn version(kit: &Kit) -> String {
    let v = this_kit(kit);
    v.strip_prefix("kit-").map(str::to_owned).unwrap_or(v)
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

/// The window, until it is closed (or gone because the game is up and then ended well).
fn window(kit: &Kit, first: Vec<Step>, folder: Option<&OsStr>) {
    let own = env!("CARGO_PKG_VERSION");
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
    if !first.is_empty() {
        start(first);
    }
    pad_thread();
    let title = format!("Launcher - {TITLE} {}", kit.edition().name());
    let fonts = theme::font_dir();
    // OpenGL first, wgpu once if it cannot open the window (as the installer: wgpu flashed black on an RTX 2060 and
    // never opened on a GeForce 920MX laptop). FORZA_RENDERER=glow|wgpu forces one.
    let forced = match std::env::var("FORZA_RENDERER").as_deref() {
        Ok("glow") => Some(eframe::Renderer::Glow),
        Ok("wgpu") => Some(eframe::Renderer::Wgpu),
        _ => None,
    };
    let mut tries = forced.map_or(vec![eframe::Renderer::Glow, eframe::Renderer::Wgpu], |r| vec![r]);
    let mut ran = Ok(());
    while !tries.is_empty() {
        let renderer = tries.remove(0);
        let options = eframe::NativeOptions {
            renderer,
            viewport: egui::ViewportBuilder::default()
                .with_title(&title)
                .with_inner_size(screen::SIZE)
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
        let fonts = &fonts;
        // a renderer that panics (a driver wgpu cannot use) is tried like one that fails: the next one, or the message
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            eframe::run_native(
                &title,
                options,
                Box::new(move |cc| {
                    let _ = CTX.set(cc.egui_ctx.clone());
                    log(&format!("window: {renderer:?} renderer"));
                    let app = App::new(kit, folder);
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
        print!("{question}");
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

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    // the run's child that switches QuickEdit off in the server's console (it changes its own console): nothing else
    if args.first().is_some_and(|a| a == launcher::helper::console::ARG) {
        return ExitCode::from(u8::try_from(launcher::helper::console::child_main(&args[1..])).unwrap_or(1));
    }
    let kit = Kit::here();
    let first = args.first().map(|a| a.to_string_lossy().to_ascii_lowercase());
    update::tidy(&kit);
    text::choose(choices::setting(&kit.settings_file(), "language").as_deref());
    // a plain start (no arguments): first run, the kit folder gets the installer's name for its edition (link.rs)
    if args.is_empty() {
        match link::first_run(kit.dir(), kit.edition()) {
            link::Outcome::Restart(to) if link::restart(&to) => return ExitCode::SUCCESS,
            link::Outcome::InPlace { warn: Some(msg) } => {
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
    window(&kit, step, folder.map(OsString::as_os_str));
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_job_is_not_started_over_a_busy_one() {
        shared().busy = true;
        assert!(!start(vec![Step::Pictures]), "a busy worker takes no second job");
        shared().busy = false;
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

    #[test]
    fn game_text_follows_language() {
        assert_eq!(game_text_for(Lang::En, None), "en");
        assert_eq!(game_text_for(Lang::It, None), "it");
        assert_eq!(game_text_for(Lang::Ja, None), "");
        assert_eq!(game_text_for(Lang::En, Some("original")), "");
        assert_eq!(game_text_for(Lang::Ja, Some("en")), "en");
    }
}
