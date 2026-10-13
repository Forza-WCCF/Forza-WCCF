//! A 2010-11 game program run under a debugger (port of Kit 6's code/hardware/debugger.py; no admin, no WER, no cdb):
//! the shared memory in place, and crashes reported - address, registers, the crashing thread's start address, the
//! return addresses on its stack (named from a Ghidra index when there is one) - with two memory snapshots each.
//!
//! The thread that calls `run` becomes the debugger (`Job::spawn_debug`) and pumps the Win32 debug loop until the
//! program ends, crashes for good, loses its window (a client) or the time runs out.  While it handles an event Windows
//! holds EVERY thread of the program (the game's video player alone starts 21 threads at once), so this process runs at
//! high priority, unthrottled, and times each event: one over 50 ms is printed, and the end prints count / mean / max
//! per kind of event.
//!
//! - Invalid-parameter stops (exit code 0xC0000417, 2026-10-05): the game's runtime (MSVCR90.dll) ends the program itself
//!   through _invoke_watson - no exception, so a debugger sees only the exit code.  A breakpoint on _invoke_watson
//!   (armed in every debugged process that loads msvcr90.dll) prints the stopping thread's callers, then lets it end.
//! - Windows' throttling off (2026-10-05): every debugged process is opted out of timer coarsening and efficiency mode.
//!   Windows 11 ignores timeBeginPeriod(1) for a program whose window is minimized or covered - the server's own window
//!   starts minimized - so its Sleep(16)+Sleep(1) loop took ~31 ms instead of ~17, and the projector's matches stuttered.
//! - The debug heap off (`_NO_DEBUG_HEAP=1`, inherited by children) unless `debug_heap`: on 2026-10-03 Windows' debug
//!   heap made control so slow it never reached the step where the bare run crashes.
//! - `follow`: every process the program starts is debugged too (control's four match engines) and how each ends is
//!   reported; otherwise children are only watched from outside (started / gone).
//! - A fault repeating at one address (a frame loop) is shown twice and then only counted; the end prints the top counts.
//! - The words play.py matches stay as they are: "its window was closed", "SECOND-chance / FATAL", "STOP: invalid
//!   parameter".

mod dump;
pub mod maps;
pub mod mem;

use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, DBG_CONTINUE, DBG_EXCEPTION_NOT_HANDLED, GetLastError, HANDLE, NTSTATUS,
};
use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
use windows_sys::Win32::System::Diagnostics::Debug::{
    CREATE_PROCESS_DEBUG_EVENT, CREATE_THREAD_DEBUG_EVENT, ContinueDebugEvent, DEBUG_EVENT, EXCEPTION_DEBUG_EVENT,
    EXIT_PROCESS_DEBUG_EVENT, EXIT_THREAD_DEBUG_EVENT, FlushInstructionCache, LOAD_DLL_DEBUG_EVENT,
    OUTPUT_DEBUG_STRING_EVENT, ReadProcessMemory, WOW64_CONTEXT, WOW64_CONTEXT_FULL, WaitForDebugEvent,
    Wow64GetThreadContext, Wow64SetThreadContext, WriteProcessMemory,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, HIGH_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
    PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation, TerminateProcess,
};

use dump::write_dumps;
use mem::{Index, MSGPARSE_RET_OFF, MSGPARSE_STACK_SCAN};

use crate::job::Job;

