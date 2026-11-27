//! The reports and the supervisor: `Run::supervise` until the run ends, then the stop.

use super::*;

impl Run<'_> {
    // ---- reports ----

    fn read_log(&self, name: &str) -> String {
        // unreadable counts as no close: the safe side keeps the rest running
        let text = fs::read(self.logs().join(name))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        // a restarted cabinet appended its session to the log: the running session is the one that says how it went
        logs::last_session(&text).to_owned()
    }

    /// SAVE LOGS (the SETTINGS panel, data\savelogs.request): the ZIP written to [`Params::logs_dir`] on a thread;
    /// data\savelogs.result tells the panel "ok PATH" or "fail WHY", and the line comes back as [`Event::Said`].
    fn save_logs(&self) {
        let (dir, kit, game) = (self.p.logs_dir.clone(), self.kit.dir().to_path_buf(), self.game.clone());
        let tx = self.tx.clone();
        thread::spawn(move || {
            let line = match crate::report::save_zip(&dir, &kit, Some(&game), &Scrub::from_env(), None) {
                Ok(path) => format!("ok {}", path.display()),
                Err(e) => format!("fail {e}"),
            };
            let _ = choices::write(&kit.join("data").join("savelogs.result"), &format!("{line}\n"));
            let _ = tx.send(Event::Said(match line.strip_prefix("ok ") {
                Some(path) => Text::key("launcher.run_logs_saved").arg("path", path),
                None => Text::key("launcher.run_logs_not_saved").arg("error", &line[5..]),
            }));
        });
    }

    /// 2017-18 with the control here: the bracket's line in the log at each change of today's Universal Championship
    /// (game\log\<date>_uc.bin), a line when a player's club leaves it, and once a day, when it has a player's club,
    /// the notice with about when it plays on the projector.
    fn watch_championship(&mut self) {
        if self.edition != Edition::E1718 || !self.alive(Role::Server) {
            return;
        }
        let date = championship::date(now_local());
        let f = championship::file(&self.game, date);
        let Ok(at) = fs::metadata(&f).and_then(|m| m.modified()) else {
            return;
        };
        if self.uc_seen == Some(at) {
            return;
        }
        let b = fs::read(&f).unwrap_or_default();
        let log = self.logs().join("run_launcher.txt");
        if championship::whole(&b).is_err() {
            // read while the control writes it: not judged, read again at the next look (one line per change)
            if self.uc_partial != Some(at) {
                self.uc_partial = Some(at);
                crate::logs::append(&log, &championship::line_of(date, &b));
            }
            return;
        }
        self.uc_seen = Some(at);
        let teams = championship::teams(&b);
        // every change of the day file gets its line (a few a day): a player's report then shows when the club went
        // in, its fix/hasDvs flags, and a swap for a CPU team
        crate::logs::append(&log, &championship::line_of(date, &b));
        let mine = championship::players(&teams);
        match &mut self.uc_player {
            Some((d, club, told)) if *d == date && mine.is_empty() && !*told => {
                *told = true;
                crate::logs::append(
                    &log,
                    &format!(
                        "championship: {date} {club} is no longer in the bracket - the control swapped it for a CPU team (fix or hasDvs 0, control 0x47DCF0 \"change cpu!\")"
                    ),
                );
            }
            _ if !mine.is_empty() => self.uc_player = Some((date, mine.clone(), false)),
            _ => {}
        }
        // shortcut: any player's club counts as "yours"; an arcade host with guests may see a guest's win, tell the
        // clubs apart (block +0 = coach id) if hosts report it
        if mine.is_empty() || self.uc_told == Some(date) {
            return;
        }
        self.uc_told = Some(date);
        // the winner alone is written as the cup's final ends, and the championship opens at the next slot (2-3 min
        // later); all four teams are written as it opens
        let opens = at + Duration::from_secs(if teams.len() < 4 { 180 } else { 0 });
        // the notice sends the player to the big screen: tell it only when the projector runs with its window shown
        if self.projector && choices::projector_shown(&self.kit.settings()) {
            self.tell("", &championship_notice(opens));
        }
    }

    // ---- the supervisor ----

    /// Requests from outside (other Launcher.exe processes, the panel's files) as events, every half second.
    fn poll_requests(&self) {
        let (tx, kit, kind) = (self.tx.clone(), self.kit.dir().to_path_buf(), Kind::of(&self.mode));
        let data = kit.join("data");
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(500));
                let mut events = Vec::new();
                if kind == Kind::Run
                    && let Some(text) = take(&data.join("restart.request"))
                {
                    events.push(Event::Restart(rules::restart_forced(&text)));
                }
                if let Some(word) = take(&data.join(format!("{}.request", kind.name()))) {
                    events.push(Event::Request(word.trim().to_owned()));
                }
                if kind == Kind::Run && take(&data.join("savelogs.request")).is_some() {
                    events.push(Event::SaveLogs);
                }
                if kind == Kind::Run && take(&data.join("report.request")).is_some() {
                    events.push(Event::Report);
                }
                if events.into_iter().any(|e| tx.send(e).is_err()) {
                    return; // the run is over
                }
            }
        });
    }

    /// Keeps the run until it is over: a closed game window stops the rest (the card rule permitting), a crash stops
    /// nothing while another game window runs, RESTART NOW restarts, requests are answered.
    pub fn supervise(mut self) -> End {
        self.ui.running(true);
        self.poll_requests();
        loop {
            let ev = match self.rx.recv_timeout(Duration::from_millis(500)) {
                Ok(ev) => ev,
                Err(RecvTimeoutError::Timeout) => {
                    self.watch_championship();
                    if let Some(end) = self.check_threads() {
                        self.ui.running(false);
                        return end;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return End::Over(OK),
            };
            let end = match ev {
                Event::Ended(id) => self.ended(id),
                Event::Restart(force) => self.restart(force),
                Event::Request(word) => self.answer(&word),
                Event::SaveLogs => {
                    self.save_logs();
                    None
                }
                Event::Said(line) => {
                    self.tell("", &line);
                    None
                }
                Event::Report => {
                    self.ui.report();
                    None
                }
            };
            if let Some(end) = end {
                self.ui.running(false);
                return end;
            }
        }
    }

    /// An arcade guest whose host closed the arcade (the tunnel heard it; without one, the club's last copy-back found
    /// the host's server gone): the end's notice, with the club home or maybe missing its last saves (then the marker
    /// stays, so the next start says so too). After `shutdown`: dropping the trip joins it, its last copy-back done.
    fn host_closed_notice(&self) -> Option<Text> {
        if matches!(self.mode, Mode::Vs(_)) {
            return None; // no host: the end says whether the club came home (finish)
        }
        let progress = self
            .visit
            .club
            .as_ref()
            .map(|p| *p.lock().unwrap_or_else(PoisonError::into_inner));
        let closed = match &self.visit.host_closed {
            Some(flag) => flag.load(Ordering::Relaxed),
            None => progress.is_some_and(|p| p.gone),
        };
        closed.then(|| rules::host_closed_notice(progress.map(|p| p.home)))
    }

    /// Stand-in and helper threads that ended by themselves (a panic, an error) are said once, in the run log; the run
    /// stops when the game cannot go on without one (the card reader, the I/O board, the scene service, the network
    /// stand-ins; 2017-18: the Aime reader, AiMeDB, the game server).  The key driver and the helpers are said and the game goes on.
    fn check_threads(&mut self) -> Option<End> {
        let mut found: Vec<(Text, bool, String)> = Vec::new();
        let mut look = |name: Text, essential: bool, h: Option<&mut Handle>| {
            if let Some(why) = h.and_then(Handle::died) {
                found.push((name, essential, why));
            }
        };
        for (h, s) in self.cabinet.net.iter_mut().zip(network::SERVICES) {
            look(Text::key("launcher.run_part_network").arg("name", s.1), true, Some(h));
        }
        let reader = match self.edition {
            Edition::E1011 => "launcher.run_part_reader",
            Edition::E1718 => "launcher.run_part_aime",
        };
        look(Text::key(reader), true, self.cabinet.reader.as_mut());
        look(
            Text::key("launcher.run_part_aimedb"),
            true,
            self.cabinet.aimedb.as_mut(),
        );
        look(Text::key("launcher.run_part_gamesvr"), true, self.gamesvr.as_mut());
        look(Text::key("launcher.run_part_vslink"), false, self.vslink.as_mut());
        look(Text::key("launcher.run_part_board"), true, self.cabinet.board.as_mut());
        look(Text::key("launcher.run_part_keys"), false, self.cabinet.keys.as_mut());
        look(Text::key("launcher.run_part_scenes"), true, self.scenes.as_mut());
        for (name, essential, h) in &mut self.cabinet.extra {
            look(Text::Raw(name.clone()), *essential, Some(h));
        }
        if let Some(h) = &mut self.helper {
            found.extend(
                h.died()
                    .into_iter()
                    .map(|(n, why)| (Text::key("launcher.run_part_helper").arg("name", n), false, why)),
            );
        }
        let mut stop = None;
        for (name, essential, why) in found {
            let key = if essential {
                "launcher.run_part_stopped_fatal"
            } else {
                "launcher.run_part_stopped"
            };
            self.tell("", &Text::key(key).part("name", name.clone()).arg("why", why));
            if essential {
                stop.get_or_insert(name);
            }
        }
        let name = stop?;
        Some(
            match self.finish(false, Some(Text::key("launcher.run_stopped_part").part("name", name))) {
                End::Over(_) | End::Again => End::Over(FAILED),
            },
        )
    }

    /// The card rule now: the refusal's words, or None to go on.
    /// Why a stop waits, and the guests it waits for (0 unless they alone hold it), from one look at them.
    fn refusal(&self, force: bool) -> Option<(Text, usize)> {
        // a server has no seat of its own, so the card rule never holds it: an arcade server's matches are its tunnel's
        // guests' (still connected even when the lobby lost the room) and this PC's seat beside it
        if self.mode.is_server()
            && let Some(g) = &self.arcade_guests
        {
            let (seat_on, guests) = (is_on(self.kit, Kind::Run), g.load(Ordering::Relaxed));
            return rules::arcade_rule(force, seat_on, guests).map(|t| (t, if seat_on { 0 } else { guests }));
        }
        let (slot, new) = self.card_files();
        let (session, _) = card::club_session(&slot, &new, self.run_started());
        let card_in = self.cabinet.board.is_some() && self.p.shared.card_in();
        rules::card_rule(self.alive(Role::Seat), force, Some(card_in), session).map(|t| (t, 0))
    }

    fn ended(&mut self, id: u32) -> Option<End> {
        let Some(i) = self.games.iter().position(|g| g.id == id) else {
            return None; // a program the run stopped itself (a cabinet restart)
        };
        let mut g = self.games.remove(i);
        if let Some(t) = g.thread.take() {
            let _ = t.join();
        }
        self.gone.push(g.role);
        if g.role == Role::Server {
            self.tell("", &Text::key("launcher.run_server_ended"));
            if self.edition == Edition::E1718 {
                crate::logs::append(&self.logs().join("run_launcher.txt"), &championship_line(&self.game));
            }
            if self.mode.is_server() {
                return Some(self.finish(false, Some(Text::key("launcher.run_server_has_ended"))));
            }
            return None;
        }
        let gone: Vec<&str> = self
            .gone
            .iter()
            .filter(|r| **r != Role::Server)
            .map(|r| r.log())
            .collect();
        let others = self.games.iter().any(|g| g.role != Role::Server && g.alive());
        let home = matches!(self.mode, Mode::Local | Mode::Vs(_));
        let seat_gone = g.role == Role::Seat && !self.alive(Role::Seat);
        let left = rules::windows_left(others, home, seat_gone);
        match rules::after_end(&gone, left, |n| self.read_log(n)) {
            Some(crashed) => {
                self.tell("", &rules::went_by_itself(&crashed, |n| self.read_log(n)));
                None
            }
            None => {
                self.tell(
                    "",
                    &Text::key(if left {
                        "launcher.run_closed_rest"
                    } else if others {
                        "launcher.run_projector_left"
                    } else {
                        "launcher.run_no_window_left"
                    }),
                );
                if let Some((why, _)) = self.refusal(false) {
                    // the projector closed during a match: seat 1 plays on, watched
                    self.tell("", &why);
                    self.tell("", &Text::key("launcher.run_not_stopped"));
                    return None;
                }
                Some(self.finish(true, None))
            }
        }
    }

    /// Ends every process of the run, in order: the helper threads and the Python kids, the game programs through their debuggers, then
    /// `Job::stop` for whatever is left (its windows closed, then ended after the deadline); then the stand-ins, the
    /// key driver first.
    pub(super) fn shutdown(&mut self) {
        self.sampler = None;
        self.helper = None; // joins its threads (a view being made is given up)
        for (_, c) in &mut self.kids {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.kids.clear();
        for g in &self.games {
            g.stop.store(true, Ordering::Relaxed);
        }
        for g in &mut self.games {
            if let Some(t) = g.thread.take() {
                let _ = t.join();
            }
        }
        self.games.clear();
        if self.job.active() > 0 {
            let s = self.job.stop(STOP_DEADLINE);
            if s.terminated > 0 {
                self.tell("  ", &Text::key("launcher.run_force_closed").arg("n", s.terminated));
            }
        }
        self.cabinet = Cabinet::default();
        self.scenes = None;
        self.gamesvr = None;
        // the club's last copy home, while the game is closed and the arcade tunnel still up
        self.club = None;
        self.vslink = None;
        self.lease = None;
        if self.tunnel_ports.is_some() {
            crate::notice::TUNNEL_DESK.store(0, Ordering::Relaxed);
        }
    }

    /// The end of the run: everything stopped, the table kept with its club, data\running.json removed, and - when a
    /// game window ended it (`ended`) - an online run that went wrong reported.
    fn finish(&mut self, ended: bool, words: Option<Text>) -> End {
        let gone: Vec<&str> = self
            .gone
            .iter()
            .filter(|r| **r != Role::Server)
            .map(|r| r.log())
            .collect();
        let why = if ended {
            rules::went_wrong(&gone, |n| self.read_log(n))
        } else {
            Vec::new()
        };
        let control_here = self.alive(Role::Server) || self.gone.contains(&Role::Server);
        self.shutdown();
        let notice = self.host_closed_notice();
        let why = if notice.is_some() {
            rules::without_link_lost(why)
        } else {
            why
        };
        let left = self.job.active();
        self.card_steps(&["keep"]); // the cards on the table kept with their club: data\save holds the latest
        self.job.stop(STOP_DEADLINE);
        let _ = fs::remove_file(self.running_file());
        if left > 0 {
            self.tell("", &Text::key("launcher.run_still_running").arg("n", left));
            return End::Over(FAILED);
        }
        if let Some(n) = &notice {
            self.tell("", n);
        }
        if self.edition == Edition::E1718 && control_here {
            let date = championship::date(now_local());
            let seen = self
                .uc_player
                .as_ref()
                .filter(|(d, ..)| *d == date)
                .map(|(_, c, _)| c.as_str());
            crate::logs::append(
                &self.logs().join("run_launcher.txt"),
                &championship::summary(&self.game, date, seen),
            );
        }
        let club_home = self
            .visit
            .club
            .as_ref()
            .map(|p| p.lock().unwrap_or_else(PoisonError::into_inner).home);
        if matches!(self.mode, Mode::Vs(_)) && club_home == Some(false) {
            // the marker stays, so the next start says so too
            self.tell("", &Text::key("launcher.run_vs_club_maybe_old"));
        }
        if let Some(words) = words {
            self.tell("", &words);
        } else if !why.is_empty() {
            self.tell("", &Text::key("launcher.run_ended_problem").arg("why", why.join("; ")));
        } else if notice.is_none() {
            self.tell("", &Text::key("launcher.run_closed_by_player"));
        }
        End::Over(if why.is_empty() { OK } else { WENT_WRONG })
    }

    /// RESTART NOW: the cabinet alone when a plain start would play the same way, else a stop (the card rule) and a
    /// plain start.
    fn restart(&mut self, force: bool) -> Option<End> {
        self.tell("", &Text::key("launcher.run_restart_requested"));
        let panel = choices::read(&self.kit.data().join("panel.txt"));
        let club = choices::setting(&self.kit.settings_file(), "club_server").unwrap_or_default();
        // a seat cannot reconnect to a server this run started that has since died: start everything again
        let server_died = self.gone.contains(&Role::Server);
        let fits = !server_died
            && rules::cabinet_restart_fits(
                &self.mode,
                self.alive(Role::Seat),
                &panel,
                self.kit.game_language(),
                (&club, self.vs_target.as_ref()),
            );
        if let Some((why, _)) = self.refusal(force) {
            self.tell("", &why);
            self.tell("", &Text::key("launcher.run_not_restarted"));
            return None;
        }
        if !fits {
            self.tell(
                "",
                &Text::key("launcher.run_restarting").arg("edition", self.edition.name()),
            );
            self.shutdown();
            self.card_steps(&["keep"]);
            let _ = fs::remove_file(self.running_file());
            thread::sleep(Duration::from_secs(2)); // Windows frees the ports and pipes
            return Some(End::Again);
        }
        // the projector, the scene service, the seat lease and the helper threads are never touched, so the projector
        // stays in step with the server
        self.tell("", &Text::key("launcher.run_restarting_seat").arg("n", self.seat_no));
        if let Some(i) = self.games.iter().position(|g| g.role == Role::Seat) {
            let mut g = self.games.remove(i);
            g.stop.store(true, Ordering::Relaxed);
            if let Some(t) = g.thread.take() {
                let _ = t.join();
            }
        }
        self.cabinet = Cabinet::default();
        self.card_steps(&["start", "board"]);
        thread::sleep(Duration::from_secs(1)); // Windows frees the cabinet's ports and pipes
        if let Err(f) = self.start_cabinet() {
            self.failed(f);
            return Some(match self.finish(false, Some(Text::key("launcher.run_stopped"))) {
                End::Over(_) | End::Again => End::Over(FAILED),
            });
        }
        self.write_running();
        self.say("");
        self.tell("", &Text::key("launcher.run_seat_back").arg("n", self.seat_no));
        None
    }

    /// A request from another Launcher.exe: answered in data\<kind>.answer ("CODE\nwords").
    fn answer(&mut self, word: &str) -> Option<End> {
        let file = answer_file(self.kit, Kind::of(&self.mode));
        let put = |code: i32, text: &str| {
            let _ = choices::write(&file, &format!("{code}\n{text}\n"));
        };
        match word {
            "status" => put(OK, &self.status()),
            "check" => {
                let (slot, new) = self.card_files();
                let (session, _) = card::club_session(&slot, &new, self.run_started());
                let n = self.job.active();
                put(
                    OK,
                    &format!(
                        "Check only, nothing stopped. Stop would end {n} processes (club card: {}, session: {})",
                        if self.p.shared.card_in() && self.cabinet.board.is_some() {
                            "in"
                        } else {
                            "out"
                        },
                        session.words()
                    ),
                );
            }
            "show" => {
                let ctl: Vec<procs::Proc> = self
                    .job_procs()
                    .into_iter()
                    .filter(|q| q.name.eq_ignore_ascii_case(self.p.control()))
                    .collect();
                if ctl.is_empty() {
                    put(FAILED, "No server is running on this PC.");
                } else {
                    let mut out: Vec<String> = ctl
                        .iter()
                        .map(|q| {
                            let titles = procs::show_windows(q.pid);
                            if titles.is_empty() {
                                format!("server pid {}: no hidden window to show", q.pid)
                            } else {
                                let t: Vec<String> = titles.iter().map(|t| format!("\"{t}\"")).collect();
                                format!("server pid {}: shown: {}", q.pid, t.join(", "))
                            }
                        })
                        .collect();
                    out.push("They are hidden again at the next start.".into());
                    put(OK, &out.join("\n"));
                }
            }
            "stop" | "force" => {
                // the answer in the player's language: the asking window or console shows it (run_launcher.txt here
                // keeps the English)
                let lang = self.ui.lang();
                if let Some((why, guests)) = self.refusal(word == "force") {
                    let not = Text::key("launcher.run_not_stopped");
                    self.tell("", &why);
                    self.tell("", &not);
                    let mut said = format!("{}\n{}", why.get(lang), not.get(lang));
                    // the asker's "Stop anyway?" names them (rules::stop_question)
                    if guests > 0 {
                        said.push_str(&format!("\n{}{guests}", rules::GUESTS_LINE));
                    }
                    put(REFUSED, &said);
                    return None;
                }
                self.tell("", &Text::key("launcher.run_stopping"));
                let stopped = Text::key("launcher.run_stopped");
                let end = self.finish(false, Some(stopped.clone()));
                put(
                    if matches!(end, End::Over(OK)) { OK } else { FAILED },
                    &stopped.get(lang),
                );
                return Some(end);
            }
            _ => put(FAILED, &format!("unknown request \"{word}\"")),
        }
        None
    }

    /// What runs (play.py's status).
    fn status(&self) -> String {
        let mut out = Vec::new();
        match &self.mode {
            Mode::Remote(ip, _) => out.push(format!("playing on the server at {ip} as seat {}", self.seat_no)),
            Mode::Server | Mode::LanServer => {
                out.push(format!("this PC is the server (TCP port {})", self.p.control_port))
            }
            Mode::Local => out.push("playing on this PC".into()),
            Mode::Vs(_) => out.push("playing on this PC, VS through the server".into()),
        }
        let threads = [
            ("scene service", self.scenes.is_some()),
            ("game server stand-in", self.gamesvr.is_some()),
            ("direct VS link", self.vslink.is_some()),
            ("keychip / network stand-in", !self.cabinet.net.is_empty()),
            (
                if self.edition == Edition::E1718 {
                    "Aime reader stand-in"
                } else {
                    "card reader stand-in"
                },
                self.cabinet.reader.is_some(),
            ),
            ("AiMeDB stand-in", self.cabinet.aimedb.is_some()),
            ("I/O board stand-in", self.cabinet.board.is_some()),
            ("key driver", self.cabinet.keys.is_some()),
            ("seat lease", self.lease.is_some()),
        ];
        out.extend(
            threads
                .iter()
                .filter(|(_, on)| *on)
                .map(|(n, _)| format!("  forza {n:<28} (in Launcher.exe)")),
        );
        for q in self.job_procs() {
            let kid = self.kids.iter().find(|(_, c)| c.id() == q.pid).map(|(r, _)| *r);
            let folder = q
                .path
                .parent()
                .and_then(Path::file_name)
                .map(|f| f.to_string_lossy().into_owned());
            out.push(match kid {
                Some(role) => format!("  forza {role:<28} pid {}", q.pid),
                None => format!("  game  {:<28} pid {}  ({})", q.name, q.pid, folder.unwrap_or_default()),
            });
        }
        if self.cabinet.on() {
            out.push(format!(
                "club card: {}",
                if self.p.shared.card_in() {
                    "IN the reader"
                } else {
                    "out"
                }
            ));
        }
        let (slot, new) = self.card_files();
        let (session, bad) = card::club_session(&slot, &new, self.run_started());
        out.push(format!(
            "card session: {}{}",
            session.words(),
            bad.map_or_else(String::new, |b| format!(", bad endings {b}"))
        ));
        out.join("\n")
    }
}

impl Drop for Run<'_> {
    fn drop(&mut self) {
        self.shutdown();
    }
}
