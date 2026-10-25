//! 2017-18 shop rooms (M2 plan 5.3): a player's PC that hosts a shop opens a room with `CREATE shop ID [public]`,
//! where ID is its shop tunnel's iroh EndpointId. A room is private: guests join with its code alone (`JOIN CODE
//! [n]`, seats 2-8), which the host passes on; `RESERVE` of a room's id gives nothing. A guest's ticket then gets the
//! host's id through `SHOP t own-id`, the host's ticket the ids of the guests that asked, so each tunnel end learns
//! the other's id from the room alone. `LOBBY` lists only `public` rooms, as a shop row with x/8 and never the code,
//! so nothing in it lets a stranger join. Sans-IO: the desk passes the clock, the random ids and tickets in.

pub use forza::online::lobby::Peers;
use forza::online::lobby::{Reserved, Shop, State as ShopState};
use std::time::Instant;

/// Seats in a shop room: the host on 1, guests on 2-8.
const SEATS: std::ops::RangeInclusive<u8> = 1..=8;
/// A room's code: this many of the desk's code letters (31^6, about 900 million, against `JOIN`'s rate limit).
pub const CODE_LEN: usize = 6;

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
    /// Listed in `LOBBY` (without its code)
    public: bool,
    /// seats[0] is the host (seat 1)
    seats: Vec<Sitter>,
}

#[derive(Default)]
pub struct ShopRooms {
    rooms: Vec<Room>,
}

impl ShopRooms {
    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// Whether `id` or `code` is a room's already (the desk draws again).
    pub fn holds(&self, id: &str, code: &str) -> bool {
        self.rooms.iter().any(|r| r.id == id || r.code == code)
    }

    /// Whether `id` is a shop room's (its `RESERVE` answers `CLOSED`: a room is joined by code).
    pub fn has_id(&self, id: &str) -> bool {
        self.rooms.iter().any(|r| r.id == id)
    }

    /// Rooms open now that `by` made.
    pub fn count_by(&self, by: &str) -> usize {
        self.rooms.iter().filter(|r| r.by == by).count()
    }

