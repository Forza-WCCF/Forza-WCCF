//! The command line: play.py's words, the stop and the plain start.

use super::*;

/// play.py's words (`rules::parse`): a start supervises its run until it ends; stop, status and show ask the running
/// game.  The exit code.
pub fn command(kit: &Kit, p: &Params, words: &[String], ui: &dyn Ui) -> i32 {
    match rules::parse(words) {
        Words::Wrong(why) => {
            ui.say(&why);
            NOT_SET_UP
        }
        Words::Stop { force, check } => stop_command(kit, p, force, check, ui),
        Words::Status => ask_each(kit, p, "status", ui).unwrap_or_else(|| {
            tell(ui, "", &Text::key("launcher.run_nothing_running"));
            outside(kit, p, ui)
        }),
        Words::Show => ask_each(kit, p, "show", ui).unwrap_or_else(|| {
            tell(ui, "", &Text::key("launcher.run_no_server"));
            FAILED
        }),
        Words::Restart if is_on(kit, Kind::Run) => {
            match choices::write(
                &kit.data().join("restart.request"),
                "RESTART NOW - asked from the command line\n",
            ) {
                Ok(()) => {
                    tell(ui, "", &Text::key("launcher.run_restarting_running"));
                    OK
                }
                Err(e) => {
                    tell(ui, "", &Text::key("launcher.run_no_restart").arg("error", e));
                    FAILED
                }
            }
        }
        Words::Restart => play(kit, p, None, false, ui),
        Words::Pictures => {
            tell(ui, "", &Text::key("launcher.msg_making_pictures"));
            if make_pictures(kit, p, ui) {
                tell(ui, "", &Text::key("launcher.run_pictures_ready"));
                OK
            } else {
                FAILED
            }
        }
        Words::Plain { debug } => play(kit, p, None, debug, ui),
        Words::Start { mode, debug } => {
            // an arcade's tunnel id is not a server address: the lobby would ask it
            if let Mode::Remote(ip, _) = &mode
                && vslink::arcade::parse_id(ip).is_none()
                && choices::setting(&kit.settings_file(), "server").as_deref() != Some(ip)
            {
                let _ = choices::set_setting(&kit.settings_file(), "server", ip); // "remote" alone uses it again
            }
            play(kit, p, Some(mode), debug, ui)
        }
        Words::RemoteRemembered { seat, debug } => match choices::setting(&kit.settings_file(), "server") {
            Some(ip) if choices::host_port(&ip, p.seat_port).is_some() => {
                play(kit, p, Some(Mode::Remote(ip, seat)), debug, ui)
            }
            _ => {
                tell(ui, "", &Text::key("launcher.run_which_server"));
                NOT_SET_UP
            }
        },
    }
}

/// Asks every run that is on; None if none is.
fn ask_each(kit: &Kit, _p: &Params, word: &str, ui: &dyn Ui) -> Option<i32> {
    let mut rc = None;
    for kind in [Kind::Run, Kind::Server] {
        if let Some((code, said)) = request(kit, kind, word) {
            for line in said.lines() {
                ui.say(line);
            }
            rc = Some(rc.unwrap_or(OK).max(code));
        }
    }
    rc
}

/// The game's programs running from this game's folders that no run of this kit holds (left by an older kit): said,
/// never stopped by name.
fn outside(kit: &Kit, p: &Params, ui: &dyn Ui) -> i32 {
    let Some(game) = game_folder(kit, p) else { return OK };
    let left = strays(&game, p);
    if left.is_empty() {
        return OK;
    }
    let list = left
        .iter()
        .map(|q| format!("{} {}", q.name, q.pid))
        .collect::<Vec<_>>()
        .join(", ");
    // 2017-18's SETUP tab has CLEAN UP, which ends those without a window
    let key = if kit.edition() == Edition::E1718 {
        "launcher.run_strays_1718"
    } else {
        "launcher.run_strays"
    };
    tell(ui, "", &Text::key(key).arg("list", list));
    FAILED
}

fn stop_command(kit: &Kit, p: &Params, force: bool, check: bool, ui: &dyn Ui) -> i32 {
    let word = if check {
        "check"
    } else if force {
        "force"
    } else {
        "stop"
    };
    let mut rc = None;
    let mut run_answer = None;
    for kind in [Kind::Run, Kind::Server] {
        if kind == Kind::Server && is_on(kit, Kind::Server) {
            // a seat that stopped lets go of its lock a moment after its answer
            let end = Instant::now() + Duration::from_secs(5);
            while run_answer == Some(OK) && is_on(kit, Kind::Run) && Instant::now() < end {
                thread::sleep(Duration::from_millis(100));
            }
            if !rules::stop_server_too(run_answer, is_on(kit, Kind::Run)) {
                tell(ui, "", &Text::key("launcher.run_server_keeps"));
                rc = Some(rc.unwrap_or(REFUSED)); // something runs: not "Nothing running."
                continue;
            }
        }
        let Some((mut code, said)) = request(kit, kind, word) else {
            continue;
        };
        let (shown, question) = rules::stop_question(&said, kit.edition());
        for line in shown {
            ui.say(line);
        }
        // never a dead end: the person at the stop decides, knowing the cost
        if code == REFUSED && !check && ui.ask(&question.get(ui.lang())) {
            tell(ui, "", &Text::key("launcher.run_stopping"));
            let (c, said) = request(kit, kind, "force").unwrap_or((OK, String::new()));
            for line in said.lines() {
                ui.say(line);
            }
            code = c;
        } else if code == REFUSED && !check {
            tell(ui, "", &Text::key("launcher.run_force_hint"));
        }
        if kind == Kind::Run {
            run_answer = Some(code);
        }
        rc = Some(rc.unwrap_or(OK).max(code));
    }
    rc.unwrap_or_else(|| {
        tell(ui, "", &Text::key("launcher.run_nothing_running"));
        outside(kit, p, ui)
    })
}

