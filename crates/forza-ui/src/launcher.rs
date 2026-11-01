//! The launcher: one frameless 600 px window in tabs to play, play online, set up, update and read the output. [`show`] draws a
//! [`LauncherState`] and returns what the player asked for; Launcher.exe (crates/launcher) does it and fills the state
//! from the real kit, the gallery and the snapshots draw [`LauncherState::sample`].

use super::Lang;
use super::lobby::{self, Grey, Kind, LobbyAction, LobbyView, Scenario as LobbyScenario};
use super::theme::{self, Edition};
use super::widgets::{
    self, Cab, Tone, action, cabinet, field, progress, segmented, start_bar, status_line, steps,
    text_w, title_bar, toggle_row,
};
use crate::theme::Accent;
use egui::{Color32, Ui, vec2};

pub const SIZE: egui::Vec2 = egui::vec2(600.0, 820.0);

/// The window's size for a screen whose work area (the screen less the taskbar) is `work_height` points tall: [`SIZE`],
/// shorter on a short screen (1366x768, 1280x720) so PLAY and the status line stay on it; the tabs scroll.
pub fn window_size(work_height: Option<f32>) -> egui::Vec2 {
    vec2(SIZE.x, work_height.map_or(SIZE.y, |h| h.min(SIZE.y)))
}

/// A release in the update list.
#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    /// A test build (a pre-release), else a release.
    pub test: bool,
    pub date: String,
    /// The version in this folder.
    pub this: bool,
    pub newer: bool,
}

/// The launcher's tabs, left to right; PLAY / STOP THE GAME (ONLINE: only STOP) and the status line sit under them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Play,
    Online,
    Setup,
    /// Only when the edition has updates (`updates`).
    Update,
    Details,
}

/// What the player asked for this frame. Launcher.exe does it; the gallery ignores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// PLAY, with the choices in the state.
    Play,
    /// STOP THE GAME, clicked twice: stop by the card rule (the run asks first with a club card in or a match on).
    Stop,
    /// SET UP the folder in the state.
    SetUp,
    /// UNDO SETUP asked: Launcher.exe checks the folder and asks to confirm.
    Undo,
    /// UNDO SETUP, confirmed.
    UndoNow,
    /// Browse for the game folder.
    Browse,
    /// The card pictures choice changed (`pictures_hq`), or "Choose pack…" (`choose`).
    Pictures { choose: bool },
    /// The skip-the-tutorial switch changed.
    SkipTutorial,
    /// The direct VS link switch changed.
    VsLink,
    /// UPDATE to the release at this place in the list: Launcher.exe asks to confirm.
    Update(usize),
    /// UPDATE, confirmed.
    UpdateNow(usize),
    /// VIEW ON GITHUB: the release page of the release at this place in the list; none chosen, every release.
    WhatsNew(Option<usize>),
    /// The window's language changed (`lang`).
    Language,
    /// SAVE LOGS: the logs ZIP into the Downloads folder.
    SaveLogs,
    /// The player chose something in the online lobby.
    Lobby(LobbyAction),
    /// "Rejoin ..." (`rejoin`): reserve the last seat again.
    Rejoin,
    /// REPORT A PROBLEM: open the report dialog (`report`).
    Report,
    /// SAVE LOGS in the report dialog: its words and the logs in one ZIP on this PC.
    SendReport,
}

/// The report dialog: the player's words, what goes with them, and where the ZIP was saved.
#[derive(Clone, Debug, Default)]
pub struct ReportForm {
    /// Opened after a crash: the title says so.
    pub crashed: bool,
    pub words: String,
    /// What is in the ZIP, by its names there.
    pub contents: Vec<String>,
    pub sending: bool,
    /// The outcome line: where the ZIP is, or why it was not saved.
    pub result: Option<(Tone, String)>,
    /// Saved: only CLOSE is left.
    pub sent: bool,
    /// Once saved: GitHub's new-issue page filled in (`launcher::report::issue_url`).
    pub issue_url: Option<String>,
}

/// A question in a modal: a title, an optional body, the button that says yes and what yes does.
#[derive(Clone, Debug)]
pub struct Confirm {
    pub title: String,
    pub body: String,
    pub yes: String,
    pub then: Action,
}

