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
//! entry 0x45DE47). Off unless WCCF_UCLIVE=on ([`opted_in`]): the projector then loops the CPU championship as before.

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

/// The day file's and GameManage's championship layout (control 0x47D0D0, 0x47CFC0): 3 match records of 7,636 bytes,
/// each with team blocks of 3,800 bytes from +36; a block's +4 is its CPU id (0 = a player's team).
pub const RECORD: usize = 7_636;
const TEAM_ID: usize = 40;
const TEAM: usize = 3_800;

/// Whether the semifinals' 4 team places (records 0 and 1, `records` from the first) hold a player's team. Only called
/// on a valid bracket (all 4 places filled, 0x47CFC0), so an empty place never counts.
#[cfg_attr(all(test, not(debug_assertions)), no_panic::no_panic)]
pub fn human_in(records: &[u8]) -> bool {
    (0..2).any(|m| {
        (0..2).any(|t| {
            let at = m * RECORD + TEAM_ID + t * TEAM;
            records.get(at..at + 4) == Some(&[0; 4])
        })
    })
}

/// Whether WCCF_UCLIVE asks for the patch: only `on` (`1` kept the original before, so it still does). Off by default:
/// with UC live 0 the control sends the projector an empty `TO MATCH INI` and its league scene reads NULL, a black
/// projector at match start (2026-11-02).
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
    let at = UC - REPLAY;
    let rel = thunk.wrapping_sub(span_at + at + 5) as u32;
    let mut call = [0x90; 7];
    call[0] = 0xE8;
    call[1..5].copy_from_slice(&rel.to_le_bytes());
    // both sites passed the check above, so both are in `span`; Err before any write all the same
    if span.get(IMM).is_none() || span.get(at..at + 7).is_none() {
        return Err(UC);
    }
    if let Some(b) = span.get_mut(IMM) {
        *b = 0;
    }
    if let Some(site) = span.get_mut(at..at + 7) {
        site.copy_from_slice(&call);
    }
    Ok(())
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

/// 1 when the current day's championship has a player's team, else 0; logged when the answer changes.
#[cfg(target_arch = "x86")]
extern "C" fn live_flag(ucs: *const u8) -> u8 {
    use std::sync::atomic::{AtomicU8, Ordering::Relaxed};
    static LAST: AtomicU8 = AtomicU8::new(u8::MAX);
    // SAFETY: control's championship object, just checked valid by 0x47CFC0: day (+16) 0 or 1, its records' pointer at
    // +8 + 4 * day, the semifinals' two records behind it
    let live = unsafe {
        let day = ucs.add(16).cast::<u32>().read_unaligned() as usize;
        let records = ucs
            .add(8 + 4 * day.min(1))
            .cast::<*const u8>()
            .read_unaligned();
        human_in(std::slice::from_raw_parts(records, 2 * RECORD))
    };
    let v = u8::from(live);
    if LAST.swap(v, Relaxed) != v {
        crate::log::log!(
            "UC live: {}",
            if live {
                "the Universal Championship has a player's club - on the projector"
            } else {
                "an all-CPU Universal Championship - kept off the projector"
            }
        );
    }
    v
}

#[cfg(target_arch = "x86")]
pub fn install() {
    use crate::log::log;
    use crate::patch::{exe, exe_is};
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

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
    let p = base.wrapping_add(REPLAY).cast_mut();
    let mut prot = 0;
    // SAFETY: LEN bytes of control's code inside its image (checked above), made writable for the patch, then put back;
    // DllMain runs before control's own code, so nothing executes them meanwhile
    let (done, restored) = unsafe {
        if VirtualProtect(p.cast(), LEN, PAGE_EXECUTE_READWRITE, &mut prot) == 0 {
            log!(
                "UC live: NOT patched (page protection) - the projector may play an all-CPU championship"
            );
            return;
        }
        let done = apply(
            std::slice::from_raw_parts_mut(p, LEN),
            p as usize,
            uc_site as *const () as usize,
        );
        let restored = VirtualProtect(p.cast(), LEN, prot, &mut prot) != 0;
        FlushInstructionCache(GetCurrentProcess(), p.cast(), LEN);
        (done, restored)
    };
    match done {
        Ok(()) => log!(
            "UC live: patched - the Universal Championship is live only with a player's club, competition matches still live"
        ),
        Err(off) => log!(
            "UC live: NOT patched - control's bytes at base+0x{off:X} differ from 2017-18's ({:02X?}), another build",
            // SAFETY: inside the image (checked above)
            unsafe { base.add(off).cast::<[u8; 7]>().read_unaligned() }
        ),
    }
    if !restored {
        log!("UC live: control's code page stays writable (its protection could not be put back)");
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
