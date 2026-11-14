//! The 2017-18 client's window held hidden through the boot and update screens: wccfnet.dll holds the game's own
//! show and puts [`PROP`] on the window (`native/wccfnet`, `boot`). The panel shows it when the game reaches the start
//! warning picture, the screen before the intro: the factory that creates the node after the boot screens (seat 1:
//! ClientStartUpWait, the projector: Prj Wakeup) is wrapped through its vtable slot, and two frames after it ran the
//! window shows (seat 1 maximized as at the start, the projector only with its setting on), without taking the focus
//! (the Launcher hands it over when it minimizes). A check that fails shows the window at once. 60 s after the hold
//! wccfnet's fallback asks the panel to show it ([`forza_native::BOOT_PANEL`]), or shows it itself when the panel never
//! held it.

use crate::patches::{CLIENT_STAMP, exe_base, exe_name, is_client, patch_code, pe_stamp, read_u32};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicUsize, Ordering::Relaxed};
use windows::Win32::Foundation::{FILETIME, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::WindowsAndMessaging::{
    GetPropW, IsWindowVisible, PostMessageW, RemovePropW, SW_SHOWNA, SW_SHOWNOACTIVATE,
    SW_SHOWNORMAL, SetPropW, ShowWindow, ShowWindowAsync,
};
use windows::core::PCWSTR;

/// wccfnet's property on the held window.
const PROP: PCWSTR = PCWSTR(forza_native::BOOT_PROP);
/// The vtable slot (RVA) of the factory that makes the node after the boot screens, and the factory it holds (RVA): seat
/// 1's makeClientStartUpWait (vtable 0x125DA30, slot 1), the projector's makePrjWakeup (vtable 0x125E1C0, slot 1).
const SEAT_SLOT: (usize, usize) = (0x00E5_DA34, 0x0000_7CC0);
const PROJ_SLOT: (usize, usize) = (0x00E5_E1C4, 0x0000_A620);
/// Presents after the factory ran before the window shows: the warning picture is on screen by then.
const FRAMES_AFTER: u32 = 2;

/// The window is held hidden and this module shows it.
static HIDDEN: AtomicBool = AtomicBool::new(false);
static WND: AtomicIsize = AtomicIsize::new(0);
static PROJECTOR: AtomicBool = AtomicBool::new(false);
/// The factory, and its slot while it points at [`make_node`] (0: put back, or never swapped).
static REAL: AtomicUsize = AtomicUsize::new(0);
static SLOT: AtomicUsize = AtomicUsize::new(0);
/// 0: the factory has not run; then 1 + the Presents since.
static MARK: AtomicU32 = AtomicU32::new(0);
/// The first frame's maximize, waiting for the window to show.
static MAX_DEFERRED: AtomicBool = AtomicBool::new(false);

type Make = unsafe extern "thiscall" fn(*mut c_void) -> *mut c_void;

/// The window is held hidden: the first-frame maximize and the pop-out board wait.
pub fn held() -> bool {
    HIDDEN.load(Relaxed)
}

/// `h` carries wccfnet's property (it counts as the game's window though hidden).
pub fn is_held(h: HWND) -> bool {
    // SAFETY: a plain window query
    !unsafe { GetPropW(h, PROP) }.is_invalid()
}

/// The first frame's maximize runs when the window shows.
pub fn defer_maximize() {
    MAX_DEFERRED.store(true, Relaxed);
}

/// Seconds since the process started (the timings in the logs count from there).
fn since_start() -> f64 {
    let n = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    let [mut c, mut e, mut k, mut u] = [FILETIME::default(); 4];
    // SAFETY: out-parameters we own, one each
    let ok =
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) }.is_ok();
    // SAFETY: a plain clock query
    let now = n(unsafe { GetSystemTimeAsFileTime() });
    if ok {
        now.saturating_sub(n(c)) as f64 / 1e7
    } else {
        0.0
    }
}

/// The slot at `at` holds `want` (the original): it now holds `new`. `Err`: what it holds instead, or that it could not
/// be written; nothing changed.
fn swap_slot(at: usize, want: usize, new: usize) -> Result<(), String> {
    match read_u32(at) {
        Some(v) if v as usize == want => {
            if patch_code(at, &(new as u32).to_le_bytes()) {
                Ok(())
            } else {
                Err(format!("slot {at:08X} not writable"))
            }
        }
        Some(v) => Err(format!("slot {at:08X} holds {v:08X}, not {want:08X}")),
        None => Err(format!("slot {at:08X} not readable")),
    }
}

