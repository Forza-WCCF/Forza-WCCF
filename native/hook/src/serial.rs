//! Virtual serial ports (2026-10-04).
//!
//! The player cabinet's Club Team Card reader is a serial device: client FUN_004d5820 opens "COM1" (COM3 on a real
//! cabinet), 115200 8N1, then polls ClearCommError for bytes and reads exactly those. MXHOOK_COM =
//! `COM1=\\.\pipe\wccf_icc_seat1` (several separated by `;`, [`crate::ports`]) makes CreateFileA of that port open the
//! named pipe instead; the Launcher's card reader stand-in is the pipe's server. On such a handle the comm functions
//! succeed, ClearCommError reports the bytes waiting in the pipe, and ReadFile waits at most the port's timeouts
//! (capped at 1 s) and never for more than is there. Without MXHOOK_COM nothing here is installed.
//!
//! The I/O board: on `\\.\mxjvs` (JVS mode 2) the game sends every command as DeviceIoControl 0x9C402000, which this
//! hook does not serve, so seat 1's exe is patched to JVS mode 0, where the game opens the board as "COM4".
//! FUN_008387b0 / FUN_00838850 write and read with an OVERLAPPED and wait 150 ms on its event, and assume a completed
//! read delivered every byte asked for. The board stand-in serves the port; these requests are completed at once
//! (event set) or, if the bytes do not come within ~120 ms, left "pending" so the game's own 150 ms timeout applies.
//! The SENSE line comes from GetCommModemStatus (FUN_00839730: DSR 0x20, RLSD 0x80 -> 0..3); the node setup (code
//! around 0x0082DA0A / 0x0082DB9F) reads 2 = no board, 3 = a board without an address, 1 = all boards addressed. So:
//! no bits until the game has sent F1 (set address), then DSR.

use crate::devices::{self, REAL_CREATE_FILE_A, RINGEDGE};
use crate::log::log;
use crate::patch::{Real, k32};
use crate::ports::{self, Port};
use std::ffi::{CStr, CString, c_void};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, OnceLock};
use windows_sys::Win32::Devices::Communication::{
    COMMTIMEOUTS, COMSTAT, DCB, MS_CTS_ON, MS_DSR_ON, PURGE_RXCLEAR,
};
use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, SetLastError,
};
use windows_sys::Win32::Storage::FileSystem::OPEN_EXISTING;
use windows_sys::Win32::System::IO::OVERLAPPED;
use windows_sys::Win32::System::Pipes::{
    PIPE_READMODE_BYTE, PeekNamedPipe, SetNamedPipeHandleState,
};
use windows_sys::Win32::System::Threading::{SetEvent, Sleep};

struct Vcom {
    port: Port,
    pipe: CString,
    dcb: DCB,
    to: COMMTIMEOUTS,
    /// JVS: the game sent "set address" (F1) since its last reset (F0 D9)
    addressed: bool,
}

static PORTS: OnceLock<Mutex<Vec<Vcom>>> = OnceLock::new();
/// Each port's open pipe handle (0 = closed): every ReadFile of the game looks here first.
static HANDLES: [AtomicUsize; ports::MAX] = [const { AtomicUsize::new(0) }; ports::MAX];

fn ports() -> MutexGuard<'static, Vec<Vcom>> {
    PORTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

type Comm<T> = unsafe extern "system" fn(HANDLE, *mut T) -> i32;
type SetupComm = unsafe extern "system" fn(HANDLE, u32, u32) -> i32;
type ByFlags = unsafe extern "system" fn(HANDLE, u32) -> i32;
type ClearCommError = unsafe extern "system" fn(HANDLE, *mut u32, *mut COMSTAT) -> i32;
type ReadFile =
    unsafe extern "system" fn(HANDLE, *mut c_void, u32, *mut u32, *mut OVERLAPPED) -> i32;
type WriteFile =
    unsafe extern "system" fn(HANDLE, *const c_void, u32, *mut u32, *mut OVERLAPPED) -> i32;
type GetOverlappedResult = unsafe extern "system" fn(HANDLE, *mut OVERLAPPED, *mut u32, i32) -> i32;
type OneHandle = unsafe extern "system" fn(HANDLE) -> i32;

