//! One run per kit folder: the named mutexes, the admission and the request files other Launcher.exe processes use.

use super::*;

/// A cabinet run (local, remote) or a server run: one of each can run, a remote cabinet beside a server.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Run,
    Server,
}

impl Kind {
    pub(super) fn of(mode: &Mode) -> Self {
        if mode.is_server() { Self::Server } else { Self::Run }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Server => "server",
        }
    }
}

/// The run's mutex name: the kit folder in the form Windows reports it (a start through the first-run link and one
/// from the folder itself are the same kit), FNV-1a over it.
fn lock_name(kit: &Kit, kind: &str) -> Vec<u16> {
    let key = procs::same_form(kit.dir())
        .encode_utf16()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, c| {
            (h ^ u64::from(c)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("Local\\wccf-kit-{kind}-{key:016x}")
        .encode_utf16()
        .chain([0])
        .collect()
}

/// The run's mutex, if no one holds it: held for as long as the run lasts.
pub(super) fn take_lock(kit: &Kit, kind: Kind) -> Option<OwnedHandle> {
    let name = lock_name(kit, kind.name());
    // SAFETY: a NUL-terminated name; the handle is checked, then owned (closed on drop).
    unsafe {
        let h = CreateMutexW(ptr::null(), FALSE, name.as_ptr());
        if h.is_null() {
            return None;
        }
        let taken = GetLastError() == ERROR_ALREADY_EXISTS;
        let h = OwnedHandle::from_raw_handle(h);
        (!taken).then_some(h)
    }
}

/// The kit folder's admission: a start (from its checks to its first game programs), setup, undo and the text switch
/// hold it in turn, across Launcher windows, so no setup or undo comes between a start's guard check and its game, and
/// a run and a server never both pass their check.  Released on drop, by the thread that took it.
pub struct Admission(OwnedHandle);

impl Drop for Admission {
    fn drop(&mut self) {
        // SAFETY: this thread owns the mutex (`admit`); the handle is still open.
        unsafe { ReleaseMutex(self.0.as_raw_handle()) };
    }
}

/// The admission, unless another thread holds it (this one may: a start's own setup); never waits.
pub fn admit(kit: &Kit) -> Option<Admission> {
    let name = lock_name(kit, "admit");
    // SAFETY: a NUL-terminated name; the handle is checked, then owned (closed on drop).
    unsafe {
        let h = CreateMutexW(ptr::null(), FALSE, name.as_ptr());
        if h.is_null() {
            return None;
        }
        let h = OwnedHandle::from_raw_handle(h);
        let got = WaitForSingleObject(h.as_raw_handle(), 0);
        // abandoned: a Launcher that held it ended - it is this thread's now
        (got == WAIT_OBJECT_0 || got == WAIT_ABANDONED).then_some(Admission(h))
    }
}

/// The Launcher window's mutex for this kit folder (2010-11 and 2017-18 installs never share it), owned by this
/// thread for as long as the handle lives; `None` while another Launcher window holds it past `wait_ms`.
pub fn hold_window(kit: &Kit, wait_ms: u32) -> Option<OwnedHandle> {
    let name = lock_name(kit, "window");
    // SAFETY: a NUL-terminated name; the handle is checked, then owned (closed on drop).
    unsafe {
        let h = CreateMutexW(ptr::null(), FALSE, name.as_ptr());
        if h.is_null() {
            return None;
        }
        let h = OwnedHandle::from_raw_handle(h);
        let got = WaitForSingleObject(h.as_raw_handle(), wait_ms);
        // abandoned: the window that held it ended
        (got == WAIT_OBJECT_0 || got == WAIT_ABANDONED).then_some(h)
    }
}

/// A run of this kind is on (in this process or another).
pub fn is_on(kit: &Kit, kind: Kind) -> bool {
    let name = lock_name(kit, kind.name());
    // SAFETY: a NUL-terminated name; a handle that opens is closed at once.
    unsafe {
        let h = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, FALSE, name.as_ptr());
        if h.is_null() {
            return false;
        }
        drop(OwnedHandle::from_raw_handle(h));
        true
    }
}

/// Any run is on in this kit folder (Launcher offers STOP THE GAME; UPDATE waits).
pub fn any_on(kit: &Kit) -> bool {
    is_on(kit, Kind::Run) || is_on(kit, Kind::Server)
}

/// What the run in this process got for the Universal Cup (2017-18, this PC: `Run::start_gamesvr` writes the control's
/// schedule from it): the window's PLAY tab shows that while the run is on, never a new preview.
static CUP_STARTED: Mutex<Option<Result<CupPlan, CupRefused>>> = Mutex::new(None);

/// [`CUP_STARTED`]: `None` while no run on this PC is on, else what that run got - a refusal included, so the PLAY tab
/// keeps showing the refusal the run was started with.
pub fn cup_started() -> Option<Result<CupPlan, CupRefused>> {
    *CUP_STARTED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// [`CUP_STARTED`]: what the run being started got; `None` once it ends.
pub(super) fn remember_cup(got: Option<Result<CupPlan, CupRefused>>) {
    *CUP_STARTED.lock().unwrap_or_else(PoisonError::into_inner) = got;
}

pub(super) fn request_file(kit: &Kit, kind: Kind) -> PathBuf {
    kit.data().join(format!("{}.request", kind.name()))
}

pub(super) fn answer_file(kit: &Kit, kind: Kind) -> PathBuf {
    kit.data().join(format!("{}.answer", kind.name()))
}

/// Asks the running game of `kind` (`stop`, `force`, `check`, `status`, `show`) and waits for its answer: its exit code
/// and what it said.  None if no such run is on.
pub fn request(kit: &Kit, kind: Kind, word: &str) -> Option<(i32, String)> {
    if !is_on(kit, kind) {
        return None;
    }
    let answer = answer_file(kit, kind);
    let _ = fs::remove_file(&answer);
    if let Err(e) = choices::write(&request_file(kit, kind), word) {
        return Some((FAILED, format!("could not ask the running game: {e}")));
    }
    let end = Instant::now() + ANSWER_WAIT;
    while Instant::now() < end {
        if let Ok(text) = fs::read_to_string(&answer) {
            let _ = fs::remove_file(&answer);
            let (code, said) = text.split_once('\n').unwrap_or((&text, ""));
            return Some((code.trim().parse().unwrap_or(FAILED), said.trim_end().to_owned()));
        }
        if !is_on(kit, kind) {
            return Some((OK, "The game has ended.".into()));
        }
        thread::sleep(Duration::from_millis(100));
    }
    Some((FAILED, "The running game did not answer.".into()))
}

/// Ctrl+C, a closed console, a closed Launcher window: the running game is asked to stop by the card rule, and does
/// not wait for the answer (the run says it).
pub fn ask_stop(kit: &Kit) {
    for kind in [Kind::Run, Kind::Server] {
        if is_on(kit, kind) {
            let _ = choices::write(&request_file(kit, kind), "stop");
        }
    }
}

/// A file a request was written to, taken (read and removed) - None if there is none, or it could not be removed.
pub(super) fn take(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    fs::remove_file(path).ok()?;
    Some(text)
}
