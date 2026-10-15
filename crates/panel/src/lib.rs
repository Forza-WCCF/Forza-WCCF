//! The in-game panel as a DLL in the 32-bit game (step 9.4): forza::ui's panel drawn over the game's own Direct3D 9 picture.
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
//! Test switches: `FORZA_PANEL_DUMP=1` writes `wccfpanel_frame.bmp` beside the program every 10 frames once a window
//! size has held for 10 (fakegame.exe's `size:` and `copy:` steps); `WCCFPANEL_DRYKEYS=1` logs the keys instead of
//! pressing them.
#![cfg(target_arch = "x86")]

mod kit;
mod money;
mod net;
mod painter;
mod patches;

use egui::{Event, Key, Modifiers, PointerButton, Pos2, TextureId, ViewportId, pos2, vec2};
use forza::cards::{self, Placed, TableFile};
use forza::ui::board::Card;
use forza::ui::panel::{self, PanelState};
use forza::ui::theme;
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
use windows::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleFileNameW};
use windows::Win32::System::Memory::*;
use windows::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
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
/// A panic in a frame: the panel stops drawing, the game goes on.
static DEAD: AtomicBool = AtomicBool::new(false);
static RENDER: Mutex<Option<Render>> = Mutex::new(None);
// the window side (its own thread) and the render side meet here
static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
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
/// This process is the projector (the second game window): money and the frame wait only, no panel.
static PROJECTOR: AtomicBool = AtomicBool::new(false);
/// A frame has been shown on our own swap chain: the window may be maximized (F11).
static SHOWN: AtomicBool = AtomicBool::new(false);
/// The player closed the window: the server link's drop that follows is the end (ENDED), not a loss.
static CLOSING: AtomicBool = AtomicBool::new(false);
/// A close refused during a card session, until this time (ms since START): a second close then quits.
static CLOSE_UNTIL: AtomicU32 = AtomicU32::new(0);
static START: OnceLock<Instant> = OnceLock::new();
/// The window's maximized state for WCCFPANEL_FS_RECT (tests: a pretend screen), and the size it had before.
static TEST_MAX: Mutex<Option<RECT>> = Mutex::new(None);
/// Our own window message: maximized (wParam 1) or the game's size (0), done on the window's thread.
const WM_FORZA_FS: u32 = WM_APP + 0x57;

