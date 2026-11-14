//! 2017-18 arcade rooms (M2 plan 5.3): a player's PC that hosts an arcade opens a room with `CREATE shop ID [public]`,
//! where ID is its arcade tunnel's iroh EndpointId. A room is private: guests join with its code alone (`JOIN CODE
//! [n]`, seats 2-8 (1-8 in a seatless room)), which the host passes on; `RESERVE` of a room's id gives nothing. A guest's ticket then gets the
//! host's id through `SHOP t own-id`, the host's ticket the ids of the guests that asked, so each tunnel end learns
//! the other's id from the room alone. `LOBBY` lists only `public` rooms, as an arcade row with x/8 and never the code;
//! `JOIN` of a public row's id seats a guest too (the host chose to list it), a private room's id joins nothing.
//! A server that hosts an arcade and plays no game (forza-arcade, `CREATE seatless`) takes no room seat: guests 1-8.
//! Sans-IO: the desk passes the clock, the random ids and tickets in.

pub use forza::online::lobby::Peers;
use forza::online::lobby::{Arcade, Reserved, State as ArcadeState};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Seats in an arcade room: the host on 1, guests on 2-8; a seatless host on 0, guests on 1-8.
const SEATS: std::ops::RangeInclusive<u8> = 1..=8;
/// A room's code: this many of the desk's code letters (31^8, about 2^39.6, against `JOIN`'s budgets).
pub const CODE_LEN: usize = 8;
/// Room secrets whose last code the desk keeps for their next `CREATE shop`, in all and per creator's address. Full:
/// a new secret's code is not kept (strangers can fill the list, never empty it of someone else's code).
const KEPT: usize = 4096;
const KEPT_PER_ADDRESS: usize = 4;
/// A kept code lapses this long after its last room ended.
const KEPT_FOR: Duration = Duration::from_secs(30 * 24 * 3600);

/// A host's room secret as the desk keeps it: its SHA-256, never the secret itself.
pub type SecretHash = [u8; 32];

/// A room secret's last code ([`ArcadeRooms::kept_code`]).
struct Kept {
    secret: SecretHash,
    code: String,
    /// The last creator's address, for [`KEPT_PER_ADDRESS`]
    by: String,
    /// Lapses then, unless a live room holds the code (then pushed on)
    until: Instant,
}

/// An iroh EndpointId as written: 64 hex digits that are an ed25519 public key (iroh's `PublicKey::from_bytes`), in
/// its canonical form and not one of the small-order points no secret key makes.
pub fn valid_id(s: &str) -> bool {
    if !forza::online::lobby::valid_endpoint_id(s) {
        return false;
    }
    let mut b = [0u8; 32];
    for (o, pair) in b.iter_mut().zip(s.as_bytes().chunks(2)) {
        let Some(v) = std::str::from_utf8(pair)
            .ok()
            .and_then(|p| u8::from_str_radix(p, 16).ok())
        else {
            return false;
        };
        *o = v;
    }
    // canonical (y below the field's prime 2^255 - 19; all-ff is not) and not of small order: what a real key is
    let mut y = b;
    y[31] &= 0x7f;
    let canonical = !(y[31] == 0x7f && y[1..31].iter().all(|&v| v == 0xff) && y[0] >= 0xed);
    canonical && ed25519_dalek::VerifyingKey::from_bytes(&b).is_ok_and(|k| !k.is_weak())
}

struct Sitter {
    seat: u8,
    ticket: String,
    end: Instant,
    /// The tunnel id `SHOP` told (the host's from `CREATE`), lower case
    id: Option<String>,
}

struct Room {
    id: String,
    code: String,
    /// The creator's address, for the per-address room cap
    by: String,
    /// The hash of the host's room secret: only that secret replaces the room ([`ArcadeRooms::replace_stale`])
    secret: Option<SecretHash>,
    /// Listed in `LOBBY` (without its code)
    public: bool,
    /// Its name in `LOBBY`, from `CREATE shop` ([`forza::online::lobby::arcade_name`]); empty: the desk's default
    name: String,
    /// seats[0] is the host (seat 1, or 0 when it takes none)
    seats: Vec<Sitter>,
}

