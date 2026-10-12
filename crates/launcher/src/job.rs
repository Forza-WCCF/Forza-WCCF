//! One Job object holds every process of a run (roadmap step 6).  It has `KILL_ON_JOB_CLOSE`, so when the last handle
//! goes - `Job` dropped, Launcher.exe crashed or killed - Windows ends every process in it.  Grandchildren join the job
//! on their own, and no process may break away.  `stop` is the ordered quit: `WM_CLOSE` to the run's windows, a wait,
//! then `TerminateJobObject`.  Nothing here goes by process name or by a remembered pid: the job is the list.

use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use std::{io, mem, ptr};

use windows_sys::Win32::Foundation::{FALSE, HANDLE, INVALID_HANDLE_VALUE, LPARAM, TRUE, WPARAM};
use windows_sys::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectAssociateCompletionPortInformation, JobObjectBasicAccountingInformation, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
};

use crate::watch::owned;

#[link(name = "ntdll")]
unsafe extern "system" {
    /// Undocumented but stable since NT 5: resumes every thread of the process (here: its one suspended thread).
    fn NtResumeProcess(process: HANDLE) -> i32;
}

/// How long `stop` waits for the job to empty after `TerminateJobObject`.
const AFTER_TERMINATE: Duration = Duration::from_secs(5);

pub struct Job {
    job: OwnedHandle,
    /// Completion port: the job posts to it as processes come and go, so waits sleep until something changes.
    port: OwnedHandle,
}

/// What `stop` did: processes that left after `WM_CLOSE`, and processes that had to be terminated.
#[derive(Debug, PartialEq, Eq)]
pub struct Stop {
    pub closed: usize,
    pub terminated: usize,
}

impl Job {
    pub fn new() -> io::Result<Self> {
        // SAFETY: null attributes and name are allowed; the returned handles are checked and owned.
        unsafe {
            let job = owned(CreateJobObjectW(ptr::null(), ptr::null()))?;
            let port = owned(CreateIoCompletionPort(INVALID_HANDLE_VALUE, ptr::null_mut(), 0, 1))?;
            // Zeroed: no breakaway, no other limits.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let assoc = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
                CompletionKey: ptr::null_mut(),
                CompletionPort: port.as_raw_handle(),
            };
            set(&job, JobObjectExtendedLimitInformation, &limits)?;
            set(&job, JobObjectAssociateCompletionPortInformation, &assoc)?;
            Ok(Self { job, port })
        }
    }

    /// Starts `cmd` in the job with no window, and returns its handle.  The child is created suspended and resumed once
    /// it is in the job, so it cannot start a grandchild outside it.  This sets the creation flags (`CREATE_NO_WINDOW`).
    pub fn spawn(&self, cmd: &mut Command) -> io::Result<Child> {
        let mut child = cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED).spawn()?;
        let h = child.as_raw_handle();
        // SAFETY: `h` is the live process handle that `child` owns, and the job handle is ours.
        let ok = unsafe { AssignProcessToJobObject(self.job.as_raw_handle(), h) != 0 && NtResumeProcess(h) >= 0 };
        if ok {
            Ok(child)
        } else {
            let err = io::Error::last_os_error();
            let _ = child.kill();
            Err(err)
        }
    }

    /// Processes in the job now (grandchildren included).
    pub fn active(&self) -> usize {
        // SAFETY: the buffer is the struct the class asks for.
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = mem::zeroed();
            query(&self.job, JobObjectBasicAccountingInformation, &mut info);
            info.ActiveProcesses as usize
        }
    }

    /// Waits for the job to empty: true if it did within `timeout`.
    pub fn wait_empty(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        loop {
            if self.active() == 0 {
                return true;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            let (mut n, mut key, mut ov): (u32, usize, *mut OVERLAPPED) = (0, 0, ptr::null_mut());
            // SAFETY: the out-pointers are live locals.  Any message (or the 0.5 s slice) wakes the loop to recount;
            // the slice covers a message the job dropped.
            unsafe {
                GetQueuedCompletionStatus(
                    self.port.as_raw_handle(),
                    &mut n,
                    &mut key,
                    &mut ov,
                    left.min(Duration::from_millis(500)).as_millis() as u32,
                );
            }
        }
    }

    /// The ordered stop: `WM_CLOSE` to every visible top-level window of the job's processes, wait up to `deadline`
    /// for the job to empty, then terminate what is left.
    pub fn stop(&self, deadline: Duration) -> Stop {
        let before = self.active();
        let pids = self.pids();
        // SAFETY: `post_close` only reads the `&[u32]` behind the pointer, which outlives the call.
        unsafe { EnumWindows(Some(post_close), &pids as *const Vec<u32> as LPARAM) };
        let mut alive = 0;
        if !self.wait_empty(deadline) {
            alive = self.active();
            // SAFETY: the job handle is ours.
            unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) };
            self.wait_empty(AFTER_TERMINATE);
        }
        Stop {
            closed: before.saturating_sub(alive),
            terminated: alive,
        }
    }

    fn pids(&self) -> Vec<u32> {
        #[repr(C)]
        struct List {
            assigned: u32,
            listed: u32,
            ids: [usize; 256],
        }
        // SAFETY: the buffer is a `JOBOBJECT_BASIC_PROCESS_ID_LIST` with room for 256 ids; more are cut off (they
        // still get terminated, only their windows miss the `WM_CLOSE`).
        unsafe {
            let mut list: List = mem::zeroed();
            query(&self.job, JobObjectBasicProcessIdList, &mut list);
            list.ids[..list.listed as usize].iter().map(|&p| p as u32).collect()
        }
    }
}

