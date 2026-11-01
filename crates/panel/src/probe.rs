//! `WCCF_PROBE_GAMEREADY=1`, 2017-18 projector only, default off: log lines (`gameready:` in
//! `forza_panel_projector.log`) that show why the projector's GAME_READY does not reach the control before a match.
//!
//! The original path: the projector's sequence (`sub_439200`, change by name, or 0 = next) reaches "Prj Before Game
//! Title Movie" (vftable 0x01405EAC). Its update (`sub_88A230`, vftable slot at 0x01405F08) runs the title movie
//! (Sofdec2 player `[0x06DCDABC]`) only while two flags and the control link hold; once the scheduler (`[0x06D9F28C]`)
//! has the control in state 4-6 and the movie is over (Sofdec state 0 or 6, or `+0xD4` > 3600 frames), it loads the
//! match (`sub_88A5A0`, step `[0x06D9F2EC]` 0..8), plays a second movie (`[0x06DCD848]`, substate `+0xE0` = 3) and at
//! its end sends GAME_READY (`sub_4B75F0`), sets scheduler `+0x47` and goes to the next sequence. The control reads it
//! only in GAME (state 5, `sub_431F10`), else it sends its own after 100 s.
//!
//! Two hooks, both checked against the original first: a five-byte jump on `sub_439200` (prologue `push ebp / mov
//! ebp,esp / and esp,-8` run from a trampoline) logs every sequence change; the Title Movie's vftable slot points to a
//! wrapper that logs whenever one of the values above changes (and every 5 s with the movie's frame count), and the
//! moment GAME_READY goes out.
//!
//! Both clients (projector and seat; each logs to its own panel log): a thread reads, every 200 ms and without calling
//! game code, the one Sofdec2 handle every movie uses (`[[0x06DCDC14]+4]`, state worked out as `sub_DC4980` /
//! `sub_DD6590` do), the movie's file name, the seat attract's step `[0x01579FD0]` (6 starts `opening.usm`, 7 plays it
//! and leaves after 3600 frames `[0x06D9F1DC]`) and CRI's error notifier (`sub_F783E2`: level 0 counted at
//! `0x068328B0`, level 1 at `0x068328B4`; `sub_F784E8` writes its message to `0x068327A0`). A `sofdec:` line goes out
//! when one of them changes, every 5 s while a movie is between DECHDR and READY, and, while it sits in PREP, once a
//! second with the words of the player's inner object that moved.

use crate::patches::{BASE_1718, exe_base, patch_code, read, read_u32};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc,
};
use windows::core::PCSTR;

const SEQ: usize = 0x0043_9200;
const SEQ_PROLOGUE: [u8; 6] = [0x55, 0x8B, 0xEC, 0x83, 0xE4, 0xF8];
const TITLE_VT: usize = 0x0140_5EAC;
const TITLE_SLOT: usize = 0x0140_5F08;
const TITLE_UPDATE: usize = 0x0088_A230;
const SCHED: usize = 0x06D9_F28C;
const GATE_OBJ: usize = 0x06D9_F288; // byte +0xD1
const GATE_BYTE: usize = 0x0683_6541;
const LINK: usize = 0x06D9_EE34;
const PLAYER: usize = 0x06DC_DABC;
const SOFDEC: usize = 0x06DC_DC14;
const SOFDEC_STATE: usize = 0x00DC_4980;
const LOAD_STEP: usize = 0x06D9_F2EC;
const MOVIE2: usize = 0x06DC_D848;
const CRI_MSG: usize = 0x0683_27A0;
const CRI_ERRORS: usize = 0x0683_28B0;
const CRI_WARNINGS: usize = 0x0683_28B4;
const CRI_CALLBACK: usize = 0x0683_28A4;
const ATTRACT_STEP: usize = 0x0157_9FD0;
const ATTRACT_FRAMES: usize = 0x06D9_F1DC;
/// Words of the player's inner object (`[handle+0x30]`) compared while in PREP; its last known field is +0x770.
const INNER_WORDS: usize = 0x780 / 4;

static LOG: Mutex<Option<fn(&str)>> = Mutex::new(None);
static SEQ_TRAMP: AtomicUsize = AtomicUsize::new(0);
static TITLE_ORIG: AtomicUsize = AtomicUsize::new(0);
/// Inside the Title Movie's update: its substate + 1 (0 = not inside).
static IN_TITLE: AtomicUsize = AtomicUsize::new(0);
static LAST: Mutex<(Option<[u32; 12]>, Option<Instant>)> = Mutex::new((None, None));