impl Confirm {
    /// A question as one text: its first paragraph is the title, the rest the body.
    pub fn new(question: &str, yes: String, then: Action) -> Self {
        let (title, body) = question.split_once("\n\n").unwrap_or((question, ""));
        Confirm {
            title: title.to_owned(),
            body: body.to_owned(),
            yes,
            then,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LauncherState {
    pub edition: Edition,
    pub lang: Lang,
    pub online: bool,
    /// The online lobby; its Direct address is the server CONNECT uses.
    pub lobby: LobbyView,
    /// The last seat, offered again within the desk's 75 s: "Rejoin FORZA ROMA, seat 3".
    pub rejoin: Option<String>,
    /// A game folder is set up.
    pub set_up: bool,
    /// A run is on in this folder (here or in another Launcher.exe): PLAY offers STOP THE GAME.
    pub running: bool,
    /// A job runs (setup, an update, the run): the choices wait.
    pub busy: bool,
    /// The job's steps done of all (a start's "N/M" lines), else its progress 0..1.
    pub steps: Option<(usize, usize)>,
    pub progress: f32,
    /// Seconds since the last step line and since the job began (the start's bar and clock).
    pub clock: (f32, f32),
    /// The status line: a tone and its text.
    pub status: Option<(Tone, String)>,
    /// The tab shown.
    pub tab: Tab,
    pub folder: String,
    /// The card pictures: the high-quality pack, else the game's own.
    pub pictures_hq: bool,
    /// The skip-the-tutorial switch (2017–18 only).
    pub skip_tutorial: Option<bool>,
    /// The club server, "HOST" or "HOST:PORT", empty for this PC (2017–18 only; saved at PLAY).
    pub club_server: Option<String>,
    /// The direct VS link switch, for club-server runs (2017–18 only).
    pub vs_link: Option<bool>,
    /// The UPDATE section (2010–11 only: the releases are 2010–11 versions).
    pub updates: bool,
    /// The version in this folder.
    pub version: String,
    /// The tag of a newer release found by the update check: shown until the UPDATE tab is opened or the update is done.
    pub update_available: Option<String>,
    pub update_status: Option<(Tone, String)>,
    pub releases: Vec<Release>,
    pub selected: Option<usize>,
    /// The chosen release's notes in the window's language, plain text (`launcher::release::notes`); remote text, drawn
    /// only as a label.
    pub notes: Option<String>,
    pub output: Vec<String>,
    /// A question shown in a modal.
    pub confirm: Option<Confirm>,
    /// The report dialog, when open.
    pub report: Option<ReportForm>,
    /// STOP THE GAME is armed until this time (egui seconds); a second click before it stops the game.
    pub stop_armed: f64,
}

/// Seconds a STOP THE GAME click stays armed.
const STOP_ARM: f64 = 8.0;

/// One click on STOP THE GAME at `now`, armed until `armed`: the new `armed` and whether the game is to stop.
fn stop_click(armed: f64, now: f64) -> (f64, bool) {
    if armed > now {
        (0.0, true)
    } else {
        (now + STOP_ARM, false)
    }
}

/// The states rendered for review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Ready,
    Running,
    StopArmed,
    NotSetUp,
    Update,
    /// SETUP open on 2017–18 (card pictures, skip the tutorial), a setup running.
    Busy,
    /// ONLINE chosen: the lobby with sample rows.
    Online,
    /// The report dialog after a crash, words typed.
    Report,
}

/// An empty lobby for the edition, open on SHOPS: the kind with nothing behind it greyed (2010–11 VS); 2017–18 shops are
/// listed, view-only.
pub fn lobby_for(edition: Edition) -> LobbyView {
    let mut v = LobbyView::new();
    match edition {
        Edition::E1011 => {
            v.grey[Kind::Vs as usize] = Some(Grey::Vs1011);
        }
        Edition::E1718 => {
            v.shops_view_only = true;
            v.codes = true;
            v.vs_hidden = !lobby::VS_LOBBY;
        }
    }
    v
}

impl LauncherState {
    pub fn sample(edition: Edition, lang: Lang, s: Scenario) -> Self {
        let release = |tag: &str, test, date: &str, this, newer| Release {
            tag: tag.into(),
            test,
            date: date.into(),
            this,
            newer,
        };
        let mut st = LauncherState {
            edition,
            lang,
            online: false,
            lobby: lobby_for(edition),
            rejoin: None,
            set_up: true,
            running: false,
            busy: false,
            steps: None,
            progress: 0.0,
            clock: (0.0, 0.0),
            status: Some((Tone::Done, tr!(lang, "launcher.status_ready"))),
            tab: Tab::Play,
            folder: r"C:\Games\Forza-WCCF\2010-2011".into(),
            pictures_hq: false,
            skip_tutorial: None,
            club_server: None,
            vs_link: None,
            updates: edition == Edition::E1011,
            version: "7.0.0".into(),
            update_available: None,
            update_status: None,
            releases: vec![
                release("v7.1.0-test.2", true, "2026-10-11", false, false),
                release("v7.0.1", false, "2026-10-09", false, true),
                release("v7.0.0", false, "2026-09-28", true, false),
                release("v6.4.2", false, "2026-08-30", false, false),
            ],
            selected: Some(1),
            notes: None,
            output: [
                "[15:02:11] start: seat 1, this PC",
                "[15:02:11] card table: 16 slots ready",
                "[15:02:12] network: stand-ins on 127.0.0.1",
                "[15:02:14] game: window 1440x900",
                "[15:02:19] club card: session open (KITAKAZE ESTE)",
            ]
            .map(String::from)
            .into(),
            confirm: None,
            report: None,
            stop_armed: 0.0,
        };
        if edition == Edition::E1718 {
            st.folder = r"C:\Games\Forza-WCCF\2017-2018".into();
            st.skip_tutorial = Some(true);
            st.club_server = Some(String::new());
            st.vs_link = Some(true);
        }
        match s {
            Scenario::Ready => {}
            Scenario::Running | Scenario::StopArmed => {
                st.running = true;
                st.busy = true;
                st.online = true;
                st.steps = Some((5, 6));
                st.clock = (12.0, 62.0);
                st.status = Some((Tone::Notice, tr!(lang, "launcher.stage_screen")));
                st.tab = Tab::Details;
                if s == Scenario::StopArmed {
                    st.stop_armed = f64::INFINITY; // never lapses: the snapshot shows 8 s
                }
            }
            Scenario::NotSetUp => {
                st.set_up = false;
                st.tab = Tab::Setup;
                st.folder = r"D:\Arcade\disc-image".into();
                st.status = Some((Tone::Notice, tr!(lang, "launcher.status_not_set_up")));
            }
            Scenario::Update => {
                st.tab = Tab::Update;
                st.notes = Some(
                    match lang {
                        Lang::En => "A short network stall no longer hands a match to the CPU.\n\nFor players\n\n\
                            - RESET LINEUP puts the board back to the start of the match.\n\
                            - The Launcher shows these notes in your language.",
                        Lang::It => "Un breve blocco della rete non passa più la partita alla CPU.\n\nPer i giocatori\n\n\
                            - RESET LINEUP riporta la lavagna all'inizio della partita.\n\
                            - Il Launcher mostra queste note nella tua lingua.",
                        Lang::Ja => "短いネットワークの停止で試合が CPU に渡らなくなりました。\n\nプレイヤー向け\n\n\
                            - RESET LINEUP でボードが試合開始時に戻ります。\n\
                            - Launcher がこのノートをあなたの言語で表示します。",
                    }
                    .to_owned(),
                );
                st.update_status = Some((
                    Tone::Refused,
                    tr!(
                        lang,
                        "launcher.msg_no_answer",
                        error = tr!(lang, "net.offline").as_str()
                    ),
                ));
            }
            Scenario::Online => {
                st.online = true;
                st.tab = Tab::Online;
                let mut v = LobbyView::sample(LobbyScenario::List);
                v.grey = st.lobby.grey;
                v.kind = st.lobby.kind;
                v.codes = st.lobby.codes;
                v.shops_view_only = st.lobby.shops_view_only;
                v.vs_hidden = st.lobby.vs_hidden;
                if v.shops_view_only {
                    // 2017-18 lists only shop rooms
                    v.shop_rooms = true;
                    v.shops.iter_mut().for_each(|r| r.room = true);
                }
                st.lobby = v;
            }
            Scenario::Report => {
                st.status = Some((Tone::Refused, tr!(lang, "launcher.msg_went_wrong")));
                st.report = Some(ReportForm {
                    crashed: true,
                    words: "Seat 1 froze after the second match.".into(),
                    contents: [
                        "report.txt",
                        "logs/launcher.txt",
                        "logs/run_seat1.txt",
                        "logs/hook/mxhook_4120.log",
                        "seat1/forza_panel.log",
                        "data/panel.txt",
                        "version.txt",
                    ]
                    .map(String::from)
                    .into(),
                    ..ReportForm::default()
                });
            }
            Scenario::Busy => {
                st.busy = true;
                st.tab = Tab::Setup;
                st.pictures_hq = true;
                st.progress = 0.42;
                st.status = Some((Tone::Notice, tr!(lang, "launcher.msg_making_pictures")));
            }
        }
        st
    }
}

/// The whole launcher window; what the player asked for, if anything.
pub fn show(ui: &mut Ui, st: &mut LauncherState) -> Option<Action> {
    let mut act = None;
    let was = st.lang;
    let accent = st.edition.accent();
    egui::Panel::top("launcher-title")
        .exact_size(44.0)
        .frame(egui::Frame::new())
        .show_separator_line(false)
        .show(ui, |ui| {
            title_bar(ui, st.edition, &st.version, &mut st.lang);
        });
    if st.lang != was {
        act = Some(Action::Language);
    }
    let lang = st.lang;
    // PLAY / STOP THE GAME and the status line stay in view on a short screen; the rest scrolls above them
    egui::Panel::bottom("launcher-footer")
        .frame(
            egui::Frame::new()
                .fill(theme::NIGHT)
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 10,
                    bottom: 14,
                }),
        )
        .show_separator_line(false)
        .show(ui, |ui| {
            let r = ui.max_rect();
            ui.painter().hline(
                r.x_range(),
                r.top() - 10.0,
                egui::Stroke::new(1.0, theme::RIM),
            );
            ui.spacing_mut().item_spacing.y = 8.0;
            footer(ui, st, lang, &mut act);
        });
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(theme::NIGHT)
                .inner_margin(egui::Margin::symmetric(16, 14)),
        )
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            tab_row(ui, st, lang);
            // UPDATE says it in its own words
            if st.tab != Tab::Update
                && let Some(tag) = &st.update_available
            {
                status_line(
                    ui,
                    Tone::Notice,
                    &tr!(lang, "launcher.msg_available", tag = tag.as_str()),
                );
            }
            // each tab gets the rest of the height; DETAILS (its log box scrolls) and ONLINE (its lobby list shrinks to fit,
            // `list_fit`) have no outer scroll
            match st.tab {
                Tab::Details => return output(ui, st, lang, &mut act),
                Tab::Online => return online(ui, st, lang, &mut act),
                _ => {}
            }
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| match st.tab {
                    Tab::Play => play(ui, st, lang, accent),
                    Tab::Setup => setup(ui, st, lang, &mut act),
                    Tab::Update => update(ui, st, lang, &mut act),
                    Tab::Details | Tab::Online => {}
                });
        });
    if let Some(c) = st.confirm.clone()
        && let Some(a) = confirm_modal(ui, st, &c, lang)
    {
        act = Some(a);
    }
    if st.report.is_some()
        && let Some(a) = report_modal(ui, st, lang)
    {
        act = Some(a);
    }
    act
}

