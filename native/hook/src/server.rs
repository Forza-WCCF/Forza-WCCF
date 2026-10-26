//! The 2010-11 server's hook: the `winmm.dll` of the original control and its match engines on a Linux server under Wine
//! (`crates/linux-host`). Beside what every hook does it loads `netguard.dll` as the second guard, and fixes three of
//! control's faults:
//!
//! | Fix | What | Off |
//! |---|---|---|
//! | MSGPARSE guard | catches control's known fault in its message parser (`FUN_00412860`, a negative copy size from a seat's carried-over header count), unwinds that call with its length and offset zeroed, and drops that count | `MXHOOK_MSGGUARD=0` |
//! | SLEEP fix | control's `Sleep` calls that do not block under Wine become timed waits; its file reader's idle poll waits `MXHOOK_IDLE_MS` (default 4) | `MXHOOK_SLEEPFIX=0` |
//! | SPIN fix | control's DistributePic thread spinning on `timeGetTime` (a full core idle); with it, about 6 % | `MXHOOK_SPINFIX=0` |
//!
//! The fixes turn on only in `control_Release.exe`; `=1` forces one on in another program (the self-tests
//! `msgguard_test`, `sleepfix_test`, `spinfix_test` in native/programs, run under Wine with
//! `WINEDLLOVERRIDES="winmm,winmm_orig,netguard=n"`: docs/SELF-HOSTING.md). Control's log says `MSGPARSE guard: on`,
//! `SLEEP fix: on`, `SPIN fix: on`.

use crate::fixes;
use crate::log::log;
use crate::patch::{Real, exe, exe_is, patch};
use forza_native::By;
use std::ffi::c_void;
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicUsize, Ordering::Relaxed,
};
use windows_sys::Win32::Foundation::{CloseHandle, EXCEPTION_ACCESS_VIOLATION, GetLastError};
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS,
};
use windows_sys::Win32::System::LibraryLoader::LoadLibraryA;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::Threading::{
    CreateEventA, ExitProcess, GetCurrentThreadId, GetCurrentThreadStackLimits, Sleep,
    WaitForSingleObject,
};

const CONTROL: &str = "control_Release.exe";

/// The original secondary guard, in every process including children; the pinned control imports wccfnet.dll as the
/// primary one. Without it the process ends.
pub fn load_netguard() {
    // SAFETY: a NUL-terminated name
    if unsafe { LoadLibraryA(c"netguard.dll".as_ptr().cast()) }.is_null() {
        log!("REFUSED: secondary netguard.dll did not load");
        // SAFETY: ends this process: it must not run without the second guard
        unsafe { ExitProcess(126) };
    }
}

/// The three fixes, in the order the log names them.
pub fn install() {
    install_msgguard();
    install_sleepfix();
    install_spinfix();
}

fn env_is(name: &str, v: &str) -> bool {
    std::env::var(name).is_ok_and(|x| x == v)
}

/// The `n` bytes of the program at `off`.
fn exe_bytes(off: usize, n: usize) -> &'static [u8] {
    // SAFETY: inside control's loaded image (its offsets are checked only in control_Release.exe)
    unsafe { std::slice::from_raw_parts(exe().add(off), n) }
}

// ---- MSGPARSE guard ----------------------------------------------------------------------------------------------
// control's message-reassembly crash guard: the 5.4 launcher's recovery of FUN_00412860, moved in-process (a vectored
// handler instead of its Windows debugger, which does not run here). control's FUN_00412860 trusts a wire length/offset
// and faults far outside its buffer (a read at control+0x129C2 or a write in msvcr90 memcpy from control+0x12A3D). On
// any first-chance access violation, find FUN_00412860's return into FUN_00412140 (control+0x1226A) on the stack; if
// its live frame is there, unwind it: restore the ebx/ebp/esi/edi it pushed, zero *len and *off so control drops the
// bad chunk, and return 1 ("consumed") to the caller. ASLR is off in control. MXHOOK_MSGGUARD_RET (hex offset) is for
// the self-test only.

