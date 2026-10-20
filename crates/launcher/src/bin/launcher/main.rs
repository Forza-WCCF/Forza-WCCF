//! Launcher.exe - the kit's one program, a window (2026-10-10; before, a console that ran play.py, and SETUP.exe beside
//! it for the rest).  Nothing it starts shows a console: the game's two windows are all a player sees.  The window is
//! eframe drawing forza's launcher screen (forza::ui::launcher, step 9.4): this file fills its state from the kit and
//! does what the player asks for there.
//!   PLAY         play on THIS PC or ONLINE on a server, the window's language, the game's text (English,
//!                Italian or Japanese) - then this process runs the game (launcher::run) and holds every process of it
//!                in one Job until it ends.  Once the game is up this window is minimized; closing a game window ends
//!                the run, and then Launcher.exe ends too.  Closing this window (or STOP THE GAME) asks the run to stop
//!                by the card rule: with the club card in or a match on, it asks first.  Launcher.exe opened again
//!                while the game runs offers STOP THE GAME, which asks the running Launcher.exe.
//!   SETUP        the game folder: set up / repair (scripts\setup.py GAME) and undo; it is typed, picked, dropped on
//!                the window or on Launcher.exe, and remembered by setup.py in data\settings.json.  The card pictures
//!                (the game's or the high-quality pack) and, for 2017-18, skip the tutorial.
//!   UPDATE       (update.rs, 2010-11 only): the kit's releases and test builds at UPDATE_URL.
//! The choices go where the kit already keeps them (choices.rs): data\panel.txt, which the in-game SETTINGS panel
//! writes too, and data\settings.json.  The scripts' output fills OUTPUT.
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
use std::io::{BufRead, BufReader, Read};
use std::os::windows::io::IntoRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;
use std::{env, mem, ptr, thread};

use eframe::egui;
use egui::ViewportCommand;
use forza::ui::launcher::{self as screen, Action, Confirm, LauncherState};
use forza::ui::theme;
use forza::ui::widgets::Tone;
use forza::ui::{Lang, text as ui_text};
use launcher::release::is_newer;
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
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW,
};
use windows_sys::core::BOOL;

/// The message boxes' title (the window's own title bar draws the name itself).
const TITLE: &str = "Forza-WCCF";

enum Step {
    Script(&'static str, Vec<OsString>),
    Update(Release),
    /// The run (launcher::run) with play.py's words; its exit code is the result
    Game(Vec<OsString>),
    /// The card pictures (launcher::run::make_pictures), after setup and when the pictures choice changes
    Pictures,
}

/// What the worker threads tell the window; each change wakes it (`poke`).
struct Shared {
    releases: Vec<Release>,
    listed: bool, // `releases` is new: the list gets it
    status: Option<(Tone, String)>,
    update_note: Option<(Tone, String)>,
    progress: u32,             // of 1000
    steps: Option<(u32, u32)>, // the run's "N/M" step lines: N of M
    log: Vec<String>,          // lines for OUTPUT, not shown yet
    busy: bool,                // a job runs
    updated: bool,             // an update has just finished: offer setup
    failed: bool,              // the last job failed: OUTPUT opens, so its output shows
    game: Option<u8>,          // a Game step has just ended with this exit code
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    releases: Vec::new(),
    listed: false,
    status: None,
    update_note: None,
    progress: 0,
    steps: None,
    log: Vec::new(),
    busy: false,
    updated: false,
    failed: false,
    game: None,
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
    shared().log.push(line.to_owned());
    poke();
}

// ---- the kit's scripts ------------------------------------------------------------------------------------------

/// A step line moves the bar: never back.
fn stepped(line: &str) {
    if let Some((n, m)) = step(line) {
        let mut s = shared();
        s.progress = s.progress.max(n * 1000 / m);
        s.steps = Some(s.steps.filter(|&(d, t)| t == m && d >= n).unwrap_or((n, m)));
    }
}

/// python\python.exe code\<script> args without a console, its output line by line into the window; its exit
/// code.  play.py's "N/M" step lines move the progress bar.
fn run_script(kit: &Kit, script: &str, args: &[OsString]) -> Result<i32, String> {
    let shown: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    log(&format!("> {script} {}", shown.join(" ")));
    let mut child = kit
        .script(script)
        .args(args)
        .current_dir(kit.dir())
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("python\\python.exe: {e}"))?;
    let readers: Vec<_> = [
        child.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>),
        child.stderr.take().map(|e| Box::new(e) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|pipe| {
        thread::spawn(move || {
            let mut pipe = BufReader::new(pipe);
            let mut line = Vec::new();
            while pipe.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
                let text = String::from_utf8_lossy(&line);
                stepped(&text);
                log(text.trim_end());
                line.clear();
            }
        })
    })
    .collect();
    let status = child.wait().map_err(|e| e.to_string())?;
    for r in readers {
        let _ = r.join();
    }
    Ok(status.code().unwrap_or(1))
}