/// The tab row, in the segmented controls' look; a tab the edition lacks falls back to PLAY.
fn tab_row(ui: &mut Ui, st: &mut LauncherState, lang: Lang) {
    let mut tabs = vec![
        (Tab::Play, tr!(lang, "launcher.play_section")),
        (Tab::Online, tr!(lang, "launcher.tab_online")),
        (Tab::Setup, tr!(lang, "launcher.setup")),
    ];
    if st.updates {
        tabs.push((Tab::Update, tr!(lang, "launcher.update")));
    }
    tabs.push((Tab::Details, tr!(lang, "launcher.output")));
    let labels: Vec<&str> = tabs.iter().map(|(_, l)| l.as_str()).collect();
    let mut i = tabs.iter().position(|(t, _)| *t == st.tab).unwrap_or(0);
    let w = ui.available_width();
    segmented(ui, &mut i, &labels, w);
    st.tab = tabs[i].0;
}

/// The PLAY tab: where to play (Online opens the ONLINE tab) and, without UPDATE, this version.
fn play(ui: &mut Ui, st: &mut LauncherState, lang: Lang, accent: Color32) {
    widgets::section(ui, &tr!(lang, "launcher.play_section"), accent, |ui| {
        let label_w = 150.0;
        let w = ui.available_width() - label_w;
        ui.add_enabled_ui(!st.running && !st.busy, |ui| {
            ui.horizontal(|ui| {
                field(ui, &tr!(lang, "launcher.where"), label_w);
                let mut i = st.online as usize;
                if segmented(
                    ui,
                    &mut i,
                    &[
                        &tr!(lang, "launcher.this_pc"),
                        &tr!(lang, "launcher.online"),
                    ],
                    w,
                ) {
                    st.online = i == 1;
                    if st.online {
                        st.tab = Tab::Online;
                    }
                }
            });
        });
    });
    if !st.updates {
        ui.label(
            egui::RichText::new(tr!(lang, "launcher.this_version", v = st.version.as_str()))
                .font(theme::body(12.0))
                .color(theme::TEXT_FAINT),
        );
    }
}