const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
static MSG_RET: AtomicU32 = AtomicU32::new(0);
static MSG_SAVED: AtomicI32 = AtomicI32::new(0);
static MSG_ASIDE: AtomicI32 = AtomicI32::new(0);

fn install_msgguard() {
    let mut ret = fixes::msgguard_ret(std::env::var("MXHOOK_MSGGUARD_RET").ok().as_deref());
    if ret == 0 {
        if !exe_is(CONTROL) {
            return;
        }
        ret = 0x1226A;
    }
    if env_is("MXHOOK_MSGGUARD", "0") {
        log!("MSGPARSE guard: OFF (MXHOOK_MSGGUARD=0)");
        return;
    }
    MSG_RET.store((exe() as u32).wrapping_add(ret), Relaxed);
    // SAFETY: a handler for the process's life
    let h = unsafe { AddVectoredExceptionHandler(1, Some(msgguard)) };
    log!(
        "MSGPARSE guard: {}",
        if h.is_null() { "NOT installed" } else { "on" }
    );
}

/// VirtualQuery, not IsBadWritePtr: no nested fault in the handler.
fn writable(p: *const c_void, n: usize) -> bool {
    // SAFETY: only queries the address
    unsafe {
        let mut m: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        VirtualQuery(p, &mut m, size_of::<MEMORY_BASIC_INFORMATION>()) != 0
            && m.State == MEM_COMMIT
            && m.Protect
                & (PAGE_READWRITE
                    | PAGE_WRITECOPY
                    | PAGE_EXECUTE_READWRITE
                    | PAGE_EXECUTE_WRITECOPY)
                != 0
            && p as usize + n <= m.BaseAddress as usize + m.RegionSize
    }
}

