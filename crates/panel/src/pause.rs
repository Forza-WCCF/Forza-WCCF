//! Pause P1 (`docs/research/PAUSE.md` §4-5): the game held inside its own Present for at most 20 s in a local match.
//!
//! The game's loop is single-threaded, so not returning from Present holds all of it: its updates, the frames it sends
//! the match engine, the frame counters and the seat's link to the control. While held, the hold loop pumps the
//! window's messages (no "Not responding"), draws the PAUSED page over the game's last frame on the panel's own swap
//! chain, and waits for the pause key, the panel's RESUME or the cap ([`forza::pause::CAP`]). The game's sound session
//! is muted and the control's Stop checkbox (ID 1035) set so the pause does not count against the 980 s GAME cap; each
//! is given back only if the pause set it, and stays owed until the give-back is confirmed: retried at later Presents,
//! outside the hold, and no new pause while Stop is owed. The rules themselves are `forza::pause`'s (tested there).
//!
//! Test switch: `WCCFPANEL_PAUSETEST=MS` (fakegame) passes the gate with made-up facts (a local match, no control
//! found) and ends each pause after MS milliseconds instead of the cap.

use crate::{FILES, RENDER, log, patches};
use forza::pause::{self, Facts, Owed, Refusal, Timer};
use forza_ui::widgets::Tone;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct3D9::IDirect3DDevice9;
use windows::Win32::UI::WindowsAndMessaging::*;

/// The pause key (virtual-key code, 0 none): keys.txt's PAUSE, set by the render side when it reads the keys.
pub static KEY: AtomicU32 = AtomicU32::new(0x50);
/// The pause key or PAUSE / RESUME was pressed: the next Present (or the hold loop) acts on it.
pub static ASK: AtomicBool = AtomicBool::new(false);
/// The hold loop runs: the window procedure keeps every input from the game.
pub static HELD: AtomicBool = AtomicBool::new(false);
/// The window was closed while held: the pause ends, then the window gets its WM_CLOSE again.
static CLOSE_AFTER: AtomicBool = AtomicBool::new(false);

/// What the pause keeps between pauses: the gap's timer, what it owes the control and the sound, the Stop checkbox it
/// set (a window handle) and the muting still at work.
#[derive(Default)]
struct State {
    timer: Timer,
    owed: Owed,
    stop_box: Option<isize>,
    muting: Option<Muting>,
}

/// The game's sound sessions muted on a thread of their own while the hold runs: done before the hold, a Core Audio
/// that is slow to answer held the game unpumped and without its PAUSED page (2026-10-21, the CI runner without a
/// playback device: 2.8 s of a 1.5 s test pause gone before the first PAUSED frame, the sound query the only slow
/// step). The sessions found (id, muted before) and the ones it muted, collected by [`settle`] once it is done.
type Muting = std::thread::JoinHandle<(Vec<(String, bool)>, Vec<String>, Duration)>;

