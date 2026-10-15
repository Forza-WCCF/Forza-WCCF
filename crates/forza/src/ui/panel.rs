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
use crate::cards::{self, AddRefused, Placed};
use crate::club_view::{self as forza_club, ClubView};
use crate::panel_txt::{PanelFile, valid_address};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::sync::Arc;

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

/// The 16 bindable actions, in the KEYS page's order (`forza::keys::NAMES`): (text key, cabinet colour).
pub const ACTIONS: [(&str, Cab); 16] = [
    ("cab.start", Cab::Plain),
    ("cab.press", Cab::Tactic),
    ("cab.shoot", Cab::Shoot),
    ("cab.keeper", Cab::Keeper),
    ("cab.data", Cab::Data),
    ("cab.key_player", Cab::KeyPlayer),
    ("cab.card", Cab::Plain),
    ("cab.coin", Cab::Plain),
    ("cab.central", Cab::Tactic),
    ("cab.counter", Cab::Tactic),
    ("cab.l_side", Cab::Tactic),
    ("cab.r_side", Cab::Tactic),
    ("keys.test", Cab::Plain),
    ("keys.service", Cab::Plain),
    ("keys.sw1", Cab::Plain),
    ("keys.sw2", Cab::Plain),
];

/// What the catalogue lists: the search line, the filters and the sort (`wccfpanel.c` `run_search_locked`).
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub search: String,
    /// 0 ALL, then GK DF MF FW.
    pub pos: usize,
    /// 0 ANY, then ALL-TIME LEGEND RARE SPECIAL REGULAR.
    pub rarity: usize,
    /// "" = any.
    pub country: String,
    pub club: String,
    /// 0 NAME, 1 TOTAL, then OFF DEF TEC POW SPD STA.
    pub sort: usize,
    pub ranges: [(u8, u8); 6],
    pub total: (u8, u8),
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            search: String::new(),
            pos: 0,
            rarity: 0,
            country: String::new(),
            club: String::new(),
            sort: 0,
            ranges: [(1, 20); 6],
            total: (60, 100),
        }
    }
}

/// The catalogue's words for the search, worked out once: each card's name, short name, club, country and season
/// folded ([`fold`]), the cards in name order, and the countries and clubs A to Z with their numbers of cards.
#[derive(Clone, Debug, Default)]
pub struct Index {
    folded: Vec<String>,
    order: Vec<usize>,
    pub countries: Vec<(String, usize)>,
    pub clubs: Vec<(String, usize)>,
}

/// Plain lower-case letters: accents dropped and other scripts spelt out, so "kaka" finds KAKÀ and "bartoli" BÀRTOLI.
pub fn fold(s: &str) -> String {
    deunicode::deunicode(s).to_lowercase()
}

impl Index {
    pub fn new(cards: &[Card]) -> Index {
        let folded: Vec<String> = cards
            .iter()
            .map(|c| {
                fold(
                    &[&c.name, &c.short, &c.club, &c.country, &c.season]
                        .map(|s| s.as_str())
                        .join("\n"),
                )
            })
            .collect();
        // the C panel's catalogue order: by the shown name, then the number
        let mut order: Vec<usize> = (0..cards.len()).collect();
        order.sort_by_cached_key(|&i| (fold(cards[i].short_name()), cards[i].number));
        let picks = |of: fn(&Card) -> &str| {
            let mut v: Vec<(String, usize)> = Vec::new();
            for c in cards {
                let name = of(c);
                if name.is_empty() {
                    continue;
                }
                match v.iter_mut().find(|(n, _)| n == name) {
                    Some((_, k)) => *k += 1,
                    None => v.push((name.to_owned(), 1)),
                }
            }
            v.sort_by_cached_key(|(n, _)| fold(n));
            v
        };
        Index {
            folded,
            order,
            countries: picks(|c| &c.country),
            clubs: picks(|c| &c.club),
        }
    }
}

/// A role search word ("dmf") lists the cards rated at least this in that role.
const ROLE_GOOD: u8 = 8;

/// One search word (folded) against one card: in its name, club, country or season; its line exactly ("fw"); a role
/// it is rated [`ROLE_GOOD`] or more in ("dmf"); digits also match the start of the card number.
fn word_matches(c: &Card, folded: &str, w: &str) -> bool {
    (w.bytes().all(|b| b.is_ascii_digit()) && c.number.to_string().starts_with(w))
        || folded.contains(w)
        || c.pos.code().eq_ignore_ascii_case(w)
        || board::ROLES
            .iter()
            .zip(c.aptitude.roles)
            .any(|(r, v)| v >= ROLE_GOOD && r.eq_ignore_ascii_case(w))
}

/// The cards that pass the filter, in its sort's order: only digits = the card numbers starting so; otherwise every
/// word must match ([`word_matches`]): "milan 2004-05", "brazil fw", "italy cb". NAME is the catalogue's name order;
/// a stat sorts best first, ties by TOTAL, then by name.
pub fn search(cards: &[Card], index: &Index, f: &Filter) -> Vec<usize> {
    let q = f.search.trim();
    let digits = !q.is_empty() && q.bytes().all(|b| b.is_ascii_digit());
    let words: Vec<String> = q.split_whitespace().map(fold).collect();
    let mut out: Vec<usize> = index
        .order
        .iter()
        .copied()
        .filter(|&i| {
            let c = &cards[i];
            (f.pos == 0 || Position::ALL.get(f.pos - 1) == Some(&c.pos))
                && (f.rarity == 0 || Rarity::ALL.get(f.rarity - 1) == Some(&c.rarity))
                && (f.country.is_empty() || c.country == f.country)
                && (f.club.is_empty() || c.club == f.club)
                && c.stats
                    .iter()
                    .zip(f.ranges)
                    .all(|(v, (lo, hi))| (lo..=hi).contains(v))
                && (f.total.0..=f.total.1).contains(&c.total)
                && if digits {
                    c.number.to_string().starts_with(q)
                } else {
                    words.iter().all(|w| word_matches(c, &index.folded[i], w))
                }
        })
        .collect();
    if f.sort > 0 {
        let key = |i: usize| match f.sort {
            1 => cards[i].total,
            s => cards[i].stats[(s - 2).min(5)],
        };
        out.sort_by(|&a, &b| {
            key(b)
                .cmp(&key(a))
                .then(cards[b].total.cmp(&cards[a].total))
        });
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct Catalogue {
    pub filter: Filter,
    /// The card in the stats pane (an index into the catalogue): the last one pointed at.
    pub focus: Option<usize>,
    /// The cards the filter lets through, in its order ([`search`]), worked out again when it changes.
    pub results: Vec<usize>,
    searched: Option<Filter>,
    index: Option<Index>,
}

impl Catalogue {
    /// The index made once and the search run again when the filter changed; `true` = the results changed.
    fn refresh(&mut self, cards: &[Card]) -> bool {
        let index = self.index.get_or_insert_with(|| Index::new(cards));
        if self.searched.as_ref() == Some(&self.filter) {
            return false;
        }
        self.results = search(cards, index, &self.filter);
        self.searched = Some(self.filter.clone());
        true
    }
}

/// A message line (done, refused or failed, notice), shown for [`NOTE_SECS`] from the frame it first appears.
#[derive(Clone, Debug)]
pub struct Note {
    pub tone: Tone,
    pub text: String,
    shown: Option<f64>,
}

const NOTE_SECS: f64 = 4.0;

impl Note {
    pub fn new(tone: Tone, text: String) -> Note {
        Note {
            tone,
            text,
            shown: None,
        }
    }
}

/// The link to the game server (this process's TCP link to its control port).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Link {
    #[default]
    Unknown,
    Down,
    /// Up, to this address (127.0.0.1: this PC).
    Up(String),
}

/// The club card's session as `launcher::card::Session` reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CardState {
    #[default]
    None,
    Unreadable,
    New,
    Open,
    Cut,
    Closed,
}

/// A file time as SETTINGS shows it: the local time ("15:04", or the date when older than a day) and its age in seconds.
pub type When = (String, u64);

/// SETTINGS' NOW, read by the panel DLL.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub link: Link,
    pub card: CardState,
    pub bad: Option<u32>,
    pub saved: Option<When>,
    /// The newest backup and how many are kept.
    pub backup: Option<(When, usize)>,
    /// The game's text now: "en", "it", "" (the original Japanese); `None` until read.
    pub text: Option<&'static str>,
    pub version: String,
    /// SEND LOGS' answer from the run: "sending", "ok CODE", "fail WHY".
    pub logs: Option<String>,
}

