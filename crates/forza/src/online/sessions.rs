//! Who is on the game server ([`super::gamesvr`]) now: the cards logged in on each open connection, the matches they
//! play and in which mode. For the server's own log and the desk's lobby count; kept in memory only, nothing stored.
//!
//! A connection is one arcade's control (it relays every seat, request +0 = the seat's slot) or one vslink PC. A card
//! is in when COACH_LIST 271 names its Aime id on a slot, and stays in until another card takes that slot or the
//! connection closes.

use super::gamesvr::{match_line, u32_at};
use crate::service::Log;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// What a play was, from what its seat sent before its end of play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    GrandTrophy,
    NetworkVs,
    Friendly,
    Other,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::GrandTrophy => "Grand Trophy",
            Mode::NetworkVs => "network VS",
            Mode::Friendly => "friendly",
            Mode::Other => "other",
        }
    }
}

/// A play's mode: a VS entry (MATCHING_ENTRY 243) with kbn (+16) 0 is a friendly (the control sends 0 when the Grand
/// Trophy window is closed, `vs::fill_game_ctrl`), another kbn network VS; else a ghost search (GHOST_MATCHING 305) of
/// match type 34 is the Grand Trophy (play mode 7, docs/research/ONLINE-2017-18.md); anything else is other.
pub fn mode(ghost_type: Option<u16>, kbn: Option<u8>) -> Mode {
    match (kbn, ghost_type) {
        (Some(0), _) => Mode::Friendly,
        (Some(_), _) => Mode::NetworkVs,
        (None, Some(34)) => Mode::GrandTrophy,
        _ => Mode::Other,
    }
}

/// The opponent of an end of play (SAVE_DATA_SET 277, at least 3,300 bytes): its coachId at +3,296 is 0 for a CPU club.
pub fn opponent(req277: &[u8]) -> &'static str {
    if u32_at(req277, 3296) == 0 {
        "CPU"
    } else {
        "player"
    }
}

#[derive(Debug, Default)]
struct Seat {
    aime: u32,
    playing: bool,
    /// The last 305 match type and 243 kbn since the seat's last end of play.
    ghost: Option<u16>,
    kbn: Option<u8>,
    said: Option<Mode>,
}

impl Seat {
    fn mode(&self) -> Mode {
        mode(self.ghost, self.kbn)
    }
}

#[derive(Debug, Default)]
struct Inner {
    next: u64,
    /// Open connections: slot -> seat.
    conns: HashMap<u64, HashMap<u32, Seat>>,
    /// Ends of play since the last [`Sessions::summary`], by mode.
    finished: BTreeMap<Mode, u32>,
}

/// The game server's open connections and their seats, shared with the desk (`forza-server all`).
#[derive(Debug, Default)]
pub struct Sessions(Mutex<Inner>);

