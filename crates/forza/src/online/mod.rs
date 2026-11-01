//! The client side of online play (step 10, piece I1; ports `online/seat.py`) and the wire protocol the server's seat
//! desk (piece I2) shares with it; [`save_logs`] packs this PC's logs into a ZIP the player sends themselves. No Windows-only code: the Linux server
//! builds this module too. [`gamesvr`] and [`vs`]: the 2017-18 game server and its VS matcher and reflect service,
//! run by the Launcher's local stand-in and by `forza-server gmsvr`.
//!
//! Seat desk (TCP [`SEAT_PORT`]), ASCII lines:
//! `WANT` or `WANT SEAT n` -> `SEAT n PROJECTOR p`, `TAKEN n`, `FULL` or `ERR ...`; then `PING` -> `PONG`.
//! A 2017-18 arcade guest's club goes to the host's desk and back with `CLUB PUT`, `CLUB GET` and `CLUB DONE` ([`club`]).

pub mod club;
pub mod gamesvr;
pub mod lan;
pub mod lobby;
pub mod save_logs;
pub mod seat;
pub mod vs;

/// The seat desk's port on the server (the Python's `WCCF_BROKER_PORT` default).
pub const SEAT_PORT: u16 = 20030;
/// Seats a server gives out: 1 to 8 (seat 0 is the projector, never handed to a PC).
pub const SEATS: std::ops::RangeInclusive<u8> = 1..=8;

/// What a PC asks the seat desk for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// `WANT`: the lowest free seat.
    Any,
    /// `WANT SEAT n`.
    Seat(u8),
}

impl Want {
    pub fn line(self) -> String {
        match self {
            Want::Any => "WANT\n".into(),
            Want::Seat(n) => format!("WANT SEAT {n}\n"),
        }
    }

    /// Reads a line without its end the way the desk does: `WANT`, or `WANT SEAT n` with n in 1..=8 (the desk
    /// answers `ERR` to everything else, a seat number out of range included). A trailing `TOKEN t` (kit 7's account
    /// sign-in) is accepted for compatibility and not checked yet.
    pub fn parse(line: &str) -> Option<Want> {
        let mut w: Vec<&str> = line.split_whitespace().collect();
        if let [.., "TOKEN", _] = w[..] {
            w.truncate(w.len() - 2);
        }
        match w[..] {
            ["WANT"] => Some(Want::Any),
            ["WANT", "SEAT", n] if n.bytes().all(|b| b.is_ascii_digit()) => {
                n.parse().ok().filter(|n| SEATS.contains(n)).map(Want::Seat)
            }
            _ => None,
        }
    }
}

/// The desk's answer to a [`Want`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Seat {
        seat: u8,
        projector: bool,
    },
    Full,
    Taken(u8),
    /// `ERR text` (the text after `ERR `), or any other line the client could not place.
    Err(String),
}

impl Reply {
    pub fn line(&self) -> String {
        match self {
            Reply::Seat { seat, projector } => {
                format!("SEAT {seat} PROJECTOR {}\n", u8::from(*projector))
            }
            Reply::Full => "FULL\n".into(),
            Reply::Taken(n) => format!("TAKEN {n}\n"),
            Reply::Err(why) => format!("ERR {why}\n"),
        }
    }

    /// The client's reading of a reply line: `SEAT n PROJECTOR p` (four words, as `seat.py` checks) is a seat; the
    /// rest keep their first word (`FULL`, `TAKEN n`) or become [`Reply::Err`] holding the whole line.
    pub fn parse(line: &str) -> Reply {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w[..] {
            ["SEAT", n, "PROJECTOR", p] => match (n.parse(), p.parse::<u32>()) {
                (Ok(seat), Ok(p)) => Reply::Seat {
                    seat,
                    projector: p != 0,
                },
                _ => Reply::Err(w.join(" ")),
            },
            ["FULL", ..] => Reply::Full,
            ["TAKEN", ..] => Reply::Taken(w.get(1).and_then(|n| n.parse().ok()).unwrap_or(0)),
            ["ERR", ..] => Reply::Err(
                line.trim()
                    .strip_prefix("ERR")
                    .unwrap_or("")
                    .trim_start()
                    .into(),
            ),
            _ => Reply::Err(w.join(" ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn want_and_reply_round_trip() {
        for w in [Want::Any, Want::Seat(1), Want::Seat(8)] {
            assert_eq!(Want::parse(w.line().trim_end()), Some(w));
        }
        assert_eq!(Want::parse("WANT TOKEN abc"), Some(Want::Any));
        assert_eq!(Want::parse("WANT SEAT 2 TOKEN abc"), Some(Want::Seat(2)));
        for bad in [
            "",
            "WANT SEAT",
            "WANT SEAT 0",
            "WANT SEAT 9",
            "WANT SEAT x",
            "WANT SEAT -1",
            "WANT 3",
            "HELLO",
            "WANT TOKEN",
            "TOKEN abc",
        ] {
            assert_eq!(Want::parse(bad), None, "{bad}");
        }
        for r in [
            Reply::Seat {
                seat: 3,
                projector: false,
            },
            Reply::Seat {
                seat: 1,
                projector: true,
            },
            Reply::Full,
            Reply::Taken(2),
            Reply::Err("say WANT or WANT SEAT n".into()),
        ] {
            assert_eq!(Reply::parse(r.line().trim_end()), r);
        }
        assert_eq!(
            Reply::parse("SEAT 3 PROJECTOR"),
            Reply::Err("SEAT 3 PROJECTOR".into())
        );
        assert_eq!(
            Reply::parse("SEAT x PROJECTOR 0"),
            Reply::Err("SEAT x PROJECTOR 0".into())
        );
    }
}
