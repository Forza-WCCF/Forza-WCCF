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

mod admission;
mod cli;
pub mod rules;
mod sampler;
mod start;
mod supervise;

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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
pub use admission::{Admission, Kind, admit, any_on, ask_stop, cup_started, hold_window, is_on, request};
use admission::{answer_file, remember_cup, request_file, take, take_lock};
use cli::{addr, answers, championship_notice, now_local, reach};
pub use cli::{command, play};
pub use rules::{Mode, Words};
pub use start::start;

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

/// What a start says of a club left out ([`club::leftover`]): `home` = when the arcade's last copy came home, which
/// it did (#434: "the arcade's last copy is in ..." read as a loss for a copy that held nothing new); else where the
/// differing copy went.
fn leftover_text(kept: &Path, home: Option<i64>) -> Text {
    match home {
        Some(t) => Text::key("launcher.club_up_to_date").arg("time", crate::logs::date_time(t)),
        None => Text::key("launcher.club_kept").arg("path", kept.display()),
    }
}

/// `text` to `ui` after `lead` (an indent, a step "1/6" the window's bar reads): shown in the player's language, the
/// English for the logs.
pub fn tell(ui: &dyn Ui, lead: &str, text: &Text) {
    ui.say_logged(
        &format!("{lead}{}", text.get(ui.lang())),
        &format!("{lead}{}", text.en()),
    );
    if lead == FAIL {
        ui.cause(&text.get(ui.lang()));
    }
}

/// The lead of a step's failure line: its words are the cause the window's status line names ([`Ui::cause`]).
pub const FAIL: &str = "  FAIL  ";

/// A start refused before anything of it began: the line, and the cause for the status line.
fn refused(ui: &dyn Ui, text: &Text) {
    tell(ui, "", text);
    ui.cause(&text.get(ui.lang()));
}

/// A line of forza's setup, game text or card pictures ([`forza::Line`]) to `ui` after `lead`, as [`tell`] does.
pub fn tell_line(ui: &dyn Ui, lead: &str, line: &forza::Line) {
    ui.say_logged(
        &format!("{lead}{}", forza_ui::line(ui.lang(), line)),
        &format!("{lead}{}", forza_ui::line(forza_ui::Lang::En, line)),
    );
    if lead == FAIL {
        ui.cause(&forza_ui::line(ui.lang(), line));
    }
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
    /// Why the step failed or the start was refused, in the player's language (the window's status line names it).
    fn cause(&self, _why: &str) {}
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
    let Err(why) = pictures_held(kit, p, ui) else {
        return true;
    };
    tell(ui, "  ", &why);
    ui.cause(&why.get(ui.lang()));
    false
}