fn at(va: usize) -> usize {
    va - BASE_1718 + exe_base()
}

fn say(msg: &str) {
    let Some(log) = *LOG.lock().unwrap_or_else(|e| e.into_inner()) else {
        return;
    };
    log(&format!("gameready: {msg}")); // the panel log stamps the time
}

fn u32_at(p: usize) -> u32 {
    if p == 0 {
        0
    } else {
        read_u32(p).unwrap_or(0xDEAD_DEAD)
    }
}
fn u8_at(p: usize) -> u32 {
    if p == 0 {
        0
    } else {
        read::<1>(p).map_or(0xDD, |b| u32::from(b[0]))
    }
}

/// The values the Title Movie's update decides on, as one row (`this` = the scene, 0 outside it).
fn row(this: usize) -> [u32; 12] {
    let s = u32_at(at(SCHED)) as usize;
    let link = u32_at(u32_at(at(LINK)) as usize) as usize;
    let player = u32_at(at(PLAYER)) as usize;
    let sofdec = u32_at(u32_at(at(SOFDEC)) as usize + 4);
    let sofdec_state = if sofdec != 0 && this != 0 {
        // SAFETY: the game's own status query (cdecl, one handle), on the game's thread inside its scene update, as
        // sub_A0AD20 calls it every frame
        let f: extern "C" fn(u32) -> i32 = unsafe { std::mem::transmute(at(SOFDEC_STATE)) };
        f(sofdec) as u32
    } else {
        99
    };
    [
        u32_at(s + 0x2C),                                 // 0 control state (scheduler)
        u8_at(u32_at(s) as usize),                        // 1 control state byte (the gate's)
        u8_at(s + 0x47),                                  // 2 set right after GAME_READY
        u8_at(u32_at(at(GATE_OBJ)) as usize + 0xD1),      // 3 flag A
        u8_at(at(GATE_BYTE)),                             // 4 flag B (A or B needed)
        u32::from(u32_at(link + 4) != 0),                 // 5 control link up
        u8_at(player + 0x40),                             // 6 title player busy (0 = may end)
        sofdec_state,                                     // 7 Sofdec state (5 play, 6 end, 0 stop)
        if this == 0 { 99 } else { u32_at(this + 0xE0) }, // 8 substate 0 load, 2 setup, 3 movie 2
        u32_at(at(LOAD_STEP)),                            // 9 load step 0..8
        if this == 0 { 99 } else { u8_at(this + 0xE8) },  // 10 load done
        u32::from(u32_at(at(MOVIE2)) != 0),               // 11 movie 2 made
    ]
}

fn show(r: &[u32; 12], frames: u32) -> String {
    format!(
        "control state {} (byte {}), +47 {}, flags {}/{}, link {}, title busy {} sofdec {} frames {}, substate {}, \
         load step {} done {}, movie2 {}",
        r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7], frames, r[8], r[9], r[10], r[11]
    )
}

unsafe extern "stdcall" fn seq_hook(scene: usize, name: *const u8) -> i32 {
    let vt = u32_at(scene) as usize;
    let to = if name.is_null() {
        "next".to_string()
    } else {
        let b = read::<48>(name as usize).unwrap_or([0; 48]);
        let n = b.iter().position(|&c| c == 0).unwrap_or(48);
        format!("\"{}\"", String::from_utf8_lossy(&b[..n]))
    };
    let from = if vt == at(TITLE_VT) {
        "Before Game Title Movie".to_string()
    } else {
        format!("vftable {:08X}", vt.wrapping_sub(exe_base()) + BASE_1718)
    };
    let r = row(0);
    if IN_TITLE.load(Relaxed) == 4 {
        // the only sequence change in its update is the one right after sub_4B75F0 (0x0088A3A6)
        say(&format!(
            "GAME_READY sent (movie 2 over) with the control in state {} (only state 5 = GAME reads it)",
            r[0]
        ));
    }
    if vt == at(TITLE_VT) {
        LAST.lock().unwrap_or_else(|e| e.into_inner()).0 = None;
    }
    say(&format!("sequence {from} -> {to}; {}", show(&r, 0)));
    // SAFETY: the trampoline runs sub_439200's first six bytes, then the rest of it, with the same arguments
    let orig: unsafe extern "stdcall" fn(usize, *const u8) -> i32 =
        unsafe { std::mem::transmute(SEQ_TRAMP.load(Relaxed)) };
    // SAFETY: as above, the game's own call passed on
    unsafe { orig(scene, name) }
}