/// `EnumWindows` callback: `WM_CLOSE` to the visible window if its process is in the list in `lparam`.
unsafe extern "system" fn post_close(hwnd: HANDLE, lparam: LPARAM) -> i32 {
    // SAFETY: `lparam` is the `&Vec<u32>` that `stop` passed, alive during the enumeration.
    let pids = unsafe { &*(lparam as *const Vec<u32>) };
    let mut pid = 0;
    // SAFETY: `hwnd` comes from EnumWindows; `pid` is a live local.
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
        if IsWindowVisible(hwnd) != 0 && pids.contains(&pid) {
            PostMessageW(hwnd, WM_CLOSE, 0 as WPARAM, 0);
        }
    }
    TRUE
}

/// # Safety
/// `T` is the structure `class` expects.
unsafe fn set<T>(job: &OwnedHandle, class: i32, info: &T) -> io::Result<()> {
    // SAFETY: as the caller promises.
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            class,
            (info as *const T).cast(),
            mem::size_of::<T>() as u32,
        )
    } == FALSE
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// # Safety
/// `T` is the structure `class` fills.
unsafe fn query<T>(job: &OwnedHandle, class: i32, info: &mut T) {
    // SAFETY: as the caller promises.  A failure leaves the zeroed struct (no processes) - or, for the pid list, a
    // truncated one (ERROR_MORE_DATA fills as many ids as fit).
    unsafe {
        QueryInformationJobObject(
            job.as_raw_handle(),
            class,
            (info as *mut T).cast(),
            mem::size_of::<T>() as u32,
            ptr::null_mut(),
        )
    };
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::{env, thread};

    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PostQuitMessage, RegisterClassW,
        SW_SHOWNA, ShowWindow, WM_DESTROY, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };

    use super::*;
    use crate::watch::wide;

    const MODE: &str = "WCCF_JOB_STAND_IN";

    unsafe extern "system" fn polite(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> isize {
        if m == WM_DESTROY {
            // SAFETY: plain call on the message-loop thread.
            unsafe { PostQuitMessage(0) };
        }
        // SAFETY: the arguments are the ones this procedure got.
        unsafe { DefWindowProcW(h, m, w, l) }
    }

    unsafe extern "system" fn deaf(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> isize {
        // SAFETY: as in `polite`.
        unsafe { if m == WM_CLOSE { 0 } else { polite(h, m, w, l) } }
    }

    /// The stand-ins are this test executable again (`WCCF_JOB_STAND_IN` set): a visible window that quits on
    /// `WM_CLOSE`, one that ignores it, a windowless sleeper with a sleeping grandchild.  Each prints a line when ready.
    #[test]
    #[allow(clippy::zombie_processes)] // the grandchild is meant to outlive this process unless the job ends it
    fn stand_in_entry() {
        let Ok(mode) = env::var(MODE) else { return };
        if mode == "hang" {
            let me = env::current_exe().unwrap();
            Command::new(me)
                .args(["stand_in_entry", "--nocapture"])
                .env(MODE, "sleep")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
        }
        if mode == "polite" || mode == "deaf" {
            let proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> isize =
                if mode == "polite" { polite } else { deaf };
            let class = wide("wccf-job-stand-in");
            // SAFETY: zeroed WNDCLASSW plus the fields set is a valid class; the strings outlive the calls.
            unsafe {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(proc),
                    hInstance: GetModuleHandleW(ptr::null()),
                    lpszClassName: class.as_ptr(),
                    ..mem::zeroed()
                };
                RegisterClassW(&wc);
                let w = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    200,
                    100,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    wc.hInstance,
                    ptr::null(),
                );
                assert!(!w.is_null());
                ShowWindow(w, SW_SHOWNA);
            }
        }
        println!("ready");
        io::stdout().flush().unwrap();
        if mode == "polite" || mode == "deaf" {
            // SAFETY: a standard message loop.
            unsafe {
                let mut msg: MSG = mem::zeroed();
                while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
                    DispatchMessageW(&msg);
                }
            }
        } else {
            thread::sleep(Duration::from_secs(3600));
        }
    }

    fn stand_in(job: &Job, mode: &str) -> Child {
        let mut cmd = Command::new(env::current_exe().unwrap());
        cmd.args(["stand_in_entry", "--nocapture"])
            .env(MODE, mode)
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        let mut child = job.spawn(&mut cmd).unwrap();
        // libtest prints its own banner first; the pipe stays open (a closed one fails the stand-in's test report).
        let ready = BufReader::new(child.stdout.as_mut().unwrap())
            .lines()
            .map_while(Result::ok)
            .any(|l| l == "ready");
        assert!(ready, "stand-in {mode} did not get ready");
        child
    }

    #[test]
    fn stop_closes_the_polite_and_terminates_the_rest() {
        let job = Job::new().unwrap();
        let mut polite = stand_in(&job, "polite");
        let mut deaf = stand_in(&job, "deaf");
        let mut hang = stand_in(&job, "hang");
        thread::sleep(Duration::from_millis(500)); // the hang's grandchild has started by its "ready"; this is for the windows
        // Each console stand-in also has a conhost.exe in the job, which leaves with its client: counts are lower bounds.
        assert!(job.active() >= 4);
        // Starting a stand-in takes about 5 s on this PC (process creation is slow); the 30 s bound is for the stop.
        let t = Instant::now();
        let stop = job.stop(Duration::from_secs(3));
        assert!(stop.terminated >= 3, "{stop:?}"); // deaf, hang, hang's grandchild
        assert_eq!(job.active(), 0);
        assert_eq!(polite.wait().unwrap().code(), Some(0));
        assert_ne!(deaf.wait().unwrap().code(), Some(0));
        assert_ne!(hang.wait().unwrap().code(), Some(0));
        assert!(t.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn dropping_the_job_kills_its_processes() {
        let job = Job::new().unwrap();
        let mut hang = stand_in(&job, "hang");
        drop(job);
        let t = Instant::now();
        while hang.try_wait().unwrap().is_none() {
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "still running after the job closed"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}
