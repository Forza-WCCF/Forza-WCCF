//! The in-game panel as a DLL in the 32-bit game (step 9.4): forza_ui's panel drawn over the game's own Direct3D 9 picture.
//!
//! The mechanism is the C panel's (`native/2010-11/panel/wccfpanel.c`): a thread started from `DllMain` makes a
//! throwaway device and reads the shared `IDirect3DDevice9::Present` and `Reset` functions from its table (slots 17
//! and 16; each device has its own table copy on Windows 11, the functions are shared), and detours their first five
//! bytes. Each Present then copies the game's finished frame into a texture, draws the panel at the window's real
//! size on a swap chain of our own on the game's device (the game's frame is the panel's picture) and presents that
//! in place of the game's. The game's device state is saved in a state block and put back, render target and depth
//! buffer by hand. The game's window is subclassed: mouse and keys go to egui, and the game does not see what egui
//! wants. Reset releases what lives in the DEFAULT pool. Unloading the DLL (`FreeLibrary`) puts the bytes, the window
//! procedure and the device back as they were.
//!
//! The panel's data (step 9.4b, [`kit`]): the edition and the language, the player's catalogue and card pictures, the
//! hover delay; the formation board is the game's card table (`fpr_table0.txt` in the game's folder, read again when
//! someone else changes it, written on every move, swap and take-off: `forza::cards`). The cabinet buttons press their
//! keys from keys.txt as the C panel does (down on the press, up after the release and at least 140 ms). Clicks on
//! the game's picture go to the game, moved into its own client area, for its own on-screen buttons.
//!
//! In the projector the panel draws nothing: its window is resizable, the picture letterboxed to it, and F11 /
//! double-click / Esc give it borderless full screen ([`projector`], [`fullscreen`]).
//!
//! Test switches: `FORZA_PANEL_DUMP=1` writes `wccfpanel_frame.bmp` beside the program every 10 frames once a window
//! size has held for 10 (fakegame.exe's `size:` and `copy:` steps); `WCCFPANEL_DRYKEYS=1` logs the keys instead of
//! pressing them.
#![cfg(target_arch = "x86")]

mod exe_text;
mod fullscreen;
mod kit;
mod money;
mod net;
mod painter;
mod patches;
mod pause;
mod probe;
mod projector;

use egui::{Event, Key, Modifiers, PointerButton, Pos2, TextureId, ViewportId, pos2, vec2};
use forza::cards::{self, Placed, TableFile};
use forza_ui::board::Card;
use forza_ui::panel::{self, PanelState};
use forza_ui::theme;
use std::cell::Cell;
use std::ffi::c_void;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D9::*;
use windows::Win32::Graphics::Gdi::RGNDATA;
use windows::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows::Win32::System::LibraryLoader::{
    DisableThreadLibraryCalls, GetModuleFileNameW, GetModuleHandleW,
};
use windows::Win32::System::Memory::*;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetCapture, GetDoubleClickTime, GetKeyState, ReleaseCapture, SetCapture,
    VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HRESULT, Interface, w};

type PresentFn = unsafe extern "system" fn(
    *mut c_void,
    *const RECT,
    *const RECT,
    HWND,
    *const RGNDATA,
) -> HRESULT;
type ResetFn = unsafe extern "system" fn(*mut c_void, *mut D3DPRESENT_PARAMETERS) -> HRESULT;

/// The game's frame, as the painter knows it.
const GAME: TextureId = TextureId::User(0);

/// The trampolines (the detoured functions' first bytes, then a jump back).
static PRESENT: AtomicUsize = AtomicUsize::new(0);
static RESET: AtomicUsize = AtomicUsize::new(0);
/// What [`unload`] puts back: (function, its first five bytes, trampoline).
static PATCHES: Mutex<Vec<(usize, [u8; 5], usize)>> = Mutex::new(Vec::new());
static INIT_DONE: AtomicBool = AtomicBool::new(false);
/// A frame was drawn by the last Present: until then (or after a failed one) wndproc keeps the mouse for the game.
static DRAWN: AtomicBool = AtomicBool::new(false);
/// A panic in a frame: the panel stops drawing, the game goes on.
static DEAD: AtomicBool = AtomicBool::new(false);
static RENDER: Mutex<Option<Render>> = Mutex::new(None);
// the window side (its own thread) and the render side meet here
static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
/// The board window's own events and pixels per point (popped out it has an egui of its own).
static BOARD_EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static BOARD_SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000);
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static OLD_PROC: AtomicIsize = AtomicIsize::new(0);
static SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000); // pixels per point (f32 bits), 1.0 until the first frame
static WANTS_KEYS: AtomicBool = AtomicBool::new(false);
/// A page is open: its keys are the page's, never the game's.
static PAGE_OPEN: AtomicBool = AtomicBool::new(false);
/// Keys pressed while a page is open (virtual-key code, Alt held), repeats left out: KEYS takes them.
static VK_DOWN: Mutex<Vec<(u32, bool)>> = Mutex::new(Vec::new());
static EXE_DIR: OnceLock<PathBuf> = OnceLock::new();
static DLL_DIR: OnceLock<PathBuf> = OnceLock::new();
static FILES: OnceLock<kit::Files> = OnceLock::new();
static CATALOGUE: OnceLock<Arc<[Card]>> = OnceLock::new();
/// The players' affinity from the game's own files (`prog_data\player_data\` beside the program), when readable.
static AFFINITY: OnceLock<Option<Arc<cards::links::Affinity>>> = OnceLock::new();
/// The game's picture on the window (pixels) while no page covers it: clicks there are the game's.
static PICTURE: Mutex<Option<egui::Rect>> = Mutex::new(None);
/// The mouse buttons whose press went to the game (bit per button): their release goes there too.
static GAME_DOWN: AtomicU32 = AtomicU32::new(0);
/// This process is the projector (the second game window): money, the frame wait and its frames for seat 1, no panel.
static PROJECTOR: AtomicBool = AtomicBool::new(false);
static JVS_OUT: OnceLock<Option<forza::standins::jvs_out::Map>> = OnceLock::new();
/// The projector's picture composed into its resized window ([`projector::View`]).
static VIEW: Mutex<Option<projector::View>> = Mutex::new(None);
/// Full screen: the panel's window and the projector's, each with what leaving restores ([`fullscreen`]).
static PANEL_FS: fullscreen::Slot = Mutex::new(None);
static PROJ_FS: fullscreen::Slot = Mutex::new(None);
/// A frame has been shown on our own swap chain: the window may be maximized (F11).
static SHOWN: AtomicBool = AtomicBool::new(false);
/// The player closed the window: the server link's drop that follows is the end (ENDED), not a loss.
static CLOSING: AtomicBool = AtomicBool::new(false);
/// The key a latched KEY PLAYER or KEEPER holds down (0: none); the window procedure lets it up on a focus loss.
static LATCH_VK: AtomicU32 = AtomicU32::new(0);
/// A close refused during a card session: the close dialog is open. Close requests are ignored while it is.
static CLOSE_ASK: AtomicBool = AtomicBool::new(false);
/// QUIT was clicked in the dialog: the next close request (the panel's own, posted) goes through.
static CLOSE_OK: AtomicBool = AtomicBool::new(false);
/// When the dialog opened (ms since START), for the tests' automatic answer.
static CLOSE_SINCE: AtomicU32 = AtomicU32::new(0);
/// The control link dropped without the player closing the window (link_watch's LOST): the crash screen shows.
static LINK_LOST: AtomicBool = AtomicBool::new(false);
static START: OnceLock<Instant> = OnceLock::new();
/// The window's maximized state for WCCFPANEL_FS_RECT (tests: a pretend screen), and the size it had before.
static TEST_MAX: Mutex<Option<RECT>> = Mutex::new(None);
/// The pop-out board's window (0: none): the formation board in a window of their own ([`board_window`]), the game's
/// window then shows the picture, the pages and the cabinet buttons (`forza_ui::panel::popped_layout`).
static BOARD: AtomicIsize = AtomicIsize::new(0);
/// The board window's thread runs (from its start until its window is gone).
static BOARD_UP: AtomicBool = AtomicBool::new(false);
/// The board window's X (or Alt+F4) was clicked: the render thread docks the board back and saves board= off.
static BOARD_CLOSED: AtomicBool = AtomicBool::new(false);
/// The board window's place after a move or resize (restored rect, maximized), for panel.txt's board_rect=.
static BOARD_PLACE: Mutex<Option<[i32; 5]>> = Mutex::new(None);
static BOARD_FS: fullscreen::Slot = Mutex::new(None);
/// The board is drawn in the board window this frame (its egui takes that window's input).
static POPPED: AtomicBool = AtomicBool::new(false);
/// The board window is in Windows' move/size loop: its swap chain is kept until the size settles.
static BOARD_SIZING: AtomicBool = AtomicBool::new(false);
/// Our own window message: maximized (wParam 1) or the game's size (0), done on the window's thread.
const WM_FORZA_FS: u32 = WM_APP + 0x57;
/// To the board window from the render thread, once it let go of the window's swap chain: the window goes.
const WM_FORZA_DOCK: u32 = WM_APP + 0x59;
/// The projector's window made resizable, done on the window's thread.
const WM_FORZA_PROJ: u32 = WM_APP + 0x58;

/// Milliseconds since the DLL loaded.
fn ms() -> u32 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u32
}

/// Overlay and capture hooks other programs load into games; they hook the same Present we do. Discord's capture hook
/// blacked a seat's picture while its owner streamed the game (2026-10-22).
const FOREIGN_HOOKS: &[&str] = &[
    "DiscordHook.dll",         // Discord overlay and Go Live
    "GameOverlayRenderer.dll", // Steam overlay
    "graphics-hook32.dll",     // OBS game capture
    "RTSSHooks.dll",           // RivaTuner Statistics Server
    "nvspcap.dll",             // NVIDIA capture (ShadowPlay)
    "fraps32.dll",             // Fraps
];

/// The [`FOREIGN_HOOKS`] loaded in this process, comma-separated; empty if none.
fn foreign_hooks() -> String {
    FOREIGN_HOOKS
        .iter()
        // SAFETY: a lookup by name with a valid, NUL-terminated wide string; it takes no reference on the module.
        .filter(|n| unsafe { GetModuleHandleW(&windows::core::HSTRING::from(**n)) }.is_ok())
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

thread_local! {
    static IN_PRESENT: Cell<bool> = const { Cell::new(false) };
}

fn log(msg: &str) {
    let dir = EXE_DIR.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Into::into))
            .unwrap_or_default()
    });
    // the projector logs into the run's logs (WCCF_LOGS), as wccfpanel.c; seat 1 beside the program
    let path = match std::env::var_os("WCCF_LOGS").filter(|_| PROJECTOR.load(Relaxed)) {
        Some(logs) => PathBuf::from(logs).join("forza_panel_projector.log"),
        None => dir.join("forza_panel.log"),
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{msg}");
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllMain(inst: HINSTANCE, reason: u32, reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        let mut buf = [0u16; 1024];
        // SAFETY: our own module handle and a buffer we own
        let n = unsafe {
            let _ = DisableThreadLibraryCalls(inst.into());
            GetModuleFileNameW(Some(inst.into()), &mut buf)
        };
        let path = PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]));
        let _ = DLL_DIR.set(path.parent().map(Into::into).unwrap_or_default());
        // the device and the window are not ours to touch under the loader lock: a thread does it once we return
        std::thread::spawn(init);
    } else if reason == DLL_PROCESS_DETACH && reserved.is_null() {
        unload(); // FreeLibrary; at process exit (`reserved` set) nothing needs putting back
    }
    TRUE
}