/// [`make_pictures`] inside a start, which holds the admission and is on (and goes on without them: no cause); Err
/// says why they are not in place.
fn pictures_held(kit: &Kit, p: &Params, ui: &dyn Ui) -> Result<(), Text> {
    let Some(game) = game_folder(kit, p) else {
        return Err(Text::key("launcher.run_pictures_no_game"));
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
    forza::cards::pictures::ensure(&game, ed, &kit.cache().join("cards"), pack.as_deref(), &say)
        .map_err(|e| Text::key("launcher.run_pictures_failed").arg("error", e))
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
    // after a cabinet restart the log holds the session before it too: the running one is the one to read
    re.captures(crate::logs::last_session(&text))?[1].parse().ok()
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

/// A club still marked out settled ([`club::leftover`]), in words for the player when there is something to tell.
fn leftover_note(dir: &Path) -> Option<Text> {
    match club::leftover(dir) {
        Ok(Some((kept, home))) => Some(leftover_text(&kept, home)),
        Ok(None) => None,
        Err(e) => Some(Text::key("launcher.run_marker_unread").arg("error", e)),
    }
}

/// Whether CLEAR BAD ENDINGS settles a club still marked out first (no trip on: the marker is a crash's, the club is
/// home). Not on a VS start: its bring re-joins with that marker first (#437); the bad endings then stay this once.
fn settles_at_card_steps(mode: &Mode, club_on: bool) -> bool {
    !club_on && !matches!(mode, Mode::Vs(_))
}

/// #437, before a bring: a re-join of the arcade the club is still out in takes its newer copy home, then a club left
/// out is settled. -> the guest to bring and what to tell. `Err` (no card, a re-join that could not ask): the marker
/// stays for the next start, as a bring would send the older home club over the arcade's copy.
fn before_bring(guest: Option<club::Guest>, dir: &Path, log: &Log) -> Result<(club::Guest, Vec<Text>), Text> {
    let Some(guest) = guest else {
        return Err(Text::key("launcher.run_no_card"));
    };
    let mut said = Vec::new();
    match guest.rejoin(log) {
        club::Rejoin::Pulled => said.push(Text::key("launcher.club_rejoin")),
        club::Rejoin::Skip => {}
        club::Rejoin::Failed(why) => return Err(Text::key("launcher.club_rejoin_failed").arg("error", why)),
    }
    said.extend(leftover_note(dir));
    Ok((guest, said))
}

fn file_log(path: PathBuf) -> Log {
    let t0 = Instant::now();
    // a stand-in's log is opened again inside one session (a text switch twice): no RESTART line, which only the
    // run's own log writes, when a session starts (logs::session_log)
    let file = Mutex::new(logs::open_log(&path).ok());
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

/// 2017-18's money line: the game loads the panel itself, so with no `forza_panel.dll` in the folder it says the
/// display is not loaded, not that the game starts it with a log for proof (#298).
fn money_line(panel: bool) -> Text {
    if panel {
        Text::key("launcher.run_money_by_game")
    } else {
        Text::key("launcher.run_money_off").part("why", Text::key("launcher.run_panel_missing"))
    }
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

/// The notice file the host's desk serves (`WCCF_NOTICE`): HOST ARCADE's own (`host_notice.json`,
/// [`crate::notice::write_host_notice`]); HOST LAN ARCADE's is this PC's copy of the notice as heard, never under the
/// "Arcade notice:" prefix ([`crate::notice::LAN_FILE`]), so its LAN guests keep the project's notice (#419).
fn desk_notice_file(data: &Path, lan_only: bool) -> PathBuf {
    data.join(if lan_only {
        crate::notice::LAN_FILE
    } else {
        "host_notice.json"
    })
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
        let code = include_str!("start.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default();
        let at = |s: &str| code.find(s).unwrap_or_else(|| panic!("{s} not found"));
        assert!(at("if helper_first {") < at("self.start_cabinet()?;"));
        assert!(at("self.start_helper(projector);") < at("self.start_cabinet()?;"));
        assert!(code.contains("let helper_first = self.edition == Edition::E1718;"));
    }

    /// The panel is gone (an antivirus took it): the money line says so and points at no log that will never appear.
    #[test]
    fn the_money_line_points_at_the_log_only_when_the_panel_is_there() {
        let there = super::money_line(true);
        assert!(there.en().contains("forza_panel_projector.log"), "{}", there.en());
        let gone = super::money_line(false);
        assert!(!gone.en().contains(".log"), "{}", gone.en());
        assert_eq!(
            gone.en(),
            "projector: money display not loaded (forza_panel.dll is missing)"
        );
    }

    /// HOST LAN ARCADE's desk serves its copy of the project server's notice, unprefixed, to released LAN guests (#419);
    /// HOST ARCADE's serves the host's own notice.
    #[test]
    fn a_lan_arcades_desk_serves_the_projects_notice() {
        let data = Path::new("data");
        assert_eq!(super::desk_notice_file(data, true), data.join("lan_notice.json"));
        assert_eq!(super::desk_notice_file(data, false), data.join("host_notice.json"));
    }

    /// The run's desk takes its notice file from [`super::desk_notice_file`] with the LAN-only choice (#419): a
    /// hard-coded `host_notice.json` there gave HOST LAN ARCADE's guests an empty notice.
    #[test]
    fn the_desk_notice_file_follows_the_lan_only_choice() {
        let code = include_str!("start.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default();
        assert!(code.contains("let file = desk_notice_file(&self.kit.data(), lan_only);"));
    }

    /// A start after a visit whose last copy was home says the club is as the arcade sent it; a differing copy keeps
    /// the old line with its backup (#434).
    #[test]
    fn a_leftover_copy_already_home_says_the_club_is_up_to_date() {
        let kept = Path::new(r"gamesvr\backups\club-shop-1.json");
        let home = super::leftover_text(kept, Some(1_700_000_000));
        assert!(
            matches!(home, super::Text::Key("launcher.club_up_to_date", _)),
            "{home:?}"
        );
        assert!(home.en().contains("as the arcade last sent it"), "{}", home.en());
        let differs = super::leftover_text(kept, None);
        assert!(
            matches!(differs, super::Text::Key("launcher.club_kept", _)),
            "{differs:?}"
        );
        assert!(differs.en().contains("club-shop-1.json"), "{}", differs.en());
    }

    /// A 2017-18 card step (CLEAR BAD ENDINGS) never settles a club marked out on a start that brings the club: an
    /// online (VS) start re-joins with that marker first (#437); a local one settles it, a guest's skips the step.
    #[test]
    fn clear_bad_endings_leaves_a_vs_starts_marker_to_the_rejoin() {
        use super::{Mode, settles_at_card_steps};
        assert!(settles_at_card_steps(&Mode::Local, false));
        assert!(!settles_at_card_steps(&Mode::Local, true), "a trip on");
        assert!(!settles_at_card_steps(&Mode::Vs(String::new()), false));
    }

    /// #437: no card, or an arcade that does not answer the re-join, leaves the marker for the next try; a bring then
    /// would send the older home club over the arcade's copy (a newer one would be set aside there).
    #[test]
    fn a_bring_keeps_the_marker_until_the_rejoin_could_ask() {
        let d = crate::scratch();
        let dir = d.path().join("gamesvr");
        fs::create_dir_all(&dir).unwrap();
        let out = forza::online::club::Out {
            arcade: "test arcade".into(),
            since: 1000,
            seat: 2,
            home_aime: 1,
            arcade_aime: 50,
            link: Default::default(),
            home_stamp: String::new(),
            host_stamp: String::new(),
            back_stamp: String::new(),
            back: 0,
        };
        let marker = dir.join("club_out.json");
        fs::write(&marker, serde_json::to_vec(&out).unwrap()).unwrap();
        let log: super::Log = std::sync::Arc::new(|_: &str| {});
        assert!(super::before_bring(None, &dir, &log).is_err());
        assert!(marker.exists(), "no card: the marker stays");
        // a port nothing listens on
        let desk = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let guest = forza::online::club::Guest {
            desk,
            seat: 2,
            code: "01234567890123456789".into(),
            home_aime: 1,
            arcade_aime: 50,
            dir: dir.clone(),
            arcade: "test arcade".into(),
            poll: forza::online::club::POLL,
        };
        assert!(super::before_bring(Some(guest), &dir, &log).is_err());
        assert!(marker.exists(), "the arcade did not answer: the marker stays");
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