/// The run is over: [`CUP_STARTED`] is cleared, so the PLAY tab previews the choice again.
struct ForgetCup;

impl Drop for ForgetCup {
    fn drop(&mut self) {
        remember_cup(None);
    }
}

/// Starts and supervises; a full RESTART NOW starts again with what a plain start chooses.
pub fn play(kit: &Kit, p: &Params, mode: Option<Mode>, debug: bool, ui: &dyn Ui) -> i32 {
    let _forget = ForgetCup; // every way out: the run it remembers has ended
    let mut mode = mode;
    loop {
        let m = mode.take().unwrap_or_else(|| plain_mode(kit, p, ui));
        match start(kit, p, m, debug, ui) {
            Err(code) => return code,
            Ok(run) => match run.supervise() {
                End::Over(code) => return code,
                End::Again => {}
            },
        }
    }
}

/// What a plain start does: what the SETTINGS panel chose - never locked out: an address that is wrong, or a server
/// that does not answer, plays on this PC and says why.
fn plain_mode(kit: &Kit, p: &Params, ui: &dyn Ui) -> Mode {
    // an unreadable file is said (and logged) like the other reasons a plain start plays locally: the player would
    // otherwise take it for their own choice and look for the lobby
    let panel = match choices::read_ok(&kit.data().join("panel.txt")) {
        Ok(panel) => panel,
        Err(e) => {
            tell(ui, "", &Text::key("launcher.run_panel_unread").arg("error", e));
            String::new()
        }
    };
    if !choices::panel_value(&panel, "play_on").is_some_and(|v| v.eq_ignore_ascii_case("online")) {
        return Mode::Local;
    }
    let ip = choices::panel_value(&panel, "server").unwrap_or_default();
    if kit.edition() == Edition::E1718 {
        // VS between homes: this PC is its own arcade, the clubs and the pairing on the server's game server
        let club = choices::setting(&kit.settings_file(), "club_server").unwrap_or_default();
        return match rules::vs_target(&panel, &club) {
            Err(why) => {
                tell(ui, "", &Text::key("launcher.run_local_because").part("why", why));
                Mode::Local
            }
            Ok(t) if reach(&t.host, t.port).is_none() => {
                tell(
                    ui,
                    "",
                    &Text::key("launcher.run_vs_no_answer")
                        .arg("host", &t.host)
                        .arg("port", t.port),
                );
                Mode::Local
            }
            Ok(t) => {
                tell(ui, "", &Text::key("launcher.run_online_vs").arg("host", &t.host));
                Mode::Vs(ip)
            }
        };
    }
    let host = choices::host_port(&ip, p.seat_port).map(|(h, _)| h);
    let Some(host) = host else {
        tell(ui, "", &Text::key("launcher.run_bad_server").arg("ip", &ip));
        return Mode::Local;
    };
    if reach(&host, p.control_port).is_none() {
        tell(ui, "", &Text::key("launcher.run_server_no_answer").arg("ip", &ip));
        return Mode::Local;
    }
    tell(ui, "", &Text::key("launcher.run_online"));
    Mode::Remote(ip, None)
}

/// Something takes a TCP connection on ip:port within 5 s (closed again at once).
pub(super) fn answers(ip: &str, port: u16) -> bool {
    format!("{ip}:{port}")
        .parse::<SocketAddr>()
        .is_ok_and(|a| TcpStream::connect_timeout(&a, Duration::from_secs(5)).is_ok())
}

/// The first of `host`'s addresses that takes a TCP connection on `port` within 5 s (a name or an IPv4 address).
pub(super) fn reach(host: &str, port: u16) -> Option<SocketAddr> {
    (host, port)
        .to_socket_addrs()
        .ok()?
        .find(|a| TcpStream::connect_timeout(a, Duration::from_secs(5)).is_ok())
}

pub(super) fn addr(ip: &str, port: u16) -> Option<SocketAddr> {
    format!("{ip}:{port}").parse().ok()
}

pub(super) fn now_local() -> time::OffsetDateTime {
    time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc())
}

/// The player's notice: their club plays the Universal Championship on the projector from about `opens` (this PC's
/// clock). The finalists' seats need nothing more: the control plays the team each winner registered for the cup and
/// sends the results itself (0x47BFF0, 0x462990).
pub(super) fn championship_notice(opens: SystemTime) -> Text {
    let t = time::OffsetDateTime::from(opens).to_offset(now_local().offset());
    Text::key("launcher.run_championship").arg("time", format!("{:02}:{:02}", t.hour(), t.minute()))
}
