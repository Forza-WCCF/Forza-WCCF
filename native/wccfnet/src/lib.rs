//! wccfnet.dll: the guarded copies of the 2017-18 game programs import it instead of IPHLPAPI.DLL. An ordinary import,
//! unlike a hook loaded into a running program, also protects the programs' children. The four calls that would change
//! the host's network (the DHCP release and renew, two route writes) are stubs that touch nothing; the six read-only
//! ones are forwarded to Windows' iphlpapi (build.rs). No adapter is ever changed. The program's name lookups and
//! WinInet calls fail for the publisher's network hosts, and its `connect` and `sendto` for the publisher's addresses
//! ([`forza_native::hosts`]).
//!
//! DllMain also loads the panel when the Launcher sets FORZA_PANEL ([`panel`]), with it keeps the 2017-18 client's
//! window hidden through the boot screens ([`boot`]), and, only with WCCF_TRACE_NET=1, logs the program's send and recv
//! ([`trace`]).
#![cfg(target_arch = "x86")]
// This code runs in the game's process, where a panic ends the game: no call that can panic (tests may, clippy.toml)
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::string_slice
)]

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
        if n > 0
            && n < buf.len()
            && let Some(name) = buf.get(..n)
        {
            let dll = PathBuf::from(String::from_utf16_lossy(name));
            let _ = LOG.set(dll.with_file_name("wccfnet.log"));
        }
        log(&forza_native::hosts::install(log));
        log(&forza_native::hosts::install_addresses(log));
        if std::env::var_os("FORZA_PANEL").is_some_and(|d| !d.is_empty()) {
            boot::start();
        }
        panel::start();
        if std::env::var("WCCF_TRACE_NET").is_ok_and(|v| v == "1") {
            trace::start();
        }
    }
    1
}

