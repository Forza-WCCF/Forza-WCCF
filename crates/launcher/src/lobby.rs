//! The Launcher's online lobby (ROADMAP, "Online lobby view", L1): a polling thread that asks each server in the
//! `servers` setting for its `LOBBY` every [`EVERY`], the [`Book`] that turns the answers into the rows
//! `forza_ui::lobby` draws, and the reserve and heartbeat workers behind JOIN.  The window owns the threads' handles;
//! dropping a [`Poller`] or a [`Beat`] ends its thread.

use crate::Edition;
use forza::online::SEAT_PORT;
use forza::online::lobby::{self, Created, Lobby, Peers, Reserved, State, VsRoom};
use forza::service::Log;
use forza_ui::Lang;
use forza_ui::lobby::{LobbyView, Row, RowState};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError, mpsc::Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// Seconds between two `LOBBY` asks of one server.
pub const EVERY: Duration = Duration::from_secs(5);
/// A server that missed this many polls in a row shows NO ANSWER.
pub const MISSES: u32 = 2;
/// The desk drops a seat this long after its player went quiet (`seat.rs`): a rejoin is offered within it.
pub const REJOIN_SECS: u64 = 75;

/// Called from the workers when something arrived (the window's `request_repaint`).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// What the workers say.
#[derive(Debug)]
pub enum Msg {
    /// One poll's outcome; the ping is the TCP connect time.
    Poll {
        server: String,
        ping_ms: Option<u32>,
        result: Result<Lobby, String>,
    },
    /// The desk's answer to a JOIN (or a rejoin).
    Reserved {
        server: String,
        id: String,
        name: String,
        result: Result<Reserved, String>,
    },
    /// The desk no longer holds the seat for this ticket.
    Gone { ticket: String },
    /// The desk's answer to HOST VS (`CREATE vs`) or QUICK MATCH on VS ROOMS (`CREATE vs open`).
    Created {
        server: String,
        result: Result<Created, String>,
    },
    /// The desk's answer to READY: false when the room or the seat is gone.
    Readied { id: String, result: Result<bool, String> },
    /// A typed shop code's outcome ([`join_code`]).
    Coded {
        server: String,
        code: String,
        result: Result<CodeJoin, String>,
    },
}

// The project's public server; the `servers` setting, "Direct address" and `WCCF_LOBBY` (host[:port]) override it.
/// The project's lobby server for 2010-11 (its seat desk on the usual port, [`SEAT_PORT`]).
pub const PROJECT_SERVER_1011: &str = "lobby.wccf.it:20030";
/// The project's lobby server for 2017-18 (its desk beside the game server on 20060).
pub const PROJECT_SERVER_1718: &str = "lobby.wccf.it:20032";

/// Where 2017-18 shop rooms are made and joined: the desk beside the club server's game server (the project server's
/// desk port on that host), or the project's server ([`project_server`]) when no club server is set. Never this PC's
/// own shop desk: the host and its guests must meet on the same public desk.
pub fn shop_lobby(club_server: &str) -> String {
    let project = project_server(Edition::E1718);
    let port = crate::choices::host_port(&project, SEAT_PORT).map_or(SEAT_PORT, |h| h.1);
    crate::choices::parse_club_server(club_server).map_or(project, |(h, _)| format!("{h}:{port}"))
}

/// The default server: `WCCF_LOBBY` when it is a valid `host[:port]` (the project's `default` desk port if none),
/// else `default`.
fn default_server(env: Option<String>, default: &str) -> String {
    let port = crate::choices::host_port(default, SEAT_PORT).map_or(SEAT_PORT, |h| h.1);
    env.and_then(|v| crate::choices::host_port(&v, port))
        .map_or_else(|| default.to_owned(), |(h, p)| format!("{h}:{p}"))
}

/// The project's server for `edition`, or `WCCF_LOBBY`'s.
pub fn project_server(edition: Edition) -> String {
    default_server(
        std::env::var("WCCF_LOBBY").ok(),
        match edition {
            Edition::E1011 => PROJECT_SERVER_1011,
            Edition::E1718 => PROJECT_SERVER_1718,
        },
    )
}