/// The factory's wrapper: the game's node as the original makes it, and the mark.
unsafe extern "thiscall" fn make_node(this: *mut c_void) -> *mut c_void {
    // SAFETY: REAL is the factory this slot held, stored before the slot pointed here; same convention
    let node = unsafe { std::mem::transmute::<usize, Make>(REAL.load(Relaxed))(this) };
    if !node.is_null() {
        let _ = MARK.compare_exchange(0, 1, Relaxed, Relaxed);
    }
    node
}

/// The window shown now (a check failed): the property off, shown as the game would have.
fn show_now(h: HWND, why: &str, log: fn(&str)) {
    log(&format!(
        "boot screens: {why} - patch refused, window shown now"
    ));
    let cmd = if PROJECTOR.load(Relaxed) {
        SW_SHOWNOACTIVATE
    } else {
        SW_SHOWNORMAL
    };
    // SAFETY: a window of this process; ShowWindowAsync never waits for its thread
    unsafe {
        if RemovePropW(h, PROP).is_ok() {
            let _ = ShowWindowAsync(h, cmd);
        }
    }
}

/// At the panel's start, once the game's window `h` is subclassed (`hooked`: Present is detoured, so frames count): if
/// wccfnet holds it hidden, the factory's slot is wrapped (its value checked first). The projector's role is set first.
pub fn install(h: HWND, hooked: bool, projector: bool, log: fn(&str)) {
    if !is_held(h) {
        return;
    }
    PROJECTOR.store(projector, Relaxed);
    // SAFETY: a plain window query
    if unsafe { IsWindowVisible(h) }.as_bool() {
        // SAFETY: a window of this process
        let _ = unsafe { RemovePropW(h, PROP) };
        log("boot screens: the window is already shown - nothing held");
        return;
    }
    if !hooked {
        return show_now(h, "no Present hook", log);
    }
    let base = exe_base();
    if !is_client(&exe_name()) || pe_stamp(base) != Some(CLIENT_STAMP) {
        return show_now(h, "not the client studied (PE stamp)", log);
    }
    let (slot, func) = if projector { PROJ_SLOT } else { SEAT_SLOT };
    let (slot, func) = (base + slot, base + func);
    REAL.store(func, Relaxed); // before the swap: the game may call the wrapper at once
    WND.store(h.0 as isize, Relaxed);
    HIDDEN.store(true, Relaxed);
    if let Err(why) = swap_slot(slot, func, make_node as Make as usize) {
        HIDDEN.store(false, Relaxed);
        return show_now(h, &why, log);
    }
    SLOT.store(slot, Relaxed);
    // wccfnet's fallback now asks this panel to show the window (the projector setting applies)
    // SAFETY: a window of this process
    if unsafe {
        SetPropW(
            h,
            PROP,
            Some(HANDLE(forza_native::BOOT_PANEL as *mut c_void)),
        )
    }
    .is_err()
    {
        log("boot screens: the window's property not updated - wccfnet's fallback shows it itself");
    }
    log(&format!(
        "boot screens: window hidden until the start warning ({} factory at {slot:08X} wrapped, t+{:.1} s)",
        if projector {
            "Prj Wakeup"
        } else {
            "ClientStartUpWait"
        },
        since_start()
    ));
}

/// The factory's slot back to the original, if it still points at the wrapper.
fn put_back(log: fn(&str)) {
    let slot = SLOT.swap(0, Relaxed);
    if slot != 0 && swap_slot(slot, make_node as Make as usize, REAL.load(Relaxed)).is_err() {
        log("boot screens: the factory's slot was changed by someone else - left as it is");
    }
}

/// Every Present while the window is held: two frames after the factory ran, the window's thread is told to show it;
/// a window someone else showed (wccfnet's fallback) gets the deferred maximize.
pub fn tick(log: fn(&str)) {
    if !HIDDEN.load(Relaxed) {
        return;
    }
    let h = HWND(WND.load(Relaxed) as *mut c_void);
    // SAFETY: a plain window query
    if unsafe { IsWindowVisible(h) }.as_bool() {
        HIDDEN.store(false, Relaxed);
        put_back(log);
        // SAFETY: a window of this process; nobody shows it again
        let _ = unsafe { RemovePropW(h, PROP) };
        log(&format!(
            "boot screens: window shown without the panel (t+{:.1} s)",
            since_start()
        ));
        if MAX_DEFERRED.swap(false, Relaxed) {
            // SAFETY: a message to the game's window, handled by the panel's window procedure on its thread
            let _ = unsafe { PostMessageW(Some(h), crate::WM_FORZA_FS, WPARAM(1), LPARAM(0)) };
        }
        return;
    }
    let n = MARK.load(Relaxed);
    if n == 0 {
        return;
    }
    MARK.store(n + 1, Relaxed);
    if n >= FRAMES_AFTER {
        HIDDEN.store(false, Relaxed);
        put_back(log);
        // SAFETY: as above
        let _ = unsafe { PostMessageW(Some(h), crate::WM_FORZA_SHOW, WPARAM(0), LPARAM(0)) };
    }
}