/// [`When`] of a file time.
pub fn when(t: std::time::SystemTime, now: std::time::SystemTime) -> When {
    let age = now.duration_since(t).map_or(0, |d| d.as_secs());
    let local = time::OffsetDateTime::from(t)
        .to_offset(time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC));
    let text = if age < 86400 {
        format!("{:02}:{:02}", local.hour(), local.minute())
    } else {
        format!(
            "{}-{:02}-{:02} {:02}:{:02}",
            local.year(),
            u8::from(local.month()),
            local.day(),
            local.hour(),
            local.minute()
        )
    };
    (text, age)
}

#[derive(Clone, Debug)]
pub struct PanelState {
    pub edition: Edition,
    pub lang: Lang,
    pub page: Page,
    pub board: Board,
    /// The player's catalogue (the CATALOGUE page's cards).
    pub cards: Arc<[Card]>,
    pub hover_delay: f32,
    /// CENTRAL, COUNTER, L-SIDE, R-SIDE, PRESS: lit when the order is on.
    pub tactics: [bool; 5],
    pub online: bool,
    pub ping_ms: u32,
    /// The last thing done, refused or worth knowing; one line for every page.
    pub message: Option<Note>,
    pub catalogue: Catalogue,
    /// Each action's key (virtual-key code, 0 none) and controller control (`forza::keys`), in [`ACTIONS`] order.
    pub keys: [u32; 16],
    pub pads: [u32; 16],
    /// The action whose KEY cell waits for a key; whose CONTROLLER cell waits for a control.
    pub capturing: Option<usize>,
    pub pad_capturing: Option<usize>,
    /// The action changed last (its cells lit).
    pub changed: Option<usize>,
    /// The controllers found (KEYS).
    pub pad_names: Vec<String>,
    // settings
    /// data\panel.txt's choices: the next start, the view.
    pub file: PanelFile,
    /// The address line while typing.
    pub address: String,
    /// What SETTINGS shows under NOW, read about once a second while it is open.
    pub status: Status,
    /// RESTART NOW / SEND LOGS armed by a first click until this time (egui's clock); asked for (waiting for the run).
    pub restart_armed: f64,
    pub restart_sent: bool,
    pub logs_armed: f64,
    pub logs_sent: bool,
    // club card
    /// data\club_view.txt as last read.
    pub club: ClubView,
    /// The choice armed by a first click, until this time (egui's clock).
    pub club_armed: Option<(ClubPick, f64)>,
    /// A choice asked for by its second click: data\panel.txt's card= or card_fix=, then RESTART NOW.
    pub club_request: Option<ClubPick>,
    /// The game's own frame as a texture of the panel DLL's painter; `None` draws the stand-in picture.
    pub picture: Option<egui::TextureId>,
    // what a frame hands the panel DLL
    /// Where the game's picture was drawn while no page covers it: clicks there go to the game.
    pub picture_rect: Option<Rect>,
    /// The board changed this frame (a card moved, swapped or taken off): the card table is written.
    pub board_changed: bool,
    /// A cabinet button pressed this frame: its action's name in keys.txt (START, PRESS, SHOOT, UP, ...).
    pub pressed: Option<&'static str>,
    /// The cabinet button whose key is down (lit until the key goes up).
    pub held: Option<&'static str>,
    /// A catalogue card to put on the table (its number): [`add_card`].
    pub add: Option<u32>,
    /// The bindings changed: keys.txt is written.
    pub keys_changed: bool,
    /// The choices changed: data\panel.txt is written.
    pub file_changed: bool,
    /// RESTART NOW asked (data\restart.request); SEND LOGS asked (data\sendlogs.request).
    pub restart: bool,
    pub send_logs: bool,
    // what the panel DLL hands a frame
    /// Keys pressed since the last frame while a page is open (virtual-key code, Alt held), repeats left out.
    pub vk_down: Vec<(u32, bool)>,
    /// A controller control pressed since the last frame (KEYS open).
    pub pad_down: Option<u32>,
}

impl PanelState {
    /// The panel at rest: the game page, no message, the default keys, the board and the catalogue given.
    pub fn new(edition: Edition, lang: Lang, board: Board, cards: Arc<[Card]>) -> Self {
        PanelState {
            edition,
            lang,
            page: Page::Game,
            board,
            cards,
            hover_delay: 1.0,
            tactics: [false; 5],
            online: false,
            ping_ms: 0,
            message: None,
            catalogue: Catalogue::default(),
            keys: crate::keys::defaults(edition == Edition::E1718),
            pads: [0; 16],
            capturing: None,
            pad_capturing: None,
            changed: None,
            pad_names: Vec::new(),
            file: PanelFile::default(),
            address: String::new(),
            status: Status::default(),
            restart_armed: 0.0,
            restart_sent: false,
            logs_armed: 0.0,
            logs_sent: false,
            club: ClubView::default(),
            club_armed: None,
            club_request: None,
            picture: None,
            picture_rect: None,
            board_changed: false,
            pressed: None,
            held: None,
            add: None,
            keys_changed: false,
            file_changed: false,
            restart: false,
            send_logs: false,
            vk_down: Vec::new(),
            pad_down: None,
        }
    }

    /// A message for the line every page shows.
    pub fn note(&mut self, tone: Tone, text: String) {
        self.message = Some(Note::new(tone, text));
    }

    /// A message by its text key, in the panel's language.
    pub fn say(&mut self, tone: Tone, key: &str) {
        self.note(tone, tr!(self.lang, key));
    }

    pub fn sample(edition: Edition, lang: Lang, page: Page) -> Self {
        let msg = match page {
            Page::Catalogue => (Tone::Done, tr!(lang, "panel.msg_added")),
            Page::ClubCard => (Tone::Refused, tr!(lang, "panel.msg_session_open")),
            _ => (Tone::Notice, tr!(lang, "panel.msg_saved")),
        };
        PanelState {
            page,
            tactics: [false, true, true, false, false],
            online: page != Page::Settings,
            ping_ms: 38,
            message: Some(Note::new(msg.0, msg.1)),
            catalogue: Catalogue {
                filter: Filter {
                    search: "ital".into(),
                    country: "Italia".into(),
                    sort: 1,
                    ranges: [(10, 20), (1, 20), (12, 20), (1, 20), (1, 20), (1, 20)],
                    total: (70, 100),
                    ..Filter::default()
                },
                ..Catalogue::default()
            },
            capturing: (page == Page::Keys).then_some(2),
            changed: (page == Page::Keys).then_some(4),
            pads: [
                0x10A, 0x105, 0x101, 0x102, 0x104, 0x103, 0x109, 0, 0x300, 0x302, 0x303, 0x301, 0,
                0, 0, 0,
            ],
            pad_names: vec!["Gamepad 1".to_owned()],
            file: PanelFile {
                online: true,
                server: "192.168.1.20".into(),
                english: Some("it".into()),
                ..PanelFile::default()
            },
            address: "192.168.1.20".into(),
            status: Status {
                link: Link::Up("192.168.1.20".into()),
                card: CardState::Closed,
                bad: Some(0),
                saved: Some(("15:04".into(), 360)),
                backup: Some((("14:58".into(), 720), 3)),
                text: Some("en"),
                version: "7.0.0".into(),
                logs: Some("ok 7K4-QD2".into()),
            },
            restart_armed: if page == Page::Settings {
                f64::INFINITY
            } else {
                0.0
            },
            club: super::sample::club_view(),
            held: Some("KEYPLAYER"), // lit on the design sheets
            ..Self::new(
                edition,
                lang,
                super::sample::board(),
                super::sample::cards().into(),
            )
        }
    }
}

/// The catalogue's card `no` put on the table where the C panel puts it ([`cards::add`]); the message says how it
/// went (added, already on the table, the table full). `true` = the table changed: the caller writes it.
pub fn add_card(st: &mut PanelState, placed: &mut Vec<Placed>, no: u32) -> bool {
    let lang = st.lang;
    let Some(card) = st.cards.iter().find(|c| c.number == no) else {
        return false;
    };
    let name = card.short_name().to_owned();
    let (tone, text, added) = match cards::add(placed, no, card.pos) {
        Ok(_) => (
            Tone::Done,
            tr!(lang, "cat.msg_added", name = name, n = placed.len()),
            true,
        ),
        Err(AddRefused::AlreadyOn) => (
            Tone::Notice,
            tr!(lang, "cat.msg_on_table", name = name),
            false,
        ),
        Err(AddRefused::Full) => (Tone::Refused, tr!(lang, "cat.msg_full"), false),
        Err(AddRefused::NoPlace) => (Tone::Refused, tr!(lang, "cat.msg_no_place"), false),
    };
    if added {
        st.board = cards::board(placed, &st.cards);
    }
    st.note(tone, text);
    added
}

