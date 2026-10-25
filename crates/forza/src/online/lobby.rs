//! The desk's `LOBBY edition` answer: one JSON line listing the shops and VS rooms the Launcher's lobby view shows
//! (ROADMAP, "Online lobby view"), and the client calls of the other words. Ids are random per server run; no answer
//! holds addresses, card numbers or names of people.
//!
//! The words, one line each way, the desk hangs up after its answer:
//! - `LOBBY edition` -> one [`Lobby`] JSON line, or `ERR slow`.
//! - `RESERVE id [n]` -> [`Reserved`]: a shop seat or the guest seat of a VS room.
//! - `HEARTBEAT t`, `RELEASE t` -> `OK` | `GONE`.
//! - `CREATE vs` -> [`Created`]: a VS room with this PC on seat 1; `CREATE vs open` puts it in the open pool instead
//!   (the desk seats it on seat 2 of the oldest room waiting there, else makes one; QUICK MATCH on VS ROOMS).
//! - `READY id t` -> `OK` | `GONE`: the ticket's seat in room `id` is ready (READY).
//! - `CLOSE id t` -> `OK` | `GONE`: the host's ticket closes room `id` (LEAVE by the host).
//! - `CREATE shop ID [public]` (2017-18) -> [`Created`]: a shop room hosted by the PC whose shop tunnel is ID (an
//!   iroh EndpointId, 64 hex digits), the host on seat 1. Private unless `public` (then listed, without its code).
//! - `JOIN CODE [n]` -> [`Reserved`]: a guest seat (2-8) in the shop room with that code; the code is the only way in.
//! - `SHOP t [ID]` -> [`Peers`] | `GONE`: a guest's ticket (telling its own tunnel id) gets the host's id, the host's
//!   ticket the ids of the guests that asked. Ids go only to tickets of the same room, never into `LOBBY`.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// The wire version this build reads and writes.
pub const VERSION: u32 = 1;
/// The desk never answers `LOBBY` with more than this many bytes; the client reads no more.
pub const MAX_ANSWER: usize = 64 * 1024;
/// A reserved seat is held this long without a `HEARTBEAT`; clients send one every [`HEARTBEAT_EVERY`] seconds.
pub const LEASE_SECS: u64 = 60;
pub const HEARTBEAT_EVERY: u64 = 20;
/// A shop room's code: this many letters and digits (the desk makes them, the Launcher's box takes them).
pub const SHOP_CODE_LEN: usize = 8;

/// The desk's answer to `RESERVE id` (or `RESERVE id n`): `TICKET t n`, `FULL`, `TAKEN` (the seat asked for is
/// busy) or `CLOSED` (not this desk's shop).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reserved {
    Ticket { ticket: String, seat: u8 },
    Full,
    Taken,
    Closed,
}

impl Reserved {
    pub fn line(&self) -> String {
        match self {
            Reserved::Ticket { ticket, seat } => format!("TICKET {ticket} {seat}\n"),
            Reserved::Full => "FULL\n".into(),
            Reserved::Taken => "TAKEN\n".into(),
            Reserved::Closed => "CLOSED\n".into(),
        }
    }

    pub fn parse(line: &str) -> Option<Reserved> {
        match line.split_whitespace().collect::<Vec<_>>()[..] {
            ["TICKET", t, n] => Some(Reserved::Ticket {
                ticket: t.into(),
                seat: n.parse().ok()?,
            }),
            ["FULL"] => Some(Reserved::Full),
            ["TAKEN"] => Some(Reserved::Taken),
            ["CLOSED"] => Some(Reserved::Closed),
            _ => None,
        }
    }
}

/// A shop's phase, as the desk reads it from the control's log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    InMatch,
    Full,
    Closed,
}

/// One shop: the 8 seats of one control.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shop {
    pub id: String,
    /// The operator's setting, up to 24 characters.
    pub name: String,
    pub seats: u8,
    pub max: u8,
    pub state: State,
    /// Seconds to the next ENTRY, when the desk knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_entry: Option<u32>,
    /// `en`, `it` or `ja`, declared by the operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

/// An iroh EndpointId as written: 64 hex digits.
pub fn valid_endpoint_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The desk's answer to `SHOP t [ID]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Peers {
    /// To a guest: `HOST id`.
    Host(String),
    /// To the host: `GUESTS id id ...` (the guests that asked, in seat order; none yet: `GUESTS`).
    Guests(Vec<String>),
}

impl Peers {
    pub fn line(&self) -> String {
        match self {
            Peers::Host(id) => format!("HOST {id}\n"),
            Peers::Guests(ids) if ids.is_empty() => "GUESTS\n".into(),
            Peers::Guests(ids) => format!("GUESTS {}\n", ids.join(" ")),
        }
    }

