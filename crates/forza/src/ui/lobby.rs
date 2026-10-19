//! The online lobby (ROADMAP, "Online lobby view"): the shops and VS rooms a server lists, one row each, drawn from
//! [`LobbyView`] alone. [`show`] reads no socket and keeps no thread: it returns the [`LobbyAction`] the player chose
//! and the caller does the rest. Milestone L0 runs on [`LobbyView::sample`] data.

use super::Lang;
use super::theme;
use super::widgets::{
    Cab, Tone, action, cabinet, caps, chip, fit, fold, section, segmented, status_line,
};
use egui::{Key, Modifiers, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// A window this view fits.
pub const SIZE: egui::Vec2 = egui::vec2(600.0, 625.0);
/// Rows are this tall at 1x.
pub const ROW_H: f32 = 32.0;
/// An answer older than this many seconds is stale: JOIN waits for the next one.
pub const STALE_S: u32 = 15;
/// The "ping < 150 ms" filter.
pub const PING_MAX_MS: u32 = 150;
const LIST_H: f32 = ROW_H * 6.0;

/// The two kinds of lobby, one list each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Shops,
    Vs,
}

/// What a row says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowState {
    Open,
    InMatch,
    Full,
    Closed,
    NoAnswer,
    /// A server running another Forza-WCCF version.
    NeedsVersion(String),
}

/// One shop or VS room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub players: u8,
    pub max: u8,
    pub ping_ms: Option<u32>,
    pub lang: Option<Lang>,
    pub state: RowState,
    /// Seconds to the next ENTRY.
    pub next_entry: Option<u32>,
}

impl Row {
    /// A seat can be reserved: open, or mid-cycle (the player then waits for the next ENTRY).
    pub fn joinable(&self) -> bool {
        matches!(self.state, RowState::Open | RowState::InMatch)
    }
}

/// Why a kind has no list in this edition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grey {
    /// 2017-18 shops wait for the LAN server.
    Shops1718,
    /// 2010-11 VS waits for live World Trophy matching.
    Vs1011,
}

/// A one-off message the caller sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// The seat went to someone else; the list was refreshed.
    Taken,
    /// The host closed the room.
    RoomClosed,
}

/// A player in a room, by random club id only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occupant {
    pub club: String,
    pub ready: bool,
}

/// The room a JOIN or HOST opened, before the start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    /// The shop's name or the VS room's code.
    pub title: String,
    pub kind: Kind,
    /// One entry per seat; `None` is an empty seat.
    pub seats: Vec<Option<Occupant>>,
    pub next_entry: Option<u32>,
}

/// What the player chose; the caller acts on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LobbyAction {
    Join(String),
    QuickMatch,
    HostVs,
    Refresh,
    Ready,
    Leave,
    /// The typed address, on Enter in the Direct address field.
    DirectAddress(String),
}

/// Everything the view draws.
#[derive(Clone, Debug)]
pub struct LobbyView {
    pub kind: Kind,
    pub shops: Vec<Row>,
    pub vs: Vec<Row>,
    /// Index into the visible rows of the current kind.
    pub selected: usize,
    /// 0 all, then English, Italian, Japanese.
    pub lang_filter: usize,
    pub open_only: bool,
    pub ping_ok: bool,
    /// Seconds since the answer, or `None` before the first one.
    pub updated_s: Option<u32>,
    pub room: Option<Room>,
    pub notice: Option<Notice>,
    /// The server stopped answering: the list is greyed and the Direct address fold opens.
    pub offline: bool,
    pub grey: [Option<Grey>; 2],
    pub address: String,
    pub direct_open: bool,
    /// Scroll the selected row into view on the next frame.
    pub scroll: bool,
}

/// Sample states for the snapshots and the gallery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    List,
    Empty,
    Full,
    Offline,
    RoomWait,
    Greyed,
}

impl Scenario {
    pub const ALL: [Scenario; 6] = [
        Scenario::List,
        Scenario::Empty,
        Scenario::Full,
        Scenario::Offline,
        Scenario::RoomWait,
        Scenario::Greyed,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            Scenario::List => "list",
            Scenario::Empty => "empty",
            Scenario::Full => "full",
            Scenario::Offline => "offline",
            Scenario::RoomWait => "room_wait",
            Scenario::Greyed => "greyed",
        }
    }
}