/// The centre of a formation slot (0-10 the pitch, 11-15 the bench) on a sheet `width` points wide whose board has
/// these spots (for tests that drag cards).
pub fn board_slot_center(width: f32, spots: &[Pos2; 11], slot: usize) -> Pos2 {
    let r = Rect::from_min_max(pos2(width - BOARD_W, 0.0), pos2(width, SIZE.y)).shrink(16.0);
    let pitch = board::pitch_rect(r.min + vec2(0.0, BOARD_HEAD), r.width());
    board::slot_rects(pitch, pitch.bottom() + 12.0, spots)[slot].center()
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
    let reserve = if st.file.compact { 60.0 } else { DECK_MIN };
    let h = (area.width() / 1.6).min(above.height() - reserve);
    // compact: no deck, so the picture sits in the middle with the message line under it
    let y = if st.file.compact {
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
    st.picture_rect = (st.page == Page::Game).then_some(pic);
    if st.page == Page::Game {
        let r = Rect::from_center_size(pos2(deck.center().x, deck.top() + 27.0), vec2(520.0, 34.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
            note_line(ui, &mut st.message)
        });
    }
    if !st.file.compact {
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
    // a press sends the action's key down (the panel DLL lets it up after the release); lit while the key is down
    let held = st.held;
    let mut pressed = None;
    let mut put =
        |ui: &mut Ui, r: Rect, key: &str, action: Option<&'static str>, kind: Cab, lit: bool| {
            let resp = ui.put(r, |ui: &mut Ui| {
                cabinet(
                    ui,
                    r.size(),
                    &tr!(lang, key),
                    kind,
                    lit || (action.is_some() && action == held),
                )
            });
            if action.is_some()
                && resp.contains_pointer()
                && ui.is_enabled()
                && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary))
            {
                pressed = action;
            }
            resp
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
        (0, (0.0, 0.0), "cab.central", "UP"),
        (1, (0.0, -1.0), "cab.counter", "DOWN"),
        (2, (-1.0, 0.0), "cab.l_side", "LEFT"),
        (3, (1.0, 0.0), "cab.r_side", "RIGHT"),
        (4, (0.0, 1.0), "cab.press", "PRESS"),
    ];
    for (i, (dx, dy), key, action) in cells {
        let r = Rect::from_center_size(
            plate.center() + vec2(dx * (cell.x + 4.0), dy * (cell.y + 4.0)),
            cell,
        );
        put(ui, r, key, Some(action), Cab::Tactic, st.tactics[i]);
    }
    // the play buttons: SHOOT the biggest
    let xb = x0 + wa + gap;
    let half = (wb - 6.0) / 2.0;
    let y = top + 20.0;
    put(
        ui,
        Rect::from_min_size(pos2(xb, y), vec2(wb, 80.0)),
        "cab.shoot",
        Some("SHOOT"),
        Cab::Shoot,
        false,
    );
    put(
        ui,
        Rect::from_min_size(pos2(xb, y + 86.0), vec2(half, 58.0)),
        "cab.keeper",
        Some("KEEPER"),
        Cab::Keeper,
        false,
    );
    put(
        ui,
        Rect::from_min_size(pos2(xb + half + 6.0, y + 86.0), vec2(half, 58.0)),
        "cab.key_player",
        Some("KEYPLAYER"),
        Cab::KeyPlayer,
        false,
    );
    put(
        ui,
        Rect::from_min_size(pos2(xb, y + 150.0), vec2(wb, 46.0)),
        "cab.data",
        Some("DATA"),
        Cab::Data,
        false,
    );
    // the machine
    let xc = xb + wb + gap;
    caps_at(ui, xc, "panel.machine");
    let half = (wc - 6.0) / 2.0;
    let mut row = |ui: &mut Ui, n: f32, keys: [(&str, Option<&'static str>); 2], enabled: bool| {
        for (k, (key, action)) in keys.into_iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(xc + k as f32 * (half + 6.0), y + n * 50.0),
                vec2(half, 44.0),
            );
            ui.add_enabled_ui(enabled || k == 0, |ui| {
                put(ui, r, key, action, Cab::Plain, false)
            });
        }
    };
    row(
        ui,
        0.0,
        [("cab.start", Some("START")), ("cab.card", Some("CARD"))],
        true,
    );
    if st.edition == Edition::E1718 {
        // BACK sends nothing yet
        row(
            ui,
            1.0,
            [("cab.coin", Some("COIN")), ("cab.back", None)],
            false,
        );
    }
    st.pressed = st.pressed.or(pressed);
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
    st.board_changed |= board::formation_board(&mut b, &mut st.board, st.hover_delay, lang);
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
    // Esc closes the page, unless a KEYS cell is waiting for its key (Esc cancels that)
    if st.capturing.is_none()
        && st.pad_capturing.is_none()
        && ui.input(|i| i.key_pressed(egui::Key::Escape))
    {
        st.page = Page::Game;
        return;
    }
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

/// The message line, while its time lasts.
fn note_line(ui: &mut Ui, message: &mut Option<Note>) {
    let now = ui.input(|i| i.time);
    let Some(n) = message else { return };
    let at = *n.shown.get_or_insert(now);
    if now - at < NOTE_SECS {
        status_line(ui, n.tone, &n.text);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(NOTE_SECS - (now - at)));
    } else {
        *message = None;
    }
}

fn catalogue(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let cards = st.cards.clone();
    let c = &mut st.catalogue;
    let fresh = c.refresh(&cards);
    let mut add = None;
    ui.spacing_mut().item_spacing = vec2(6.0, 8.0);
    // Enter puts the first card listed on the table; typed letters always go to the search line
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        add = c.results.first().map(|&i| cards[i].number);
    }
    let search = ui.add(
        egui::TextEdit::singleline(&mut c.filter.search)
            .id_salt("cat-search")
            .hint_text(tr!(lang, "cat.search_hint"))
            .font(theme::body(16.0))
            .desired_width(f32::INFINITY)
            .margin(vec2(12.0, 8.0)),
    );
    if !search.has_focus() && !egui::Popup::is_any_open(ui.ctx()) {
        search.request_focus();
    }
    let f = &mut c.filter;
    let label_w = 110.0;
    ui.horizontal_wrapped(|ui| {
        field(ui, &tr!(lang, "cat.position"), label_w);
        if chip(ui, &tr!(lang, "cat.all"), f.pos == 0, None).clicked() {
            f.pos = 0;
        }
        for (i, p) in Position::ALL.iter().enumerate() {
            if chip(ui, p.code(), f.pos == i + 1, Some(p.color())).clicked() {
                f.pos = i + 1;
            }
        }
        ui.add_space(16.0);
        let index = c.index.as_ref();
        field(ui, &tr!(lang, "cat.country"), 80.0);
        combo(
            ui,
            "country",
            &mut f.country,
            index.map_or(&[][..], |x| &x.countries),
            lang,
        );
        field(ui, &tr!(lang, "cat.club"), 56.0);
        combo(
            ui,
            "club",
            &mut f.club,
            index.map_or(&[][..], |x| &x.clubs),
            lang,
        );
    });
    ui.horizontal_wrapped(|ui| {
        field(ui, &tr!(lang, "cat.rarity"), label_w);
        if chip(ui, &tr!(lang, "cat.any"), f.rarity == 0, None).clicked() {
            f.rarity = 0;
        }
        for (i, r) in Rarity::ALL.iter().enumerate() {
            if chip(
                ui,
                &tr!(lang, rarity_key(*r)),
                f.rarity == i + 1,
                Some(r.rim()),
            )
            .clicked()
            {
                f.rarity = i + 1;
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
            if chip(ui, s, f.sort == i, None).clicked() {
                f.sort = i;
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
                            &mut f.ranges[i * 2],
                            (1, 20),
                            w,
                        );
                        range_slider(
                            ui,
                            &tr!(lang, STATS[i * 2 + 1]),
                            &mut f.ranges[i * 2 + 1],
                            (1, 20),
                            w,
                        );
                        ui.end_row();
                    }
                    range_slider(ui, &tr!(lang, "cat.total"), &mut f.total, (60, 100), w);
                    ui.horizontal(|ui| {
                        ui.add_space(w - 110.0);
                        if action(ui, &tr!(lang, "cat.reset")).clicked() {
                            f.ranges = [(1, 20); 6];
                            f.total = (60, 100);
                        }
                    });
                    ui.end_row();
                });
        });
    // results and the stats pane
    let on_table: Vec<u32> = st
        .board
        .slots
        .iter()
        .flatten()
        .filter_map(|&i| st.board.cards.get(i).map(|k| k.number))
        .collect();
    let results = &c.results;
    let focus = &mut c.focus;
    let message = &mut st.message;
    egui_extras::StripBuilder::new(ui)
        .size(egui_extras::Size::remainder())
        .size(egui_extras::Size::exact(250.0))
        .horizontal(|mut strip| {
            strip.cell(|ui| {
                ui.horizontal(|ui| {
                    caps(ui, &tr!(lang, "cat.results", n = results.len()));
                });
                let size = vec2(84.0, 118.0);
                let gap = 10.0;
                let per_row = ((ui.available_width() + gap) / (size.x + gap))
                    .floor()
                    .max(1.0) as usize;
                let mut area = egui::ScrollArea::vertical()
                    .id_salt("results")
                    .auto_shrink(false);
                if fresh {
                    area = area.vertical_scroll_offset(0.0); // a new search starts at the top
                }
                ui.spacing_mut().item_spacing = vec2(gap, gap);
                // only the rows in sight are laid out: the whole catalogue is thousands of cards
                area.show_rows(ui, size.y, results.len().div_ceil(per_row), |ui, rows| {
                    for row in rows {
                        ui.horizontal(|ui| {
                            for &i in results.iter().skip(row * per_row).take(per_row) {
                                let card = &cards[i];
                                let resp = board::catalogue_card(
                                    ui,
                                    card,
                                    size,
                                    *focus == Some(i),
                                    on_table.contains(&card.number),
                                );
                                if resp.hovered() {
                                    *focus = Some(i);
                                }
                                if resp.clicked() {
                                    add = Some(card.number);
                                }
                            }
                        });
                    }
                });
            });
            strip.cell(|ui| {
                let shown = focus
                    .filter(|i| results.contains(i))
                    .or(results.first().copied());
                if let Some(card) = shown.and_then(|i| cards.get(i))
                    && stats_pane(ui, card, lang)
                {
                    add = Some(card.number);
                }
                note_line(ui, message);
            });
        });
    st.add = st.add.or(add);
}

