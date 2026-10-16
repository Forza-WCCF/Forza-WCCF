//! Launcher.exe - the kit's one program, a window (2026-10-10; before, a console that ran play.py, and SETUP.exe beside
//! it for the rest).  Nothing it starts shows a console: the game's two windows are all a player sees.
//!   PLAY         play on THIS PC or ONLINE on a server, the window's language, the game's text (English,
//!                Italian or Japanese) - then this process runs the game (launcher::run) and holds every process of it
//!                in one Job until it ends.  Once the game is up this window is minimized; closing a game window ends
//!                the run, and then Launcher.exe ends too.  Closing this window (or STOP THE GAME) asks the run to stop
//!                by the card rule: with the club card in or a match on, it asks first.  Launcher.exe opened again
//!                while the game runs offers STOP THE GAME, which asks the running Launcher.exe.
//!   SETUP AND UPDATES (folded away until wanted, or until setup is needed)
//!                the game folder: set up / repair (scripts\setup.py GAME) and undo; it is typed, picked, dropped on
//!                the window or on Launcher.exe, and remembered by setup.py in data\settings.json.
//!                UPDATE (update.rs): the kit's releases and test builds at UPDATE_URL.
//! The choices go where the kit already keeps them (choices.rs): data\panel.txt, which the in-game SETTINGS panel
//! writes too, and data\settings.json.  The scripts' output fills the box at the bottom.
//!
//! The command line still works: "Launcher.exe GAME-FOLDER" / "Launcher.exe undo" (setup at once), and play.py's words
//! (local, server, remote ADDRESS [seat N], debug, stop, status, show, restart): from a console they print there and a
//! start holds the console until the run ends (Ctrl+C asks it to stop, by the card rule); from elsewhere (a shortcut)
//! they run in this window.

#![windows_subsystem = "windows"]

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
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::{env, mem, ptr, thread};

