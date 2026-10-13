//! The in-game panel on its 1440×900 design sheet. The FORMATION BOARD owns the right strip (the last 580 points);
//! the middle holds the game picture at the top, the cabinet deck under it (tactics, SHOOT, KEEPER, KEY PLAYER, DATA,
//! the machine's buttons) and the bar of page buttons along the bottom. Pages (CATALOGUE, KEYS, SETTINGS, CLUB CARD)
//! open over the picture and the deck. The sheet scales by `min(H/900, W/1440)` through `pixels_per_point`; this code
//! works in sheet points, and a wider sheet widens the middle.

use super::Lang;
use super::board::{self, Board, Card, Position, Rarity};
use super::theme::{self, Edition};
use super::widgets::{
    self, Cab, Face, KeyState, Tone, action, cabinet, cabinet_face, caps, chip, field, key_cell,
    range_slider, segmented, stat_bar, status_line,
};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

pub const SIZE: egui::Vec2 = egui::vec2(1440.0, 900.0);
/// The formation strip's width, pinned to the right edge.
pub const BOARD_W: f32 = 580.0;
/// The strip's title row above the pitch.
const BOARD_HEAD: f32 = 34.0;
/// The bar of page buttons along the bottom of the middle.
const TABS_H: f32 = 60.0;
/// The cabinet deck under the picture keeps at least this; on wide sheets the picture stops growing first.
const DECK_MIN: f32 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Game,
    Catalogue,
    Keys,
    Settings,
    ClubCard,
}

impl Page {
    pub const ALL: [Page; 5] = [
        Page::Game,
        Page::Catalogue,
        Page::Keys,
        Page::Settings,
        Page::ClubCard,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            Page::Game => "game",
            Page::Catalogue => "catalogue",
            Page::Keys => "keys",
            Page::Settings => "settings",
            Page::ClubCard => "clubcard",
        }
    }
}

/// The 16 bindable actions, in the KEYS page's order: (text key, keyboard, controller, cabinet colour).
pub const ACTIONS: [(&str, &str, &str, Cab); 16] = [
    ("cab.start", "Enter", "Start", Cab::Plain),
    ("cab.press", "Q", "LB", Cab::Tactic),
    ("cab.shoot", "Space", "A", Cab::Shoot),
    ("cab.keeper", "K", "B", Cab::Keeper),
    ("cab.data", "D", "Y", Cab::Data),
    ("cab.key_player", "P", "X", Cab::KeyPlayer),
    ("cab.card", "C", "Back", Cab::Plain),
    ("cab.coin", "5", "", Cab::Plain),
    ("keys.up", "↑", "D-pad ↑", Cab::Tactic),
    ("keys.down", "↓", "D-pad ↓", Cab::Tactic),
    ("keys.left", "←", "D-pad ←", Cab::Tactic),
    ("keys.right", "→", "D-pad →", Cab::Tactic),
    ("keys.test", "F9", "", Cab::Plain),
    ("keys.service", "F10", "", Cab::Plain),
    ("keys.sw1", "1", "", Cab::Plain),
    ("keys.sw2", "2", "", Cab::Plain),
];

#[derive(Clone, Debug)]
pub struct Catalogue {
    pub search: String,
    /// 0 ALL, then GK DF MF FW.
    pub pos: usize,
    /// 0 ANY, then ALL-TIME LEGEND RARE SPECIAL REGULAR.
    pub rarity: usize,
    pub country: String,
    pub club: String,
    pub sort: usize,
    pub ranges: [(u8, u8); 6],
    pub total: (u8, u8),
    /// The card under the mouse, for the stats pane.
    pub focus: usize,
    pub message: Option<(Tone, &'static str)>,
}

#[derive(Clone, Debug)]
pub struct PanelState {
    pub edition: Edition,
    pub lang: Lang,
    pub page: Page,
    pub board: Board,
    pub hover_delay: f32,
    /// CENTRAL, COUNTER, L-SIDE, R-SIDE, PRESS: lit when the order is on.
    pub tactics: [bool; 5],
    pub online: bool,
    pub ping_ms: u32,
    pub message: Option<(Tone, &'static str)>,
    pub catalogue: Catalogue,
    pub keys: Vec<(String, String)>,
    pub capturing: Option<usize>,
    pub changed: Option<usize>,
    // settings
    pub next_online: bool,
    pub address: String,
    pub game_text: usize,
    pub compact: bool,
    pub hover_choice: usize,
    pub auto_reports: bool,
    pub restart_armed: bool,
    // club card
    pub club_pick: usize,
    /// The game's own frame as a texture of the panel DLL's painter; `None` draws the stand-in picture.
    pub picture: Option<egui::TextureId>,
}

impl PanelState {
    pub fn sample(edition: Edition, lang: Lang, page: Page) -> Self {
        let board = super::sample::board();
        PanelState {
            edition,
            lang,
            page,
            board,
            hover_delay: 1.0,
            tactics: [false, true, true, false, false],
            online: page != Page::Settings,
            ping_ms: 38,
            message: Some(match page {
                Page::Catalogue => (Tone::Done, "panel.msg_added"),
                Page::ClubCard => (Tone::Refused, "panel.msg_session_open"),
                _ => (Tone::Notice, "panel.msg_saved"),
            }),
            catalogue: Catalogue {
                search: "ital".into(),
                pos: 0,
                rarity: 0,
                country: "Italia".into(),
                club: String::new(),
                sort: 1,
                ranges: [(10, 20), (1, 20), (12, 20), (1, 20), (1, 20), (1, 20)],
                total: (70, 100),
                focus: 5,
                message: Some((Tone::Notice, "cat.msg_on_table")),
            },
            keys: ACTIONS
                .iter()
                .map(|(_, k, c, _)| ((*k).to_owned(), (*c).to_owned()))
                .collect(),
            capturing: (page == Page::Keys).then_some(2),
            changed: (page == Page::Keys).then_some(4),
            next_online: true,
            address: "192.168.1.20".into(),
            game_text: 1,
            compact: false,
            hover_choice: 1,
            auto_reports: true,
            restart_armed: page == Page::Settings,
            club_pick: 1,
            picture: None,
        }
    }
}

/// The centre of a formation slot on the sheet (for tests that drag cards).
pub fn board_slot_center(slot: usize) -> Pos2 {
    let r = Rect::from_min_max(pos2(SIZE.x - BOARD_W, 0.0), SIZE.to_pos2()).shrink(16.0);
    let pitch = board::pitch_rect(r.min + vec2(0.0, BOARD_HEAD), r.width());
    board::spot_rect(pitch, super::sample::board().spots[slot]).center()
}

pub fn show(ui: &mut Ui, st: &mut PanelState) {
    let lang = st.lang;
    egui::Panel::right("panel-board")
        .exact_size(BOARD_W)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new())
        .show(ui, |ui| board_strip(ui, st, lang));
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::WELL))
        .show(ui, |ui| middle(ui, st, lang));
}

