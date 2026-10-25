//! 2017-18 shop rooms (M2 plan 5.3): a player's PC that hosts a shop puts it in the lobby with `CREATE shop ID`,
//! where ID is its shop tunnel's iroh EndpointId (64 hex digits). Guests `RESERVE` a seat (2-8) as for any shop and
//! then ask `SHOP t own-id`: a guest's ticket gets the host's id, the host's ticket gets the ids of the guests that
//! asked, so each tunnel end learns the other's id from the room alone. The lobby lists a room as a shop row with its
//! code and x/8; ids, tickets and addresses never leave through `LOBBY`. Sans-IO: the desk passes the clock, the random
//! ids and tickets in.

pub use forza::online::lobby::{Peers, valid_endpoint_id as valid_id};
use forza::online::lobby::{Reserved, Shop, State as ShopState};
use std::time::Instant;

/// Seats in a shop room: the host on 1, guests on 2-8.
const SEATS: std::ops::RangeInclusive<u8> = 1..=8;

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

    /// A new room with its host on seat 1 (`host_id` checked by the caller with [`valid_id`]).
    pub fn create(
        &mut self,
        id: String,
        code: String,
        host_id: &str,
        ticket: String,
        end: Instant,
    ) {
        self.rooms.push(Room {
            id,
            code,
            seats: vec![Sitter {
                seat: 1,
                ticket,
                end,
                id: Some(host_id.to_ascii_lowercase()),
            }],
        });
    }

    /// `RESERVE id [n]` for a shop room: the lowest free guest seat (or n), `FULL` or `TAKEN`; `None` when `id` is
    /// no shop room.
    pub fn reserve(
        &mut self,
        id: &str,
        seat: Option<u8>,
        ticket: String,
        end: Instant,
    ) -> Option<Reserved> {
        let r = self.rooms.iter_mut().find(|r| r.id == id)?;
        let busy = |n: u8| r.seats.iter().any(|s| s.seat == n);
        let n = match seat {
            Some(n) if n == 1 || !SEATS.contains(&n) || busy(n) => return Some(Reserved::Taken),
            Some(n) => n,
            None => match SEATS.skip(1).find(|n| !busy(*n)) {
                Some(n) => n,
                None => return Some(Reserved::Full),
            },
        };
        r.seats.push(Sitter {
            seat: n,
            ticket: ticket.clone(),
            end,
            id: None,
        });
        Some(Reserved::Ticket { ticket, seat: n })
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

    /// The rooms as lobby rows: id, code (also as the name), seats taken of 8. Nothing else.
    pub fn listed(&self) -> Vec<Shop> {
        self.rooms
            .iter()
            .map(|r| {
                let seats = r.seats.len() as u8;
                Shop {
                    id: r.id.clone(),
                    name: r.code.clone(),
                    seats,
                    max: 8,
                    state: if seats >= 8 {
                        ShopState::Full
                    } else {
                        ShopState::Open
                    },
                    next_entry: None,
                    lang: None,
                    code: Some(r.code.clone()),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const HOST: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
    const GUEST: &str = "bb00000000000000000000000000000000000000000000000000000000000002";

    fn room() -> (ShopRooms, Instant) {
        let now = Instant::now();
        let mut r = ShopRooms::default();
        r.create(
            "r1".into(),
            "K7QM".into(),
            HOST,
            "th".into(),
            now + Duration::from_secs(60),
        );
        (r, now)
    }

    #[test]
    fn ids_are_64_hex_digits() {
        assert!(valid_id(HOST) && valid_id(&HOST.to_uppercase()));
        assert!(
            !valid_id(&HOST[..63])
                && !valid_id(&format!("{HOST}0"))
                && !valid_id(&HOST.replace('a', "g"))
        );
    }

    #[test]
    fn guests_get_seats_2_to_8_then_full() {
        let (mut r, now) = room();
        let end = now + Duration::from_secs(60);
        for n in 2..=8u8 {
            assert_eq!(
                r.reserve("r1", None, format!("t{n}"), end),
                Some(Reserved::Ticket {
                    ticket: format!("t{n}"),
                    seat: n
                })
            );
        }
        assert_eq!(
            r.reserve("r1", None, "t9".into(), end),
            Some(Reserved::Full)
        );
        assert_eq!(
            r.reserve("r1", Some(1), "x".into(), end),
            Some(Reserved::Taken),
            "the host's seat"
        );
        assert_eq!(
            r.reserve("other", None, "x".into(), end),
            None,
            "not a shop room"
        );
        assert_eq!(r.listed()[0].seats, 8);
        assert_eq!(r.listed()[0].state, ShopState::Full);
    }

    /// Each end learns the other's id from the room: the guest the host's, the host the guests' that asked.
    #[test]
    fn the_room_swaps_tunnel_ids() {
        let (mut r, now) = room();
        let end = now + Duration::from_secs(60);
        r.reserve("r1", None, "tg".into(), end);
        assert_eq!(
            r.peers("th", None),
            Some(Peers::Guests(vec![])),
            "no guest asked yet"
        );
        assert_eq!(
            r.peers("tg", Some(&GUEST.to_uppercase())),
            Some(Peers::Host(HOST.into()))
        );
        assert_eq!(r.peers("th", None), Some(Peers::Guests(vec![GUEST.into()])));
        assert_eq!(r.peers("nope", None), None);
        let line = Peers::Guests(vec![GUEST.into()]).line();
        assert_eq!(Peers::parse(&line), Some(Peers::Guests(vec![GUEST.into()])));
        assert_eq!(Peers::parse("GUESTS\n"), Some(Peers::Guests(vec![])));
        assert_eq!(
            Peers::parse(&Peers::Host(HOST.into()).line()),
            Some(Peers::Host(HOST.into()))
        );
        assert_eq!(Peers::parse("HOST 1.2.3.4"), None);
    }

    #[test]
    fn leases_and_leaving() {
        let (mut r, now) = room();
        r.reserve("r1", None, "tg".into(), now + Duration::from_secs(10));
        assert!(r.heartbeat("tg", now + Duration::from_secs(120)));
        assert!(r.expire(now + Duration::from_secs(30)).is_empty());
        assert_eq!(r.listed()[0].seats, 2, "the guest's lease was extended");
        assert_eq!(
            r.expire(now + Duration::from_secs(61)),
            vec!["K7QM".to_string()],
            "the host's lease ended"
        );
        assert!(r.is_empty());

        let (mut r, now) = room();
        r.reserve("r1", None, "tg".into(), now + Duration::from_secs(60));
        assert_eq!(r.leave(None, "tg"), Some(None), "a guest leaves its seat");
        assert_eq!(
            r.leave(Some("other"), "th"),
            None,
            "CLOSE needs the room's id"
        );
        assert_eq!(
            r.leave(Some("r1"), "th"),
            Some(Some("K7QM".into())),
            "the host closes the room"
        );
        assert_eq!(r.leave(None, "th"), None);
    }

    /// The lobby row: id, code, x/8, and nothing a stranger could dial or claim with.
    #[test]
    fn lobby_rows_hold_no_ids_or_tickets() {
        let (mut r, now) = room();
        r.reserve("r1", None, "tg".into(), now + Duration::from_secs(60));
        r.peers("tg", Some(GUEST));
        let json = serde_json::to_string(&r.listed()).unwrap();
        for secret in [HOST, GUEST, "th", "tg"] {
            assert!(
                !json.contains(&format!("\"{secret}")) && !json.contains(secret),
                "{secret} in {json}"
            );
        }
        assert!(
            json.contains("\"code\":\"K7QM\"") && json.contains("\"max\":8"),
            "{json}"
        );
    }
}