#[link(name = "ntdll")]
unsafe extern "system" {
    /// Here only with ProcessCommandLineInformation (60, Windows 8.1 and later): a UNICODE_STRING and its text.
    fn NtQueryInformationProcess(h: HANDLE, class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

/// The arcade program never quits when its window is closed: the window goes, the program runs on without it (seen
/// 2026-10-08: seat 4's client ran on windowless, and the whole run with it).  So a client is ended once its window, up
/// for this long, has been gone for two looks in a row.  Never control: its windows are hidden.
const WINDOW_SETTLE: f64 = 5.0;
const EXCEPTION_ACCESS_VIOLATION: u32 = 0xC000_0005;
/// Native and WOW64 breakpoints.
const BREAKPOINTS: [u32; 2] = [0x8000_0003, 0x4000_001F];
/// MSVC thread naming.
const QUIET_CODES: [u32; 1] = [0x406D_1388];
/// The kind of a debug event, by its code.
const EVENT_NAMES: [&str; 10] = [
    "0",
    "exception",
    "thread start",
    "process start",
    "thread end",
    "process end",
    "DLL load",
    "DLL unload",
    "debug text",
    "RIP",
];
/// Programs with a Ghidra index, lower case, and the index's folder.
const INDEXED: [(&str, &str); 4] = [
    ("client_release.exe", "client"),
    ("control_release.exe", "control"),
    ("match_release.exe", "match"),
    ("launcher1011.exe", "launcher"),
];
/// The program the message-reassembly guard is for.
const CONTROL: &str = "control_release.exe";

/// What the program is: a client (a cabinet or the projector: its window is watched), control (the server: its debug
/// text is written line by line) or a match engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Client,
    Control,
    Match,
}

/// The shared memory to make before the start (debugger.py without "nomaps").
pub struct Maps {
    /// The game's extracted folder, where the motion files are.
    pub game: PathBuf,
    /// The mappings' name prefix: `maps::GAME_ID`, or a test's own.
    pub id: String,
    /// SBWG_moSuppl too (the client crashes without it; control does not read it).
    pub mosuppl: bool,
    /// Which game's mappings.
    pub edition: crate::kit::Edition,
}

/// One debugged run, as debugger.py's command line said it.
pub struct Options {
    pub role: Role,
    /// The program; its folder is where it runs (debugger.py's dir=).
    pub exe: PathBuf,
    /// Appended to its command line (arg=).
    pub args: Vec<String>,
    pub timeout: Duration,
    pub follow: bool,
    /// Its windows start hidden (the server unless "debug": its console and settings window, where a click could stop
    /// it).
    pub hide: bool,
    pub debug_heap: bool,
    /// None: no shared memory made (tests, or a stack that already has it).
    pub maps: Option<Maps>,
    /// The debug channel's file: `<logs>\_dbg_<role><_folder name when not the game folder>.txt`.
    pub dbg_file: PathBuf,
    /// Where crash snapshots go: data\dumps.
    pub dumps: PathBuf,
    /// The Ghidra index folder (`<it>\<client|control|match>\_index.csv`); None: addresses unnamed.
    pub index: Option<PathBuf>,
    /// The message-reassembly guard for control (WCCF_MSGGUARD, on unless "0").
    pub msgguard: bool,
    /// Added to the program's environment (the run's WCCF_ and MXHOOK_ settings).
    pub env: Vec<(String, String)>,
    /// Set by the run to end the program now (a quit or a cabinet restart), as the timeout does.
    pub stop: Option<Arc<AtomicBool>>,
}

macro_rules! say {
    ($out:expr, $($t:tt)*) => { let _ = writeln!($out, $($t)*); };
}

/// One debugged process: its memory handle, image, threads, DLLs and index.
struct Target {
    pid: u32,
    /// Owned by the debug events: Windows closes it after the process's end event.
    hproc: HANDLE,
    base: u64,
    end: u64,
    name: String,
    /// thread id -> (handle, start address)
    threads: HashMap<u32, (HANDLE, u64)>,
    dlls: Vec<(u64, u64, String)>,
    /// MSVCR90!_invoke_watson's address and original byte while its breakpoint is armed.
    watson: Option<(u64, u8)>,
    index: Rc<Index>,
    /// Why the message guard stepped aside at the last fault.
    why: String,
}

impl Target {
    fn rd(&self, addr: u64, n: usize) -> Option<Vec<u8>> {
        if self.hproc.is_null() || addr == 0 || n == 0 {
            return None;
        }
        let mut buf = vec![0u8; n];
        let mut got = 0usize;
        // SAFETY: the buffer holds `n` bytes; the handle is the debuggee's.
        let ok =
            unsafe { ReadProcessMemory(self.hproc, addr as *const c_void, buf.as_mut_ptr().cast(), n, &mut got) != 0 };
        (ok && got > 0).then(|| {
            buf.truncate(got);
            buf
        })
    }

    fn wr(&self, addr: u64, data: &[u8]) -> bool {
        let mut got = 0usize;
        // SAFETY: the data is live for the call; the handle is the debuggee's (all access).
        unsafe {
            let ok = WriteProcessMemory(
                self.hproc,
                addr as *const c_void,
                data.as_ptr().cast(),
                data.len(),
                &mut got,
            );
            FlushInstructionCache(self.hproc, addr as *const c_void, data.len());
            ok != 0 && got == data.len()
        }
    }

    fn reader(&self) -> impl Fn(u64, usize) -> Option<Vec<u8>> + '_ {
        |a, n| self.rd(a, n)
    }

    fn thread(&self, tid: u32) -> (HANDLE, u64) {
        self.threads.get(&tid).copied().unwrap_or((ptr::null_mut(), 0))
    }

    fn name_of(&self, va: u64) -> String {
        mem::name_of(va, &self.index, self.base, self.end, &self.name, &self.dlls)
    }

    fn image_end(&self, base: u64) -> u64 {
        mem::image_end(&self.reader(), base)
    }

    fn looks_like_return(&self, va: u64) -> bool {
        va >= 7 && self.rd(va - 7, 7).is_some_and(|b| mem::looks_like_return(&b))
    }

    /// If thread `tid` faulted while control's FUN_00412860 was on the stack (the message reassembler - a read or a
    /// memcpy it makes), make that function return cleanly (drop the bad message) and return a short note.  Only for
    /// control, only when the guard is on.  Each time it steps aside it says why in `why` (2026-10-08: it stepped aside
    /// twice on the live server and nothing said why).
    fn try_recover_412860(&mut self, tid: u32, guard: bool) -> Option<String> {
        self.why.clear();
        if !guard || !self.name.eq_ignore_ascii_case(CONTROL) {
            return None;
        }
        let h = self.thread(tid).0;
        if h.is_null() {
            self.why = format!("no handle for thread {tid}");
            return None;
        }
        let Some(mut ctx) = context(h) else {
            self.why = format!("could not read the thread's registers (err {})", last_error());
            return None;
        };
        let fault_eip = ctx.Eip;
        let Some(plan) = mem::plan_412860_recovery(ctx.Esp, &self.reader(), self.base as u32, MSGPARSE_RET_OFF) else {
            let stack = self.rd(u64::from(ctx.Esp), MSGPARSE_STACK_SCAN).unwrap_or_default();
            let ret = (self.base as u32).wrapping_add(MSGPARSE_RET_OFF);
            self.why = mem::no_plan_note(ctx.Esp, &stack, ret);
            return None;
        };
        for addr in [plan.p_len, plan.p_off] {
            // remaining length and offset -> 0: abandon the bad chunk
            if !self.wr(u64::from(addr), &[0; 4]) {
                self.why = format!("could not write 0x{addr:08X} (err {})", last_error());
                return None;
            }
        }
        (ctx.Edi, ctx.Esi, ctx.Ebp, ctx.Ebx) = (plan.edi, plan.esi, plan.ebp, plan.ebx);
        (ctx.Eip, ctx.Esp, ctx.Eax) = (plan.eip, plan.esp, plan.eax);
        // SAFETY: the context was read from this thread and is live for the call.
        if unsafe { Wow64SetThreadContext(h, &ctx) } == 0 {
            self.why = format!("could not set the thread's registers (err {})", last_error());
            return None;
        }
        Some(format!(
            "fault at 0x{fault_eip:08X} ({}, frame +0x{:X}); len@0x{:08X}=0 off@0x{:08X}=0 -> return 1 to 0x{:08X}",
            self.name_of(u64::from(fault_eip)),
            plan.at,
            plan.p_len,
            plan.p_off,
            plan.eip
        ))
    }

    /// A breakpoint on MSVCR90!_invoke_watson -> a line for the log.
    fn arm_watson(&mut self, base: u64) -> String {
        let va = mem::export_va(&self.reader(), base, "_invoke_watson");
        let old = va.and_then(|v| self.rd(v, 1));
        match (va, old) {
            (Some(v), Some(old)) if self.wr(v, &[0xCC]) => {
                self.watson = Some((v, old[0]));
                format!("armed at 0x{v:08X}")
            }
            _ => format!(
                "NOT armed (_invoke_watson {}, error {})",
                va.map_or_else(|| "not found".to_owned(), |v| format!("at 0x{v:08X}")),
                last_error()
            ),
        }
    }

    /// The original byte back, and the thread's EIP back onto it: _invoke_watson then runs as if never stopped.
    fn unarm_watson(&mut self, tid: u32) {
        let Some((va, old)) = self.watson.take() else { return };
        self.wr(va, &[old]);
        let h = self.thread(tid).0;
        if let Some(mut ctx) = context(h).filter(|c| u64::from(c.Eip) == va + 1) {
            ctx.Eip = va as u32;
            // SAFETY: the context was read from this thread.
            unsafe { Wow64SetThreadContext(h, &ctx) };
        }
    }

    /// The process's command line; None if unreadable.
    fn command_line(&self) -> Option<String> {
        let mut buf = vec![0u64; 1024];
        let mut got = 0u32;
        // SAFETY: the buffer is 8 KB, 8-byte aligned, its size passed; the handle is the debuggee's.
        let st = unsafe {
            NtQueryInformationProcess(
                self.hproc,
                60,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut got,
            )
        };
        if st < 0 {
            return None;
        }
        // SAFETY: on success the buffer starts with a UNICODE_STRING whose text lies inside the buffer.
        unsafe {
            let len = usize::from(*(buf.as_ptr().cast::<u16>())) / 2;
            let text = *(buf.as_ptr().cast::<u8>().add(8).cast::<*const u16>());
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(text, len)))
        }
    }

    fn dump_thread(&self, out: &mut dyn Write, tid: u32) {
        let (h, start) = self.thread(tid);
        if h.is_null() {
            say!(out, "    (no handle for thread {tid})");
            return;
        }
        say!(
            out,
            "    thread {tid}, started at 0x{start:08X} = {}",
            self.name_of(start)
        );
        let Some(ctx) = context(h) else {
            say!(out, "    (Wow64GetThreadContext failed err={})", last_error());
            return;
        };
        say!(out, "    EIP={:08X}  {}", ctx.Eip, self.name_of(u64::from(ctx.Eip)));
        say!(
            out,
            "    EAX={:08X} EBX={:08X} ECX={:08X} EDX={:08X} ESI={:08X} EDI={:08X} EBP={:08X} ESP={:08X}",
            ctx.Eax,
            ctx.Ebx,
            ctx.Ecx,
            ctx.Edx,
            ctx.Esi,
            ctx.Edi,
            ctx.Ebp,
            ctx.Esp
        );
        if let Some(code) = self.rd(u64::from(ctx.Eip), 16) {
            let hex: Vec<String> = code.iter().map(|b| format!("{b:02x}")).collect();
            say!(out, "    code at EIP: {}", hex.join(" "));
        }
        let stack = mem::rd_upto(&self.reader(), u64::from(ctx.Esp), 0x2000);
        say!(
            out,
            "    return addresses on the stack (ESP+offset: address = function; a raw scan, so leftovers of earlier \
             calls show too):"
        );
        let mut shown = 0;
        for off in (0..stack.len().saturating_sub(3)).step_by(4) {
            let v = u64::from(u32::from_le_bytes(stack[off..off + 4].try_into().unwrap()));
            let name = self.name_of(v);
            if name != "?" && self.looks_like_return(v) {
                say!(out, "      +{off:03X}: {v:08X} = {name}");
                shown += 1;
                if shown >= 80 {
                    break;
                }
            }
        }
        self.dump_texts(out, &stack, &ctx);
    }

    /// The texts the stack points to - in a stop inside vsprintf_s the format being parsed is one of them, marked *
    /// when it holds a % - and the bytes around EBX, the printf engine's read position in msvcr90 9.0.30729 (seat 1's
    /// stops of 2026-10-05: the game's text drawer FUN_0040e170 uses each text AS A FORMAT).
    fn dump_texts(&self, out: &mut dyn Write, stack: &[u8], ctx: &WOW64_CONTEXT) {
        say!(
            out,
            "    texts the stack points to (ESP+offset: address -> text; * = it holds a %):"
        );
        let (mut seen, mut shown) = (std::collections::HashSet::new(), 0);
        for off in (0..stack.len().saturating_sub(3)).step_by(4) {
            let v = u32::from_le_bytes(stack[off..off + 4].try_into().unwrap());
            if !seen.insert(v) {
                continue;
            }
            if let Some(s) = mem::text_at(&self.reader(), u64::from(v)) {
                let mark = if s.contains(&b'%') { "*" } else { " " };
                say!(
                    out,
                    "      +{off:03X}: {v:08X} -> {mark}\"{}\"",
                    mem::esc(&s[..s.len().min(160)])
                );
                shown += 1;
                if shown >= 40 {
                    break;
                }
            }
        }
        let ebx = u64::from(ctx.Ebx);
        let before = if ebx > 64 {
            mem::rd_upto(&self.reader(), ebx - 64, 64)
        } else {
            Vec::new()
        };
        let after = mem::rd_upto(&self.reader(), ebx, 48);
        if !before.is_empty() || !after.is_empty() {
            say!(
                out,
                "    bytes around EBX {:08X}: \"{}\" <EBX> \"{}\"",
                ctx.Ebx,
                mem::esc(&before),
                mem::esc(&after)
            );
        }
    }
}