// ---- the middle: picture, deck, page bar ----

fn middle(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let area = ui.max_rect();
    let tabs = area.with_min_y(area.bottom() - TABS_H);
    let above = area.with_max_y(tabs.top());
    // the picture keeps its aspect, as wide as the middle unless the deck would get too short
    let reserve = if st.compact { 60.0 } else { DECK_MIN };
    let h = (area.width() / 1.6).min(above.height() - reserve);
    // compact: no deck, so the picture sits in the middle with the message line under it
    let y = if st.compact {
        above.top() + (above.height() - reserve) / 2.0
    } else {
        above.top() + h / 2.0
    };
    let pic = Rect::from_center_size(pos2(area.center().x, y), vec2(h * 1.6, h));
    let deck = above.with_min_y(pic.bottom());
    let p = ui.painter();
    widgets::gradient(
        p,
        deck,
        0,
        Color32::from_rgb(0x12, 0x1C, 0x2E),
        theme::NIGHT,
        true,
    );
    p.rect_filled(tabs, 0, theme::NIGHT);
    for y in [deck.top(), tabs.top()] {
        p.line_segment(
            [pos2(area.left(), y + 0.5), pos2(area.right(), y + 0.5)],
            Stroke::new(1.0, theme::RIM),
        );
    }
    p.line_segment(
        [
            pos2(area.right() - 0.5, area.top()),
            pos2(area.right() - 0.5, area.bottom()),
        ],
        Stroke::new(1.0, theme::RIM),
    );
    game_picture(ui, pic, st.picture);
    if st.page == Page::Game
        && let Some((tone, key)) = st.message
    {
        let r = Rect::from_center_size(pos2(deck.center().x, deck.top() + 27.0), vec2(520.0, 34.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
            status_line(ui, tone, &tr!(lang, key))
        });
    }
    if !st.compact {
        cabinet_deck(ui, st, deck.with_min_y(deck.top() + 54.0), lang);
    }
    page_bar(ui, st, tabs, lang);
    if st.page != Page::Game {
        let mut pg = ui.new_child(egui::UiBuilder::new().max_rect(above));
        page(&mut pg, st, lang);
    }
}

/// The cabinet's controls under the picture, three columns centred: the tactic cross, the play buttons, the machine.
fn cabinet_deck(ui: &mut Ui, st: &mut PanelState, area: Rect, lang: Lang) {
    let (wa, wb, wc, gap) = (300.0, 300.0, 204.0, 16.0);
    let x0 = area.center().x - (wa + wb + wc + 2.0 * gap) / 2.0;
    let top = area.top();
    let put = |ui: &mut Ui, r: Rect, key: &str, kind: Cab, lit: bool| {
        ui.put(r, |ui: &mut Ui| {
            cabinet(ui, r.size(), &tr!(lang, key), kind, lit)
        })
    };
    let caps_at = |ui: &Ui, x: f32, key: &str| {
        ui.painter().text(
            pos2(x, top + 8.0),
            Align2::LEFT_CENTER,
            tr!(lang, key),
            theme::display(15.0),
            theme::TEXT_DIM,
        );
    };
    // the plus-shaped tactic cluster on its plate
    caps_at(ui, x0, "panel.tactics");
    let plate = Rect::from_min_size(pos2(x0, top + 20.0), vec2(wa, 196.0));
    ui.painter()
        .rect_filled(plate, theme::RADIUS_BIG, theme::WELL);
    ui.painter().rect_stroke(
        plate,
        theme::RADIUS_BIG,
        Stroke::new(1.0, theme::RIM),
        StrokeKind::Inside,
    );
    let cell = vec2(94.0, 60.0);
    let cells = [
        (0, (0.0, 0.0), "cab.central"),
        (1, (0.0, -1.0), "cab.counter"),
        (2, (-1.0, 0.0), "cab.l_side"),
        (3, (1.0, 0.0), "cab.r_side"),
        (4, (0.0, 1.0), "cab.press"),
    ];
    for (i, (dx, dy), key) in cells {
        let r = Rect::from_center_size(
            plate.center() + vec2(dx * (cell.x + 4.0), dy * (cell.y + 4.0)),
            cell,
        );
        if put(ui, r, key, Cab::Tactic, st.tactics[i]).clicked() {
            st.tactics[i] = !st.tactics[i];
        }
    }
    // the play buttons: SHOOT the biggest
    let xb = x0 + wa + gap;
    let half = (wb - 6.0) / 2.0;
    let y = top + 20.0;
    put(
        ui,
        Rect::from_min_size(pos2(xb, y), vec2(wb, 80.0)),
        "cab.shoot",
        Cab::Shoot,
        false,
    );
    put(
        ui,
        Rect::from_min_size(pos2(xb, y + 86.0), vec2(half, 58.0)),
        "cab.keeper",
        Cab::Keeper,
        false,
    );
    put(
        ui,
        Rect::from_min_size(pos2(xb + half + 6.0, y + 86.0), vec2(half, 58.0)),
        "cab.key_player",
        Cab::KeyPlayer,
        true,
    );
    put(
        ui,
        Rect::from_min_size(pos2(xb, y + 150.0), vec2(wb, 46.0)),
        "cab.data",
        Cab::Data,
        false,
    );
    // the machine
    let xc = xb + wb + gap;
    caps_at(ui, xc, "panel.machine");
    let half = (wc - 6.0) / 2.0;
    let row = |ui: &mut Ui, n: f32, keys: [&str; 2], enabled: bool| {
        for (k, key) in keys.into_iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(xc + k as f32 * (half + 6.0), y + n * 50.0),
                vec2(half, 44.0),
            );
            ui.add_enabled_ui(enabled || k == 0, |ui| put(ui, r, key, Cab::Plain, false));
        }
    };
    row(ui, 0.0, ["cab.start", "cab.card"], true);
    if st.edition == Edition::E1718 {
        // BACK sends nothing yet
        row(ui, 1.0, ["cab.coin", "cab.back"], false);
    }
}