impl Room {
    /// The host's tunnel id, lower case.
    fn host_id(&self) -> Option<&str> {
        self.seats.first()?.id.as_deref()
    }
}

/// What a host's earlier room is to its new `CREATE shop` ([`ArcadeRooms::replace_stale`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Prior {
    None,
    /// Gone quiet, so taken out: its code.
    Replaced(String),
    Alive,
}

#[derive(Default)]
pub struct ArcadeRooms {
    rooms: Vec<Room>,
    /// Each room secret's last code: the host back from a restart gets it again, so guests keep the one they were
    /// given. Memory only: a desk restart draws anew.
    kept: Vec<Kept>,
}

impl ArcadeRooms {
    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// Whether `id` or `code` is a room's id or code already, or `code` another secret's kept code (the desk draws
    /// again): `JOIN` takes either.
    pub fn holds(&self, id: &str, code: &str, secret: Option<&SecretHash>) -> bool {
        self.rooms.iter().any(|r| {
            [&r.id, &r.code]
                .iter()
                .any(|k| k.eq_ignore_ascii_case(id) || k.eq_ignore_ascii_case(code))
        }) || self
            .kept
            .iter()
            .any(|k| k.code.eq_ignore_ascii_case(code) && Some(&k.secret) != secret)
    }

    /// The code of the last room made with this room secret: only its holder gets it, whatever its tunnel id or
    /// address.
    pub fn kept_code(&self, secret: &SecretHash) -> Option<String> {
        self.kept
            .iter()
            .find(|k| k.secret == *secret)
            .map(|k| k.code.clone())
    }

    /// Whether `id` is an arcade room's (its `RESERVE` answers `CLOSED`: a room is joined by code).
    pub fn has_id(&self, id: &str) -> bool {
        self.rooms.iter().any(|r| r.id == id)
    }

    /// Whether the tunnel `host` holds a live room (one room per host).
    pub fn has_host(&self, host: &str) -> bool {
        self.rooms
            .iter()
            .any(|r| r.host_id().is_some_and(|h| h.eq_ignore_ascii_case(host)))
    }

    /// Before a `CREATE shop` by tunnel `host` from address `by` with `secret`: no room of its yet, or its earlier
    /// rooms (that tunnel id's, or that secret's under any id) taken out once quiet, else alive. A room made with a
    /// secret is quiet to that secret alone, when its lease runs to no later than `quiet` (no `HEARTBEAT` for a while):
    /// a secret read off the wire never takes a live room, nor does whoever learnt the tunnel id. One made without (a
    /// Launcher before secrets) is quiet to a `CREATE` from its creator's address by `old_quiet`.
    pub fn replace_stale(
        &mut self,
        host: &str,
        by: &str,
        secret: Option<&SecretHash>,
        quiet: Instant,
        old_quiet: Instant,
    ) -> Prior {
        let mine = |r: &Room| {
            (secret.is_some() && r.secret.as_ref() == secret)
                || r.host_id().is_some_and(|h| h.eq_ignore_ascii_case(host))
        };
        let stale = |r: &Room| match &r.secret {
            Some(s) => Some(s) == secret && r.seats.first().is_none_or(|h| h.end <= quiet),
            None => r.by == by && r.seats.first().is_none_or(|h| h.end <= old_quiet),
        };
        if self.rooms.iter().any(|r| mine(r) && !stale(r)) {
            return Prior::Alive;
        }
        let mut old = Vec::new();
        self.rooms.retain(|r| {
            let keep = !mine(r);
            if !keep {
                old.push(r.code.clone());
            }
            keep
        });
        if old.is_empty() {
            Prior::None
        } else {
            Prior::Replaced(old.join(" "))
        }
    }

    /// Rooms open now that `by` made.
    pub fn count_by(&self, by: &str) -> usize {
        self.rooms.iter().filter(|r| r.by == by).count()
    }