fn combo(ui: &mut Ui, id: &str, value: &mut String, options: &[(String, usize)], lang: Lang) {
    let all = tr!(lang, "cat.all");
    let shown = if value.is_empty() {
        all.clone()
    } else {
        value.clone()
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(150.0)
        .height(400.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, String::new(), all);
            for (o, n) in options {
                ui.selectable_value(value, o.clone(), format!("{o} ({n})"));
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

/// The card's stats and its ADD button; `true` = ADD clicked.
fn stats_pane(ui: &mut Ui, card: &Card, lang: Lang) -> bool {
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
                        egui::RichText::new(card.total.to_string())
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
            cabinet(ui, vec2(w, 44.0), &tr!(lang, "cat.add"), Cab::Go, true).clicked()
        })
        .inner
}

// ---- KEYS ----

/// The keys and controls the page was given since the last frame, by `wccfpanel.c`'s rules (`keys_key`,
/// `pad_assign_locked`, `pad_tick`): a waiting KEY cell takes an allowed key, and the action that had it takes the
/// waiting one's old key (Esc cancels; Alt or a key that must stay free is refused); a waiting CONTROLLER cell takes the
/// control pressed, swapped the same way. Each change sets `keys_changed` (keys.txt is written).
pub fn keys_input(st: &mut PanelState) {
    let lang = st.lang;
    let label = |i: usize| tr!(lang, ACTIONS[i].0);
    for (vk, alt) in std::mem::take(&mut st.vk_down) {
        if let Some(k) = st.pad_capturing {
            if vk == 0x1B {
                st.pad_capturing = None;
                let now = pad_name(lang, st.pads[k]);
                st.note(
                    Tone::Notice,
                    tr!(lang, "keys.msg_keeps", action = label(k), key = now),
                );
            } else {
                let key = crate::keys::key_label(vk);
                st.note(Tone::Notice, tr!(lang, "keys.msg_is_a_key", key = key));
            }
            continue;
        }
        let Some(k) = st.capturing else { continue };
        let old = st.keys[k];
        if vk == 0x1B || vk == old {
            st.capturing = None;
            let key = key_name(lang, old);
            st.note(
                Tone::Notice,
                tr!(lang, "keys.msg_keeps", action = label(k), key = key),
            );
        } else if alt || !crate::keys::is_allowed(i128::from(vk)) {
            let key = if alt && vk != 0x12 {
                "Alt".into()
            } else {
                crate::keys::key_label(vk)
            };
            st.note(Tone::Refused, tr!(lang, "keys.msg_refused", key = key));
        } else {
            let other = (0..16).find(|&j| j != k && st.keys[j] == vk);
            st.keys[k] = vk;
            if let Some(j) = other {
                st.keys[j] = old;
            }
            (st.capturing, st.changed, st.keys_changed) = (None, Some(k), true);
            let text = match other {
                Some(j) => tr!(
                    lang,
                    "keys.msg_swapped",
                    action = label(k),
                    key = key_name(lang, vk),
                    other = label(j),
                    old = key_name(lang, old)
                ),
                None => tr!(
                    lang,
                    "keys.msg_set",
                    action = label(k),
                    key = key_name(lang, vk)
                ),
            };
            st.note(Tone::Done, text);
        }
    }
    let Some(code) = st.pad_down.take() else {
        return;
    };
    if let Some(k) = st.pad_capturing {
        let old = st.pads[k];
        st.pad_capturing = None;
        if code == old {
            st.note(
                Tone::Notice,
                tr!(
                    lang,
                    "keys.msg_keeps",
                    action = label(k),
                    key = pad_name(lang, code)
                ),
            );
            return;
        }
        let other = (0..16).find(|&j| j != k && st.pads[j] == code);
        st.pads[k] = code;
        if let Some(j) = other {
            st.pads[j] = old;
        }
        (st.changed, st.keys_changed) = (Some(k), true);
        let text = match other {
            Some(j) => tr!(
                lang,
                "keys.msg_swapped",
                action = label(k),
                key = pad_name(lang, code),
                other = label(j),
                old = pad_name(lang, old)
            ),
            None => tr!(
                lang,
                "keys.msg_set",
                action = label(k),
                key = pad_name(lang, code)
            ),
        };
        st.note(Tone::Done, text);
    } else if st.capturing.is_some() {
        st.note(
            Tone::Notice,
            tr!(lang, "keys.msg_is_a_control", key = pad_name(lang, code)),
        );
    }
}

fn key_name(lang: Lang, vk: u32) -> String {
    if vk == 0 {
        tr!(lang, "keys.none")
    } else {
        crate::keys::key_label(vk)
    }
}

fn pad_name(lang: Lang, code: u32) -> String {
    if code == 0 {
        tr!(lang, "keys.none")
    } else {
        crate::keys::pad_label(code)
    }
}

fn keys(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    keys_input(st);
    ui.label(egui::RichText::new(tr!(lang, "keys.hint")).color(theme::TEXT_DIM));
    if st.pad_names.is_empty() {
        status_line(ui, Tone::Notice, &tr!(lang, "keys.no_controller"));
    } else {
        status_line(
            ui,
            Tone::Done,
            &tr!(
                lang,
                "keys.controller_found",
                name = st.pad_names.join(", ")
            ),
        );
    }
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
    ui.horizontal(|ui| {
        if action(ui, &tr!(lang, "keys.defaults")).clicked() {
            st.keys = crate::keys::defaults(st.edition == Edition::E1718);
            st.pads = [0; 16];
            (st.capturing, st.pad_capturing, st.changed, st.keys_changed) =
                (None, None, None, true);
            st.say(Tone::Done, "keys.msg_defaults");
        }
        ui.add_space(12.0);
        note_line(ui, &mut st.message);
    });
}

fn row(ui: &mut Ui, st: &mut PanelState, lang: Lang, i: usize, cell_w: f32) {
    let (key, kind) = ACTIONS[i];
    let disabled = key == "cab.coin" && st.edition == Edition::E1011; // free play: COIN is not needed
    let (r, _) = ui.allocate_exact_size(vec2(130.0, 30.0), Sense::hover());
    let lit = st.capturing == Some(i) || st.pad_capturing == Some(i);
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
    let state = |bound: bool, cap: bool| {
        if cap {
            KeyState::Capturing
        } else if !bound {
            KeyState::Empty
        } else if st.changed == Some(i) {
            KeyState::Changed
        } else {
            KeyState::Bound
        }
    };
    let ks = state(st.keys[i] != 0, st.capturing == Some(i));
    let cs = state(st.pads[i] != 0, st.pad_capturing == Some(i));
    let (k, c) = (
        crate::keys::key_label(st.keys[i]),
        crate::keys::pad_label(st.pads[i]),
    );
    let r1 = key_cell(ui, &k, ks, cell_w, lang);
    let r2 = key_cell(ui, &c, cs, cell_w, lang);
    if r1.clicked() {
        st.pad_capturing = None;
        st.capturing = (st.capturing != Some(i)).then_some(i); // a second click cancels
        st.message = None;
    }
    if r2.clicked() {
        st.capturing = None;
        st.pad_capturing = (st.pad_capturing != Some(i)).then_some(i);
        st.message = None;
        if st.pad_capturing.is_some() && st.pad_names.is_empty() {
            st.say(Tone::Notice, "keys.msg_no_pad_yet");
        }
    }
    if r2.secondary_clicked() {
        st.pad_capturing = None;
        let label = tr!(lang, key);
        if st.pads[i] == 0 {
            st.note(
                Tone::Notice,
                tr!(lang, "keys.msg_no_control", action = label),
            );
        } else {
            st.pads[i] = 0;
            st.keys_changed = true;
            st.note(Tone::Done, tr!(lang, "keys.msg_cleared", action = label));
        }
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
        // a long value is cut short (the whole of it on hover), never wider than its column
        ui.add(
            egui::Label::new(
                egui::RichText::new(v)
                    .color(theme::TEXT)
                    .font(theme::bold(14.0)),
            )
            .truncate(),
        );
    });
    ui.end_row();
}

