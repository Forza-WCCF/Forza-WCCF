//! Money with commas and the format guard, ported from the C panel (`native/2010-11/panel/wccfpanel.c`, the "a bad %
//! no longer stops seat 1" and "money with commas" sections, and `money_install`). The 2017-18 overlay's C
//! (`native/2017-18/overlay/wccfpanel.c`) carries the very same code: the same table, addresses, helper heads and
//! import slots, byte for byte. So both editions run the one table below, and the checks decide: a program that is not
//! the one the table was made for is refused (off, said in the log, nothing patched).
//!
//! The format guard: the game's 2D text drawer FUN_0040e170 uses every text AS A FORMAT (vsprintf_s(buf, 0x401, text,
//! ...)), and msvcr90's secure printf ends the program (0xC0000417) when a text holds a % that starts no valid code.
//! The game's own import of MSVCR90.dll!vsprintf_s points at [`safe_vsprintf_s`]: the call goes to msvcr90 as before,
//! under a per-thread flag that our invalid-parameter handler honours. When msvcr90 refuses THIS call, the text is
//! drawn as it is and written to the log once. Every other invalid parameter goes on to msvcr90's `_invoke_watson`.
//!
//! Money: the game cuts each amount into a 100-million-yen part and a 10,000-yen part and formats them with its own
//! templates (docs\research\MONEY-2010-11.md). The places in [`MSITES`] (known by WHERE the game called from, never by
//! text) write "$2,299,000" instead: 'S' = the game's FUN_006d7da0(fmt, ...) -> vsprintf_s (the place is the return
//! address at ap - 8), 'Q' = FUN_00429750 -> vsprintf_s (ap - 0xC), 'P' = the game's own sprintf_s call (the place is
//! our return address). Only digits, commas, "$" and "-" are written (never a %), cut to the size the game gave.
//!
//! Variadics: the two replacements are naked cdecl thunks that hand the return address and the va_list (a pointer to
//! the arguments on x86 MSVC) to plain `extern "C"` functions. The C's TlsAlloc flag is a `thread_local!` [`Cell`].
#![allow(dead_code)] // until lib.rs calls the install functions

use std::cell::Cell;
use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{GetLastError, SetLastError};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, PAGE_PROTECTION_FLAGS,
    PAGE_READWRITE, VirtualProtect, VirtualQuery,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::core::s;

/// msvcr90's `vsprintf_s(char *, size_t, const char *, va_list)`.
type VsprintfS = unsafe extern "C" fn(*mut u8, usize, *const u8, *const u8) -> i32;
/// An invalid-parameter handler (expression, function, file, line, reserved).
type InvalidParamH = unsafe extern "C" fn(*const u16, *const u16, *const u16, u32, usize);
type SetInvalidParamH = unsafe extern "C" fn(Option<InvalidParamH>) -> Option<InvalidParamH>;

/// FUN_006d7da0's call [vsprintf_s] returns here.
const MONEY_SETTEXT_RET: usize = 0x006D_7DD5;
/// FUN_00429750's call [vsprintf_s] returns here ('Q' places).
const MONEY_P429_RET: usize = 0x0042_9769;
/// Where the kit's English "$%d,000,000" sits (the English marker, and three places' other format).
const ENGFMT: usize = 0x00A6_2914;

/// The log the hooks write to (set by the install functions).
static LOG: OnceLock<fn(&str)> = OnceLock::new();
static REAL_VSPRINTF_S: AtomicUsize = AtomicUsize::new(0);
static INVOKE_WATSON: AtomicUsize = AtomicUsize::new(0);
/// The guard's wrapper is in the vsprintf_s import slot.
static FMT_GUARD_ON: AtomicBool = AtomicBool::new(false);
/// The refused texts already written to the log (FNV-1a hashes).
static BADFMT_SEEN: [AtomicU32; 64] = [const { AtomicU32::new(0) }; 64];
static BADFMT_N: AtomicI32 = AtomicI32::new(0);
static MONEY_ON: AtomicBool = AtomicBool::new(false);
/// msvcr90's vsprintf_s for [`money_sprintf_s`]'s other calls when the fmt guard is not installed.
static MSVCR_VSPRINTF_S: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// The C's TLS slot: 1 = inside our vsprintf_s, 2 = msvcr90 refused it.
    static FMT_FLAG: Cell<u8> = const { Cell::new(0) };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Nil,
    /// one call, the value in $100
    Full,
    /// one call, the two parts
    Full2,
    /// one call, the 100-million part alone
    FullOku,
    /// the 100-million box or piece: left empty, its count waits for the box beside it
    Oku,
    /// no 100-million part: the game's own empty text
    Pass,
    /// the rest is 0
    Zero,
    /// the rest, or the whole value when nothing waits
    Man,
    /// the pieces' join ("%s%s")
    Final,
}
use Role::*;

struct MSite {
    ret: u32,
    kind: u8,
    grp: u8,
    role: Role,
    alt_role: Role,
    alt_fmt: u32,
    what: &'static str,
}

const fn m(
    ret: u32,
    kind: u8,
    grp: u8,
    role: Role,
    alt_role: Role,
    alt_fmt: u32,
    what: &'static str,
) -> MSite {
    MSite {
        ret,
        kind,
        grp,
        role,
        alt_role,
        alt_fmt,
        what,
    }
}