unsafe extern "thiscall" fn title_hook(this: usize) {
    let before = row(this);
    let frames = u32_at(u32_at(at(PLAYER)) as usize + 0xD4);
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let beat = last.1.is_none_or(|t| t.elapsed().as_secs() >= 5);
        if last.0 != Some(before) || beat {
            if last.0.is_none() {
                say("Before Game Title Movie: first update");
            }
            say(&show(&before, frames));
            *last = (Some(before), Some(Instant::now()));
        }
    }
    // SAFETY: the original update, same object
    let orig: unsafe extern "thiscall" fn(usize) =
        unsafe { std::mem::transmute(TITLE_ORIG.load(Relaxed)) };
    IN_TITLE.store(before[8] as usize + 1, Relaxed);
    // SAFETY: as above
    unsafe { orig(this) };
    IN_TITLE.store(0, Relaxed);
}

/// `sub_DC4980`'s answer from memory: `h84` = handle +0x84, `err` = inner +0x770, `raw` = the core's status
/// (`[[inner+4]+4]`, `sub_DD6590`), `held` = inner +0x450. 0 stop, 1 DECHDR, 2 WAIT_PREP, 3 PREP, 4 READY, 5 PLAYING,
/// 6 PLAYEND, 7 ERROR.
fn sofdec_state(h84: u32, err: u32, raw: u32, held: u32) -> u32 {
    if h84 == 1 || err == 1 {
        return 7;
    }
    match raw {
        1..=4 => raw,
        5 | 6 => 5,
        7 => 6,
        8..=10 => held,
        _ => 0,
    }
}

/// A C string of at most `N` bytes at `p`.
fn cstr<const N: usize>(p: usize) -> String {
    let b = if p == 0 {
        [0; N]
    } else {
        read::<N>(p).unwrap_or([0; N])
    };
    let n = b.iter().position(|&c| c == 0).unwrap_or(N);
    String::from_utf8_lossy(&b[..n]).into_owned()
}

/// The 200 ms reader (see the module comment); runs for the process's life.
fn sampler() {
    let mut last: Option<(u32, u32, u32, u32, u32, String)> = None;
    let mut beat = Instant::now();
    let mut inner_last: Option<(usize, Vec<u32>)> = None;
    let mut inner_t = Instant::now();
    let mut audio_said = false;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let h = u32_at(u32_at(at(SOFDEC)) as usize + 4) as usize;
        let inner = if h == 0 { 0 } else { u32_at(h + 0x30) as usize };
        let raw = if inner == 0 {
            0
        } else {
            u32_at(u32_at(inner + 4) as usize + 4)
        };
        let state = if h == 0 {
            99
        } else {
            sofdec_state(
                u32_at(h + 0x84),
                u32_at(inner + 0x770),
                raw,
                u32_at(inner + 0x450),
            )
        };
        let player = u32_at(at(PLAYER)) as usize;
        // the movie's name: std::string at player +0x24, text at +0x28 (inline below 16 bytes of capacity +0x3C)
        let file = if player == 0 {
            String::new()
        } else if u32_at(player + 0x3C) < 16 {
            cstr::<16>(player + 0x28)
        } else {
            cstr::<96>(u32_at(player + 0x28) as usize)
        };
        let step = u32_at(at(ATTRACT_STEP));
        let (errs, warns) = (u32_at(at(CRI_ERRORS)), u32_at(at(CRI_WARNINGS)));
        let now = (state, raw, step, errs, warns, file);
        let changed = last.as_ref() != Some(&now);
        let stuck = (1..=4).contains(&state) && beat.elapsed().as_secs() >= 5;
        if changed || stuck {
            let cri = if last.as_ref().is_none_or(|l| (l.3, l.4) != (errs, warns)) {
                format!(
                    ", CRI last message \"{}\" (callback {:08X})",
                    cstr::<256>(at(CRI_MSG)),
                    u32_at(at(CRI_CALLBACK))
                )
            } else {
                String::new()
            };
            say(&format!(
                "sofdec: state {state} (core {raw}) movie \"{}\" busy {} frames {}; seat attract step {step} frames \
                 {}; CRI errors {errs} warnings {warns}{cri}",
                now.5,
                u8_at(player + 0x40),
                if player == 0 {
                    0
                } else {
                    u32_at(player + 0xD4)
                },
                u32_at(at(ATTRACT_FRAMES)),
            ));
            beat = Instant::now();
            last = Some(now);
        }
        if state != 0 && state != 99 && !audio_said {
            audio_said = true;
            let loaded: Vec<String> = [
                &b"xaudio2_7.dll\0"[..],
                b"xaudio2_8.dll\0",
                b"xaudio2_9.dll\0",
                b"dsound.dll\0",
            ]
            .into_iter()
            // SAFETY: a module-list lookup on a NUL-terminated literal
            .filter(|m| unsafe { GetModuleHandleA(PCSTR(m.as_ptr())) }.is_ok())
            .map(|m| String::from_utf8_lossy(&m[..m.len() - 1]).into_owned())
            .collect();
            say(&format!(
                "sofdec: first movie; audio modules loaded: {loaded:?}"
            ));
        }
        if state == 3 && inner != 0 && inner_t.elapsed().as_secs() >= 1 {
            inner_t = Instant::now();
            let words: Vec<u32> = (0..INNER_WORDS).map(|i| u32_at(inner + 4 * i)).collect();
            if let Some((at0, old)) = inner_last.as_ref().filter(|(a, _)| *a == inner) {
                let moved: Vec<String> = (0..INNER_WORDS)
                    .filter(|&i| old[i] != words[i])
                    .take(16)
                    .map(|i| format!("+{:X}={:X}", 4 * i, words[i]))
                    .collect();
                say(&format!(
                    "sofdec: PREP, inner {:08X} words moved in 1 s: {}",
                    at0,
                    if moved.is_empty() {
                        "none".to_string()
                    } else {
                        moved.join(" ")
                    }
                ));
            }
            inner_last = Some((inner, words));
        }
    }
}

