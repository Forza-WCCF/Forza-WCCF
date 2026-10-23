//! The Launcher's online lobby (ROADMAP, "Online lobby view", L1): a polling thread that asks each server in the
//! `servers` setting for its `LOBBY` every [`EVERY`], the [`Book`] that turns the answers into the rows
//! `forza_ui::lobby` draws, and the reserve and heartbeat workers behind JOIN.  The window owns the threads' handles;
//! dropping a [`Poller`] or a [`Beat`] ends its thread.

use crate::Edition;
use forza::online::SEAT_PORT;
use forza::online::lobby::{self, Created, Lobby, Reserved, State, VsRoom};
use forza_ui::Lang;
use forza_ui::lobby::{LobbyView, Row, RowState};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, mpsc::Receiver};
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
}

// The project's public server; the `servers` setting, "Direct address" and `WCCF_LOBBY` (host[:port]) override it.
/// The project's lobby server for 2010-11 (its seat desk on the usual port, [`SEAT_PORT`]).
pub const PROJECT_SERVER_1011: &str = "lobby.wccf.it:20030";
/// The project's lobby server for 2017-18 (its desk beside the game server on 20060).
pub const PROJECT_SERVER_1718: &str = "lobby.wccf.it:20032";

/// The default server: `WCCF_LOBBY` when it is a valid `host[:port]` (the project's `default` desk port if none),
/// else `default`.
fn default_server(env: Option<String>, default: &str) -> String {
    let port = crate::choices::host_port(default, SEAT_PORT).map_or(SEAT_PORT, |h| h.1);
    env.and_then(|v| crate::choices::host_port(&v, port))
        .map_or_else(|| default.to_owned(), |(h, p)| format!("{h}:{p}"))
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
    let project = default_server(
        std::env::var("WCCF_LOBBY").ok(),
        match edition {
            Edition::E1011 => PROJECT_SERVER_1011,
            Edition::E1718 => PROJECT_SERVER_1718,
        },
    );
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
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, server, ticket, out) = (stop.clone(), server.to_owned(), ticket.to_owned(), self.tx.clone());
        thread::spawn(move || {
            let Some(a) = socket(&server) else { return };
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
                        let _ = out.send(Msg::Gone { ticket });
                        wake();
                    }
                    return;
                }
            }
        });
        Beat(stop)
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
    fn wccf_lobby_replaces_the_default_server() {
        let s = |v: &str| Some(v.to_owned());
        assert_eq!(default_server(None, PROJECT_SERVER_1718), PROJECT_SERVER_1718);
        assert_eq!(default_server(s("a b"), PROJECT_SERVER_1718), PROJECT_SERVER_1718);
        assert_eq!(default_server(s("lobby.test"), PROJECT_SERVER_1718), "lobby.test:20032");
        assert_eq!(default_server(s("10.0.0.5:99"), PROJECT_SERVER_1011), "10.0.0.5:99");
    }
}