/// The C's MSITES (MONEY-2010-11.md Appendix A's places for step 1, D9 and the pieces), by address.
#[rustfmt::skip]
static MSITES: [MSite; 71] = [
    m(0x0045F188, b'S', 12, Full2, Nil, 0, "D12 manager license salary"),
    m(0x0045F1CB, b'S', 12, Full, Nil, 0, "D12 manager license salary"),
    m(0x00533634, b'S', 1, FullOku, Nil, 0, "D1 fan event income"),
    m(0x005337AD, b'S', 2, Oku, Nil, 0, "D2 fan event Prize Money, start (100-million box)"),
    m(0x005337C5, b'S', 2, Zero, Nil, 0, "D2 fan event Prize Money, start"),
    m(0x00533803, b'S', 2, Man, Nil, 0, "D2 fan event Prize Money, start"),
    m(0x0053385B, b'P', 3, FullOku, Nil, 0, "D3 fan event UP line"),
    m(0x00533B29, b'S', 2, Oku, Nil, 0, "D2 fan event Prize Money, counting up (100-million box)"),
    m(0x00533B41, b'S', 2, Zero, Nil, 0, "D2 fan event Prize Money, counting up"),
    m(0x00533B89, b'S', 2, Man, Nil, 0, "D2 fan event Prize Money, counting up"),
    m(0x00533C02, b'S', 2, Oku, Nil, 0, "D2 fan event Prize Money (100-million box)"),
    m(0x00533C19, b'S', 2, Zero, Nil, 0, "D2 fan event Prize Money"),
    m(0x00533C6C, b'S', 2, Man, Nil, 0, "D2 fan event Prize Money"),
    m(0x0053862D, b'S', 4, Oku, Nil, 0, "D4 golden age Prize Money, start (100-million box)"),
    m(0x00538645, b'S', 4, Zero, Nil, 0, "D4 golden age Prize Money, start"),
    m(0x00538683, b'S', 4, Man, Nil, 0, "D4 golden age Prize Money, start"),
    m(0x005386FB, b'P', 5, Full2, Nil, 0, "D5 golden age UP line"),
    m(0x00538716, b'P', 5, FullOku, Full, 0x00A27240, "D5 golden age UP line"),
    m(0x00538A7F, b'S', 4, Oku, Nil, 0, "D4 golden age Prize Money, counting up (100-million box)"),
    m(0x00538A93, b'S', 4, Zero, Nil, 0, "D4 golden age Prize Money, counting up"),
    m(0x00538ABD, b'S', 4, Man, Nil, 0, "D4 golden age Prize Money, counting up"),
    m(0x00538B92, b'S', 4, Oku, Nil, 0, "D4 golden age Prize Money (100-million box)"),
    m(0x00538BA9, b'S', 4, Zero, Nil, 0, "D4 golden age Prize Money"),
    m(0x00538BFC, b'S', 4, Man, Nil, 0, "D4 golden age Prize Money"),
    m(0x005432C0, b'Q', 15, Oku, Nil, 0, "D15 old license card salary (100-million piece)"),
    m(0x00543328, b'Q', 15, Man, Nil, 0, "D15 old license card salary"),
    m(0x00543386, b'S', 15, Final, Nil, 0, "D15 old license card salary (joined)"),
    m(0x00550128, b'Q', 14, Oku, Nil, 0, "D14 coach data salary (100-million piece)"),
    m(0x0055018D, b'Q', 14, Man, Nil, 0, "D14 coach data salary"),
    m(0x005501F8, b'S', 14, Final, Nil, 0, "D14 coach data salary (joined)"),
    m(0x00552C2F, b'S', 13, Full, Nil, 0, "D13 coach room prize (zero)"),
    m(0x00552C4D, b'Q', 13, Oku, Nil, 0, "D13 coach room prize (100-million piece)"),
    m(0x00552CB5, b'Q', 13, Man, Nil, 0, "D13 coach room prize"),
    m(0x00552D18, b'S', 13, Final, Nil, 0, "D13 coach room prize (joined)"),
    m(0x0055839D, b'Q', 16, Oku, Nil, 0, "D16 match offer seat prize (100-million piece)"),
    m(0x005583FE, b'Q', 16, Man, Nil, 0, "D16 match offer seat prize"),
    m(0x00558465, b'S', 16, Final, Nil, 0, "D16 match offer seat prize (joined)"),
    m(0x00576F69, b'S', 171, Full, Nil, 0, "D17 CLUB TEAM DATA Prize Money (zero)"),
    m(0x00576FF3, b'P', 171, Oku, Nil, 0, "D17 CLUB TEAM DATA Prize Money (100-million piece)"),
    m(0x0057705A, b'P', 171, Man, Nil, 0, "D17 CLUB TEAM DATA Prize Money"),
    m(0x005770B8, b'S', 171, Final, Nil, 0, "D17 CLUB TEAM DATA Prize Money (joined)"),
    m(0x0057758C, b'S', 172, Full, Nil, 0, "D17 CLUB TEAM DATA Annual Salary (zero)"),
    m(0x00577610, b'P', 172, Oku, Nil, 0, "D17 CLUB TEAM DATA Annual Salary (100-million piece)"),
    m(0x0057766E, b'P', 172, Man, Nil, 0, "D17 CLUB TEAM DATA Annual Salary"),
    m(0x005776CE, b'S', 172, Final, Nil, 0, "D17 CLUB TEAM DATA Annual Salary (joined)"),
    m(0x0060345D, b'P', 18, Full2, Nil, 0, "D18/D19 projector prize"),
    m(0x006034A0, b'P', 18, Full, Nil, 0, "D18/D19 projector prize"),
    m(0x00661703, b'P', 19, Full, Nil, 0, "D19 projector ranking prize"),
    m(0x0066172A, b'P', 19, Full2, Nil, 0, "D19 projector ranking prize"),
    m(0x006F5049, b'S', 11, Full, Nil, 0, "D11 contract end prize (zero)"),
    m(0x006F50FB, b'S', 11, FullOku, Nil, 0, "D11 contract end previous salary"),
    m(0x006F515F, b'S', 11, Full, Nil, 0, "D11 contract end previous salary (zero)"),
    m(0x006F51A9, b'S', 11, Full2, Full, 0x00A2BDB4, "D11 contract end previous salary"),
    m(0x006F5E57, b'S', 11, Full, Nil, 0, "D11 contract end last pay (zero)"),
    m(0x00705437, b'S', 9, Oku, Nil, 0, "D9 after-match total, counting up (100-million box)"),
    m(0x00705455, b'S', 9, Pass, Nil, 0, "D9 after-match total, counting up (100-million box)"),
    m(0x00705470, b'S', 9, Zero, Nil, 0, "D9 after-match total, counting up"),
    m(0x007054AA, b'S', 9, Man, Nil, 0, "D9 after-match total, counting up"),
    m(0x0070558C, b'S', 7, Full2, Nil, 0, "D7 after-match Match Prize / Matches"),
    m(0x007055D4, b'S', 7, Full, FullOku, 0x00A62914, "D7 after-match Match Prize / Matches"),
    m(0x00705636, b'S', 8, Full2, Nil, 0, "D8 after-match Club Income / Titles"),
    m(0x0070567D, b'S', 8, Full, FullOku, 0x00A62914, "D8 after-match Club Income / Titles"),
    m(0x00705702, b'S', 9, Oku, Nil, 0, "D9 after-match total (100-million box)"),
    m(0x00705722, b'S', 9, Pass, Nil, 0, "D9 after-match total (100-million box)"),
    m(0x0070573D, b'S', 9, Zero, Nil, 0, "D9 after-match total / Annual Salary"),
    m(0x0070577F, b'S', 9, Man, Nil, 0, "D9 after-match total / Annual Salary"),
    m(0x007079D5, b'S', 10, Full2, Nil, 0, "D10 kick-off box, home"),
    m(0x007079E9, b'S', 10, Full, FullOku, 0x00A62914, "D10 kick-off box, home"),
    m(0x00707C24, b'S', 10, FullOku, Nil, 0, "D10 kick-off box, away"),
    m(0x00707CA1, b'S', 10, Full2, Nil, 0, "D10 kick-off box, away"),
    m(0x00707CB5, b'S', 10, Full, Nil, 0, "D10 kick-off box, away"),
];