/// "15:04 (6 min ago)".
fn when_text(lang: Lang, (t, age): &When) -> String {
    match age {
        0..60 => tr!(lang, "settings.just_now", t = t),
        60..3600 => tr!(lang, "settings.min_ago", t = t, n = age / 60),
        3600..86400 => tr!(lang, "settings.h_ago", t = t, n = age / 3600),
        _ => t.clone(),
    }
}

/// The game's text choices: (english= word, text now, label).
const TEXTS: [(&str, &str); 3] = [("on", "en"), ("it", "it"), ("off", "")];

fn settings(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let accent = st.edition.accent();
    let now = ui.input(|i| i.time);
    let s = st.status.clone();
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
                    let (v, c) = match &s.link {
                        Link::Up(p) if p == "127.0.0.1" => {
                            (tr!(lang, "settings.link_local"), theme::GREEN)
                        }
                        Link::Up(p) => (tr!(lang, "settings.link_online", ip = p), theme::GREEN),
                        Link::Down => (tr!(lang, "settings.link_down"), theme::RED_TEXT),
                        Link::Unknown => (tr!(lang, "settings.unknown"), theme::TEXT_FAINT),
                    };
                    kv(ui, &tr!(lang, "settings.server"), &v, Some(c));
                    let (k, c) = match s.card {
                        CardState::Open => ("settings.card_open", theme::AMBER),
                        CardState::Closed => ("settings.card_closed", theme::GREEN),
                        CardState::Cut => ("settings.card_cut", theme::AMBER),
                        CardState::New => ("settings.card_new", theme::TEXT_FAINT),
                        CardState::Unreadable => ("settings.card_unreadable", theme::RED_TEXT),
                        CardState::None => ("settings.card_none", theme::TEXT_FAINT),
                    };
                    kv(ui, &tr!(lang, "settings.session"), &tr!(lang, k), Some(c));
                    let (v, c) = match s.bad {
                        None => ("-".to_owned(), None),
                        Some(0) => ("0".to_owned(), Some(theme::GREEN)),
                        Some(1) => (tr!(lang, "settings.bad_one"), Some(theme::AMBER)),
                        Some(n) => (tr!(lang, "settings.bad_many", n = n), Some(theme::RED_TEXT)),
                    };
                    kv(ui, &tr!(lang, "settings.bad_endings"), &v, c);
                    let v = s
                        .saved
                        .as_ref()
                        .map_or_else(|| tr!(lang, "settings.never"), |w| when_text(lang, w));
                    kv(ui, &tr!(lang, "settings.last_saved"), &v, None);
                    let v = s.backup.as_ref().map_or_else(
                        || tr!(lang, "settings.no_backup"),
                        |(w, n)| tr!(lang, "settings.backups", t = when_text(lang, w), n = n),
                    );
                    kv(ui, &tr!(lang, "settings.backup"), &v, None);
                    let v = match s.text {
                        Some("en") => "English".to_owned(),
                        Some("it") => "Italiano".to_owned(),
                        Some(_) => tr!(lang, "launcher.original_ja"),
                        None => "-".to_owned(),
                    };
                    kv(ui, &tr!(lang, "settings.game_text"), &v, None);
                    kv(
                        ui,
                        &tr!(lang, "settings.version"),
                        &format!("Forza-WCCF {}", s.version),
                        None,
                    );
                });
        });
        widgets::section(left, &tr!(lang, "settings.view"), accent, |ui| {
            let w = ui.available_width();
            field(ui, &tr!(lang, "settings.layout"), w);
            let mut i = st.file.compact as usize;
            if segmented(
                ui,
                &mut i,
                &[
                    &tr!(lang, "settings.cabinet"),
                    &tr!(lang, "settings.compact"),
                ],
                w,
            ) {
                st.file.compact = i == 1;
                st.file_changed = true;
                st.say(
                    Tone::Done,
                    if i == 1 {
                        "settings.msg_compact"
                    } else {
                        "settings.msg_cabinet"
                    },
                );
            }
            field(ui, &tr!(lang, "settings.hover"), w);
            const HOVER: [u32; 4] = [500, 1000, 2000, 3000];
            let mut i = HOVER
                .iter()
                .position(|&m| m == st.file.hover_ms)
                .unwrap_or(usize::MAX);
            if segmented(ui, &mut i, &["0.5 s", "1 s", "2 s", "3 s"], w) {
                st.file.hover_ms = HOVER[i];
                st.hover_delay = HOVER[i] as f32 / 1000.0;
                st.file_changed = true;
                let t = format!("{}", st.hover_delay);
                st.note(Tone::Done, tr!(lang, "settings.msg_hover", s = t));
            }
        });
        widgets::section(right, &tr!(lang, "settings.next"), accent, |ui| {
            let w = ui.available_width();
            field(ui, &tr!(lang, "launcher.where"), w);
            let mut i = st.file.online as usize;
            if segmented(
                ui,
                &mut i,
                &[
                    &tr!(lang, "launcher.this_pc"),
                    &tr!(lang, "launcher.online"),
                ],
                w,
            ) {
                if i == 0 {
                    st.file.online = false;
                    st.file_changed = true;
                    st.say(Tone::Done, "settings.msg_this_pc");
                } else if valid_address(&st.file.server) {
                    st.file.online = true;
                    st.file_changed = true;
                    let ip = st.file.server.clone();
                    st.note(Tone::Done, tr!(lang, "settings.msg_online", ip = ip));
                } else {
                    st.address.clear();
                    st.say(Tone::Notice, "settings.msg_type_address");
                    ui.memory_mut(|m| m.request_focus(egui::Id::new("settings-address")));
                }
            }
            // the server's address: digits and dots; Enter (or a click elsewhere) keeps a good one, Esc leaves it
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.address)
                    .id(egui::Id::new("settings-address"))
                    .char_limit(15)
                    .hint_text(tr!(lang, "settings.address_hint"))
                    .font(theme::mono(15.0))
                    .desired_width(w)
                    .margin(vec2(10.0, 7.0)),
            );
            st.address.retain(|c| c.is_ascii_digit() || c == '.');
            if r.lost_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) || st.address == st.file.server {
                    st.address = st.file.server.clone();
                } else if valid_address(&st.address) {
                    st.file.server = st.address.clone();
                    st.file.online = true;
                    st.file_changed = true;
                    let ip = st.file.server.clone();
                    st.note(Tone::Done, tr!(lang, "settings.msg_online", ip = ip));
                } else {
                    let a = std::mem::replace(&mut st.address, st.file.server.clone());
                    st.note(Tone::Refused, tr!(lang, "settings.msg_not_address", a = a));
                }
            }
            field(ui, &tr!(lang, "launcher.game_text"), w);
            let shown = st.file.english.as_deref().or_else(|| {
                let t = s.text?;
                TEXTS.iter().find(|x| x.1 == t).map(|x| x.0)
            });
            let mut i = TEXTS
                .iter()
                .position(|x| Some(x.0) == shown)
                .unwrap_or(usize::MAX);
            if segmented(
                ui,
                &mut i,
                &["English", "Italiano", &tr!(lang, "launcher.original_ja")],
                w,
            ) {
                match s.text {
                    None => st.say(Tone::Notice, "settings.msg_reading_text"),
                    Some(now) => {
                        let same = TEXTS[i].1 == now;
                        st.file.english = (!same).then(|| TEXTS[i].0.to_owned());
                        st.file_changed = true;
                        st.say(
                            Tone::Done,
                            if same {
                                "settings.msg_text_stays"
                            } else {
                                "settings.msg_text_changes"
                            },
                        );
                    }
                }
            }
            ui.add_space(4.0);
            let armed = st.restart_armed > now;
            let label = if st.restart_sent {
                tr!(lang, "settings.restarting")
            } else if armed {
                let left = (st.restart_armed - now).ceil().min(8.0) as u32;
                tr!(lang, "settings.restart_again", s = left)
            } else {
                tr!(lang, "settings.restart")
            };
            if armed && st.restart_armed.is_finite() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }
            let lit = armed || st.restart_sent;
            if cabinet(
                ui,
                vec2(w, 48.0),
                &label,
                if lit { Cab::Tactic } else { Cab::Plain },
                lit,
            )
            .clicked()
            {
                if st.restart_sent {
                    st.say(Tone::Notice, "settings.msg_restart_asked");
                } else if s.card == CardState::Open {
                    st.say(Tone::Refused, "panel.msg_session_open");
                } else if !armed {
                    st.restart_armed = now + 8.0;
                    st.say(Tone::Notice, "settings.msg_restart_armed");
                } else {
                    st.restart_armed = 0.0;
                    st.restart = true;
                }
            }
            let (hint, c) = if s.card == CardState::Open {
                ("settings.restart_refused", theme::RED_TEXT)
            } else {
                ("settings.restart_hint", theme::TEXT_DIM)
            };
            ui.label(
                egui::RichText::new(tr!(lang, hint))
                    .font(theme::body(12.0))
                    .color(c),
            );
        });
        widgets::section(right, &tr!(lang, "settings.logs"), accent, |ui| {
            let w = ui.available_width();
            let armed = st.logs_armed > now;
            let busy = st.logs_sent || s.logs.as_deref() == Some("sending");
            let label = if busy {
                tr!(lang, "settings.sending")
            } else if armed {
                tr!(lang, "settings.send_again")
            } else {
                tr!(lang, "settings.send_logs")
            };
            if cabinet(ui, vec2(w, 44.0), &label, Cab::Data, armed || busy).clicked() {
                if busy {
                    st.say(Tone::Notice, "settings.msg_sending");
                } else if !valid_address(&st.file.server) {
                    st.say(Tone::Refused, "settings.msg_logs_need_server");
                } else if !armed {
                    st.logs_armed = now + 8.0;
                    let ip = st.file.server.clone();
                    st.note(Tone::Notice, tr!(lang, "settings.msg_logs_armed", ip = ip));
                } else {
                    st.logs_armed = 0.0;
                    st.send_logs = true;
                }
            }
            match s.logs.as_deref() {
                Some(r) if r.starts_with("ok ") => {
                    status_line(ui, Tone::Done, &tr!(lang, "settings.sent", code = &r[3..]));
                }
                Some(r) if r.starts_with("fail ") => {
                    status_line(
                        ui,
                        Tone::Refused,
                        &tr!(lang, "settings.not_sent", why = &r[5..]),
                    );
                }
                _ if busy => {
                    status_line(ui, Tone::Notice, &tr!(lang, "settings.msg_sending"));
                }
                _ => {}
            }
            let mut auto = !st.file.auto_logs_off;
            if widgets::toggle_row(
                ui,
                &mut auto,
                &tr!(lang, "settings.auto_reports"),
                &tr!(lang, "settings.auto_reports_hint"),
            )
            .clicked()
            {
                st.file.auto_logs_off = !auto;
                st.file_changed = true;
            }
        });
    });
    note_line(ui, &mut st.message);
}

