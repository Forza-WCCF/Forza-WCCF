//! The run: WCCF 2010-11 or 2017-18 started, kept and stopped on this PC (port of Kit 6's play.py, which this
//! replaces, of the 2017-18 kit's older fork of it, and of the watcher that ended a run when a game window closed).
//!
//! The edition comes from the kit (`Kit::edition`).  2017-18 differs in: its stand-ins (no scene service; the game
//! server stand-in beside control; the Aime reader on COM2 and AiMeDB instead of the club card reader; the keychip
//! says SDDY), its shared memory (`debug::maps`), its guard (`crate::guard` before anything starts; wccfnet.dll
//! also loads the panel into seat 1 and the projector, from `FORZA_PANEL`), the server's projector rule (the first PC gets it), and what it
//! leaves out (the club card's steps and view, the message guard).
//!
//! Order, each step waiting for the one before: scene service -> server and its 4 match engines -> projector -> the
//! cabinet's stand-ins (keychip and network, card reader, I/O board) -> seat 1 -> its overlay -> the key driver -> the
//! helper threads.  The stand-ins are threads of this process (`forza::standins`); every program starts through one
//! `Job` (`KILL_ON_JOB_CLOSE`: nothing outlives this process), each game program under a debugger thread of its own
//! (`debug::run`) that creates it and pumps its events.
//!
//! The process that starts a run supervises it until it ends (`Run::supervise`): events arrive on one channel - a game
//! window closed or crashed (its debugger thread ended), RESTART NOW (data\restart.request, written by the panel), and
//! the requests of other Launcher.exe processes (data\run.request or data\server.request: stop, status, show; the
//! answer goes to the .answer file beside it).  A named mutex per kit folder says a run is on.  Nothing is stopped by
//! name or by a remembered process id: a stop ends the debugged programs through their debuggers, then
//! `Job::stop(10 s)` closes and ends whatever is left in the job.
//!
//! The card rule: nothing that would end seat 1 goes on while the club card is in the reader (the I/O board's state in
//! memory) or the game has a card session open - that would end a match without its locker-room save and give the card
//! a bad ending - unless forced or the person at the stop says so.
//!
//! The helper threads (`crate::helper`: the club card view, the starting volumes) and the stand-ins are watched by the
//! supervisor: one that ends by itself (a panic, an error) is said in the run log, and the run stops if the game cannot
//! go on without it (the card reader, the I/O board, the scene service, the network stand-ins).
//!
//! In this process too (no Python since 2026-10-20): the club card steps (`forza::cards::steps`: the CLUB CARD panel's
//! requests, the table and the club card view), the game's text and setup (`crate::setup`).  The server side is
//! `programs\forza-server.exe all` (2017-18: `Launcher.exe server all`), a child in the job.

pub mod rules;
mod sampler;

use std::fs::{self, File};
use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};
use std::{env, ptr};

use crate::helper::{self, Helper, Target, ViewFiles};
use forza::online::save_logs::{self, Scrub};
use forza::online::seat::{self, Lease};
use forza::online::{championship, club, gamesvr};
use forza::standins::{Handle, Log, aime, aimedb, card_reader, io_board, network, scenes};
use forza_ui::lobby::{CupPlan, CupRefused};
use regex::Regex;
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, FALSE, GetLastError, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenMutexW, ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};

use crate::card;
use crate::debug::{self, Maps, Options};
use crate::job::Job;
use crate::kit::{Edition, find_game_named, seat_dir};
use crate::report::championship_line;
use crate::{Kit, choices, conf, guard, logs, procs};
pub use rules::{Mode, Words};

/// Exit codes, as play.py's: 0 ok, 1 a step failed, 2 not set up (or wrong words), 3 refused; 4 refused because
/// Windows would load the game without its guard.
pub const OK: i32 = 0;
pub const FAILED: i32 = 1;
pub const NOT_SET_UP: i32 = 2;
pub const REFUSED: i32 = 3;
pub const UNGUARDED: i32 = 4;
/// Refused because the network guard Setup makes is not in place (2017-18; 2010-11 says NOT_SET_UP).
pub const NOT_GUARDED: i32 = 5;
/// The run ended because a game window went by itself or crashed (`rules::went_wrong`): the window asks for the logs.
pub const WENT_WRONG: i32 = 6;

/// A month and a day: a backstop only.  A player's run ends when its game window is closed; a server for others
/// stays up all month (a keeper task on that machine restarts it whenever it stops).
const LIFE: Duration = Duration::from_secs(32 * 24 * 3600);
/// How long a stop waits for the run's processes after closing their windows, before it ends them.
const STOP_DEADLINE: Duration = Duration::from_secs(10);
/// How long a request waits for the running game's answer (a stop takes up to its deadline and more).
const ANSWER_WAIT: Duration = Duration::from_secs(60);

/// The panel's line in `seat1\forza_panel.log` once its Present detour has drawn a frame: the run's proof it is up.
pub(crate) const PANEL_UP: &str = "first frame shown";

/// 2017-18 arcade guest: what the end needs to tell that the host closed the arcade, and whether the club came home.
#[derive(Default)]
struct Visit {
    /// Through the tunnel: its `ArcadeLink::host_closed`.
    host_closed: Option<Arc<AtomicBool>>,
    /// The club trip's progress.
    club: Option<Arc<Mutex<club::Progress>>>,
}

/// A line the player reads, by its `locales/ui.yml` key (forza-ui) and arguments, which may be lines too: the window
/// shows it in the player's language, the log files keep the English (support reads them).  `Raw`: words that are
/// the same in every language (an error from Windows, another program's line).
#[derive(Clone, Debug, PartialEq)]
pub enum Text {
    Raw(String),
    Key(&'static str, Vec<(&'static str, Text)>),
}

impl Text {
    pub fn key(key: &'static str) -> Self {
        Self::Key(key, Vec::new())
    }

    /// `%{name}` filled with `value` as it is.
    #[must_use]
    pub fn arg(self, name: &'static str, value: impl std::fmt::Display) -> Self {
        self.part(name, Self::Raw(value.to_string()))
    }

    /// `%{name}` filled with another line, in the same language.
    #[must_use]
    pub fn part(self, name: &'static str, value: Text) -> Self {
        match self {
            Self::Key(k, mut args) => {
                args.push((name, value));
                Self::Key(k, args)
            }
            raw => raw,
        }
    }

    pub fn get(&self, lang: forza_ui::Lang) -> String {
        match self {
            Self::Raw(s) => s.clone(),
            Self::Key(key, args) => {
                let args: Vec<(&str, String)> = args.iter().map(|(n, t)| (*n, t.get(lang))).collect();
                let args: Vec<(&str, &str)> = args.iter().map(|(n, v)| (*n, v.as_str())).collect();
                forza_ui::text(lang, key, &args)
            }
        }
    }

    pub fn en(&self) -> String {
        self.get(forza_ui::Lang::En)
    }
}

impl From<String> for Text {
    fn from(s: String) -> Self {
        Self::Raw(s)
    }
}

/// The game server's port could not be bound: taken, this PC is already hosting an arcade on it (its own earlier start, or a
/// server beside it), which "os error 10048" did not say; any other error keeps its port and reason.
fn gamesvr_bind_error(port: u16, e: &io::Error) -> Text {
    if e.kind() == io::ErrorKind::AddrInUse {
        Text::key("launcher.run_gamesvr_busy").arg("port", port)
    } else {
        Text::key("launcher.run_gamesvr_port").arg("port", port).arg("error", e)
    }
}

/// `text` to `ui` after `lead` (an indent, a step "1/6" the window's bar reads): shown in the player's language, the
/// English for the logs.
pub fn tell(ui: &dyn Ui, lead: &str, text: &Text) {
    ui.say_logged(
        &format!("{lead}{}", text.get(ui.lang())),
        &format!("{lead}{}", text.en()),
    );
}

/// A line of forza's setup, game text or card pictures ([`forza::Line`]) to `ui` after `lead`, as [`tell`] does.
pub fn tell_line(ui: &dyn Ui, lead: &str, line: &forza::Line) {
    ui.say_logged(
        &format!("{lead}{}", forza_ui::line(ui.lang(), line)),
        &format!("{lead}{}", forza_ui::line(forza_ui::Lang::En, line)),
    );
}

/// A `Ui` over a closure that gets each line as shown and in English (setup's lines into a log file and the run's box).
pub struct Lines<'a>(pub forza_ui::Lang, pub &'a dyn Fn(&str, &str));

impl Ui for Lines<'_> {
    fn say(&self, line: &str) {
        (self.1)(line, line);
    }

    fn say_logged(&self, shown: &str, logged: &str) {
        (self.1)(shown, logged);
    }

    fn lang(&self) -> forza_ui::Lang {
        self.0
    }
}

/// Where a run tells what it does: the window's box or the console.  `ask` gets a yes/no question (no one to ask:
/// no); `running` hears when the game is up (true) and when the run is over (false).
pub trait Ui {
    fn say(&self, line: &str);
    /// A line in the player's language (`shown`) whose log files get the English (`logged`).
    fn say_logged(&self, shown: &str, _logged: &str) {
        self.say(shown);
    }
    fn ask(&self, _question: &str) -> bool {
        false
    }
    fn running(&self, _up: bool) {}
    /// The start is about to make the card pictures (a while): the window's status line.
    fn updating_pictures(&self) {}
    /// The run's own steps begin here: step 1 is timed from now on (the setup again and the card catalogue before it
    /// are not a step the player waits through).
    fn staged(&self) {}
    /// REPORT A PROBLEM asked by the panel (data\report.request): the window opens its report dialog.
    fn report(&self) {}
    /// The language of the run's lines ([`Text`]).
    fn lang(&self) -> forza_ui::Lang {
        forza_ui::Lang::En
    }
}

/// The game's names and numbers; tests swap them for stand-ins, free ports and names of their own.
pub struct Params {
    /// The client, control and match programs' file names, as find_game looks for them.
    pub programs: [String; 3],
    /// Seat 1's command line (None: its seat number) and the projector's (None: "0").
    pub seat_args: Option<Vec<String>>,
    pub projector_args: Option<Vec<String>>,
    pub control_args: Vec<String>,
    /// How long seat 1's window may take to come up (the game's first start can be slow).
    pub window_wait: Duration,
    /// The server is up once this many match engines run.
    pub match_engines: usize,
    pub control_port: u16,
    pub relay_port: u16,
    pub seat_port: u16,
    /// Where SAVE LOGS writes its ZIP (the Downloads folder).
    pub logs_dir: PathBuf,
    /// The network stand-ins' ports, in `network::SERVICES` order (0: any free one).
    pub network_ports: [u16; 6],
    pub icc_pipe: String,
    pub jvs_pipe: String,
    /// 2017-18: the Aime reader's pipe (COM2), AiMeDB's port and the game server stand-in's port.
    pub aime_pipe: String,
    pub aimedb_port: u16,
    pub gamesvr_port: u16,
    pub scenes: scenes::Names,
    /// The shared memory's name prefix (`debug::maps::GAME_ID`); None: none made.
    pub maps: Option<String>,
    /// The cabinet's inputs, shared by the key driver and the I/O board stand-in.
    pub shared: Arc<io_board::Shared>,
    /// Extra stand-in threads for the supervisor to watch (name, essential, handle); a cabinet takes them when it
    /// starts.  Tests put a thread that panics here; the game has none.
    pub extra_watch: Mutex<Vec<(String, bool, Handle)>>,
    /// 2010-11: every program must load the kit's hook through its renamed import (`guard::hook_1011`).  Off only in
    /// tests whose stand-ins are no game programs.
    pub hook_imports: bool,
    /// 2017-18: setup run again by a start (`setup::setup_1718`) with the kit's and the game's folders.  Tests put a
    /// stand-in here: their runtimes hold no original game programs.
    pub setup_1718: fn(&Path, &Path) -> Result<(), String>,
    /// 2017-18: where a game folder whose path is not plain ASCII gets its ASCII junction (`alias`).
    pub run_links: PathBuf,
}

impl Params {
    pub fn game() -> Self {
        Self {
            programs: crate::kit::GAME_PROGRAMS.map(String::from),
            seat_args: None,
            projector_args: None,
            control_args: Vec::new(),
            window_wait: Duration::from_secs(120),
            match_engines: 4,
            control_port: 20002,
            relay_port: 20040,
            seat_port: forza::online::SEAT_PORT,
            logs_dir: save_logs::downloads(),
            network_ports: network::SERVICES.map(|s| s.0),
            icc_pipe: r"\\.\pipe\wccf_icc_seat1".into(),
            jvs_pipe: r"\\.\pipe\wccf_jvs_seat1".into(),
            aime_pipe: r"\\.\pipe\wccf_aime_seat1".into(),
            aimedb_port: 22345,
            gamesvr_port: 20050, // ctrl_user_option.conf GAME_SVR_PORT
            scenes: scenes::Names::game(),
            maps: Some(debug::maps::GAME_ID.into()),
            hook_imports: true,
            setup_1718: crate::setup::setup_1718,
            run_links: crate::alias::base(),
            shared: io_board::Shared::new(),
            extra_watch: Mutex::default(),
        }
    }

    /// The game's names for the kit's edition (2017-18: its shared memory's prefix).
    pub fn of(kit: &Kit) -> Self {
        let mut p = Self::game();
        p.maps = Some(kit.edition().game_id().into());
        p
    }

    fn client(&self) -> &str {
        &self.programs[0]
    }

    fn control(&self) -> &str {
        &self.programs[1]
    }

    fn matcher(&self) -> &str {
        &self.programs[2]
    }
}

// ---- one run per kit folder ---------------------------------------------------------------------------------------

/// A cabinet run (local, remote) or a server run: one of each can run, a remote cabinet beside a server.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Run,
    Server,
}

impl Kind {
    fn of(mode: &Mode) -> Self {
        if mode.is_server() { Self::Server } else { Self::Run }
    }

