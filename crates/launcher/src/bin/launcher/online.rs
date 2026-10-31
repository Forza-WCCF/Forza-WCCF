//! The window's side of the online lobby: the polling thread while ONLINE is shown, JOIN and its ticket, the heartbeat
//! while the lease waits for the run, the last seat offered again within 75 s, and the VS rooms: HOST VS, READY, the
//! 5 s countdown that starts the run, the CPU fallback; 2017-18 shop rooms joined by their code (HOST SHOP itself is
//! `main.rs`'s job) (`launcher::lobby` has the workers).

use super::{plain_error, poke, refuse, t, tf};
use forza::online::lobby::{Created, HEARTBEAT_EVERY, Reserved};
use forza_ui::launcher::{LauncherState, Tab};
use forza_ui::lobby::{Kind, LobbyAction, Notice, Occupant, Room, RowState};
use launcher::choices;
use launcher::lobby::CodeJoin;
use launcher::lobby::{Beat, Book, Msg, Poller, REJOIN_SECS, Wake, close, quick_shop, release};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The countdown from both ready to the run.
const COUNTDOWN: Duration = Duration::from_secs(5);
/// The countdown from no opponent to the CPU match: time for KEEP WAITING or PLAY CPU NOW.
const CPU_COUNTDOWN: Duration = Duration::from_secs(10);
/// READY alone this long: the run starts anyway and the game gives a CPU team. The client polls REQ_MATCH every 10 s,
/// `vs::POLLS` times, the first 10 s after its entry, so its control commits the CPU fallback about 10 * POLLS s in; 10 s
/// more of margin.
/// ponytail: from the disassembly; a live capture of the poll times confirms it (M3's gate).
const VS_WAIT: Duration = Duration::from_secs(10 * forza::online::vs::POLLS as u64 + 10);
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
    /// A shop room's guest seat (2017-18, by code): released when its run ends.
    shop: bool,
    since: Instant,
}

/// What [`Online::look`] asks the window to start.
pub enum Go {
    /// PLAY, as the button does (a VS room's countdown ran out).
    Play,
    /// `remote ID`: a shop room's host, reached through its tunnel.
    Remote(String),
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

impl Clock {
    /// One look at the room (`alone`: a seat still empty): READY alone for [`VS_WAIT`] turns to the CPU team and its
    /// countdown, every seat ready to the start's; true when the countdown ran out (the run starts).
    fn tick(&mut self, now: Instant, alone: bool, all_ready: bool) -> bool {
        self.alone = if self.ready && alone {
            self.alone.or(Some(now))
        } else {
            None
        };
        if self.alone.is_some_and(|t| now >= t + VS_WAIT) {
            self.cpu = true;
        }
        if !alone && self.cpu {
            // an opponent came during the CPU countdown: no CPU team
            (self.cpu, self.go) = (false, None);
        }
        if all_ready || self.cpu {
            let wait = if all_ready { COUNTDOWN } else { CPU_COUNTDOWN };
            self.go.get_or_insert(now + wait);
        } else {
            self.go = None; // the opponent left during the countdown
        }
        self.go.is_some_and(|t| now >= t)
    }

    /// The seconds the countdown has left.
    fn left(&self, now: Instant) -> Option<u32> {
        self.go
            .map(|t| t.saturating_duration_since(now).as_secs_f32().ceil() as u32)
    }

    /// KEEP WAITING: no CPU team; the wait for an opponent starts again.
    fn keep_waiting(&mut self, now: Instant) {
        (self.cpu, self.go, self.alone) = (false, None, Some(now));
    }