/// The panel: when the Launcher sets FORZA_PANEL (seat 1 and the projector only), the DLL it names is loaded into the
/// game 3 s after the game's window shows or is held hidden ([`boot`]) (the trigger inject.exe had), by a thread, never
/// in DllMain itself.
mod panel {
    use super::log;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM};
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetPropW, GetWindowThreadProcessId, IsWindowVisible,
    };

    pub fn start() {
        let Some(dll) = std::env::var_os("FORZA_PANEL").filter(|d| !d.is_empty()) else {
            return;
        };
        // this process only, as inject.exe did: never its children
        // SAFETY: DllMain, before this DLL starts any thread of its own
        unsafe { std::env::remove_var("FORZA_PANEL") };
        // no thread (out of resources): no panel, and the game runs on
        if std::thread::Builder::new()
            .spawn(move || load(dll))
            .is_err()
        {
            log("panel: no thread to load it, not loaded");
        }
    }

    unsafe extern "system" fn own_window(w: HWND, found: LPARAM) -> windows_sys::core::BOOL {
        let mut pid = 0;
        // SAFETY: a window handle EnumWindows gives, and the flag `load` passes
        unsafe {
            GetWindowThreadProcessId(w, &mut pid);
            // the window held hidden through the boot screens counts as shown ([`super::boot`])
            if pid == std::process::id()
                && (IsWindowVisible(w) != 0 || !GetPropW(w, super::boot::PROP).is_null())
            {
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

/// The black window at the start (2017-18 client_Release.exe with FORZA_PANEL: seat 1 and the projector). The game's
/// window function (sub_10C86F0) shows its new window ~20 s before the game draws anything, so the boot and update
/// screens were an unpainted black window. Its ShowWindow import is pointed here before the game runs; the one call from
/// inside that function with SW_SHOWNORMAL, on a window that is not WS_POPUP (exclusive full screen), is held: the
/// window stays hidden and gets [`PROP`], and the panel shows it at the start warning picture. Every other call goes
/// to user32 unchanged. The slot is put back after the first hold (the game reads it once into a register for its loop,
/// so the hook keeps its own copy of the real target). [`FALLBACK`] after a hold with the property still set, the window
/// is shown anyway, without the focus (by the panel when it holds the window, so the projector setting applies). Any
/// check that fails leaves the slot alone: the window shows as before.
pub mod boot {
    use super::log;
    use forza_native::{By, imports, redirect};
    use std::ops::Range;
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleHandleA, GetModuleHandleW, GetProcAddress,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetPropW, GetWindowLongW, PostMessageW, RemovePropW, SW_SHOWNOACTIVATE,
        SW_SHOWNORMAL, SetPropW, ShowWindowAsync, WS_POPUP,
    };
    use windows_sys::core::BOOL;

    /// On the game's window while it is held hidden; whoever removes it shows the window (the panel or [`fallback`]).
    pub use forza_native::BOOT_PROP as PROP;
    use forza_native::{BOOT_PANEL, CLIENT_1718_STAMP as STAMP, WM_BOOT_SHOW};
    /// Its window function sub_10C86F0 (RVA), where the held call returns to.
    const WINDOW_FN: Range<usize> = 0x00CC_86F0..0x00CC_893D;
    /// Its import slot __imp_ShowWindow (RVA).
    const SHOW_SLOT: usize = 0x00DA_9C60;
    /// The window shows by itself this long after the hold if the panel has not shown it.
    pub const FALLBACK: Duration = Duration::from_secs(60);

    type Show = unsafe extern "system" fn(HWND, i32) -> BOOL;
    static REAL: AtomicUsize = AtomicUsize::new(0);
    static BASE: AtomicUsize = AtomicUsize::new(0);
    /// The patched slot while it points at [`show_stub`] (0: put back, or never patched).
    static SLOT: AtomicUsize = AtomicUsize::new(0);

    /// Held: the call returns into the window function (`ret` inside it, the program at `base`), shows normally, and
    /// the window is no exclusive full-screen one.
    pub fn holds(ret: usize, base: usize, cmd: i32, popup: bool) -> bool {
        cmd == SW_SHOWNORMAL
            && !popup
            && ret
                .checked_sub(base)
                .is_some_and(|r| WINDOW_FN.contains(&r))
    }

    /// The program's TimeDateStamp and SizeOfImage from its PE header at `base`.
    fn pe_stamp(base: usize) -> (u32, u32) {
        let u32_at = |off: usize| {
            // SAFETY: inside the headers of the loaded program
            unsafe { ((base + off) as *const u32).read_unaligned() }
        };
        let pe = u32_at(0x3C) as usize;
        (u32_at(pe + 8), u32_at(pe + 0x50))
    }

    pub fn start() {
        let exe = std::env::current_exe().unwrap_or_default();
        if !exe
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("client_Release.exe"))
        {
            return;
        }
        // SAFETY: the process's main module, loaded for its whole life; user32 is one of its imports
        unsafe {
            let base = GetModuleHandleW(std::ptr::null()) as usize;
            if pe_stamp(base) != STAMP {
                log(
                    "boot hold: not the client studied (PE stamp) - refused, the window shows as before",
                );
                return;
            }
            let slot = imports(base as *const u8)
                .find(|d| d.name().to_bytes().eq_ignore_ascii_case(b"user32.dll"))
                .and_then(|d| d.find(By::Name(c"ShowWindow")));
            let user32 = GetModuleHandleA(c"user32.dll".as_ptr().cast());
            let real =
                GetProcAddress(user32, c"ShowWindow".as_ptr().cast()).map_or(0, |f| f as usize);
            let Some(slot) =
                slot.filter(|&s| s as usize == base + SHOW_SLOT && s.read() == real && real != 0)
            else {
                log(&format!(
                    "boot hold: ShowWindow slot at {:08X} is not user32's ShowWindow {real:08X} - refused, the window shows as before",
                    base + SHOW_SLOT
                ));
                return;
            };
            REAL.store(real, Relaxed);
            BASE.store(base, Relaxed);
            let stub: Show = show_stub;
            if redirect(slot, stub as usize).is_none() {
                log(
                    "boot hold: ShowWindow slot not writable - refused, the window shows as before",
                );
                return;
            }
            SLOT.store(slot as usize, Relaxed);
        }
        log("boot hold: ShowWindow import patched - the window waits for the boot screens");
    }

    /// The import's target: the caller's return address goes to [`show`] as a first argument (stdcall: `show` pops it
    /// with the two the caller pushed).
    #[unsafe(naked)]
    unsafe extern "system" fn show_stub(_: HWND, _: i32) -> BOOL {
        core::arch::naked_asm!("push dword ptr [esp]", "jmp {f}", f = sym show);
    }

    unsafe extern "system" fn show(ret: usize, h: HWND, cmd: i32) -> BOOL {
        // SAFETY: REAL is user32's ShowWindow, stored before the slot pointed here
        let real = unsafe { std::mem::transmute::<usize, Show>(REAL.load(Relaxed)) };
        // SAFETY: plain window calls on the caller's window
        unsafe {
            let popup = GetWindowLongW(h, GWL_STYLE) as u32 & WS_POPUP != 0;
            if !holds(ret, BASE.load(Relaxed), cmd, popup) || SetPropW(h, PROP, 1 as _) == 0 {
                return real(h, cmd);
            }
        }
        let hw = h as usize;
        // no thread for the fallback: no hold either
        if std::thread::Builder::new()
            .spawn(move || {
                std::thread::sleep(FALLBACK);
                if let Some(line) = fallback(hw as HWND) {
                    log(line);
                }
            })
            .is_err()
        {
            // SAFETY: as above
            unsafe {
                RemovePropW(h, PROP);
                return real(h, cmd);
            }
        }
        put_back();
        log(&format!(
            "boot hold: window {hw:08X} kept hidden until the boot screens are done (ShowWindow from {ret:08X})"
        ));
        0 // "was hidden", as the real call says for a new window
    }

    /// The slot back to user32's ShowWindow, if it still points here.
    fn put_back() {
        let slot = SLOT.swap(0, Relaxed) as *mut usize;
        let stub: Show = show_stub;
        // SAFETY: the slot patched in `start`, in the loaded program
        if !slot.is_null() && unsafe { slot.read() } == stub as usize {
            // SAFETY: as above; REAL has the import's signature
            unsafe { redirect(slot, REAL.load(Relaxed)) };
        }
    }

    /// The window shown if it is still held; the log line. The panel holds it ([`BOOT_PANEL`]): the panel is told to
    /// show it, on the window's thread and with the projector setting. Otherwise the property is taken off first (so
    /// only one side shows it) and the window shows without taking the focus.
    fn fallback(h: HWND) -> Option<&'static str> {
        // SAFETY: plain window calls; a window gone since has no property
        unsafe {
            if GetPropW(h, PROP) as usize == BOOT_PANEL {
                PostMessageW(h, WM_BOOT_SHOW, 1, 0);
                return Some(
                    "boot hold: no reveal from the panel in 60 s - the panel is told to show the window (fallback)",
                );
            }
            if RemovePropW(h, PROP).is_null() {
                return None;
            }
            ShowWindowAsync(h, SW_SHOWNOACTIVATE);
        }
        Some("boot hold: no reveal from the panel in 60 s - window shown (fallback)")
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, GetPropW, IsWindowVisible, MSG,
            PM_REMOVE, PeekMessageW, SW_HIDE, SW_MAXIMIZE, WS_OVERLAPPEDWINDOW,
        };
        use windows_sys::core::w;

        const BASE: usize = 0x00D5_0000;

        #[test]
        fn holds_only_the_window_functions_normal_show() {
            let site = BASE + 0x00CC_88B1; // the return address after `call ebx` at 0x10C88AF
            assert!(holds(site, BASE, SW_SHOWNORMAL, false));
            assert!(holds(BASE + WINDOW_FN.start, BASE, SW_SHOWNORMAL, false));
            assert!(!holds(BASE + WINDOW_FN.end, BASE, SW_SHOWNORMAL, false));
            assert!(!holds(
                BASE + WINDOW_FN.start - 1,
                BASE,
                SW_SHOWNORMAL,
                false
            ));
            assert!(!holds(site, BASE, SW_HIDE, false)); // sub_10C8590's hide of other screens
            assert!(!holds(site, BASE, SW_MAXIMIZE, false));
            assert!(!holds(site, BASE, SW_SHOWNORMAL, true)); // exclusive full screen
            assert!(!holds(0x010C_88B1, BASE, SW_SHOWNORMAL, false)); // the IDA address, not rebased
            assert!(!holds(0x1000, BASE, SW_SHOWNORMAL, false)); // below the program
        }

        fn window() -> HWND {
            // SAFETY: a plain hidden test window of a system class
            let h = unsafe {
                CreateWindowExW(
                    0,
                    w!("STATIC"),
                    w!("boot test"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    200,
                    100,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!h.is_null());
            h
        }

        fn pump() {
            // SAFETY: this thread's own messages
            unsafe {
                let mut m: MSG = std::mem::zeroed();
                for _ in 0..20 {
                    while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                        DispatchMessageW(&m);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }

        #[test]
        fn fallback_shows_a_held_window_once_and_says_so() {
            let h = window();
            // SAFETY: our test window
            unsafe {
                assert_ne!(SetPropW(h, PROP, 1 as _), 0);
                let line = fallback(h).expect("held: shown");
                assert!(line.contains("window shown (fallback)"), "{line}");
                pump();
                assert_ne!(IsWindowVisible(h), 0);
                assert!(GetPropW(h, PROP).is_null());
                assert_eq!(fallback(h), None, "already shown: nothing to do");
                DestroyWindow(h);
            }
        }

        #[test]
        fn fallback_tells_the_panel_when_it_holds_the_window() {
            let h = window();
            // SAFETY: our test window, on this thread: the posted message waits in its queue
            unsafe {
                assert_ne!(SetPropW(h, PROP, BOOT_PANEL as _), 0);
                let line = fallback(h).expect("held by the panel: told");
                assert!(line.contains("the panel is told"), "{line}");
                let mut m: MSG = std::mem::zeroed();
                assert_ne!(
                    PeekMessageW(&mut m, h, WM_BOOT_SHOW, WM_BOOT_SHOW, PM_REMOVE),
                    0
                );
                assert_eq!(m.wParam, 1, "marked as the fallback");
                pump();
                assert_eq!(IsWindowVisible(h), 0, "the panel shows it, not wccfnet");
                assert_eq!(
                    GetPropW(h, PROP) as usize,
                    BOOT_PANEL,
                    "the property stays for the panel"
                );
                DestroyWindow(h);
            }
        }

        #[test]
        fn fallback_leaves_a_window_the_panel_showed() {
            let h = window();
            // SAFETY: our test window
            unsafe {
                assert_eq!(fallback(h), None);
                pump();
                assert_eq!(IsWindowVisible(h), 0);
                DestroyWindow(h);
            }
        }

        static SEEN: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "system" fn fake_show(h: HWND, cmd: i32) -> BOOL {
            SEEN.store(h as usize ^ cmd as usize, Relaxed);
            7
        }

        #[test]
        fn stub_passes_other_calls_through_with_the_stack_kept() {
            let fake: Show = fake_show;
            REAL.store(fake as usize, Relaxed);
            super::BASE.store(usize::MAX - 0x0100_0000, Relaxed); // every return address is below it: never held
            let h = window();
            let stub: Show = show_stub;
            // SAFETY: the stub with the import's signature; REAL is set
            let r = unsafe { stub(h, SW_MAXIMIZE) };
            assert_eq!(r, 7);
            assert_eq!(SEEN.load(Relaxed), h as usize ^ SW_MAXIMIZE as usize);
            // SAFETY: our test window
            unsafe {
                assert!(GetPropW(h, PROP).is_null());
                DestroyWindow(h);
            }
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
        // SAFETY: `real` is the import's own target (set before the slot pointed here), with the hook's signature
        unsafe {
            let f = std::mem::transmute::<usize, Xfer>(real.load(Relaxed));
            // patched only after WS is set; without it the call goes through untraced
            let Some(ws) = WS.get() else {
                return f(s, buf, len, flags);
            };
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