/// Each place says its first amount in the log, once.
static MSITE_SAID: [AtomicBool; MSITES.len()] = [const { AtomicBool::new(false) }; MSITES.len()];

/// An OKU's count waiting for its ZERO/MAN/FINAL (tid 0 = none): the C's `g_mpend`.
struct Pend {
    tid: u32,
    grp: u8,
    oku: i32,
}
static PEND: Mutex<Pend> = Mutex::new(Pend {
    tid: 0,
    grp: 0,
    oku: 0,
});

fn pend() -> std::sync::MutexGuard<'static, Pend> {
    PEND.lock().unwrap_or_else(|e| e.into_inner())
}

/// The C's `money_text`: `v` in $100 -> "$2,299,000", "-$15,000", "$0", cut to the box with its 0; the length, -1 =
/// no box.
fn money_text(buf: &mut [u8], v: i64) -> i32 {
    if buf.is_empty() {
        return -1;
    }
    let mut d = v.unsigned_abs() * 100;
    let mut out = Vec::with_capacity(32);
    let mut digits = 0;
    loop {
        if digits != 0 && digits % 3 == 0 {
            out.push(b',');
        }
        out.push(b'0' + (d % 10) as u8);
        d /= 10;
        digits += 1;
        if d == 0 {
            break;
        }
    }
    out.push(b'$');
    if v < 0 {
        out.push(b'-');
    }
    out.reverse();
    let n = out.len().min(buf.len() - 1);
    buf[..n].copy_from_slice(&out[..n]);
    buf[n] = 0;
    n as i32
}

/// The C's `money_value`: the amount in $100, from the game's own arguments.
fn money_value(role: Role, a: [i32; 2]) -> i64 {
    let (o, w) = (i64::from(a[0]), i64::from(a[1]));
    match role {
        Full => o,
        FullOku => o * 10000,
        _ if o < 0 => o * 10000 - w.abs(), // -(|o| x 10000 + w)
        _ => o * 10000 + w,                // FULL2
    }
}

/// The C's `money_place`: a place the table knows -> the amount written there (its length); -2 = not ours, the game's
/// own text goes on.
///
/// # Safety
/// `buf` is null or `size` writable bytes; `ap` points at the game's int arguments (two readable for FULL2).
unsafe fn money_place(
    log: fn(&str),
    ret: u32,
    kind: u8,
    buf: *mut u8,
    size: usize,
    fmt: usize,
    ap: *const i32,
) -> i32 {
    let Some(i) = MSITES.iter().position(|s| s.ret == ret && s.kind == kind) else {
        return -2;
    };
    let site = &MSITES[i];
    let buf: &mut [u8] = if buf.is_null() || size == 0 {
        &mut []
    } else {
        // SAFETY: the caller's box, `size` bytes
        unsafe { std::slice::from_raw_parts_mut(buf, size) }
    };
    // SAFETY: the caller's arguments
    let a0 = || unsafe { ap.read() };
    let role = if site.alt_fmt != 0 && fmt == site.alt_fmt as usize {
        site.alt_role
    } else {
        site.role
    };
    // SAFETY: a plain query
    let tid = unsafe { GetCurrentThreadId() };
    let (waiting, oku) = {
        let mut p = pend();
        let waiting = p.tid == tid && p.grp == site.grp;
        let oku = if waiting { p.oku } else { 0 };
        if p.tid == tid {
            p.tid = 0; // every place of ours on this thread uses up what was waiting
        }
        (waiting, i64::from(oku))
    };
    let r = match role {
        Oku => {
            if buf.is_empty() {
                return -2;
            }
            *pend() = Pend {
                tid,
                grp: site.grp,
                oku: a0(),
            };
            buf[0] = 0;
            0
        }
        Pass => return -2, // no 100-million part: the game's own empty text
        Final if !waiting => return -2, // the pieces carry the whole amount already: the game joins them
        Final => money_text(buf, oku * 10000), // "0000万" came instead of a MAN piece: the waiting count's amount
        Zero if !waiting => return -2,         // never seen: the game's own text
        Zero => money_text(buf, oku * 10000),
        // the rest comes as |rest| after a 100-million part
        Man if waiting => {
            let rest = i64::from(a0()).abs();
            money_text(buf, oku * 10000 + if oku < 0 { -rest } else { rest })
        }
        Man => money_text(buf, i64::from(a0())), // after PASS: the whole value, signed
        _ => {
            // SAFETY: FULL2's two ints (the others read only the first)
            let a = unsafe { [ap.read(), if role == Full2 { ap.add(1).read() } else { 0 }] };
            money_text(buf, money_value(role, a))
        }
    };
    if r < 0 {
        return -2;
    }
    if !MSITE_SAID[i].swap(true, Relaxed) {
        let how = if role != site.role {
            ", its other format"
        } else if role == Man && waiting {
            ", with its 100-million part"
        } else {
            ""
        };
        let text = String::from_utf8_lossy(&buf[..r as usize]);
        log(&format!(
            "money: {} (place 0x{ret:08X}{how}) -> \"{text}\"",
            site.what
        ));
    }
    r
}

/// The C's `fmt_handler`: msvcr90's invalid-parameter handler while the guard is on.
unsafe extern "C" fn fmt_handler(
    e: *const u16,
    f: *const u16,
    file: *const u16,
    line: u32,
    r: usize,
) {
    // SAFETY: plain queries
    let err = unsafe { GetLastError() };
    if FMT_FLAG.get() == 1 {
        FMT_FLAG.set(2);
        // SAFETY: plain call
        unsafe { SetLastError(err) };
        return; // vsprintf_s now returns -1 to our wrapper
    }
    // SAFETY: plain call; msvcr90's own _invoke_watson, read at install: the stop, exactly as before
    unsafe {
        SetLastError(err);
        std::mem::transmute::<usize, InvalidParamH>(INVOKE_WATSON.load(Relaxed))(
            e, f, file, line, r,
        );
    }
}