fn mute() -> Option<Muting> {
    let pid = std::process::id();
    std::thread::Builder::new()
        .spawn(move || {
            use forza::sound;
            let t0 = Instant::now();
            let found = sound::mutes(pid).unwrap_or_default();
            let clear: Vec<String> = found.iter().filter(|s| !s.1).map(|s| s.0.clone()).collect();
            let set = if clear.is_empty() {
                Vec::new()
            } else {
                sound::set_mute(pid, &clear, true).unwrap_or_default()
            };
            (found, set, t0.elapsed())
        })
        .ok()
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The state, also after a panic that held it (what it owes must outlive the panic).
fn state() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The test switch: the hold's length in ms instead of the cap.
fn test_ms() -> Option<u64> {
    std::env::var("WCCFPANEL_PAUSETEST")
        .ok()
        .and_then(|v| v.parse().ok())
}

// 2010-11 is not relocated (base 0x00400000); 2017-18 may be, so its address moves with the base.
const SHOP_1011: usize = 0x0113_700C;
const SHOP_1718: usize = 0x06D9_EB08;
const BASE: usize = 0x0040_0000;

/// The seat's copy of the shop state (`GAME-CLOCK.md`: the object at C10 `*(0x113700C)`, C17 `*(0x6D9EB08)`, its
/// dword +12; 0-7 = boot, ENTRY, CR_DATA_WAIT, MATCHING_WAIT, MATCHING, GAME, GAME_RESULT_WAIT, GAME_RESULT). `None`
/// when this is not the client, the object is not there yet or the value is no state.
pub(crate) fn shop_state(e1718: bool) -> Option<u32> {
    if !patches::is_client(&patches::exe_name()) {
        return None;
    }
    let base = patches::exe_base();
    let at = match (e1718, base) {
        (true, _) => SHOP_1718 - BASE + base,
        (false, BASE) => SHOP_1011,
        _ => return None,
    };
    let obj = patches::read_u32(at).filter(|&o| o != 0)?;
    patches::read_u32(obj as usize + 12).filter(|&s| s <= 7)
}

/// The kit's control on this PC (`control_Release.exe` from this game's folders): its process and its Stop checkbox
/// (ID 1035 on its `#32770` main dialog, hidden by the run); either may be missing.
fn control() -> (Option<u32>, Option<HWND>) {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
    else {
        return (None, None);
    };
    let Some(pid) = forza::procs::game_processes(&dir)
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case("control_Release.exe"))
        .map(|p| p.pid)
    else {
        return (None, None);
    };
    let stop = forza::procs::windows(pid)
        .into_iter()
        .filter(|w| w.class == "#32770")
        .find_map(|w| {
            // SAFETY: plain window queries on a window Windows just listed
            unsafe {
                let b = GetDlgItem(Some(HWND(w.hwnd)), 1035).ok()?;
                let mut class = [0u16; 32];
                let n = GetClassNameW(b, &mut class).max(0) as usize;
                String::from_utf16_lossy(&class[..n])
                    .eq_ignore_ascii_case("Button")
                    .then_some(b)
            }
        });
    (Some(pid), stop)
}

/// A message to the control's checkbox, never waiting more than half a second (the control may be busy or hung).
fn ask(b: HWND, msg: u32, wp: usize) -> Option<usize> {
    let mut out = 0usize;
    // SAFETY: BM_GETCHECK / BM_SETCHECK carry no pointer, so they cross processes as they are
    let r = unsafe {
        SendMessageTimeoutW(
            b,
            msg,
            WPARAM(wp),
            LPARAM(0),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            500,
            Some(&mut out),
        )
    };
    (r.0 != 0).then_some(out)
}

const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;

/// The newest write of the club card or the 2017-18 game server's store (`data\save`), as an age.
fn card_age() -> Option<Duration> {
    let save = FILES.get()?.data.join("save");
    [
        save.join(forza::card::SLOT),
        save.join(forza::card::NEW),
        save.join("gamesvr").join("gamesvr.db"),
        save.join("gamesvr").join("gamesvr.db-wal"),
    ]
    .iter()
    .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
    .map(|t| SystemTime::now().duration_since(t).unwrap_or_default())
    .min()
}

/// `f` on a thread of its own, so COM never changes on the game's thread; `None` if the thread could not start or
/// panicked.
fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    std::thread::Builder::new().spawn(f).ok()?.join().ok()
}

/// These sessions muted or unmuted; the ones that were set.
fn set_mute(ids: Vec<String>, on: bool) -> Vec<String> {
    let pid = std::process::id();
    on_thread(move || forza::sound::set_mute(pid, &ids, on))
        .and_then(Result::ok)
        .unwrap_or_default()
}

