//! The rules of the 2010-11 server fixes ([`crate::server`] applies them): their settings, and where the MSGPARSE guard
//! finds control's parser frame in a faulting thread's stack.

/// How far above esp the MSGPARSE guard looks for the parser's frame (0x120 until 2026-10-18; a server's crash dump
/// showed deeper frames).
pub const STACK_SCAN: usize = 0x400;

/// C's `strtoul(s, NULL, radix)` for the settings: leading blanks, an optional `+` (and `0x` in base 16), then the
/// digits up to the first other character; 0 without digits, `u32::MAX` past it.
fn strtoul(s: &str, radix: u32) -> u32 {
    let s = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0B', '\x0C']);
    let s = s.strip_prefix('+').unwrap_or(s);
    let s = match radix {
        16 => s
            .strip_prefix("0x")
            .or_else(|| s.strip_prefix("0X"))
            .unwrap_or(s),
        _ => s,
    };
    s.chars()
        .map_while(|c| c.to_digit(radix))
        .try_fold(0u32, |n, d| n.checked_mul(radix)?.checked_add(d))
        .unwrap_or(u32::MAX)
}

/// MXHOOK_MSGGUARD_RET (the self-test's caller return, a hex offset in the program): 0 when unset or 16 characters or
/// longer.
pub fn msgguard_ret(v: Option<&str>) -> u32 {
    v.filter(|v| !v.is_empty() && v.len() < 16)
        .map_or(0, |v| strtoul(v, 16))
}

/// MXHOOK_IDLE_MS: how long the file reader's idle poll waits, 1 to 15 ms; 4 otherwise.
pub fn idle_ms(v: Option<&str>) -> u32 {
    v.filter(|v| !v.is_empty() && v.len() < 16)
        .map(|v| strtoul(v, 10))
        .filter(|n| (1..=15).contains(n))
        .unwrap_or(4)
}

/// The live frame of control's FUN_00412860 in the stack words above `esp` (`words[i]` at esp + 4i, up to the stack's
/// top or [`STACK_SCAN`]): the offset of its return into FUN_00412140 (`ret`). FUN_00412860 reuses its &len slot as a
/// local at 0x41287F, so only buf and &off are left to match (until 2026-10-19 the guard read &len and so never
/// fired): FUN_00412140 passes its own locals, buf at its esp+0x28 and off at +0x10, so the live frame has buf ==
/// slot+0x38 and &off == slot+0x20.
pub fn find_frame(esp: u32, words: &[u32], ret: u32) -> Option<usize> {
    (0x10..STACK_SCAN - 0x10)
        .step_by(4)
        .take_while(|off| (off + 0x10) / 4 <= words.len())
        .find(|&off| {
            let (i, a) = (off / 4, esp.wrapping_add(off as u32));
            words[i] == ret
                && words[i + 2] == a.wrapping_add(0x38)
                && words[i + 3] == a.wrapping_add(0x20)
        })
}

/// The words above esp that look like returns into control's code (ASLR is off in control), at most 24: the stepped-
/// aside fault's report.
pub fn returns(words: &[u32]) -> impl Iterator<Item = (usize, u32)> + '_ {
    words
        .iter()
        .take(STACK_SCAN / 4)
        .enumerate()
        .map(|(i, &v)| (i * 4, v))
        .filter(|&(_, v)| (0x401000..0x700000).contains(&v))
        .take(24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_like_strtoul() {
        assert_eq!(msgguard_ret(Some("1226a")), 0x1226A);
        assert_eq!(msgguard_ret(Some("0x1226A")), 0x1226A);
        assert_eq!(msgguard_ret(Some(" 12zz")), 0x12);
        assert_eq!(msgguard_ret(Some("zz")), 0);
        assert_eq!(msgguard_ret(Some("1234567890abcdef")), 0, "16 characters");
        assert_eq!(msgguard_ret(Some("fffffffff")), u32::MAX);
        assert_eq!(msgguard_ret(None), 0);
        assert_eq!(idle_ms(None), 4);
        assert_eq!(idle_ms(Some("8")), 8);
        assert_eq!(idle_ms(Some("15ms")), 15);
        assert_eq!(idle_ms(Some("0")), 4);
        assert_eq!(idle_ms(Some("16")), 4);
    }

    /// The stack as control's FUN_00412140 -> FUN_00412860 call leaves it (msgguard_test builds the same frame).
    fn frame(esp: u32, at: usize, ret: u32) -> Vec<u32> {
        let mut w = vec![0; 64];
        let a = esp + at as u32;
        w[at / 4] = ret;
        w[at / 4 + 1] = 0x1C; // the reused &len slot
        w[at / 4 + 2] = a + 0x38;
        w[at / 4 + 3] = a + 0x20;
        w
    }

    #[test]
    fn the_live_frame_is_found_and_a_stale_return_is_not() {
        let (esp, ret) = (0x0019_F000, 0x0041_226A);
        assert_eq!(find_frame(esp, &frame(esp, 0x40, ret), ret), Some(0x40));
        let mut stale = frame(esp, 0x40, ret);
        stale[0x40 / 4 + 3] += 4;
        assert_eq!(
            find_frame(esp, &stale, ret),
            None,
            "not the live frame's args"
        );
        let mut both = frame(esp, 0x80, ret);
        both[0x20 / 4] = ret; // an older return below the live one
        assert_eq!(find_frame(esp, &both, ret), Some(0x80));
        assert_eq!(
            find_frame(esp, &frame(esp, 0x40, ret)[..0x4C / 4], ret),
            None,
            "the frame's args must lie below the stack's top"
        );
        assert_eq!(find_frame(esp, &frame(esp, 0x08, ret), ret), None);
    }

    #[test]
    fn returns_into_control() {
        let w = [0x401000, 0x400FFF, 0x6FFFFF, 0x700000, 0x1234];
        assert_eq!(
            returns(&w).collect::<Vec<_>>(),
            [(0, 0x401000), (8, 0x6FFFFF)]
        );
        assert_eq!(returns(&[0x500000; 300]).count(), 24);
    }
}