#[allow(clippy::too_many_arguments)] // sample data only
fn row(
    id: &str,
    name: &str,
    players: u8,
    max: u8,
    ping: Option<u32>,
    lang: Option<Lang>,
    state: RowState,
    next: Option<u32>,
) -> Row {
    Row {
        id: id.into(),
        name: name.into(),
        players,
        max,
        ping_ms: ping,
        lang,
        state,
        next_entry: next,
    }
}

impl LobbyView {
    /// A view with nothing in it.
    pub fn new() -> Self {
        LobbyView {
            kind: Kind::Shops,
            shops: vec![],
            vs: vec![],
            selected: 0,
            lang_filter: 0,
            open_only: false,
            ping_ok: false,
            updated_s: Some(4),
            room: None,
            notice: None,
            offline: false,
            grey: [None, None],
            address: "192.168.1.20".into(),
            direct_open: false,
            scroll: true,
        }
    }

    /// Made-up data for a snapshot or the gallery.
    pub fn sample(s: Scenario) -> Self {
        use Lang::{En, It, Ja};
        use RowState::*;
        let mut v = LobbyView::new();
        v.shops = vec![
            row(
                "k3f9q2",
                "FORZA the game server",
                3,
                8,
                Some(21),
                Some(It),
                Open,
                Some(32),
            ),
            row(
                "t8n2d1",
                "TOKYO NIGHT",
                8,
                8,
                Some(240),
                Some(Ja),
                Full,
                None,
            ),
            row(
                "m4c7u0",
                "MILANO CUP",
                5,
                8,
                Some(38),
                Some(It),
                InMatch,
                Some(580),
            ),
        ];
        v.vs = vec![
            row("r7x1", "K7QM", 1, 2, Some(33), Some(It), Open, None),
            row("r2b9", "TQ4D", 1, 2, Some(112), Some(En), Open, None),
            row("r5h3", "WZ8P", 2, 2, Some(61), Some(Ja), Full, None),
        ];
        match s {
            Scenario::List => {}
            Scenario::Empty => {
                v.kind = Kind::Vs;
                v.vs.clear();
            }
            Scenario::Full => {
                let names = [
                    "FORZA the game server",
                    "TOKYO NIGHT",
                    "MILANO CUP",
                    "OSAKA RING",
                    "ROMA 24",
                    "NAPOLI SUD",
                    "SHIBUYA CLUB",
                    "TORINO EST",
                    "KYOTO CORNER",
                    "BOLOGNA",
                    "NAGOYA SQ",
                    "PALERMO",
                ];
                v.shops = names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        let (state, seats, next) = match i % 4 {
                            0 => (Open, 2 + i as u8 % 5, Some(10 + 7 * i as u32)),
                            1 => (Full, 8, None),
                            2 => (InMatch, 4, Some(300 + 11 * i as u32)),
                            _ => (Closed, 0, None),
                        };
                        let lang = [It, Ja, En][i % 3];
                        row(
                            &format!("s{i}"),
                            n,
                            seats,
                            8,
                            Some(18 + 23 * i as u32),
                            Some(lang),
                            state,
                            next,
                        )
                    })
                    .collect();
                v.selected = 7;
            }
            Scenario::Offline => {
                v.updated_s = Some(40);
                v.offline = true;
                v.direct_open = true;
                for r in &mut v.shops {
                    r.state = NoAnswer;
                }
            }
            Scenario::RoomWait => {
                v.kind = Kind::Vs;
                v.room = Some(Room {
                    title: "K7QM".into(),
                    kind: Kind::Vs,
                    seats: vec![
                        Some(Occupant {
                            club: "A7K2".into(),
                            ready: true,
                        }),
                        None,
                    ],
                    next_entry: None,
                });
            }
            Scenario::Greyed => {
                v.grey[0] = Some(Grey::Shops1718);
                v.shops.clear();
            }
        }
        v
    }

    pub fn rows(&self) -> &[Row] {
        match self.kind {
            Kind::Shops => &self.shops,
            Kind::Vs => &self.vs,
        }
    }

    pub fn grey_now(&self) -> Option<Grey> {
        self.grey[self.kind as usize]
    }

    fn stale(&self) -> bool {
        self.updated_s.is_some_and(|s| s > STALE_S)
    }

    /// The rows the filters let through, as indexes into [`rows`](Self::rows).
    pub fn visible(&self) -> Vec<usize> {
        let want = [None, Some(Lang::En), Some(Lang::It), Some(Lang::Ja)][self.lang_filter.min(3)];
        self.rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                want.is_none_or(|l| r.lang == Some(l))
                    && (!self.open_only || r.state == RowState::Open)
                    && (!self.ping_ok || r.ping_ms.is_some_and(|p| p < PING_MAX_MS))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The selected row, when it can be joined now.
    pub fn joinable(&self) -> Option<&Row> {
        if self.offline || self.stale() || self.grey_now().is_some() {
            return None;
        }
        let vis = self.visible();
        let r = &self.rows()[*vis.get(self.selected)?];
        r.joinable().then_some(r)
    }

    fn set_kind(&mut self, k: Kind) {
        if self.kind != k {
            self.kind = k;
            self.selected = 0;
            self.scroll = true;
        }
    }
}