/// The `servers` setting the lobby asks; none saved yet: the address the player had before (if any), then the
/// project's server for `edition`.
pub fn servers(settings: &Path, address: &str, edition: Edition) -> Vec<String> {
    let saved = crate::choices::setting_json(settings, "servers").and_then(|v| {
        v.as_array().map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
    });
    if let Some(s) = saved.filter(|s| !s.is_empty()) {
        return s;
    }
    let project = project_server(edition);
    let mut list: Vec<String> = Some(address.trim())
        .filter(|a| valid_server(a))
        .map(str::to_owned)
        .into_iter()
        .collect();
    if !list.contains(&project) {
        list.push(project);
    }
    list
}

/// A `servers` entry: `host` or `host:port`, the host a name or an IPv4 address (the seat desk's port by default).
pub fn valid_server(server: &str) -> bool {
    crate::choices::host_port(server, SEAT_PORT).is_some()
}

/// A `servers` entry as a socket address; a name is resolved (it may block: call it on a worker).
pub fn socket(server: &str) -> Option<SocketAddr> {
    crate::choices::resolve(server, SEAT_PORT)
}

/// `CLOSE id ticket` on its own thread (the host leaves its VS room; best effort like [`release`]).
pub fn close(server: &str, id: &str, ticket: &str) -> thread::JoinHandle<()> {
    let (server, id, ticket) = (server.to_owned(), id.to_owned(), ticket.to_owned());
    thread::spawn(move || {
        if let Some(a) = socket(&server) {
            let _ = lobby::close(a, &id, &ticket);
        }
    })
}

/// `RELEASE ticket` on its own thread (best effort: the lease lapses by itself if this fails). Join the handle
/// when the process is about to exit.
pub fn release(server: &str, ticket: &str) -> thread::JoinHandle<()> {
    let (server, ticket) = (server.to_owned(), ticket.to_owned());
    thread::spawn(move || {
        if let Some(a) = socket(&server) {
            let _ = lobby::release(a, &ticket);
        }
    })
}

/// The polling thread. Alive only while the lobby is shown: drop it and the thread ends at its next wake.
pub struct Poller {
    pub rx: Receiver<Msg>,
    tx: Sender<Msg>,
    nudge: Sender<()>,
}

impl Poller {
    /// Polls every server in `servers` for `edition` ("2010-11"), the first poll at once, then every `every`.
    /// ponytail: one thread, servers one after the other (3 s each when dead); a thread per server if the list grows.
    pub fn start(servers: Vec<String>, edition: &'static str, every: Duration, wake: Wake) -> Poller {
        let (tx, rx) = mpsc::channel();
        let (nudge, nudged) = mpsc::channel::<()>();
        let out = tx.clone();
        thread::spawn(move || {
            loop {
                for server in &servers {
                    let (ping_ms, result) = match socket(server) {
                        Some(a) => (ping(a), lobby::fetch(a, edition)),
                        None => (None, Err("not an address".into())),
                    };
                    let msg = Msg::Poll {
                        server: server.clone(),
                        ping_ms,
                        result,
                    };
                    if out.send(msg).is_err() {
                        return;
                    }
                    wake();
                }
                match nudged.recv_timeout(every) {
                    Err(RecvTimeoutError::Disconnected) => return,
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                }
            }
        });
        Poller { rx, tx, nudge }
    }

    /// Poll now (F5, a refused JOIN).
    pub fn refresh(&self) {
        let _ = self.nudge.send(());
    }

    /// One call to `server` on a worker; `msg` turns the address (or why there is none) into the answer's message.
    fn call(&self, server: &str, wake: Wake, msg: impl FnOnce(Result<SocketAddr, String>) -> Msg + Send + 'static) {
        let (server, out) = (server.to_owned(), self.tx.clone());
        thread::spawn(move || {
            let a = socket(&server).ok_or_else(|| "not an address".to_owned());
            let _ = out.send(msg(a));
            wake();
        });
    }

    /// JOIN: `RESERVE id` (or `RESERVE id n` for a rejoin) on a worker; the answer comes back as [`Msg::Reserved`].
    pub fn reserve(&self, server: &str, id: &str, name: &str, seat: Option<u8>, wake: Wake) {
        let (server2, id, name) = (server.to_owned(), id.to_owned(), name.to_owned());
        self.call(server, wake, move |a| Msg::Reserved {
            result: a.and_then(|a| lobby::reserve_seat(a, &id, seat)),
            server: server2,
            id,
            name,
        });
    }

    /// HOST VS (`open` false) or QUICK MATCH on VS ROOMS (`open` true); the answer comes back as [`Msg::Created`].
    pub fn create(&self, server: &str, open: bool, wake: Wake) {
        let server2 = server.to_owned();
        self.call(server, wake, move |a| Msg::Created {
            result: a.and_then(|a| lobby::create(a, open)),
            server: server2,
        });
    }