fn init() {
    ms();
    std::thread::sleep(Duration::from_millis(300));
    let files = kit::Files::find(DLL_DIR.get().map_or(Path::new(""), PathBuf::as_path));
    exe_text::install(files.edition == theme::Edition::E1718, &files.data, log); // seat 1 and the projector
    if is_projector() {
        PROJECTOR.store(true, Relaxed);
        log(&format!(
            "forza-panel init (pid {}) - the PROJECTOR: money with commas, the frame wait and its frames for seat 1, no panels",
            std::process::id()
        ));
        let e1718 = files.edition == theme::Edition::E1718;
        let _ = FILES.set(files);
        money::money_install(e1718, true, log);
        patches::limiter_start(
            e1718,
            DLL_DIR.get().map_or(Path::new(""), PathBuf::as_path),
            log,
        ); // the frame wait sleeps instead of spinning a core
        hook_device(); // its window composes the picture at any size
        std::thread::spawn(projector_window);
        INIT_DONE.store(true, Relaxed);
        return;
    }
    log(&format!("forza-panel init (pid {})", std::process::id()));
    if std::env::var_os("WCCFPANEL_RELAYTEST").is_some() {
        net::relay_selftest(log); // the tests only
    }
    let e1718 = files.edition == theme::Edition::E1718;
    money::fmt_guard_install(e1718, log); // a bad % no longer stops seat 1
    money::money_install(e1718, false, log); // amounts with commas (after the guard: box texts go through it)
    patches::limiter_start(
        e1718,
        DLL_DIR.get().map_or(Path::new(""), PathBuf::as_path),
        log,
    );
    net::relay_install(e1718, log); // on a server: a match against another player through its relay
    net::ping_install(log); // on a server: the ping meter
    patches::dispenser_install(e1718, log); // no player card owed to a dispenser that is not there
    patches::free_play_install(e1718, log); // the original FREE PLAY: no coin needed anywhere
    patches::stall_tolerance_install(e1718, log); // a short network stall no longer drops a VS
    if std::env::var_os("WCCF_LOGS").is_some() {
        // ponytail: runs for the process's life (the game never unloads the panel; only a run sets WCCF_LOGS, never
        // the unload test)
        std::thread::spawn(link_watch);
    }
    let catalogue = cards::load_catalogue(&files.catalogue).unwrap_or_else(|e| {
        log(&format!(
            "no catalogue at {}: {e}",
            files.catalogue.display()
        ));
        Vec::new()
    });
    log(&format!(
        "edition {}, data {}, catalogue {} cards, pictures {}, keys {}",
        files.edition.label(),
        files.data.display(),
        catalogue.len(),
        files.cards.display(),
        files.keys.display()
    ));
    let _ = AFFINITY.set(affinity(e1718, &catalogue));
    let _ = CATALOGUE.set(catalogue.into());
    let _ = FILES.set(files);
    hook_device();
    // injected early? wait up to 60 s for the game's window
    for _ in 0..120 {
        if let Some(h) = game_window() {
            // not hooked (Present prologue unknown): no frames, so no subclass either
            if PRESENT.load(Relaxed) != 0 {
                // SAFETY: a window of this process; cross-thread subclassing within a process is allowed. The old
                // procedure is published first: wndproc may run the moment it is installed.
                unsafe {
                    OLD_PROC.store(GetWindowLongPtrW(h, GWLP_WNDPROC) as isize, Relaxed);
                    SetWindowLongPtrW(h, GWLP_WNDPROC, wndproc as *const () as usize as _);
                }
                WINDOW.store(h.0 as isize, Relaxed);
                log(&format!("input from window {:?}", h.0));
            }
            retitle(h); // after the subclass: it can block, and a WM_FORZA_FS posted on the first frame must arrive
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    INIT_DONE.store(true, Relaxed);
}

/// The players' affinity from the game's own files: `prog_data\player_data\` in the program's folder, the default
/// rule's tables from the program itself (2010-11; 2017-18 listed pairs only). Read-only; logged with counts.
/// Present and Reset detoured (seat 1: the panel; the projector: its frame copied for seat 1).
fn hook_device() {
    // SAFETY: the throwaway device is ours; the detours replace functions with the same signature
    match unsafe { device_functions() } {
        // SAFETY: as above
        Some((present, reset)) => unsafe {
            if detour(present, hook_present as *const () as usize, &PRESENT).is_some() {
                log("Present detoured");
            }
            let _ = detour(reset, hook_reset as *const () as usize, &RESET);
        },
        None => log("no throwaway Direct3D 9 device: not hooked"),
    }
}

/// The projector: its window made resizable and given the F11 / double-click / Esc handling ([`proj_wndproc`]).
fn projector_window() {
    // ponytail: runs once; the projector never unloads the panel
    let wnd = loop {
        if let Some(h) = game_window() {
            break h;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    // SAFETY: a window of this process; cross-thread subclassing within a process is allowed. The old procedure is
    // published first: proj_wndproc may run the moment it is installed.
    unsafe {
        OLD_PROC.store(GetWindowLongPtrW(wnd, GWLP_WNDPROC) as isize, Relaxed);
        SetWindowLongPtrW(wnd, GWLP_WNDPROC, proj_wndproc as *const () as usize as _);
    }
    WINDOW.store(wnd.0 as isize, Relaxed);
    // SAFETY: as above; the message is handled by proj_wndproc on the window's thread
    let _ = unsafe { PostMessageW(Some(wnd), WM_FORZA_PROJ, WPARAM(0), LPARAM(0)) };
    retitle(wnd); // "Projector - Forza-WCCF <edition>"; this is a worker thread
}

/// The pop-out board's window, on this thread until it goes: its own class, resizable, at its saved place (restored
/// rect and maximized, `board_rect=`; Windows moves a place left on a monitor that is gone back on screen), shown
/// without taking the focus from the game. Its messages run here; the render thread draws into it.
fn board_window(place: Option<[i32; 5]>, title: &str) {
    let title: Vec<u16> = title.encode_utf16().collect();
    // SAFETY: a window class and a window of our own, made and run on this thread; `title` outlives the call
    let made = unsafe {
        let inst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let class = w!("ForzaWCCFBoard");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(board_wndproc),
            hInstance: inst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc); // docked and out again: the class is there already, which is fine
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            windows::core::PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            960, // the board with its bench column (panel::desk_board_rect)
            1232,
            None,
            None,
            Some(inst),
            None,
        )
    };
    let h = match made {
        Ok(h) => h,
        Err(e) => {
            log(&format!(
                "board window: not made ({e}) - the board stays in the game's window"
            ));
            BOARD_UP.store(false, Relaxed);
            BOARD_CLOSED.store(true, Relaxed);
            return;
        }
    };
    // SAFETY: our own window, on its thread
    unsafe {
        match place {
            Some([x, y, w, hh, max]) => {
                let wp = WINDOWPLACEMENT {
                    length: size_of::<WINDOWPLACEMENT>() as u32,
                    showCmd: if max != 0 {
                        SW_SHOWMAXIMIZED.0 as u32
                    } else {
                        SW_SHOWNOACTIVATE.0 as u32
                    },
                    rcNormalPosition: RECT {
                        left: x,
                        top: y,
                        right: x + w,
                        bottom: y + hh,
                    },
                    ..Default::default()
                };
                let _ = SetWindowPlacement(h, &wp);
            }
            None => {
                // 960x1232, or less to stay inside the monitor's work area (the board letterboxed), centred on it
                let wa = projector::work_area(h, None);
                let (ww, wh) = (wa.right - wa.left, wa.bottom - wa.top);
                let (w, hh) = (960.min(ww), 1232.min(wh));
                let _ = SetWindowPos(
                    h,
                    None,
                    wa.left + (ww - w) / 2,
                    wa.top + (wh - hh) / 2,
                    w,
                    hh,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            }
        }
    }
    BOARD_SIZING.store(false, Relaxed);
    BOARD.store(h.0 as isize, Relaxed);
    log(&format!("board window: up at {place:?}"));
    let mut m = MSG::default();
    // SAFETY: this thread's own message loop
    unsafe {
        while GetMessageW(&mut m, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    let _ = BOARD.compare_exchange(h.0 as isize, 0, Relaxed, Relaxed);
    BOARD_UP.store(false, Relaxed);
    log("board window: gone");
}

/// The board window's place now (its restored rect, maximized), for panel.txt; not while it is full screen.
fn board_place(h: HWND) {
    if fullscreen::is_on(&BOARD_FS) {
        return;
    }
    let mut wp = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: plain queries on our own window
    unsafe {
        if GetWindowPlacement(h, &mut wp).is_ok() {
            let r = wp.rcNormalPosition;
            let max = i32::from(IsZoomed(h).as_bool());
            if let Ok(mut p) = BOARD_PLACE.lock() {
                *p = Some([r.left, r.top, r.right - r.left, r.bottom - r.top, max]);
            }
        }
    }
}

/// The board window's procedure: mouse and keys to egui (the game's keys reach the game anyway: the key driver reads
/// them while any window of the game is in front), the pause key, F11 full screen; its X or Alt+F4 docks the board
/// back, never closing the game; the Launcher's plain close is the game window's to answer.
unsafe extern "system" fn board_wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    thread_local! { static MOVING: Cell<bool> = const { Cell::new(false) }; }
    if m == WM_KILLFOCUS || (m == WM_ACTIVATEAPP && wp.0 == 0) {
        latch_up();
    }
    match m {
        WM_FORZA_DOCK => {
            // SAFETY: our own window, on its thread
            let _ = unsafe { DestroyWindow(h) };
            return LRESULT(0);
        }
        WM_DESTROY => {
            latch_up();
            BOARD_SIZING.store(false, Relaxed); // a window closed in its move loop never sees WM_EXITSIZEMOVE
            // SAFETY: this thread's own loop ends
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        // the Launcher's ordered stop closes every window: the game's ends the run, this one waits for it
        WM_CLOSE => return LRESULT(0),
        WM_SYSCOMMAND if (wp.0 as u32 & 0xFFF0) == SC_CLOSE => {
            BOARD_CLOSED.store(true, Relaxed);
            // a stopped panel draws nothing here any more and serves no request: the window goes now
            if DEAD.load(Relaxed) {
                // SAFETY: our own window, on its thread
                let _ = unsafe { DestroyWindow(h) };
            }
            return LRESULT(0);
        }
        // Alt shows the deck's keycaps: no menu on its release
        WM_SYSCOMMAND if (wp.0 as u32 & 0xFFF0) == SC_KEYMENU => return LRESULT(0),
        WM_ENTERSIZEMOVE => {
            MOVING.set(true);
            BOARD_SIZING.store(true, Relaxed);
        }
        WM_EXITSIZEMOVE => {
            MOVING.set(false);
            BOARD_SIZING.store(false, Relaxed);
            board_place(h);
        }
        WM_SIZE
            if !MOVING.get() && (wp.0 as u32 == SIZE_MAXIMIZED || wp.0 as u32 == SIZE_RESTORED) =>
        {
            board_place(h)
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_KEYDOWN | WM_KEYUP if wp.0 == 0x7A => {
            if m == WM_KEYDOWN && lp.0 & (1 << 30) == 0 {
                toggle_fullscreen(h, &BOARD_FS);
            }
            return LRESULT(0);
        }
        WM_KEYDOWN if wp.0 == 0x1B && fullscreen::leave(h, &BOARD_FS, None) => {
            log_fullscreen(h, false);
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            // SAFETY: capture for our own window, from its own thread
            unsafe {
                SetCapture(h);
            }
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP if wp.0 & 0x13 == 0 => {
            // SAFETY: as above
            let _ = unsafe { ReleaseCapture() };
        }
        0x0215 if lp.0 != 0 && lp.0 != h.0 as isize => push(true, vec![Event::PointerGone]),
        _ => {}
    }
    if matches!(m, WM_KEYDOWN | WM_KEYUP)
        && let Some(r) = pause::message(true, m, wp, lp)
    {
        return r;
    }
    if !DEAD.load(Relaxed) && POPPED.load(Relaxed) && input(true, m, wp, lp) && m != WM_SYSKEYDOWN {
        return LRESULT(0);
    }
    // SAFETY: the default procedure for our own window
    unsafe { DefWindowProcW(h, m, wp, lp) }
}

fn affinity(e1718: bool, catalogue: &[Card]) -> Option<Arc<cards::links::Affinity>> {
    let exe = std::env::current_exe().ok()?;
    let game = exe.parent()?;
    let layout = if e1718 {
        cards::links::E1718
    } else {
        cards::links::E1011
    };
    match cards::links::Affinity::load(layout, game, &exe, catalogue) {
        Ok(a) => {
            let (pairs, rule) = a.counts();
            log(&format!(
                "links: affinity {pairs} listed pair(s) of {} player(s), default rule {}",
                a.people.len(),
                if rule { "on" } else { "off" }
            ));
            Some(Arc::new(a))
        }
        Err(e) => {
            log(&format!("links: no affinity ({e})"));
            None
        }
    }
}

/// The projector, as `wccfpanel.c` `is_projector`: WCCFPANEL_ROLE (projector or not), else a program in a folder
/// named "extracted", else the program's folder is WCCF_GAME.
fn is_projector() -> bool {
    if let Ok(r) = std::env::var("WCCFPANEL_ROLE") {
        return r.eq_ignore_ascii_case("projector");
    }
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    else {
        return false;
    };
    if dir
        .file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("extracted"))
    {
        return true;
    }
    std::env::var_os("WCCF_GAME")
        .and_then(|g| std::path::absolute(g).ok())
        .is_some_and(|g| {
            let g = g.to_string_lossy();
            let g = g.trim_end_matches('\\');
            g.eq_ignore_ascii_case(&dir.to_string_lossy())
        })
}

/// The server link, watched all the time (`wccfpanel.c` `link_watch`): every 2 s this process's link to the server's
/// control port; one dated line in WCCF_LOGS\server_link.txt when it comes up ("UP 1.2.3.4") and when it drops ("LOST
/// (it was up N s)", or ENDED after the player closed the window: no fault). The run reads it (launcher run::rules).
fn link_watch() {
    use forza_ui::panel::Link;
    let Some(dir) = std::env::var_os("WCCF_LOGS") else {
        return;
    };
    let path = PathBuf::from(dir).join("server_link.txt");
    // the tests' link (WCCFPANEL_LINKTEST=ADDRESS:PORT): this process connects there itself
    let _test = std::env::var("WCCFPANEL_LINKTEST")
        .ok()
        .and_then(|a| std::net::TcpStream::connect(a).ok());
    let mut up_at: Option<Instant> = None;
    loop {
        let now = match kit::server_link() {
            Link::Unknown => None,
            Link::Up(peer) => Some(Some(peer)),
            Link::Down => Some(None),
        };
        if let Some(now) = now
            && now.is_some() != up_at.is_some()
        {
            // SAFETY: a plain clock query
            let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
            let stamp = format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
            );
            LINK_LOST.store(now.is_none() && !CLOSING.load(Relaxed), Relaxed);
            let line = match (&now, up_at) {
                (Some(peer), _) => format!("{stamp} UP {peer}\n"),
                (None, at) => format!(
                    "{stamp} {} (it was up {} s)\n",
                    if CLOSING.load(Relaxed) {
                        "ENDED"
                    } else {
                        "LOST"
                    },
                    at.map_or(0, |a| a.elapsed().as_secs())
                ),
            };
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = f.write_all(line.as_bytes());
            }
            up_at = now.map(|_| Instant::now());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// WM_CLOSE (`wccfpanel.c` `close_allowed`): during a card session (the club card or a manager transfer's new card,
/// open since this run started) a close only opens the close dialog; the window closes when QUIT is clicked in it
/// (`CLOSE_OK`). Further closes while the dialog is open do nothing. No card session: the window closes.
fn close_allowed() -> bool {
    if CLOSE_OK.swap(false, Relaxed) {
        log("close: quit chosen in the dialog - the game ends");
        return true;
    }
    if CLOSE_ASK.load(Relaxed) {
        log("close: the dialog is open - ignored");
        return false;
    }
    if !card_open() {
        log("close: the window closes - the game ends");
        return true;
    }
    CLOSE_SINCE.store(ms(), Relaxed);
    CLOSE_ASK.store(true, Relaxed);
    log("close: a card session is open - dialog opened, not closed");
    false
}

/// A card session open (the club card or a manager transfer's new card, since this run started): from the card's read
/// at a match's start to its locker-room save.
fn card_open() -> bool {
    FILES.get().is_some_and(|f| {
        use forza::card;
        let since = stamp(&f.data.join("running.json"));
        let save = f.data.join("save");
        card::club_session(&save.join(card::SLOT), &save.join(card::NEW), since).0
            == card::Session::Open
    })
}

/// This seat plays a match (IMPORT LINEUP is refused then, the owner's rule); the decision and its inputs are logged.
fn in_match() -> bool {
    let e1718 = FILES
        .get()
        .is_some_and(|f| f.edition == theme::Edition::E1718);
    let seat = if e1718 { pause::seat_state() } else { None };
    let shop = pause::shop_state(e1718);
    let card = card_open();
    let refuse = match_now(e1718, seat, shop, card);
    log(&format!(
        "import: match check - seat state {seat:?}, shop state {shop:?}, card session {card} -> {}",
        if refuse { "in a match" } else { "free" }
    ));
    refuse
}

/// 2017-18: the seat's own sequence state is the match one (the shop state is the whole shop's: another seat's
/// match sets it too); the shop state MATCHING-GAME_RESULT_WAIT (4-6) only when the seat state cannot be read.
/// 2010-11: that shop state. Either: a card session open (2010-11's club card).
fn match_now(e1718: bool, seat: Option<u32>, shop: Option<u32>, card: bool) -> bool {
    let playing = match seat {
        Some(s) if e1718 => s == pause::SEAT_MATCH,
        _ => matches!(shop, Some(4..=6)),
    };
    playing || card
}

/// The clipboard's text, at most 64K characters; `None`: no text there, or another program holds the clipboard (tried
/// twice). `WCCFPANEL_IMPORTTEST=FILE` reads that file instead (fakegame: the player's clipboard is left alone).
fn clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    if let Some(p) = std::env::var_os("WCCFPANEL_IMPORTTEST") {
        return std::fs::read_to_string(p).ok();
    }
    const CF_UNICODETEXT: u32 = 13;
    // SAFETY: the handle is the clipboard's own, read only while the clipboard is open and the memory locked
    unsafe {
        if OpenClipboard(None).is_err() {
            std::thread::sleep(Duration::from_millis(30));
            OpenClipboard(None).ok()?;
        }
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g).cast::<u16>();
            if p.is_null() {
                return None;
            }
            let all = std::slice::from_raw_parts(p, (GlobalSize(g) / 2).min(65536));
            let n = all.iter().position(|&c| c == 0).unwrap_or(all.len());
            let t = String::from_utf16_lossy(&all[..n]);
            let _ = GlobalUnlock(g);
            Some(t)
        });
        let _ = CloseClipboard();
        text
    }
}

/// `text` on the clipboard as Unicode text; `false`: the clipboard could not be opened or taken.
/// `WCCFPANEL_IMPORTTEST=FILE` writes `FILE.copy` instead (fakegame: the player's clipboard is left alone).
fn set_clipboard_text(text: &str) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    if let Some(p) = std::env::var_os("WCCFPANEL_IMPORTTEST") {
        let mut p = PathBuf::from(p).into_os_string();
        p.push(".copy");
        return std::fs::write(p, text).is_ok();
    }
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text
        .replace('\n', "\r\n")
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: a fresh movable block filled while locked; once SetClipboardData takes it the clipboard owns it, else it
    // is freed here
    unsafe {
        let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else {
            return false;
        };
        let p = GlobalLock(g).cast::<u16>();
        if p.is_null() {
            let _ = GlobalFree(Some(g));
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        let _ = GlobalUnlock(g);
        if OpenClipboard(None).is_err() {
            std::thread::sleep(Duration::from_millis(30));
            if OpenClipboard(None).is_err() {
                let _ = GlobalFree(Some(g));
                return false;
            }
        }
        let ok =
            EmptyClipboard().is_ok() && SetClipboardData(CF_UNICODETEXT, Some(HANDLE(g.0))).is_ok();
        if !ok {
            let _ = GlobalFree(Some(g));
        }
        let _ = CloseClipboard();
        ok
    }
}

/// The screen's dpi as this process sees it: the real one when the game runs DPI-aware (the Launcher starts it with
/// `__COMPAT_LAYER=HighDpiAware`), 96 when Windows stretches it. Asked once: a system-aware process keeps the dpi it
/// started with (no WM_DPICHANGED), so a change of the scaling takes a new start.
fn screen_dpi() -> i32 {
    static DPI: OnceLock<i32> = OnceLock::new();
    *DPI.get_or_init(|| {
        use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, LOGPIXELSY, ReleaseDC};
        // SAFETY: the screen's device context, given back at once
        let dpi = unsafe {
            let dc = GetDC(None);
            let dpi = GetDeviceCaps(Some(dc), LOGPIXELSY);
            ReleaseDC(None, dc);
            dpi
        };
        log(&format!("screen: {dpi} dpi"));
        if dpi > 0 { dpi } else { 96 }
    })
}

/// WCCFPANEL_FS_RECT=x,y,w,h (tests): a pretend screen off every real monitor.
fn test_screen() -> Option<RECT> {
    let v = std::env::var("WCCFPANEL_FS_RECT").ok()?;
    let n: Vec<i32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    let [x, y, w, h] = n[..] else { return None };
    (w > 0 && h > 0).then_some(RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    })
}

/// Maximized with its title bar kept (minimize, maximize/restore and close), or the game's own size, as `wccfpanel.c`
/// `set_maximized`; on the window's thread.
fn set_maximized(h: HWND, on: bool) {
    // SAFETY: window calls on a window of this process, from its own thread
    unsafe {
        let st = GetWindowLongW(h, GWL_STYLE);
        let want = (st & !(WS_POPUP.0 as i32))
            | (WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX).0 as i32;
        if want != st {
            SetWindowLongW(h, GWL_STYLE, want);
            let _ = SetWindowPos(
                h,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOZORDER
                    | SWP_NOOWNERZORDER
                    | SWP_NOACTIVATE
                    | SWP_FRAMECHANGED,
            );
        }
        if let Some(m) = test_screen() {
            let Ok(mut saved) = TEST_MAX.lock() else {
                return;
            };
            let flags = SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE;
            if on && saved.is_none() {
                let mut r = RECT::default();
                let _ = GetWindowRect(h, &mut r);
                *saved = Some(r);
                let _ = SetWindowPos(
                    h,
                    None,
                    m.left,
                    m.top,
                    m.right - m.left,
                    m.bottom - m.top,
                    flags,
                );
            } else if !on && let Some(r) = saved.take() {
                let _ = SetWindowPos(
                    h,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    flags,
                );
            }
        } else {
            let _ = ShowWindow(h, if on { SW_MAXIMIZE } else { SW_RESTORE });
        }
        let mut cr = RECT::default();
        let _ = GetClientRect(h, &mut cr);
        log(&format!(
            "{}: client now {}x{}",
            if on { "maximized" } else { "window" },
            cr.right,
            cr.bottom
        ));
    }
}

/// F11 on `slot`'s window: borderless full screen, or back; logs the client size.
fn toggle_fullscreen(h: HWND, slot: &fullscreen::Slot) {
    let on = fullscreen::toggle(h, slot, test_screen());
    log_fullscreen(h, on);
}

fn log_fullscreen(h: HWND, on: bool) {
    let mut cr = RECT::default();
    // SAFETY: a plain window query
    let _ = unsafe { GetClientRect(h, &mut cr) };
    let who = if PROJECTOR.load(Relaxed) {
        "projector: "
    } else {
        ""
    };
    log(&format!(
        "{who}{}: client now {}x{}",
        if on { "fullscreen" } else { "window" },
        cr.right,
        cr.bottom
    ));
}

/// The projector's window procedure: its frame (once), F11 and a double-click for borderless full screen, Esc to leave;
/// everything else is the game's.
unsafe extern "system" fn proj_wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // the window's own thread only
    thread_local! { static LAST_CLICK: Cell<(u32, i32, i32)> = const { Cell::new((0, 0, 0)) }; }
    match m {
        WM_FORZA_PROJ => {
            projector::frame(h, test_screen(), log);
            return LRESULT(0);
        }
        WM_KEYDOWN | WM_KEYUP if wp.0 == 0x7A => {
            if m == WM_KEYDOWN && lp.0 & (1 << 30) == 0 {
                toggle_fullscreen(h, &PROJ_FS);
            }
            return LRESULT(0);
        }
        // Esc leaves full screen and still goes on to the game
        WM_KEYDOWN if wp.0 == 0x1B => {
            if fullscreen::leave(h, &PROJ_FS, test_screen()) {
                log_fullscreen(h, false);
            }
        }
        WM_LBUTTONDBLCLK => {
            LAST_CLICK.set((0, 0, 0));
            toggle_fullscreen(h, &PROJ_FS);
            return LRESULT(0);
        }
        // a window class without CS_DBLCLKS: two presses within the double-click time and distance
        WM_LBUTTONDOWN => {
            let (x, y) = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
            // SAFETY: plain queries
            let (now, gap, dx, dy) = unsafe {
                (
                    GetTickCount(),
                    GetDoubleClickTime(),
                    GetSystemMetrics(SM_CXDOUBLECLK) / 2,
                    GetSystemMetrics(SM_CYDOUBLECLK) / 2,
                )
            };
            let (lt, lx, ly) = LAST_CLICK.get();
            if lt != 0
                && now.wrapping_sub(lt) <= gap
                && (x - lx).abs() <= dx
                && (y - ly).abs() <= dy
            {
                LAST_CLICK.set((0, 0, 0));
                toggle_fullscreen(h, &PROJ_FS);
                return LRESULT(0);
            }
            LAST_CLICK.set((now, x, y));
        }
        _ => {}
    }
    // SAFETY: the procedure we replaced, with the message as it came
    unsafe {
        CallWindowProcW(
            std::mem::transmute::<isize, WNDPROC>(OLD_PROC.load(Relaxed)),
            h,
            m,
            wp,
            lp,
        )
    }
}

