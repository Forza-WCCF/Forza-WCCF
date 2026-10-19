//! The seat desk (`seat_desk.py`, and 2017-18's `_seat_broker.py`): hands each PC the lowest free cabinet seat 1-8,
//! keeps it while the PC's connection lives and pings, never gives a seat that is in the game (2010-11: read from
//! control's debug log), and holds the projector box out of the game until a player is in (2010-11).
//!
//! Lines in [`forza::online`]'s words: `WANT` / `WANT SEAT n` -> `SEAT n PROJECTOR p` | `TAKEN n` | `FULL` | `ERR ...`,
//! then `PING` -> `PONG`. `RELEASE t` -> `OK` | `GONE` drops a lobby reservation. `HOLD` -> `HOLD 1` | `HOLD 0` (2010-11 only; the box asks it).

use crate::{Edition, Handle, Log};
use forza::online::lobby::{self, Lobby, Reserved, Shop, State as ShopState};
use forza::online::{Reply, SEATS, Want};
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Open connections one address may hold at once (the desk answers the rest by hanging up). A cabinet has 8 PCs;
/// the rest is room for lobby polls behind one NAT.
const MAX_PER_IP: usize = 16;

const HOLD_RULE: &str = "WCCF hold projector box";

#[derive(Clone, Debug)]
pub struct DeskConfig {
    pub edition: Edition,
    /// `WCCF_BIND`, default `0.0.0.0`.
    pub bind: String,
    /// `WCCF_BROKER_PORT`, default 20030; 0 = any (tests).
    pub port: u16,
    /// `WCCF_BROKER_SILENT`, default 75 s: a PC silent this long is gone.
    pub silent: Duration,
    /// Control's debug log (`WCCF_CONTROL_LOG`, else `_dbg_control.txt` beside the desk's log). 2010-11 only.
    pub control_log: Option<PathBuf>,
    /// `data\hold_projector.txt` (`WCCF_HOLD_FILE`). 2010-11 only.
    pub hold_file: Option<PathBuf>,
    /// `WCCF_HOLD_FW=FILE` (tests): the firewall rule's on/off is appended to FILE as `block ADDRESS` / `open`.
    pub hold_fw_test: Option<PathBuf>,
    /// How often the hold looks at the game (the Python: 1 s).
    pub hold_every: Duration,
    /// The shop's name in the lobby (`WCCF_SHOP_NAME`, `--name`), at most 24 characters.
    pub name: String,
    /// The shop's language in the lobby: `en`, `it` or `ja` (`WCCF_SHOP_LANG`); unset = not declared.
    pub lang: Option<String>,
    /// How long a `RESERVE` holds its seat without a `HEARTBEAT` (60 s; tests shorten it).
    pub reserve_lease: Duration,
    /// One `LOBBY` per address in this time, else `ERR slow` (2 s; tests set 0).
    pub lobby_gap: Duration,
    /// The `LOBBY` answer is rebuilt at most this often (1 s; tests set 0).
    pub lobby_cache: Duration,
}

impl DeskConfig {
    pub fn new(edition: Edition) -> Self {
        Self {
            edition,
            bind: "0.0.0.0".into(),
            port: forza::online::SEAT_PORT,
            silent: Duration::from_secs(75),
            control_log: None,
            hold_file: None,
            hold_fw_test: None,
            hold_every: Duration::from_secs(1),
            name: std::env::var("WCCF_SHOP_NAME").unwrap_or_else(|_| "Forza-WCCF".into()),
            lang: std::env::var("WCCF_SHOP_LANG")
                .ok()
                .filter(|l| ["en", "it", "ja"].contains(&l.as_str())),
            reserve_lease: Duration::from_secs(lobby::LEASE_SECS),
            lobby_gap: Duration::from_secs(2),
            lobby_cache: Duration::from_secs(1),
        }
    }
}

