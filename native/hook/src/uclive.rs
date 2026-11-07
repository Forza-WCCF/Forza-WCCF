//! The Universal Championship on the 2017-18 projector only with a player's club in it (2026-11-02). Control's
//! GameManage sets "UC live" (+0x14938) during a FriendlyMatch slot (competition 17) when it has data-match replays
//! (0x45DF7B) or a valid Universal Championship (0x45DFAC), and the projector then plays it live (control 0x433D00).
//! The championship is the day's 4 Universal Cup winners; empty places get random local CPU teams (0x47DCF0), so a day
//! without a player's winner (yesterday's fill at start, the cup Off, nights before 05:00) is a CPU-only bracket the
//! projector replays forever. Its results are processed only while UC live is 1 (0x45E840), one match per slot.
//!
//! The hook: the replay store writes 0 (cmd 261 carries no replays), and the championship store becomes a call to
//! [`live_flag`], which writes 1 only when the current day's semifinals hold a player's team (team block +4 == 0; a CPU
//! team carries its CPU id there, 0x47DB20/0x45E840). A real bracket then shows, advances and sends its results (cmd
//! 301/257); an all-CPU one never goes live. Every other store to the flag writes 0 (constructor 0x45CAB4, each state
//! entry 0x45DE47). Off unless WCCF_UCLIVE=on ([`opted_in`]), which the Launcher sets when the CPU championship setting
//! is off; else the projector loops the CPU championship as before. With nothing live the control sends the projector
//! its own demo pair (0x4394E0), which the panel's "projector league" patch lets show (crates/panel/src/patches.rs).
//!
//! The slot clock (2026-11-07): a FriendlyMatch slot's GAME state ends 14 s after control 0x461CD0 sees its match
//! finished, the projector's live championship match while UC live is 1, else a seat's match; with neither it waits
//! for the 980-s timeout (CoStateManage 0x456027), so with UC live 0 an empty slot lasted about 19 min instead of
//! about 8 and a Launcher cup "In 8 min" opened 20 min after the start (next-0.7.1 UC-TRIAGE.md). The second patch
//! replaces that call (0x456064) with [`slot_site`]: with UC live 0 the match counts as finished once the GAME state
//! is [`GAME_SECS`] old, as long as the live championship match takes, so slots keep the original's length.

/// The two `mov byte [ebp+14938h], 1` in GameManage's state entry (control sub_45D840), as offsets from the program's
/// base (ASLR moves it): the replay store, patched to store 0, and the championship store, replaced by the call. Both
/// lie behind `cmp ecx, 11h` at 0x45DF54: only a FriendlyMatch slot reaches them.
pub const REPLAY: usize = 0x5DF7B;
pub const UC: usize = 0x5DFAC;
/// The original instruction at each site; its last byte is the stored value.
pub const ORIGINAL: [u8; 7] = [0xC6, 0x85, 0x38, 0x49, 0x01, 0x00, 0x01];
/// Where in [`ORIGINAL`] the stored value sits.
pub const IMM: usize = 6;
/// The bytes the hook writes, from the replay site to the end of the championship site (one page).
pub const LEN: usize = UC + ORIGINAL.len() - REPLAY;

/// CoStateManage's GAME-state test (control sub_455AD0 case 5): `call sub_461CD0` at 0x456064, whose answer "the
/// slot's match is over" ends the state 14 s later.
pub const SLOT: usize = 0x5_6064;
/// The original `call sub_461CD0` (rel32 to 0x461CD0).
pub const SLOT_ORIGINAL: [u8; 5] = [0xE8, 0x67, 0xBC, 0x00, 0x00];
/// sub_461CD0, from the program's base.
pub const FINISHED: usize = 0x6_1CD0;
/// How long the GAME state of a slot without a live match lasts, seconds: the original's live championship match
/// ends 318-334 s into it (control logs of 2026-11-01 and 11-04: slots of 489-505 s = 157 s around GAME + 14 s +
/// this), so slots stay near the Launcher's 490 s (crates/forza-ui/src/lobby.rs CUP_SLOT_SECS).
pub const GAME_SECS: u32 = 325;

