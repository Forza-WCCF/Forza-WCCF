//! The window's side of the online lobby: the polling thread while ONLINE is shown, JOIN and its ticket, the heartbeat
//! while the lease waits for the run, the last seat offered again within 75 s, and the VS rooms: HOST VS, READY, the
//! 5 s countdown that starts the run, the CPU fallback (`launcher::lobby` has the workers).

use super::{poke, refuse, t, tf};
use forza::online::lobby::{Created, HEARTBEAT_EVERY, Reserved};
use forza::ui::launcher::LauncherState;
use forza::ui::lobby::{Kind, LobbyAction, Notice, Occupant, Room};
use launcher::choices;
use launcher::lobby::{Beat, Book, Msg, Poller, REJOIN_SECS, Wake, close, quick_shop, release};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The countdown from both ready to the run.
const COUNTDOWN: Duration = Duration::from_secs(5);
/// READY alone this long: the run starts anyway and the game gives a CPU team.
/// ponytail: a guess; how long the 2017-18 control polls 245 before its CPU fallback is measured in M3's gate.
const VS_WAIT: Duration = Duration::from_secs(90);
/// In a VS room the list is asked for this often (the desk takes one `LOBBY` per address every 2 s).
const ROOM_EVERY: Duration = Duration::from_secs(2);
/// The rejoin offer's time is kept fresh this often while a run is on (a Launcher killed with its game still offers).
const LAST_EVERY: u64 = 20;

/// A seat the desk holds for this ticket until the run claims it.
struct Held {
    ticket: String,
    server: String,
    /// PLAY started: the run spends the ticket, so a `GONE` from the desk is expected now.
    played: bool,
    /// The heartbeat lives as long as this window, and the window outlives every run it starts: the run is a thread
    /// of this process (`start`'s `Step::Game`) whose Job ends the game when the process goes (`job.rs`,
    /// KILL_ON_JOB_CLOSE), and `main.rs` cancels a close while a run is busy or up (a close asks the run to stop, by
    /// the card rule). The THIS PC / ONLINE choice is greyed while a run is on, so `look` never lets go mid-run.
    _beat: Beat,
    /// A VS room: its id, and whether this PC made it (LEAVE then closes it).
    vs: Option<(String, bool)>,
    since: Instant,
}

/// The VS room's clock: READY pressed here, alone since, the start.
#[derive(Default)]
struct Clock {
    ready: bool,
    alone: Option<Instant>,
    go: Option<Instant>,
    cpu: bool,
    asked: Option<Instant>,
}

/// The last seat, in data\settings.json "lobby_last".
#[derive(Clone)]
struct Last {
    server: String,
    id: String,
    name: String,
    seat: u8,
    at: u64,
}

fn unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Gives a held seat back to the desk unless PLAY already spent the ticket; a host closes its VS room.
fn let_go(h: Option<Held>) -> Option<std::thread::JoinHandle<()>> {
    h.filter(|h| !h.played).map(give_back)
}

/// `CLOSE` (the host of a VS room) or `RELEASE`.
fn give_back(h: Held) -> std::thread::JoinHandle<()> {
    match &h.vs {
        Some((id, true)) => close(&h.server, id, &h.ticket),
        _ => release(&h.server, &h.ticket),
    }
}

impl Drop for Online {
    /// The Launcher closes: the seat goes back now (the call has a short timeout).
    fn drop(&mut self) {
        if let Some(j) = let_go(self.held.take()) {
            let _ = j.join();
        }
    }
}

#[derive(Default)]
pub struct Online {
    poller: Option<Poller>,
    book: Book,
    /// Row id -> the server that listed it.
    rows: HashMap<String, String>,
    pub servers: Vec<String>,
    held: Option<Held>,
    last: Option<Last>,
    was_running: bool,
    clock: Clock,
}

impl Online {
    pub fn new(settings: &Path, address: &str, edition: launcher::Edition) -> Online {
        let last = choices::setting_json(settings, "lobby_last").and_then(|v| {
            Some(Last {
                server: v["server"].as_str()?.into(),
                id: v["id"].as_str()?.into(),
                name: v["name"].as_str()?.into(),
                seat: v["seat"].as_u64()?.try_into().ok()?,
                at: v["at"].as_u64()?,
            })
        });
        let servers = launcher::lobby::servers(settings, address, edition);
        let mut o = Online::default();
        o.book = Book::new(&servers);
        o.servers = servers;
        o.last = last;
        o
    }