/// [`crate::WM_FORZA_SHOW`] on the window's thread (`fallback`: wccfnet sent it, 60 s after the hold): the window
/// shown without taking the focus (seat 1: maximized if the first frame asked for it; the projector: only when
/// `projector_on`), the property off and the factory's slot back. Nothing if the property is gone (shown already).
pub fn show(h: HWND, projector_on: bool, fallback: bool, log: fn(&str)) {
    // SAFETY: a window of this process, on its own thread
    if unsafe { RemovePropW(h, PROP) }.is_err() {
        return;
    }
    HIDDEN.store(false, Relaxed);
    put_back(log);
    let t = since_start();
    let how = if fallback {
        " by wccfnet's fallback, no start warning in 60 s"
    } else {
        ""
    };
    if PROJECTOR.load(Relaxed) {
        if projector_on {
            // SAFETY: as above
            let _ = unsafe { ShowWindow(h, SW_SHOWNOACTIVATE) };
            log(&format!(
                "boot screens done: window shown{how} (t+{t:.1} s)"
            ));
        } else {
            log(&format!(
                "boot screens done: window kept hidden{how}, projector setting off (t+{t:.1} s)"
            ));
        }
        return;
    }
    if MAX_DEFERRED.swap(false, Relaxed) {
        crate::set_maximized(h, true); // SW_MAXIMIZE, as the first frame did before the hold
    }
    // SAFETY: as above; SW_SHOWNA keeps a maximized window maximized
    let _ = unsafe { ShowWindow(h, SW_SHOWNA) };
    log(&format!(
        "boot screens done: window shown{how} (t+{t:.1} s)"
    ));
}