    /// READY in room `id`; the answer comes back as [`Msg::Readied`].
    pub fn ready(&self, server: &str, id: &str, ticket: &str, wake: Wake) {
        let (id, ticket) = (id.to_owned(), ticket.to_owned());
        self.call(server, wake, move |a| Msg::Readied {
            result: a.and_then(|a| lobby::ready(a, &id, &ticket)),
            id,
        });
    }

    /// Keeps a reserved seat: `HEARTBEAT ticket` every `every` (`lobby::HEARTBEAT_EVERY`) until the [`Beat`] drops.
    pub fn beat(&self, server: &str, ticket: &str, every: Duration, wake: Wake) -> Beat {
        let (out, t) = (self.tx.clone(), ticket.to_owned());
        beat(server, ticket, every, move || {
            let _ = out.send(Msg::Gone { ticket: t });
            wake();
        })
    }

    /// A shop room's code (2017-18), typed, or a listed room's id: `JOIN`, then `SHOP t own` ([`join_code`]) on a
    /// worker; the answer comes back as [`Msg::Coded`].
    pub fn join_code(&self, server: &str, code: &str, own: &str, wake: Wake) {
        let (server2, code, own) = (server.to_owned(), code.to_owned(), own.to_owned());
        self.call(server, wake, move |a| Msg::Coded {
            result: a.and_then(|a| join_code(a, &code, &own)),
            server: server2,
            code,
        });
    }
}

/// `HEARTBEAT ticket` every `every` until the [`Beat`] drops; `gone` runs once when the desk no longer holds it.
fn beat(server: &str, ticket: &str, every: Duration, gone: impl FnOnce() + Send + 'static) -> Beat {
    let stop = Arc::new(AtomicBool::new(false));
    let (flag, server, ticket) = (stop.clone(), server.to_owned(), ticket.to_owned());
    thread::spawn(move || {
        let Some(a) = socket(&server) else {
            // no desk to keep the seat with: as good as gone (the lease lapses)
            if !flag.load(Ordering::Relaxed) {
                gone();
            }
            return;
        };
        loop {
            let end = Instant::now() + every;
            while Instant::now() < end {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(20).min(end - Instant::now()));
            }
            // an I/O error is not "gone": the next beat tries again inside the 60 s lease
            if lobby::heartbeat(a, &ticket) == Ok(false) {
                if !flag.load(Ordering::Relaxed) {
                    gone();
                }
                return;
            }
        }
    });
    Beat(stop)
}

/// The key whose id a guest tells its shop room (`SHOP t own`) and dials the host with (data\shop_join.key).
pub const SHOP_JOIN_KEY: &str = "shop_join.key";

/// A typed shop code's outcome (2017-18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeJoin {
    /// A guest seat: the ticket to keep alive while the run lasts, and the host's tunnel id to dial.
    In { ticket: String, host: String },
    /// No room has that code.
    NoRoom,
    /// The room has no free seat.
    Full,
    /// The room closed between `JOIN` and `SHOP`.
    Gone,
}

/// A guest's way into a shop room: `JOIN CODE`, then `SHOP t own` (`own` = this PC's join key id, which the host's
/// tunnel then admits; it must be told before the dial).
pub fn join_code(a: SocketAddr, code: &str, own: &str) -> Result<CodeJoin, String> {
    let ticket = match lobby::join_shop(a, code, None)? {
        Reserved::Ticket { ticket, .. } => ticket,
        Reserved::Closed => return Ok(CodeJoin::NoRoom),
        Reserved::Full | Reserved::Taken => return Ok(CodeJoin::Full),
    };
    match lobby::shop_peers(a, &ticket, Some(own)) {
        Ok(Some(Peers::Host(host))) => Ok(CodeJoin::In { ticket, host }),
        Ok(None) => Ok(CodeJoin::Gone),
        other => {
            let _ = lobby::release(a, &ticket);
            Err(match other {
                Err(e) => e,
                _ => "not a guest answer".into(),
            })
        }
    }
}

