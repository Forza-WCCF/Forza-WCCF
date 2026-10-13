//! What runs on this PC, as the run sees it: processes by full path, their windows, the TCP ports something listens on
//! and the named pipes.  Read-only except `kill` and showing or hiding a window.  Port of kit_common.py's processes,
//! same_form, game_processes, kill, listening_ports, pipes, windows_of, top_windows, hide_windows and show_windows,
//! with the Prefer-System32 check the 2010-11 run refuses to start under.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::{fs, mem, ptr};

use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HWND, INVALID_HANDLE_VALUE, LPARAM, TRUE};
use windows_sys::Win32::NetworkManagement::IpHelper::{GetTcpTable, MIB_TCP_STATE_LISTEN, MIB_TCPROW_LH};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::SystemServices::PROCESS_MITIGATION_IMAGE_LOAD_POLICY;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessMitigationPolicy, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, ProcessImageLoadPolicy, QueryFullProcessImageNameW, TerminateProcess,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SW_HIDE, SW_SHOWNOACTIVATE,
    ShowWindowAsync,
};
use windows_sys::core::BOOL;

/// The game's programs, lower case.
pub const GAME_EXES: [&str; 5] = [
    "client_release.exe",
    "control_release.exe",
    "match_release.exe",
    "logowin.exe",
    "fpr_emu.exe",
];

/// The window classes `show_windows` brings back: a dialog and a console (what the server and the scripts open).
const SHOWN_CLASSES: [&str; 2] = ["#32770", "ConsoleWindowClass"];

/// One running process: its id, its parent's, its file name and its full path (empty if Windows would not say).
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    pub path: PathBuf,
}

/// Every process, with the full path of those this user may look at.
pub fn processes() -> Vec<Proc> {
    let mut out = Vec::new();
    // SAFETY: the snapshot handle is checked and closed once; the entry is plain data with its size set.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push(Proc {
                pid: e.th32ProcessID,
                parent: e.th32ParentProcessID,
                name: String::from_utf16_lossy(&e.szExeFile[..len]),
                path: PathBuf::new(),
            });
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    for p in &mut out {
        p.path = image(p.pid).unwrap_or_default();
    }
    out
}

/// The full path of a process's program, as Windows reports it (a program started through a junction: under the
/// junction's target).
pub fn image(pid: u32) -> Option<PathBuf> {
    // SAFETY: the handle is checked and closed once; the buffer and its length go together.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| PathBuf::from(OsString::from_wide(&buf[..len as usize])))
    }
}

/// A path with Windows' case folded and `/` as `\` (os.path.normcase): the form to compare a process's path in.
pub fn normcase(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

/// A path in the form Windows reports a running program's: absolute, junctions and links resolved, case folded
/// (os.path.normcase(os.path.realpath(..))).  A kit started through its first-run link C:\Games\Forza-WCCF\2010-2011
/// runs programs that Windows reports under the link's target, so a path made from the kit's folder is put in this
/// form before it is compared with a process's (2026-10-12: "stop" found none of the kit's helpers).  The part that
/// does not exist is kept as it is.
pub fn same_form(path: &Path) -> String {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut rest = Vec::new();
    let mut at = full.as_path();
    let resolved = loop {
        if let Ok(real) = fs::canonicalize(at) {
            break real;
        }
        match (at.parent(), at.file_name()) {
            (Some(up), Some(name)) => {
                rest.push(name);
                at = up;
            }
            _ => break full.clone(),
        }
    };
    let resolved = rest.iter().rev().fold(resolved, |p, n| p.join(n));
    let text = resolved.to_string_lossy();
    // canonicalize answers in the \\?\ form; Python's realpath and Windows' process paths do not
    let text = match text.strip_prefix(r"\\?\UNC\") {
        Some(unc) => format!(r"\\{unc}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned(),
    };
    normcase(Path::new(&text))
}

/// The game's programs running from this game's folders (the game folder and the seat1 and misc folders beside it).
pub fn game_processes(game: &Path) -> Vec<Proc> {
    let game = std::path::absolute(game).unwrap_or_else(|_| game.to_path_buf());
    let root = same_form(game.parent().unwrap_or(&game)) + "\\";
    processes()
        .into_iter()
        .filter(|p| GAME_EXES.contains(&p.name.to_lowercase().as_str()) && normcase(&p.path).starts_with(&root))
        .collect()
}

/// A program running from `dir` (in the form Windows reports it), this process aside: Launcher.exe's UPDATE writes
/// over the kit only when there is none.
pub fn running_from(dir: &Path) -> Option<PathBuf> {
    let root = same_form(dir) + "\\";
    let me = std::process::id();
    processes()
        .into_iter()
        .find(|p| p.pid != me && normcase(&p.path).starts_with(&root))
        .map(|p| p.path)
}

/// Ends a process at once (no questions to the program): true if Windows did.
pub fn kill(pid: u32) -> bool {
    // SAFETY: the handle is checked and closed once.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, FALSE, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1) != 0;
        CloseHandle(h);
        ok
    }
}

/// A top-level window: its handle, class name, title and whether it is visible.
pub struct Window {
    pub hwnd: HWND,
    pub class: String,
    pub title: String,
    pub visible: bool,
}

/// Every top-level window of a process (a console counts as the program's that writes to it).
pub fn windows(pid: u32) -> Vec<Window> {
    struct Find {
        pid: u32,
        found: Vec<Window>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        // SAFETY: lp is the &mut Find given to EnumWindows below, alive for the whole enumeration.
        let find = unsafe { &mut *(lp as *mut Find) };
        let mut owner = 0;
        // SAFETY: a window handle Windows gave; the buffers and their lengths go together.
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut owner);
            if owner == find.pid {
                let (mut class, mut title) = ([0u16; 256], [0u16; 256]);
                let c = GetClassNameW(hwnd, class.as_mut_ptr(), 256).max(0) as usize;
                let t = GetWindowTextW(hwnd, title.as_mut_ptr(), 256).max(0) as usize;
                find.found.push(Window {
                    hwnd,
                    class: String::from_utf16_lossy(&class[..c]),
                    title: String::from_utf16_lossy(&title[..t]),
                    visible: IsWindowVisible(hwnd) != 0,
                });
            }
        }
        TRUE
    }
    let mut find = Find { pid, found: Vec::new() };
    // SAFETY: `find` outlives the call; `each` only touches it through lp.
    unsafe { EnumWindows(Some(each), (&raw mut find) as LPARAM) };
    find.found
}