impl Default for LobbyView {
    fn default() -> Self {
        Self::new()
    }
}

/// `m:ss`.
pub fn clock(s: u32) -> String {
    format!("{}:{:02}", s / 60, s % 60)
}

/// The keys: Up/Down the row, Left/Right or Tab the kind, Enter join (ready in a room), F5 refresh, Esc leave.
fn keys(ui: &Ui, v: &mut LobbyView) -> Option<LobbyAction> {
    let pressed = |k: Key, m: Modifiers| ui.input_mut(|i| i.consume_key(m, k));
    let none = Modifiers::NONE;
    if pressed(Key::Escape, none) {
        return Some(LobbyAction::Leave);
    }
    if pressed(Key::F5, none) {
        return Some(LobbyAction::Refresh);
    }
    if v.room.is_some() {
        return pressed(Key::Enter, none).then_some(LobbyAction::Ready);
    }
    let n = v.visible().len();
    if pressed(Key::ArrowDown, none) && v.selected + 1 < n {
        v.selected += 1;
        v.scroll = true;
    }
    if pressed(Key::ArrowUp, none) && v.selected > 0 {
        v.selected -= 1;
        v.scroll = true;
    }
    if pressed(Key::ArrowLeft, none) {
        v.set_kind(Kind::Shops);
    }
    if pressed(Key::ArrowRight, none) {
        v.set_kind(Kind::Vs);
    }
    if pressed(Key::Tab, none) || pressed(Key::Tab, Modifiers::SHIFT) {
        v.set_kind(if v.kind == Kind::Shops {
            Kind::Vs
        } else {
            Kind::Shops
        });
    }
    if pressed(Key::Enter, none) {
        return v.joinable().map(|r| LobbyAction::Join(r.id.clone()));
    }
    None
}

/// Draws the lobby from `v` and returns what the player chose this frame.
pub fn show(ui: &mut Ui, v: &mut LobbyView, lang: Lang) -> Option<LobbyAction> {
    let mut act = if ui.ctx().egui_wants_keyboard_input() {
        None
    } else {
        keys(ui, v)
    };
    let title = tr!(lang, "lobby.title");
    section(ui, &title, theme::AMBER, |ui| {
        if v.room.is_some() {
            room_view(ui, v, lang, &mut act);
        } else {
            list_view(ui, v, lang, &mut act);
        }
    });
    ui.add_space(8.0);
    let summary = v.address.clone();
    let direct = tr!(lang, "lobby.direct");
    fold(
        ui,
        &direct,
        &summary,
        &mut v.direct_open,
        theme::BLUE,
        |ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut v.address)
                    .font(theme::mono(15.0))
                    .desired_width(ui.available_width())
                    .margin(vec2(10.0, 7.0)),
            );
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                act = Some(LobbyAction::DirectAddress(v.address.clone()));
            }
        },
    );
    act
}

fn state_text(lang: Lang, r: &Row) -> String {
    let t = |s: &str| match s {
        "open" => tr!(lang, "lobby.open"),
        "in_match" => tr!(lang, "lobby.in_match"),
        "full" => tr!(lang, "lobby.full"),
        "closed" => tr!(lang, "lobby.closed"),
        _ => tr!(lang, "lobby.no_answer"),
    };
    let time = r.next_entry.map(clock);
    match (&r.state, time) {
        (RowState::Open, Some(c)) => format!("{} · {c}", t("open")),
        (RowState::InMatch, Some(c)) => format!("{} {c}", t("in_match")),
        (RowState::Open, None) => t("open"),
        (RowState::InMatch, None) => t("in_match"),
        (RowState::Full, _) => t("full"),
        (RowState::Closed, _) => t("closed"),
        (RowState::NoAnswer, _) => t("no_answer"),
        (RowState::NeedsVersion(x), _) => tr!(lang, "lobby.needs_version", v = x),
    }
}

