//! The run's decisions, without Windows (port of play.py's pure parts): what the command line asks, the card rule,
//! what a game window's end means, whether a run went wrong, which seat an online PC takes and when RESTART NOW needs
//! only the cabinet.  Their cases are ported from test_ended.py, test_take_seat.py and play.py's own rules.

use forza::online::seat::{Fault, SeatError};

use crate::card::Session;

/// debug::run's line when it ends a game because its window was closed.
pub const WINDOW_CLOSED: &str = "its window was closed";
/// debug::run's words at a crash.
pub const CRASH_SIGNS: [&str; 2] = ["SECOND-chance / FATAL", "STOP: invalid parameter"];

/// What a run is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Everything on this PC.
    Local,
    /// The server side only, for other players' PCs.
    Server,
    /// The cabinet only, on the server at this address; a seat asked for (1-8), or the next free one.
    Remote(String, Option<u8>),
    /// 2017-18 online: everything on this PC (its own shop), the clubs and VS on the server data\panel.txt names
    /// (`server=`, the value held here) through [`vs_target`].
    Vs(String),
}

impl Mode {
    pub fn word(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Server => "server",
            Self::Remote(..) => "remote",
            Self::Vs(_) => "vs",
        }
    }
}

/// What the command line asks (play.py's words).
#[derive(Debug, PartialEq, Eq)]
pub enum Words {
    /// A plain start: what the SETTINGS panel chose.
    Plain {
        debug: bool,
    },
    Start {
        mode: Mode,
        debug: bool,
    },
    /// "remote" without an address: the one remembered in data\settings.json.
    RemoteRemembered {
        seat: Option<u8>,
        debug: bool,
    },
    Stop {
        force: bool,
        check: bool,
    },
    Status,
    Show,
    Restart,
    /// Make the card pictures (what the Setup step runs last; also from a console).
    Pictures,
    /// Wrong words: what to say (exit 2).
    Wrong(String),
}

/// 2017-18 has no remote seats in a shared shop (ROADMAP M2): a server or remote run is refused (what the player is
/// told). Its online play is VS between homes: ONLINE in the Launcher, then PLAY (a plain start, [`Mode::Vs`]).
pub const ONLINE_1718_TEXT: &str = "2017-18 has no remote seats or server mode yet. To play online, choose ONLINE in \
                                    the Launcher's window, then PLAY: this PC plays as its own shop, VS through the \
                                    server.";

/// 2017-18 refuses the shared-shop modes (server, remote).
pub fn online_refused(edition: crate::Edition, mode: &Mode) -> bool {
    edition == crate::Edition::E1718 && matches!(mode, Mode::Server | Mode::Remote(..))
}

/// Where a 2017-18 online run plays: the game server (control's GAME_SVR_IP / GAME_SVR_PORT, with GAME_SVR_ENTRY=1)
/// and, for a VS room the lobby held, the desk that holds its ticket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VsTarget {
    pub host: String,
    pub port: u16,
    /// The room's desk address and ticket (data\panel.txt `server=` and `ticket=`).
    pub room: Option<(String, String)>,
}

/// data\panel.txt's `server=` and `ticket=` and data\settings.json `club_server` into a [`VsTarget`]. A held room
/// plays on its own server's game server (port [`crate::choices::CLUB_PORT`]); else a club server overrides the
/// panel's server. `Err`: what is wrong, for the player.
pub fn vs_target(panel: &str, club_server: &str) -> Result<VsTarget, String> {
    use crate::choices::{CLUB_PORT, panel_value, parse_club_server, valid_address};
    let server = panel_value(panel, "server").filter(|s| valid_address(s));
    let ticket = panel_value(panel, "ticket").filter(|t| !t.is_empty());
    let club = club_server.trim();
    if let (Some(s), Some(t)) = (&server, ticket) {
        return Ok(VsTarget {
            host: s.clone(),
            port: CLUB_PORT,
            room: Some((s.clone(), t)),
        });
    }
    if !club.is_empty() {
        let (host, port) = parse_club_server(club)
            .ok_or_else(|| format!("club_server \"{club}\" in data\\settings.json is not HOST or HOST:PORT"))?;
        return Ok(VsTarget { host, port, room: None });
    }
    match server {
        Some(host) => Ok(VsTarget {
            host,
            port: CLUB_PORT,
            room: None,
        }),
        None => Err("the SETTINGS panel says play online, but names no server address".into()),
    }
}