    fn name(self) -> &'static str {
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
fn take_lock(kit: &Kit, kind: Kind) -> Option<OwnedHandle> {
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
fn remember_cup(got: Option<Result<CupPlan, CupRefused>>) {
    *CUP_STARTED.lock().unwrap_or_else(PoisonError::into_inner) = got;
}

fn request_file(kit: &Kit, kind: Kind) -> PathBuf {
    kit.data().join(format!("{}.request", kind.name()))
}

fn answer_file(kit: &Kit, kind: Kind) -> PathBuf {
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
fn take(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    fs::remove_file(path).ok()?;
    Some(text)
}

// ---- the command line ---------------------------------------------------------------------------------------------

/// play.py's words (`rules::parse`): a start supervises its run until it ends; stop, status and show ask the running
/// game.  The exit code.
pub fn command(kit: &Kit, p: &Params, words: &[String], ui: &dyn Ui) -> i32 {
    match rules::parse(words) {
        Words::Wrong(why) => {
            ui.say(&why);
            NOT_SET_UP
        }
        Words::Stop { force, check } => stop_command(kit, p, force, check, ui),
        Words::Status => ask_each(kit, p, "status", ui).unwrap_or_else(|| {
            tell(ui, "", &Text::key("launcher.run_nothing_running"));
            outside(kit, p, ui)
        }),
        Words::Show => ask_each(kit, p, "show", ui).unwrap_or_else(|| {
            tell(ui, "", &Text::key("launcher.run_no_server"));
            FAILED
        }),
        Words::Restart if is_on(kit, Kind::Run) => {
            match choices::write(
                &kit.data().join("restart.request"),
                "RESTART NOW - asked from the command line\n",
            ) {
                Ok(()) => {
                    tell(ui, "", &Text::key("launcher.run_restarting_running"));
                    OK
                }
                Err(e) => {
                    tell(ui, "", &Text::key("launcher.run_no_restart").arg("error", e));
                    FAILED
                }
            }
        }
        Words::Restart => play(kit, p, None, false, ui),
        Words::Pictures => {
            tell(ui, "", &Text::key("launcher.msg_making_pictures"));
            if make_pictures(kit, p, ui) {
                tell(ui, "", &Text::key("launcher.run_pictures_ready"));
                OK
            } else {
                FAILED
            }
        }
        Words::Plain { debug } => play(kit, p, None, debug, ui),
        Words::Start { mode, debug } => {
            // an arcade's tunnel id is not a server address: the lobby would ask it
            if let Mode::Remote(ip, _) = &mode
                && vslink::arcade::parse_id(ip).is_none()
                && choices::setting(&kit.settings_file(), "server").as_deref() != Some(ip)
            {
                let _ = choices::set_setting(&kit.settings_file(), "server", ip); // "remote" alone uses it again
            }
            play(kit, p, Some(mode), debug, ui)
        }
        Words::RemoteRemembered { seat, debug } => match choices::setting(&kit.settings_file(), "server") {
            Some(ip) if choices::host_port(&ip, p.seat_port).is_some() => {
                play(kit, p, Some(Mode::Remote(ip, seat)), debug, ui)
            }
            _ => {
                tell(ui, "", &Text::key("launcher.run_which_server"));
                NOT_SET_UP
            }
        },
    }
}

/// Asks every run that is on; None if none is.
fn ask_each(kit: &Kit, _p: &Params, word: &str, ui: &dyn Ui) -> Option<i32> {
    let mut rc = None;
    for kind in [Kind::Run, Kind::Server] {
        if let Some((code, said)) = request(kit, kind, word) {
            for line in said.lines() {
                ui.say(line);
            }
            rc = Some(rc.unwrap_or(OK).max(code));
        }
    }
    rc
}

/// The game's programs running from this game's folders that no run of this kit holds (left by an older kit): said,
/// never stopped by name.
fn outside(kit: &Kit, p: &Params, ui: &dyn Ui) -> i32 {
    let Some(game) = game_folder(kit, p) else { return OK };
    let left = strays(&game, p);
    if left.is_empty() {
        return OK;
    }
    let list = left
        .iter()
        .map(|q| format!("{} {}", q.name, q.pid))
        .collect::<Vec<_>>()
        .join(", ");
    tell(ui, "", &Text::key("launcher.run_strays").arg("list", list));
    FAILED
}

fn stop_command(kit: &Kit, p: &Params, force: bool, check: bool, ui: &dyn Ui) -> i32 {
    let word = if check {
        "check"
    } else if force {
        "force"
    } else {
        "stop"
    };
    let mut rc = None;
    let mut run_answer = None;
    for kind in [Kind::Run, Kind::Server] {
        if kind == Kind::Server && is_on(kit, Kind::Server) {
            // a seat that stopped lets go of its lock a moment after its answer
            let end = Instant::now() + Duration::from_secs(5);
            while run_answer == Some(OK) && is_on(kit, Kind::Run) && Instant::now() < end {
                thread::sleep(Duration::from_millis(100));
            }
            if !rules::stop_server_too(run_answer, is_on(kit, Kind::Run)) {
                tell(ui, "", &Text::key("launcher.run_server_keeps"));
                rc = Some(rc.unwrap_or(REFUSED)); // something runs: not "Nothing running."
                continue;
            }
        }
        let Some((mut code, said)) = request(kit, kind, word) else {
            continue;
        };
        for line in said.lines() {
            ui.say(line);
        }
        // never a dead end: the person at the stop decides, knowing the cost
        if code == REFUSED
            && !check
            && ui.ask("Stop anyway? Type Y and press Enter (Enter alone keeps the game running): ")
        {
            tell(ui, "", &Text::key("launcher.run_stopping"));
            let (c, said) = request(kit, kind, "force").unwrap_or((OK, String::new()));
            for line in said.lines() {
                ui.say(line);
            }
            code = c;
        } else if code == REFUSED && !check {
            tell(ui, "", &Text::key("launcher.run_force_hint"));
        }
        if kind == Kind::Run {
            run_answer = Some(code);
        }
        rc = Some(rc.unwrap_or(OK).max(code));
    }
    rc.unwrap_or_else(|| {
        tell(ui, "", &Text::key("launcher.run_nothing_running"));
        outside(kit, p, ui)
    })
}

/// Starts and supervises; a full RESTART NOW starts again with what a plain start chooses.
pub fn play(kit: &Kit, p: &Params, mode: Option<Mode>, debug: bool, ui: &dyn Ui) -> i32 {
    // every way out: the run it remembers has ended (CUP_STARTED cleared), so the PLAY tab previews the
    // choice again
    let _forget = core::mem::DropGuard::new((), |()| remember_cup(None));
    let mut mode = mode;
    loop {
        let m = mode.take().unwrap_or_else(|| plain_mode(kit, p, ui));
        match start(kit, p, m, debug, ui) {
            Err(code) => return code,
            Ok(run) => match run.supervise() {
                End::Over(code) => return code,
                End::Again => {}
            },
        }
    }
}

/// What a plain start does: what the SETTINGS panel chose - never locked out: an address that is wrong, or a server
/// that does not answer, plays on this PC and says why.
fn plain_mode(kit: &Kit, p: &Params, ui: &dyn Ui) -> Mode {
    // an unreadable file is said (and logged) like the other reasons a plain start plays locally: the player would
    // otherwise take it for their own choice and look for the lobby
    let panel = match choices::read_ok(&kit.data().join("panel.txt")) {
        Ok(panel) => panel,
        Err(e) => {
            tell(ui, "", &Text::key("launcher.run_panel_unread").arg("error", e));
            String::new()
        }
    };
    if !choices::panel_value(&panel, "play_on").is_some_and(|v| v.eq_ignore_ascii_case("online")) {
        return Mode::Local;
    }
    let ip = choices::panel_value(&panel, "server").unwrap_or_default();
    if kit.edition() == Edition::E1718 {
        // VS between homes: this PC is its own arcade, the clubs and the pairing on the server's game server
        let club = choices::setting(&kit.settings_file(), "club_server").unwrap_or_default();
        return match rules::vs_target(&panel, &club) {
            Err(why) => {
                tell(ui, "", &Text::key("launcher.run_local_because").part("why", why));
                Mode::Local
            }
            Ok(t) if reach(&t.host, t.port).is_none() => {
                tell(
                    ui,
                    "",
                    &Text::key("launcher.run_vs_no_answer")
                        .arg("host", &t.host)
                        .arg("port", t.port),
                );
                Mode::Local
            }
            Ok(t) => {
                tell(ui, "", &Text::key("launcher.run_online_vs").arg("host", &t.host));
                Mode::Vs(ip)
            }
        };
    }
    let host = choices::host_port(&ip, p.seat_port).map(|(h, _)| h);
    let Some(host) = host else {
        tell(ui, "", &Text::key("launcher.run_bad_server").arg("ip", &ip));
        return Mode::Local;
    };
    if reach(&host, p.control_port).is_none() {
        tell(ui, "", &Text::key("launcher.run_server_no_answer").arg("ip", &ip));
        return Mode::Local;
    }
    tell(ui, "", &Text::key("launcher.run_online"));
    Mode::Remote(ip, None)
}

/// Something takes a TCP connection on ip:port within 5 s (closed again at once).
fn answers(ip: &str, port: u16) -> bool {
    format!("{ip}:{port}")
        .parse::<SocketAddr>()
        .is_ok_and(|a| TcpStream::connect_timeout(&a, Duration::from_secs(5)).is_ok())
}

/// The first of `host`'s addresses that takes a TCP connection on `port` within 5 s (a name or an IPv4 address).
fn reach(host: &str, port: u16) -> Option<SocketAddr> {
    (host, port)
        .to_socket_addrs()
        .ok()?
        .find(|a| TcpStream::connect_timeout(a, Duration::from_secs(5)).is_ok())
}

fn addr(ip: &str, port: u16) -> Option<SocketAddr> {
    format!("{ip}:{port}").parse().ok()
}

fn now_local() -> time::OffsetDateTime {
    time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc())
}

/// The player's notice: their club plays the Universal Championship on the projector from about `opens` (this PC's
/// clock). The finalists' seats need nothing more: the control plays the team each winner registered for the cup and
/// sends the results itself (0x47BFF0, 0x462990).
fn championship_notice(opens: SystemTime) -> Text {
    let t = time::OffsetDateTime::from(opens).to_offset(now_local().offset());
    Text::key("launcher.run_championship").arg("time", format!("{:02}:{:02}", t.hour(), t.minute()))
}

// ---- the start ----------------------------------------------------------------------------------------------------

/// The game folder setup recorded, found by the run's program names; 2017-18 without a record: `game\` beside the kit
/// (the 2017-18 setup's runtime: `kit\`, `game\`, `seat1\`).
fn game_folder(kit: &Kit, p: &Params) -> Option<PathBuf> {
    let set = kit.settings().get("game").and_then(|g| g.as_str().map(str::to_owned));
    let set = set.or_else(|| match kit.edition() {
        Edition::E1718 => Some(kit.dir().parent()?.join("game").display().to_string()),
        Edition::E1011 => None,
    })?;
    find_game_named(&set, p.programs.clone())
}

/// Whether a start has to make the card pictures first (an install made without them, or by an older version: an
/// update whose setup did not run): a game is set up and the folder is not as `ensure` wants it, unless it already
/// failed for this version (SET UP and `Launcher.exe pictures` try again).
fn pictures_due(kit: &Kit, p: &Params) -> bool {
    due(
        &kit.cache().join("cards"),
        game_folder(kit, p).is_some(),
        pack_of(kit).as_deref(),
    )
}

fn due(cards: &Path, game: bool, pack: Option<&Path>) -> bool {
    game && !forza::cards::pictures::is_wanted(cards, pack)
}

/// The high-quality pack the settings choose, when it is there.
fn pack_of(kit: &Kit) -> Option<PathBuf> {
    let s = kit.settings();
    (s["card_pictures"] == "hq")
        .then(|| s["card_pack"].as_str().map(PathBuf::from).filter(|p| p.is_file()))
        .flatten()
}

/// The board's card pictures, made at setup (and again when Launcher's choice changes) by
/// `forza::cards::pictures::ensure` into the folder the panel reads: the player's game's own, and with
/// data\\settings.json "card_pictures": "hq" the high-quality pack at "card_pack" over them. The run only reads them.
/// `true` when they are in place.
pub fn make_pictures(kit: &Kit, p: &Params, ui: &dyn Ui) -> bool {
    // like SET UP (setup.rs `admitted`): never under a run, or beside a start or another setup, in any Launcher window
    let Some(_admitted) = admit(kit).filter(|_| !any_on(kit)) else {
        tell(ui, "  FAIL  ", &Text::key("launcher.run_pictures_busy"));
        return false;
    };
    pictures_held(kit, p, ui)
}

/// [`make_pictures`] inside a start, which holds the admission and is on.
fn pictures_held(kit: &Kit, p: &Params, ui: &dyn Ui) -> bool {
    let Some(game) = game_folder(kit, p) else {
        tell(ui, "  ", &Text::key("launcher.run_pictures_no_game"));
        return false;
    };
    let ed = match kit.edition() {
        Edition::E1011 => "1011",
        Edition::E1718 => "1718",
    };
    let pack = pack_of(kit);
    if pack.is_none() && kit.settings()["card_pictures"] == "hq" {
        tell(ui, "  ", &Text::key("launcher.run_no_hq_pack"));
    }
    let say = |lead: &str, l: &forza::Line| tell_line(ui, lead, l);
    match forza::cards::pictures::ensure(&game, ed, &kit.cache().join("cards"), pack.as_deref(), &say) {
        Ok(()) => true,
        Err(e) => {
            tell(ui, "  ", &Text::key("launcher.run_pictures_failed").arg("error", e));
            false
        }
    }
}

/// The game's programs (by the run's names) running from the game folder and the folders beside it.
fn strays(game: &Path, p: &Params) -> Vec<procs::Proc> {
    let root = procs::same_form(game.parent().unwrap_or(game)) + "\\";
    let names: Vec<String> = p.programs.iter().map(|n| n.to_lowercase()).collect();
    procs::processes()
        .into_iter()
        .filter(|q| {
            let n = q.name.to_lowercase();
            (names.contains(&n) || procs::GAME_EXES.contains(&n.as_str()))
                && procs::normcase(&q.path).starts_with(&root)
        })
        .collect()
}

/// A step that failed: the start stops and what was started is stopped.
struct Failed(Text);

impl Failed {
    /// Nothing more to say (said already: the game is not set up).
    fn quiet() -> Self {
        Self(Text::Raw(String::new()))
    }

    fn is_quiet(&self) -> bool {
        self.0 == Text::Raw(String::new())
    }
}

impl From<io::Error> for Failed {
    fn from(e: io::Error) -> Self {
        Self(Text::Raw(e.to_string()))
    }
}

impl From<Text> for Failed {
    fn from(t: Text) -> Self {
        Self(t)
    }
}

fn wait_for(
    what: Text,
    secs: u64,
    log: &str,
    alive: impl Fn() -> bool,
    mut test: impl FnMut() -> bool,
) -> Result<(), Failed> {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if test() {
            return Ok(());
        }
        if !alive() {
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(Failed(if log.is_empty() {
        Text::key("launcher.run_not_up").part("what", what)
    } else {
        Text::key("launcher.run_not_up_log").part("what", what).arg("log", log)
    }))
}

/// `cmd` in `logs`, without an earlier run's MXHOOK_ / WCCF settings and with `env`.
fn clean_env(cmd: &mut Command, logs: &Path, env: &[(String, String)]) {
    cmd.current_dir(logs).stdin(Stdio::null());
    for (k, _) in env::vars_os() {
        let up = k.to_string_lossy().to_uppercase();
        if up.starts_with("MXHOOK_") || up.starts_with("WCCF") {
            cmd.env_remove(k);
        }
    }
    cmd.envs(env.iter().map(|(k, v)| (k, v)));
}

/// `cmd` in the job, its output to `log`; its exit code and output once it ends within `limit`.
fn run_in_job(job: &Job, mut cmd: Command, log: &Path, limit: Duration) -> Result<(i32, String), String> {
    let out = File::create(log).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    cmd.stdout(out).stderr(err);
    let mut child = job.spawn(&mut cmd).map_err(|e| e.to_string())?;
    let end = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            let text = fs::read(log)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            return Ok((status.code().unwrap_or(1), text));
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("it did not finish in {} s", limit.as_secs()));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// The game programs' compatibility layer (`rules::compat_layer`): crisp on a screen scaled above 100%.  The control's
/// match engines, its children, inherit it.
fn game_dpi() -> (String, String) {
    let key = "__COMPAT_LAYER";
    (key.into(), rules::compat_layer(env::var(key).ok().as_deref()))
}

/// The program's id from its debugger's "launched NAME pid N" line in `data\logs\<log>`, once it is there.
fn launched_pid(logs: &Path, log: &str) -> Option<u32> {
    let text = fs::read_to_string(logs.join(log)).ok()?;
    let re = Regex::new(r"launched \S+ pid (\d+)").expect("a fixed pattern");
    re.captures(&text)?[1].parse().ok()
}

/// A log of the run's stand-ins: data\logs\<name>, a line each with the wall clock and the seconds since it started.
/// The name a LAN arcade answers searches with: its cleaned arcade name, else this PC's.
fn lan_name(arcade: String) -> String {
    if arcade.is_empty() {
        env::var("COMPUTERNAME").unwrap_or_else(|_| "Forza-WCCF".into())
    } else {
        arcade
    }
}

fn file_log(path: PathBuf) -> Log {
    let t0 = Instant::now();
    let file = Mutex::new(File::create(path).ok());
    Arc::new(move |msg| {
        if let Some(f) = file.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut() {
            let now = logs::clock(logs::local(std::time::SystemTime::now()));
            let _ = writeln!(f, "{now} {:8.2}  {msg}", t0.elapsed().as_secs_f64());
        }
    })
}

/// A game program of the run, under its own debugger thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Server,
    Projector,
    Seat,
}

/// 2017-18: the panel's role, said outright (WCCFPANEL_ROLE).  A program started through a run junction
/// (`alias::for_dir`) is not in WCCF_GAME's folder, so the panel cannot tell the projector by its folder.
fn panel_role(edition: Edition, role: Role) -> Option<(String, String)> {
    let r = match (edition, role) {
        (Edition::E1718, Role::Projector) => "projector",
        (Edition::E1718, Role::Seat) => "seat",
        _ => return None,
    };
    Some(("WCCFPANEL_ROLE".into(), r.into()))
}

impl Role {
    fn log(self) -> &'static str {
        match self {
            Self::Server => "run_server.txt",
            Self::Projector => rules::PROJECTOR_LOG,
            Self::Seat => "run_seat1.txt", // every seat: the cabinet is the seat1 folder
        }
    }
}

struct Game {
    id: u32,
    role: Role,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Game {
    fn alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

enum Event {
    /// A game program's debugger thread ended (by its id): its window was closed, it crashed, or the run stopped it.
    Ended(u32),
    /// RESTART NOW from the panel; true when its crash screen asked it (`force=crashed`: the control is gone).
    Restart(bool),
    Request(String),
    SaveLogs,
    /// A line for the window from a helper thread (SAVE LOGS' result).
    Said(Text),
    Report,
}

/// What a supervised run ended with.
pub enum End {
    Over(i32),
    /// A full RESTART NOW: start again with what a plain start chooses.
    Again,
}

/// Seat 1's side: dropped in this order - the key driver before the board, so every button is let go.
#[derive(Default)]
struct Cabinet {
    keys: Option<Handle>,
    board: Option<Handle>,
    /// The club card reader (2010-11) or the Aime reader (2017-18).
    reader: Option<Handle>,
    aimedb: Option<Handle>,
    net: Vec<Handle>,
    extra: Vec<(String, bool, Handle)>,
}

impl Cabinet {
    fn on(&self) -> bool {
        self.keys.is_some()
            || self.board.is_some()
            || self.reader.is_some()
            || self.aimedb.is_some()
            || !self.net.is_empty()
            || !self.extra.is_empty()
    }
}

/// A run: what it started, held until it ends.
pub struct Run<'a> {
    kit: &'a Kit,
    p: &'a Params,
    ui: &'a dyn Ui,
    edition: Edition,
    game: PathBuf,
    seat: PathBuf,
    mode: Mode,
    seat_no: u8,
    debug: bool,
    beside_server: bool,
    /// Remote: the server is this PC's own (its address is one of this PC's), so its game server is this PC's store.
    own_store: bool,
    /// Remote through the arcade tunnel: the loopback ports it listens on (the arcade's own unless this PC holds them).
    tunnel_ports: Option<vslink::arcade::Ports>,
    job: Arc<Job>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    games: Vec<Game>,
    next_id: u32,
    /// Ended game programs: their logs (for the end's decisions).
    gone: Vec<Role>,
    kids: Vec<(&'static str, Child)>,
    helper: Option<Helper>,
    cabinet: Cabinet,
    scenes: Option<Handle>,
    /// 2017-18: the game server stand-in (control keeps the clubs there).
    gamesvr: Option<Handle>,
    /// 2017-18 with a club server: the direct VS link (crates/vslink), beside the game. An arcade's host or a seat that
    /// joins it by its id: the arcade tunnel (vslink::arcade).
    vslink: Option<Handle>,
    /// 2017-18 online: where this run's VS plays (RESTART NOW keeps the cabinet only for the same target).
    vs_target: Option<rules::VsTarget>,
    lease: Option<Lease>,
    /// 2017-18 arcade guest: its home club, out in the arcade until this is dropped (online::club).
    club: Option<club::Trip>,
    visit: Visit,
    /// Remote: the server as the game takes it (its IPv4 address) and its seat desk's port (`host:port` written:
    /// that port), found at step 2 from [`Mode::Remote`]'s entry.
    remote: Option<(String, u16)>,
    /// 2017-18 server: its arcade room in the lobby (the code guests type), or why it has none.
    arcade_room: Option<Result<crate::lobby::HostRoom, String>>,
    /// 2017-18 host: its arcade id and room code, for the run's `session:` line.
    arcade: Option<String>,
    /// The CPU and RAM sampler (run_sampler.txt), from the run's start to its end.
    sampler: Option<Handle>,
    /// 2017-18 server: the guests its arcade tunnel has connected now.
    arcade_guests: Option<Arc<AtomicUsize>>,
    /// When the start began, and the setup and guard checks' end (logged once the log folder is rotated).
    t0: Instant,
    early: [Duration; 2],
    /// 2017-18: the Universal Championship's day file as last read (its time), and the date already looked at.
    uc_seen: Option<SystemTime>,
    /// The day file's time when it was last read while it changed (logged once).
    uc_partial: Option<SystemTime>,
    uc_told: Option<u32>,
    /// The player's club last seen in that day's bracket (date, names, swap logged), for the swap line and the run's
    /// summary.
    uc_player: Option<(u32, String, bool)>,
    /// The projector program runs (2017-18: the CPU championship is on, and this PC holds the projector). Its window
    /// may be hidden: the championship plays unseen then.
    projector: bool,
    /// 2017-18: a game folder could not get its ASCII junction, and the player was told.
    told_path: bool,
    _lock: OwnedHandle,
}

/// Starts a run; `Err` is the exit code (what went wrong is said).
pub fn start<'a>(kit: &'a Kit, p: &'a Params, mode: Mode, debug: bool, ui: &'a dyn Ui) -> Result<Run<'a>, i32> {
    let t0 = Instant::now();
    if procs::prefers_system32() {
        tell(ui, "", &Text::key("launcher.run_unguarded"));
        return Err(UNGUARDED);
    }
    let Some(game) = game_folder(kit, p) else {
        tell(ui, "", &Text::key("launcher.run_not_set_up"));
        return Err(NOT_SET_UP);
    };
    let seat = seat_dir(&game);
    let edition = kit.edition();
    let arcade = rules::arcade_1718_on(&kit.settings());
    if rules::online_refused(edition, &mode, arcade) {
        tell(ui, "", &Text::key("launcher.run_online_1718"));
        return Err(REFUSED);
    }
    if edition == Edition::E1011 && !seat.join(".wccf-kit").is_file() {
        tell(ui, "", &Text::key("launcher.run_not_set_up"));
        return Err(NOT_SET_UP);
    }
    // an antivirus quarantining them leaves the game without its card table and panel; a server-only run needs neither
    let missing = kit.missing_programs();
    if !mode.is_server() && !missing.is_empty() {
        tell(
            ui,
            "",
            &Text::key("launcher.run_no_program").arg("files", missing.join(", ")),
        );
        return Err(REFUSED);
    }
    // held to the end of the start: see Admission
    let Some(_admitted) = admit(kit) else {
        tell(ui, "", &Text::key("launcher.run_busy"));
        return Err(REFUSED);
    };
    let check = || guard::check(&game, &seat, p.client(), p.control(), p.matcher());
    // a runtime an older setup made or the guard check refuses is set up again, once; the check after it decides
    let again = (edition == Edition::E1718)
        .then(|| crate::setup::why_again(kit, &game, check))
        .flatten();
    if let Some(why) = &again
        && !crate::setup::again(kit, &game, why, p.setup_1718, ui)
    {
        return Err(NOT_GUARDED);
    }
    let setup_done = t0.elapsed();
    // before anything of the run exists: no game program may start unguarded (docs/SAFETY.md); with no setup again the
    // check passed in why_again (2010-11 has its own: ensure_hook)
    if again.is_some()
        && let Err(why) = check()
    {
        tell(ui, "", &Text::key("launcher.run_not_guarded").arg("why", why));
        return Err(NOT_GUARDED);
    }
    let early = [setup_done, t0.elapsed()];
    // the panel's card browser only: never a reason not to start.  A play folder an older kit set up (copied without
    // overlay\catalogue.tsv) gets one made here, as SET UP makes it; the note says it is still missing.
    if !kit.cache().join("catalogue.tsv").is_file()
        && (edition != Edition::E1718 || crate::setup::catalogue_1718(kit.dir(), &game).is_err())
    {
        tell(ui, "  ", &Text::key("launcher.run_no_catalogue"));
    }
    // step 1's time starts here: the setup again and the catalogue build above are not steps of the run
    ui.staged();
    let kind = Kind::of(&mode);
    let beside_server = matches!(mode, Mode::Remote(..)) && is_on(kit, Kind::Server);
    let left = if beside_server { Vec::new() } else { strays(&game, p) };
    let busy = is_on(kit, Kind::Run) || (!beside_server && is_on(kit, Kind::Server)) || !left.is_empty();
    let lock = if busy { None } else { take_lock(kit, kind) };
    let Some(lock) = lock else {
        let mut names: Vec<&str> = left.iter().map(|q| q.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        let names = if names.is_empty() {
            Text::key("launcher.run_the_game")
        } else {
            Text::Raw(names.join(", "))
        };
        tell(ui, "", &Text::key("launcher.run_already").part("names", names));
        return Err(REFUSED);
    };
    let job = match Job::new() {
        Ok(j) => Arc::new(j),
        Err(e) => {
            let why = Text::key("launcher.run_no_job").arg("error", e);
            tell(ui, "", &Text::key("launcher.run_failed").part("why", why));
            return Err(FAILED);
        }
    };
    let (tx, rx) = channel();
    let mut run = Run {
        kit,
        p,
        ui,
        edition,
        game,
        seat,
        mode,
        seat_no: 1,
        debug,
        beside_server,
        own_store: false,
        tunnel_ports: None,
        arcade: None,
        sampler: None,
        job,
        tx,
        rx,
        games: Vec::new(),
        next_id: 0,
        gone: Vec::new(),
        kids: Vec::new(),
        helper: None,
        cabinet: Cabinet::default(),
        scenes: None,
        gamesvr: None,
        vslink: None,
        vs_target: None,
        lease: None,
        club: None,
        visit: Visit::default(),
        remote: None,
        arcade_room: None,
        arcade_guests: None,
        t0,
        early,
        uc_seen: None,
        uc_partial: None,
        uc_told: None,
        uc_player: None,
        projector: false,
        told_path: false,
        _lock: lock,
    };
    // requests left by a run that ended are never carried out
    for f in [
        request_file(kit, kind),
        answer_file(kit, kind),
        kit.data().join("savelogs.request"),
    ] {
        let _ = fs::remove_file(f);
    }
    if kind == Kind::Run {
        let _ = fs::remove_file(kit.data().join("restart.request"));
    }
    match run.start_all() {
        Ok(()) => Ok(run),
        Err(f) => {
            // an empty reason: the game is not set up (said already); the reason goes in run_launcher.txt too
            let quiet = f.is_quiet();
            if !quiet {
                run.failed(f);
            }
            run.shutdown();
            let _ = fs::remove_file(run.running_file());
            Err(if quiet { NOT_SET_UP } else { FAILED })
        }
    }
}

impl Run<'_> {
    fn say(&self, line: &str) {
        self.ui.say(line);
        // and in data\logs\run_launcher.txt: the window's box and a console are gone when they close, and a console
        // Launcher.exe's lines do not reach a cmd redirect (first live run, 2026-10-13: the start's lines were lost)
        crate::logs::append(&self.logs().join("run_launcher.txt"), line);
    }

