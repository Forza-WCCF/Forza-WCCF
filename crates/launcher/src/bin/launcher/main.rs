//! Launcher.exe - the kit's one program, a window (2026-10-10; before, a console that ran play.py, and SETUP.exe beside
//! it for the rest).  Nothing it starts shows a console: the game's two windows are all a player sees.
//!   PLAY         play on THIS PC or ONLINE on a server, the window's language, the game's text (English,
//!                Italian or Japanese) - then scripts\play.py starts the game; once it is up this window goes, and this
//!                process stays behind without a window as the run's watcher (watch.rs): closing a game window stops
//!                the rest, then Launcher.exe ends.  While the game runs, Launcher.exe opened again offers STOP THE GAME.
//!   SETUP AND UPDATES (folded away until wanted, or until setup is needed)
//!                the game folder: set up / repair (scripts\setup.py GAME) and undo; it is typed, picked, dropped on
//!                the window or on Launcher.exe, and remembered by setup.py in data\settings.json.
//!                UPDATE (update.rs): the kit's releases and test builds at UPDATE_URL.
//! The choices go where the kit already keeps them (choices.rs): data\panel.txt, which the in-game SETTINGS panel
//! writes too, and data\settings.json.  The scripts' output fills the box at the bottom.
//!
//! The command line still works: "Launcher.exe restart" (the in-game RESTART NOW: this window, restarting, then gone),
//! "Launcher.exe GAME-FOLDER" / "Launcher.exe undo" (setup at once), and play.py's own words (local, server, remote ADDRESS,
//! debug, stop, status, show): from a console they print there, as before; from elsewhere (a shortcut) they run in
//! this window.  "--watch" is the watcher alone, which a console start leaves behind.

#![windows_subsystem = "windows"]

mod choices;
mod link;
mod text;
mod update;

use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::io::IntoRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::{env, fs, mem, ptr, thread};

use launcher::Kit;
use launcher::release::is_newer;
use text::{LANGUAGES, fill, t};
use update::Release;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontIndirectW, FW_BOLD, GetDC, GetDeviceCaps, HFONT, LOGPIXELSY, ReleaseDC,
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::{SS_ETCHEDHORZ, SS_RIGHT};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::UI::Controls::{
    BST_CHECKED, EM_REPLACESEL, EM_SETLIMITTEXT, EM_SETSEL, ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, PBM_SETPOS, PBM_SETRANGE32, PROGRESS_CLASSW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::Shell::{
    BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, DragAcceptFiles, DragFinish, DragQueryFileW, HDROP,
    SHBrowseForFolderW, SHGetPathFromIDListW, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::{PCWSTR, w};

const ID_FOLDER: i32 = 10;
const ID_BROWSE: i32 = 11;
const ID_SETUP: i32 = 12;
const ID_UNDO: i32 = 13;
const ID_HEADING: i32 = 17;
const ID_LIST: i32 = 18;
const ID_UPDATE: i32 = 19;
const ID_NOTES: i32 = 20;
const ID_STATUS: i32 = 21;
const ID_BAR: i32 = 22;
const ID_LOG: i32 = 23;
const ID_THIS_PC: i32 = 30;
const ID_ONLINE: i32 = 31;
const ID_SERVER: i32 = 32;
const ID_LANG: i32 = 33;
const ID_TEXT_EN: i32 = 34;
const ID_TEXT_JA: i32 = 35;
const ID_PLAY: i32 = 36;
const ID_MORE: i32 = 37;
const ID_VERSION: i32 = 38;
const ID_UPNOTE: i32 = 39;
const ID_L_PLAY_ON: i32 = 40;
const ID_L_LANG: i32 = 41;
const ID_L_TEXT: i32 = 42;
const ID_L_FOLDER: i32 = 43;
const ID_TEXT_IT: i32 = 44;
/// Disabled while a job runs.
const BUTTONS: [i32; 14] = [
    ID_BROWSE, ID_SETUP, ID_UNDO, ID_UPDATE, ID_NOTES, ID_THIS_PC, ID_ONLINE, ID_SERVER, ID_LANG, ID_TEXT_EN,
    ID_TEXT_IT, ID_TEXT_JA, ID_PLAY, ID_FOLDER,
];

/// The window's client height folded and unfolded (96-dpi pixels), and its width.
const FOLDED: i32 = 282;
const UNFOLDED: i32 = 720;
const WIDTH: i32 = 600;

enum Step {
    Script(&'static str, Vec<OsString>),
    Update(Release),
    /// play.py with these words; its exit code is the result
    Game(Vec<OsString>),
}

/// What the worker threads tell the window; the window looks every 100 ms.
struct Shared {
    releases: Vec<Release>,
    listed: bool, // `releases` is new: the list box gets it
    status: String,
    update_note: String,
    progress: u32,    // of 1000
    log: String,      // lines for the output box, not shown yet
    busy: bool,       // a job runs
    updated: bool,    // an update has just finished: offer setup
    failed: bool,     // the last job failed: unfold, so its output shows
    game: Option<u8>, // a Game step has just ended with this exit code
    changed: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    releases: Vec::new(),
    listed: false,
    status: String::new(),
    update_note: String::new(),
    progress: 0,
    log: String::new(),
    busy: false,
    updated: false,
    failed: false,
    game: None,
    changed: true,
});

/// This process took the run's watch over (watch.rs): once its window is gone it watches the run, whatever the start
/// did - a refused restart leaves the run running, and someone must stop the rest when its window is closed.
static WATCHES: AtomicBool = AtomicBool::new(false);
/// The game (or a kit tool) runs from this kit folder: PLAY offers STOP THE GAME.
static RUNNING: AtomicBool = AtomicBool::new(false);
static UNFOLD: AtomicBool = AtomicBool::new(false);
/// The screen's dots per inch (96 at 100 %), for the window's size.
static DPI: AtomicI32 = AtomicI32::new(96);

fn shared() -> MutexGuard<'static, Shared> {
    SHARED.lock().unwrap_or_else(PoisonError::into_inner)
}

fn say(status: impl Into<String>, progress: u32) {
    let mut s = shared();
    s.status = status.into();
    s.progress = progress;
    s.changed = true;
}

fn log(line: &str) {
    let mut s = shared();
    s.log.push_str(line);
    s.log.push_str("\r\n");
    s.changed = true;
}

// ---- the kit's scripts ------------------------------------------------------------------------------------------

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
                if let Some(per_mille) = step(&text) {
                    let mut s = shared();
                    s.progress = s.progress.max(per_mille);
                }
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

/// "  4/6 cabinet stand-ins ..." -> 4 of 6, in thousandths.
fn step(line: &str) -> Option<u32> {
    let (n, rest) = line.trim_start().split_once('/')?;
    let m: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let (n, m) = (n.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (n <= m && m > 0 && rest[m.to_string().len()..].starts_with(' ')).then(|| n * 1000 / m)
}

/// English is on: english.py's list of the original files it backed up is not empty (as kit_common.english_on reads it).
fn english_on(kit: &Kit) -> bool {
    fs::read_to_string(kit.dir().join("data").join("english_backup").join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty()) || v.as_array().is_some_and(|a| !a.is_empty()))
}

/// The language english.py has in place, as kit_common.game_language reads it: "en" or "it" (its language.txt; English
/// before that file existed), "" for the original Japanese.
fn game_language(kit: &Kit) -> &'static str {
    if !english_on(kit) {
        return "";
    }
    let file = kit.dir().join("data").join("english_backup").join("language.txt");
    if fs::read_to_string(file).is_ok_and(|t| t.trim() == "it") {
        "it"
    } else {
        "en"
    }
}