fn last_error() -> u32 {
    // SAFETY: plain call.
    unsafe { GetLastError() }
}

/// A thread's 32-bit registers.
fn context(h: HANDLE) -> Option<WOW64_CONTEXT> {
    if h.is_null() {
        return None;
    }
    let mut ctx = WOW64_CONTEXT {
        ContextFlags: WOW64_CONTEXT_FULL,
        ..Default::default()
    };
    // SAFETY: the context is live and has its flags set; the handle is a debuggee thread's.
    (unsafe { Wow64GetThreadContext(h, &mut ctx) } != 0).then_some(ctx)
}

/// The file name of an image handle from a debug event.
fn path_of(h: HANDLE) -> Option<String> {
    if h.is_null() {
        return None;
    }
    let mut buf = [0u16; 520];
    // SAFETY: the buffer and its length are passed; the handle is the event's file handle.
    let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
    if n == 0 || n > buf.len() {
        return None;
    }
    let p = String::from_utf16_lossy(&buf[..n]);
    Some(p.rsplit(['\\', '/']).next().unwrap_or(&p).to_owned())
}

/// Opts a process out of Windows' throttling; what was switched off, or None.
fn full_speed(h: HANDLE) -> Option<&'static str> {
    let (speed, timer) = (
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
    );
    for (mask, what) in [
        (speed | timer, "timer and speed"),
        (timer, "timer"),
        (speed, "speed only (this Windows has no timer opt-out)"),
    ] {
        let s = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: mask,
            StateMask: 0,
        };
        // SAFETY: the structure is the one the class takes, live for the call.
        let ok = !h.is_null()
            && unsafe {
                SetProcessInformation(
                    h,
                    ProcessPowerThrottling,
                    (&s as *const PROCESS_POWER_THROTTLING_STATE).cast(),
                    size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                )
            } != 0;
        if ok {
            return Some(what);
        }
    }
    None
}