/// The bottom bar: the edition, the four page buttons (lit while their page is open), the ping and the window key.
fn page_bar(ui: &mut Ui, st: &mut PanelState, bar: Rect, lang: Lang) {
    let r = bar.shrink2(vec2(12.0, 8.0));
    let p = ui.painter();
    widgets::mark(p, pos2(r.left() + 11.0, r.center().y), st.edition.accent());
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(
        pos2(r.left() + 30.0, r.center().y - 11.0),
        pos2(r.left() + 130.0, r.center().y + 11.0),
    )));
    widgets::badge(&mut b, st.edition);
    let pages = [
        (Page::Catalogue, "panel.add_card"),
        (Page::Keys, "panel.keys"),
        (Page::Settings, "panel.settings"),
        (Page::ClubCard, "panel.club_card"),
    ];
    let w = 124.0;
    for (i, (pg, key)) in pages.into_iter().enumerate() {
        let t = Rect::from_min_size(
            pos2(r.left() + 132.0 + i as f32 * (w + 6.0), r.top()),
            vec2(w, r.height()),
        );
        let open = st.page == pg;
        let resp = ui.put(t, |ui: &mut Ui| {
            cabinet(ui, t.size(), &tr!(lang, key), Cab::Tactic, open)
        });
        if resp.clicked() {
            st.page = if open { Page::Game } else { pg };
        }
    }
    // the right end: ping (online only) over the window key
    let right = Rect::from_min_max(
        pos2(r.left() + 132.0 + 4.0 * (w + 6.0) + 8.0, r.top()),
        r.max,
    );
    let p = ui.painter();
    if st.online {
        ping(
            p,
            Rect::from_min_size(right.min, vec2(right.width(), 18.0)),
            st.ping_ms,
            lang,
        );
    }
    let g = widgets::fit(
        p,
        &tr!(lang, "panel.f11"),
        theme::body(11.0),
        theme::TEXT_FAINT,
        right.width(),
    );
    p.galley(
        pos2(right.left(), right.bottom() - g.size().y),
        g,
        theme::TEXT_FAINT,
    );
}

fn strip_ground(ui: &Ui) {
    let r = ui.max_rect();
    widgets::gradient(
        ui.painter(),
        r,
        0,
        Color32::from_rgb(0x12, 0x1C, 0x2E),
        theme::NIGHT,
        true,
    );
}

/// The ping meter: five bars and the milliseconds.
fn ping(p: &egui::Painter, r: Rect, ms: u32, lang: Lang) {
    let good = match ms {
        0..=40 => 5,
        41..=80 => 4,
        81..=140 => 3,
        141..=250 => 2,
        _ => 1,
    };
    let col = if good >= 4 {
        theme::GREEN
    } else if good == 3 {
        theme::AMBER
    } else {
        theme::RED_TEXT
    };
    for i in 0..5 {
        let h = 6.0 + i as f32 * 3.0;
        let b = Rect::from_min_size(
            pos2(r.left() + i as f32 * 6.0, r.bottom() - h),
            vec2(4.0, h),
        );
        p.rect_filled(b, 1, if i < good { col } else { theme::RIM });
    }
    p.text(
        pos2(r.left() + 36.0, r.center().y + 2.0),
        Align2::LEFT_CENTER,
        tr!(lang, "panel.ping", ms = ms),
        theme::display(15.0),
        theme::TEXT_DIM,
    );
}

fn board_strip(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    strip_ground(ui);
    let r = ui.max_rect().shrink(16.0);
    let pitch = board::pitch_rect(r.min + vec2(0.0, BOARD_HEAD), r.width());
    let p = ui.painter();
    p.text(
        pos2(r.left(), r.top() + 12.0),
        Align2::LEFT_CENTER,
        tr!(lang, "board.title"),
        theme::display(24.0),
        theme::TEXT,
    );
    let count = st.board.slots.iter().flatten().count();
    p.text(
        pos2(r.right(), r.top() + 12.0),
        Align2::RIGHT_CENTER,
        format!("{count}/16"),
        theme::display(20.0),
        theme::TEXT_DIM,
    );
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pitch.min, r.max)));
    b.spacing_mut().item_spacing.y = 4.0;
    board::formation_board(&mut b, &mut st.board, st.hover_delay, lang);
    b.add_space(4.0);
    b.label(
        egui::RichText::new(tr!(lang, "board.hint"))
            .font(theme::body(12.0))
            .color(theme::TEXT_FAINT),
    );
}

// ---- the game picture ----

/// A neutral stand-in for the game's picture: diagonal hatching and its size.
fn game_picture(ui: &mut Ui, pic: Rect, picture: Option<egui::TextureId>) {
    let p = ui.painter();
    if let Some(tex) = picture {
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        p.image(tex, pic, uv, Color32::WHITE);
        return;
    }
    widgets::gradient(
        p,
        pic,
        0,
        Color32::from_rgb(0x1A, 0x22, 0x2E),
        Color32::from_rgb(0x10, 0x16, 0x1F),
        true,
    );
    // a neutral stand-in: diagonal hatching and the picture's size
    let clip = p.with_clip_rect(pic);
    let mut x = pic.left() - pic.height();
    while x < pic.right() {
        clip.line_segment(
            [pos2(x, pic.bottom()), pos2(x + pic.height(), pic.top())],
            Stroke::new(1.0, Color32::from_white_alpha(6)),
        );
        x += 28.0;
    }
    p.text(
        pic.center(),
        Align2::CENTER_CENTER,
        "1440 × 900",
        theme::display(40.0),
        Color32::from_white_alpha(28),
    );
    p.rect_stroke(
        pic,
        0,
        Stroke::new(1.0, Color32::from_white_alpha(18)),
        StrokeKind::Inside,
    );
}