/// The day file's and GameManage's championship layout (control 0x47D0D0, 0x47CFC0): 3 match records of 7,636 bytes,
/// each with team blocks of 3,800 bytes from +36; a block's +4 is its CPU id (0 = a player's team).
pub const RECORD: usize = 7_636;
const TEAM_ID: usize = 40;
const TEAM: usize = 3_800;

/// The semifinals' team places (records 0 and 1, `records` from the first) that hold a player's team: (semifinal 0 or
/// 1, place 0 or 1, the block's coach id at +0). Only called on a valid bracket (all 4 places filled, 0x47CFC0), so an
/// empty place never counts.
pub fn players(records: &[u8]) -> Vec<(usize, usize, u32)> {
    let u32_at = |at: usize| {
        records
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap_or_default()))
    };
    let mut out = vec![];
    for m in 0..2 {
        for t in 0..2 {
            let at = m * RECORD + TEAM_ID + t * TEAM;
            if u32_at(at) == Some(0) {
                out.push((m, t, u32_at(at - 4).unwrap_or(0)));
            }
        }
    }
    out
}

/// Whether the semifinals hold a player's team ([`players`]).
pub fn human_in(records: &[u8]) -> bool {
    !players(records).is_empty()
}

/// [`live_flag`]'s line: the championship's match `idx` (0, 1 semifinals, 2 the final) of `day` (0 today, 1 the next
/// day's) and its decision.
pub fn verdict(records: &[u8], day: u32, idx: u32) -> String {
    let p = players(records);
    let head = format!("UC live: championship match {}/3 (day {day})", idx + 1);
    if p.is_empty() {
        return format!("{head}: all-CPU - kept off the projector");
    }
    let places: Vec<String> = p
        .iter()
        .map(|(m, t, coach)| format!("semifinal {} place {} (coach {coach})", m + 1, t + 1))
        .collect();
    format!(
        "{head}: a player's club in {} - live on the projector",
        places.join(", ")
    )
}

/// The slot line ([`slot_over`], once per GAME state): `uc_live` the flag, `cup` a Universal Cup round, `checked`
/// whether [`live_flag`] looked at a bracket since the last slot line (else the control had no valid one, or played
/// replays).
pub fn slot_line(uc_live: bool, cup: bool, checked: bool) -> &'static str {
    match (cup, uc_live, checked) {
        (true, ..) => "UC live: slot GAME state: a Universal Cup round",
        (_, true, _) => {
            "UC live: slot GAME state: friendly slot, the Universal Championship plays live on the projector"
        }
        (_, _, true) => {
            "UC live: slot GAME state: friendly slot, the championship is all-CPU - not shown"
        }
        _ => {
            "UC live: slot GAME state: friendly slot, no championship this slot (no valid bracket today, or none held yet)"
        }
    }
}

/// Whether WCCF_UCLIVE asks for the patch: only `on` (`1` kept the original before, so it still does). Unset, the
/// original: the CPU championship setting on. With UC live 0 the projector gets the control's demo pair, whose league
/// scene read NULL (a black projector, 2026-11-02) until the panel's "projector league" patch.
pub fn opted_in(var: Option<&str>) -> bool {
    var == Some("on")
}

/// Checks both sites in `span` (the program's [`LEN`] bytes from [`REPLAY`], at address `span_at`) and patches them:
/// the replay store's value to 0, the championship store to `call thunk; nop; nop`. Err: the first site that is not
/// [`ORIGINAL`] (another build, or patched already); then nothing is written.
pub fn apply(span: &mut [u8], span_at: usize, thunk: usize) -> Result<(), usize> {
    for off in [REPLAY, UC] {
        if span.get(off - REPLAY..off - REPLAY + 7) != Some(&ORIGINAL) {
            return Err(off);
        }
    }
    span[IMM] = 0;
    let at = UC - REPLAY;
    let rel = thunk.wrapping_sub(span_at + at + 5) as u32;
    span[at] = 0xE8;
    span[at + 1..at + 5].copy_from_slice(&rel.to_le_bytes());
    span[at + 5..at + 7].copy_from_slice(&[0x90, 0x90]);
    Ok(())
}