    pub fn parse(line: &str) -> Option<Peers> {
        match line.split_whitespace().collect::<Vec<_>>()[..] {
            ["HOST", id] if valid_endpoint_id(id) => Some(Peers::Host(id.to_ascii_lowercase())),
            ["GUESTS", ref ids @ ..] if ids.iter().all(|i| valid_endpoint_id(i)) => Some(
                Peers::Guests(ids.iter().map(|i| i.to_ascii_lowercase()).collect()),
            ),
            _ => None,
        }
    }
}

/// A player in a VS room: the seat (1 = the host), a random club id, and whether READY was pressed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occupant {
    pub seat: u8,
    pub club: String,
    #[serde(default)]
    pub ready: bool,
}

/// One VS room: a one-PC shop waiting for, or holding, its opponent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VsRoom {
    pub id: String,
    /// The four-letter code players read out.
    pub code: String,
    pub players: u8,
    pub max: u8,
    /// The game's VS kind number (243 entries are kind 2).
    pub kind: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Who sits in the room, by random club id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub occupants: Vec<Occupant>,
}

/// The desk's answer to `CREATE vs`: `ROOM id code t seat` (`t` a ticket like `RESERVE`'s, `seat` 1 in a new room,
/// 2 when `open` joined a waiting one; a desk without the seat field means 1) or `FULL` (the desk holds as many rooms
/// as it takes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Created {
    Room {
        id: String,
        code: String,
        ticket: String,
        seat: u8,
    },
    Full,
}

impl Created {
    pub fn line(&self) -> String {
        match self {
            Created::Room {
                id,
                code,
                ticket,
                seat,
            } => format!("ROOM {id} {code} {ticket} {seat}\n"),
            Created::Full => "FULL\n".into(),
        }
    }

    pub fn parse(line: &str) -> Option<Created> {
        match line.split_whitespace().collect::<Vec<_>>()[..] {
            ["ROOM", id, code, t] => Some(Created::Room {
                id: id.into(),
                code: code.into(),
                ticket: t.into(),
                seat: 1,
            }),
            ["ROOM", id, code, t, seat] => Some(Created::Room {
                id: id.into(),
                code: code.into(),
                ticket: t.into(),
                seat: seat.parse().ok().filter(|s| (1..=2).contains(s))?,
            }),
            ["FULL"] => Some(Created::Full),
            _ => None,
        }
    }
}

/// A request line the Launcher sends (the server side parses these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    Lobby { edition: String },
    Reserve { id: String, seat: Option<u8> },
    Heartbeat { ticket: String },
    Release { ticket: String },
    Create { open: bool },
    CreateShop { host: String, public: bool },
    JoinShop { code: String, seat: Option<u8> },
    Shop { ticket: String, own: Option<String> },
    Ready { id: String, ticket: String },
    Close { id: String, ticket: String },
}

impl Ask {
    pub fn line(&self) -> String {
        match self {
            Ask::Lobby { edition } => format!("LOBBY {edition}\n"),
            Ask::Reserve { id, seat: None } => format!("RESERVE {id}\n"),
            Ask::Reserve { id, seat: Some(n) } => format!("RESERVE {id} {n}\n"),
            Ask::Heartbeat { ticket } => format!("HEARTBEAT {ticket}\n"),
            Ask::Release { ticket } => format!("RELEASE {ticket}\n"),
            Ask::Create { open: false } => "CREATE vs\n".into(),
            Ask::Create { open: true } => "CREATE vs open\n".into(),
            Ask::CreateShop {
                host,
                public: false,
            } => format!("CREATE shop {host}\n"),
            Ask::CreateShop { host, public: true } => format!("CREATE shop {host} public\n"),
            Ask::JoinShop { code, seat: None } => format!("JOIN {code}\n"),
            Ask::JoinShop {
                code,
                seat: Some(n),
            } => format!("JOIN {code} {n}\n"),
            Ask::Shop { ticket, own: None } => format!("SHOP {ticket}\n"),
            Ask::Shop {
                ticket,
                own: Some(id),
            } => format!("SHOP {ticket} {id}\n"),
            Ask::Ready { id, ticket } => format!("READY {id} {ticket}\n"),
            Ask::Close { id, ticket } => format!("CLOSE {id} {ticket}\n"),
        }
    }

