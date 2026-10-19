//! Pause P1 (`docs/research/PAUSE.md` §5): the in-game panel holds the game's frames for a short, capped pause in a
//! local match. These are its rules, kept apart from the DLL so the workspace's tests run them: where a pause is
//! allowed, the cap and the gap between pauses, and the "give back only what we took" rule for the control's Stop
//! checkbox and the game's mute.

use std::time::{Duration, Instant};

/// The longest pause: auto-resume after it. Below every wall-clock limit that keeps counting while the seat's frames
/// are held: the client's error 2001 comes 30 s after the last `CONTROL_STATE`, which may be up to 3 s old when the pause
/// starts (27 s worst case, both editions), and the 2010-11 control drops a seat after ~33-36 s of missed answers
/// (2017-18 ~63-66 s).
pub const CAP: Duration = Duration::from_secs(20);
/// The least play between two pauses: at least three `CONTROL_STATE` / `KEEP_ALIVE` rounds (one every 2-3 s), so the
/// control's missed-answer counter is back at zero before the next pause starts counting it again.
pub const GAP: Duration = Duration::from_secs(10);
/// The shop state GAME (the match itself): the only state with a pause, and the only one the Stop checkbox may be set
/// in (in ENTRY it changes seat admission, in 0 it would hold start-up).
pub const GAME: u32 = 5;

/// What the panel knows when the pause key or button is pressed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The seat's copy of the shop state; `None`: not readable (not the game's program).
    pub shop_state: Option<u32>,
    /// A run on a server (the match relay is set, or the link to the control is not to this PC).
    pub online: bool,
    /// Seats linked to this PC's control (the projector counts); `None`: no control on this PC.
    pub seats: Option<usize>,
    /// The 2010-11 projector's frames arrive.
    pub projector: bool,
    /// The club card or the game server's store was written this long ago; `None`: never.
    pub card_age: Option<Duration>,
    /// The panel draws on its own swap chain (the hold draws the PAUSED page there; a full-screen game has none).
    pub own_picture: bool,
    /// The control's Stop checkbox as read; `None`: not found (the 20 s cap keeps the pause safe without it).
    pub stop: Option<bool>,
}

/// Why a pause was refused; each has its notice in the panel's texts (`pause.msg_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    NotInMatch,
    Online,
    Seats,
    Projector,
    Card,
    NoPicture,
    /// Stop was already checked by someone else (the original's cup mode, a start-up setting): left alone.
    StopSet,
    /// The gap since the last pause is not over: this many seconds are left (rounded up).
    TooSoon(u64),
}

impl Refusal {
    /// The notice's text key.
    pub fn key(self) -> &'static str {
        match self {
            Refusal::NotInMatch => "pause.msg_not_match",
            Refusal::Online => "pause.msg_online",
            Refusal::Seats => "pause.msg_seats",
            Refusal::Projector => "pause.msg_projector",
            Refusal::Card => "pause.msg_card",
            Refusal::NoPicture => "pause.msg_no_picture",
            Refusal::StopSet => "pause.msg_stop_set",
            Refusal::TooSoon(_) => "pause.msg_too_soon",
        }
    }
}

/// No card traffic for this long before a pause (the card file's or the game server's store's last write).
pub const CARD_QUIET: Duration = Duration::from_secs(2);

/// Where a pause is allowed (PAUSE.md §5): the match itself (shop state 5) of an offline run whose control is on this
/// PC with one seat and no projector, no card written in the last [`CARD_QUIET`], the panel on its own swap chain, and
/// Stop not already set by someone else.
pub fn allowed(f: &Facts) -> Result<(), Refusal> {
    if f.shop_state != Some(GAME) {
        return Err(Refusal::NotInMatch);
    }
    if f.online || f.seats.is_none() {
        return Err(Refusal::Online);
    }
    if f.projector {
        return Err(Refusal::Projector);
    }
    if f.seats != Some(1) {
        return Err(Refusal::Seats);
    }
    if f.card_age.is_some_and(|a| a < CARD_QUIET) {
        return Err(Refusal::Card);
    }
    if !f.own_picture {
        return Err(Refusal::NoPicture);
    }
    if f.stop == Some(true) {
        return Err(Refusal::StopSet);
    }
    Ok(())
}

/// The cap and the gap: when the pause began, when the last one ended.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timer {
    since: Option<Instant>,
    ended: Option<Instant>,
}

impl Timer {
    /// A pause starts now, unless the gap since the last one is not over (`Err(seconds left)`).
    pub fn start(&mut self, now: Instant) -> Result<(), u64> {
        if let Some(e) = self.ended {
            let run = now.saturating_duration_since(e);
            if run < GAP {
                return Err((GAP - run).as_millis().div_ceil(1000) as u64);
            }
        }
        self.since = Some(now);
        Ok(())
    }

    /// The pause's time left, `None` when not paused; zero once the cap is reached.
    pub fn left(&self, now: Instant) -> Option<Duration> {
        self.since
            .map(|s| CAP.saturating_sub(now.saturating_duration_since(s)))
    }

