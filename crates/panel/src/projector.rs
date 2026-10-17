//! The projector's picture in seat 1's panel, and the projector's own window off the player's screen.
//!
//! The projector process (its panel in projector mode) copies its frame at Present, downscaled to at most
//! [`MAX_W`]x[`MAX_H`], about 12 times a second, into a named shared memory section ([`Shm`]); seat 1's panel reads
//! it and draws it as a texture (PROJECTOR). The copy never waits for the GPU: a StretchRect into a small render target,
//! GetRenderTargetData into a system-memory surface, and a lock on a later Present with `D3DLOCK_DONOTWAIT`.
//!
//! The section: a 64-byte header, then the pixels (B, G, R, X bytes, `width * 4` a row). `seq` is odd while the
//! projector writes and counts frames (two a frame); seat 1 writes its heartbeat (`reader_ms`) and its window there,
//! and the projector moves its own window off-screen while that heartbeat is fresh and seat 1's window shares its
//! monitor ([`Hider`]).

use std::sync::atomic::{AtomicU32, Ordering::*};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D9::*;
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromRect, MonitorFromWindow};
use windows::Win32::System::Memory::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HRESULT, HSTRING, PCWSTR};

pub const MAX_W: u32 = 800;
pub const MAX_H: u32 = 600;
const HEAD: usize = 64;
const SIZE: usize = HEAD + (MAX_W * MAX_H * 4) as usize;
const MAGIC: u32 = u32::from_le_bytes(*b"FPRJ");
/// The pixels' format: B, G, R, X bytes (D3DFMT_X8R8G8B8 in memory).
pub const BGRX: u32 = 1;
/// A heartbeat older than this is no reader.
const FRESH_MS: u32 = 3000;

#[repr(C)]
struct Head {
    magic: AtomicU32,
    seq: AtomicU32,
    width: AtomicU32,
    height: AtomicU32,
    format: AtomicU32,
    reader_ms: AtomicU32,
    reader_wnd: AtomicU32,
}

// CreateFileMappingW without the windows crate's Win32_Security feature (the default security is all it needs)
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileMappingW(
        file: HANDLE,
        sa: *const std::ffi::c_void,
        protect: u32,
        high: u32,
        low: u32,
        name: PCWSTR,
    ) -> HANDLE;
}

/// Milliseconds on a clock both processes share (the wall clock, cut to 32 bits).
pub fn now_ms() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u32)
}

/// The section's name for a run: its logs folder (WCCF_LOGS, the same for the projector and seat 1), else `fallback`.
pub fn name(fallback: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let key = std::env::var_os("WCCF_LOGS").map_or_else(
        || fallback.to_string_lossy().to_lowercase(),
        |l| l.to_string_lossy().to_lowercase(),
    );
    let mut h = std::hash::DefaultHasher::new();
    key.trim_end_matches('\\').hash(&mut h);
    format!("Local\\ForzaWCCF.projector.{:016x}", h.finish())
}