/// What an ended pause still owes, tried now if a try is due: Stop cleared and confirmed clear (or its control gone),
/// the sessions it muted unmuted (owed once its muting is done). On the game's thread, never inside the hold.
fn settle(st: &mut State) {
    if st.muting.as_ref().is_some_and(Muting::is_finished)
        && let Some(m) = st.muting.take()
    {
        let (found, set, took) = m.join().unwrap_or_default();
        let take = st.owed.take_mute(&found);
        let sound = match (found.len(), take.len()) {
            (0, _) => "none open".to_owned(),
            (_, 0) => "already muted - left alone".to_owned(),
            (all, n) => format!(
                "{} of {n} muted ({} of {all} were the player's)",
                set.len(),
                all - n
            ),
        };
        log(&format!(
            "pause: sound {sound} (in {} ms)",
            took.as_millis()
        ));
    }
    let Some(n) = st.owed.due(Instant::now()) else {
        return;
    };
    if st.owed.stop_owed() {
        let b = st.stop_box.map(|b| HWND(b as *mut c_void));
        // SAFETY: a plain query on a window handle, which may be stale
        let cleared = match b.filter(|&b| unsafe { IsWindow(Some(b)) }.as_bool()) {
            Some(b) => {
                let _ = ask(b, BM_SETCHECK, 0);
                ask(b, BM_GETCHECK, 0) == Some(0)
            }
            None => true, // the control's window is gone, its Stop with it
        };
        if cleared {
            st.owed.stop_cleared();
        }
    }
    let muted = st.owed.muted();
    if !muted.is_empty() {
        st.owed.unmuted(&set_mute(muted, false));
    }
    let settled = st.owed.settled();
    if settled && n > 1 {
        log(&format!("pause: given back at try {n}"));
    } else if !settled && (n == 1 || n == pause::RETRIES) {
        log(&format!(
            "pause: not given back yet (try {n} of {}): Stop {}, {} sound session(s) still muted{}",
            pause::RETRIES,
            if st.owed.stop_owed() {
                "maybe still set"
            } else {
                "clear"
            },
            st.owed.muted().len(),
            match (n == pause::RETRIES, st.owed.stop_owed()) {
                (false, _) => "",
                (true, false) => " - given up",
                (true, true) => " - given up, no more pauses",
            }
        ));
    }
}

/// While the hold runs: on every way out, unwinding included, the game gets its input back and the PAUSED page goes.
struct Holding;

impl Drop for Holding {
    fn drop(&mut self) {
        HELD.store(false, Relaxed);
        if let Ok(mut r) = RENDER.lock()
            && let Some(r) = r.as_mut()
        {
            r.st.paused = None;
        }
    }
}

