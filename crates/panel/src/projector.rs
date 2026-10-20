//! The projector's own window: resizable, the picture letterboxed to the client area, and (with [`crate::fullscreen`])
//! borderless full screen. The game's back buffer keeps its size (1600x600, two screens side by side); no device Reset.
//! At the game's own client size its Present runs untouched. At any other size the panel draws the frame, scaled with
//! its shape kept on black bars, on a swap chain of its own at the client size and shows that instead
//! (`wccfpanel.c` `present_composed`).

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D9::*;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HRESULT;

/// The frame `(bw, bh)` scaled to fit the client `(cw, ch)`, its shape kept, centred.
pub fn letterbox(cw: u32, ch: u32, bw: u32, bh: u32) -> RECT {
    let s = (cw as f32 / bw.max(1) as f32).min(ch as f32 / bh.max(1) as f32);
    let (w, h) = (
        ((bw as f32 * s) as i32).clamp(1, cw as i32),
        ((bh as f32 * s) as i32).clamp(1, ch as i32),
    );
    let (x, y) = ((cw as i32 - w) / 2, (ch as i32 - h) / 2);
    RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

/// The start size: the game's client `(cw, ch)` as it is, or scaled down (shape kept) until the window with its frame
/// `(fw, fh)` fits the work area `(ww, wh)`.
pub fn clamp_client(cw: i32, ch: i32, fw: i32, fh: i32, ww: i32, wh: i32) -> (i32, i32) {
    let s = ((ww - fw) as f32 / cw as f32)
        .min((wh - fh) as f32 / ch as f32)
        .min(1.0);
    if s >= 1.0 {
        return (cw, ch);
    }
    (
        ((cw as f32 * s) as i32).max(160),
        ((ch as f32 * s) as i32).max(60),
    )
}

/// The work area (the monitor without the taskbar) of the monitor `h` is on; `over` stands in for it (the tests).
pub fn work_area(h: HWND, over: Option<RECT>) -> RECT {
    if let Some(r) = over {
        return r;
    }
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: plain queries with an out-parameter we own
    unsafe {
        let _ = GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi);
    }
    mi.rcWork
}