/// The desk replaces a host's room from before (a crash) once it has had no `HEARTBEAT` this long (`DeskConfig::host_stale`,
/// 15 s by default); a host that gets `ERR host` waits this much more than that, then asks once more. A desk set to a
/// longer stale time answers `ERR host` again, and the host says so.
pub const DESK_HOST_STALE: Duration = Duration::from_secs(15);
/// How often the host asks the desk for its room's guests without a guest waiting (a guest at the door asks at once).
const GUESTS_EVERY: Duration = Duration::from_secs(10);
/// How long a guest at the door waits for the desk's answer (up to 9 s of time-outs on a slow desk, after the
/// one-second ask gap) before it is told to come back.
const ADMIT_WAIT: Duration = Duration::from_secs(12);
/// At most one guest-list ask per this time, however many knock.
const ASK_GAP: Duration = Duration::from_secs(1);

/// What a host room's worker and its tunnel's admit rule share.
#[derive(Default)]
struct RoomState {
    /// The guests the desk named last.
    guests: Vec<String>,
    /// Answers from the desk so far; a guest at the door waits for one asked after it came.
    asked: u64,
    /// A guest is waiting: ask now.
    want: bool,
    /// An ask is on the wire (its answer may predate the guest's own `SHOP`).
    busy: bool,
    /// The last ask's answer was the desk's list (not an I/O error).
    fresh: bool,
    stop: bool,
}

/// A shop room this PC hosts (2017-18, `CREATE shop`): its code; a worker keeps it with heartbeats and keeps its
/// guest list; closed when dropped.
pub struct HostRoom {
    pub code: String,
    /// The lobby server that holds it.
    pub server: String,
    addr: SocketAddr,
    id: String,
    ticket: String,
    state: Arc<(Mutex<RoomState>, std::sync::Condvar)>,
    lost: Arc<AtomicBool>,
}

impl HostRoom {
    /// A room on `server` ([`shop_lobby`]) for the shop tunnel `shop_id`, private unless `public` (listed in SHOPS as
    /// `name`, never with its code; empty: the desk's name); else why not (`FULL`, `ERR slow`, `ERR host`, no answer). `note` hears about a wait (this
    /// id's room from before still on the desk).
    pub fn open(
        server: &str,
        shop_id: &str,
        public: bool,
        name: &str,
        log: Log,
        note: &dyn Fn(&crate::run::Text),
    ) -> Result<HostRoom, String> {
        let addr = socket(server).ok_or_else(|| format!("{server} is not an address"))?;
        // the name only shows in SHOPS: a private room sends none (an old desk's ERR costs a CREATE)
        let mut name = if public { name } else { "" };
        let mut made = lobby::create_shop(addr, shop_id, public, name);
        // a desk from before names answers ERR to the named word: once more, with its own name, and keep it so
        if !lobby::shop_name(name).is_empty()
            && made
                .as_ref()
                .is_err_and(|e| e.starts_with("ERR") && e != "ERR host" && e != "ERR slow")
        {
            name = "";
            made = lobby::create_shop(addr, shop_id, public, name);
        }
        if made.as_ref().is_err_and(|e| e == "ERR host") {
            // one more CREATE only: each counts in the desk's per-address limit
            let wait = DESK_HOST_STALE + Duration::from_secs(2);
            note(
                &crate::run::Text::key("launcher.run_room_waiting")
                    .arg("server", server)
                    .arg("s", wait.as_secs()),
            );
            thread::sleep(wait);
            made = lobby::create_shop(addr, shop_id, public, name);
        }
        match made {
            Ok(Created::Room { id, code, ticket, .. }) => {
                let room = HostRoom {
                    code,
                    server: server.to_owned(),
                    addr,
                    id,
                    ticket,
                    state: Arc::default(),
                    lost: Arc::default(),
                };
                room.work(log);
                Ok(room)
            }
            Ok(Created::Full) => Err(format!("{server}: full")),
            Err(e) if e == "ERR host" => Err(format!(
                "{server}: this arcade's room from before is still there; try again in a minute"
            )),
            Err(e) => Err(format!("{server}: {e}")),
        }
    }

    /// Set once the desk no longer holds the room (its lease lapsed, the desk restarted): nobody new gets in.
    pub fn lost(&self) -> Arc<AtomicBool> {
        self.lost.clone()
    }

