//! One log per process, `mxhook_<pid>.log` in the folder MXHOOK_LOG_DIR names (the Launcher sets it; unset = no log).
//! A line is formatted on the stack (the MSGPARSE guard logs from inside an exception handler) and flushed at once:
//! the game may fault next.

use std::fmt;
use std::io::{Cursor, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use windows_sys::Win32::Foundation::{
    GENERIC_WRITE, GetLastError, INVALID_HANDLE_VALUE, SetLastError,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FlushFileBuffers, WriteFile,
};
use windows_sys::Win32::System::Environment::GetEnvironmentVariableW;

static FILE: AtomicUsize = AtomicUsize::new(0);
static LOCK: Mutex<()> = Mutex::new(());

/// Opens the log on the stack only, as the C did: DllMain must not abort on a failed allocation.
pub fn open() {
    const DIR: usize = 400;
    let mut wide = [0u16; DIR + 32];
    // SAFETY: a NUL-terminated name and a buffer of DIR characters
    let n = unsafe {
        GetEnvironmentVariableW(
            windows_sys::w!("MXHOOK_LOG_DIR"),
            wide.as_mut_ptr(),
            DIR as u32,
        )
    } as usize;
    if n == 0 || n >= DIR {
        return; // unset, empty or too long: no log
    }
    let mut name = [0u8; 31];
    let mut c = Cursor::new(&mut name[..]);
    let _ = write!(c, "\\mxhook_{}.log", std::process::id()); // at most 26 bytes
    let len = c.position() as usize;
    let name = &name[..len];
    for (w, &b) in wide[n..].iter_mut().zip(name) {
        *w = u16::from(b);
    }
    wide[n + name.len()] = 0;
    // SAFETY: a NUL-terminated path; the handle lives as long as the process
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if h != INVALID_HANDLE_VALUE {
        FILE.store(h as usize, Relaxed);
    }
}

/// Is the environment variable `name` set (and not empty)? No allocation (DllMain).
pub fn env_set(name: *const u16) -> bool {
    // SAFETY: a NUL-terminated name; a zero-size query writes nothing
    unsafe { GetEnvironmentVariableW(name, std::ptr::null_mut(), 0) > 0 }
}

/// Writes one line; the caller's last-error survives it.
pub fn line(args: fmt::Arguments) {
    let h = FILE.load(Relaxed);
    if h == 0 {
        return;
    }
    let mut buf = [0u8; 1200];
    let mut c = Cursor::new(&mut buf[..1198]);
    let _ = c.write_fmt(args); // a longer line is cut
    let n = c.position() as usize;
    buf[n..n + 2].copy_from_slice(b"\r\n");
    let _held = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: our own log handle and a buffer we own
    unsafe {
        let err = GetLastError();
        let mut put = 0;
        WriteFile(
            h as _,
            buf.as_ptr(),
            (n + 2) as u32,
            &mut put,
            std::ptr::null_mut(),
        );
        FlushFileBuffers(h as _);
        SetLastError(err);
    }
}

/// `log!("...", args)`: one line in this process's log.
macro_rules! log {
    ($($a:tt)*) => { $crate::log::line(format_args!($($a)*)) };
}
pub(crate) use log;

/// A NUL-terminated string from the game, shown as text (`(null)` for none).
pub struct Text(pub *const u8);

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0.is_null() {
            return f.write_str("(null)");
        }
        // SAFETY: the game passes NUL-terminated strings
        let s = unsafe { std::ffi::CStr::from_ptr(self.0.cast()) };
        f.write_str(&String::from_utf8_lossy(s.to_bytes()))
    }
}

/// A NUL-terminated UTF-16 string from the game.
pub struct Wide(pub *const u16);

impl Wide {
    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        let mut i = 0;
        let units = std::iter::from_fn(move || {
            if self.0.is_null() {
                return None;
            }
            // SAFETY: the game passes NUL-terminated strings
            let u = unsafe { self.0.add(i).read() };
            i += 1;
            (u != 0).then_some(u)
        });
        char::decode_utf16(units).map(|c| c.unwrap_or('?'))
    }
}

impl fmt::Display for Wide {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0.is_null() {
            return f.write_str("(null)");
        }
        self.chars().try_for_each(|c| fmt::Write::write_char(f, c))
    }
}
