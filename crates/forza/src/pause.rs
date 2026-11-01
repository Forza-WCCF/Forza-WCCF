//! Pause P1 (`docs/research/PAUSE.md` §5): the in-game panel holds the game's frames for a short, capped pause in a
//! local match. These are its rules, kept apart from the DLL so the workspace's tests run them: where a pause is
//! allowed, the cap and the gap between pauses, and the "give back only what we took" rule for the control's Stop
//! checkbox and the game's mute, kept owed until each is confirmed given back.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// The longest pause: auto-resume after it. Below every wall-clock limit that keeps counting while the seat's frames
/// are held: the client's error 2001 comes 30 s after the last `CONTROL_STATE`, which may be up to 3 s old when the pause
/// starts (27 s worst case, both editions), and the 2010-11 control drops a seat after ~33-36 s of missed answers
/// (2017-18 ~63-66 s).
pub const CAP: Duration = Duration::from_secs(20);
/// The least play between two pauses: at least three `CONTROL_STATE` / `KEEP_ALIVE` rounds (one every 2-3 s), so the
/// control's missed-answer counter is back at zero before the next pause starts counting it again.
pub const GAP: Duration = Duration::from_secs(10);
/// The arcade state GAME (the match itself): the only state with a pause, and the only one the Stop checkbox may be set
/// in (in ENTRY it changes seat admission, in 0 it would hold start-up).
pub const GAME: u32 = 5;

/// What the panel knows when the pause key or button is pressed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The seat's copy of the arcade state; `None`: not readable (not the game's program).
    pub arcade_state: Option<u32>,
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
    /// An earlier pause's Stop is not confirmed cleared yet ([`Owed`]).
    pub stop_owed: bool,
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