/// FreeLibrary: everything back as it was, on the thread that unloads us. The real game never unloads the panel;
/// fakegame does it between frames, so no Present or window message is running in our code.
fn unload() {
    latch_up(); // one SendInput under the loader lock: user32 is loaded already, and a key left down is worse
    if !INIT_DONE.load(Relaxed) {
        log("unloaded before init finished: the init thread would run in freed code"); // ponytail: no wait under the loader lock
    }
    let h = WINDOW.swap(0, Relaxed);
    if h != 0 {
        // out of full screen first: no handler is left to leave it
        for slot in [&PANEL_FS, &PROJ_FS] {
            fullscreen::leave(HWND(h as *mut c_void), slot, test_screen());
        }
        // SAFETY: the procedure we replaced, back on the same window
        unsafe {
            SetWindowLongPtrW(
                HWND(h as *mut c_void),
                GWLP_WNDPROC,
                OLD_PROC.load(Relaxed) as _,
            )
        };
    }
    for (target, bytes, tramp) in PATCHES
        .lock()
        .map(|mut p| std::mem::take(&mut *p))
        .unwrap_or_default()
    {
        // SAFETY: the five bytes we overwrote, at the address we overwrote; nothing runs the trampoline any more
        unsafe {
            write_code(target, &bytes);
            let _ = VirtualFree(tramp as *mut c_void, 0, MEM_RELEASE);
        }
    }
    if let Ok(mut r) = RENDER.lock()
        && let Some(r) = r.take()
    {
        r.cost(true);
    }
    // after the render state (and the board window's swap chain with it) is gone
    let b = BOARD.swap(0, Relaxed);
    if b != 0 {
        // shortcut: the board window's thread is told to go, not waited for (no wait under the loader lock); the game
        // never unloads the panel and fakegame's unload test runs without the pop-out board
        // SAFETY: a message to a window of this process
        let _ = unsafe {
            PostMessageW(
                Some(HWND(b as *mut c_void)),
                WM_FORZA_DOCK,
                WPARAM(0),
                LPARAM(0),
            )
        };
        log("unloaded with the board window open: told to close");
    }
    log("unloaded");
}

/// The shared Present and Reset functions, read from a throwaway device's table.
unsafe fn device_functions() -> Option<(usize, usize)> {
    // SAFETY: a window and a device of our own, gone before we return; the table pointer is the COM object's first field
    unsafe {
        let d3d = Direct3DCreate9(D3D_SDK_VERSION)?;
        let hw = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!(""),
            WS_OVERLAPPED,
            0,
            0,
            8,
            8,
            None,
            None,
            None,
            None,
        )
        .ok()?;
        let mut pp = D3DPRESENT_PARAMETERS {
            BackBufferWidth: 8,
            BackBufferHeight: 8,
            BackBufferFormat: D3DFMT_X8R8G8B8,
            BackBufferCount: 1,
            SwapEffect: D3DSWAPEFFECT_DISCARD,
            hDeviceWindow: hw,
            Windowed: TRUE,
            ..Default::default()
        };
        let fpu = D3DCREATE_FPU_PRESERVE as u32;
        let mut dev = None;
        for (t, f) in [
            (
                D3DDEVTYPE_HAL,
                D3DCREATE_HARDWARE_VERTEXPROCESSING as u32 | fpu,
            ),
            (
                D3DDEVTYPE_HAL,
                D3DCREATE_SOFTWARE_VERTEXPROCESSING as u32 | fpu,
            ),
            (
                D3DDEVTYPE_REF,
                D3DCREATE_SOFTWARE_VERTEXPROCESSING as u32 | fpu,
            ),
        ] {
            if d3d
                .CreateDevice(D3DADAPTER_DEFAULT, t, hw, f, &mut pp, &mut dev)
                .is_ok()
                && dev.is_some()
            {
                break;
            }
        }
        let fns = dev.map(|d| {
            let table = *(d.as_raw() as *const *const usize);
            (*table.add(17), *table.add(16)) // Present, Reset
        });
        let _ = DestroyWindow(hw);
        fns
    }
}

/// The C panel's five-byte detour: `mov edi,edi; push ebp; mov ebp,esp` (the hot-patch prologue every Windows
/// Present and Reset have) becomes a jump to `to`; the trampoline runs those bytes and jumps back. Any other
/// prologue is left alone (no panel, the game unharmed). The trampoline is stored in `slot` before the jump is written.
unsafe fn detour(target: usize, to: usize, slot: &AtomicUsize) -> Option<usize> {
    const PROLOGUE: [u8; 5] = [0x8B, 0xFF, 0x55, 0x8B, 0xEC];
    // SAFETY: `target` is a function's first byte; the trampoline is our own executable page
    unsafe {
        let bytes = *(target as *const [u8; 5]);
        if bytes != PROLOGUE {
            log(&format!(
                "prologue {bytes:02X?} at {target:#x} is not the hot-patch stub: not hooked (safe)"
            ));
            return None;
        }
        let tramp =
            VirtualAlloc(None, 16, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
        if tramp == 0 {
            return None;
        }
        let back = jmp(tramp + 5, target + 5);
        std::ptr::copy_nonoverlapping([bytes, back].as_flattened().as_ptr(), tramp as *mut u8, 10);
        slot.store(tramp, Relaxed); // before the jump exists: a call in between must find the trampoline
        write_code(target, &jmp(target, to));
        PATCHES.lock().ok()?.push((target, bytes, tramp));
        Some(tramp)
    }
}

/// `jmp rel32` placed at `at`, to `to`.
fn jmp(at: usize, to: usize) -> [u8; 5] {
    let [a, b, c, d] = (to.wrapping_sub(at + 5) as u32).to_le_bytes();
    [0xE9, a, b, c, d]
}

unsafe fn write_code(at: usize, bytes: &[u8; 5]) {
    let mut old = PAGE_PROTECTION_FLAGS(0);
    // SAFETY: five bytes of a function's code, made writable for the copy
    unsafe {
        if VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old).is_ok() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, 5);
            let _ = VirtualProtect(at as *const c_void, 5, old, &mut old);
            let _ = FlushInstructionCache(GetCurrentProcess(), Some(at as *const c_void), 5);
        }
    }
}

