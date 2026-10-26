//! FPR_Emu.exe: stands in for the original card-table emulator, which the WCCF 2010-11 client expects but the disc does
//! not have (2026-10-04; checked against the decompile).
//!
//! With FPR_MODE=2 the client (wccf::io::EmulatorFprInterface, FUN_004d2a40) runs
//!     ..\misc\FlatPanelReader_Emulator\exe\FPR_Emu.exe <table> <slots>      (table 0 or 1, slots 32)
//! from its own folder, with this process's stdout set to a pipe it reads. It waits 2 s, then looks ONCE for a
//! visible, unowned window of this process (no retry), then polls about every 10 ms (vfunction2 @0x004d2c90):
//!     SendMessage(window, WM_USER+1)  ->  we write ONE frame to stdout  ->  it reads up to 4096 bytes.
//! Frame: "slot,card,x,y\r\n" for slots 1..16 (empty slot = "slot,-1,0,0"), then "-1,0,0,0\r\n" = commit.
//!   - Never more than 16 slot lines: the game counts them and warns "max 16 players" above that ([`SLOTS`]).
//!   - CR LF ends a line; the game copies each field into an 8-byte slot with NO bound: fields <= 7 characters.
//!   - A commit does not clear the other buffer, so every slot is written every frame.
//!   - Never "-2": it ends the game's reader thread for good.
//!
//! WM_USER+2 = the game is closing -> exit. Also exits when the game process ends or the pipe breaks.
//!
//! Cards: "fpr_table<N>.txt" in the current folder = the game's folder (seat1\ for seat 1; the projector runs from
//! another folder, finds no file and gets empty tables). Lines "slot card x y" (commas or spaces, # comments), slot
//! 1..16, x/y in the table's units (pitch about -1..+1, bench x > 1.16). Re-read when its write time changes; a read
//! that fails (file being swapped) keeps the last good table. Table 1 is always sent empty. Nothing but frames ever
//! goes to stdout: a log goes to fpr_emu_table<N>.log in the same folder.
#![windows_subsystem = "windows"] // the game starts it: no console window; its stdout is still the game's pipe

use std::cell::RefCell;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Write as _;
use windows_sys::Win32::Foundation::{
    FILETIME, GetLastError, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, WAIT_OBJECT_0,
    WPARAM,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileAttributesExW, GetFileExInfoStandard, WIN32_FILE_ATTRIBUTE_DATA, WriteFile,
};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::Environment::GetCommandLineW;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
    PostQuitMessage, RegisterClassW, SW_SHOWNOACTIVATE, SetTimer, ShowWindow, TranslateMessage,
    WM_DESTROY, WM_TIMER, WM_USER, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

const POLL: u32 = WM_USER + 1;
const CLOSE: u32 = WM_USER + 2;
/// 16, not 20 (2026-10-05): the game counts the slot lines of each frame (empty "s,-1,0,0" lines too) into the table
/// interface's +0x138, and FUN_004ed890 (cmp [table0+0x138],0x10 / ja) raises 「1チームに登録できるのは16人までです」 above
/// 16, which blocks the decide button on card arrangement, the Club Make card step and team practice. With 20 lines it
/// was on whatever cards lay on the table. The game reads slots 1-20; 17-20, never sent, stay at their empty start
/// value. The board, the catalogue and the panel use slots 1-16 only.
const SLOTS: usize = 16;
/// Polls between "still polling" log lines (about a minute).
const LOG_EVERY: u64 = 6000;

#[derive(Clone, Copy, Default)]
struct Card {
    card: i32,
    x: f32,
    y: f32,
}

struct State {
    table: i32,
    out: HANDLE,
    path: String,
    stamp: Option<(u32, u32)>,
    cards: [Option<Card>; SLOTS + 1],
    count: usize,
    polls: u64,
    parent: HANDLE,
    log: Option<File>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

impl State {
    fn log(&mut self, msg: std::fmt::Arguments) {
        let t = forza_native::local_time();
        if let Some(f) = self.log.as_mut() {
            let _ = writeln!(
                f,
                "{:02}:{:02}:{:02}.{:03}  {msg}",
                t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
            );
        }
    }