/// Once, on the window's thread: a frame with a caption and a sizing border, the client at the game's size or smaller
/// to fit the work area, the whole window moved inside it.
pub fn frame(h: HWND, over: Option<RECT>, log: fn(&str)) {
    // SAFETY: window calls on a window of this process, from its own thread
    unsafe {
        let mut cr = RECT::default();
        let _ = GetClientRect(h, &mut cr);
        let (cw, ch) = (cr.right.max(1), cr.bottom.max(1));
        let st =
            (GetWindowLongW(h, GWL_STYLE) & !(WS_POPUP.0 as i32)) | WS_OVERLAPPEDWINDOW.0 as i32;
        SetWindowLongW(h, GWL_STYLE, st);
        let ex = WINDOW_EX_STYLE(GetWindowLongW(h, GWL_EXSTYLE) as u32);
        let mut o = RECT {
            right: cw,
            bottom: ch,
            ..Default::default()
        };
        let _ = AdjustWindowRectEx(&mut o, WINDOW_STYLE(st as u32), false, ex);
        let (fw, fh) = (o.right - o.left - cw, o.bottom - o.top - ch);
        let wa = work_area(h, over);
        let (w, hh) = clamp_client(cw, ch, fw, fh, wa.right - wa.left, wa.bottom - wa.top);
        let (ow, oh) = (w + fw, hh + fh);
        let mut wr = RECT::default();
        let _ = GetWindowRect(h, &mut wr);
        let x = wr.left.clamp(wa.left, (wa.right - ow).max(wa.left));
        let y = wr.top.clamp(wa.top, (wa.bottom - oh).max(wa.top));
        let _ = SetWindowPos(
            h,
            None,
            x,
            y,
            ow,
            oh,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        log(&format!(
            "projector: resizable window, client {w}x{hh} (game {cw}x{ch}) at {x},{y}"
        ));
    }
}

/// The projector's swap chain at the client size, made on the first frame that needs it.
#[derive(Default)]
pub struct View {
    sc: Option<(IDirect3DSwapChain9, u32, u32)>,
    failed: bool,
    shown: Option<(u32, u32)>,
}

// SAFETY: used only on the game's render thread (behind VIEW's lock); Reset, which releases it, runs there too
unsafe impl Send for View {}

impl View {
    /// Before a device reset: the swap chain goes (made again on the next frame).
    pub fn release(&mut self) {
        self.sc = None;
    }

    /// At Present: the game's frame letterboxed into `wnd`'s client area and shown, and the Present's result; `None`
    /// when the game's own Present should run (client = back buffer, minimized, or anything failed).
    ///
    /// # Safety
    /// `dev` is the game's device, on its render thread.
    pub unsafe fn present(
        &mut self,
        dev: &IDirect3DDevice9,
        wnd: HWND,
        log: fn(&str),
    ) -> Option<HRESULT> {
        if self.failed || wnd.is_invalid() {
            return None;
        }
        // SAFETY: device and window calls with objects of this device and out-parameters we own
        unsafe {
            let mut cr = RECT::default();
            if IsIconic(wnd).as_bool()
                || GetClientRect(wnd, &mut cr).is_err()
                || cr.right < 1
                || cr.bottom < 1
            {
                return None;
            }
            let (cw, ch) = (cr.right as u32, cr.bottom as u32);
            let bb = dev.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO).ok()?;
            let mut d = D3DSURFACE_DESC::default();
            bb.GetDesc(&mut d).ok()?;
            if (cw, ch) == (d.Width, d.Height) {
                return None;
            }
            if self.sc.as_ref().is_none_or(|s| (s.1, s.2) != (cw, ch)) {
                self.sc = None;
                let mut ip = D3DPRESENT_PARAMETERS::default();
                dev.GetSwapChain(0)
                    .ok()?
                    .GetPresentParameters(&mut ip)
                    .ok()?;
                let mut pp = D3DPRESENT_PARAMETERS {
                    BackBufferWidth: cw,
                    BackBufferHeight: ch,
                    BackBufferFormat: D3DFMT_X8R8G8B8,
                    BackBufferCount: 1,
                    SwapEffect: D3DSWAPEFFECT_DISCARD,
                    hDeviceWindow: wnd,
                    Windowed: true.into(),
                    PresentationInterval: ip.PresentationInterval, // the game's own pacing
                    ..Default::default()
                };
                let mut sc = None;
                if !ip.Windowed.as_bool()
                    || dev.CreateAdditionalSwapChain(&mut pp, &mut sc).is_err()
                {
                    self.failed = true;
                    log(
                        "projector: no swap chain of its own - the picture stays at the game's size",
                    );
                    return None;
                }
                self.sc = Some((sc?, cw, ch));
            }
            let sc = &self.sc.as_ref()?.0;
            let scbb = sc.GetBackBuffer(0, D3DBACKBUFFER_TYPE_MONO).ok()?;
            dev.ColorFill(&scbb, std::ptr::null(), 0xFF00_0000).ok()?;
            let dst = letterbox(cw, ch, d.Width, d.Height);
            dev.StretchRect(&bb, std::ptr::null(), &scbb, &dst, D3DTEXF_LINEAR)
                .ok()?;
            if self.shown != Some((cw, ch)) {
                self.shown = Some((cw, ch));
                log(&format!(
                    "projector: composed {}x{} into {cw}x{ch}",
                    d.Width, d.Height
                ));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_keeps_the_shape() {
        let r = |l, t, rr, b| RECT {
            left: l,
            top: t,
            right: rr,
            bottom: b,
        };
        // 1600x600 into a 1200x700 client: full width, bars above and below
        assert_eq!(letterbox(1200, 700, 1600, 600), r(0, 125, 1200, 575));
        // into 3200x1000: full height, bars left and right
        assert_eq!(letterbox(3200, 1000, 1600, 600), r(267, 0, 2933, 1000));
        assert_eq!(letterbox(1600, 600, 1600, 600), r(0, 0, 1600, 600));
    }

    #[test]
    fn start_size_fits_the_work_area() {
        // fits: the game's size
        assert_eq!(clamp_client(1600, 600, 16, 39, 2560, 1400), (1600, 600));
        // 1440x860 work area: 1424 wide, shape kept
        let (w, h) = clamp_client(1600, 600, 16, 39, 1440, 860);
        assert_eq!(w, 1424);
        assert!((533..=534).contains(&h));
        // never absurdly small
        assert!(clamp_client(1600, 600, 16, 39, 100, 100).0 >= 160);
    }
}