    fn save_last(&self, settings: &Path) {
        let v = self.last.as_ref().map_or(
            Value::Null,
            |l| json!({"server": l.server, "id": l.id, "name": l.name, "seat": l.seat, "at": l.at}),
        );
        let _ = choices::set_setting_json(settings, "lobby_last", v);
    }

    /// The server PLAY goes to and the ticket for it, when a seat is held; the heartbeat goes on until the run
    /// spends the ticket. `vs_only` (2017-18): only a VS room's ticket, the one the run's vslink hello carries.
    pub fn take_ticket(&mut self, vs_only: bool) -> Option<(String, String)> {
        let h = self.held.as_mut().filter(|h| !vs_only || h.vs.is_some())?;
        h.played = true;
        Some((h.server.clone(), h.ticket.clone()))
    }

    /// Once per frame: the thread alive only while ONLINE is shown, its answers into the rows, the Rejoin offer, the
    /// VS room's countdown. True when the countdown ran out: start the run now.
    pub fn look(&mut self, st: &mut LauncherState, edition: &'static str, settings: &Path) -> bool {
        let wake: Wake = Arc::new(poke);
        if st.online && !st.running {
            if self.poller.is_none() {
                self.poller = Some(Poller::start(
                    self.servers.clone(),
                    edition,
                    launcher::lobby::EVERY,
                    wake,
                ));
            }
        } else {
            self.poller = None;
        }
        if !st.online {
            let_go(self.held.take());
            st.lobby.room = None;
        }
        if self.was_running && !st.running {
            // the run ended: the seat is free, and offered again for a short while; a VS room's run held its
            // ticket to the end (the run claims no seat), so the room goes now and is not offered again
            if let Some(h) = self.held.take().filter(|h| h.vs.is_some()) {
                give_back(h);
                self.last = None;
                self.save_last(settings);
            }
            st.lobby.room = None;
            if let Some(l) = &mut self.last {
                l.at = unix();
                self.save_last(settings);
            }
        }
        if st.running
            && let Some(l) = &mut self.last
            && unix() >= l.at + LAST_EVERY
        {
            l.at = unix();
            self.save_last(settings);
        }
        self.was_running = st.running;
        let msgs: Vec<Msg> = self.poller.iter().flat_map(|p| p.rx.try_iter()).collect();
        for m in msgs {
            match m {
                Msg::Poll { .. } => self.book.apply(&m, Instant::now()),
                Msg::Reserved {
                    server,
                    id,
                    name,
                    result,
                } => self.reserved(st, settings, server, id, name, result),
                Msg::Gone { ticket } => {
                    if self.held.as_ref().is_some_and(|h| h.ticket == ticket)
                        && let Some(h) = self.held.take().filter(|h| !h.played)
                    {
                        st.lobby.room = None;
                        st.lobby.notice = Some(if h.vs.is_some() {
                            Notice::RoomClosed
                        } else {
                            Notice::Taken
                        });
                    }
                }
                Msg::Created { server, result } => self.created(st, settings, server, result),
                Msg::Readied { id, result } => {
                    let here = self
                        .held
                        .as_ref()
                        .and_then(|h| h.vs.as_ref())
                        .is_some_and(|v| v.0 == id);
                    match result {
                        Ok(true) => {}
                        Ok(false) if here => self.closed(st),
                        Ok(false) => {}
                        Err(e) => {
                            self.clock.ready = false;
                            refuse(tf("launcher.msg_no_answer", &[("error", &e)]));
                        }
                    }
                }
            }
        }
        if self.poller.is_some() {
            self.rows = self.book.fill(&mut st.lobby, Instant::now());
            if st.lobby.grey[Kind::Shops as usize].is_some() {
                st.lobby.shops.clear(); // 2017-18: nothing behind SHOPS before the hub, the tab shows its line
            }
        }
        let go = self.room(st);
        st.rejoin = self
            .last
            .iter()
            .filter(|l| self.held.is_none() && !st.running && unix() <= l.at + REJOIN_SECS)
            .map(|l| tf("launcher.rejoin", &[("name", &l.name), ("seat", &l.seat.to_string())]))
            .next();
        go
    }

    /// The room went away under this PC (the host closed it, or the desk forgot it).
    fn closed(&mut self, st: &mut LauncherState) {
        self.held = None;
        self.clock = Clock::default();
        st.lobby.room = None;
        st.lobby.notice = Some(Notice::RoomClosed);
    }