fn page(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let r = ui.max_rect().shrink(12.0);
    ui.painter()
        .rect_filled(ui.max_rect(), 0, Color32::from_black_alpha(120));
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(r));
    egui::Frame::new()
        .fill(theme::DEEP)
        .stroke(Stroke::new(1.0, theme::RIM_HI))
        .corner_radius(theme::RADIUS_BIG)
        .inner_margin(egui::Margin::symmetric(20, 16))
        .shadow(egui::Shadow {
            offset: [0, 10],
            blur: 30,
            spread: 0,
            color: Color32::from_black_alpha(160),
        })
        .show(&mut ui, |ui| {
            ui.set_min_size(r.size() - vec2(40.0, 32.0));
            let title = match st.page {
                Page::Catalogue => tr!(lang, "cat.title"),
                Page::Keys => tr!(lang, "panel.keys"),
                Page::Settings => tr!(lang, "panel.settings"),
                _ => tr!(lang, "panel.club_card"),
            };
            ui.horizontal(|ui| {
                let (bar, _) = ui.allocate_exact_size(vec2(5.0, 26.0), Sense::hover());
                ui.painter().rect_filled(bar, 2, theme::AMBER);
                ui.label(
                    egui::RichText::new(title)
                        .font(theme::display(32.0))
                        .color(theme::TEXT),
                );
                widgets::badge(ui, st.edition);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if close(ui, lang).clicked() {
                        st.page = Page::Game;
                    }
                });
            });
            ui.add_space(6.0);
            match st.page {
                Page::Catalogue => catalogue(ui, st, lang),
                Page::Keys => keys(ui, st, lang),
                Page::Settings => settings(ui, st, lang),
                _ => club_card(ui, st, lang),
            }
        });
}

/// The page's close button, with its key.
fn close(ui: &mut Ui, lang: Lang) -> egui::Response {
    let text = tr!(lang, "panel.close");
    let g = ui
        .painter()
        .layout_no_wrap(text.clone(), theme::display(16.0), theme::TEXT_DIM);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 44.0, 32.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(
        r,
        theme::RADIUS,
        if resp.hovered() {
            theme::RAISED_HI
        } else {
            theme::RAISED
        },
    );
    p.rect_stroke(
        r,
        theme::RADIUS,
        Stroke::new(1.0, theme::RIM),
        StrokeKind::Inside,
    );
    let c = pos2(r.left() + 16.0, r.center().y);
    let s = Stroke::new(1.8, theme::TEXT);
    p.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], s);
    p.line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], s);
    p.galley(
        pos2(r.left() + 30.0, r.center().y - g.size().y / 2.0),
        g,
        theme::TEXT_DIM,
    );
    resp
}

// ---- CATALOGUE ----

const STATS: [&str; 6] = [
    "cat.off", "cat.def", "cat.tec", "cat.pow", "cat.spd", "cat.sta",
];

fn rarity_key(r: Rarity) -> &'static str {
    match r {
        Rarity::AllTime => "cat.all_time",
        Rarity::Legend => "cat.legend",
        Rarity::Rare => "cat.rare",
        Rarity::Special => "cat.special",
        Rarity::Regular => "cat.regular",
    }
}

fn catalogue(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let c = &mut st.catalogue;
    ui.spacing_mut().item_spacing = vec2(6.0, 8.0);
    ui.add(
        egui::TextEdit::singleline(&mut c.search)
            .hint_text(tr!(lang, "cat.search_hint"))
            .font(theme::body(16.0))
            .desired_width(f32::INFINITY)
            .margin(vec2(12.0, 8.0)),
    );
    let label_w = 110.0;
    ui.horizontal_wrapped(|ui| {
        field(ui, &tr!(lang, "cat.position"), label_w);
        if chip(ui, &tr!(lang, "cat.all"), c.pos == 0, None).clicked() {
            c.pos = 0;
        }
        for (i, p) in Position::ALL.iter().enumerate() {
            if chip(ui, p.code(), c.pos == i + 1, Some(p.color())).clicked() {
                c.pos = i + 1;
            }
        }
        ui.add_space(16.0);
        field(ui, &tr!(lang, "cat.country"), 80.0);
        combo(
            ui,
            "country",
            &mut c.country,
            &["Italia", "Hispania", "日本", "Nordland"],
            lang,
        );
        field(ui, &tr!(lang, "cat.club"), 56.0);
        combo(
            ui,
            "club",
            &mut c.club,
            &["Porto Azzurro", "Kitakaze", "Real Olmedo"],
            lang,
        );
    });
    ui.horizontal_wrapped(|ui| {
        field(ui, &tr!(lang, "cat.rarity"), label_w);
        if chip(ui, &tr!(lang, "cat.any"), c.rarity == 0, None).clicked() {
            c.rarity = 0;
        }
        for (i, r) in Rarity::ALL.iter().enumerate() {
            if chip(
                ui,
                &tr!(lang, rarity_key(*r)),
                c.rarity == i + 1,
                Some(r.rim()),
            )
            .clicked()
            {
                c.rarity = i + 1;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        field(ui, &tr!(lang, "cat.sort"), label_w);
        let sorts = [
            &tr!(lang, "cat.name"),
            &tr!(lang, "cat.total"),
            &tr!(lang, STATS[0]),
            &tr!(lang, STATS[1]),
            &tr!(lang, STATS[2]),
            &tr!(lang, STATS[3]),
            &tr!(lang, STATS[4]),
            &tr!(lang, STATS[5]),
        ];
        for (i, s) in sorts.iter().enumerate() {
            if chip(ui, s, c.sort == i, None).clicked() {
                c.sort = i;
            }
        }
    });
    // the stat ranges, two columns
    egui::Frame::new()
        .fill(theme::WELL)
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let w = (ui.available_width() - 24.0) / 2.0;
            egui::Grid::new("ranges")
                .num_columns(2)
                .spacing(vec2(24.0, 4.0))
                .show(ui, |ui| {
                    for i in 0..3 {
                        range_slider(
                            ui,
                            &tr!(lang, STATS[i * 2]),
                            &mut c.ranges[i * 2],
                            (1, 20),
                            w,
                        );
                        range_slider(
                            ui,
                            &tr!(lang, STATS[i * 2 + 1]),
                            &mut c.ranges[i * 2 + 1],
                            (1, 20),
                            w,
                        );
                        ui.end_row();
                    }
                    range_slider(ui, &tr!(lang, "cat.total"), &mut c.total, (60, 100), w);
                    ui.horizontal(|ui| {
                        ui.add_space(w - 110.0);
                        if action(ui, &tr!(lang, "cat.reset")).clicked() {
                            c.ranges = [(1, 20); 6];
                            c.total = (60, 100);
                        }
                    });
                    ui.end_row();
                });
        });
    // results and the stats pane
    let cards = &st.board.cards;
    let on_table: Vec<usize> = st.board.slots.iter().flatten().copied().collect();
    egui_extras::StripBuilder::new(ui)
        .size(egui_extras::Size::remainder())
        .size(egui_extras::Size::exact(250.0))
        .horizontal(|mut strip| {
            strip.cell(|ui| {
                ui.horizontal(|ui| {
                    caps(ui, &tr!(lang, "cat.results", n = cards.len()));
                });
                egui::ScrollArea::vertical()
                    .id_salt("results")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                        let size = vec2(84.0, 118.0);
                        let per_row = ((ui.available_width() + 10.0) / (size.x + 10.0))
                            .floor()
                            .max(1.0) as usize;
                        for row in cards.chunks(per_row).enumerate() {
                            ui.horizontal(|ui| {
                                for (j, card) in row.1.iter().enumerate() {
                                    let i = row.0 * per_row + j;
                                    let resp = board::catalogue_card(
                                        ui,
                                        card,
                                        size,
                                        i == c.focus,
                                        on_table.contains(&i),
                                    );
                                    if resp.hovered() {
                                        c.focus = i;
                                    }
                                }
                            });
                        }
                    });
            });
            strip.cell(|ui| stats_pane(ui, &cards[c.focus], c.message, lang));
        });
}

