//! fakegame.exe: a stand-in for seat 1's client_Release.exe, to see the panel at work without the game (2026-10-08).
//! A 1440x900 Direct3D 9 window (the game's own size) that presents a plain picture about 60 times a second, loads
//! the panel, then plays the steps on its command line and quits:
//!
//!     fakegame.exe PANEL.dll STEP ...
//!       wait:MS          let MS milliseconds of frames go by
//!       click:X,Y        a left click at X,Y in the window (client pixels)
//!       move:X,Y         the mouse to X,Y, no click (the left button held after a down:)
//!       down:X,Y         the left button pressed at X,Y (a drag: down, moves, up)
//!       up:X,Y           the left button released at X,Y
//!       rclick:X,Y       a right click at X,Y
//!       close            the window's X (WM_CLOSE): like the real game, the program runs on after its window goes
//!       type:TEXT        the keys of TEXT (letters, digits, space)
//!       key:VK           one key by its virtual-key code (27 Esc, 8 Backspace, 13 Enter)
//!       size:W,H         the window's client size: the panel writes its frame dump (wccfpanel_frame.bmp, beside
//!                        this exe) once a new size has held for 10 frames
//!       copy:NAME        wccfpanel_frame.bmp copied to NAME
//!       unload           FreeLibrary on the panel: the frames go on without it
//!       sysmove:MS       MS from now (inside a pause's hold too) the window's Move command (WM_SYSCOMMAND SC_MOVE),
//!                        then 2 s later an Esc, which ends Windows' move loop if the command got that far
//!
//! The window says which mouse buttons reach it ("game: left down at X,Y"): what the panel passes on to the game.
//!
//! Each frame checks that the device comes back from Present as the game left it (render target, viewport, scissor
//! test, a cull mode set at start) and says so once if not, or if a Present fails.
//!
//! The panel's game-only parts (money, the % guard, the dispenser, the relay) check that they are in the real game
//! and stay off here. Run it with WCCFPANEL_DRYKEYS=1 (button keys are logged, never pressed), WCCFPANEL_FULLSCREEN=0
//! (the window is never maximized over the screen) and WCCF_DATA / WCCF_KEYS on scratch files. The window opens off
//! every screen and is never activated, so it takes no focus from what the player is doing.
#![allow(clippy::missing_safety_doc)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{FreeLibrary, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D9::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, LoadLibraryW};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MAPVK_VK_TO_VSC, MapVirtualKeyA, VK_ESCAPE, VkKeyScanA,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Interface, s};

const GAME_W: u32 = 1440;
const MK_LBUTTON: usize = 1;
const MK_RBUTTON: usize = 2;
const WM_MOUSELEAVE: u32 = 0x02A3;
const GAME_H: u32 = 900;

unsafe extern "system" fn game_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let what = match m {
        WM_LBUTTONDOWN => Some("left down"),
        WM_LBUTTONUP => Some("left up"),
        WM_RBUTTONDOWN => Some("right down"),
        WM_RBUTTONUP => Some("right up"),
        _ => None,
    };
    if let Some(what) = what {
        println!(
            "game: {what} at {},{}",
            lp.0 as u16 as i16,
            (lp.0 >> 16) as u16 as i16
        );
    }
    // SAFETY: our own window's message, passed on
    unsafe { DefWindowProcA(h, m, wp, lp) }
}

fn xy_lparam(x: i32, y: i32) -> LPARAM {
    LPARAM(((x as u16 as u32) | ((y as u16 as u32) << 16)) as isize)
}

fn post(w: HWND, m: u32, wp: usize, lp: LPARAM) {
    // SAFETY: a message to our own window
    let _ = unsafe { PostMessageA(Some(w), m, WPARAM(wp), lp) };
}

fn client_size(w: HWND, cw: i32, ch: i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: cw,
        bottom: ch,
    };
    // SAFETY: our own window
    unsafe {
        let _ = AdjustWindowRect(&mut r, WS_OVERLAPPEDWINDOW, false);
        let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
        let _ = SetWindowPos(w, None, 0, 0, r.right - r.left, r.bottom - r.top, flags);
    }
}

