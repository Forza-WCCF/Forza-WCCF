//! Borderless full screen, one helper for both windows (the panel's F11 and the projector's F11, double-click and Esc):
//! the style and placement saved, WS_POPUP over the whole monitor the window is on (Windows then hides the taskbar),
//! and both put back on leaving. No exclusive Direct3D mode: the game's device stays windowed.

use std::sync::Mutex;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// What leaving restores: the window style and its placement (maximized or not).
pub struct Saved {
    style: i32,
    place: WINDOWPLACEMENT,
}

/// A window's full-screen state: `Some` while it is full screen.
pub type Slot = Mutex<Option<Saved>>;

pub fn is_on(slot: &Slot) -> bool {
    slot.lock().is_ok_and(|s| s.is_some())
}

/// The monitor `h` is on (its whole area); `over` stands in for it (the tests' pretend screen).
pub fn monitor(h: HWND, over: Option<RECT>) -> RECT {
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
    mi.rcMonitor
}

/// F11: into full screen, or back. On the window's thread. Returns whether it is full screen now.
pub fn toggle(h: HWND, slot: &Slot, over: Option<RECT>) -> bool {
    let Ok(mut s) = slot.lock() else { return false };
    // SAFETY: window calls on a window of this process, from its own thread
    unsafe {
        if let Some(saved) = s.take() {
            SetWindowLongW(h, GWL_STYLE, saved.style);
            let _ = SetWindowPlacement(h, &saved.place);
            let _ = SetWindowPos(
                h,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
            );
            return false;
        }
        let style = GetWindowLongW(h, GWL_STYLE);
        let mut place = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        let _ = GetWindowPlacement(h, &mut place);
        *s = Some(Saved { style, place });
        if IsZoomed(h).as_bool() {
            let _ = ShowWindow(h, SW_SHOWNORMAL); // the placement above brings the maximized state back
        }
        SetWindowLongW(
            h,
            GWL_STYLE,
            (style & !(WS_OVERLAPPEDWINDOW.0 as i32)) | WS_POPUP.0 as i32,
        );
        let m = monitor(h, over);
        let _ = SetWindowPos(
            h,
            Some(HWND_TOP),
            m.left,
            m.top,
            m.right - m.left,
            m.bottom - m.top,
            SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
        );
        true
    }
}

/// Esc: out of full screen when in it. Returns whether it left.
pub fn leave(h: HWND, slot: &Slot, over: Option<RECT>) -> bool {
    is_on(slot) && !toggle(h, slot, over)
}