    /// The worker: `HEARTBEAT` every 20 s, the guest list (`SHOP t` with the host's ticket) every [`GUESTS_EVERY`] and
    /// whenever a guest waits at the door. `GONE` to either: the room is lost, the list emptied. An I/O error keeps the
    /// last list (the desk may be back at the next ask) and is logged once per run of errors.
    fn work(&self, log: Log) {
        let (addr, ticket, code) = (self.addr, self.ticket.clone(), self.code.clone());
        let (state, lost) = (self.state.clone(), self.lost.clone());
        thread::spawn(move || {
            let (lock, cv) = &*state;
            let every = Duration::from_secs(lobby::HEARTBEAT_EVERY);
            let (mut beat_at, mut list_at) = (Instant::now() + every, Instant::now());
            let mut asked_at: Option<Instant> = None;
            let mut failing = false;
            loop {
                let mut st = lock.lock().unwrap_or_else(PoisonError::into_inner);
                loop {
                    let now = Instant::now();
                    if st.stop {
                        return;
                    }
                    // a knock asks at once, but never sooner than ASK_GAP after the last ask
                    let soonest = asked_at.map_or(now, |t| t + ASK_GAP);
                    let list_due = if st.want { soonest } else { list_at.max(soonest) };
                    if now >= beat_at || now >= list_due {
                        break;
                    }
                    let until = beat_at.min(list_due) - now;
                    st = cv.wait_timeout(st, until).unwrap_or_else(PoisonError::into_inner).0;
                }
                st.busy = true;
                st.want = false;
                drop(st);
                let mut gone = false;
                let mut err = None;
                if Instant::now() >= beat_at {
                    beat_at = Instant::now() + every;
                    match lobby::heartbeat(addr, &ticket) {
                        Ok(false) => gone = true,
                        Ok(true) => {}
                        Err(e) => err = Some(e),
                    }
                }
                let guests = if gone {
                    None
                } else {
                    list_at = Instant::now() + GUESTS_EVERY;
                    asked_at = Some(Instant::now());
                    match lobby::shop_peers(addr, &ticket, None) {
                        Ok(Some(Peers::Guests(g))) => Some(g),
                        Ok(None) => {
                            gone = true;
                            None
                        }
                        Ok(Some(other)) => {
                            err = Some(format!("not a host answer: {}", other.line().trim()));
                            None
                        }
                        Err(e) => {
                            err = Some(e);
                            None
                        }
                    }
                };
                match (&err, failing) {
                    (Some(e), false) => log(&format!(
                        "shop room {code}: the lobby did not answer ({e}); trying again"
                    )),
                    (None, true) => log(&format!("shop room {code}: the lobby answers again")),
                    _ => {}
                }
                failing = err.is_some();
                let mut st = lock.lock().unwrap_or_else(PoisonError::into_inner);
                if st.stop {
                    return; // dropped meanwhile: its own CLOSE may be the GONE, nothing is lost
                }
                st.fresh = guests.is_some();
                if gone {
                    st.guests.clear();
                    lost.store(true, Ordering::Relaxed);
                    log(&format!(
                        "shop room {code}: the lobby no longer holds it; nobody new gets in"
                    ));
                } else if let Some(g) = guests {
                    st.guests = g;
                }
                st.asked += 1;
                st.busy = false;
                cv.notify_all();
                if gone {
                    st.stop = true;
                    return;
                }
            }
        });
    }

    /// The tunnel's admit rule: a guest the desk named in this room. One not on the last list waits (at most
    /// [`ADMIT_WAIT`]) for an answer the worker asked after it came, so its own `SHOP` is in it. Not named by a fresh
    /// list, a lost or closed room: not in the room. No fresh answer in time: no answer (the guest comes back).
    pub fn admit(&self) -> vslink::shop::Admit {
        use vslink::shop::Verdict;
        let (state, lost) = (self.state.clone(), self.lost.clone());
        Arc::new(move |id: &[u8; 32]| {
            let id = vslink::shop::id_text(id);
            let (lock, cv) = &*state;
            let mut st = lock.lock().unwrap_or_else(PoisonError::into_inner);
            if st.stop || lost.load(Ordering::Relaxed) {
                return Verdict::NotInRoom;
            }
            if st.guests.contains(&id) {
                return Verdict::In;
            }
            // an answer already on the wire may predate this guest's SHOP: wait for the one after it
            let target = st.asked + if st.busy { 2 } else { 1 };
            st.want = true;
            cv.notify_all();
            let (st, _) = cv
                .wait_timeout_while(st, ADMIT_WAIT, |s| s.asked < target && !s.stop)
                .unwrap_or_else(PoisonError::into_inner);
            if st.stop || lost.load(Ordering::Relaxed) {
                Verdict::NotInRoom
            } else if st.guests.contains(&id) {
                Verdict::In
            } else if st.asked < target || !st.fresh {
                Verdict::NoAnswer
            } else {
                Verdict::NotInRoom
            }
        })
    }
}

