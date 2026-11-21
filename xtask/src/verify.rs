// ---- verify ----
use super::*;

/// A check that ran and failed, as against xtask failing to run it: `verify` exits 1 on it, 4 on any other error.
#[derive(Debug)]
pub(crate) struct CheckFailed {
    /// the check's name, which `VERIFY FAIL` prints
    pub(crate) stage: String,
    pub(crate) msg: String,
}

impl std::fmt::Display for CheckFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for CheckFailed {}

/// How a `verify` run ended: its exit code and last line.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Pass,
    Fail(String),
    NotRun(String),
    Error(String),
}

impl Verdict {
    pub(crate) fn of(r: &Result<()>) -> Self {
        match r {
            Ok(()) => Self::Pass,
            Err(e) => match e.downcast_ref::<CheckFailed>() {
                Some(f) => Self::Fail(f.stage.clone()),
                None => Self::Error(format!("{e:#}")),
            },
        }
    }

    pub(crate) fn code(&self) -> i32 {
        match self {
            Self::Pass => 0,
            Self::Fail(_) => 1,
            // 3 as run-cargo.ps1 refusing a running game, so a caller reads one code through either
            Self::NotRun(_) => 3,
            Self::Error(_) => 4,
        }
    }

    /// The last line; the stage is one word and every line one line, so a script splits it on spaces.
    pub(crate) fn line(&self, secs: u64) -> String {
        let one = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        match self {
            Self::Pass => format!("VERIFY OK {secs}s"),
            Self::Fail(stage) => format!("VERIFY FAIL {} {secs}s", one(stage).replace(' ', "-")),
            Self::NotRun(why) => format!("VERIFY NOT-RUN {}", one(why)),
            Self::Error(msg) => format!("VERIFY ERROR {}", one(msg)),
        }
    }

    /// The CSV's result column: the word after `VERIFY`, with the stage of a failure.
    pub(crate) fn result(&self) -> String {
        match self {
            Self::Pass => "OK".into(),
            Self::Fail(stage) => format!("FAIL:{}", stage.replace([' ', ','], "-")),
            Self::NotRun(_) => "NOT-RUN".into(),
            Self::Error(_) => "ERROR".into(),
        }
    }
}

/// The programs no build may run beside (AGENTS.md: a release check stalled a live match), lower case.
pub(crate) const GAME_RUNNING: [&str; 4] = [
    "client_release.exe",
    "match_release.exe",
    "control_release.exe",
    "launcher.exe",
];

/// A process that stops a `verify`: a game program, or a Launcher.exe that is not PowerToys' (one whose path Windows
/// would not give counts).
pub(crate) fn blocks_verify(name: &str, path: &str) -> bool {
    GAME_RUNNING.contains(&name.to_lowercase().as_str())
        && !(name.eq_ignore_ascii_case("launcher.exe")
            && path.to_lowercase().contains("\\powertoys\\"))
}

/// Why `verify` must not run now, if it must not.
pub(crate) fn preflight(root: &Path) -> Option<String> {
    if let Some((name, pid, path)) = host::processes()
        .into_iter()
        .find(|(n, _, p)| blocks_verify(n, p))
    {
        return Some(format!("a game process runs: {name} pid={pid} {path}"));
    }
    if host::prefers_system32() {
        return Some(
            "\"Prefer System32 images\" is on (a shell started from WSL): run it through run-cargo.ps1 or a Windows shell"
                .into(),
        );
    }
    let dir = Some(target_dir(root))
        .filter(|d| d.is_dir())
        .unwrap_or_else(|| root.to_owned());
    match host::free_bytes(&dir) {
        Some(free) if free < VERIFY_MIN_FREE => Some(format!(
            "{} GB free on the drive of {} (under {} GB)",
            free >> 30,
            dir.display(),
            VERIFY_MIN_FREE >> 30
        )),
        Some(_) => None,
        None => {
            eprintln!(
                "verify: the free space on the drive of {} is unknown: running anyway",
                dir.display()
            );
            None
        }
    }
}

pub(crate) const VERIFY_MIN_FREE: u64 = 20 << 30;