    /// A new room with its host on seat 1 (`host_id` checked by the caller with [`valid_id`]).
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &mut self,
        id: String,
        code: String,
        host_id: &str,
        by: &str,
        public: bool,
        ticket: String,
        end: Instant,
    ) {
        self.rooms.push(Room {
            id,
            code,
            by: by.to_owned(),
            public,
            seats: vec![Sitter {
                seat: 1,
                ticket,
                end,
                id: Some(host_id.to_ascii_lowercase()),
            }],
        });
    }

    /// `JOIN CODE [n]`: the lowest free guest seat (or n) of the room with that code (any case), `FULL`, `TAKEN`, or
    /// `CLOSED` (no such code).
    pub fn join(&mut self, code: &str, seat: Option<u8>, ticket: String, end: Instant) -> Reserved {
        let Some(r) = self
            .rooms
            .iter_mut()
            .find(|r| r.code.eq_ignore_ascii_case(code))
        else {
            return Reserved::Closed;
        };
        let busy = |n: u8| r.seats.iter().any(|s| s.seat == n);
        let n = match seat {
            Some(n) if n == 1 || !SEATS.contains(&n) || busy(n) => return Reserved::Taken,
            Some(n) => n,
            None => match SEATS.skip(1).find(|n| !busy(*n)) {
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

    /// `HEARTBEAT t`: the ticket's lease runs to `end`; false when it is no shop room's.
    pub fn heartbeat(&mut self, t: &str, end: Instant) -> bool {
        self.find(t)
            .map(|(r, s)| self.rooms[r].seats[s].end = end)
            .is_some()
    }

    /// `RELEASE t` (or `CLOSE id t` with the room's id): a guest leaves its seat, the host closes the room (its code
    /// returned, for the log). `None`: no shop room's ticket.
    pub fn leave(&mut self, id: Option<&str>, t: &str) -> Option<Option<String>> {
        let (r, s) = self.find(t)?;
        if id.is_some_and(|id| id != self.rooms[r].id) {
            return None;
        }
        if s == 0 {
            return Some(Some(self.rooms.remove(r).code));
        }
        self.rooms[r].seats.remove(s);
        Some(None)
    }

    /// `SHOP t [own-id]`: a guest's ticket records its tunnel id (when given) and gets the host's; the host's ticket
    /// gets the guests' ids. `None`: no shop room's ticket (`GONE`).
    pub fn peers(&mut self, t: &str, own: Option<&str>) -> Option<Peers> {
        let (r, s) = self.find(t)?;
        let room = &mut self.rooms[r];
        if s == 0 {
            let mut guests: Vec<&Sitter> =
                room.seats[1..].iter().filter(|g| g.id.is_some()).collect();
            guests.sort_by_key(|g| g.seat);
            return Some(Peers::Guests(
                guests.iter().filter_map(|g| g.id.clone()).collect(),
            ));
        }
        if let Some(id) = own {
            room.seats[s].id = Some(id.to_ascii_lowercase());
        }
        room.seats[0].id.clone().map(Peers::Host)
    }

    /// Drops guests whose lease ended and rooms whose host's did; the closed rooms' codes.
    pub fn expire(&mut self, now: Instant) -> Vec<String> {
        for r in &mut self.rooms {
            r.seats.retain(|s| s.seat == 1 || s.end > now);
        }
        let mut closed = Vec::new();
        self.rooms.retain(|r| {
            let live = r.seats[0].end > now;
            if !live {
                closed.push(r.code.clone());
            }
            live
        });
        closed
    }

    /// The public rooms as lobby rows: id (which joins nothing), seats taken of 8. Never the code.
    pub fn listed(&self) -> Vec<Shop> {
        self.rooms
            .iter()
            .filter(|r| r.public)
            .map(|r| {
                let seats = r.seats.len() as u8;
                Shop {
                    id: r.id.clone(),
                    name: "Shop room".into(),
                    seats,
                    max: 8,
                    state: if seats >= 8 {
                        ShopState::Full
                    } else {
                        ShopState::Open
                    },
                    next_entry: None,
                    lang: None,
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

    fn room(public: bool) -> (ShopRooms, Instant, String) {
        let now = Instant::now();
        let mut r = ShopRooms::default();
        let host = key(1);
        r.create(
            "r1".into(),
            "K7QMZD".into(),
            &host,
            "10.0.0.1",
            public,
            "th".into(),
            now + Duration::from_secs(60),
        );
        (r, now, host)
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
            !valid_id(&host[..63]) && !valid_id(&format!("{host}0")) && !valid_id(&"zz".repeat(32))
        );
    }

    #[test]
    fn guests_join_by_code_on_seats_2_to_8_then_full() {
        let (mut r, now, _) = room(false);
        let end = now + Duration::from_secs(60);
        for n in 2..=8u8 {
            let code = if n % 2 == 0 { "K7QMZD" } else { "k7qmzd" };
            assert_eq!(
                r.join(code, None, format!("t{n}"), end),
                Reserved::Ticket {
                    ticket: format!("t{n}"),
                    seat: n
                }
            );
        }
        assert_eq!(r.join("K7QMZD", None, "t9".into(), end), Reserved::Full);
        assert_eq!(
            r.join("K7QMZD", Some(1), "x".into(), end),
            Reserved::Taken,
            "the host's seat"
        );
        assert_eq!(
            r.join("K7QMZE", None, "x".into(), end),
            Reserved::Closed,
            "no such code"
        );
        assert!(r.has_id("r1") && !r.has_id("r2"));
        assert_eq!(r.count_by("10.0.0.1"), 1);
        assert_eq!(r.count_by("10.0.0.2"), 0);
    }

    /// Each end learns the other's id from the room: the guest the host's, the host the guests' that asked.
    #[test]
    fn the_room_swaps_tunnel_ids() {
        let (mut r, now, host) = room(false);
        let guest = key(2);
        r.join("K7QMZD", None, "tg".into(), now + Duration::from_secs(60));
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
        r.join("K7QMZD", None, "tg".into(), now + Duration::from_secs(10));
        assert!(r.heartbeat("tg", now + Duration::from_secs(120)));
        assert!(r.expire(now + Duration::from_secs(30)).is_empty());
        assert_eq!(r.rooms[0].seats.len(), 2, "the guest's lease was extended");
        assert_eq!(
            r.expire(now + Duration::from_secs(61)),
            vec!["K7QMZD".to_string()],
            "the host's lease ended"
        );
        assert!(r.is_empty());

        let (mut r, now, _) = room(false);
        r.join("K7QMZD", None, "tg".into(), now + Duration::from_secs(60));
        assert_eq!(r.leave(None, "tg"), Some(None), "a guest leaves its seat");
        assert_eq!(
            r.leave(Some("other"), "th"),
            None,
            "CLOSE needs the room's id"
        );
        assert_eq!(
            r.leave(Some("r1"), "th"),
            Some(Some("K7QMZD".into())),
            "the host closes the room"
        );
        assert_eq!(r.leave(None, "th"), None);
    }

    /// Private rooms are not listed; a public one shows x/8 and an id that joins nothing, never the code, a tunnel id
    /// or a ticket.
    #[test]
    fn lobby_rows_let_nobody_join() {
        let (r, _, _) = room(false);
        assert!(r.listed().is_empty(), "private by default");
        let (mut r, now, host) = room(true);
        r.join("K7QMZD", None, "tg".into(), now + Duration::from_secs(60));
        r.peers("tg", Some(&key(2)));
        let json = serde_json::to_string(&r.listed()).unwrap();
        for secret in [
            host.as_str(),
            key(2).as_str(),
            "\"th\"",
            "\"tg\"",
            "K7QMZD",
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