impl Drop for HostRoom {
    /// The room goes at once (the guests' tickets with it); best effort, the lease lapses by itself.
    fn drop(&mut self) {
        let (lock, cv) = &*self.state;
        lock.lock().unwrap_or_else(PoisonError::into_inner).stop = true; // no beat after the close
        cv.notify_all();
        let _ = lobby::close(self.addr, &self.id, &self.ticket);
    }
}

/// The heartbeat thread's handle; dropping it ends the beats (the lease then lapses by itself).
pub struct Beat(Arc<AtomicBool>);

impl Drop for Beat {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// TCP connect time in ms, or `None` when nothing answers.
fn ping(a: SocketAddr) -> Option<u32> {
    let t = Instant::now();
    TcpStream::connect_timeout(&a, Duration::from_secs(3)).ok()?;
    Some(t.elapsed().as_millis().min(9999) as u32)
}

#[derive(Default)]
struct Entry {
    at: Option<Instant>,
    ping_ms: Option<u32>,
    lobby: Option<Lobby>,
    misses: u32,
}

/// The answers so far, per server.
#[derive(Default)]
pub struct Book {
    servers: HashMap<String, Entry>,
    order: Vec<String>,
    /// A refused JOIN: the selection moves to the next open row once the list is refreshed.
    pub reselect: bool,
}

fn lang(code: &Option<String>) -> Option<Lang> {
    Lang::ALL.into_iter().find(|l| Some(l.code()) == code.as_deref())
}

/// QUICK MATCH on SHOPS: the open shop with the lowest ping, the nearest next ENTRY breaking a tie.
pub fn quick_shop(v: &LobbyView) -> Option<String> {
    v.shops
        .iter()
        .filter(|r| r.state == RowState::Open)
        .min_by_key(|r| (r.ping_ms.unwrap_or(u32::MAX), r.next_entry.unwrap_or(u32::MAX)))
        .map(|r| r.id.clone())
}

impl Book {
    pub fn new(servers: &[String]) -> Book {
        Book {
            order: servers.to_vec(),
            ..Book::default()
        }
    }

    /// Takes one poll. Other messages are the window's.
    pub fn apply(&mut self, m: &Msg, now: Instant) {
        let Msg::Poll {
            server,
            ping_ms,
            result,
        } = m
        else {
            return;
        };
        let e = self.servers.entry(server.clone()).or_default();
        match result {
            Ok(l) => {
                (e.at, e.ping_ms, e.lobby, e.misses) = (Some(now), *ping_ms, Some(l.clone()), 0);
            }
            Err(_) => e.misses += 1,
        }
    }

    /// VS room `id` as its server listed it last.
    pub fn room(&self, id: &str) -> Option<&VsRoom> {
        self.servers
            .values()
            .filter_map(|e| e.lobby.as_ref())
            .flat_map(|l| &l.vs)
            .find(|r| r.id == id)
    }

    /// The first server that answered, for HOST VS.
    pub fn answering(&self) -> Option<&String> {
        self.order.iter().find(|s| {
            self.servers
                .get(*s)
                .is_some_and(|e| e.lobby.is_some() && e.misses < MISSES)
        })
    }

    /// When `server` last answered.
    pub fn answered(&self, server: &str) -> Option<Instant> {
        self.servers.get(server).and_then(|e| e.at)
    }

    /// True when every server in the list has missed [`MISSES`] polls in a row.
    pub fn offline(&self) -> bool {
        !self.order.is_empty()
            && self
                .order
                .iter()
                .all(|s| self.servers.get(s).is_some_and(|e| e.misses >= MISSES))
    }