    /// Re-reads the table file if it changed. On any failure it keeps what it had: the panel swaps the file whole.
    fn load_table(&mut self) {
        let wide: Vec<u16> = self.path.encode_utf16().chain([0]).collect();
        // SAFETY: a plain C struct, filled by the call below
        let mut a: WIN32_FILE_ATTRIBUTE_DATA = unsafe { std::mem::zeroed() };
        // SAFETY: a NUL-terminated name and a struct of the asked-for kind
        if unsafe {
            GetFileAttributesExW(wide.as_ptr(), GetFileExInfoStandard, (&raw mut a).cast())
        } == 0
        {
            return;
        }
        let FILETIME {
            dwLowDateTime: lo,
            dwHighDateTime: hi,
        } = a.ftLastWriteTime;
        if self.stamp == Some((lo, hi)) {
            return;
        }
        let Ok(bytes) = std::fs::read(&self.path) else {
            return; // being swapped: the stamp is not taken, so the next poll tries again
        };
        self.cards = parse(&String::from_utf8_lossy(&bytes));
        self.stamp = Some((lo, hi));
        let n = self.cards.iter().flatten().count();
        if n != self.count || self.polls < 2 {
            self.log(format_args!("table file read: {n} card(s)"));
        }
        self.count = n;
    }

    /// One frame to the game; false when it cannot be written (the game is gone).
    fn send_frame(&mut self) -> bool {
        if self.table == 0 {
            self.load_table();
        }
        let f = frame(if self.table == 0 {
            &self.cards
        } else {
            &[None; SLOTS + 1]
        });
        let mut put = 0;
        // SAFETY: the game's pipe and our own buffer
        let ok = unsafe {
            WriteFile(
                self.out,
                f.as_ptr(),
                f.len() as u32,
                &mut put,
                std::ptr::null_mut(),
            )
        } != 0;
        if !ok || put as usize != f.len() {
            // SAFETY: plain Win32 call
            let err = unsafe { GetLastError() };
            self.log(format_args!(
                "writing to the game failed (err {err}) - exiting"
            ));
            return false;
        }
        self.polls += 1;
        if self.polls == 1 {
            let c = self.count;
            self.log(format_args!(
                "first poll answered ({} bytes, {c} card(s))",
                f.len()
            ));
        } else if self.polls.is_multiple_of(LOG_EVERY) {
            let (n, c) = (self.polls, self.count);
            self.log(format_args!("{n} polls answered, {c} card(s)"));
        }
        true
    }
}

/// A coordinate as the game can take it: "%.3f" of it is at most 6 characters.
fn clamp(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-9.99, 9.99)
    }
}

/// The table file's lines "slot card x y" (commas or spaces, # comments); anything else is skipped.
fn parse(text: &str) -> [Option<Card>; SLOTS + 1] {
    let mut cards = [None; SLOTS + 1];
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().replace(',', " ");
        let mut w = line.split_whitespace();
        let (Some(s), Some(c), Some(x), Some(y)) = (w.next(), w.next(), w.next(), w.next()) else {
            continue;
        };
        if let (Ok(slot), Ok(card), Ok(x), Ok(y)) = (
            s.parse::<usize>(),
            c.parse::<i32>(),
            x.parse::<f32>(),
            y.parse::<f32>(),
        ) && (1..=SLOTS).contains(&slot)
            && (1..=999_999).contains(&card)
        {
            cards[slot] = Some(Card {
                card,
                x: clamp(x),
                y: clamp(y),
            });
        }
    }
    cards
}

/// Every slot, then the commit.
fn frame(cards: &[Option<Card>; SLOTS + 1]) -> String {
    let mut f = String::new();
    for (s, c) in cards.iter().enumerate().skip(1) {
        let _ = match c {
            Some(c) => write!(f, "{s},{},{:.3},{:.3}\r\n", c.card, c.x, c.y),
            None => write!(f, "{s},-1,0,0\r\n"),
        };
    }
    f + "-1,0,0,0\r\n"
}