/// Checks the slot site's 5 bytes in `span` (at address `at`) and makes them `call thunk`. False: not
/// [`SLOT_ORIGINAL`]; then nothing is written.
pub fn apply_slot(span: &mut [u8], at: usize, thunk: usize) -> bool {
    if span.get(..5) != Some(&SLOT_ORIGINAL) {
        return false;
    }
    span[0] = 0xE8;
    span[1..5].copy_from_slice(&(thunk.wrapping_sub(at + 5) as u32).to_le_bytes());
    true
}

// the original's 980-s timeout stays far behind; slots stay near the Launcher's 490-s estimate
const _: () =
    assert!(GAME_SECS < 980 && 157 + GAME_SECS + 14 >= 480 && 157 + GAME_SECS + 14 <= 505);

/// The GAME state's answer: the original's `finished`, or with UC live 0 and no Universal Cup round running (a cup
/// round waits for its own matches, as before) a state [`GAME_SECS`] old (`secs` since it started). A seat's friendly
/// still running then is not cut: the next state (game_result_wait, 0x456139) waits until every listed seat match has
/// its result (0x460F20 -> GameManage +0x14355 with UC live 0), as the original's UC-live slots end on the projector's
/// match alone.
pub fn game_over(finished: bool, uc_live: bool, cup: bool, secs: i32) -> bool {
    finished || (!uc_live && !cup && secs >= GAME_SECS as i32)
}

/// The championship site's call: `esi` = the championship object (GameManage +0x14934), `ebp` = GameManage. Every
/// register and flag is kept; the flag gets [`live_flag`]'s answer.
#[cfg(target_arch = "x86")]
#[unsafe(naked)]
unsafe extern "C" fn uc_site() {
    core::arch::naked_asm!(
        "pushfd",
        "pushad",
        "push esi",
        "call {f}",
        "add esp, 4",
        "mov byte ptr [ebp+0x14938], al",
        "popad",
        "popfd",
        "ret",
        f = sym live_flag,
    )
}