unsafe extern "system" fn hook_present(
    dev: *mut c_void,
    src: *const RECT,
    dst: *const RECT,
    wnd: HWND,
    dirty: *const RGNDATA,
) -> HRESULT {
    // SAFETY: the trampoline runs the original Present with the same arguments
    let orig: PresentFn = unsafe { std::mem::transmute(PRESENT.load(Relaxed)) };
    if IN_PRESENT.get() || DEAD.load(Relaxed) {
        DRAWN.store(false, Relaxed);
        // SAFETY: as above
        return unsafe { orig(dev, src, dst, wnd, dirty) };
    }
    IN_PRESENT.set(true);
    if PROJECTOR.load(Relaxed) {
        // the projector: nothing drawn; its picture composed into the resized window, else the game's own Present
        let shown = std::panic::catch_unwind(|| {
            // SAFETY: the game's device, alive for the call; we are on its render thread
            let dev = unsafe { IDirect3DDevice9::from_raw_borrowed(&dev) }?;
            let wnd = HWND(WINDOW.load(Relaxed) as *mut c_void);
            let mut v = VIEW.lock().ok()?;
            // SAFETY: as above
            unsafe {
                v.get_or_insert_with(Default::default)
                    .present(dev, wnd, log)
            }
        })
        .unwrap_or_else(|_| {
            DEAD.store(true, Relaxed);
            log("projector: a frame panicked - the picture stays at the game's size");
            None
        });
        IN_PRESENT.set(false);
        // SAFETY: as above
        return shown.unwrap_or_else(|| unsafe { orig(dev, src, dst, wnd, dirty) });
    }
    // the pause key or button: a pause holds the game right here, before its frame is shown
    if std::panic::catch_unwind(|| {
        // SAFETY: the game's device, alive for the call; we are on its render thread
        if let Some(dev) = unsafe { IDirect3DDevice9::from_raw_borrowed(&dev) } {
            pause::present(dev, wnd);
        }
    })
    .is_err()
    {
        pause::HELD.store(false, Relaxed);
        log("pause: panicked - the game goes on");
    }
    // the game's own parts, every frame (each acts on its own clock): as wccfpanel.c hook_present
    patches::frame_stats_tick(log);
    patches::dispenser_tick(log);
    patches::free_play_tick(log);
    // a panic must not cross into the game: the panel stops, the game goes on
    let shown = std::panic::catch_unwind(|| {
        // SAFETY: the game's device, alive for the call; we are on its render thread
        let dev = unsafe { IDirect3DDevice9::from_raw_borrowed(&dev) }?;
        let mut r = RENDER.lock().ok()?;
        // SAFETY: as above
        unsafe { r.get_or_insert_with(Render::new).frame(dev, wnd) }
    })
    .unwrap_or_else(|_| {
        DEAD.store(true, Relaxed);
        latch_up(); // no frame is left to let it go
        log("a frame panicked: the panel stops drawing");
        None
    });
    IN_PRESENT.set(false);
    DRAWN.store(shown.is_some(), Relaxed);
    // SAFETY: as above: not shown on our swap chain, the game's own Present shows its frame
    shown.unwrap_or_else(|| unsafe { orig(dev, src, dst, wnd, dirty) })
}

unsafe extern "system" fn hook_reset(dev: *mut c_void, pp: *mut D3DPRESENT_PARAMETERS) -> HRESULT {
    if let Ok(mut r) = RENDER.lock()
        && let Some(r) = r.as_mut()
    {
        r.release_default();
    }
    if let Ok(mut v) = VIEW.lock()
        && let Some(v) = v.as_mut()
    {
        v.release();
    }
    log("device reset");
    // SAFETY: the original Reset through its trampoline
    unsafe { std::mem::transmute::<usize, ResetFn>(RESET.load(Relaxed))(dev, pp) }
}

/// Everything the render thread keeps: egui, the painter, our swap chain and the game frame's copy.
struct Render {
    ctx: egui::Context,
    /// The board window's egui (popped out), its painter: the same panel state, drawn there as the board alone.
    board_ctx: egui::Context,
    board_painter: painter::Painter,
    st: PanelState,
    table: TableFile,
    /// The table as last read or written; `st.board.cards[i]` is `placed[i]`.
    placed: Vec<Placed>,
    /// A cabinet button's key that is down, since when; the mouse still held on it.
    key: Option<(u32, Instant)>,
    /// The key the latch holds down (0: none): `LATCH_VK` as this thread last set it.
    latch: u32,
    mouse_down: bool,
    /// The page shown last frame (KEYS opening reads keys.txt again and looks for controllers).
    last_page: panel::Page,
    /// The controllers, while KEYS is open, and the controls held at the last look.
    pads: Option<forza::keys::Pads>,
    pads_held: std::collections::BTreeSet<u32>,
    /// When the controllers were last looked for.
    pads_scan: Instant,
    /// SETTINGS: when NOW was read; when RESTART NOW and SAVE LOGS were asked; SAVE LOGS' answer file as last read.
    status_at: Option<Instant>,
    restart_at: Option<Instant>,
    logs_at: Option<Instant>,
    /// The crash screen's report request, when it was written.
    report_at: Option<Instant>,
    logs_stamp: Option<std::time::SystemTime>,
    /// The deal: when the club card was last looked at, and its session then.
    deal_at: Instant,
    deal_last: Option<forza::card::Session>,
    /// Links: when the club card was last looked at, and its time then.
    links_at: Option<Instant>,
    links_stamp: Option<Option<std::time::SystemTime>>,
    /// CLUB CARD: when club_view.txt was last looked at, and its time then.
    club_at: Option<Instant>,
    club_stamp: Option<std::time::SystemTime>,
    painter: painter::Painter,
    start: Instant,
    sc: Option<(IDirect3DSwapChain9, u32, u32, HWND)>,
    sc_failed: bool,
    /// The board window's swap chain (the pop-out board), and whether its failure was logged.
    board_sc: Option<(IDirect3DSwapChain9, u32, u32, HWND)>,
    board_warned: bool,
    /// The window that carries WCCF_TYPING while a page is open (the game's).
    typing: isize,
    game: Option<(IDirect3DTexture9, IDirect3DSurface9, u32, u32)>,
    max_tex: usize,
    dump: bool,
    size: (u32, u32),
    held: u32,
    frames: u32,
    hooks: String,
    spent: Duration,
    worst: Duration,
}

// SAFETY: the Direct3D objects are used only on the game's render thread (behind RENDER's lock); Reset and unload,
// which also release them, are called on that thread too
unsafe impl Send for Render {}

impl Render {
    fn new() -> Render {
        let ctx = egui::Context::default();
        let board_ctx = egui::Context::default();
        let files = FILES.get_or_init(|| kit::Files::find(Path::new("")));
        let dir = font_dir();
        if let Err(e) = theme::install(&ctx, files.edition, &dir) {
            log(&format!(
                "fonts from {}: {e} (egui's own fonts used)",
                dir.display()
            ));
        }
        let _ = theme::install(&board_ctx, files.edition, &dir);
        forza_ui::board::set_picture_dir(&ctx, Some(files.cards.clone()));
        forza_ui::board::set_picture_dir(&board_ctx, Some(files.cards.clone()));
        let mut table = TableFile::in_dir(EXE_DIR.get().map_or(Path::new(""), PathBuf::as_path));
        let placed = table.read().unwrap_or_default();
        log(&format!(
            "board: table {} - {} card(s)",
            table.table.display(),
            placed.len()
        ));
        let lang = files.lang();
        let cards = CATALOGUE.get().cloned().unwrap_or_else(|| Arc::new([]));
        let mut st = PanelState::new(files.edition, lang, cards::board(&placed, &cards), cards);
        st.file = files.panel_file();
        st.hover_delay = st.file.hover_ms.map(|ms| ms as f32 / 1000.0);
        st.address = st.file.server.clone();
        (st.keys, st.pads) = files.load_keys();
        st.picture = Some(GAME);
        log(&format!(
            "language {}, hover delay {}",
            lang.code(),
            st.hover_delay.map_or("off".into(), |s| format!("{s} s"))
        ));
        Render {
            ctx,
            board_ctx,
            board_painter: painter::Painter::default(),
            st,
            table,
            placed,
            key: None,
            latch: 0,
            mouse_down: false,
            last_page: panel::Page::Game,
            pads: None,
            pads_held: Default::default(),
            pads_scan: Instant::now(),
            status_at: None,
            restart_at: None,
            logs_at: None,
            report_at: None,
            // an answer from an earlier run is not shown
            logs_stamp: stamp(&files.request("savelogs.result")),
            links_at: None,
            links_stamp: None,
            club_at: None,
            club_stamp: None,
            deal_at: Instant::now(),
            deal_last: None,
            painter: painter::Painter::default(),
            start: Instant::now(),
            sc: None,
            sc_failed: false,
            board_sc: None,
            board_warned: false,
            typing: 0,
            game: None,
            max_tex: 2048,
            dump: std::env::var_os("FORZA_PANEL_DUMP").is_some_and(|v| v == "1"),
            size: (0, 0),
            held: 0,
            frames: 0,
            hooks: String::new(),
            spent: Duration::ZERO,
            worst: Duration::ZERO,
        }
    }

    /// DEFAULT-pool objects go before a reset; the next frame makes them again.
    fn release_default(&mut self) {
        self.sc = None;
        self.board_sc = None;
        self.sc_failed = false;
        self.game = None;
        self.painter.reset();
        self.board_painter.reset();
    }

    /// One frame on our swap chain; `None` = not shown, the game's own Present runs.
    unsafe fn frame(&mut self, dev: &IDirect3DDevice9, wnd: HWND) -> Option<HRESULT> {
        let t0 = Instant::now();
        // the board window's close, dock and start first: served even while the game's window is not drawn
        let board = self.board_window();
        // SAFETY: device calls with out-parameters we own
        unsafe {
            let target = if wnd.is_invalid() {
                device_window(dev)?
            } else {
                wnd
            };
            let mut cr = RECT::default();
            if !drawable(target) || GetClientRect(target, &mut cr).is_err() {
                // the game's window minimized: the board window still draws (the game's own Present shows its frame)
                let mut popped = false;
                if let Some(bw) = board
                    && let Ok(sb) = dev.CreateStateBlock(D3DSBT_ALL)
                {
                    let mut vp = D3DVIEWPORT9::default();
                    let _ = dev.GetViewport(&mut vp);
                    popped = self.board_frame(dev, bw);
                    let _ = dev.SetViewport(&vp);
                    let _ = sb.Apply();
                }
                POPPED.store(popped, Relaxed);
                self.st.set_popped(popped);
                return None;
            }
            let (w, h) = (cr.right as u32, cr.bottom as u32);
            let sc = self.swap_chain(dev, target, w, h)?;
            let sb = dev.CreateStateBlock(D3DSBT_ALL).ok()?;
            let mut vp = D3DVIEWPORT9::default();
            let _ = dev.GetViewport(&mut vp);
            // popped out: the board in its own window, the game's window the picture, the pages and the buttons; a board
            // window that could not be drawn (minimized, tiny, no swap chain) gives the board back to the game's window
            // for the frame
            let popped = board.is_some_and(|bw| self.board_frame(dev, bw));
            POPPED.store(popped, Relaxed);
            self.st.set_popped(popped);
            let drawn = self.compose(dev, &sc, w, h, true);
            let _ = dev.SetViewport(&vp);
            let _ = sb.Apply();
            drawn?;
            let spent = t0.elapsed();
            self.spent += spent;
            self.worst = self.worst.max(spent);
            self.frames += 1;
            if self.frames == 1 {
                log(&format!("first frame shown ({w}x{h})"));
                SHOWN.store(true, Relaxed);
                // shown at the window's own size, so a bigger window is safe: maximized once, at the start
                // (WCCFPANEL_FULLSCREEN=0 keeps the game's size: the tests)
                if std::env::var("WCCFPANEL_FULLSCREEN").is_ok_and(|v| v.starts_with('0')) {
                    log("fullscreen at start: no");
                } else {
                    // a message to the game's window, handled by our window procedure on its thread
                    let _ = PostMessageW(Some(target), WM_FORZA_FS, WPARAM(1), LPARAM(0));
                }
            }
            if self.frames.is_multiple_of(600) {
                self.cost(false);
            }
            if self.frames == 1 || self.frames.is_multiple_of(600) {
                let hooks = foreign_hooks();
                // the first frame always says it ("none" included), later frames only a change
                if self.frames == 1 || hooks != self.hooks {
                    log(&format!(
                        "foreign hooks: {}",
                        if hooks.is_empty() { "none" } else { &hooks }
                    ));
                    self.hooks = hooks;
                }
            }
            Some(
                match sc.Present(
                    std::ptr::null(),
                    std::ptr::null(),
                    HWND::default(),
                    std::ptr::null(),
                    0,
                ) {
                    Ok(()) => HRESULT(0),
                    Err(e) => e.code(),
                },
            )
        }
    }

