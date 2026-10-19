//! The run: WCCF 2010-11 or 2017-18 started, kept and stopped on this PC (port of Kit 6's play.py, which this
//! replaces, of the 2017-18 kit's older fork of it, and of the watcher that ended a run when a game window closed).
//!
//! The edition comes from the kit (`Kit::edition`).  2017-18 differs in: its stand-ins (no scene service; the game
//! server stand-in beside control; the Aime reader on COM2 and AiMeDB instead of the club card reader; the keychip
//! says SDDY), its shared memory (`debug::maps`), its guard (`crate::guard` before anything starts, then netguard.dll
//! injected into control, the projector and seat 1), the server's projector rule (the first PC gets it), and what it
//! leaves out (the club card's steps and view, the log inbox, the automatic report, the message guard).
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
//! Still Python, started as children in the job without a window: card_steps.py (the CLUB CARD panel's requests, the
//! table and the club card view), english.py and setup.py.  The server side is `programs\forza-server.exe all`, also a
//! child in the job.

pub mod rules;

use std::fs::{self, File};
use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};
use std::{env, ptr};

use crate::helper::{self, Helper, Target, ViewFiles};
use forza::online::seat::{self, Lease};
use forza::online::send_logs::{self, Scrub};
use forza::standins::{Handle, Log, aime, aimedb, card_reader, gamesvr, io_board, network, scenes};
use regex::Regex;
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, FALSE, GetLastError};
use windows_sys::Win32::System::Threading::{CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};

use crate::card;
use crate::debug::{self, Maps, Options};
use crate::job::Job;
use crate::kit::{Edition, find_game_named, seat_dir};
use crate::{Kit, choices, conf, guard, logs, procs};
pub use rules::{Mode, Words};

/// Exit codes, as play.py's: 0 ok, 1 a step failed, 2 not set up (or wrong words), 3 refused; 4 refused because
/// Windows would load the game without its guard.
pub const OK: i32 = 0;
pub const FAILED: i32 = 1;
pub const NOT_SET_UP: i32 = 2;
pub const REFUSED: i32 = 3;
pub const UNGUARDED: i32 = 4;
/// 2017-18: refused because the network guard Setup makes is not in place.
pub const NOT_GUARDED: i32 = 5;

/// A month and a day: a backstop only.  A player's run ends when its game window is closed; a server for others
/// stays up all month (a keeper task on that machine restarts it whenever it stops).
const LIFE: Duration = Duration::from_secs(32 * 24 * 3600);
/// How long a stop waits for the run's processes after closing their windows, before it ends them.
const STOP_DEADLINE: Duration = Duration::from_secs(10);
/// How long a request waits for the running game's answer (a stop takes up to its deadline and more).
const ANSWER_WAIT: Duration = Duration::from_secs(60);

pub const UNGUARDED_TEXT: &str = "NOT started: Windows was told to make the programs Launcher.exe starts use only \
    Windows' own copies of system files (\"Prefer System32 images\", passed on by the program that started \
    Launcher.exe - a WSL terminal does this). The game would then run without the kit's hook, which keeps it from \
    changing this PC's network settings. Start Launcher.exe from Explorer (double-click it) instead.";
/// The panel's line in `seat1\forza_panel.log` once its Present detour has drawn a frame: the run's proof it is up.
pub(crate) const PANEL_UP: &str = "first frame shown";

const NOT_SET_UP_TEXT: &str = "Not set up yet - set up first: Launcher.exe, Setup and updates.";
pub const NOT_GUARDED_TEXT: &str = "NOT started: the 2017-18 network guard is not in place, so the game could change \
    this PC's network settings (it has dropped the DHCP lease before). Press Set up / repair in \
    Launcher (Setup and updates), then start again. What is wrong:";

