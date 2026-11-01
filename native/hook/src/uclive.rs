//! No CPU championship on the 2017-18 projector (2026-11-01): control's GameManage sets "UC live" (+0x14938) during a
//! FriendlyMatch slot (competition 17) when it has a valid Universal Championship or loaded data-match replays, and the
//! projector then plays that championship live (control 0x433D00). With no arcade finalists the control fills it with
//! random local CPU teams (0x47DCF0) and replays its 3 matches forever. The hook turns both stores of 1 into stores of 0,
//! so the flag stays 0 (its value outside friendly slots): scheduled competition matches, the Universal Cup's rounds
//! (competition 31) among them, still go to the projector through the schedule path, as before. These are the only two
//! stores of 1 to the flag; the constructor and every state entry (0x45DE47) store 0. WCCF_UCLIVE=1 leaves the original.

/// The two `mov byte [ebp+14938h], 1` in GameManage's state entry (control sub_45D840), as offsets from the program's
/// base (ASLR moves it): 0x45DF7B (replays loaded) and 0x45DFAC (a valid Universal Championship). Both lie behind
/// `cmp ecx, 11h` at 0x45DF54: only a FriendlyMatch slot reaches them.
pub const SITES: [usize; 2] = [0x5DF7B, 0x5DFAC];
/// The original instruction at each site; its last byte is the stored value.
pub const ORIGINAL: [u8; 7] = [0xC6, 0x85, 0x38, 0x49, 0x01, 0x00, 0x01];
/// Where in [`ORIGINAL`] the stored value sits, and what the hook writes there.
pub const IMM: usize = 6;
pub const PATCHED: u8 = 0;

/// The first site whose bytes are not [`ORIGINAL`] (`at` gives the program's 7 bytes at an offset), or None when both
/// match and the patch may be written.
pub fn mismatch(at: impl Fn(usize) -> [u8; 7]) -> Option<usize> {
    SITES.into_iter().find(|&off| at(off) != ORIGINAL)
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
    if std::env::var("WCCF_UCLIVE").is_ok_and(|v| v == "1") {
        log!("UC live: original (WCCF_UCLIVE=1) - the projector may play the CPU championship");
        return;
    }
    // SAFETY: inside control's loaded image (both sites lie in its code section)
    let at = |off: usize| unsafe { exe().add(off).cast::<[u8; 7]>().read_unaligned() };
    if let Some(off) = mismatch(at) {
        log!(
            "UC live: NOT patched - control's bytes at base+0x{off:X} differ from 2017-18's ({:02X?}), another build",
            at(off)
        );
        return;
    }
    // both sites in one span (one page), so the two writes happen together or not at all
    let (lo, hi) = (SITES[0] + IMM, SITES[1] + IMM + 1);
    let mut prot = 0;
    // SAFETY: control's code between the two checked sites, made writable for the two byte writes, its protection put
    // back
    let ok = unsafe {
        let p = exe().add(lo).cast_mut();
        let ok = VirtualProtect(p.cast(), hi - lo, PAGE_EXECUTE_READWRITE, &mut prot) != 0;
        if ok {
            for off in SITES {
                p.add(off + IMM - lo).write(PATCHED);
            }
            VirtualProtect(p.cast(), hi - lo, prot, &mut prot);
            FlushInstructionCache(GetCurrentProcess(), p.cast(), hi - lo);
        }
        ok
    };
    if ok {
        log!(
            "UC live: patched - no CPU championship on the projector, competition matches still live"
        );
    } else {
        log!(
            "UC live: NOT patched (page protection) - the projector may play the CPU championship"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program image with the original instruction at both sites.
    fn image() -> Vec<u8> {
        let mut img = vec![0x90; 0x5E000];
        for off in SITES {
            img[off..off + 7].copy_from_slice(&ORIGINAL);
        }
        img
    }

    fn at(img: &[u8]) -> impl Fn(usize) -> [u8; 7] + '_ {
        |off| img[off..off + 7].try_into().unwrap()
    }

    #[test]
    fn the_sites_are_control_s_two_uc_live_stores() {
        // control_Release.exe 2017-18, image base 0x400000: 0x45DF7B and 0x45DFAC
        assert_eq!(SITES.map(|o| 0x40_0000 + o), [0x45_DF7B, 0x45_DFAC]);
        // mov byte [ebp+disp32], imm8: C6 /0 with mod 10 rm 101 (85), disp 0x14938 (GameManage +84280), imm 1
        assert_eq!(ORIGINAL[..2], [0xC6, 0x85]);
        assert_eq!(
            u32::from_le_bytes(ORIGINAL[2..6].try_into().unwrap()),
            0x14938
        );
        assert_eq!((ORIGINAL[IMM], PATCHED), (1, 0));
    }

    #[test]
    fn the_original_bytes_are_checked_at_both_sites() {
        let mut img = image();
        assert_eq!(mismatch(at(&img)), None);
        img[SITES[1] + IMM] = PATCHED; // already patched or another build: not touched
        assert_eq!(mismatch(at(&img)), Some(SITES[1]));
        img = image();
        img[SITES[0]] = 0x90;
        assert_eq!(mismatch(at(&img)), Some(SITES[0]));
        // one VirtualProtect span covers both stored bytes
        assert_eq!(SITES[0] >> 12, SITES[1] >> 12);
    }
}