/// The C's `safe_vsprintf_s`, put in the game's vsprintf_s import slot: a cdecl thunk handing the return address on.
#[unsafe(naked)]
unsafe extern "C" fn safe_vsprintf_s() -> i32 {
    // [esp] = ret, +4 buf, +8 size, +12 fmt, +16 ap: each push moves the next one to +16
    core::arch::naked_asm!(
        "push dword ptr [esp+16]",
        "push dword ptr [esp+16]",
        "push dword ptr [esp+16]",
        "push dword ptr [esp+16]",
        "push dword ptr [esp+16]",
        "call {f}",
        "add esp, 20",
        "ret",
        f = sym safe_vsprintf_s_impl,
    )
}

extern "C" fn safe_vsprintf_s_impl(
    ret: usize,
    buf: *mut u8,
    size: usize,
    fmt: *const u8,
    ap: *const u8,
) -> i32 {
    // SAFETY: plain query
    let mut err = unsafe { GetLastError() };
    let log = LOG.get().copied().unwrap_or(|_| {});
    let place = if MONEY_ON.load(Relaxed) && ret == MONEY_SETTEXT_RET {
        Some((8, b'S')) // a box text: one of ours?
    } else if MONEY_ON.load(Relaxed) && ret == MONEY_P429_RET {
        Some((0xC, b'Q')) // a piece: one of ours?
    } else {
        None
    };
    if let Some((back, kind)) = place {
        // SAFETY: the helper's frame, checked byte by byte at install: the place's return address sits at ap - back
        let r = unsafe {
            let at = (ap.sub(back) as *const u32).read();
            money_place(log, at, kind, buf, size, fmt as usize, ap as *const i32)
        };
        // SAFETY: plain call
        unsafe { SetLastError(err) };
        if r >= 0 {
            return r;
        }
    }
    FMT_FLAG.set(1);
    // SAFETY: msvcr90's vsprintf_s (read at install) with the game's own arguments
    let mut r = unsafe {
        SetLastError(err);
        std::mem::transmute::<usize, VsprintfS>(REAL_VSPRINTF_S.load(Relaxed))(buf, size, fmt, ap)
    };
    // SAFETY: plain query
    err = unsafe { GetLastError() };
    let was = FMT_FLAG.replace(0);
    // SAFETY: plain call
    unsafe { SetLastError(err) };
    if was == 2 && !buf.is_null() && size > 0 {
        // refused: the text as it is, cut to the box
        let text: &[u8] = if fmt.is_null() {
            b""
        } else {
            // SAFETY: the game's format, a C string
            unsafe { CStr::from_ptr(fmt.cast()) }.to_bytes()
        };
        let n = text.len().min(size - 1);
        // SAFETY: the game's box, `size` bytes
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
            *buf.add(n) = 0;
        }
        r = n as i32;
        let h = text.iter().fold(2_166_136_261u32, |h, &c| {
            (h ^ u32::from(c)).wrapping_mul(16_777_619)
        });
        let known = BADFMT_N.load(Relaxed).min(64) as usize;
        let seen = BADFMT_SEEN[..known].iter().any(|s| s.load(Relaxed) == h);
        if !seen && BADFMT_N.load(Relaxed) < 64 {
            let k = BADFMT_N.fetch_add(1, Relaxed);
            if k < 64 {
                BADFMT_SEEN[k as usize].store(h, Relaxed);
            }
            let mut esc = String::new();
            for &c in text {
                if esc.len() >= 400 - 6 {
                    break;
                }
                if (32..127).contains(&c) && c != b'\\' {
                    esc.push(c as char);
                } else {
                    esc.push_str(&format!("\\x{c:02x}"));
                }
            }
            log(&format!(
                "badfmt: msvcr90 refused this text as a format - drawn as it is instead of stopping seat 1: \"{esc}\""
            ));
        }
    }
    r
}

/// The C's `iat_slot`: the import slot of `dll!func` in the module at `base` (by name, from the import table).
///
/// # Safety
/// `base` is a loaded 32-bit module.
unsafe fn iat_slot(base: usize, dll: &str, func: &str) -> Option<*mut usize> {
    // SAFETY: the module's own headers and import table, as the loader left them
    unsafe {
        let u32_at = |a: usize| (a as *const u32).read_unaligned() as usize;
        let cstr = |a: usize| CStr::from_ptr(a as *const _).to_bytes();
        let nt = base + u32_at(base + 0x3C);
        let imports = u32_at(nt + 0x80); // OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT]
        if imports == 0 {
            return None;
        }
        let mut d = base + imports;
        while u32_at(d + 12) != 0 {
            let (names, slots) = (u32_at(d), u32_at(d + 16)); // OriginalFirstThunk, FirstThunk
            if cstr(base + u32_at(d + 12)).eq_ignore_ascii_case(dll.as_bytes()) && names != 0 {
                let (mut n, mut s) = (base + names, base + slots);
                while u32_at(n) != 0 {
                    let by = u32_at(n);
                    if by & 0x8000_0000 == 0 && cstr(base + by + 2) == func.as_bytes() {
                        return Some(s as *mut usize);
                    }
                    n += 4;
                    s += 4;
                }
            }
            d += 20;
        }
        None
    }
}

/// Points the import slot at `to`; Err(GetLastError) when it could not be opened.
///
/// # Safety
/// `slot` is an import slot of the program.
unsafe fn write_slot(slot: *mut usize, to: usize) -> Result<(), u32> {
    let mut old = PAGE_PROTECTION_FLAGS::default();
    // SAFETY: one pointer of the program's import table, put back to its protection right after
    unsafe {
        if VirtualProtect(slot as *const c_void, 4, PAGE_READWRITE, &mut old).is_err() {
            return Err(GetLastError().0);
        }
        slot.write(to);
        let _ = VirtualProtect(slot as *const c_void, 4, old, &mut old);
    }
    Ok(())
}

fn proc(m: windows::Win32::Foundation::HMODULE, name: windows::core::PCSTR) -> usize {
    // SAFETY: a loaded module, a name
    unsafe { GetProcAddress(m, name) }.map_or(0, |f| f as usize)
}