    /// The pause ends now; how long it held.
    pub fn end(&mut self, now: Instant) -> Duration {
        self.ended = Some(now);
        self.since
            .take()
            .map_or(Duration::ZERO, |s| now.saturating_duration_since(s))
    }
}

/// A switch the pause borrows (the control's Stop checkbox, the game's mute): set only when it was found clear, and
/// on resume cleared only if the pause set it, so a Stop or a mute someone else set is never undone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Borrowed(bool);

impl Borrowed {
    /// The switch as found (`None`: none found): whether to set it now. The pause owns it from here.
    pub fn take(&mut self, found: Option<bool>) -> bool {
        self.0 = found == Some(false);
        self.0
    }

    /// Whether to clear it now (only if [`take`](Self::take) set it); after this the pause owns nothing.
    pub fn give_back(&mut self) -> bool {
        std::mem::take(&mut self.0)
    }
}

/// Stop is set only in the match: the shop state read just before, and the checkbox as found.
pub fn take_stop(stop: &mut Borrowed, shop_state: Option<u32>, found: Option<bool>) -> bool {
    stop.take(found.filter(|_| shop_state == Some(GAME)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Facts {
        Facts {
            shop_state: Some(GAME),
            seats: Some(1),
            own_picture: true,
            stop: Some(false),
            ..Facts::default()
        }
    }

    #[test]
    fn allowed_only_in_a_local_match() {
        assert_eq!(allowed(&ok()), Ok(()));
        // Stop not found, a card written long ago: still allowed
        let f = Facts {
            stop: None,
            card_age: Some(Duration::from_secs(60)),
            ..ok()
        };
        assert_eq!(allowed(&f), Ok(()));
        for (state, want) in [
            (None, false),
            (Some(0), false),
            (Some(1), false),
            (Some(4), false),
            (Some(6), false),
        ] {
            let f = Facts {
                shop_state: state,
                ..ok()
            };
            assert_eq!(allowed(&f).is_ok(), want, "state {state:?}");
        }
        let refused = |f: Facts| allowed(&f).unwrap_err();
        assert_eq!(
            refused(Facts {
                online: true,
                ..ok()
            }),
            Refusal::Online
        );
        assert_eq!(
            refused(Facts {
                seats: None,
                ..ok()
            }),
            Refusal::Online
        );
        assert_eq!(
            refused(Facts {
                seats: Some(2),
                ..ok()
            }),
            Refusal::Seats
        );
        assert_eq!(
            refused(Facts {
                seats: Some(0),
                ..ok()
            }),
            Refusal::Seats
        );
        assert_eq!(
            refused(Facts {
                projector: true,
                ..ok()
            }),
            Refusal::Projector
        );
        let card = Some(Duration::from_millis(500));
        assert_eq!(
            refused(Facts {
                card_age: card,
                ..ok()
            }),
            Refusal::Card
        );
        assert_eq!(
            refused(Facts {
                own_picture: false,
                ..ok()
            }),
            Refusal::NoPicture
        );
        assert_eq!(
            refused(Facts {
                stop: Some(true),
                ..ok()
            }),
            Refusal::StopSet
        );
        // not in the match wins over everything else
        let f = Facts {
            shop_state: Some(1),
            online: true,
            ..ok()
        };
        assert_eq!(refused(f), Refusal::NotInMatch);
    }

    #[test]
    fn cap_and_gap() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut t = Timer::default();
        assert_eq!(t.left(t0), None);
        assert_eq!(t.start(t0), Ok(()));
        assert_eq!(t.left(t0 + s(5)), Some(s(15)));
        assert_eq!(t.left(t0 + s(25)), Some(Duration::ZERO));
        assert_eq!(t.end(t0 + s(20)), s(20));
        assert_eq!(t.left(t0 + s(21)), None);
        // the next pause waits for the gap: 10 s after the end
        assert_eq!(t.start(t0 + s(21)), Err(9));
        assert_eq!(t.start(t0 + s(29) + Duration::from_millis(1)), Err(1));
        assert_eq!(t.start(t0 + s(30)), Ok(()));
        assert_eq!(t.end(t0 + s(31)), s(1));
        assert!(
            CAP < s(27),
            "the cap must stay under the client's worst-case 2001"
        );
    }

    #[test]
    fn stop_cleared_only_if_we_set_it() {
        let mut b = Borrowed::default();
        // found clear in the match: set, then cleared on resume, once
        assert!(take_stop(&mut b, Some(GAME), Some(false)));
        assert!(b.give_back());
        assert!(!b.give_back());
        // already set by someone else: left alone both ways
        assert!(!take_stop(&mut b, Some(GAME), Some(true)));
        assert!(!b.give_back());
        // not found: nothing to do
        assert!(!take_stop(&mut b, Some(GAME), None));
        assert!(!b.give_back());
        // never outside state 5
        for state in [None, Some(0), Some(1), Some(4), Some(6), Some(7)] {
            assert!(!take_stop(&mut b, state, Some(false)), "state {state:?}");
            assert!(!b.give_back());
        }
        // the mute: the player's own mute kept
        assert!(!b.take(Some(true)));
        assert!(!b.give_back());
        assert!(b.take(Some(false)));
        assert!(b.give_back());
    }
}