/// Set by [`live_flag`], taken by the next slot line.
#[cfg(target_arch = "x86")]
static CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 1 when the current day's championship has a player's team, else 0; its [`verdict`] logged when it changes (a new
/// match, day or answer).
#[cfg(target_arch = "x86")]
extern "C" fn live_flag(ucs: *const u8) -> u8 {
    use std::sync::Mutex;
    use std::sync::atomic::Ordering::Relaxed;
    static LAST: Mutex<String> = Mutex::new(String::new());
    // SAFETY: control's championship object, just checked valid by 0x47CFC0: day (+16) 0 or 1, the match (+20, 0..2,
    // as 0x45DFB3 reads it), its records' pointer at +8 + 4 * day, the semifinals' two records behind it
    let (line, live) = unsafe {
        let day = ucs.add(16).cast::<u32>().read_unaligned();
        let idx = ucs.add(20).cast::<u32>().read_unaligned();
        let records = ucs
            .add(8 + 4 * day.min(1) as usize)
            .cast::<*const u8>()
            .read_unaligned();
        let r = std::slice::from_raw_parts(records, 2 * RECORD);
        (verdict(r, day, idx.min(2)), human_in(r))
    };
    CHECKED.store(true, Relaxed);
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *last != line {
        crate::log::log!("{line}");
        *last = line;
    }
    u8::from(live)
}

/// sub_461CD0's address in the running control, set by [`install`] before the patch.
#[cfg(target_arch = "x86")]
static FINISHED_AT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The slot site's call: `eax` = GameManage (sub_461CD0's argument; +0x14904 the running cup and +0x14908 its "on",
/// as sub_461CD0 and 0x45DF25 read them), `ebp` = CoStateManage (+0x80 its seconds, +0xA4 when the GAME state began). Returns sub_461CD0's `al`, or [`game_over`]'s; `ecx` and `edx` are free there (the
/// caller reloads them, as after sub_461CD0).
#[cfg(target_arch = "x86")]
#[unsafe(naked)]
unsafe extern "C" fn slot_site() {
    core::arch::naked_asm!(
        "push eax",
        "call dword ptr [{orig}]",
        "pop ecx",
        "pushfd",
        "pushad",
        "mov edx, dword ptr [ebp+0x80]",
        "sub edx, dword ptr [ebp+0xA4]",
        "push edx",
        "xor edx, edx",
        "cmp dword ptr [ecx+0x14904], 0",
        "je 2f",
        "movzx edx, byte ptr [ecx+0x14908]",
        "2:",
        "push edx",
        "movzx edx, byte ptr [ecx+0x14938]",
        "push edx",
        "movzx eax, al",
        "push eax",
        "call {f}",
        "add esp, 16",
        "mov byte ptr [esp+28], al",
        "popad",
        "popfd",
        "ret",
        orig = sym FINISHED_AT,
        f = sym slot_over,
    )
}

/// [`game_over`] for [`slot_site`]; logs each GAME state's [`slot_line`] and the first slot it ends.
#[cfg(target_arch = "x86")]
extern "C" fn slot_over(finished: u32, uc_live: u32, cup: u32, secs: i32) -> u8 {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};
    static TOLD: AtomicBool = AtomicBool::new(false);
    static LAST_SECS: AtomicI32 = AtomicI32::new(i32::MAX);
    // a new GAME state: its seconds start again from 0
    if secs < LAST_SECS.swap(secs, Relaxed) {
        crate::log::log!(
            "{}",
            slot_line(uc_live != 0, cup != 0, CHECKED.swap(false, Relaxed))
        );
    }
    let over = game_over(finished != 0, uc_live != 0, cup != 0, secs);
    if over && finished == 0 && !TOLD.swap(true, Relaxed) {
        crate::log::log!(
            "UC live: a slot without a live match ended after {secs} s of play, as with the championship on the projector"
        );
    }
    u8::from(over)
}

/// Makes `len` bytes of control's code at `base + off` writable, lets `f` patch them (given their address) and puts
/// the protection back. None: the page could not be made writable.
#[cfg(target_arch = "x86")]
fn patch_code<R>(
    base: *const u8,
    off: usize,
    len: usize,
    f: impl FnOnce(&mut [u8], usize) -> R,
) -> Option<R> {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let p = base.wrapping_add(off).cast_mut();
    let mut prot = 0;
    // SAFETY: len bytes of control's code inside its image (the caller checked its size), made writable for the patch,
    // then put back; DllMain runs before control's own code, so nothing executes them meanwhile
    unsafe {
        if VirtualProtect(p.cast(), len, PAGE_EXECUTE_READWRITE, &mut prot) == 0 {
            return None;
        }
        let r = f(std::slice::from_raw_parts_mut(p, len), p as usize);
        if VirtualProtect(p.cast(), len, prot, &mut prot) == 0 {
            crate::log::log!(
                "UC live: control's code page at base+0x{off:X} stays writable (its protection could not be put back)"
            );
        }
        FlushInstructionCache(GetCurrentProcess(), p.cast(), len);
        Some(r)
    }
}

