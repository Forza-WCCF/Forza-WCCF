//! The C panel's three small game patches, both editions (`native/2010-11/panel/wccfpanel.c`,
//! `native/2017-18/overlay/wccfpanel.c`): the frame limiter with its frame stats, the card dispenser fix, and FREE PLAY
//! (2010-11 only). Every change checks the program's name, its base where the C does, and the original bytes before
//! it writes; anything else leaves the game as it is. The C reads the game's memory under `__try`; here
//! [`mem_ok`] (VirtualQuery) refuses memory that would fault, with the same outcome.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicIsize, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed,
};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_NOACCESS, PAGE_PROTECTION_FLAGS, PAGE_READWRITE, PAGE_WRITECOPY,
    VirtualProtect, VirtualQuery,
};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, GetCurrentProcess, GetCurrentThreadId, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::s;

// ---------------------------------------------------------------- shared helpers

/// The program's file name (the C's `strrchr(exe, '\\')` on `GetModuleFileNameA(NULL)`).
pub(crate) fn exe_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

pub(crate) fn is_client(name: &str) -> bool {
    name.eq_ignore_ascii_case("client_Release.exe")
}

/// The program's base address (`GetModuleHandleA(NULL)`).
pub(crate) fn exe_base() -> usize {
    // SAFETY: GetModuleHandleW(NULL) only reads the process's own module list
    unsafe { GetModuleHandleW(None) }.map_or(0, |h| h.0 as usize)
}

/// `len` bytes at `at` are committed and readable (`write`: and writable), what the C's `__try` would not have faulted on.
/// The panel reads and writes its own process directly once this says yes (2026-10-29: Read/WriteProcessMemory
/// imports in a DLL loaded into a game are what antivirus heuristics flag).
pub(crate) fn mem_ok(at: usize, len: usize, write: bool) -> bool {
    let Some(end) = at.checked_add(len) else {
        return false;
    };
    let mut a = at;
    while a < end {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: a query about our own process's memory
        if unsafe { VirtualQuery(Some(a as *const c_void), &mut mbi, size_of_val(&mbi)) } == 0
            || mbi.State != MEM_COMMIT
            || mbi.Protect.0 & (PAGE_NOACCESS.0 | PAGE_GUARD.0) != 0
            || (write
                && (mbi.Protect
                    & (PAGE_READWRITE
                        | PAGE_WRITECOPY
                        | PAGE_EXECUTE_READWRITE
                        | PAGE_EXECUTE_WRITECOPY))
                    .0
                    == 0)
        {
            return false;
        }
        a = mbi.BaseAddress as usize + mbi.RegionSize;
    }
    true
}

/// `N` bytes at `at`, or `None` where the C's `__try` would have caught a fault.
pub(crate) fn read<const N: usize>(at: usize) -> Option<[u8; N]> {
    // SAFETY: checked committed and readable; the game's image and heap pages stay mapped while it runs
    mem_ok(at, N, false).then(|| unsafe { std::ptr::read_unaligned(at as *const [u8; N]) })
}

pub(crate) fn read_u32(at: usize) -> Option<u32> {
    read::<4>(at).map(u32::from_le_bytes)
}

/// Data written into the game, `false` where the C's `__try` would have caught a fault.
fn write_data(at: usize, bytes: &[u8]) -> bool {
    if !mem_ok(at, bytes.len(), true) {
        return false;
    }
    // SAFETY: checked committed and writable
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len()) };
    true
}