// ---- CLUB CARD ----

/// A choice on CLUB CARD that needs a second click within 8 s, then a restart: another card from YOUR CARDS (its file in
/// data\save\cards and its club), a new card, or CLEAR BAD ENDINGS (2010-11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClubPick {
    Card { file: String, club: String },
    New,
    Fix,
}

/// The view's row labels and headings the page translates; any other label shows as the view writes it.
const CLUB_LABELS: [(&str, &str); 16] = [
    ("CLUB", "club.row_club"),
    ("MANAGER", "club.row_manager"),
    ("CONTRACT", "club.row_contract"),
    ("SALARY", "club.row_salary"),
    ("LEAGUE", "club.row_league"),
    ("ALL MATCHES", "club.row_all_matches"),
    ("PRIZE MONEY", "club.row_prize"),
    ("TRADE RIGHTS", "club.row_trade"),
    ("LAST PLAYED", "club.row_last_played"),
    ("CARD HEALTH", "club.row_health"),
    ("SESSION", "club.row_session"),
    ("BAD ENDINGS", "club.row_bad"),
    ("SAVED COPIES", "club.row_copies"),
    ("CARD SAVED", "club.row_saved"),
    ("BACKUPS", "club.backups"),
    ("SQUAD", "club.squad"),
];

fn club_label(lang: Lang, label: &str) -> String {
    CLUB_LABELS
        .iter()
        .find(|(l, _)| *l == label)
        .map_or_else(|| label.to_owned(), |(_, k)| tr!(lang, k))
}

/// The view's colour letters: w white, g green, y yellow, r red, o orange, d dim.
fn club_colour(c: char) -> Color32 {
    match c {
        'g' => theme::GREEN,
        'y' => theme::GOLD,
        'r' => theme::RED_TEXT,
        'o' => theme::AMBER,
        'd' => theme::TEXT_FAINT,
        _ => theme::TEXT,
    }
}

/// A click on a CLUB CARD choice, as `wccfpanel.c` `club_click`: refused while a restart is asked or a card session
/// is open, NEW refused while the slot holds a blank card; a first click arms it for 8 s, a second asks for it
/// ([`PanelState::club_request`]: the panel DLL writes data\panel.txt and asks for the restart).
pub fn club_click(st: &mut PanelState, pick: ClubPick, now: f64) {
    let lang = st.lang;
    if st.restart_sent {
        st.say(Tone::Notice, "club.msg_asked");
    } else if st.club.session == "open" || st.status.card == CardState::Open {
        st.say(Tone::Refused, "club.session_refused");
    } else if pick == ClubPick::New && st.club.state == "none" {
        st.say(Tone::Notice, "club.msg_blank");
    } else if st
        .club_armed
        .as_ref()
        .is_none_or(|(p, until)| *p != pick || *until <= now)
    {
        let text = match &pick {
            ClubPick::Fix => tr!(lang, "club.msg_arm_fix"),
            ClubPick::New => tr!(lang, "club.msg_arm_new"),
            ClubPick::Card { club, .. } => tr!(lang, "club.msg_arm_play", club = club),
        };
        st.note(Tone::Notice, text);
        st.club_armed = Some((pick, now + 8.0));
    } else {
        st.club_armed = None;
        st.club_request = Some(pick);
    }
}