/// english.py's arguments that put `lang` ("en", "it" or "" for the original Japanese) in place.
fn language_args(lang: &str) -> Vec<OsString> {
    match lang {
        "it" => vec!["on".into(), "it".into()],
        "en" => vec!["on".into()],
        _ => vec!["off".into()],
    }
}

/// The game's text as the window shows it.
fn text_choice(hwnd: HWND) -> &'static str {
    if checked(hwnd, ID_TEXT_EN) {
        "en"
    } else if checked(hwnd, ID_TEXT_IT) {
        "it"
    } else {
        ""
    }
}

/// The game's text radio buttons: only `lang` ("en", "it", else Japanese) on.
fn pick_text(hwnd: HWND, lang: &str) {
    for (id, code) in [(ID_TEXT_EN, "en"), (ID_TEXT_IT, "it"), (ID_TEXT_JA, "")] {
        // SAFETY: a button of our own window.
        unsafe {
            SendMessageW(
                item(hwnd, id),
                BM_SETCHECK,
                if code == lang { BST_CHECKED as usize } else { 0 },
                0,
            )
        };
    }
}

fn panel_file(kit: &Kit) -> PathBuf {
    kit.dir().join("data").join("panel.txt")
}

fn settings_file(kit: &Kit) -> PathBuf {
    kit.dir().join("data").join("settings.json")
}

fn this_kit(kit: &Kit) -> String {
    kit.version().unwrap_or_else(|| t().not_known.into())
}

