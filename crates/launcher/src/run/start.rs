//! The start: `start` and the `Run` steps that bring each program up.

use super::*;

/// Starts a run; `Err` is the exit code (what went wrong is said).
pub fn start<'a>(kit: &'a Kit, p: &'a Params, mode: Mode, debug: bool, ui: &'a dyn Ui) -> Result<Run<'a>, i32> {
    let t0 = Instant::now();
    if procs::prefers_system32() {
        tell(ui, "", &Text::key("launcher.run_unguarded"));
        return Err(UNGUARDED);
    }
    let Some(game) = game_folder(kit, p) else {
        tell(ui, "", &Text::key("launcher.run_not_set_up"));
        return Err(NOT_SET_UP);
    };
    let seat = seat_dir(&game);
    let edition = kit.edition();
    let arcade = rules::arcade_1718_on(&kit.settings());
    if rules::online_refused(edition, &mode, arcade) {
        refused(ui, &Text::key("launcher.run_online_1718"));
        return Err(REFUSED);
    }
    if edition == Edition::E1011 && !seat.join(".wccf-kit").is_file() {
        tell(ui, "", &Text::key("launcher.run_not_set_up"));
        return Err(NOT_SET_UP);
    }
    // an antivirus quarantining them leaves the game without its card table and panel; a server-only run needs neither
    let missing = kit.missing_programs();
    if !mode.is_server() && !missing.is_empty() {
        refused(
            ui,
            &Text::key("launcher.run_no_program").arg("files", missing.join(", ")),
        );
        return Err(REFUSED);
    }
    // held to the end of the start: see Admission
    let Some(_admitted) = admit(kit) else {
        refused(ui, &Text::key("launcher.run_busy"));
        return Err(REFUSED);
    };
    let check = || guard::check(&game, &seat, p.client(), p.control(), p.matcher());
    // a runtime an older setup made or the guard check refuses is set up again, once; the check after it decides
    let again = (edition == Edition::E1718)
        .then(|| crate::setup::why_again(kit, &game, check))
        .flatten();
    if let Some(why) = &again
        && !crate::setup::again(kit, &game, why, p.setup_1718, ui)
    {
        return Err(NOT_GUARDED);
    }
    let setup_done = t0.elapsed();
    // before anything of the run exists: no game program may start unguarded (docs/SAFETY.md); with no setup again the
    // check passed in why_again (2010-11 has its own: ensure_hook)
    if again.is_some()
        && let Err(why) = check()
    {
        tell(ui, "", &Text::key("launcher.run_not_guarded").arg("why", why));
        return Err(NOT_GUARDED);
    }
    let early = [setup_done, t0.elapsed()];
    // the panel's card browser only: never a reason not to start.  A play folder an older kit set up (copied without
    // overlay\catalogue.tsv) gets one made here, as SET UP makes it; the note says it is still missing.
    if !kit.cache().join("catalogue.tsv").is_file()
        && (edition != Edition::E1718 || crate::setup::catalogue_1718(kit.dir(), &game).is_err())
    {
        tell(ui, "  ", &Text::key("launcher.run_no_catalogue"));
    }
    // step 1's time starts here: the setup again and the catalogue build above are not steps of the run
    ui.staged();
    let kind = Kind::of(&mode);
    let beside_server = matches!(mode, Mode::Remote(..)) && is_on(kit, Kind::Server);
    let left = if beside_server { Vec::new() } else { strays(&game, p) };
    let busy = is_on(kit, Kind::Run) || (!beside_server && is_on(kit, Kind::Server)) || !left.is_empty();
    let lock = if busy { None } else { take_lock(kit, kind) };
    let Some(lock) = lock else {
        let mut names: Vec<&str> = left.iter().map(|q| q.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        let names = if names.is_empty() {
            Text::key("launcher.run_the_game")
        } else {
            Text::Raw(names.join(", "))
        };
        refused(ui, &Text::key("launcher.run_already").part("names", names));
        return Err(REFUSED);
    };
    let job = match Job::new() {
        Ok(j) => Arc::new(j),
        Err(e) => {
            ui.cause(&e.to_string()); // the status line says "Could not start" already
            let why = Text::key("launcher.run_no_job").arg("error", e);
            tell(ui, "", &Text::key("launcher.run_failed").part("why", why));
            return Err(FAILED);
        }
    };
    let (tx, rx) = channel();
    let mut run = Run {
        kit,
        p,
        ui,
        edition,
        game,
        seat,
        mode,
        seat_no: 1,
        debug,
        beside_server,
        own_store: false,
        tunnel_ports: None,
        arcade: None,
        sampler: None,
        job,
        tx,
        rx,
        games: Vec::new(),
        next_id: 0,
        gone: Vec::new(),
        kids: Vec::new(),
        helper: None,
        cabinet: Cabinet::default(),
        scenes: None,
        gamesvr: None,
        vslink: None,
        vs_target: None,
        lease: None,
        club: None,
        visit: Visit::default(),
        remote: None,
        arcade_room: None,
        arcade_guests: None,
        t0,
        early,
        uc_seen: None,
        uc_partial: None,
        uc_told: None,
        uc_player: None,
        projector: false,
        told_path: false,
        _lock: lock,
    };
    // requests left by a run that ended are never carried out
    for f in [
        request_file(kit, kind),
        answer_file(kit, kind),
        kit.data().join("savelogs.request"),
    ] {
        let _ = fs::remove_file(f);
    }
    if kind == Kind::Run {
        let _ = fs::remove_file(kit.data().join("restart.request"));
    }
    match run.start_all() {
        Ok(()) => Ok(run),
        Err(f) => {
            // an empty reason: the game is not set up (said already); the reason goes in run_launcher.txt too
            let quiet = f.is_quiet();
            if !quiet {
                run.failed(f);
            }
            run.shutdown();
            let _ = fs::remove_file(run.running_file());
            Err(if quiet { NOT_SET_UP } else { FAILED })
        }
    }
}