    /// The pop-out board as panel.txt asks: its window started on a thread of its own, or docked back (the window's X,
    /// or the setting turned off); its place kept for panel.txt. `Some`: the board window is up, the panel draws there.
    fn board_window(&mut self) -> Option<HWND> {
        use forza_ui::widgets::Tone;
        if BOARD_CLOSED.swap(false, Relaxed) && self.st.file.board_window {
            log("board window: closed - the board docks back into the game's window");
            self.st.file.board_window = false;
            self.st.file_changed = true;
            self.st.say(Tone::Done, "settings.msg_board_docked");
        }
        if let Some(place) = BOARD_PLACE.lock().ok().and_then(|mut p| p.take())
            && self.st.file.board_rect != Some(place)
        {
            self.st.file.board_rect = Some(place);
            self.st.file_changed = true;
        }
        let h = BOARD.load(Relaxed);
        // saved now: the frame may not paint (a minimized game window), and the player may quit before it does
        if self.st.file_changed {
            self.settings_out();
        }
        if !self.st.file.board_window {
            if h != 0 {
                self.board_sc = None; // the swap chain lets go of the window before it goes
                BOARD.store(0, Relaxed);
                // SAFETY: a message to a window of this process, handled by board_wndproc on its thread
                let _ = unsafe {
                    PostMessageW(
                        Some(HWND(h as *mut c_void)),
                        WM_FORZA_DOCK,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
                log("board window: docked back");
            }
            return None;
        }
        if h == 0 {
            // one thread at a time: a window docked a moment ago may still be going
            if !BOARD_UP.swap(true, Relaxed) {
                let place = self.st.file.board_rect;
                let title = format!(
                    "{} - Forza-WCCF {}\0",
                    forza_ui::text(self.st.lang, "board.window_title", &[]),
                    self.st.edition.label()
                );
                std::thread::spawn(move || board_window(place, &title));
            }
            return None;
        }
        Some(HWND(h as *mut c_void))
    }

    /// The board window's swap chain at its client size, and that size. It shows without waiting for the screen (the
    /// game's own Present waits), so the second window never halves the game's frame rate. While the window is being
    /// resized the chain it has is kept (Present stretches it to the window), made again once the size settles.
    unsafe fn board_swap_chain(
        &mut self,
        dev: &IDirect3DDevice9,
        wnd: HWND,
        w: u32,
        h: u32,
    ) -> Option<(IDirect3DSwapChain9, u32, u32)> {
        if let Some((sc, sw, sh, swnd)) = &self.board_sc
            && *swnd == wnd
            && ((*sw, *sh) == (w, h) || BOARD_SIZING.load(Relaxed))
        {
            return Some((sc.clone(), *sw, *sh));
        }
        self.board_sc = None;
        let mut pp = D3DPRESENT_PARAMETERS {
            BackBufferWidth: w,
            BackBufferHeight: h,
            BackBufferFormat: D3DFMT_X8R8G8B8,
            BackBufferCount: 1,
            SwapEffect: D3DSWAPEFFECT_DISCARD,
            hDeviceWindow: wnd,
            Windowed: TRUE,
            PresentationInterval: D3DPRESENT_INTERVAL_IMMEDIATE as u32,
            ..Default::default()
        };
        let mut sc = None;
        // SAFETY: a device call with parameters we own, on the game's render thread
        if let Err(e) = unsafe { dev.CreateAdditionalSwapChain(&mut pp, &mut sc) } {
            if !std::mem::replace(&mut self.board_warned, true) {
                log(&format!(
                    "board window: no swap chain yet ({e}) - trying again"
                ));
            }
            return None;
        }
        let sc = sc?;
        log(&format!("board window: swap chain {w}x{h}"));
        self.board_sc = Some((sc.clone(), w, h, wnd));
        Some((sc, w, h))
    }

    /// The board in the board window, at its client size on its own swap chain; `false` when nothing was drawn there
    /// (minimized, tiny, no swap chain yet): the caller draws the board in the game's window instead.
    unsafe fn board_frame(&mut self, dev: &IDirect3DDevice9, wnd: HWND) -> bool {
        let mut cr = RECT::default();
        // SAFETY: plain window queries, then device calls on the game's render thread
        unsafe {
            if !drawable(wnd) || GetClientRect(wnd, &mut cr).is_err() {
                return false;
            }
            let Some((sc, w, h)) =
                self.board_swap_chain(dev, wnd, cr.right as u32, cr.bottom as u32)
            else {
                return false;
            };
            if self.compose(dev, &sc, w, h, false).is_none() {
                return false;
            }
            if let Err(e) = sc.Present(
                std::ptr::null(),
                std::ptr::null(),
                HWND::default(),
                std::ptr::null(),
                0,
            ) && !std::mem::replace(&mut self.board_warned, true)
            {
                log(&format!("board window: present failed ({e})"));
            }
            true
        }
    }

    fn cost(&self, last: bool) {
        if self.frames > 0 {
            log(&format!(
                "frame cost{}: {:.2} ms average, {:.2} ms worst, over {} frames",
                if last { " (all)" } else { "" },
                self.spent.as_secs_f64() * 1000.0 / f64::from(self.frames),
                self.worst.as_secs_f64() * 1000.0,
                self.frames
            ));
        }
    }

    unsafe fn swap_chain(
        &mut self,
        dev: &IDirect3DDevice9,
        wnd: HWND,
        w: u32,
        h: u32,
    ) -> Option<IDirect3DSwapChain9> {
        if let Some((sc, sw, sh, swnd)) = &self.sc
            && (*sw, *sh, *swnd) == (w, h, wnd)
        {
            return Some(sc.clone());
        }
        if self.sc_failed {
            return None;
        }
        self.sc = None;
        // SAFETY: device calls with parameters we own
        unsafe {
            let mut ip = D3DPRESENT_PARAMETERS::default();
            dev.GetSwapChain(0)
                .ok()?
                .GetPresentParameters(&mut ip)
                .ok()?;
            let mut pp = D3DPRESENT_PARAMETERS {
                BackBufferWidth: w,
                BackBufferHeight: h,
                BackBufferFormat: D3DFMT_X8R8G8B8,
                BackBufferCount: 1,
                SwapEffect: D3DSWAPEFFECT_DISCARD,
                hDeviceWindow: wnd,
                Windowed: TRUE,
                PresentationInterval: ip.PresentationInterval, // the game's own pacing
                ..Default::default()
            };
            let mut sc = None;
            if !ip.Windowed.as_bool() || dev.CreateAdditionalSwapChain(&mut pp, &mut sc).is_err() {
                self.sc_failed = true;
                log("no swap chain of our own (full-screen game?): the panel stays off");
                return None;
            }
            let mut caps = D3DCAPS9::default();
            if dev.GetDeviceCaps(&mut caps).is_ok() {
                self.max_tex = caps.MaxTextureWidth.min(caps.MaxTextureHeight) as usize;
            }
            log(&format!(
                "own swap chain {w}x{h} (game {}x{})",
                ip.BackBufferWidth, ip.BackBufferHeight
            ));
            let sc = sc?;
            self.sc = Some((sc.clone(), w, h, wnd));
            Some(sc)
        }
    }

    /// The game's frame copied (`copy`: the panel shows it), the panel drawn into our back buffer; the render target and
    /// depth buffer put back.
    unsafe fn compose(
        &mut self,
        dev: &IDirect3DDevice9,
        sc: &IDirect3DSwapChain9,
        w: u32,
        h: u32,
        copy: bool,
    ) -> Option<()> {
        // SAFETY: device calls with objects of this device and out-parameters we own
        unsafe {
            let bb = dev.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO).ok()?;
            let scbb = sc.GetBackBuffer(0, D3DBACKBUFFER_TYPE_MONO).ok()?;
            let old_rt = dev.GetRenderTarget(0).ok()?;
            let old_ds = dev.GetDepthStencilSurface().ok();
            let mut d = D3DSURFACE_DESC::default();
            bb.GetDesc(&mut d).ok()?;
            if copy
                && self
                    .game
                    .as_ref()
                    .is_none_or(|g| (g.2, g.3) != (d.Width, d.Height))
            {
                self.game = None;
                let mut t = None;
                dev.CreateTexture(
                    d.Width,
                    d.Height,
                    1,
                    D3DUSAGE_RENDERTARGET as u32,
                    D3DFMT_X8R8G8B8,
                    D3DPOOL_DEFAULT,
                    &mut t,
                    std::ptr::null_mut(),
                )
                .ok()?;
                let t = t?;
                let s = t.GetSurfaceLevel(0).ok()?;
                self.painter.set_user(GAME, t.clone());
                self.game = Some((t, s, d.Width, d.Height));
            }
            if copy {
                let (_, game, _, _) = self.game.as_ref()?;
                dev.StretchRect(
                    &bb,
                    std::ptr::null(),
                    game,
                    std::ptr::null(),
                    D3DTEXF_LINEAR,
                )
                .ok()?;
            }
            dev.SetRenderTarget(0, &scbb).ok()?; // the viewport follows: the whole back buffer
            let _ = dev.SetDepthStencilSurface(None); // the game's depth buffer may be smaller than ours
            let _ = dev.Clear(
                0,
                std::ptr::null(),
                D3DCLEAR_TARGET as u32,
                0xFF0B_0B0F,
                1.0,
                0,
            );
            let painted = dev.BeginScene().is_ok() && {
                // no copy: the board window's frame
                let ok = if copy {
                    self.paint(dev, w, h)
                } else {
                    self.paint_board(dev, w, h)
                };
                let _ = dev.EndScene();
                ok
            };
            if painted && copy && self.dump {
                self.dump(dev, &scbb, w, h);
            }
            let _ = dev.SetRenderTarget(0, &old_rt);
            let _ = dev.SetDepthStencilSurface(old_ds.as_ref());
            painted.then_some(())
        }
    }

    unsafe fn paint(&mut self, dev: &IDirect3DDevice9, w: u32, h: u32) -> bool {
        // 1 point a pixel from 1440x900 up (a bigger window, a bigger sheet), 2 from 4K, the monitor's scaling when
        // Windows scales it (125% = 1.25), scaled down below 1440x900 (forza_ui::panel)
        let ppp = panel::scale(w as f32, h as f32, screen_dpi() as f32);
        SCALE.store(ppp.to_bits(), Relaxed);
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                vec2(w as f32, h as f32) / ppp,
            )),
            time: Some(self.start.elapsed().as_secs_f64()),
            max_texture_side: Some(self.max_tex),
            focused: true,
            events: EVENTS
                .lock()
                .map(|mut e| std::mem::take(&mut *e))
                .unwrap_or_default(),
            ..Default::default()
        };
        // Alt shows the deck's keycaps and has no event of its own: polled here, as the render thread is not the
        // window's (GetKeyState would be stale); first, so the events after it keep their own modifiers
        raw.events.insert(
            0,
            Event::ModifiersChanged(modifiers_with(|vk| {
                // SAFETY: a plain key state query
                (unsafe { GetAsyncKeyState(i32::from(vk)) }) < 0
            })),
        );
        raw.viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(ppp);
        self.table_in();
        self.links_in();
        self.settings_in();
        self.club_in();
        self.deal_in();
        self.keys_in();
        // the tactics lamps from the I/O board's outputs (2017-18 only; 2010-11's bits are not mapped)
        if self.st.edition == theme::Edition::E1718
            && let Some(o) = JVS_OUT
                .get_or_init(|| forza::standins::jvs_out::Map::create(1))
                .as_ref()
                .and_then(|m| m.get())
        {
            self.st.tactics = forza::standins::jvs_out::tactics(&o);
        }
        // the ping meter, online only
        match net::ping_ms() {
            None => self.st.online = false,
            Some(ms) => {
                self.st.online = true;
                self.st.ping_ms = u32::try_from(ms).unwrap_or(u32::MAX);
            }
        }
        self.st.close_warn = CLOSE_ASK.load(Relaxed);
        self.st.crash = LINK_LOST.load(Relaxed);
        if self.st.close_warn {
            self.ctx.request_repaint();
            // tests: WCCFPANEL_CLOSETEST=quit answers QUIT two seconds after the dialog opened
            if std::env::var("WCCFPANEL_CLOSETEST").is_ok_and(|v| v == "quit")
                && ms().wrapping_sub(CLOSE_SINCE.load(Relaxed)) > 2000
            {
                self.st.close_quit = true;
            }
        }
        let dealt = self.st.deal.as_ref().map(|d| d.back);
        let st = &mut self.st;
        let out = self.ctx.run_ui(raw, |ui| panel::show(ui, st));
        match (dealt, self.st.deal.as_ref().map(|d| d.back)) {
            (Some(_), None) => log("deal: closed"),
            (Some(a), Some(b)) if a != b => log(if b {
                "deal: turned over"
            } else {
                "deal: face up"
            }),
            _ => {}
        }
        self.close_out();
        WANTS_KEYS.store(self.ctx.egui_wants_keyboard_input(), Relaxed);
        pause::KEY.store(self.st.keys[forza::keys::PAUSE], Relaxed);
        if std::mem::take(&mut self.st.pause_asked) {
            pause::ASK.store(true, Relaxed);
        }
        self.page_open();
        if let Ok(mut p) = PICTURE.lock() {
            *p = self
                .st
                .picture_rect
                .map(|r| egui::Rect::from_min_max(r.min * ppp, r.max * ppp));
        }
        self.add();
        self.import();
        self.keys_out();
        self.settings_out();
        self.club_out();
        self.table_out();
        self.keys();
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        // SAFETY: the device the painter's textures belong to, on its render thread
        match unsafe {
            self.painter.paint(
                dev,
                [w, h],
                out.pixels_per_point,
                &prims,
                &out.textures_delta,
            )
        } {
            Ok(()) => true,
            Err(e) => {
                log(&format!("paint: {e}"));
                false
            }
        }
    }

    /// The board window's frame (popped out): the same panel state drawn as the board alone by its own egui, then what
    /// the board does handed on (a move, UNDO, DOCK, a cabinet button with the buttons switch on).
    unsafe fn paint_board(&mut self, dev: &IDirect3DDevice9, w: u32, h: u32) -> bool {
        let ppp = panel::board_scale(w as f32, h as f32, screen_dpi() as f32);
        BOARD_SCALE.store(ppp.to_bits(), Relaxed);
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                vec2(w as f32, h as f32) / ppp,
            )),
            time: Some(self.start.elapsed().as_secs_f64()),
            max_texture_side: Some(self.max_tex),
            focused: true,
            events: BOARD_EVENTS
                .lock()
                .map(|mut e| std::mem::take(&mut *e))
                .unwrap_or_default(),
            ..Default::default()
        };
        // Alt shows the deck's keycaps (the buttons switch on), as in [`Render::paint`]
        raw.events.insert(
            0,
            Event::ModifiersChanged(modifiers_with(|vk| {
                // SAFETY: a plain key state query
                (unsafe { GetAsyncKeyState(i32::from(vk)) }) < 0
            })),
        );
        raw.viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(ppp);
        self.table_in();
        self.st.desk = true;
        let st = &mut self.st;
        let out = self.board_ctx.run_ui(raw, |ui| panel::show(ui, st));
        self.st.desk = false;
        self.import();
        self.settings_out();
        self.table_out();
        self.keys();
        let prims = self.board_ctx.tessellate(out.shapes, out.pixels_per_point);
        // SAFETY: the device the painter's textures belong to, on its render thread
        match unsafe {
            self.board_painter.paint(
                dev,
                [w, h],
                out.pixels_per_point,
                &prims,
                &out.textures_delta,
            )
        } {
            Ok(()) => true,
            Err(e) => {
                log(&format!("board window: paint: {e}"));
                false
            }
        }
    }

    /// A card in hand on either window's board.
    fn dragging(&self) -> bool {
        egui::DragAndDrop::has_any_payload(&self.ctx)
            || egui::DragAndDrop::has_any_payload(&self.board_ctx)
    }

    /// The table read again when someone else changed it (FPR_Emu's own panel, a hand edit), never mid-drag.
    fn table_in(&mut self) {
        if self.frames.is_multiple_of(8)
            && !self.dragging()
            && self.table.changed()
            && let Some(placed) = self.table.read()
        {
            log(&format!("board: table read - {} card(s)", placed.len()));
            self.st.board = cards::board(&placed, catalogue());
            self.placed = placed;
        }
    }

    /// The board's links: the club card's link values (data\save\seat1_club.bin, read again every 2 s when its time
    /// changed) with the affinity read at the start. 2017-18: its card's link values are not located, affinity only.
    fn links_in(&mut self) {
        if self
            .links_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.links_at = Some(Instant::now());
        let Some(f) = FILES.get() else { return };
        let card = f.data.join("save").join(forza::card::SLOT);
        let st = stamp(&card);
        if self.links_stamp == Some(st) {
            return;
        }
        self.links_stamp = Some(st);
        let club = if f.edition == theme::Edition::E1718 {
            log("links: 2017-18 club link values not located - no lines, affinity only");
            None
        } else {
            cards::links::ClubLinks::read(&card)
        };
        let club = club.map(|c| c.by_cards()).unwrap_or_default();
        log(&format!(
            "links: club card {} - {} pair(s), {} drawn (150+), {} golden",
            card.display(),
            club.len(),
            club.values().filter(|&&v| v >= 150).count(),
            club.values().filter(|&&v| v == 255).count()
        ));
        self.st.links = forza_ui::board::Links {
            club,
            affinity: AFFINITY.get().cloned().flatten(),
        };
    }

    /// A move, swap or take-off on the board: the table written whole (and the formation copy beside it); a write
    /// that could not be swapped in is tried again next frame.
    fn table_out(&mut self) {
        if std::mem::take(&mut self.st.board_changed) {
            self.placed = cards::table_after(&self.placed, &self.st.board);
            // a card in hand: the table follows it, the board stays as it is (a rebuild reorders the slots the drag
            // holds), no log line per frame
            if self.dragging() {
                return self.table_write();
            }
            // the board's cards follow the table's order again
            self.st.board = cards::board(&self.placed, catalogue());
            let line: Vec<String> = self
                .placed
                .iter()
                .map(|c| format!("{}@{:.3},{:.3}", c.no, c.x, c.y))
                .collect();
            log(&format!(
                "board: {} card(s): {}",
                line.len(),
                line.join(" ")
            ));
        } else if !self.table.pending {
            return self.table.retry_copy(&self.placed);
        }
        self.table_write();
    }

    fn table_write(&mut self) {
        if let Err(e) = self.table.write(&self.placed) {
            log(&format!("board: table write pending ({e})"));
        }
    }

    /// The close dialog's answer: KEEP PLAYING shuts it; QUIT lets the next close through and posts it.
    fn close_out(&mut self) {
        if std::mem::take(&mut self.st.close_keep) {
            CLOSE_ASK.store(false, Relaxed);
            log("close: kept playing");
        }
        if std::mem::take(&mut self.st.close_quit) {
            CLOSE_ASK.store(false, Relaxed);
            CLOSE_OK.store(true, Relaxed);
            let h = HWND(WINDOW.load(Relaxed) as *mut c_void);
            // SAFETY: a message to a window of this process, handled by our window procedure on its thread
            let _ = unsafe { PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
    }

    /// A page opened or closed: while one is open, the key driver leaves the keys alone (the window's WCCF_TYPING, as
    /// the C panel sets it), and the window procedure keeps them from the game.
    fn page_open(&mut self) {
        // the close dialog and the crash screen hold the keys too
        let open = self.st.page != panel::Page::Game || self.st.close_warn || self.st.crash;
        // the window the page is in: the game's (popped out too)
        let want = if open { WINDOW.load(Relaxed) } else { 0 };
        if want != self.typing {
            // SAFETY: windows of this process and a constant name
            unsafe {
                if self.typing != 0 {
                    let _ = RemovePropW(HWND(self.typing as *mut c_void), w!("WCCF_TYPING"));
                }
                if want != 0 {
                    let _ = SetPropW(
                        HWND(want as *mut c_void),
                        w!("WCCF_TYPING"),
                        Some(HANDLE(std::ptr::without_provenance_mut(1))),
                    );
                }
            }
            self.typing = want;
        }
        if PAGE_OPEN.swap(open, Relaxed) == open {
            return;
        }
        log(&format!(
            "page {}{}{}",
            self.st.page.tag(),
            if self.st.close_warn {
                " (close dialog)"
            } else {
                ""
            },
            if self.st.crash { " (crash screen)" } else { "" }
        ));
    }

    /// SETTINGS: opened, data\panel.txt read again (the run may have done a request since) and NOW read; while open, NOW
    /// read again each second. Always: SAVE LOGS' answer, and a request no run took within 3 s (RESTART NOW) or 5 s
    /// (SAVE LOGS, the report dialog) taken back and said.
    fn settings_in(&mut self) {
        use forza_ui::widgets::Tone;
        let Some(f) = FILES.get() else { return };
        let open = self.st.page == panel::Page::Settings;
        if open && self.last_page != panel::Page::Settings {
            self.st.file = f.panel_file();
            self.st.hover_delay = self.st.file.hover_ms.map(|ms| ms as f32 / 1000.0);
            self.st.address = self.st.file.server.clone();
            self.status_at = None;
        }
        // NOW each second while SETTINGS is open, every 2 s otherwise (the card lamp, the crash screen)
        if self
            .status_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(if open { 1 } else { 2 }))
        {
            let logs = self.st.status.logs.take();
            let before = format!(
                "{:?}",
                (
                    &self.st.status.link,
                    self.st.status.card,
                    self.st.status.bad
                )
            );
            self.st.status = f.status();
            self.st.status.logs = logs;
            let after = format!(
                "{:?}",
                (
                    &self.st.status.link,
                    self.st.status.card,
                    self.st.status.bad
                )
            );
            if before != after {
                log(&format!("settings: status {after}"));
            }
            self.status_at = Some(Instant::now());
        }
        let result = f.request("savelogs.result");
        let st = stamp(&result);
        if st.is_some()
            && st != self.logs_stamp
            && let Ok(t) = std::fs::read_to_string(&result)
        {
            // a path can be long: 300 characters
            let line: String = t.lines().next().unwrap_or("").chars().take(300).collect();
            log(&format!("settings: save logs - {line}"));
            self.logs_stamp = st;
            self.logs_at = None;
            self.st.logs_sent = false;
            self.st.status.logs = Some(line);
        }
        for (what, at, secs) in [
            ("restart.request", &mut self.restart_at, 3),
            ("savelogs.request", &mut self.logs_at, 5),
            ("report.request", &mut self.report_at, 5),
        ] {
            if at.is_some_and(|t| t.elapsed() >= Duration::from_secs(secs)) {
                *at = None;
                if std::fs::remove_file(f.request(what)).is_ok() {
                    log(&format!("settings: {what} not picked up - removed"));
                    self.st.say(Tone::Refused, "settings.msg_no_helper");
                    self.st.restart_sent &= what != "restart.request";
                    self.st.logs_sent &= what != "savelogs.request";
                    self.st.report_sent &= what != "report.request";
                }
            }
        }
    }

    /// SETTINGS' changes: data\panel.txt written whole; RESTART NOW and SAVE LOGS asked of the run through their
    /// request files (the run takes them within half a second), RESTART NOW refused during a card session.
    fn settings_out(&mut self) {
        use forza_ui::widgets::Tone;
        let Some(f) = FILES.get() else { return };
        if std::mem::take(&mut self.st.file_changed) {
            let p = &self.st.file;
            match f.save_panel(p) {
                Ok(()) => log(&format!(
                    "settings: saved play_on={} server={} english={} layout={} hover={} auto_logs={} board={} board_buttons={} board_rect={:?}",
                    if p.online { "online" } else { "this_pc" },
                    if p.server.is_empty() { "-" } else { &p.server },
                    p.english.as_deref().unwrap_or("-"),
                    if p.compact { "compact" } else { "cabinet" },
                    p.hover_ms.map_or("off".into(), |ms| ms.to_string()),
                    if p.auto_logs_off { "off" } else { "on" },
                    if p.board_window { "window" } else { "game" },
                    if p.board_buttons { "on" } else { "off" },
                    p.board_rect,
                )),
                Err(e) => {
                    log(&format!("settings: could not save panel.txt: {e}"));
                    self.st.say(Tone::Refused, "settings.msg_not_saved");
                }
            }
        }
        if std::mem::take(&mut self.st.restart) {
            if self.st.crash {
                // the game is dead: its card session cannot be in a match any more, so the card rule does not apply
                if self.ask_restart(f, "CRASH", true) {
                    log("crash: restart requested");
                } else {
                    self.st.say(Tone::Refused, "settings.msg_restart_failed");
                }
            } else if f.status().card == panel::CardState::Open {
                log("settings: restart refused - a card session is open");
                self.st.say(Tone::Refused, "panel.msg_session_open");
            } else {
                if self.ask_restart(f, "SETTINGS", false) {
                    log("settings: restart requested");
                    self.st.say(Tone::Done, "settings.msg_restarting");
                } else {
                    self.st.say(Tone::Refused, "settings.msg_restart_failed");
                }
            }
        }
        if std::mem::take(&mut self.st.save_logs) {
            // the run writes the ZIP to the Downloads folder and answers in savelogs.result
            if kit::swap_write(&f.request("savelogs.request"), "save\n").is_ok() {
                log("settings: save logs asked");
                self.logs_at = Some(Instant::now());
                self.st.logs_sent = true;
                self.st.status.logs = None;
            } else {
                self.st.say(Tone::Refused, "settings.msg_logs_failed");
            }
        }
        if std::mem::take(&mut self.st.open_report) {
            if self.ask_open_report(f) {
                log("crash: report dialog asked (OPEN_REPORT)");
            } else {
                self.st.say(Tone::Refused, "settings.msg_logs_failed");
            }
        }
    }

    /// data\restart.request for the run (`page` names who asked); taken back after 3 s when no run took it.
    /// `force`: the crash screen's request, a second line "force=crashed" that says the game is dead already.
    fn ask_restart(&mut self, f: &kit::Files, page: &str, force: bool) -> bool {
        let text = format!(
            "RESTART NOW - asked by the {page} panel in seat 1 (pid {})\n{}",
            std::process::id(),
            if force { "force=crashed\n" } else { "" }
        );
        let ok = kit::swap_write(&f.request("restart.request"), &text).is_ok();
        if ok {
            self.restart_at = Some(Instant::now());
            self.st.restart_sent = true;
        }
        ok
    }

    /// The crash screen's SAVE LOGS: data\report.request, for the Launcher to open its report dialog (the player says what
    /// happened, the Launcher saves the logs on this PC). The one place that names the request, so it is easy to align. The file:
    /// line 1 `OPEN_REPORT`, then `reason=control_lost`, `server=ADDRESS` (the server played on, for the log) and `pid=` of this game process. No logs leave the panel.
    fn ask_open_report(&mut self, f: &kit::Files) -> bool {
        let text = format!(
            "OPEN_REPORT\nreason=control_lost\nserver={}\npid={}\n",
            self.st.file.server,
            std::process::id()
        );
        let ok = kit::swap_write(&f.request("report.request"), &text).is_ok();
        if ok {
            self.report_at = Some(Instant::now());
            self.st.report_sent = true;
        }
        ok
    }

    /// A card dealt after each match (wccfpanel.c deal_tick): once a second the club card's session; open, then closed
    /// (the locker-room save) = a match finished. `WCCFPANEL_DEALTEST=1` deals one at the first frame (the tests).
    /// Only shown: nothing is written.
    fn deal_in(&mut self) {
        use forza::card::{self, Session};
        let test = self.frames == 0 && std::env::var("WCCFPANEL_DEALTEST").is_ok_and(|v| v == "1");
        if !test && self.deal_at.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.deal_at = Instant::now();
        // 2017-18: the game handed out a player card (the dispenser fix counts them)
        let finished = test || patches::take_owed_new() > 0 || {
            let Some(f) = FILES.get() else { return };
            let (now, _) = card::session(&f.data.join("save").join(card::SLOT), None);
            // no card, or caught mid-write: wait for the next look
            if !matches!(now, Session::Open | Session::Closed) {
                return;
            }
            let was = self.deal_last.replace(now);
            was == Some(Session::Open) && now == Session::Closed
        };
        if finished {
            self.deal();
        }
    }

    fn deal(&mut self) {
        match cards::deal_now(catalogue()) {
            Some(i) => {
                let c = &catalogue()[i];
                log(&format!(
                    "deal: card {} {} ({:?}) - shown only, the club card is not changed",
                    c.number,
                    c.short_name(),
                    c.rarity
                ));
                self.st.deal = Some(panel::Deal::new(i));
            }
            None => log("deal: a match finished, but no catalogue card to show"),
        }
    }

    /// CLUB CARD: data\club_view.txt read on opening and again each second while open when it changed (the run's helper
    /// rewrites it whole whenever the card changes); the slot's session read with it, for the refusal.
    fn club_in(&mut self) {
        let Some(f) = FILES.get() else { return };
        if self.st.page != panel::Page::ClubCard {
            return;
        }
        let opened = self.last_page != panel::Page::ClubCard;
        if !opened
            && self
                .club_at
                .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.club_at = Some(Instant::now());
        let path = f.data.join("club_view.txt");
        let st = stamp(&path);
        if !opened && st == self.club_stamp {
            return;
        }
        self.club_stamp = st;
        self.st.status.card = f.status().card;
        self.st.club = match std::fs::read(&path) {
            Ok(raw) => forza::club_view::ClubView::parse(&raw),
            Err(_) => Default::default(),
        };
        let v = &self.st.club;
        log(&format!(
            "club: view read (state {}, {} lines, {} players, {} backups, session {}, {} other cards)",
            v.state,
            v.lines.len(),
            v.squad.len(),
            v.backups.len(),
            if v.session.is_empty() {
                "-"
            } else {
                &v.session
            },
            v.wallet.len()
        ));
    }

    /// CLUB CARD's choice, asked by its second click: data\panel.txt read again with card=FILE / card=new or
    /// card_fix=bad_endings added, written whole, then RESTART NOW (the run switches the cards while nothing runs).
    fn club_out(&mut self) {
        use forza_ui::panel::ClubPick;
        use forza_ui::widgets::Tone;
        let Some(pick) = self.st.club_request.take() else {
            return;
        };
        let Some(f) = FILES.get() else { return };
        if f.status().card == panel::CardState::Open {
            log("club: switch refused - a card session is open");
            self.st.say(Tone::Refused, "club.session_refused");
            return;
        }
        let mut file = f.panel_file();
        match &pick {
            ClubPick::Fix => file.card_fix = true,
            ClubPick::New => file.card = "new".into(),
            ClubPick::Card { file: name, .. } => file.card = name.clone(),
        }
        if let Err(e) = f.save_panel(&file) {
            log(&format!("club: could not save panel.txt: {e}"));
            self.st.say(Tone::Refused, "settings.msg_not_saved");
            return;
        }
        self.st.file = file;
        if !self.ask_restart(f, "CLUB CARD", false) {
            self.st.say(Tone::Refused, "club.msg_restart_failed");
        } else if pick == ClubPick::Fix {
            log("club: clear bad endings requested");
            self.st.say(Tone::Done, "club.msg_fixing");
        } else {
            log(&format!("club: switch to {} requested", self.st.file.card));
            self.st.say(Tone::Done, "club.msg_switching");
        }
    }

    /// KEYS: opened, keys.txt read again (it may have been edited by hand) and the controllers looked for afresh;
    /// while open, the keys pressed and the first controller control pressed since the last frame; closed, the
    /// controllers let go.
    fn keys_in(&mut self) {
        let keys = self.st.page == panel::Page::Keys;
        let typed = VK_DOWN
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default();
        if keys != (self.last_page == panel::Page::Keys) {
            if keys {
                if let Some(f) = FILES.get() {
                    (self.st.keys, self.st.pads) = f.load_keys();
                }
                let p = pads();
                p.rescan();
                self.pads = Some(p);
                self.pads_scan = Instant::now();
                self.pads_held.clear();
            } else {
                self.pads = None;
            }
        }
        self.last_page = self.st.page;
        if !keys {
            return;
        }
        for (vk, alt) in &typed {
            log(&format!(
                "keys: key 0x{vk:02X}{}",
                if *alt { " with Alt" } else { "" }
            ));
        }
        self.st.vk_down.extend(typed);
        if let Some(p) = &self.pads {
            // ponytail: looked for on the render thread every 2 s while KEYS is open (an empty slot answered in 0.01 ms
            // for the C panel; a thread of our own would outlive an unloaded DLL); a thread if a PC proves slow
            if self.pads_scan.elapsed() >= Duration::from_secs(2) {
                p.rescan();
                self.pads_scan = Instant::now();
            }
            let held = p.poll();
            let new: Vec<u32> = held.difference(&self.pads_held).copied().collect();
            // the first control pressed: a button, else the hat, else a stick or axis (wccfpanel.c pad_tick)
            let first = new
                .iter()
                .find(|&&c| c < forza::keys::PAD_AXIS)
                .or_else(|| new.iter().find(|&&c| c >= forza::keys::PAD_HAT))
                .or(new.first());
            if let Some(&c) = first {
                log(&format!("keys: controller {}", forza::keys::pad_label(c)));
                self.st.pad_down = Some(c);
            }
            self.pads_held = held;
            self.st.pad_names = p.names();
        }
    }

    /// The bindings changed on KEYS: keys.txt written whole (the key driver reads it again at once).
    fn keys_out(&mut self) {
        if !std::mem::take(&mut self.st.keys_changed) {
            return;
        }
        let Some(f) = FILES.get() else { return };
        match f.save_keys(&self.st.keys, &self.st.pads) {
            Ok(()) => log(&format!("keys: saved {}", f.keys.display())),
            Err(e) => {
                log(&format!("keys: could not save {}: {e}", f.keys.display()));
                self.st
                    .say(forza_ui::widgets::Tone::Refused, "keys.msg_not_saved");
            }
        }
    }

    /// A catalogue card asked for (a click, Enter or ADD): put on the table and the table written.
    fn add(&mut self) {
        let Some(no) = self.st.add.take() else { return };
        if panel::add_card(&mut self.st, &mut self.placed, no) {
            log(&format!(
                "catalogue: card {no} added - {} card(s)",
                self.placed.len()
            ));
            if let Err(e) = self.table.write(&self.placed) {
                log(&format!("board: table write pending ({e})"));
            }
        } else {
            log(&format!("catalogue: card {no} not added"));
        }
    }

    /// IMPORT LINEUP: the clipboard's text checked, then on the table once the dialog's CONFIRM comes
    /// ([`Render::import_text`]); its UNDO: the table before it.
    fn import(&mut self) {
        self.formations();
        if std::mem::take(&mut self.st.import_undone)
            && let Some(old) = panel::undo_import(&mut self.st, in_match())
        {
            log(&format!("import: undone - {} card(s)", old.len()));
            self.placed = old;
            self.table_write();
        }
        if std::mem::take(&mut self.st.import) {
            match clipboard_text() {
                Some(text) => {
                    if !panel::ask_import(&mut self.st, &text, in_match()) {
                        let why = self.st.message.as_ref().map_or("", |m| m.text.as_str());
                        log(&format!("import: refused - {}", why.replace('\n', " / ")));
                    }
                }
                None => {
                    log(
                        "import: the clipboard could not be read (held by another program, or no text)",
                    );
                    self.st
                        .say(forza_ui::widgets::Tone::Refused, "cat.msg_clipboard");
                }
            }
        }
        if std::mem::take(&mut self.st.import_go)
            && let Some(ask) = self.st.import_ask.take()
        {
            self.import_text(&ask.text);
        }
    }

    /// FORMATIONS (issue #89): the slots read on opening and after a SAVE; a slot loaded as IMPORT LINEUP loads a
    /// lineup (UNDO the same); SAVE writes the table with the club it came from; COPY puts the site's text on the
    /// clipboard.
    fn formations(&mut self) {
        use forza::cards::formations as fm;
        use forza_ui::widgets::Tone;
        let Some(f) = FILES.get() else { return };
        let dir = fm::dir(&f.data.join("save"));
        let edition = self.st.edition;
        if std::mem::take(&mut self.st.formations_open) {
            self.st.in_match = in_match();
            self.st.formations = Some(panel::Formations {
                slots: fm::read(&dir, edition),
                naming: None,
            });
            log(&format!("formations: opened {}", dir.display()));
        }
        if let Some(i) = self.st.formation_load.take() {
            let now = self.ctx.input(|i| i.time);
            if let Some(new) = panel::load_formation(&mut self.st, &self.placed, i, in_match(), now)
            {
                log(&format!(
                    "formations: slot {} loaded - {} card(s)",
                    i + 1,
                    new.len()
                ));
                let keep = self
                    .table
                    .table
                    .with_file_name("fpr_table0.before-import.txt");
                if let Err(e) = std::fs::write(&keep, cards::table_text(&self.placed)) {
                    log(&format!("formations: {} not written ({e})", keep.display()));
                }
                self.placed = new;
                self.table_write();
            } else {
                let why = self.st.message.as_ref().map_or("", |m| m.text.as_str());
                log(&format!(
                    "formations: slot {} not loaded - {}",
                    i + 1,
                    why.replace('\n', " / ")
                ));
            }
        }
        if let Some((i, name)) = self.st.formation_save.take() {
            self.formation_save(&dir, i, &name, &f.data);
        }
        if let Some(which) = self.st.formation_copy.take() {
            let text = match which {
                None => Some(cards::lineup::export(&self.placed, edition, "")),
                Some(i) => self
                    .st
                    .formations
                    .as_ref()
                    .and_then(|d| d.slots.get(i)?.as_ref())
                    .and_then(|s| fm::site_text(&s.text, edition)),
            };
            if text.as_deref().is_some_and(set_clipboard_text) {
                log(&format!("formations: copied {which:?}"));
                self.st.say(Tone::Done, "form.msg_copied");
            } else {
                log(&format!("formations: copy {which:?} failed"));
                self.st.say(Tone::Refused, "form.msg_not_copied");
            }
        }
    }

    /// SAVE: the table into slot `i` with its club (data\club_view.txt's CLUB row), then the slots read again.
    fn formation_save(&mut self, dir: &Path, i: usize, name: &str, data: &Path) {
        use forza::cards::formations as fm;
        use forza_ui::widgets::Tone;
        let edition = self.st.edition;
        if self.placed.is_empty() {
            self.st.say(Tone::Refused, "form.msg_empty_table");
            return;
        }
        let club = std::fs::read(data.join("club_view.txt"))
            .ok()
            .and_then(|raw| {
                forza::club_view::ClubView::parse(&raw)
                    .row("CLUB")
                    // "NAME  (founded DATE)"
                    .map(|v| v.split("  (").next().unwrap_or(v).trim().to_owned())
            })
            .unwrap_or_default();
        match fm::write(dir, i, &fm::text(&self.placed, edition, name, &club)) {
            Ok(()) => {
                log(&format!(
                    "formations: slot {} saved ({} card(s), club {club:?})",
                    i + 1,
                    self.placed.len()
                ));
                let text = forza_ui::text(
                    self.st.lang,
                    "form.msg_saved",
                    &[("n", &(i + 1).to_string())],
                );
                self.st.note(Tone::Done, text);
            }
            Err(e) => {
                log(&format!("formations: slot {} not saved ({e})", i + 1));
                self.st.say(Tone::Refused, "form.msg_not_saved");
            }
        }
        if let Some(d) = self.st.formations.as_mut() {
            d.slots = fm::read(dir, edition);
        }
    }

    /// Lineup text (the clipboard's; a share link's once fetched) as the whole table, the table before it written
    /// beside it (fpr_table0.before-import.txt) and kept for UNDO.
    fn import_text(&mut self, text: &str) {
        let now = self.ctx.input(|i| i.time);
        let Some(new) = panel::import_lineup(&mut self.st, &self.placed, text, in_match(), now)
        else {
            let why = self.st.message.as_ref().map_or("", |m| m.text.as_str());
            log(&format!("import: refused - {}", why.replace('\n', " / ")));
            return;
        };
        let keep = self
            .table
            .table
            .with_file_name("fpr_table0.before-import.txt");
        if let Err(e) = std::fs::write(&keep, cards::table_text(&self.placed)) {
            log(&format!("import: {} not written ({e})", keep.display()));
        }
        self.placed = new;
        let line: Vec<String> = self
            .placed
            .iter()
            .map(|c| format!("{}@{:.3},{:.3}", c.no, c.x, c.y))
            .collect();
        log(&format!(
            "import: {} card(s): {}",
            line.len(),
            line.join(" ")
        ));
        self.table_write();
    }

    /// The cabinet buttons' keys: down on a press, up once the mouse is released and the key has been down 140 ms
    /// (the C panel's rule: a press shorter than the game's input poll would be missed).
    fn keys(&mut self) {
        self.latch();
        if let Some(action) = self.st.pressed.take() {
            if let Some((vk, _)) = self.key.take() {
                kit::send_key(vk, true, log);
            }
            let vk = forza::keys::NAMES
                .iter()
                .position(|n| *n == action)
                .map_or(0, |i| self.st.keys[i]);
            log(&format!("button {action} -> key 0x{vk:02X}"));
            kit::send_key(vk, false, log);
            self.key = Some((vk, Instant::now()));
            self.st.held = Some(action);
            self.mouse_down = true;
        }
        if self.mouse_down
            && ![&self.ctx, &self.board_ctx]
                .iter()
                .any(|c| c.input(|i| i.pointer.primary_down()))
        {
            self.mouse_down = false;
        }
        if let Some((vk, since)) = self.key
            && !self.mouse_down
            && since.elapsed() >= Duration::from_millis(140)
        {
            kit::send_key(vk, true, log);
            self.key = None;
            self.st.held = None;
        }
    }

    /// The latched button's key: down while `st.latched` names it, up when the latch goes (a click, the crash
    /// screen, or the window's focus loss, which lets the key up itself: [`latch_up`]).
    fn latch(&mut self) {
        let dropped = self.latch != 0 && LATCH_VK.load(Relaxed) == 0;
        if dropped {
            self.latch = 0;
            log("latch: let go (focus lost)");
        }
        self.st.latched = panel::latch_kept(self.st.latched, self.st.crash, !dropped);
        let want = self.st.latched.map_or(0, |a| {
            forza::keys::NAMES
                .iter()
                .position(|n| *n == a)
                .map_or(0, |i| self.st.keys[i])
        });
        if want == self.latch {
            return;
        }
        if self.latch != 0 {
            log(&format!("latch off -> key 0x{:02X} up", self.latch));
            latch_up();
        }
        if want != 0 {
            log(&format!(
                "latch {} -> key 0x{want:02X} down",
                self.st.latched.unwrap_or_default()
            ));
            // stored first; a focus loss between the store and the key-down (latch_up on the window's thread) has
            // already sent its key-up, so the key goes up again here
            LATCH_VK.store(want, Relaxed);
            kit::send_key(want, false, log);
            if LATCH_VK.load(Relaxed) != want {
                kit::send_key(want, true, log);
            }
        }
        self.latch = want;
    }

    /// `wccfpanel_frame.bmp` beside the program, every 10 frames once a window size has held for 10.
    unsafe fn dump(&mut self, dev: &IDirect3DDevice9, scbb: &IDirect3DSurface9, w: u32, h: u32) {
        if self.size != (w, h) {
            (self.size, self.held) = ((w, h), 0);
        }
        self.held += 1;
        if !self.held.is_multiple_of(10) {
            return;
        }
        // SAFETY: a system-memory copy of our back buffer, read within its locked rows
        let rgb = unsafe {
            let mut s = None;
            let mut read = || -> windows::core::Result<Vec<u8>> {
                dev.CreateOffscreenPlainSurface(
                    w,
                    h,
                    D3DFMT_X8R8G8B8,
                    D3DPOOL_SYSTEMMEM,
                    &mut s,
                    std::ptr::null_mut(),
                )?;
                let s = s.as_ref().ok_or_else(windows::core::Error::empty)?;
                dev.GetRenderTargetData(scbb, s)?;
                let mut lr = D3DLOCKED_RECT::default();
                s.LockRect(&mut lr, std::ptr::null(), D3DLOCK_READONLY as u32)?;
                let mut rgb = Vec::with_capacity((w * h * 3) as usize);
                for y in 0..h as usize {
                    let row = std::slice::from_raw_parts(
                        lr.pBits.cast::<u8>().add(y * lr.Pitch as usize),
                        w as usize * 4,
                    );
                    rgb.extend(
                        row.as_chunks::<4>()
                            .0
                            .iter()
                            .flat_map(|p| [p[2], p[1], p[0]]),
                    );
                }
                s.UnlockRect()?;
                Ok(rgb)
            };
            read()
        };
        let dir = EXE_DIR.get().cloned().unwrap_or_default();
        // written aside, then swapped in: a copy taken meanwhile never sees half a picture
        let tmp = dir.join("wccfpanel_frame~.bmp");
        match rgb.map_err(|e| e.to_string()).and_then(|rgb| {
            image::save_buffer(&tmp, &rgb, w, h, image::ExtendedColorType::Rgb8)
                .and_then(|()| Ok(std::fs::rename(&tmp, dir.join("wccfpanel_frame.bmp"))?))
                .map_err(|e| e.to_string())
        }) {
            Ok(()) if self.held == 10 => log(&format!("frame dump {w}x{h}")),
            Ok(()) => {}
            Err(e) => log(&format!("frame dump: {e}")),
        }
    }
}