fn post_key(w: HWND, vk: u32) {
    // SAFETY: a plain lookup
    let lp = 1 | ((unsafe { MapVirtualKeyA(vk, MAPVK_VK_TO_VSC) } as isize) << 16);
    post(w, WM_KEYDOWN, vk as usize, LPARAM(lp));
    post(w, WM_KEYUP, vk as usize, LPARAM(lp | (1 << 30) | (1 << 31)));
}

unsafe extern "system" fn sysmove_timer(h: HWND, _m: u32, id: usize, _t: u32) {
    // SAFETY: our own window's timers
    unsafe {
        let _ = KillTimer(Some(h), id);
        if id == 1 {
            post(h, WM_SYSCOMMAND, SC_MOVE as usize, LPARAM(0));
            SetTimer(Some(h), 2, 2000, Some(sysmove_timer));
        } else {
            post_key(h, u32::from(VK_ESCAPE.0));
        }
    }
}

/// The leading whole number of `s` (C's atoi: 0 without one).
fn atoi(s: &str) -> i32 {
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && c == '-')))
        .map_or(s.len(), |(i, _)| i);
    s[..end].parse().unwrap_or(0)
}

/// "X,Y" of a step.
fn xy(s: &str) -> Option<(i32, i32)> {
    let (a, b) = s.split_once(',')?;
    let num = |t: &str| {
        t.trim_start()
            .starts_with(|c: char| c.is_ascii_digit() || c == '-')
            .then(|| atoi(t.trim_start()))
    };
    Some((num(a)?, num(b)?))
}

struct Game {
    win: HWND,
    panel: HMODULE,
    dir: PathBuf,
    /// the buttons held by the steps (MK_LBUTTON)
    buttons: usize,
    said: bool,
}

impl Game {
    /// One step of the command line; how long to let frames run before the next one (ms).
    fn step(&mut self, s: &str) -> u64 {
        let w = self.win;
        let mk_l = MK_LBUTTON;
        if let Some(ms) = s.strip_prefix("wait:") {
            return atoi(ms).max(0) as u64;
        }
        if let Some((a, b)) = s.strip_prefix("click:").and_then(xy) {
            post(w, WM_MOUSEMOVE, 0, xy_lparam(a, b));
            post(w, WM_LBUTTONDOWN, mk_l, xy_lparam(a, b));
            post(w, WM_LBUTTONUP, 0, xy_lparam(a, b));
            return 150;
        }
        if s == "close" {
            post(w, WM_CLOSE, 0, LPARAM(0));
            return 150;
        }
        if let Some((a, b)) = s.strip_prefix("move:").and_then(xy) {
            post(w, WM_MOUSEMOVE, self.buttons, xy_lparam(a, b));
            return 150;
        }
        if let Some((a, b)) = s.strip_prefix("down:").and_then(xy) {
            self.buttons = mk_l;
            post(w, WM_MOUSEMOVE, 0, xy_lparam(a, b));
            post(w, WM_LBUTTONDOWN, mk_l, xy_lparam(a, b));
            return 150;
        }
        if let Some((a, b)) = s.strip_prefix("up:").and_then(xy) {
            self.buttons = 0;
            post(w, WM_MOUSEMOVE, mk_l, xy_lparam(a, b));
            post(w, WM_LBUTTONUP, 0, xy_lparam(a, b));
            return 150;
        }
        if let Some((a, b)) = s.strip_prefix("rclick:").and_then(xy) {
            post(w, WM_MOUSEMOVE, 0, xy_lparam(a, b));
            post(w, WM_RBUTTONDOWN, MK_RBUTTON, xy_lparam(a, b));
            post(w, WM_RBUTTONUP, 0, xy_lparam(a, b));
            return 150;
        }
        if let Some(text) = s.strip_prefix("type:") {
            for c in text.bytes() {
                // SAFETY: a plain lookup
                post_key(w, (unsafe { VkKeyScanA(c as i8) } & 0xFF) as u32);
            }
            return 150;
        }
        if s == "unload" {
            // SAFETY: the panel this program loaded; the frames go on without it
            match unsafe { FreeLibrary(self.panel) } {
                Ok(()) => println!("unloaded the panel"),
                Err(e) => println!("unload failed ({})", e.code().0 & 0xFFFF),
            }
            return 0;
        }
        if let Some(ms) = s.strip_prefix("sysmove:") {
            // SAFETY: a timer of our own window
            unsafe { SetTimer(Some(w), 1, atoi(ms).max(0) as u32, Some(sysmove_timer)) };
            return 0;
        }
        if let Some(vk) = s.strip_prefix("key:") {
            post_key(w, atoi(vk) as u32);
            return 150;
        }
        if let Some((a, b)) = s.strip_prefix("size:").and_then(xy) {
            client_size(w, a, b);
            return 600;
        }
        if let Some(name) = s.strip_prefix("copy:") {
            let (from, to) = (self.dir.join("wccfpanel_frame.bmp"), self.dir.join(name));
            if let Err(e) = std::fs::copy(&from, &to) {
                println!(
                    "copy: {} -> {} failed ({})",
                    from.display(),
                    to.display(),
                    e.raw_os_error().unwrap_or(0)
                );
            }
            return 0;
        }
        println!("unknown step: {s}");
        0
    }