    /// `text` after `lead` ([`tell`]), and its English in data\logs\run_launcher.txt.
    fn tell(&self, lead: &str, text: &Text) {
        self.say_pair(
            &format!("{lead}{}", text.get(self.ui.lang())),
            &format!("{lead}{}", text.en()),
        );
    }

    /// `startup: <stage> <ms>` in data\logs\run_launcher.txt: ms since the start began (PLAY).
    fn startup(&self, stage: &str, since: Duration) {
        crate::logs::append(
            &self.logs().join("run_launcher.txt"),
            &rules::startup_line(stage, since),
        );
    }

    /// A line as shown and in English (the log's).
    fn say_pair(&self, shown: &str, en: &str) {
        self.ui.say_logged(shown, en);
        crate::logs::append(&self.logs().join("run_launcher.txt"), en);
    }

    /// A start step's failure: said, then "Stopping.".
    fn failed(&self, f: Failed) {
        self.tell("", &Text::key("launcher.run_failed").part("why", f.0));
        self.tell("", &Text::key("launcher.run_stopping"));
    }

    fn logs(&self) -> PathBuf {
        self.kit.logs()
    }

    fn running_file(&self) -> PathBuf {
        self.kit.data().join("running.json")
    }

    fn card_files(&self) -> (PathBuf, PathBuf) {
        let save = self.kit.save();
        (save.join(card::SLOT), save.join(card::NEW))
    }

    /// Remote: the server's IPv4 address (once step 2 found it).
    fn ip(&self) -> Option<&str> {
        self.remote.as_ref().map(|(ip, _)| ip.as_str())
    }

    /// What every program of the run gets: where the game and the logs are, and where the panel finds its data.
    fn kit_env(&self) -> Vec<(String, String)> {
        let s = |p: PathBuf| p.display().to_string();
        let cache = self.kit.cache();
        vec![
            ("WCCF_GAME".into(), s(self.game.clone())),
            ("WCCF_LOGS".into(), s(self.logs())),
            ("MXHOOK_LOG_DIR".into(), s(self.logs().join("hook"))),
            ("WCCF_CATALOGUE".into(), s(cache.join("catalogue.tsv"))),
            ("WCCF_CARDS".into(), s(cache.join("cards"))),
        ]
    }

    /// `cmd` in the job, its output to data\logs\<log>; its exit code and output once it ends within `limit`.
    fn run_child(&self, cmd: Command, log: &str, limit: Duration) -> Result<(i32, String), String> {
        run_in_job(&self.job, cmd, &self.logs().join(log), limit)
    }

    /// The helper threads: the club card view (written again when the card changes) and the starting volumes of the projector
    /// (muted) and seat 1.  They never stop the run.
    fn start_helper(&mut self, projector: bool) {
        let say = file_log(self.logs().join("run_helper.txt"));
        let mut h = Helper::default();
        let (data, logs) = (self.kit.data(), self.logs());
        // 2017-18 keeps its clubs on the game server stand-in: no club card file to show
        if self.edition == Edition::E1011 {
            h.club_view(
                ViewFiles::new(&self.kit.data()),
                move || {
                    let (code, out) = forza::cards::steps::run(&data, &["view"]);
                    let _ = fs::write(logs.join("run_club_view.txt"), &out);
                    let last = out
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("-")
                        .trim()
                        .to_owned();
                    if code == 0 {
                        Ok(last.trim_start_matches("club card view written: ").to_owned())
                    } else {
                        Err(last)
                    }
                },
                say.clone(),
            );
        }
        let (logs, mut targets) = (self.logs(), Vec::new());
        if projector {
            let logs = logs.clone();
            targets.push(Target {
                name: "the projector",
                pid: Box::new(move || launched_pid(&logs, Role::Projector.log())),
                mute: true,
            });
        }
        targets.push(Target {
            name: "seat 1",
            pid: Box::new(move || launched_pid(&logs, Role::Seat.log())),
            mute: false,
        });
        h.start_volumes(targets, Helper::core_audio(), say);
        self.helper = Some(h);
    }

    /// The card steps the CLUB CARD panel asked for (`forza::cards::steps`: a manager transfer finished, a card switch, its
    /// bad endings cleared, the table following the club), done while the card reader is not running; never a reason
    /// not to start.
    fn card_steps(&self, what: &[&str]) {
        if self.edition == Edition::E1718 {
            // its club is on the game server stand-in, its card is an Aime: no club card file; CLEAR BAD ENDINGS
            // (the panel's card_fix=bad_endings) clears the stand-in's counters instead
            let panel = self.kit.data().join("panel.txt");
            let steps = forza::cards::steps::read_panel(&panel);
            if what.contains(&"start") && steps.contains_key("card_fix") {
                let dir = self.kit.save().join("gamesvr");
                let done = if matches!(self.mode, Mode::Remote(..)) {
                    "the club is on another PC's server - clear its bad endings there".to_owned()
                } else if steps["card_fix"].eq_ignore_ascii_case("bad_endings") {
                    if self.club.is_none() {
                        self.club_leftover(&dir); // no trip on: an out marker is a crash's, the club is home
                    }
                    club::clear_bad_endings(&dir).unwrap_or_else(|e| format!("bad endings not cleared ({e})"))
                } else {
                    String::new()
                };
                if !done.is_empty() {
                    self.say(&format!("  club: {done}"));
                }
                forza::cards::steps::drop_panel_line("card_fix", &panel);
            }
            return;
        }
        let seat = self.seat.display().to_string();
        let mut args = what.to_vec();
        args.push(&seat);
        let (_, out) = forza::cards::steps::run(&self.kit.data(), &args);
        let _ = fs::write(self.logs().join("run_card_steps.txt"), &out);
        out.lines().filter(|l| !l.trim().is_empty()).for_each(|l| self.say(l));
    }

    /// The 2010-11 network guard (docs/SAFETY.md), checked before any game program starts: the game and seat 1
    /// hold this kit's hook (wccfk.dll) and every program loads it (`guard::hook_1011`).  Kit 6 loads its hook as
    /// wccfk.dll; a game set up by an older kit, a newer kit unzipped over this one by hand, or a program put back by
    /// hand fails that - set up once, as Setup does, then check again: still not guarded, nothing starts.
    fn ensure_hook(&self) -> Result<(), Failed> {
        let p = self.p;
        let guarded = || -> Result<(), String> {
            let ours = fs::read(self.kit.programs().join("wccfk.dll")).ok();
            for dir in [&self.game, &self.seat] {
                if ours.is_none() || fs::read(dir.join("wccfk.dll")).ok() != ours {
                    return Err(format!(
                        "{} is not the Forza-WCCF hook",
                        dir.join("wccfk.dll").display()
                    ));
                }
            }
            if !p.hook_imports {
                return Ok(());
            }
            let mut exes: Vec<PathBuf> = p.programs.iter().map(|n| self.game.join(n)).collect();
            exes.push(self.seat.join(p.client()));
            guard::hook_1011(&exes)
        };
        if guarded().is_ok() {
            return Ok(());
        }
        self.tell("", &Text::key("launcher.run_updating_hook"));
        let log = file_log(self.logs().join("run_setup.txt"));
        let say = |shown: &str, en: &str| {
            log(en);
            self.say_pair(shown, en);
        };
        let ui = Lines(self.ui.lang(), &say);
        if crate::setup::folder_step(self.kit, &self.game.display().to_string(), false, &ui) != 0 {
            self.tell("", &Text::key("launcher.run_setup_unfinished"));
            return Err(Failed::quiet());
        }
        guarded().map_err(|why| {
            self.tell("", &Text::key("launcher.run_not_guarded").arg("why", why));
            Failed::quiet()
        })
    }