fn state_color(s: &RowState) -> egui::Color32 {
    match s {
        RowState::Open => theme::GREEN,
        RowState::InMatch => theme::AMBER,
        RowState::NoAnswer => theme::RED_TEXT,
        _ => theme::TEXT_FAINT,
    }
}

/// The columns of a row of width `w`: x offsets and widths of NAME, PLAYERS, PING, LANG, STATE.
fn columns(w: f32) -> [(f32, f32); 5] {
    let pad = 10.0;
    let (name, players, ping, lang) = (w * 0.34, w * 0.14, w * 0.14, w * 0.10);
    let x1 = pad + name;
    let x2 = x1 + players;
    let x3 = x2 + ping;
    let x4 = x3 + lang;
    [
        (pad, name - 8.0),
        (x1, players - 8.0),
        (x2, ping - 8.0),
        (x3, lang - 8.0),
        (x4, w - x4 - pad),
    ]
}

fn cell(ui: &Ui, rect: Rect, col: (f32, f32), text: &str, font: egui::FontId, c: egui::Color32) {
    let g = fit(ui.painter(), text, font, c, col.1);
    ui.painter().galley(
        pos2(rect.left() + col.0, rect.center().y - g.size().y / 2.0),
        g,
        c,
    );
}

fn list_view(ui: &mut Ui, v: &mut LobbyView, lang: Lang, act: &mut Option<LobbyAction>) {
    let w = ui.available_width();
    // the kind tabs, the age of the answer, refresh
    ui.horizontal(|ui| {
        let mut k = v.kind as usize;
        let labels = [
            format!("{} {}", tr!(lang, "lobby.shops"), v.shops.len()),
            format!("{} {}", tr!(lang, "lobby.vs"), v.vs.len()),
        ];
        if segmented(ui, &mut k, &[&labels[0], &labels[1]], w * 0.52) {
            v.set_kind(if k == 0 { Kind::Shops } else { Kind::Vs });
        }
        if action(ui, &tr!(lang, "lobby.refresh")).clicked() {
            *act = Some(LobbyAction::Refresh);
        }
        if let Some(s) = v.updated_s {
            ui.label(
                egui::RichText::new(tr!(lang, "lobby.updated", s = s))
                    .font(theme::body(12.0))
                    .color(if v.stale() {
                        theme::AMBER
                    } else {
                        theme::TEXT_DIM
                    }),
            );
        }
    });
    ui.add_space(6.0);
    // filters
    ui.horizontal(|ui| {
        let all = tr!(lang, "lobby.all_langs");
        if segmented(ui, &mut v.lang_filter, &[&all, "EN", "IT", "JA"], w * 0.44) {
            v.selected = 0;
        }
        if chip(ui, &tr!(lang, "lobby.open_only"), v.open_only, None).clicked() {
            v.open_only = !v.open_only;
            v.selected = 0;
        }
        if chip(ui, &tr!(lang, "lobby.ping_lt"), v.ping_ok, None).clicked() {
            v.ping_ok = !v.ping_ok;
            v.selected = 0;
        }
    });
    ui.add_space(8.0);
    // column heads
    let (head, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    let cols = columns(w);
    let name_key = if v.kind == Kind::Shops {
        "lobby.name"
    } else {
        "lobby.code"
    };
    for (i, key) in [
        name_key,
        "lobby.players",
        "lobby.ping",
        "lobby.lang",
        "lobby.state",
    ]
    .into_iter()
    .enumerate()
    {
        cell(
            ui,
            head,
            cols[i],
            &tr!(lang, key),
            theme::display(13.0),
            theme::TEXT_DIM,
        );
    }
    // the rows
    let dim = v.offline;
    let vis = v.visible();
    v.selected = v.selected.min(vis.len().saturating_sub(1));
    egui::ScrollArea::vertical()
        .id_salt("lobby_rows")
        .min_scrolled_height(LIST_H)
        .max_height(LIST_H)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(g) = v.grey_now() {
                let key = match g {
                    Grey::Shops1718 => "lobby.grey_shops_1718",
                    Grey::Vs1011 => "lobby.grey_vs_1011",
                };
                hint(ui, &tr!(lang, key));
                return;
            }
            if vis.is_empty() {
                hint(ui, &tr!(lang, "lobby.empty"));
                return;
            }
            let rw = ui.available_width();
            let cols = columns(rw);
            for (n, &i) in vis.iter().enumerate() {
                let r = &v.rows()[i].clone();
                let (rect, resp) = ui.allocate_exact_size(vec2(rw, ROW_H), Sense::click());
                let sel = n == v.selected;
                let off = dim || !r.joinable();
                resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, sel, &r.name)
                });
                if resp.clicked() {
                    v.selected = n;
                }
                if resp.double_clicked() && r.joinable() && !dim {
                    *act = Some(LobbyAction::Join(r.id.clone()));
                }
                if sel && std::mem::take(&mut v.scroll) {
                    resp.scroll_to_me(None);
                }
                let p = ui.painter();
                if sel {
                    p.rect_filled(rect, theme::RADIUS, theme::RAISED_HI);
                    p.rect_filled(
                        Rect::from_min_size(rect.min, vec2(3.0, ROW_H)),
                        1,
                        theme::AMBER,
                    );
                    p.rect_stroke(
                        rect,
                        theme::RADIUS,
                        Stroke::new(2.0, theme::BLUE),
                        StrokeKind::Inside,
                    );
                } else if resp.hovered() {
                    p.rect_filled(rect, theme::RADIUS, theme::RAISED.gamma_multiply(0.6));
                }
                let ink = if off { theme::TEXT_FAINT } else { theme::TEXT };
                let font = if v.kind == Kind::Vs {
                    theme::mono(15.0)
                } else {
                    theme::bold(14.0)
                };
                cell(ui, rect, cols[0], &r.name, font, ink);
                cell(
                    ui,
                    rect,
                    cols[1],
                    &format!("{}/{}", r.players, r.max),
                    theme::bold(14.0),
                    ink,
                );
                let ping = r.ping_ms.map_or("-".into(), |p| format!("{p} ms"));
                cell(
                    ui,
                    rect,
                    cols[2],
                    &ping,
                    theme::body(14.0),
                    if off {
                        theme::TEXT_FAINT
                    } else {
                        theme::TEXT_DIM
                    },
                );
                let l = r.lang.map_or(String::new(), |l| l.code().to_uppercase());
                cell(
                    ui,
                    rect,
                    cols[3],
                    &l,
                    theme::body(14.0),
                    if off {
                        theme::TEXT_FAINT
                    } else {
                        theme::TEXT_DIM
                    },
                );
                let sc = if dim {
                    theme::TEXT_FAINT
                } else {
                    state_color(&r.state)
                };
                cell(
                    ui,
                    rect,
                    cols[4],
                    &state_text(lang, r),
                    theme::bold(13.0),
                    sc,
                );
            }
        });
    ui.add_space(8.0);
    // one status line
    if let Some((tone, text)) = status(v, lang) {
        status_line(ui, tone, &text);
        ui.add_space(8.0);
    }
    // the three actions
    let can_join = v.joinable().is_some();
    let live = v.grey_now().is_none() && !v.offline;
    let gap = 8.0;
    let bw = (w - 2.0 * gap) / 3.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let size = vec2(bw, 40.0);
        let on = |ui: &mut Ui, ok: bool, label: String, kind: Cab| {
            ui.add_enabled_ui(ok, |ui| cabinet(ui, size, &label, kind, false))
                .inner
                .clicked()
        };
        if on(ui, live, tr!(lang, "lobby.quick"), Cab::Plain) {
            *act = Some(LobbyAction::QuickMatch);
        }
        if on(ui, can_join, tr!(lang, "lobby.join"), Cab::Go) {
            *act = v.joinable().map(|r| LobbyAction::Join(r.id.clone()));
        }
        if on(ui, live, tr!(lang, "lobby.host"), Cab::Plain) {
            *act = Some(LobbyAction::HostVs);
        }
    });
}