/// Where a run tells what it does: the window's box or the console.  `ask` gets a yes/no question (no one to ask:
/// no); `running` hears when the game is up (true) and when the run is over (false).
pub trait Ui {
    fn say(&self, line: &str);
    fn ask(&self, _question: &str) -> bool {
        false
    }
    fn running(&self, _up: bool) {}
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
    pub inbox_port: u16,
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
    /// 2017-18 online modes (remote, server) run instead of being refused: the online work and its tests; players
    /// get the refusal (the alpha has no 2017-18 online).
    pub online_1718: bool,
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
            inbox_port: forza::online::INBOX_PORT,
            network_ports: network::SERVICES.map(|s| s.0),
            icc_pipe: r"\\.\pipe\wccf_icc_seat1".into(),
            jvs_pipe: r"\\.\pipe\wccf_jvs_seat1".into(),
            aime_pipe: r"\\.\pipe\wccf_aime_seat1".into(),
            aimedb_port: 22345,
            gamesvr_port: 20050, // ctrl_user_option.conf GAME_SVR_PORT
            scenes: scenes::Names::game(),
            maps: Some(debug::maps::GAME_ID.into()),
            online_1718: false,
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
        if *mode == Mode::Server { Self::Server } else { Self::Run }
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
fn lock_name(kit: &Kit, kind: Kind) -> Vec<u16> {
    let key = procs::same_form(kit.dir())
        .encode_utf16()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, c| {
            (h ^ u64::from(c)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("Local\\wccf-kit-{}-{key:016x}", kind.name())
        .encode_utf16()
        .chain([0])
        .collect()
}

/// The run's mutex, if no one holds it: held for as long as the run lasts.
fn take_lock(kit: &Kit, kind: Kind) -> Option<OwnedHandle> {
    let name = lock_name(kit, kind);
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

/// A run of this kind is on (in this process or another).
pub fn is_on(kit: &Kit, kind: Kind) -> bool {
    let name = lock_name(kit, kind);
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
            ui.say("Nothing running.");
            outside(kit, p, ui)
        }),
        Words::Show => ask_each(kit, p, "show", ui).unwrap_or_else(|| {
            ui.say("No server is running on this PC.");
            FAILED
        }),
        Words::Restart if is_on(kit, Kind::Run) => {
            match choices::write(
                &kit.data().join("restart.request"),
                "RESTART NOW - asked from the command line\n",
            ) {
                Ok(()) => {
                    ui.say("RESTART NOW: the running game restarts (its window says so).");
                    OK
                }
                Err(e) => {
                    ui.say(&format!("RESTART NOW could not be asked: {e}"));
                    FAILED
                }
            }
        }
        Words::Restart => play(kit, p, None, false, ui),
        Words::Pictures => {
            ui.say("Making the card pictures (the first time takes a while)...");
            if make_pictures(kit, p, ui) {
                ui.say("The card pictures are in place.");
                OK
            } else {
                FAILED
            }
        }
        Words::Plain { debug } => play(kit, p, None, debug, ui),
        Words::Start { mode, debug } => {
            if let Mode::Remote(ip, _) = &mode
                && choices::setting(&kit.settings_file(), "server").as_deref() != Some(ip)
            {
                let _ = choices::set_setting(&kit.settings_file(), "server", ip); // "remote" alone uses it again
            }
            play(kit, p, Some(mode), debug, ui)
        }
        Words::RemoteRemembered { seat, debug } => match choices::setting(&kit.settings_file(), "server") {
            Some(ip) if choices::valid_address(&ip) => play(kit, p, Some(Mode::Remote(ip, seat)), debug, ui),
            _ => {
                ui.say("Which server? Launcher.exe remote, then the server's address (four numbers with dots).");
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
    ui.say(&format!(
        "Running from the game's folders, not started by this Launcher.exe: {} - close them yourself (or restart \
         Windows).",
        left.iter()
            .map(|q| format!("{} {}", q.name, q.pid))
            .collect::<Vec<_>>()
            .join(", ")
    ));
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
    for kind in [Kind::Run, Kind::Server] {
        let Some((mut code, said)) = request(kit, kind, word) else {
            continue;
        };
        for line in said.lines() {
            ui.say(line);
        }
        // never a dead end: the person at the stop decides, knowing the cost
        if code == REFUSED
            && !check
            && ui.ask("Stop anyway? Type Y and press Enter to stop now - just Enter leaves the game running: ")
        {
            ui.say("Stopping, as asked.");
            let (c, said) = request(kit, kind, "force").unwrap_or((OK, String::new()));
            for line in said.lines() {
                ui.say(line);
            }
            code = c;
        } else if code == REFUSED && !check {
            ui.say("(\"Launcher.exe stop force\" stops without asking.)");
        }
        rc = Some(rc.unwrap_or(OK).max(code));
    }
    rc.unwrap_or_else(|| {
        ui.say("Nothing running.");
        outside(kit, p, ui)
    })
}

/// Starts and supervises; a full RESTART NOW starts again with what a plain start chooses.
fn play(kit: &Kit, p: &Params, mode: Option<Mode>, debug: bool, ui: &dyn Ui) -> i32 {
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
    let panel = choices::read(&kit.data().join("panel.txt"));
    if !choices::panel_value(&panel, "play_on").is_some_and(|v| v.eq_ignore_ascii_case("online")) {
        return Mode::Local;
    }
    if kit.edition() == Edition::E1718 {
        return Mode::Server; // start refuses it: 2017-18 online is not built, and the panel asked for online
    }
    let ip = choices::panel_value(&panel, "server").unwrap_or_default();
    if !choices::valid_address(&ip) {
        ui.say(&format!(
            "The SETTINGS panel says play online, but \"{ip}\" is not a server address - playing on this PC. (In the \
             game: SETTINGS, then type the address again.)"
        ));
        return Mode::Local;
    }
    if !answers(&ip, p.control_port) {
        ui.say(&format!(
            "The SETTINGS panel says play online, but the server at {ip} does not answer - playing on this PC instead. \
             (The panel keeps ONLINE for next time; THIS PC there stops this try.)"
        ));
        return Mode::Local;
    }
    ui.say("Online, as the SETTINGS panel chose (Launcher.exe local plays on this PC).");
    Mode::Remote(ip, None)
}

/// Something takes a TCP connection on ip:port within 5 s (closed again at once).
fn answers(ip: &str, port: u16) -> bool {
    format!("{ip}:{port}")
        .parse::<SocketAddr>()
        .is_ok_and(|a| TcpStream::connect_timeout(&a, Duration::from_secs(5)).is_ok())
}

fn addr(ip: &str, port: u16) -> Option<SocketAddr> {
    format!("{ip}:{port}").parse().ok()
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

/// The board's card pictures, made at setup (and again when Launcher's choice changes) by
/// `forza::cards::pictures::ensure` into the folder the panel reads: the player's game's own, and with
/// data\\settings.json "card_pictures": "hq" the high-quality pack at "card_pack" over them. The run only reads them.
/// `true` when they are in place.
pub fn make_pictures(kit: &Kit, p: &Params, ui: &dyn Ui) -> bool {
    let Some(game) = game_folder(kit, p) else {
        ui.say("  (no game is set up yet - the card pictures are made when you set up)");
        return false;
    };
    let ed = match kit.edition() {
        Edition::E1011 => "1011",
        Edition::E1718 => "1718",
    };
    let s = kit.settings();
    let mut pack = None;
    if s["card_pictures"] == "hq" {
        pack = s["card_pack"].as_str().map(PathBuf::from).filter(|p| p.is_file());
        if pack.is_none() {
            ui.say("  (the high-quality card pack is not there any more - the game's own pictures are used)");
        }
    }
    let say = |l: &str| ui.say(l);
    match forza::cards::pictures::ensure(&game, ed, &kit.cache().join("cards"), pack.as_deref(), &say) {
        Ok(()) => true,
        Err(e) => {
            ui.say(&format!(
                "  (the card pictures could not be made: {e} - the board shows stand-ins)"
            ));
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
struct Failed(String);

impl From<io::Error> for Failed {
    fn from(e: io::Error) -> Self {
        Self(e.to_string())
    }
}

fn wait_for(
    what: &str,
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
        format!("{what} did not come up")
    } else {
        format!("{what} did not come up - see data\\logs\\{log}")
    }))
}

/// A kit Python script, without an earlier run's MXHOOK_ / WCCF settings and with `env`.
fn python_cmd(kit: &Kit, logs: &Path, env: &[(String, String)], script: &str, args: &[String]) -> Command {
    let mut cmd = kit.script(script);
    cmd.args(args);
    clean_env(&mut cmd, logs, env);
    cmd.env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUNBUFFERED", "1");
    cmd
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

/// `cmd` in the job, its output to `log`; its exit code and output once it ends within `limit` (or `stop` is set: it
/// is ended then).
fn run_in_job(
    job: &Job,
    mut cmd: Command,
    log: &Path,
    limit: Duration,
    stop: &AtomicBool,
) -> Result<(i32, String), String> {
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
        if stop.load(Ordering::Relaxed) || Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if stop.load(Ordering::Relaxed) {
                "the run is ending".to_owned()
            } else {
                format!("it did not finish in {} s", limit.as_secs())
            });
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

/// A log of the run's stand-ins: data\logs\<name>, a line each with the seconds since it started.
fn file_log(path: PathBuf) -> Log {
    let t0 = Instant::now();
    let file = Mutex::new(File::create(path).ok());
    Arc::new(move |msg| {
        if let Some(f) = file.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut() {
            let _ = writeln!(f, "{:8.2}  {msg}", t0.elapsed().as_secs_f64());
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

impl Role {
    fn log(self) -> &'static str {
        match self {
            Self::Server => "run_server.txt",
            Self::Projector => "run_projector.txt",
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
    Restart,
    Request(String),
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
    lease: Option<Lease>,
    _lock: OwnedHandle,
}

/// Starts a run; `Err` is the exit code (what went wrong is said).
pub fn start<'a>(kit: &'a Kit, p: &'a Params, mode: Mode, debug: bool, ui: &'a dyn Ui) -> Result<Run<'a>, i32> {
    if procs::prefers_system32() {
        ui.say(UNGUARDED_TEXT);
        return Err(UNGUARDED);
    }
    let Some(game) = game_folder(kit, p) else {
        ui.say(NOT_SET_UP_TEXT);
        return Err(NOT_SET_UP);
    };
    let seat = seat_dir(&game);
    let edition = kit.edition();
    if rules::online_refused(edition, &mode) && !p.online_1718 {
        ui.say(rules::ONLINE_1718_TEXT);
        return Err(REFUSED);
    }
    if edition == Edition::E1011 && !seat.join(".wccf-kit").is_file() {
        ui.say(NOT_SET_UP_TEXT);
        return Err(NOT_SET_UP);
    }
    // before anything of the run exists: no game program may start unguarded (AGENTS.md section 5)
    if edition == Edition::E1718
        && let Err(why) = guard::check(&game, &seat, p.client(), p.control())
    {
        ui.say(&format!("{NOT_GUARDED_TEXT} {why}"));
        return Err(NOT_GUARDED);
    }
    if !kit.cache().join("catalogue.tsv").is_file() {
        // the panel's card browser only: never a reason not to start
        ui.say("  (the card CATALOGUE is not built yet - the panel's catalogue will be empty; set up rebuilds it)");
    }
    let kind = Kind::of(&mode);
    let beside_server = matches!(mode, Mode::Remote(..)) && is_on(kit, Kind::Server);
    let left = if beside_server { Vec::new() } else { strays(&game, p) };
    let busy = is_on(kit, Kind::Run) || (!beside_server && is_on(kit, Kind::Server)) || !left.is_empty();
    let lock = if busy { None } else { take_lock(kit, kind) };
    let Some(lock) = lock else {
        let mut names: Vec<&str> = left.iter().map(|q| q.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        ui.say(&format!(
            "Already running ({}) - close the game's window first (a game that is stuck: Launcher.exe stop).",
            if names.is_empty() {
                "the game".into()
            } else {
                names.join(", ")
            }
        ));
        return Err(REFUSED);
    };
    let job = match Job::new() {
        Ok(j) => Arc::new(j),
        Err(e) => {
            ui.say(&format!("FAILED: no Job object for the run ({e})"));
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
        lease: None,
        _lock: lock,
    };
    // requests left by a run that ended are never carried out
    for f in [
        request_file(kit, kind),
        answer_file(kit, kind),
        kit.data().join("sendlogs.request"),
    ] {
        let _ = fs::remove_file(f);
    }
    if kind == Kind::Run {
        let _ = fs::remove_file(kit.data().join("restart.request"));
    }
    match run.start_all() {
        Ok(()) => Ok(run),
        Err(Failed(why)) => {
            // an empty reason: the game is not set up (said already); the reason goes in run_launcher.txt too
            if !why.is_empty() {
                run.say(&format!("FAILED: {why}"));
                run.say("Stopping what was started ...");
            }
            run.shutdown();
            let _ = fs::remove_file(run.running_file());
            Err(if why.is_empty() { NOT_SET_UP } else { FAILED })
        }
    }
}

impl Run<'_> {
    fn say(&self, line: &str) {
        self.ui.say(line);
        // and in data\logs\run_launcher.txt: the window's box and a console are gone when they close, and a console
        // Launcher.exe's lines do not reach a cmd redirect (first live run, 2026-10-13: the start's lines were lost)
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.logs().join("run_launcher.txt"))
        {
            let t = crate::logs::local(std::time::SystemTime::now());
            let _ = writeln!(f, "{:02}:{:02}:{:02}  {line}", t.hour(), t.minute(), t.second());
        }
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

    fn ip(&self) -> Option<&str> {
        match &self.mode {
            Mode::Remote(ip, _) => Some(ip),
            _ => None,
        }
    }

    /// What every program of the run gets: where the game and the logs are, and where the panel finds its data.
    fn kit_env(&self) -> Vec<(String, String)> {
        let s = |p: PathBuf| p.display().to_string();
        let cache = self.kit.cache();
        vec![
            ("WCCF_GAME".into(), s(self.game.clone())),
            ("WCCF_LOGS".into(), s(self.logs())),
            ("MXHOOK_LOG_DIR".into(), s(self.logs().join("hook"))),
            ("WCCF_SKIN".into(), s(self.kit.dir().join("art").join("skin.tex"))),
            ("WCCF_CATALOGUE".into(), s(cache.join("catalogue.tsv"))),
            ("WCCF_CARDS".into(), s(cache.join("cards"))),
        ]
    }

    /// A kit Python script, without an earlier run's MXHOOK_ / WCCF settings and with this run's.
    fn python(&self, script: &str, args: &[String]) -> Command {
        python_cmd(self.kit, &self.logs(), &self.kit_env(), script, args)
    }

    /// `cmd` in the job, its output to data\logs\<log>; its exit code and output once it ends within `limit`.
    fn run_child(&self, cmd: Command, log: &str, limit: Duration) -> Result<(i32, String), String> {
        run_in_job(&self.job, cmd, &self.logs().join(log), limit, &AtomicBool::new(false))
    }

    /// The helper threads: the club card view (view.py when the card changes) and the starting volumes of the projector
    /// (muted) and seat 1.  They never stop the run.
    fn start_helper(&mut self, projector: bool) {
        let say = file_log(self.logs().join("run_helper.txt"));
        let mut h = Helper::default();
        let (kit, job, logs, env) = (Kit::at(self.kit.dir()), self.job.clone(), self.logs(), self.kit_env());
        // 2017-18 keeps its clubs on the game server stand-in: no club card file to show
        if self.edition == Edition::E1011 {
            h.club_view(
                ViewFiles::new(&self.kit.data()),
                move |stop| {
                    let cmd = python_cmd(&kit, &logs, &env, "card_steps.py", &["view".to_owned()]);
                    let (code, out) = run_in_job(
                        &job,
                        cmd,
                        &logs.join("run_club_view.txt"),
                        Duration::from_secs(60),
                        stop,
                    )?;
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

    /// The card steps the CLUB CARD panel asked for (card_steps.py: a manager transfer finished, a card switch, its
    /// bad endings cleared, the table following the club), done while the card reader is not running; never a reason
    /// not to start.
    fn card_steps(&self, what: &[&str]) {
        if self.edition == Edition::E1718 {
            return; // its club is on the game server stand-in, its card is an Aime: no club card file
        }
        let mut args: Vec<String> = what.iter().map(|s| (*s).to_owned()).collect();
        args.push(self.seat.display().to_string());
        match self.run_child(
            self.python("card_steps.py", &args),
            "run_card_steps.txt",
            Duration::from_secs(120),
        ) {
            Ok((_, out)) => out.lines().filter(|l| !l.trim().is_empty()).for_each(|l| self.say(l)),
            Err(e) => self.say(&format!(
                "  club card: the card steps did not run ({e}) - the card in the slot stays"
            )),
        }
    }

    /// Kit 6 loads its hook as wccfk.dll; a game set up by an older kit, or a newer kit unzipped over this one by hand,
    /// has another copy (or none) - set up once, as Setup does, instead of starting the old hook or refusing.
    fn ensure_hook(&self) -> Result<(), Failed> {
        let ours = fs::read(self.kit.programs().join("wccfk.dll")).ok();
        let same = |dir: &Path| ours.is_some() && fs::read(dir.join("wccfk.dll")).ok() == ours;
        if same(&self.game) && same(&self.seat) {
            return Ok(());
        }
        self.say(
            "The game's copy of the kit's helper (wccfk.dll) is missing or not this kit's - setting the game up for it \
             once (as Setup does) ...",
        );
        let cmd = self.python("setup.py", &[self.game.display().to_string()]);
        match self.run_child(cmd, "run_setup.txt", Duration::from_secs(3600)) {
            Ok((code, out)) => {
                out.lines().for_each(|l| self.say(l));
                if code == 0 {
                    return Ok(());
                }
            }
            Err(e) => self.say(&format!("setup.py: {e}")),
        }
        self.say(
            "Setting the game up did not finish (see above) - open Launcher.exe, Setup and updates, and press set up.",
        );
        Err(Failed(String::new()))
    }

    /// The SETTINGS panel's english=on|it|off: done now, before anything starts (english.py refuses while the game
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
            let name = match lang {
                "en" => "English",
                "it" => "Italian",
                _ => "the original Japanese",
            };
            self.say(&format!(
                "  the game's text into {name}, as the SETTINGS panel asked - this takes a moment ..."
            ));
            let args: Vec<String> = match lang {
                "it" => vec!["on".into(), "it".into()],
                "en" => vec!["on".into()],
                _ => vec!["off".into()],
            };
            // english.py swaps each file in one step, so even a stop half way leaves every file in place
            let why = match self.run_child(
                self.python("english.py", &args),
                "run_english.txt",
                Duration::from_secs(600),
            ) {
                Ok((0, _)) => None,
                Ok((code, out)) => Some(
                    out.lines()
                        .rfind(|l| !l.trim().is_empty())
                        .map_or_else(|| format!("english.py exit {code}"), |l| l.trim().to_owned()),
                ),
                Err(e) => Some(format!("it did not finish: {e}")),
            };
            match why {
                Some(why) => self.say(&format!(
                    "  the text could not be switched ({why}) - the game starts as it is"
                )),
                None => self.say(&format!("  done: the game's text is {name}")),
            }
        }
        let left = choices::set_panel_value(&choices::read(&path), "english", None);
        let _ = choices::write(&path, &left);
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
            // control+0x1226A is 2010-11's control: never 2017-18's
            msgguard: self.edition == Edition::E1011 && env::var("WCCF_MSGGUARD").map_or(true, |v| v != "0"),
            env: self.kit_env().into_iter().chain(env).chain([game_dpi()]).collect(),
            stop: Some(stop.clone()),
        };
        let mut log = File::create(self.logs().join(role.log()))?;
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

    /// A DLL into the running game (the panel, forza_panel.dll, or 2017-18's netguard.dll): "" when it loaded, else
    /// why not.
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

    /// 2017-18: netguard.dll (the game folder's, whose hash the guard checked) into a game program - the second line
    /// behind wccfnet.dll; said, never a reason to stop (as play.py's inject_guard).
    fn netguard(&self, pid: u32, what: &str) {
        if self.edition != Edition::E1718 {
            return;
        }
        let why = self.inject(pid, &self.game.join("netguard.dll"));
        self.say(&if why.is_empty() {
            format!("     {what}: netguard.dll loaded")
        } else {
            format!("     {what}: netguard.dll NOT loaded ({why})")
        });
    }

    fn start_all(&mut self) -> Result<(), Failed> {
        let kit = self.kit;
        let p = self.p;
        if self.beside_server {
            fs::create_dir_all(self.logs().join("hook"))?; // its logs are open: no rotation
            let panel = choices::read(&kit.data().join("panel.txt"));
            if rules::english_change(&panel, kit.game_language()).is_some() {
                self.say(
                    "  (the SETTINGS panel's language change waits: the server on this PC uses the same game files)",
                );
            }
        } else {
            if self.edition == Edition::E1011 {
                self.ensure_hook()?; // 2017-18: setup.py puts its hook in place
            }
            for line in logs::rotate(&self.logs())? {
                self.say(&line);
            }
            self.apply_english_request();
        }
        // the card reader is not running yet in any mode; nor FPR_Emu or the overlay, which read the table
        if self.mode == Mode::Server {
            self.card_steps(&["start"]);
        } else {
            self.card_steps(&["start", "board"]);
        }
        let steps = if self.mode == Mode::Server { 2 } else { 6 };
        let confs = [
            self.game.join(r"local\client_user_option.conf"),
            self.seat.join(r"local\client_user_option.conf"),
        ];
        self.say(&format!(
            "Starting WCCF {} from {}{}",
            self.edition.name(),
            self.game.display(),
            match &self.mode {
                Mode::Server => " - the server side only".to_owned(),
                Mode::Remote(ip, _) => format!(" - the cabinets, on the server at {ip}"),
                Mode::Local => String::new(),
            }
        ));
        if self.edition == Edition::E1718 {
            self.start_gamesvr(steps)?;
        } else if self.beside_server {
            self.say(&format!(
                "  1/{steps} scene service (the server's, already running on this PC)"
            ));
        } else {
            let xscn = self.game.join(r"data\motion\launcherMotion\xscn");
            let log = file_log(self.logs().join("xscn_watch_log.txt"));
            let h = scenes::spawn(&xscn, &p.scenes, log).map_err(|e| {
                Failed(format!(
                    "the scene service did not start ({e}) - see data\\logs\\xscn_watch_log.txt"
                ))
            })?;
            self.scenes = Some(h);
            self.say(&format!("  1/{steps} scene service"));
        }

        let mut projector = true;
        if let Mode::Remote(ip, want) = self.mode.clone() {
            if !answers(&ip, p.control_port) {
                return Err(Failed(format!(
                    "the server at {ip} does not answer on port {} - is it running there (Launcher.exe server), and \
                     is that port open to this PC? (Launcher.exe local plays on this PC instead)",
                    p.control_port
                )));
            }
            if self.edition == Edition::E1011 {
                self.send_pending_report(); // 2017-18's server has no log inbox
            }
            let (seat, here) = self.take_seat(&ip, want)?;
            (self.seat_no, projector) = (seat, here);
            for c in &confs {
                conf::set_control_ip(c, &ip)?;
            }
            conf::set_satellite_no(&confs[1], u32::from(seat))?; // the cabinet's seat
            // never "WAITING FOR THE PROJECTOR" on a server: its projector is on another PC, or gone
            conf::set_conf_value(&confs[1], "WAIT_PROJECTOR", "0")?;
            self.say(&format!(
                "  2/{steps} server: {ip} answers on port {} - this PC is seat {seat}{}",
                p.control_port,
                if here { " and shows the projector" } else { "" }
            ));
        } else {
            let ctrl_conf = self.game.join(r"local\ctrl_user_option.conf");
            if self.mode == Mode::Local {
                for c in &confs {
                    conf::set_control_ip(c, "127.0.0.1")?; // after a "remote" run: the cabinets play on this PC again
                }
                conf::set_satellite_no(&confs[1], 1)?;
                conf::set_conf_value(&confs[1], "WAIT_PROJECTOR", "1")?; // the game's own way, its projector here
                conf::set_conf_value(&ctrl_conf, "WAIT_PROJECTOR", "1")?;
            } else if self.edition == Edition::E1718 {
                // a server for others: its seat desk gives the first PC the projector, which control waits for
                conf::set_conf_value(&ctrl_conf, "WAIT_PROJECTOR", "1")?;
            } else {
                // a server for others: no projector waited for (the seat desk gives no PC the projector)
                conf::set_conf_value(&ctrl_conf, "WAIT_PROJECTOR", "0")?;
            }
            let game = self.game.clone();
            self.start_game(Role::Server, p.control_args.clone(), &game, !self.debug, Vec::new())?;
            wait_for(
                "the server",
                90,
                Role::Server.log(),
                || self.alive(Role::Server),
                || procs::listening_ports().contains(&p.control_port) && self.job_count(p.matcher()) >= p.match_engines,
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
                self.netguard(ctl.pid, "server");
                if !self.debug {
                    procs::hide_windows(ctl.pid); // it starts hidden; a window it shows by itself is hidden here
                }
            }
            self.say(&format!(
                "  2/{steps} server: listening on port {}, {} match engines",
                p.control_port,
                self.job_count(p.matcher())
            ));
        }
        if self.mode == Mode::Server {
            return self.start_server_side();
        }

        if projector {
            let args = p.projector_args.clone().unwrap_or_else(|| vec!["0".into()]);
            let game = self.game.clone();
            self.start_game(Role::Projector, args, &game, false, Vec::new())?;
            self.say("  3/6 projector (seat 0) - its window opens in a few seconds");
        } else {
            self.say(match self.edition {
                Edition::E1011 => "  3/6 projector: none on this PC - online, each PC runs only its own cabinet",
                Edition::E1718 => "  3/6 projector: none on this PC - the server's first PC shows it",
            });
        }
        self.start_cabinet()?;
        if projector {
            if let Some(pid) = self.launched(Role::Projector) {
                self.netguard(pid, "projector");
            }
            // the overlay in its money-only mode: the projector's amounts with commas; it plays on without it
            let panel = self.kit.program("forza_panel.dll");
            let why = self
                .launched(Role::Projector)
                .map_or_else(|| "its process was not found".into(), |pid| self.inject(pid, &panel));
            self.say(&if why.is_empty() {
                "       projector: amounts with commas".to_owned()
            } else {
                format!("       projector: its amounts stay without commas ({why})")
            });
        }
        self.start_helper(projector);
        self.write_running();
        self.say("");
        if let Mode::Remote(ip, _) = &self.mode {
            self.say(&format!(
                "Playing on the server at {ip}{}.",
                if projector {
                    String::new()
                } else {
                    format!(" as seat {} (no projector on this PC)", self.seat_no)
                }
            ));
        }
        self.say("Running. Click the cabinet's window, then play with the keyboard (it works only while a game window is in front):");
        self.say("  Enter START (free play - no coin needed)   X decide   C shoot   arrows: tactics");
        self.say("  I club card in / out   F1 test menu   F11 window size   - or click the buttons around the picture");
        self.say(match self.edition {
            Edition::E1011 => {
                "First time: put the card in with I, and the game makes your club. It is saved in data\\save\\seat1_club.bin."
            }
            Edition::E1718 => {
                "Your Aime card is data\\save\\seat1_aime.txt; the game server stand-in keeps your club in data\\save\\gamesvr."
            }
        });
        self.say(
            "To finish: let the card come out after a match, then close the game's window - the rest stops with it.",
        );
        Ok(())
    }

    /// "server": `forza-server all` (the seat desk, the match relay and the log inbox in one Job child, each with its
    /// own log in data\logs), then the run waits for a stop.
    fn start_server_side(&mut self) -> Result<(), Failed> {
        let logs = self.logs();
        let mut env = self.kit_env();
        env.extend([
            ("WCCF_BROKER_PORT".to_owned(), self.p.seat_port.to_string()),
            ("WCCF_RELAY_PORT".to_owned(), self.p.relay_port.to_string()),
            ("WCCF_INBOX_PORT".to_owned(), self.p.inbox_port.to_string()),
        ]);
        let mut cmd = Command::new(self.kit.program("forza-server.exe"));
        cmd.args(["--edition", self.edition.name(), "all"])
            .arg((LIFE.as_secs() + 60).to_string())
            .arg(&logs);
        clean_env(&mut cmd, &logs, &env);
        let out = "run_forza_server.txt";
        let role = "online server";
        let started = File::create(logs.join(out)).and_then(|f| {
            cmd.stderr(f.try_clone()?).stdout(f);
            self.job.spawn(&mut cmd)
        });
        match started {
            Ok(child) => self.kids.push((role, child)),
            Err(e) => self.say(&format!("     the {role} did not start ({e})")),
        }
        thread::sleep(Duration::from_millis(1500));
        let alive = self
            .kids
            .last_mut()
            .is_some_and(|(r, c)| *r == role && c.try_wait().is_ok_and(|s| s.is_none()));
        if alive {
            self.say(&format!(
                "     seat desk: TCP {} - each player's PC gets the next free seat ({})",
                self.p.seat_port,
                match self.edition {
                    Edition::E1011 => "its cabinet only, no projector",
                    Edition::E1718 => "the first also the projector",
                }
            ));
            self.say(&format!(
                "     match relay: UDP {} - a match between two players goes through this machine",
                self.p.relay_port
            ));
            if self.edition == Edition::E1011 {
                self.say(&format!(
                    "     log inbox: TCP {} - logs players send from SETTINGS go to data\\player_logs",
                    self.p.inbox_port
                ));
            }
        } else {
            self.say(&format!(
                "     the {role} did not start or ended (data\\logs\\{out}, seat_broker.txt, match_relay.txt, log_inbox.txt)"
            ));
        }
        self.write_running();
        self.say("");
        self.say(
            "Server running. On each player's PC: Launcher.exe remote, then this machine's address - the one their PC",
        );
        self.say(&match self.edition {
            Edition::E1011 => format!(
                "can reach. This machine must let in TCP ports {}, {} and {}, and UDP {}. Launcher.exe stop ends it.",
                self.p.control_port, self.p.seat_port, self.p.inbox_port, self.p.relay_port
            ),
            Edition::E1718 => format!(
                "can reach. This machine must let in TCP ports {} and {}, and UDP {}. Launcher.exe stop ends it.",
                self.p.control_port, self.p.seat_port, self.p.relay_port
            ),
        });
        Ok(())
    }

    /// Steps 4-6 and the key driver: seat N's own side - the keychip and network, card reader and I/O board stand-ins,
    /// the cabinet (the seat1 folder) with its overlay.  A cabinet restart runs only this.
    fn start_cabinet(&mut self) -> Result<(), Failed> {
        let p = self.p;
        let net_log = file_log(self.logs().join("ringedge_log.txt"));
        for (&(_, name, kind), &port) in network::SERVICES.iter().zip(&p.network_ports) {
            let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
                Failed(format!(
                    "the cabinet's stand-ins did not come up: 127.0.0.1:{port} ({e}) - is the other edition's game running?"
                ))
            })?;
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
                    .map_err(|e| Failed(format!("the card reader stand-in did not start ({e})")))?,
            );
            format!("COM3={0};COM1={0}", p.icc_pipe)
        };
        let jvs_log = file_log(self.logs().join("jvs_log.txt"));
        self.cabinet.board = Some(
            io_board::spawn(&p.jvs_pipe, p.shared.clone(), jvs_log)
                .map_err(|e| Failed(format!("the I/O board stand-in did not start ({e})")))?,
        );
        self.say(match self.edition {
            Edition::E1011 => {
                "  4/6 cabinet stand-ins: keychip/network, card reader (card file data\\save\\seat1_club.bin), I/O board"
            }
            Edition::E1718 => {
                "  4/6 cabinet stand-ins: keychip/network, Aime reader (card file data\\save\\seat1_aime.txt), AiMeDB, I/O board"
            }
        });

        let mut env = vec![
            ("MXHOOK_RINGEDGE".to_owned(), "1".to_owned()),
            ("MXHOOK_COM".into(), format!("{coms};mxjvs={0};COM4={0}", p.jvs_pipe)),
        ];
        if let Some(ip) = self.ip() {
            // a match against another player goes through the server's relay (forza_panel.dll)
            env.push(("WCCF_RELAY".into(), format!("{ip}:{}", p.relay_port)));
            env.push(("WCCF_SEAT".into(), self.seat_no.to_string()));
        }
        if self.debug {
            env.push(("MXHOOK_VERBOSE".into(), "1".into()));
        }
        let n = self.seat_no;
        let args = p.seat_args.clone().unwrap_or_else(|| vec![n.to_string()]);
        let seat = self.seat.clone();
        self.start_game(Role::Seat, args, &seat, false, env)?;
        let mut pid = None;
        wait_for(
            &format!("seat {n}"),
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
            &format!("seat {n}'s window"),
            p.window_wait.as_secs(),
            Role::Seat.log(),
            || self.alive(Role::Seat),
            || procs::has_window(pid),
        )?;
        self.say(&format!("  5/6 seat {n} (the player cabinet) - window up"));
        self.netguard(pid, &format!("seat {n}"));
        thread::sleep(Duration::from_secs(3));
        let olog = self.seat.join("forza_panel.log");
        let skip = fs::metadata(&olog).map_or(0, |m| m.len() as usize);
        let why = self.inject(pid, &self.kit.program("forza_panel.dll"));
        if why.is_empty() {
            let up = wait_for(
                "the overlay",
                60,
                "",
                || true,
                || {
                    fs::read(&olog)
                        .is_ok_and(|t| String::from_utf8_lossy(t.get(skip..).unwrap_or_default()).contains(PANEL_UP))
                },
            );
            self.say(&if up.is_ok() {
                format!("  6/6 overlay in seat {n}'s window")
            } else {
                "  6/6 overlay loaded (it did not report its window yet - see seat1\\forza_panel.log)".to_owned()
            });
        } else {
            // the game runs without the overlay - never a reason to take the game away
            self.say(&format!(
                "  6/6 the overlay did not load ({why}) - the game runs without its panels; the keyboard works as usual"
            ));
        }
        let keys_path = env::var_os("WCCF_KEYS").map_or_else(|| self.kit.data().join("keys.txt"), PathBuf::from);
        self.cabinet.keys = Some(crate::keys::spawn(
            p.shared.clone(),
            keys_path,
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
                .map_err(|e| Failed(format!("the Aime reader stand-in did not start ({e})")))?,
        );
        let listener = TcpListener::bind(("127.0.0.1", p.aimedb_port)).map_err(|e| {
            Failed(format!(
                "the AiMeDB stand-in did not start: 127.0.0.1:{} ({e})",
                p.aimedb_port
            ))
        })?;
        let log = file_log(self.logs().join("aimedb_log.txt"));
        self.cabinet.aimedb = Some(
            aimedb::spawn(listener, save.join("aimedb.json"), log)
                .map_err(|e| Failed(format!("the AiMeDB stand-in did not start ({e})")))?,
        );
        Ok(())
    }

    /// Step 1 for 2017-18: no scene service; control keeps the clubs on the game server, so the stand-in runs beside
    /// it (ctrl_user_option.conf GAME_SVR_IP=localhost).  A cabinet on a server uses that server's.
    fn start_gamesvr(&mut self, steps: u32) -> Result<(), Failed> {
        if matches!(self.mode, Mode::Remote(..)) {
            self.say(&format!("  1/{steps} game server stand-in: the server's"));
            return Ok(());
        }
        let port = self.p.gamesvr_port;
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
            Failed(format!(
                "the game server stand-in did not start: 127.0.0.1:{port} ({e})"
            ))
        })?;
        let log = file_log(self.logs().join("gamesvr_log.txt"));
        // the run's game folder (also recorded as "game" by the 2017-18 setup): the stand-in pre-registers its 2D-code cards,
        // and data\settings.json "skip_tutorial": true makes it give a new Aime id a club from the game's cpu_50.dat
        let skip = self.kit.settings()["skip_tutorial"].as_bool().unwrap_or(false);
        let game = Some(self.game.clone());
        self.gamesvr = Some(
            gamesvr::spawn_with(listener, self.kit.save().join("gamesvr"), log, game, skip)
                .map_err(|e| Failed(format!("the game server stand-in did not start ({e})")))?,
        );
        self.say(&format!(
            "  1/{steps} game server stand-in on port {port} (club saves in data\\save\\gamesvr)"
        ));
        Ok(())
    }

    /// Online: this PC's seat from the server's seat desk, held while the run lasts; None asked = the next free seat.
    fn take_seat(&mut self, ip: &str, want: Option<u8>) -> Result<(u8, bool), Failed> {
        let at = addr(ip, self.p.seat_port).ok_or_else(|| Failed(format!("{ip} is not an address")))?;
        let log = file_log(self.logs().join("run_seat_lease.txt"));
        match seat::claim(at, want, LIFE, seat::Timing::default(), log) {
            Ok(lease) => {
                let got = (lease.seat, lease.projector);
                self.lease = Some(lease);
                Ok(got)
            }
            Err(e) => match e.no_desk_seat(want) {
                Some((s, proj)) => {
                    self.say(&format!(
                        "  (the server at {ip} has no seat desk - seat {s}{}, as before)",
                        if proj { " with the projector" } else { "" }
                    ));
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

    /// A report the last run could not send (data\report.pending): tried again in the background while the start
    /// goes on; removed only once the server took it.
    fn send_pending_report(&self) {
        let pending = self.kit.data().join("report.pending");
        let Ok(text) = fs::read_to_string(&pending) else { return };
        let Some(at) = text.lines().next().and_then(|s| addr(s.trim(), self.p.inbox_port)) else {
            return;
        };
        let (kit, game) = (self.kit.dir().to_path_buf(), self.game.clone());
        let out = self.logs().join("run_report.txt");
        thread::spawn(move || {
            let r = send_logs::upload(at, &kit, Some(&game), &Scrub::from_env(), Some(&pending));
            let _ = fs::write(out, format!("{r:?}\n"));
        });
    }

    /// Online, after a run that went wrong: this PC's logs to the server's log inbox by themselves (what SEND LOGS
    /// sends, never the club card), or data\report.pending for the next start; panel.txt auto_logs=0 turns it off.
    fn report_bad_run(&self, why: &[String]) {
        let Some(ip) = self.ip() else { return };
        if self.edition == Edition::E1718 {
            return; // its server has no log inbox
        }
        let panel = choices::read(&self.kit.data().join("panel.txt"));
        if why.is_empty() || choices::panel_value(&panel, "auto_logs").as_deref() == Some("0") {
            return;
        }
        let note = format!(
            "sent by itself at the end of an online run that went wrong:\n{}\n",
            why.join("\n")
        );
        let sent = addr(ip, self.p.inbox_port)
            .ok_or_else(|| "no address".to_owned())
            .and_then(|at| {
                let (blob, _) = send_logs::pack(
                    self.kit.dir(),
                    Some(&self.game),
                    &Scrub::from_env(),
                    send_logs::TAIL,
                    Some(&note),
                )
                .map_err(|e| e.to_string())?;
                send_logs::send(at, &blob, &send_logs::version(self.kit.dir()), Duration::from_secs(60))
                    .map_err(|e| e.to_string())
            });
        match sent {
            Ok(code) => self.say(&format!(
                "This run went wrong ({}) - its logs went to the server at {ip} by themselves: code {code}.",
                why[0]
            )),
            Err(e) => {
                let _ = fs::write(self.kit.data().join("report.pending"), format!("{ip}\n{note}"));
                self.say(&format!(
                    "This run went wrong ({}) - its logs could not reach the server ({e}); the next start sends them.",
                    why[0]
                ));
            }
        }
    }

    /// SEND LOGS (the SETTINGS panel): data\sendlogs.request holds the server's address; the logs are packed and sent
    /// on a thread, and data\sendlogs.result tells the panel "sending", then "ok CODE" or "fail WHY".
    fn send_logs(kit: &Path, inbox_port: u16, address: &str) {
        let result = kit.join("data").join("sendlogs.result");
        let put = move |text: &str| {
            let _ = choices::write(&result, &format!("{text}\n"));
        };
        let address = address.trim().to_owned();
        let Some(at) = addr(&address, inbox_port).filter(|_| choices::valid_address(&address)) else {
            put("fail no server address - type it under PLAY ON first");
            return;
        };
        put("sending");
        let kit = kit.to_path_buf();
        thread::spawn(
            move || match send_logs::upload(at, &kit, None, &Scrub::from_env(), None) {
                Ok((code, _, _)) => put(&format!("ok {code}")),
                Err(e) => put(&format!("fail {e}")),
            },
        );
    }

    // ---- the supervisor ----

    /// Requests from outside (other Launcher.exe processes, the panel's files) as events, every half second.
    fn poll_requests(&self) {
        let (tx, kit, kind) = (self.tx.clone(), self.kit.dir().to_path_buf(), Kind::of(&self.mode));
        let inbox = self.p.inbox_port;
        let data = kit.join("data");
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(500));
                let mut events = Vec::new();
                if kind == Kind::Run && take(&data.join("restart.request")).is_some() {
                    events.push(Event::Restart);
                }
                if let Some(word) = take(&data.join(format!("{}.request", kind.name()))) {
                    events.push(Event::Request(word.trim().to_owned()));
                }
                if kind == Kind::Run
                    && let Some(address) = take(&data.join("sendlogs.request"))
                {
                    Self::send_logs(&kit, inbox, &address);
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
                Event::Restart => self.restart(),
                Event::Request(word) => self.answer(&word),
            };
            if let Some(end) = end {
                self.ui.running(false);
                return end;
            }
        }
    }

    /// Stand-in and helper threads that ended by themselves (a panic, an error) are said once, in the run log; the run
    /// stops when the game cannot go on without one (the card reader, the I/O board, the scene service, the network
    /// stand-ins; 2017-18: the Aime reader, AiMeDB, the game server).  The key driver and the helpers are said and the game goes on.
    fn check_threads(&mut self) -> Option<End> {
        let mut found: Vec<(String, bool, String)> = Vec::new();
        let mut look = |name: String, essential: bool, h: Option<&mut Handle>| {
            if let Some(why) = h.and_then(Handle::died) {
                found.push((name, essential, why));
            }
        };
        for (h, s) in self.cabinet.net.iter_mut().zip(network::SERVICES) {
            look(format!("the network stand-in {}", s.1), true, Some(h));
        }
        let reader = match self.edition {
            Edition::E1011 => "the card reader stand-in",
            Edition::E1718 => "the Aime reader stand-in",
        };
        look(reader.into(), true, self.cabinet.reader.as_mut());
        look("the AiMeDB stand-in".into(), true, self.cabinet.aimedb.as_mut());
        look("the game server stand-in".into(), true, self.gamesvr.as_mut());
        look("the I/O board stand-in".into(), true, self.cabinet.board.as_mut());
        look("the key driver".into(), false, self.cabinet.keys.as_mut());
        look("the scene service".into(), true, self.scenes.as_mut());
        for (name, essential, h) in &mut self.cabinet.extra {
            look(name.clone(), *essential, Some(h));
        }
        if let Some(h) = &mut self.helper {
            found.extend(
                h.died()
                    .into_iter()
                    .map(|(n, why)| (format!("the {n} helper"), false, why)),
            );
        }
        let mut stop = None;
        for (name, essential, why) in found {
            if essential {
                self.say(&format!(
                    "{name} ended by itself ({why}) - the game cannot go on without it: stopping the run."
                ));
                stop.get_or_insert(name);
            } else {
                self.say(&format!(
                    "{name} ended by itself ({why}) - the game goes on without it."
                ));
            }
        }
        let name = stop?;
        Some(match self.finish(false, &format!("Stopped: {name} ended by itself.")) {
            End::Over(_) | End::Again => End::Over(FAILED),
        })
    }

    /// The card rule now: the refusal's words, or None to go on.
    fn refusal(&self, force: bool) -> Option<String> {
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
            self.say("The server ended - see data\\logs\\run_server.txt.");
            if self.mode == Mode::Server {
                return Some(self.finish(false, "The server has ended."));
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
        match rules::after_end(&gone, others, |n| self.read_log(n)) {
            Some(crashed) => {
                self.say(&format!(
                    "A game window went by itself, not closed (a crash - see data\\logs\\{}): the rest keeps running, \
                     the projector too.",
                    crashed.join(", ")
                ));
                None
            }
            None => {
                self.say(&format!(
                    "{} - stopping the rest.",
                    if others {
                        "A game window was closed"
                    } else {
                        "No game window is left"
                    }
                ));
                if let Some(why) = self.refusal(false) {
                    // the projector closed during a match: seat 1 plays on, watched
                    self.say(&why);
                    self.say("NOT stopped - the game is still running.");
                    return None;
                }
                Some(self.finish(true, ""))
            }
        }
    }

    /// Ends every process of the run, in order: the helper threads and the Python kids, the game programs through their debuggers, then
    /// `Job::stop` for whatever is left (its windows closed, then ended after the deadline); then the stand-ins, the
    /// key driver first.
    fn shutdown(&mut self) {
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
                self.say(&format!("  {} processes did not close in time - ended.", s.terminated));
            }
        }
        self.cabinet = Cabinet::default();
        self.scenes = None;
        self.gamesvr = None;
        self.lease = None;
    }

    /// The end of the run: everything stopped, the table kept with its club, data\running.json removed, and - when a
    /// game window ended it (`ended`) - an online run that went wrong reported.
    fn finish(&mut self, ended: bool, words: &str) -> End {
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
        self.shutdown();
        let left = self.job.active();
        self.card_steps(&["keep"]); // the cards on the table kept with their club: data\save holds the latest
        self.job.stop(STOP_DEADLINE);
        let _ = fs::remove_file(self.running_file());
        self.report_bad_run(&why);
        if left > 0 {
            self.say(&format!("Still running: {left} processes of the run."));
            return End::Over(FAILED);
        }
        self.say(&if !words.is_empty() {
            words.to_owned()
        } else if why.is_empty() {
            "The game has ended: closed by the player. Everything of the run has stopped.".to_owned()
        } else {
            format!(
                "The game has ended, and it went wrong: {}. Everything of the run has stopped.",
                why.join("; ")
            )
        });
        End::Over(OK)
    }

    /// RESTART NOW: the cabinet alone when a plain start would play the same way, else a stop (the card rule) and a
    /// plain start.
    fn restart(&mut self) -> Option<End> {
        self.say("RESTART NOW asked.");
        let panel = choices::read(&self.kit.data().join("panel.txt"));
        let fits = rules::cabinet_restart_fits(&self.mode, self.alive(Role::Seat), &panel, self.kit.game_language());
        if let Some(why) = self.refusal(false) {
            self.say(&why);
            self.say("NOT restarted - the game is still running.");
            return None;
        }
        if !fits {
            self.say(&format!(
                "Restarting WCCF {}, as the SETTINGS panel asked.",
                self.edition.name()
            ));
            self.shutdown();
            self.card_steps(&["keep"]);
            let _ = fs::remove_file(self.running_file());
            thread::sleep(Duration::from_secs(2)); // Windows frees the ports and pipes
            return Some(End::Again);
        }
        // the projector, the scene service, the seat lease and the helper threads are never touched, so the projector
        // stays in step with the server
        self.say(&format!(
            "Restarting seat {}'s cabinet only - the projector keeps running.",
            self.seat_no
        ));
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
        if let Err(Failed(why)) = self.start_cabinet() {
            self.say(&format!("FAILED: {why}"));
            self.say("Stopping what was started ...");
            return Some(match self.finish(false, "Stopped.") {
                End::Over(_) | End::Again => End::Over(FAILED),
            });
        }
        self.write_running();
        self.say("");
        self.say(&format!(
            "Seat {}'s cabinet is back; the projector was not touched.",
            self.seat_no
        ));
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
                        "check only - nothing stopped: stop would end the run's {n} processes and its stand-ins (club card: \
                     {}, session: {})",
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
                    out.push("To look at, not to click. They are hidden again at the next start.".into());
                    put(OK, &out.join("\n"));
                }
            }
            "stop" | "force" => {
                if let Some(why) = self.refusal(word == "force") {
                    self.say(&why);
                    self.say("NOT stopped - the game is still running.");
                    put(REFUSED, &format!("{why}\nNOT stopped - the game is still running."));
                    return None;
                }
                self.say("Stopping the game, as asked.");
                let end = self.finish(false, "Stopped, as asked.");
                put(if matches!(end, End::Over(OK)) { OK } else { FAILED }, "Stopped.");
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
            Mode::Server => out.push(format!("this PC is the server (TCP port {})", self.p.control_port)),
            Mode::Local => out.push("playing on this PC".into()),
        }
        let threads = [
            ("scene service", self.scenes.is_some()),
            ("game server stand-in", self.gamesvr.is_some()),
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
                .map(|(n, _)| format!("  kit   {n:<28} (in Launcher.exe)")),
        );
        for q in self.job_procs() {
            let kid = self.kids.iter().find(|(_, c)| c.id() == q.pid).map(|(r, _)| *r);
            let folder = q
                .path
                .parent()
                .and_then(Path::file_name)
                .map(|f| f.to_string_lossy().into_owned());
            out.push(match kid {
                Some(role) => format!("  kit   {role:<28} pid {}", q.pid),
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
