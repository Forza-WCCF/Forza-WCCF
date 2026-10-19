//! The window's side of the online lobby: the polling thread while ONLINE is shown, JOIN and its ticket, the heartbeat
//! while the lease waits for the run, and the last seat offered again within 75 s (`launcher::lobby` has the workers).

use super::{poke, refuse, t, tf};
use forza::online::lobby::{HEARTBEAT_EVERY, Reserved};
use forza::ui::launcher::LauncherState;
use forza::ui::lobby::{Kind, LobbyAction, Notice, Occupant, Room};
use launcher::choices;
use launcher::lobby::{Beat, Book, Msg, Poller, REJOIN_SECS, Wake, quick_shop};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A seat the desk holds for this ticket until the run claims it.
struct Held {
    ticket: String,
    server: String,
    /// PLAY started: the run spends the ticket, so a `GONE` from the desk is expected now.
    played: bool,
    _beat: Beat,
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
}

/// The `servers` setting; none saved yet: the address the player had before (so nothing changes for old setups).
pub fn servers(settings: &Path, address: &str) -> Vec<String> {
    let saved = choices::setting_json(settings, "servers").and_then(|v| {
        v.as_array().map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
    });
    match saved {
        Some(s) => s,
        None if choices::valid_address(address) => vec![address.to_owned()],
        None => vec![],
    }
}

impl Online {
    pub fn new(settings: &Path, address: &str) -> Online {
        let last = choices::setting_json(settings, "lobby_last").and_then(|v| {
            Some(Last {
                server: v["server"].as_str()?.into(),
                id: v["id"].as_str()?.into(),
                name: v["name"].as_str()?.into(),
                seat: v["seat"].as_u64()?.try_into().ok()?,
                at: v["at"].as_u64()?,
            })
        });
        let servers = servers(settings, address);
        Online {
            book: Book::new(&servers),
            servers,
            last,
            ..Online::default()
        }
    }

    fn save_last(&self, settings: &Path) {
        let v = self.last.as_ref().map_or(
            Value::Null,
            |l| json!({"server": l.server, "id": l.id, "name": l.name, "seat": l.seat, "at": l.at}),
        );
        let _ = choices::set_setting_json(settings, "lobby_last", v);
    }

    /// The server PLAY goes to and the ticket for it, when a seat is held; the heartbeat goes on until the run
    /// spends the ticket.
    pub fn take_ticket(&mut self) -> Option<(String, String)> {
        let h = self.held.as_mut()?;
        h.played = true;
        Some((h.server.clone(), h.ticket.clone()))
    }

    /// Once per frame: the thread alive only while ONLINE is shown, its answers into the rows, the Rejoin offer.
    pub fn look(&mut self, st: &mut LauncherState, edition: &'static str, settings: &Path) {
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
            self.held = None;
            st.lobby.room = None;
        }
        if self.was_running && !st.running {
            // the run ended: the seat is free, and offered again for a short while
            self.held = None;
            if let Some(l) = &mut self.last {
                l.at = unix();
                self.save_last(settings);
            }
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
                    if self.held.as_ref().is_some_and(|h| h.ticket == ticket) {
                        if self.held.as_ref().is_some_and(|h| !h.played) {
                            st.lobby.room = None;
                            st.lobby.notice = Some(Notice::Taken);
                        }
                        self.held = None;
                    }
                }
            }
        }
        if self.poller.is_some() {
            self.rows = self.book.fill(&mut st.lobby, Instant::now());
        }
        st.rejoin = self
            .last
            .iter()
            .filter(|l| self.held.is_none() && !st.running && unix() <= l.at + REJOIN_SECS)
            .map(|l| tf("launcher.rejoin", &[("name", &l.name), ("seat", &l.seat.to_string())]))
            .next();
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
                let next_entry = st.lobby.shops.iter().find(|r| r.id == id).and_then(|r| r.next_entry);
                let mut seats: Vec<Option<Occupant>> = vec![None; 8];
                seats[usize::from(seat).clamp(1, 8) - 1] = Some(Occupant {
                    club: ticket.to_uppercase(),
                    ready: true,
                });
                st.lobby.room = Some(Room {
                    title: name.clone(),
                    kind: Kind::Shops,
                    seats,
                    next_entry,
                });
                st.lobby.notice = None;
                self.last = Some(Last {
                    server: server.clone(),
                    id,
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
            .find(|r| r.id == id)
            .map_or("", |r| r.name.as_str());
        p.reserve(server, id, name, seat, Arc::new(poke));
    }

    /// What the player chose in the lobby (`Action::Lobby`).
    pub fn act(&mut self, a: LobbyAction, st: &mut LauncherState, settings: &Path) {
        match a {
            LobbyAction::Join(id) => self.join(st, &id, None),
            LobbyAction::QuickMatch => {
                if let Some(id) = quick_shop(&st.lobby) {
                    self.join(st, &id, None);
                }
            }
            LobbyAction::Refresh => self.poller.iter().for_each(Poller::refresh),
            LobbyAction::Leave => {
                if st.lobby.room.is_some() {
                    // the heartbeat stops; the desk frees the seat when the 60 s lease lapses
                    self.held = None;
                    self.last = None;
                    self.save_last(settings);
                    st.lobby.room = None;
                }
            }
            LobbyAction::DirectAddress(a) => {
                let a = a.trim().to_owned();
                if !choices::valid_address(&a) {
                    refuse(t("launcher.msg_bad_address"));
                } else if !self.servers.contains(&a) {
                    self.servers.insert(0, a);
                    let _ = choices::set_setting_json(settings, "servers", json!(self.servers));
                    self.book = Book::new(&self.servers);
                    self.poller = None; // restarted on the next look with the new list
                }
            }
            // VS rooms (HOST VS, READY) wait for the matcher (L2/L3)
            LobbyAction::HostVs | LobbyAction::Ready => {}
        }
    }

    /// "Rejoin ...": the last seat by number.
    pub fn rejoin(&self) {
        if let (Some(p), Some(l)) = (&self.poller, &self.last) {
            p.reserve(&l.server, &l.id, &l.name, Some(l.seat), Arc::new(poke));
        }
    }
}