/// FreeLibrary: the factory's slot back, and a window still held shown now (wccfnet's fallback would only post to
/// this panel's window procedure, which goes).
pub fn unload(log: fn(&str)) {
    put_back(log);
    if HIDDEN.swap(false, Relaxed) {
        show_now(
            HWND(WND.load(Relaxed) as *mut c_void),
            "the panel unloads",
            log,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, MSG, PM_REMOVE, PeekMessageW, WINDOW_EX_STYLE,
        WS_OVERLAPPEDWINDOW,
    };
    use windows::core::w;

    /// The tests that use the module's state take turns (`cargo test` runs them on threads of one process).
    static SERIAL: Mutex<()> = Mutex::new(());
    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn keep(line: &str) {
        LINES.lock().unwrap().push(line.to_owned());
    }

    fn lines() -> Vec<String> {
        LINES.lock().unwrap().clone()
    }

    /// A hidden window held by the panel, as `install` leaves the game's: the module's state set to match.
    fn held_window(projector: bool) -> HWND {
        // SAFETY: a plain hidden test window of a system class, on this thread
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("boot test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                200,
                100,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        // SAFETY: our test window
        unsafe {
            SetPropW(
                h,
                PROP,
                Some(HANDLE(forza_native::BOOT_PANEL as *mut c_void)),
            )
        }
        .unwrap();
        WND.store(h.0 as isize, Relaxed);
        PROJECTOR.store(projector, Relaxed);
        MARK.store(0, Relaxed);
        MAX_DEFERRED.store(false, Relaxed);
        SLOT.store(0, Relaxed);
        HIDDEN.store(true, Relaxed);
        h
    }

    /// The show message waiting for `h`, and its wParam.
    fn posted(h: HWND) -> Option<usize> {
        let mut m = MSG::default();
        // SAFETY: this thread's own queue
        unsafe {
            PeekMessageW(
                &mut m,
                Some(h),
                crate::WM_FORZA_SHOW,
                crate::WM_FORZA_SHOW,
                PM_REMOVE,
            )
        }
        .as_bool()
        .then_some(m.wParam.0)
    }

    fn visible(h: HWND) -> bool {
        // SAFETY: a plain window query
        unsafe { IsWindowVisible(h) }.as_bool()
    }

    #[test]
    fn the_window_shows_on_the_second_present_after_the_factory_and_once() {
        let _turn = SERIAL.lock().unwrap();
        let h = held_window(false);
        tick(keep);
        assert_eq!(posted(h), None, "the factory has not run: still held");
        MARK.store(1, Relaxed); // what make_node does
        tick(keep);
        assert_eq!(
            posted(h),
            None,
            "one Present after: the warning picture may not be up yet"
        );
        assert!(held());
        tick(keep);
        assert_eq!(
            posted(h),
            Some(0),
            "the second Present after: shown, not as a fallback"
        );
        assert!(!held());
        tick(keep);
        assert_eq!(posted(h), None, "told once");
        show(h, true, false, keep);
        assert!(visible(h) && !is_held(h));
        let said = lines();
        assert!(
            said.last()
                .unwrap()
                .starts_with("boot screens done: window shown (t+"),
            "{said:?}"
        );
        show(h, true, true, keep); // wccfnet's fallback arriving late
        assert_eq!(lines().len(), said.len(), "the second show does nothing");
        // SAFETY: our test window
        unsafe { DestroyWindow(h) }.unwrap();
    }

    #[test]
    fn the_projector_with_its_setting_off_stays_hidden_at_the_fallback() {
        let _turn = SERIAL.lock().unwrap();
        let h = held_window(true);
        show(h, false, true, keep);
        assert!(!visible(h) && !is_held(h) && !held());
        let said = lines();
        assert!(
            said.last().unwrap().starts_with(
                "boot screens done: window kept hidden by wccfnet's fallback, no start warning in 60 s, projector setting off"
            ),
            "{said:?}"
        );
        // SAFETY: our test window
        unsafe { DestroyWindow(h) }.unwrap();
    }

    #[test]
    fn a_window_shown_by_someone_else_ends_the_hold() {
        let _turn = SERIAL.lock().unwrap();
        let h = held_window(false);
        // SAFETY: our test window
        let _ = unsafe { ShowWindow(h, SW_SHOWNA) };
        MARK.store(1, Relaxed);
        tick(keep);
        assert!(!held() && !is_held(h), "the hold and the property gone");
        assert_eq!(posted(h), None);
        assert!(
            lines()
                .last()
                .unwrap()
                .starts_with("boot screens: window shown without the panel"),
            "{:?}",
            lines()
        );
        // SAFETY: our test window
        unsafe { DestroyWindow(h) }.unwrap();
    }

    /// The slot as memory holds it (swap_slot wrote it through a raw pointer).
    fn get(slot: &[usize; 1]) -> usize {
        // SAFETY: our own array
        unsafe { std::ptr::read_volatile(slot.as_ptr()) }
    }

    #[test]
    fn swap_slot_refuses_a_value_it_does_not_expect() {
        let mut slot = [0x00D5_7CC0usize];
        let at = slot.as_mut_ptr() as usize;
        let err = swap_slot(at, 0x00D5_A620, 0x1234_5678).unwrap_err();
        assert!(err.contains("holds 00D57CC0, not 00D5A620"), "{err}");
        assert_eq!(get(&slot), 0x00D5_7CC0);
    }

    #[test]
    fn swap_slot_swaps_the_expected_value() {
        let mut slot = [0x00D5_7CC0usize];
        let at = slot.as_mut_ptr() as usize;
        swap_slot(at, 0x00D5_7CC0, 0x1234_5678).unwrap();
        assert_eq!(get(&slot), 0x1234_5678);
        swap_slot(at, 0x1234_5678, 0x00D5_7CC0).unwrap(); // put back, as put_back does
        assert_eq!(get(&slot), 0x00D5_7CC0);
    }

    #[test]
    fn the_wrapper_marks_once_and_returns_the_node() {
        let _turn = SERIAL.lock().unwrap();
        MARK.store(0, Relaxed);
        unsafe extern "thiscall" fn fake(this: *mut c_void) -> *mut c_void {
            this.wrapping_add(0x10)
        }
        REAL.store(fake as Make as usize, Relaxed);
        let base = 0x1000 as *mut c_void;
        // SAFETY: the wrapper over a fake factory with its convention
        let node = unsafe { make_node(base) };
        assert_eq!(node as usize, 0x1010);
        assert_eq!(MARK.load(Relaxed), 1);
    }

    #[test]
    fn the_process_clock_runs() {
        let t = since_start();
        assert!(t > 0.0 && t < 3600.0, "{t}");
    }
}