/// The ONLINE tab: the lobby. Looking changes nothing; joining, hosting or CONNECT there plays online.
fn online(ui: &mut Ui, st: &mut LauncherState, lang: Lang, act: &mut Option<Action>) {
    let free = !st.running && !st.busy;
    // the list takes the height the tab leaves: the tab has no scroll of its own
    st.lobby.list_fit = true;
    // HOST SHOP's code and LEAVE stay live while the shop and this PC's seat run
    ui.add_enabled_ui(free || st.lobby.hosting, |ui| {
        if let Some(text) = st.rejoin.clone()
            && action(ui, &text).clicked()
        {
            // the seat it reserves is held only while the mode is online (online.rs `look`)
            st.online = true;
            *act = Some(Action::Rejoin);
        }
        let mut vs_link = false;
        let (club_server, link) = (&mut st.club_server, &mut st.vs_link);
        // the 2017-18 club server rows live with online play, under the Direct address
        let chose = lobby::show_with(ui, &mut st.lobby, lang, |ui| {
            vs_link = club_rows(ui, club_server, link, lang);
        });
        if let Some(a) = chose {
            if !matches!(a, LobbyAction::Refresh | LobbyAction::DirectAddress(_)) {
                st.online = true;
            }
            // CONNECT starts online play at the Direct address, as PLAY did before ONLINE lost it
            *act = Some(if a == LobbyAction::Connect {
                Action::Play
            } else {
                Action::Lobby(a)
            });
        } else if vs_link {
            *act = Some(Action::VsLink);
        }
    });
}