/// A window the panel can draw in: not minimized, its client at least 64x64.
fn drawable(h: HWND) -> bool {
    let mut cr = RECT::default();
    // SAFETY: plain window queries
    let (iconic, ok) = unsafe { (IsIconic(h).as_bool(), GetClientRect(h, &mut cr).is_ok()) };
    big_enough(iconic, ok.then_some((cr.right, cr.bottom)))
}

/// [`drawable`]'s rule: not minimized, and a client of at least 64x64.
fn big_enough(iconic: bool, client: Option<(i32, i32)>) -> bool {
    !iconic && client.is_some_and(|(w, h)| w >= 64 && h >= 64)
}

fn stamp(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The controllers through winmm (from the system folder: seat 1's own winmm.dll is the hook), or the test's
/// (`WCCFPANEL_FAKEPAD`).
fn pads() -> forza::keys::Pads {
    // ponytail: winmm looked up again at each KEYS opening; cheap (the module stays loaded)
    let say: forza::standins::Log = Arc::new(|m: &str| log(&format!("pads: {m}")));
    match std::env::var_os("WCCFPANEL_FAKEPAD").filter(|v| !v.is_empty()) {
        Some(file) => {
            log("pads: FAKE controllers (a test)");
            let names = kit::FakePads(file.clone().into());
            forza::keys::Pads::new(
                Box::new(kit::FakePads(file.into())),
                say,
                Box::new(move |c| names.name(c)),
            )
        }
        None => match kit::SysWinmm::load() {
            Some(w) => forza::keys::Pads::new(Box::new(w), say, Box::new(forza::keys::pad_name)),
            None => {
                log(
                    "pads: no joystick calls in the system's winmm.dll - controllers cannot be read here",
                );
                forza::keys::Pads::new(Box::new(kit::NoPads), say, Box::new(|_| String::new()))
            }
        },
    }
}

fn catalogue() -> &'static [Card] {
    CATALOGUE.get().map_or(&[], |c| c)
}

/// The fonts: a `fonts/` folder beside the DLL or one above it (the kit's top), else the repository's `assets/fonts/`
/// above the DLL (a build), else forza's own search.
fn font_dir() -> PathBuf {
    let dll = DLL_DIR.get().cloned().unwrap_or_default();
    dll.ancestors()
        .take(6)
        .flat_map(|a| [a.join("fonts"), a.join("assets/fonts")])
        .find(|d| d.is_dir())
        .unwrap_or_else(theme::font_dir)
}

unsafe fn device_window(dev: &IDirect3DDevice9) -> Option<HWND> {
    let mut ip = D3DPRESENT_PARAMETERS::default();
    // SAFETY: device calls with out-parameters we own
    unsafe {
        dev.GetSwapChain(0)
            .ok()?
            .GetPresentParameters(&mut ip)
            .ok()?
    };
    (!ip.hDeviceWindow.is_invalid()).then_some(ip.hDeviceWindow)
}

/// The game's window titled "Client - ..." / "Projector - ..." in every language (the original's title is its class name).
/// SendMessageTimeout: the game's thread may be busy, and this one must not wait for it.
fn retitle(h: HWND) {
    let edition = FILES.get().map_or("", |f| f.edition.label());
    let who = if is_projector() {
        "Projector"
    } else {
        "Client"
    };
    let title: Vec<u16> = format!("{}\0", game_title(who, edition))
        .encode_utf16()
        .collect();
    // the game's window may not take it at once: a few tries (callers are worker threads)
    for _ in 0..5 {
        // SAFETY: `title` is a NUL-terminated UTF-16 string that outlives the call
        let ok = unsafe {
            SendMessageTimeoutW(
                h,
                WM_SETTEXT,
                WPARAM(0),
                LPARAM(title.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                1000,
                None,
            )
        };
        if ok.0 != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// The title for the game's own windows, ASCII only: they are ANSI windows (CreateWindowExA), and Windows converts a
/// WM_SETTEXT to the system code page, which turned the en dash of "2017–18" into a cut-off "2017–1".
fn game_title(who: &str, edition: &str) -> String {
    format!("{who} - Forza-WCCF {}", edition.replace('–', "-"))
}

/// The biggest visible top-level window of this process: the game's.
fn game_window() -> Option<HWND> {
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        // SAFETY: `lp` is the `best` below, alive for the enumeration
        let best = unsafe { &mut *(lp.0 as *mut (isize, i64)) };
        let mut pid = 0;
        let mut r = RECT::default();
        // SAFETY: plain window queries
        unsafe {
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid == GetCurrentProcessId()
                && IsWindowVisible(h).as_bool()
                && GetWindow(h, GW_OWNER).is_err()
                && GetWindowRect(h, &mut r).is_ok()
            {
                let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
                if area > best.1 {
                    *best = (h.0 as isize, area);
                }
            }
        }
        TRUE
    }
    let mut best = (0isize, 0i64);
    // SAFETY: the callback only writes `best`
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut best as *mut _ as isize)) };
    (best.0 != 0).then_some(HWND(best.0 as *mut c_void))
}