/// Client sub_80EDF0, which sends a Universal Cup winner's DATA_VS_ENTRY, from the program's base (ASLR moves it):
/// `cmp byte [6D9F24F],1 / mov byte [6D9F258],0 / jnz / mov eax,[6D9F288] / cmp byte [eax+0D1h],0 / mov bl,1 / jnz /
/// mov bl,[683653F] / mov al,[6D9F25A] / mov [esp+7],al`: it sends when bl (the online flag, 0x80EE07) is set and
/// [6D9F25A] is 0, and then sets [6D9F258] (0x80EE38).
pub const ENTRY: usize = 0x40_EDF0;
/// The original bytes, absolute addresses as at image base 0x400000.
pub const ENTRY_SIG: [u8; 49] = [
    0x51, 0x80, 0x3D, 0x4F, 0xF2, 0xD9, 0x06, 0x01, 0x53, 0xC6, 0x05, 0x58, 0xF2, 0xD9, 0x06, 0x00,
    0x75, 0x55, 0xA1, 0x88, 0xF2, 0xD9, 0x06, 0x80, 0xB8, 0xD1, 0x00, 0x00, 0x00, 0x00, 0xB3, 0x01,
    0x75, 0x06, 0x8A, 0x1D, 0x3F, 0x65, 0x83, 0x06, 0xA0, 0x5A, 0xF2, 0xD9, 0x06, 0x88, 0x44, 0x24,
    0x07,
];
/// Where [`ENTRY_SIG`] holds the globals' addresses: the gate [6D9F24F], sent [6D9F258], the online object's pointer
/// [6D9F288] (its +209 = 0xD1 byte), the second online flag [683653F] (HIBYTE of dword_683653C), entered [6D9F25A].
pub const ENTRY_ADDRS: [usize; 5] = [3, 11, 19, 36, 41];

/// The globals' addresses from `code` (sub_80EDF0's first bytes in the running client, its base `delta` above
/// 0x400000), or None when it is not [`ENTRY_SIG`] relocated by `delta`.
pub fn entry_globals(code: &[u8], delta: u32) -> Option<[u32; 5]> {
    let mut want = ENTRY_SIG;
    for at in ENTRY_ADDRS {
        let a = u32::from_le_bytes(want[at..at + 4].try_into().ok()?).wrapping_add(delta);
        want[at..at + 4].copy_from_slice(&a.to_le_bytes());
    }
    (code.get(..want.len())? == want).then(|| {
        ENTRY_ADDRS.map(|at| u32::from_le_bytes(want[at..at + 4].try_into().unwrap_or_default()))
    })
}

/// The client's entry line: the gate, the two online flags (`a` = [[6D9F288]+209], None when unreadable), entered and
/// sent, and what they mean for DATA_VS_ENTRY.
pub fn entry_line(gate: u8, a: Option<u8>, b: u8, entered: u8, sent: u8) -> String {
    let online = a.unwrap_or(0) != 0 || b != 0;
    let why = match (sent != 0, gate == 1, online, entered != 0) {
        (true, ..) => "DATA_VS_ENTRY sent",
        (_, false, ..) => "no entry due (gate not 1)",
        (_, _, false, _) => "entry due but NOT sent: the online flag is off",
        (_, _, _, true) => "entry due but not sent: already entered",
        _ => "entry due, not sent yet",
    };
    let a = a.map_or("?".to_owned(), |a| a.to_string());
    format!(
        "UC entry (client 0x80EDF0): gate {gate}, online [6D9F288]+209 {a} / [683653F] {b}, entered {entered}, sent {sent} - {why}"
    )
}

/// Reads `N` bytes of this process at `at`; None when they are not readable (ReadProcessMemory never faults).
#[cfg(target_arch = "x86")]
fn peek<const N: usize>(at: u32) -> Option<[u8; N]> {
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut b = [0u8; N];
    let mut got = 0;
    // SAFETY: reads into our own buffer of N bytes; the source may be anything, the call checks it
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            at as usize as *const _,
            b.as_mut_ptr().cast(),
            N,
            &mut got,
        )
    };
    (ok != 0 && got == N).then_some(b)
}

/// The client's watch on sub_80EDF0's globals (read-only, no patch): a line at the start and whenever they change.
#[cfg(target_arch = "x86")]
fn watch_entry() {
    use crate::log::log;
    let base = crate::patch::exe() as usize as u32;
    let delta = base.wrapping_sub(0x40_0000);
    let Some(g) = peek::<49>(base + ENTRY as u32).and_then(|c| entry_globals(&c, delta)) else {
        log!("UC entry: client 0x80EDF0 is not the code studied - its flags are not logged");
        return;
    };
    log!("UC entry: watching client 0x80EDF0's flags (read-only)");
    std::thread::spawn(move || {
        let mut last = String::new();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let byte = |at: u32| peek::<1>(at).map_or(0, |b| b[0]);
            let a = peek::<4>(g[2])
                .map(u32::from_le_bytes)
                .filter(|&p| p != 0)
                .and_then(|p| peek::<1>(p.wrapping_add(0xD1)))
                .map(|b| b[0]);
            let line = entry_line(byte(g[0]), a, byte(g[3]), byte(g[4]), byte(g[1]));
            if line != last {
                log!("{line}");
                last = line;
            }
        }
    });
}