pub const USAGE: &str = "Launcher.exe [local | server | remote [ADDRESS] [seat N]] [debug] | stop [force | check] | \
                         status | show | restart | pictures";

/// play.py main()'s reading of its words.
pub fn parse(words: &[String]) -> Words {
    let a: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    match a.first().map(String::as_str) {
        Some("stop") => {
            return Words::Stop {
                force: a.iter().any(|w| w == "force"),
                check: a.iter().any(|w| w == "check"),
            };
        }
        Some("status") => return Words::Status,
        Some("show") if a.len() == 1 => return Words::Show,
        Some("restart") => return Words::Restart,
        Some("pictures") if a.len() == 1 => return Words::Pictures,
        _ => {}
    }
    let debug = a.iter().any(|w| w == "debug");
    let rest: Vec<&str> = a.iter().map(String::as_str).filter(|w| *w != "debug").collect();
    match rest.as_slice() {
        [] => Words::Plain { debug },
        ["local"] => Words::Start {
            mode: Mode::Local,
            debug,
        },
        ["server"] => Words::Start {
            mode: Mode::Server,
            debug,
        },
        ["remote", more @ ..] => {
            let mut words = more.to_vec();
            let mut seat = None;
            if words.len() >= 2 && words[words.len() - 2] == "seat" {
                match words[words.len() - 1].parse::<u8>() {
                    Ok(n) if (1..=8).contains(&n) && words[words.len() - 1].bytes().all(|b| b.is_ascii_digit()) => {
                        seat = Some(n);
                    }
                    _ => {
                        return Words::Wrong(
                            "Seats are 1 to 8, e.g. Launcher.exe remote 192.168.1.20 seat 2 - or leave the seat out \
                             and the server gives the next free one."
                                .into(),
                        );
                    }
                }
                words.truncate(words.len() - 2);
            }
            match words.as_slice() {
                [] => Words::RemoteRemembered { seat, debug },
                [ip] if crate::choices::valid_address(ip) => Words::Start {
                    mode: Mode::Remote((*ip).to_owned(), seat),
                    debug,
                },
                [ip] => Words::Wrong(format!(
                    "\"{ip}\" is not an address the game can use: it takes four numbers 0-255 with dots, no names."
                )),
                _ => Words::Wrong(USAGE.into()),
            }
        }
        _ => Words::Wrong(USAGE.into()),
    }
}

/// The card rule (play.py's stop_guard): while seat 1 runs, nothing that would end it goes on while the club card is in
/// the reader or a card session is open - unless forced.  The refusal's words, or None to go on.
pub fn card_rule(seat_up: bool, force: bool, card_in: Option<bool>, session: Session) -> Option<String> {
    if !seat_up || force || (card_in == Some(false) && session != Session::Open) {
        return None;
    }
    Some(if session == Session::Open {
        "A card session is open: a match or Club Make is on, and the game saves your card in the locker room after \
         the match. Stopping now ends this match without that save, and the card gets a bad ending (trade rights are \
         lost at 2). If the game is stuck (\"WCCF CONTROL NOT FOUND\"), that match is lost already and stopping is \
         the way out."
            .into()
    } else {
        format!(
            "{} - the game may be saving it. If you can, press I in the game to take the card out first.",
            if card_in == Some(true) {
                "Your club card is IN the reader"
            } else {
                "Cannot tell whether your club card is in"
            }
        )
    })
}