static REAL_GET_COMM_STATE: Real<Comm<DCB>> = Real::new();
static REAL_SET_COMM_STATE: Real<Comm<DCB>> = Real::new();
static REAL_SETUP_COMM: Real<SetupComm> = Real::new();
static REAL_PURGE_COMM: Real<ByFlags> = Real::new();
static REAL_GET_COMM_TIMEOUTS: Real<Comm<COMMTIMEOUTS>> = Real::new();
static REAL_SET_COMM_TIMEOUTS: Real<Comm<COMMTIMEOUTS>> = Real::new();
static REAL_GET_COMM_MODEM_STATUS: Real<Comm<u32>> = Real::new();
static REAL_CLEAR_COMM_ERROR: Real<ClearCommError> = Real::new();
static REAL_READ_FILE: Real<ReadFile> = Real::new();
static REAL_WRITE_FILE: Real<WriteFile> = Real::new();
static REAL_GET_OVERLAPPED_RESULT: Real<GetOverlappedResult> = Real::new();
static REAL_CANCEL_IO: Real<OneHandle> = Real::new();
static REAL_CLOSE_HANDLE: Real<OneHandle> = Real::new();
/// CloseHandle is hooked for the ports and for the RingEdge devices: once.
static CLOSE_HOOKED: AtomicUsize = AtomicUsize::new(0);

/// The ports of MXHOOK_COM and their hooks; nothing without it.
pub fn install() {
    let spec = std::env::var("MXHOOK_COM").unwrap_or_default();
    let list: Vec<Vcom> = ports::parse(&spec)
        .into_iter()
        .filter_map(|port| {
            Some(Vcom {
                pipe: CString::new(port.pipe.clone()).ok()?,
                port,
                // SAFETY: plain C structs, all zeros until the port opens
                dcb: unsafe { std::mem::zeroed() },
                // SAFETY: as dcb
                to: unsafe { std::mem::zeroed() },
                addressed: false,
            })
        })
        .collect();
    if list.is_empty() {
        return;
    }
    let n = [
        k32(&REAL_GET_COMM_STATE, c"GetCommState", hook_get_comm_state),
        k32(&REAL_SET_COMM_STATE, c"SetCommState", hook_set_comm_state),
        k32(&REAL_SETUP_COMM, c"SetupComm", hook_setup_comm),
        k32(&REAL_PURGE_COMM, c"PurgeComm", hook_purge_comm),
        k32(
            &REAL_GET_COMM_TIMEOUTS,
            c"GetCommTimeouts",
            hook_get_comm_timeouts,
        ),
        k32(
            &REAL_SET_COMM_TIMEOUTS,
            c"SetCommTimeouts",
            hook_set_comm_timeouts,
        ),
        k32(
            &REAL_GET_COMM_MODEM_STATUS,
            c"GetCommModemStatus",
            hook_get_comm_modem_status,
        ),
        k32(
            &REAL_CLEAR_COMM_ERROR,
            c"ClearCommError",
            hook_clear_comm_error,
        ),
        k32(&REAL_READ_FILE, c"ReadFile", hook_read_file),
        k32(&REAL_WRITE_FILE, c"WriteFile", hook_write_file),
        k32(
            &REAL_GET_OVERLAPPED_RESULT,
            c"GetOverlappedResult",
            hook_get_overlapped_result,
        ),
        k32(&REAL_CANCEL_IO, c"CancelIo", hook_cancel_io),
        hook_close(),
    ]
    .iter()
    .filter(|&&b| b)
    .count();
    for v in &list {
        log!(
            "virtual port {} -> {}{} (serial hooks installed: {n} of 13)",
            v.port.com,
            v.port.pipe,
            if v.port.jvs { " [JVS I/O board]" } else { "" }
        );
    }
    *ports() = list;
}

/// Hooks CloseHandle (once); true if it took.
pub fn hook_close() -> bool {
    match CLOSE_HOOKED.load(Relaxed) {
        0 => {
            let took = k32(&REAL_CLOSE_HANDLE, c"CloseHandle", hook_close_handle);
            CLOSE_HOOKED.store(if took { 1 } else { 2 }, Relaxed);
            took
        }
        n => n == 1,
    }
}