fn combo(ui: &mut Ui, id: &str, value: &mut String, options: &[&str], lang: Lang) {
    let all = tr!(lang, "cat.all");
    let shown = if value.is_empty() {
        all.clone()
    } else {
        value.clone()
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(150.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, String::new(), all);
            for o in options {
                ui.selectable_value(value, (*o).to_owned(), *o);
            }
        });
}

fn inferred_role(card: &Card) -> &'static str {
    let s = card.stats;
    match card.pos {
        Position::Gk => "cat.role_keeper",
        Position::Df if s[4] >= 15 => "cat.role_wingback",
        Position::Df => "cat.role_stopper",
        Position::Mf if s[2] >= 17 => "cat.role_playmaker",
        Position::Mf => "cat.role_box",
        Position::Fw if s[3] >= 16 => "cat.role_target",
        Position::Fw => "cat.role_poacher",
    }
}

fn stats_pane(ui: &mut Ui, card: &Card, message: Option<(Tone, &'static str)>, lang: Lang) {
    egui::Frame::new()
        .fill(theme::WELL)
        .corner_radius(theme::RADIUS_BIG)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(86.0, 120.0), Sense::hover());
                board::card_face(ui.painter(), r, card, board::Look::default());
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(tr!(lang, "cat.total"))
                            .font(theme::display(14.0))
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(
                        egui::RichText::new(card.total().to_string())
                            .font(theme::display(48.0))
                            .color(theme::AMBER),
                    );
                    ui.horizontal(|ui| {
                        chip(ui, card.pos.code(), true, Some(card.pos.color()));
                    });
                });
            });
            ui.label(
                egui::RichText::new(&card.name)
                    .font(theme::display(22.0))
                    .color(theme::TEXT),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} · {} · {}",
                    card.club, card.country, card.season
                ))
                .font(theme::body(12.0))
                .color(theme::TEXT_DIM),
            );
            ui.label(
                egui::RichText::new(tr!(
                    lang,
                    "cat.inferred",
                    role = tr!(lang, inferred_role(card))
                ))
                .font(theme::body(12.0))
                .italics()
                .color(theme::TEXT_DIM),
            );
            ui.add_space(4.0);
            let w = ui.available_width();
            for (k, v) in STATS.iter().zip(card.stats) {
                stat_bar(ui, &tr!(lang, *k), v, w);
            }
            ui.add_space(6.0);
            cabinet(ui, vec2(w, 44.0), &tr!(lang, "cat.add"), Cab::Go, true);
            if let Some((tone, key)) = message {
                status_line(ui, tone, &tr!(lang, key));
            }
        });
}

// ---- KEYS ----