/// The seats control has a cabinet on now (0 = the projector), from its debug log. Each look reads only what control
/// wrote since the last; a new run's log (another birth time, or shorter) is read from its start; no log = nobody.
struct InGame {
    path: Option<PathBuf>,
    born: Option<SystemTime>,
    pos: u64,
    part: String,
    seats: BTreeMap<u32, String>,
    seat: Option<u32>,
    sate: Regex,
    accept: Regex,
    gone: Regex,
    step: Regex,
    /// Control's last state (0-7, see [`InGame::phase`]) and the start of its last ENTRY.
    state: Option<u8>,
    entry_start: Option<Instant>,
    /// ENTRY start to next ENTRY start. The `optionTime` line (ENTRY seconds, 120 in every observed cycle) is
    /// not read: the lobby has no field for the remaining ENTRY seconds.
    cycle: Option<Duration>,
}

impl InGame {
    fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            born: None,
            pos: 0,
            part: String::new(),
            seats: BTreeMap::new(),
            seat: None,
            sate: Regex::new(r"client\[\d+\]\s*:\s*sateID=(\d+)").unwrap(),
            accept: Regex::new(r"\+\+ACCEPT\+\+\s+'([0-9.]+):\d+'").unwrap(),
            gone: Regex::new(r"---DISCONNECT---\s*sateID=(\d+)").unwrap(),
            step: Regex::new(r"^state\s+\w+\(\d\)\s*->\s*\w+\((\d)\)").unwrap(),
            state: None,
            entry_start: None,
            cycle: None,
        }
    }

    fn reset(&mut self, born: Option<SystemTime>) {
        (self.born, self.pos, self.seat) = (born, 0, None);
        self.part.clear();
        self.seats.clear();
        (self.state, self.entry_start, self.cycle) = (None, None, None);
    }

    fn now(&mut self) -> BTreeMap<u32, String> {
        self.at(Instant::now())
    }

    /// Control's state and the seconds to its next ENTRY: `Some(0)` during ENTRY, `None` until a full cycle was
    /// seen. Times are when the desk read the lines, not the `ctime` text on them: a late look (the desk starting
    /// on a running log, a stalled poll) dates every new line to that look. A cycle read in one go has length 0
    /// and is dropped. ponytail: parse the ctime text if that ceiling ever matters.
    fn phase(&mut self, now: Instant) -> (Option<u8>, Option<u32>) {
        self.at(now);
        let next = match (self.state, self.entry_start, self.cycle) {
            (Some(1), ..) => Some(0),
            (_, Some(start), Some(cycle)) => {
                Some((start + cycle).saturating_duration_since(now).as_secs() as u32)
            }
            _ => None,
        };
        (self.state, next)
    }

    fn at(&mut self, now: Instant) -> BTreeMap<u32, String> {
        let Some(meta) = self.path.as_ref().and_then(|p| std::fs::metadata(p).ok()) else {
            self.reset(None);
            return BTreeMap::new();
        };
        let born = meta.created().ok();
        if born != self.born || meta.len() < self.pos {
            self.reset(born);
        }
        let mut data = Vec::new();
        let read = std::fs::File::open(self.path.as_ref().unwrap()).and_then(|mut f| {
            io::Seek::seek(&mut f, io::SeekFrom::Start(self.pos))?;
            f.read_to_end(&mut data)
        });
        if read.is_err() {
            return self.seats.clone();
        }
        self.pos += data.len() as u64;
        let text = format!("{}{}", self.part, String::from_utf8_lossy(&data));
        let mut lines: Vec<&str> = text.split('\n').collect();
        self.part = lines.pop().unwrap_or("").to_string(); // a line control is still writing: next time
        for line in lines {
            if let Some(m) = self.step.captures(line) {
                self.state = m[1].parse().ok();
                if self.state == Some(1) {
                    if let Some(prev) = self.entry_start {
                        self.cycle = Some(now.duration_since(prev)).filter(|c| !c.is_zero());
                    }
                    self.entry_start = Some(now);
                }
            }
            if let Some(m) = self.sate.captures(line) {
                self.seat = m[1].parse().ok();
            }
            if let (Some(m), Some(seat)) = (self.accept.captures(line), self.seat) {
                self.seats.insert(seat, m[1].to_string());
            }
            if let Some(m) = self.gone.captures(line)
                && let Ok(s) = m[1].parse()
            {
                self.seats.remove(&s);
            }
        }
        self.seats.clone()
    }
}