/// PLAY / STOP THE GAME, the start's bar and the status line: the window's footer. ONLINE has no PLAY (its lobby
/// buttons and CONNECT start the game), only STOP THE GAME while a game runs.
fn footer(ui: &mut Ui, st: &mut LauncherState, lang: Lang, act: &mut Option<Action>) {
    let now = ui.input(|i| i.time);
    let armed = st.running && st.stop_armed > now;
    if armed && st.stop_armed.is_finite() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
    if st.running || st.tab != Tab::Online {
        let size = vec2(ui.available_width(), 64.0);
        let (label, kind) = if armed {
            let left = (st.stop_armed - now).ceil().min(STOP_ARM) as u32;
            (tr!(lang, "launcher.stop_again", s = left), Cab::Stop)
        } else if st.running {
            (tr!(lang, "launcher.stop"), Cab::Stop)
        } else {
            (tr!(lang, "launcher.play"), Cab::Go)
        };
        // STOP THE GAME stays on while this Launcher.exe runs the game (that run is its job)
        let resp = ui
            .add_enabled_ui(st.running || !st.busy, |ui| {
                cabinet(ui, size, &label, kind, st.set_up || st.running)
            })
            .inner;
        if resp.clicked() {
            if st.running {
                let (a, stop) = stop_click(st.stop_armed, now);
                st.stop_armed = a;
                if stop {
                    *act = Some(Action::Stop);
                }
            } else if st.online {
                // online play starts from the lobby: PLAY shows it instead of playing a hidden address
                st.tab = Tab::Online;
            } else {
                *act = Some(Action::Play);
            }
        }
    }
    if st.busy {
        match st.steps {
            Some((n, 6)) => start_bar(ui, n, st.clock, ui.available_width()),
            Some((n, m)) => steps(ui, n, m, ui.available_width()),
            None => progress(ui, st.progress, ui.available_width()),
        };
    }
    if armed {
        status_line(ui, Tone::Notice, &tr!(lang, "launcher.msg_stop_armed"));
    } else if let Some((tone, text)) = &st.status {
        status_line(ui, *tone, text);
    }
}