/// Code bytes written as the C does: VirtualProtect, copy, protection back, FlushInstructionCache. `false`: the page
/// could not be opened, nothing written.
pub(crate) fn patch_code(at: usize, bytes: &[u8]) -> bool {
    let mut old = PAGE_PROTECTION_FLAGS(0);
    // SAFETY: the caller checked the original bytes at `at`; the page is made writable for the copy only
    unsafe {
        if VirtualProtect(
            at as *const c_void,
            bytes.len(),
            PAGE_EXECUTE_READWRITE,
            &mut old,
        )
        .is_err()
        {
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        let _ = VirtualProtect(at as *const c_void, bytes.len(), old, &mut old);
        let _ = FlushInstructionCache(GetCurrentProcess(), Some(at as *const c_void), bytes.len());
    }
    true
}

/// The C's "every 2 s" (`next`, set to now on the first look, then now + 2000): true when this tick acts.
fn every_2s(next: &Mutex<Option<Instant>>) -> bool {
    let now = Instant::now();
    let Ok(mut n) = next.lock() else { return false };
    if n.is_some_and(|t| now < t) {
        return false;
    }
    *n = Some(now + Duration::from_secs(2));
    true
}

// ---------------------------------------------------------------- the frame wait (limiter_*)
// The original frame limiter FUN_0075fcd0 spins on the clock FUN_00720540 (microseconds) until the frame's due time,
// held in esi: a whole core per game window. Its loop's one call goes to limiter_now: when the frame is due in more than
// 3 ms it first sleeps on a high-resolution waitable timer until 2 ms before, then returns the clock as FUN_00720540
// does; the loop's own compares keep the timing. "limiter_off" beside the DLL switches it off within a second,
// "frame_stats" logs seat 1's frame timing every 10 s, WCCFPANEL_LIMITER=0 keeps it out.

/// "call FUN_00720540" inside the spin loop, 16-byte aligned.
const LIMITER_CALL: usize = 0x0075_FD90;
const LIMITER_CLOCK: usize = 0x0072_0540;
/// call clock; cmp eax,edi; ja back; cmp eax,esi; jb back; jmp; imul
const LIMITER_LOOP: [u8; 16] = [
    0xE8, 0xAB, 0x07, 0xFC, 0xFF, 0x3B, 0xC7, 0x77, 0xF7, 0x3B, 0xC6, 0x72, 0xF3, 0xEB, 0x0D, 0x69,
];

/// The clock `limiter_wait` reads (`g_lclock`): the game's, or the self-test's stand-in.
static CLOCK: AtomicUsize = AtomicUsize::new(LIMITER_CLOCK);
static TIMER: AtomicIsize = AtomicIsize::new(0);
/// The one thread that sleeps (the game's main loop).
static LTID: AtomicU32 = AtomicU32::new(0);
/// The loop's call goes to limiter_now.
static LIMITER_IN: AtomicBool = AtomicBool::new(false);
static LIMITER_ON: AtomicBool = AtomicBool::new(false);
static STATS_ON: AtomicBool = AtomicBool::new(false);
/// Sleeps, and sleeps that woke after the frame was due.
static LSLEEPS: AtomicI32 = AtomicI32::new(0);
static LLATE: AtomicI32 = AtomicI32::new(0);
/// Microseconds between frame waits: count, sum, longest, over 45 ms.
static LGAPS: AtomicI32 = AtomicI32::new(0);
static LGAP_SUM: AtomicI32 = AtomicI32::new(0);
static LGAP_MAX: AtomicI32 = AtomicI32::new(0);
static LHITCH: AtomicI32 = AtomicI32::new(0);
/// The clock when the last frame's sleep ended (the waiting thread).
static LLAST: AtomicU32 = AtomicU32::new(0);

/// The timer's relative due time (100 ns, negative) for a frame due at `due` with the clock at `now`, or `None`: no
/// sleep. Unsigned, as the C: a past or wrapped due time is huge and does not sleep.
fn sleep_100ns(due: u32, now: u32) -> Option<i64> {
    let left = due.wrapping_sub(now);
    (left > 3000 && left < 200_000).then(|| -i64::from(left - 2000) * 10)
}

/// The C's `limiter_wait`: sleep until 2 ms before `due` (the game's main thread only), then the clock.
extern "C" fn limiter_wait(due: u32) -> u32 {
    // SAFETY: CLOCK holds a cdecl `unsigned int (void)`: the game's FUN_00720540 or the self-test's lt_clock
    let clock: extern "C" fn() -> u32 = unsafe { std::mem::transmute(CLOCK.load(Relaxed)) };
    let mut now = clock();
    let timer = HANDLE(TIMER.load(Relaxed) as *mut c_void);
    let Some(t) = sleep_100ns(due, now) else {
        return now;
    };
    if !LIMITER_ON.load(Relaxed) || timer.0.is_null() {
        return now;
    }
    // SAFETY: plain thread id query
    let me = unsafe { GetCurrentThreadId() };
    let _ = LTID.compare_exchange(0, me, Relaxed, Relaxed);
    if me != LTID.load(Relaxed) {
        return now;
    }
    // SAFETY: our own timer handle and a due time on the stack
    let woke = unsafe {
        SetWaitableTimer(timer, &t, 0, None, None, false).is_ok()
            && WaitForSingleObject(timer, 300) == WAIT_OBJECT_0
    };
    now = clock();
    if woke {
        LSLEEPS.fetch_add(1, Relaxed);
        if (now.wrapping_sub(due) as i32) > 0 {
            LLATE.fetch_add(1, Relaxed);
        }
        let last = LLAST.load(Relaxed);
        if last != 0 {
            // frames are 33.3 ms apart; a late frame skips its wait: ~67 ms
            let gap = now.wrapping_sub(last) as i32;
            if gap > 0 && gap < 2_000_000 {
                LGAPS.fetch_add(1, Relaxed);
                LGAP_SUM.fetch_add(gap, Relaxed);
                LGAP_MAX.fetch_max(gap, Relaxed);
                if gap > 45_000 {
                    LHITCH.fetch_add(1, Relaxed);
                }
            }
        }
        LLAST.store(now, Relaxed);
    }
    now
}

/// The C's `limiter_now`: replaces "call FUN_00720540" in the loop; esi holds the due time, eax gets the clock, as the
/// loop expects.
// SAFETY: a cdecl call to limiter_wait with esi pushed as its argument; the stack is balanced before ret
#[unsafe(naked)]
extern "C" fn limiter_now() {
    core::arch::naked_asm!("push esi", "call {wait}", "add esp, 4", "ret", wait = sym limiter_wait);
}

/// The 8 bytes at LIMITER_CALL with the call's target moved to `to` (the E8 and the bytes after the call stay).
fn call_patch(old8: u64, at: usize, to: usize) -> u64 {
    let mut b = old8.to_le_bytes();
    b[1..5].copy_from_slice(&(to.wrapping_sub(at + 5) as u32).to_le_bytes());
    u64::from_le_bytes(b)
}

/// "CreateWaitableTimerExA" looked up at run time, as the C, with CREATE_WAITABLE_TIMER_HIGH_RESOLUTION (Windows 10
/// 1803 or later); `None` without one.
fn hires_timer() -> Option<HANDLE> {
    type Cte = unsafe extern "system" fn(*const c_void, *const u8, u32, u32) -> HANDLE;
    // SAFETY: kernel32 is always loaded; the export has the Cte signature (SECURITY_ATTRIBUTES*, LPCSTR, DWORD, DWORD)
    unsafe {
        let k = GetModuleHandleA(s!("kernel32.dll")).ok()?;
        let f = GetProcAddress(k, s!("CreateWaitableTimerExA"))?;
        let cte: Cte = std::mem::transmute(f);
        let h = cte(
            std::ptr::null(),
            std::ptr::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS.0,
        );
        (!h.0.is_null()).then_some(h)
    }
}

/// The watcher's line every 10 s (frame_stats on).
fn limiter_stats_line(sleeps: i32, late: i32, n: i32, sum: i32, mx: i32, hitch: i32) -> String {
    let avg = if n != 0 {
        f64::from(sum) / 1000.0 / f64::from(n)
    } else {
        0.0
    };
    format!(
        "limiter: {sleeps} sleeps in 10 s, {late} woke after the frame was due; frames {avg:.1} ms apart on average, \
         the longest {:.1} ms, {hitch} over 45 ms",
        f64::from(mx) / 1000.0
    )
}

/// The C's `limiter_watch`: the two switch files once a second, the sleep counts every 10 s.
fn limiter_watch(dir: PathBuf, log: fn(&str)) {
    let off = dir.join("limiter_off");
    let stats = dir.join("frame_stats");
    let mut next = Instant::now() + Duration::from_secs(10);
    let (mut s0, mut l0) = (0i32, 0i32);
    loop {
        let on = !off.exists();
        let st = stats.exists();
        if LIMITER_ON.swap(on, Relaxed) != on && LIMITER_IN.load(Relaxed) {
            log(if on {
                "limiter: ON - the frame wait sleeps until 2 ms before each frame"
            } else {
                "limiter: OFF (limiter_off beside the DLL) - the original spin, as before"
            });
        }
        if STATS_ON.swap(st, Relaxed) != st {
            log(if st {
                "frame stats on (frame_stats beside the DLL)"
            } else {
                "frame stats off"
            });
        }
        if st && LIMITER_IN.load(Relaxed) && Instant::now() >= next {
            let (s, l) = (LSLEEPS.load(Relaxed), LLATE.load(Relaxed));
            let (n, sum) = (LGAPS.swap(0, Relaxed), LGAP_SUM.swap(0, Relaxed));
            let (mx, h) = (LGAP_MAX.swap(0, Relaxed), LHITCH.swap(0, Relaxed));
            log(&limiter_stats_line(
                s.wrapping_sub(s0),
                l.wrapping_sub(l0),
                n,
                sum,
                mx,
                h,
            ));
            (s0, l0) = (s, l);
            next = Instant::now() + Duration::from_secs(10);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// frame_stats_tick's state between frames (the C's statics; `None`: `was` = 0).
struct FrameStats {
    last: Instant,
    t0: Instant,
    n: i32,
    late: i32,
    sum: f64,
    mx: f64,
}
static FRAMES: Mutex<Option<FrameStats>> = Mutex::new(None);

fn frames_line(n: i32, secs: f64, sum: f64, mx: f64, late: i32, sleeps: bool) -> String {
    format!(
        "frames: {n} in {secs:.1} s = {:.2} a second; {:.1} ms apart on average, the longest {mx:.1} ms, {late} over \
         45 ms; the frame wait {}",
        f64::from(n) / secs,
        sum / f64::from(n),
        if sleeps {
            "sleeps"
        } else {
            "spins (the original)"
        }
    )
}

/// The C's `frame_stats_tick`: seat 1's render thread, every game frame (hook_present); a line every 10 s while
/// "frame_stats" is beside the DLL.
pub fn frame_stats_tick(log: fn(&str)) {
    let Ok(mut g) = FRAMES.lock() else { return };
    if !STATS_ON.load(Relaxed) {
        *g = None;
        return;
    }
    let now = Instant::now();
    let Some(s) = g.as_mut() else {
        *g = Some(FrameStats {
            last: now,
            t0: now,
            n: 0,
            late: 0,
            sum: 0.0,
            mx: 0.0,
        });
        return;
    };
    let ms = (now - s.last).as_secs_f64() * 1000.0;
    s.last = now;
    s.n += 1;
    s.sum += ms;
    s.mx = s.mx.max(ms);
    if ms > 45.0 {
        s.late += 1;
    }
    let secs = (now - s.t0).as_secs_f64();
    if secs >= 10.0 {
        let sleeps = LIMITER_IN.load(Relaxed) && LIMITER_ON.load(Relaxed);
        log(&frames_line(s.n, secs, s.sum, s.mx, s.late, sleeps));
        (s.n, s.late, s.sum, s.mx, s.t0) = (0, 0, 0.0, 0.0, now);
    }
}

/// The C's `limiter_install`. Both editions' C has the same code and addresses; the 2017-18 program moves (ASLR), so
/// there its base is not 0x00400000 and this logs "not the game".
fn limiter_install(dll_dir: &Path, log: fn(&str)) {
    if std::env::var_os("WCCFPANEL_LIMITER")
        .is_some_and(|v| v.as_encoded_bytes().first() == Some(&b'0'))
    {
        log("limiter: kept out (WCCFPANEL_LIMITER=0) - the original spin");
        return;
    }
    let nm = exe_name();
    if !is_client(&nm) || exe_base() != 0x0040_0000 {
        log(&format!("limiter: not the game ({nm}) - nothing changed"));
        return;
    }
    match read::<16>(LIMITER_CALL) {
        None => {
            log("limiter: the frame wait could not be read - the original spin stays");
            return;
        }
        Some(b) if b != LIMITER_LOOP => {
            log("limiter: the frame wait is not the code studied - the original spin stays");
            return;
        }
        Some(_) => {}
    }
    let Some(timer) = hires_timer() else {
        log(
            "limiter: no high-resolution timer here (Windows 10 1803 or later has one) - the original spin stays",
        );
        return;
    };
    TIMER.store(timer.0 as isize, Relaxed);
    let p = LIMITER_CALL as *mut u64;
    let mut prot = PAGE_PROTECTION_FLAGS(0);
    // SAFETY: the 8 code bytes just checked, made writable for the exchange
    if unsafe { VirtualProtect(p as *const c_void, 8, PAGE_EXECUTE_READWRITE, &mut prot) }.is_err()
    {
        // SAFETY: plain last-error query
        let err = unsafe { GetLastError() }.0;
        log(&format!(
            "limiter: the code could not be opened (error {err}) - the original spin stays"
        ));
        return;
    }
    // SAFETY: 0x0075FD90 is 8-byte aligned, readable (checked above) and writable now; cmpxchg8b swaps the call
    // atomically under a running game loop, as the C's InterlockedCompareExchange64
    let swapped = unsafe {
        let old8 = std::ptr::read_volatile(p);
        LIMITER_ON.store(!dll_dir.join("limiter_off").exists(), Relaxed); // on unless limiter_off is there
        let new8 = call_patch(old8, LIMITER_CALL, limiter_now as *const () as usize);
        AtomicU64::from_ptr(p)
            .compare_exchange(old8, new8, Relaxed, Relaxed)
            .is_ok()
    };
    // SAFETY: the protection read above, put back; the instruction cache flushed over the bytes changed
    unsafe {
        let _ = VirtualProtect(p as *const c_void, 8, prot, &mut prot);
        if swapped {
            let _ = FlushInstructionCache(GetCurrentProcess(), Some(p as *const c_void), 8);
        }
    }
    if !swapped {
        log("limiter: the code changed while being patched - the original spin stays");
        return;
    }
    LIMITER_IN.store(true, Relaxed);
    log(&format!(
        "limiter: the frame wait (0x{LIMITER_CALL:08X}) now sleeps until 2 ms before each frame; the original loop \
         keeps the timing{}",
        if LIMITER_ON.load(Relaxed) {
            ""
        } else {
            " - but limiter_off is there: OFF for now"
        }
    ));
}

/// The C's `limiter_start`, both roles (seat and projector): the self-test when WCCFPANEL_LIMITERTEST is set, the
/// change, the two switch files. The limiter is the same in both editions (`e1718` changes nothing; see
/// [`limiter_install`]).
pub fn limiter_start(e1718: bool, dll_dir: &Path, log: fn(&str)) {
    if std::env::var_os("WCCFPANEL_LIMITERTEST").is_some_and(|v| !v.is_empty()) {
        limiter_selftest(log);
    }
    limiter_install(dll_dir, log);
    if e1718 && crate::is_projector() {
        prj_gate_install(log); // here because both roles call limiter_start and lib.rs is not ours to change
    }
    if e1718 && is_client(&exe_name()) {
        crate::probe::install(log); // WCCF_PROBE_GAMEREADY=1 only; projector and seat
    }
    let dir = dll_dir.to_path_buf();
    // ponytail: the watcher runs for the process's life, as the C's; the real game never unloads the panel. Only in
    // the game: fakegame's FreeLibrary would leave it running in freed code (its switch files serve the game alone)
    let game = std::env::current_exe().ok().is_some_and(|e| {
        e.file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("client_Release.exe"))
    });
    if game {
        std::thread::spawn(move || limiter_watch(dir, log));
    }
}

// WCCFPANEL_LIMITERTEST=1 (the tests): limiter_wait and limiter_now with a stand-in clock (QueryPerformanceCounter in
// microseconds), on a high-resolution timer of their own; nothing of the game is touched.

/// The C's `lt_clock`.
extern "C" fn lt_clock() -> u32 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    // ponytail: Instant (QueryPerformanceCounter) since the first call; the C's c * 1000000 / f overflows after
    // ~10 days up, this wraps after 71 minutes, as the game's own microsecond clock does
    START.get_or_init(Instant::now).elapsed().as_micros() as u32
}

/// The C's `lt_call_now`: calls limiter_now the way the loop does, the due time in esi.
fn lt_call_now(due: u32) -> u32 {
    let r: u32;
    // SAFETY: esi is saved and restored around the call; limiter_now is cdecl with its argument in esi and its result
    // in eax; the C ABI's scratch registers are declared clobbered
    unsafe {
        core::arch::asm!(
            "push esi",
            "mov esi, {due}",
            "call {now}",
            "pop esi",
            due = in(reg) due,
            now = sym limiter_now,
            lateout("eax") r,
            clobber_abi("C"),
        );
    }
    r
}

/// The C's `limiter_selftest`; its lines go to `log`.
fn limiter_selftest(log: fn(&str)) {
    let pf = |ok: bool| if ok { "PASS" } else { "FAIL" };
    CLOCK.store(lt_clock as *const () as usize, Relaxed);
    let timer = hires_timer();
    log(&format!(
        "limiter test: high-resolution timer {}",
        if timer.is_some() {
            "made"
        } else {
            "NOT available"
        }
    ));
    let Some(timer) = timer else {
        CLOCK.store(LIMITER_CLOCK, Relaxed);
        return;
    };
    TIMER.store(timer.0 as isize, Relaxed);
    LIMITER_ON.store(true, Relaxed);
    LTID.store(0, Relaxed);
    let (mut worst, mut ok_all) = (0i32, true);
    for _ in 0..20 {
        // 20 frames 20 ms ahead: it must return 1.0-3.0 ms early
        let due = lt_clock().wrapping_add(20_000);
        let early = due.wrapping_sub(lt_call_now(due));
        if (early as i32) < 1000 || early > 3000 {
            ok_all = false;
        }
        worst = worst.max(2000i32.wrapping_sub(early as i32));
    }
    log(&format!(
        "limiter test: {}  20 frames 20 ms ahead returned 1.0-3.0 ms before the due time (latest wake {worst} us past \
         the 2 ms mark)",
        pf(ok_all)
    ));
    let s0 = LSLEEPS.load(Relaxed);
    let no_sleep = || LSLEEPS.load(Relaxed) == s0;
    let due = lt_clock().wrapping_add(2500);
    let got = lt_call_now(due);
    log(&format!(
        "limiter test: {}  due in 2.5 ms: no sleep, the clock at once",
        pf(no_sleep() && due.wrapping_sub(got) as i32 > 2000)
    ));
    lt_call_now(lt_clock().wrapping_sub(5000));
    log(&format!(
        "limiter test: {}  due 5 ms ago (late frame): no sleep",
        pf(no_sleep())
    ));
    lt_call_now(lt_clock().wrapping_add(400_000));
    log(&format!(
        "limiter test: {}  due 400 ms ahead (not a frame): no sleep",
        pf(no_sleep())
    ));
    let other = std::thread::spawn(|| {
        let due = lt_clock().wrapping_add(20_000);
        due.wrapping_sub(limiter_wait(due)) // how long before the due time it returned
    })
    .join()
    .unwrap_or(0);
    log(&format!(
        "limiter test: {}  another thread: no sleep (returned {other} us before its due time)",
        pf(no_sleep() && other > 15_000)
    ));
    LIMITER_ON.store(false, Relaxed);
    let due = lt_clock().wrapping_add(20_000);
    let got = lt_call_now(due);
    log(&format!(
        "limiter test: {}  switched off: no sleep",
        pf(no_sleep() && due.wrapping_sub(got) as i32 > 15_000)
    ));
    // SAFETY: the timer made above, no longer in TIMER once closed
    unsafe {
        TIMER.store(0, Relaxed);
        let _ = CloseHandle(timer);
    }
    CLOCK.store(LIMITER_CLOCK, Relaxed);
    LTID.store(0, Relaxed);
}

// ---------------------------------------------------------------- the card dispenser that is not there
// Every player card earned adds one to an "owed" count (FUN_004cf650) that only a real dispenser brings down, so seat 1
// kept saying "You are owed N Player Card(s)". 2010-11: FUN_004cf650 = ret, and the count kept in backup memory is set
// to 0 every 2 s; the count is at *( *(0x009FBCAC) + 8 ) + 5. 2017-18: the same function but its addresses is
// FUN_005c0970 (exe base + 0x1C0970, the exe moves; its global is read from its own mov); its four callers are the
// game's card hand-outs, and it jumps to owed_hit instead: the card is not owed, and counted for the deal window.

const OWED_SIG: [u8; 16] = [
    0x55, 0x8B, 0xEC, 0x83, 0xE4, 0xF8, 0xA1, 0xAC, 0xBC, 0x9F, 0x00, 0x8B, 0x40, 0x08, 0x56, 0x57,
];
const OWED_2010: usize = 0x004C_F650;
const OWED_2017_RVA: usize = 0x001C_0970;
static OWED_FIX: AtomicBool = AtomicBool::new(false);
/// The count is at *( *OWED_AT + 8 ) + 5.
static OWED_AT: AtomicUsize = AtomicUsize::new(0x009F_BCAC);
/// 2017-18: player cards the game handed out since the last [`take_owed_new`].
static OWED_NEW: AtomicU32 = AtomicU32::new(0);
static OWED_NEXT: Mutex<Option<Instant>> = Mutex::new(None);
static OWED_SAID: AtomicU32 = AtomicU32::new(0);

/// The C's `owed_code_at` on bytes already read: the function studied (`any_global`: its global, bytes 7-10, not
/// compared).
fn owed_sig_ok(code: &[u8; 16], any_global: bool) -> bool {
    code[..7] == OWED_SIG[..7]
        && (any_global || code[7..11] == OWED_SIG[7..11])
        && code[11..] == OWED_SIG[11..]
}

/// The C's `owed_hit` (2017-18): FUN_005c0970 jumps here; cdecl, no arguments, nothing returned, as the `ret` it
/// stands for.
extern "C" fn owed_hit() {
    OWED_NEW.fetch_add(1, Relaxed);
}

/// 2017-18: cards handed out since the last call (the deal window); always 0 on 2010-11.
pub fn take_owed_new() -> u32 {
    OWED_NEW.swap(0, Relaxed)
}

/// The C's `dispenser_install`, each edition's.
pub fn dispenser_install(e1718: bool, log: fn(&str)) {
    let nm = exe_name();
    if !is_client(&nm) {
        log(&format!("dispenser: not the game ({nm}) - nothing changed"));
        return;
    }
    if !e1718 {
        if read::<16>(OWED_2010) != Some(OWED_SIG) {
            log("dispenser: FUN_004cf650 is not the code studied - nothing changed");
            return;
        }
        if !patch_code(OWED_2010, &[0xC3]) {
            log("dispenser: code not writable - nothing changed");
            return;
        }
        OWED_FIX.store(true, Relaxed);
        log("dispenser: FUN_004cf650 patched (ret) - no player card is owed from now on");
        return;
    }
    // 2017-18's C: the 2010-11 place first, then FUN_005c0970
    let mut f = OWED_2010;
    let mut y1718 = false;
    if !read::<16>(f).is_some_and(|c| owed_sig_ok(&c, false)) {
        f = exe_base() + OWED_2017_RVA;
        let Some(code) = read::<16>(f).filter(|c| owed_sig_ok(c, true)) else {
            log("dispenser: FUN_004cf650 / FUN_005c0970 is not the code studied - nothing changed");
            return;
        };
        OWED_AT.store(
            u32::from_le_bytes([code[7], code[8], code[9], code[10]]) as usize,
            Relaxed,
        );
        y1718 = true;
    }
    let ok = if y1718 {
        patch_code(f, &crate::jmp(f, owed_hit as *const () as usize)) // jmp owed_hit: counted, not owed
    } else {
        patch_code(f, &[0xC3]) // ret: a card it cannot hand out is no longer counted
    };
    if !ok {
        log("dispenser: code not writable - nothing changed");
        return;
    }
    OWED_FIX.store(true, Relaxed);
    if y1718 {
        log(&format!(
            "dispenser: FUN_005c0970 at {f:08X} -> owed_hit - each player card handed out is shown, none owed"
        ));
    } else {
        log("dispenser: FUN_004cf650 patched (ret) - no player card is owed from now on");
    }
}

/// The C's `dispenser_tick`: each frame; every 2 s the owed count back to 0.
pub fn dispenser_tick(log: fn(&str)) {
    if !OWED_FIX.load(Relaxed) || !every_2s(&OWED_NEXT) {
        return;
    }
    let reached = (|| {
        let p = read_u32(OWED_AT.load(Relaxed))? as usize;
        if p == 0 {
            return Some(());
        }
        let q = read_u32(p + 8)? as usize;
        if q == 0 {
            return Some(());
        }
        let n = read_u32(q + 5)? as i32;
        if n != 0 {
            if OWED_SAID.load(Relaxed) < 5 {
                log(&format!("dispenser: {n} player card(s) owed -> 0"));
                OWED_SAID.fetch_add(1, Relaxed);
            }
            write_data(q + 5, &0u32.to_le_bytes()).then_some(())?;
        }
        Some(())
    })();
    if reached.is_none() {
        OWED_FIX.store(false, Relaxed);
        log("dispenser: the owed count could not be reached - stopped");
    }
}

// ---------------------------------------------------------------- free play
// The original FREE PLAY: byte 2 of the credit library's coin setting. At 1 every credit check passes and no credit is
// taken. The library clears its block when it starts and then sets its "credits up" word to 1: from then on the byte is
// set here, and again every 2 s. Only if the code that reads it is the one studied. The key driver's coin on START stays
// as the fallback. 2010-11 (fixed addresses): check FUN_0082BFE0, word 0x00CD0F90, byte 0x00CD0F9A. 2017-18 (the
// program is relocated, so RVAs): check 0x00D381A0 (sub_D381A0), word 0x0654B178, byte 0x0654B182. (A code stub that
// stored the byte in the exe broke the relocated 2017-18 client: the stub's absolute operand and jump were not in the
// relocation table.)

const FREE_SIG: [u8; 19] = [
    0x33, 0xC0, 0x39, 0x05, 0x90, 0x0F, 0xCD, 0x00, 0x75, 0x03, 0xC2, 0x04, 0x00, 0x38, 0x05, 0x9A,
    0x0F, 0xCD, 0x00,
];
pub(crate) const BASE_1718: usize = 0x0040_0000;
/// (check, credits-up word, free-play byte) as virtual addresses at the preferred base, per edition.
const FREE_2010: [usize; 3] = [0x0082_BFE0, 0x00CD_0F90, 0x00CD_0F9A];
const FREE_1718: [usize; 3] = [0x00D3_81A0, 0x0654_B178, 0x0654_B182];
static CREDITS_UP: AtomicUsize = AtomicUsize::new(0);
static FREE_BYTE: AtomicUsize = AtomicUsize::new(0);
static FREE_PLAY: AtomicBool = AtomicBool::new(false);
static FREE_NEXT: Mutex<Option<Instant>> = Mutex::new(None);
static FREE_SAID: AtomicU32 = AtomicU32::new(0);

/// Is `code` the credit check studied, with its two operands at `credits` and `byte`? (The operands are the only
/// bytes that move with the base.)
fn free_sig_ok(code: &[u8; 19], credits: usize, byte: usize) -> bool {
    code[..4] == FREE_SIG[..4]
        && code[8..15] == FREE_SIG[8..15]
        && u32::from_le_bytes([code[4], code[5], code[6], code[7]]) as usize == credits
        && u32::from_le_bytes([code[15], code[16], code[17], code[18]]) as usize == byte
}

// The red "2D-code reader not connected" notice (top right, no USB camera here). It is a row of the game's notice
// table: the notice shows while that row's condition function returns true. 2017-18's condition (sub_5F4A20, RVA
// 0x001F4A20 from the preferred base) is `cmp byte [flag], 0 / setz al / ret` on the "reader found" flag. Only that table
// row calls it, so `xor eax, eax / ret` hides the notice and leaves the flag, and so the reader logic, as it was. Written
// in memory after the loader's relocation (an exe patch over the flag's absolute operand would be rewritten by it).
const QR_COND: usize = 0x005F_4A20;
const QR_FLAG: usize = 0x0157_29A6;

/// Is `code` the notice's condition, comparing the flag at `flag`? (The operand is the only byte run that moves.)
fn qr_cond_ok(code: &[u8; 11], flag: usize) -> bool {
    code[..2] == [0x80, 0x3D]
        && code[6..] == [0x00, 0x0F, 0x94, 0xC0, 0xC3]
        && u32::from_le_bytes([code[2], code[3], code[4], code[5]]) as usize == flag
}

/// 2017-18 only: no red "2D-code reader not connected" notice. Called by `free_play_install` (the game process only).
fn qr_notice_hide(log: fn(&str)) {
    let base = exe_base();
    let at = QR_COND - BASE_1718 + base;
    if !read::<11>(at).is_some_and(|c| qr_cond_ok(&c, QR_FLAG - BASE_1718 + base)) {
        log("qr notice: the condition is not the code studied - nothing changed");
    } else if !patch_code(at, &[0x33, 0xC0, 0xC3]) {
        log("qr notice: code not writable - nothing changed");
    } else {
        log(&format!(
            "qr notice: sub_5F4A20 at {at:08X} -> xor eax,eax / ret - the \"2D-code reader not connected\" notice stays hidden"
        ));
    }
}

// The projector's "Now Synchronizing with the Control" screen. After its warning movie, PrjWakeupWarning's update
// (sub_88B1B0) moves on to the schedule (Prj Advertise) only while the control is in ENTRY (state 1) with more than
// 20 s of entry left; the control's first ENTRY is over before the projector looks, and the next one is a whole round
// (up to ~20 min) away. At 0x0088B2B3 eax = the projector scheduler (sub_84FEF0), edi = the control's state byte. The
// new code goes to the schedule (0x0088B2FE, the original "pass" branch) always; outside ENTRY it first forgets the
// last entry time (scheduler +0x60/+0x64 = -1, as Advertise's own init does), else the schedule (sub_883E90) would read
// a stale 0 and restart itself every 2 s. In ENTRY it keeps the live time, so the round goes on as the original does:
// the schedule waits for the next entry end, then Before Game and the live match. Only relative calls and jumps in the
// checked bytes, so nothing here moves with the base (the loader's fix-ups nearest are at 0x0088B2CB and 0x0088B2DF).
const PRJ_GATE: usize = 0x0088_B2A9;
/// 0x0088B2A9: call sub_84FEF0 / mov ecx,[eax] / movsx edi,byte [ecx] / cmp edi,1 / jnz / push eax / call sub_850210
/// / test edx,edx / jg pass / jl wait / cmp eax,14h / ja pass.
const PRJ_GATE_SIG: [u8; 32] = [
    0xE8, 0x42, 0x4C, 0xFC, 0xFF, 0x8B, 0x08, 0x0F, 0xBE, 0x39, 0x83, 0xFF, 0x01, 0x75, 0x11, 0x50,
    0xE8, 0x52, 0x4F, 0xFC, 0xFF, 0x85, 0xD2, 0x7F, 0x3C, 0x7C, 0x05, 0x83, 0xF8, 0x14, 0x77, 0x35,
];
/// At 0x0088B2B3 (sig byte 10): cmp edi,1 / je pass / mov dword [eax+60h],-1 / mov dword [eax+64h],-1 / jmp pass / nop.
const PRJ_GATE_NEW: [u8; 22] = [
    0x83, 0xFF, 0x01, 0x74, 0x46, 0xC7, 0x40, 0x60, 0xFF, 0xFF, 0xFF, 0xFF, 0xC7, 0x40, 0x64, 0xFF,
    0xFF, 0xFF, 0xFF, 0xEB, 0x36, 0x90,
];
/// 0x0088B2FE, the "pass" branch both jumps land on: push 0 / push esi / call (sub_439200, the next sequence).
const PRJ_PASS_SIG: [u8; 4] = [0x6A, 0x00, 0x56, 0xE8];

/// 2017-18 projector only: it leaves "Now Synchronizing with the Control" on its first look. Called from
/// `limiter_start`.
fn prj_gate_install(log: fn(&str)) {
    let at = PRJ_GATE - BASE_1718 + exe_base();
    if read::<32>(at) != Some(PRJ_GATE_SIG) || read::<4>(at + 0x55) != Some(PRJ_PASS_SIG) {
        log(
            "projector gate: not the code studied - nothing changed (it waits for an entry window)",
        );
    } else if !patch_code(at + 10, &PRJ_GATE_NEW) {
        log("projector gate: code not writable - nothing changed (it waits for an entry window)");
    } else {
        log(&format!(
            "projector gate: sub_88B1B0 at {:08X} -> the schedule after the warning, whatever the control's state - no \"Synchronizing with the Control\" wait",
            at + 10
        ));
    }
}

// The card-in mode select (TUTORIAL / NORMAL, Scene_ADV_NetModeSelect). Its setup sub_77BE90 sets the scene's byte
// +0x118 when the Aime holds no club in COACH_LIST 272 (sub_5BF060 == 0); the original then keeps the cursor on TUTORIAL
// (sub_77C8D0) and lays "Not available" over NORMAL (sub_77D3C0), so a brand-new card must start with the tutorial, by
// design: a server can only lift it by listing a club. Two jumps change, the byte stays: back still leads to the
// attract screen without a club. NORMAL then makes a club the regular way (sub_77CB10: card state 3, Club Make); the
// tutorial keeps state 4. Only relative jumps in the checked bytes, so nothing moves with the base.
const MODE_CURSOR: usize = 0x0077_C8F4;
/// 0x0077C8F4: cmp byte [edi+118h],0 / mov eax,[edi+104h] / mov [edi+108h],eax / jnz 0x0077C993 (skip the left/right
/// read). The jnz (sig byte 19) becomes 6 nops.
const MODE_CURSOR_SIG: [u8; 25] = [
    0x80, 0xBF, 0x18, 0x01, 0x00, 0x00, 0x00, 0x8B, 0x87, 0x04, 0x01, 0x00, 0x00, 0x89, 0x87, 0x08,
    0x01, 0x00, 0x00, 0x0F, 0x85, 0x86, 0x00, 0x00, 0x00,
];
const MODE_LOCK: usize = 0x0077_D555;
/// 0x0077D555: cmp byte [ebx+118h],0 / jz show / cmp edi,1 / jnz show (the black "Not available" layer for item 1). The
/// jz (sig byte 7) becomes jmp: the card always shows.
const MODE_LOCK_SIG: [u8; 14] = [
    0x80, 0xBB, 0x18, 0x01, 0x00, 0x00, 0x00, 0x74, 0x34, 0x83, 0xFF, 0x01, 0x75, 0x2F,
];

/// 2017-18 only: NORMAL selectable on the mode select for a card without a club. Called by `free_play_install`.
fn normal_mode_unlock(log: fn(&str)) {
    let base = exe_base();
    let (cursor, lock) = (MODE_CURSOR - BASE_1718 + base, MODE_LOCK - BASE_1718 + base);
    if read::<25>(cursor) != Some(MODE_CURSOR_SIG) || read::<14>(lock) != Some(MODE_LOCK_SIG) {
        log(
            "normal mode: not the code studied - nothing changed (a new card plays the tutorial first)",
        );
    } else if !patch_code(cursor + 19, &[0x90; 6]) || !patch_code(lock + 7, &[0xEB]) {
        log("normal mode: code not writable - NORMAL may stay locked for a new card");
    } else {
        log(&format!(
            "normal mode: sub_77C8D0 at {:08X} and sub_77D3C0 at {:08X} -> NORMAL selectable for a card without a club",
            cursor + 19,
            lock + 7
        ));
    }
}

/// The C's `free_play_install`: 2010-11 and 2017-18 each at their own addresses.
pub fn free_play_install(e1718: bool, log: fn(&str)) {
    let nm = exe_name();
    if !is_client(&nm) {
        log(&format!("free play: not the game ({nm}) - nothing changed"));
        return;
    }
    if e1718 {
        qr_notice_hide(log);
        normal_mode_unlock(log);
    }
    let [check, credits, byte] = if e1718 {
        let base = exe_base();
        FREE_1718.map(|va| va - BASE_1718 + base)
    } else {
        FREE_2010
    };
    if !read::<19>(check).is_some_and(|c| free_sig_ok(&c, credits, byte)) {
        log(
            "free play: the credit check is not the code studied - nothing changed (a coin is still put in on START)",
        );
        return;
    }
    let mut mi = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: VirtualQuery fills our own structure
    let got = unsafe { VirtualQuery(Some(credits as *const c_void), &mut mi, size_of_val(&mi)) };
    if got == 0
        || mi.State != MEM_COMMIT
        || (mi.Protect & (PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE)).0 == 0
    {
        log("free play: the credit setting is not writable memory - nothing changed");
        return;
    }
    CREDITS_UP.store(credits, Relaxed);
    FREE_BYTE.store(byte, Relaxed);
    FREE_PLAY.store(true, Relaxed);
    log("free play: on - set as soon as the game's credits start, then every 2 s");
}

/// The C's `free_play_tick`: each frame; every 2 s FREE PLAY set once the game's credits run.
pub fn free_play_tick(log: fn(&str)) {
    if !FREE_PLAY.load(Relaxed) || !every_2s(&FREE_NEXT) {
        return;
    }
    let (credits, byte) = (CREDITS_UP.load(Relaxed), FREE_BYTE.load(Relaxed));
    let reached = (|| {
        if read_u32(credits)? == 1 && read::<1>(byte)?[0] != 1 {
            write_data(byte, &[1]).then_some(())?;
            if FREE_SAID.load(Relaxed) < 3 {
                log("free play: the game's credits are running - FREE PLAY set");
                FREE_SAID.fetch_add(1, Relaxed);
            }
        }
        Some(())
    })();
    if reached.is_none() {
        FREE_PLAY.store(false, Relaxed);
        log("free play: the credit setting could not be reached - stopped");
    }
}

// ---------------------------------------------------------------- VS stall tolerance (2017-18)
// An online VS checks the opponent's (frame, engine RNG value) samples against its own last 90 (sub_4B41E0 ->
// sub_4B4690). A peer sample inside our window but missing from our history counts down the session's +0x2564 from 30;
// at 0 the opponent is lost (sub_4B1F90, flag 0x200000) and the CPU takes over. A matched or ahead-of-us sample loads 30
// again. The count moves only when peer packets arrive: during a link stall the frame blocks instead (sub_4C4640's
// Sleep(1) loop, up to TIMEOUT_P2P 7 x 500 turns, then its own TIMEOUT(ON) drop), and the late packets after it can use
// up 30 at once. 90 (the window's own length) lets a whole window of samples go unchecked. A value mismatch (flag
// 0x2000) still drops at once. The four `mov dword [reg+2564h], 1Eh` are the counter's only writes besides its `dec`.

/// The `mov dword [ebp|esi+2564h], 1Eh` sites: the session reset, the "ahead" and "matched" reloads, the history reset.
const STALL_SITES: [(usize, u8); 4] = [
    (0x004B_2A5C, 0x85),
    (0x004B_445B, 0x85),
    (0x004B_446A, 0x85),
    (0x004B_475A, 0x86),
];
/// The client studied (SHA-256 8b55456f... before setup): its PE header's TimeDateStamp and SizeOfImage.
const CLIENT_STAMP: (u32, u32) = (0x5AAA_3F0A, 0x06C8_E000);
/// Peer samples that may go unmatched in a row (the original 30).
pub(crate) const STALL_SAMPLES: u32 = 90;

/// Is `code` the original `mov dword [<modrm>+2564h], 30`?
fn stall_site_ok(code: &[u8; 10], modrm: u8) -> bool {
    *code == [0xC7, modrm, 0x64, 0x25, 0x00, 0x00, 0x1E, 0x00, 0x00, 0x00]
}

/// The program's TimeDateStamp and SizeOfImage from its PE header at `base`.
fn pe_stamp(base: usize) -> Option<(u32, u32)> {
    let pe = base + read_u32(base + 0x3C)? as usize;
    Some((read_u32(pe + 8)?, read_u32(pe + 0x50)?))
}

/// The image at `base`: every site checked first, then the four counts set to [`STALL_SAMPLES`]. One log line.
fn stall_install_at(base: usize, log: fn(&str)) -> bool {
    let at = |va: usize| va - BASE_1718 + base;
    if pe_stamp(base) != Some(CLIENT_STAMP) {
        log(
            "vs stall: not the client studied (PE stamp) - nothing changed (30 missing peer frames drop a VS)",
        );
        return false;
    }
    if let Some((va, _)) = STALL_SITES
        .iter()
        .find(|&&(va, m)| !read::<10>(at(va)).is_some_and(|c| stall_site_ok(&c, m)))
    {
        log(&format!(
            "vs stall: {va:08X} is not the code studied - nothing changed (30 missing peer frames drop a VS)"
        ));
        return false;
    }
    let n = STALL_SITES
        .iter()
        .filter(|&&(va, _)| patch_code(at(va) + 6, &STALL_SAMPLES.to_le_bytes()))
        .count();
    if n < STALL_SITES.len() {
        log(&format!(
            "vs stall: code not writable - {n} of 4 sites set to {STALL_SAMPLES}"
        ));
        return false;
    }
    log(&format!(
        "vs stall: sub_4B41E0's missing-peer-frame limit 30 -> {STALL_SAMPLES} at 4 sites - a short network stall no longer drops a VS"
    ));
    true
}

/// 2017-18 client only: a VS tolerates a short network stall. Called from the panel's start.
pub fn stall_tolerance_install(e1718: bool, log: fn(&str)) {
    let nm = exe_name();
    if e1718 && is_client(&nm) {
        stall_install_at(exe_base(), log);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_patch_checks_then_writes() {
        // a fake image: the PE header and the four sites at their offsets from the preferred base
        let mut img = vec![0u8; 0x0B_5000];
        img[0x3C..0x40].copy_from_slice(&0x100u32.to_le_bytes());
        img[0x108..0x10C].copy_from_slice(&CLIENT_STAMP.0.to_le_bytes());
        img[0x150..0x154].copy_from_slice(&CLIENT_STAMP.1.to_le_bytes());
        let orig = |m| [0xC7, m, 0x64, 0x25, 0x00, 0x00, 0x1E, 0x00, 0x00, 0x00];
        for (va, m) in STALL_SITES {
            img[va - BASE_1718..][..10].copy_from_slice(&orig(m));
        }
        let base = img.as_mut_ptr() as usize;
        let site = |img: &[u8], i: usize| img[STALL_SITES[i].0 - BASE_1718..][..10].to_vec();
        // one site off: nothing written anywhere
        img[STALL_SITES[3].0 - BASE_1718 + 1] = 0x85;
        assert!(!stall_install_at(base, |_| {}));
        assert_eq!(site(&img, 0), orig(0x85));
        img[STALL_SITES[3].0 - BASE_1718 + 1] = 0x86;
        // another build: nothing written
        img[0x108] ^= 1;
        assert!(!stall_install_at(base, |_| {}));
        assert_eq!(site(&img, 1), orig(0x85));
        img[0x108] ^= 1;
        // the client studied: all four take 90, the rest of each instruction stays
        assert!(stall_install_at(base, |_| {}));
        for (i, (_, m)) in STALL_SITES.into_iter().enumerate() {
            let mut want = orig(m);
            want[6] = 90;
            assert_eq!(site(&img, i), want, "site {i}");
        }
        // a second run finds 90, not 30: refused, unchanged
        assert!(!stall_install_at(base, |_| {}));
        assert_eq!(site(&img, 2)[6], 90);
    }

    #[test]
    fn owed_signature_per_edition() {
        assert!(owed_sig_ok(&OWED_SIG, false));
        let mut y1718 = OWED_SIG;
        y1718[7..11].copy_from_slice(&0x0129_30F4u32.to_le_bytes()); // 2017-18's global
        assert!(!owed_sig_ok(&y1718, false));
        assert!(owed_sig_ok(&y1718, true));
        for i in (0..7).chain(11..16) {
            let mut bad = OWED_SIG;
            bad[i] ^= 0xFF;
            assert!(!owed_sig_ok(&bad, true), "byte {i}");
        }
    }

    #[test]
    fn projector_gate_lands_on_pass() {
        // the original's "jg pass" (sig byte 23) and the new je/jmp all reach the same pass branch, sig offset 0x55
        let jump = |code: &[u8], at: usize| at + 2 + code[at + 1] as usize;
        assert_eq!(jump(&PRJ_GATE_SIG, 23), 0x55);
        let mut new = PRJ_GATE_SIG;
        new[10..].copy_from_slice(&PRJ_GATE_NEW);
        assert_eq!((new[13], jump(&new, 13)), (0x74, 0x55)); // in ENTRY: straight on
        assert_eq!((new[29], jump(&new, 29)), (0xEB, 0x55)); // outside: after the two -1s
        assert_eq!(new[15..19], [0xC7, 0x40, 0x60, 0xFF]); // scheduler +0x60 (entry seconds) and +0x64 (its high half)
        assert_eq!(new[22..26], [0xC7, 0x40, 0x64, 0xFF]);
        assert_eq!(new[..10], PRJ_GATE_SIG[..10]); // the call and the state read stay
    }

    #[test]
    fn memory_checks_read_and_refuse() {
        let buf = FREE_SIG;
        assert_eq!(read::<19>(buf.as_ptr() as usize), Some(FREE_SIG));
        assert!(free_sig_ok(&FREE_SIG, FREE_2010[1], FREE_2010[2]));
        let mut y1718 = FREE_SIG; // 2017-18, relocated by 0x10000
        y1718[4..8].copy_from_slice(&(FREE_1718[1] as u32 + 0x1_0000).to_le_bytes());
        y1718[15..19].copy_from_slice(&(FREE_1718[2] as u32 + 0x1_0000).to_le_bytes());
        assert!(free_sig_ok(
            &y1718,
            FREE_1718[1] + 0x1_0000,
            FREE_1718[2] + 0x1_0000
        ));
        assert!(!free_sig_ok(&y1718, FREE_1718[1], FREE_1718[2])); // the base moved: operands must follow
        y1718[0] ^= 1;
        assert!(!free_sig_ok(
            &y1718,
            FREE_1718[1] + 0x1_0000,
            FREE_1718[2] + 0x1_0000
        ));
        assert_eq!(read::<4>(0x10), None); // unmapped: refused, not a fault
        let mut word = 7u32;
        assert!(write_data(&raw mut word as usize, &0u32.to_le_bytes()));
        assert_eq!(word, 0);
        assert!(!write_data(0x10, &[1]));
    }

    #[test]
    fn normal_mode_unlock_jumps() {
        // the cursor's jnz is the 6 bytes patched, rel32 0x86 from 0x0077C90D = the skip at 0x0077C993
        assert_eq!(MODE_CURSOR_SIG[19..21], [0x0F, 0x85]);
        assert_eq!(MODE_CURSOR + 25 + 0x86, 0x0077_C993);
        // the lock's jz becomes a jmp to the same place: the card shown, not its black layer
        assert_eq!(MODE_LOCK_SIG[7], 0x74);
        let mut new = MODE_LOCK_SIG;
        new[7] = 0xEB;
        assert_eq!((new[7], new[8]), (0xEB, MODE_LOCK_SIG[8]));
        // both read the scene's no-club byte +0x118
        assert_eq!(MODE_CURSOR_SIG[2..4], [0x18, 0x01]);
        assert_eq!(MODE_LOCK_SIG[2..4], [0x18, 0x01]);
    }

    #[test]
    fn qr_condition_signature() {
        let mut code = [0x80, 0x3D, 0, 0, 0, 0, 0x00, 0x0F, 0x94, 0xC0, 0xC3];
        code[2..6].copy_from_slice(&(QR_FLAG as u32 + 0x1_0000).to_le_bytes()); // relocated by 0x10000
        assert!(qr_cond_ok(&code, QR_FLAG + 0x1_0000));
        assert!(!qr_cond_ok(&code, QR_FLAG)); // the base moved: the operand must follow
        code[7] ^= 1;
        assert!(!qr_cond_ok(&code, QR_FLAG + 0x1_0000));
    }

    #[test]
    fn limiter_call_patch_moves_only_the_target() {
        let old8 = u64::from_le_bytes(LIMITER_LOOP[..8].try_into().unwrap());
        assert_eq!(call_patch(old8, LIMITER_CALL, LIMITER_CLOCK), old8); // the original call, rebuilt
        let new8 = call_patch(old8, LIMITER_CALL, 0x1000_0000).to_le_bytes();
        assert_eq!(new8[0], 0xE8);
        assert_eq!(new8[5..], LIMITER_LOOP[5..8]);
        assert_eq!(
            i32::from_le_bytes(new8[1..5].try_into().unwrap()),
            0x1000_0000 - 0x0075_FD95
        );
    }

    #[test]
    fn wait_arithmetic() {
        assert_eq!(sleep_100ns(1_020_000, 1_000_000), Some(-180_000)); // 20 ms ahead: sleep 18 ms
        assert_eq!(sleep_100ns(1_002_500, 1_000_000), None); // 2.5 ms: spin
        assert_eq!(sleep_100ns(995_000, 1_000_000), None); // late
        assert_eq!(sleep_100ns(1_400_000, 1_000_000), None); // not a frame
        assert_eq!(sleep_100ns(10_000, u32::MAX - 9_999), Some(-180_000)); // across the clock's wrap
    }

    #[test]
    fn stats_lines() {
        assert_eq!(
            frames_line(300, 10.02, 10_010.0, 51.25, 2, true),
            "frames: 300 in 10.0 s = 29.94 a second; 33.4 ms apart on average, the longest 51.2 ms, 2 over 45 ms; \
             the frame wait sleeps"
        );
        assert!(
            frames_line(1, 10.0, 10.0, 10.0, 0, false)
                .ends_with("the frame wait spins (the original)")
        );
        assert_eq!(
            limiter_stats_line(299, 1, 298, 9_933_400, 67_100, 1),
            "limiter: 299 sleeps in 10 s, 1 woke after the frame was due; frames 33.3 ms apart on average, the \
             longest 67.1 ms, 1 over 45 ms"
        );
        assert_eq!(
            limiter_stats_line(0, 0, 0, 0, 0, 0),
            "limiter: 0 sleeps in 10 s, 0 woke after the frame was due; frames 0.0 ms apart on average, the longest \
             0.0 ms, 0 over 45 ms"
        );
    }

    /// The C's WCCFPANEL_LIMITERTEST self-test: milliseconds on a real timer, so a busy machine fails it. Run it with
    /// `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn limiter_selftest_lines() {
        static FAILS: AtomicU32 = AtomicU32::new(0);
        limiter_selftest(|l| {
            println!("{l}");
            if l.contains("FAIL") || l.contains("NOT available") {
                FAILS.fetch_add(1, Relaxed);
            }
        });
        assert_eq!(FAILS.load(Relaxed), 0);
    }
}