    /// `lang` ("en", "it", "" Japanese) into the game files, logged in run_english.txt: None when done, else why not.
    fn switch_text(&self, lang: &str) -> Option<String> {
        // forza::text swaps each file in one step, so even a stop half way leaves every file in place
        let log = file_log(self.logs().join("run_english.txt"));
        let last = Mutex::new(String::new());
        let say = |_: &str, l: &str| {
            log(l);
            if !l.trim().is_empty() {
                *last.lock().unwrap_or_else(PoisonError::into_inner) = l.trim().to_owned();
            }
        };
        match crate::setup::text_step(self.kit, lang, &Lines(self.ui.lang(), &say)) {
            0 => None,
            code => Some(
                Some(last.into_inner().unwrap_or_else(PoisonError::into_inner))
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| format!("exit {code}")),
            ),
        }
    }

    /// 2017-18: the original-text backup is kept beside the game; a Forza-WCCF folder with none of its own takes it.
    /// Text that is not original with no matching backup is said, not changed.
    fn adopt_text_backup(&self) {
        let backup = self.kit.data().join("english_backup");
        if forza::text::sync_beside(self.edition, &self.game, &backup, self.kit.dir()) == forza::text::Beside::Foreign {
            self.tell("  ", &Text::key("launcher.run_text_foreign"));
        }
    }

    /// English or Italian built from an older kit's sheets (a kit copied over the old one, no text switch since):
    /// built again from this kit's, in the same language, before anything starts.
    fn refresh_old_text(&self) {
        let lang = self.kit.game_language();
        let backup = self.kit.data().join("english_backup");
        if lang.is_empty() || !forza::text::stale(self.edition, &backup, self.kit.dir()) {
            return;
        }
        self.tell("  ", &Text::key("launcher.run_text_old"));
        match self.switch_text(lang) {
            Some(why) => self.tell("  ", &Text::key("launcher.run_text_not_built").arg("why", why)),
            None => self.tell("  ", &Text::key("launcher.run_text_current")),
        }
    }

    /// The SETTINGS panel's english=on|it|off: done now, before anything starts (the switch refuses while the game
    /// runs), then taken out of the file - a request, not a lasting setting.
    fn apply_english_request(&self) {
        let path = self.kit.data().join("panel.txt");
        let panel = choices::read(&path);
        let Some(want) = choices::panel_value(&panel, "english").map(|v| v.to_lowercase()) else {
            return;
        };
        if !["on", "it", "off"].contains(&want.as_str()) {
            return;
        }
        if let Some(lang) = rules::english_change(&panel, self.kit.game_language()) {
            let name = Text::key(match lang {
                "en" => "launcher.run_lang_en",
                "it" => "launcher.run_lang_it",
                _ => "launcher.run_lang_ja",
            });
            self.tell(
                "  ",
                &Text::key("launcher.run_text_switching").part("name", name.clone()),
            );
            match self.switch_text(lang) {
                Some(why) => self.tell("  ", &Text::key("launcher.run_text_not_switched").arg("why", why)),
                None => self.tell("  ", &Text::key("launcher.run_text_now").part("name", name)),
            }
        }
        // read again, and fallible: this text is written back whole, so a file that cannot be read now (a scanner
        // holding it) must not be saved over as if the request had been taken out of it
        let text = match choices::read_ok(&path) {
            Ok(text) => text,
            Err(e) => {
                self.tell(
                    "  ",
                    &Text::key("launcher.msg_read_failed")
                        .arg("path", path.display())
                        .arg("error", e),
                );
                return;
            }
        };
        let left = choices::set_panel_value(&text, "english", None);
        let _ = choices::write(&path, &left);
    }

    /// The run's `session:` line in run_launcher.txt, once it has started (`projector`: None on a server).
    fn session(&mut self, projector: Option<bool>) {
        let arcade = match &self.mode {
            Mode::Remote(entry, _) => Some(entry.as_str()),
            _ => self.arcade.as_deref(),
        };
        crate::logs::append(
            &self.logs().join("run_launcher.txt"),
            &rules::session_line(&self.mode, arcade, self.seat_no, projector),
        );
        let logs = self.logs();
        self.sampler.get_or_insert_with(|| sampler::spawn(logs));
    }