/// The pause key or button, acted on at the game's Present (the game's thread, before its frame is shown): a pause
/// asked for is checked, then held here until it ends.
pub fn present(dev: &IDirect3DDevice9, wnd: HWND) {
    {
        let mut st = state();
        settle(st.get_or_insert_with(State::default));
    }
    if !ASK.swap(false, Relaxed) {
        return;
    }
    let test = test_ms();
    let e1718 = FILES
        .get()
        .is_some_and(|f| f.edition == forza_ui::theme::Edition::E1718);
    let (pid, stop_box) = if test.is_some() {
        (None, None)
    } else {
        control()
    };
    let stop_found = stop_box
        .and_then(|b| ask(b, BM_GETCHECK, 0))
        .map(|v| v == 1);
    let shop = if test.is_some() {
        Some(pause::GAME)
    } else {
        shop_state(e1718)
    };
    let mut st = state();
    let st = st.get_or_insert_with(State::default);
    // the pause's start, for the cap and the test switch alike: what follows before the hold (the Stop checkbox)
    // holds the game too
    let begun = Instant::now();
    let refused = {
        let Ok(mut r) = RENDER.lock() else { return };
        let Some(r) = r.as_mut() else { return };
        let facts = Facts {
            shop_state: shop,
            online: std::env::var_os("WCCF_RELAY").is_some_and(|v| !v.is_empty()),
            seats: if test.is_some() {
                Some(1)
            } else {
                pid.and_then(crate::kit::seats_on)
            },
            projector: false, // the seat count already includes a projector; the frame section that told is gone
            card_age: if test.is_some() { None } else { card_age() },
            own_picture: r.sc.is_some(),
            stop: stop_found,
            stop_owed: st.owed.stop_owed(),
        };
        let refused = pause::allowed(&facts)
            .err()
            .or_else(|| st.timer.start(begun).err().map(Refusal::TooSoon));
        match refused {
            Some(Refusal::TooSoon(s)) => {
                let text =
                    forza_ui::text(r.st.lang, "pause.msg_too_soon", &[("s", &s.to_string())]);
                r.st.note(Tone::Refused, text);
            }
            Some(why) => r.st.say(Tone::Refused, why.key()),
            None => {}
        }
        refused.map(|why| (why, facts))
    };
    if let Some((why, facts)) = refused {
        log(&format!("pause: refused - {why:?} ({facts:?})"));
        return;
    }
    // what the pause borrows: Stop (only now, in state 5, if it was clear) and the sound sessions not muted; owed
    // from before the set, as a set that timed out may still happen
    let stop = match stop_box {
        Some(b) if st.owed.take_stop(shop, stop_found) => {
            st.stop_box = Some(b.0 as isize);
            let _ = ask(b, BM_SETCHECK, 1);
            if ask(b, BM_GETCHECK, 0) == Some(1) {
                "set"
            } else {
                "NOT confirmed set (no answer) - the 20 s cap alone"
            }
        }
        Some(_) => "not read (no answer) - the 20 s cap alone",
        None if pid.is_some() => "not found - the 20 s cap alone",
        None => "no control",
    };
    // a muting still at work from the last pause (at least the 10 s gap ago) is left to finish and be collected
    let sound = if st.muting.is_none() {
        st.muting = mute();
        if st.muting.is_some() {
            "being muted"
        } else {
            "not muted (no thread)"
        }
    } else {
        "still being muted from the last pause"
    };
    log(&format!(
        "pause: on (shop state {shop:?}; Stop {stop}; sound {sound}; {} ms to set up)",
        begun.elapsed().as_millis()
    ));
    let holding = Holding;
    HELD.store(true, Relaxed);
    let (why, frames) = hold(
        dev,
        wnd,
        &st.timer,
        test.map(|ms| begun + Duration::from_millis(ms)),
    );
    drop(holding);
    let held = st.timer.end(Instant::now());
    settle(st);
    if why == "cap"
        && let Ok(mut r) = RENDER.lock()
        && let Some(r) = r.as_mut()
    {
        r.st.say(Tone::Notice, "pause.msg_resumed_cap");
    }
    log(&format!(
        "pause: off ({why}) after {} ms, {frames} frame(s) of the PAUSED page",
        held.as_millis()
    ));
    if CLOSE_AFTER.swap(false, Relaxed) {
        // SAFETY: a message to the game's own window, handled once the game runs again
        let _ = unsafe { PostMessageW(Some(wnd_of(wnd)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
}

/// The game's window: Present's, else the subclassed one.
fn wnd_of(wnd: HWND) -> HWND {
    if wnd.is_invalid() {
        HWND(crate::WINDOW.load(Relaxed) as *mut c_void)
    } else {
        wnd
    }
}

/// The hold loop: messages pumped, the PAUSED page drawn, until the key or RESUME, the cap, a close, or a frame that
/// could not be shown (`test`: the test switch's end instead of the cap). Why it ended, and how many frames it drew.
fn hold(
    dev: &IDirect3DDevice9,
    wnd: HWND,
    timer: &Timer,
    test: Option<Instant>,
) -> (&'static str, u32) {
    let mut frames = 0;
    loop {
        let t0 = Instant::now();
        let mut m = MSG::default();
        // a bounded batch per frame, so a queue that keeps filling never starves the cap's check below
        for _ in 0..64 {
            // SAFETY: the game's thread's own message queue, pumped as the game's loop does
            if !unsafe { PeekMessageW(&mut m, None, 0, 0, PM_REMOVE) }.as_bool() {
                break;
            }
            if m.message == WM_QUIT {
                // SAFETY: the quit message put back for the game's own loop
                unsafe { PostQuitMessage(m.wParam.0 as i32) };
                return ("quit", frames);
            }
            // SAFETY: as above
            unsafe {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
        if ASK.swap(false, Relaxed) {
            return ("asked", frames);
        }
        if CLOSE_AFTER.load(Relaxed) {
            return ("window closed", frames);
        }
        let now = Instant::now();
        let left = timer.left(now).unwrap_or_default();
        let over = match test {
            Some(end) => now >= end,
            None => left.is_zero(),
        };
        if over {
            return ("cap", frames);
        }
        let shown = std::panic::catch_unwind(|| {
            let mut r = RENDER.lock().ok()?;
            let r = r.as_mut()?;
            r.st.paused = Some(left.as_millis().div_ceil(1000) as u64);
            // SAFETY: the game's device on its own thread, inside its Present
            unsafe { r.frame(dev, wnd) }
        })
        .unwrap_or_else(|_| {
            crate::DEAD.store(true, Relaxed);
            log("pause: a frame panicked - the pause ends, the panel stops drawing");
            None
        });
        match shown {
            Some(hr) if hr.is_ok() => frames += 1,
            Some(hr) => {
                log(&format!("pause: our Present failed ({hr:?})"));
                return ("lost device", frames);
            }
            None => return ("no frame", frames),
        }
        // the game's own interval paces our Present; an immediate one still leaves the CPU alone
        if let Some(rest) = Duration::from_millis(16).checked_sub(t0.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

/// The window procedure's part: while held every input is egui's or ours, never the game's (`None`: not input, the
/// game's procedure gets it), and nothing enters Windows' own modal loops (moving or sizing the window, its system
/// menu), which would hold the hold past its cap; otherwise the pause key asks for a pause unless egui types or a
/// page is open.
pub fn message(m: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let key = KEY.load(Relaxed);
    let is_key = key != 0 && wp.0 as u32 == key;
    if !HELD.load(Relaxed) {
        if is_key
            && matches!(m, WM_KEYDOWN | WM_KEYUP)
            && !crate::WANTS_KEYS.load(Relaxed)
            && !crate::PAGE_OPEN.load(Relaxed)
        {
            if m == WM_KEYDOWN && lp.0 & (1 << 30) == 0 {
                ASK.store(true, Relaxed);
            }
            return Some(LRESULT(0));
        }
        return None;
    }
    match m {
        WM_CLOSE => {
            CLOSE_AFTER.store(true, Relaxed);
            Some(LRESULT(0))
        }
        // the window's X: the close path above, without Windows tracking the button in its own loop
        WM_NCLBUTTONDOWN if wp.0 == HTCLOSE as usize => {
            CLOSE_AFTER.store(true, Relaxed);
            Some(LRESULT(0))
        }
        WM_KEYDOWN | WM_KEYUP if is_key => {
            if m == WM_KEYDOWN && lp.0 & (1 << 30) == 0 {
                ASK.store(true, Relaxed);
            }
            Some(LRESULT(0))
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR
        | WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN
        | WM_RBUTTONUP | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN | WM_MBUTTONUP | WM_MBUTTONDBLCLK
        | WM_MOUSEWHEEL => {
            // egui's (RESUME, the pages); F11 refused: no resize inside the hold
            // popped out, the game window's mouse is not the board's
            let game_mouse =
                crate::POPPED.load(Relaxed) && (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&m);
            if !(matches!(m, WM_KEYDOWN | WM_KEYUP) && wp.0 == 0x7A) && !game_mouse {
                crate::input(m, wp, lp);
            }
            Some(LRESULT(0))
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_NCRBUTTONDOWN | WM_NCRBUTTONUP
        | WM_NCRBUTTONDBLCLK | WM_CONTEXTMENU => Some(LRESULT(0)),
        WM_SYSCOMMAND if (wp.0 & 0xFFF0) as u32 != SC_CLOSE => Some(LRESULT(0)),
        _ => None,
    }
}