/// The steps one by one in a worker; the first that fails ends the job.
fn start(steps: Vec<Step>) {
    {
        let mut s = shared();
        if s.busy {
            return;
        }
        s.busy = true;
        s.progress = 0;
        s.changed = true;
    }
    thread::spawn(move || {
        let kit = Kit::here();
        let mut result = Ok(());
        let mut updated = None;
        let mut game = None;
        for step in steps {
            result = match &step {
                Step::Script(script, args) => {
                    say(format!("{script} ..."), 0);
                    run_script(&kit, script, args).and_then(|rc| if rc == 0 { Ok(()) } else { Err(t().failed.into()) })
                }
                Step::Update(r) => update::apply(&kit, r).map(|()| updated = Some(r.tag.clone())),
                Step::Game(args) => {
                    let looks_only = args.first().is_some_and(|a| a == "status" || a == "show");
                    if !looks_only {
                        launcher::take_over(&kit); // an earlier run's watcher lets go: this process watches now
                        WATCHES.store(true, Ordering::Relaxed);
                    }
                    run_script(&kit, "play.py", args).map(|rc| game = Some(u8::try_from(rc).unwrap_or(1)))
                }
            };
            if result.is_err() {
                break;
            }
        }
        RUNNING.store(launcher::kit_program_running(&kit).is_some(), Ordering::Relaxed);
        let failed = result.is_err() || game.is_some_and(|rc| rc != 0);
        let status = match (&result, &updated, game) {
            (Err(e), _, _) => e.clone(),
            (Ok(()), Some(tag), _) => fill(t().updated, &[tag]),
            (Ok(()), None, Some(2)) => t().not_set_up.to_owned(),
            (Ok(()), None, Some(rc)) if rc != 0 => t().start_failed.to_owned(),
            (Ok(()), None, _) => t().done.to_owned(),
        };
        log(&status);
        log("");
        let mut s = shared();
        s.status = status;
        s.progress = if failed { 0 } else { 1000 };
        s.updated = updated.is_some();
        s.failed = failed;
        s.game = game;
        s.busy = false;
        s.changed = true;
    });
}

/// The release list, asked in the background when the window opens.
fn ask_updates() {
    thread::spawn(|| {
        shared().update_note = t().asking.into();
        let found = update::fetch();
        let installed = this_kit(&Kit::here());
        let mut s = shared();
        match found {
            Ok(list) if !list.is_empty() => {
                if let Some(r) = list.iter().find(|r| !r.test && is_newer(&r.tag, &installed)) {
                    if !s.busy {
                        s.status = fill(t().available, &[&r.tag]);
                    }
                    s.update_note = String::new();
                } else {
                    s.update_note = t().pick.into();
                }
                s.releases = list;
                s.listed = true;
            }
            Ok(_) => s.update_note = t().none_listed.into(),
            Err(e) => s.update_note = format!("{} {}", fill(t().no_answer, &[&e]), t().retry),
        }
        s.changed = true;
    });
}

// ---- the window -------------------------------------------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn item(hwnd: HWND, id: i32) -> HWND {
    // SAFETY: a control of our own window (null if there is none: every call below takes that).
    unsafe { GetDlgItem(hwnd, id) }
}

fn set_text(hwnd: HWND, id: i32, text: &str) {
    // SAFETY: a NUL-terminated string for a control of our own window.
    unsafe { SetWindowTextW(item(hwnd, id), wide(text).as_ptr()) };
}

fn get_text(hwnd: HWND, id: i32) -> String {
    let c = item(hwnd, id);
    // SAFETY: the buffer holds the length Windows reports plus the NUL.
    unsafe {
        let mut buf = vec![0u16; GetWindowTextLengthW(c) as usize + 1];
        let n = GetWindowTextW(c, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize]).trim().to_owned()
    }
}

fn checked(hwnd: HWND, id: i32) -> bool {
    // SAFETY: a button of our own window.
    unsafe { SendMessageW(item(hwnd, id), BM_GETCHECK, 0, 0) == BST_CHECKED as LRESULT }
}

/// One of a pair of radio buttons on, the other off.
fn pick(hwnd: HWND, on: i32, off: i32) {
    // SAFETY: buttons of our own window.
    unsafe {
        SendMessageW(item(hwnd, on), BM_SETCHECK, BST_CHECKED as usize, 0);
        SendMessageW(item(hwnd, off), BM_SETCHECK, 0, 0);
    }
}