/// The latched key up now, once (the render thread or the window procedure, whichever comes first).
fn latch_up() {
    let vk = LATCH_VK.swap(0, Relaxed);
    if vk != 0 {
        kit::send_key(vk, true, log);
    }
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // the focus gone (or the window), also while paused: a latched button lets go, so its key cannot stay down
    if m == WM_KILLFOCUS || m == WM_DESTROY || (m == WM_ACTIVATEAPP && wp.0 == 0) {
        latch_up();
    }
    if let Some(r) = pause::message(false, m, wp, lp) {
        return r;
    }
    match m {
        WM_FORZA_FS => {
            set_maximized(h, wp.0 != 0);
            return LRESULT(0);
        }
        WM_CLOSE if !close_allowed() => return LRESULT(0), // a match is on: warned once
        WM_CLOSE => {
            CLOSING.store(true, Relaxed); // the window goes: link_watch calls the drop ENDED
            latch_up();
        }
        // F11: borderless full screen <-> the window as it was (maximized at the start), once frames are shown; the
        // game never sees it
        WM_KEYDOWN | WM_KEYUP if wp.0 == 0x7A => {
            if m == WM_KEYDOWN && lp.0 & (1 << 30) == 0 && SHOWN.load(Relaxed) {
                toggle_fullscreen(h, &PANEL_FS);
            }
            return LRESULT(0);
        }
        // Esc leaves full screen, and still goes on to egui and the game (it closes the deal window and the server field)
        WM_KEYDOWN if wp.0 == 0x1B && fullscreen::is_on(&PANEL_FS) => {
            if fullscreen::leave(h, &PANEL_FS, test_screen()) {
                log_fullscreen(h, false);
            }
        }
        // a button held on the panel keeps the mouse, so its release arrives even outside the window (drag and drop)
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            // SAFETY: capture for our own window, from its own thread
            unsafe {
                if GetCapture() != h {
                    SetCapture(h);
                    log("drag: capture on");
                }
            }
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP if wp.0 & 0x13 == 0 => {
            // SAFETY: as above
            unsafe {
                if GetCapture() == h {
                    let _ = ReleaseCapture();
                    log("drag: capture off");
                }
            }
        }
        // WM_CAPTURECHANGED: another window took the mouse; egui must not wait for a release it will not get
        // (lParam 0 is our own ReleaseCapture on every release: not lost)
        0x0215 if lp.0 != 0 && lp.0 != h.0 as isize => {
            log("drag: capture lost");
            push(false, vec![Event::PointerGone]);
        }
        _ => {}
    }
    if !DRAWN.load(Relaxed) {
        // nothing of ours on screen: the game gets every message as it came, and egui keeps nothing queued
        if let Ok(mut p) = PICTURE.lock() {
            *p = None;
        }
        if let Ok(mut e) = EVENTS.lock() {
            e.clear();
        }
        // SAFETY: as below
        return unsafe {
            CallWindowProcW(
                std::mem::transmute::<isize, WNDPROC>(OLD_PROC.load(Relaxed)),
                h,
                m,
                wp,
                lp,
            )
        };
    }
    // popped out too: the board is the board window's, the picture, the pages and the buttons stay here
    let lp = match to_game(h, m, lp) {
        Some(game) => game,
        None if input(false, m, wp, lp) => return LRESULT(0), // egui's: the game does not see it
        None => lp,
    };
    // SAFETY: the procedure we replaced, with the message as it came (a mouse message on the picture moved into the
    // game's client area)
    unsafe {
        CallWindowProcW(
            std::mem::transmute::<isize, WNDPROC>(OLD_PROC.load(Relaxed)),
            h,
            m,
            wp,
            lp,
        )
    }
}