fn setup(ui: &mut Ui, st: &mut LauncherState, lang: Lang, act: &mut Option<Action>) {
    if st.set_up {
        status_line(ui, Tone::Done, &tr!(lang, "launcher.setup_done"));
    } else {
        status_line(ui, Tone::Notice, &tr!(lang, "launcher.setup_needed"));
    }
    ui.add_enabled_ui(!st.busy, |ui| {
        ui.label(
            egui::RichText::new(tr!(lang, "launcher.folder_hint"))
                .font(theme::body(13.0))
                .color(theme::TEXT_DIM),
        );
        ui.horizontal(|ui| {
            let browse = tr!(lang, "launcher.browse");
            let bw = text_w(ui, &browse) + 40.0;
            ui.add(
                egui::TextEdit::singleline(&mut st.folder)
                    .font(theme::mono(14.0))
                    .desired_width(ui.available_width() - bw)
                    .margin(vec2(10.0, 7.0)),
            );
            if action(ui, &browse).clicked() {
                *act = Some(Action::Browse);
            }
        });
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 8.0) / 2.0;
            if cabinet(
                ui,
                vec2(w, 44.0),
                &tr!(lang, "launcher.set_up"),
                Cab::Go,
                !st.set_up,
            )
            .clicked()
            {
                *act = Some(Action::SetUp);
            }
            let can_undo = !st.folder.trim().is_empty();
            let undo = ui.add_enabled_ui(can_undo, |ui| {
                cabinet(
                    ui,
                    vec2(w, 44.0),
                    &tr!(lang, "launcher.undo"),
                    Cab::Plain,
                    false,
                )
            });
            if undo.inner.clicked() {
                *act = Some(Action::Undo);
            }
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let label_w = 150.0;
            field(ui, &tr!(lang, "launcher.card_pictures"), label_w);
            let choose = tr!(lang, "launcher.choose_pack");
            let w = ui.available_width() - text_w(ui, &choose) - 40.0;
            let mut i = st.pictures_hq as usize;
            if segmented(
                ui,
                &mut i,
                &[
                    &tr!(lang, "launcher.from_game"),
                    &tr!(lang, "launcher.hq_pack"),
                ],
                w,
            ) {
                st.pictures_hq = i == 1;
                *act = Some(Action::Pictures { choose: false });
            }
            if action(ui, &choose).clicked() {
                *act = Some(Action::Pictures { choose: true });
            }
        });
        if let Some(skip) = &mut st.skip_tutorial
            && toggle_row(
                ui,
                skip,
                &tr!(lang, "launcher.skip_tutorial"),
                &tr!(lang, "launcher.skip_tutorial_hint"),
            )
            .clicked()
        {
            *act = Some(Action::SkipTutorial);
        }
    });
}

/// The 2017-18 club server and the direct VS link switch (when the edition has them); true when the switch changed.
fn club_rows(
    ui: &mut Ui,
    club_server: &mut Option<String>,
    vs_link: &mut Option<bool>,
    lang: Lang,
) -> bool {
    if let Some(club) = club_server {
        ui.horizontal(|ui| {
            field(ui, &tr!(lang, "launcher.club_server"), 150.0);
            ui.add(
                egui::TextEdit::singleline(club)
                    .font(theme::mono(14.0))
                    .hint_text(tr!(lang, "launcher.club_server_hint"))
                    .desired_width(ui.available_width())
                    .margin(vec2(10.0, 7.0)),
            );
        });
    }
    vs_link.as_mut().is_some_and(|on| {
        toggle_row(
            ui,
            on,
            &tr!(lang, "launcher.vs_link"),
            &tr!(lang, "launcher.vs_link_hint"),
        )
        .clicked()
    })
}

/// One release in the UPDATE list: "v0.5.0 · Stable · 2026-10-28", this folder's marked.
fn release_text(r: &Release, lang: Lang) -> String {
    let kind = if r.test {
        "launcher.kind_test"
    } else {
        "launcher.kind_stable"
    };
    let mut s = format!("{} · {} · {}", r.tag, tr!(lang, kind), r.date);
    if r.this {
        s = format!("{s} {}", tr!(lang, "launcher.mark_this"));
    } else if r.newer {
        s = format!("{s} {}", tr!(lang, "launcher.mark_newer"));
    }
    s
}

/// The UPDATE tab: this version or the newest, the releases in a list box with UPDATE beside it, VIEW ON GITHUB and the
/// chosen release's notes.
fn update(ui: &mut Ui, st: &mut LauncherState, lang: Lang, act: &mut Option<Action>) {
    let version = match &st.update_available {
        Some(tag) => tr!(lang, "launcher.update_available", tag = tag.as_str()),
        None => tr!(lang, "launcher.this_version", v = st.version.as_str()),
    };
    ui.label(
        egui::RichText::new(version)
            .font(theme::body(14.0))
            .color(theme::TEXT_DIM),
    );
    let chosen = st.selected.filter(|&i| i < st.releases.len());
    ui.horizontal(|ui| {
        let bw = 160.0;
        // the list box as tall as UPDATE beside it
        ui.spacing_mut().interact_size.y = 44.0;
        ui.add_enabled_ui(!st.busy, |ui| {
            let shown = chosen.map_or_else(String::new, |i| release_text(&st.releases[i], lang));
            egui::ComboBox::from_id_salt("releases")
                .selected_text(egui::RichText::new(shown).font(theme::body(14.0)))
                .width(ui.available_width() - bw - 8.0)
                .height(400.0)
                .show_ui(ui, |ui| {
                    for (i, r) in st.releases.iter().enumerate() {
                        ui.selectable_value(
                            &mut st.selected,
                            Some(i),
                            egui::RichText::new(release_text(r, lang)).font(theme::body(14.0)),
                        );
                    }
                });
        });
        let up = ui.add_enabled_ui(chosen.is_some() && !st.busy, |ui| {
            cabinet(
                ui,
                vec2(bw, 44.0),
                &tr!(lang, "launcher.update_button"),
                Cab::Go,
                true,
            )
        });
        if let (true, Some(i)) = (up.inner.clicked(), chosen) {
            *act = Some(Action::Update(i));
        }
    });
    if action(ui, &tr!(lang, "launcher.view_on_github")).clicked() {
        *act = Some(Action::WhatsNew(chosen));
    }
    if let Some((tone, text)) = &st.update_status {
        status_line(ui, *tone, text);
    }
    // the release notes: remote text, so a plain label (no markup, no links)
    if let Some(notes) = &st.notes {
        ui.label(
            egui::RichText::new(tr!(lang, "launcher.whats_new"))
                .font(theme::body(14.0))
                .color(theme::TEXT_DIM),
        );
        egui::Frame::new()
            .fill(theme::WELL)
            .corner_radius(theme::RADIUS)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    egui::Label::new(egui::RichText::new(notes.as_str()).font(theme::body(13.0)))
                        .wrap(),
                );
            });
    }
}