/// Where a pause is allowed (PAUSE.md §5): the match itself (arcade state 5) of an offline run whose control is on this
/// PC with one seat and no projector, no card written in the last [`CARD_QUIET`], the panel on its own swap chain, and
/// Stop not already set by someone else.
pub fn allowed(f: &Facts) -> Result<(), Refusal> {
    if f.arcade_state != Some(GAME) {
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
    if f.stop == Some(true) || f.stop_owed {
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

/// The least time between two tries to give back what a pause owes.
pub const RETRY: Duration = Duration::from_secs(1);
/// Tries per pause before giving up (logged); an owed Stop still refuses every later pause.
pub const RETRIES: u32 = 60;

/// What the pause borrowed and still owes: the control's Stop checkbox and the game's muted sound sessions (by
/// session id). Set only when found clear, so a Stop or a mute someone else set is never undone; owed from the moment
/// a set may have happened (a timed-out answer may still have set it) until the clear is confirmed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Owed {
    stop: bool,
    muted: BTreeSet<String>,
    tries: u32,
    last: Option<Instant>,
}

impl Owed {
    /// Stop as found, with the arcade state read just before: whether to set it now (only clear, only in the match).
    pub fn take_stop(&mut self, arcade_state: Option<u32>, found: Option<bool>) -> bool {
        let take = arcade_state == Some(GAME) && found == Some(false);
        if take {
            self.stop = true;
            self.fresh();
        }
        take
    }

    /// The game's sessions as read (id, muted): the clear ones, to mute now; each owed from here.
    pub fn take_mute(&mut self, sessions: &[(String, bool)]) -> Vec<String> {
        let clear: Vec<String> = sessions
            .iter()
            .filter(|s| !s.1)
            .map(|s| s.0.clone())
            .collect();
        if !clear.is_empty() {
            self.muted.extend(clear.iter().cloned());
            self.fresh();
        }
        clear
    }

    fn fresh(&mut self) {
        self.tries = 0;
        self.last = None;
    }

    /// Stop may still be checked by an earlier pause: no new pause.
    pub fn stop_owed(&self) -> bool {
        self.stop
    }

    /// The sessions still to unmute.
    pub fn muted(&self) -> Vec<String> {
        self.muted.iter().cloned().collect()
    }

    /// Something is owed and a try is due now: its number (1 = the first, at resume). At most one per [`RETRY`],
    /// [`RETRIES`] in all.
    pub fn due(&mut self, now: Instant) -> Option<u32> {
        let wait = self
            .last
            .is_some_and(|l| now.saturating_duration_since(l) < RETRY);
        if self.settled() || self.tries >= RETRIES || wait {
            return None;
        }
        self.tries += 1;
        self.last = Some(now);
        Some(self.tries)
    }

    /// Stop confirmed clear.
    pub fn stop_cleared(&mut self) {
        self.stop = false;
    }

    /// These sessions were unmuted.
    pub fn unmuted(&mut self, ids: &[String]) {
        for id in ids {
            self.muted.remove(id);
        }
    }

    /// Nothing owed.
    pub fn settled(&self) -> bool {
        !self.stop && self.muted.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Facts {
        Facts {
            arcade_state: Some(GAME),
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
                arcade_state: state,
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
            arcade_state: Some(1),
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
        let mut o = Owed::default();
        // already set by someone else, not found, or outside state 5: never taken
        assert!(!o.take_stop(Some(GAME), Some(true)));
        assert!(!o.take_stop(Some(GAME), None));
        for state in [None, Some(0), Some(1), Some(4), Some(6), Some(7)] {
            assert!(!o.take_stop(state, Some(false)), "state {state:?}");
        }
        assert!(o.settled());
        assert_eq!(o.due(Instant::now()), None);
        // found clear in the match: set, owed until confirmed
        assert!(o.take_stop(Some(GAME), Some(false)));
        assert!(o.stop_owed() && !o.settled());
        o.stop_cleared();
        assert!(o.settled());
    }

    #[test]
    fn a_clear_is_retried_until_confirmed() {
        let t0 = Instant::now();
        let mut o = Owed::default();
        assert!(o.take_stop(Some(GAME), Some(false)));
        // the first try at once (resume), then at most one per RETRY
        assert_eq!(o.due(t0), Some(1));
        assert_eq!(o.due(t0 + Duration::from_millis(500)), None);
        assert_eq!(o.due(t0 + RETRY), Some(2));
        // no new pause while Stop is owed
        let f = Facts {
            arcade_state: Some(GAME),
            seats: Some(1),
            own_picture: true,
            stop: Some(false),
            stop_owed: o.stop_owed(),
            ..Facts::default()
        };
        assert_eq!(allowed(&f), Err(Refusal::StopSet));
        // bounded: RETRIES tries in all, then given up (the pause still refused)
        let mut n = 2;
        while o.due(t0 + RETRY * (n + 1)).is_some() {
            n += 1;
        }
        assert_eq!(n, RETRIES);
        assert!(o.stop_owed());
        // confirmed: nothing owed, pauses allowed again
        o.stop_cleared();
        assert!(o.settled());
        assert_eq!(
            allowed(&Facts {
                stop_owed: false,
                ..f
            }),
            Ok(())
        );
    }

    #[test]
    fn only_the_sessions_we_muted_are_unmuted() {
        let t0 = Instant::now();
        let mut o = Owed::default();
        let s = |id: &str, muted| (id.to_owned(), muted);
        // the player muted "a": left alone; "b" and "c" muted by the pause
        let take = o.take_mute(&[s("a", true), s("b", false), s("c", false)]);
        assert_eq!(take, ["b", "c"]);
        assert_eq!(o.muted(), ["b", "c"]);
        assert_eq!(o.due(t0), Some(1));
        // "c" could not be unmuted (its device gone): still owed, tried again later
        o.unmuted(&["b".to_owned()]);
        assert_eq!(o.muted(), ["c"]);
        assert!(!o.settled() && !o.stop_owed());
        // a new pause meanwhile: "c" (still muted by us) stays owed, "a" still the player's
        assert_eq!(
            o.take_mute(&[s("a", true), s("c", true)]),
            Vec::<String>::new()
        );
        assert_eq!(o.due(t0 + RETRY), Some(2));
        o.unmuted(&["c".to_owned()]);
        assert!(o.settled());
        // all already muted: nothing taken, nothing owed
        assert!(o.take_mute(&[s("a", true)]).is_empty());
        assert!(o.settled());
    }
}