/// The address in the hold file (its first word, four numbers 0-255), or `None`.
pub fn read_hold_ip(path: &Path) -> Option<String> {
    let text = String::from_utf8_lossy(&std::fs::read(path).ok()?).into_owned();
    let ip = text.split_whitespace().next()?;
    let octet = |p: &str| {
        let t = p.trim_start_matches('0');
        !p.is_empty()
            && p.bytes().all(|b| b.is_ascii_digit())
            && t.len() <= 3
            && t.parse::<u16>().unwrap_or(0) < 256
    };
    let parts: Vec<&str> = ip.split('.').collect();
    (parts.len() == 4 && parts.iter().all(|p| octet(p))).then(|| ip.to_string())
}

#[derive(Default)]
struct State {
    leases: HashMap<u8, (String, u64)>,
    /// `RESERVE`d seats: seat -> (ticket, lease end). A seat in here is busy until claimed or expired.
    reserved: HashMap<u8, (String, Instant)>,
    next_id: u64,
    /// 2017-18: the seat whose PC shows the projector.
    projector: Option<u8>,
}

struct Shared {
    cfg: DeskConfig,
    log: Log,
    state: Mutex<State>,
    in_game: Mutex<InGame>,
    held: AtomicBool,
    /// This run's shop id in the lobby: random, never stored.
    shop_id: String,
    /// Last `LOBBY` per address, and the cached answer with its build time.
    lobby_seen: Mutex<HashMap<String, Instant>>,
    lobby_cached: Mutex<Option<(Instant, String)>>,
    /// Open connections per address ([`MAX_PER_IP`]).
    open: Mutex<HashMap<String, usize>>,
}

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("the OS has randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Drops the leases that ran out.
fn expire(st: &mut State, now: Instant) {
    st.reserved.retain(|_, (_, end)| *end > now);
}

/// The firewall rule on (block `ip` on TCP 20002) or off; never two of it.
fn firewall(cfg: &DeskConfig, ip: &str, block: bool) -> bool {
    if let Some(file) = &cfg.hold_fw_test {
        let line = if block {
            format!("block {ip}\n")
        } else {
            "open\n".into()
        };
        return std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .and_then(|mut f| f.write_all(line.as_bytes()))
            .is_ok();
    }
    netsh(ip, block)
}

#[cfg(windows)]
fn netsh(ip: &str, block: bool) -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let root = std::env::var_os("SystemRoot").unwrap_or(r"C:\Windows".into());
    let run = |args: &[&str]| {
        // ponytail: no timeout (the Python had 30 s); a hung netsh stalls the hold, add one if it ever does
        Command::new(Path::new(&root).join(r"System32\netsh.exe"))
            .args(["advfirewall", "firewall"])
            .args(args)
            .creation_flags(0x0800_0000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    };
    let name = format!("name={HOLD_RULE}");
    if run(&["delete", "rule", &name]).is_err() {
        return false;
    }
    !block
        || run(&[
            "add",
            "rule",
            &name,
            "dir=in",
            "action=block",
            "protocol=TCP",
            "localport=20002",
            &format!("remoteip={ip}"),
        ])
        .is_ok_and(|s| s.success())
}

/// There is no firewall rule to set off Windows (the Python's `netsh` call failed there as well).
#[cfg(not(windows))]
fn netsh(_ip: &str, _block: bool) -> bool {
    let _ = HOLD_RULE;
    false
}

/// The projector box's hold: held while no player is in the game and the box is not in it.
struct Hold {
    ip: String,
    held: Option<bool>,
    fails: u32,
}

impl Hold {
    fn tick(&mut self, sh: &Shared) {
        let game = sh.in_game.lock().unwrap().now();
        let player = game.keys().any(|&s| s >= 1);
        let want = !player && game.get(&0) != Some(&self.ip);
        if Some(want) == self.held {
            return;
        }
        if !firewall(&sh.cfg, &self.ip, want) {
            self.fails += 1;
            if matches!(self.fails, 1 | 60) || self.fails.is_multiple_of(600) {
                (sh.log)(&format!(
                    "projector box {}: the firewall rule could not be {} (try {}) - is this the server task (SYSTEM)?",
                    self.ip,
                    if want { "set" } else { "lifted" },
                    self.fails
                ));
            }
            return;
        }
        (self.held, self.fails) = (Some(want), 0);
        sh.held.store(want, Ordering::Relaxed);
        (sh.log)(&format!(
            "projector box {}: {}",
            self.ip,
            if want {
                "held - it joins the game once a player is in"
            } else if player {
                "let in - a player is in the game"
            } else {
                "in the game - not held"
            }
        ));
    }
}

/// One line from the PC without its end; errors when the PC is gone or the line is too long.
struct Lines {
    conn: TcpStream,
    buf: Vec<u8>,
}

impl Lines {
    fn read(&mut self) -> io::Result<String> {
        while !self.buf.contains(&b'\n') {
            if self.buf.len() > 200 {
                return Err(io::Error::other("line too long"));
            }
            let mut b = [0u8; 256];
            match self.conn.read(&mut b)? {
                0 => return Err(io::Error::other("closed")),
                n => self.buf.extend_from_slice(&b[..n]),
            }
        }
        let at = self.buf.iter().position(|&b| b == b'\n').unwrap();
        let line: Vec<u8> = self.buf.drain(..=at).collect();
        Ok(String::from_utf8_lossy(line.trim_ascii()).into_owned())
    }
}

fn taken(st: &State) -> String {
    let mut seats: Vec<u8> = st.leases.keys().copied().collect();
    seats.sort_unstable();
    if seats.is_empty() {
        return "none".into();
    }
    seats
        .iter()
        .map(|s| {
            format!(
                "{s}{}",
                if st.projector == Some(*s) {
                    " (projector)"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn serve(sh: &Shared, conn: TcpStream, peer: String) {
    let _ = conn.set_read_timeout(Some(sh.cfg.silent));
    let _ = conn.set_write_timeout(Some(sh.cfg.silent));
    let mut lease: Option<(u8, u64)> = None;
    let _ = session(sh, conn, &peer, &mut lease);
    if let Some((seat, id)) = lease {
        let mut st = sh.state.lock().unwrap();
        if st.leases.get(&seat).is_some_and(|l| l.1 == id) {
            st.leases.remove(&seat);
            if st.projector == Some(seat) {
                st.projector = None;
            }
            (sh.log)(&format!(
                "{peer}: seat {seat} free again - taken now: {}",
                taken(&st)
            ));
        }
    }
}

fn session(
    sh: &Shared,
    conn: TcpStream,
    peer: &str,
    lease: &mut Option<(u8, u64)>,
) -> io::Result<()> {
    let log = &sh.log;
    let e1011 = sh.cfg.edition == Edition::E1011;
    let mut lines = Lines {
        conn: conn.try_clone()?,
        buf: Vec::new(),
    };
    let mut out = conn;
    let line = lines.read()?;
    let ip = peer.rsplit_once(':').map_or(peer, |p| p.0);
    if e1011 && line.split_whitespace().eq(["HOLD"]) {
        // the projector box: held? (no seat taken)
        let held = sh.held.load(Ordering::Relaxed);
        return out.write_all(if held { b"HOLD 1\n" } else { b"HOLD 0\n" });
    }
    match line.split_whitespace().collect::<Vec<_>>()[..] {
        ["LOBBY", edition] => return lobby_answer(sh, ip, edition, &mut out),
        ["RESERVE", id] => return reserve(sh, id, None, &mut out),
        ["RESERVE", id, n] => {
            return match n.parse().ok().filter(|n| SEATS.contains(n)) {
                Some(n) => reserve(sh, id, Some(n), &mut out),
                None => out.write_all(b"ERR seat\n"),
            };
        }
        ["HEARTBEAT", t] => return heartbeat(sh, t, &mut out),
        ["RELEASE", t] => return release(sh, t, &mut out),
        _ => {}
    }
    // `WANT ... TICKET t` claims the seat `RESERVE` held
    let (line, ticket) = match line.split_whitespace().collect::<Vec<_>>()[..] {
        [.., "TICKET", t] => (
            line.rsplitn(3, char::is_whitespace)
                .last()
                .unwrap_or("")
                .to_string(),
            Some(t.to_string()),
        ),
        _ => (line, None),
    };
    {
        let mut st = sh.state.lock().unwrap();
        expire(&mut st, Instant::now());
        let others: BTreeMap<u32, String> = if e1011 {
            sh.in_game.lock().unwrap().now()
        } else {
            BTreeMap::new()
        }
        .into_iter()
        .filter(|(_, a)| a != ip)
        .collect();
        let busy = |st: &State, s: u8| {
            st.leases.contains_key(&s)
                || st.reserved.contains_key(&s)
                || others.contains_key(&u32::from(s))
        };
        let want = Want::parse(&line);
        let seat = if let Some(t) = ticket {
            let held = st
                .reserved
                .iter()
                .find(|(_, (k, _))| *k == t)
                .map(|(s, _)| *s);
            let (Some(want), Some(seat)) = (want, held) else {
                return out.write_all(b"ERR ticket\n");
            };
            if let Want::Seat(n) = want
                && n != seat
            {
                return out.write_all(Reply::Taken(n).line().as_bytes());
            }
            st.reserved.remove(&seat);
            // the seat may have been taken since RESERVE: the reservation is gone, the client asks again
            if st.leases.contains_key(&seat) || others.contains_key(&u32::from(seat)) {
                return out.write_all(Reply::Taken(seat).line().as_bytes());
            }
            seat
        } else {
            match want {
                Some(Want::Any) => {
                    let Some(seat) = SEATS.clone().find(|&s| !busy(&st, s)) else {
                        out.write_all(Reply::Full.line().as_bytes())?;
                        log(&format!("{peer}: no seat - all 8 taken"));
                        return Ok(());
                    };
                    let skipped: Vec<u8> = SEATS
                        .clone()
                        .filter(|&s| {
                            s < seat
                                && !st.leases.contains_key(&s)
                                && others.contains_key(&u32::from(s))
                        })
                        .collect();
                    if !skipped.is_empty() {
                        let seats = skipped
                            .iter()
                            .map(u8::to_string)
                            .collect::<Vec<_>>()
                            .join(", ");
                        let addrs = skipped
                            .iter()
                            .map(|s| others[&u32::from(*s)].as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        log(&format!(
                            "{peer}: seat {seats} skipped - in the game ({addrs}) with no lease"
                        ));
                    }
                    seat
                }
                Some(Want::Seat(n)) => {
                    if busy(&st, n) {
                        out.write_all(Reply::Taken(n).line().as_bytes())?;
                        let by = if st.leases.contains_key(&n) || st.reserved.contains_key(&n) {
                            String::new()
                        } else {
                            format!(" (in the game by {})", others[&u32::from(n)])
                        };
                        log(&format!("{peer}: seat {n} refused - taken{by}"));
                        return Ok(());
                    }
                    n
                }
                None => {
                    return out.write_all(
                        Reply::Err("say WANT or WANT SEAT n".into())
                            .line()
                            .as_bytes(),
                    );
                }
            }
        };
        st.next_id += 1;
        let id = st.next_id;
        st.leases.insert(seat, (peer.to_string(), id));
        *lease = Some((seat, id));
        let show = !e1011 && st.projector.is_none();
        if show {
            st.projector = Some(seat);
        }
        out.write_all(
            Reply::Seat {
                seat,
                projector: show,
            }
            .line()
            .as_bytes(),
        )?;
        log(&format!(
            "{peer}: seat {seat}{} - taken now: {}",
            if show { " + the projector" } else { "" },
            taken(&st)
        ));
    }
    loop {
        if lines.read()? == "PING" {
            out.write_all(b"PONG\n")?;
        }
    }
}

/// Seats in use: led by a lease, held by a reservation, or in the game (2010-11).
fn used(sh: &Shared, st: &State) -> Vec<u8> {
    let game = if sh.cfg.edition == Edition::E1011 {
        sh.in_game.lock().unwrap().now()
    } else {
        BTreeMap::new()
    };
    SEATS
        .filter(|s| {
            st.leases.contains_key(s)
                || st.reserved.contains_key(s)
                || game.contains_key(&u32::from(*s))
        })
        .collect()
}

/// `LOBBY edition`: this desk's shop as the one JSON line, then the hang-up. The answer holds no address, card
/// number, Aime id or name of a person; ids are random per run.
fn lobby_answer(sh: &Shared, ip: &str, edition: &str, out: &mut TcpStream) -> io::Result<()> {
    if edition != sh.cfg.edition.name() {
        return out.write_all(b"ERR edition\n");
    }
    let now = Instant::now();
    {
        let mut seen = sh.lobby_seen.lock().unwrap();
        seen.retain(|_, t| now.duration_since(*t) < sh.cfg.lobby_gap.max(Duration::from_secs(60)));
        if seen
            .insert(ip.to_string(), now)
            .is_some_and(|t| now.duration_since(t) < sh.cfg.lobby_gap)
        {
            return out.write_all(b"ERR slow\n");
        }
    }
    let mut cache = sh.lobby_cached.lock().unwrap();
    if let Some((at, line)) = cache.as_ref()
        && now.duration_since(*at) < sh.cfg.lobby_cache
    {
        return out.write_all(line.as_bytes());
    }
    let seats = {
        let mut st = sh.state.lock().unwrap();
        expire(&mut st, now);
        used(sh, &st).len() as u8
    };
    let (phase, next_entry) = sh.in_game.lock().unwrap().phase(now);
    let shop = Shop {
        id: sh.shop_id.clone(),
        name: sh.cfg.name.chars().take(24).collect(),
        seats,
        max: SEATS.count() as u8,
        // Control's states: 0 boot, 1 ENTRY (seats open), 2 CR_DATA_WAIT, 3 MATCHING_WAIT, 4 MATCHING, 5 GAME,
        // 6 GAME_RESULT_WAIT, 7 GAME_RESULT. From 2 to 6 a PC that connects waits for the next ENTRY; in 7 and 1 it
        // can sit down, so those stay OPEN.
        state: if seats >= 8 {
            ShopState::Full
        } else if phase.is_some_and(|p| (2..=6).contains(&p)) {
            ShopState::InMatch
        } else {
            ShopState::Open
        },
        next_entry,
        lang: sh.cfg.lang.clone(),
    };
    let answer = Lobby {
        v: lobby::VERSION,
        edition: sh.cfg.edition.name().into(),
        at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        shops: vec![shop],
        vs: vec![],
    }
    .line();
    debug_assert!(answer.len() <= lobby::MAX_ANSWER);
    *cache = Some((now, answer.clone()));
    out.write_all(answer.as_bytes())
}

/// `RESERVE id [n]`: a ticket for the lowest free seat (or seat n) for the lease, `FULL`, `TAKEN` (n is busy) or
/// `CLOSED` (not this desk's shop).
fn reserve(sh: &Shared, id: &str, seat: Option<u8>, out: &mut TcpStream) -> io::Result<()> {
    let mut st = sh.state.lock().unwrap();
    let now = Instant::now();
    expire(&mut st, now);
    let reply = if id != sh.shop_id {
        Reserved::Closed
    } else {
        let busy = used(sh, &st);
        match seat {
            Some(n) if busy.contains(&n) => Reserved::Taken,
            Some(n) => take(&mut st, n, now, sh.cfg.reserve_lease),
            None => match SEATS.clone().find(|s| !busy.contains(s)) {
                Some(n) => take(&mut st, n, now, sh.cfg.reserve_lease),
                None => Reserved::Full,
            },
        }
    };
    drop(st);
    out.write_all(reply.line().as_bytes())
}

fn take(st: &mut State, seat: u8, now: Instant, lease: Duration) -> Reserved {
    let ticket = random_hex(8);
    st.reserved.insert(seat, (ticket.clone(), now + lease));
    Reserved::Ticket { ticket, seat }
}

/// `HEARTBEAT t` -> `OK` (lease extended) or `GONE`.
fn heartbeat(sh: &Shared, t: &str, out: &mut TcpStream) -> io::Result<()> {
    let mut st = sh.state.lock().unwrap();
    let now = Instant::now();
    expire(&mut st, now);
    let ok = st
        .reserved
        .values_mut()
        .find(|(k, _)| k == t)
        .map(|(_, end)| *end = now + sh.cfg.reserve_lease);
    drop(st);
    out.write_all(if ok.is_some() { b"OK\n" } else { b"GONE\n" })
}

/// `RELEASE t` -> `OK` (the seat is free at once) or `GONE`. The ticket is the secret, so only its holder gets here.
fn release(sh: &Shared, t: &str, out: &mut TcpStream) -> io::Result<()> {
    let mut st = sh.state.lock().unwrap();
    expire(&mut st, Instant::now());
    let had = st.reserved.len();
    st.reserved.retain(|_, (k, _)| k != t);
    let ok = st.reserved.len() < had;
    drop(st);
    out.write_all(if ok { b"OK\n" } else { b"GONE\n" })
}

/// Starts the desk. Err(io error) when the port cannot be bound (the caller logs "cannot listen ...").
pub fn start(cfg: DeskConfig, log: Log) -> io::Result<Handle> {
    let listener = TcpListener::bind((cfg.bind.as_str(), cfg.port))?;
    let addr = listener.local_addr()?;
    let e1011 = cfg.edition == Edition::E1011;
    let sh = Arc::new(Shared {
        in_game: Mutex::new(InGame::new(cfg.control_log.clone().filter(|_| e1011))),
        cfg,
        log: log.clone(),
        state: Mutex::default(),
        held: AtomicBool::new(false),
        shop_id: random_hex(4),
        lobby_seen: Mutex::default(),
        lobby_cached: Mutex::default(),
        open: Mutex::default(),
    });
    log(&format!(
        "seat broker listening on TCP {}: seats 1-8, {}",
        addr.port(),
        if e1011 {
            "no projector for the PCs"
        } else {
            "the first PC also shows the projector"
        }
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = vec![];
    let mut ticker = None;
    let hold_ip = sh
        .cfg
        .hold_file
        .as_deref()
        .filter(|_| e1011)
        .and_then(read_hold_ip);
    if let Some(ip) = hold_ip {
        let mut hold = Hold {
            ip,
            held: None,
            fails: 0,
        };
        hold.tick(&sh); // at once: the box must not be the first in
        let (tx, rx) = mpsc::channel::<()>();
        ticker = Some(tx);
        let (sh, every) = (sh.clone(), sh.cfg.hold_every);
        threads.push(thread::spawn(move || {
            while rx.recv_timeout(every) == Err(mpsc::RecvTimeoutError::Timeout) {
                hold.tick(&sh);
            }
        }));
    }
    let stopped = stop.clone();
    threads.push(thread::spawn(move || {
        for conn in listener.incoming() {
            if stopped.load(Ordering::Relaxed) {
                break;
            }
            let Ok(conn) = conn else { continue };
            let Ok(peer) = conn.peer_addr() else { continue };
            let sh = sh.clone();
            let ip = peer.ip().to_string();
            {
                let mut open = sh.open.lock().unwrap();
                let n = open.entry(ip.clone()).or_default();
                if *n >= MAX_PER_IP {
                    continue; // dropped: hangs up
                }
                *n += 1;
            }
            thread::spawn(move || {
                serve(&sh, conn, peer.to_string());
                let mut open = sh.open.lock().unwrap();
                if let Some(n) = open.get_mut(&ip) {
                    *n -= 1;
                    if *n == 0 {
                        open.remove(&ip);
                    }
                }
            });
        }
    }));
    Ok(Handle::new(addr, stop, true, threads, ticker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(what: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forza_deskunit_{what}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join("game.log")
    }

    fn step(from: &str, to: &str) -> String {
        format!("state  {from} -> {to} :whatever\n")
    }

    #[test]
    fn lobby_phase_follows_control_states() {
        let path = tmp("lobby_phase");
        let mut log = std::fs::File::create(&path).unwrap();
        let mut g = InGame::new(Some(path.clone()));
        let t0 = Instant::now();
        assert_eq!(g.phase(t0), (None, None));

        write!(
            log,
            "noise\nstate garbage\nstate  A(x) -> B(y)\n{}",
            step("PRJ_BOOT_WAIT(0)", "ENTRY(1)")
        )
        .unwrap();
        assert_eq!(g.phase(t0), (Some(1), Some(0)));
        write!(log, "{}", step("ENTRY(1)", "CR_DATA_WAIT(2)")).unwrap();
        assert_eq!(g.phase(t0 + Duration::from_secs(120)), (Some(2), None));
        for (to, n) in [
            ("MATCHING_WAIT(3)", 3),
            ("MATCHING(4)", 4),
            ("GAME(5)", 5),
            ("GAME_RESULT_WAIT(6)", 6),
        ] {
            write!(log, "{}", step("X(0)", to)).unwrap();
            assert_eq!(g.phase(t0 + Duration::from_secs(130)).0, Some(n));
        }
        write!(log, "{}", step("GAME_RESULT_WAIT(6)", "GAME_RESULT(7)")).unwrap();
        assert_eq!(g.phase(t0 + Duration::from_secs(300)).0, Some(7));

        // second ENTRY 400 s after the first: cycle 400 s
        write!(log, "{}partial", step("GAME_RESULT(7)", "ENTRY(1)")).unwrap();
        assert_eq!(g.phase(t0 + Duration::from_secs(400)), (Some(1), Some(0)));
        write!(log, " line\n{}", step("ENTRY(1)", "CR_DATA_WAIT(2)")).unwrap();
        assert_eq!(g.phase(t0 + Duration::from_secs(520)), (Some(2), Some(280)));
        assert_eq!(g.phase(t0 + Duration::from_secs(900)), (Some(2), Some(0))); // overdue: never negative
    }

    #[test]
    fn restart_log_forgets_the_old_run() {
        let path = tmp("restart_log");
        std::fs::write(&path, step("ENTRY(1)", "GAME(5)") + &"x".repeat(200) + "\n").unwrap();
        let mut g = InGame::new(Some(path.clone()));
        assert_eq!(g.phase(Instant::now()).0, Some(5));
        std::fs::write(&path, step("PRJ_BOOT_WAIT(0)", "ENTRY(1)")).unwrap(); // shorter: a new run
        assert_eq!(g.phase(Instant::now()), (Some(1), Some(0)));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(g.phase(Instant::now()), (None, None));
    }

    #[test]
    fn a_backlog_read_in_one_go_gives_no_cycle() {
        let path = tmp("backlog");
        std::fs::write(
            &path,
            step("A(0)", "ENTRY(1)") + &step("ENTRY(1)", "GAME(5)") + &step("GAME(5)", "ENTRY(1)"),
        )
        .unwrap();
        assert_eq!(
            InGame::new(Some(path)).phase(Instant::now()),
            (Some(1), Some(0))
        );
    }
}