/// The DETAILS tab: the output filling the tab above COPY, SAVE LOGS and REPORT A PROBLEM.
fn output(ui: &mut Ui, st: &mut LauncherState, lang: Lang, act: &mut Option<Action>) {
    // the buttons' row and the box's margins stay below the log
    let h = (ui.available_height() - 64.0).max(80.0);
    egui::Frame::new()
        .fill(theme::WELL)
        .corner_radius(theme::RADIUS)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let font = theme::mono(12.5);
            let row = ui.fonts_mut(|f| f.row_height(&font));
            // setup's output runs to thousands of lines: only the rows in view are laid out
            egui::ScrollArea::both()
                .max_height(h)
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show_rows(ui, row, st.output.len(), |ui, range| {
                    for l in &st.output[range] {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(l)
                                    .font(font.clone())
                                    .color(Color32::from_rgb(0xA8, 0xD8, 0xB0)),
                            )
                            .extend(),
                        );
                    }
                });
        });
    ui.horizontal(|ui| {
        if action(ui, &tr!(lang, "launcher.copy")).clicked() {
            ui.ctx().copy_text(st.output.join("\r\n"));
        }
        if action(ui, &tr!(lang, "launcher.save_logs")).clicked() {
            *act = Some(Action::SaveLogs);
        }
        if action(ui, &tr!(lang, "launcher.report_problem")).clicked() {
            *act = Some(Action::Report);
        }
    });
}

fn confirm_modal(ui: &mut Ui, st: &mut LauncherState, c: &Confirm, lang: Lang) -> Option<Action> {
    let mut act = None;
    let modal = egui::Modal::new(egui::Id::new("launcher-confirm"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(
            egui::Frame::new()
                .fill(theme::DEEP)
                .stroke(egui::Stroke::new(1.0, theme::RIM))
                .corner_radius(theme::RADIUS_BIG)
                .inner_margin(20),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(400.0);
            ui.label(
                egui::RichText::new(&c.title)
                    .font(theme::display(26.0))
                    .color(theme::TEXT),
            );
            if !c.body.is_empty() {
                ui.label(egui::RichText::new(&c.body).color(theme::TEXT_DIM));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if cabinet(ui, vec2(196.0, 48.0), &c.yes, Cab::Go, true).clicked() {
                    act = Some(c.then.clone());
                }
                if cabinet(
                    ui,
                    vec2(196.0, 48.0),
                    &tr!(lang, "common.cancel"),
                    Cab::Plain,
                    false,
                )
                .clicked()
                {
                    st.confirm = None;
                }
            });
        });
    if act.is_some() || modal.should_close() {
        st.confirm = None;
    }
    act
}