    /// PLAY CPU NOW: the CPU countdown ends at the next look.
    fn cpu_now(&mut self, now: Instant) {
        if self.cpu {
            self.go = Some(now);
        }
    }
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
    /// The shop room being joined is a listed row's (its id), not a typed code.
    joining_listed: bool,
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
    pub fn look(&mut self, st: &mut LauncherState, edition: &'static str, settings: &Path) -> Option<Go> {
        let wake: Wake = Arc::new(poke);
        // the list while it shows; a seat or room this PC holds while PLAY plays online
        if (st.online || st.tab == Tab::Online) && !st.running {
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
            st.lobby.joining = None;
        }
        // a shop guest's run is over (or never came up): its seat goes back
        if !st.running
            && !st.busy
            && self.held.as_ref().is_some_and(|h| h.shop && h.played)
            && let Some(h) = self.held.take()
        {
            give_back(h);
            st.lobby.joining = None;
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
                Msg::Coded { server, code, result } => {
                    if let Some(host) = self.coded(st, server, &code, result) {
                        return Some(Go::Remote(host));
                    }
                }
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
                            refuse(plain_error(&e));
                        }
                    }
                }
            }
        }
        if self.poller.is_some() {
            self.rows = self.book.fill(&mut st.lobby, Instant::now());
        }
        let go = self.room(st).then_some(Go::Play);
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
        let go = c.tick(now, alone, room.all_ready());
        room.cpu = c.cpu;
        room.countdown = c.left(now);
        if go {
            *c = Clock::default();
        }
        go
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
                    shop: false,
                    since: Instant::now(),
                });
            }
            Ok(Created::Full) => refuse(t("launcher.msg_rooms_full")),
            Err(e) => refuse(plain_error(&e)),
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
                // 2017-18 (SHOPS view-only) reserves only VS rooms, also a rejoin before the list came
                let (kind, n) = match vs {
                    Some(r) => (Kind::Vs, usize::from(r.max.max(2))),
                    None if st.lobby.shops_view_only => (Kind::Vs, 2),
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
                    shop: false,
                    since: Instant::now(),
                });
            }
            Ok(Reserved::Full | Reserved::Taken | Reserved::Closed) => {
                st.lobby.notice = Some(Notice::Taken);
                self.book.reselect = true;
                p.refresh();
            }
            Err(e) => refuse(plain_error(&e)),
        }
    }

    /// A typed shop code's answer: in, the seat is held (its heartbeat runs until the run ends) and the run goes to the
    /// host's id; else why not.
    fn coded(
        &mut self,
        st: &mut LauncherState,
        server: String,
        code: &str,
        result: Result<CodeJoin, String>,
    ) -> Option<String> {
        if st.lobby.joining.as_deref() != Some(code) {
            // given up meanwhile (ONLINE left, another code): the seat goes back now
            if let Ok(CodeJoin::In { ticket, .. }) = result {
                release(&server, &ticket);
            }
            return None;
        }
        let p = self.poller.as_ref()?;
        let notice = match result {
            Ok(CodeJoin::In { ticket, host }) => {
                let beat = p.beat(&server, &ticket, Duration::from_secs(HEARTBEAT_EVERY), Arc::new(poke));
                let_go(self.held.take());
                self.held = Some(Held {
                    ticket,
                    server,
                    // the run starts now: a GONE from here on is the run's to tell
                    played: true,
                    _beat: beat,
                    vs: None,
                    shop: true,
                    since: Instant::now(),
                });
                st.lobby.notice = None;
                st.lobby.joining = None;
                return Some(host);
            }
            // a listed row's room left the desk since the list came
            Ok(CodeJoin::NoRoom) if self.joining_listed => Notice::ShopClosed,
            Ok(CodeJoin::NoRoom) => Notice::NoRoom,
            Ok(CodeJoin::Full) => Notice::RoomFull,
            Ok(CodeJoin::Gone) => Notice::ShopClosed,
            Err(e) => {
                st.lobby.joining = None;
                refuse(plain_error(&e));
                return None;
            }
        };
        st.lobby.joining = None;
        st.lobby.notice = Some(notice);
        None
    }

    /// A code typed: an open VS room's as the list shows it, else (with the shared shop on) a shop room's, asked of
    /// the club server's desk, where hosts make them.
    fn join_code(&mut self, st: &mut LauncherState, code: &str) {
        let vs = st
            .lobby
            .vs
            .iter()
            .find(|r| !st.lobby.vs_hidden && r.name == code && r.state == RowState::Open);
        if let Some(id) = vs.map(|r| r.id.clone()) {
            st.lobby.notice = None;
            self.join(st, &id, None);
            return;
        }
        if self.poller.is_none() {
            return;
        }
        let server = launcher::lobby::shop_lobby(st.club_server.as_deref().unwrap_or_default());
        st.lobby.desk.clone_from(&server); // a "no room" names it: the host's room may be on another club server
        // a code of another length is no room's: not worth one of the desk's wrong-code tries
        if !st.lobby.shop_rooms || code.len() != forza::online::lobby::SHOP_CODE_LEN {
            st.lobby.notice = Some(Notice::NoRoom);
            return;
        }
        self.shop_join(st, &server, code, false);
    }

    /// `JOIN` of a shop room on `server` by its code, or by a listed row's id (`listed`).
    fn shop_join(&mut self, st: &mut LauncherState, server: &str, key: &str, listed: bool) {
        let Some(p) = &self.poller else { return };
        let file = launcher::Kit::here().data().join(launcher::lobby::SHOP_JOIN_KEY);
        match vslink::shop::own_id(&file) {
            Ok(own) => {
                self.joining_listed = listed;
                st.lobby.joining = Some(key.to_owned());
                st.lobby.notice = None;
                p.join_code(server, key, &vslink::shop::id_text(&own), Arc::new(poke));
            }
            Err(e) => refuse(plain_error(&format!("{}: {e}", file.display()))),
        }
    }

    fn join(&mut self, st: &mut LauncherState, id: &str, seat: Option<u8>) {
        let (Some(p), Some(server)) = (&self.poller, self.rows.get(id)) else {
            return;
        };
        // a listed shop room (2017-18): JOIN its id, as a typed code would be
        if st.lobby.shop_rooms && st.lobby.shops.iter().any(|r| r.room && r.id == id) {
            let server = server.clone();
            st.lobby.desk.clone_from(&server);
            self.shop_join(st, &server, id, true);
            return;
        }
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
            LobbyAction::KeepWaiting => self.clock.keep_waiting(Instant::now()),
            LobbyAction::CpuNow => self.clock.cpu_now(Instant::now()),
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
            LobbyAction::JoinCode(code) => self.join_code(st, &code),
            // HOST SHOP and the LAN fold are jobs of the window's (main.rs); CONNECT arrives as Action::Play
            LobbyAction::HostShop | LobbyAction::HostLan | LobbyAction::JoinLan(_) | LobbyAction::Connect => {}
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

#[cfg(test)]
mod tests {
    use super::*;

    /// No opponent in time: the CPU countdown; KEEP WAITING starts the wait again, the countdown running out or PLAY
    /// CPU NOW starts the run.
    #[test]
    fn the_cpu_countdown_waits_on_or_starts() {
        let t0 = Instant::now();
        let ready = || Clock {
            ready: true,
            ..Clock::default()
        };
        let mut c = ready();
        assert!(!c.tick(t0, true, false));
        let t1 = t0 + VS_WAIT;
        assert!(!c.tick(t1, true, false));
        assert!(c.cpu);
        assert_eq!(c.left(t1), Some(CPU_COUNTDOWN.as_secs() as u32));
        c.keep_waiting(t1);
        assert!(!c.tick(t1 + CPU_COUNTDOWN, true, false), "kept waiting");
        assert!(!c.cpu && c.left(t1).is_none());
        // alone the whole wait again: the countdown again, and its end starts the run
        let t2 = t1 + VS_WAIT;
        assert!(!c.tick(t2, true, false) && c.cpu);
        assert!(!c.tick(t2 + CPU_COUNTDOWN / 2, true, false));
        assert!(c.tick(t2 + CPU_COUNTDOWN, true, false));

        let mut c = ready();
        c.cpu_now(t0); // no CPU team yet: nothing
        assert!(!c.tick(t0, true, false));
        assert!(!c.tick(t1, true, false));
        c.cpu_now(t1);
        assert!(c.tick(t1, true, false));

        // an opponent sits down during the CPU countdown: no CPU team, the countdown waits for both ready
        let mut c = ready();
        c.tick(t0, true, false);
        assert!(!c.tick(t1, true, false) && c.cpu);
        assert!(!c.tick(t1 + CPU_COUNTDOWN, false, false), "no CPU start");
        assert!(!c.cpu && c.left(t1).is_none());
        assert!(!c.tick(t1 + CPU_COUNTDOWN, false, true));
        assert_eq!(c.left(t1 + CPU_COUNTDOWN), Some(COUNTDOWN.as_secs() as u32));
    }
}