/// The shared section, mapped. Both sides create it (the second gets the first's).
pub struct Shm {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

// SAFETY: the view is shared memory touched only through atomics (the header) and whole-row copies
unsafe impl Send for Shm {}
// SAFETY: as above
unsafe impl Sync for Shm {}

impl Shm {
    pub fn create(name: &str) -> Option<Shm> {
        // SAFETY: a page-file section of SIZE bytes and its view, closed in Drop
        unsafe {
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE.0,
                0,
                SIZE as u32,
                PCWSTR(HSTRING::from(name).as_ptr()),
            );
            if handle.is_invalid() {
                return None;
            }
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, SIZE);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return None;
            }
            Some(Shm { handle, view })
        }
    }

    fn head(&self) -> &Head {
        // SAFETY: the view is SIZE bytes, page-aligned; Head is atomics only
        unsafe { &*self.view.Value.cast::<Head>() }
    }

    fn pixels(&self) -> *mut u8 {
        // SAFETY: inside the view
        unsafe { self.view.Value.cast::<u8>().add(HEAD) }
    }

    /// One frame of `w` x `h` (at most MAX_W x MAX_H), row `y` filled by `row(y, dst)`.
    pub fn write(&self, w: u32, h: u32, mut row: impl FnMut(usize, &mut [u8])) {
        let (w, h) = (w.min(MAX_W), h.min(MAX_H));
        let hd = self.head();
        let s = hd.seq.load(Relaxed) | 1;
        hd.seq.store(s, Release); // odd: being written
        for y in 0..h as usize {
            // SAFETY: row y of w * 4 bytes lies inside the view (w, h within the maxima)
            let dst = unsafe {
                std::slice::from_raw_parts_mut(
                    self.pixels().add(y * w as usize * 4),
                    w as usize * 4,
                )
            };
            row(y, dst);
        }
        hd.width.store(w, Relaxed);
        hd.height.store(h, Relaxed);
        hd.format.store(BGRX, Relaxed);
        hd.magic.store(MAGIC, Relaxed);
        hd.seq.store(s.wrapping_add(1), Release); // even: complete
    }

    /// The latest complete frame: (seq, width, height); None before the first or while one is written.
    pub fn frame(&self) -> Option<(u32, u32, u32)> {
        let hd = self.head();
        let s = hd.seq.load(Acquire);
        let (w, h) = (hd.width.load(Relaxed), hd.height.load(Relaxed));
        (hd.magic.load(Relaxed) == MAGIC
            && hd.format.load(Relaxed) == BGRX
            && s.is_multiple_of(2)
            && (1..=MAX_W).contains(&w)
            && (1..=MAX_H).contains(&h))
        .then_some((s, w, h))
    }

    /// Row `y` of the frame [`frame`](Self::frame) gave (`w` pixels wide).
    pub fn row(&self, w: u32, y: usize) -> &[u8] {
        let w = w.min(MAX_W) as usize;
        let y = y.min(MAX_H as usize - 1);
        // SAFETY: inside the view
        unsafe { std::slice::from_raw_parts(self.pixels().add(y * w * 4), w * 4) }
    }

    /// The frame `seq` was not overwritten meanwhile (a read between `frame` and this is whole).
    pub fn still(&self, seq: u32) -> bool {
        self.head().seq.load(Acquire) == seq
    }

    /// Seat 1: it is there, and its window.
    pub fn beat(&self, wnd: HWND) {
        let hd = self.head();
        hd.reader_wnd.store(wnd.0 as usize as u32, Relaxed);
        hd.reader_ms.store(now_ms(), Release);
    }

    /// The projector: seat 1's window while its heartbeat is fresh.
    pub fn reader(&self) -> Option<HWND> {
        let hd = self.head();
        let at = hd.reader_ms.load(Acquire);
        let wnd = hd.reader_wnd.load(Relaxed);
        (at != 0 && now_ms().wrapping_sub(at) < FRESH_MS && wnd != 0)
            .then_some(HWND(wnd as usize as *mut _))
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        // SAFETY: our own view and handle
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

/// The projector's side on its render thread: the frame copied about every [`EVERY_MS`] without waiting for the GPU.
pub struct Capture {
    rt: Option<(IDirect3DSurface9, IDirect3DSurface9, u32, u32)>, // render target, system-memory copy, size
    pending: bool,
    last: Option<std::time::Instant>,
    pub frames: u32,
    presents: u32,
    spent: std::time::Duration,
    failed: bool,
}

// SAFETY: the surfaces are used only on the game's render thread (behind CAPTURE's lock); Reset, which releases
// them, runs there too
unsafe impl Send for Capture {}

const EVERY_MS: u64 = 80;
const D3DERR_WASSTILLDRAWING: HRESULT = HRESULT(0x8876_021C_u32 as i32);

impl Capture {
    pub fn new() -> Capture {
        Capture {
            rt: None,
            pending: false,
            last: None,
            frames: 0,
            presents: 0,
            spent: Default::default(),
            failed: false,
        }
    }

    /// Before a device reset: the DEFAULT-pool render target goes (made again on the next copy).
    pub fn release(&mut self) {
        self.rt = None;
        self.pending = false;
    }

    /// At each Present: a finished copy into `shm`, then a new one when due.
    ///
    /// # Safety
    /// `dev` is the game's device, on its render thread.
    pub unsafe fn tick(&mut self, dev: &IDirect3DDevice9, shm: &Shm, log: fn(&str)) {
        if self.failed {
            return;
        }
        let t0 = std::time::Instant::now();
        // SAFETY: as the caller promises; the lock is read within its rows and unlocked
        let r = unsafe { self.step(dev, shm, log) };
        if let Err(e) = r {
            self.failed = true;
            log(&format!(
                "projector: frame copy failed ({e}) - seat 1 shows no projector picture"
            ));
        }
        self.spent += t0.elapsed();
        self.presents += 1;
    }

    unsafe fn step(
        &mut self,
        dev: &IDirect3DDevice9,
        shm: &Shm,
        log: fn(&str),
    ) -> windows::core::Result<()> {
        // SAFETY: device calls with objects of this device and out-parameters we own
        unsafe {
            if self.pending
                && let Some((_, sys, w, h)) = &self.rt
            {
                let mut lr = D3DLOCKED_RECT::default();
                match sys.LockRect(
                    &mut lr,
                    std::ptr::null(),
                    (D3DLOCK_READONLY | D3DLOCK_DONOTWAIT) as u32,
                ) {
                    Ok(()) => {
                        shm.write(*w, *h, |y, dst| {
                            let src = std::slice::from_raw_parts(
                                lr.pBits.cast::<u8>().add(y * lr.Pitch as usize),
                                dst.len(),
                            );
                            dst.copy_from_slice(src);
                        });
                        sys.UnlockRect()?;
                        self.pending = false;
                        self.frames += 1;
                        if self.frames == 1 {
                            log(&format!("projector: frames shared {w}x{h}"));
                        } else if self.frames.is_multiple_of(750) {
                            log(&format!(
                                "projector: {} frames shared, {:.2} ms a Present on average",
                                self.frames,
                                self.spent.as_secs_f64() * 1000.0 / f64::from(self.presents.max(1))
                            ));
                        }
                    }
                    Err(e) if e.code() == D3DERR_WASSTILLDRAWING => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            if self
                .last
                .is_some_and(|t| t.elapsed().as_millis() < u128::from(EVERY_MS))
            {
                return Ok(());
            }
            self.last = Some(std::time::Instant::now());
            let bb = dev.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO)?;
            let mut d = D3DSURFACE_DESC::default();
            bb.GetDesc(&mut d)?;
            let (w, h) = fit(d.Width, d.Height);
            if self.rt.as_ref().is_none_or(|r| (r.2, r.3) != (w, h)) {
                self.rt = None;
                let (mut rt, mut sys) = (None, None);
                dev.CreateRenderTarget(
                    w,
                    h,
                    D3DFMT_X8R8G8B8,
                    D3DMULTISAMPLE_NONE,
                    0,
                    false,
                    &mut rt,
                    std::ptr::null_mut(),
                )?;
                dev.CreateOffscreenPlainSurface(
                    w,
                    h,
                    D3DFMT_X8R8G8B8,
                    D3DPOOL_SYSTEMMEM,
                    &mut sys,
                    std::ptr::null_mut(),
                )?;
                let none = windows::core::Error::empty;
                self.rt = Some((rt.ok_or_else(none)?, sys.ok_or_else(none)?, w, h));
            }
            let Some((rt, sys, _, _)) = &self.rt else {
                return Ok(());
            };
            dev.StretchRect(&bb, std::ptr::null(), rt, std::ptr::null(), D3DTEXF_LINEAR)?;
            dev.GetRenderTargetData(rt, sys)?;
            self.pending = true;
            Ok(())
        }
    }
}

/// The copy's size: the frame scaled down to fit MAX_W x MAX_H (never up), its ratio kept.
pub fn fit(w: u32, h: u32) -> (u32, u32) {
    let s = (MAX_W as f32 / w.max(1) as f32)
        .min(MAX_H as f32 / h.max(1) as f32)
        .min(1.0);
    (
        ((w as f32 * s) as u32).max(1),
        ((h as f32 * s) as u32).max(1),
    )
}

/// The projector's window off the player's screen while seat 1 shows its picture: moved left of every monitor, out of
/// the taskbar and Alt+Tab, still shown (never minimized: the game keeps presenting), and put back as it was once
/// seat 1's heartbeat stops. Left alone when seat 1's window is on another monitor (a real second screen), and with
/// WCCFPANEL_PROJECTOR_WINDOW=keep.
pub struct Hider {
    wnd: HWND,
    /// Where it was and its extended style, while moved away.
    saved: Option<(RECT, isize)>,
}

impl Hider {
    pub fn new(wnd: HWND) -> Hider {
        Hider { wnd, saved: None }
    }

    /// One look (every half second): hide or put back.
    pub fn tick(&mut self, seat: Option<HWND>, log: fn(&str)) {
        let mut r = RECT::default();
        // SAFETY: window queries and changes on a window of this process; the async forms never wait for its thread
        unsafe {
            if GetWindowRect(self.wnd, &mut r).is_err() {
                return;
            }
            let home = self.saved.map_or(r, |s| s.0);
            let want = seat.is_some_and(|s| {
                MonitorFromWindow(s, MONITOR_DEFAULTTONEAREST)
                    == MonitorFromRect(&home, MONITOR_DEFAULTTONEAREST)
            });
            let away_x = GetSystemMetrics(SM_XVIRTUALSCREEN) - (r.right - r.left) - 64;
            match (want, self.saved) {
                (true, None) => {
                    let ex = GetWindowLongPtrW(self.wnd, GWL_EXSTYLE) as isize;
                    self.saved = Some((r, ex));
                    let _ = ShowWindowAsync(self.wnd, SW_HIDE); // the taskbar sees a style change on show only
                    SetWindowLongPtrW(
                        self.wnd,
                        GWL_EXSTYLE,
                        ((ex | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize)) as _,
                    );
                    self.place(away_x, r.top);
                    let _ = ShowWindowAsync(self.wnd, SW_SHOWNOACTIVATE);
                    log(&format!(
                        "projector: window moved off-screen (x {away_x}, was {},{}) - seat 1 shows its picture",
                        r.left, r.top
                    ));
                }
                // the game put it back by itself: away again
                (true, Some(_)) if r.right > GetSystemMetrics(SM_XVIRTUALSCREEN) => {
                    self.place(away_x, r.top)
                }
                (false, Some((old, ex))) => {
                    self.saved = None;
                    let _ = ShowWindowAsync(self.wnd, SW_HIDE);
                    SetWindowLongPtrW(self.wnd, GWL_EXSTYLE, ex as _);
                    self.place(old.left, old.top);
                    let _ = ShowWindowAsync(self.wnd, SW_SHOWNOACTIVATE);
                    log("projector: window back on screen - seat 1 no longer shows its picture");
                }
                _ => {}
            }
        }
    }

    fn place(&self, x: i32, y: i32) {
        // SAFETY: as in tick
        let _ = unsafe {
            SetWindowPos(
                self.wnd,
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let name = format!("Local\\ForzaWCCF.projector.test{}", std::process::id());
        let a = Shm::create(&name).expect("the section");
        let b = Shm::create(&name).expect("the same section, again");
        assert_eq!(b.frame(), None, "no frame before the first");
        assert_eq!(b.reader(), None);
        a.write(4, 2, |y, row| {
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                px.copy_from_slice(&[x as u8, y as u8, 7, 0]);
            }
        });
        let (s, w, h) = b.frame().expect("a frame");
        assert_eq!((w, h), (4, 2));
        assert_eq!(b.row(w, 1)[8..12], [2, 1, 7, 0]);
        assert!(b.still(s));
        a.write(4, 2, |_, row| row.fill(9));
        let (s2, ..) = b.frame().unwrap();
        assert_eq!(s2, s + 2);
        assert!(!b.still(s) && b.row(4, 0)[0] == 9);
        // too big: cut to the maxima
        a.write(5000, 5000, |_, row| row.fill(1));
        assert_eq!(b.frame().map(|f| (f.1, f.2)), Some((MAX_W, MAX_H)));
        // seat 1's heartbeat
        b.beat(HWND(0x1234 as *mut _));
        assert_eq!(a.reader(), Some(HWND(0x1234 as *mut _)));
        assert_eq!(fit(1600, 600), (800, 300));
        assert_eq!(fit(640, 480), (640, 480));
    }
}