    /// The device as the game left it, after a Present the panel may have drawn in; said once.
    fn check_device(&mut self, dev: &IDirect3DDevice9, hr: i32) {
        // SAFETY: plain reads of our own device's state
        let (rt, bb, vp, cull, scissor) = unsafe {
            let mut vp = D3DVIEWPORT9::default();
            let _ = dev.GetViewport(&mut vp);
            let (mut cull, mut scissor) = (0, 1);
            let _ = dev.GetRenderState(D3DRS_CULLMODE, &mut cull);
            let _ = dev.GetRenderState(D3DRS_SCISSORTESTENABLE, &mut scissor);
            (
                dev.GetRenderTarget(0).ok(),
                dev.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO).ok(),
                vp,
                cull,
                scissor,
            )
        };
        let kept = rt.as_ref().map(Interface::as_raw) == bb.as_ref().map(Interface::as_raw);
        let changed = hr < 0 || rt.is_none() || !kept || vp.Width != GAME_W || vp.Height != GAME_H;
        if !self.said && (changed || cull != D3DCULL_CW.0 as u32 || scissor != 0) {
            self.said = true;
            println!(
                "device changed: Present hr=0x{:08X}, render target {}, viewport {}x{}, cull {cull}, scissor {scissor}",
                hr as u32,
                if kept { "kept" } else { "changed" },
                vp.Width,
                vp.Height
            );
        }
    }
}

fn d3d_device(win: HWND) -> Result<(IDirect3D9, Option<IDirect3DDevice9>), ExitCode> {
    // SAFETY: Direct3D 9 on our own window
    unsafe {
        let Some(d3d) = Direct3DCreate9(D3D_SDK_VERSION) else {
            println!("no Direct3D 9");
            return Err(ExitCode::from(1));
        };
        let mut pp = D3DPRESENT_PARAMETERS {
            Windowed: true.into(),
            SwapEffect: D3DSWAPEFFECT_DISCARD,
            BackBufferFormat: D3DFMT_X8R8G8B8,
            BackBufferWidth: GAME_W,
            BackBufferHeight: GAME_H,
            BackBufferCount: 1,
            hDeviceWindow: win,
            ..Default::default()
        };
        let flags = (D3DCREATE_SOFTWARE_VERTEXPROCESSING | D3DCREATE_FPU_PRESERVE) as u32;
        let mut dev = None;
        if d3d
            .CreateDevice(
                D3DADAPTER_DEFAULT,
                D3DDEVTYPE_HAL,
                win,
                flags,
                &mut pp,
                &mut dev,
            )
            .is_err()
            || dev.is_none()
        {
            // a machine without a graphics device (CI): the panel still loads and its self-tests run (WCCFPANEL_*TEST)
            println!("no Direct3D 9 device - the panel loads, nothing is drawn");
            return Ok((d3d, None));
        }
        if let Some(d) = &dev {
            let _ = d.SetRenderState(D3DRS_CULLMODE, D3DCULL_CW.0 as u32); // not the default: a panel must keep it
        }
        Ok((d3d, dev))
    }
}

