//! The run's light system sampler: every [`EVERY`] one line in data\logs\run_sampler.txt (about 100 bytes) with the
//! PC's CPU use and free RAM, and the client's and match engines' CPU use, private bytes and page faults since the line
//! before; a WARN line when free RAM drops below [`LOW_RAM_MB`]. It tells a stalled PC from a stalled link.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use forza::standins::Handle;
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, GetSystemTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

pub const EVERY: Duration = Duration::from_secs(10);
/// Below this much free RAM the game may page: a WARN line.
pub const LOW_RAM_MB: u64 = 300;

/// One program's share over a sample: CPU % of the whole PC, private bytes in MB, page faults since the last line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub cpu: u32,
    pub mb: u64,
    pub faults: u64,
}

/// The line: `cpu 23% free 5123 MB | client 12% 812 MB pf +1234 | match x4 9% 840 MB pf +22` (the match engines
/// summed; "-" for a program not running).
pub fn line(cpu: u32, free_mb: u64, client: Option<Usage>, matches: (usize, Usage)) -> String {
    let client = client.map_or("-".into(), |u| format!("{}% {} MB pf +{}", u.cpu, u.mb, u.faults));
    let (n, m) = matches;
    let matches = if n == 0 {
        "-".into()
    } else {
        format!("x{n} {}% {} MB pf +{}", m.cpu, m.mb, m.faults)
    };
    format!("cpu {cpu}% free {free_mb} MB | client {client} | match {matches}")
}

/// The WARN line for `free_mb` of free RAM, if it is low.
pub fn low_ram(free_mb: u64) -> Option<String> {
    (free_mb < LOW_RAM_MB).then(|| format!("WARN free RAM {free_mb} MB, below {LOW_RAM_MB} MB"))
}

fn ticks(t: FILETIME) -> u64 {
    u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)
}

/// (CPU time in 100 ns, private bytes, page faults) of `pid`; None when it cannot be read.
fn process(pid: u32) -> Option<(u64, u64, u64)> {
    // SAFETY: the handle is checked and closed once; the structs are plain data with their sizes set
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let z = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        let mut m: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        m.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0
            && GetProcessMemoryInfo(h, (&raw mut m).cast(), m.cb) != 0;
        CloseHandle(h);
        ok.then(|| (ticks(k) + ticks(u), m.PrivateUsage as u64, u64::from(m.PageFaultCount)))
    }
}

fn free_mb() -> u64 {
    // SAFETY: a zeroed MEMORYSTATUSEX with its length set, as the call asks
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) == 0 {
            return 0;
        }
        m.ullAvailPhys >> 20
    }
}

/// (idle, kernel + user) in 100 ns; kernel time includes idle.
fn system() -> (u64, u64) {
    let z = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut i, mut k, mut u) = (z, z, z);
    // SAFETY: three FILETIMEs to fill
    unsafe { GetSystemTimes(&mut i, &mut k, &mut u) };
    (ticks(i), ticks(k) + ticks(u))
}

/// The sampler until the run drops its handle.
pub fn spawn(logs: PathBuf) -> Handle {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = thread::spawn(move || {
        let file = logs.join("run_sampler.txt");
        let cpus = thread::available_parallelism().map_or(1, std::num::NonZero::get) as u64;
        let (mut last_sys, mut last_at) = (system(), Instant::now());
        // pid -> (CPU time, page faults) at the last line
        let mut last: HashMap<u32, (u64, u64)> = HashMap::new();
        while !flag.load(Ordering::Relaxed) {
            let until = Instant::now() + EVERY;
            while Instant::now() < until && !flag.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(200));
            }
            let (sys, at) = (system(), Instant::now());
            let (idle, total) = (sys.0.saturating_sub(last_sys.0), sys.1.saturating_sub(last_sys.1));
            let cpu = (100 * total.saturating_sub(idle)).checked_div(total).unwrap_or(0) as u32;
            let wall = at.duration_since(last_at).as_nanos() as u64 / 100 * cpus;
            (last_sys, last_at) = (sys, at);
            let (mut client, mut matches, mut seen) = (None, (0, Usage::default()), HashMap::new());
            // shortcut: by program name, not by folder; a second kit's game on the same PC would count too
            for p in crate::procs::snapshot() {
                let name = p.name.to_ascii_lowercase();
                let is_client = name == "client_release.exe";
                if !is_client && name != "match_release.exe" {
                    continue;
                }
                let Some((t, bytes, faults)) = process(p.pid) else {
                    continue;
                };
                let (t0, f0) = last.get(&p.pid).copied().unwrap_or((t, faults));
                seen.insert(p.pid, (t, faults));
                let u = Usage {
                    cpu: (100 * t.saturating_sub(t0)).checked_div(wall).unwrap_or(0) as u32,
                    mb: bytes >> 20,
                    faults: faults.saturating_sub(f0),
                };
                if is_client {
                    client = Some(u);
                } else {
                    matches.0 += 1;
                    matches.1.cpu += u.cpu;
                    matches.1.mb += u.mb;
                    matches.1.faults += u.faults;
                }
            }
            last = seen;
            let free = free_mb();
            crate::logs::append(&file, &line(cpu, free, client, matches));
            if let Some(w) = low_ram(free) {
                crate::logs::append(&file, &w);
            }
        }
    });
    Handle::new(stop, thread)
}

#[cfg(test)]
mod tests {
    use super::{Usage, line, low_ram};

    #[test]
    fn the_sampler_line_is_short_and_says_who_is_missing() {
        let u = Usage {
            cpu: 12,
            mb: 812,
            faults: 1234,
        };
        let m = Usage {
            cpu: 9,
            mb: 840,
            faults: 22,
        };
        let l = line(23, 5123, Some(u), (4, m));
        assert_eq!(
            l,
            "cpu 23% free 5123 MB | client 12% 812 MB pf +1234 | match x4 9% 840 MB pf +22"
        );
        assert!(l.len() <= 100, "{} bytes", l.len());
        assert_eq!(
            line(5, 900, None, (0, Usage::default())),
            "cpu 5% free 900 MB | client - | match -"
        );
    }

    #[test]
    fn free_ram_below_300_mb_warns() {
        assert_eq!(low_ram(299).as_deref(), Some("WARN free RAM 299 MB, below 300 MB"));
        assert_eq!(low_ram(300), None);
    }
}