/// Both hooks, each only on the original bytes, and the Sofdec reader. Called from `limiter_start` in both 2017-18
/// clients (the Title Movie scene only runs in the projector).
pub fn install(log: fn(&str)) {
    if !std::env::var("WCCF_PROBE_GAMEREADY").is_ok_and(|v| v == "1") {
        return;
    }
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(log);
    std::thread::spawn(sampler);
    log("gameready probe: Sofdec / CRI error / seat attract reader every 200 ms");
    let seq = at(SEQ);
    if read::<6>(seq) != Some(SEQ_PROLOGUE) {
        log("gameready probe: sub_439200 is not the code studied - no sequence log");
    } else {
        // SAFETY: our own executable page for the trampoline
        let tramp =
            unsafe { VirtualAlloc(None, 16, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) }
                as usize;
        if tramp != 0 {
            let back = jmp(tramp + 6, seq + 6);
            // SAFETY: 11 bytes into the page we just made
            unsafe {
                std::ptr::copy_nonoverlapping(SEQ_PROLOGUE.as_ptr(), tramp as *mut u8, 6);
                std::ptr::copy_nonoverlapping(back.as_ptr(), (tramp + 6) as *mut u8, 5);
            }
            SEQ_TRAMP.store(tramp, Relaxed);
            let mut j = [0x90u8; 6];
            j[..5].copy_from_slice(&jmp(seq, seq_hook as *const () as usize));
            if patch_code(seq, &j) {
                log("gameready probe: sequence changes logged (sub_439200)");
            }
        }
    }
    let slot = at(TITLE_SLOT);
    let orig = at(TITLE_UPDATE);
    if read_u32(slot) != Some(orig as u32) || read_u32(at(TITLE_VT)) != Some(at(0x0088_9CF0) as u32)
    {
        log("gameready probe: the Title Movie vftable is not the one studied - no scene log");
    } else {
        TITLE_ORIG.store(orig, Relaxed);
        if patch_code(slot, &(title_hook as *const () as u32).to_le_bytes()) {
            log("gameready probe: Before Game Title Movie update logged (sub_88A230)");
        }
    }
}

/// `jmp rel32` placed at `at`, to `to`.
fn jmp(at: usize, to: usize) -> [u8; 5] {
    let [a, b, c, d] = (to.wrapping_sub(at + 5) as u32).to_le_bytes();
    [0xE9, a, b, c, d]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sofdec_state_as_the_game_reads_it() {
        assert_eq!(sofdec_state(0, 0, 3, 0), 3); // PREP
        assert_eq!(sofdec_state(0, 0, 6, 0), 5); // two core states read as PLAYING
        assert_eq!(sofdec_state(0, 0, 7, 0), 6); // PLAYEND
        assert_eq!(sofdec_state(0, 0, 9, 4), 4); // stopping: the state it held
        assert_eq!(sofdec_state(0, 0, 11, 0), 0);
        assert_eq!(sofdec_state(1, 0, 5, 0), 7); // either error flag: ERROR
        assert_eq!(sofdec_state(0, 1, 5, 0), 7);
        assert_eq!(cstr::<8>(0), "");
    }
}