    /// The held VS room from the last answer: its seats, READY, the countdown; true when it ran out.
    fn room(&mut self, st: &mut LauncherState) -> bool {
        let now = Instant::now();
        let Some(h) = &self.held else {
            self.clock = Clock::default();
            return false;
        };
        let Some((id, _)) = h.vs.clone() else { return false };
        if h.played {
            return false;
        }
        // asked more often while in a room, so the two PCs see "both ready" close together
        if self.clock.asked.is_none_or(|t| now >= t + ROOM_EVERY) {
            self.clock.asked = Some(now);
            self.poller.iter().for_each(Poller::refresh);
        }
        let Some(listed) = self.book.room(&id).cloned() else {
            // a room missing from an answer asked after it was made: closed (the in-flight poll may predate it)
            if self
                .book
                .answered(&h.server)
                .is_some_and(|t| t > h.since + Duration::from_secs(3))
            {
                self.closed(st);
            }
            return false;
        };
        let Some(room) = &mut st.lobby.room else { return false };
        let mut seats = vec![None; room.seats.len().max(usize::from(listed.max))];
        for o in &listed.occupants {
            if let Some(s) = seats.get_mut(usize::from(o.seat).wrapping_sub(1)) {
                *s = Some(Occupant {
                    club: o.club.clone(),
                    ready: o.ready,
                });
            }
        }
        // READY pressed here shows at once, before the desk's next answer says so
        if let Some(Some(o)) = room.mine.and_then(|i| seats.get_mut(i)) {
            o.ready |= self.clock.ready;
        }
        room.seats = seats;
        let c = &mut self.clock;
        let alone = room.seats.iter().any(Option::is_none);
        c.alone = if c.ready && alone { c.alone.or(Some(now)) } else { None };
        if c.alone.is_some_and(|t| now >= t + VS_WAIT) {
            c.cpu = true;
        }
        if room.all_ready() || c.cpu {
            c.go.get_or_insert(now + COUNTDOWN);
        } else {
            c.go = None; // the opponent left during the countdown
        }
        room.cpu = c.cpu;
        room.countdown =
            c.go.map(|t| t.saturating_duration_since(now).as_secs_f32().ceil() as u32);
        if c.go.is_some_and(|t| now >= t) {
            *c = Clock::default();
            return true;
        }
        false
    }

    fn created(&mut self, st: &mut LauncherState, settings: &Path, server: String, result: Result<Created, String>) {
        let Some(p) = &self.poller else { return };
        match result {
            Ok(Created::Room { id, code, ticket, seat }) => {
                let beat = p.beat(&server, &ticket, Duration::from_secs(HEARTBEAT_EVERY), Arc::new(poke));
                let mut room = Room::new(&code, Kind::Vs, 2);
                // seat 2 when QUICK MATCH sat down in a waiting room; the club id comes with the next LOBBY
                let i = usize::from(seat.clamp(1, 2)) - 1;
                room.mine = Some(i);
                room.seats[i] = Some(Occupant {
                    club: String::new(),
                    ready: false,
                });
                st.lobby.room = Some(room);
                st.lobby.notice = None;
                self.clock = Clock::default();
                self.last = Some(Last {
                    server: server.clone(),
                    id: id.clone(),
                    name: code,
                    seat,
                    at: unix(),
                });
                self.save_last(settings);
                self.held = Some(Held {
                    ticket,
                    server,
                    played: false,
                    _beat: beat,
                    vs: Some((id, seat == 1)),
                    since: Instant::now(),
                });
            }
            Ok(Created::Full) => refuse(t("launcher.msg_rooms_full")),
            Err(e) => refuse(tf("launcher.msg_no_answer", &[("error", &e)])),
        }
    }

    /// HOST VS (`open` false) or QUICK MATCH on VS ROOMS, on the first server that answers.
    fn host(&self, open: bool) {
        if let (Some(p), Some(server)) = (&self.poller, self.book.answering()) {
            p.create(server, open, Arc::new(poke));
        }
    }