fn keys(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    ui.label(egui::RichText::new(tr!(lang, "keys.hint")).color(theme::TEXT_DIM));
    status_line(
        ui,
        Tone::Done,
        &tr!(lang, "keys.controller_found", name = "Gamepad 1"),
    );
    ui.add_space(4.0);
    let cell_w = 200.0;
    egui::Grid::new("keys")
        .num_columns(4)
        .spacing(vec2(16.0, 6.0))
        .striped(false)
        .show(ui, |ui| {
            caps(ui, &tr!(lang, "keys.action"));
            ui.label("");
            caps(ui, &tr!(lang, "keys.keyboard"));
            caps(ui, &tr!(lang, "keys.controller"));
            ui.end_row();
            for i in 0..ACTIONS.len() {
                row(ui, st, lang, i, cell_w);
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    action(ui, &tr!(lang, "keys.defaults"));
}

fn row(ui: &mut Ui, st: &mut PanelState, lang: Lang, i: usize, cell_w: f32) {
    let (key, _, _, kind) = ACTIONS[i];
    let disabled = key == "cab.coin" && st.edition == Edition::E1011;
    let (r, _) = ui.allocate_exact_size(vec2(130.0, 30.0), Sense::hover());
    let lit = st.capturing == Some(i);
    cabinet_face(
        ui.painter(),
        r,
        &tr!(lang, key),
        kind,
        Face {
            lit,
            glow: lit as u8 as f32,
            press: 0.0,
            hover: 0.0,
            enabled: !disabled,
        },
    );
    ui.label("");
    let (k, c) = &st.keys[i];
    let state = |text: &str, cap: bool| {
        if cap {
            KeyState::Capturing
        } else if st.changed == Some(i) {
            KeyState::Changed
        } else if text.is_empty() {
            KeyState::Empty
        } else {
            KeyState::Bound
        }
    };
    let ks = state(k, lit);
    let cs = state(c, false);
    let (k, c) = (k.clone(), c.clone());
    let r1 = key_cell(ui, &k, ks, cell_w, lang);
    let r2 = key_cell(ui, &c, cs, cell_w, lang);
    if r1.clicked() || r2.clicked() {
        st.capturing = Some(i);
    }
    if r1.secondary_clicked() {
        st.keys[i].0.clear();
    }
    if r2.secondary_clicked() {
        st.keys[i].1.clear();
    }
}

// ---- SETTINGS ----

fn kv(ui: &mut Ui, k: &str, v: &str, dot: Option<Color32>) {
    ui.label(
        egui::RichText::new(k)
            .color(theme::TEXT_DIM)
            .font(theme::body(14.0)),
    );
    ui.horizontal(|ui| {
        if let Some(c) = dot {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, c);
        }
        ui.label(
            egui::RichText::new(v)
                .color(theme::TEXT)
                .font(theme::bold(14.0)),
        );
    });
    ui.end_row();
}

fn settings(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let accent = st.edition.accent();
    ui.columns(2, |cols| {
        let (a, b) = cols.split_at_mut(1);
        let left = &mut a[0];
        let right = &mut b[0];
        left.spacing_mut().item_spacing.y = 12.0;
        right.spacing_mut().item_spacing.y = 12.0;
        widgets::section(left, &tr!(lang, "settings.now"), accent, |ui| {
            egui::Grid::new("now")
                .num_columns(2)
                .spacing(vec2(16.0, 8.0))
                .striped(true)
                .min_col_width(150.0)
                .show(ui, |ui| {
                    kv(
                        ui,
                        &tr!(lang, "settings.server"),
                        &tr!(lang, "settings.server_local"),
                        Some(theme::GREEN),
                    );
                    kv(
                        ui,
                        &tr!(lang, "settings.session"),
                        &tr!(lang, "settings.session_none"),
                        Some(theme::TEXT_FAINT),
                    );
                    kv(ui, &tr!(lang, "settings.bad_endings"), "0", None);
                    kv(ui, &tr!(lang, "settings.last_saved"), "15:04", None);
                    kv(
                        ui,
                        &tr!(lang, "settings.backup"),
                        &tr!(lang, "settings.today_at", t = "14:58"),
                        None,
                    );
                    kv(ui, &tr!(lang, "settings.game_text"), "English", None);
                    kv(ui, &tr!(lang, "settings.version"), "Forza-WCCF 7.0.0", None);
                });
        });
        widgets::section(left, &tr!(lang, "settings.view"), accent, |ui| {
            let w = ui.available_width();
            field(ui, &tr!(lang, "settings.layout"), w);
            let mut i = st.compact as usize;
            if segmented(
                ui,
                &mut i,
                &[
                    &tr!(lang, "settings.cabinet"),
                    &tr!(lang, "settings.compact"),
                ],
                w,
            ) {
                st.compact = i == 1;
            }
            field(ui, &tr!(lang, "settings.hover"), w);
            segmented(ui, &mut st.hover_choice, &["0.5 s", "1 s", "2 s", "3 s"], w);
        });
        widgets::section(right, &tr!(lang, "settings.next"), accent, |ui| {
            let w = ui.available_width();
            field(ui, &tr!(lang, "launcher.where"), w);
            let mut i = st.next_online as usize;
            if segmented(
                ui,
                &mut i,
                &[
                    &tr!(lang, "launcher.this_pc"),
                    &tr!(lang, "launcher.online"),
                ],
                w,
            ) {
                st.next_online = i == 1;
            }
            if st.next_online {
                ui.add(
                    egui::TextEdit::singleline(&mut st.address)
                        .font(theme::mono(15.0))
                        .desired_width(w)
                        .margin(vec2(10.0, 7.0)),
                );
            }
            field(ui, &tr!(lang, "launcher.game_text"), w);
            segmented(
                ui,
                &mut st.game_text,
                &["English", "Italiano", &tr!(lang, "launcher.original_ja")],
                w,
            );
            ui.add_space(4.0);
            let label = if st.restart_armed {
                tr!(lang, "settings.restart_again", s = 6)
            } else {
                tr!(lang, "settings.restart")
            };
            if cabinet(
                ui,
                vec2(w, 48.0),
                &label,
                if st.restart_armed {
                    Cab::Tactic
                } else {
                    Cab::Plain
                },
                st.restart_armed,
            )
            .clicked()
            {
                st.restart_armed = !st.restart_armed;
            }
        });
        widgets::section(right, &tr!(lang, "settings.logs"), accent, |ui| {
            let w = ui.available_width();
            cabinet(
                ui,
                vec2(w, 44.0),
                &tr!(lang, "settings.send_logs"),
                Cab::Data,
                false,
            );
            status_line(
                ui,
                Tone::Done,
                &tr!(lang, "settings.sent", code = "7K4-QD2"),
            );
            widgets::toggle_row(
                ui,
                &mut st.auto_reports,
                &tr!(lang, "settings.auto_reports"),
                &tr!(lang, "settings.auto_reports_hint"),
            );
        });
    });
}

// ---- CLUB CARD ----

fn club_card(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let accent = st.edition.accent();
    egui_extras::StripBuilder::new(ui)
        .size(egui_extras::Size::exact(420.0))
        .size(egui_extras::Size::remainder())
        .horizontal(|mut strip| {
            strip.cell(|ui| {
                // the club card itself: landscape, as the physical card
                let (r, _) =
                    ui.allocate_exact_size(vec2(400.0, 400.0 * 58.0 / 81.0), Sense::hover());
                let p = ui.painter();
                let s = egui::Shadow {
                    offset: [0, 10],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(160),
                };
                p.add(s.as_shape(r, 14));
                widgets::gradient(
                    p,
                    r,
                    14,
                    accent.lerp_to_gamma(theme::DEEP, 0.35),
                    theme::DEEP.lerp_to_gamma(theme::WELL, 0.4),
                    false,
                );
                p.rect_stroke(r, 14, Stroke::new(1.5, accent), StrokeKind::Inside);
                // a chip contact, drawn
                let chip_r = Rect::from_min_size(r.min + vec2(24.0, 70.0), vec2(46.0, 36.0));
                widgets::gradient(
                    p,
                    chip_r,
                    6,
                    theme::GOLD.lerp_to_gamma(Color32::WHITE, 0.3),
                    theme::GOLD,
                    true,
                );
                for k in 1..3 {
                    let y = chip_r.top() + chip_r.height() * k as f32 / 3.0;
                    p.line_segment(
                        [pos2(chip_r.left(), y), pos2(chip_r.right(), y)],
                        Stroke::new(1.0, theme::INK.gamma_multiply(0.5)),
                    );
                }
                p.text(
                    r.min + vec2(24.0, 22.0),
                    Align2::LEFT_TOP,
                    tr!(lang, "club.label"),
                    theme::display(16.0),
                    Color32::from_white_alpha(170),
                );
                p.text(
                    r.min + vec2(24.0, 126.0),
                    Align2::LEFT_TOP,
                    "KITAKAZE ESTE",
                    theme::display(40.0),
                    Color32::WHITE,
                );
                p.text(
                    r.min + vec2(24.0, 172.0),
                    Align2::LEFT_TOP,
                    tr!(lang, "club.manager", name = "H. Sakuraba"),
                    theme::body(15.0),
                    theme::TEXT,
                );
                p.text(
                    r.right_bottom() + vec2(-20.0, -18.0),
                    Align2::RIGHT_BOTTOM,
                    st.edition.label(),
                    theme::display(22.0),
                    accent,
                );
                ui.add_space(14.0);
                status_line(ui, Tone::Notice, &tr!(lang, "club.session_open"));
                egui::Grid::new("club")
                    .num_columns(2)
                    .spacing(vec2(16.0, 8.0))
                    .striped(true)
                    .min_col_width(150.0)
                    .show(ui, |ui| {
                        kv(ui, &tr!(lang, "club.played"), "142", None);
                        kv(ui, &tr!(lang, "club.record"), "81 - 27 - 34", None);
                        kv(ui, &tr!(lang, "settings.last_saved"), "15:04", None);
                        kv(
                            ui,
                            &tr!(lang, "settings.bad_endings"),
                            "1",
                            Some(theme::AMBER),
                        );
                    });
            });
            strip.cell(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                caps(ui, &tr!(lang, "club.your_cards"));
                let clubs = [
                    ("KITAKAZE ESTE", "2026-10-12"),
                    ("PORTO AZZURRO FC", "2026-10-03"),
                    ("NORDHAVN UNITED", "2026-09-21"),
                ];
                for (i, (name, date)) in clubs.iter().enumerate() {
                    club_row(
                        ui,
                        name,
                        &tr!(lang, "club.last_played", d = *date),
                        i == 0,
                        st.club_pick == i,
                        &mut st.club_pick,
                        i,
                        lang,
                    );
                }
                club_row(
                    ui,
                    &tr!(lang, "club.new_card"),
                    &tr!(lang, "club.new_hint"),
                    false,
                    st.club_pick == 3,
                    &mut st.club_pick,
                    3,
                    lang,
                );
                ui.add_space(8.0);
                let w = ui.available_width();
                ui.add_enabled_ui(false, |ui| {
                    cabinet(
                        ui,
                        vec2(w, 48.0),
                        &tr!(lang, "settings.restart"),
                        Cab::Plain,
                        false,
                    )
                });
                if let Some((tone, key)) = st.message {
                    status_line(ui, tone, &tr!(lang, key));
                }
                if st.edition == Edition::E1011 {
                    ui.add_space(8.0);
                    cabinet(
                        ui,
                        vec2(w, 40.0),
                        &tr!(lang, "club.clear_bad"),
                        Cab::Stop,
                        false,
                    );
                }
            });
        });
}

#[allow(clippy::too_many_arguments)]
fn club_row(
    ui: &mut Ui,
    name: &str,
    sub: &str,
    current: bool,
    selected: bool,
    pick: &mut usize,
    i: usize,
    lang: Lang,
) {
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 54.0), Sense::click());
    if resp.clicked() {
        *pick = i;
    }
    let p = ui.painter();
    let fill = if selected {
        theme::AMBER.gamma_multiply(0.14)
    } else if resp.hovered() {
        theme::RAISED_HI
    } else {
        theme::RAISED
    };
    p.rect_filled(r, theme::RADIUS_BIG, fill);
    p.rect_stroke(
        r,
        theme::RADIUS_BIG,
        Stroke::new(
            if selected { 2.0 } else { 1.0 },
            if selected { theme::AMBER } else { theme::RIM },
        ),
        StrokeKind::Inside,
    );
    // a radio mark
    let c = pos2(r.left() + 22.0, r.center().y);
    p.circle_stroke(
        c,
        8.0,
        Stroke::new(
            1.5,
            if selected {
                theme::AMBER
            } else {
                theme::TEXT_DIM
            },
        ),
    );
    if selected {
        p.circle_filled(c, 4.5, theme::AMBER);
    }
    p.text(
        pos2(r.left() + 42.0, r.top() + 17.0),
        Align2::LEFT_CENTER,
        name,
        theme::display(20.0),
        theme::TEXT,
    );
    p.text(
        pos2(r.left() + 42.0, r.top() + 38.0),
        Align2::LEFT_CENTER,
        sub,
        theme::body(12.0),
        theme::TEXT_DIM,
    );
    if current {
        let t = tr!(lang, "club.in_use");
        let g = p.layout_no_wrap(t, theme::display(14.0), theme::INK);
        let b = Rect::from_min_size(
            pos2(r.right() - g.size().x - 26.0, r.center().y - 11.0),
            vec2(g.size().x + 14.0, 22.0),
        );
        p.rect_filled(b, 11, theme::GREEN);
        p.galley(b.center() - g.size() / 2.0, g, theme::INK);
    }
}