fn hint(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), LIST_H - 8.0), Sense::hover());
    let g = fit(
        ui.painter(),
        text,
        theme::body(14.0),
        theme::TEXT_DIM,
        r.width() - 40.0,
    );
    ui.painter()
        .galley(r.center() - g.size() / 2.0, g, theme::TEXT_DIM);
}

/// The one status line the state calls for, if any.
fn status(v: &LobbyView, lang: Lang) -> Option<(Tone, String)> {
    if v.offline {
        return Some((Tone::Refused, tr!(lang, "lobby.offline")));
    }
    match v.notice {
        Some(Notice::Taken) => return Some((Tone::Notice, tr!(lang, "lobby.taken"))),
        Some(Notice::RoomClosed) => return Some((Tone::Notice, tr!(lang, "lobby.room_closed"))),
        None => {}
    }
    if let (true, Some(s)) = (v.stale(), v.updated_s) {
        return Some((Tone::Notice, tr!(lang, "lobby.stale", s = s)));
    }
    if v.grey_now().is_none() && v.rows().is_empty() {
        return Some((Tone::Notice, tr!(lang, "lobby.empty")));
    }
    if let Some(r) = v.joinable()
        && r.state == RowState::InMatch
    {
        let t = clock(r.next_entry.unwrap_or(0));
        return Some((Tone::Notice, tr!(lang, "lobby.wait_entry", t = t)));
    }
    None
}