    fn write_running(&self) {
        let t = logs::local(SystemTime::now());
        let started = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute(),
            t.second()
        );
        // a start stamp: the panel and the card rule read this file's write time as the run's start
        let info = serde_json::json!({"started": started, "mode": self.mode.word(), "server": self.ip(),
                                      "seat": self.seat_no});
        let _ = choices::write(&self.running_file(), &info.to_string());
    }

    fn run_started(&self) -> Option<SystemTime> {
        fs::metadata(self.running_file()).and_then(|m| m.modified()).ok()
    }

    /// A game program under a debugger thread of its own (it creates the program, then pumps its events); its end
    /// comes as an event.
    fn start_game(
        &mut self,
        role: Role,
        args: Vec<String>,
        dir: &Path,
        hide: bool,
        env: Vec<(String, String)>,
    ) -> Result<(), Failed> {
        let (exe, prog, drole) = match role {
            Role::Server => (self.p.control(), "control", debug::Role::Control),
            _ => (self.p.client(), "client", debug::Role::Client),
        };
        let tag = if dir == self.game {
            String::new()
        } else {
            format!("_{}", dir.file_name().unwrap_or_default().to_string_lossy())
        };
        // 2017-18: the original programs take ASCII paths only; the guard checked the real folder this points at
        let from = if self.edition == Edition::E1718 {
            match crate::alias::for_dir(dir, &self.p.run_links) {
                Ok(link) => link,
                Err(e) => {
                    if !self.told_path {
                        self.told_path = true;
                        let line = if e.kind() == io::ErrorKind::AlreadyExists {
                            Text::key("launcher.run_path_taken").arg("folder", self.p.run_links.display())
                        } else {
                            Text::key("launcher.run_path_not_ascii")
                                .arg("folder", self.p.run_links.display())
                                .arg("error", &e)
                        };
                        self.tell("  ", &line);
                        crate::logs::append(
                            &self.logs().join("run_launcher.txt"),
                            &format!("no ASCII junction for {}: {e}", dir.display()),
                        );
                    }
                    None
                }
            }
        } else {
            None
        };
        // through a junction: the started process must be the program the guard checked (job::Job::spawn_debug)
        let checked = from.is_some().then(|| dir.join(exe));
        let dir = from.as_deref().unwrap_or(dir);
        let index = self.kit.dir().join(r"code\hardware\ghidra_out_1011_classes");
        let stop = Arc::new(AtomicBool::new(false));
        let opts = Options {
            role: drole,
            exe: dir.join(exe),
            args,
            timeout: LIFE,
            follow: true,
            hide,
            debug_heap: false,
            maps: self.p.maps.clone().map(|id| Maps {
                game: self.game.clone(),
                id,
                mosuppl: true,
                edition: self.edition,
            }),
            dbg_file: self.logs().join(format!("_dbg_{prog}{tag}.txt")),
            dumps: self.kit.data().join("dumps"),
            index: index.is_dir().then_some(index),
            // control+0x1226A is 2010-11's control: 2017-18's is guarded by its hook (whole messages)
            above_normal: self.edition == Edition::E1718,
            background: role == Role::Projector && self.edition == Edition::E1718,
            msgguard: (self.edition == Edition::E1011).then(|| env::var("WCCF_MSGGUARD").map_or(true, |v| v != "0")),
            env: self
                .kit_env()
                .into_iter()
                .chain(panel_role(self.edition, role))
                .chain(env)
                .chain([game_dpi()])
                .collect(),
            stop: Some(stop.clone()),
            hang_after: crate::debug::HANG_AFTER,
            checked,
        };
        let mut log = logs::Stamped::new(File::create(self.logs().join(role.log()))?);
        let (job, tx, id) = (self.job.clone(), self.tx.clone(), self.next_id);
        self.next_id += 1;
        let thread = thread::spawn(move || {
            if let Err(e) = debug::run(&opts, &job, &mut log) {
                let _ = writeln!(log, "the debugger could not go on: {e}");
            }
            let _ = tx.send(Event::Ended(id));
        });
        self.games.push(Game {
            id,
            role,
            stop,
            thread: Some(thread),
        });
        Ok(())
    }

    fn alive(&self, role: Role) -> bool {
        self.games.iter().any(|g| g.role == role && g.alive())
    }

    /// The program's id from its debugger's "launched NAME pid N" line, once it is there.
    fn launched(&self, role: Role) -> Option<u32> {
        launched_pid(&self.logs(), role.log())
    }

    /// The job's processes now, with their names.
    fn job_procs(&self) -> Vec<procs::Proc> {
        let pids = self.job.pids();
        procs::processes()
            .into_iter()
            .filter(|q| pids.contains(&q.pid))
            .collect()
    }

    fn job_count(&self, name: &str) -> usize {
        self.job_procs()
            .iter()
            .filter(|q| q.name.eq_ignore_ascii_case(name))
            .count()
    }

    /// 2010-11: the panel (forza_panel.dll) into the running game: "" when it loaded, else why not.
    fn inject(&self, pid: u32, dll: &Path) -> String {
        let mut cmd = Command::new(self.kit.program("inject.exe"));
        cmd.arg(pid.to_string()).arg(dll);
        match self.run_child(cmd, "run_inject.txt", Duration::from_secs(60)) {
            Ok((0, _)) => String::new(),
            Ok((code, out)) if out.trim().is_empty() => format!("exit {code}"),
            Ok((_, out)) => out.trim().to_owned(),
            Err(e) => e,
        }
    }

    /// 2017-18: the env that makes wccfnet.dll load the panel into this game process (seat 1, projector; never control).
    fn panel_env(&self) -> Vec<(String, String)> {
        if self.edition != Edition::E1718 {
            return Vec::new();
        }
        let mut env = vec![(
            "FORZA_PANEL".to_owned(),
            self.kit.program("forza_panel.dll").to_string_lossy().into_owned(),
        )];
        // a join's tunnel moved the control port: the panel's link watch looks for the seat's link on that one
        if let Some(t) = self.tunnel_ports.filter(|t| t.control != self.p.control_port) {
            env.push(("WCCFPANEL_CONTROL_PORT".into(), t.control.to_string()));
        }
        env
    }

    fn start_all(&mut self) -> Result<(), Failed> {
        let kit = self.kit;
        let p = self.p;
        if self.beside_server {
            fs::create_dir_all(self.logs().join("hook"))?; // its logs are open: no rotation
            let panel = choices::read(&kit.data().join("panel.txt"));
            if rules::english_change(&panel, kit.game_language()).is_some() {
                self.tell("  ", &Text::key("launcher.run_lang_skipped"));
            }
        } else {
            for line in logs::rotate(&self.logs())? {
                self.say(&line);
            }
            self.adopt_text_backup();
            self.apply_english_request();
            self.refresh_old_text();
            if self.edition == Edition::E1011 {
                // after the text switch, which puts programs back: 2017-18's guard is checked in `start`
                self.ensure_hook()?;
            }
        }
        self.startup("setup", self.early[0]);
        self.startup("guard", self.early[1]);
        // here the start is admitted (nothing else writes the pictures), unrefused, guarded and the folder repaired; a
        // server that does not answer is only found at step 2, after this wait. Not beside a server on this PC: its
        // game reads the folder. A failure never stops the start.
        if !self.beside_server && pictures_due(kit, p) {
            self.ui.updating_pictures();
            self.tell("", &Text::key("launcher.msg_updating_pictures"));
            if !pictures_held(kit, p, self.ui) {
                self.tell("  ", &Text::key("launcher.run_pictures_as_is"));
            }
        }
        // the card reader is not running yet in any mode; nor FPR_Emu or the overlay, which read the table
        if self.mode.is_server() {
            self.card_steps(&["start"]);
        } else {
            self.card_steps(&["start", "board"]);
        }
        let steps = if self.mode.is_server() { 2 } else { 6 };
        let confs = [
            self.game.join(r"local\client_user_option.conf"),
            self.seat.join(r"local\client_user_option.conf"),
        ];
        if self.edition == Edition::E1718 {
            // shortcut: a guest at another PC's arcade shows the default, not that arcade's name; upgrade when the desk
            // tells its guests the name
            let name = match self.mode {
                Mode::Remote(..) => String::new(),
                _ => choices::setting(&self.kit.settings_file(), "shop_name").unwrap_or_default(),
            };
            for c in confs.iter().filter(|c| c.is_file()) {
                // a label only: a conf that cannot be written is told and the start goes on
                if let Err(e) = conf::set_place(c, &name) {
                    self.tell(
                        "  ",
                        &Text::key("launcher.run_place_failed")
                            .arg("file", c.display())
                            .arg("error", e),
                    );
                }
            }
        }
        let starting = match &self.mode {
            Mode::Server | Mode::LanServer => Text::key("launcher.run_starting_server"),
            Mode::Remote(ip, _) => Text::key("launcher.run_starting_remote").arg("ip", ip),
            Mode::Vs(_) => Text::key("launcher.run_starting_vs"),
            Mode::Local => Text::key("launcher.run_starting"),
        };
        self.tell(
            "",
            &starting
                .arg("edition", self.edition.name())
                .arg("folder", self.game.display()),
        );
        if self.edition == Edition::E1718 {
            self.start_gamesvr(steps)?;
        } else if self.beside_server {
            self.tell(&format!("  1/{steps} "), &Text::key("launcher.run_scenes_shared"));
        } else {
            let xscn = self.game.join(r"data\motion\launcherMotion\xscn");
            let log = file_log(self.logs().join("xscn_watch_log.txt"));
            let h = scenes::spawn(&xscn, &p.scenes, log)
                .map_err(|e| Failed(Text::key("launcher.run_scenes_failed").arg("error", e)))?;
            self.scenes = Some(h);
            self.tell(&format!("  1/{steps} "), &Text::key("launcher.run_scenes"));
        }

        let mut projector = true;
        if let Mode::Remote(entry, want) = self.mode.clone() {
            // the entry as written: a 2017-18 arcade's id (its tunnel serves this PC's loopback), or a name or an IPv4
            // address, ":port" the seat desk's (the lobby's entry)
            let arcade = vslink::arcade::parse_id(&entry).filter(|_| self.edition == Edition::E1718);
            let (ip, desk) = match arcade {
                Some(id) => {
                    let ports = self.start_arcade_tunnel(vslink::arcade::Role::Join(id))?;
                    self.tunnel_ports = Some(ports);
                    ("127.0.0.1".to_owned(), ports.desk)
                }
                None => choices::resolve(&entry, p.seat_port)
                    .map(|a| (a.ip().to_string(), a.port()))
                    .ok_or_else(|| Failed(Text::key("launcher.run_not_found").arg("entry", &entry)))?,
            };
            // through the tunnel the arcade is at 127.0.0.1 too, but its game server is the host's
            self.own_store = rules::own_store(&ip, arcade.is_some());
            // this PC's own server by its LAN address: loopback, where a 2017-18 arcade's desk and relay listen
            let ip = if self.own_store { "127.0.0.1".to_owned() } else { ip };
            self.remote = Some((ip.clone(), desk));
            let control_port = self.tunnel_ports.map_or(p.control_port, |t| t.control);
            if !answers(&ip, control_port) {
                return Err(Failed(
                    Text::key("launcher.run_remote_no_answer")
                        .arg("entry", &entry)
                        .arg("port", control_port),
                ));
            }
            let (seat, here) = self.take_seat(&ip, want)?;
            (self.seat_no, projector) = (seat, here);
            for c in &confs {
                conf::set_control_ip(c, &ip)?;
                if self.edition == Edition::E1718 {
                    // the tunnel's control port when this PC holds 20002 itself
                    conf::set_conf_value(c, "CONTROL_PORT", &control_port.to_string())?;
                }
            }
            conf::set_satellite_no(&confs[1], u32::from(seat))?; // the cabinet's seat
            if self.edition == Edition::E1718 && !self.own_store {
                self.club = None; // a trip of an earlier start comes home first
                self.visit.club = None; // and its progress is not this start's
                if let Err(why) = self.bring_club(addr(&ip, desk), seat, &entry) {
                    self.tell(
                        "  ",
                        &Text::key("launcher.run_with_why")
                            .part("text", Text::key("launcher.club_not_in"))
                            .part("why", why),
                    );
                }
            }
            // never "WAITING FOR THE PROJECTOR" on a server: its projector is on another PC, or gone
            conf::set_conf_value(&confs[1], "WAIT_PROJECTOR", "0")?;
            self.tell(
                &format!("  2/{steps} "),
                &Text::key(if here {
                    "launcher.run_remote_seat_projector"
                } else {
                    "launcher.run_remote_seat"
                })
                .arg("ip", &ip)
                .arg("port", control_port)
                .arg("seat", seat),
            );
        } else {
            let ctrl_conf = self.game.join(r"local\ctrl_user_option.conf");
            if matches!(self.mode, Mode::Local | Mode::Vs(_)) {
                // 2017-18: the projector program runs for its window or for the CPU championship it plays (hidden when
                // only the championship wants it); with both off, none, and nobody waits for it
                let s = self.kit.settings();
                projector =
                    self.edition != Edition::E1718 || choices::projector_shown(&s) || choices::cpu_championship_on(&s);
                let wait = if projector { "1" } else { "0" };
                for c in &confs {
                    conf::set_control_ip(c, "127.0.0.1")?; // after a "remote" run: the cabinets play on this PC again
                    if self.edition == Edition::E1718 {
                        conf::set_conf_value(c, "CONTROL_PORT", &p.control_port.to_string())?;
                    }
                }
                conf::set_satellite_no(&confs[1], 1)?;
                conf::set_conf_value(&confs[1], "WAIT_PROJECTOR", wait)?; // the game's own way, its projector here
                conf::set_conf_value(&ctrl_conf, "WAIT_PROJECTOR", wait)?;
            } else if self.edition == Edition::E1718 {
                // a server for others: its seat desk gives the first PC the projector, which control waits for
                conf::set_conf_value(&ctrl_conf, "WAIT_PROJECTOR", "1")?;
            } else {
                // a server for others: no projector waited for (the seat desk gives no PC the projector)
                conf::set_conf_value(&ctrl_conf, "WAIT_PROJECTOR", "0")?;
            }
            let game = self.game.clone();
            if self.edition == Edition::E1718 {
                // the control saves the Universal Championship in log\<date>_uc.bin but makes no folder (0x41EF00)
                fs::create_dir_all(game.join("log"))?;
            }
            // the CPU championship off: the control's hook keeps an all-CPU Universal Championship off the projector
            // (native/hook/src/uclive.rs); the projector then plays the control's own demo pair instead
            let env = if self.edition == Edition::E1718 && !choices::cpu_championship_on(&self.kit.settings()) {
                vec![("WCCF_UCLIVE".to_owned(), "on".to_owned())]
            } else {
                Vec::new()
            };
            self.start_game(Role::Server, p.control_args.clone(), &game, !self.debug, env)?;
            let said = std::cell::Cell::new(false);
            wait_for(
                Text::key("launcher.run_part_server"),
                90,
                Role::Server.log(),
                || self.alive(Role::Server),
                || match procs::listening_ports_result() {
                    Ok(ports) => ports.contains(&p.control_port) && self.job_count(p.matcher()) >= p.match_engines,
                    // the table could not be read: that is not "the server is not up", and the job's programs are a
                    // weaker but honest sign; said once, so the log says why the wait is longer than usual
                    Err(e) => {
                        if !said.replace(true) {
                            self.tell("  ", &Text::key("launcher.run_ports_unread").arg("error", &e));
                        }
                        self.job_count(p.matcher()) >= p.match_engines
                    }
                },
            )?;
            if let Some(ctl) = self
                .job_procs()
                .into_iter()
                .find(|q| q.name.eq_ignore_ascii_case(p.control()))
            {
                // a click in the server's console would freeze it (text-selection mode): switch that off, in a child of
                // Launcher.exe (it changes the console of the process that does it)
                let exe = self.kit.dir().join(Kit::exe_name());
                if exe.is_file() {
                    let mut cmd = Command::new(exe);
                    cmd.arg(helper::console::ARG).arg(ctl.pid.to_string());
                    let _ = self.run_child(cmd, "run_console.txt", Duration::from_secs(15));
                }
                if !self.debug {
                    procs::hide_windows(ctl.pid); // it starts hidden; a window it shows by itself is hidden here
                }
            }
            self.tell(
                &format!("  2/{steps} "),
                &Text::key("launcher.run_server_up")
                    .arg("port", p.control_port)
                    .arg("n", self.job_count(p.matcher())),
            );
        }
        self.startup("engines", self.t0.elapsed());
        if self.mode.is_server() {
            return self.start_server_side();
        }
        self.projector = projector;

        if projector {
            let args = p.projector_args.clone().unwrap_or_else(|| vec!["0".into()]);
            let game = self.game.clone();
            self.start_game(Role::Projector, args, &game, false, self.panel_env())?;
            let hidden = self.edition == Edition::E1718 && !choices::projector_shown(&self.kit.settings());
            self.tell(
                "  3/6 ",
                &Text::key(if hidden {
                    "launcher.run_projector_hidden"
                } else {
                    "launcher.run_projector"
                }),
            );
        } else {
            self.tell(
                "  3/6 ",
                &Text::key(match self.edition {
                    Edition::E1011 => "launcher.run_no_projector_1011",
                    Edition::E1718 if self.remote.is_none() => "launcher.run_projector_off",
                    Edition::E1718 => "launcher.run_no_projector_1718",
                }),
            );
        }
        // 2017-18 shows seat 1's window only at its start warning (#229), ~20 s after the projector plays: mute it now
        let helper_first = self.edition == Edition::E1718;
        if helper_first {
            self.start_helper(projector);
        }
        self.start_cabinet()?;
        if projector {
            // the overlay in its money-only mode: the projector's amounts with commas; it plays on without it.
            // 2017-18: the game loads it itself (wccfnet.dll, from FORZA_PANEL)
            if self.edition == Edition::E1718 {
                self.tell("       ", &Text::key("launcher.run_money_by_game"));
            } else {
                let panel = self.kit.program("forza_panel.dll");
                let why = self.launched(Role::Projector).map_or_else(
                    || Text::key("launcher.run_no_process"),
                    |pid| Text::Raw(self.inject(pid, &panel)),
                );
                self.tell(
                    "       ",
                    &if why == Text::Raw(String::new()) {
                        Text::key("launcher.run_money_on")
                    } else {
                        Text::key("launcher.run_money_off").part("why", why)
                    },
                );
            }
        }
        if !helper_first {
            self.start_helper(projector);
        }
        self.write_running();
        self.session(Some(projector));
        self.say("");
        if let Mode::Remote(ip, _) = &self.mode {
            let playing = if projector {
                Text::key("launcher.run_playing_remote")
            } else {
                Text::key("launcher.run_playing_remote_seat").arg("seat", self.seat_no)
            };
            self.tell("", &playing.arg("ip", ip));
        }
        self.tell("", &Text::key("launcher.run_running"));
        self.tell("  ", &Text::key("launcher.run_keys1"));
        self.tell("  ", &Text::key("launcher.run_keys2"));
        self.tell(
            "",
            &Text::key(match self.edition {
                Edition::E1011 => "launcher.run_card_1011",
                Edition::E1718 => "launcher.run_card_1718",
            }),
        );
        self.tell("", &Text::key("launcher.run_to_quit"));
        Ok(())
    }

    /// "server": `forza-server all` (2017-18: `Launcher.exe server all`; the seat desk and the match relay in one Job child, each with its
    /// own log in data\logs), then the run waits for a stop.
    fn start_server_side(&mut self) -> Result<(), Failed> {
        let logs = self.logs();
        let mut env = self.kit_env();
        env.extend([
            ("WCCF_BROKER_PORT".to_owned(), self.p.seat_port.to_string()),
            ("WCCF_RELAY_PORT".to_owned(), self.p.relay_port.to_string()),
        ]);
        // 2017-18: guests come through the arcade tunnel, so the desk and relay listen on loopback only (no firewall
        // prompt, nothing open on the LAN) unless data\settings.json "shop_lan": true asks for the plain-address path
        // `server lan` (2017-18): this network only - no tunnel, no lobby room, LAN searches answered
        let lan_only = self.edition == Edition::E1718 && self.mode == Mode::LanServer;
        // "shop_lan": settings key kept from before the arcade rename
        let lan = lan_only || self.edition == Edition::E1011 || self.kit.settings()["shop_lan"].as_bool() == Some(true);
        if !lan {
            env.push(("WCCF_BIND".to_owned(), "127.0.0.1".to_owned()));
        }
        if self.edition == Edition::E1718 && !lan_only {
            // every tunnel guest reaches the desk as 127.0.0.1: no per-address limits there
            env.push(("WCCF_DESK_TUNNEL".to_owned(), "1".to_owned()));
        }
        // the arcade's name this runtime set; empty: the desk's and the LAN answer's defaults
        let name = forza::online::lobby::arcade_name(
            &choices::setting(&self.kit.settings_file(), "shop_name").unwrap_or_default(),
        );
        if self.edition == Edition::E1718 {
            // guests bring their own clubs into this PC's store (online::club), on the tunnel and on the LAN
            let db = self.kit.save().join("gamesvr").join("gamesvr.db");
            env.push(("WCCF_CLUB_DB".to_owned(), db.display().to_string()));
            // the ARCADES row for this desk's own arcade (a private room is never listed)
            if !name.is_empty() {
                env.push(("WCCF_SHOP_NAME".to_owned(), name.clone()));
            }
        }
        // 2017-18 ships no forza-server.exe (an AV false positive): the kit's Launcher.exe runs it as `server ...`
        let mut cmd = if self.edition == Edition::E1718 {
            let mut c = Command::new(self.kit.dir().join("Launcher.exe"));
            c.arg("server");
            c
        } else {
            Command::new(self.kit.program("forza-server.exe"))
        };
        cmd.args(["--edition", self.edition.name(), "all"]);
        if self.edition == Edition::E1718 {
            // this PC's game server is the run's own stand-in (start_gamesvr)
            cmd.arg("--no-gmsvr");
        }
        if lan_only {
            // the name guests see in their list: the arcade's, as on its ARCADES row, else this PC's
            cmd.arg("--lan").arg(lan_name(name));
        }
        cmd.arg((LIFE.as_secs() + 60).to_string()).arg(&logs);
        clean_env(&mut cmd, &logs, &env);
        let out = "run_forza_server.txt";
        let role = "online server";
        let started = File::create(logs.join(out)).and_then(|f| {
            cmd.stderr(f.try_clone()?).stdout(f);
            self.job.spawn(&mut cmd)
        });
        match started {
            Ok(child) => self.kids.push((role, child)),
            Err(e) => self.tell("     ", &Text::key("launcher.run_server_not_started").arg("error", e)),
        }
        thread::sleep(Duration::from_millis(1500));
        let alive = self
            .kids
            .last_mut()
            .is_some_and(|(r, c)| *r == role && c.try_wait().is_ok_and(|s| s.is_none()));
        if alive {
            let desk = match self.edition {
                Edition::E1011 => "launcher.run_desk_1011",
                Edition::E1718 => "launcher.run_desk_1718",
            };
            self.tell("     ", &Text::key(desk).arg("port", self.p.seat_port));
            self.tell("     ", &Text::key("launcher.run_relay").arg("port", self.p.relay_port));
            if lan_only {
                let ips: Vec<String> = forza::online::lan::addresses()
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                let ips = if ips.is_empty() {
                    Text::key("launcher.run_none_found")
                } else {
                    Text::Raw(ips.join(", "))
                };
                self.tell(
                    "     ",
                    &Text::key("launcher.run_lan_arcade")
                        .arg("port", forza::online::lan::PORT)
                        .part("ips", ips),
                );
            }
        } else {
            self.tell("     ", &Text::key("launcher.run_server_ended_early").arg("log", out));
        }
        if self.edition == Edition::E1718 && !lan_only {
            // guests reach this arcade through iroh: no inbound port; the plain address stays the LAN fallback
            if let Err(Failed(why)) = self.start_arcade_tunnel(vslink::arcade::Role::Host) {
                let off = Text::key("launcher.run_tunnel_off").part("why", why);
                self.tell("     ", &off);
                self.arcade_room = Some(Err(off.get(self.ui.lang())));
                self.tell(
                    "     ",
                    &Text::key(if lan {
                        "launcher.run_lan_still"
                    } else {
                        "launcher.run_lan_only_flag"
                    }),
                );
            }
        }
        self.write_running();
        self.session(None);
        self.say("");
        self.tell(
            "",
            &Text::key(if lan_only {
                "launcher.run_server_lan"
            } else if self.edition == Edition::E1718 && !lan {
                "launcher.run_server_tunnel"
            } else {
                "launcher.run_server_plain"
            }),
        );
        self.tell(
            "",
            &if self.edition == Edition::E1718 && !lan {
                Text::key("launcher.run_local_ports")
            } else {
                Text::key("launcher.run_open_ports")
                    .arg("control", self.p.control_port)
                    .arg("desk", self.p.seat_port)
                    .arg("relay", self.p.relay_port)
            },
        );
        Ok(())
    }

    /// Steps 4-6 and the key driver: seat N's own side - the keychip and network, card reader and I/O board stand-ins,
    /// the cabinet (the seat1 folder) with its overlay.  A cabinet restart runs only this.
    fn start_cabinet(&mut self) -> Result<(), Failed> {
        let p = self.p;
        let net_log = file_log(self.logs().join("ringedge_log.txt"));
        for (&(_, name, kind), &port) in network::SERVICES.iter().zip(&p.network_ports) {
            let listener = TcpListener::bind(("127.0.0.1", port))
                .map_err(|e| Failed(Text::key("launcher.run_net_ports").arg("port", port).arg("error", e)))?;
            self.cabinet.net.push(network::spawn(
                listener,
                kind,
                name,
                self.kit.edition().game_id(),
                net_log.clone(),
            )?);
        }
        self.cabinet.extra = std::mem::take(&mut *p.extra_watch.lock().unwrap_or_else(PoisonError::into_inner));
        let coms = if self.edition == Edition::E1718 {
            self.start_aime()?;
            format!("COM2={}", p.aime_pipe)
        } else {
            let (slot, new) = self.card_files();
            // the card reader logs every byte: only with "debug" (~60 MB an hour)
            let icc_log: Log = if self.debug {
                file_log(self.logs().join("icc_log.txt"))
            } else {
                Arc::new(|_: &str| {})
            };
            self.cabinet.reader = Some(
                card_reader::spawn(&p.icc_pipe, Some(&slot), Some(&new), icc_log)
                    .map_err(|e| Failed(Text::key("launcher.run_reader_failed").arg("error", e)))?,
            );
            format!("COM3={0};COM1={0}", p.icc_pipe)
        };
        let jvs_log = file_log(self.logs().join("jvs_log.txt"));
        self.cabinet.board = Some(
            io_board::spawn(&p.jvs_pipe, p.shared.clone(), jvs_log)
                .map_err(|e| Failed(Text::key("launcher.run_board_failed").arg("error", e)))?,
        );
        self.tell(
            "  4/6 ",
            &Text::key(match self.edition {
                Edition::E1011 => "launcher.run_cabinet_1011",
                Edition::E1718 => "launcher.run_cabinet_1718",
            }),
        );

        let mut env = self.panel_env();
        env.extend([
            ("MXHOOK_RINGEDGE".to_owned(), "1".to_owned()),
            ("MXHOOK_COM".into(), format!("{coms};mxjvs={0};COM4={0}", p.jvs_pipe)),
        ]);
        if let Some(ip) = self.ip() {
            // a match against another player goes through the server's relay (forza_panel.dll)
            let relay = self.tunnel_ports.map_or(p.relay_port, |t| t.relay);
            env.push(("WCCF_RELAY".into(), format!("{ip}:{relay}")));
            env.push(("WCCF_SEAT".into(), self.seat_no.to_string()));
        }
        if self.debug {
            env.push(("MXHOOK_VERBOSE".into(), "1".into()));
        }
        let n = self.seat_no;
        let args = p.seat_args.clone().unwrap_or_else(|| vec![n.to_string()]);
        let seat = self.seat.clone();
        // before the start: the 2017-18 window is held hidden until the start warning, long after "first frame shown"
        let olog = seat.join("forza_panel.log");
        let skip = fs::metadata(&olog).map_or(0, |m| m.len() as usize);
        self.start_game(Role::Seat, args, &seat, false, env)?;
        let mut pid = None;
        wait_for(
            Text::key("launcher.run_part_seat").arg("n", n),
            30,
            Role::Seat.log(),
            || self.alive(Role::Seat),
            || {
                pid = self.launched(Role::Seat);
                pid.is_some()
            },
        )?;
        let pid = pid.unwrap_or_default();
        wait_for(
            Text::key("launcher.run_part_seat_window").arg("n", n),
            p.window_wait.as_secs(),
            Role::Seat.log(),
            || self.alive(Role::Seat),
            || procs::has_window(pid),
        )?;
        self.tell("  5/6 ", &Text::key("launcher.run_window_up").arg("n", n));
        self.startup("window", self.t0.elapsed());
        // 2017-18: wccfnet.dll loads the panel 3 s after the window (FORZA_PANEL); 2010-11: inject.exe does
        let panel = self.kit.program("forza_panel.dll");
        let why = if self.edition == Edition::E1718 {
            if panel.is_file() {
                Text::Raw(String::new())
            } else {
                Text::key("launcher.run_panel_missing")
            }
        } else {
            thread::sleep(Duration::from_secs(3));
            Text::Raw(self.inject(pid, &panel))
        };
        if why == Text::Raw(String::new()) {
            let up = wait_for(
                Text::key("launcher.run_part_overlay"),
                60,
                "",
                || true,
                || {
                    fs::read(&olog)
                        .is_ok_and(|t| String::from_utf8_lossy(t.get(skip..).unwrap_or_default()).contains(PANEL_UP))
                },
            );
            self.tell(
                "  6/6 ",
                &Text::key(if up.is_ok() {
                    "launcher.run_panels"
                } else {
                    "launcher.run_panels_unconfirmed"
                }),
            );
        } else {
            // the game runs without the overlay - never a reason to take the game away
            self.tell("  6/6 ", &Text::key("launcher.run_panels_failed").part("why", why));
        }
        let keys_path = env::var_os("WCCF_KEYS").map_or_else(|| self.kit.data().join("keys.txt"), PathBuf::from);
        self.cabinet.keys = Some(crate::keys::spawn(
            p.shared.clone(),
            keys_path,
            // 2017-18's panel SETTINGS has CONTROLLER IN THE BACKGROUND (data\panel.txt); 2010-11 never sets it
            (self.kit.edition() == Edition::E1718).then(|| self.kit.data().join("panel.txt")),
            // START puts in a coin only where the game is not in FREE PLAY: 2017-18 runs it from the panel (seen live, run 8)
            self.kit.edition() != Edition::E1718,
            file_log(self.logs().join("run_keys.txt")),
        ));
        Ok(())
    }

    /// 2017-18's card: the Aime reader on its pipe (the game opens COM2) and AiMeDB, which gives the card its Aime id.
    fn start_aime(&mut self) -> Result<(), Failed> {
        let (p, save) = (self.p, self.kit.save());
        fs::create_dir_all(&save)?;
        let log = file_log(self.logs().join("aime_log.txt"));
        self.cabinet.reader = Some(
            aime::spawn(&p.aime_pipe, Some(&save.join("seat1_aime.txt")), log)
                .map_err(|e| Failed(Text::key("launcher.run_aime_failed").arg("error", e)))?,
        );
        let listener = TcpListener::bind(("127.0.0.1", p.aimedb_port)).map_err(|e| {
            Failed(
                Text::key("launcher.run_aimedb_port")
                    .arg("port", p.aimedb_port)
                    .arg("error", e),
            )
        })?;
        let log = file_log(self.logs().join("aimedb_log.txt"));
        self.cabinet.aimedb = Some(
            // on another PC's game server every card answers the hash of its access code; this PC alone and a seat on
            // this PC's own server keep their store's IDs (aimedb.rs)
            aimedb::spawn_with(
                listener,
                save.join("aimedb.json"),
                log,
                rules::aime_hashed(&self.mode, self.own_store),
            )
            .map_err(|e| Failed(Text::key("launcher.run_aimedb_failed").arg("error", e)))?,
        );
        Ok(())
    }

    /// Step 1 for 2017-18: no scene service; control keeps the clubs on a game server. This PC: the stand-in beside it
    /// (ctrl_user_option.conf GAME_SVR_IP=localhost). Online ([`Mode::Vs`]): the server's game server with
    /// GAME_SVR_ENTRY=1 (the control's VS entries, 243-246), the lobby's VS room claimed at its desk, the direct VS
    /// link. A cabinet on a server uses that server's.
    fn start_gamesvr(&mut self, steps: u32) -> Result<(), Failed> {
        // a new run: nothing of the last one until this one picks its cup
        remember_cup(None);
        if !matches!(self.mode, Mode::Remote(..)) {
            self.club_leftover(&self.kit.save().join("gamesvr"));
        }
        if matches!(self.mode, Mode::Remote(..)) {
            self.tell(&format!("  1/{steps} "), &Text::key("launcher.run_gamesvr_remote"));
            return Ok(());
        }
        let port = self.p.gamesvr_port;
        let ctrl_conf = self.game.join(r"local\ctrl_user_option.conf");
        // the Universal Cup: the host dialog's choice, which the control reads only as it starts (a new choice needs a
        // new start; local play's PLAY tab writes the same setting). A cup at a time is a schedule slot
        // (local\schedule_option.conf); VS keeps the game's times.
        let plan = rules::cup_plan(&self.mode, &self.kit.settings(), forza_ui::lobby::local_clock());
        // the window's PLAY tab shows this run's result while it runs, not a new one every frame
        remember_cup(Some(plan));
        let ((times, friendlies), line) = match plan {
            Ok(p) => (
                conf::cup_files(p),
                match p {
                    CupPlan::Off => Text::key("launcher.run_cup_off"),
                    CupPlan::GameTimes => Text::key("launcher.run_cup_game_times"),
                    CupPlan::Slot { opens, .. } => {
                        Text::key("launcher.run_cup_at").arg("time", forza_ui::lobby::clock_text(opens))
                    }
                },
            ),
            Err(r) => (
                (None, None),
                Text::key("launcher.run_cup_refused").part("why", Text::key(r.key()).arg("m", r.minutes())),
            ),
        };
        conf::set_cup_times(&ctrl_conf, times)?;
        conf::set_cup_schedule(&self.game.join(r"local\schedule_option.conf"), friendlies)?;
        self.tell("  ", &line);
        // for live checks: when the control should open the Universal Championship, the cup's winner in it (a refused
        // time leaves the game's times)
        let plan_line = match plan.unwrap_or(CupPlan::GameTimes).championship() {
            Some(t) => format!(
                "championship plan: from about {}, the slot after the cup's final, {} matches",
                forza_ui::lobby::clock_text(t),
                forza_ui::lobby::CHAMPIONSHIP_SLOTS
            ),
            None => "championship plan: none, no cup".to_owned(),
        };
        crate::logs::append(&self.logs().join("run_launcher.txt"), &plan_line);
        if let Mode::Vs(_) = self.mode {
            let panel_file = self.kit.data().join("panel.txt");
            let panel = choices::read_ok(&panel_file).map_err(|e| {
                Failed(
                    Text::key("launcher.msg_read_failed")
                        .arg("path", panel_file.display())
                        .arg("error", e),
                )
            })?;
            let club = choices::setting(&self.kit.settings_file(), "club_server").unwrap_or_default();
            let t = rules::vs_target(&panel, &club).map_err(Failed)?;
            self.vs_target = Some(t.clone());
            // Note: only "localhost" is proven as GAME_SVR_IP (a literal 127.0.0.1 means the publisher's address);
            // a numeric IP or a hostname needs the live test.
            let found = reach(&t.host, t.port).ok_or_else(|| {
                Failed(
                    Text::key("launcher.run_gamesvr_no_answer")
                        .arg("host", &t.host)
                        .arg("port", t.port),
                )
            })?;
            // the server's own game server (a room's too) keeps no club of this PC's card: it comes along, as to an
            // arcade, or the game does not start. A club server keeps the club itself.
            if t.room.is_some() || club.trim().is_empty() {
                let server = choices::panel_value(&panel, "server").unwrap_or_default();
                self.club = None;
                self.visit.club = None;
                self.bring_club(choices::resolve(&server, self.p.seat_port), 1, &server)
                    .map_err(|why| Failed(Text::key("launcher.run_vs_club_refused").part("why", why)))?;
            }
            conf::set_game_server(&ctrl_conf, &t.host, t.port, true)?;
            self.tell(
                &format!("  1/{steps} "),
                &Text::key("launcher.run_gamesvr_at")
                    .arg("host", &t.host)
                    .arg("port", t.port),
            );
            // the lobby's VS room: its ticket goes to the game server in vslink's hello (crates/server/README.md "VS
            // rooms"); single use, so the line goes now. The window's heartbeat keeps the room while the run lasts.
            let ticket = t.room.map(|(_, ticket)| ticket);
            if ticket.is_some() {
                let _ = choices::write(&panel_file, &choices::set_panel_value(&panel, "ticket", None));
            }
            self.start_vslink(found, ticket);
            return Ok(());
        }
        // this PC: back to the stand-in's address, VS entries off, after an online run
        conf::set_game_server(&ctrl_conf, "localhost", port, false)?;
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| Failed(gamesvr_bind_error(port, &e)))?;
        let log = file_log(self.logs().join("gamesvr_log.txt"));
        // the run's game folder (also recorded as "game" by the 2017-18 setup): the stand-in pre-registers its 2D-code cards,
        // and data\settings.json "skip_tutorial": true makes it give a new Aime id a club from the game's cpu_50.dat
        // (no window sets it now: seed-club.sh and the fixture use it, players play the tutorial)
        let skip = self.kit.settings()["skip_tutorial"].as_bool().unwrap_or(false);
        let game = Some(self.game.clone());
        // guests' clubs a crashed arcade left here (online::club): their players keep their home copies
        let store = self.kit.save().join("gamesvr");
        fs::create_dir_all(&store)?;
        match gamesvr::Store::open(&store.join("gamesvr.db")).and_then(|mut s| s.purge_guests()) {
            Ok(0) => {}
            Ok(n) => self.tell("  ", &Text::key("launcher.run_guests_dropped").arg("n", n)),
            Err(e) => self.tell("  ", &Text::key("launcher.run_guests_unchecked").arg("error", e)),
        }
        self.gamesvr = Some(
            gamesvr::spawn_with(listener, self.kit.save().join("gamesvr"), log, game, skip)
                .map_err(|e| Failed(Text::key("launcher.run_gamesvr_failed").arg("error", e)))?,
        );
        self.tell(
            &format!("  1/{steps} "),
            &Text::key("launcher.run_gamesvr_local").arg("port", port),
        );
        Ok(())
    }

    /// A club still marked out in an arcade (the last trip ended before it came home): the home copy stays, the arcade's
    /// last copy goes to backups, and the player is told once.
    fn club_leftover(&self, dir: &Path) {
        match club::leftover(dir) {
            Ok(Some(kept)) => {
                self.tell("  ", &Text::key("launcher.club_kept").arg("path", kept.display()));
            }
            Ok(None) => {}
            Err(e) => self.tell("  ", &Text::key("launcher.run_marker_unread").arg("error", e)),
        }
    }

    /// 2017-18 arcade guest, or online on a server's own arcade: the home club comes along to the desk at `desk`
    /// (online::club), under the card's home Aime id (an older card's integer id) here and its hash there. `Err`: why
    /// it stayed home.
    fn bring_club(&mut self, desk: Option<SocketAddr>, seat: u8, arcade: &str) -> Result<(), Text> {
        let save = self.kit.save();
        let dir = save.join("gamesvr");
        self.club_leftover(&dir);
        // a first run: the card the reader will read, made now so its club (none yet) comes along too
        aime::ensure_card(&save.join("seat1_aime.txt"));
        let code = fs::read_to_string(save.join("seat1_aime.txt"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| s.len() == 20 && s.bytes().all(|b| b.is_ascii_digit()));
        let home = code
            .as_ref()
            .map(|c| aimedb::Db::open(save.join("aimedb.json")).map(|db| db.home_id(c)));
        let (Some(code), Some(Ok(home)), Some(desk)) = (code, home, desk) else {
            return Err(Text::key("launcher.run_no_card"));
        };
        let guest = club::Guest {
            desk,
            seat,
            arcade_aime: aimedb::hash_id(&code) as u32,
            code,
            home_aime: home as u32,
            dir,
            arcade: arcade.to_owned(),
            poll: club::POLL,
        };
        match guest.bring(file_log(self.logs().join("club_trip.txt"))) {
            Ok(trip) => {
                self.visit.club = Some(trip.progress());
                self.club = Some(trip);
                self.tell("  ", &Text::key("launcher.club_in"));
                Ok(())
            }
            Err(why) => Err(Text::Raw(why)),
        }
    }

    /// The direct VS link to the online game server, with the lobby room's ticket if one is held;
    /// data\\settings.json "vs_link": false turns it off.  A failure is said and the game goes on: it gives a CPU team
    /// when no link is up.
    fn start_vslink(&mut self, gmsvr: SocketAddr, ticket: Option<String>) {
        if self.kit.settings()["vs_link"].as_bool() == Some(false) {
            self.tell(
                "  ",
                &Text::key(if ticket.is_some() {
                    "launcher.run_vs_off_room"
                } else {
                    "launcher.run_vs_off"
                }),
            );
            return;
        }
        let room = ticket.is_some();
        let mut cfg = vslink::Config::public(
            gmsvr,
            self.kit.data().join("vslink.key"),
            file_log(self.logs().join("vslink_log.txt")),
        );
        cfg.ticket = ticket;
        // gmsvr pairs only PCs whose match engine math is the same; a room partner with another is said once
        if self.edition == Edition::E1718 {
            cfg.dmath = Some(seat::dmath_token(&self.game, &self.seat));
            let tx = self.tx.clone();
            cfg.on_dmath_differs = Some(Arc::new(move |mine| {
                let _ = tx.send(Event::Said(
                    Text::key("launcher.run_vs_dmath_differs").arg("mine", mine),
                ));
            }));
        }
        // WCCF_VS_RELAY, else settings "vs_relay" (URL); settings "vs_relay_ca" (PEM file) is the relay's only trusted root
        let settings = self.kit.settings();
        let saved = |k: &str| settings[k].as_str().map(str::to_owned);
        if let Some(url) = choices::env_or_var("WCCF_VS_RELAY", saved("vs_relay")) {
            let ca = saved("vs_relay_ca").filter(|c| !c.trim().is_empty());
            if let Err(e) = cfg.set_relay(&url, ca.as_deref().map(Path::new)) {
                self.tell("  ", &Text::key("launcher.run_vs_failed").arg("error", e));
                return;
            }
        }
        match vslink::spawn(cfg) {
            Ok(link) => {
                self.tell(
                    "  ",
                    &Text::key(if room {
                        "launcher.run_vs_on_room"
                    } else {
                        "launcher.run_vs_on"
                    }),
                );
                self.vslink = Some(link.handle);
            }
            Err(e) => self.tell("  ", &Text::key("launcher.run_vs_failed").arg("error", e)),
        }
    }

    /// 2017-18 arcade over iroh (vslink::arcade). Host: beside `forza-server all`, its id said and kept in
    /// data\shop_id.txt. Join: this PC's loopback control, desk and relay ports lead to that host. The relay as for VS
    /// (WCCF_VS_RELAY, settings "vs_relay" and "vs_relay_ca"); `VSLINK_DELAY_MS` (tests only) adds that much to the
    /// round trip.
    fn start_arcade_tunnel(&mut self, role: vslink::arcade::Role) -> Result<vslink::arcade::Ports, Failed> {
        let p = self.p;
        let host = role == vslink::arcade::Role::Host;
        let usual = vslink::arcade::Ports {
            control: p.control_port,
            desk: p.seat_port,
            relay: p.relay_port,
        };
        // a server on this PC holds the usual ports: a join's tunnel takes free ones, the seat confs and env follow them
        let ports = rules::tunnel_ports(usual, host, procs::listening_ports, |p| procs::udp_ports().contains(&p));
        let key = self
            .kit
            .data()
            // file names kept from before the arcade rename: players' installs already hold these keys and logs
            .join(if host { "shop_host.key" } else { "shop_join.key" });
        let log = file_log(self.logs().join("shop_tunnel.txt"));
        let mut cfg = vslink::arcade::ArcadeConfig::public(role, ports, key, log);
        let settings = self.kit.settings();
        let saved = |k: &str| settings[k].as_str().map(str::to_owned);
        if let Some(url) = choices::env_or_var("WCCF_VS_RELAY", saved("vs_relay")) {
            let ca = saved("vs_relay_ca").filter(|c| !c.trim().is_empty());
            cfg.set_relay(&url, ca.as_deref().map(Path::new))
                .map_err(|e| Failed(Text::Raw(e)))?;
        }
        // a host's room first: its code is the only way in, and the tunnel admits only the guests the room names
        let room = host.then(|| self.open_arcade_room(&cfg.key, cfg.log.clone()));
        if let Some(Ok(r)) = &room {
            cfg.admit = Some(r.admit());
        }
        let knob = rules::delay_knob(env::var("VSLINK_DELAY_MS").ok().as_deref());
        cfg.delay = knob / 2;
        if !knob.is_zero() {
            self.tell(
                "     ",
                &Text::key("launcher.run_test_delay").arg("ms", knob.as_millis()),
            );
        }
        let link =
            vslink::arcade::spawn(cfg).map_err(|e| Failed(Text::key("launcher.run_tunnel_failed").arg("error", e)))?;
        if host {
            let id = vslink::arcade::id_text(&link.id);
            // file name kept from before the arcade rename
            let _ = choices::write(&self.kit.data().join("shop_id.txt"), &format!("{id}\n"));
            self.arcade = Some(match &room {
                Some(Ok(r)) => format!("{id} code {}", r.code),
                _ => id.clone(),
            });
            match &room {
                Some(Ok(r)) => self.tell(
                    "     ",
                    &Text::key("launcher.run_arcade_room")
                        .arg("code", &r.code)
                        .arg("server", &r.server)
                        .arg("id", &id),
                ),
                Some(Err(why)) => self.tell(
                    "     ",
                    &Text::key("launcher.run_arcade_no_room").arg("why", why).arg("id", &id),
                ),
                None => {}
            }
            self.tell("     ", &Text::key("launcher.run_arcade_id_file"));
            self.arcade_room = room;
            self.arcade_guests = Some(link.guests());
        } else if link.ports != ports {
            self.tell(
                "       ",
                &Text::key("launcher.run_tunnel_ports")
                    .arg("control", link.ports.control)
                    .arg("desk", link.ports.desk)
                    .arg("relay", link.ports.relay),
            );
        } else {
            self.tell("       ", &Text::key("launcher.run_tunnel_up"));
        }
        if !host {
            self.visit.host_closed = Some(link.host_closed());
        }
        self.vslink = Some(link.handle);
        Ok(link.ports)
    }

    /// The lobby room for the host's tunnel (`key` its endpoint key), on the club server's desk ([`crate::lobby::arcade_lobby`]),
    /// where the window's guests type its code; listed in ARCADES when data\settings.json `list_shop` is true, as
    /// `shop_name` (empty: the desk's name).
    fn open_arcade_room(&self, key: &Path, log: Log) -> Result<crate::lobby::HostRoom, String> {
        let id = vslink::arcade::own_id(key).map_err(|e| format!("{}: {e}", key.display()))?;
        let club = choices::setting(&self.kit.settings_file(), "club_server").unwrap_or_default();
        let desk = crate::lobby::arcade_lobby(&club);
        // settings keys kept from before the arcade rename: players' settings.json already holds them
        let public = self.kit.settings()["list_shop"].as_bool() == Some(true);
        let name = choices::setting(&self.kit.settings_file(), "shop_name").unwrap_or_default();
        let secret = crate::lobby::room_secret(key);
        crate::lobby::HostRoom::open(
            &desk,
            &vslink::arcade::id_text(&id),
            secret.as_deref(),
            public,
            &name,
            log,
            &|t| self.tell("     ", t),
        )
    }

    /// A 2017-18 server's arcade room: its code, the lobby server holding it and the flag set when that lobby loses it;
    /// or why it has none (the window's HOST ARCADE shows one or the other).
    pub fn arcade_code(&self) -> Result<(String, String, Arc<AtomicBool>), String> {
        match &self.arcade_room {
            Some(Ok(r)) => Ok((r.code.clone(), r.server.clone(), r.lost())),
            Some(Err(why)) => Err(why.clone()),
            None => Err(Text::key("launcher.run_no_tunnel").get(self.ui.lang())),
        }
    }

    /// 2017-18 server: its arcade tunnel's live guest count (HOST ARCADE closes an empty arcade on its own).
    pub fn arcade_guests(&self) -> Option<Arc<AtomicUsize>> {
        self.arcade_guests.clone()
    }

    /// Online: this PC's seat from the server's seat desk, held while the run lasts; None asked = the next free seat.
    fn take_seat(&mut self, ip: &str, want: Option<u8>) -> Result<(u8, bool), Failed> {
        let desk = self.remote.as_ref().map_or(self.p.seat_port, |r| r.1);
        let at = addr(ip, desk).ok_or_else(|| Failed(Text::key("launcher.run_not_address").arg("ip", ip)))?;
        let log = file_log(self.logs().join("run_seat_lease.txt"));
        // a seat the lobby reserved: data\panel.txt "ticket=" (single use, so the line goes before the claim)
        let panel = self.kit.data().join("panel.txt");
        let text = choices::read(&panel);
        let ticket = choices::panel_value(&text, "ticket").filter(|t| !t.is_empty());
        if ticket.is_some() {
            let _ = choices::write(&panel, &choices::set_panel_value(&text, "ticket", None));
        }
        // every seat of an arcade must run the same match engine math (the desk refuses another)
        let dmath = (self.edition == Edition::E1718).then(|| seat::dmath_token(&self.game, &self.seat));
        match seat::claim_with(
            at,
            want,
            ticket.as_deref(),
            dmath.as_deref(),
            LIFE,
            seat::Timing::default(),
            log,
        ) {
            Ok(lease) => {
                let got = (lease.seat, lease.projector);
                self.lease = Some(lease);
                Ok(got)
            }
            Err(e) => match rules::desk_fallback(self.edition, &e, want) {
                Some((s, proj)) => {
                    let key = if proj {
                        "launcher.run_no_desk_projector"
                    } else {
                        "launcher.run_no_desk"
                    };
                    self.tell("  ", &Text::key(key).arg("ip", ip).arg("seat", s));
                    Ok((s, proj))
                }
                None => Err(Failed(rules::seat_refusal(&e, want, ip))),
            },
        }
    }

    // ---- reports ----

    fn read_log(&self, name: &str) -> String {
        // unreadable counts as no close: the safe side keeps the rest running
        fs::read(self.logs().join(name))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    }

    /// SAVE LOGS (the SETTINGS panel, data\savelogs.request): the ZIP written to [`Params::logs_dir`] on a thread;
    /// data\savelogs.result tells the panel "ok PATH" or "fail WHY", and the line comes back as [`Event::Said`].
    fn save_logs(&self) {
        let (dir, kit, game) = (self.p.logs_dir.clone(), self.kit.dir().to_path_buf(), self.game.clone());
        let tx = self.tx.clone();
        thread::spawn(move || {
            let line = match crate::report::save_zip(&dir, &kit, Some(&game), &Scrub::from_env(), None) {
                Ok(path) => format!("ok {}", path.display()),
                Err(e) => format!("fail {e}"),
            };
            let _ = choices::write(&kit.join("data").join("savelogs.result"), &format!("{line}\n"));
            let _ = tx.send(Event::Said(match line.strip_prefix("ok ") {
                Some(path) => Text::key("launcher.run_logs_saved").arg("path", path),
                None => Text::key("launcher.run_logs_not_saved").arg("error", &line[5..]),
            }));
        });
    }

    /// 2017-18 with the control here: the bracket's line in the log at each change of today's Universal Championship
    /// (game\log\<date>_uc.bin), a line when a player's club leaves it, and once a day, when it has a player's club,
    /// the notice with about when it plays on the projector.
    fn watch_championship(&mut self) {
        if self.edition != Edition::E1718 || !self.alive(Role::Server) {
            return;
        }
        let date = championship::date(now_local());
        let f = championship::file(&self.game, date);
        let Ok(at) = fs::metadata(&f).and_then(|m| m.modified()) else {
            return;
        };
        if self.uc_seen == Some(at) {
            return;
        }
        let b = fs::read(&f).unwrap_or_default();
        let log = self.logs().join("run_launcher.txt");
        if championship::whole(&b).is_err() {
            // read while the control writes it: not judged, read again at the next look (one line per change)
            if self.uc_partial != Some(at) {
                self.uc_partial = Some(at);
                crate::logs::append(&log, &championship::line_of(date, &b));
            }
            return;
        }
        self.uc_seen = Some(at);
        let teams = championship::teams(&b);
        // every change of the day file gets its line (a few a day): a player's report then shows when the club went
        // in, its fix/hasDvs flags, and a swap for a CPU team
        crate::logs::append(&log, &championship::line_of(date, &b));
        let mine = championship::players(&teams);
        match &mut self.uc_player {
            Some((d, club, told)) if *d == date && mine.is_empty() && !*told => {
                *told = true;
                crate::logs::append(
                    &log,
                    &format!(
                        "championship: {date} {club} is no longer in the bracket - the control swapped it for a CPU team (fix or hasDvs 0, control 0x47DCF0 \"change cpu!\")"
                    ),
                );
            }
            _ if !mine.is_empty() => self.uc_player = Some((date, mine.clone(), false)),
            _ => {}
        }
        // shortcut: any player's club counts as "yours"; an arcade host with guests may see a guest's win, tell the
        // clubs apart (block +0 = coach id) if hosts report it
        if mine.is_empty() || self.uc_told == Some(date) {
            return;
        }
        self.uc_told = Some(date);
        // the winner alone is written as the cup's final ends, and the championship opens at the next slot (2-3 min
        // later); all four teams are written as it opens
        let opens = at + Duration::from_secs(if teams.len() < 4 { 180 } else { 0 });
        // the notice sends the player to the big screen: tell it only when the projector runs with its window shown
        if self.projector && choices::projector_shown(&self.kit.settings()) {
            self.tell("", &championship_notice(opens));
        }
    }

    // ---- the supervisor ----

    /// Requests from outside (other Launcher.exe processes, the panel's files) as events, every half second.
    fn poll_requests(&self) {
        let (tx, kit, kind) = (self.tx.clone(), self.kit.dir().to_path_buf(), Kind::of(&self.mode));
        let data = kit.join("data");
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(500));
                let mut events = Vec::new();
                if kind == Kind::Run
                    && let Some(text) = take(&data.join("restart.request"))
                {
                    events.push(Event::Restart(rules::restart_forced(&text)));
                }
                if let Some(word) = take(&data.join(format!("{}.request", kind.name()))) {
                    events.push(Event::Request(word.trim().to_owned()));
                }
                if kind == Kind::Run && take(&data.join("savelogs.request")).is_some() {
                    events.push(Event::SaveLogs);
                }
                if kind == Kind::Run && take(&data.join("report.request")).is_some() {
                    events.push(Event::Report);
                }
                if events.into_iter().any(|e| tx.send(e).is_err()) {
                    return; // the run is over
                }
            }
        });
    }

    /// Keeps the run until it is over: a closed game window stops the rest (the card rule permitting), a crash stops
    /// nothing while another game window runs, RESTART NOW restarts, requests are answered.
    pub fn supervise(mut self) -> End {
        self.ui.running(true);
        self.poll_requests();
        loop {
            let ev = match self.rx.recv_timeout(Duration::from_millis(500)) {
                Ok(ev) => ev,
                Err(RecvTimeoutError::Timeout) => {
                    self.watch_championship();
                    if let Some(end) = self.check_threads() {
                        self.ui.running(false);
                        return end;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return End::Over(OK),
            };
            let end = match ev {
                Event::Ended(id) => self.ended(id),
                Event::Restart(force) => self.restart(force),
                Event::Request(word) => self.answer(&word),
                Event::SaveLogs => {
                    self.save_logs();
                    None
                }
                Event::Said(line) => {
                    self.tell("", &line);
                    None
                }
                Event::Report => {
                    self.ui.report();
                    None
                }
            };
            if let Some(end) = end {
                self.ui.running(false);
                return end;
            }
        }
    }

    /// An arcade guest whose host closed the arcade (the tunnel heard it; without one, the club's last copy-back found
    /// the host's server gone): the end's notice, with the club home or maybe missing its last saves (then the marker
    /// stays, so the next start says so too). After `shutdown`: dropping the trip joins it, its last copy-back done.
    fn host_closed_notice(&self) -> Option<Text> {
        if matches!(self.mode, Mode::Vs(_)) {
            return None; // no host: the end says whether the club came home (finish)
        }
        let progress = self
            .visit
            .club
            .as_ref()
            .map(|p| *p.lock().unwrap_or_else(PoisonError::into_inner));
        let closed = match &self.visit.host_closed {
            Some(flag) => flag.load(Ordering::Relaxed),
            None => progress.is_some_and(|p| p.gone),
        };
        closed.then(|| rules::host_closed_notice(progress.map(|p| p.home)))
    }

    /// Stand-in and helper threads that ended by themselves (a panic, an error) are said once, in the run log; the run
    /// stops when the game cannot go on without one (the card reader, the I/O board, the scene service, the network
    /// stand-ins; 2017-18: the Aime reader, AiMeDB, the game server).  The key driver and the helpers are said and the game goes on.
    fn check_threads(&mut self) -> Option<End> {
        let mut found: Vec<(Text, bool, String)> = Vec::new();
        let mut look = |name: Text, essential: bool, h: Option<&mut Handle>| {
            if let Some(why) = h.and_then(Handle::died) {
                found.push((name, essential, why));
            }
        };
        for (h, s) in self.cabinet.net.iter_mut().zip(network::SERVICES) {
            look(Text::key("launcher.run_part_network").arg("name", s.1), true, Some(h));
        }
        let reader = match self.edition {
            Edition::E1011 => "launcher.run_part_reader",
            Edition::E1718 => "launcher.run_part_aime",
        };
        look(Text::key(reader), true, self.cabinet.reader.as_mut());
        look(
            Text::key("launcher.run_part_aimedb"),
            true,
            self.cabinet.aimedb.as_mut(),
        );
        look(Text::key("launcher.run_part_gamesvr"), true, self.gamesvr.as_mut());
        look(Text::key("launcher.run_part_vslink"), false, self.vslink.as_mut());
        look(Text::key("launcher.run_part_board"), true, self.cabinet.board.as_mut());
        look(Text::key("launcher.run_part_keys"), false, self.cabinet.keys.as_mut());
        look(Text::key("launcher.run_part_scenes"), true, self.scenes.as_mut());
        for (name, essential, h) in &mut self.cabinet.extra {
            look(Text::Raw(name.clone()), *essential, Some(h));
        }
        if let Some(h) = &mut self.helper {
            found.extend(
                h.died()
                    .into_iter()
                    .map(|(n, why)| (Text::key("launcher.run_part_helper").arg("name", n), false, why)),
            );
        }
        let mut stop = None;
        for (name, essential, why) in found {
            let key = if essential {
                "launcher.run_part_stopped_fatal"
            } else {
                "launcher.run_part_stopped"
            };
            self.tell("", &Text::key(key).part("name", name.clone()).arg("why", why));
            if essential {
                stop.get_or_insert(name);
            }
        }
        let name = stop?;
        Some(
            match self.finish(false, Some(Text::key("launcher.run_stopped_part").part("name", name))) {
                End::Over(_) | End::Again => End::Over(FAILED),
            },
        )
    }

    /// The card rule now: the refusal's words, or None to go on.
    fn refusal(&self, force: bool) -> Option<Text> {
        // a server has no seat of its own, so the card rule never holds it: an arcade server's matches are its tunnel's
        // guests' (still connected even when the lobby lost the room) and this PC's seat beside it
        if self.mode.is_server()
            && let Some(g) = &self.arcade_guests
        {
            return rules::arcade_rule(force, is_on(self.kit, Kind::Run), g.load(Ordering::Relaxed));
        }
        let (slot, new) = self.card_files();
        let (session, _) = card::club_session(&slot, &new, self.run_started());
        let card_in = self.cabinet.board.is_some() && self.p.shared.card_in();
        rules::card_rule(self.alive(Role::Seat), force, Some(card_in), session)
    }

    fn ended(&mut self, id: u32) -> Option<End> {
        let Some(i) = self.games.iter().position(|g| g.id == id) else {
            return None; // a program the run stopped itself (a cabinet restart)
        };
        let mut g = self.games.remove(i);
        if let Some(t) = g.thread.take() {
            let _ = t.join();
        }
        self.gone.push(g.role);
        if g.role == Role::Server {
            self.tell("", &Text::key("launcher.run_server_ended"));
            if self.edition == Edition::E1718 {
                crate::logs::append(&self.logs().join("run_launcher.txt"), &championship_line(&self.game));
            }
            if self.mode.is_server() {
                return Some(self.finish(false, Some(Text::key("launcher.run_server_has_ended"))));
            }
            return None;
        }
        let gone: Vec<&str> = self
            .gone
            .iter()
            .filter(|r| **r != Role::Server)
            .map(|r| r.log())
            .collect();
        let others = self.games.iter().any(|g| g.role != Role::Server && g.alive());
        let home = matches!(self.mode, Mode::Local | Mode::Vs(_));
        let seat_gone = g.role == Role::Seat && !self.alive(Role::Seat);
        let left = rules::windows_left(others, home, seat_gone);
        match rules::after_end(&gone, left, |n| self.read_log(n)) {
            Some(crashed) => {
                self.tell("", &rules::went_by_itself(&crashed, |n| self.read_log(n)));
                None
            }
            None => {
                self.tell(
                    "",
                    &Text::key(if left {
                        "launcher.run_closed_rest"
                    } else if others {
                        "launcher.run_projector_left"
                    } else {
                        "launcher.run_no_window_left"
                    }),
                );
                if let Some(why) = self.refusal(false) {
                    // the projector closed during a match: seat 1 plays on, watched
                    self.tell("", &why);
                    self.tell("", &Text::key("launcher.run_not_stopped"));
                    return None;
                }
                Some(self.finish(true, None))
            }
        }
    }

    /// Ends every process of the run, in order: the helper threads and the Python kids, the game programs through their debuggers, then
    /// `Job::stop` for whatever is left (its windows closed, then ended after the deadline); then the stand-ins, the
    /// key driver first.
    fn shutdown(&mut self) {
        self.sampler = None;
        self.helper = None; // joins its threads (a view being made is given up)
        for (_, c) in &mut self.kids {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.kids.clear();
        for g in &self.games {
            g.stop.store(true, Ordering::Relaxed);
        }
        for g in &mut self.games {
            if let Some(t) = g.thread.take() {
                let _ = t.join();
            }
        }
        self.games.clear();
        if self.job.active() > 0 {
            let s = self.job.stop(STOP_DEADLINE);
            if s.terminated > 0 {
                self.tell("  ", &Text::key("launcher.run_force_closed").arg("n", s.terminated));
            }
        }
        self.cabinet = Cabinet::default();
        self.scenes = None;
        self.gamesvr = None;
        // the club's last copy home, while the game is closed and the arcade tunnel still up
        self.club = None;
        self.vslink = None;
        self.lease = None;
    }

    /// The end of the run: everything stopped, the table kept with its club, data\running.json removed, and - when a
    /// game window ended it (`ended`) - an online run that went wrong reported.
    fn finish(&mut self, ended: bool, words: Option<Text>) -> End {
        let gone: Vec<&str> = self
            .gone
            .iter()
            .filter(|r| **r != Role::Server)
            .map(|r| r.log())
            .collect();
        let why = if ended {
            rules::went_wrong(&gone, |n| self.read_log(n))
        } else {
            Vec::new()
        };
        let control_here = self.alive(Role::Server) || self.gone.contains(&Role::Server);
        self.shutdown();
        let notice = self.host_closed_notice();
        let why = if notice.is_some() {
            rules::without_link_lost(why)
        } else {
            why
        };
        let left = self.job.active();
        self.card_steps(&["keep"]); // the cards on the table kept with their club: data\save holds the latest
        self.job.stop(STOP_DEADLINE);
        let _ = fs::remove_file(self.running_file());
        if left > 0 {
            self.tell("", &Text::key("launcher.run_still_running").arg("n", left));
            return End::Over(FAILED);
        }
        if let Some(n) = &notice {
            self.tell("", n);
        }
        if self.edition == Edition::E1718 && control_here {
            let date = championship::date(now_local());
            let seen = self
                .uc_player
                .as_ref()
                .filter(|(d, ..)| *d == date)
                .map(|(_, c, _)| c.as_str());
            crate::logs::append(
                &self.logs().join("run_launcher.txt"),
                &championship::summary(&self.game, date, seen),
            );
        }
        let club_home = self
            .visit
            .club
            .as_ref()
            .map(|p| p.lock().unwrap_or_else(PoisonError::into_inner).home);
        if matches!(self.mode, Mode::Vs(_)) && club_home == Some(false) {
            // the marker stays, so the next start says so too
            self.tell("", &Text::key("launcher.run_vs_club_maybe_old"));
        }
        if let Some(words) = words {
            self.tell("", &words);
        } else if !why.is_empty() {
            self.tell("", &Text::key("launcher.run_ended_problem").arg("why", why.join("; ")));
        } else if notice.is_none() {
            self.tell("", &Text::key("launcher.run_closed_by_player"));
        }
        End::Over(if why.is_empty() { OK } else { WENT_WRONG })
    }

    /// RESTART NOW: the cabinet alone when a plain start would play the same way, else a stop (the card rule) and a
    /// plain start.
    fn restart(&mut self, force: bool) -> Option<End> {
        self.tell("", &Text::key("launcher.run_restart_requested"));
        let panel = choices::read(&self.kit.data().join("panel.txt"));
        let club = choices::setting(&self.kit.settings_file(), "club_server").unwrap_or_default();
        // a seat cannot reconnect to a server this run started that has since died: start everything again
        let server_died = self.gone.contains(&Role::Server);
        let fits = !server_died
            && rules::cabinet_restart_fits(
                &self.mode,
                self.alive(Role::Seat),
                &panel,
                self.kit.game_language(),
                (&club, self.vs_target.as_ref()),
            );
        if let Some(why) = self.refusal(force) {
            self.tell("", &why);
            self.tell("", &Text::key("launcher.run_not_restarted"));
            return None;
        }
        if !fits {
            self.tell(
                "",
                &Text::key("launcher.run_restarting").arg("edition", self.edition.name()),
            );
            self.shutdown();
            self.card_steps(&["keep"]);
            let _ = fs::remove_file(self.running_file());
            thread::sleep(Duration::from_secs(2)); // Windows frees the ports and pipes
            return Some(End::Again);
        }
        // the projector, the scene service, the seat lease and the helper threads are never touched, so the projector
        // stays in step with the server
        self.tell("", &Text::key("launcher.run_restarting_seat").arg("n", self.seat_no));
        if let Some(i) = self.games.iter().position(|g| g.role == Role::Seat) {
            let mut g = self.games.remove(i);
            g.stop.store(true, Ordering::Relaxed);
            if let Some(t) = g.thread.take() {
                let _ = t.join();
            }
        }
        self.cabinet = Cabinet::default();
        self.card_steps(&["start", "board"]);
        thread::sleep(Duration::from_secs(1)); // Windows frees the cabinet's ports and pipes
        if let Err(f) = self.start_cabinet() {
            self.failed(f);
            return Some(match self.finish(false, Some(Text::key("launcher.run_stopped"))) {
                End::Over(_) | End::Again => End::Over(FAILED),
            });
        }
        self.write_running();
        self.say("");
        self.tell("", &Text::key("launcher.run_seat_back").arg("n", self.seat_no));
        None
    }

    /// A request from another Launcher.exe: answered in data\<kind>.answer ("CODE\nwords").
    fn answer(&mut self, word: &str) -> Option<End> {
        let file = answer_file(self.kit, Kind::of(&self.mode));
        let put = |code: i32, text: &str| {
            let _ = choices::write(&file, &format!("{code}\n{text}\n"));
        };
        match word {
            "status" => put(OK, &self.status()),
            "check" => {
                let (slot, new) = self.card_files();
                let (session, _) = card::club_session(&slot, &new, self.run_started());
                let n = self.job.active();
                put(
                    OK,
                    &format!(
                        "Check only, nothing stopped. Stop would end {n} processes (club card: {}, session: {})",
                        if self.p.shared.card_in() && self.cabinet.board.is_some() {
                            "in"
                        } else {
                            "out"
                        },
                        session.words()
                    ),
                );
            }
            "show" => {
                let ctl: Vec<procs::Proc> = self
                    .job_procs()
                    .into_iter()
                    .filter(|q| q.name.eq_ignore_ascii_case(self.p.control()))
                    .collect();
                if ctl.is_empty() {
                    put(FAILED, "No server is running on this PC.");
                } else {
                    let mut out: Vec<String> = ctl
                        .iter()
                        .map(|q| {
                            let titles = procs::show_windows(q.pid);
                            if titles.is_empty() {
                                format!("server pid {}: no hidden window to show", q.pid)
                            } else {
                                let t: Vec<String> = titles.iter().map(|t| format!("\"{t}\"")).collect();
                                format!("server pid {}: shown: {}", q.pid, t.join(", "))
                            }
                        })
                        .collect();
                    out.push("They are hidden again at the next start.".into());
                    put(OK, &out.join("\n"));
                }
            }
            "stop" | "force" => {
                // the answer in the player's language: the asking window or console shows it (run_launcher.txt here
                // keeps the English)
                let lang = self.ui.lang();
                if let Some(why) = self.refusal(word == "force") {
                    let not = Text::key("launcher.run_not_stopped");
                    self.tell("", &why);
                    self.tell("", &not);
                    put(REFUSED, &format!("{}\n{}", why.get(lang), not.get(lang)));
                    return None;
                }
                self.tell("", &Text::key("launcher.run_stopping"));
                let stopped = Text::key("launcher.run_stopped");
                let end = self.finish(false, Some(stopped.clone()));
                put(
                    if matches!(end, End::Over(OK)) { OK } else { FAILED },
                    &stopped.get(lang),
                );
                return Some(end);
            }
            _ => put(FAILED, &format!("unknown request \"{word}\"")),
        }
        None
    }

    /// What runs (play.py's status).
    fn status(&self) -> String {
        let mut out = Vec::new();
        match &self.mode {
            Mode::Remote(ip, _) => out.push(format!("playing on the server at {ip} as seat {}", self.seat_no)),
            Mode::Server | Mode::LanServer => {
                out.push(format!("this PC is the server (TCP port {})", self.p.control_port))
            }
            Mode::Local => out.push("playing on this PC".into()),
            Mode::Vs(_) => out.push("playing on this PC, VS through the server".into()),
        }
        let threads = [
            ("scene service", self.scenes.is_some()),
            ("game server stand-in", self.gamesvr.is_some()),
            ("direct VS link", self.vslink.is_some()),
            ("keychip / network stand-in", !self.cabinet.net.is_empty()),
            (
                if self.edition == Edition::E1718 {
                    "Aime reader stand-in"
                } else {
                    "card reader stand-in"
                },
                self.cabinet.reader.is_some(),
            ),
            ("AiMeDB stand-in", self.cabinet.aimedb.is_some()),
            ("I/O board stand-in", self.cabinet.board.is_some()),
            ("key driver", self.cabinet.keys.is_some()),
            ("seat lease", self.lease.is_some()),
        ];
        out.extend(
            threads
                .iter()
                .filter(|(_, on)| *on)
                .map(|(n, _)| format!("  forza {n:<28} (in Launcher.exe)")),
        );
        for q in self.job_procs() {
            let kid = self.kids.iter().find(|(_, c)| c.id() == q.pid).map(|(r, _)| *r);
            let folder = q
                .path
                .parent()
                .and_then(Path::file_name)
                .map(|f| f.to_string_lossy().into_owned());
            out.push(match kid {
                Some(role) => format!("  forza {role:<28} pid {}", q.pid),
                None => format!("  game  {:<28} pid {}  ({})", q.name, q.pid, folder.unwrap_or_default()),
            });
        }
        if self.cabinet.on() {
            out.push(format!(
                "club card: {}",
                if self.p.shared.card_in() {
                    "IN the reader"
                } else {
                    "out"
                }
            ));
        }
        let (slot, new) = self.card_files();
        let (session, bad) = card::club_session(&slot, &new, self.run_started());
        out.push(format!(
            "card session: {}{}",
            session.words(),
            bad.map_or_else(String::new, |b| format!(", bad endings {b}"))
        ));
        out.join("\n")
    }
}

impl Drop for Run<'_> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::due;
    use std::fs;
    use std::path::Path;

    /// 2017-18's helper mutes the projector before the cabinet wait, which lasts until seat 1's start warning
    /// (2026-11-11: ~20 s of projector sound).
    #[test]
    fn the_projector_is_muted_before_the_cabinet_wait() {
        let code = include_str!("mod.rs").split("#[cfg(test)]").next().unwrap_or_default();
        let at = |s: &str| code.find(s).unwrap_or_else(|| panic!("{s} not found"));
        assert!(at("if helper_first {") < at("self.start_cabinet()?;"));
        assert!(at("self.start_helper(projector);") < at("self.start_cabinet()?;"));
        assert!(code.contains("let helper_first = self.edition == Edition::E1718;"));
    }

    /// LAN search answers carry the arcade's name, as its ARCADES row does (2026-11-06: they showed the PC's).
    #[test]
    fn a_lan_arcade_answers_with_its_name() {
        assert_eq!(super::lan_name("Bar Sport".into()), "Bar Sport");
        assert!(!super::lan_name(String::new()).is_empty());
    }

    #[test]
    fn the_championship_notice_gives_the_time_in_every_language() {
        let opens = std::time::SystemTime::now();
        let t = time::OffsetDateTime::from(opens).to_offset(super::now_local().offset());
        let hhmm = format!("{:02}:{:02}", t.hour(), t.minute());
        let n = super::championship_notice(opens);
        assert_eq!(
            n.en(),
            format!(
                "Your club is in the Universal Championship: it plays on the big screen from about {hhmm}. Your club plays with its last lineup; you watch."
            )
        );
        for lang in [forza_ui::Lang::It, forza_ui::Lang::Ja] {
            let s = n.get(lang);
            assert!(s.contains(&hhmm) && s != n.en() && !s.contains("%{"), "{lang:?}: {s}");
        }
    }

    /// A 2017-18 game in a non-ASCII folder starts through a run junction, so the panel cannot tell the projector
    /// by its folder (WCCF_GAME is the real one): every client is told its role, 2010-11's are not.
    #[test]
    fn the_2017_18_clients_are_told_their_panel_role() {
        use super::{Edition, Role, panel_role};
        let role = |e, r| panel_role(e, r).map(|(k, v)| format!("{k}={v}"));
        assert_eq!(
            role(Edition::E1718, Role::Projector).as_deref(),
            Some("WCCFPANEL_ROLE=projector")
        );
        // any other value is a seat for the panel (`is_projector`)
        assert_eq!(role(Edition::E1718, Role::Seat).as_deref(), Some("WCCFPANEL_ROLE=seat"));
        assert_eq!(role(Edition::E1718, Role::Server), None);
        for r in [Role::Server, Role::Projector, Role::Seat] {
            assert_eq!(role(Edition::E1011, r), None, "2010-11 stays as it is");
        }
    }

    #[test]
    fn a_taken_game_server_port_reads_as_a_hosted_arcade() {
        // a second arcade on this PC holds 20050: PLAY -> THIS PC used to stop with "os error 10048"
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let taken = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap_err();
        assert_eq!(taken.kind(), std::io::ErrorKind::AddrInUse);
        let t = super::gamesvr_bind_error(port, &taken);
        assert_eq!(
            t.en(),
            format!("This PC is hosting an arcade on port {port}. Stop the arcade, or join it from ONLINE.")
        );
        for lang in [forza_ui::Lang::It, forza_ui::Lang::Ja] {
            let s = t.get(lang);
            assert!(
                !s.contains("%{") && s != t.en() && s.contains(&port.to_string()),
                "{lang:?}: {s}"
            );
        }
        // any other reason keeps the port and Windows' error
        let other = super::gamesvr_bind_error(20050, &std::io::Error::from_raw_os_error(87));
        assert!(
            other.en().contains("127.0.0.1:20050") && other.en().contains("os error 87"),
            "{}",
            other.en()
        );
    }

    #[test]
    fn pictures_are_made_at_play_when_missing_older_or_changed_but_not_after_a_failure() {
        let v = forza::cards::pictures::VERSION;
        let d = crate::scratch();
        let cards = d.path().join("cards");
        assert!(due(&cards, true, None), "no folder: made, as before");
        assert!(!due(&cards, false, None), "no game set up: nothing to make them from");
        fs::create_dir_all(&cards).unwrap();
        for old in [
            "game".to_owned(),
            "1 game".into(),
            "1 hq 5 P".into(),
            format!("{} game", v - 1),
        ] {
            fs::write(cards.join("source.txt"), &old).unwrap();
            assert!(due(&cards, true, None), "{old:?}: an older marker is made again");
            assert!(!due(&cards, false, None), "{old:?}: still no game");
        }
        fs::write(cards.join("source.txt"), format!("{v} game")).unwrap();
        assert!(!due(&cards, true, None), "current: nothing to do");
        assert!(
            due(&cards, true, Some(Path::new("pack.zip"))),
            "a pack chosen since: made again"
        );
        // a failed try is not repeated at every PLAY, with or without a pack
        fs::write(cards.join("source.txt"), format!("{v} failed")).unwrap();
        assert!(!due(&cards, true, None) && !due(&cards, true, Some(Path::new("pack.zip"))));
    }
}