/// A mouse message on the game's picture (or the release of a button pressed there), moved from the picture into the
/// game's whole client area, where the game drew its frame before the panel shrank it; `None` = not the game's.
fn to_game(h: HWND, m: u32, lp: LPARAM) -> Option<LPARAM> {
    let bit = match m {
        WM_MOUSEMOVE => 0,
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK => 1,
        WM_RBUTTONDOWN | WM_RBUTTONUP | WM_RBUTTONDBLCLK => 2,
        WM_MBUTTONDOWN | WM_MBUTTONUP | WM_MBUTTONDBLCLK => 4,
        _ => return None,
    };
    let pic = (*PICTURE.lock().ok()?)?;
    let p = pos2(f32::from(lp.0 as i16), f32::from((lp.0 >> 16) as i16));
    let down = matches!(m, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN);
    let up = matches!(m, WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP);
    let held = GAME_DOWN.load(Relaxed);
    let ours = if up {
        held & bit != 0
    } else {
        pic.contains(p) || (m == WM_MOUSEMOVE && held != 0)
    };
    if !ours {
        return None;
    }
    if down {
        GAME_DOWN.fetch_or(bit, Relaxed);
    } else if up {
        GAME_DOWN.fetch_and(!bit, Relaxed);
    }
    let mut cr = RECT::default();
    // SAFETY: a plain window query
    unsafe { GetClientRect(h, &mut cr) }.ok()?;
    let at = |v: f32, a: f32, b: f32, size: i32| {
        (((v - a) / (b - a) * size as f32) as i32).clamp(0, (size - 1).max(0))
    };
    let (gx, gy) = (
        at(p.x, pic.left(), pic.right(), cr.right),
        at(p.y, pic.top(), pic.bottom(), cr.bottom),
    );
    if down {
        log(&format!(
            "click at {},{} -> the game at {gx},{gy}",
            p.x as i32, p.y as i32
        ));
    }
    // egui follows the pointer, but the press is the game's
    push(
        false,
        vec![Event::PointerMoved(
            pos2(p.x, p.y) / f32::from_bits(SCALE.load(Relaxed)),
        )],
    );
    Some(LPARAM(((gy as u16 as isize) << 16) | gx as u16 as isize))
}

fn modifiers() -> Modifiers {
    // SAFETY: plain key state queries
    modifiers_with(|vk| unsafe { GetKeyState(i32::from(vk)) } < 0)
}

fn modifiers_with(down: impl Fn(u16) -> bool) -> Modifiers {
    let ctrl = down(VK_CONTROL.0);
    Modifiers {
        alt: down(VK_MENU.0),
        ctrl,
        shift: down(VK_SHIFT.0),
        mac_cmd: false,
        command: ctrl,
    }
}

/// One window message into egui's events (`board`: the board window's egui); `true` when the game must not see it
/// (egui wants that input).
fn input(board: bool, m: u32, wp: WPARAM, lp: LPARAM) -> bool {
    let ppp = f32::from_bits(if board { &BOARD_SCALE } else { &SCALE }.load(Relaxed));
    let pos = pos2(
        f32::from(lp.0 as i16) / ppp,
        f32::from((lp.0 >> 16) as i16) / ppp,
    );
    let modifiers = modifiers();
    let button = |button, pressed| {
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            },
        ]
    };
    let key = |pressed| {
        vk_key(wp.0 as u32).map(|key| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: pressed && lp.0 & (1 << 30) != 0,
            modifiers,
        })
    };
    let (events, keys): (Vec<Event>, bool) = match m {
        WM_MOUSEMOVE => {
            push(board, vec![Event::PointerMoved(pos)]);
            return true; // off the game's picture (to_game): not the game's
        }
        WM_LBUTTONDOWN => (button(PointerButton::Primary, true), false),
        WM_LBUTTONUP => (button(PointerButton::Primary, false), false),
        WM_RBUTTONDOWN => (button(PointerButton::Secondary, true), false),
        WM_RBUTTONUP => (button(PointerButton::Secondary, false), false),
        WM_MBUTTONDOWN => (button(PointerButton::Middle, true), false),
        WM_MBUTTONUP => (button(PointerButton::Middle, false), false),
        WM_MOUSEWHEEL => (
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, f32::from((wp.0 >> 16) as i16) / 120.0),
                phase: egui::TouchPhase::Move,
                modifiers,
            }],
            false,
        ),
        0x02A3 => {
            // WM_MOUSELEAVE
            push(board, vec![Event::PointerGone]);
            return false;
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if PAGE_OPEN.load(Relaxed)
                && lp.0 & (1 << 30) == 0
                && let Ok(mut v) = VK_DOWN.lock()
            {
                v.push((wp.0 as u32, lp.0 & (1 << 29) != 0));
            }
            (key(true).into_iter().collect(), true)
        }
        WM_KEYUP | WM_SYSKEYUP => (key(false).into_iter().collect(), true),
        WM_CHAR => (
            char::from_u32(wp.0 as u32)
                .filter(|c| !c.is_control())
                .map(|c| Event::Text(c.into()))
                .into_iter()
                .collect(),
            true,
        ),
        _ => return false,
    };
    push(board, events);
    // the mouse off the game's picture is the panel's (to_game took the picture's); keys while egui types or a page
    // is open
    !keys || WANTS_KEYS.load(Relaxed) || PAGE_OPEN.load(Relaxed)
}

fn push(board: bool, mut events: Vec<Event>) {
    if let Ok(mut e) = if board { &BOARD_EVENTS } else { &EVENTS }.lock() {
        e.append(&mut events);
    }
}

fn vk_key(vk: u32) -> Option<Key> {
    let name = match vk {
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0D => "Enter",
        0x1B => "Escape",
        0x20 => "Space",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x23 => "End",
        0x24 => "Home",
        0x25 => "ArrowLeft",
        0x26 => "ArrowUp",
        0x27 => "ArrowRight",
        0x28 => "ArrowDown",
        0x2D => "Insert",
        0x2E => "Delete",
        0x30..=0x39 | 0x41..=0x5A => return Key::from_name(&char::from(vk as u8).to_string()),
        0x70..=0x7B => return Key::from_name(&format!("F{}", vk - 0x6F)),
        _ => return None,
    };
    Key::from_name(name)
}

#[cfg(test)]
mod tests {
    use super::{big_enough, game_title, match_now};

    /// IMPORT LINEUP: 2017-18 asks the seat's own state (another seat's match puts the shared shop in GAME); the shop
    /// state only when that cannot be read; 2010-11 keeps the shop state; a card session always counts.
    #[test]
    fn import_is_refused_only_in_this_seats_match() {
        assert!(
            !match_now(true, Some(3), Some(5), false),
            "another seat plays"
        );
        assert!(match_now(true, Some(19), Some(5), false));
        assert!(match_now(true, Some(19), Some(1), false));
        assert!(match_now(true, None, Some(5), false));
        assert!(!match_now(true, None, Some(1), false));
        assert!(match_now(false, None, Some(4), false));
        assert!(!match_now(false, None, Some(7), false));
        assert!(match_now(false, None, None, true));
        assert!(match_now(true, Some(3), Some(1), true));
    }

    /// The game's windows are ANSI: the whole edition survives as ASCII.
    #[test]
    fn the_game_title_is_ascii_and_whole() {
        let t = game_title("Client", forza::Edition::E1718.label());
        assert_eq!(t, "Client - Forza-WCCF 2017-18");
        assert!(t.is_ascii());
        assert_eq!(
            game_title("Projector", "2010–11"),
            "Projector - Forza-WCCF 2010-11"
        );
    }

    /// The board window (and the game's) is drawn in only when shown and at least 64x64; otherwise the panel falls
    /// back to the game's window for the frame.
    #[test]
    fn a_minimized_or_tiny_window_is_not_drawn() {
        assert!(big_enough(false, Some((800, 600))));
        assert!(big_enough(false, Some((64, 64))));
        assert!(!big_enough(true, Some((800, 600))));
        assert!(!big_enough(false, Some((63, 600))));
        assert!(!big_enough(false, None));
    }
}
