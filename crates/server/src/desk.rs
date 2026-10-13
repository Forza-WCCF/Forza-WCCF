//! The seat desk (`seat_desk.py`, and 2017-18's `_seat_broker.py`): hands each PC the lowest free cabinet seat 1-8,
//! keeps it while the PC's connection lives and pings, never gives a seat that is in the game (2010-11: read from
//! control's debug log), and holds the projector box out of the game until a player is in (2010-11).
//!
//! Lines in [`forza::online`]'s words: `WANT` / `WANT SEAT n` -> `SEAT n PROJECTOR p` | `TAKEN n` | `FULL` | `ERR ...`,
//! then `PING` -> `PONG`. `HOLD` -> `HOLD 1` | `HOLD 0` (2010-11 only; the box asks it).

use crate::{Edition, Handle, Log};
use forza::online::{Reply, SEATS, Want};
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime};

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
        }
    }

    fn reset(&mut self, born: Option<SystemTime>) {
        (self.born, self.pos, self.seat) = (born, 0, None);
        self.part.clear();
        self.seats.clear();
    }

    fn now(&mut self) -> BTreeMap<u32, String> {
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
    {
        let mut st = sh.state.lock().unwrap();
        let others: BTreeMap<u32, String> = if e1011 {
            sh.in_game.lock().unwrap().now()
        } else {
            BTreeMap::new()
        }
        .into_iter()
        .filter(|(_, a)| a != ip)
        .collect();
        let busy =
            |st: &State, s: u8| st.leases.contains_key(&s) || others.contains_key(&u32::from(s));
        let seat = match Want::parse(&line) {
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
                    let by = if st.leases.contains_key(&n) {
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
            thread::spawn(move || serve(&sh, conn, peer.to_string()));
        }
    }));
    Ok(Handle::new(addr, stop, true, threads, ticker))
}