#[cfg(target_arch = "x86")]
pub fn install() {
    use crate::log::log;
    use crate::patch::{exe, exe_is};

    if exe_is("client_Release.exe") {
        watch_entry();
        return;
    }
    if !exe_is("control_Release.exe") {
        return;
    }
    if !opted_in(std::env::var("WCCF_UCLIVE").ok().as_deref()) {
        log!(
            "UC live: original (WCCF_UCLIVE=on patches it) - the projector plays the championship, CPU-only ones too"
        );
        return;
    }
    let base = exe();
    // SAFETY: the loaded program's headers: e_lfanew, then SizeOfImage at +0x50 of the PE32 headers
    let size = unsafe {
        let nt = base.add(0x3c).cast::<u32>().read_unaligned() as usize;
        base.add(nt + 0x50).cast::<u32>().read_unaligned() as usize
    };
    if size < REPLAY + LEN {
        log!(
            "UC live: NOT patched - control is smaller than 2017-18's (0x{size:X} bytes), another build"
        );
        return;
    }
    match patch_code(base, REPLAY, LEN, |span, at| {
        apply(span, at, uc_site as *const () as usize)
    }) {
        None => log!(
            "UC live: NOT patched (page protection) - the projector may play an all-CPU championship"
        ),
        Some(Ok(())) => log!(
            "UC live: patched - the Universal Championship is live only with a player's club, competition matches still live"
        ),
        Some(Err(off)) => log!(
            "UC live: NOT patched - control's bytes at base+0x{off:X} differ from 2017-18's ({:02X?}), another build",
            // SAFETY: inside the image (checked above)
            unsafe { base.add(off).cast::<[u8; 7]>().read_unaligned() }
        ),
    }
    FINISHED_AT.store(
        base as usize + FINISHED,
        std::sync::atomic::Ordering::Relaxed,
    );
    match patch_code(base, SLOT, SLOT_ORIGINAL.len(), |span, at| {
        apply_slot(span, at, slot_site as *const () as usize)
    }) {
        Some(true) => log!(
            "UC live: slot clock patched - a slot without a live match lasts as long as one with it"
        ),
        _ => log!(
            "UC live: slot clock NOT patched (control's bytes at base+0x{SLOT:X} differ, or page protection) - empty slots last about 19 min"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Vec<u8> {
        let mut s = vec![0x90; LEN];
        s[..7].copy_from_slice(&ORIGINAL);
        s[UC - REPLAY..].copy_from_slice(&ORIGINAL);
        s
    }

    #[test]
    fn the_sites_are_control_s_two_uc_live_stores() {
        // control_Release.exe 2017-18, image base 0x400000: 0x45DF7B and 0x45DFAC, one page
        assert_eq!([REPLAY, UC].map(|o| 0x40_0000 + o), [0x45_DF7B, 0x45_DFAC]);
        assert_eq!(REPLAY >> 12, (REPLAY + LEN - 1) >> 12);
        // mov byte [ebp+disp32], imm8: C6 /0 with mod 10 rm 101 (85), disp 0x14938 (GameManage +84280), imm 1
        assert_eq!(ORIGINAL[..2], [0xC6, 0x85]);
        assert_eq!(
            u32::from_le_bytes(ORIGINAL[2..6].try_into().unwrap()),
            0x14938
        );
        assert_eq!(ORIGINAL[IMM], 1);
    }

    #[test]
    fn apply_zeroes_the_replay_store_and_calls_the_thunk_at_the_championship_store() {
        let mut s = span();
        let (at, thunk) = (0x40_0000 + REPLAY, 0x1000_0000);
        assert_eq!(apply(&mut s, at, thunk), Ok(()));
        let mut want = span();
        want[IMM] = 0;
        let c = UC - REPLAY;
        let rel = (thunk - (at + c + 5)) as u32;
        want[c] = 0xE8;
        want[c + 1..c + 5].copy_from_slice(&rel.to_le_bytes());
        want[c + 5..c + 7].copy_from_slice(&[0x90, 0x90]);
        assert_eq!(s, want);
        // the call lands on the thunk; a thunk below the program wraps like the CPU does
        let target = |s: &[u8], at: usize| {
            (at + c + 5).wrapping_add(
                i32::from_le_bytes(s[c + 1..c + 5].try_into().unwrap()) as isize as usize
            ) as u32
        };
        assert_eq!(target(&s, at), thunk as u32);
        let mut s = span();
        apply(&mut s, at, 0x1_0000).unwrap();
        assert_eq!(target(&s, at), 0x1_0000);
    }

    #[test]
    fn apply_writes_nothing_unless_both_sites_are_the_original() {
        for off in [REPLAY, UC] {
            let mut s = span();
            s[off - REPLAY + IMM] = 0; // patched already or another build
            let before = s.clone();
            assert_eq!(apply(&mut s, 0x40_0000 + REPLAY, 0x1000_0000), Err(off));
            assert_eq!(s, before);
        }
        assert_eq!(apply(&mut [0u8; 8], 0, 0), Err(REPLAY), "a short span");
    }

    #[test]
    fn by_default_the_championship_store_stays_original() {
        // install's gate: only WCCF_UCLIVE=on applies; unset, the old opt-out 1 and anything else leave both stores
        for v in [None, Some(""), Some("1"), Some("off"), Some("ON")] {
            let mut s = span();
            if opted_in(v) {
                apply(&mut s, 0x40_0000 + REPLAY, 0x1000_0000).unwrap();
            }
            assert_eq!(s, span(), "WCCF_UCLIVE={v:?}");
            assert_eq!(s[UC - REPLAY..], ORIGINAL, "0x45DFAC");
        }
    }

    #[test]
    fn opting_in_installs_the_patch() {
        assert!(opted_in(Some("on")));
        let mut s = span();
        apply(&mut s, 0x40_0000 + REPLAY, 0x1000_0000).unwrap();
        assert_eq!(s[UC - REPLAY], 0xE8, "the call at 0x45DFAC");
        assert_eq!(s[IMM], 0, "the replay store");
    }

    #[test]
    fn the_slot_site_is_the_game_state_s_call_to_sub_461cd0() {
        // 0x456064: E8 rel32, rel from the next instruction 0x456069
        let rel = i32::from_le_bytes(SLOT_ORIGINAL[1..].try_into().unwrap());
        assert_eq!(0x40_0000 + SLOT + 5 + rel as usize, 0x40_0000 + FINISHED);
        assert_eq!(0x40_0000 + FINISHED, 0x46_1CD0);
        let (at, thunk) = (0x40_0000 + SLOT, 0x1000_0000);
        let mut s = SLOT_ORIGINAL;
        assert!(apply_slot(&mut s, at, thunk));
        assert_eq!(s[0], 0xE8);
        assert_eq!(
            (at + 5).wrapping_add(i32::from_le_bytes(s[1..].try_into().unwrap()) as usize),
            thunk
        );
        let before = s;
        assert!(!apply_slot(&mut s, at, thunk), "patched already");
        assert_eq!(s, before);
        assert!(!apply_slot(&mut [0xE8; 4], at, thunk), "short");
    }

    #[test]
    fn only_an_empty_friendly_slot_ends_at_game_secs() {
        let g = GAME_SECS as i32;
        // the original's answer always counts
        assert!(game_over(true, false, false, 0) && game_over(true, true, true, 0));
        // UC live 0, no cup round: the live match's length
        assert!(!game_over(false, false, false, g - 1));
        assert!(game_over(false, false, false, g));
        // the championship live (its match ends the slot) or a cup round (its matches do): untouched
        assert!(!game_over(false, true, false, 900));
        assert!(!game_over(false, false, true, 900));
    }

    #[test]
    fn the_lines_name_the_player_s_places_and_the_slot_s_state() {
        let mut r = vec![0u8; 2 * RECORD];
        for (m, t) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let at = m * RECORD + 40 + t * 3_800;
            r[at..at + 4].copy_from_slice(&950u32.to_le_bytes());
        }
        assert_eq!(
            verdict(&r, 0, 0),
            "UC live: championship match 1/3 (day 0): all-CPU - kept off the projector"
        );
        let at = RECORD + 40 + 3_800; // semifinal 2, place 2
        r[at..at + 4].fill(0);
        r[at - 4..at].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(players(&r), [(1, 1, 3)]);
        assert_eq!(
            verdict(&r, 1, 2),
            "UC live: championship match 3/3 (day 1): a player's club in semifinal 2 place 2 (coach 3) - live on the projector"
        );
        assert!(slot_line(false, true, false).ends_with("a Universal Cup round"));
        assert!(slot_line(true, false, true).contains("plays live"));
        assert!(slot_line(false, false, true).contains("all-CPU"));
        assert!(slot_line(false, false, false).contains("no championship this slot"));
    }

    #[test]
    fn the_entry_watch_reads_only_the_studied_code_and_says_why_no_entry_went() {
        // 0x80EE07 is the online test, 23 bytes in
        assert_eq!(ENTRY + 23, 0x40_EE07);
        assert_eq!(ENTRY_SIG[23..30], [0x80, 0xB8, 0xD1, 0, 0, 0, 0]);
        let g = [
            0x06D9_F24F,
            0x06D9_F258,
            0x06D9_F288,
            0x0683_653F,
            0x06D9_F25A,
        ];
        assert_eq!(entry_globals(&ENTRY_SIG, 0), Some(g));
        // relocated: each address moves with the base
        let delta = 0x0010_0000;
        let mut moved = ENTRY_SIG;
        for at in ENTRY_ADDRS {
            let a = u32::from_le_bytes(moved[at..at + 4].try_into().unwrap()) + delta;
            moved[at..at + 4].copy_from_slice(&a.to_le_bytes());
        }
        assert_eq!(entry_globals(&moved, delta), Some(g.map(|a| a + delta)));
        assert_eq!(
            entry_globals(&ENTRY_SIG, delta),
            None,
            "not relocated as the base says"
        );
        let mut other = ENTRY_SIG;
        other[24] = 0xB9;
        assert_eq!(entry_globals(&other, 0), None, "another build");
        assert_eq!(entry_globals(&ENTRY_SIG[..48], 0), None, "short");
        assert!(entry_line(1, Some(0), 0, 0, 0).ends_with("NOT sent: the online flag is off"));
        assert!(entry_line(1, Some(1), 0, 0, 1).ends_with("- DATA_VS_ENTRY sent"));
        assert!(entry_line(1, None, 1, 1, 0).ends_with("already entered"));
        assert!(entry_line(0, Some(1), 1, 0, 0).ends_with("(gate not 1)"));
        assert_eq!(
            entry_line(1, Some(1), 0, 0, 0),
            "UC entry (client 0x80EDF0): gate 1, online [6D9F288]+209 1 / [683653F] 0, entered 0, sent 0 - entry due, not sent yet"
        );
    }

    #[test]
    fn only_a_semifinal_team_with_cpu_id_0_makes_the_championship_live() {
        let mut r = vec![0u8; 2 * RECORD];
        let ids = [(0, 0), (0, 1), (1, 0), (1, 1)].map(|(m, t)| m * RECORD + 40 + t * 3_800);
        for at in ids {
            r[at..at + 4].copy_from_slice(&950u32.to_le_bytes());
        }
        assert!(!human_in(&r), "all CPU");
        for at in ids {
            let mut h = r.clone();
            h[at..at + 4].fill(0);
            assert!(human_in(&h), "a player's team at {at}");
        }
        assert!(!human_in(&[]));
    }
}
