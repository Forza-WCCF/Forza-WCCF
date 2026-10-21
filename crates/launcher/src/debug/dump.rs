//! A memory snapshot at a crash, so every crash leaves enough to find its cause.  The report printed at a crash says
//! WHERE (address, registers, the callers); the snapshot holds what WHY needs.  Two per crash, in data\dumps (beside data\logs, so a start's log
//! rotation never zips them): _lean = every thread's stack and the memory it points to (a cabinet ~1 MB, ~90 KB zipped:
//! SEND LOGS and the automatic report carry it) and _full = also the program's globals (the server 3.5 MB, a cabinet
//! 36 MB: kept on this PC).  The newest `DUMPS_KEEP` of each kind stay.  Open with Visual Studio or WinDbg (.effmach x86).
//!
//! dbghelp's MiniDumpWriteDump through windows-sys, not the minidump-writer crate: on Windows that crate is a wrapper of
//! the same call that needs a crash context (EXCEPTION_POINTERS inside the crashed process, which a debugger does not
//! have), opens the process again by pid with PROCESS_ALL_ACCESS, and brings goblin, serde and a dozen other crates.

use std::fs;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;
use std::time::SystemTime;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Diagnostics::Debug::{MINIDUMP_TYPE, MiniDumpWriteDump};

/// Handles, unloaded modules, indirectly referenced memory, process/thread data, thread info.
const DUMP_LEAN: MINIDUMP_TYPE = 0x1164;
/// The lean one plus the data segments (the globals).
const DUMP_FULL: MINIDUMP_TYPE = DUMP_LEAN | 0x1;
const DUMPS_KEEP: usize = 5;

/// Both snapshots of a process the debugger holds stopped, into `folder` -> a line for the log.  Never fails: a report
/// must not stop the debugger.
pub(crate) fn write_dumps(hproc: HANDLE, pid: u32, name: &str, why: &str, folder: &Path) -> String {
    if let Err(e) = fs::create_dir_all(folder) {
        return format!("memory snapshot ({why}) not written: {e}");
    }
    let t = crate::logs::local(SystemTime::now());
    let stem = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}_{}_{pid}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name)
    );
    let mut made = Vec::new();
    for (kind, flags) in [("lean", DUMP_LEAN), ("full", DUMP_FULL)] {
        let path = folder.join(format!("{stem}_{kind}.dmp"));
        let file = match fs::File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                made.push(format!("{kind}: cannot create ({})", e.raw_os_error().unwrap_or(0)));
                continue;
            }
        };
        // SAFETY: `hproc` is the debuggee's handle from its debug event (all access), the file handle is live; no
        // exception, user-stream or callback parameters.
        let ok = unsafe {
            MiniDumpWriteDump(
                hproc,
                pid,
                file.as_raw_handle(),
                flags,
                ptr::null(),
                ptr::null(),
                ptr::null(),
            )
        } != 0;
        let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
        drop(file);
        made.push(if ok {
            format!("{kind} {} KB", fs::metadata(&path).map_or(0, |m| m.len()) / 1024)
        } else {
            format!("{kind}: failed 0x{err:08X}")
        });
        let suffix = format!("_{kind}.dmp");
        let mut old: Vec<_> = fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(&suffix))
            .collect();
        old.sort();
        for n in &old[..old.len().saturating_sub(DUMPS_KEEP)] {
            let _ = fs::remove_file(folder.join(n));
        }
    }
    format!("memory snapshot ({why}): data\\dumps\\{stem}_* - {}", made.join(", "))
}