fn club_card(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let accent = st.edition.accent();
    let now = ui.input(|i| i.time);
    let armed = st
        .club_armed
        .clone()
        .filter(|(_, until)| *until > now)
        .map(|(p, until)| {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(until - now));
            p
        });
    let view = st.club.clone();
    let ok = view.state == "ok";
    let mut click = None;
    ui.label(
        egui::RichText::new(tr!(lang, "club.intro"))
            .font(theme::body(13.0))
            .color(theme::TEXT_DIM),
    );
    ui.add_space(6.0);
    let foot = 50.0;
    let h = ui.available_height() - foot;
    ui.allocate_ui(vec2(ui.available_width(), h), |ui| {
        egui_extras::StripBuilder::new(ui)
            .size(egui_extras::Size::exact(420.0))
            .size(egui_extras::Size::remainder())
            .horizontal(|mut strip| {
                strip.cell(|ui| {
                    ui.set_max_height(h);
                    // the club card itself: landscape, as the physical card, with the club and its manager
                    let (r, _) =
                        ui.allocate_exact_size(vec2(340.0, 340.0 * 58.0 / 81.0), Sense::hover());
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
                    let chip_r = Rect::from_min_size(r.min + vec2(24.0, 58.0), vec2(42.0, 32.0));
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
                    // "KITAKAZE  (founded 2026-10-01)": the name alone on the card
                    let first = |v: Option<&str>| {
                        v.map(|v| v.split("  (").next().unwrap_or(v).trim().to_owned())
                            .filter(|v| !v.is_empty() && v != "-")
                    };
                    if let Some(club) = first(view.row("CLUB")) {
                        let g = widgets::fit(p, &club, theme::display(34.0), Color32::WHITE, 300.0);
                        p.galley(r.min + vec2(24.0, 104.0), g, Color32::WHITE);
                    }
                    if let Some(m) = first(view.row("MANAGER")) {
                        p.text(
                            r.min + vec2(24.0, 146.0),
                            Align2::LEFT_TOP,
                            tr!(lang, "club.manager", name = m),
                            theme::body(15.0),
                            theme::TEXT,
                        );
                    }
                    p.text(
                        r.right_bottom() + vec2(-20.0, -18.0),
                        Align2::RIGHT_BOTTOM,
                        st.edition.label(),
                        theme::display(22.0),
                        accent,
                    );
                    ui.add_space(10.0);
                    // CLEAR BAD ENDINGS (2010-11), under the card while there are some or the last session was cut
                    if st.edition == Edition::E1011
                        && ok
                        && (view.bad_endings() > 0 || view.session == "cut")
                    {
                        let lit = armed == Some(ClubPick::Fix);
                        let key = if lit {
                            "club.clear_again"
                        } else {
                            "club.clear_bad"
                        };
                        if cabinet(ui, vec2(300.0, 40.0), &tr!(lang, key), Cab::Stop, lit).clicked()
                        {
                            click = Some(ClubPick::Fix);
                        }
                        ui.label(
                            egui::RichText::new(tr!(lang, "club.clear_hint"))
                                .font(theme::body(12.0))
                                .color(theme::TEXT_DIM),
                        );
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("club-left")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().interact_size.y = 18.0;
                            if !ok {
                                // no club to show: why, then the backups
                                let (msg, tone) = match view.state.as_str() {
                                    "noview" if st.edition == Edition::E1718 => {
                                        (tr!(lang, "club.none_1718"), Tone::Notice)
                                    }
                                    "noview" => (tr!(lang, "club.noview"), Tone::Notice),
                                    "unreadable" => (view.reason.clone(), Tone::Refused),
                                    _ if view.reason.is_empty() => {
                                        (view.state.clone(), Tone::Notice)
                                    }
                                    _ => (view.reason.clone(), Tone::Notice),
                                };
                                status_line(ui, tone, &msg);
                                if !view.backups.is_empty() {
                                    ui.add_space(8.0);
                                    caps(ui, &tr!(lang, "club.backups"));
                                    for b in &view.backups {
                                        ui.label(
                                            egui::RichText::new(b)
                                                .font(theme::mono(13.0))
                                                .color(theme::TEXT),
                                        );
                                    }
                                    ui.label(
                                        egui::RichText::new(tr!(lang, "club.backups_hint"))
                                            .font(theme::body(12.0))
                                            .color(theme::TEXT_DIM),
                                    );
                                }
                                return;
                            }
                            let mut grid = 0;
                            let mut rows: Vec<(&str, &str, char)> = Vec::new();
                            let mut flush = |ui: &mut Ui, rows: &mut Vec<(&str, &str, char)>| {
                                if rows.is_empty() {
                                    return;
                                }
                                grid += 1;
                                egui::Grid::new(("club-rows", grid))
                                    .num_columns(2)
                                    .spacing(vec2(14.0, 4.0))
                                    .striped(true)
                                    .min_col_width(110.0)
                                    .show(ui, |ui| {
                                        for (l, v, c) in rows.drain(..) {
                                            kv_coloured(
                                                ui,
                                                &club_label(lang, l),
                                                v,
                                                club_colour(c),
                                            );
                                        }
                                    });
                            };
                            for line in &view.lines {
                                match line {
                                    forza_club::Line::Head(t) => {
                                        flush(ui, &mut rows);
                                        ui.add_space(4.0);
                                        caps(ui, &club_label(lang, t));
                                    }
                                    forza_club::Line::Row {
                                        label,
                                        value,
                                        colour,
                                    } => {
                                        rows.push((label, value, *colour));
                                    }
                                }
                            }
                            flush(ui, &mut rows);
                        });
                });
                strip.cell(|ui| {
                    ui.set_max_height(h);
                    egui::ScrollArea::vertical()
                        .id_salt("club-right")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 8.0;
                            if ok && !view.squad.is_empty() {
                                caps(ui, &tr!(lang, "club.squad"));
                                egui::Grid::new("club-squad")
                                    .num_columns(7)
                                    .spacing(vec2(12.0, 4.0))
                                    .striped(true)
                                    .show(ui, |ui| {
                                        for k in [
                                            "club.sq_no",
                                            "club.sq_name",
                                            "club.sq_pos",
                                            "club.sq_apps",
                                            "club.sq_goals",
                                            "club.sq_assists",
                                            "club.sq_cond",
                                        ] {
                                            ui.label(
                                                egui::RichText::new(tr!(lang, k))
                                                    .font(theme::display(13.0))
                                                    .color(theme::TEXT_DIM),
                                            );
                                        }
                                        ui.end_row();
                                        for p in &view.squad {
                                            let pos = match p[2].chars().next() {
                                                Some('G') => theme::POS_GK,
                                                Some('D') => theme::POS_DF,
                                                Some('M') => theme::POS_MF,
                                                _ => theme::POS_FW,
                                            };
                                            let injured = p[7] != "-";
                                            let cell = |ui: &mut Ui, t: &str, c: Color32| {
                                                ui.label(
                                                    egui::RichText::new(t)
                                                        .font(theme::body(13.0))
                                                        .color(c),
                                                );
                                            };
                                            cell(ui, &p[0], theme::TEXT);
                                            cell(
                                                ui,
                                                &p[1],
                                                if injured {
                                                    theme::RED_TEXT
                                                } else {
                                                    theme::TEXT
                                                },
                                            );
                                            cell(ui, &p[2], pos);
                                            for v in &p[3..7] {
                                                cell(ui, v, theme::TEXT);
                                            }
                                            ui.end_row();
                                        }
                                    });
                                ui.label(
                                    egui::RichText::new(tr!(lang, "club.injured"))
                                        .font(theme::body(12.0))
                                        .color(theme::TEXT_DIM),
                                );
                                ui.add_space(6.0);
                            }
                            caps(ui, &tr!(lang, "club.your_cards"));
                            if view.wallet.is_empty() {
                                ui.label(
                                    egui::RichText::new(tr!(lang, "club.no_others"))
                                        .font(theme::body(13.0))
                                        .color(theme::TEXT_DIM),
                                );
                            }
                            for w in &view.wallet {
                                let pick = ClubPick::Card {
                                    file: w.file.clone(),
                                    club: w.club.clone(),
                                };
                                let lit = armed.as_ref() == Some(&pick);
                                if club_row(ui, &w.club, &w.summary, w.transferred, lit, lang) {
                                    click = Some(pick);
                                }
                            }
                            ui.add_space(4.0);
                            let w = ui.available_width().min(360.0);
                            let lit = armed == Some(ClubPick::New);
                            let key = if lit {
                                "club.new_again"
                            } else {
                                "club.new_card"
                            };
                            if cabinet(ui, vec2(w, 44.0), &tr!(lang, key), Cab::Tactic, lit)
                                .clicked()
                            {
                                click = Some(ClubPick::New);
                            }
                            let open = view.session == "open" || st.status.card == CardState::Open;
                            ui.label(
                                egui::RichText::new(tr!(
                                    lang,
                                    if open {
                                        "club.session_refused"
                                    } else {
                                        "club.switch_hint"
                                    }
                                ))
                                .font(theme::body(12.0))
                                .color(if open {
                                    theme::RED_TEXT
                                } else {
                                    theme::TEXT_DIM
                                }),
                            );
                        });
                });
            });
    });
    if let Some(pick) = click {
        club_click(st, pick, now);
    }
    note_line(ui, &mut st.message);
}

/// A row and its value in the view's colour.
fn kv_coloured(ui: &mut Ui, k: &str, v: &str, c: Color32) {
    ui.label(
        egui::RichText::new(k)
            .color(theme::TEXT_DIM)
            .font(theme::body(13.0)),
    );
    ui.add(egui::Label::new(egui::RichText::new(v).color(c).font(theme::bold(13.0))).truncate());
    ui.end_row();
}