impl Run<'_> {
    pub(super) fn say(&self, line: &str) {
        self.ui.say(line);
        // and in data\logs\run_launcher.txt: the window's box and a console are gone when they close, and a console
        // Launcher.exe's lines do not reach a cmd redirect (first live run, 2026-10-13: the start's lines were lost)
        crate::logs::append(&self.logs().join("run_launcher.txt"), line);
    }

    /// `text` after `lead` ([`tell`]), and its English in data\logs\run_launcher.txt.
    pub(super) fn tell(&self, lead: &str, text: &Text) {
        self.say_pair(
            &format!("{lead}{}", text.get(self.ui.lang())),
            &format!("{lead}{}", text.en()),
        );
    }

    /// `startup: <stage> <ms>` in data\logs\run_launcher.txt: ms since the start began (PLAY).
    fn startup(&self, stage: &str, since: Duration) {
        crate::logs::append(
            &self.logs().join("run_launcher.txt"),
            &rules::startup_line(stage, since),
        );
    }

    /// A line as shown and in English (the log's).
    fn say_pair(&self, shown: &str, en: &str) {
        self.ui.say_logged(shown, en);
        crate::logs::append(&self.logs().join("run_launcher.txt"), en);
    }

    /// A start step's failure: said, then "Stopping.".
    pub(super) fn failed(&self, f: Failed) {
        self.ui.cause(&f.0.get(self.ui.lang()));
        self.tell("", &Text::key("launcher.run_failed").part("why", f.0));
        self.tell("", &Text::key("launcher.run_stopping"));
    }

    pub(super) fn logs(&self) -> PathBuf {
        self.kit.logs()
    }

    pub(super) fn running_file(&self) -> PathBuf {
        self.kit.data().join("running.json")
    }

    pub(super) fn card_files(&self) -> (PathBuf, PathBuf) {
        let save = self.kit.save();
        (save.join(card::SLOT), save.join(card::NEW))
    }

    /// Remote: the server's IPv4 address (once step 2 found it).
    fn ip(&self) -> Option<&str> {
        self.remote.as_ref().map(|(ip, _)| ip.as_str())
    }

    /// What every program of the run gets: where the game and the logs are, and where the panel finds its data.
    fn kit_env(&self) -> Vec<(String, String)> {
        let s = |p: PathBuf| p.display().to_string();
        let cache = self.kit.cache();
        vec![
            ("WCCF_GAME".into(), s(self.game.clone())),
            ("WCCF_LOGS".into(), s(self.logs())),
            ("MXHOOK_LOG_DIR".into(), s(self.logs().join("hook"))),
            ("WCCF_CATALOGUE".into(), s(cache.join("catalogue.tsv"))),
            ("WCCF_CARDS".into(), s(cache.join("cards"))),
        ]
    }

    /// `cmd` in the job, its output to data\logs\<log>; its exit code and output once it ends within `limit`.
    fn run_child(&self, cmd: Command, log: &str, limit: Duration) -> Result<(i32, String), String> {
        run_in_job(&self.job, cmd, &self.logs().join(log), limit)
    }

    /// The helper threads: the club card view (written again when the card changes) and the starting volumes of the projector
    /// (muted) and seat 1.  They never stop the run.
    fn start_helper(&mut self, projector: bool) {
        let say = file_log(self.logs().join("run_helper.txt"));
        let mut h = Helper::default();
        let (data, logs) = (self.kit.data(), self.logs());
        // 2017-18 keeps its clubs on the game server stand-in: no club card file to show
        if self.edition == Edition::E1011 {
            h.club_view(
                ViewFiles::new(&self.kit.data()),
                move || {
                    let (code, out) = forza::cards::steps::run(&data, &["view"]);
                    let _ = fs::write(logs.join("run_club_view.txt"), &out);
                    let last = out
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("-")
                        .trim()
                        .to_owned();
                    if code == 0 {
                        Ok(last.trim_start_matches("club card view written: ").to_owned())
                    } else {
                        Err(last)
                    }
                },
                say.clone(),
            );
        }
        let (logs, mut targets) = (self.logs(), Vec::new());
        if projector {
            let logs = logs.clone();
            targets.push(Target {
                name: "the projector",
                pid: Box::new(move || launched_pid(&logs, Role::Projector.log())),
                mute: true,
            });
        }
        targets.push(Target {
            name: "seat 1",
            pid: Box::new(move || launched_pid(&logs, Role::Seat.log())),
            mute: false,
        });
        h.start_volumes(targets, Helper::core_audio(), say);
        self.helper = Some(h);
    }

    /// The card steps the CLUB CARD panel asked for (`forza::cards::steps`: a manager transfer finished, a card switch, its
    /// bad endings cleared, the table following the club), done while the card reader is not running; never a reason
    /// not to start.
    pub(super) fn card_steps(&self, what: &[&str]) {
        if self.edition == Edition::E1718 {
            // its club is on the game server stand-in, its card is an Aime: no club card file; CLEAR BAD ENDINGS
            // (the panel's card_fix=bad_endings) clears the stand-in's counters instead
            let panel = self.kit.data().join("panel.txt");
            let steps = forza::cards::steps::read_panel(&panel);
            if what.contains(&"start") && steps.contains_key("card_fix") {
                let dir = self.kit.save().join("gamesvr");
                let done = if matches!(self.mode, Mode::Remote(..)) {
                    "the club is on another PC's server - clear its bad endings there".to_owned()
                } else if steps["card_fix"].eq_ignore_ascii_case("bad_endings") {
                    if settles_at_card_steps(&self.mode, self.club.is_some()) {
                        self.club_leftover(&dir);
                    }
                    club::clear_bad_endings(&dir).unwrap_or_else(|e| format!("bad endings not cleared ({e})"))
                } else {
                    String::new()
                };
                if !done.is_empty() {
                    self.say(&format!("  club: {done}"));
                }
                forza::cards::steps::drop_panel_line("card_fix", &panel);
            }
            return;
        }
        let seat = self.seat.display().to_string();
        let mut args = what.to_vec();
        args.push(&seat);
        let (_, out) = forza::cards::steps::run(&self.kit.data(), &args);
        let _ = fs::write(self.logs().join("run_card_steps.txt"), &out);
        out.lines().filter(|l| !l.trim().is_empty()).for_each(|l| self.say(l));
    }

    /// The 2010-11 network guard (docs/SAFETY.md), checked before any game program starts: the game and seat 1
    /// hold this kit's hook (wccfk.dll) and every program loads it (`guard::hook_1011`).  Kit 6 loads its hook as
    /// wccfk.dll; a game set up by an older kit, a newer kit unzipped over this one by hand, or a program put back by
    /// hand fails that - set up once, as Setup does, then check again: still not guarded, nothing starts.
    fn ensure_hook(&self) -> Result<(), Failed> {
        let p = self.p;
        let guarded = || -> Result<(), String> {
            let ours = fs::read(self.kit.programs().join("wccfk.dll")).ok();
            for dir in [&self.game, &self.seat] {
                if ours.is_none() || fs::read(dir.join("wccfk.dll")).ok() != ours {
                    return Err(format!(
                        "{} is not the Forza-WCCF hook",
                        dir.join("wccfk.dll").display()
                    ));
                }
            }
            if !p.hook_imports {
                return Ok(());
            }
            let mut exes: Vec<PathBuf> = p.programs.iter().map(|n| self.game.join(n)).collect();
            exes.push(self.seat.join(p.client()));
            guard::hook_1011(&exes)
        };
        if guarded().is_ok() {
            return Ok(());
        }
        self.tell("", &Text::key("launcher.run_updating_hook"));
        let log = file_log(self.logs().join("run_setup.txt"));
        let say = |shown: &str, en: &str| {
            log(en);
            self.say_pair(shown, en);
        };
        let ui = Lines(self.ui.lang(), &say);
        if crate::setup::folder_step(self.kit, &self.game.display().to_string(), false, &ui) != 0 {
            self.tell("", &Text::key("launcher.run_setup_unfinished"));
            return Err(Failed::quiet());
        }
        guarded().map_err(|why| {
            self.tell("", &Text::key("launcher.run_not_guarded").arg("why", why));
            Failed::quiet()
        })
    }

    /// `lang` ("en", "it", "" Japanese) into the game files, logged in run_english.txt: None when done, else why not.
    fn switch_text(&self, lang: &str) -> Option<String> {
        // forza::text swaps each file in one step, so even a stop half way leaves every file in place
        let log = file_log(self.logs().join("run_english.txt"));
        let last = Mutex::new(String::new());
        let say = |_: &str, l: &str| {
            log(l);
            if !l.trim().is_empty() {
                *last.lock().unwrap_or_else(PoisonError::into_inner) = l.trim().to_owned();
            }
        };
        match crate::setup::text_step(self.kit, lang, &Lines(self.ui.lang(), &say)) {
            0 => None,
            code => Some(
                Some(last.into_inner().unwrap_or_else(PoisonError::into_inner))
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| format!("exit {code}")),
            ),
        }
    }

    /// 2017-18: the original-text backup is kept beside the game; a Forza-WCCF folder with none of its own takes it.
    /// Text that is not original with no matching backup is said, not changed.
    fn adopt_text_backup(&self) {
        let backup = self.kit.data().join("english_backup");
        if forza::text::sync_beside(self.edition, &self.game, &backup, self.kit.dir()) == forza::text::Beside::Foreign {
            self.tell("  ", &Text::key("launcher.run_text_foreign"));
        }
    }

    /// English or Italian built from an older kit's sheets (a kit copied over the old one, no text switch since):
    /// built again from this kit's, in the same language, before anything starts.
    fn refresh_old_text(&self) {
        let lang = self.kit.game_language();
        let backup = self.kit.data().join("english_backup");
        if lang.is_empty() || !forza::text::stale(self.edition, &backup, self.kit.dir()) {
            return;
        }
        self.tell("  ", &Text::key("launcher.run_text_old"));
        match self.switch_text(lang) {
            Some(why) => self.tell("  ", &Text::key("launcher.run_text_not_built").arg("why", why)),
            None => self.tell("  ", &Text::key("launcher.run_text_current")),
        }
    }

    /// The SETTINGS panel's english=on|it|off: done now, before anything starts (the switch refuses while the game
    /// runs), then taken out of the file - a request, not a lasting setting.
    fn apply_english_request(&self) {
        let path = self.kit.data().join("panel.txt");
        let panel = choices::read(&path);
        let Some(want) = choices::panel_value(&panel, "english").map(|v| v.to_lowercase()) else {
            return;
        };
        if !["on", "it", "off"].contains(&want.as_str()) {
            return;
        }
        if let Some(lang) = rules::english_change(&panel, self.kit.game_language()) {
            let name = Text::key(match lang {
                "en" => "launcher.run_lang_en",
                "it" => "launcher.run_lang_it",
                _ => "launcher.run_lang_ja",
            });
            self.tell(
                "  ",
                &Text::key("launcher.run_text_switching").part("name", name.clone()),
            );
            match self.switch_text(lang) {
                Some(why) => self.tell("  ", &Text::key("launcher.run_text_not_switched").arg("why", why)),
                None => self.tell("  ", &Text::key("launcher.run_text_now").part("name", name)),
            }
        }
        // read again, and fallible: this text is written back whole, so a file that cannot be read now (a scanner
        // holding it) must not be saved over as if the request had been taken out of it
        let text = match choices::read_ok(&path) {
            Ok(text) => text,
            Err(e) => {
                self.tell(
                    "  ",
                    &Text::key("launcher.msg_read_failed")
                        .arg("path", path.display())
                        .arg("error", e),
                );
                return;
            }
        };
        let left = choices::set_panel_value(&text, "english", None);
        let _ = choices::write(&path, &left);
    }

    /// The run's `session:` line in run_launcher.txt, once it has started (`projector`: None on a server).
    fn session(&mut self, projector: Option<bool>) {
        let arcade = match &self.mode {
            Mode::Remote(entry, _) => Some(entry.as_str()),
            _ => self.arcade.as_deref(),
        };
        crate::logs::append(
            &self.logs().join("run_launcher.txt"),
            &rules::session_line(&self.mode, arcade, self.seat_no, projector),
        );
        let logs = self.logs();
        self.sampler.get_or_insert_with(|| sampler::spawn(logs));
    }

    pub(super) fn write_running(&self) {
        let t = logs::local(SystemTime::now());
        let started = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute(),
            t.second()
        );
        // a start stamp: the panel and the card rule read this file's write time as the run's start
        let info = serde_json::json!({"started": started, "mode": self.mode.word(), "server": self.ip(),
                                      "seat": self.seat_no});
        let _ = choices::write(&self.running_file(), &info.to_string());
    }

    pub(super) fn run_started(&self) -> Option<SystemTime> {
        fs::metadata(self.running_file()).and_then(|m| m.modified()).ok()
    }

    /// A game program under a debugger thread of its own (it creates the program, then pumps its events); its end
    /// comes as an event.
    fn start_game(
        &mut self,
        role: Role,
        args: Vec<String>,
        dir: &Path,
        hide: bool,
        env: Vec<(String, String)>,
    ) -> Result<(), Failed> {
        let (exe, prog, drole) = match role {
            Role::Server => (self.p.control(), "control", debug::Role::Control),
            _ => (self.p.client(), "client", debug::Role::Client),
        };
        let tag = if dir == self.game {
            String::new()
        } else {
            format!("_{}", dir.file_name().unwrap_or_default().to_string_lossy())
        };
        // 2017-18: the original programs take ASCII paths only; the guard checked the real folder this points at
        let from = if self.edition == Edition::E1718 {
            match crate::alias::for_dir(dir, &self.p.run_links) {
                Ok(link) => link,
                Err(e) => {
                    if !self.told_path {
                        self.told_path = true;
                        let line = if e.kind() == io::ErrorKind::AlreadyExists {
                            Text::key("launcher.run_path_taken").arg("folder", self.p.run_links.display())
                        } else {
                            Text::key("launcher.run_path_not_ascii")
                                .arg("folder", self.p.run_links.display())
                                .arg("error", &e)
                        };
                        self.tell("  ", &line);
                        crate::logs::append(
                            &self.logs().join("run_launcher.txt"),
                            &format!("no ASCII junction for {}: {e}", dir.display()),
                        );
                    }
                    None
                }
            }
        } else {
            None
        };
        // through a junction: the started process must be the program the guard checked (job::Job::spawn_debug)
        let checked = from.is_some().then(|| dir.join(exe));
        let dir = from.as_deref().unwrap_or(dir);
        let index = self.kit.dir().join(r"code\hardware\ghidra_out_1011_classes");
        let stop = Arc::new(AtomicBool::new(false));
        let opts = Options {
            role: drole,
            exe: dir.join(exe),
            args,
            timeout: LIFE,
            follow: true,
            hide,
            debug_heap: false,
            maps: self.p.maps.clone().map(|id| Maps {
                game: self.game.clone(),
                id,
                mosuppl: true,
                edition: self.edition,
            }),
            dbg_file: self.logs().join(format!("_dbg_{prog}{tag}.txt")),
            dumps: self.kit.data().join("dumps"),
            index: index.is_dir().then_some(index),
            // control+0x1226A is 2010-11's control: 2017-18's is guarded by its hook (whole messages)
            above_normal: self.edition == Edition::E1718,
            background: role == Role::Projector && self.edition == Edition::E1718,
            msgguard: (self.edition == Edition::E1011).then(|| env::var("WCCF_MSGGUARD").map_or(true, |v| v != "0")),
            env: self
                .kit_env()
                .into_iter()
                .chain(panel_role(self.edition, role))
                .chain(env)
                .chain([game_dpi()])
                .collect(),
            stop: Some(stop.clone()),
            hang_after: crate::debug::HANG_AFTER,
            checked,
        };
        let mut log = logs::Stamped::new(logs::session_log(&self.logs().join(role.log()))?);
        let (job, tx, id) = (self.job.clone(), self.tx.clone(), self.next_id);
        self.next_id += 1;
        let thread = thread::spawn(move || {
            if let Err(e) = debug::run(&opts, &job, &mut log) {
                let _ = writeln!(log, "the debugger could not go on: {e}");
            }
            let _ = tx.send(Event::Ended(id));
        });
        self.games.push(Game {
            id,
            role,
            stop,
            thread: Some(thread),
        });
        Ok(())
    }

    pub(super) fn alive(&self, role: Role) -> bool {
        self.games.iter().any(|g| g.role == role && g.alive())
    }

    /// The program's id from its debugger's "launched NAME pid N" line, once it is there.
    fn launched(&self, role: Role) -> Option<u32> {
        launched_pid(&self.logs(), role.log())
    }

    /// The job's processes now, with their names.
    pub(super) fn job_procs(&self) -> Vec<procs::Proc> {
        let pids = self.job.pids();
        procs::processes()
            .into_iter()
            .filter(|q| pids.contains(&q.pid))
            .collect()
    }

    fn job_count(&self, name: &str) -> usize {
        self.job_procs()
            .iter()
            .filter(|q| q.name.eq_ignore_ascii_case(name))
            .count()
    }

    /// 2010-11: the panel (forza_panel.dll) into the running game: "" when it loaded, else why not.
    fn inject(&self, pid: u32, dll: &Path) -> String {
        let mut cmd = Command::new(self.kit.program("inject.exe"));
        cmd.arg(pid.to_string()).arg(dll);
        match self.run_child(cmd, "run_inject.txt", Duration::from_secs(60)) {
            Ok((0, _)) => String::new(),
            Ok((code, out)) if out.trim().is_empty() => format!("exit {code}"),
            Ok((_, out)) => out.trim().to_owned(),
            Err(e) => e,
        }
    }

    /// 2017-18: the env that makes wccfnet.dll load the panel into this game process (seat 1, projector; never control).
    fn panel_env(&self) -> Vec<(String, String)> {
        if self.edition != Edition::E1718 {
            return Vec::new();
        }
        let mut env = vec![(
            "FORZA_PANEL".to_owned(),
            self.kit.program("forza_panel.dll").to_string_lossy().into_owned(),
        )];
        // a join's tunnel moved the control port: the panel's link watch looks for the seat's link on that one
        if let Some(t) = self.tunnel_ports.filter(|t| t.control != self.p.control_port) {
            env.push(("WCCFPANEL_CONTROL_PORT".into(), t.control.to_string()));
        }
        env
    }

    fn start_all(&mut self) -> Result<(), Failed> {
        let kit = self.kit;
        let p = self.p;
        if self.beside_server {
            fs::create_dir_all(self.logs().join("hook"))?; // its logs are open: no rotation
            let panel = choices::read(&kit.data().join("panel.txt"));
            if rules::english_change(&panel, kit.game_language()).is_some() {
                self.tell("  ", &Text::key("launcher.run_lang_skipped"));
            }
        } else {
            for line in logs::rotate(&self.logs())? {
                self.say(&line);
            }
            self.adopt_text_backup();
            self.apply_english_request();
            self.refresh_old_text();
            if self.edition == Edition::E1011 {
                // after the text switch, which puts programs back: 2017-18's guard is checked in `start`
                self.ensure_hook()?;
            }
        }
        self.startup("setup", self.early[0]);
        self.startup("guard", self.early[1]);
        // here the start is admitted (nothing else writes the pictures), unrefused, guarded and the folder repaired; a
        // server that does not answer is only found at step 2, after this wait. Not beside a server on this PC: its
        // game reads the folder. A failure never stops the start.
        if !self.beside_server && pictures_due(kit, p) {
            self.ui.updating_pictures();
            self.tell("", &Text::key("launcher.msg_updating_pictures"));
            if let Err(why) = pictures_held(kit, p, self.ui) {
                self.tell("  ", &why);
                self.tell("  ", &Text::key("launcher.run_pictures_as_is"));
            }
        }
        // the card reader is not running yet in any mode; nor FPR_Emu or the overlay, which read the table
        if self.mode.is_server() {
            self.card_steps(&["start"]);
        } else {
            self.card_steps(&["start", "board"]);
        }
        let steps = if self.mode.is_server() { 2 } else { 6 };
        let confs = [
            self.game.join(r"local\client_user_option.conf"),
            self.seat.join(r"local\client_user_option.conf"),
        ];
        if self.edition == Edition::E1718 {
            // shortcut: a guest at another PC's arcade shows the default, not that arcade's name; upgrade when the desk
            // tells its guests the name
            let name = match self.mode {
                Mode::Remote(..) => String::new(),
                _ => choices::setting(&self.kit.settings_file(), "shop_name").unwrap_or_default(),
            };
            for c in confs.iter().filter(|c| c.is_file()) {
                // a label only: a conf that cannot be written is told and the start goes on
                if let Err(e) = conf::set_place(c, &name) {
                    self.tell(
                        "  ",
                        &Text::key("launcher.run_place_failed")
                            .arg("file", c.display())
                            .arg("error", e),
                    );
                }
            }
        }
        let starting = match &self.mode {
            Mode::Server | Mode::LanServer => Text::key("launcher.run_starting_server"),
            Mode::Remote(ip, _) => Text::key("launcher.run_starting_remote").arg("ip", ip),
            Mode::Vs(_) => Text::key("launcher.run_starting_vs"),
            Mode::Local => Text::key("launcher.run_starting"),
        };
        self.tell(
            "",
            &starting
                .arg("edition", self.edition.name())
                .arg("folder", self.game.display()),
        );
        if self.edition == Edition::E1718 {
            self.start_gamesvr(steps)?;
        } else if self.beside_server {
            self.tell(&format!("  1/{steps} "), &Text::key("launcher.run_scenes_shared"));
        } else {
            let xscn = self.game.join(r"data\motion\launcherMotion\xscn");
            let log = file_log(self.logs().join("xscn_watch_log.txt"));
            let h = scenes::spawn(&xscn, &p.scenes, log)
                .map_err(|e| Failed(Text::key("launcher.run_scenes_failed").arg("error", e)))?;
            self.scenes = Some(h);
            self.tell(&format!("  1/{steps} "), &Text::key("launcher.run_scenes"));
        }

        let mut projector = true;
        if let Mode::Remote(entry, want) = self.mode.clone() {
            // the entry as written: a 2017-18 arcade's id (its tunnel serves this PC's loopback), or a name or an IPv4
            // address, ":port" the seat desk's (the lobby's entry)
            let arcade = vslink::arcade::parse_id(&entry).filter(|_| self.edition == Edition::E1718);
            let (ip, desk) = match arcade {
                Some(id) => {
                    let ports = self.start_arcade_tunnel(vslink::arcade::Role::Join(id))?;
                    self.tunnel_ports = Some(ports);
                    // the window asks the host's desk for its notice while the run lasts
                    crate::notice::TUNNEL_DESK.store(ports.desk, Ordering::Relaxed);
                    ("127.0.0.1".to_owned(), ports.desk)
                }
                None => choices::resolve(&entry, p.seat_port)
                    .map(|a| (a.ip().to_string(), a.port()))
                    .ok_or_else(|| Failed(Text::key("launcher.run_not_found").arg("entry", &entry)))?,
            };
            // through the tunnel the arcade is at 127.0.0.1 too, but its game server is the host's
            self.own_store = rules::own_store(&ip, arcade.is_some());
            // this PC's own server by its LAN address: loopback, where a 2017-18 arcade's desk and relay listen
            let ip = if self.own_store { "127.0.0.1".to_owned() } else { ip };
            self.remote = Some((ip.clone(), desk));
            let control_port = self.tunnel_ports.map_or(p.control_port, |t| t.control);
            if !answers(&ip, control_port) {
                return Err(Failed(
                    Text::key("launcher.run_remote_no_answer")
                        .arg("entry", &entry)
                        .arg("port", control_port),
                ));
            }
            let (seat, here) = self.take_seat(&ip, want)?;
            (self.seat_no, projector) = (seat, here);
            for c in &confs {
                conf::set_control_ip(c, &ip)?;
                if self.edition == Edition::E1718 {
                    // the tunnel's control port when this PC holds 20002 itself
                    conf::set_conf_value(c, "CONTROL_PORT", &control_port.to_string())?;
                }
            }
            conf::set_satellite_no(&confs[1], u32::from(seat))?; // the cabinet's seat
            if self.edition == Edition::E1718 && !self.own_store {
                self.club = None; // a trip of an earlier start comes home first
                self.visit.club = None; // and its progress is not this start's
                if let Err(why) = self.bring_club(addr(&ip, desk), seat, &entry) {
                    self.tell(
                        "  ",
                        &Text::key("launcher.run_with_why")
                            .part("text", Text::key("launcher.club_not_in"))
                            .part("why", why),
                    );
                }
            }
            // never "WAITING FOR THE PROJECTOR" on a server: its projector is on another PC, or gone
            conf::set_conf_value(&confs[1], "WAIT_PROJECTOR", "0")?;
            self.tell(
                &format!("  2/{steps} "),
                &Text::key(if here {
                    "launcher.run_remote_seat_projector"
                } else {
                    "launcher.run_remote_seat"
                })
                .arg("ip", &ip)
                .arg("port", control_port)
                .arg("seat", seat),
            );
        } else {
            let ctrl_conf = self.game.join(r"local\ctrl_user_option.conf");
            if matches!(self.mode, Mode::Local | Mode::Vs(_)) {
                // 2017-18: the projector program runs for its window or for the CPU championship it plays (hidden when
                // only the championship wants it); with both off, none, and nobody waits for it
                let s = self.kit.settings();
                projector =
                    self.edition != Edition::E1718 || choices::projector_shown(&s) || choices::cpu_championship_on(&s);
                let wait = if projector { "1" } else { "0" };
                for c in &confs {
                    conf::set_control_ip(c, "127.0.0.1")?; // after a "remote" run: the cabinets play on this PC again
                    if self.edition == Edition::E1718 {
                        conf::set_conf_value(c, "CONTROL_PORT", &p.control_port.to_string())?;
                    }
                }
                conf::set_satellite_no(&confs[1], 1)?;
                conf::set_conf_value(&confs[1], "WAIT_PROJECTOR", wait)?; // the game's own way, its projector here
                conf::set_conf_value(&ctrl_conf, "WAIT_PROJECTOR", wait)?;
            } else if self.edition == Edition::E1718 {
                // a server for others: its seat desk gives the first PC the projector, which control waits for
                conf::set_conf_value(&ctrl_conf, "WAIT_PROJECTOR", "1")?;
            } else {
                // a server for others: no projector waited for (the seat desk gives no PC the projector)
                conf::set_conf_value(&ctrl_conf, "WAIT_PROJECTOR", "0")?;
            }
            let game = self.game.clone();
            if self.edition == Edition::E1718 {
                // the control saves the Universal Championship in log\<date>_uc.bin but makes no folder (0x41EF00)
                fs::create_dir_all(game.join("log"))?;
            }
            // the CPU championship off: the control's hook keeps an all-CPU Universal Championship off the projector
            // (native/hook/src/uclive.rs); the projector then plays the control's own demo pair instead
            let env = if self.edition == Edition::E1718 && !choices::cpu_championship_on(&self.kit.settings()) {
                vec![("WCCF_UCLIVE".to_owned(), "on".to_owned())]
            } else {
                Vec::new()
            };
            self.start_game(Role::Server, p.control_args.clone(), &game, !self.debug, env)?;
            let said = std::cell::Cell::new(false);
            wait_for(
                Text::key("launcher.run_part_server"),
                90,
                Role::Server.log(),
                || self.alive(Role::Server),
                || match procs::listening_ports_result() {
                    Ok(ports) => ports.contains(&p.control_port) && self.job_count(p.matcher()) >= p.match_engines,
                    // the table could not be read: that is not "the server is not up", and the job's programs are a
                    // weaker but honest sign; said once, so the log says why the wait is longer than usual
                    Err(e) => {
                        if !said.replace(true) {
                            self.tell("  ", &Text::key("launcher.run_ports_unread").arg("error", &e));
                        }
                        self.job_count(p.matcher()) >= p.match_engines
                    }
                },
            )?;
            if let Some(ctl) = self
                .job_procs()
                .into_iter()
                .find(|q| q.name.eq_ignore_ascii_case(p.control()))
            {
                // a click in the server's console would freeze it (text-selection mode): switch that off, in a child of
                // Launcher.exe (it changes the console of the process that does it)
                let exe = self.kit.dir().join(Kit::exe_name());
                if exe.is_file() {
                    let mut cmd = Command::new(exe);
                    cmd.arg(helper::console::ARG).arg(ctl.pid.to_string());
                    let _ = self.run_child(cmd, "run_console.txt", Duration::from_secs(15));
                }
                if !self.debug {
                    procs::hide_windows(ctl.pid); // it starts hidden; a window it shows by itself is hidden here
                }
            }
            self.tell(
                &format!("  2/{steps} "),
                &Text::key("launcher.run_server_up")
                    .arg("port", p.control_port)
                    .arg("n", self.job_count(p.matcher())),
            );
        }
        self.startup("engines", self.t0.elapsed());
        if self.mode.is_server() {
            return self.start_server_side();
        }
        self.projector = projector;

        if projector {
            let args = p.projector_args.clone().unwrap_or_else(|| vec!["0".into()]);
            let game = self.game.clone();
            self.start_game(Role::Projector, args, &game, false, self.panel_env())?;
            let hidden = self.edition == Edition::E1718 && !choices::projector_shown(&self.kit.settings());
            self.tell(
                "  3/6 ",
                &Text::key(if hidden {
                    "launcher.run_projector_hidden"
                } else {
                    "launcher.run_projector"
                }),
            );
        } else {
            self.tell(
                "  3/6 ",
                &Text::key(match self.edition {
                    Edition::E1011 => "launcher.run_no_projector_1011",
                    Edition::E1718 if self.remote.is_none() => "launcher.run_projector_off",
                    Edition::E1718 => "launcher.run_no_projector_1718",
                }),
            );
        }
        // 2017-18 shows seat 1's window only at its start warning (#229), ~20 s after the projector plays: mute it now
        let helper_first = self.edition == Edition::E1718;
        if helper_first {
            self.start_helper(projector);
        }
        self.start_cabinet()?;
        if projector {
            // the overlay in its money-only mode: the projector's amounts with commas; it plays on without it.
            // 2017-18: the game loads it itself (wccfnet.dll, from FORZA_PANEL)
            if self.edition == Edition::E1718 {
                let panel = self.kit.program("forza_panel.dll").is_file();
                self.tell("       ", &money_line(panel));
            } else {
                let panel = self.kit.program("forza_panel.dll");
                let why = self.launched(Role::Projector).map_or_else(
                    || Text::key("launcher.run_no_process"),
                    |pid| Text::Raw(self.inject(pid, &panel)),
                );
                self.tell(
                    "       ",
                    &if why == Text::Raw(String::new()) {
                        Text::key("launcher.run_money_on")
                    } else {
                        Text::key("launcher.run_money_off").part("why", why)
                    },
                );
            }
        }
        if !helper_first {
            self.start_helper(projector);
        }
        self.write_running();
        self.session(Some(projector));
        self.say("");
        if let Mode::Remote(ip, _) = &self.mode {
            let playing = if projector {
                Text::key("launcher.run_playing_remote")
            } else {
                Text::key("launcher.run_playing_remote_seat").arg("seat", self.seat_no)
            };
            self.tell("", &playing.arg("ip", ip));
        }
        self.tell("", &Text::key("launcher.run_running"));
        self.tell("  ", &Text::key("launcher.run_keys1"));
        self.tell("  ", &Text::key("launcher.run_keys2"));
        self.tell(
            "",
            &Text::key(match self.edition {
                Edition::E1011 => "launcher.run_card_1011",
                Edition::E1718 => "launcher.run_card_1718",
            }),
        );
        self.tell("", &Text::key("launcher.run_to_quit"));
        Ok(())
    }

    /// "server": `forza-server all` (2017-18: `Launcher.exe server all`; the seat desk and the match relay in one Job child, each with its
    /// own log in data\logs), then the run waits for a stop.
    fn start_server_side(&mut self) -> Result<(), Failed> {
        let logs = self.logs();
        let mut env = self.kit_env();
        env.extend([
            ("WCCF_BROKER_PORT".to_owned(), self.p.seat_port.to_string()),
            ("WCCF_RELAY_PORT".to_owned(), self.p.relay_port.to_string()),
        ]);
        // 2017-18: guests come through the arcade tunnel, so the desk and relay listen on loopback only (no firewall
        // prompt, nothing open on the LAN) unless data\settings.json "shop_lan": true asks for the plain-address path
        // `server lan` (2017-18): this network only - no tunnel, no lobby room, LAN searches answered
        let lan_only = self.edition == Edition::E1718 && self.mode == Mode::LanServer;
        // "shop_lan": settings key kept from before the arcade rename
        let lan = lan_only || self.edition == Edition::E1011 || self.kit.settings()["shop_lan"].as_bool() == Some(true);
        if !lan {
            env.push(("WCCF_BIND".to_owned(), "127.0.0.1".to_owned()));
        }
        if self.edition == Edition::E1718 && !lan_only {
            // every tunnel guest reaches the desk as 127.0.0.1: no per-address limits there
            env.push(("WCCF_DESK_TUNNEL".to_owned(), "1".to_owned()));
        }
        // the arcade's name this runtime set; empty: the desk's and the LAN answer's defaults
        let name = forza::online::lobby::arcade_name(
            &choices::setting(&self.kit.settings_file(), "shop_name").unwrap_or_default(),
        );
        if self.edition == Edition::E1718 {
            // guests bring their own clubs into this PC's store (online::club), on the tunnel and on the LAN
            let db = self.kit.save().join("gamesvr").join("gamesvr.db");
            env.push(("WCCF_CLUB_DB".to_owned(), db.display().to_string()));
            // the ARCADES row for this desk's own arcade (a private room is never listed)
            if !name.is_empty() {
                env.push(("WCCF_SHOP_NAME".to_owned(), name.clone()));
            }
            // HOST ARCADE: the host's notice for its tunnel guests (#328), written to its own file here; HOST LAN ARCADE
            // has no box: its desk serves this PC's copy of the project's notice, never written here (desk_notice_file)
            let file = desk_notice_file(&self.kit.data(), lan_only);
            let written = if lan_only {
                Ok(true)
            } else {
                let text = choices::setting(&self.kit.settings_file(), "arcade_notice").unwrap_or_default();
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                crate::notice::write_host_notice(&file, &text, now)
            };
            if let Err(e) = &written {
                let line = format!("notice: {} not written ({e}), no arcade notice", file.display());
                crate::logs::append(&self.logs().join("run_launcher.txt"), &line);
                // a last session's notice must not stay up under its old id
                let _ = std::fs::remove_file(&file);
            }
            // HOST ARCADE only (HOST LAN ARCADE writes nothing): a file still there after a failed write and remove is
            // stale, so the desk keeps its default notice file
            if written.is_ok() || !file.exists() {
                env.push(("WCCF_NOTICE".to_owned(), file.display().to_string()));
            }
        }
        // 2017-18 ships no forza-server.exe (an AV false positive): the kit's Launcher.exe runs it as `server ...`
        let mut cmd = if self.edition == Edition::E1718 {
            let mut c = Command::new(self.kit.dir().join("Launcher.exe"));
            c.arg("server");
            c
        } else {
            Command::new(self.kit.program("forza-server.exe"))
        };
        cmd.args(["--edition", self.edition.name(), "all"]);
        if self.edition == Edition::E1718 {
            // this PC's game server is the run's own stand-in (start_gamesvr)
            cmd.arg("--no-gmsvr");
        }
        if lan_only {
            // the name guests see in their list: the arcade's, as on its ARCADES row, else this PC's
            cmd.arg("--lan").arg(lan_name(name));
        }
        cmd.arg((LIFE.as_secs() + 60).to_string()).arg(&logs);
        clean_env(&mut cmd, &logs, &env);
        let out = "run_forza_server.txt";
        let role = "online server";
        let started = File::create(logs.join(out)).and_then(|f| {
            cmd.stderr(f.try_clone()?).stdout(f);
            self.job.spawn(&mut cmd)
        });
        match started {
            Ok(child) => self.kids.push((role, child)),
            Err(e) => self.tell("     ", &Text::key("launcher.run_server_not_started").arg("error", e)),
        }
        thread::sleep(Duration::from_millis(1500));
        let alive = self
            .kids
            .last_mut()
            .is_some_and(|(r, c)| *r == role && c.try_wait().is_ok_and(|s| s.is_none()));
        if alive {
            let desk = match self.edition {
                Edition::E1011 => "launcher.run_desk_1011",
                Edition::E1718 => "launcher.run_desk_1718",
            };
            self.tell("     ", &Text::key(desk).arg("port", self.p.seat_port));
            self.tell("     ", &Text::key("launcher.run_relay").arg("port", self.p.relay_port));
            if lan_only {
                let ips: Vec<String> = forza::online::lan::addresses()
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                let ips = if ips.is_empty() {
                    Text::key("launcher.run_none_found")
                } else {
                    Text::Raw(ips.join(", "))
                };
                self.tell(
                    "     ",
                    &Text::key("launcher.run_lan_arcade")
                        .arg("port", forza::online::lan::PORT)
                        .part("ips", ips),
                );
            }
        } else {
            self.tell("     ", &Text::key("launcher.run_server_ended_early").arg("log", out));
        }
        if self.edition == Edition::E1718 && !lan_only {
            // guests reach this arcade through iroh: no inbound port; the plain address stays the LAN fallback
            if let Err(Failed(why)) = self.start_arcade_tunnel(vslink::arcade::Role::Host) {
                let off = Text::key("launcher.run_tunnel_off").part("why", why);
                self.tell("     ", &off);
                self.arcade_room = Some(Err(off.get(self.ui.lang())));
                self.tell(
                    "     ",
                    &Text::key(if lan {
                        "launcher.run_lan_still"
                    } else {
                        "launcher.run_lan_only_flag"
                    }),
                );
            }
        }
        self.write_running();
        self.session(None);
        self.say("");
        self.tell(
            "",
            &Text::key(if lan_only {
                "launcher.run_server_lan"
            } else if self.edition == Edition::E1718 && !lan {
                "launcher.run_server_tunnel"
            } else {
                "launcher.run_server_plain"
            }),
        );
        self.tell(
            "",
            &if self.edition == Edition::E1718 && !lan {
                Text::key("launcher.run_local_ports")
            } else {
                Text::key("launcher.run_open_ports")
                    .arg("control", self.p.control_port)
                    .arg("desk", self.p.seat_port)
                    .arg("relay", self.p.relay_port)
            },
        );
        Ok(())
    }

    /// Steps 4-6 and the key driver: seat N's own side - the keychip and network, card reader and I/O board stand-ins,
    /// the cabinet (the seat1 folder) with its overlay.  A cabinet restart runs only this.
    pub(super) fn start_cabinet(&mut self) -> Result<(), Failed> {
        let p = self.p;
        let net_log = file_log(self.logs().join("ringedge_log.txt"));
        for (&(_, name, kind), &port) in network::SERVICES.iter().zip(&p.network_ports) {
            let listener = TcpListener::bind(("127.0.0.1", port))
                .map_err(|e| Failed(Text::key("launcher.run_net_ports").arg("port", port).arg("error", e)))?;
            self.cabinet.net.push(network::spawn(
                listener,
                kind,
                name,
                self.kit.edition().game_id(),
                net_log.clone(),
            )?);
        }
        self.cabinet.extra = std::mem::take(&mut *p.extra_watch.lock().unwrap_or_else(PoisonError::into_inner));
        let coms = if self.edition == Edition::E1718 {
            self.start_aime()?;
            format!("COM2={}", p.aime_pipe)
        } else {
            let (slot, new) = self.card_files();
            // the card reader logs every byte: only with "debug" (~60 MB an hour)
            let icc_log: Log = if self.debug {
                file_log(self.logs().join("icc_log.txt"))
            } else {
                Arc::new(|_: &str| {})
            };
            self.cabinet.reader = Some(
                card_reader::spawn(&p.icc_pipe, Some(&slot), Some(&new), icc_log)
                    .map_err(|e| Failed(Text::key("launcher.run_reader_failed").arg("error", e)))?,
            );
            format!("COM3={0};COM1={0}", p.icc_pipe)
        };
        let jvs_log = file_log(self.logs().join("jvs_log.txt"));
        self.cabinet.board = Some(
            io_board::spawn(&p.jvs_pipe, p.shared.clone(), jvs_log)
                .map_err(|e| Failed(Text::key("launcher.run_board_failed").arg("error", e)))?,
        );
        self.tell(
            "  4/6 ",
            &Text::key(match self.edition {
                Edition::E1011 => "launcher.run_cabinet_1011",
                Edition::E1718 => "launcher.run_cabinet_1718",
            }),
        );

        let mut env = self.panel_env();
        env.extend([
            ("MXHOOK_RINGEDGE".to_owned(), "1".to_owned()),
            ("MXHOOK_COM".into(), format!("{coms};mxjvs={0};COM4={0}", p.jvs_pipe)),
        ]);
        if let Some(ip) = self.ip() {
            // a match against another player goes through the server's relay (forza_panel.dll)
            let relay = self.tunnel_ports.map_or(p.relay_port, |t| t.relay);
            env.push(("WCCF_RELAY".into(), format!("{ip}:{relay}")));
            env.push(("WCCF_SEAT".into(), self.seat_no.to_string()));
        }
        if self.debug {
            env.push(("MXHOOK_VERBOSE".into(), "1".into()));
        }
        let n = self.seat_no;
        let args = p.seat_args.clone().unwrap_or_else(|| vec![n.to_string()]);
        let seat = self.seat.clone();
        // before the start: the 2017-18 window is held hidden until the start warning, long after "first frame shown"
        let olog = seat.join("forza_panel.log");
        let skip = fs::metadata(&olog).map_or(0, |m| m.len() as usize);
        self.start_game(Role::Seat, args, &seat, false, env)?;
        let mut pid = None;
        wait_for(
            Text::key("launcher.run_part_seat").arg("n", n),
            30,
            Role::Seat.log(),
            || self.alive(Role::Seat),
            || {
                pid = self.launched(Role::Seat);
                pid.is_some()
            },
        )?;
        let pid = pid.unwrap_or_default();
        wait_for(
            Text::key("launcher.run_part_seat_window").arg("n", n),
            p.window_wait.as_secs(),
            Role::Seat.log(),
            || self.alive(Role::Seat),
            || procs::has_window(pid),
        )?;
        self.tell("  5/6 ", &Text::key("launcher.run_window_up").arg("n", n));
        self.startup("window", self.t0.elapsed());
        // 2017-18: wccfnet.dll loads the panel 3 s after the window (FORZA_PANEL); 2010-11: inject.exe does
        let panel = self.kit.program("forza_panel.dll");
        let why = if self.edition == Edition::E1718 {
            if panel.is_file() {
                Text::Raw(String::new())
            } else {
                Text::key("launcher.run_panel_missing")
            }
        } else {
            thread::sleep(Duration::from_secs(3));
            Text::Raw(self.inject(pid, &panel))
        };
        if why == Text::Raw(String::new()) {
            let up = wait_for(
                Text::key("launcher.run_part_overlay"),
                60,
                "",
                || true,
                || {
                    fs::read(&olog)
                        .is_ok_and(|t| String::from_utf8_lossy(t.get(skip..).unwrap_or_default()).contains(PANEL_UP))
                },
            );
            self.tell(
                "  6/6 ",
                &Text::key(if up.is_ok() {
                    "launcher.run_panels"
                } else {
                    "launcher.run_panels_unconfirmed"
                }),
            );
        } else {
            // the game runs without the overlay - never a reason to take the game away
            self.tell("  6/6 ", &Text::key("launcher.run_panels_failed").part("why", why));
        }
        let keys_path = env::var_os("WCCF_KEYS").map_or_else(|| self.kit.data().join("keys.txt"), PathBuf::from);
        self.cabinet.keys = Some(crate::keys::spawn(
            p.shared.clone(),
            keys_path,
            // 2017-18's panel SETTINGS has CONTROLLER IN THE BACKGROUND (data\panel.txt); 2010-11 never sets it
            (self.kit.edition() == Edition::E1718).then(|| self.kit.data().join("panel.txt")),
            // START puts in a coin only where the game is not in FREE PLAY: 2017-18 runs it from the panel (seen live, run 8)
            self.kit.edition() != Edition::E1718,
            file_log(self.logs().join("run_keys.txt")),
        ));
        Ok(())
    }

    /// 2017-18's card: the Aime reader on its pipe (the game opens COM2) and AiMeDB, which gives the card its Aime id.
    fn start_aime(&mut self) -> Result<(), Failed> {
        let (p, save) = (self.p, self.kit.save());
        fs::create_dir_all(&save)?;
        let log = file_log(self.logs().join("aime_log.txt"));
        self.cabinet.reader = Some(
            aime::spawn(&p.aime_pipe, Some(&save.join("seat1_aime.txt")), log)
                .map_err(|e| Failed(Text::key("launcher.run_aime_failed").arg("error", e)))?,
        );
        let listener = TcpListener::bind(("127.0.0.1", p.aimedb_port)).map_err(|e| {
            Failed(
                Text::key("launcher.run_aimedb_port")
                    .arg("port", p.aimedb_port)
                    .arg("error", e),
            )
        })?;
        let log = file_log(self.logs().join("aimedb_log.txt"));
        self.cabinet.aimedb = Some(
            // on another PC's game server every card answers the hash of its access code; this PC alone and a seat on
            // this PC's own server keep their store's IDs (aimedb.rs)
            aimedb::spawn_with(
                listener,
                save.join("aimedb.json"),
                log,
                rules::aime_hashed(&self.mode, self.own_store),
            )
            .map_err(|e| Failed(Text::key("launcher.run_aimedb_failed").arg("error", e)))?,
        );
        Ok(())
    }

    /// Step 1 for 2017-18: no scene service; control keeps the clubs on a game server. This PC: the stand-in beside it
    /// (ctrl_user_option.conf GAME_SVR_IP=localhost). Online ([`Mode::Vs`]): the server's game server with
    /// GAME_SVR_ENTRY=1 (the control's VS entries, 243-246), the lobby's VS room claimed at its desk, the direct VS
    /// link. A cabinet on a server uses that server's.
    fn start_gamesvr(&mut self, steps: u32) -> Result<(), Failed> {
        // a new run: nothing of the last one until this one picks its cup
        remember_cup(None);
        // a guest's run (Remote) and an online one that brings the club (Vs) settle a club left out at the bring, after a
        // re-join takes the arcade's newer copy (#437)
        if !matches!(self.mode, Mode::Remote(..) | Mode::Vs(_)) {
            self.club_leftover(&self.kit.save().join("gamesvr"));
        }
        if matches!(self.mode, Mode::Remote(..)) {
            self.tell(&format!("  1/{steps} "), &Text::key("launcher.run_gamesvr_remote"));
            return Ok(());
        }
        let port = self.p.gamesvr_port;
        let ctrl_conf = self.game.join(r"local\ctrl_user_option.conf");
        // the Universal Cup: the host dialog's choice, which the control reads only as it starts (a new choice needs a
        // new start; local play's PLAY tab writes the same setting). A cup at a time is a schedule slot
        // (local\schedule_option.conf); VS keeps the game's times.
        let plan = rules::cup_plan(&self.mode, &self.kit.settings(), forza_ui::lobby::local_clock());
        // the window's PLAY tab shows this run's result while it runs, not a new one every frame
        remember_cup(Some(plan));
        let ((times, friendlies), line) = match plan {
            Ok(p) => (
                conf::cup_files(p),
                match p {
                    CupPlan::Off => Text::key("launcher.run_cup_off"),
                    CupPlan::GameTimes => Text::key("launcher.run_cup_game_times"),
                    CupPlan::Slot { opens, .. } => {
                        Text::key("launcher.run_cup_at").arg("time", forza_ui::lobby::clock_text(opens))
                    }
                },
            ),
            Err(r) => (
                (None, None),
                Text::key("launcher.run_cup_refused").part("why", Text::key(r.key()).arg("m", r.minutes())),
            ),
        };
        conf::set_cup_times(&ctrl_conf, times)?;
        conf::set_cup_schedule(&self.game.join(r"local\schedule_option.conf"), friendlies)?;
        self.tell("  ", &line);
        // for live checks: when the control should open the Universal Championship, the cup's winner in it (a refused
        // time leaves the game's times)
        let plan_line = match plan.unwrap_or(CupPlan::GameTimes).championship() {
            Some(t) => format!(
                "championship plan: from about {}, the slot after the cup's final, {} matches",
                forza_ui::lobby::clock_text(t),
                forza_ui::lobby::CHAMPIONSHIP_SLOTS
            ),
            None => "championship plan: none, no cup".to_owned(),
        };
        crate::logs::append(&self.logs().join("run_launcher.txt"), &plan_line);
        if let Mode::Vs(_) = self.mode {
            let panel_file = self.kit.data().join("panel.txt");
            let panel = choices::read_ok(&panel_file).map_err(|e| {
                Failed(
                    Text::key("launcher.msg_read_failed")
                        .arg("path", panel_file.display())
                        .arg("error", e),
                )
            })?;
            let club = choices::setting(&self.kit.settings_file(), "club_server").unwrap_or_default();
            let t = rules::vs_target(&panel, &club).map_err(Failed)?;
            self.vs_target = Some(t.clone());
            // Note: only "localhost" is proven as GAME_SVR_IP (a literal 127.0.0.1 means the publisher's address);
            // a numeric IP or a hostname needs the live test.
            let found = reach(&t.host, t.port).ok_or_else(|| {
                Failed(
                    Text::key("launcher.run_gamesvr_no_answer")
                        .arg("host", &t.host)
                        .arg("port", t.port),
                )
            })?;
            // the server's own game server (a room's too) keeps no club of this PC's card: it comes along, as to an
            // arcade, or the game does not start. A club server keeps the club itself.
            if t.room.is_some() || club.trim().is_empty() {
                let server = choices::panel_value(&panel, "server").unwrap_or_default();
                self.club = None;
                self.visit.club = None;
                self.bring_club(choices::resolve(&server, self.p.seat_port), 1, &server)
                    .map_err(|why| Failed(Text::key("launcher.run_vs_club_refused").part("why", why)))?;
            } else {
                self.club_leftover(&self.kit.save().join("gamesvr"));
            }
            conf::set_game_server(&ctrl_conf, &t.host, t.port, true)?;
            self.tell(
                &format!("  1/{steps} "),
                &Text::key("launcher.run_gamesvr_at")
                    .arg("host", &t.host)
                    .arg("port", t.port),
            );
            // the lobby's VS room: its ticket goes to the game server in vslink's hello (crates/server/README.md "VS
            // rooms"); single use, so the line goes now. The window's heartbeat keeps the room while the run lasts.
            let ticket = t.room.map(|(_, ticket)| ticket);
            if ticket.is_some() {
                let _ = choices::write(&panel_file, &choices::set_panel_value(&panel, "ticket", None));
            }
            self.start_vslink(found, ticket);
            return Ok(());
        }
        // this PC: back to the stand-in's address, VS entries off, after an online run
        conf::set_game_server(&ctrl_conf, "localhost", port, false)?;
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| Failed(gamesvr_bind_error(port, &e)))?;
        let log = file_log(self.logs().join("gamesvr_log.txt"));
        // the run's game folder (also recorded as "game" by the 2017-18 setup): the stand-in pre-registers its 2D-code cards,
        // and data\settings.json "skip_tutorial": true makes it give a new Aime id a club from the game's cpu_50.dat
        // (no window sets it now: seed-club.sh and the fixture use it, players play the tutorial)
        let skip = self.kit.settings()["skip_tutorial"].as_bool().unwrap_or(false);
        let game = Some(self.game.clone());
        // guests' clubs a crashed arcade left here (online::club): their players keep their home copies
        let store = self.kit.save().join("gamesvr");
        fs::create_dir_all(&store)?;
        match gamesvr::Store::open(&store.join("gamesvr.db")).and_then(|mut s| s.purge_guests()) {
            Ok(0) => {}
            Ok(n) => self.tell("  ", &Text::key("launcher.run_guests_dropped").arg("n", n)),
            Err(e) => self.tell("  ", &Text::key("launcher.run_guests_unchecked").arg("error", e)),
        }
        self.gamesvr = Some(
            gamesvr::spawn_with(listener, self.kit.save().join("gamesvr"), log, game, skip)
                .map_err(|e| Failed(Text::key("launcher.run_gamesvr_failed").arg("error", e)))?,
        );
        self.tell(
            &format!("  1/{steps} "),
            &Text::key("launcher.run_gamesvr_local").arg("port", port),
        );
        Ok(())
    }

    /// A club still marked out in an arcade (the last trip ended before it came home): the home copy stays, the arcade's
    /// last copy goes to backups, and the player is told once.
    fn club_leftover(&self, dir: &Path) {
        if let Some(t) = leftover_note(dir) {
            self.tell("  ", &t);
        }
    }

    /// 2017-18 arcade guest, or online on a server's own arcade: the home club comes along to the desk at `desk`
    /// (online::club), under the card's home Aime id (an older card's integer id) here and its hash there. `Err`: why
    /// it stayed home.
    fn bring_club(&mut self, desk: Option<SocketAddr>, seat: u8, arcade: &str) -> Result<(), Text> {
        let save = self.kit.save();
        let dir = save.join("gamesvr");
        // a first run: the card the reader will read, made now so its club (none yet) comes along too
        aime::ensure_card(&save.join("seat1_aime.txt"));
        let code = fs::read_to_string(save.join("seat1_aime.txt"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| s.len() == 20 && s.bytes().all(|b| b.is_ascii_digit()));
        let home = code
            .as_ref()
            .map(|c| aimedb::Db::open(save.join("aimedb.json")).map(|db| db.home_id(c)));
        let guest = match (code, home, desk) {
            (Some(code), Some(Ok(home)), Some(desk)) => Some(club::Guest {
                desk,
                seat,
                arcade_aime: aimedb::hash_id(&code) as u32,
                code,
                home_aime: home as u32,
                dir: dir.clone(),
                arcade: arcade.to_owned(),
                poll: club::POLL,
            }),
            _ => None,
        };
        let log = file_log(self.logs().join("club_trip.txt"));
        let (guest, said) = before_bring(guest, &dir, &log)?;
        said.iter().for_each(|t| self.tell("  ", t));
        match guest.bring(log) {
            Ok(trip) => {
                self.visit.club = Some(trip.progress());
                self.club = Some(trip);
                self.tell("  ", &Text::key("launcher.club_in"));
                Ok(())
            }
            Err(why) => Err(Text::Raw(why)),
        }
    }

    /// The direct VS link to the online game server, with the lobby room's ticket if one is held;
    /// data\\settings.json "vs_link": false turns it off.  A failure is said and the game goes on: it gives a CPU team
    /// when no link is up.
    fn start_vslink(&mut self, gmsvr: SocketAddr, ticket: Option<String>) {
        if self.kit.settings()["vs_link"].as_bool() == Some(false) {
            self.tell(
                "  ",
                &Text::key(if ticket.is_some() {
                    "launcher.run_vs_off_room"
                } else {
                    "launcher.run_vs_off"
                }),
            );
            return;
        }
        let room = ticket.is_some();
        let mut cfg = vslink::Config::public(
            gmsvr,
            self.kit.data().join("vslink.key"),
            file_log(self.logs().join("vslink_log.txt")),
        );
        cfg.ticket = ticket;
        // gmsvr pairs only PCs whose match engine math is the same; a room partner with another is said once
        if self.edition == Edition::E1718 {
            cfg.dmath = Some(seat::dmath_token(&self.game, &self.seat));
            let tx = self.tx.clone();
            cfg.on_dmath_differs = Some(Arc::new(move |mine| {
                let _ = tx.send(Event::Said(
                    Text::key("launcher.run_vs_dmath_differs").arg("mine", mine),
                ));
            }));
        }
        // WCCF_VS_RELAY, else settings "vs_relay" (URL); settings "vs_relay_ca" (PEM file) is the relay's only trusted root
        let settings = self.kit.settings();
        let saved = |k: &str| settings[k].as_str().map(str::to_owned);
        if let Some(url) = choices::env_or_var("WCCF_VS_RELAY", saved("vs_relay")) {
            let ca = saved("vs_relay_ca").filter(|c| !c.trim().is_empty());
            if let Err(e) = cfg.set_relay(&url, ca.as_deref().map(Path::new)) {
                self.tell("  ", &Text::key("launcher.run_vs_failed").arg("error", e));
                return;
            }
        }
        match vslink::spawn(cfg) {
            Ok(link) => {
                self.tell(
                    "  ",
                    &Text::key(if room {
                        "launcher.run_vs_on_room"
                    } else {
                        "launcher.run_vs_on"
                    }),
                );
                self.vslink = Some(link.handle);
            }
            Err(e) => self.tell("  ", &Text::key("launcher.run_vs_failed").arg("error", e)),
        }
    }

    /// 2017-18 arcade over iroh (vslink::arcade). Host: beside `forza-server all`, its id said and kept in
    /// data\shop_id.txt. Join: this PC's loopback control, desk and relay ports lead to that host. The relay as for VS
    /// (WCCF_VS_RELAY, settings "vs_relay" and "vs_relay_ca"); `VSLINK_DELAY_MS` (tests only) adds that much to the
    /// round trip.
    fn start_arcade_tunnel(&mut self, role: vslink::arcade::Role) -> Result<vslink::arcade::Ports, Failed> {
        let p = self.p;
        let host = role == vslink::arcade::Role::Host;
        let usual = vslink::arcade::Ports {
            control: p.control_port,
            desk: p.seat_port,
            relay: p.relay_port,
        };
        // a server on this PC holds the usual ports: a join's tunnel takes free ones, the seat confs and env follow them
        let ports = rules::tunnel_ports(usual, host, procs::listening_ports, |p| procs::udp_ports().contains(&p));
        let key = self
            .kit
            .data()
            // file names kept from before the arcade rename: players' installs already hold these keys and logs
            .join(if host { "shop_host.key" } else { "shop_join.key" });
        let log = file_log(self.logs().join("shop_tunnel.txt"));
        let mut cfg = vslink::arcade::ArcadeConfig::public(role, ports, key, log);
        let settings = self.kit.settings();
        let saved = |k: &str| settings[k].as_str().map(str::to_owned);
        if let Some(url) = choices::env_or_var("WCCF_VS_RELAY", saved("vs_relay")) {
            let ca = saved("vs_relay_ca").filter(|c| !c.trim().is_empty());
            cfg.set_relay(&url, ca.as_deref().map(Path::new))
                .map_err(|e| Failed(Text::Raw(e)))?;
        }
        // a host's room first: its code is the only way in, and the tunnel admits only the guests the room names
        let room = host.then(|| self.open_arcade_room(&cfg.key, cfg.log.clone()));
        if let Some(Ok(r)) = &room {
            cfg.admit = Some(r.admit());
        }
        let knob = rules::delay_knob(env::var("VSLINK_DELAY_MS").ok().as_deref());
        cfg.delay = knob / 2;
        if !knob.is_zero() {
            self.tell(
                "     ",
                &Text::key("launcher.run_test_delay").arg("ms", knob.as_millis()),
            );
        }
        let link =
            vslink::arcade::spawn(cfg).map_err(|e| Failed(Text::key("launcher.run_tunnel_failed").arg("error", e)))?;
        if host {
            let id = vslink::arcade::id_text(&link.id);
            // file name kept from before the arcade rename
            let _ = choices::write(&self.kit.data().join("shop_id.txt"), &format!("{id}\n"));
            self.arcade = Some(match &room {
                Some(Ok(r)) => format!("{id} code {}", r.code),
                _ => id.clone(),
            });
            match &room {
                Some(Ok(r)) => self.tell(
                    "     ",
                    &Text::key("launcher.run_arcade_room")
                        .arg("code", &r.code)
                        .arg("server", &r.server)
                        .arg("id", &id),
                ),
                Some(Err(why)) => self.tell(
                    "     ",
                    &Text::key("launcher.run_arcade_no_room").arg("why", why).arg("id", &id),
                ),
                None => {}
            }
            self.tell("     ", &Text::key("launcher.run_arcade_id_file"));
            self.arcade_room = room;
            self.arcade_guests = Some(link.guests());
        } else if link.ports != ports {
            self.tell(
                "       ",
                &Text::key("launcher.run_tunnel_ports")
                    .arg("control", link.ports.control)
                    .arg("desk", link.ports.desk)
                    .arg("relay", link.ports.relay),
            );
        } else {
            self.tell("       ", &Text::key("launcher.run_tunnel_up"));
        }
        if !host {
            self.visit.host_closed = Some(link.host_closed());
        }
        self.vslink = Some(link.handle);
        Ok(link.ports)
    }

    /// The lobby room for the host's tunnel (`key` its endpoint key), on the club server's desk ([`crate::lobby::arcade_lobby`]),
    /// where the window's guests type its code; listed in ARCADES when data\settings.json `list_shop` is true, as
    /// `shop_name` (empty: the desk's name).
    fn open_arcade_room(&self, key: &Path, log: Log) -> Result<crate::lobby::HostRoom, String> {
        let id = vslink::arcade::own_id(key).map_err(|e| format!("{}: {e}", key.display()))?;
        let club = choices::setting(&self.kit.settings_file(), "club_server").unwrap_or_default();
        let desk = crate::lobby::arcade_lobby(&club);
        // settings keys kept from before the arcade rename: players' settings.json already holds them
        let public = self.kit.settings()["list_shop"].as_bool() == Some(true);
        let name = choices::setting(&self.kit.settings_file(), "shop_name").unwrap_or_default();
        let secret = crate::lobby::room_secret(key);
        crate::lobby::HostRoom::open(
            &desk,
            &vslink::arcade::id_text(&id),
            secret.as_deref(),
            public,
            &name,
            log,
            &|t| self.tell("     ", t),
        )
    }

    /// A 2017-18 server's arcade room: its code, the lobby server holding it and the flag set when that lobby loses it;
    /// or why it has none (the window's HOST ARCADE shows one or the other).
    pub fn arcade_code(&self) -> Result<(String, String, Arc<AtomicBool>), String> {
        match &self.arcade_room {
            Some(Ok(r)) => Ok((r.code.clone(), r.server.clone(), r.lost())),
            Some(Err(why)) => Err(why.clone()),
            None => Err(Text::key("launcher.run_no_tunnel").get(self.ui.lang())),
        }
    }

    /// 2017-18 server: its arcade tunnel's live guest count (HOST ARCADE closes an empty arcade on its own).
    pub fn arcade_guests(&self) -> Option<Arc<AtomicUsize>> {
        self.arcade_guests.clone()
    }

    /// Online: this PC's seat from the server's seat desk, held while the run lasts; None asked = the next free seat.
    fn take_seat(&mut self, ip: &str, want: Option<u8>) -> Result<(u8, bool), Failed> {
        let desk = self.remote.as_ref().map_or(self.p.seat_port, |r| r.1);
        let at = addr(ip, desk).ok_or_else(|| Failed(Text::key("launcher.run_not_address").arg("ip", ip)))?;
        let log = file_log(self.logs().join("run_seat_lease.txt"));
        // a seat the lobby reserved: data\panel.txt "ticket=" (single use, so the line goes before the claim)
        let panel = self.kit.data().join("panel.txt");
        let text = choices::read(&panel);
        let ticket = choices::panel_value(&text, "ticket").filter(|t| !t.is_empty());
        if ticket.is_some() {
            let _ = choices::write(&panel, &choices::set_panel_value(&text, "ticket", None));
        }
        // every seat of an arcade must run the same match engine math (the desk refuses another)
        let dmath = (self.edition == Edition::E1718).then(|| seat::dmath_token(&self.game, &self.seat));
        match seat::claim_with(
            at,
            want,
            ticket.as_deref(),
            dmath.as_deref(),
            LIFE,
            seat::Timing::default(),
            log,
        ) {
            Ok(lease) => {
                let got = (lease.seat, lease.projector);
                self.lease = Some(lease);
                Ok(got)
            }
            Err(e) => match rules::desk_fallback(self.edition, &e, want) {
                Some((s, proj)) => {
                    let key = if proj {
                        "launcher.run_no_desk_projector"
                    } else {
                        "launcher.run_no_desk"
                    };
                    self.tell("  ", &Text::key(key).arg("ip", ip).arg("seat", s));
                    Ok((s, proj))
                }
                None => Err(Failed(rules::seat_refusal(&e, want, ip))),
            },
        }
    }
}