fn room_view(ui: &mut Ui, v: &mut LobbyView, lang: Lang, act: &mut Option<LobbyAction>) {
    let w = ui.available_width();
    let room = v.room.clone().expect("room_view runs with a room");
    caps(
        ui,
        &tr!(
            lang,
            if room.kind == Kind::Vs {
                "lobby.vs"
            } else {
                "lobby.shops"
            }
        ),
    );
    ui.label(
        egui::RichText::new(&room.title)
            .font(if room.kind == Kind::Vs {
                theme::mono(26.0)
            } else {
                theme::display(26.0)
            })
            .color(theme::TEXT),
    );
    ui.add_space(8.0);
    for (n, seat) in room.seats.iter().enumerate() {
        let (rect, _) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, theme::RADIUS, theme::WELL);
        cell(
            ui,
            rect,
            (10.0, 20.0),
            &(n + 1).to_string(),
            theme::bold(14.0),
            theme::TEXT_DIM,
        );
        let c = pos2(rect.left() + 48.0, rect.center().y);
        let (text, ink) = match seat {
            Some(o) => {
                let col = if o.ready { theme::GREEN } else { theme::AMBER };
                ui.painter().circle_filled(c, 6.0, col);
                let mut t = tr!(lang, "lobby.club", id = o.club);
                if o.ready {
                    t = format!("{t}  {}", tr!(lang, "lobby.ready"));
                }
                (t, theme::TEXT)
            }
            None => {
                ui.painter()
                    .circle_stroke(c, 6.0, Stroke::new(1.5, theme::TEXT_FAINT));
                (tr!(lang, "lobby.waiting"), theme::TEXT_DIM)
            }
        };
        cell(ui, rect, (64.0, w - 80.0), &text, theme::body(14.0), ink);
        ui.add_space(4.0);
    }
    ui.add_space(6.0);
    let all_ready = !room.seats.is_empty()
        && room
            .seats
            .iter()
            .all(|s| s.as_ref().is_some_and(|o| o.ready));
    let line = if all_ready && room.kind == Kind::Vs {
        Some((Tone::Done, tr!(lang, "lobby.starting")))
    } else {
        room.next_entry.map(|s| {
            let t = clock(s);
            (Tone::Notice, tr!(lang, "lobby.next_entry", t = t))
        })
    };
    if let Some((tone, text)) = line {
        status_line(ui, tone, &text);
        ui.add_space(8.0);
    }
    if let Some(Notice::RoomClosed) = v.notice {
        status_line(ui, Tone::Notice, &tr!(lang, "lobby.room_closed"));
        ui.add_space(8.0);
    }
    let gap = 8.0;
    let size = vec2((w - gap) / 2.0, 40.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        if cabinet(ui, size, &tr!(lang, "lobby.ready"), Cab::Go, false).clicked() {
            *act = Some(LobbyAction::Ready);
        }
        if cabinet(ui, size, &tr!(lang, "lobby.leave"), Cab::Stop, false).clicked() {
            *act = Some(LobbyAction::Leave);
        }
    });
}