/// "" for the program itself, "[name pid] " for a child.
fn who(main: u32, pid: u32, name: &str) -> String {
    if pid == main {
        String::new()
    } else {
        format!("[{name} {pid}] ")
    }
}

/// The debug channel's file: line by line for control (2026-10-07: through an 8 KB buffer - about 25 minutes of
/// control's output - a quiet file was misread as a frozen server), buffered for a cabinet, where every debug event
/// holds the game until it is answered; written out whenever no event waits.
struct DbgFile {
    file: BufWriter<File>,
    each_line: bool,
}

impl DbgFile {
    fn line(&mut self, s: &str) {
        let _ = writeln!(self.file, "{s}");
        if self.each_line {
            let _ = self.file.flush();
        }
    }
}

/// Runs the program under this thread's debugger, in `job`, and reports to `out` (the run's log).  Returns 1 if it
/// crashed for good, 2 if it is missing, else 0.  Every debugged process left is ended before it returns.
pub fn run(opts: &Options, job: &Job, out: &mut dyn Write) -> io::Result<i32> {
    let exe_name = opts
        .exe
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let run_dir = opts.exe.parent().unwrap_or(Path::new("."));
    if !opts.exe.is_file() {
        say!(out, "no {exe_name} in {}", run_dir.display());
        return Ok(2);
    }
    say!(
        out,
        "{}; target={exe_name} {}; debug heap {}; children {}:",
        match &opts.maps {
            Some(m) => format!(
                "creating mappings (moSuppl={})",
                if m.mosuppl { "True" } else { "False" }
            ),
            None => "NO mappings (nomaps)".to_owned(),
        },
        opts.args.join(" "),
        if opts.debug_heap { "ON" } else { "off" },
        if opts.follow {
            "DEBUGGED (follow)"
        } else {
            "watched only"
        }
    );
    let _maps: Vec<OwnedHandle> = match &opts.maps {
        Some(m) => maps::make_all(out, &m.game, &m.id, m.mosuppl, m.edition)?,
        None => Vec::new(),
    };
    // SAFETY: the pseudo handle of this process.
    let me = unsafe { GetCurrentProcess() };
    // every event freezes the game until it is answered: answer fast
    // SAFETY: plain call on this process.
    let high = unsafe { SetPriorityClass(me, HIGH_PRIORITY_CLASS) } != 0;
    say!(
        out,
        "  this debugger: high priority {}, Windows' throttling off: {}",
        if high { "yes" } else { "NO" },
        full_speed(me).unwrap_or("NO")
    );
    let mut env: Vec<(&str, &str)> = opts.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    if !opts.debug_heap {
        env.push(("_NO_DEBUG_HEAP", "1"));
    }
    let child = job.spawn_debug(&opts.exe, &opts.args, run_dir, &env, opts.follow, opts.hide)?;
    let main = child.pid;
    say!(
        out,
        "launched {exe_name} pid {main} under debugger from {}; watching up to {}s{}",
        run_dir.display(),
        opts.timeout.as_secs(),
        if opts.hide { "; its windows hidden" } else { "" }
    );

    let mut procs: HashMap<u32, Target> = HashMap::new();
    let mut ended: Vec<(String, u32, String)> = Vec::new();
    let mut indexes: HashMap<&str, Rc<Index>> = HashMap::new();
    let t0 = Instant::now();
    let mut seen_codes: BTreeMap<u32, u32> = BTreeMap::new();
    let mut seen_at: Vec<((String, u32, u64), u32)> = Vec::new(); // (process name, code, address) -> count
    let mut dbg = DbgFile {
        file: BufWriter::with_capacity(8192, File::create(&opts.dbg_file)?),
        each_line: opts.role == Role::Control,
    };
    let (mut dbg_lines, mut dbg_shown) = (0, 0);
    let mut kids: Vec<(u32, String)> = Vec::new();
    let mut next_kid_check = 0.0;
    let mut fatal = false;
    let mut stops = 0; // invalid-parameter stops caught
    let mut guarded = 0u32; // control message-reassembly faults caught and recovered
    let mut ev_time: BTreeMap<u32, (u32, f64, f64)> = BTreeMap::new(); // event kind -> count, total ms, max ms
    let watch_window = opts.role == Role::Client;
    let (mut next_win_check, mut win_since, mut win_gone) = (0.0, None::<f64>, 0);
    let mut timed_out = true;
    let mut main_exited = false;
    let mut de = DEBUG_EVENT::default();
    while t0.elapsed() < opts.timeout {
        if opts.stop.as_ref().is_some_and(|s| s.load(Ordering::Relaxed)) {
            say!(
                out,
                "  t+{:5.1}s the run is stopping - ending {exe_name}",
                t0.elapsed().as_secs_f64()
            );
            timed_out = false;
            break;
        }
        let now = t0.elapsed().as_secs_f64();
        if watch_window && now >= next_win_check {
            // its window closed: the game ends
            next_win_check = now + 1.0;
            if crate::procs::has_window(main) {
                win_since.get_or_insert(now);
                win_gone = 0;
            } else if win_since.is_some_and(|s| now - s >= WINDOW_SETTLE) {
                win_gone += 1;
                if win_gone >= 2 {
                    say!(
                        out,
                        "  t+{now:5.1}s its window was closed - ending {exe_name} (the program never quits by itself)"
                    );
                    let _ = out.flush();
                    timed_out = false;
                    break;
                }
            } else {
                win_since = None; // a window replaced while it starts: not a close
            }
        }
        if !opts.follow && now >= next_kid_check {
            next_kid_check = now + 1.0;
            let cur = children_of(main);
            for (cpid, cname) in &cur {
                if !kids.iter().any(|k| k.0 == *cpid) {
                    say!(out, "  t+{now:5.1}s child started: {cname} pid {cpid}");
                }
            }
            for (cpid, cname) in &kids {
                if !cur.iter().any(|k| k.0 == *cpid) {
                    say!(out, "  t+{now:5.1}s child gone: {cname} pid {cpid}");
                }
            }
            kids = cur;
        }
        // SAFETY: the event is a live local.
        if unsafe { WaitForDebugEvent(&mut de, 250) } == 0 {
            // nothing waiting: the game runs, so writing the buffer now holds nothing up.  A stop kills this launcher,
            // and what was only in the buffer was lost (2026-10-11: seat 1's side of an Error 3000)
            let _ = dbg.file.flush();
            continue;
        }
        let t_ev = Instant::now();
        let now = t0.elapsed().as_secs_f64();
        let (code, pid, tid) = (de.dwDebugEventCode, de.dwProcessId, de.dwThreadId);
        let mut status: NTSTATUS = DBG_CONTINUE;
        let mut name_for_slow = procs.get(&pid).map(|p| p.name.clone());
        if code == CREATE_PROCESS_DEBUG_EVENT {
            // SAFETY: the union member this event code fills.
            let ci = unsafe { de.u.CreateProcessInfo };
            let name = path_of(ci.hFile).unwrap_or_else(|| format!("pid{pid}"));
            if !ci.hFile.is_null() {
                // SAFETY: the event's file handle is the debugger's to close.
                unsafe { CloseHandle(ci.hFile) };
            }
            let lower = name.to_lowercase();
            let index = match INDEXED.iter().find(|(n, _)| *n == lower) {
                Some((_, prog)) => indexes
                    .entry(prog)
                    .or_insert_with(|| Rc::new(load_index(out, opts.index.as_deref(), prog)))
                    .clone(),
                None => Rc::default(),
            };
            let mut pr = Target {
                pid,
                hproc: ci.hProcess,
                base: ci.lpBaseOfImage as u64,
                end: ci.lpBaseOfImage as u64 + 0x1000,
                name,
                threads: HashMap::new(),
                dlls: Vec::new(),
                watson: None,
                index,
                why: String::new(),
            };
            pr.end = pr.image_end(pr.base);
            pr.threads
                .insert(tid, (ci.hThread, ci.lpStartAddress.map_or(0, |f| f as usize as u64)));
            let fast = full_speed(ci.hProcess);
            if pid == main || fast.is_none() {
                say!(
                    out,
                    "  t+{now:5.1}s {}Windows' throttling off: {}",
                    who(main, pid, &pr.name),
                    // SAFETY: plain call.
                    fast.map_or_else(|| format!("FAILED (error {})", last_error()), str::to_owned)
                );
            }
            if pid == main {
                say!(out, "  image base = 0x{:08X} (to 0x{:08X})", pr.base, pr.end);
                if pr.name.eq_ignore_ascii_case(CONTROL) {
                    let ret = (pr.base as u32).wrapping_add(MSGPARSE_RET_OFF);
                    let mapped = pr.rd(u64::from(ret), 1).is_some();
                    say!(
                        out,
                        "  MSGPARSE guard: {} (any fault while FUN_00412860 is on the stack -> drop the message; \
                         caller return 0x{ret:08X}; off with WCCF_MSGGUARD=0)",
                        if !mapped {
                            format!("NOT armed - control+0x{MSGPARSE_RET_OFF:X} is not mapped (different build?)")
                        } else if opts.msgguard {
                            "ON".to_owned()
                        } else {
                            "off (WCCF_MSGGUARD=0)".to_owned()
                        }
                    );
                }
            } else {
                say!(
                    out,
                    "  t+{now:5.1}s process started: {} pid {pid} (debugged)  command line: {}",
                    pr.name,
                    pr.command_line().unwrap_or_else(|| "None".to_owned())
                );
            }
            name_for_slow = Some(pr.name.clone());
            procs.insert(pid, pr);
        } else if let Some(pr) = procs.get_mut(&pid) {
            match code {
                CREATE_THREAD_DEBUG_EVENT => {
                    // SAFETY: the union member this event code fills.
                    let ct = unsafe { de.u.CreateThread };
                    pr.threads
                        .insert(tid, (ct.hThread, ct.lpStartAddress.map_or(0, |f| f as usize as u64)));
                }
                EXIT_THREAD_DEBUG_EVENT => {
                    pr.threads.remove(&tid);
                }
                LOAD_DLL_DEBUG_EVENT => {
                    // SAFETY: the union member this event code fills.
                    let li = unsafe { de.u.LoadDll };
                    let base = li.lpBaseOfDll as u64;
                    let nm = path_of(li.hFile).unwrap_or_else(|| format!("dll@{base:08X}"));
                    if !li.hFile.is_null() {
                        // SAFETY: the event's file handle is the debugger's to close.
                        unsafe { CloseHandle(li.hFile) };
                    }
                    let end = pr.image_end(base);
                    pr.dlls.push((base, end, nm.clone()));
                    if nm.eq_ignore_ascii_case("msvcr90.dll") {
                        let how = pr.arm_watson(base);
                        if pid == main || pr.watson.is_none() {
                            say!(
                                out,
                                "  t+{now:5.1}s {}invalid-parameter catch (msvcr90.dll _invoke_watson) {how}",
                                who(main, pid, &pr.name)
                            );
                        }
                    }
                }
                OUTPUT_DEBUG_STRING_EVENT => {
                    // SAFETY: the union member this event code fills.
                    let ds = unsafe { de.u.DebugString };
                    let raw = pr
                        .rd(
                            ds.lpDebugStringData as u64,
                            usize::from(ds.nDebugStringLength).min(1024),
                        )
                        .unwrap_or_default();
                    let txt = if ds.fUnicode != 0 {
                        let w: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
                        String::from_utf16_lossy(&w)
                    } else {
                        mem::cp932(&raw, false).unwrap_or_default()
                    };
                    let txt = txt.trim_end_matches(['\0', '\r', '\n']);
                    let w = who(main, pid, &pr.name);
                    dbg.line(&format!("t+{now:6.2}s {w}{txt}"));
                    dbg_lines += 1;
                    if !txt.starts_with("inst(") && dbg_shown < 60 {
                        // inst(0) = one team name each, hundreds
                        say!(out, "  t+{now:5.1}s dbg> {w}{txt}");
                        dbg_shown += 1;
                    }
                }
                EXCEPTION_DEBUG_EVENT => {
                    // SAFETY: the union member this event code fills.
                    let ex = unsafe { de.u.Exception };
                    let er = ex.ExceptionRecord;
                    let ec = er.ExceptionCode as u32;
                    let first = ex.dwFirstChance != 0;
                    let addr = er.ExceptionAddress as u64;
                    let w = who(main, pid, &pr.name);
                    if BREAKPOINTS.contains(&ec) && pr.watson.is_some_and(|(v, _)| addr == v || addr == v + 1) {
                        stops += 1;
                        say!(
                            out,
                            "  t+{now:5.1}s {w}STOP: invalid parameter (msvcr90.dll _invoke_watson) - the program is \
                             ending itself with 0xC0000417; the callers below passed the bad value  [thread {tid}]"
                        );
                        pr.dump_thread(out, tid);
                        say!(
                            out,
                            "  t+{now:5.1}s {}",
                            write_dumps(pr.hproc, pid, &pr.name, "invalid-parameter stop", &opts.dumps)
                        );
                        let _ = out.flush();
                        pr.unarm_watson(tid);
                    } else if BREAKPOINTS.contains(&ec) && first {
                        // the start's breakpoints: go on
                    } else if QUIET_CODES.contains(&ec) {
                        status = DBG_EXCEPTION_NOT_HANDLED;
                    } else if ec == EXCEPTION_ACCESS_VIOLATION
                        && first
                        && pr.try_recover_412860(tid, opts.msgguard).is_some()
                    {
                        // the known control message-reassembly fault: recovered (bad message dropped), control lives on
                        guarded += 1;
                        if guarded <= 5 || guarded.is_multiple_of(50) {
                            say!(
                                out,
                                "  t+{now:5.1}s GUARD: control OOB message read at 0x{addr:08X} caught & dropped \
                                 (#{guarded})  [thread {tid}]"
                            );
                        }
                        dbg.line(&format!(
                            "t+{now:6.2}s GUARD #{guarded} recovered 0x{addr:08X}: len/off zeroed, FUN_00412860 returns 1"
                        ));
                    } else {
                        status = DBG_EXCEPTION_NOT_HANDLED;
                        let n = seen_codes.entry(ec).or_default();
                        *n += 1;
                        let n = *n;
                        let key = (pr.name.clone(), ec, addr);
                        let at_n = match seen_at.iter_mut().find(|(k, _)| *k == key) {
                            Some((_, c)) => {
                                *c += 1;
                                *c
                            }
                            None => {
                                seen_at.push((key, 1));
                                1
                            }
                        };
                        // the same fault in a loop: show twice, then count
                        if !first || (at_n <= 2 && (ec == EXCEPTION_ACCESS_VIOLATION || n <= 3)) {
                            let detail = if ec == EXCEPTION_ACCESS_VIOLATION {
                                let acc = match er.ExceptionInformation[0] {
                                    0 => "read",
                                    1 => "write",
                                    8 => "execute",
                                    _ => "access",
                                };
                                format!("  ({acc} of 0x{:08X})", er.ExceptionInformation[1] as u32)
                            } else {
                                String::new()
                            };
                            say!(
                                out,
                                "  t+{now:5.1}s {w}{} exception 0x{ec:08X} at 0x{addr:08X} = {}{detail}  [thread {tid}]",
                                if first { "FIRST-chance" } else { "SECOND-chance / FATAL" },
                                pr.name_of(addr)
                            );
                            if ec == EXCEPTION_ACCESS_VIOLATION || !first {
                                pr.dump_thread(out, tid);
                            }
                        }
                        if !first {
                            // the program dies now: keep its memory as it is
                            let why = format!("fatal exception 0x{ec:08X} at 0x{addr:08X}");
                            say!(
                                out,
                                "  t+{now:5.1}s {}",
                                write_dumps(pr.hproc, pid, &pr.name, &why, &opts.dumps)
                            );
                            let _ = out.flush();
                        }
                        if ec == EXCEPTION_ACCESS_VIOLATION && first && !pr.why.is_empty() {
                            say!(out, "  t+{now:5.1}s MSGPARSE guard stepped aside: {}", pr.why);
                            dbg.line(&format!("t+{now:6.2}s MSGPARSE guard stepped aside: {}", pr.why));
                        }
                        if !first && pid == main {
                            fatal = true;
                            // SAFETY: answers the event just received.
                            unsafe { ContinueDebugEvent(pid, tid, status) };
                            timed_out = false;
                            break;
                        }
                    }
                }
                EXIT_PROCESS_DEBUG_EVENT => {
                    // SAFETY: the union member this event code fills.
                    let how = format!("exit code 0x{:08X}", unsafe { de.u.ExitProcess.dwExitCode });
                    if pid == main {
                        say!(out, "  t+{now:5.1}s process exited, {how}");
                        main_exited = true;
                        // SAFETY: answers the event just received.
                        unsafe { ContinueDebugEvent(pid, tid, status) };
                        timed_out = false;
                        break;
                    }
                    say!(out, "  t+{now:5.1}s process ended: {} pid {pid}, {how}", pr.name);
                    ended.push((pr.name.clone(), pid, how));
                    procs.remove(&pid);
                }
                _ => {}
            }
        } // else: an event for a process never seen starting
        let held = t_ev.elapsed().as_secs_f64() * 1000.0;
        let st = ev_time.entry(code).or_insert((0, 0.0, 0.0));
        *st = (st.0 + 1, st.1 + held, st.2.max(held));
        if held > 50.0 {
            say!(
                out,
                "  t+{now:5.1}s SLOW: a {} event held {} for {held:.0} ms",
                event_name(code),
                name_for_slow.unwrap_or_else(|| format!("pid {pid}"))
            );
        }
        // SAFETY: answers the event just received.
        unsafe { ContinueDebugEvent(pid, tid, status) };
    }
    if timed_out {
        say!(
            out,
            "  timed out after {}s (no fatal crash); still running",
            opts.timeout.as_secs()
        );
    }
    let _ = dbg.file.flush();
    drop(dbg);
    say!(
        out,
        "  debug channel: {dbg_lines} lines, all in {}",
        opts.dbg_file.display()
    );
    if stops > 0 {
        say!(out, "  invalid-parameter stops caught: {stops} (callers printed above)");
    }
    if guarded > 0 {
        say!(
            out,
            "  MSGPARSE guard: {guarded} control OOB message read(s) caught & dropped - control kept running"
        );
    }
    if !ev_time.is_empty() {
        say!(
            out,
            "  time the game was held per debug event: {}",
            held_summary(&ev_time)
        );
    }
    if !seen_codes.is_empty() {
        let counts: Vec<String> = seen_codes.iter().map(|(c, n)| format!("0x{c:08X} x{n}")).collect();
        say!(out, "  exception counts: {}", counts.join(", "));
        seen_at.sort_by_key(|a| std::cmp::Reverse(a.1)); // stable: first seen first among equals
        for ((nm, c, a), n) in seen_at.iter().take(10) {
            let owner = procs.values().find(|q| q.name == *nm);
            say!(
                out,
                "    {nm}: 0x{c:08X} at 0x{a:08X} = {}  x{n}",
                owner.map_or("?".to_owned(), |q| q.name_of(*a))
            );
        }
    }
    if opts.follow {
        let e: Vec<String> = ended.iter().map(|(n, p, h)| format!("{n} {p} ({h})")).collect();
        say!(
            out,
            "  debugged children that ended: {}",
            if e.is_empty() { "none".to_owned() } else { e.join(", ") }
        );
        let r: Vec<String> = procs
            .values()
            .filter(|q| q.pid != main)
            .map(|q| format!("{} {}", q.name, q.pid))
            .collect();
        say!(
            out,
            "  still running at the end: {}",
            if r.is_empty() { "none".to_owned() } else { r.join(", ") }
        );
    } else {
        let c: Vec<String> = children_of(main).iter().map(|(p, n)| format!("{n} {p}")).collect();
        say!(
            out,
            "  children at the end: {}",
            if c.is_empty() { "none".to_owned() } else { c.join(", ") }
        );
    }
    let _ = out.flush();
    // The program ends by its own handle; every process still debugged with it (debugger.py's exit ended them).
    // SAFETY: the handles are the program's own and the debug events' (all access).
    unsafe {
        TerminateProcess(child.process.as_raw_handle(), 1);
        for q in procs.values().filter(|q| q.pid != main) {
            TerminateProcess(q.hproc, 1);
        }
    }
    drain(
        main_exited.then_some(main),
        procs.keys().copied().filter(|&p| p != main).collect(),
    );
    Ok(i32::from(fatal))
}