/// One of YOUR CARDS: its club and summary, and PLAY THIS CLUB (CLICK AGAIN while armed), or TRANSFERRED as a word;
/// `true` = PLAY THIS CLUB clicked.
fn club_row(ui: &mut Ui, name: &str, sub: &str, transferred: bool, lit: bool, lang: Lang) -> bool {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 54.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(
        r,
        theme::RADIUS_BIG,
        if lit {
            theme::AMBER.gamma_multiply(0.14)
        } else {
            theme::RAISED
        },
    );
    p.rect_stroke(
        r,
        theme::RADIUS_BIG,
        Stroke::new(
            if lit { 2.0 } else { 1.0 },
            if lit { theme::AMBER } else { theme::RIM },
        ),
        StrokeKind::Inside,
    );
    let bw = 170.0;
    let tw = w - bw - 30.0;
    let g = widgets::fit(p, name, theme::display(20.0), theme::TEXT, tw);
    p.galley(pos2(r.left() + 14.0, r.top() + 6.0), g, theme::TEXT);
    let g = widgets::fit(p, sub, theme::body(12.0), theme::TEXT_DIM, tw);
    p.galley(pos2(r.left() + 14.0, r.top() + 31.0), g, theme::TEXT_DIM);
    let b = Rect::from_min_size(pos2(r.right() - bw - 8.0, r.top() + 8.0), vec2(bw, 38.0));
    if transferred {
        p.text(
            b.center(),
            Align2::CENTER_CENTER,
            tr!(lang, "club.transferred"),
            theme::display(15.0),
            theme::TEXT_FAINT,
        );
        return false;
    }
    let key = if lit { "club.click_again" } else { "club.play" };
    ui.put(b, |ui: &mut Ui| {
        cabinet(ui, b.size(), &tr!(lang, key), Cab::Tactic, lit)
    })
    .clicked()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::board::Aptitude;

    fn card(
        no: u32,
        name: &str,
        club: &str,
        country: &str,
        pos: Position,
        stats: [u8; 6],
        total: u8,
    ) -> Card {
        Card {
            number: no,
            name: name.into(),
            short: name.rsplit(' ').next().unwrap_or(name).to_uppercase(),
            club: club.into(),
            country: country.into(),
            season: "2004-05".into(),
            pos,
            rarity: if no.is_multiple_of(2) {
                Rarity::Legend
            } else {
                Rarity::Regular
            },
            stats,
            total,
            aptitude: Aptitude::default(),
        }
    }

    fn cards() -> Vec<Card> {
        use Position::*;
        let mut v = vec![
            card(
                10,
                "Ricardo Kakà",
                "Milano Rossa",
                "Brasilia",
                Mf,
                [18, 6, 18, 12, 16, 14],
                88,
            ),
            card(
                101,
                "Jens Ødegård",
                "Nordhavn",
                "Nordland",
                Df,
                [6, 17, 10, 15, 12, 15],
                80,
            ),
            card(
                210,
                "Andrea Bàrtoli",
                "Milano Rossa",
                "Italia",
                Fw,
                [19, 4, 15, 16, 14, 12],
                88,
            ),
            card(
                32,
                "Paolo Abate",
                "Porto Azzurro",
                "Italia",
                Gk,
                [2, 16, 8, 12, 9, 13],
                72,
            ),
        ];
        v[1].aptitude.roles[7] = 9; // DMF 9
        v[3].aptitude.roles[7] = 7; // DMF 7: not a DMF by the search
        v
    }

    fn found(cards: &[Card], f: &Filter) -> Vec<u32> {
        search(cards, &Index::new(cards), f)
            .into_iter()
            .map(|i| cards[i].number)
            .collect()
    }

    fn q(search: &str) -> Filter {
        Filter {
            search: search.into(),
            ..Filter::default()
        }
    }

    #[test]
    fn search_words_and_numbers() {
        let c = cards();
        assert_eq!(fold("KAKÀ Ødegård"), "kaka odegard");
        // NAME order is the short names' (ABATE, BARTOLI, KAKA, ODEGARD)
        assert_eq!(found(&c, &q("")), [32, 210, 10, 101]);
        assert_eq!(found(&c, &q("kaka")), [10]);
        assert_eq!(found(&c, &q("milano")), [210, 10]);
        // words ANDed, over name, club, country, season, the line and the roles
        assert_eq!(found(&c, &q("milano fw")), [210]);
        assert_eq!(found(&c, &q("italia 2004-05")), [32, 210]);
        assert_eq!(found(&c, &q("dmf")), [101]);
        assert_eq!(found(&c, &q("brasilia porto")), Vec::<u32>::new());
        // digits alone: the card numbers starting so; a digits word among others also matches a number's start
        assert_eq!(found(&c, &q("10")), [10, 101]);
        assert_eq!(found(&c, &q("21 milano")), [210]);
        assert_eq!(found(&c, &q("2004")), Vec::<u32>::new());
    }

    #[test]
    fn filters_and_sorts() {
        let c = cards();
        let f = |g: fn(&mut Filter)| {
            let mut f = Filter::default();
            g(&mut f);
            found(&c, &f)
        };
        assert_eq!(f(|f| f.pos = 3), [10]); // MF
        assert_eq!(f(|f| f.rarity = 2), [32, 210, 10]); // LEGEND
        assert_eq!(f(|f| f.country = "Italia".into()), [32, 210]);
        assert_eq!(f(|f| f.club = "Nordhavn".into()), [101]);
        assert_eq!(f(|f| f.ranges[0] = (15, 20)), [210, 10]); // OFF 15-20
        assert_eq!(f(|f| f.total = (60, 80)), [32, 101]);
        // TOTAL best first, ties by name; a stat best first, ties by TOTAL
        assert_eq!(f(|f| f.sort = 1), [210, 10, 101, 32]);
        assert_eq!(f(|f| f.sort = 3), [101, 32, 10, 210]); // DEF
        assert_eq!(f(|f| f.sort = 4), [10, 210, 101, 32]); // TEC
        let index = Index::new(&c);
        assert_eq!(
            index.countries,
            [
                ("Brasilia".into(), 1),
                ("Italia".into(), 2),
                ("Nordland".into(), 1)
            ]
        );
    }

    #[test]
    fn add_card_says_how_it_went() {
        let mut st = PanelState::new(
            Edition::E1011,
            Lang::En,
            cards::board(&[], &[]),
            cards().into(),
        );
        let mut placed = Vec::new();
        assert!(add_card(&mut st, &mut placed, 10));
        assert_eq!(placed[0].no, 10);
        assert_eq!(st.board.cards[0].number, 10);
        assert_eq!(st.message.as_ref().unwrap().text, "KAKÀ added (1/16)");
        assert!(!add_card(&mut st, &mut placed, 10));
        assert_eq!(
            st.message.as_ref().unwrap().text,
            "KAKÀ is already on the table"
        );
        for no in 1000..1015 {
            placed.push(Placed {
                slot: 0,
                no,
                x: 9.0,
                y: 0.0,
            });
        }
        assert!(!add_card(&mut st, &mut placed, 32));
        assert_eq!(st.message.as_ref().unwrap().tone, Tone::Refused);
        assert!(!add_card(&mut st, &mut placed, 999)); // not in the catalogue: nothing happens
    }

    #[test]
    fn keys_swap_refuse_and_cancel() {
        let mut st = PanelState::new(
            Edition::E1011,
            Lang::En,
            cards::board(&[], &[]),
            cards().into(),
        );
        // SHOOT (C) takes X, PRESS's key: PRESS gets C
        st.capturing = Some(2);
        st.vk_down = vec![(0x58, false)];
        keys_input(&mut st);
        assert_eq!((st.keys[2], st.keys[1], st.capturing), (0x58, 0x43, None));
        assert!(std::mem::take(&mut st.keys_changed));
        assert_eq!(
            st.message.as_ref().unwrap().text,
            "SHOOT = X   (PRESS gets C)"
        );
        // Esc and F10 (must stay free) and Alt+G: refused or cancelled, nothing changes
        st.capturing = Some(0);
        st.vk_down = vec![(0x79, false), (0x47, true)];
        keys_input(&mut st);
        assert_eq!(
            st.message.as_ref().unwrap().text,
            "Alt cannot be used: pick another key"
        );
        assert_eq!(st.capturing, Some(0));
        st.vk_down = vec![(0x1B, false)];
        keys_input(&mut st);
        assert_eq!(
            (st.keys[0], st.capturing, st.keys_changed),
            (0x0D, None, false)
        );
        // a controller button for KEEPER, then the same button for DATA: KEEPER gives it up
        st.pad_capturing = Some(3);
        st.pad_down = Some(0x103);
        keys_input(&mut st);
        st.pad_capturing = Some(4);
        st.pad_down = Some(0x103);
        keys_input(&mut st);
        assert_eq!((st.pads[3], st.pads[4]), (0, 0x103));
        assert_eq!(
            crate::keys::keys_text(false, &st.keys, &st.pads)
                .lines()
                .last(),
            Some("PAD DATA=B3")
        );
    }
}