/// `cargo xtask verify`: the preflight, then `check quick`; prints the last line and returns the exit code.
pub(crate) fn verify(root: &Path, base: &str) -> i32 {
    let start = std::time::Instant::now();
    let verdict = if let Some(why) = preflight(root) {
        Verdict::NotRun(why)
    } else {
        let quick = Task::Check {
            mode: Some("quick".into()),
            base: Some(base.to_owned()),
            fast: false,
            release: false,
            test_args: vec![],
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(quick, root)))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("xtask panicked")));
        if let Err(e) = &r {
            eprintln!("Error: {e:?}");
        }
        Verdict::of(&r)
    };
    let secs = start.elapsed().as_secs();
    if let Err(e) = log_verify(root, base, &verdict, secs) {
        eprintln!("verify: the run log was not written: {e:#}");
    }
    let _ = std::io::stderr().flush();
    println!("{}", verdict.line(secs));
    verdict.code()
}

/// One CSV line per run: FORZA_VERIFY_CSV, else `wccf-builds/verify-runs.csv` beside the repository when that folder
/// exists (the maintainers' build folder), else none.
pub(crate) fn log_verify(root: &Path, base: &str, v: &Verdict, secs: u64) -> Result<()> {
    let csv = match std::env::var_os("FORZA_VERIFY_CSV") {
        Some(p) => PathBuf::from(p),
        None => match root.parent().map(|p| p.join("wccf-builds")) {
            Some(d) if d.is_dir() => d.join("verify-runs.csv"),
            _ => return Ok(()),
        },
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let t = dos_time(i64::try_from(now)?)?;
    let new = !csv.is_file();
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&csv)?;
    if new {
        writeln!(f, "date,worktree,base,result,secs")?;
    }
    writeln!(
        f,
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z,{},{},{},{secs}",
        t.year(),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        root.display(),
        base.replace(',', "-"),
        v.result()
    )?;
    Ok(())
}

/// The few Windows calls the preflight makes; elsewhere (CI's Linux lint job builds xtask) they find nothing.
#[cfg(windows)]
mod host {
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::SystemServices::PROCESS_MITIGATION_IMAGE_LOAD_POLICY;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessMitigationPolicy, OpenProcess, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, ProcessImageLoadPolicy, QueryFullProcessImageNameW,
    };

    /// Every process: its file name, id and full path (empty when Windows would not say); as `forza::procs`.
    pub fn processes() -> Vec<(String, u32, String)> {
        let mut out = Vec::new();
        // SAFETY: the snapshot handle is checked and closed once; the entry is plain data with its size set.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(0);
            let mut ok = Process32FirstW(snap, &raw mut e);
            while ok != 0 {
                let len = e
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..len]);
                out.push((name, e.th32ProcessID, image(e.th32ProcessID)));
                ok = Process32NextW(snap, &raw mut e);
            }
            CloseHandle(snap);
        }
        out
    }

    fn image(pid: u32) -> String {
        // SAFETY: the handle is checked and closed once; the buffer and its length go together.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
            if h.is_null() {
                return String::new();
            }
            let mut buf = vec![0u16; 32_768];
            let mut len = u32::try_from(buf.len()).unwrap_or(0);
            let ok =
                QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &raw mut len);
            CloseHandle(h);
            if ok == 0 {
                return String::new();
            }
            String::from_utf16_lossy(&buf[..len as usize])
        }
    }

    /// "Prefer System32 images" on this process, which every program a WSL session starts inherits (docs/SAFETY.md)
    /// and four Launcher tests fail on.
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

    /// The bytes this user may still write on the drive of `dir`.
    pub fn free_bytes(dir: &Path) -> Option<u64> {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
        let mut free = 0u64;
        // SAFETY: a NUL-terminated path; the other two outputs may be null.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &raw mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(free)
    }
}

#[cfg(not(windows))]
mod host {
    pub fn processes() -> Vec<(String, u32, String)> {
        vec![]
    }
    pub fn prefers_system32() -> bool {
        false
    }
    pub fn free_bytes(_: &std::path::Path) -> Option<u64> {
        None
    }
}