unsafe extern "system" fn msgguard(ep: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: the system passes the fault's record and the thread's context
    let (rec, c) = unsafe { (&*(*ep).ExceptionRecord, &mut *(*ep).ContextRecord) };
    let (mut low, mut top) = (0, 0);
    // SAFETY: two out-parameters
    unsafe { GetCurrentThreadStackLimits(&mut low, &mut top) };
    if rec.ExceptionCode != EXCEPTION_ACCESS_VIOLATION {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let esp = c.Esp;
    let n = top.saturating_sub(esp as usize).min(fixes::STACK_SCAN) / 4;
    let mut words = [0u32; fixes::STACK_SCAN / 4];
    for (i, w) in words[..n].iter_mut().enumerate() {
        // SAFETY: the faulting thread's stack, from esp up to its top
        *w = unsafe { (esp as usize as *const u32).add(i).read_unaligned() };
    }
    let words = &words[..n];
    let rw = if rec.ExceptionInformation[0] != 0 {
        "write"
    } else {
        "read"
    };
    let at = rec.ExceptionInformation[1] as u32;
    let ret = MSG_RET.load(Relaxed);
    if let Some(off) = fixes::find_frame(esp, words, ret) {
        let i = off / 4;
        // &off is the live frame's (find_frame), &len the word below it; FUN_00412860 pushed edi, esi (the seat), ebp
        // and ebx (FUN_00412140's this) under its return
        let p_off = words[i + 3] as usize as *mut u32;
        let p_len = p_off.wrapping_sub(1);
        let (this, seat) = (words[i - 1] as usize as *mut u32, words[i - 3]);
        // SAFETY: p_len and p_off are FUN_00412140's locals on this thread's stack (find_frame matched them)
        unsafe {
            log!(
                "MSGPARSE guard: access violation at eip 0x{:08X} ({rw} 0x{at:08X}) inside FUN_00412860 - message dropped (len {}, off {}), control continues [{}]",
                c.Eip,
                p_len.read(),
                p_off.read(),
                MSG_SAVED.fetch_add(1, Relaxed) + 1
            );
            p_len.write(0);
            p_off.write(0);
        }
        // the seat's carried-over header count (this+0x44) with no buffer left (this+0x48) is what faulted (a count of
        // 44571 gave memmove a negative size, 2026-10-19); kept, it would fault again on the next message
        let field = this.wrapping_add(17);
        if seat < 64 && writable(field.cast(), 8) {
            // SAFETY: this+0x44 and +0x48 are writable (checked); each array slot is checked before it is touched
            unsafe {
                let count = (field.read() as usize as *mut u32).wrapping_add(seat as usize);
                let pend = (field.add(1).read() as usize as *mut u32).wrapping_add(seat as usize);
                if writable(count.cast(), 4)
                    && writable(pend.cast(), 4)
                    && pend.read() == 0
                    && count.read() != 0
                {
                    log!(
                        "MSGPARSE guard: seat {seat}'s carried-over header count {} dropped",
                        count.read()
                    );
                    count.write(0);
                }
            }
        }
        c.Edi = words[i - 4];
        c.Esi = words[i - 3];
        c.Ebp = words[i - 2];
        c.Ebx = words[i - 1];
        c.Eip = ret;
        c.Esp = esp.wrapping_add(off as u32 + 0x10);
        c.Eax = 1;
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    // the program's own handlers may still take it
    if MSG_ASIDE.fetch_add(1, Relaxed) < 20 {
        log!(
            "MSGPARSE guard stepped aside: access violation at eip 0x{:08X} ({rw} 0x{at:08X}), no FUN_00412860 frame in the {} stack bytes above esp 0x{esp:08X}; ecx={:08X} edx={:08X} esi={:08X} edi={:08X}",
            c.Eip,
            fixes::STACK_SCAN,
            c.Ecx,
            c.Edx,
            c.Esi,
            c.Edi
        );
        for (off, v) in fixes::returns(words) {
            log!("  stack esp+0x{off:03X}: 0x{v:08X}");
        }
    }
    EXCEPTION_CONTINUE_SEARCH
}

// ---- SLEEP fix ---------------------------------------------------------------------------------------------------
// control's Sleep loops under Wine on Box64 (2026-10-17, research/control-spin). Only two control threads loop on
// Sleep() for good: the main thread (frame 0x48A940: Sleep(16) via 0x48F610, plus Sleep(1) at 0x48B3AF) and the
// file-reader thread 0x48E6D0 (CreateThread at 0x48E8E3, Sleep(1) at 0x48E6F2 while its queue is empty). Every other
// thread blocks in WaitFor*(INFINITE) or exits. On a Linux server exactly two threads burn a core each, and the main
// thread's ~490 voluntary switches/s with 99.9% CPU rule out real 16 ms sleeps, so Sleep() does not block there.
// This routes the exe's Sleep(n>0) to a timed wait on an event that is never set (the wineserver's timer, not
// NtDelayExecution), and lets the reader's idle poll wait MXHOOK_IDLE_MS (default 4) instead of 1 ms: a new request
// waits at most that long, well inside one 16.7 ms frame. The first SLEEP_SAMPLE calls of Sleep(1) and Sleep(16) are
// timed with the original Sleep, the next SLEEP_SAMPLE with the replacement, and both means are logged.

const SLEEP_SAMPLE: i32 = 200;
type SleepFn = unsafe extern "system" fn(u32);
static REAL_SLEEP: Real<SleepFn> = Real::new();
static SLEEP_EV: AtomicUsize = AtomicUsize::new(0);
static IDLE_RET: AtomicUsize = AtomicUsize::new(0);
static IDLE_MS: AtomicU32 = AtomicU32::new(4);
static SLEEP_N: [AtomicI32; 2] = [const { AtomicI32::new(0) }; 2];
static SLEEP_US: [[AtomicI32; 2]; 2] = [const { [const { AtomicI32::new(0) }; 2] }; 2];
static QPF: AtomicI64 = AtomicI64::new(1);

/// Sleep's hook: passes its caller's return address on to [`sleep_from`].
#[unsafe(naked)]
unsafe extern "system" fn hook_sleep(_ms: u32) {
    core::arch::naked_asm!(
        "push dword ptr [esp+4]",
        "push dword ptr [esp+4]",
        "call {imp}",
        "add esp, 8",
        "ret 4",
        imp = sym sleep_from,
    )
}

fn qpc() -> i64 {
    let mut t = 0;
    // SAFETY: an out-parameter
    unsafe { QueryPerformanceCounter(&mut t) };
    t
}

extern "C" fn sleep_from(ret: usize, ms: u32) {
    let ev = SLEEP_EV.load(Relaxed) as *mut c_void;
    // SAFETY: the event made by install_sleepfix, never set nor closed while the hook is in place
    let wait = |ms| unsafe { WaitForSingleObject(ev, ms) };
    if ms == 0 {
        // SAFETY: the original Sleep
        unsafe { REAL_SLEEP.get()(0) };
        return;
    }
    if ret == IDLE_RET.load(Relaxed) {
        wait(ms.max(IDLE_MS.load(Relaxed)));
        return;
    }
    let k = match ms {
        1 => 0,
        16 => 1,
        _ => {
            wait(ms);
            return;
        }
    };
    let n = SLEEP_N[k].fetch_add(1, Relaxed) + 1;
    if n > 2 * SLEEP_SAMPLE {
        wait(ms);
        return;
    }
    let a = qpc();
    if n <= SLEEP_SAMPLE {
        // SAFETY: the original Sleep: the "before" figure
        unsafe { REAL_SLEEP.get()(ms) };
    } else {
        wait(ms);
    }
    let us = (qpc() - a) * 1_000_000 / QPF.load(Relaxed);
    SLEEP_US[k][usize::from(n > SLEEP_SAMPLE)].fetch_add(us as i32, Relaxed);
    if n == 2 * SLEEP_SAMPLE {
        log!(
            "SLEEP fix: Sleep({ms}) over {SLEEP_SAMPLE} calls: original {} us mean, replacement {} us mean",
            SLEEP_US[k][0].load(Relaxed) / SLEEP_SAMPLE,
            SLEEP_US[k][1].load(Relaxed) / SLEEP_SAMPLE
        );
    }
}

fn install_sleepfix() {
    const IDLE_SITE: [u8; 8] = [0x6A, 0x01, 0xFF, 0x15, 0x80, 0xE0, 0x55, 0x00]; // push 1; call [Sleep]
    let forced = env_is("MXHOOK_SLEEPFIX", "1");
    if env_is("MXHOOK_SLEEPFIX", "0") {
        log!("SLEEP fix: OFF (MXHOOK_SLEEPFIX=0)");
        return;
    }
    if !forced && !exe_is(CONTROL) {
        return;
    }
    IDLE_MS.store(
        fixes::idle_ms(std::env::var("MXHOOK_IDLE_MS").ok().as_deref()),
        Relaxed,
    );
    let mut f = 0;
    // SAFETY: an out-parameter
    unsafe { QueryPerformanceFrequency(&mut f) };
    QPF.store(f.max(1), Relaxed);
    // SAFETY: an unnamed manual-reset event, never set
    let ev = unsafe { CreateEventA(std::ptr::null(), 1, 0, std::ptr::null()) };
    if ev.is_null() {
        // SAFETY: no arguments
        log!("SLEEP fix: NOT installed (CreateEvent err {})", unsafe {
            GetLastError()
        });
        return;
    }
    SLEEP_EV.store(ev as usize, Relaxed);
    // the reader's idle poll, only if its original bytes are where control 2010-11 has them
    if !forced && exe_bytes(0x8E6F0, 8) == IDLE_SITE {
        IDLE_RET.store(exe() as usize + 0x8E6F8, Relaxed);
    }
    if !patch(&REAL_SLEEP, "kernel32.dll", By::Name(c"Sleep"), hook_sleep) {
        // SAFETY: the event made above, not in use
        unsafe { CloseHandle(ev) };
        log!("SLEEP fix: NOT installed (no Sleep import)");
        return;
    }
    let idle = IDLE_RET.load(Relaxed) != 0;
    log!(
        "SLEEP fix: on - Sleep(n>0) waits on the wineserver timer; reader idle poll {} {} ms",
        if idle {
            "at 0x48E6F2 now"
        } else {
            "not matched (other exe or bytes differ), stays at"
        },
        if idle { IDLE_MS.load(Relaxed) } else { 1 }
    );
}

// ---- SPIN fix ----------------------------------------------------------------------------------------------------
// control's DistributePic thread spins (2026-10-17, a Linux server: the one control thread at ~100% CPU with 6
// voluntary switches in total, created ~2 s before ENTRY). DistributePic::run (0x44CAF0, a 0x490770 thread created by
// game init 0x48AC00 via 0x44CA70) loops `while (!stop) exec(this);`, and exec (0x44CBC0) returns at once while no
// picture download is due: a critical section, two timeGetTime calls and a compare, no wait and no syscall. This
// sleeps SPIN_MS before exec's per-pass timeGetTime at 0x44CC52. exec's own timers are 1 s (one download read per
// second) and longer, so a 10 ms pass changes none of them. Forced, every timeGetTime call of another exe waits.

const SPIN_MS: u32 = 10;
type TimeGetTime = unsafe extern "system" fn() -> u32;
static REAL_TIME_GET_TIME: Real<TimeGetTime> = Real::new();
static SPIN_RET: AtomicUsize = AtomicUsize::new(0);
static SPIN_FORCED: AtomicBool = AtomicBool::new(false);
static SPIN_SEEN: AtomicBool = AtomicBool::new(false);

/// timeGetTime's hook: passes its caller's return address on to [`time_from`].
#[unsafe(naked)]
unsafe extern "system" fn hook_time_get_time() -> u32 {
    core::arch::naked_asm!(
        "push dword ptr [esp]",
        "call {imp}",
        "add esp, 4",
        "ret",
        imp = sym time_from,
    )
}

extern "C" fn time_from(ret: usize) -> u32 {
    if SPIN_FORCED.load(Relaxed) || ret == SPIN_RET.load(Relaxed) {
        if !SPIN_SEEN.swap(true, Relaxed) {
            log!(
                "SPIN fix: DistributePic pass reached, each pass now waits {SPIN_MS} ms (thread {})",
                // SAFETY: no arguments
                unsafe { GetCurrentThreadId() }
            );
        }
        // SAFETY: kernel32's own Sleep, not the program's import
        unsafe { Sleep(SPIN_MS) };
    }
    // SAFETY: the original timeGetTime
    unsafe { REAL_TIME_GET_TIME.get()() }
}

fn install_spinfix() {
    const SITE: [u8; 6] = [0xFF, 0x15, 0x20, 0xE6, 0x55, 0x00]; // call [timeGetTime]
    if env_is("MXHOOK_SPINFIX", "0") {
        log!("SPIN fix: OFF (MXHOOK_SPINFIX=0)");
        return;
    }
    let forced = env_is("MXHOOK_SPINFIX", "1");
    SPIN_FORCED.store(forced, Relaxed);
    if !forced {
        if !exe_is(CONTROL) {
            return;
        }
        if exe_bytes(0x4CC52, 6) != SITE {
            log!("SPIN fix: NOT installed (bytes at 0x44CC52 differ from control 2010-11)");
            return;
        }
        SPIN_RET.store(exe() as usize + 0x4CC58, Relaxed);
    }
    if !patch(
        &REAL_TIME_GET_TIME,
        "winmm.dll",
        By::Name(c"timeGetTime"),
        hook_time_get_time,
    ) {
        log!("SPIN fix: NOT installed (no timeGetTime import)");
        return;
    }
    log!(
        "SPIN fix: on - DistributePic loop (0x44CAF0) waits {SPIN_MS} ms per pass at 0x44CC52{}",
        if forced {
            " (forced: every timeGetTime call)"
        } else {
            ""
        }
    );
}