/// Answers the debug events of ended processes until the program (unless it has already gone: None) and `others` have gone (at most 5 s): a debugged
/// process finishes ending only once its debugger has seen it go.
fn drain(main: Option<u32>, mut others: Vec<u32>) {
    let mut main_gone = main.is_none();
    let t = Instant::now();
    let mut de = DEBUG_EVENT::default();
    while !(main_gone && others.is_empty()) && t.elapsed() < Duration::from_secs(5) {
        // SAFETY: the event is a live local.
        if unsafe { WaitForDebugEvent(&mut de, 250) } == 0 {
            continue;
        }
        // SAFETY: the union member each event code fills; their file handles are the debugger's to close.
        let file = unsafe {
            match de.dwDebugEventCode {
                CREATE_PROCESS_DEBUG_EVENT => de.u.CreateProcessInfo.hFile,
                LOAD_DLL_DEBUG_EVENT => de.u.LoadDll.hFile,
                _ => ptr::null_mut(),
            }
        };
        if !file.is_null() {
            // SAFETY: as above.
            unsafe { CloseHandle(file) };
        }
        if de.dwDebugEventCode == EXIT_PROCESS_DEBUG_EVENT {
            main_gone |= main == Some(de.dwProcessId);
            others.retain(|&p| p != de.dwProcessId);
        }
        let status = if de.dwDebugEventCode == EXCEPTION_DEBUG_EVENT {
            DBG_EXCEPTION_NOT_HANDLED
        } else {
            DBG_CONTINUE
        };
        // SAFETY: answers the event just received.
        unsafe { ContinueDebugEvent(de.dwProcessId, de.dwThreadId, status) };
    }
}