fn ask(hwnd: HWND, text: &str) -> bool {
    // SAFETY: NUL-terminated strings, our own window as the owner.
    unsafe {
        MessageBoxW(
            hwnd,
            wide(text).as_ptr(),
            wide(t().title).as_ptr(),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}

fn selected(hwnd: HWND) -> Option<Release> {
    // SAFETY: a list box of our own window.
    let i = unsafe { SendMessageW(item(hwnd, ID_LIST), LB_GETCURSEL, 0, 0) };
    usize::try_from(i).ok().and_then(|i| shared().releases.get(i).cloned())
}

/// The game folder box, or a message that it is empty.
fn folder(hwnd: HWND) -> Option<OsString> {
    let f = get_text(hwnd, ID_FOLDER);
    if f.is_empty() {
        unfold(hwnd, true);
        say(t().no_folder, 0);
        return None;
    }
    Some(f.into())
}

fn browse(hwnd: HWND) {
    let title = wide(t().browse_title);
    let info = BROWSEINFOW {
        hwndOwner: hwnd,
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        ..Default::default()
    };
    // SAFETY: `info` and `title` live across the call; the path buffer is MAX_PATH long, as Windows asks; the item
    // list Windows gives back is freed once.
    unsafe {
        let pidl = SHBrowseForFolderW(&info);
        if pidl.is_null() {
            return;
        }
        let mut path = [0u16; 260];
        let ok = SHGetPathFromIDListW(pidl, path.as_mut_ptr());
        CoTaskMemFree(pidl.cast());
        if ok != 0 {
            let len = path.iter().position(|&c| c == 0).unwrap_or(path.len());
            set_text(hwnd, ID_FOLDER, &String::from_utf16_lossy(&path[..len]));
        }
    }
}

fn dropped(hwnd: HWND, drop: HDROP) {
    // SAFETY: the drop handle Windows gave with WM_DROPFILES, finished once; the buffer and its length go together.
    unsafe {
        let mut path = vec![0u16; 32_768];
        let n = DragQueryFileW(drop, 0, path.as_mut_ptr(), path.len() as u32);
        DragFinish(drop);
        if n > 0 {
            unfold(hwnd, true);
            set_text(hwnd, ID_FOLDER, &String::from_utf16_lossy(&path[..n as usize]));
        }
    }
}

fn update_clicked(hwnd: HWND) {
    let Some(r) = selected(hwnd) else { return };
    if ask(
        hwnd,
        &fill(if r.test { t().confirm_test } else { t().confirm_release }, &[&r.tag]),
    ) {
        start(vec![Step::Update(r)]);
    }
}

fn open_notes(hwnd: HWND) {
    let Some(page) = selected(hwnd).map(|r| r.page) else {
        return;
    };
    // SAFETY: NUL-terminated strings; Windows opens the page in the default browser.
    unsafe {
        ShellExecuteW(
            hwnd,
            w!("open"),
            wide(&page).as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
}

/// PLAY: the choices into data\panel.txt, then play.py.  While the game runs: STOP THE GAME.
fn play_clicked(hwnd: HWND) {
    if RUNNING.load(Ordering::Relaxed) {
        if ask(hwnd, t().stop_confirm) {
            say(t().stopping, 0);
            start(vec![Step::Game(vec!["stop".into(), "force".into()])]);
        }
        return;
    }
    let kit = Kit::here();
    let online = checked(hwnd, ID_ONLINE);
    let server = get_text(hwnd, ID_SERVER);
    if online && !choices::valid_address(&server) {
        say(t().bad_address, 0);
        // SAFETY: a control of our own window.
        unsafe { SetFocus(item(hwnd, ID_SERVER)) };
        return;
    }
    let lang = text_choice(hwnd);
    let path = panel_file(&kit);
    let mut panel = choices::read(&path);
    panel = choices::set_panel_value(&panel, "play_on", Some(if online { "online" } else { "this_pc" }));
    if !server.is_empty() {
        panel = choices::set_panel_value(&panel, "server", Some(&server));
    }
    // a change only: play.py switches the game's text at the start (english.py), then takes the line out
    let change = (lang != game_language(&kit)).then_some(match lang {
        "en" => "on",
        "it" => "it",
        _ => "off",
    });
    panel = choices::set_panel_value(&panel, "english", change);
    if let Err(e) = choices::write(&path, &panel) {
        say(
            fill(t().write_failed, &[&path.display().to_string(), &e.to_string()]),
            0,
        );
        return;
    }
    say(t().starting, 0);
    start(vec![Step::Game(Vec::new())]);
}

fn language_picked(hwnd: HWND) {
    // SAFETY: a combo box of our own window.
    let i = unsafe { SendMessageW(item(hwnd, ID_LANG), CB_GETCURSEL, 0, 0) };
    let Some(&(code, _, _)) = usize::try_from(i).ok().and_then(|i| LANGUAGES.get(i)) else {
        return;
    };
    text::set(code);
    let _ = choices::set_setting(&settings_file(&Kit::here()), "language", code);
    // the game's text follows: Japanese for 日本語, Italian for Italiano, else English (still a choice of its own)
    pick_text(
        hwnd,
        match code {
            "ja" => "",
            "it" => "it",
            _ => "en",
        },
    );
    retext(hwnd);
}

/// Every word of the window in the language chosen.
fn retext(hwnd: HWND) {
    let tx = t();
    let kit = Kit::here();
    let title = wide(&format!("{} - {}", tx.title, kit.dir().display()));
    // SAFETY: a NUL-terminated string for our own window.
    unsafe { SetWindowTextW(hwnd, title.as_ptr()) };
    for (id, words) in [
        (ID_L_PLAY_ON, tx.play_on),
        (ID_THIS_PC, tx.this_pc),
        (ID_ONLINE, tx.online),
        (ID_L_LANG, tx.language),
        (ID_L_TEXT, tx.game_text),
        (ID_TEXT_EN, tx.english),
        (ID_TEXT_IT, tx.italian),
        (ID_TEXT_JA, tx.japanese),
        (ID_L_FOLDER, tx.game_folder),
        (ID_BROWSE, tx.browse),
        (ID_SETUP, tx.set_up),
        (ID_UNDO, tx.undo),
        (ID_UPDATE, tx.update),
        (ID_NOTES, tx.whats_new),
    ] {
        set_text(hwnd, id, words);
    }
    set_text(
        hwnd,
        ID_PLAY,
        if RUNNING.load(Ordering::Relaxed) {
            tx.stop_game
        } else {
            tx.play
        },
    );
    set_text(
        hwnd,
        ID_MORE,
        if UNFOLD.load(Ordering::Relaxed) {
            tx.less
        } else {
            tx.more
        },
    );
    let mut s = shared();
    s.listed = !s.releases.is_empty(); // "release", "(this kit)" ... again
    s.changed = true;
}

/// Setup and updates shown, or folded away.
fn unfold(hwnd: HWND, open: bool) {
    UNFOLD.store(open, Ordering::Relaxed);
    set_text(hwnd, ID_MORE, if open { t().less } else { t().more });
    // SAFETY: our own window; the rectangle is filled by Windows.
    unsafe {
        let dpi = DPI.load(Ordering::Relaxed);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: WIDTH * dpi / 96,
            bottom: (if open { UNFOLDED } else { FOLDED }) * dpi / 96,
        };
        AdjustWindowRectEx(&mut rect, GetWindowLongW(hwnd, GWL_STYLE) as u32, 0, 0);
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOMOVE | SWP_NOZORDER,
        );
    }
}

/// The timer: what the workers said since the last look.
fn refresh(hwnd: HWND) {
    let (status, note, progress, log, busy, has_list, list, updated, failed, game) = {
        let mut s = shared();
        if !mem::take(&mut s.changed) {
            return;
        }
        let list = mem::take(&mut s.listed).then(|| s.releases.clone());
        (
            s.status.clone(),
            s.update_note.clone(),
            s.progress,
            mem::take(&mut s.log),
            s.busy,
            !s.releases.is_empty(),
            list,
            mem::take(&mut s.updated),
            mem::take(&mut s.failed),
            s.game.take(),
        )
    };
    let kit = Kit::here();
    let installed = this_kit(&kit);
    // the game is up: this window goes, and the process watches the run (main, after the window)
    if game == Some(0) && WATCHES.load(Ordering::Relaxed) && launcher::run_recorded(&kit) > 0 {
        // SAFETY: our own window, from its own thread.
        unsafe { DestroyWindow(hwnd) };
        return;
    }
    if failed {
        unfold(hwnd, true);
    }
    set_text(hwnd, ID_STATUS, &status);
    set_text(hwnd, ID_UPNOTE, &note);
    set_text(hwnd, ID_HEADING, &fill(t().update_heading, &[&installed]));
    let shown = kit
        .version()
        .map(|v| format!("Kit {}", v.strip_prefix("kit-").unwrap_or(&v)));
    set_text(hwnd, ID_VERSION, &shown.unwrap_or_default());
    let running = RUNNING.load(Ordering::Relaxed);
    set_text(hwnd, ID_PLAY, if running { t().stop_game } else { t().play });
    // SAFETY: controls of our own window; every string is NUL-terminated and copied by the control.
    unsafe {
        SendMessageW(item(hwnd, ID_BAR), PBM_SETPOS, progress as usize, 0);
        for id in BUTTONS {
            let on = !busy && (id != ID_UPDATE || has_list);
            EnableWindow(item(hwnd, id), i32::from(on));
        }
        if !log.is_empty() {
            let out = item(hwnd, ID_LOG);
            let end = GetWindowTextLengthW(out) as usize;
            SendMessageW(out, EM_SETSEL, end, end as LPARAM);
            SendMessageW(out, EM_REPLACESEL, 0, wide(&log).as_ptr() as LPARAM);
        }
        if let Some(list) = list {
            let box_ = item(hwnd, ID_LIST);
            SendMessageW(box_, LB_RESETCONTENT, 0, 0);
            for r in &list {
                let kind = if r.test { t().test_build } else { t().release };
                let mark = if r.tag == installed {
                    t().this_kit
                } else if !r.test && is_newer(&r.tag, &installed) {
                    t().newer
                } else {
                    ""
                };
                let line = wide(&format!("{}\t{kind}\t{}\t{mark}", r.tag, r.date));
                SendMessageW(box_, LB_ADDSTRING, 0, line.as_ptr() as LPARAM);
            }
            let first = list.iter().position(|r| !r.test).unwrap_or(0);
            SendMessageW(box_, LB_SETCURSEL, first, 0);
        }
    }
    if updated {
        {
            let mut s = shared();
            s.listed = true; // the "(this kit)" mark moves
            s.changed = true;
        }
        let game = get_text(hwnd, ID_FOLDER);
        let lang = game_language(&kit);
        let question = match lang {
            "en" => t().after_update_english,
            "it" => t().after_update_italian,
            _ => t().after_update,
        };
        if !game.is_empty() && ask(hwnd, question) {
            let mut steps = vec![Step::Script("setup.py", vec![game.into()])];
            if !lang.is_empty() {
                steps.push(Step::Script("english.py", language_args(lang)));
            }
            start(steps);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND if (wp >> 16) as u32 == BN_CLICKED => {
            match (wp & 0xffff) as i32 {
                ID_PLAY => play_clicked(hwnd),
                ID_MORE => unfold(hwnd, !UNFOLD.load(Ordering::Relaxed)),
                ID_BROWSE => browse(hwnd),
                ID_SETUP => {
                    if let Some(game) = folder(hwnd) {
                        start(vec![Step::Script("setup.py", vec![game])]);
                    }
                }
                ID_UNDO => {
                    if let Some(game) = folder(hwnd).filter(|_| ask(hwnd, t().undo_confirm)) {
                        // setup.py refuses an undo while English is on: the original files go back first
                        let mut steps = Vec::new();
                        if english_on(&Kit::here()) {
                            steps.push(Step::Script("english.py", vec!["off".into()]));
                        }
                        steps.push(Step::Script("setup.py", vec!["undo".into(), game]));
                        start(steps);
                    }
                }
                ID_UPDATE => update_clicked(hwnd),
                ID_NOTES => open_notes(hwnd),
                _ => {}
            }
            0
        }
        WM_COMMAND if (wp >> 16) as u32 == CBN_SELCHANGE && (wp & 0xffff) as i32 == ID_LANG => {
            language_picked(hwnd);
            0
        }
        WM_DROPFILES => {
            dropped(hwnd, wp as HDROP);
            0
        }
        WM_TIMER => {
            refresh(hwnd);
            0
        }
        // never a kit left half copied, nor a start half done: the window stays while a job runs
        WM_CLOSE if shared().busy => {
            say(t().busy, shared().progress);
            0
        }
        WM_DESTROY => {
            // SAFETY: plain Win32 call.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: the message as Windows gave it.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// The message font, `scale` times its height, bold if asked.
///
/// # Safety
/// Plain Win32 calls; the font is never freed (it lives as long as the window).
unsafe fn font(scale: i32, bold: bool) -> HFONT {
    // SAFETY: the structure is plain data, filled by Windows.
    unsafe {
        let mut metrics: NONCLIENTMETRICSW = mem::zeroed();
        metrics.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
        SystemParametersInfoW(SPI_GETNONCLIENTMETRICS, metrics.cbSize, (&raw mut metrics).cast(), 0);
        let mut f = metrics.lfMessageFont;
        f.lfHeight *= scale;
        if bold {
            f.lfWeight = FW_BOLD as i32;
        }
        CreateFontIndirectW(&f)
    }
}

/// The window, and its message loop until it is closed (or gone because the game is up).
fn window(kit: &Kit, first: Option<Step>, folder: Option<&OsStr>) {
    // SAFETY: plain Win32 window set-up on this (the only UI) thread; every string is NUL-terminated and lives
    // across its call, every handle comes from the call before.
    unsafe {
        SetProcessDPIAware();
        CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32); // the folder picker's new style needs COM
        let screen = GetDC(ptr::null_mut());
        let dpi = GetDeviceCaps(screen, LOGPIXELSY as i32);
        ReleaseDC(ptr::null_mut(), screen);
        DPI.store(dpi, Ordering::Relaxed);
        let s = |v: i32| v * dpi / 96;
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_PROGRESS_CLASS,
        };
        InitCommonControlsEx(&icc);
        let (normal, big) = (font(1, false), font(2, true));

        let instance = GetModuleHandleW(ptr::null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_BTNFACE + 1) as usize as _,
            lpszClassName: w!("WccfKitPlay"),
            ..mem::zeroed()
        };
        RegisterClassW(&class);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: s(WIDTH),
            bottom: s(FOLDED),
        };
        AdjustWindowRectEx(&mut rect, style, 0, 0);
        let hwnd = CreateWindowExW(
            0,
            w!("WccfKitPlay"),
            w!(""),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        let child = |class: PCWSTR, style: u32, id: i32, x: i32, y: i32, cx: i32, cy: i32| {
            let framed = class == w!("LISTBOX") || class == w!("EDIT");
            let c = CreateWindowExW(
                if framed { WS_EX_CLIENTEDGE } else { 0 },
                class,
                w!(""),
                WS_CHILD | WS_VISIBLE | style,
                s(x),
                s(y),
                s(cx),
                s(cy),
                hwnd,
                id as usize as HMENU,
                instance,
                ptr::null(),
            );
            SendMessageW(c, WM_SETFONT, normal as usize, 0);
            c
        };
        let tab = WS_TABSTOP | WS_GROUP;
        let radio = BS_AUTORADIOBUTTON as u32;
        let title = child(w!("STATIC"), 0, 0, 12, 10, 360, 36);
        SetWindowTextW(title, w!("WCCF 2010-11"));
        SendMessageW(title, WM_SETFONT, big as usize, 0);
        child(w!("STATIC"), SS_RIGHT, ID_VERSION, 372, 22, 216, 20);
        child(w!("STATIC"), 0, ID_L_PLAY_ON, 12, 60, 110, 20);
        child(w!("BUTTON"), radio | tab, ID_THIS_PC, 124, 56, 110, 24);
        child(w!("BUTTON"), radio, ID_ONLINE, 238, 56, 150, 24);
        child(w!("EDIT"), ES_AUTOHSCROLL as u32 | tab, ID_SERVER, 392, 55, 196, 24);
        child(w!("STATIC"), 0, ID_L_LANG, 12, 96, 110, 20);
        let lang = child(
            w!("COMBOBOX"),
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL | tab,
            ID_LANG,
            124,
            90,
            110,
            200,
        );
        child(w!("STATIC"), 0, ID_L_TEXT, 244, 96, 96, 20);
        child(w!("BUTTON"), radio | tab, ID_TEXT_EN, 344, 92, 80, 24);
        child(w!("BUTTON"), radio, ID_TEXT_IT, 424, 92, 80, 24);
        child(w!("BUTTON"), radio, ID_TEXT_JA, 504, 92, 84, 24);
        let play = child(w!("BUTTON"), BS_DEFPUSHBUTTON as u32 | tab, ID_PLAY, 12, 128, 576, 48);
        SendMessageW(play, WM_SETFONT, big as usize, 0);
        child(w!("STATIC"), 0, ID_STATUS, 12, 184, 576, 36);
        let bar = child(PROGRESS_CLASSW, 0, ID_BAR, 12, 222, 576, 12);
        SendMessageW(bar, PBM_SETRANGE32, 0, 1000);
        child(w!("BUTTON"), tab, ID_MORE, 12, 244, 280, 28);

        child(w!("STATIC"), SS_ETCHEDHORZ, 0, 12, 284, 576, 2);
        child(w!("STATIC"), 0, ID_L_FOLDER, 12, 300, 110, 20);
        child(w!("EDIT"), ES_AUTOHSCROLL as u32 | tab, ID_FOLDER, 124, 294, 356, 24);
        child(w!("BUTTON"), tab, ID_BROWSE, 488, 294, 100, 24);
        child(w!("BUTTON"), tab, ID_SETUP, 124, 326, 180, 28);
        child(w!("BUTTON"), tab, ID_UNDO, 312, 326, 168, 28);
        child(w!("STATIC"), SS_ETCHEDHORZ, 0, 12, 364, 576, 2);
        child(w!("STATIC"), 0, ID_HEADING, 12, 374, 576, 20);
        let list_style = (LBS_NOTIFY | LBS_USETABSTOPS) as u32 | WS_VSCROLL | tab;
        let list = child(w!("LISTBOX"), list_style, ID_LIST, 12, 396, 576, 100);
        let stops: [i32; 3] = [90, 150, 210]; // dialog units: tag, kind, date, "(this kit)"
        SendMessageW(list, LB_SETTABSTOPS, stops.len(), stops.as_ptr() as LPARAM);
        child(w!("BUTTON"), tab, ID_UPDATE, 12, 500, 130, 28);
        child(w!("BUTTON"), tab, ID_NOTES, 150, 500, 130, 28);
        child(w!("STATIC"), 0, ID_UPNOTE, 290, 500, 298, 36);
        child(w!("STATIC"), SS_ETCHEDHORZ, 0, 12, 542, 576, 2);
        let out = (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32 | WS_VSCROLL;
        let out = child(w!("EDIT"), out, ID_LOG, 12, 552, 576, 156);
        SendMessageW(out, EM_SETLIMITTEXT, 0, 0); // more than the 32,767 characters a box takes by default

        // the choices as they are now
        for (_, name, _) in LANGUAGES {
            SendMessageW(lang, CB_ADDSTRING, 0, wide(name).as_ptr() as LPARAM);
        }
        SendMessageW(lang, CB_SETCURSEL, text::chosen(), 0);
        let panel = choices::read(&panel_file(kit));
        let online = choices::panel_value(&panel, "play_on").is_some_and(|v| v.eq_ignore_ascii_case("online"));
        if online {
            pick(hwnd, ID_ONLINE, ID_THIS_PC);
        } else {
            pick(hwnd, ID_THIS_PC, ID_ONLINE);
        }
        let server = choices::panel_value(&panel, "server")
            .or_else(|| choices::setting(&settings_file(kit), "server"))
            .unwrap_or_default();
        set_text(hwnd, ID_SERVER, &server);
        pick_text(
            hwnd,
            match choices::panel_value(&panel, "english").as_deref() {
                Some("on") => "en",
                Some("it") => "it",
                Some("off") => "",
                _ => game_language(kit),
            },
        );
        let game = folder.map_or_else(
            || choices::setting(&settings_file(kit), "game").unwrap_or_default(),
            |f| f.to_string_lossy().into_owned(),
        );
        set_text(hwnd, ID_FOLDER, &game);
        RUNNING.store(launcher::kit_program_running(kit).is_some(), Ordering::Relaxed);
        retext(hwnd);
        let unset = game.is_empty();
        unfold(hwnd, unset || folder.is_some());
        if unset {
            say(t().not_set_up, 0);
        } else if RUNNING.load(Ordering::Relaxed) {
            say(t().running, 0);
        }

        DragAcceptFiles(hwnd, 1);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetTimer(hwnd, 1, 100, None);
        ask_updates();
        if let Some(step) = first {
            start(vec![step]);
        }

        let mut msg: MSG = mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

// ---- from a console ---------------------------------------------------------------------------------------------

/// Started from a console (cmd, PowerShell): its output goes there.  A program built as a window has no console of
/// its own, so it joins the one it was started from; false if there is none.
fn attach_console() -> bool {
    // SAFETY: plain Win32 calls; each console handle is opened once and handed to Windows as this process's own.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return false;
        }
        let open = |name: &str| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(name)
                .ok()
                .map(|f| f.into_raw_handle())
        };
        if let Some(out) = open("CONOUT$") {
            SetStdHandle(STD_OUTPUT_HANDLE, out);
            SetStdHandle(STD_ERROR_HANDLE, out);
        }
        if let Some(input) = open("CONIN$") {
            SetStdHandle(STD_INPUT_HANDLE, input);
        }
    }
    true
}

/// play.py in the console Launcher.exe was started from, as before 2026-10-10; a start leaves the watcher behind.
fn console(kit: &Kit, args: &[OsString]) -> ExitCode {
    let looks_only = args
        .first()
        .is_some_and(|a| a.eq_ignore_ascii_case("status") || a.eq_ignore_ascii_case("show"));
    if !looks_only {
        launcher::take_over(kit);
    }
    println!();
    let rc = kit.run("play.py", args);
    if !looks_only {
        launcher::spawn_watcher(kit);
    }
    ExitCode::from(rc)
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let kit = Kit::here();
    let first = args.first().map(|a| a.to_string_lossy().to_ascii_lowercase());
    if first.as_deref() == Some("--watch") {
        launcher::watch(&kit);
        return ExitCode::SUCCESS;
    }
    update::tidy(&kit);
    text::choose(choices::setting(&settings_file(&kit), "language").as_deref());
    // a plain start (no arguments): first run, the kit folder gets the installer's name (link.rs)
    if args.is_empty() {
        match link::first_run(kit.dir()) {
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
        None => None,
        Some(_) if folder.is_some() => Some(Step::Script("setup.py", args.clone())),
        Some("undo") => Some(Step::Script("setup.py", args.clone())),
        Some("restart") => {
            say(t().restarting, 0);
            Some(Step::Game(args.clone()))
        }
        Some(_) if attach_console() => return console(&kit, &args),
        Some(_) => Some(Step::Game(args.clone())),
    };
    window(&kit, step, folder.map(OsString::as_os_str));
    if WATCHES.load(Ordering::Relaxed) {
        launcher::watch(&kit); // the game's windows are the run now; returns at once if there is no run
    }
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
        assert_eq!(step("  4/6 cabinet stand-ins: keychip/network"), Some(666));
        assert_eq!(step("  1/2 scene service"), Some(500));
        assert_eq!(step("  6/6 overlay in seat 1's window"), Some(1000));
        assert_eq!(step("     seat desk: TCP 20030"), None);
        assert_eq!(step("see data\\logs\\run_seat1.txt"), None);
        assert_eq!(step("  7/6 x"), None);
        assert_eq!(step("  3/6"), None);
    }

    #[test]
    fn fill_in_order() {
        assert_eq!(fill("{}: {} ({})", &["a", "b"]), "a: b ()");
        assert_eq!(fill("no gaps", &["a"]), "no gaps");
    }
}