/// The table number from the raw command line: the game's is " %s %d" with NO program name in front (CreateProcessA
/// with the exe as the application name), so argv[0] may be eaten. The second-to-last whole-number word (the last one
/// is the slot count, 32).
fn table_from(cmd: &str) -> i32 {
    let nums: Vec<i32> = cmd
        .split([' ', '\t'])
        .filter_map(|w| w.parse().ok())
        .take(16)
        .collect();
    match nums.len() {
        0 => 0,
        1 => nums[0],
        n => nums[n - 2],
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // the state is let go before DestroyWindow, which comes back here with WM_DESTROY
    let close = STATE.with_borrow_mut(|st| {
        let st = st.as_mut()?;
        match msg {
            POLL => Some(!st.send_frame()),
            CLOSE => {
                let n = st.polls;
                st.log(format_args!(
                    "the game said goodbye (WM_USER+2) after {n} polls"
                ));
                Some(true)
            }
            // the game gone without saying goodbye
            // SAFETY: the parent's wait-only handle
            WM_TIMER => Some(
                !st.parent.is_null()
                    && unsafe { WaitForSingleObject(st.parent, 0) } == WAIT_OBJECT_0
                    && {
                        let n = st.polls;
                        st.log(format_args!(
                            "the game process ended - exiting after {n} polls"
                        ));
                        true
                    },
            ),
            _ => None,
        }
    });
    // SAFETY: our own window
    unsafe {
        match (msg, close) {
            (WM_DESTROY, _) => PostQuitMessage(0),
            (_, Some(true)) => drop(DestroyWindow(hwnd)),
            (_, Some(false)) => {}
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
    0
}

fn main() {
    // SAFETY: the process's own command line, NUL-terminated
    let cmd = unsafe {
        let p = GetCommandLineW();
        String::from_utf16_lossy(std::slice::from_raw_parts(
            p,
            (0..).take_while(|&i| *p.add(i) != 0).count(),
        ))
    };
    // the stdout handle IS the game's pipe; an unknown table is served empty rather than give our cards to the wrong
    // reader
    let table = Some(table_from(&cmd))
        .filter(|t| (0..=1).contains(t))
        .unwrap_or(1);
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (class, title) = (
        wide("WCCF_FPR_Emu"),
        wide(&format!("FPR_Emu table {table}")),
    );
    // SAFETY: a window class and window of our own; the strings outlive the calls
    let (out, hwnd) = unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let inst = GetModuleHandleW(std::ptr::null());
        // the window first: the game looks for it once, about 2 s after starting us
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: inst,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let ex = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let null = std::ptr::null_mut();
        let hwnd = CreateWindowExW(
            ex,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            null,
            null,
            inst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            std::process::exit(1);
        }
        ShowWindow(hwnd, SW_SHOWNOACTIVATE); // visible (the game checks IsWindowVisible), 1x1 px, no taskbar button
        (out, hwnd)
    };
    let ppid = forza_native::parent_pid();
    let mut st = State {
        table,
        out,
        path: format!("fpr_table{table}.txt"),
        stamp: None,
        cards: [None; SLOTS + 1],
        count: 0,
        polls: 0,
        parent: if ppid == 0 {
            std::ptr::null_mut()
        } else {
            // SAFETY: a wait-only handle to the game, for the process's life
            unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, ppid) }
        },
        log: std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(format!("fpr_emu_table{table}.log"))
            .ok(),
    };
    let path = st.path.clone();
    st.log(format_args!(
        "==== FPR_Emu table {table} (command line [{cmd}], argc {}), parent pid {ppid}, stdout {:08X}, cards from .\\{path} ====",
        std::env::args_os().count(),
        out as usize
    ));
    if out.is_null() || out == INVALID_HANDLE_VALUE {
        st.log(format_args!("no stdout handle - the game will get nothing"));
    }
    if table == 0 {
        st.load_table();
    }
    STATE.set(Some(st));
    // SAFETY: our own window and this thread's message queue
    unsafe {
        SetTimer(hwnd, 1, 1000, None);
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    STATE.with_borrow_mut(|st| {
        if let Some(st) = st.as_mut() {
            st.log(format_args!("exit"));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_file_and_the_frame() {
        let cards = parse(
            "# a comment\n3 1234 0.5 -0.25\n4,99,12,-30  # clamped\n17 5 0 0\n2 0 0 0\n1 7 x 0\n5 8 0.1\n",
        );
        let f = frame(&cards);
        let lines: Vec<&str> = f.split("\r\n").collect();
        assert_eq!(
            lines.len(),
            SLOTS + 2,
            "16 slots, the commit and the empty rest"
        );
        assert_eq!(lines[0], "1,-1,0,0", "a word that is no number: no card");
        assert_eq!(lines[1], "2,-1,0,0", "card 0 is no card");
        assert_eq!(lines[2], "3,1234,0.500,-0.250");
        assert_eq!(lines[3], "4,99,9.990,-9.990");
        assert_eq!(lines[4], "5,-1,0,0", "three fields are not a card");
        assert_eq!(lines[16], "-1,0,0,0");
        assert!(
            lines[..17]
                .iter()
                .all(|l| l.split(',').all(|w| w.len() <= 7)),
            "fields of at most 7 characters"
        );
    }

    #[test]
    fn the_table_number() {
        assert_eq!(table_from(r#""FPR_Emu.exe" 0 32"#), 0);
        assert_eq!(
            table_from(" 1 32"),
            1,
            "the game's own line: no program name"
        );
        assert_eq!(table_from("FPR_Emu.exe 7"), 7);
        assert_eq!(table_from("FPR_Emu.exe"), 0);
    }
}