const fn xrgb(r: u32, g: u32, b: u32) -> u32 {
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!(
            "fakegame.exe PANEL.dll [wait:MS click:X,Y move:X,Y down:X,Y up:X,Y rclick:X,Y close type:TEXT key:VK size:W,H copy:NAME unload sysmove:MS ...]"
        );
        return ExitCode::from(2);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map_or_else(PathBuf::new, Path::to_path_buf);

    // SAFETY: a window class and window of our own
    let win = unsafe {
        let inst = GetModuleHandleA(None).unwrap_or_default();
        let wc = WNDCLASSA {
            lpfnWndProc: Some(game_proc),
            hInstance: inst.into(),
            lpszClassName: s!("fakegame"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassA(&wc);
        let style = WS_OVERLAPPEDWINDOW;
        let title = s!("fakegame (wccfpanel test)");
        let (w, h) = (GAME_W as i32, GAME_H as i32);
        match CreateWindowExA(
            WINDOW_EX_STYLE(0),
            s!("fakegame"),
            title,
            style,
            -32000,
            -32000,
            w,
            h,
            None,
            None,
            Some(inst.into()),
            None,
        ) {
            Ok(win) => win,
            Err(e) => {
                println!("no window ({})", e.code().0 & 0xFFFF);
                return ExitCode::from(1);
            }
        }
    };
    client_size(win, GAME_W as i32, GAME_H as i32);
    // SAFETY: our own window
    let _ = unsafe { ShowWindow(win, SW_SHOWNOACTIVATE) };
    let (_d3d, dev) = match d3d_device(win) {
        Ok(d) => d,
        Err(code) => return code,
    };
    // SAFETY: the panel under test; it hooks the device's Present from its own thread
    let panel = match unsafe { LoadLibraryW(&HSTRING::from(args[1].as_str())) } {
        Ok(m) => m,
        Err(e) => {
            println!("could not load {} ({})", args[1], e.code().0 & 0xFFFF);
            return ExitCode::from(1);
        }
    };
    let mut game = Game {
        win,
        panel,
        dir,
        buttons: 0,
        said: false,
    };
    let mut next = 2;
    let mut due = Instant::now() + Duration::from_millis(2500); // the panel's init: its Present detour, the window, the files
    loop {
        // SAFETY: this thread's message queue
        unsafe {
            let mut m = MSG::default();
            while PeekMessageA(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                if m.message == WM_MOUSELEAVE {
                    continue; // the real pointer is not over this window: it stays where the steps put it (the panel's 8 s hover)
                }
                let _ = TranslateMessage(&m);
                DispatchMessageA(&m);
            }
        }
        if Instant::now() >= due {
            let Some(step) = args.get(next) else { break };
            next += 1;
            due = Instant::now() + Duration::from_millis(game.step(step));
        }
        if let Some(dev) = &dev {
            // "the game": a green pitch with a lighter band
            let band = D3DRECT {
                x1: 0,
                y1: GAME_H as i32 / 3,
                x2: GAME_W as i32,
                y2: GAME_H as i32 * 2 / 3,
            };
            // SAFETY: our own device, drawn between frames of the message loop as the game does
            let hr = unsafe {
                let _ = dev.Clear(
                    0,
                    std::ptr::null(),
                    D3DCLEAR_TARGET as u32,
                    xrgb(20, 92, 40),
                    1.0,
                    0,
                );
                let _ = dev.Clear(1, &band, D3DCLEAR_TARGET as u32, xrgb(34, 120, 58), 1.0, 0);
                let _ = dev.BeginScene();
                let _ = dev.EndScene();
                dev.Present(
                    std::ptr::null(),
                    std::ptr::null(),
                    HWND::default(),
                    std::ptr::null(),
                )
                .map_or_else(|e| e.code().0, |()| 0)
            };
            game.check_device(dev, hr);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    ExitCode::SUCCESS
}