    fn reserved(
        &mut self,
        st: &mut LauncherState,
        settings: &Path,
        server: String,
        id: String,
        name: String,
        result: Result<Reserved, String>,
    ) {
        let Some(p) = &self.poller else { return };
        match result {
            Ok(Reserved::Ticket { ticket, seat }) => {
                let beat = p.beat(&server, &ticket, Duration::from_secs(HEARTBEAT_EVERY), Arc::new(poke));
                let vs = st.lobby.vs.iter().find(|r| r.id == id);
                // 2017-18 (SHOPS greyed) reserves only VS rooms, also a rejoin before the list came
                let (kind, n) = match vs {
                    Some(r) => (Kind::Vs, usize::from(r.max.max(2))),
                    None if st.lobby.grey[Kind::Shops as usize].is_some() => (Kind::Vs, 2),
                    None => (Kind::Shops, 8),
                };
                let mut room = Room::new(&name, kind, n);
                room.next_entry = st.lobby.shops.iter().find(|r| r.id == id).and_then(|r| r.next_entry);
                let i = usize::from(seat).clamp(1, n) - 1;
                room.mine = Some(i);
                // a shop seat is ready once reserved; a VS seat waits for READY
                // the club id comes with the next LOBBY (the desk's, never the ticket)
                room.seats[i] = Some(Occupant {
                    club: String::new(),
                    ready: kind == Kind::Shops,
                });
                st.lobby.room = Some(room);
                self.clock = Clock::default();
                st.lobby.notice = None;
                self.last = Some(Last {
                    server: server.clone(),
                    id: id.clone(),
                    name,
                    seat,
                    at: unix(),
                });
                self.save_last(settings);
                self.held = Some(Held {
                    ticket,
                    server,
                    played: false,
                    _beat: beat,
                    vs: (kind == Kind::Vs).then_some((id, false)),
                    since: Instant::now(),
                });
            }
            Ok(Reserved::Full | Reserved::Taken | Reserved::Closed) => {
                st.lobby.notice = Some(Notice::Taken);
                self.book.reselect = true;
                p.refresh();
            }
            Err(e) => refuse(tf("launcher.msg_no_answer", &[("error", &e)])),
        }
    }

    fn join(&self, st: &LauncherState, id: &str, seat: Option<u8>) {
        let (Some(p), Some(server)) = (&self.poller, self.rows.get(id)) else {
            return;
        };
        let name = st
            .lobby
            .shops
            .iter()
            .chain(&st.lobby.vs)
            .find(|r| r.id == id)
            .map_or("", |r| r.name.as_str());
        p.reserve(server, id, name, seat, Arc::new(poke));
    }

    /// What the player chose in the lobby (`Action::Lobby`).
    pub fn act(&mut self, a: LobbyAction, st: &mut LauncherState, settings: &Path) {
        match a {
            LobbyAction::Join(id) => self.join(st, &id, None),
            LobbyAction::QuickMatch if st.lobby.kind == Kind::Vs => self.host(true),
            LobbyAction::QuickMatch => {
                if let Some(id) = quick_shop(&st.lobby) {
                    self.join(st, &id, None);
                }
            }
            LobbyAction::HostVs => self.host(false),
            LobbyAction::Ready => {
                let (Some(p), Some(h)) = (&self.poller, &self.held) else {
                    return;
                };
                if let Some((id, _)) = &h.vs {
                    self.clock.ready = true;
                    p.ready(&h.server, id, &h.ticket, Arc::new(poke));
                }
            }
            LobbyAction::Refresh => self.poller.iter().for_each(Poller::refresh),
            LobbyAction::Leave => {
                if st.lobby.room.is_some() {
                    // the heartbeat stops and the desk frees the seat now
                    let_go(self.held.take());
                    self.clock = Clock::default();
                    self.last = None;
                    self.save_last(settings);
                    st.lobby.room = None;
                }
            }
            LobbyAction::DirectAddress(a) => {
                let a = a.trim().to_owned();
                if !launcher::lobby::valid_server(&a) {
                    refuse(t("launcher.msg_bad_address"));
                } else if !self.servers.contains(&a) {
                    self.servers.insert(0, a);
                    let _ = choices::set_setting_json(settings, "servers", json!(self.servers));
                    self.book = Book::new(&self.servers);
                    self.poller = None; // restarted on the next look with the new list
                }
            }
        }
    }

    /// "Rejoin ...": the last seat by number.
    pub fn rejoin(&self) {
        if let (Some(p), Some(l)) = (&self.poller, &self.last) {
            p.reserve(&l.server, &l.id, &l.name, Some(l.seat), Arc::new(poke));
        }
    }
}