/// The C's `fmt_guard_install`: seat 1 only, before [`money_install`]. The C of both editions is the same.
pub fn fmt_guard_install(_e1718: bool, log: fn(&str)) {
    let _ = LOG.set(log);
    // SAFETY: plain query
    let crt = unsafe { GetModuleHandleA(s!("msvcr90.dll")) }.ok();
    let (set, real, watson) = crt.map_or((0, 0, 0), |c| {
        (
            proc(c, s!("_set_invalid_parameter_handler")),
            proc(c, s!("vsprintf_s")),
            proc(c, s!("_invoke_watson")),
        )
    });
    // SAFETY: the program's own module
    let me = unsafe { GetModuleHandleA(None) }.map_or(0, |m| m.0 as usize);
    let slot = if crt.is_some() {
        // SAFETY: the program is loaded
        unsafe { iat_slot(me, "msvcr90.dll", "vsprintf_s") }
    } else {
        None
    };
    let crt = crt.map_or(0, |c| c.0 as usize);
    INVOKE_WATSON.store(watson, Relaxed);
    let Some(slot) = slot.filter(|_| crt != 0 && set != 0 && real != 0 && watson != 0) else {
        log(&format!(
            "fmt guard: not installed (msvcr90 {crt:08X}, handler setter {set:08X}, vsprintf_s {real:08X}, _invoke_watson {watson:08X}, the program's import slot {:08X})",
            slot.map_or(0, |s| s as usize)
        ));
        return;
    };
    // SAFETY: the program's import slot
    if unsafe { slot.read() } != real {
        log("fmt guard: the import slot does not hold msvcr90's vsprintf_s - left alone");
        return;
    }
    // (the C's "no TLS slot" refusal cannot happen: the flag is a thread_local)
    REAL_VSPRINTF_S.store(real, Relaxed);
    // SAFETY: msvcr90's _set_invalid_parameter_handler
    let set = unsafe { std::mem::transmute::<usize, SetInvalidParamH>(set) };
    // SAFETY: our handler, cdecl as msvcr90 calls it
    if let Some(prev) = unsafe { set(Some(fmt_handler)) } {
        // the game has its own handler: do not stand in its way
        // SAFETY: as above
        unsafe { set(Some(prev)) };
        log("fmt guard: the program already has an invalid-parameter handler - not installed");
        return;
    }
    // SAFETY: the program's import slot, checked above
    if let Err(e) = unsafe { write_slot(slot, safe_vsprintf_s as *const () as usize) } {
        // SAFETY: as above
        unsafe { set(None) };
        log(&format!(
            "fmt guard: the import slot could not be opened (error {e}) - not installed"
        ));
        return;
    }
    FMT_GUARD_ON.store(true, Relaxed);
    log(&format!(
        "fmt guard: vsprintf_s (import slot {:08X}) checked - a text msvcr90 refuses is drawn as it is and logged",
        slot as usize
    ));
}

/// The C's `money_sprintf_s`, put in the game's sprintf_s import slot: a cdecl variadic thunk handing on the return
/// address and the va_list (the address of the first `...` argument).
#[unsafe(naked)]
unsafe extern "C" fn money_sprintf_s() -> i32 {
    // [esp] = ret, +4 buf, +8 size, +12 fmt, +16 the first `...`
    core::arch::naked_asm!(
        "lea eax, [esp+16]",
        "push eax",
        "push dword ptr [esp+16]",
        "push dword ptr [esp+16]",
        "push dword ptr [esp+16]",
        "push dword ptr [esp+16]",
        "call {f}",
        "add esp, 20",
        "ret",
        f = sym money_sprintf_s_impl,
    )
}

/// One of the table's 'P' places -> the amount with commas; anything else -> formatted exactly as before (through the
/// fmt guard when it is on, so a bad % in a sprintf_s text no longer stops seat 1 either).
extern "C" fn money_sprintf_s_impl(
    ret: usize,
    buf: *mut u8,
    size: usize,
    fmt: *const u8,
    ap: *const u8,
) -> i32 {
    // SAFETY: plain query
    let err = unsafe { GetLastError() };
    let mut r = -2;
    if MONEY_ON.load(Relaxed) {
        let log = LOG.get().copied().unwrap_or(|_| {});
        // SAFETY: the game's box and arguments
        r = unsafe {
            money_place(
                log,
                ret as u32,
                b'P',
                buf,
                size,
                fmt as usize,
                ap as *const i32,
            )
        };
    }
    // SAFETY: plain call
    unsafe { SetLastError(err) };
    if r >= 0 {
        return r;
    }
    if FMT_GUARD_ON.load(Relaxed) {
        safe_vsprintf_s_impl(0, buf, size, fmt, ap)
    } else {
        // SAFETY: msvcr90's vsprintf_s (read at install) with the game's own arguments
        unsafe {
            std::mem::transmute::<usize, VsprintfS>(MSVCR_VSPRINTF_S.load(Relaxed))(
                buf, size, fmt, ap,
            )
        }
    }
}

/// The C's `mt_*`: the self-test's count and its log.
struct Mt {
    n: u32,
    fail: u32,
    log: fn(&str),
}

impl Mt {
    fn check(&mut self, name: &str, got: &str, want: &str) {
        let ok = got == want;
        self.n += 1;
        self.fail += u32::from(!ok);
        let tail = if ok {
            String::new()
        } else {
            format!("  (wanted \"{want}\")")
        };
        (self.log)(&format!(
            "money test: {}  {name} -> \"{got}\"{tail}",
            if ok { "PASS" } else { "FAIL" }
        ));
    }

    #[allow(clippy::too_many_arguments)] // the C's mt_place, one case per line
    fn place(&mut self, name: &str, ret: u32, kind: u8, a0: i32, a1: i32, fmt: usize, want: &str) {
        let got = mt_place(self.log, ret, kind, a0, a1, fmt);
        self.check(name, &got, want);
    }

    fn text(&mut self, name: &str, size: usize, v: i64, want: &str) {
        let mut b = [0u8; 48];
        money_text(&mut b[..size], v);
        self.check(name, &cstr(&b), want);
    }
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or_default()).into_owned()
}

fn mt_place(log: fn(&str), ret: u32, kind: u8, a0: i32, a1: i32, fmt: usize) -> String {
    let args = [a0, a1];
    let mut b = [0u8; 48];
    // SAFETY: our own box and arguments
    if unsafe { money_place(log, ret, kind, b.as_mut_ptr(), b.len(), fmt, args.as_ptr()) } == -2 {
        return "(the game's own)".into();
    }
    cstr(&b)
}