/// When a game window's launcher has ended (play.py's crashed_logs): `gone` = the logs of the window launchers that
/// have ended, `others_alive` = another game window still runs.  None: stop the rest (a window was closed, or none is
/// left).  Some(logs): they went by themselves (crashes) - nothing stops, the projector keeps running.
pub fn after_end(gone: &[&str], others_alive: bool, read: impl Fn(&str) -> String) -> Option<Vec<String>> {
    let mut gone: Vec<String> = gone.iter().map(|s| (*s).to_owned()).collect();
    gone.sort();
    gone.dedup();
    if gone.iter().any(|log| read(log).contains(WINDOW_CLOSED)) || !others_alive {
        return None;
    }
    Some(gone)
}

/// Why a run went wrong, empty if it did not (play.py's went_wrong): a game window that went without being closed, a
/// crash a launcher recorded, the server link lost (forza_panel.dll's server_link.txt; a drop after the player's close is
/// written ENDED, not LOST, and is no fault).
pub fn went_wrong(launcher_logs: &[&str], read: impl Fn(&str) -> String) -> Vec<String> {
    let mut why: Vec<String> = launcher_logs
        .iter()
        .filter(|n| !read(n).contains(WINDOW_CLOSED))
        .map(|n| format!("{n}: the game window went by itself, not closed"))
        .collect();
    let mut logs: Vec<&str> = launcher_logs.iter().copied().chain(["run_server.txt"]).collect();
    logs.sort_unstable();
    logs.dedup();
    for n in logs {
        let text = read(n);
        why.extend(
            CRASH_SIGNS
                .iter()
                .filter(|s| text.contains(**s))
                .map(|s| format!("{n}: a crash recorded ({s})")),
        );
    }
    why.extend(
        read("server_link.txt")
            .lines()
            .filter(|l| l.contains(" LOST "))
            .map(|l| format!("server_link.txt: {}", l.trim())),
    );
    why
}

/// Why no seat was taken on the server at `ip` (play.py's _seat_decision messages): every fault but "no broker" stops
/// the start, so two PCs never land on one seat (A4).
pub fn seat_refusal(e: &SeatError, asked: Option<u8>, ip: &str) -> String {
    let detail = if e.detail.is_empty() {
        e.fault.as_str()
    } else {
        &e.detail
    };
    match e.fault {
        Fault::Full => format!("the server at {ip} is full: 8 cabinets play there now"),
        Fault::Taken => format!(
            "seat {} is taken on the server at {ip} - leave the seat number out to get the next free one",
            asked.map_or_else(|| "?".into(), |s| s.to_string())
        ),
        Fault::NoAnswer => format!(
            "the server's seat desk at {ip} did not answer in time ({detail}) - not started, so this PC cannot land on \
             a seat another player is already holding. Try again, or Launcher.exe local."
        ),
        _ => format!("the server's seat desk at {ip} would not give a seat ({detail})"),
    }
}

/// RESTART NOW needs only the cabinet (play.py's cabinet_restart_fits) when a plain start would play the same way -
/// THIS PC, or ONLINE on the same server - and no change of the game's text is asked: the projector would come back
/// exactly as it is, so it is kept.  `panel` is data\panel.txt, `language` the game's text now ("en", "it", "");
/// `vs` (2017-18 online) data\settings.json `club_server` and the [`VsTarget`] the run started with: the same game
/// server, and no other VS room's ticket waiting in the panel (the run's own ticket left it at the start).
pub fn cabinet_restart_fits(
    mode: &Mode,
    has_cabinet: bool,
    panel: &str,
    language: &str,
    vs: (&str, Option<&VsTarget>),
) -> bool {
    use crate::choices::panel_value;
    if !has_cabinet {
        return false;
    }
    if english_change(panel, language).is_some() {
        return false;
    }
    let online = panel_value(panel, "play_on").is_some_and(|v| v.eq_ignore_ascii_case("online"));
    match mode {
        Mode::Remote(ip, _) => online && panel_value(panel, "server").as_deref() == Some(ip.as_str()),
        Mode::Local => !online,
        Mode::Vs(_) => {
            let (club, started) = vs;
            online
                && started.is_some_and(|s| {
                    vs_target(panel, club).is_ok_and(|now| {
                        (now.host.as_str(), now.port) == (s.host.as_str(), s.port)
                            && now.room.is_none_or(|r| Some(&r) == s.room.as_ref())
                    })
                })
        }
        Mode::Server => false,
    }
}