/// "  4/6 cabinet stand-ins ..." -> 4 of 6.
fn step(line: &str) -> Option<(u32, u32)> {
    let (n, rest) = line.trim_start().split_once('/')?;
    let m: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let (n, m) = (n.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (n <= m && m > 0 && rest[m.to_string().len()..].starts_with(' ')).then_some((n, m))
}

/// english.py's arguments that put `lang` ("en", "it" or "" for the original Japanese) in place.
fn language_args(lang: &str) -> Vec<OsString> {
    match lang {
        "it" => vec!["on".into(), "it".into()],
        "en" => vec!["on".into()],
        _ => vec!["off".into()],
    }
}

/// The game's text: the screen's place (English, Italian, the original Japanese) <-> english.py's code.
const GAME_TEXT: [&str; 3] = ["en", "it", ""];

fn game_text_of(code: &str) -> usize {
    GAME_TEXT.iter().position(|&c| c == code).unwrap_or(2)
}

fn panel_file(kit: &Kit) -> PathBuf {
    kit.data().join("panel.txt")
}

fn this_kit(kit: &Kit) -> String {
    kit.version().unwrap_or_else(|| t("launcher.msg_not_known"))
}

/// The steps one by one in a worker; the first that fails ends the job.
fn start(steps: Vec<Step>) {
    {
        let mut s = shared();
        if s.busy {
            return;
        }
        WAS_UP.store(false, Ordering::Relaxed);
        s.busy = true;
        s.progress = 0;
        s.steps = None;
    }
    poke();
    thread::spawn(move || {
        let kit = Kit::here();
        let mut result = Ok(());
        let mut updated = None;
        let mut game = None;
        for step in steps {
            result = match &step {
                Step::Script(script, args) => {
                    say(format!("{script} ..."), 0);
                    run_script(&kit, script, args)
                        .and_then(|rc| if rc == 0 { Ok(()) } else { Err(t("launcher.msg_failed")) })
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
                    Ok(())
                }
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
            (Ok(()), None, Some(rc)) if rc != 0 => t("launcher.msg_start_failed"),
            (Ok(()), None, _) => t("launcher.msg_done"),
        };
        log(&status);
        log("");
        {
            let mut s = shared();
            s.status = Some((if failed { Tone::Refused } else { Tone::Done }, status));
            s.progress = if failed { 0 } else { 1000 };
            s.updated = updated.is_some();
            s.failed = failed;
            s.game = game;
            s.busy = false;
        }
        poke();
    });
}

/// The run's lines go to OUTPUT (its "N/M" steps move the bar); its question - stop anyway, after the card rule
/// refused a stop - is asked in a message box in front of the game; the game up minimizes the window.
struct WindowUi;

impl run::Ui for WindowUi {
    fn say(&self, line: &str) {
        stepped(line);
        log(line);
    }

    fn ask(&self, _question: &str) -> bool {
        // no owner (any thread may ask), kept in front of the game
        message(
            &t("launcher.msg_stop_anyway"),
            MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
        ) == IDYES
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

/// The release list, asked in the background when the window opens.
fn ask_updates() {
    shared().update_note = Some((Tone::Notice, t("launcher.msg_asking")));
    thread::spawn(|| {
        let found = update::fetch();
        let installed = this_kit(&Kit::here());
        {
            let mut s = shared();
            match found {
                Ok(list) if !list.is_empty() => {
                    if let Some(r) = list.iter().find(|r| !r.test && is_newer(&r.tag, &installed)) {
                        if !s.busy {
                            s.status = Some((Tone::Notice, tf("launcher.msg_available", &[("tag", &r.tag)])));
                        }
                        s.update_note = None;
                    } else {
                        s.update_note = Some((Tone::Notice, t("launcher.msg_pick")));
                    }
                    s.releases = list;
                    s.listed = true;
                }
                Ok(_) => s.update_note = Some((Tone::Done, t("launcher.msg_none_listed"))),
                Err(e) => s.update_note = Some((Tone::Refused, tf("launcher.msg_no_answer", &[("error", &e)]))),
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
}

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
        let game_text = match choices::panel_value(&panel, "english").as_deref() {
            Some("on") => "en",
            Some("it") => "it",
            Some("off") => "",
            _ => kit.game_language(),
        };
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
        let online_state = online::Online::new(&settings, &address);
        let mut lobby = screen::lobby_for(ed);
        lobby.address = address;
        let st = LauncherState {
            edition: ed,
            lang: text::lang(),
            online,
            lobby,
            rejoin: None,
            game_text: game_text_of(game_text),
            set_up,
            running: RUNNING.load(Ordering::Relaxed),
            busy: false,
            steps: None,
            progress: 0.0,
            status: None,
            setup_open: !set_up || folder.is_some(),
            folder: game,
            pictures_hq: choices::setting(&settings, "card_pictures").as_deref() == Some("hq"),
            skip_tutorial: (kit.edition() == Edition::E1718)
                .then(|| kit.settings()["skip_tutorial"].as_bool() == Some(true)),
            club_server: (kit.edition() == Edition::E1718)
                .then(|| choices::setting(&settings, "club_server").unwrap_or_default()),
            vs_link: (kit.edition() == Edition::E1718).then(|| kit.settings()["vs_link"].as_bool() != Some(false)),
            updates: kit.edition() == Edition::E1011,
            version: version(kit),
            update_open: false,
            update_status: None,
            releases: Vec::new(),
            selected: None,
            output_open: false,
            output: Vec::new(),
            confirm: None,
            stop_armed: 0.0,
        };
        App {
            st,
            releases: Vec::new(),
            online: online_state,
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
                this: r.tag == installed,
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
            self.st.setup_open = true;
            refuse(t("launcher.msg_no_folder"));
            return None;
        }
        Some(f.into())
    }

    /// What the workers said since the last look; the window minimized for the game, back when its run ended badly,
    /// closed when it ended well.  Runs also while the window is minimized.
    fn look(&mut self, ctx: &egui::Context) {
        let (log, list, updated, failed, game) = {
            let mut s = shared();
            self.st.status.clone_from(&s.status);
            self.st.update_status.clone_from(&s.update_note);
            self.st.progress = s.progress as f32 / 1000.0;
            self.st.steps = s.steps.map(|(n, m)| (n as usize, m as usize));
            self.st.busy = s.busy;
            (
                mem::take(&mut s.log),
                mem::take(&mut s.listed).then(|| s.releases.clone()),
                mem::take(&mut s.updated),
                mem::take(&mut s.failed),
                s.game.take(),
            )
        };
        self.st.running = RUNNING.load(Ordering::Relaxed);
        self.st.output.extend(log);
        let kit = Kit::here();
        // the VS room's countdown ran out: the run starts as PLAY would
        if self
            .online
            .look(&mut self.st, kit.edition().name(), &kit.settings_file())
            && !self.st.busy
            && !self.st.running
        {
            self.play(&kit);
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
            self.st.output_open = true;
        }
        if let Some(list) = list {
            self.st.selected = Some(list.iter().position(|r| !r.test).unwrap_or(0));
            self.releases = list;
            self.list(&kit);
        }
        if updated {
            self.st.version = version(&kit);
            self.list(&kit); // the "this version" mark moves
            let question = match kit.game_language() {
                "en" => t("launcher.msg_after_update_english"),
                "it" => t("launcher.msg_after_update_italian"),
                _ => t("launcher.msg_after_update"),
            };
            if !self.st.folder.trim().is_empty() {
                self.ask(&question, "launcher.set_up", Action::SetUpAfterUpdate);
            }
        }
        // closing Launcher while the game runs is a quit: asked of the run, by the card rule; and never a kit left
        // half copied, nor a start half done: the window stays while a job runs
        if ctx.input(|i| i.viewport().close_requested()) {
            if UP.load(Ordering::Relaxed) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                stop_game();
            } else if self.st.busy {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                say(t("launcher.msg_busy"), shared().progress);
            }
        }
    }

    /// What the player asked for in the screen.
    fn act(&mut self, a: Action, ctx: &egui::Context, frame: &eframe::Frame) {
        let kit = Kit::here();
        match a {
            Action::Play => self.play(&kit),
            Action::Stop => stop_game(),
            Action::SetUp => {
                if let Some(game) = self.folder() {
                    start(vec![Step::Script("setup.py", vec![game]), Step::Pictures]);
                }
            }
            Action::Undo => {
                if self.folder().is_some() {
                    self.ask(&t("launcher.msg_undo_confirm"), "launcher.undo", Action::UndoNow);
                }
            }
            Action::UndoNow => {
                if let Some(game) = self.folder() {
                    // setup.py refuses an undo while English is on: the original files go back first
                    let mut steps = Vec::new();
                    if kit.english_on() {
                        steps.push(Step::Script("english.py", vec!["off".into()]));
                    }
                    steps.push(Step::Script("setup.py", vec!["undo".into(), game]));
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
            Action::SetUpAfterUpdate => {
                let lang = kit.game_language();
                let mut steps = vec![Step::Script("setup.py", vec![self.st.folder.trim().into()])];
                if !lang.is_empty() {
                    steps.push(Step::Script("english.py", language_args(lang)));
                }
                steps.push(Step::Pictures); // last: a failure here must not skip english.py
                start(steps);
            }
            Action::Lobby(a) => self.online.act(a, &mut self.st, &kit.settings_file()),
            Action::Rejoin => self.online.rejoin(),
            Action::Language => {
                let lang = self.st.lang;
                text::set(lang);
                let _ = choices::set_setting(&kit.settings_file(), "language", lang.code());
                // the game's text follows: Japanese for 日本語, Italian for Italiano, else English (still a choice of
                // its own)
                self.st.game_text = match lang {
                    Lang::Ja => 2,
                    Lang::It => 1,
                    Lang::En => 0,
                };
            }
        }
    }

    /// PLAY: the choices into data\panel.txt, then the run.
    fn play(&mut self, kit: &Kit) {
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
        } else if online && !choices::valid_address(&server) && !club_only {
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
        let lang = GAME_TEXT[self.st.game_text.min(2)];
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
        start(vec![Step::Game(Vec::new())]);
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
            Ok(()) => start(vec![Step::Pictures]),
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

/// Gamepad presses since the window last looked, as the lobby's keys (`forza::ui::lobby::pad_key`).
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
                .filter_map(|c| forza::ui::lobby::pad_key(*c))
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
        if !raw.focused || !self.st.online {
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
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // a folder dropped on the window: the game folder
        let dropped = ui.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(p) = dropped.filter(|_| !self.st.busy) {
            self.st.folder = p.display().to_string();
            self.st.setup_open = true;
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

/// The window, until it is closed (or gone because the game is up and then ended well).
fn window(kit: &Kit, first: Vec<Step>, folder: Option<&OsStr>) {
    if kit.edition() == Edition::E1011 {
        ask_updates(); // the releases are 2010-11 kits: never offered to the 2017-18 kit
    }
    if !first.is_empty() {
        start(first);
    }
    pad_thread();
    let title = format!("{TITLE} {} - {}", kit.edition().name(), kit.dir().display());
    let fonts = theme::font_dir();
    // wgpu first; if it cannot open the window (no Direct3D/Vulkan), OpenGL once. FORZA_RENDERER=glow|wgpu forces one.
    let forced = match std::env::var("FORZA_RENDERER").as_deref() {
        Ok("glow") => Some(eframe::Renderer::Glow),
        Ok("wgpu") => Some(eframe::Renderer::Wgpu),
        _ => None,
    };
    let mut tries = forced.map_or(vec![eframe::Renderer::Wgpu, eframe::Renderer::Glow], |r| vec![r]);
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
                .with_icon(egui::IconData::default()), // Windows' own, not eframe's
            ..Default::default()
        };
        let fonts = &fonts;
        ran = eframe::run_native(
            &title,
            options,
            Box::new(move |cc| {
                let _ = CTX.set(cc.egui_ctx.clone());
                log(&format!("window: {renderer:?} renderer"));
                let app = App::new(kit, folder);
                if let Err(e) = theme::install(&cc.egui_ctx, app.st.edition, fonts) {
                    return Err(tf(
                        "launcher.msg_no_fonts",
                        &[("error", &format!("{}: {e}", fonts.display()))],
                    )
                    .into());
                }
                Ok(Box::new(app))
            }),
        );
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
    // the run's own lines are English: its refusals also in the chosen language
    let said = match rc {
        run::UNGUARDED => Some(t("launcher.msg_unguarded")),
        run::NOT_GUARDED => Some(t("launcher.msg_not_guarded")),
        _ => None,
    };
    if let Some(line) = said.filter(|_| text::lang() != Lang::En) {
        println!("{line}");
    }
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
    // the 2017-18 kit: no updates (the releases are 2010-11 kits)
    let e1011 = kit.edition() == Edition::E1011;
    if e1011 {
        update::tidy(&kit);
    }
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
        None => Vec::new(),
        Some(_) if folder.is_some() => vec![Step::Script("setup.py", args.clone()), Step::Pictures],
        Some("undo") => vec![Step::Script("setup.py", args.clone())],
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
    fn game_text_arguments() {
        assert_eq!(language_args("en"), ["on"]);
        assert_eq!(language_args("it"), ["on", "it"]);
        assert_eq!(language_args(""), ["off"]);
    }

    #[test]
    fn play_py_steps() {
        assert_eq!(step("  4/6 cabinet stand-ins: keychip/network"), Some((4, 6)));
        assert_eq!(step("  1/2 scene service"), Some((1, 2)));
        assert_eq!(step("  6/6 overlay in seat 1's window"), Some((6, 6)));
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
    fn game_text_places() {
        assert_eq!(game_text_of("en"), 0);
        assert_eq!(game_text_of("it"), 1);
        assert_eq!(game_text_of(""), 2);
        assert_eq!(GAME_TEXT[game_text_of("it")], "it");
    }
}