    /// A new room with its host on seat 1, or on none when `seatless` (`host_id` checked by the caller with
    /// [`valid_id`], `name` already [`forza::online::lobby::arcade_name`]); its code kept for `secret`'s next room,
    /// when there is space.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &mut self,
        id: String,
        code: String,
        host_id: &str,
        by: &str,
        secret: Option<SecretHash>,
        public: bool,
        name: String,
        ticket: String,
        end: Instant,
        seatless: bool,
    ) {
        if let Some(secret) = secret {
            let until = end + KEPT_FOR;
            if let Some(k) = self.kept.iter_mut().find(|k| k.secret == secret) {
                (k.code, k.by, k.until) = (code.clone(), by.to_owned(), until);
            } else if self.kept.len() < KEPT
                && self.kept.iter().filter(|k| k.by == by).count() < KEPT_PER_ADDRESS
            {
                self.kept.push(Kept {
                    secret,
                    code: code.clone(),
                    by: by.to_owned(),
                    until,
                });
            }
        }
        self.rooms.push(Room {
            id,
            code,
            by: by.to_owned(),
            secret,
            public,
            name,
            seats: vec![Sitter {
                seat: u8::from(!seatless),
                ticket,
                end,
                id: Some(host_id.to_ascii_lowercase()),
            }],
        });
    }

    /// `JOIN CODE|ID [n]`: the lowest free guest seat (or n) of the room with that code, or of the public room with
    /// that id (any case), `FULL`, `TAKEN`, or `CLOSED` (no such code or public id).
    pub fn join(&mut self, code: &str, seat: Option<u8>, ticket: String, end: Instant) -> Reserved {
        let Some(r) = self.rooms.iter_mut().find(|r| {
            r.code.eq_ignore_ascii_case(code) || (r.public && r.id.eq_ignore_ascii_case(code))
        }) else {
            return Reserved::Closed;
        };
        let busy = |n: u8| r.seats.iter().any(|s| s.seat == n);
        let n = match seat {
            Some(n) if !SEATS.contains(&n) || busy(n) => return Reserved::Taken,
            Some(n) => n,
            None => match SEATS.clone().find(|n| !busy(*n)) {
                Some(n) => n,
                None => return Reserved::Full,
            },
        };
        r.seats.push(Sitter {
            seat: n,
            ticket: ticket.clone(),
            end,
            id: None,
        });
        Reserved::Ticket { ticket, seat: n }
    }

    fn find(&mut self, t: &str) -> Option<(usize, usize)> {
        self.rooms.iter().enumerate().find_map(|(r, room)| {
            room.seats
                .iter()
                .position(|s| s.ticket == t)
                .map(|s| (r, s))
        })
    }

    /// `HEARTBEAT t`: the ticket's lease runs to `end`; false when it is no arcade room's.
    pub fn heartbeat(&mut self, t: &str, end: Instant) -> bool {
        self.find(t)
            .and_then(|(r, s)| self.rooms.get_mut(r)?.seats.get_mut(s))
            .map(|s| s.end = end)
            .is_some()
    }

    /// `RELEASE t` (or `CLOSE id t` with the room's id): a guest leaves its seat, the host closes the room (its code
    /// returned, for the log). `None`: no arcade room's ticket.
    pub fn leave(&mut self, id: Option<&str>, t: &str) -> Option<Option<String>> {
        let (r, s) = self.find(t)?;
        if id.is_some_and(|id| self.rooms.get(r).is_none_or(|room| id != room.id)) {
            return None;
        }
        if s == 0 {
            return Some(Some(self.rooms.remove(r).code));
        }
        self.rooms.get_mut(r)?.seats.remove(s);
        Some(None)
    }

    /// `SHOP t [own-id]`: a guest's ticket records its tunnel id (when given) and gets the host's; the host's ticket
    /// gets the guests' ids. `None`: no arcade room's ticket (`GONE`).
    pub fn peers(&mut self, t: &str, own: Option<&str>) -> Option<Peers> {
        let (r, s) = self.find(t)?;
        let room = self.rooms.get_mut(r)?;
        if s == 0 {
            let mut guests: Vec<&Sitter> = room
                .seats
                .iter()
                .skip(1)
                .filter(|g| g.id.is_some())
                .collect();
            guests.sort_by_key(|g| g.seat);
            return Some(Peers::Guests(
                guests.iter().filter_map(|g| g.id.clone()).collect(),
            ));
        }
        if let (Some(id), Some(g)) = (own, room.seats.get_mut(s)) {
            g.id = Some(id.to_ascii_lowercase());
        }
        room.host_id().map(|h| Peers::Host(h.to_owned()))
    }

    /// Drops guests whose lease ended and rooms whose host's did; the closed rooms' codes.
    pub fn expire(&mut self, now: Instant) -> Vec<String> {
        for r in &mut self.rooms {
            // the host (first) stays: its own lease closes the room below
            let mut host = true;
            r.seats.retain(|s| std::mem::take(&mut host) || s.end > now);
        }
        let mut closed = Vec::new();
        self.rooms.retain(|r| {
            let live = r.seats.first().is_some_and(|h| h.end > now);
            if !live {
                closed.push(r.code.clone());
            }
            live
        });
        let live: HashSet<&str> = self.rooms.iter().map(|r| r.code.as_str()).collect();
        self.kept.retain_mut(|k| {
            if live.contains(k.code.as_str()) {
                k.until = now + KEPT_FOR;
            }
            k.until > now
        });
        closed
    }

    /// The public rooms as lobby rows: id (which `JOIN` takes), seats taken of 8. Never the code.
    pub fn listed(&self) -> Vec<Arcade> {
        self.rooms
            .iter()
            .filter(|r| r.public)
            .map(|r| {
                let seats = r.seats.iter().filter(|s| s.seat != 0).count() as u8;
                Arcade {
                    id: r.id.clone(),
                    name: if r.name.is_empty() {
                        forza::online::lobby::ARCADE_ROOM_NAME.into()
                    } else {
                        r.name.clone()
                    },
                    seats,
                    max: 8,
                    state: if seats >= 8 {
                        ArcadeState::Full
                    } else {
                        ArcadeState::Open
                    },
                    next_entry: None,
                    lang: None,
                    room: true,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A real tunnel id: the public key of a fixed secret.
    fn key(seed: u8) -> String {
        let k = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]).verifying_key();
        k.to_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn room(public: bool) -> (ArcadeRooms, Instant, String) {
        let now = Instant::now();
        let mut r = ArcadeRooms::default();
        let host = key(1);
        r.create(
            "r1".into(),
            "K7QMZDRX".into(),
            &host,
            "10.0.0.1",
            Some([1; 32]),
            public,
            String::new(),
            "th".into(),
            now + Duration::from_secs(60),
            false,
        );
        (r, now, host)
    }

    /// A room for `secret` from `by` with code `code`, its host's lease 60 s from `now`.
    fn make(r: &mut ArcadeRooms, now: Instant, code: &str, by: &str, secret: Option<SecretHash>) {
        let t = format!("t{code}");
        r.create(
            format!("r{code}"),
            code.into(),
            &key(2),
            by,
            secret,
            false,
            String::new(),
            t,
            now + Duration::from_secs(60),
            false,
        );
    }

    /// 64 hex digits are not enough: they must be a point iroh takes as a public key.
    #[test]
    fn ids_are_real_public_keys() {
        let host = key(1);
        assert!(valid_id(&host) && valid_id(&host.to_uppercase()));
        assert!(!valid_id(&"ff".repeat(32)), "all ff: hex, but no key");
        assert!(!valid_id(&"00".repeat(32)), "a small-order point");
        let mut identity = "00".repeat(32);
        identity.replace_range(..2, "01");
        assert!(!valid_id(&identity), "the identity point");
        assert!(
            !valid_id(host.get(..63).unwrap())
                && !valid_id(&format!("{host}0"))
                && !valid_id(&"zz".repeat(32))
        );
    }

    #[test]
    fn guests_join_by_code_on_seats_2_to_8_then_full() {
        let (mut r, now, _) = room(false);
        let end = now + Duration::from_secs(60);
        for n in 2..=8u8 {
            let code = if n % 2 == 0 { "K7QMZDRX" } else { "k7qmzdrx" };
            assert_eq!(
                r.join(code, None, format!("t{n}"), end),
                Reserved::Ticket {
                    ticket: format!("t{n}"),
                    seat: n
                }
            );
        }
        assert_eq!(r.join("K7QMZDRX", None, "t9".into(), end), Reserved::Full);
        assert_eq!(
            r.join("K7QMZDRX", Some(1), "x".into(), end),
            Reserved::Taken,
            "the host's seat"
        );
        assert_eq!(
            r.join("K7QMZDRY", None, "x".into(), end),
            Reserved::Closed,
            "no such code"
        );
        assert!(r.has_id("r1") && !r.has_id("r2"));
        assert!(r.has_host(&key(1).to_uppercase()) && !r.has_host(&key(2)));
        assert_eq!(r.count_by("10.0.0.1"), 1);
        assert_eq!(r.count_by("10.0.0.2"), 0);
    }

    /// A player's arcade counts its host, who plays on seat 1: 1/8 empty, full at 7 guests.
    #[test]
    fn a_players_arcade_counts_its_host() {
        let (mut r, now, _) = room(true);
        assert_eq!(r.listed()[0].seats, 1);
        let end = now + Duration::from_secs(60);
        for n in 2..=8u8 {
            r.join("K7QMZDRX", None, format!("t{n}"), end);
        }
        assert_eq!(
            (r.listed()[0].seats, r.listed()[0].state),
            (8, ArcadeState::Full)
        );
    }

    /// A server-run arcade (forza-arcade) plays no game: its host takes no seat, so 8 guests fit on seats 1-8.
    #[test]
    fn a_seatless_host_leaves_8_seats_to_guests() {
        let now = Instant::now();
        let end = now + Duration::from_secs(60);
        let mut r = ArcadeRooms::default();
        r.create(
            "r1".into(),
            "K7QMZDRX".into(),
            &key(1),
            "10.0.0.1",
            None,
            true,
            String::new(),
            "th".into(),
            end,
            true,
        );
        let row = |r: &ArcadeRooms| (r.listed()[0].seats, r.listed()[0].max, r.listed()[0].state);
        assert_eq!(row(&r), (0, 8, ArcadeState::Open), "empty: 0/8");
        for n in 1..=8u8 {
            assert_eq!(
                r.join("r1", None, format!("t{n}"), end),
                Reserved::Ticket {
                    ticket: format!("t{n}"),
                    seat: n
                }
            );
        }
        assert_eq!(row(&r), (8, 8, ArcadeState::Full));
        assert_eq!(
            r.join("r1", None, "t9".into(), end),
            Reserved::Full,
            "a 9th"
        );
        // the host still keeps the room: its lease, its guests' ids, closing it
        r.peers("t1", Some(&key(2)));
        assert_eq!(r.peers("th", None), Some(Peers::Guests(vec![key(2)])));
        assert!(r.expire(now + Duration::from_secs(30)).is_empty());
        assert_eq!(r.leave(None, "t1"), Some(None), "a guest leaves seat 1");
        assert_eq!(row(&r).0, 7);
        assert_eq!(
            r.join("r1", Some(1), "t1b".into(), end),
            Reserved::Ticket {
                ticket: "t1b".into(),
                seat: 1
            },
            "seat 1 is a guest's"
        );
        assert_eq!(
            r.leave(None, "th"),
            Some(Some("K7QMZDRX".into())),
            "the host closes it"
        );
    }

    /// Each end learns the other's id from the room: the guest the host's, the host the guests' that asked.
    #[test]
    fn the_room_swaps_tunnel_ids() {
        let (mut r, now, host) = room(false);
        let guest = key(2);
        r.join("K7QMZDRX", None, "tg".into(), now + Duration::from_secs(60));
        assert_eq!(
            r.peers("th", None),
            Some(Peers::Guests(vec![])),
            "no guest asked yet"
        );
        assert_eq!(
            r.peers("tg", Some(&guest.to_uppercase())),
            Some(Peers::Host(host))
        );
        assert_eq!(
            r.peers("th", None),
            Some(Peers::Guests(vec![guest.clone()]))
        );
        assert_eq!(r.peers("nope", None), None);
        let line = Peers::Guests(vec![guest.clone()]).line();
        assert_eq!(Peers::parse(&line), Some(Peers::Guests(vec![guest])));
        assert_eq!(Peers::parse("GUESTS\n"), Some(Peers::Guests(vec![])));
        assert_eq!(Peers::parse("HOST 1.2.3.4"), None);
    }

    #[test]
    fn leases_and_leaving() {
        let (mut r, now, _) = room(false);
        r.join("K7QMZDRX", None, "tg".into(), now + Duration::from_secs(10));
        assert!(r.heartbeat("tg", now + Duration::from_secs(120)));
        assert!(r.expire(now + Duration::from_secs(30)).is_empty());
        assert_eq!(r.rooms[0].seats.len(), 2, "the guest's lease was extended");
        assert_eq!(
            r.expire(now + Duration::from_secs(61)),
            vec!["K7QMZDRX".to_string()],
            "the host's lease ended"
        );
        assert!(r.is_empty());

        let (mut r, now, _) = room(false);
        r.join("K7QMZDRX", None, "tg".into(), now + Duration::from_secs(60));
        assert_eq!(r.leave(None, "tg"), Some(None), "a guest leaves its seat");
        assert_eq!(
            r.leave(Some("other"), "th"),
            None,
            "CLOSE needs the room's id"
        );
        assert_eq!(
            r.leave(Some("r1"), "th"),
            Some(Some("K7QMZDRX".into())),
            "the host closes the room"
        );
        assert_eq!(r.leave(None, "th"), None);
    }

    /// A secret's last code stays for its next room, from any tunnel id or address; another secret neither gets it
    /// nor draws it, nor takes it away by making rooms of its own.
    #[test]
    fn a_secret_keeps_its_code_across_rooms() {
        let (mut r, now, _) = room(false);
        assert_eq!(r.leave(Some("r1"), "th"), Some(Some("K7QMZDRX".into())));
        assert_eq!(r.kept_code(&[1; 32]).as_deref(), Some("K7QMZDRX"));
        assert_eq!(r.kept_code(&[2; 32]), None, "another secret, same address");
        assert!(
            r.holds("r2", "k7qmzdrx", Some(&[2; 32])),
            "kept for its secret"
        );
        assert!(r.holds("r2", "K7QMZDRX", None), "nor drawn without one");
        assert!(!r.holds("r2", "K7QMZDRX", Some(&[1; 32])));
        make(&mut r, now, "AAAAAAAA", "10.0.0.1", Some([2; 32]));
        make(&mut r, now, "BBBBBBBB", "10.0.0.1", None);
        assert_eq!(
            r.kept_code(&[1; 32]).as_deref(),
            Some("K7QMZDRX"),
            "still its"
        );
    }

    /// A secret read off the wire never takes a live room, under its tunnel id or another; once quiet, its own secret
    /// replaces it from any id. A room without a secret is replaced only once quiet by the old rule, from its
    /// creator's address.
    #[test]
    fn a_room_is_replaced_only_once_quiet() {
        let (mut r, now, host) = room(false);
        let (live, quiet) = (now, now + Duration::from_secs(60));
        let mine = Some(&[1; 32]);
        for (id, why) in [(&host, "its tunnel id"), (&key(9), "another id")] {
            let p = r.replace_stale(id, "10.9.9.9", mine, live, quiet);
            assert_eq!(p, Prior::Alive, "live, {why}");
        }
        for other in [Some(&[2; 32]), None] {
            let p = r.replace_stale(&host, "10.0.0.1", other, quiet, quiet);
            assert_eq!(p, Prior::Alive, "{other:?}");
        }
        assert_eq!(
            r.replace_stale(&key(9), "10.9.9.9", mine, quiet, live),
            Prior::Replaced("K7QMZDRX".into())
        );
        assert!(r.is_empty());
        make(&mut r, now, "AAAAAAAA", "10.0.0.1", None);
        let p = r.replace_stale(&key(2), "10.0.0.2", None, quiet, quiet);
        assert_eq!(p, Prior::Alive, "another address");
        let p = r.replace_stale(&key(2), "10.0.0.1", mine, quiet, live);
        assert_eq!(p, Prior::Alive, "not quiet by the old rule yet");
        assert_eq!(
            r.replace_stale(&key(2), "10.0.0.1", None, live, quiet),
            Prior::Replaced("AAAAAAAA".into())
        );
        assert_eq!(
            r.replace_stale(&key(2), "10.0.0.1", None, live, live),
            Prior::None
        );
    }

    /// The kept list is bounded per address and in all: a full list keeps no new code and drops nobody's.
    #[test]
    fn kept_codes_are_capped_per_address_and_in_all() {
        let now = Instant::now();
        let mut r = ArcadeRooms::default();
        for n in 0..KEPT_PER_ADDRESS as u8 + 1 {
            make(&mut r, now, &format!("A{n}"), "10.0.0.1", Some([n; 32]));
        }
        assert_eq!(r.kept.len(), KEPT_PER_ADDRESS);
        assert_eq!(
            r.kept_code(&[0; 32]).as_deref(),
            Some("A0"),
            "the first stays"
        );
        assert_eq!(r.kept_code(&[KEPT_PER_ADDRESS as u8; 32]), None);
        let mut r = ArcadeRooms::default();
        for n in 0..=KEPT as u16 {
            let mut secret = [0; 32];
            secret[..2].copy_from_slice(&n.to_le_bytes());
            make(
                &mut r,
                now,
                &format!("B{n}"),
                &format!("10.1.{}.{}", n >> 8, n & 255),
                Some(secret),
            );
        }
        assert_eq!(r.kept.len(), KEPT);
        assert_eq!(r.kept_code(&[0; 32]).as_deref(), Some("B0"));
    }

    /// A kept code lapses KEPT_FOR after its room ended; while a live room holds it, it stays.
    #[test]
    fn kept_codes_lapse() {
        let (mut r, now, _) = room(false);
        let later = now + Duration::from_secs(60) + KEPT_FOR;
        r.heartbeat("th", later + Duration::from_secs(60));
        r.expire(later);
        assert_eq!(r.kept_code(&[1; 32]).as_deref(), Some("K7QMZDRX"), "live");
        r.leave(Some("r1"), "th");
        r.expire(later + KEPT_FOR - Duration::from_secs(1));
        assert_eq!(r.kept_code(&[1; 32]).as_deref(), Some("K7QMZDRX"));
        r.expire(later + KEPT_FOR);
        assert_eq!(r.kept_code(&[1; 32]), None, "lapsed");
    }

    /// Private rooms are not listed and their id joins nothing; a public one shows x/8 and its id, never the code, a
    /// tunnel id or a ticket.
    #[test]
    fn lobby_rows_show_no_secret() {
        let (mut r, now, _) = room(false);
        assert!(r.listed().is_empty(), "private by default");
        let end = now + Duration::from_secs(60);
        assert_eq!(r.join("r1", None, "tx".into(), end), Reserved::Closed);
        let (mut r, now, host) = room(true);
        r.join("K7QMZDRX", None, "tg".into(), now + Duration::from_secs(60));
        r.peers("tg", Some(&key(2)));
        let json = serde_json::to_string(&r.listed()).unwrap();
        for secret in [
            host.as_str(),
            key(2).as_str(),
            "\"th\"",
            "\"tg\"",
            "K7QMZDRX",
            "10.0.0.1",
        ] {
            assert!(!json.contains(secret), "{secret} in {json}");
        }
        assert!(
            json.contains("\"seats\":2") && json.contains("\"max\":8"),
            "{json}"
        );
    }
}