    pub fn parse(line: &str) -> Option<Ask> {
        let s = |x: &str| x.to_owned();
        Some(match line.split_whitespace().collect::<Vec<_>>()[..] {
            ["LOBBY", e] => Ask::Lobby { edition: s(e) },
            ["RESERVE", id] => Ask::Reserve {
                id: s(id),
                seat: None,
            },
            ["RESERVE", id, n] => Ask::Reserve {
                id: s(id),
                seat: Some(n.parse().ok()?),
            },
            ["HEARTBEAT", t] => Ask::Heartbeat { ticket: s(t) },
            ["RELEASE", t] => Ask::Release { ticket: s(t) },
            ["CREATE", "vs"] => Ask::Create { open: false },
            ["CREATE", "vs", "open"] => Ask::Create { open: true },
            ["CREATE", "shop", id] if valid_endpoint_id(id) => Ask::CreateShop {
                host: s(id),
                public: false,
            },
            ["CREATE", "shop", id, "public"] if valid_endpoint_id(id) => Ask::CreateShop {
                host: s(id),
                public: true,
            },
            ["JOIN", c] => Ask::JoinShop {
                code: s(c),
                seat: None,
            },
            ["JOIN", c, n] => Ask::JoinShop {
                code: s(c),
                seat: Some(n.parse().ok()?),
            },
            ["SHOP", t] => Ask::Shop {
                ticket: s(t),
                own: None,
            },
            ["SHOP", t, id] if valid_endpoint_id(id) => Ask::Shop {
                ticket: s(t),
                own: Some(s(id)),
            },
            ["READY", id, t] => Ask::Ready {
                id: s(id),
                ticket: s(t),
            },
            ["CLOSE", id, t] => Ask::Close {
                id: s(id),
                ticket: s(t),
            },
            _ => return None,
        })
    }
}

/// The whole answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lobby {
    pub v: u32,
    pub edition: String,
    /// Seconds since the Unix epoch, when the desk built the answer.
    pub at: u64,
    #[serde(default)]
    pub shops: Vec<Shop>,
    #[serde(default)]
    pub vs: Vec<VsRoom>,
}

impl Lobby {
    /// Reads one answer line. A version this build does not know is an error, not a guess.
    pub fn parse(line: &str) -> Result<Lobby, String> {
        let l: Lobby = serde_json::from_str(line.trim()).map_err(|e| e.to_string())?;
        if l.v != VERSION {
            return Err(format!(
                "lobby version {} (this build reads {VERSION})",
                l.v
            ));
        }
        Ok(l)
    }

    /// The answer as one line, newline included.
    pub fn line(&self) -> String {
        let mut s = serde_json::to_string(self).expect("plain data serializes");
        s.push('\n');
        s
    }
}

/// Connect, send, read time-outs: the Launcher's polling thread must never hang on a dead desk.
const TIMEOUT: Duration = Duration::from_secs(3);