/// Milliseconds since the DLL loaded.
fn ms() -> u32 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u32
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
    if is_projector() {
        PROJECTOR.store(true, Relaxed);
        log(&format!(
            "forza-panel init (pid {}) - the PROJECTOR: money with commas and the frame wait only, no panels",
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
    // SAFETY: the throwaway device is ours; the detours replace functions with the same signature
    match unsafe { device_functions() } {
        // SAFETY: as above
        Some((present, reset)) => unsafe {
            if let Some(t) = detour(present, hook_present as *const () as usize) {
                PRESENT.store(t, Relaxed);
                log("Present detoured");
            }
            if let Some(t) = detour(reset, hook_reset as *const () as usize) {
                RESET.store(t, Relaxed);
            }
        },
        None => log("no throwaway Direct3D 9 device: not hooked"),
    }
    // injected early? wait up to 60 s for the game's window
    for _ in 0..120 {
        if let Some(h) = game_window() {
            // SAFETY: a window of this process; cross-thread subclassing within a process is allowed
            let old =
                unsafe { SetWindowLongPtrW(h, GWLP_WNDPROC, wndproc as *const () as usize as _) };
            OLD_PROC.store(old as isize, Relaxed);
            WINDOW.store(h.0 as isize, Relaxed);
            log(&format!("input from window {:?}", h.0));
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    INIT_DONE.store(true, Relaxed);
}

/// The players' affinity from the game's own files: `prog_data\player_data\` in the program's folder, the default
/// rule's tables from the program itself (2010-11; 2017-18 listed pairs only). Read-only; logged with counts.
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
    use forza::ui::panel::Link;
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
/// open since this run started) the first close only warns, for 8 s; a second close within them quits anyway.
fn close_allowed() -> bool {
    let until = CLOSE_UNTIL.load(Relaxed);
    if until != 0 && ms() < until {
        log("close: closed again during the card session - the game ends");
        return true;
    }
    let open = FILES.get().is_some_and(|f| {
        use launcher::card;
        let since = stamp(&f.data.join("running.json"));
        let save = f.data.join("save");
        card::club_session(&save.join(card::SLOT), &save.join(card::NEW), since).0
            == card::Session::Open
    });
    if !open {
        log("close: the window closes - the game ends");
        return true;
    }
    CLOSE_UNTIL.store(ms() + 8000, Relaxed);
    log("close: a card session is open - warned, not closed");
    false
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

fn is_maximized(h: HWND) -> bool {
    if test_screen().is_some() {
        return TEST_MAX.lock().is_ok_and(|m| m.is_some());
    }
    // SAFETY: a plain window query
    unsafe { IsZoomed(h) }.as_bool()
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

/// FreeLibrary: everything back as it was, on the thread that unloads us. The real game never unloads the panel;
/// fakegame does it between frames, so no Present or window message is running in our code.
fn unload() {
    if !INIT_DONE.load(Relaxed) {
        log("unloaded before init finished: the init thread would run in freed code"); // ponytail: no wait under the loader lock
    }
    let h = WINDOW.swap(0, Relaxed);
    if h != 0 {
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
/// prologue is left alone (no panel, the game unharmed).
unsafe fn detour(target: usize, to: usize) -> Option<usize> {
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
        // SAFETY: as above
        return unsafe { orig(dev, src, dst, wnd, dirty) };
    }
    IN_PRESENT.set(true);
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
        log("a frame panicked: the panel stops drawing");
        None
    });
    IN_PRESENT.set(false);
    // SAFETY: as above: not shown on our swap chain, the game's own Present shows its frame
    shown.unwrap_or_else(|| unsafe { orig(dev, src, dst, wnd, dirty) })
}

unsafe extern "system" fn hook_reset(dev: *mut c_void, pp: *mut D3DPRESENT_PARAMETERS) -> HRESULT {
    if let Ok(mut r) = RENDER.lock()
        && let Some(r) = r.as_mut()
    {
        r.release_default();
    }
    log("device reset");
    // SAFETY: the original Reset through its trampoline
    unsafe { std::mem::transmute::<usize, ResetFn>(RESET.load(Relaxed))(dev, pp) }
}

/// Everything the render thread keeps: egui, the painter, our swap chain and the game frame's copy.
struct Render {
    ctx: egui::Context,
    st: PanelState,
    table: TableFile,
    /// The table as last read or written; `st.board.cards[i]` is `placed[i]`.
    placed: Vec<Placed>,
    /// A cabinet button's key that is down, since when; the mouse still held on it.
    key: Option<(u32, Instant)>,
    mouse_down: bool,
    /// The page shown last frame (KEYS opening reads keys.txt again and looks for controllers).
    last_page: panel::Page,
    /// The controllers, while KEYS is open, and the controls held at the last look.
    pads: Option<launcher::keys::Pads>,
    pads_held: std::collections::BTreeSet<u32>,
    /// When the controllers were last looked for.
    pads_scan: Instant,
    /// SETTINGS: when NOW was read; when RESTART NOW and SEND LOGS were asked; SEND LOGS' answer file as last read.
    status_at: Option<Instant>,
    restart_at: Option<Instant>,
    logs_at: Option<Instant>,
    logs_stamp: Option<std::time::SystemTime>,
    /// The deal: when the club card was last looked at, and its session then.
    deal_at: Instant,
    deal_last: Option<launcher::card::Session>,
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
    game: Option<(IDirect3DTexture9, IDirect3DSurface9, u32, u32)>,
    max_tex: usize,
    dump: bool,
    size: (u32, u32),
    held: u32,
    frames: u32,
    spent: Duration,
    worst: Duration,
}

// SAFETY: the Direct3D objects are used only on the game's render thread (behind RENDER's lock); Reset and unload,
// which also release them, are called on that thread too
unsafe impl Send for Render {}

impl Render {
    fn new() -> Render {
        let ctx = egui::Context::default();
        let files = FILES.get_or_init(|| kit::Files::find(Path::new("")));
        let dir = font_dir();
        if let Err(e) = theme::install(&ctx, files.edition, &dir) {
            log(&format!(
                "fonts from {}: {e} (egui's own fonts used)",
                dir.display()
            ));
        }
        forza::ui::board::set_picture_dir(&ctx, Some(files.cards.clone()));
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
        st.hover_delay = st.file.hover_ms as f32 / 1000.0;
        st.address = st.file.server.clone();
        (st.keys, st.pads) = files.load_keys();
        st.picture = Some(GAME);
        log(&format!(
            "language {}, hover delay {} s",
            lang.code(),
            st.hover_delay
        ));
        Render {
            ctx,
            st,
            table,
            placed,
            key: None,
            mouse_down: false,
            last_page: panel::Page::Game,
            pads: None,
            pads_held: Default::default(),
            pads_scan: Instant::now(),
            status_at: None,
            restart_at: None,
            logs_at: None,
            // an answer from an earlier run is not shown
            logs_stamp: stamp(&files.request("sendlogs.result")),
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
            game: None,
            max_tex: 2048,
            dump: std::env::var_os("FORZA_PANEL_DUMP").is_some_and(|v| v == "1"),
            size: (0, 0),
            held: 0,
            frames: 0,
            spent: Duration::ZERO,
            worst: Duration::ZERO,
        }
    }

    /// DEFAULT-pool objects go before a reset; the next frame makes them again.
    fn release_default(&mut self) {
        self.sc = None;
        self.sc_failed = false;
        self.game = None;
        self.painter.reset();
    }

    /// One frame on our swap chain; `None` = not shown, the game's own Present runs.
    unsafe fn frame(&mut self, dev: &IDirect3DDevice9, wnd: HWND) -> Option<HRESULT> {
        let t0 = Instant::now();
        // SAFETY: device calls with out-parameters we own
        unsafe {
            let target = if wnd.is_invalid() {
                device_window(dev)?
            } else {
                wnd
            };
            let mut cr = RECT::default();
            if IsIconic(target).as_bool()
                || GetClientRect(target, &mut cr).is_err()
                || cr.right < 64
                || cr.bottom < 64
            {
                return None;
            }
            let (w, h) = (cr.right as u32, cr.bottom as u32);
            let sc = self.swap_chain(dev, target, w, h)?;
            let sb = dev.CreateStateBlock(D3DSBT_ALL).ok()?;
            let mut vp = D3DVIEWPORT9::default();
            let _ = dev.GetViewport(&mut vp);
            let drawn = self.compose(dev, &sc, w, h);
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

    /// The game's frame copied, the panel drawn into our back buffer; the render target and depth buffer put back.
    unsafe fn compose(
        &mut self,
        dev: &IDirect3DDevice9,
        sc: &IDirect3DSwapChain9,
        w: u32,
        h: u32,
    ) -> Option<()> {
        // SAFETY: device calls with objects of this device and out-parameters we own
        unsafe {
            let bb = dev.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO).ok()?;
            let scbb = sc.GetBackBuffer(0, D3DBACKBUFFER_TYPE_MONO).ok()?;
            let old_rt = dev.GetRenderTarget(0).ok()?;
            let old_ds = dev.GetDepthStencilSurface().ok();
            let mut d = D3DSURFACE_DESC::default();
            bb.GetDesc(&mut d).ok()?;
            if self
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
            let (_, game, _, _) = self.game.as_ref()?;
            dev.StretchRect(
                &bb,
                std::ptr::null(),
                game,
                std::ptr::null(),
                D3DTEXF_LINEAR,
            )
            .ok()?;
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
                let ok = self.paint(dev, w, h);
                let _ = dev.EndScene();
                ok
            };
            if painted && self.dump {
                self.dump(dev, &scbb, w, h);
            }
            let _ = dev.SetRenderTarget(0, &old_rt);
            let _ = dev.SetDepthStencilSurface(old_ds.as_ref());
            painted.then_some(())
        }
    }

    unsafe fn paint(&mut self, dev: &IDirect3DDevice9, w: u32, h: u32) -> bool {
        // the 1440x900 design sheet scaled to fit; a wider window widens the middle (forza::ui::panel)
        let ppp = (h as f32 / panel::SIZE.y).min(w as f32 / panel::SIZE.x);
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
        // the ping meter, online only
        match net::ping_ms() {
            None => self.st.online = false,
            Some(ms) => {
                self.st.online = true;
                self.st.ping_ms = u32::try_from(ms).unwrap_or(u32::MAX);
            }
        }
        let until = CLOSE_UNTIL.load(Relaxed);
        self.st.close_warn = until != 0 && ms() < until;
        if self.st.close_warn {
            self.ctx.request_repaint();
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
        WANTS_KEYS.store(self.ctx.egui_wants_keyboard_input(), Relaxed);
        self.page_open();
        if let Ok(mut p) = PICTURE.lock() {
            *p = self
                .st
                .picture_rect
                .map(|r| egui::Rect::from_min_max(r.min * ppp, r.max * ppp));
        }
        self.add();
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

    /// The table read again when someone else changed it (FPR_Emu's own panel, a hand edit), never mid-drag.
    fn table_in(&mut self) {
        if self.frames.is_multiple_of(8)
            && !egui::DragAndDrop::has_any_payload(&self.ctx)
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
        let card = f.data.join("save").join(launcher::card::SLOT);
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
        self.st.links = forza::ui::board::Links {
            club,
            affinity: AFFINITY.get().cloned().flatten(),
        };
    }

    /// A move, swap or take-off on the board: the table written whole (and the formation copy beside it); a write
    /// that could not be swapped in is tried again next frame.
    fn table_out(&mut self) {
        if std::mem::take(&mut self.st.board_changed) {
            self.placed = cards::table_after(&self.placed, &self.st.board);
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
            return;
        }
        if let Err(e) = self.table.write(&self.placed) {
            log(&format!("board: table write pending ({e})"));
        }
    }

    /// A page opened or closed: while one is open, the key driver leaves the keys alone (the window's WCCF_TYPING, as
    /// the C panel sets it), and the window procedure keeps them from the game.
    fn page_open(&mut self) {
        let open = self.st.page != panel::Page::Game;
        if PAGE_OPEN.swap(open, Relaxed) == open {
            return;
        }
        let h = HWND(WINDOW.load(Relaxed) as *mut c_void);
        if !h.is_invalid() {
            // SAFETY: a window of this process and a constant name
            unsafe {
                if open {
                    let _ = SetPropW(
                        h,
                        w!("WCCF_TYPING"),
                        Some(HANDLE(std::ptr::without_provenance_mut(1))),
                    );
                } else {
                    let _ = RemovePropW(h, w!("WCCF_TYPING"));
                }
            }
        }
        log(&format!("page {}", self.st.page.tag()));
    }

    /// SETTINGS: opened, data\panel.txt read again (the run may have done a request since) and NOW read; while open, NOW
    /// read again each second. Always: SEND LOGS' answer, and a request no run took within 3 s (RESTART NOW) or 5 s
    /// (SEND LOGS) taken back and said.
    fn settings_in(&mut self) {
        use forza::ui::widgets::Tone;
        let Some(f) = FILES.get() else { return };
        let open = self.st.page == panel::Page::Settings;
        if open && self.last_page != panel::Page::Settings {
            self.st.file = f.panel_file();
            self.st.address = self.st.file.server.clone();
            self.status_at = None;
        }
        if open
            && self
                .status_at
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
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
        let result = f.request("sendlogs.result");
        let st = stamp(&result);
        if st.is_some()
            && st != self.logs_stamp
            && let Ok(t) = std::fs::read_to_string(&result)
        {
            let line: String = t.lines().next().unwrap_or("").chars().take(95).collect();
            log(&format!("settings: send logs - {line}"));
            self.logs_stamp = st;
            self.logs_at = None;
            self.st.logs_sent = false;
            self.st.status.logs = Some(line);
        }
        for (what, at, secs) in [
            ("restart.request", &mut self.restart_at, 3),
            ("sendlogs.request", &mut self.logs_at, 5),
        ] {
            if at.is_some_and(|t| t.elapsed() >= Duration::from_secs(secs)) {
                *at = None;
                if std::fs::remove_file(f.request(what)).is_ok() {
                    log(&format!("settings: {what} not picked up - removed"));
                    self.st.say(Tone::Refused, "settings.msg_no_helper");
                    self.st.restart_sent &= what != "restart.request";
                    self.st.logs_sent &= what != "sendlogs.request";
                }
            }
        }
    }

    /// SETTINGS' changes: data\panel.txt written whole; RESTART NOW and SEND LOGS asked of the run through their
    /// request files (the run takes them within half a second), RESTART NOW refused during a card session.
    fn settings_out(&mut self) {
        use forza::ui::widgets::Tone;
        let Some(f) = FILES.get() else { return };
        if std::mem::take(&mut self.st.file_changed) {
            let p = &self.st.file;
            match f.save_panel(p) {
                Ok(()) => log(&format!(
                    "settings: saved play_on={} server={} english={} layout={} hover={} auto_logs={}",
                    if p.online { "online" } else { "this_pc" },
                    if p.server.is_empty() { "-" } else { &p.server },
                    p.english.as_deref().unwrap_or("-"),
                    if p.compact { "compact" } else { "cabinet" },
                    p.hover_ms,
                    if p.auto_logs_off { "off" } else { "on" },
                )),
                Err(e) => {
                    log(&format!("settings: could not save panel.txt: {e}"));
                    self.st.say(Tone::Refused, "settings.msg_not_saved");
                }
            }
        }
        if std::mem::take(&mut self.st.restart) {
            if f.status().card == panel::CardState::Open {
                log("settings: restart refused - a card session is open");
                self.st.say(Tone::Refused, "panel.msg_session_open");
            } else {
                if self.ask_restart(f, "SETTINGS") {
                    log("settings: restart requested");
                    self.st.say(Tone::Done, "settings.msg_restarting");
                } else {
                    self.st.say(Tone::Refused, "settings.msg_restart_failed");
                }
            }
        }
        if std::mem::take(&mut self.st.send_logs) {
            let server = self.st.file.server.clone();
            if kit::swap_write(&f.request("sendlogs.request"), &format!("{server}\n")).is_ok() {
                log(&format!("settings: send logs asked (to {server})"));
                self.logs_at = Some(Instant::now());
                self.st.logs_sent = true;
                self.st.status.logs = None;
                self.st.say(Tone::Notice, "settings.msg_sending");
            } else {
                self.st.say(Tone::Refused, "settings.msg_logs_failed");
            }
        }
    }

    /// data\restart.request for the run (`page` names who asked); taken back after 3 s when no run took it.
    fn ask_restart(&mut self, f: &kit::Files, page: &str) -> bool {
        let text = format!(
            "RESTART NOW - asked by the {page} panel in seat 1 (pid {})\n",
            std::process::id()
        );
        let ok = kit::swap_write(&f.request("restart.request"), &text).is_ok();
        if ok {
            self.restart_at = Some(Instant::now());
            self.st.restart_sent = true;
        }
        ok
    }

    /// A card dealt after each match (wccfpanel.c deal_tick): once a second the club card's session; open, then closed
    /// (the locker-room save) = a match finished. `WCCFPANEL_DEALTEST=1` deals one at the first frame (the tests).
    /// Only shown: nothing is written.
    fn deal_in(&mut self) {
        use launcher::card::{self, Session};
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
        use forza::ui::panel::ClubPick;
        use forza::ui::widgets::Tone;
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
        if !self.ask_restart(f, "CLUB CARD") {
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
                .find(|&&c| c < launcher::keys::PAD_AXIS)
                .or_else(|| new.iter().find(|&&c| c >= launcher::keys::PAD_HAT))
                .or(new.first());
            if let Some(&c) = first {
                log(&format!(
                    "keys: controller {}",
                    launcher::keys::pad_label(c)
                ));
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
                    .say(forza::ui::widgets::Tone::Refused, "keys.msg_not_saved");
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

    /// The cabinet buttons' keys: down on a press, up once the mouse is released and the key has been down 140 ms
    /// (the C panel's rule: a press shorter than the game's input poll would be missed).
    fn keys(&mut self) {
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
        if self.mouse_down && !self.ctx.input(|i| i.pointer.primary_down()) {
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

fn stamp(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The controllers through winmm (from the system folder: seat 1's own winmm.dll is the hook), or the test's
/// (`WCCFPANEL_FAKEPAD`).
fn pads() -> launcher::keys::Pads {
    // ponytail: winmm looked up again at each KEYS opening; cheap (the module stays loaded)
    let say: forza::standins::Log = Arc::new(|m: &str| log(&format!("pads: {m}")));
    match std::env::var_os("WCCFPANEL_FAKEPAD").filter(|v| !v.is_empty()) {
        Some(file) => {
            log("pads: FAKE controllers (a test)");
            let names = kit::FakePads(file.clone().into());
            launcher::keys::Pads::new(
                Box::new(kit::FakePads(file.into())),
                say,
                Box::new(move |c| names.name(c)),
            )
        }
        None => match kit::SysWinmm::load() {
            Some(w) => {
                launcher::keys::Pads::new(Box::new(w), say, Box::new(launcher::keys::pad_name))
            }
            None => {
                log(
                    "pads: no joystick calls in the system's winmm.dll - controllers cannot be read here",
                );
                launcher::keys::Pads::new(Box::new(kit::NoPads), say, Box::new(|_| String::new()))
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

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_FORZA_FS => {
            set_maximized(h, wp.0 != 0);
            return LRESULT(0);
        }
        WM_CLOSE if !close_allowed() => return LRESULT(0), // a match is on: warned once
        WM_CLOSE => CLOSING.store(true, Relaxed), // the window goes: link_watch calls the drop ENDED
        // F11: maximized <-> the game's size, once frames are shown; the game never sees it
        WM_KEYDOWN | WM_KEYUP if wp.0 == 0x7A => {
            if m == WM_KEYDOWN && lp.0 & (1 << 30) == 0 && SHOWN.load(Relaxed) {
                set_maximized(h, !is_maximized(h));
            }
            return LRESULT(0);
        }
        _ => {}
    }
    let lp = match to_game(h, m, lp) {
        Some(game) => game,
        None if input(m, wp, lp) => return LRESULT(0), // egui's: the game does not see it
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
    push(vec![Event::PointerMoved(
        pos2(p.x, p.y) / f32::from_bits(SCALE.load(Relaxed)),
    )]);
    Some(LPARAM(((gy as u16 as isize) << 16) | gx as u16 as isize))
}

fn modifiers() -> Modifiers {
    // SAFETY: plain key state queries
    let down = |vk: u16| unsafe { GetKeyState(i32::from(vk)) } < 0;
    let ctrl = down(VK_CONTROL.0);
    Modifiers {
        alt: down(VK_MENU.0),
        ctrl,
        shift: down(VK_SHIFT.0),
        mac_cmd: false,
        command: ctrl,
    }
}

/// One window message into egui's events; `true` when the game must not see it (egui wants that input).
fn input(m: u32, wp: WPARAM, lp: LPARAM) -> bool {
    let ppp = f32::from_bits(SCALE.load(Relaxed));
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
            push(vec![Event::PointerMoved(pos)]);
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
            push(vec![Event::PointerGone]);
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
    push(events);
    // the mouse off the game's picture is the panel's (to_game took the picture's); keys while egui types or a page
    // is open
    !keys || WANTS_KEYS.load(Relaxed) || PAGE_OPEN.load(Relaxed)
}

fn push(mut events: Vec<Event>) {
    if let Ok(mut e) = EVENTS.lock() {
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