impl Sessions {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Distinct cards logged in on open connections (one card on two connections counts once; players behind one NAT
    /// address count apart).
    pub fn cards(&self) -> usize {
        let i = self.lock();
        let cards: HashSet<u32> = i
            .conns
            .values()
            .flat_map(|s| s.values().map(|s| s.aime))
            .collect();
        cards.len()
    }

    /// The periodic line: cards online, connections with a card, plays under way, and the ends of play since the last
    /// call (which it resets). None while nobody is on and nothing ended.
    pub fn summary(&self) -> Option<String> {
        let mut i = self.lock();
        let finished = std::mem::take(&mut i.finished);
        let busy: Vec<_> = i.conns.values().filter(|s| !s.is_empty()).collect();
        if busy.is_empty() && finished.is_empty() {
            return None;
        }
        let playing = busy
            .iter()
            .flat_map(|s| s.values())
            .filter(|s| s.playing)
            .count();
        let cards: HashSet<u32> = busy
            .iter()
            .flat_map(|s| s.values().map(|s| s.aime))
            .collect();
        let ended = if finished.is_empty() {
            "none".to_owned()
        } else {
            finished
                .iter()
                .map(|(m, n)| format!("{n} {}", m.name()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        Some(format!(
            "online: {} card(s), {} session(s), {playing} match(es) in progress; ended since the last line: {ended}",
            cards.len(),
            busy.len()
        ))
    }

    /// A new connection from `peer`; dropping the [`Session`] ends it.
    pub fn open(self: &Arc<Self>, peer: SocketAddr, log: Log) -> Session {
        let id = {
            let mut i = self.lock();
            i.next += 1;
            let id = i.next;
            i.conns.insert(id, HashMap::new());
            id
        };
        let now = Instant::now();
        Session {
            all: self.clone(),
            id,
            peer,
            log,
            since: now,
            matches: 0,
            cards: 0,
            polls: (0, now),
        }
    }
}

/// How often a connection's vslink lookups (904, every 0.5 s while a VS link is up) are said as one count.
pub const POLL_LINE_EVERY: Duration = Duration::from_secs(60);

/// One open connection.
pub struct Session {
    all: Arc<Sessions>,
    id: u64,
    peer: SocketAddr,
    log: Log,
    since: Instant,
    matches: u32,
    /// Card logins on this connection.
    cards: u32,
    /// vslink 904 lookups not yet said, since when.
    polls: (u32, Instant),
}

impl Session {
    /// One answered request (`refused`: the store's reason when it answered errCode 1): updates the seat and returns
    /// the lines for the log.
    pub fn request(&mut self, cmd: u16, req: &[u8], refused: Option<&str>) -> Vec<String> {
        let mut out = Vec::new();
        if req.len() < 4 {
            return out;
        }
        let (slot, id) = (u32_at(req, 0), self.id);
        let mut i = self.all.lock();
        let Inner {
            conns, finished, ..
        } = &mut *i;
        let Some(seats) = conns.get_mut(&id) else {
            return out;
        };
        match cmd {
            271 if req.len() >= 20 && refused.is_none() => {
                let aime = u32_at(req, 4);
                if aime != 0 && seats.get(&slot).is_none_or(|s| s.aime != aime) {
                    seats.insert(
                        slot,
                        Seat {
                            aime,
                            ..Seat::default()
                        },
                    );
                    self.cards += 1;
                    out.push(format!(
                        "card in: Aime id {aime}, seat {slot}, conn {id} from {}, 2017-18 rev {}",
                        self.peer,
                        u32_at(req, 16)
                    ));
                }
            }
            289 => {
                if let Some(line) = match_line(cmd, req, refused) {
                    if let Some(s) = seats.get_mut(&slot) {
                        s.playing = true;
                    }
                    out.push(format!("{line}, seat {slot} conn {id}"));
                }
            }
            243 | 305 => {
                if let Some(s) = seats.get_mut(&slot) {
                    if cmd == 243 && req.len() > 16 {
                        s.kbn = Some(req[16]);
                    }
                    if cmd == 305 && req.len() >= 12 {
                        s.ghost = Some(u16::from_le_bytes([req[10], req[11]]));
                    }
                    let m = s.mode();
                    if s.said != Some(m) {
                        s.said = Some(m);
                        out.push(format!(
                            "match mode: Aime id {}, seat {slot} conn {id}: {}",
                            s.aime,
                            m.name()
                        ));
                    }
                }
            }
            277 => {
                if let Some(line) = match_line(cmd, req, refused) {
                    let s = seats.get_mut(&slot);
                    let m = s.as_ref().map_or(Mode::Other, |s| s.mode());
                    out.push(format!(
                        "{line}; {}, opponent {}, seat {slot} conn {id}",
                        m.name(),
                        opponent(req)
                    ));
                    // save kind 2 = end of play (1: a new club's first save, 3: the club moved to a new card)
                    if req[24] == 2 {
                        self.matches += 1;
                        *finished.entry(m).or_default() += 1;
                        if let Some(s) = s {
                            *s = Seat {
                                aime: s.aime,
                                ..Seat::default()
                            };
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// One vslink lookup 904, kept off the log: a line with the count once [`POLL_LINE_EVERY`] has passed.
    pub fn vslink_poll(&mut self) -> Option<String> {
        self.polls.0 += 1;
        (self.polls.1.elapsed() >= POLL_LINE_EVERY).then(|| self.polls_line())
    }

    fn polls_line(&mut self) -> String {
        let (n, since) = std::mem::replace(&mut self.polls, (0, Instant::now()));
        format!(
            "{}: {n} vslink lookup(s) (cmd 904) in {} s, conn {}",
            self.peer,
            since.elapsed().as_secs(),
            self.id
        )
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.all.lock().conns.remove(&self.id);
        if self.polls.0 > 0 {
            let line = self.polls_line();
            (self.log)(&line);
        }
        if self.cards > 0 {
            let s = self.since.elapsed().as_secs();
            (self.log)(&format!(
                "session end: conn {} from {} after {} min {} s, {} card(s), {} match(es) played",
                self.id,
                self.peer,
                s / 60,
                s % 60,
                self.cards,
                self.matches
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(len: usize, words: &[(usize, u32)]) -> Vec<u8> {
        let mut r = vec![0u8; len];
        for &(at, v) in words {
            r[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        r
    }

    #[test]
    fn mode_and_opponent_from_the_requests() {
        assert_eq!(mode(None, None), Mode::Other);
        assert_eq!(mode(Some(34), None), Mode::GrandTrophy);
        assert_eq!(mode(Some(33), None), Mode::Other);
        assert_eq!(mode(Some(34), Some(0)), Mode::Friendly);
        assert_eq!(mode(None, Some(3)), Mode::NetworkVs);
        assert_eq!(opponent(&req(8572, &[])), "CPU");
        assert_eq!(opponent(&req(8572, &[(3296, 13)])), "player");
    }

    #[test]
    fn a_session_counts_cards_matches_and_modes() {
        let said: Arc<Mutex<Vec<String>>> = Arc::default();
        let log: Log = {
            let said = said.clone();
            Arc::new(move |l: &str| said.lock().unwrap().push(l.to_owned()))
        };
        let all = Arc::new(Sessions::default());
        let peer: SocketAddr = "192.0.2.1:5000".parse().unwrap();
        assert_eq!(all.summary(), None, "idle: no line");
        let mut a = all.open(peer, log.clone());
        let mut b = all.open(peer, log.clone());
        assert_eq!(all.summary(), None, "connected without a card: no line");
        let login = |slot, aime| req(20, &[(0, slot), (4, aime), (16, 47565)]);
        let lines = a.request(271, &login(1, 77), None);
        assert_eq!(
            lines,
            ["card in: Aime id 77, seat 1, conn 1 from 192.0.2.1:5000, 2017-18 rev 47565"]
        );
        assert!(
            a.request(271, &login(1, 77), None).is_empty(),
            "the same card again"
        );
        assert!(
            a.request(271, &login(2, 88), Some("revision 1")).is_empty(),
            "refused"
        );
        b.request(271, &login(1, 77), None);
        b.request(271, &login(2, 99), None);
        assert_eq!(all.cards(), 2, "by card, not by connection");
        // a Grand Trophy play against the CPU
        a.request(
            289,
            &req(28, &[(0, 1), (4, 77), (8, 13), (12, 13), (16, 1)]),
            None,
        );
        let m = a.request(305, &req(24, &[(0, 1), (8, 0x0022_0001)]), None);
        assert_eq!(m, ["match mode: Aime id 77, seat 1 conn 1: Grand Trophy"]);
        assert!(
            all.summary()
                .unwrap()
                .contains("1 match(es) in progress; ended since the last line: none")
        );
        let mut end = req(8572, &[(0, 1), (4, 77), (8, 13)]);
        end[24] = 2;
        let e = a.request(277, &end, None);
        assert!(
            e[0].starts_with("match end: aime 77 coach 13 vs ")
                && e[0].ends_with("; Grand Trophy, opponent CPU, seat 1 conn 1"),
            "{e:?}"
        );
        assert_eq!(
            all.summary().unwrap(),
            "online: 2 card(s), 2 session(s), 0 match(es) in progress; ended since the last line: 1 Grand Trophy"
        );
        drop(a);
        assert_eq!(all.cards(), 2, "77 is still in on b");
        drop(b);
        assert_eq!(all.cards(), 0);
        assert_eq!(all.summary(), None);
        let said = said.lock().unwrap();
        assert!(
            said[0].starts_with(
                "session end: conn 1 from 192.0.2.1:5000 after 0 min 0 s, 1 card(s), 1 match(es)"
            ),
            "{said:?}"
        );
    }
}