/// One request line, one answer line (at most [`MAX_ANSWER`] bytes), the desk hangs up.
fn ask(addr: SocketAddr, line: &str) -> Result<String, String> {
    let io = |e: std::io::Error| e.to_string();
    let mut c = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(io)?;
    c.set_read_timeout(Some(TIMEOUT)).map_err(io)?;
    c.set_write_timeout(Some(TIMEOUT)).map_err(io)?;
    c.write_all(line.as_bytes()).map_err(io)?;
    let mut buf = Vec::new();
    c.take(MAX_ANSWER as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(io)?;
    if buf.len() > MAX_ANSWER {
        return Err("answer too long".into());
    }
    Ok(String::from_utf8_lossy(&buf).trim().to_string())
}

/// `LOBBY edition`: the desk's answer, or why there is none (`ERR slow` included).
pub fn fetch(addr: SocketAddr, edition: &str) -> Result<Lobby, String> {
    Lobby::parse(&ask(
        addr,
        &Ask::Lobby {
            edition: edition.into(),
        }
        .line(),
    )?)
}

/// `OK` true, `GONE` false, anything else the error.
fn ok_gone(a: String) -> Result<bool, String> {
    match a.as_str() {
        "OK" => Ok(true),
        "GONE" => Ok(false),
        _ => Err(a),
    }
}

/// `CREATE vs` (or `CREATE vs open`, the open pool): a room with this PC on seat 1.
pub fn create(addr: SocketAddr, open: bool) -> Result<Created, String> {
    let a = ask(addr, &Ask::Create { open }.line())?;
    Created::parse(&a).ok_or(a)
}

/// `CREATE shop ID [public]` (2017-18): a shop room for the PC whose shop tunnel is `host`, this PC on seat 1;
/// `public` lists it in `LOBBY` (never its code).
pub fn create_shop(addr: SocketAddr, host: &str, public: bool) -> Result<Created, String> {
    let a = ask(
        addr,
        &Ask::CreateShop {
            host: host.into(),
            public,
        }
        .line(),
    )?;
    Created::parse(&a).ok_or(a)
}

/// `JOIN CODE [n]`: a guest seat in the shop room with that code.
pub fn join_shop(addr: SocketAddr, code: &str, seat: Option<u8>) -> Result<Reserved, String> {
    let a = ask(
        addr,
        &Ask::JoinShop {
            code: code.into(),
            seat,
        }
        .line(),
    )?;
    Reserved::parse(&a).ok_or(a)
}

/// `SHOP t [ID]`: the room's other tunnel ids for this ticket; `Ok(None)` when the desk says `GONE`.
pub fn shop_peers(
    addr: SocketAddr,
    ticket: &str,
    own: Option<&str>,
) -> Result<Option<Peers>, String> {
    let a = ask(
        addr,
        &Ask::Shop {
            ticket: ticket.into(),
            own: own.map(str::to_owned),
        }
        .line(),
    )?;
    if a == "GONE" {
        return Ok(None);
    }
    Peers::parse(&a).map(Some).ok_or(a)
}

/// `READY id t`: true while the room and the ticket's seat still exist.
pub fn ready(addr: SocketAddr, id: &str, ticket: &str) -> Result<bool, String> {
    ok_gone(ask(
        addr,
        &Ask::Ready {
            id: id.into(),
            ticket: ticket.into(),
        }
        .line(),
    )?)
}

/// `CLOSE id t`: the host closes the room (the guest's ticket is let go by the desk).
pub fn close(addr: SocketAddr, id: &str, ticket: &str) -> Result<bool, String> {
    ok_gone(ask(
        addr,
        &Ask::Close {
            id: id.into(),
            ticket: ticket.into(),
        }
        .line(),
    )?)
}

/// `RESERVE id`: a seat held for [`LEASE_SECS`]. Claim it with `WANT ... TICKET t`.
pub fn reserve(addr: SocketAddr, id: &str) -> Result<Reserved, String> {
    reserve_seat(addr, id, None)
}

/// `RESERVE id n`: that seat (a rejoin), or the lowest free one when `seat` is `None`.
pub fn reserve_seat(addr: SocketAddr, id: &str, seat: Option<u8>) -> Result<Reserved, String> {
    let a = ask(
        addr,
        &Ask::Reserve {
            id: id.into(),
            seat,
        }
        .line(),
    )?;
    Reserved::parse(&a).ok_or(a)
}

/// `HEARTBEAT t`: true while the desk still holds the seat (send every [`HEARTBEAT_EVERY`] seconds).
pub fn heartbeat(addr: SocketAddr, ticket: &str) -> Result<bool, String> {
    ok_gone(ask(
        addr,
        &Ask::Heartbeat {
            ticket: ticket.into(),
        }
        .line(),
    )?)
}

/// `RELEASE t`: the seat is free at once; false when the ticket is spent, expired or unknown.
pub fn release(addr: SocketAddr, ticket: &str) -> Result<bool, String> {
    ok_gone(ask(
        addr,
        &Ask::Release {
            ticket: ticket.into(),
        }
        .line(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"v":1,"edition":"2017-18","at":1792400000,"shops":[{"id":"k3f9q2","name":"FORZA ROMA","seats":3,"max":8,"state":"open","next_entry":32,"lang":"it"},{"id":"m1","name":"MILANO CUP","seats":5,"max":8,"state":"in_match"}],"vs":[{"id":"r7x1","code":"K7QM","players":1,"max":2,"kind":2}]}"#;

    #[test]
    fn parses_and_round_trips() {
        let l = Lobby::parse(SAMPLE).unwrap();
        assert_eq!(l.shops.len(), 2);
        assert_eq!(l.shops[0].state, State::Open);
        assert_eq!(l.shops[1].state, State::InMatch);
        assert_eq!(l.shops[1].next_entry, None);
        assert_eq!(l.vs[0].code, "K7QM");
        assert_eq!(l.line(), format!("{SAMPLE}\n"));
        assert_eq!(Lobby::parse(&l.line()).unwrap(), l);
    }

    #[test]
    fn room_occupants_round_trip() {
        let j = r#"{"v":1,"edition":"2017-18","at":1,"vs":[{"id":"r7x1","code":"K7QM","players":2,"max":2,"kind":2,"occupants":[{"seat":1,"club":"A7K2","ready":true},{"seat":2,"club":"Q3ZD","ready":false}]}]}"#;
        let l = Lobby::parse(j).unwrap();
        assert_eq!(l.vs[0].occupants[1].club, "Q3ZD");
        assert!(l.vs[0].occupants[0].ready && !l.vs[0].occupants[1].ready);
        assert_eq!(Lobby::parse(&l.line()).unwrap(), l);
    }

    /// The shop-room words: only a well-formed tunnel id is a word.
    #[test]
    fn shop_room_words_round_trip() {
        let id = "ab".repeat(32);
        for ask in [
            Ask::CreateShop {
                host: id.clone(),
                public: false,
            },
            Ask::CreateShop {
                host: id.clone(),
                public: true,
            },
            Ask::JoinShop {
                code: "K7QMZD".into(),
                seat: None,
            },
            Ask::JoinShop {
                code: "K7QMZD".into(),
                seat: Some(3),
            },
            Ask::Shop {
                ticket: "0123456789abcdef".into(),
                own: None,
            },
            Ask::Shop {
                ticket: "0123456789abcdef".into(),
                own: Some(id.clone()),
            },
        ] {
            assert_eq!(Ask::parse(&ask.line()), Some(ask.clone()), "{ask:?}");
        }
        assert_eq!(Ask::parse("CREATE shop 1.2.3.4"), None);
        assert_eq!(Ask::parse(&format!("SHOP t {}", &id[..60])), None);
    }

    #[test]
    fn words_round_trip() {
        for a in [
            Ask::Lobby {
                edition: "2017-18".into(),
            },
            Ask::Reserve {
                id: "r7x1".into(),
                seat: None,
            },
            Ask::Reserve {
                id: "k3".into(),
                seat: Some(3),
            },
            Ask::Heartbeat {
                ticket: "t1".into(),
            },
            Ask::Release {
                ticket: "t1".into(),
            },
            Ask::Create { open: false },
            Ask::Create { open: true },
            Ask::Ready {
                id: "r7x1".into(),
                ticket: "t1".into(),
            },
            Ask::Close {
                id: "r7x1".into(),
                ticket: "t1".into(),
            },
        ] {
            assert_eq!(Ask::parse(&a.line()), Some(a.clone()), "{a:?}");
        }
        assert_eq!(Ask::Create { open: false }.line(), "CREATE vs\n");
        assert_eq!(Ask::parse("READY r7x1"), None); // no ticket: no seat to mark
        assert_eq!(Ask::parse("CREATE shop"), None);
        for c in [
            Created::Room {
                id: "r7x1".into(),
                code: "K7QM".into(),
                ticket: "t9".into(),
                seat: 2,
            },
            Created::Full,
        ] {
            assert_eq!(Created::parse(&c.line()), Some(c.clone()));
        }
        assert_eq!(Created::parse("ERR slow"), None);
        assert_eq!(Created::parse("ROOM r7x1 K7QM t9 3"), None);
    }

    /// The client calls against a stand-in desk that answers each word as B4's desk will.
    #[test]
    fn client_calls_send_the_words() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let desk = std::thread::spawn(move || {
            let mut heard = vec![];
            for _ in 0..4 {
                let (c, _) = l.accept().unwrap();
                let mut line = String::new();
                std::io::BufRead::read_line(&mut std::io::BufReader::new(&c), &mut line).unwrap();
                let answer = match Ask::parse(&line) {
                    // the old desk's 3-field form: seat 1
                    Some(Ask::Create { open: false }) => "ROOM r7x1 K7QM t9\n",
                    Some(Ask::Create { open: true }) => "FULL\n",
                    Some(Ask::Ready { .. }) => "OK\n",
                    Some(Ask::Close { .. }) => "GONE\n",
                    _ => "ERR\n",
                };
                (&c).write_all(answer.as_bytes()).unwrap();
                heard.push(line);
            }
            heard
        });
        let room = Created::Room {
            id: "r7x1".into(),
            code: "K7QM".into(),
            ticket: "t9".into(),
            seat: 1,
        };
        assert_eq!(create(addr, false), Ok(room));
        assert_eq!(create(addr, true), Ok(Created::Full));
        assert_eq!(ready(addr, "r7x1", "t9"), Ok(true));
        assert_eq!(close(addr, "r7x1", "t9"), Ok(false));
        let heard = desk.join().unwrap();
        assert_eq!(
            heard,
            [
                "CREATE vs\n",
                "CREATE vs open\n",
                "READY r7x1 t9\n",
                "CLOSE r7x1 t9\n"
            ]
        );
    }

    #[test]
    fn refuses_other_versions_and_junk() {
        assert!(Lobby::parse(&SAMPLE.replace("\"v\":1", "\"v\":2")).is_err());
        assert!(Lobby::parse("ERR slow").is_err());
    }
}