/// The process has a visible top-level window (kit_common.windows_of, as play.py uses it).
pub fn has_window(pid: u32) -> bool {
    windows(pid).iter().any(|w| w.visible)
}

/// Hides every visible top-level window of a process; their titles.  Asynchronous: never waits for the program (a stuck
/// one included), so the windows go a moment later.
pub fn hide_windows(pid: u32) -> Vec<String> {
    windows(pid)
        .into_iter()
        .filter(|w| w.visible)
        // SAFETY: a window handle Windows just gave.
        .inspect(|w| unsafe {
            ShowWindowAsync(w.hwnd, SW_HIDE);
        })
        .map(|w| w.title)
        .collect()
}

/// Shows a process's hidden dialog and console windows again without taking the keyboard - never its other hidden
/// windows (Windows' own helpers such as "Default IME"); their titles.
pub fn show_windows(pid: u32) -> Vec<String> {
    windows(pid)
        .into_iter()
        .filter(|w| !w.visible && SHOWN_CLASSES.contains(&w.class.as_str()))
        // SAFETY: a window handle Windows just gave.
        .inspect(|w| unsafe {
            ShowWindowAsync(w.hwnd, SW_SHOWNOACTIVATE);
        })
        .map(|w| w.title)
        .collect()
}

/// The IPv4 TCP ports something listens on, from Windows' own table (not netstat's translated text).
pub fn listening_ports() -> Vec<u16> {
    for _ in 0..5 {
        let mut size = 0u32;
        // SAFETY: a size query first (no buffer), then a buffer of the size Windows asked for and room to grow; the
        // table is a row count and that many rows, u32-aligned like the buffer.
        unsafe {
            GetTcpTable(ptr::null_mut(), &mut size, FALSE);
            let mut buf = vec![0u32; (size as usize + 1024) / 4];
            let mut size = (buf.len() * 4) as u32;
            if GetTcpTable(buf.as_mut_ptr().cast(), &mut size, FALSE) != 0 {
                continue;
            }
            let n = buf[0] as usize;
            let rows = buf.as_ptr().add(1).cast::<MIB_TCPROW_LH>();
            return (0..n)
                .map(|i| &*rows.add(i))
                .filter(|r| r.Anonymous.dwState == MIB_TCP_STATE_LISTEN as u32)
                .map(|r| u16::from_be(r.dwLocalPort as u16))
                .collect();
        }
    }
    Vec::new()
}

/// The named pipes that exist now, lower case.
pub fn pipes() -> Vec<String> {
    fs::read_dir(r"\\.\pipe\")
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

/// This process loads only Windows' own copy of a DLL that Windows also has ("Prefer System32 images", which every
/// program it starts inherits).  The 2010-11 game would then load Windows' winmm.dll instead of the kit's hook beside
/// it and run without the guard: seen 2026-10-10 for every program started from a WSL session (wslhost.exe has it
/// on), so the run refuses to start the game then.
pub fn prefers_system32() -> bool {
    let mut policy = PROCESS_MITIGATION_IMAGE_LOAD_POLICY::default();
    // SAFETY: the structure and its size go together; the pseudo-handle needs no closing.
    let ok = unsafe {
        GetProcessMitigationPolicy(
            GetCurrentProcess(),
            ProcessImageLoadPolicy,
            (&raw mut policy).cast(),
            size_of::<PROCESS_MITIGATION_IMAGE_LOAD_POLICY>(),
        )
    };
    // SAFETY: the union is plain flags.
    ok != 0 && unsafe { policy.Anonymous.Flags } & 0b100 != 0 // PreferSystem32Images
}
