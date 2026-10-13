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
//! Test switch: `FORZA_PANEL_DUMP=1` writes `wccfpanel_frame.bmp` beside the program once a window size has held for
//! 10 frames (fakegame.exe's `size:` and `copy:` steps).
#![cfg(target_arch = "x86")]

mod painter;

use egui::{Event, Key, Modifiers, PointerButton, Pos2, TextureId, ViewportId, pos2, vec2};
use forza::ui::Lang;
use forza::ui::panel::{self, Page, PanelState};
use forza::ui::theme::{self, Edition};
use std::cell::Cell;
use std::ffi::c_void;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
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
static WANTS_POINTER: AtomicBool = AtomicBool::new(false);
static EXE_DIR: OnceLock<PathBuf> = OnceLock::new();
static DLL_DIR: OnceLock<PathBuf> = OnceLock::new();

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
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("forza_panel.log"))
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
    std::thread::sleep(Duration::from_millis(300));
    log(&format!("forza-panel init (pid {})", std::process::id()));
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
        let dir = font_dir();
        if let Err(e) = theme::install(&ctx, Edition::E1011, &dir) {
            log(&format!(
                "fonts from {}: {e} (egui's own fonts used)",
                dir.display()
            ));
        }
        let mut st = PanelState::sample(Edition::E1011, Lang::En, Page::Game);
        st.picture = Some(GAME);
        Render {
            ctx,
            st,
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
        let st = &mut self.st;
        let out = self.ctx.run_ui(raw, |ui| panel::show(ui, st));
        WANTS_KEYS.store(self.ctx.egui_wants_keyboard_input(), Relaxed);
        WANTS_POINTER.store(self.ctx.egui_wants_pointer_input(), Relaxed);
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

    /// `wccfpanel_frame.bmp` beside the program, once a window size has held for 10 frames.
    unsafe fn dump(&mut self, dev: &IDirect3DDevice9, scbb: &IDirect3DSurface9, w: u32, h: u32) {
        if self.size != (w, h) {
            (self.size, self.held) = ((w, h), 0);
        }
        self.held += 1;
        if self.held != 10 {
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
        let path = EXE_DIR
            .get()
            .cloned()
            .unwrap_or_default()
            .join("wccfpanel_frame.bmp");
        match rgb.map_err(|e| e.to_string()).and_then(|rgb| {
            image::save_buffer(&path, &rgb, w, h, image::ExtendedColorType::Rgb8)
                .map_err(|e| e.to_string())
        }) {
            Ok(()) => log(&format!("frame dump {w}x{h}")),
            Err(e) => log(&format!("frame dump: {e}")),
        }
    }
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
    if input(m, wp, lp) {
        return LRESULT(0); // egui's: the game does not see it
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
            return false; // the game follows the mouse too
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
        WM_KEYDOWN | WM_SYSKEYDOWN => (key(true).into_iter().collect(), true),
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
    if keys {
        WANTS_KEYS.load(Relaxed)
    } else {
        WANTS_POINTER.load(Relaxed)
    }
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