fn event_name(code: u32) -> String {
    EVENT_NAMES
        .get(code as usize)
        .map_or_else(|| code.to_string(), |s| (*s).to_owned())
}

/// "exception 3 x mean 0.12 ms max 0.3 ms, ..." by event code.
pub fn held_summary(ev_time: &BTreeMap<u32, (u32, f64, f64)>) -> String {
    let parts: Vec<String> = ev_time
        .iter()
        .map(|(c, (n, tot, mx))| {
            format!(
                "{} {n} x mean {:.2} ms max {mx:.1} ms",
                event_name(*c),
                tot / f64::from(*n)
            )
        })
        .collect();
    parts.join(", ")
}

fn load_index(out: &mut dyn Write, dir: Option<&Path>, prog: &str) -> Index {
    let Some(dir) = dir else { return Index::default() };
    let path = dir.join(prog).join("_index.csv");
    Index::load(&path).unwrap_or_else(|| {
        say!(
            out,
            "  (no Ghidra index at {} - addresses will be unnamed)",
            path.display()
        );
        Index::default()
    })
}

/// (pid, file name) of the processes `pid` started.
fn children_of(pid: u32) -> Vec<(u32, String)> {
    crate::procs::processes()
        .into_iter()
        .filter(|p| p.parent == pid)
        .map(|p| (p.pid, p.name))
        .collect()
}