    /// Writes the rows, the age of the freshest answer and the offline flag into `v`; returns row id -> server.
    /// The selection stays on its row when that row is still listed.
    pub fn fill(&mut self, v: &mut LobbyView, now: Instant) -> HashMap<String, String> {
        let kept = v.visible().get(v.selected).map(|&i| v.rows()[i].id.clone());
        let (mut shops, mut vs, mut at) = (vec![], vec![], HashMap::new());
        for s in &self.order {
            let Some(e) = self.servers.get(s) else { continue };
            let Some(l) = &e.lobby else { continue };
            let silent = e.misses >= MISSES;
            let pick = |st: RowState| if silent { RowState::NoAnswer } else { st };
            for sh in &l.shops {
                at.insert(sh.id.clone(), s.clone());
                shops.push(Row {
                    id: sh.id.clone(),
                    name: sh.name.clone(),
                    players: sh.seats,
                    max: sh.max,
                    ping_ms: e.ping_ms,
                    lang: lang(&sh.lang),
                    state: pick(match sh.state {
                        State::Open => RowState::Open,
                        State::InMatch => RowState::InMatch,
                        State::Full => RowState::Full,
                        State::Closed => RowState::Closed,
                    }),
                    next_entry: sh.next_entry,
                    room: sh.room,
                });
            }
            for r in &l.vs {
                at.insert(r.id.clone(), s.clone());
                vs.push(Row {
                    id: r.id.clone(),
                    name: r.code.clone(),
                    players: r.players,
                    max: r.max,
                    ping_ms: e.ping_ms,
                    lang: lang(&r.lang),
                    state: pick(if r.players < r.max {
                        RowState::Open
                    } else {
                        RowState::Full
                    }),
                    next_entry: None,
                    room: false,
                });
            }
        }
        (v.shops, v.vs) = (shops, vs);
        v.updated_s = self
            .servers
            .values()
            .filter_map(|e| e.at)
            .max()
            .map(|t| now.saturating_duration_since(t).as_secs() as u32);
        let offline = self.offline();
        if offline && !v.offline {
            v.direct_open = true; // the way out is the address below
        }
        v.offline = offline;
        let vis = v.visible();
        let rows = v.rows();
        let pos = if std::mem::take(&mut self.reselect) {
            vis.iter().position(|&i| rows[i].joinable())
        } else {
            kept.and_then(|id| vis.iter().position(|&i| rows[i].id == id))
        };
        v.selected = pos.unwrap_or(v.selected);
        v.selected = v.selected.min(v.visible().len().saturating_sub(1));
        at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shop_rooms_meet_on_the_club_servers_desk() {
        if std::env::var_os("WCCF_LOBBY").is_some() {
            return;
        }
        assert_eq!(shop_lobby(""), PROJECT_SERVER_1718);
        assert_eq!(shop_lobby("  "), PROJECT_SERVER_1718);
        assert_eq!(shop_lobby("club.example"), "club.example:20032");
        assert_eq!(shop_lobby("203.0.113.10:20060"), "203.0.113.10:20032");
    }

    /// ovh1's answer on 2026-11-01: its always-on arcade, without `room`, showed "ARCADES 0" in 0.6.1.
    #[test]
    fn a_servers_own_arcade_is_listed_and_joins() {
        const OVH1: &str = r#"{"v":1,"edition":"2017-18","at":1793523072,"shops":[{"id":"bc07afa0","name":"Forza-WCCF","seats":0,"max":8,"state":"open"}],"vs":[]}"#;
        let server = PROJECT_SERVER_1718.to_owned();
        let mut book = Book::new(std::slice::from_ref(&server));
        let now = Instant::now();
        book.apply(
            &Msg::Poll {
                server: server.clone(),
                ping_ms: Some(30),
                result: Lobby::parse(OVH1),
            },
            now,
        );
        for shop_rooms in [false, true] {
            let mut v = forza_ui::launcher::lobby_for(forza_ui::theme::Edition::E1718);
            v.shop_rooms = shop_rooms;
            let at = book.fill(&mut v, now);
            let names: Vec<&str> = v.visible().iter().map(|&i| v.rows()[i].name.as_str()).collect();
            assert_eq!(names, ["Forza-WCCF"]);
            assert_eq!(v.joinable().map(|r| r.id.as_str()), Some("bc07afa0"));
            assert_eq!(at.get("bc07afa0"), Some(&server));
        }
    }

    #[test]
    fn wccf_lobby_replaces_the_default_server() {
        let s = |v: &str| Some(v.to_owned());
        assert_eq!(default_server(None, PROJECT_SERVER_1718), PROJECT_SERVER_1718);
        assert_eq!(default_server(s("a b"), PROJECT_SERVER_1718), PROJECT_SERVER_1718);
        assert_eq!(default_server(s("lobby.test"), PROJECT_SERVER_1718), "lobby.test:20032");
        assert_eq!(default_server(s("10.0.0.5:99"), PROJECT_SERVER_1011), "10.0.0.5:99");
    }
}