/// The C's `money_selftest` (WCCFPANEL_MONEYTEST=1): every role through `money_place` itself, with the game's arguments
/// faked; "money test: PASS|FAIL ..." lines and a total in the log. What it changed is put back afterwards.
pub fn money_selftest(_e1718: bool, log: fn(&str)) {
    const T: [(i64, &str); 13] = [
        (0, "$0"),
        (1, "$100"),
        (99, "$9,900"),
        (150, "$15,000"),
        (2299, "$229,900"),
        (10000, "$1,000,000"),
        (22990, "$2,299,000"),
        (37072, "$3,707,200"),
        (100000, "$10,000,000"),
        (-150, "-$15,000"),
        (-22990, "-$2,299,000"),
        (2_147_483_647, "$214,748,364,700"),
        (-2_147_483_648, "-$214,748,364,800"),
    ];
    const F2: [(i32, i32, &str); 5] = [
        (2, 2990, "$2,299,000"),
        (10, 0, "$10,000,000"),
        (0, 7500, "$750,000"),
        (-2, 2990, "-$2,299,000"),
        (3, 7072, "$3,707,200"),
    ];
    let mut mt = Mt { n: 0, fail: 0, log };
    for (v, want) in T {
        mt.text(&format!("money({v})"), 48, v, want);
    }
    for (o, w, want) in F2 {
        mt.text(
            &format!("FULL2 {o}, {w}"),
            48,
            money_value(Full2, [o, w]),
            want,
        );
    }
    mt.text(
        "FULLOKU 10",
        48,
        money_value(FullOku, [10, 0]),
        "$10,000,000",
    );
    mt.text("22990 in a 6-byte box", 6, 22990, "$2,29");
    mt.text("22990 in a 1-byte box", 1, 22990, "");
    mt.check(
        "no box at all",
        if money_text(&mut [], 5) == -1 {
            "-1"
        } else {
            "?"
        },
        "-1",
    );
    mt_places(&mut mt);
    log(&format!("money test: {} checks, {} failed", mt.n, mt.fail));
    for s in &MSITE_SAID {
        s.store(false, Relaxed); // the real run says its first amounts again
    }
    pend().tid = 0;
}

/// The C's `mt_place` cases, in its order: D9, D2/D4, the pieces displays, the one-call places.
#[rustfmt::skip]
fn mt_places(mt: &mut Mt) {
    // compared as pointers, never read
    let (x, empty, ss) = (c"x".as_ptr() as usize, c"".as_ptr() as usize, c"%s%s".as_ptr() as usize);
    let log = mt.log;
    // D9, as FUN_00705330 calls it: the 100-million box, then the box beside it
    mt.place("D9 OKU 3", 0x00705702, b'S', 3, 0, x, "");
    mt.place("D9 then MAN 7612", 0x0070577F, b'S', 7612, 0, x, "$3,761,200");
    mt.place("D9 PASS", 0x00705722, b'S', 0, 0, empty, "(the game's own)");
    mt.place("D9 then MAN 2309 (Annual Salary)", 0x0070577F, b'S', 2309, 0, x, "$230,900");
    mt.place("D9 count-up OKU 1", 0x00705437, b'S', 1, 0, x, "");
    mt.place("D9 then ZERO", 0x00705470, b'S', 0, 0, empty, "$1,000,000");
    mt.place("D9 OKU -1", 0x00705702, b'S', -1, 0, x, "");
    mt.place("D9 then MAN 5000 (|rest|)", 0x0070577F, b'S', 5000, 0, x, "-$1,500,000");
    mt.place("D9 count-up MAN 0, nothing waiting", 0x007054AA, b'S', 0, 0, x, "$0");
    mt.place("D9 ZERO, nothing waiting", 0x0070573D, b'S', 0, 0, empty, "(the game's own)");
    mt.place("D9 OKU 2", 0x00705702, b'S', 2, 0, x, "");
    mt.place("then D7 Matches 133 (uses the wait up)", 0x007055D4, b'S', 133, 0, x, "$13,300");
    mt.place("then D9 MAN 99: not 2 million more", 0x0070577F, b'S', 99, 0, x, "$9,900");
    // an OKU on another thread: must never reach this one's MAN
    let got = std::thread::scope(|t| t.spawn(|| mt_place(log, 0x00705702, b'S', 5, 0, x)).join());
    mt.check("another thread: OKU 5", &got.unwrap_or_default(), "");
    mt.place("D9 MAN 99 here, after the other thread's OKU", 0x0070577F, b'S', 99, 0, x, "$9,900");
    // D2 / D4, as their start / count-up / final functions call them: [the 100-million box], then the box beside it
    mt.place("D2 fan event final: OKU 5", 0x00533C02, b'S', 5, 0, x, "");
    mt.place("then MAN 4319 (the fan event screenshot)", 0x00533C6C, b'S', 4319, 0, x, "$5,431,900");
    mt.place("D2 counting up: OKU 2", 0x00533B29, b'S', 2, 0, x, "");
    mt.place("then ZERO", 0x00533B41, b'S', 0, 0, empty, "$2,000,000");
    mt.place("D2 start: MAN 9500, no 100-million part", 0x00533803, b'S', 9500, 0, x, "$950,000");
    mt.place("D4 golden age counting up: OKU 1", 0x00538A7F, b'S', 1, 0, x, "");
    mt.place("then MAN 2990", 0x00538ABD, b'S', 2990, 0, x, "$1,299,000");
    mt.place("D4 start: OKU 3", 0x0053862D, b'S', 3, 0, x, "");
    mt.place("then ZERO", 0x00538645, b'S', 0, 0, empty, "$3,000,000");
    mt.place("D2 start: OKU 3", 0x005337AD, b'S', 3, 0, x, "");
    mt.place("then D4 final MAN 100: not D2's 3 million", 0x00538BFC, b'S', 100, 0, x, "$10,000");
    mt.place("D4 final ZERO, nothing waiting (a total of 0)", 0x00538BA9, b'S', 0, 0, empty, "(the game's own)");
    // the pieces displays, as FUN_00576ca0 and the others call them: [the 100-million piece], then the rest's piece
    // or the literal "0000万" (no call), then the join
    mt.place("D17 Prize Money: OKU 3", 0x00576FF3, b'P', 3, 0, x, "");
    mt.place("then MAN 8174", 0x0057705A, b'P', 8174, 0, x, "$3,817,400");
    mt.place("then the join", 0x005770B8, b'S', 0, 0, ss, "(the game's own)");
    mt.place("D17 Prize Money: OKU 3 again", 0x00576FF3, b'P', 3, 0, x, "");
    mt.place("then the join with \"0000\" instead of MAN", 0x005770B8, b'S', 0, 0, ss, "$3,000,000");
    mt.place("D17 Annual Salary: MAN 2309, no OKU", 0x0057766E, b'P', 2309, 0, x, "$230,900");
    mt.place("then its join", 0x005776CE, b'S', 0, 0, ss, "(the game's own)");
    mt.place("D13 coach room: OKU 1 (FUN_00429750)", 0x00552C4D, b'Q', 1, 0, x, "");
    mt.place("then MAN 2500", 0x00552CB5, b'Q', 2500, 0, x, "$1,250,000");
    mt.place("then its join", 0x00552D18, b'S', 0, 0, ss, "(the game's own)");
    mt.place("D16 match offer: MAN 0, no OKU", 0x005583FE, b'Q', 0, 0, x, "$0");
    mt.place("D17 Prize Money: OKU 3, once more", 0x00576FF3, b'P', 3, 0, x, "");
    mt.place("then Annual Salary's join: another display", 0x005776CE, b'S', 0, 0, ss, "(the game's own)");
    mt.place("then Prize Money's join: the wait was used up", 0x005770B8, b'S', 0, 0, ss, "(the game's own)");
    mt.place("a piece place reached through sprintf_s", 0x00552C4D, b'P', 1, 0, x, "(the game's own)");
    // the one-call places
    mt.place("D7 Match Prize FULL2 2, 2990", 0x0070558C, b'S', 2, 2990, x, "$2,299,000");
    mt.place("D8 Titles FULL 0", 0x0070567D, b'S', 0, 0, x, "$0");
    mt.place("D8 Club Income, its other format: FULLOKU 2", 0x0070567D, b'S', 2, 0, ENGFMT, "$2,000,000");
    mt.place("D10 kick-off home FULL2 3, 7072", 0x007079D5, b'S', 3, 7072, x, "$3,707,200");
    mt.place("D10 kick-off away FULL 7500", 0x00707CB5, b'S', 7500, 0, x, "$750,000");
    mt.place("D5 its other format: FULL 150", 0x00538716, b'P', 150, 0, 0x00A2_7240, "$15,000");
    mt.place("projector prize FULL2 10, 0", 0x0060345D, b'P', 10, 0, x, "$10,000,000");
    mt.place("projector prize FULL 7500", 0x006034A0, b'P', 7500, 0, x, "$750,000");
    mt.place("a projector place reached as a box text", 0x0060345D, b'S', 10, 0, x, "(the game's own)");
    mt.place("a box place reached through sprintf_s", 0x0070558C, b'P', 2, 2990, x, "(the game's own)");
    mt.place("a place not in the table", 0x00401000, b'S', 1, 2, x, "(the game's own)");
}