// ---- the widget sheet ----

pub const SHEET_SIZE: egui::Vec2 = egui::vec2(1200.0, 820.0);

/// State for the sheet that shows every widget state side by side.
#[derive(Clone, Debug)]
pub struct WidgetSheet {
    pub lang: Lang,
    pub seg: usize,
    pub range: (u8, u8),
    pub total: (u8, u8),
    pub toggles: [bool; 2],
    pub launcher_lang: Lang,
}

impl WidgetSheet {
    pub fn sample(lang: Lang) -> Self {
        WidgetSheet {
            lang,
            seg: 1,
            range: (8, 17),
            total: (60, 100),
            toggles: [true, false],
            launcher_lang: lang,
        }
    }
}

pub fn widget_sheet(ui: &mut Ui, ws: &mut WidgetSheet) {
    let lang = ws.lang;
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::NIGHT).inner_margin(20))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            // the title bar, at the launcher's width
            let (bar, _) = ui.allocate_exact_size(vec2(600.0, 44.0), Sense::hover());
            let mut tb = ui.new_child(egui::UiBuilder::new().max_rect(bar));
            widgets::title_bar(&mut tb, Edition::E1011, &mut ws.launcher_lang);
            caps(ui, "CABINET: IDLE · LIT · PRESSED · HOVER · DISABLED");
            ui.horizontal(|ui| {
                let kinds = [
                    (Cab::Tactic, "cab.central"),
                    (Cab::Shoot, "cab.shoot"),
                    (Cab::Keeper, "cab.keeper"),
                    (Cab::KeyPlayer, "cab.key_player"),
                    (Cab::Data, "cab.data"),
                    (Cab::Plain, "cab.start"),
                    (Cab::Go, "launcher.play"),
                ];
                for (kind, key) in kinds {
                    ui.vertical(|ui| {
                        let label = tr!(lang, key);
                        for f in [
                            Face {
                                lit: false,
                                glow: 0.0,
                                press: 0.0,
                                hover: 0.0,
                                enabled: true,
                            },
                            Face {
                                lit: true,
                                glow: 1.0,
                                press: 0.0,
                                hover: 0.0,
                                enabled: true,
                            },
                            Face {
                                lit: true,
                                glow: 1.0,
                                press: 1.0,
                                hover: 0.0,
                                enabled: true,
                            },
                            Face {
                                lit: false,
                                glow: 0.0,
                                press: 0.0,
                                hover: 1.0,
                                enabled: true,
                            },
                            Face {
                                lit: false,
                                glow: 0.0,
                                press: 0.0,
                                hover: 0.0,
                                enabled: false,
                            },
                        ] {
                            let (r, _) = ui.allocate_exact_size(vec2(112.0, 48.0), Sense::hover());
                            cabinet_face(ui.painter(), r, &label, kind, f);
                        }
                    });
                }
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    caps(ui, "CARDS");
                    ui.horizontal(|ui| {
                        let cards = super::sample::cards();
                        for i in [5, 1, 3] {
                            let (r, _) = ui.allocate_exact_size(vec2(100.0, 140.0), Sense::hover());
                            board::card_face(ui.painter(), r, &cards[i], board::Look::default());
                        }
                    });
                    ui.horizontal(|ui| {
                        let cards = super::sample::cards();
                        let looks = [
                            board::Look::default(),
                            board::Look {
                                lifted: true,
                                ..Default::default()
                            },
                            board::Look {
                                ghost: true,
                                ..Default::default()
                            },
                            board::Look {
                                target: true,
                                ..Default::default()
                            },
                            board::Look {
                                warn: true,
                                ..Default::default()
                            },
                            board::Look {
                                selected: true,
                                ..Default::default()
                            },
                        ];
                        for (j, look) in looks.into_iter().enumerate() {
                            ui.add_space(4.0);
                            let (r, _) = ui.allocate_exact_size(
                                board::CARD_SMALL + vec2(12.0, 16.0),
                                Sense::hover(),
                            );
                            board::card_face(
                                ui.painter(),
                                Rect::from_center_size(r.center(), board::CARD_SMALL),
                                &cards[6 + j],
                                look,
                            );
                        }
                    });
                });
            });
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(520.0);
                    status_line(ui, Tone::Done, &tr!(lang, "panel.msg_added"));
                    status_line(ui, Tone::Refused, &tr!(lang, "panel.msg_session_open"));
                    status_line(ui, Tone::Notice, &tr!(lang, "panel.msg_saved"));
                    segmented(
                        ui,
                        &mut ws.seg,
                        &[
                            &tr!(lang, "launcher.this_pc"),
                            &tr!(lang, "launcher.online"),
                        ],
                        520.0,
                    );
                    ui.horizontal(|ui| {
                        chip(ui, &tr!(lang, "cat.all"), true, None);
                        chip(ui, &tr!(lang, "cat.legend"), false, None);
                        for p in Position::ALL {
                            chip(ui, p.code(), p == Position::Mf, Some(p.color()));
                        }
                        widgets::badge(ui, Edition::E1011);
                        widgets::badge(ui, Edition::E1718);
                    });
                    widgets::steps(ui, 3, 7, 520.0);
                    widgets::progress(ui, 0.62, 520.0);
                    widgets::toggle_row(
                        ui,
                        &mut ws.toggles[0],
                        &tr!(lang, "settings.auto_reports"),
                        &tr!(lang, "settings.auto_reports_hint"),
                    );
                    widgets::toggle_row(
                        ui,
                        &mut ws.toggles[1],
                        &tr!(lang, "installer.opt_shortcuts"),
                        &tr!(lang, "installer.opt_shortcuts_hint"),
                    );
                });
                ui.add_space(20.0);
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    for (k, v) in STATS.iter().zip([4u8, 9, 12, 15, 18, 20]) {
                        stat_bar(ui, &tr!(lang, *k), v, 300.0);
                    }
                    range_slider(ui, &tr!(lang, "cat.off"), &mut ws.range, (1, 20), 300.0);
                    range_slider(ui, &tr!(lang, "cat.total"), &mut ws.total, (60, 100), 300.0);
                });
                ui.add_space(20.0);
                ui.vertical(|ui| {
                    for (t, s) in [
                        ("Space", KeyState::Bound),
                        ("", KeyState::Capturing),
                        ("", KeyState::Empty),
                        ("K", KeyState::Changed),
                    ] {
                        key_cell(ui, t, s, 200.0, lang);
                    }
                    for r in Rarity::ALL {
                        ui.horizontal(|ui| {
                            let (m, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                            board::rarity_mark(ui.painter(), m.center(), 18.0, r);
                            ui.label(
                                egui::RichText::new(tr!(lang, rarity_key(r)))
                                    .font(theme::display(16.0))
                                    .color(r.rim()),
                            );
                        });
                    }
                });
            });
        });
}