use launcher::release::is_newer;
use launcher::run::{self, Params};
use launcher::{Edition, Kit, choices};
use text::{LANGUAGES, fill, t};
use update::Release;
use windows_sys::Win32::Foundation::{HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontIndirectW, FW_BOLD, GetDC, GetDeviceCaps, HFONT, LOGPIXELSY, ReleaseDC,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, GetConsoleMode,
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetStdHandle,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::{SS_ETCHEDHORZ, SS_RIGHT};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
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
use windows_sys::core::{BOOL, PCWSTR, w};

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
const ID_L_PICS: i32 = 45;
const ID_PICS_GAME: i32 = 46;
const ID_PICS_HQ: i32 = 47;
const ID_PACK: i32 = 48;
/// Disabled while a job runs.
const BUTTONS: [i32; 17] = [
    ID_BROWSE,
    ID_SETUP,
    ID_UNDO,
    ID_UPDATE,
    ID_NOTES,
    ID_THIS_PC,
    ID_ONLINE,
    ID_SERVER,
    ID_LANG,
    ID_TEXT_EN,
    ID_TEXT_IT,
    ID_TEXT_JA,
    ID_PLAY,
    ID_FOLDER,
    ID_PICS_GAME,
    ID_PICS_HQ,
    ID_PACK,
];

/// The window's client height folded and unfolded (96-dpi pixels), and its width.
const FOLDED: i32 = 282;
const UNFOLDED: i32 = 752;
const WIDTH: i32 = 600;

enum Step {
    Script(&'static str, Vec<OsString>),
    Update(Release),
    /// The run (launcher::run) with play.py's words; its exit code is the result
    Game(Vec<OsString>),
    /// The card pictures (launcher::run::make_pictures), after setup and when the pictures choice changes
    Pictures,
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

/// The game is up in this process (its run is supervised here): the window is minimized, closing it asks the run to stop.
static UP: AtomicBool = AtomicBool::new(false);
/// The game came up during the current job: when the run then ends well, Launcher.exe ends with it.
static WAS_UP: AtomicBool = AtomicBool::new(false);
/// A run is on in this kit folder (here or in another Launcher.exe): PLAY offers STOP THE GAME.
static RUNNING: AtomicBool = AtomicBool::new(false);
static UNFOLD: AtomicBool = AtomicBool::new(false);
/// The window was minimized for the game that is up (once per run, so the player can bring it back).
static MINIMIZED: AtomicBool = AtomicBool::new(false);
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
    if kit.edition() == Edition::E1718 && script == "setup.py" {
        return Err(t().setup_1718.into()); // its setup is setup1718.py, from WSL (step 11 ports it)
    }
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
    kit.data().join("panel.txt")
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
        WAS_UP.store(false, Ordering::Relaxed);
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
                Step::Pictures => {
                    say(t().making_pictures, 0);
                    run::make_pictures(&kit, &Params::of(&kit), &WindowUi)
                        .then_some(())
                        .ok_or_else(|| t().failed.to_owned())
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
            (Ok(()), Some(tag), _) => fill(t().updated, &[tag]),
            (Ok(()), None, Some(2)) => t().not_set_up.to_owned(),
            (Ok(()), None, Some(4)) => t().unguarded.to_owned(),
            (Ok(()), None, Some(5)) => t().not_guarded.to_owned(),
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

/// The run's lines go to the output box (its "N/M" steps move the bar); its question - stop anyway, after the card
/// rule refused a stop - is asked in a message box; the game up minimizes the window.
struct WindowUi;

impl run::Ui for WindowUi {
    fn say(&self, line: &str) {
        if let Some(per_mille) = step(line) {
            let mut s = shared();
            s.progress = s.progress.max(per_mille);
        }
        log(line);
    }

    fn ask(&self, _question: &str) -> bool {
        let text = wide(t().stop_anyway);
        // SAFETY: NUL-terminated strings; no owner (any thread may ask), kept in front of the game.
        unsafe {
            MessageBoxW(
                ptr::null_mut(),
                text.as_ptr(),
                wide(t().title).as_ptr(),
                MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
            ) == IDYES
        }
    }

    fn running(&self, up: bool) {
        UP.store(up, Ordering::Relaxed);
        WAS_UP.fetch_or(up, Ordering::Relaxed);
        RUNNING.store(up, Ordering::Relaxed);
        let mut s = shared();
        if up {
            s.status = t().running.into();
            s.progress = 1000;
        }
        s.changed = true;
    }
}

/// STOP THE GAME, or this window closed while the game runs: the running game is asked to stop (`force`: without the
/// card rule), on a thread of its own - the run itself may be this process's job.
fn stop_game(force: bool) {
    say(t().stopping, 0);
    thread::spawn(move || {
        let words: Vec<String> = if force {
            vec!["stop".into(), "force".into()]
        } else {
            vec!["stop".into()]
        };
        let kit = Kit::here();
        run::command(&kit, &Params::of(&kit), &words, &WindowUi);
        RUNNING.store(run::any_on(&kit), Ordering::Relaxed);
        shared().changed = true;
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

/// The board's card pictures: the game's own, or the high-quality pack (data\\settings.json "card_pictures" and
/// "card_pack"; made at setup and here, launcher::run::make_pictures).  The pack is asked for when none is known yet, or with Choose pack.
fn pictures_picked(hwnd: HWND, hq: bool, choose: bool) {
    let file = Kit::here().settings_file();
    let known = choices::setting(&file, "card_pack").filter(|p| Path::new(p).is_file());
    let pack = if !hq {
        None
    } else if let Some(p) = known.filter(|_| !choose) {
        Some(p)
    } else if let Some(p) = pick_pack(hwnd) {
        Some(p)
    } else {
        // no pack: the choice stays as it was
        let was = choices::setting(&file, "card_pictures").as_deref() == Some("hq");
        let (on, off) = if was {
            (ID_PICS_HQ, ID_PICS_GAME)
        } else {
            (ID_PICS_GAME, ID_PICS_HQ)
        };
        pick(hwnd, on, off);
        return;
    };
    let mut done = choices::set_setting(&file, "card_pictures", if pack.is_some() { "hq" } else { "game" });
    if let Some(p) = &pack {
        done = done.and_then(|()| choices::set_setting(&file, "card_pack", p));
        pick(hwnd, ID_PICS_HQ, ID_PICS_GAME);
    }
    match done {
        Ok(()) => start(vec![Step::Pictures]),
        Err(e) => say(
            fill(t().write_failed, &[&file.display().to_string(), &e.to_string()]),
            0,
        ),
    }
}

/// The open-file box for the pack's ZIP.
fn pick_pack(hwnd: HWND) -> Option<String> {
    let title = wide(t().pack_title);
    let filter: Vec<u16> = "ZIP\0*.zip\0\0".encode_utf16().collect();
    let mut path = vec![0u16; 32_768];
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: filter.as_ptr(),
        lpstrFile: path.as_mut_ptr(),
        nMaxFile: path.len() as u32,
        lpstrTitle: title.as_ptr(),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // SAFETY: the structure, the filter, the title and the path buffer live across the call; the buffer's length is
    // the one given.
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let len = path.iter().position(|&c| c == 0).unwrap_or(path.len());
    Some(String::from_utf16_lossy(&path[..len]))
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
            stop_game(true);
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
    let change = (lang != kit.game_language()).then_some(match lang {
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
    let _ = choices::set_setting(&Kit::here().settings_file(), "language", code);
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
        (ID_L_PICS, tx.card_pictures),
        (ID_PICS_GAME, tx.from_game),
        (ID_PICS_HQ, tx.hq_pack),
        (ID_PACK, tx.choose_pack),
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
    // the run this process held has ended well: Launcher.exe ends with it
    if game == Some(0) && WAS_UP.load(Ordering::Relaxed) {
        // SAFETY: our own window, from its own thread.
        unsafe { DestroyWindow(hwnd) };
        return;
    }
    // SAFETY: our own window, from its own thread.
    unsafe {
        if UP.load(Ordering::Relaxed) && IsIconic(hwnd) == 0 && !MINIMIZED.swap(true, Ordering::Relaxed) {
            ShowWindow(hwnd, SW_MINIMIZE); // the game's windows are in front; this one waits in the taskbar
        } else if game.is_some() && IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE); // the run ended badly: what it said is shown
        }
    }
    if !UP.load(Ordering::Relaxed) {
        MINIMIZED.store(false, Ordering::Relaxed);
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
        let lang = kit.game_language();
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
            steps.push(Step::Pictures); // last: a failure here must not skip english.py
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
                        start(vec![Step::Script("setup.py", vec![game]), Step::Pictures]);
                    }
                }
                ID_UNDO => {
                    if let Some(game) = folder(hwnd).filter(|_| ask(hwnd, t().undo_confirm)) {
                        // setup.py refuses an undo while English is on: the original files go back first
                        let mut steps = Vec::new();
                        if Kit::here().english_on() {
                            steps.push(Step::Script("english.py", vec!["off".into()]));
                        }
                        steps.push(Step::Script("setup.py", vec!["undo".into(), game]));
                        start(steps);
                    }
                }
                ID_UPDATE => update_clicked(hwnd),
                ID_NOTES => open_notes(hwnd),
                ID_PICS_GAME => pictures_picked(hwnd, false, false),
                ID_PICS_HQ => pictures_picked(hwnd, true, false),
                ID_PACK => pictures_picked(hwnd, true, true),
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
        // closing Launcher while the game runs is a quit: asked of the run, by the card rule
        WM_CLOSE if UP.load(Ordering::Relaxed) => {
            stop_game(false);
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
fn window(kit: &Kit, first: Vec<Step>, folder: Option<&OsStr>) {
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
        child(w!("STATIC"), 0, ID_L_PICS, 12, 368, 110, 20);
        child(w!("BUTTON"), radio | tab, ID_PICS_GAME, 124, 364, 150, 24);
        child(w!("BUTTON"), radio, ID_PICS_HQ, 278, 364, 170, 24);
        child(w!("BUTTON"), tab, ID_PACK, 452, 364, 136, 24);
        child(w!("STATIC"), SS_ETCHEDHORZ, 0, 12, 396, 576, 2);
        child(w!("STATIC"), 0, ID_HEADING, 12, 406, 576, 20);
        let list_style = (LBS_NOTIFY | LBS_USETABSTOPS) as u32 | WS_VSCROLL | tab;
        let list = child(w!("LISTBOX"), list_style, ID_LIST, 12, 428, 576, 100);
        let stops: [i32; 3] = [90, 150, 210]; // dialog units: tag, kind, date, "(this kit)"
        SendMessageW(list, LB_SETTABSTOPS, stops.len(), stops.as_ptr() as LPARAM);
        child(w!("BUTTON"), tab, ID_UPDATE, 12, 532, 130, 28);
        child(w!("BUTTON"), tab, ID_NOTES, 150, 532, 130, 28);
        child(w!("STATIC"), 0, ID_UPNOTE, 290, 532, 298, 36);
        child(w!("STATIC"), SS_ETCHEDHORZ, 0, 12, 574, 576, 2);
        let out = (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32 | WS_VSCROLL;
        let out = child(w!("EDIT"), out, ID_LOG, 12, 584, 576, 156);
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
            .or_else(|| choices::setting(&kit.settings_file(), "server"))
            .unwrap_or_default();
        set_text(hwnd, ID_SERVER, &server);
        if choices::setting(&kit.settings_file(), "card_pictures").as_deref() == Some("hq") {
            pick(hwnd, ID_PICS_HQ, ID_PICS_GAME);
        } else {
            pick(hwnd, ID_PICS_GAME, ID_PICS_HQ);
        }
        pick_text(
            hwnd,
            match choices::panel_value(&panel, "english").as_deref() {
                Some("on") => "en",
                Some("it") => "it",
                Some("off") => "",
                _ => kit.game_language(),
            },
        );
        let game = folder.map_or_else(
            || choices::setting(&kit.settings_file(), "game").unwrap_or_default(),
            |f| f.to_string_lossy().into_owned(),
        );
        set_text(hwnd, ID_FOLDER, &game);
        RUNNING.store(run::any_on(kit), Ordering::Relaxed);
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
        if kit.edition() == Edition::E1011 {
            ask_updates(); // the releases are 2010-11 kits: never offered to the 2017-18 kit
        }
        if !first.is_empty() {
            start(first);
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
        run::UNGUARDED => Some(t().unguarded),
        run::NOT_GUARDED => Some(t().not_guarded),
        _ => None,
    };
    if let Some(line) = said.filter(|_| text::chosen() != 0) {
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
    // the 2017-18 kit: no updates (the releases are 2010-11 kits) and no first-run link (its name is 2010-11's place)
    let e1011 = kit.edition() == Edition::E1011;
    if e1011 {
        update::tidy(&kit);
    }
    text::choose(choices::setting(&kit.settings_file(), "language").as_deref());
    // a plain start (no arguments): first run, the kit folder gets the installer's name (link.rs)
    if args.is_empty() && e1011 {
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
        None => Vec::new(),
        Some(_) if folder.is_some() => vec![Step::Script("setup.py", args.clone()), Step::Pictures],
        Some("undo") => vec![Step::Script("setup.py", args.clone())],
        Some("restart") => {
            say(t().restarting, 0);
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