/// The SETTINGS panel's english=on|it|off when it asks for another text than `language` has in place: the language
/// it asks for ("en", "it", "").
pub fn english_change(panel: &str, language: &str) -> Option<&'static str> {
    let want = match crate::choices::panel_value(panel, "english")?.to_lowercase().as_str() {
        "on" => "en",
        "it" => "it",
        "off" => "",
        _ => return None,
    };
    (want != language).then_some(want)
}

/// The `__COMPAT_LAYER` a game program starts with: `existing` (the Launcher's own, if any) plus `HighDpiAware`.  The
/// game programs declare no DPI awareness, so on a screen scaled above 100% Windows draws them at a lower size and
/// stretches the picture (and the panel on it) blurry; this per-process compatibility layer makes Windows give them
/// real pixels instead.  Nothing on the host changes: it is an environment variable of the child alone.
pub fn compat_layer(existing: Option<&str>) -> String {
    let mut layers: Vec<&str> = existing.unwrap_or_default().split_whitespace().collect();
    if !layers.iter().any(|l| l.eq_ignore_ascii_case("HighDpiAware")) {
        layers.push("HighDpiAware");
    }
    layers.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_programs_are_dpi_aware() {
        assert_eq!(compat_layer(None), "HighDpiAware");
        assert_eq!(compat_layer(Some("")), "HighDpiAware");
        assert_eq!(compat_layer(Some("~ RUNASADMIN")), "~ RUNASADMIN HighDpiAware");
        assert_eq!(compat_layer(Some("highdpiaware")), "highdpiaware");
    }

    #[test]
    fn online_1718_is_refused() {
        use crate::Edition::{E1011, E1718};
        let remote = Mode::Remote("192.168.1.20".into(), None);
        assert!(online_refused(E1718, &remote));
        assert!(online_refused(E1718, &Mode::Server));
        assert!(!online_refused(E1718, &Mode::Local));
        assert!(!online_refused(E1718, &Mode::Vs("192.168.1.20".into())));
        assert!(!online_refused(E1011, &remote));
        assert!(!online_refused(E1011, &Mode::Server));
    }

    #[test]
    fn vs_target_room_then_club_then_panel() {
        let t = |panel: &str, club: &str| vs_target(panel, club);
        let room = t(
            "play_on=online\nserver=203.0.113.7\nticket=ab12cd34\n",
            "club.example.org:2000",
        )
        .unwrap();
        assert_eq!(
            room,
            VsTarget {
                host: "203.0.113.7".into(),
                port: 20060,
                room: Some(("203.0.113.7".into(), "ab12cd34".into())),
            },
            "a held room plays on its own server, whatever the club server says"
        );
        let club = t("play_on=online\nserver=203.0.113.7\n", " club.example.org:2000 ").unwrap();
        assert_eq!(
            (club.host.as_str(), club.port, club.room),
            ("club.example.org", 2000, None)
        );
        let plain = t("play_on=online\nserver=203.0.113.7\nticket=\n", "").unwrap();
        assert_eq!(
            (plain.host.as_str(), plain.port, plain.room),
            ("203.0.113.7", 20060, None)
        );
        assert!(t("play_on=online\n", "").is_err());
        assert!(t("play_on=online\nserver=not an address\n", "").is_err());
        assert!(t("play_on=online\nserver=203.0.113.7\n", "bad host:0").is_err());
    }

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn logs(pairs: &[(&str, &str)]) -> impl Fn(&str) -> String {
        let map: Vec<(String, String)> = pairs.iter().map(|(a, b)| ((*a).into(), (*b).into())).collect();
        move |n: &str| {
            map.iter()
                .find(|(k, _)| k == n)
                .map_or_else(String::new, |(_, v)| v.clone())
        }
    }

    const CLOSED: &str = "  t+ 42.0s its window was closed - ending client_Release.exe\n";

    #[test]
    fn words() {
        assert_eq!(parse(&[]), Words::Plain { debug: false });
        assert_eq!(parse(&w("DEBUG")), Words::Plain { debug: true });
        assert_eq!(
            parse(&w("local")),
            Words::Start {
                mode: Mode::Local,
                debug: false
            }
        );
        assert_eq!(
            parse(&w("server debug")),
            Words::Start {
                mode: Mode::Server,
                debug: true
            }
        );
        assert_eq!(
            parse(&w("remote 1.2.3.4 seat 3")),
            Words::Start {
                mode: Mode::Remote("1.2.3.4".into(), Some(3)),
                debug: false
            }
        );
        assert_eq!(
            parse(&w("remote")),
            Words::RemoteRemembered {
                seat: None,
                debug: false
            }
        );
        assert_eq!(
            parse(&w("remote seat 2")),
            Words::RemoteRemembered {
                seat: Some(2),
                debug: false
            }
        );
        assert!(matches!(parse(&w("remote 1.2.3.4 seat 9")), Words::Wrong(m) if m.starts_with("Seats are 1 to 8")));
        assert!(matches!(parse(&w("remote host.example")), Words::Wrong(m) if m.contains("no names")));
        assert!(matches!(parse(&w("remote 1.2.3.4 5.6.7.8")), Words::Wrong(_)));
        assert!(
            matches!(parse(&w("ended")), Words::Wrong(_)),
            "the watcher's word is gone"
        );
        assert_eq!(
            parse(&w("stop")),
            Words::Stop {
                force: false,
                check: false
            }
        );
        assert_eq!(
            parse(&w("stop force")),
            Words::Stop {
                force: true,
                check: false
            }
        );
        assert_eq!(
            parse(&w("stop check")),
            Words::Stop {
                force: false,
                check: true
            }
        );
        assert_eq!(parse(&w("status")), Words::Status);
        assert_eq!(parse(&w("show")), Words::Show);
        assert_eq!(parse(&w("restart")), Words::Restart);
        assert_eq!(parse(&w("Pictures")), Words::Pictures);
        assert!(matches!(parse(&w("pictures now")), Words::Wrong(_)));
    }

    #[test]
    fn the_card_rule() {
        assert_eq!(
            card_rule(false, false, Some(true), Session::Open),
            None,
            "seat 1 not up: nothing to protect"
        );
        assert_eq!(card_rule(true, true, Some(true), Session::Open), None, "forced");
        assert_eq!(
            card_rule(true, false, Some(false), Session::Closed),
            None,
            "card out, no session"
        );
        assert_eq!(
            card_rule(true, false, Some(false), Session::Cut),
            None,
            "an old cut is no open session"
        );
        let r = card_rule(true, false, Some(true), Session::Closed).unwrap();
        assert!(r.starts_with("Your club card is IN the reader"), "{r}");
        let r = card_rule(true, false, None, Session::Closed).unwrap();
        assert!(r.starts_with("Cannot tell"), "{r}");
        let r = card_rule(true, false, Some(false), Session::Open).unwrap();
        assert!(
            r.starts_with("A card session is open"),
            "the session counts with the card out: {r}"
        );
    }

    /// test_ended.py
    #[test]
    fn a_closed_window_stops_the_rest_a_crash_does_not() {
        let seat = "run_seat1.txt";
        let proj = "run_projector.txt";
        assert_eq!(
            after_end(&[seat], true, logs(&[(seat, CLOSED)])),
            None,
            "seat closed: the rest stops"
        );
        assert_eq!(
            after_end(&[seat], true, logs(&[(seat, "  FATAL 0xC0000005\n")])),
            Some(vec![seat.to_owned()]),
            "seat crashed while the projector runs: nothing stops"
        );
        assert_eq!(
            after_end(&[proj], true, logs(&[])),
            Some(vec![proj.to_owned()]),
            "projector crashed: seat 1 plays on"
        );
        assert_eq!(
            after_end(&[seat, proj], false, logs(&[])),
            None,
            "no game window left: the rest stops"
        );
        assert_eq!(
            after_end(&[], true, logs(&[])),
            Some(vec![]),
            "nothing ended: nothing stops"
        );
    }

    /// test_ended.py: the automatic report's reasons
    #[test]
    fn what_went_wrong() {
        let seat = ["run_seat1.txt"];
        assert!(
            went_wrong(&seat, logs(&[("run_seat1.txt", CLOSED)])).is_empty(),
            "closed by the player"
        );
        let why = went_wrong(&seat, logs(&[("run_seat1.txt", "  t+ 9.0s process exited\n")]));
        assert!(
            why[0].starts_with("run_seat1.txt: the game window went by itself"),
            "{why:?}"
        );
        let crash = format!("{CLOSED}  SECOND-chance / FATAL exception 0xC0000005\n");
        let why = went_wrong(&seat, logs(&[("run_seat1.txt", &crash)]));
        assert_eq!(why, ["run_seat1.txt: a crash recorded (SECOND-chance / FATAL)"]);
        let lost = "2026-10-11 20:01:02 UP 1.2.3.4\n2026-10-11 20:09:40 LOST (it was up 518 s)\n";
        let why = went_wrong(&seat, logs(&[("run_seat1.txt", CLOSED), ("server_link.txt", lost)]));
        assert_eq!(why, ["server_link.txt: 2026-10-11 20:09:40 LOST (it was up 518 s)"]);
        let ended = "2026-10-13 00:01:00 UP 1.2.3.4\n2026-10-13 00:02:03 ENDED (it was up 63 s)\n";
        let why = went_wrong(&seat, logs(&[("run_seat1.txt", CLOSED), ("server_link.txt", ended)]));
        assert!(why.is_empty(), "a drop after the player's close is no fault: {why:?}");
        let why = went_wrong(
            &seat,
            logs(&[
                ("run_seat1.txt", CLOSED),
                ("run_server.txt", "  STOP: invalid parameter\n"),
            ]),
        );
        assert_eq!(why, ["run_server.txt: a crash recorded (STOP: invalid parameter)"]);
    }

    /// The words must stay the launcher's and the panel's own (test_ended.py read debugger.py and wccfpanel.c; now the Rust panel's link_watch).
    #[test]
    fn the_words_match_their_writers() {
        let dbg = include_str!("../debug/mod.rs");
        assert!(
            dbg.contains(WINDOW_CLOSED),
            "debug::run no longer prints {WINDOW_CLOSED:?}"
        );
        for s in CRASH_SIGNS {
            assert!(dbg.contains(s), "debug::run no longer prints {s:?}");
        }
        let panel = include_str!("../../../panel/src/lib.rs");
        assert!(
            panel.contains("\"LOST\"") && panel.contains(" (it was up "),
            "forza-panel link_watch's LOST line"
        );
        assert!(
            panel.contains("\"ENDED\"") && panel.contains("CLOSING.load"),
            "forza-panel link_watch's ENDED after a close"
        );
        assert!(
            panel.contains(&format!("\"{}", super::super::PANEL_UP)),
            "the panel no longer logs the line the run waits for ({:?})",
            super::super::PANEL_UP
        );
        assert!(
            panel.contains("running.json"),
            "the panel reads the run's start from running.json's time"
        );
    }

    /// test_take_seat.py's messages (the decision itself is SeatError::no_desk_seat, tested in forza).
    #[test]
    fn seat_refusals_name_the_trouble() {
        let e = |fault, detail: &str| SeatError {
            fault,
            detail: detail.into(),
        };
        assert!(seat_refusal(&e(Fault::Full, "FULL"), None, "20.0.0.1").contains("full"));
        let m = seat_refusal(&e(Fault::NoAnswer, "slow"), None, "20.0.0.1");
        assert!(m.contains("did not answer") && m.contains("local"), "{m}");
        assert!(seat_refusal(&e(Fault::Taken, "TAKEN 2"), Some(2), "20.0.0.1").starts_with("seat 2 is taken"));
        assert_eq!(e(Fault::NoBroker, "refused").no_desk_seat(None), Some((1, true)));
        assert_eq!(e(Fault::Full, "").no_desk_seat(None), None);
    }

    #[test]
    fn restart_keeps_the_projector_only_when_nothing_changes() {
        const NO_VS: (&str, Option<&VsTarget>) = ("", None);
        let remote = Mode::Remote("1.2.3.4".into(), None);
        assert!(cabinet_restart_fits(&Mode::Local, true, "play_on=this_pc\n", "", NO_VS));
        assert!(cabinet_restart_fits(&Mode::Local, true, "", "en", NO_VS));
        assert!(
            !cabinet_restart_fits(&Mode::Local, false, "", "", NO_VS),
            "no cabinet running"
        );
        assert!(!cabinet_restart_fits(
            &Mode::Local,
            true,
            "play_on=online\nserver=1.2.3.4\n",
            "",
            NO_VS
        ));
        assert!(cabinet_restart_fits(
            &remote,
            true,
            "play_on=online\nserver=1.2.3.4\n",
            "",
            NO_VS
        ));
        assert!(!cabinet_restart_fits(
            &remote,
            true,
            "play_on=online\nserver=5.6.7.8\n",
            "",
            NO_VS
        ));
        assert!(
            !cabinet_restart_fits(&Mode::Local, true, "english=on\n", "", NO_VS),
            "a text switch needs a full restart"
        );
        assert!(
            cabinet_restart_fits(&Mode::Local, true, "english=on\n", "en", NO_VS),
            "already English: no switch"
        );
        assert!(!cabinet_restart_fits(&Mode::Server, true, "", "", NO_VS));
        // 2017-18 online: the game server the run started on, and no new room's ticket
        let vs = Mode::Vs("1.2.3.4".into());
        let on = "play_on=online\nserver=1.2.3.4\n";
        let club = vs_target(on, "club.example:20061").unwrap();
        assert!(cabinet_restart_fits(
            &vs,
            true,
            on,
            "",
            ("club.example:20061", Some(&club))
        ));
        assert!(
            !cabinet_restart_fits(&vs, true, on, "", ("", Some(&club))),
            "club server taken off"
        );
        assert!(!cabinet_restart_fits(&vs, true, on, "", ("other.example", Some(&club))));
        let room = vs_target("play_on=online\nserver=1.2.3.4\nticket=ab12\n", "").unwrap();
        assert!(
            cabinet_restart_fits(&vs, true, on, "", ("", Some(&room))),
            "the room's ticket was spent"
        );
        let next = "play_on=online\nserver=1.2.3.4\nticket=cd34\n";
        assert!(
            !cabinet_restart_fits(&vs, true, next, "", ("", Some(&room))),
            "another room"
        );
        assert!(!cabinet_restart_fits(&vs, true, on, "", ("", None)));
        assert_eq!(english_change("english=IT\n", "en"), Some("it"));
        assert_eq!(english_change("english=off\n", ""), None);
        assert_eq!(english_change("english=maybe\n", ""), None);
    }
}