/// The port a CreateFileA name opens, if it is one of ours.
pub fn port_for_name(name: *const u8) -> Option<usize> {
    if name.is_null() || PORTS.get().is_none() {
        return None;
    }
    // SAFETY: the game's NUL-terminated file name
    let name = unsafe { CStr::from_ptr(name.cast()) }.to_str().ok()?;
    ports::find(ports().iter().map(|v| v.port.com.as_str()), name)
}

fn port_of(h: HANDLE) -> Option<usize> {
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        return None;
    }
    HANDLES.iter().position(|x| x.load(Relaxed) == h as usize)
}

fn waiting(h: HANDLE) -> u32 {
    let mut avail = 0;
    // SAFETY: a pipe handle we opened
    if unsafe {
        PeekNamedPipe(
            h,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut avail,
            std::ptr::null_mut(),
        )
    } == 0
    {
        avail = 0;
    }
    avail
}

/// Opens port `i`'s pipe in place of the serial port.
pub fn open(i: usize) -> HANDLE {
    let mut list = ports();
    let v = &mut list[i];
    // SAFETY: the pipe's NUL-terminated name; the handle goes to the game, which closes it
    unsafe {
        let h = REAL_CREATE_FILE_A.get()(
            v.pipe.as_ptr().cast(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        let err = GetLastError();
        if h != INVALID_HANDLE_VALUE {
            let mode = PIPE_READMODE_BYTE;
            SetNamedPipeHandleState(h, &mode, std::ptr::null(), std::ptr::null());
            v.dcb = std::mem::zeroed();
            v.dcb.DCBlength = size_of::<DCB>() as u32;
            v.dcb.BaudRate = 115_200;
            v.dcb.ByteSize = 8;
            v.dcb._bitfield = 1; // fBinary
            v.to = std::mem::zeroed();
            HANDLES[i].store(h as usize, Relaxed);
        }
        let state = if h == INVALID_HANDLE_VALUE {
            "NO READER (pipe not there)"
        } else {
            "connected"
        };
        log!(
            "virtual port {} -> pipe {} : {state} (err {err})",
            v.port.com,
            v.port.pipe
        );
        SetLastError(if h == INVALID_HANDLE_VALUE {
            ERROR_FILE_NOT_FOUND
        } else {
            0
        });
        h
    }
}

unsafe extern "system" fn hook_get_comm_state(h: HANDLE, d: *mut DCB) -> i32 {
    let Some(i) = port_of(h) else {
        // SAFETY: the game's own call, passed on
        return unsafe { REAL_GET_COMM_STATE.get()(h, d) };
    };
    // SAFETY: the game's DCB
    unsafe { d.write(ports()[i].dcb) };
    1
}

unsafe extern "system" fn hook_set_comm_state(h: HANDLE, d: *mut DCB) -> i32 {
    let Some(i) = port_of(h) else {
        // SAFETY: the game's own call, passed on
        return unsafe { REAL_SET_COMM_STATE.get()(h, d) };
    };
    // SAFETY: the game's DCB
    let d = unsafe { d.read() };
    let mut list = ports();
    list[i].dcb = d;
    log!(
        "virtual port {}: SetCommState {} baud, {} bits, parity {}, stop {}",
        list[i].port.com,
        d.BaudRate,
        d.ByteSize,
        d.Parity,
        d.StopBits
    );
    1
}

unsafe extern "system" fn hook_setup_comm(h: HANDLE, i: u32, o: u32) -> i32 {
    if port_of(h).is_none() {
        // SAFETY: the game's own call, passed on
        unsafe { REAL_SETUP_COMM.get()(h, i, o) }
    } else {
        1
    }
}

unsafe extern "system" fn hook_purge_comm(h: HANDLE, flags: u32) -> i32 {
    if port_of(h).is_none() {
        // SAFETY: the game's own call, passed on
        return unsafe { REAL_PURGE_COMM.get()(h, flags) };
    }
    if flags & PURGE_RXCLEAR != 0 {
        let mut junk = [0u8; 256];
        loop {
            let n = waiting(h).min(junk.len() as u32);
            let mut got = 0;
            if n == 0
                // SAFETY: our pipe handle and buffer
                || unsafe {
                    REAL_READ_FILE.get()(
                        h,
                        junk.as_mut_ptr().cast(),
                        n,
                        &mut got,
                        std::ptr::null_mut(),
                    )
                } == 0
                || got == 0
            {
                break;
            }
        }
    }
    1
}

unsafe extern "system" fn hook_get_comm_timeouts(h: HANDLE, t: *mut COMMTIMEOUTS) -> i32 {
    let Some(i) = port_of(h) else {
        // SAFETY: the game's own call, passed on
        return unsafe { REAL_GET_COMM_TIMEOUTS.get()(h, t) };
    };
    // SAFETY: the game's struct
    unsafe { t.write(ports()[i].to) };
    1
}

unsafe extern "system" fn hook_set_comm_timeouts(h: HANDLE, t: *mut COMMTIMEOUTS) -> i32 {
    let Some(i) = port_of(h) else {
        // SAFETY: the game's own call, passed on
        return unsafe { REAL_SET_COMM_TIMEOUTS.get()(h, t) };
    };
    // SAFETY: the game's struct
    ports()[i].to = unsafe { t.read() };
    1
}

unsafe extern "system" fn hook_get_comm_modem_status(h: HANDLE, s: *mut u32) -> i32 {
    let Some(i) = port_of(h) else {
        // SAFETY: the game's own call, passed on
        return unsafe { REAL_GET_COMM_MODEM_STATUS.get()(h, s) };
    };
    let list = ports();
    // the sense line: 1 = addressed, 3 = a board waiting
    let bits = match (list[i].port.jvs, list[i].addressed) {
        (true, true) => MS_DSR_ON,
        (true, false) => 0,
        (false, _) => MS_CTS_ON | MS_DSR_ON,
    };
    // SAFETY: the game's status word
    unsafe { s.write(bits) };
    1
}

/// A request on a virtual port is finished at once: the OVERLAPPED says done and its event is set.
///
/// # Safety
/// `ov` and `got` are null or the game's own.
unsafe fn complete(ov: *mut OVERLAPPED, got: *mut u32, n: u32) -> i32 {
    // SAFETY: the caller's contract
    unsafe {
        if !got.is_null() {
            got.write(n);
        }
        if let Some(ov) = ov.as_mut() {
            ov.Internal = 0; // STATUS_SUCCESS
            ov.InternalHigh = n as usize;
            if !ov.hEvent.is_null() {
                SetEvent(ov.hEvent);
            }
        }
    }
    1
}

/// JVS: what the game sends for reset (F0 D9) and set address (F1 n) drives the sense line.
fn jvs_watch(v: &mut Vcom, p: &[u8]) {
    for w in p.windows(2) {
        if w == [0xF0, 0xD9] && v.addressed {
            v.addressed = false;
            log!("JVS: reset seen - sense = board waiting for an address");
        } else if w[0] == 0xF1 && !v.addressed {
            v.addressed = true;
            log!("JVS: set address {} seen - sense = all addressed", w[1]);
        }
    }
}

unsafe extern "system" fn hook_write_file(
    h: HANDLE,
    buf: *const c_void,
    n: u32,
    put: *mut u32,
    ov: *mut OVERLAPPED,
) -> i32 {
    let Some(i) = port_of(h) else {
        // SAFETY: the game's own call, passed on
        return unsafe { REAL_WRITE_FILE.get()(h, buf, n, put, ov) };
    };
    {
        let mut list = ports();
        if list[i].port.jvs && !buf.is_null() {
            // SAFETY: the game's buffer of n bytes
            jvs_watch(&mut list[i], unsafe {
                std::slice::from_raw_parts(buf.cast(), n as usize)
            });
        }
    }
    let mut done = 0;
    // SAFETY: the pipe is synchronous: written now, then reported done
    unsafe {
        if REAL_WRITE_FILE.get()(h, buf, n, &mut done, std::ptr::null_mut()) == 0 {
            return 0;
        }
        complete(ov, put, done)
    }
}

unsafe extern "system" fn hook_get_overlapped_result(
    h: HANDLE,
    ov: *mut OVERLAPPED,
    got: *mut u32,
    wait: i32,
) -> i32 {
    // SAFETY: the game's own call and OVERLAPPED
    unsafe {
        if port_of(h).is_none() {
            return REAL_GET_OVERLAPPED_RESULT.get()(h, ov, got, wait);
        }
        if !got.is_null() {
            got.write((*ov).InternalHigh as u32);
        }
    }
    1
}

unsafe extern "system" fn hook_cancel_io(h: HANDLE) -> i32 {
    if port_of(h).is_none() {
        // SAFETY: the game's own call, passed on
        unsafe { REAL_CANCEL_IO.get()(h) }
    } else {
        1
    }
}

unsafe extern "system" fn hook_clear_comm_error(
    h: HANDLE,
    errs: *mut u32,
    st: *mut COMSTAT,
) -> i32 {
    // SAFETY: the game's own call and structs
    unsafe {
        if port_of(h).is_none() {
            return REAL_CLEAR_COMM_ERROR.get()(h, errs, st);
        }
        if !errs.is_null() {
            errs.write(0);
        }
        if !st.is_null() {
            st.write(std::mem::zeroed());
            (*st).cbInQue = waiting(h);
        }
    }
    1
}

unsafe extern "system" fn hook_read_file(
    h: HANDLE,
    buf: *mut c_void,
    n: u32,
    got: *mut u32,
    ov: *mut OVERLAPPED,
) -> i32 {
    let Some(i) = port_of(h) else {
        // SAFETY: the game's own call, passed on
        return unsafe { REAL_READ_FILE.get()(h, buf, n, got, ov) };
    };
    let read = |len: u32, done: &mut u32| {
        // SAFETY: the game's buffer holds n >= len bytes; our pipe handle
        unsafe { REAL_READ_FILE.get()(h, buf, len, done, std::ptr::null_mut()) != 0 }
    };
    let (mut waited, mut done) = (0, 0);
    if !ov.is_null() {
        // overlapped (the I/O board): wait for ALL n bytes; if they do not come, leave it "pending"
        let mut avail = waiting(h);
        while avail < n && waited < 120 {
            // SAFETY: plain Win32 call
            unsafe { Sleep(1) };
            waited += 1;
            avail = waiting(h);
        }
        if avail < n {
            // SAFETY: plain Win32 call
            unsafe { SetLastError(ERROR_IO_PENDING) };
            return 0;
        }
        if !read(n, &mut done) {
            return 0;
        }
        // SAFETY: the game's own pointers
        return unsafe { complete(ov, got, done) };
    }
    let to = ports()[i].to;
    let mut limit = to
        .ReadTotalTimeoutConstant
        .wrapping_add(to.ReadTotalTimeoutMultiplier.wrapping_mul(n));
    if to.ReadIntervalTimeout == u32::MAX && limit == 0 {
        limit = 0; // "return at once with what is there"
    } else if limit == 0 || limit > 1000 {
        limit = 1000; // a real port would block; never hang the game
    }
    let mut avail = waiting(h);
    while avail == 0 && waited < limit {
        // SAFETY: plain Win32 call
        unsafe { Sleep(1) };
        waited += 1;
        avail = waiting(h);
    }
    if avail > 0 && !read(n.min(avail), &mut done) {
        return 0;
    }
    if !got.is_null() {
        // SAFETY: the game's pointer
        unsafe { got.write(done) };
    }
    1
}

unsafe extern "system" fn hook_close_handle(h: HANDLE) -> i32 {
    if let Some(i) = port_of(h) {
        log!("virtual port {} closed", ports()[i].port.com);
        HANDLES[i].store(0, Relaxed);
    }
    if RINGEDGE.load(Relaxed) {
        devices::forget(h);
    }
    // SAFETY: the game's own call, passed on
    unsafe { REAL_CLOSE_HANDLE.get()(h) }
}
