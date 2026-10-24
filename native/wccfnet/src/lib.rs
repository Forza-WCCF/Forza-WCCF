//! wccfnet.dll: the guarded copies of the 2017-18 game programs import it instead of IPHLPAPI.DLL. An ordinary import,
//! unlike a hook loaded into a running program, also protects the programs' children. The four calls that would change
//! the host's network (the DHCP release and renew, two route writes) are stubs that touch nothing; the six read-only
//! ones are forwarded to Windows' iphlpapi (build.rs). No adapter is ever changed.
//!
//! DllMain also loads the panel when the Launcher sets FORZA_PANEL ([`panel`]), and, only with WCCF_TRACE_NET=1, logs
//! the program's send and recv ([`trace`]).
#![cfg(target_arch = "x86")]

use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleFileNameW};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

/// wccfnet.log, beside this DLL.
static LOG: OnceLock<PathBuf> = OnceLock::new();

/// One line in wccfnet.log (every game program of the folder appends to the same file).
fn log(text: &str) {
    let Some(path) = LOG.get() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
    {
        let _ = f.write_all(format!("pid={} {text}\r\n", std::process::id()).as_bytes()); // one write: whole lines
    }
}

fn blocked(name: &str) -> u32 {
    log(&format!("blocked {name}"));
    0 // NO_ERROR
}

#[unsafe(no_mangle)]
pub extern "system" fn IpReleaseAddress(_: *mut c_void) -> u32 {
    blocked("IpReleaseAddress")
}