/// The two helpers' code from their first byte to the return of their vsprintf_s call, as read 2026-10-06: the place
/// is found at ap - 8 (FUN_006d7da0) and ap - 0xC (FUN_00429750) only while these bytes are these.
const SETTEXT_HEAD: [u8; 0x35] = [
    0x81, 0xEC, 0x08, 0x04, 0x00, 0x00, 0xA1, 0x08, 0x32, 0xAE, 0x00, 0x33, 0xC4, 0x89, 0x84, 0x24,
    0x04, 0x04, 0x00, 0x00, 0x8B, 0x8C, 0x24, 0x0C, 0x04, 0x00, 0x00, 0x57, 0x8D, 0x84, 0x24, 0x14,
    0x04, 0x00, 0x00, 0x50, 0x51, 0x8D, 0x54, 0x24, 0x0C, 0x68, 0x01, 0x04, 0x00, 0x00, 0x52, 0xFF,
    0x15, 0xA8, 0xE6, 0x97, 0x00,
];
const P429_HEAD: [u8; 0x19] = [
    0x8B, 0x4C, 0x24, 0x08, 0x53, 0x8D, 0x44, 0x24, 0x10, 0x50, 0x8B, 0x44, 0x24, 0x0C, 0x51, 0x52,
    0x50, 0xB3, 0x01, 0xFF, 0x15, 0xA8, 0xE6, 0x97, 0x00,
];
// the C checks these at run time; they are constants
const _: () = assert!(0x006D_7DA0 + SETTEXT_HEAD.len() == MONEY_SETTEXT_RET);
const _: () = assert!(0x0042_9750 + P429_HEAD.len() == MONEY_P429_RET);