/// The report dialog: "What happened?", what is in the ZIP, SAVE LOGS and CANCEL; once saved, where it is and CLOSE.
fn report_modal(ui: &mut Ui, st: &mut LauncherState, lang: Lang) -> Option<Action> {
    let mut act = None;
    let mut close = false;
    let f = st.report.as_mut()?;
    let modal = egui::Modal::new(egui::Id::new("launcher-report"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(
            egui::Frame::new()
                .fill(theme::DEEP)
                .stroke(egui::Stroke::new(1.0, theme::RIM))
                .corner_radius(theme::RADIUS_BIG)
                .inner_margin(20),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(460.0);
            let title = if f.crashed {
                tr!(lang, "report.title_crashed")
            } else {
                tr!(lang, "launcher.report_problem")
            };
            ui.label(
                egui::RichText::new(title)
                    .font(theme::display(24.0))
                    .color(theme::TEXT),
            );
            ui.add_space(4.0);
            field(ui, &tr!(lang, "report.what_happened"), 460.0);
            ui.add_enabled(
                !f.sending && !f.sent,
                egui::TextEdit::multiline(&mut f.words)
                    .char_limit(2000)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .font(theme::body(14.0))
                    .margin(vec2(10.0, 7.0)),
            );
            ui.add_space(4.0);
            field(ui, &tr!(lang, "report.what_is_sent"), 460.0);
            egui::Frame::new()
                .fill(theme::WELL)
                .corner_radius(theme::RADIUS)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .max_height(96.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for c in &f.contents {
                                ui.label(
                                    egui::RichText::new(c)
                                        .font(theme::mono(12.0))
                                        .color(theme::TEXT_DIM),
                                );
                            }
                        });
                });
            ui.label(
                egui::RichText::new(tr!(lang, "report.scrubbed"))
                    .font(theme::body(12.0))
                    .color(theme::TEXT_FAINT),
            );
            if let Some((tone, text)) = &f.result {
                status_line(ui, *tone, text);
                if let Some(url) = &f.issue_url {
                    ui.hyperlink_to(tr!(lang, "report.open_issue"), url);
                }
            } else if f.sending {
                status_line(ui, Tone::Notice, &tr!(lang, "settings.msg_saving"));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 8.0) / 2.0;
                if f.sent {
                    if cabinet(
                        ui,
                        vec2(w, 48.0),
                        &tr!(lang, "common.close"),
                        Cab::Plain,
                        false,
                    )
                    .clicked()
                    {
                        close = true;
                    }
                    return;
                }
                let send = ui.add_enabled_ui(!f.sending, |ui| {
                    cabinet(
                        ui,
                        vec2(w, 48.0),
                        &tr!(lang, "launcher.save_logs"),
                        Cab::Go,
                        true,
                    )
                });
                if send.inner.clicked() {
                    act = Some(Action::SendReport);
                }
                let cancel = ui.add_enabled_ui(!f.sending, |ui| {
                    cabinet(
                        ui,
                        vec2(w, 48.0),
                        &tr!(lang, "common.cancel"),
                        Cab::Plain,
                        false,
                    )
                });
                if cancel.inner.clicked() {
                    close = true;
                }
            });
        });
    // a report being saved keeps the dialog: where it went is still to come
    if close || (modal.should_close() && !f.sending) {
        st.report = None;
    }
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_needs_two_clicks_within_the_window() {
        let (armed, stop) = stop_click(0.0, 10.0);
        assert!(!stop && armed == 10.0 + STOP_ARM);
        assert_eq!(stop_click(armed, 12.0), (0.0, true));
        // lapsed: the click arms again
        assert_eq!(stop_click(armed, 18.5), (18.5 + STOP_ARM, false));
    }

    #[test]
    fn shops_listed_but_not_joinable_on_2017_18() {
        // a server's own arcade joins (by CONNECT); with shop rooms off a listed shop room does not
        let mut v = lobby_for(Edition::E1718);
        assert_eq!(
            (v.kind, v.grey_now(), v.grey[Kind::Shops as usize]),
            (Kind::Shops, None, None)
        );
        v.shops = LobbyView::sample(LobbyScenario::List).shops;
        assert!(!v.rows().is_empty() && v.view_only_now());
        assert!(v.joinable().is_some());
        v.shops.iter_mut().for_each(|r| r.room = true);
        assert!(v.joinable().is_none());
        v.shops.iter_mut().for_each(|r| r.room = false);
        // 2010-11 keeps joinable shops
        let mut w = lobby_for(Edition::E1011);
        w.shops = v.shops.clone();
        w.kind = Kind::Shops;
        assert!(!w.view_only_now() && w.joinable().is_some());
    }

    #[test]
    fn looking_at_online_keeps_the_play_mode() {
        let mut st = LauncherState::sample(Edition::E1011, Lang::En, Scenario::Ready);
        st.tab = Tab::Online;
        let ctx = egui::Context::default();
        theme::install(&ctx, Edition::E1011, &theme::font_dir()).unwrap();
        // the fonts take effect from the second frame
        for _ in 0..2 {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, &mut st);
            })
            .textures_delta
            .clear();
        }
        assert!(!st.online && st.tab == Tab::Online);
    }
}