#[unsafe(no_mangle)]
pub extern "system" fn IpRenewAddress(_: *mut c_void) -> u32 {
    blocked("IpRenewAddress")
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateIpForwardEntry(_: *mut c_void) -> u32 {
    blocked("CreateIpForwardEntry")
}

#[unsafe(no_mangle)]
pub extern "system" fn SetIpForwardEntry(_: *mut c_void) -> u32 {
    blocked("SetIpForwardEntry")
}

#[unsafe(no_mangle)]
extern "system" fn DllMain(
    inst: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> windows_sys::core::BOOL {
    if reason == DLL_PROCESS_ATTACH {
        let mut buf = [0u16; 1024];
        // SAFETY: our own module handle and a buffer we own
        let n = unsafe {
            DisableThreadLibraryCalls(inst);
            GetModuleFileNameW(inst, buf.as_mut_ptr(), buf.len() as u32) as usize
        };
        if n > 0 && n < buf.len() {
            let dll = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
            let _ = LOG.set(dll.with_file_name("wccfnet.log"));
        }
        panel::start();
        if std::env::var("WCCF_TRACE_NET").is_ok_and(|v| v == "1") {
            trace::start();
        }
    }
    1
}

/// The panel: when the Launcher sets FORZA_PANEL (seat 1 and the projector only), the DLL it names is loaded into the
/// game 3 s after the game's window shows (the trigger inject.exe had), by a thread, never in DllMain itself.
mod panel {
    use super::log;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM};
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
    };

    pub fn start() {
        let Some(dll) = std::env::var_os("FORZA_PANEL").filter(|d| !d.is_empty()) else {
            return;
        };
        // this process only, as inject.exe did: never its children
        // SAFETY: DllMain, before this DLL starts any thread of its own
        unsafe { std::env::remove_var("FORZA_PANEL") };
        std::thread::spawn(move || load(dll));
    }

    unsafe extern "system" fn own_window(w: HWND, found: LPARAM) -> windows_sys::core::BOOL {
        let mut pid = 0;
        // SAFETY: a window handle EnumWindows gives, and the flag `load` passes
        unsafe {
            GetWindowThreadProcessId(w, &mut pid);
            if pid == std::process::id() && IsWindowVisible(w) != 0 {
                *(found as *mut bool) = true;
                return 0;
            }
        }
        1
    }

    fn load(dll: OsString) {
        let mut seen = false;
        for _ in 0..240 {
            // SAFETY: the callback writes only the flag it is given, during this call
            unsafe { EnumWindows(Some(own_window), &raw mut seen as LPARAM) };
            if seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if !seen {
            log("panel: no window in 120 s, not loaded");
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        let wide: Vec<u16> = dll.encode_wide().chain([0]).collect();
        let shown = dll.to_string_lossy();
        // SAFETY: a NUL-terminated path; the panel stays loaded for the process's life
        if unsafe { LoadLibraryW(wide.as_ptr()) }.is_null() {
            // SAFETY: plain Win32 call
            let err = unsafe { GetLastError() };
            log(&format!("panel: did NOT load {shown} (error {err})"));
        } else {
            log(&format!("panel: loaded {shown}"));
        }
    }
}

/// Debug trace, WCCF_TRACE_NET=1 only: the program's imports of send (ordinal 19) and recv (16) from WS2_32 or WSOCK32
/// come here; each call is logged (time, program, socket, ports, length, the first 64 bytes) and passed on unchanged,
/// its error code kept. The games import no WSASend or WSARecv. Every winsock function comes from the game's own WS2_32
/// (GetProcAddress): this DLL never imports it. Without the variable nothing is patched.
mod trace {
    use super::log;
    use forza_native::{By, imports, redirect};
    use std::ffi::CStr;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, SOCKADDR_IN, SOCKET, SOCKET_ERROR, WSAEWOULDBLOCK,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleHandleA, GetModuleHandleExA, GetModuleHandleW, GetProcAddress,
    };

    type Xfer = unsafe extern "system" fn(SOCKET, *mut u8, i32, i32) -> i32;
    /// What GetProcAddress gives.
    type Proc = unsafe extern "system" fn() -> isize;
    type Name = unsafe extern "system" fn(SOCKET, *mut SOCKADDR_IN, *mut i32) -> i32;
    struct Ws {
        sockname: Option<Name>,
        peername: Option<Name>,
        last_error: unsafe extern "system" fn() -> i32,
        set_last_error: unsafe extern "system" fn(i32),
        /// the program's folder and name: game\control_Release.exe, seat1\client_Release.exe
        role: String,
    }
    static WS: OnceLock<Ws> = OnceLock::new();
    static REAL_SEND: AtomicUsize = AtomicUsize::new(0);
    static REAL_RECV: AtomicUsize = AtomicUsize::new(0);

    pub fn start() {
        // SAFETY: the game imports WS2_32, so it is loaded already; each pointer is cast to its documented signature
        let ws = unsafe {
            let ws = GetModuleHandleA(c"ws2_32.dll".as_ptr().cast());
            if ws.is_null() {
                log("trace: WS2_32 not loaded, no trace");
                return;
            }
            let f = |n: &CStr| GetProcAddress(ws, n.as_ptr().cast());
            let (Some(le), Some(sle)) = (f(c"WSAGetLastError"), f(c"WSASetLastError")) else {
                log("trace: no WSAGetLastError, no trace");
                return;
            };
            Ws {
                sockname: f(c"getsockname").map(|p| std::mem::transmute::<Proc, Name>(p)),
                peername: f(c"getpeername").map(|p| std::mem::transmute::<Proc, Name>(p)),
                last_error: std::mem::transmute::<Proc, unsafe extern "system" fn() -> i32>(le),
                set_last_error: std::mem::transmute::<Proc, unsafe extern "system" fn(i32)>(sle),
                role: String::new(),
            }
        };
        let exe = std::env::current_exe().unwrap_or_default();
        let role = match (exe.parent().and_then(|p| p.file_name()), exe.file_name()) {
            (Some(d), Some(f)) => format!("{}\\{}", d.to_string_lossy(), f.to_string_lossy()),
            (_, f) => f
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let _ = WS.set(Ws { role, ..ws });
        let s = patch(19, c"send", traced_send as Xfer, &REAL_SEND);
        let r = patch(16, c"recv", traced_recv as Xfer, &REAL_RECV);
        log(&format!(
            "trace: on in {}, send hooked {s}, recv hooked {r}",
            exe.display()
        ));
    }

    /// The program's imports of `ordinal` (or of `name`) from a winsock DLL now call `hook`; the old target goes to
    /// `real`. Only a resolved slot (it points into a loaded DLL), never one the loader has yet to fill.
    fn patch(ordinal: u16, name: &CStr, hook: Xfer, real: &AtomicUsize) -> usize {
        let mut done = 0;
        // SAFETY: the process's main module, loaded for its whole life
        for d in unsafe { imports(GetModuleHandleW(std::ptr::null()) as *const u8) } {
            let n = d.name().to_bytes();
            if !n.eq_ignore_ascii_case(b"ws2_32.dll") && !n.eq_ignore_ascii_case(b"wsock32.dll") {
                continue;
            }
            for (by, slot) in d.entries() {
                if by != By::Ordinal(ordinal) && by != By::Name(name) {
                    continue;
                }
                let mut owner: HMODULE = std::ptr::null_mut();
                // SAFETY: an import slot of the loaded program; the hook has the import's signature
                unsafe {
                    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                        | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
                    if GetModuleHandleExA(flags, slot.read() as *const u8, &mut owner) == 0 {
                        continue;
                    }
                    if let Some(old) = redirect(slot, hook as usize) {
                        real.store(old, Relaxed);
                        done += 1;
                    }
                }
            }
        }
        done
    }

    fn port_of(f: Option<Name>, s: SOCKET) -> i32 {
        // SAFETY: an AF_INET name fits a SOCKADDR_IN; winsock fills it
        unsafe {
            let mut a: SOCKADDR_IN = std::mem::zeroed();
            let mut n = size_of::<SOCKADDR_IN>() as i32;
            match f {
                Some(f) if f(s, &mut a, &mut n) == 0 && a.sin_family == AF_INET => {
                    i32::from(u16::from_be(a.sin_port))
                }
                _ => -1,
            }
        }
    }

    fn trace(ws: &Ws, dir: &str, s: SOCKET, buf: *const u8, ret: i32, err: i32) {
        let n = ret.clamp(0, 64) as usize;
        let bytes = if n == 0 {
            &[][..]
        } else {
            // SAFETY: send sent and recv received `ret` bytes of the game's buffer
            unsafe { std::slice::from_raw_parts(buf, n) }
        };
        let hex: String = bytes.iter().map(|b| format!("{b:02x} ")).collect();
        let asc: String = bytes
            .iter()
            .map(|&c| {
                if (32..127).contains(&c) {
                    c as char
                } else {
                    '.'
                }
            })
            .collect();
        let t = forza_native::local_time();
        log(&format!(
            "{:02}:{:02}:{:02}.{:03} {} {dir} sock={s} local={} peer={} len={ret} err={} | {hex}| {asc}",
            t.wHour,
            t.wMinute,
            t.wSecond,
            t.wMilliseconds,
            ws.role,
            port_of(ws.sockname, s),
            port_of(ws.peername, s),
            if ret < 0 { err } else { 0 }
        ));
    }

    /// The real call, then the line (a "nothing yet" of a non-blocking socket is not logged), its error kept.
    fn pass(dir: &str, real: &AtomicUsize, s: SOCKET, buf: *mut u8, len: i32, flags: i32) -> i32 {
        let ws = WS.get().expect("patched only after WS is set");
        // SAFETY: `real` is the import's own target (set before the slot pointed here), with the hook's signature
        unsafe {
            let f = std::mem::transmute::<usize, Xfer>(real.load(Relaxed));
            let ret = f(s, buf, len, flags);
            let err = (ws.last_error)();
            if ret != SOCKET_ERROR || err != WSAEWOULDBLOCK {
                trace(ws, dir, s, buf, ret, err);
            }
            (ws.set_last_error)(err);
            ret
        }
    }

    unsafe extern "system" fn traced_send(s: SOCKET, buf: *mut u8, len: i32, flags: i32) -> i32 {
        pass("send", &REAL_SEND, s, buf, len, flags)
    }

    unsafe extern "system" fn traced_recv(s: SOCKET, buf: *mut u8, len: i32, flags: i32) -> i32 {
        pass("recv", &REAL_RECV, s, buf, len, flags)
    }
}