/// `len` bytes at `at` are committed and readable (the C reads them under `__try`).
fn readable(at: usize, len: usize) -> bool {
    let mut a = at;
    while a < at + len {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: a query about our own process's memory
        if unsafe {
            VirtualQuery(
                Some(a as *const c_void),
                &mut mbi,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
            || mbi.State != MEM_COMMIT
            || mbi.Protect.0 & (PAGE_NOACCESS.0 | PAGE_GUARD.0) != 0
        {
            return false;
        }
        a = mbi.BaseAddress as usize + mbi.RegionSize;
    }
    true
}

/// `len` bytes of the program at `at`, None when they cannot be read.
fn bytes(at: usize, len: usize) -> Option<&'static [u8]> {
    // SAFETY: checked readable; the program's code and data stay mapped while it runs
    readable(at, len).then(|| unsafe { std::slice::from_raw_parts(at as *const u8, len) })
}

/// The C's place and head checks: the count of places / helpers not as studied; None = a read failed.
fn money_checks(log: fn(&str)) -> Option<u32> {
    const SPR_CALL: [u8; 6] = [0xFF, 0x15, 0x98, 0xE6, 0x97, 0x00];
    if bytes(ENGFMT, 12)? != b"$%d,000,000\0" {
        log("money: off - English is not on (0x00A62914 is not the kit's \"$%d,000,000\")");
        return Some(u32::MAX);
    }
    let mut bad = 0;
    for s in &MSITES {
        let p = s.ret as usize;
        let b = bytes(p - 6, 6)?;
        let to = (p as u32).wrapping_add(u32::from_le_bytes([b[2], b[3], b[4], b[5]])); // where an E8 call before p goes
        let ok = match s.kind {
            b'S' => b[1] == 0xE8 && to == 0x006D_7DA0,
            b'Q' => b[1] == 0xE8 && to == 0x0042_9750,
            _ => b == SPR_CALL || (b[4] == 0xFF && b[5] == 0xD7),
        };
        if !ok {
            log(&format!(
                "money: place 0x{:08X} ({}) is not the call studied",
                s.ret, s.what
            ));
            bad += 1;
        }
    }
    if bytes(0x006D_7DA0, SETTEXT_HEAD.len())? != SETTEXT_HEAD {
        log("money: FUN_006d7da0 is not the code studied");
        bad += 1;
    }
    if bytes(0x0042_9750, P429_HEAD.len())? != P429_HEAD {
        log("money: FUN_00429750 is not the code studied");
        bad += 1;
    }
    Some(bad)
}

/// The C's `money_install`: amounts with commas, after [`fmt_guard_install`] in seat 1 (box texts go through it).
/// Runs [`money_selftest`] first when WCCFPANEL_MONEYTEST is set.
pub fn money_install(e1718: bool, projector: bool, log: fn(&str)) {
    let _ = LOG.set(log);
    if std::env::var_os("WCCFPANEL_MONEYTEST").is_some_and(|v| !v.is_empty()) {
        money_selftest(e1718, log);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let nm = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !nm.eq_ignore_ascii_case("client_Release.exe") {
        log(&format!("money: off - not the game ({nm})"));
        return;
    }
    // SAFETY: plain queries
    let me = unsafe { GetModuleHandleA(None) }.map_or(0, |m| m.0 as usize);
    // SAFETY: as above
    let crt = unsafe { GetModuleHandleA(s!("msvcr90.dll")) }.ok();
    let real = crt.map_or(0, |c| proc(c, s!("sprintf_s")));
    if me != 0x0040_0000 || crt.is_none() || real == 0 {
        log(&format!(
            "money: off - the program is not where the table was made for (base {me:08X}, msvcr90 {:08X}, sprintf_s {real:08X})",
            crt.map_or(0, |c| c.0 as usize)
        ));
        return;
    }
    match money_checks(log) {
        None => return log("money: off - the program's memory is not as studied (a read failed)"),
        Some(u32::MAX) => return, // English is not on: said
        Some(0) => {}
        Some(bad) => {
            return log(&format!(
                "money: off - {bad} place(s) not as studied: not the program the table was made for"
            ));
        }
    }
    // SAFETY: the program sits where the table was made for (checked above)
    let slot = unsafe { iat_slot(me, "msvcr90.dll", "sprintf_s") };
    let vs = crt.map_or(0, |c| proc(c, s!("vsprintf_s")));
    MSVCR_VSPRINTF_S.store(vs, Relaxed);
    // SAFETY: the slot found in the program's import table
    let Some(slot) =
        slot.filter(|&s| s as usize == 0x0097_E698 && unsafe { s.read() } == real && vs != 0)
    else {
        log(&format!(
            "money: off - the program's sprintf_s import is not as studied (slot {:08X})",
            slot.map_or(0, |s| s as usize)
        ));
        return;
    };
    MONEY_ON.store(true, Relaxed); // before the slot changes: our sprintf_s must find it on
    // SAFETY: the program's sprintf_s import slot, checked above
    if let Err(e) = unsafe { write_slot(slot, money_sprintf_s as *const () as usize) } {
        MONEY_ON.store(false, Relaxed);
        log(&format!(
            "money: off - the sprintf_s import could not be opened (error {e})"
        ));
        return;
    }
    let how = if projector {
        "the projector: its 4 places"
    } else if FMT_GUARD_ON.load(Relaxed) {
        "seat 1: box texts through the fmt guard, pieces through sprintf_s"
    } else {
        "seat 1: pieces only - the fmt guard is not installed, so no box texts"
    };
    log(&format!(
        "money: on - {} places checked, amounts with commas ({how})",
        MSITES.len()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn collect(s: &str) {
        LINES.lock().unwrap().push(s.into());
    }

    /// Every self-test case: money_text, money_value and every role through money_place, the other thread's OKU too.
    #[test]
    fn selftest_passes() {
        money_selftest(false, collect);
        let lines = LINES.lock().unwrap();
        let fails: Vec<_> = lines
            .iter()
            .filter(|l| l.starts_with("money test: FAIL"))
            .collect();
        assert!(fails.is_empty(), "{fails:#?}");
        assert_eq!(lines.last().unwrap(), "money test: 75 checks, 0 failed");
    }

    /// The two thunks hand the game's arguments over in the right order.
    #[test]
    fn thunks_pass_the_arguments() {
        static SEEN: Mutex<Vec<usize>> = Mutex::new(Vec::new());
        unsafe extern "C" fn fake(buf: *mut u8, size: usize, fmt: *const u8, ap: *const u8) -> i32 {
            // SAFETY: the test's two ints
            let a = unsafe { [*(ap as *const i32), *(ap as *const i32).add(1)] };
            *SEEN.lock().unwrap() = vec![
                buf as usize,
                size,
                fmt as usize,
                a[0] as usize,
                a[1] as usize,
            ];
            42
        }
        MSVCR_VSPRINTF_S.store(fake as *const () as usize, Relaxed);
        REAL_VSPRINTF_S.store(fake as *const () as usize, Relaxed);
        let mut b = [0u8; 8];
        let fmt = c"%d%d".as_ptr() as *const u8;
        let want = vec![b.as_mut_ptr() as usize, 8, fmt as usize, 7, 9];
        // SAFETY: the thunk is cdecl sprintf_s
        let spr = unsafe {
            std::mem::transmute::<
                unsafe extern "C" fn() -> i32,
                unsafe extern "C" fn(*mut u8, usize, *const u8, ...) -> i32,
            >(money_sprintf_s)
        };
        // SAFETY: a box of 8 and two ints, as the fake reads
        assert_eq!(unsafe { spr(b.as_mut_ptr(), 8, fmt, 7i32, 9i32) }, 42);
        assert_eq!(*SEEN.lock().unwrap(), want);
        let args = [7i32, 9];
        // SAFETY: the thunk is cdecl vsprintf_s
        let vspr = unsafe {
            std::mem::transmute::<unsafe extern "C" fn() -> i32, VsprintfS>(safe_vsprintf_s)
        };
        // SAFETY: as above, with a va_list
        let r = unsafe { vspr(b.as_mut_ptr(), 8, fmt, args.as_ptr().cast()) };
        assert_eq!(r, 42);
        assert_eq!(*SEEN.lock().unwrap(), want);
    }
}
