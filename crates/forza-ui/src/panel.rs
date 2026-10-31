//! The in-game panel on its 1440×900 design sheet. The FORMATION BOARD owns the right strip (the last 580 points);
//! the middle holds the game picture at the top, the cabinet deck under it (tactics, SHOOT, KEEPER, KEY PLAYER, DATA,
//! the machine's buttons) and the bar of page buttons along the bottom. Pages (CATALOGUE, KEYS, SETTINGS, CLUB CARD)
//! open over the picture and the deck. The sheet scales by `min(H/900, W/1440)` through `pixels_per_point`; this code
//! works in sheet points, and a wider sheet widens the middle.

use super::Lang;
use super::board::{self, Board, Card, Position, Rarity};
use super::theme::{self, Edition};
use super::widgets::{
    self, Cab, Dir, Face, KeyState, Shape, Tone, action, cabinet, cabinet_face, cabinet_shaped,
    caps, chip, field, key_cell, range_slider, segmented, shape_contains, stat_bar, status_line,
};
use crate::board::{PositionColor, RarityRim};
use crate::sizing;
use crate::theme::Accent;
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use forza::cards::lineup::{self, LineupError, Skip, Skipped};
use forza::cards::{self, AddRefused, Placed};
use forza::club_view::{self as forza_club, ClubView};
use forza::panel_txt::{PanelFile, valid_address};
use std::sync::Arc;

/// The game's own picture size (both editions) and the smallest sheet: a smaller window scales this sheet down.
pub const SIZE: egui::Vec2 = egui::vec2(1440.0, 900.0);
/// The formation strip's least width, pinned to the right edge (its width at 1440x900).
pub const BOARD_W: f32 = 580.0;
/// Space above the pitch inside the strip (no title row any more).
const BOARD_HEAD: f32 = 0.0;
/// The least size, in points, of a hint or a bar label (the theme's Small is 12; hints on a TV-sized seat need more).
const HINT: f32 = 13.0;
/// The bar of page buttons along the bottom of the middle.
const TABS_H: f32 = 56.0;
/// The deck row under the picture keeps at least this; on wide sheets the picture stops growing first.
const DECK_MIN: f32 = 148.0;
/// What the board's height needs per point of pitch width (pitch 1.25, the artwork's bench 0.205) and besides (margins 32,
/// gaps 12, a hint of two lines 36).
const BOARD_PER_W: f32 = 1.455;
const BOARD_EXTRA: f32 = 80.0;
/// The pop-out board's window lays its bench beside the pitch (a column, [`board::COLUMN_ASPECT`]) when its room is at
/// least this wide for its height (3:4 and wider), under the pitch when narrower (9:16).
pub const BENCH_BESIDE: f32 = 2.0 / 3.0;
/// The pop-out board's foot: a gap 4 and one row for DOCK and UNDO 24.
const DESK_FOOT: f32 = 28.0;

/// Points per pixel for a window of `w` x `h` pixels on a monitor of `dpi` (96 = 100%): 1 from 1440x900 up (a bigger
/// window gets a bigger sheet, so the game's picture can grow past its own size), 2 from 2880x1800 (4K), the window's
/// ratio to 1440x900 below that. A monitor Windows scales (120 dpi = 125%) sets it instead, so text is as big as in
/// other programs and drawn at the screen's own pixels, but never so much that the sheet drops under 1440x900 points.
pub fn scale(w: f32, h: f32, dpi: f32) -> f32 {
    scale_from((w / SIZE.x).min(h / SIZE.y), dpi)
}

/// The board window's points per pixel ([`PanelState::desk`]): as [`scale`], against a 900-point square; the board fills
/// the window at any scale, this sizes its words and buttons.
pub fn board_scale(w: f32, h: f32, dpi: f32) -> f32 {
    scale_from(w.min(h) / 900.0, dpi)
}

/// Points per pixel for a window that fits its least sheet `fit` times, on a monitor of `dpi`.
fn scale_from(fit: f32, dpi: f32) -> f32 {
    let monitor = dpi / 96.0;
    if fit < 1.0 {
        fit
    } else if monitor > 1.0 {
        monitor.min(fit)
    } else if fit >= 2.0 {
        2.0
    } else {
        1.0
    }
}

/// Where the parts sit on a sheet: the formation board on the right, the game's picture 16:10 at the top of the
/// middle (centred), the cabinet deck under it, the page bar along the bottom. A pure function of the sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub board: Rect,
    pub middle: Rect,
    pub picture: Rect,
    pub deck: Rect,
    pub tabs: Rect,
    /// Where the pages open: over the picture and the deck (the board window held tall: over the board too).
    pub pages: Rect,
    /// The deck drawn this many times its own size (1 but in [`popped_layout`]).
    pub deck_scale: f32,
}

/// The layout of `sheet` (at least 1440x900): the picture takes the largest 16:10 size that leaves the board its
/// least width and the deck row its least height (compact: no deck); the picture sits flush at the top of the middle,
/// the deck row straight under it. The board then takes what the picture left, up to what its own height can use;
/// the middle takes the rest.
pub fn layout(sheet: Rect, compact: bool) -> Layout {
    let reserve = if compact { 0.0 } else { DECK_MIN };
    let h = ((sheet.width() - BOARD_W) / 1.6).min(sheet.height() - TABS_H - reserve);
    let board_max = (sheet.height() - BOARD_EXTRA) / BOARD_PER_W + 32.0;
    let bw = (sheet.width() - h * 1.6).clamp(BOARD_W, board_max.max(BOARD_W));
    let board = Rect::from_min_max(pos2(sheet.right() - bw, sheet.top()), sheet.max);
    let middle = sheet.with_max_x(board.left());
    let tabs = middle.with_min_y(middle.bottom() - TABS_H);
    let above = middle.with_max_y(tabs.top());
    let picture = Rect::from_center_size(
        pos2(middle.center().x, above.top() + h / 2.0),
        vec2(h * 1.6, h),
    );
    let deck = above.with_min_y(picture.bottom());
    Layout {
        board,
        middle,
        picture,
        deck,
        tabs,
        pages: above,
        deck_scale: 1.0,
    }
}

/// The popped-out game window's deck: its plates stacked, this far apart, with this margin above and under them.
const SIDE_GAP: f32 = 16.0;
const SIDE_UNDER: f32 = 8.0;
/// The popped-out game window's toast line, between the picture and the page bar.
const SIDE_TOAST: f32 = 48.0;
/// The popped-out game window's right column: at least this wide (the deck about 1.3 times its size, a card's large
/// pane fits).
const SIDE_COLUMN: f32 = 440.0;

/// The stacked plates: as wide as the widest plate, as high as the plates one under the other.
fn side_deck(both: bool) -> egui::Vec2 {
    let n = if both { 3.0 } else { 2.0 };
    vec2(PLATES[1].1, n * ROW.y + (n - 1.0) * SIDE_GAP)
}

/// The game window while the board is popped out ([`PanelState::popped`]). Left part: the picture 16:10, centred in
/// what the toast line and the page bar under it leave. Right part, one column ([`SIDE_COLUMN`] wide, wider when the
/// full-height picture leaves more): the deck at its foot, its plates stacked and scaled to the column's width, and
/// over it (`board`) the room where the card touched on the board window shows. `both`: the deck has the machine
/// plate (2010-11). Pages open over the left part.
pub fn popped_layout(sheet: Rect, both: bool) -> Layout {
    let d = side_deck(both);
    let pic_h = sheet.height() - TABS_H - SIDE_TOAST;
    let cw = SIDE_COLUMN.max(sheet.width() - pic_h * 1.6);
    // as wide as the column, never more than 70% of its height (the three 2010-11 plates)
    let s = ((cw - 32.0) / d.x)
        .min((sheet.height() * 0.7 - 2.0 * SIDE_UNDER) / d.y)
        .max(0.0);
    let col = sheet.with_min_x(sheet.right() - cw);
    let deck = col.with_min_y(col.bottom() - d.y * s - 2.0 * SIDE_UNDER);
    let left = sheet.with_max_x(col.left());
    let tabs = left.with_min_y(left.bottom() - TABS_H);
    let above = left.with_max_y(tabs.top());
    let room = above.with_max_y(above.bottom() - SIDE_TOAST);
    let h = (room.width() / 1.6).min(room.height());
    Layout {
        board: col.with_max_y(deck.top()),
        middle: left,
        picture: Rect::from_center_size(room.center(), vec2(h * 1.6, h)),
        deck,
        tabs,
        pages: above,
        deck_scale: s,
    }
}

/// The board window's parts: the board's room and, with `buttons` (`board_buttons=on`), the deck's: its rect, its
/// scale and whether its plates are stacked. A landscape window stacks them in a column at the right (a quarter of the
/// width), a portrait one puts them in a row under the board; either scaled to fill its side.
pub fn desk_parts(sheet: Rect, buttons: bool, both: bool) -> (Rect, Option<(Rect, f32, bool)>) {
    if !buttons {
        return (sheet, None);
    }
    if sheet.width() >= sheet.height() {
        let d = side_deck(both);
        let cw = sheet.width() / 4.0;
        let s = ((cw - 32.0) / d.x)
            .min((sheet.height() - 2.0 * SIDE_UNDER) / d.y)
            .max(0.0);
        let deck = sheet.with_min_x(sheet.right() - cw);
        return (sheet.with_max_x(deck.left()), Some((deck, s, true)));
    }
    let w = if both {
        ROW.x
    } else {
        PLATES[1].0 + PLATES[1].1
    };
    let s = ((sheet.width() - 32.0) / w)
        .min(sheet.height() * 0.2 / ROW.y)
        .max(0.0);
    let deck = sheet.with_min_y(sheet.bottom() - ROW.y * s - 2.0 * SIDE_UNDER);
    (sheet.with_max_y(deck.top()), Some((deck, s, false)))
}

/// The pop-out board in `room` (16 points in from its edges, the DOCK row under it): its rect, as large as the room
/// allows and centred (the rest a bar on two sides), and whether its bench is a column beside the pitch
/// ([`BENCH_BESIDE`]), else a strip under it.
pub fn desk_board_rect(room: Rect) -> (Rect, bool) {
    let r = room.shrink(16.0);
    let r = r.with_max_y(r.bottom() - DESK_FOOT);
    let beside = r.width() >= BENCH_BESIDE * r.height();
    let size = if beside {
        let w = r.width().min(r.height() / board::COLUMN_ASPECT);
        vec2(w, w * board::COLUMN_ASPECT)
    } else {
        let per = board::row_height(1.0) - board::row_height(0.0);
        let w = r.width().min((r.height() - board::row_height(0.0)) / per);
        vec2(w, board::row_height(w))
    };
    let at = Rect::from_center_size(
        r.center() + vec2(0.0, DESK_FOOT / 2.0),
        size + vec2(0.0, DESK_FOOT),
    );
    (at.with_max_y(at.bottom() - DESK_FOOT), beside)
}

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

/// The bindable actions, in the KEYS page's order (`forza::keys::NAMES`): (text key, cabinet colour).
pub const ACTIONS: [(&str, Cab); forza::keys::COUNT] = [
    ("cab.start", Cab::Go),
    ("cab.press", Cab::Press),
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
    ("keys.pause", Cab::Plain),
];

/// The operator's switches (TEST, SERVICE, SW1, SW2): the KEYS page folds them under "Advanced".
const OPERATOR: std::ops::Range<usize> = 12..16;

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

/// The catalogue's words for the search, worked out once: each card's name, short name, club, country, season (also
/// short: 2005-06 as 0506) and printed code (EMVP, WGK-3) folded ([`fold`]), the cards in name order, and the countries and clubs A to Z with their numbers of cards.
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
        // "regular", "normale", "レギュラー": the rarity as the chips name it, in any language
        let rarities =
            Rarity::ALL.map(|r| (r, Lang::ALL.map(|l| tr!(l, rarity_key(r))).join("\n")));
        let folded: Vec<String> = cards
            .iter()
            .map(|c| {
                let rarity = rarities
                    .iter()
                    .find(|(r, _)| *r == c.rarity)
                    .map_or(String::new(), |(_, w)| w.clone());
                let short_season = match c.season.as_bytes() {
                    [_, _, a, b, b'-', c, d] => {
                        String::from_utf8_lossy(&[*a, *b, *c, *d]).into_owned()
                    }
                    _ => String::new(),
                };
                fold(
                    &[
                        &c.name,
                        &c.short,
                        &c.club,
                        &c.country,
                        &c.season,
                        &short_season,
                        &c.code,
                        &rarity,
                    ]
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
    // "12/13" as the season is written, "2012-13"
    let words: Vec<String> = q
        .split_whitespace()
        .map(|w| fold(w).replace('/', "-"))
        .collect();
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
    /// The card view (a full page, front and back at their own pixels): the card shown (an index into the catalogue),
    /// and its back chosen when only one side fits.
    pub view: Option<usize>,
    pub view_back: bool,
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
    secs: f64,
}

const NOTE_SECS: f64 = 4.0;

impl Note {
    pub fn new(tone: Tone, text: String) -> Note {
        Note {
            tone,
            text,
            shown: None,
            secs: NOTE_SECS,
        }
    }

    /// A message with more to read (an import's skipped cards): shown for [`IMPORT_UNDO_SECS`].
    pub fn long(tone: Tone, text: String) -> Note {
        Note {
            secs: IMPORT_UNDO_SECS,
            ..Note::new(tone, text)
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

/// The club card's session as `forza::card::Session` reads it.
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
    /// SAVE LOGS' answer from the run: "ok PATH", "fail WHY".
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
    /// The club's links and the players' affinity, for the board's lines and the partner lists.
    pub links: board::Links,
    /// The player's catalogue (the CATALOGUE page's cards).
    pub cards: Arc<[Card]>,
    /// Seconds before a resting mouse shows the card big; None: never (CARD HOVER DELAY off).
    pub hover_delay: Option<f32>,
    /// CENTRAL, COUNTER, L-SIDE, R-SIDE, PRESS: lit when the order is on.
    pub tactics: [bool; 5],
    pub online: bool,
    /// The round trip to the server's relay (online only); `u32::MAX` = no answer for 6 s.
    pub ping_ms: u32,
    /// The last thing done, refused or worth knowing; one line for every page.
    pub message: Option<Note>,
    pub catalogue: Catalogue,
    /// Each action's key (virtual-key code, 0 none) and controller control (`forza::keys`), in [`ACTIONS`] order.
    pub keys: [u32; forza::keys::COUNT],
    pub pads: [u32; forza::keys::COUNT],
    /// The action whose KEY cell waits for a key; whose CONTROLLER cell waits for a control.
    pub capturing: Option<usize>,
    pub pad_capturing: Option<usize>,
    /// The action changed last (its cells lit).
    pub changed: Option<usize>,
    /// The KEYS page's "Advanced" group is open.
    pub keys_advanced: bool,
    /// The controllers found (KEYS).
    pub pad_names: Vec<String>,
    // settings
    /// data\panel.txt's choices: the next start, the view.
    pub file: PanelFile,
    /// The address line while typing.
    pub address: String,
    /// What SETTINGS shows under NOW, read about once a second while it is open.
    pub status: Status,
    /// RESTART NOW armed by a first click until this time (egui's clock); RESTART NOW / SAVE LOGS asked for (waiting
    /// for the run).
    pub restart_armed: f64,
    pub restart_sent: bool,
    pub logs_sent: bool,
    // club card
    /// data\club_view.txt as last read.
    pub club: ClubView,
    /// The choice armed by a first click, until this time (egui's clock).
    pub club_armed: Option<(ClubPick, f64)>,
    /// A choice asked for by its second click: data\panel.txt's card= or card_fix=, then RESTART NOW.
    pub club_request: Option<ClubPick>,
    /// The window's close was refused during a card session: the warning shows (the panel DLL sets it while it holds).
    pub close_warn: bool,
    /// The close dialog's answers this frame: QUIT clicked (the panel DLL lets the window close), KEEP PLAYING / Esc /
    /// Enter (the dialog goes).
    pub close_quit: bool,
    pub close_keep: bool,
    /// The game stopped under the panel (the control program is gone): the crash screen takes the board's place.
    pub crash: bool,
    /// SAVE LOGS on the crash screen asked: the Launcher opens its report dialog (`ask_open_report` in the panel DLL).
    pub open_report: bool,
    pub report_sent: bool,
    /// A card dealt after a match: shown over the game for [`DEAL_SECS`], or until a click closes it.
    pub deal: Option<Deal>,
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
    /// The card taken off the board last and when (the frame's time): UNDO puts it back for [`UNDO_SECS`].
    pub undo: Option<(board::Removed, f64)>,
    /// KEY PLAYER or KEEPER latched down by a right-click ([`latch_click`]): its key stays down until the next click.
    pub latched: Option<&'static str>,
    /// A catalogue card to put on the table (its number): [`add_card`].
    pub add: Option<u32>,
    /// IMPORT LINEUP clicked: the panel DLL reads the clipboard and calls [`ask_import`].
    pub import: bool,
    /// A lineup read and checked, waiting for the import dialog's CONFIRM or CANCEL.
    pub import_ask: Option<ImportAsk>,
    /// CONFIRM clicked: the panel DLL takes `import_ask` and calls [`import_lineup`].
    pub import_go: bool,
    /// The table before the last import and when (the frame's time): UNDO in the board strip for [`IMPORT_UNDO_SECS`].
    pub import_undo: Option<(Vec<Placed>, f64)>,
    /// That UNDO clicked: the panel DLL calls [`undo_import`].
    pub import_undone: bool,
    /// The bindings changed: keys.txt is written.
    pub keys_changed: bool,
    /// The choices changed: data\panel.txt is written.
    pub file_changed: bool,
    /// RESTART NOW asked (data\restart.request).
    pub restart: bool,
    /// SAVE LOGS asked (data\savelogs.request): the run saves the ZIP in the Downloads folder.
    pub save_logs: bool,
    // what the panel DLL hands a frame
    /// Keys pressed since the last frame while a page is open (virtual-key code, Alt held), repeats left out.
    pub vk_down: Vec<(u32, bool)>,
    /// A controller control pressed since the last frame (KEYS open).
    pub pad_down: Option<u32>,
    // pause (the panel DLL holds the game's frames: `forza::pause`)
    /// PAUSE pressed on the page bar this frame: a pause asked for, or the pause ended.
    pub pause_asked: bool,
    /// Paused: the seconds left before the match goes on by itself; the PAUSED page shows.
    pub paused: Option<u64>,
    /// Drawn in the board's own window (the pop-out board, `file.board_window`): the board alone ([`desk_parts`]).
    pub desk: bool,
    /// The game window's frame while the board window shows the board: [`popped_layout`].
    pub popped: bool,
    /// The card pressed or in hand on the board window last, and when (the frame's time): the game window shows it
    /// for [`TOUCH_SECS`] after.
    pub touched: Option<(Card, f64)>,
}

/// How long the game window keeps showing the card touched on the board window, once the touch ended.
pub const TOUCH_SECS: f64 = 6.0;

impl PanelState {
    /// The panel at rest: the game page, no message, the default keys, the board and the catalogue given.
    pub fn new(edition: Edition, lang: Lang, board: Board, cards: Arc<[Card]>) -> Self {
        PanelState {
            edition,
            lang,
            page: Page::Game,
            board,
            links: board::Links::default(),
            cards,
            hover_delay: Some(0.5),
            tactics: [false; 5],
            online: false,
            ping_ms: 0,
            message: None,
            catalogue: Catalogue::default(),
            keys: forza::keys::defaults(edition == Edition::E1718),
            pads: [0; forza::keys::COUNT],
            capturing: None,
            pad_capturing: None,
            changed: None,
            keys_advanced: false,
            pad_names: Vec::new(),
            file: PanelFile::default(),
            address: String::new(),
            status: Status::default(),
            restart_armed: 0.0,
            restart_sent: false,
            logs_sent: false,
            club: ClubView::default(),
            club_armed: None,
            club_request: None,
            close_warn: false,
            close_quit: false,
            close_keep: false,
            crash: false,
            open_report: false,
            report_sent: false,
            deal: None,
            picture: None,
            picture_rect: None,
            board_changed: false,
            pressed: None,
            held: None,
            latched: None,
            undo: None,
            add: None,
            import: false,
            import_ask: None,
            import_go: false,
            import_undo: None,
            import_undone: false,
            keys_changed: false,
            file_changed: false,
            restart: false,
            save_logs: false,
            vk_down: Vec::new(),
            pad_down: None,
            pause_asked: false,
            paused: None,
            desk: false,
            popped: false,
            touched: None,
        }
    }

    /// A message for the line every page shows.
    pub fn note(&mut self, tone: Tone, text: String) {
        self.message = Some(Note::new(tone, text));
    }

    /// The game window's frame popped out or not ([`PanelState::popped`]); a dock or a pop-out forgets the card touched
    /// last, so its details don't come back.
    pub fn set_popped(&mut self, popped: bool) {
        if self.popped != popped {
            self.touched = None;
        }
        self.popped = popped;
    }

    /// A message by its text key, in the panel's language.
    pub fn say(&mut self, tone: Tone, key: &str) {
        self.note(tone, tr!(self.lang, key));
    }

    pub fn sample(edition: Edition, lang: Lang, page: Page) -> Self {
        let msg = match page {
            Page::Catalogue => (Tone::Done, tr!(lang, "panel.msg_added")),
            Page::ClubCard => (Tone::Refused, tr!(lang, "panel.msg_session_open")),
            _ => (Tone::Done, tr!(lang, "panel.msg_saved")),
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
                0, 0, 0, 0,
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
                logs: Some(
                    r"ok C:\Users\Mario\Downloads\forza-wccf-logs-20261027-150412.zip".into(),
                ),
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
    // a team holds one card per player: refused while another card of this one is on the table
    let other = placed.iter().filter(|p| p.no != no).find_map(|p| {
        st.cards
            .iter()
            .find(|c| c.number == p.no && c.same_player(card))
    });
    if let Some(other) = other {
        let text = tr!(
            lang,
            "cat.msg_same_player",
            name = name,
            season = other.season
        );
        st.note(Tone::Refused, text);
        return false;
    }
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

/// How long an import's UNDO and its message stay.
pub const IMPORT_UNDO_SECS: f64 = 10.0;

/// A lineup waiting for the import dialog: its text and what it would put on the table.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportAsk {
    pub text: String,
    /// The cards that would be placed and those skipped.
    pub cards: usize,
    pub skipped: usize,
}

/// The lineup `text` checked as [`import_lineup`] checks it; a good one waits in `import_ask` for the import dialog's
/// CONFIRM. `false`: refused, the message says why.
pub fn ask_import(st: &mut PanelState, text: &str, in_match: bool) -> bool {
    let Some(imp) = checked_lineup(st, text, in_match) else {
        return false;
    };
    st.import_ask = Some(ImportAsk {
        text: text.to_owned(),
        cards: imp.placed.len(),
        skipped: imp.skipped.len(),
    });
    true
}

/// The lineup `text` (the site's export: the clipboard's, later a share link's) as the whole table in place of
/// `placed`. Refused during a match (the owner's rule) and for another edition's lineup. The message names what came
/// in, the site's key player (the table has none: the player sets it in the game) and the cards skipped, and why.
/// `Some(new table)`: the caller writes it; the old one waits in `import_undo` for [`undo_import`].
pub fn import_lineup(
    st: &mut PanelState,
    placed: &[Placed],
    text: &str,
    in_match: bool,
    now: f64,
) -> Option<Vec<Placed>> {
    let lang = st.lang;
    let imp = checked_lineup(st, text, in_match)?;
    let name = imp.name.clone().unwrap_or_else(|| tr!(lang, "cat.lineup"));
    let mut text = tr!(lang, "cat.msg_imported", name = name, n = imp.placed.len());
    if let Some(kp) = imp.kp.and_then(|k| st.cards.iter().find(|c| c.number == k)) {
        text += "\n";
        text += &tr!(lang, "cat.msg_import_kp", name = kp.short_name());
    }
    if !imp.skipped.is_empty() {
        text += "\n";
        text += &tr!(
            lang,
            "cat.msg_import_skipped",
            list = skip_list(&imp.skipped, lang)
        );
    }
    let tone = if imp.skipped.is_empty() {
        Tone::Done
    } else {
        Tone::Notice
    };
    st.message = Some(Note::long(tone, text));
    st.board = cards::board(&imp.placed, &st.cards);
    st.import_undo = Some((placed.to_vec(), now));
    st.undo = None; // UNDO now means the import
    Some(imp.placed)
}

/// The lineup `text` on this game's catalogue, or `None` with the message saying why: refused in a match, for another
/// edition, empty, or nothing this game has.
fn checked_lineup(st: &mut PanelState, text: &str, in_match: bool) -> Option<lineup::Import> {
    let lang = st.lang;
    if in_match {
        st.say(Tone::Refused, "cat.msg_import_match");
        return None;
    }
    let l = match lineup::parse(text, st.edition) {
        Ok(l) => l,
        Err(LineupError::WrongEdition(g)) => {
            // the site writes 0203, 0506, 1011, 1718
            let game = match g.as_bytes() {
                [_, _, _, _] if g.bytes().all(|b| b.is_ascii_digit()) => {
                    format!("20{}-{}", &g[..2], &g[2..])
                }
                _ => g,
            };
            st.note(
                Tone::Refused,
                tr!(lang, "cat.msg_import_edition", game = game),
            );
            return None;
        }
        Err(LineupError::Empty) => {
            st.say(Tone::Refused, "cat.msg_import_empty");
            return None;
        }
    };
    match lineup::table(&l, &st.cards) {
        Ok(imp) => Some(imp),
        Err(_) => {
            // nothing placed: every entry the text named is missing from this game's catalogue
            let mut skipped = l.skipped.clone();
            skipped.extend(l.pitch.iter().chain(&l.bench).map(|e| Skipped {
                label: e.label.clone(),
                why: Skip::NotInCatalogue,
            }));
            let text = format!(
                "{}\n{}",
                tr!(lang, "cat.msg_import_none"),
                skip_list(&skipped, lang)
            );
            st.message = Some(Note::long(Tone::Refused, text));
            None
        }
    }
}

/// The table before the last import, back on the board; `Some`: the caller writes it. Refused during a match, as the
/// import is (the UNDO line stays while its time lasts).
pub fn undo_import(st: &mut PanelState, in_match: bool) -> Option<Vec<Placed>> {
    st.import_undo.as_ref()?;
    if in_match {
        st.say(Tone::Refused, "cat.msg_import_match");
        return None;
    }
    let (old, _) = st.import_undo.take()?;
    st.board = cards::board(&old, &st.cards);
    Some(old)
}

/// "NAME (why), ...", the first 8, then "+N more".
fn skip_list(skipped: &[Skipped], lang: Lang) -> String {
    let mut list: Vec<String> = skipped
        .iter()
        .take(8)
        .map(|s| {
            let why = match s.why {
                Skip::NoGameNumber => "cat.skip_no_number",
                Skip::BadLine => "cat.skip_bad_line",
                Skip::Duplicate => "cat.skip_duplicate",
                Skip::NotInCatalogue => "cat.skip_not_in_catalogue",
                Skip::NoRoom => "cat.skip_no_room",
            };
            format!("{} ({})", s.label, tr!(lang, why))
        })
        .collect();
    if skipped.len() > 8 {
        list.push(tr!(lang, "board.more", n = skipped.len() - 8));
    }
    list.join(", ")
}

/// The centre of a formation slot (0-10 the pitch, 11-15 the bench) on a sheet `width` points wide whose board has
/// these spots (for tests that drag cards).
pub fn board_slot_center(width: f32, spots: &[Pos2; 11], slot: usize) -> Pos2 {
    let r = layout(Rect::from_min_size(Pos2::ZERO, vec2(width, SIZE.y)), false)
        .board
        .shrink(16.0);
    let pitch = board::pitch_rect(r.min + vec2(0.0, BOARD_HEAD), r.width());
    board::slot_rects(pitch, pitch.bottom() + 12.0, spots)[slot].center()
}

pub fn show(ui: &mut Ui, st: &mut PanelState) {
    let lang = st.lang;
    let sheet = ui.max_rect();
    if st.desk {
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| board_window(ui, st, lang, sheet));
        return;
    }
    let lay = if st.popped {
        popped_layout(sheet, st.edition != Edition::E1718)
    } else {
        layout(sheet, st.file.compact)
    };
    if !st.popped {
        egui::Panel::right("panel-board")
            .exact_size(lay.board.width())
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                if st.crash {
                    crash_screen(ui, st, lang);
                } else {
                    board_strip(ui, st, lang);
                }
            });
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::WELL))
        .show(ui, |ui| middle(ui, st, lang, &lay));
}

// ---- the middle: picture, deck, page bar ----

fn middle(ui: &mut Ui, st: &mut PanelState, lang: Lang, lay: &Layout) {
    let Layout {
        middle: area,
        picture: pic,
        deck,
        tabs,
        pages: above,
        deck_scale,
        ..
    } = *lay;
    let side = st.popped;
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
    for r in [deck, tabs] {
        p.line_segment(
            [
                pos2(r.left(), r.top() + 0.5),
                pos2(r.right(), r.top() + 0.5),
            ],
            Stroke::new(1.0, theme::RIM),
        );
    }
    // the edge between the middle and the board (popped out: the left part and the column)
    p.line_segment(
        [
            pos2(area.right() - 0.5, area.top()),
            pos2(area.right() - 0.5, area.bottom()),
        ],
        Stroke::new(1.0, theme::RIM),
    );
    game_picture(ui, pic, st.picture);
    // the dealt card covers the picture: clicks there are its own
    st.picture_rect = (st.page == Page::Game && st.deal.is_none() && !st.close_warn).then_some(pic);
    // popped out the deck is the column's: COMPACT is the docked layout's choice
    let compact = st.file.compact && !side;
    if st.page == Page::Game {
        // a toast in the gap above the deck row when there is one, else over the picture's lower edge (popped out:
        // over the page bar)
        let gap = (deck.height() - ROW.y) / 2.0;
        let (x, y) = if side {
            (tabs.center().x, tabs.top() - SIDE_TOAST / 2.0)
        } else if !compact && gap >= 40.0 {
            (deck.center().x, deck.top() + gap / 2.0)
        } else {
            (deck.center().x, pic.bottom() - 30.0)
        };
        let r = Rect::from_center_size(pos2(x, y), vec2(520.0, 34.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
            note_line(ui, &mut st.message)
        });
    }
    if !compact {
        cabinet_deck(ui, st, deck, lang, deck_scale, side);
    } else {
        st.latched = None; // no deck: no button left to let it go
    }
    page_bar(ui, st, tabs, lang, side);
    // popped out: the card touched on the board window, over the deck in the column
    if side && !st.close_warn {
        touched_card(ui, st, lay.board.shrink(16.0), lang);
    }
    // before the page: Enter and Esc answer the dialog, not the catalogue's search line or the page's close
    if st.import_ask.is_some() {
        import_dialog(ui, st, lang);
    }
    if st.page != Page::Game {
        let mut pg = ui.new_child(egui::UiBuilder::new().max_rect(above));
        page(&mut pg, st, lang);
    }
    deal_window(ui, st, above, lang);
    if let Some(left) = st.paused {
        let key = forza::keys::key_label(st.keys[forza::keys::PAUSE]);
        paused(ui, pic, lang, &key, left);
    }
    if st.close_warn {
        close_dialog(ui, st, lang);
    }
}

/// The card touched on the board window, its details in `area` (the popped-out column over the deck), until
/// [`TOUCH_SECS`] after the touch.
fn touched_card(ui: &Ui, st: &mut PanelState, area: Rect, lang: Lang) {
    let now = ui.input(|i| i.time);
    st.touched = st.touched.take().filter(|(_, at)| now - at < TOUCH_SECS);
    if let Some((card, at)) = &st.touched {
        board::card_view(ui, card, area, lang);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(TOUCH_SECS - (now - at)));
    }
}

/// The PAUSED page over the game's picture (the game's last frame stays under it): how to resume and the seconds left
/// before the match goes on by itself.
fn paused(ui: &Ui, pic: Rect, lang: Lang, key: &str, left: u64) {
    let p = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("paused"),
    ));
    p.rect_filled(pic, 0, Color32::from_black_alpha(150));
    let r = Rect::from_center_size(pic.center(), vec2((pic.width() - 40.0).min(560.0), 150.0));
    p.rect_filled(
        r,
        theme::RADIUS_BIG,
        Color32::from_rgba_unmultiplied(10, 16, 28, 240),
    );
    p.rect_stroke(
        r,
        theme::RADIUS_BIG,
        Stroke::new(3.0, theme::GOLD),
        StrokeKind::Inside,
    );
    let w = r.width() - 32.0;
    let lines = [
        (tr!(lang, "pause.title"), theme::display(36.0), theme::GOLD),
        (
            tr!(lang, "pause.body", key = key),
            theme::body(16.0),
            theme::TEXT,
        ),
        (
            tr!(lang, "pause.left", s = left),
            theme::bold(16.0),
            theme::TEXT_DIM,
        ),
    ];
    for (i, (text, font, col)) in lines.into_iter().enumerate() {
        let g = widgets::fit(&p, &text, font, col, w);
        let y = r.top() + 34.0 + 42.0 * i as f32;
        p.galley(
            pos2(r.center().x - g.size().x / 2.0, y - g.size().y / 2.0),
            g,
            col,
        );
    }
}

/// The close dialog over everything: a match is on, closing now costs the card a bad ending. KEEP PLAYING is the
/// default (focused; Enter and Esc take it), QUIT needs its own click. The panel DLL ignores further close requests
/// while the dialog is open.
fn close_dialog(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let edition = st.edition;
    let modal = egui::Modal::new(egui::Id::new("close-dialog"))
        .backdrop_color(Color32::from_black_alpha(170))
        .frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(24, 14, 14))
                .stroke(Stroke::new(3.0, theme::RED_TEXT))
                .corner_radius(theme::RADIUS_BIG)
                .inner_margin(24),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(440.0);
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                ui.label(
                    egui::RichText::new(tr!(lang, "close.title"))
                        .font(theme::display(32.0))
                        .color(theme::RED_TEXT),
                );
                // 2017-18 saves on the server: no card to damage
                let body = if edition == Edition::E1718 {
                    "close.body_1718"
                } else {
                    "close.body"
                };
                ui.label(
                    egui::RichText::new(tr!(lang, body))
                        .font(theme::body(17.0))
                        .color(theme::TEXT),
                );
                ui.add_space(8.0);
                let keep = widgets::cabinet(
                    ui,
                    vec2(440.0, 56.0),
                    &tr!(lang, "close.keep"),
                    Cab::Go,
                    true,
                );
                // the default: focused whenever the dialog is up
                keep.request_focus();
                let quit = widgets::cabinet(
                    ui,
                    vec2(440.0, 44.0),
                    &tr!(lang, "close.quit"),
                    Cab::Stop,
                    false,
                );
                (keep, quit)
            })
            .inner
        });
    let (keep, quit) = modal.inner;
    let key = ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape));
    if quit.clicked() {
        st.close_quit = true;
    } else if keep.clicked() || key {
        st.close_keep = true;
    }
}

/// IMPORT LINEUP's question before the table is replaced: how many cards come in and how many are skipped. CONFIRM
/// (or Enter, the cabinet's START) asks the panel DLL to import; CANCEL (or Esc) leaves the table as it is.
fn import_dialog(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    let Some(ask) = st.import_ask.clone() else {
        return;
    };
    let modal = egui::Modal::new(egui::Id::new("import-dialog"))
        .backdrop_color(Color32::from_black_alpha(170))
        .frame(
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(10, 16, 28, 240))
                .stroke(Stroke::new(3.0, theme::GOLD))
                .corner_radius(theme::RADIUS_BIG)
                .inner_margin(24),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(440.0);
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                ui.label(
                    egui::RichText::new(tr!(lang, "cat.ask_title"))
                        .font(theme::display(32.0))
                        .color(theme::GOLD),
                );
                let mut body = tr!(lang, "cat.ask_body", n = ask.cards);
                if ask.skipped > 0 {
                    body += "\n";
                    body += &tr!(lang, "cat.ask_skipped", n = ask.skipped);
                }
                ui.label(
                    egui::RichText::new(body)
                        .font(theme::body(17.0))
                        .color(theme::TEXT),
                );
                ui.add_space(8.0);
                let go = widgets::cabinet(
                    ui,
                    vec2(440.0, 56.0),
                    &tr!(lang, "cat.ask_confirm"),
                    Cab::Go,
                    true,
                );
                let cancel = widgets::cabinet(
                    ui,
                    vec2(440.0, 44.0),
                    &tr!(lang, "cat.ask_cancel"),
                    Cab::Plain,
                    false,
                );
                (go, cancel)
            })
            .inner
        });
    let (go, cancel) = modal.inner;
    let enter = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let esc = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if cancel.clicked() || esc {
        st.import_ask = None;
    } else if go.clicked() || enter {
        st.import_go = true;
    }
}

// ---- the card deal ----

/// How long a dealt card stays (`wccfpanel.c` DEAL_MS).
pub const DEAL_SECS: f64 = 20.0;

/// A card dealt after a match: its index in the catalogue, when it goes (egui's clock, set when first shown).
#[derive(Clone, Debug, PartialEq)]
pub struct Deal {
    pub card: usize,
    pub until: Option<f64>,
    /// Turned over: its back.
    pub back: bool,
}

impl Deal {
    pub fn new(card: usize) -> Deal {
        Deal {
            card,
            until: None,
            back: false,
        }
    }
}

/// The dealt card's window over the middle, as `wccfpanel.c` `deal_draw`: NEW PLAYER CARD (RARE! for a card better
/// than black), the card big, its name, club, stats and TOTAL; it says it is only shown. A click on it closes it.
fn deal_window(ui: &mut Ui, st: &mut PanelState, area: Rect, lang: Lang) {
    let Some(d) = st.deal.as_mut() else { return };
    let Some(card) = st.cards.get(d.card).cloned() else {
        st.deal = None;
        return;
    };
    let now = ui.input(|i| i.time);
    let until = *d.until.get_or_insert(now + DEAL_SECS);
    if now >= until {
        st.deal = None;
        return;
    }
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs_f64(until - now));
    let back = d.back;
    let rare = cards::deal_class(&card) == 0;
    let col = match cards::deal_class(&card) {
        0 => theme::GOLD,
        2 => theme::TEXT_DIM,
        _ => theme::TEXT,
    };
    let size = vec2(360.0, 640.0f32.min(area.height() - 20.0));
    let r = Rect::from_center_size(area.center(), size);
    let resp = egui::Area::new(egui::Id::new("deal"))
        .order(egui::Order::Foreground)
        .fixed_pos(r.min)
        .show(ui.ctx(), |ui| {
            let (r, resp) = ui.allocate_exact_size(size, Sense::click());
            let p = ui.painter().clone();
            p.add(
                egui::Shadow {
                    offset: [0, 10],
                    blur: 30,
                    spread: 0,
                    color: Color32::from_black_alpha(180),
                }
                .as_shape(r, theme::RADIUS_BIG),
            );
            p.rect_filled(r, theme::RADIUS_BIG, theme::DEEP);
            p.rect_stroke(
                r,
                theme::RADIUS_BIG,
                Stroke::new(3.0, col),
                StrokeKind::Inside,
            );
            let title = tr!(
                lang,
                if rare {
                    "deal.title_rare"
                } else {
                    "deal.title"
                }
            );
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &title));
            let g = widgets::fit(&p, &title, theme::display(28.0), col, size.x - 24.0);
            p.galley(
                pos2(r.center().x - g.size().x / 2.0, r.top() + 14.0),
                g,
                col,
            );
            let face = Rect::from_center_size(
                pos2(r.center().x, r.top() + 60.0 + 140.0),
                vec2(200.0, 280.0),
            );
            if back {
                board::card_back(&p, face, &card, lang);
            } else {
                board::card_face(&p, face, &card, board::Look::default());
            }
            let mut y = face.bottom() + 12.0;
            let g = widgets::fit(
                &p,
                &card.name,
                theme::display(22.0),
                theme::TEXT,
                size.x - 24.0,
            );
            p.galley(pos2(r.center().x - g.size().x / 2.0, y), g, theme::TEXT);
            y += 28.0;
            let line = format!("{} · {} · {}", card.club, card.country, card.season);
            let g = widgets::fit(&p, &line, theme::body(HINT), theme::TEXT_DIM, size.x - 24.0);
            p.galley(pos2(r.center().x - g.size().x / 2.0, y), g, theme::TEXT_DIM);
            y += 24.0;
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(
                pos2(r.left() + 24.0, y),
                pos2(r.right() - 24.0, r.bottom() - 34.0),
            )));
            c.spacing_mut().item_spacing.y = 3.0;
            let w = c.available_width();
            let names = STATS.map(|k| tr!(lang, k));
            widgets::stat_bars(&mut c, &card, &names, &tr!(lang, "cat.secret"), w);
            c.label(
                egui::RichText::new(format!("{}  {}", tr!(lang, "cat.total"), card.total))
                    .font(theme::display(20.0))
                    .color(theme::AMBER),
            );
            let g = p.layout(
                tr!(lang, "deal.foot"),
                theme::body(HINT),
                theme::TEXT_DIM,
                size.x - 32.0,
            );
            p.galley(
                pos2(
                    r.center().x - g.size().x / 2.0,
                    r.bottom() - 10.0 - g.size().y,
                ),
                g,
                theme::TEXT_DIM,
            );
            // a click on the card turns it over; anywhere else closes the window
            let on_card = resp.clicked()
                && resp
                    .interact_pointer_pos()
                    .is_some_and(|p| face.contains(p));
            (resp, on_card)
        })
        .inner;
    if resp.0.clicked() {
        match st.deal.as_mut() {
            Some(d) if resp.1 => d.back = !d.back,
            _ => st.deal = None,
        }
    }
}

/// One button of the cabinet's two plates: where it sits and how it looks. `label` is the side a small button's name
/// is printed on (the big ones carry it inside); `tactic` indexes `PanelState::tactics`.
pub struct DeckItem {
    pub key: &'static str,
    pub action: &'static str,
    pub kind: Cab,
    pub shape: Shape,
    pub rect: Rect,
    pub label: Option<Dir>,
    pub tactic: Option<usize>,
}

/// The deck row: three plates side by side (tactics, play keys, machine), 778 points wide, 144 high.
const ROW: egui::Vec2 = vec2(778.0, 144.0);
/// Each plate's left edge and width in the row.
const PLATES: [(f32, f32); 3] = [(0.0, 300.0), (316.0, 328.0), (660.0, 118.0)];

/// The three plates, the row drawn centred in `area` and `s` times its size: without the machine plate (`both` false,
/// 2017-18) the row is only the first two plates wide. `stack` (the side layout): one plate under the other, each
/// centred across.
fn deck_plates(area: Rect, both: bool, s: f32, stack: bool) -> [Rect; 3] {
    let c = area.center();
    if stack {
        let top = c.y - side_deck(both).y / 2.0;
        let mut i = 0.0;
        return PLATES.map(|(_, w)| {
            let r = Rect::from_min_size(
                pos2(c.x - w / 2.0, top + i * (ROW.y + SIDE_GAP)),
                vec2(w, ROW.y),
            );
            i += 1.0;
            grow(r, c, s)
        });
    }
    let w = if both {
        ROW.x
    } else {
        PLATES[1].0 + PLATES[1].1
    };
    let o = c - vec2(w, ROW.y) / 2.0;
    PLATES.map(|(x, w)| grow(Rect::from_min_size(o + vec2(x, 0.0), vec2(w, ROW.y)), c, s))
}

/// `r` drawn `s` times its size about `c` (the side layout's deck).
fn grow(r: Rect, c: Pos2, s: f32) -> Rect {
    Rect::from_min_max(c + (r.min - c) * s, c + (r.max - c) * s)
}

/// The cabinet's buttons in one row (a departure from the cabinet on purpose, to leave the picture room). Left: a
/// diamond of yellow triangles (CENTRAL up, COUNTER down, L-SIDE left, R-SIDE right) round a red PRESS square, a
/// small green DATA square beside it. Middle: small yellow KEY PLAYER, big red SHOOT, big blue KEEPER, small green
/// START in a line.
pub fn deck_layout(area: Rect) -> Vec<DeckItem> {
    deck_layout_for(area, true, 1.0, false)
}

/// [`deck_layout`] for an edition with (`both`) or without the machine plate.
fn deck_layout_for(area: Rect, both: bool, s: f32, stack: bool) -> Vec<DeckItem> {
    let [left, mid, _] = deck_plates(area, both, 1.0, stack);
    let (cy, my) = (left.center().y, mid.center().y);
    let c = pos2(left.left() + 132.0, cy);
    let (sq, tl, g) = (40.0, 26.0, 2.0);
    let off = sq / 2.0 + g + tl / 2.0;
    let item = |key, action, kind, shape, centre: Pos2, size: egui::Vec2, label, tactic| DeckItem {
        key,
        action,
        kind,
        shape,
        rect: grow(Rect::from_center_size(centre, size), area.center(), s),
        label,
        tactic,
    };
    let (tall, wide) = (vec2(sq, tl), vec2(tl, sq));
    let tri = |d| Shape::Tri(d);
    let (big, small) = (80.0, 34.0);
    // the line: KEY PLAYER (its label is wider than the key, so 32 each side), SHOOT, KEEPER, START
    let key_x = mid.left() + 32.0 + small / 2.0;
    let shoot_x = key_x + small / 2.0 + 32.0 + big / 2.0;
    let keeper_x = shoot_x + big + 12.0;
    let start_x = keeper_x + big / 2.0 + 12.0 + small / 2.0;
    vec![
        item(
            "cab.central",
            "UP",
            Cab::Tactic,
            tri(Dir::Up),
            c - vec2(0.0, off),
            tall,
            Some(Dir::Up),
            Some(0),
        ),
        item(
            "cab.counter",
            "DOWN",
            Cab::Tactic,
            tri(Dir::Down),
            c + vec2(0.0, off),
            tall,
            Some(Dir::Down),
            Some(1),
        ),
        item(
            "cab.l_side",
            "LEFT",
            Cab::Tactic,
            tri(Dir::Left),
            c - vec2(off, 0.0),
            wide,
            Some(Dir::Left),
            Some(2),
        ),
        item(
            "cab.r_side",
            "RIGHT",
            Cab::Tactic,
            tri(Dir::Right),
            c + vec2(off, 0.0),
            wide,
            Some(Dir::Right),
            Some(3),
        ),
        item(
            "cab.press",
            "PRESS",
            Cab::Press,
            Shape::Square,
            c,
            vec2(sq, sq),
            None,
            Some(4),
        ),
        item(
            "cab.data",
            "DATA",
            Cab::Data,
            Shape::Square,
            pos2(left.right() - 19.0, cy),
            vec2(30.0, 30.0),
            Some(Dir::Down),
            None,
        ),
        item(
            "cab.key_player",
            "KEYPLAYER",
            Cab::KeyPlayer,
            Shape::Round,
            pos2(key_x, my),
            vec2(small, small),
            Some(Dir::Down),
            None,
        ),
        item(
            "cab.start",
            "START",
            Cab::Go,
            Shape::Round,
            pos2(start_x, my),
            vec2(small, small),
            Some(Dir::Down),
            None,
        ),
        item(
            "cab.shoot",
            "SHOOT",
            Cab::Shoot,
            Shape::Round,
            pos2(shoot_x, my),
            vec2(big, big),
            None,
            None,
        ),
        item(
            "cab.keeper",
            "KEEPER",
            Cab::Keeper,
            Shape::Round,
            pos2(keeper_x, my),
            vec2(big, big),
            None,
            None,
        ),
    ]
}

/// The machine plate's one key, CARD, centred (2017-18 has no card and no machine plate).
fn card_key(area: Rect, s: f32, stack: bool) -> Rect {
    Rect::from_center_size(
        deck_plates(area, true, s, stack)[2].center(),
        vec2(102.0, 32.0) * s,
    )
}

const KEYCAP_H: f32 = 13.0;

/// The key bound to a deck button as its keycap text ("X", "Enter"), else its controller control ("Button 3"); empty
/// when neither is bound, or while `alt` is not held (the keycaps show only under Alt).
fn deck_keycap(st: &PanelState, action: &str, alt: bool) -> String {
    let Some(i) = forza::keys::NAMES.iter().position(|n| *n == action) else {
        return String::new();
    };
    if !alt {
        return String::new();
    }
    match (st.keys[i], st.pads[i]) {
        (0, 0) => String::new(),
        (0, pad) => forza::keys::pad_label(pad),
        (key, _) => forza::keys::key_label(key),
    }
}

/// A keycap's width for `text`, at most `max` (the text shrinks to fit); 0 when there is no text.
fn keycap_width(p: &egui::Painter, text: &str, max: f32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let g = widgets::fit(p, text, theme::body(10.0), theme::TEXT_DIM, max - 8.0);
    (g.size().x + 8.0).max(KEYCAP_H)
}

fn paint_keycap(p: &egui::Painter, r: Rect, text: &str) {
    p.rect_filled(r, 3, theme::DEEP);
    p.rect_stroke(r, 3, Stroke::new(1.0, theme::RIM), StrokeKind::Inside);
    let g = widgets::fit(p, text, theme::body(10.0), theme::TEXT_DIM, r.width() - 8.0);
    p.galley(r.center() - g.size() / 2.0, g, theme::TEXT_DIM);
}

/// The cabinet's controls under the picture, one row: the tactic diamond, the play buttons, the machine.
fn cabinet_deck(ui: &mut Ui, st: &mut PanelState, area: Rect, lang: Lang, s: f32, stack: bool) {
    // a press sends the action's key down (the panel DLL lets it up after the release); the buttons do not light.
    // A right-click on KEY PLAYER or KEEPER latches it down until the next click on it ([`latch_click`]).
    let mut pressed = None;
    let held = st.latched;
    let mut latch = st.latched;
    let alt = ui.input(|i| i.modifiers.alt); // the keycaps show only while Alt is held
    let mut put = |ui: &mut Ui,
                   r: Rect,
                   key: &str,
                   action: Option<&'static str>,
                   kind: Cab,
                   shape: Shape,
                   lit: bool,
                   inside: bool| {
        let resp = ui.put(r, |ui: &mut Ui| {
            cabinet_shaped(ui, r.size(), &tr!(lang, key), kind, shape, lit, inside)
        });
        let hit = |b| {
            ui.input(|i| {
                i.pointer.button_pressed(b)
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| shape_contains(shape, r, p))
            })
        };
        if let Some(a) = action
            && ui.is_enabled()
        {
            let (left, right) = (
                hit(egui::PointerButton::Primary),
                hit(egui::PointerButton::Secondary),
            );
            if left || right {
                let next = latch_click(latch, a, right);
                if left && next == latch {
                    pressed = action;
                }
                latch = next;
            }
        }
        resp
    };
    let both = st.edition != Edition::E1718; // 2017-18 has no card: no machine plate
    for plate in deck_plates(area, both, s, stack)
        .into_iter()
        .take(if both { 3 } else { 2 })
    {
        ui.painter()
            .rect_filled(plate, theme::RADIUS_BIG, theme::WELL);
        ui.painter().rect_stroke(
            plate,
            theme::RADIUS_BIG,
            Stroke::new(1.0, theme::RIM),
            StrokeKind::Inside,
        );
    }
    for it in deck_layout_for(area, both, s, stack) {
        let on = held == Some(it.action);
        let lit = on || it.tactic.is_some_and(|i| st.tactics[i]);
        // a name printed beside the button is not printed in it too (the side layout's big buttons)
        let inside = it.label.is_none();
        put(
            ui,
            it.rect,
            it.key,
            Some(it.action),
            it.kind,
            it.shape,
            lit,
            inside,
        );
        if on {
            let p = ui.painter();
            p.circle_stroke(
                it.rect.center(),
                it.rect.width() / 2.0 + 5.0,
                Stroke::new(2.5, theme::GOLD),
            );
            p.text(
                it.rect.center_top() - vec2(0.0, 7.0),
                Align2::CENTER_BOTTOM,
                tr!(lang, "cab.held"),
                theme::display(13.0),
                theme::GOLD,
            );
        }
        let cap = deck_keycap(st, it.action, alt);
        let p = ui.painter();
        if let Some(side) = it.label {
            // the name, and the keycap after it, on one line beside the button
            let name = p.layout_no_wrap(tr!(lang, it.key), theme::display(14.0), theme::TEXT_DIM);
            let room = match side {
                Dir::Left => it.rect.left() - deck_plates(area, both, s, stack)[0].left() - 8.0,
                _ => 110.0,
            };
            let cap_w = keycap_width(p, &cap, room - name.size().x - 4.0);
            let w = name.size().x + if cap_w > 0.0 { 4.0 + cap_w } else { 0.0 };
            let h = name.size().y;
            let (left, cy) = match side {
                Dir::Up => (it.rect.center().x - w / 2.0, it.rect.top() - 4.0 - h / 2.0),
                Dir::Down => (
                    it.rect.center().x - w / 2.0,
                    it.rect.bottom() + 4.0 + h / 2.0,
                ),
                Dir::Left => (it.rect.left() - 4.0 - w, it.rect.center().y),
                Dir::Right => (it.rect.right() + 4.0, it.rect.center().y),
            };
            // never past the plate's edge (DATA sits close to it)
            let plate = deck_plates(area, both, s, stack)
                .into_iter()
                .find(|r| r.contains(it.rect.center()))
                .unwrap_or(area);
            let left = left.min(plate.right() - 4.0 - w);
            p.galley(pos2(left, cy - h / 2.0), name.clone(), theme::TEXT_DIM);
            if cap_w > 0.0 {
                let r = Rect::from_min_size(
                    pos2(left + name.size().x + 4.0, cy - KEYCAP_H / 2.0),
                    vec2(cap_w, KEYCAP_H),
                );
                paint_keycap(p, r, &cap);
            }
        } else {
            // the big buttons carry their name: the keycap sits inside, at the bottom
            let cap_w = keycap_width(p, &cap, it.rect.width() - 10.0);
            if cap_w > 0.0 {
                let r = Rect::from_center_size(
                    pos2(
                        it.rect.center().x,
                        it.rect.bottom() - 2.0 - KEYCAP_H / 2.0 - 2.0,
                    ),
                    vec2(cap_w, KEYCAP_H),
                );
                paint_keycap(p, r, &cap);
            }
        }
    }
    // the machine: CARD only (free play needs no COIN, and BACK sent nothing; 2017-18 has no card to put in)
    if both {
        put(
            ui,
            card_key(area, s, stack),
            "cab.card",
            Some("CARD"),
            Cab::Plain,
            Shape::Key,
            false,
            true,
        );
        let cap = deck_keycap(st, "CARD", alt);
        let p = ui.painter();
        let w = keycap_width(p, &cap, 100.0);
        if w > 0.0 {
            let r = Rect::from_center_size(
                card_key(area, s, stack).center_bottom() + vec2(0.0, 4.0 + KEYCAP_H / 2.0),
                vec2(w, KEYCAP_H),
            );
            paint_keycap(p, r, &cap);
        }
    }
    st.pressed = st.pressed.or(pressed);
    st.latched = latch;
}

/// The buttons a right-click holds down until the next click on them: one mouse cannot hold KEY PLAYER (or KEEPER)
/// and rub a card at once.
pub const LATCHING: [&str; 2] = ["KEYPLAYER", "KEEPER"];

/// The latch after a click on the cabinet button `action` (`right`: the right button): a right-click on a
/// [`LATCHING`] button holds it, the next click on the held one lets it go, other clicks leave it as it is.
pub fn latch_click(
    latched: Option<&'static str>,
    action: &'static str,
    right: bool,
) -> Option<&'static str> {
    if latched == Some(action) {
        None
    } else if right && LATCHING.contains(&action) {
        Some(action)
    } else {
        latched
    }
}

/// The latch kept for the next frame: let go when the game stopped (the crash screen) or the window lost the focus
/// (`focused` false), so no button stays down.
pub fn latch_kept(
    latched: Option<&'static str>,
    crash: bool,
    focused: bool,
) -> Option<&'static str> {
    latched.filter(|_| !crash && focused)
}

/// Whether 2017-18 has the in-match pause: the PAUSE button, the pause key (and its row on the KEYS page) and the
/// panel DLL's hold. Off (owner, 2026-10-31, on hold); `true` brings it all back. 2010-11 is not behind it.
pub const PAUSE_ON: bool = false;

/// The pause is offered to this edition.
pub fn pause_on(edition: Edition) -> bool {
    PAUSE_ON || edition != Edition::E1718
}

/// The page bar's button `i` (0 CATALOGUE, 1 KEYS, 2 SETTINGS, 3 CLUB CARD) on the bar `bar` (also for tests that
/// click it).
pub fn page_button(bar: Rect, i: usize) -> Rect {
    let r = bar.shrink2(vec2(12.0, 8.0));
    Rect::from_min_size(
        pos2(r.left() + 132.0 + i as f32 * 98.0, r.top()),
        vec2(92.0, r.height()),
    )
}

/// The bottom bar: the edition, the four page buttons (lit while their page is open), the ping and the window key.
fn page_bar(ui: &mut Ui, st: &mut PanelState, bar: Rect, lang: Lang, side: bool) {
    let r = bar.shrink2(vec2(12.0, 8.0));
    let p = ui.painter();
    widgets::mark(p, pos2(r.left() + 11.0, r.center().y), st.edition.accent());
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(
        pos2(r.left() + 30.0, r.center().y - 11.0),
        pos2(r.left() + 130.0, r.center().y + 11.0),
    )));
    widgets::badge(&mut b, st.edition);
    let mut pages = vec![
        (Page::Catalogue, "panel.add_card"),
        (Page::Keys, "panel.keys"),
        (Page::Settings, "panel.settings"),
    ];
    // 2017-18 keeps its clubs on the game server: no club card page
    if st.edition != Edition::E1718 {
        pages.push((Page::ClubCard, "panel.club_card"));
    }
    // the four pages and PAUSE; the right end keeps ~140 points for the ping and the window key at 1440x900
    let w = 92.0;
    let n = pages.len() as f32;
    for (i, (pg, key)) in pages.into_iter().enumerate() {
        let t = page_button(bar, i);
        let open = st.page == pg;
        let resp = ui.put(t, |ui: &mut Ui| {
            cabinet(ui, t.size(), &tr!(lang, key), Cab::Tactic, open)
        });
        if resp.clicked() {
            st.page = if open { Page::Game } else { pg };
        }
    }
    // PAUSE: asks the panel DLL for a pause (it says why when it refuses), RESUME while paused
    let t = Rect::from_min_size(
        pos2(r.left() + 132.0 + n * (w + 6.0), r.top()),
        vec2(w, r.height()),
    );
    let key = if st.paused.is_some() {
        "pause.resume"
    } else {
        "pause.button"
    };
    // 2017-18 hides PAUSE while [`PAUSE_ON`] is off (RESUME stays if a pause somehow holds)
    if pause_on(st.edition) || st.paused.is_some() {
        let resp = ui.put(t, |ui: &mut Ui| {
            cabinet(
                ui,
                t.size(),
                &tr!(lang, key),
                Cab::Plain,
                st.paused.is_some(),
            )
        });
        if resp.clicked() {
            st.pause_asked = true;
        }
    }
    // the right end, two rows: the card lamp (2010-11) and the ping (online only), then the window key
    let right = Rect::from_min_max(
        pos2(r.left() + 132.0 + (n + 1.0) * (w + 6.0) + 8.0, r.top()),
        r.max,
    );
    let row =
        |i: f32| Rect::from_min_size(right.min + vec2(0.0, i * 24.0), vec2(right.width(), 22.0));
    let p = ui.painter();
    let mut x = right.left();
    if side {
        // the side layout: the lamp and the ping hug the bar's right end
        let lamp = if st.edition != Edition::E1718 {
            lamp_width(p, st.status.card, lang) + 10.0
        } else {
            0.0
        };
        let ping_w = if st.online {
            ping_width(p, st.ping_ms, lang)
        } else {
            0.0
        };
        x = x.max(right.right() - lamp - ping_w);
    }
    if st.edition != Edition::E1718 {
        x += card_lamp(
            p,
            Rect::from_min_max(pos2(x, row(0.0).top()), row(0.0).max),
            st.status.card,
            lang,
        ) + 10.0;
    }
    if st.online {
        let r0 = row(0.0);
        ping(
            p,
            Rect::from_min_max(pos2(x, r0.top()), r0.max),
            st.ping_ms,
            lang,
        );
    }
    let g = widgets::fit(
        p,
        &tr!(lang, "panel.f11"),
        theme::body(HINT),
        theme::TEXT_DIM,
        right.width(),
    );
    let gx = if side {
        right.right() - g.size().x
    } else {
        right.left()
    };
    p.galley(
        pos2(gx, row(1.0).center().y - g.size().y / 2.0),
        g,
        theme::TEXT_DIM,
    );
}

/// The card lamp: IN USE (amber), SAFE TO CLOSE (green), NO CARD (grey); a mark and a word, never colour alone.
/// Closing is only unsafe while the session is open (the same rule as the close dialog). Returns the width it took.
fn card_lamp(p: &egui::Painter, r: Rect, card: CardState, lang: Lang) -> f32 {
    let (tone, text, col) = lamp_look(card, lang);
    let g = p.layout_no_wrap(text, theme::display(16.0), col);
    let w = g.size().x + 44.0;
    let pill = Rect::from_min_size(r.min, vec2(w, r.height()));
    p.rect_filled(pill, 11, col.gamma_multiply(0.16));
    p.rect_stroke(pill, 11, Stroke::new(1.0, col), StrokeKind::Inside);
    let c = pos2(pill.left() + 14.0, pill.center().y);
    match tone {
        Some(t) => widgets::tone_icon(p, c, t),
        None => {
            p.circle_stroke(c, 8.0, Stroke::new(2.0, col));
            p.line_segment(
                [c - vec2(4.0, 0.0), c + vec2(4.0, 0.0)],
                Stroke::new(2.0, col),
            );
        }
    }
    p.galley(
        pos2(pill.left() + 28.0, pill.center().y - g.size().y / 2.0),
        g,
        col,
    );
    w
}

/// The card lamp's mark, word and colour.
fn lamp_look(card: CardState, lang: Lang) -> (Option<Tone>, String, Color32) {
    let (tone, key, col) = match card {
        CardState::Open => (Some(Tone::Notice), "panel.lamp_in_use", theme::AMBER),
        CardState::Closed | CardState::Cut => (Some(Tone::Done), "panel.lamp_safe", theme::GREEN),
        _ => (None, "panel.lamp_none", theme::TEXT_DIM),
    };
    (tone, tr!(lang, key), col)
}

/// The card lamp's width ([`card_lamp`]).
fn lamp_width(p: &egui::Painter, card: CardState, lang: Lang) -> f32 {
    let (_, text, col) = lamp_look(card, lang);
    p.layout_no_wrap(text, theme::display(16.0), col).size().x + 44.0
}

/// The ping meter's text.
fn ping_text(ms: u32, lang: Lang) -> String {
    if ms == u32::MAX {
        tr!(lang, "panel.ping", ms = "--")
    } else {
        tr!(lang, "panel.ping", ms = ms)
    }
}

/// The ping meter's width ([`ping`]).
fn ping_width(p: &egui::Painter, ms: u32, lang: Lang) -> f32 {
    36.0 + p
        .layout_no_wrap(ping_text(ms, lang), theme::display(15.0), theme::TEXT_DIM)
        .size()
        .x
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

/// The ping meter: five bars and the milliseconds; `u32::MAX` = no answer for 6 s ("-- ms", no bar lit).
fn ping(p: &egui::Painter, r: Rect, ms: u32, lang: Lang) {
    let good = match ms {
        u32::MAX => 0,
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
        ping_text(ms, lang),
        theme::display(15.0),
        theme::TEXT_DIM,
    );
}

fn board_strip(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    strip_ground(ui);
    let r = ui.max_rect().shrink(16.0);
    // as wide as the strip, or as its height can use, centred
    let w = r
        .width()
        .min((ui.max_rect().height() - BOARD_EXTRA) / BOARD_PER_W);
    let r = Rect::from_min_size(pos2(r.center().x - w / 2.0, r.top()), vec2(w, r.height()));
    let pitch = board::pitch_rect(r.min + vec2(0.0, BOARD_HEAD), r.width());
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pitch.min, r.max)));
    b.spacing_mut().item_spacing.y = 4.0;
    st.board_changed |= board::formation_board(
        &mut b,
        &mut st.board,
        &st.links,
        st.hover_delay,
        lang,
        false,
        false,
    );
    let Some((b, btn)) = board_foot(&mut b, st, lang) else {
        return;
    };
    // the hovered card's pane and the partner list: in this column only, under the hint or over the pitch
    let hint_bottom = b.min_rect().bottom().max(btn.bottom());
    board::info_panes(
        &b,
        &st.board,
        &st.links,
        pitch,
        hint_bottom,
        r.bottom(),
        lang,
    );
}

/// The board window ([`PanelState::desk`]): the board alone, as large as the window allows ([`desk_board_rect`]), its
/// DOCK and UNDO row under it; with `board_buttons=on` the cabinet buttons beside or under it ([`desk_parts`]). The
/// crash screen takes the board's place.
fn board_window(ui: &mut Ui, st: &mut PanelState, lang: Lang, sheet: Rect) {
    strip_ground(ui);
    let both = st.edition != Edition::E1718;
    let (room, deck) = desk_parts(sheet, st.file.board_buttons, both);
    if let Some((r, s, stack)) = deck {
        widgets::gradient(
            ui.painter(),
            r,
            0,
            Color32::from_rgb(0x12, 0x1C, 0x2E),
            theme::NIGHT,
            true,
        );
        cabinet_deck(ui, st, r, lang, s, stack);
    }
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(room));
    if st.crash {
        return crash_screen(&mut c, st, lang);
    }
    let (at, beside) = desk_board_rect(room);
    let mut b =
        c.new_child(egui::UiBuilder::new().max_rect(at.with_max_y(at.bottom() + DESK_FOOT)));
    b.spacing_mut().item_spacing.y = 4.0;
    st.board_changed |=
        board::formation_board(&mut b, &mut st.board, &st.links, None, lang, beside, true);
    if let Some(card) = board::take_touched(&b) {
        st.touched = Some((card, b.input(|i| i.time)));
    }
    board_foot(&mut b, st, lang);
}

/// Under a board: a card just taken off (its name and UNDO) or a lineup just imported (its UNDO) at the left, POP OUT
/// or DOCK at the right; else, on the game window's board, the hint. `None` when the import's line shows, else the
/// row's left part and the button.
fn board_foot(b: &mut Ui, st: &mut PanelState, lang: Lang) -> Option<(Ui, Rect)> {
    let now = b.input(|i| i.time);
    if let Some(r) = board::take_removed(b) {
        st.undo = Some((r, now));
        st.import_undo = None; // UNDO now means this card
    }
    b.add_space(4.0);
    // at the hint's right: the board in its own window, or back with the game
    let row = Rect::from_min_max(b.cursor().min, b.max_rect().max);
    let label = egui::RichText::new(tr!(
        lang,
        if st.file.board_window {
            "board.dock"
        } else {
            "board.pop_out"
        }
    ))
    .font(theme::bold(12.0));
    let text_w = b
        .painter()
        .layout_no_wrap(label.text().to_owned(), theme::bold(12.0), theme::TEXT)
        .size()
        .x;
    let pad = b.spacing().button_padding.x * 2.0 + 2.0;
    let btn = Rect::from_min_size(
        pos2(row.right() - text_w - pad, row.top()),
        vec2(text_w + pad, 22.0),
    );
    let button = egui::Button::new(label).wrap_mode(egui::TextWrapMode::Extend);
    if b.put(btn, button).clicked() {
        st.file.board_window = !st.file.board_window;
        st.file_changed = true;
        st.say(
            Tone::Done,
            if st.file.board_window {
                "settings.msg_board_window"
            } else {
                "settings.msg_board_docked"
            },
        );
    }
    let mut b = b.new_child(egui::UiBuilder::new().max_rect(row.with_max_x(btn.left() - 8.0)));
    if import_undo_line(&mut b, st, lang, now) {
        return None;
    }
    // a card just taken off: its name and UNDO for a few seconds, in the hint's place
    st.undo = st.undo.take().filter(|(_, at)| now - at < UNDO_SECS);
    if let Some((r, at)) = st.undo.clone() {
        b.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(UNDO_SECS - (now - at)));
        b.horizontal(|ui| {
            ui.label(
                egui::RichText::new(tr!(lang, "board.removed", name = r.card.short_name()))
                    .font(theme::body(HINT))
                    .color(theme::TEXT),
            );
            if widgets::action(ui, &tr!(lang, "board.undo")).clicked() {
                st.board_changed |= board::undo(&mut st.board, &r);
                st.undo = None;
            }
        });
    } else if !st.desk {
        b.label(
            egui::RichText::new(tr!(lang, "board.hint"))
                .font(theme::body(HINT))
                .color(theme::TEXT_DIM),
        );
    }
    Some((b, btn))
}

/// A lineup just imported: a line and its UNDO for [`IMPORT_UNDO_SECS`], in the hint's place; `false`: none.
fn import_undo_line(ui: &mut Ui, st: &mut PanelState, lang: Lang, now: f64) -> bool {
    st.import_undo = st
        .import_undo
        .take()
        .filter(|(_, at)| now - at < IMPORT_UNDO_SECS);
    let Some(at) = st.import_undo.as_ref().map(|u| u.1) else {
        return false;
    };
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs_f64(
            IMPORT_UNDO_SECS - (now - at),
        ));
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(tr!(lang, "board.imported"))
                .font(theme::body(HINT))
                .color(theme::TEXT),
        );
        if widgets::action(ui, &tr!(lang, "board.undo")).clicked() {
            st.import_undone = true;
        }
    });
    true
}

/// How long UNDO stays offered after a card is taken off the board.
const UNDO_SECS: f64 = 5.0;

/// The crash screen, in the board's place: the game lost its control program (or stopped), the card's state, and the two
/// ways on. RESTART THE GAME asks the Launcher (`restart`), SAVE LOGS asks it to open its report dialog (`open_report`).
fn crash_screen(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    strip_ground(ui);
    let r = ui.max_rect().shrink2(vec2(24.0, 0.0));
    let w = r.width().min(440.0);
    let col = Rect::from_center_size(
        pos2(r.center().x, r.center().y - 30.0),
        vec2(w, r.height().min(520.0)),
    );
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(col));
    let ui = &mut ui;
    ui.spacing_mut().item_spacing.y = 14.0;
    let (ic, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
    ui.painter().text(
        ic.center(),
        Align2::CENTER_CENTER,
        tr!(lang, "panel.crash_title"),
        theme::display(38.0),
        theme::RED_TEXT,
    );
    ui.label(
        egui::RichText::new(tr!(lang, "panel.crash_lost"))
            .font(theme::body(17.0))
            .color(theme::TEXT),
    );
    ui.add_space(4.0);
    let s = st.status.clone();
    egui::Grid::new("crash-now")
        .num_columns(2)
        .spacing(vec2(16.0, 8.0))
        .min_col_width(150.0)
        .show(ui, |ui| {
            if st.edition == Edition::E1718 {
                // the club lives on the server: nothing to say about a card
                ui.label(
                    egui::RichText::new(tr!(lang, "panel.crash_1718"))
                        .font(theme::bold(15.0))
                        .color(theme::GREEN),
                );
                ui.end_row();
                return;
            }
            let (k, c) = card_text(s.card);
            kv(ui, &tr!(lang, "settings.session"), &tr!(lang, k), Some(c));
            let v = s
                .saved
                .as_ref()
                .map_or_else(|| tr!(lang, "settings.never"), |w| when_text(lang, w));
            let c = s.saved.is_some().then_some(theme::GREEN);
            kv(ui, &tr!(lang, "settings.last_saved"), &v, c);
            if let Some(n) = s.bad {
                let v = match n {
                    0 => "0".to_owned(),
                    1 => tr!(lang, "settings.bad_one"),
                    n => tr!(lang, "settings.bad_many", n = n),
                };
                kv(ui, &tr!(lang, "settings.bad_endings"), &v, None);
            }
        });
    ui.add_space(8.0);
    let restart = if st.restart_sent {
        tr!(lang, "settings.restarting")
    } else {
        tr!(lang, "panel.crash_restart")
    };
    if cabinet(ui, vec2(w, 56.0), &restart, Cab::Go, true).clicked() && !st.restart_sent {
        st.restart = true;
    }
    if cabinet(
        ui,
        vec2(w, 56.0),
        &tr!(lang, "settings.save_logs"),
        Cab::Plain,
        false,
    )
    .clicked()
    {
        st.open_report = true;
    }
    if st.report_sent {
        ui.label(
            egui::RichText::new(tr!(lang, "panel.crash_report_asked"))
                .font(theme::body(HINT))
                .color(theme::TEXT_DIM),
        );
    }
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
        // the card view goes back to the catalogue's list, not out of the catalogue
        if st.page == Page::Catalogue && st.catalogue.view.take().is_some() {
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        } else {
            st.page = Page::Game;
            return;
        }
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
    if now - at < n.secs {
        status_line(ui, n.tone, &n.text);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(n.secs - (now - at)));
    } else {
        *message = None;
    }
}

fn catalogue(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    if st.catalogue.view.is_some() {
        return card_view(ui, st, lang);
    }
    let cards = st.cards.clone();
    let c = &mut st.catalogue;
    let fresh = c.refresh(&cards);
    let mut add = None;
    let mut open = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
    ui.spacing_mut().item_spacing = vec2(6.0, 8.0);
    // Enter puts the first card listed on the table; typed letters always go to the search line
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        add = c.results.first().map(|&i| cards[i].number);
    }
    let mut import = false;
    // the search line, IMPORT LINEUP at its right end
    let row = vec2(ui.available_width(), 36.0);
    let search = ui
        .allocate_ui_with_layout(
            row,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                import = action(ui, &tr!(lang, "cat.import")).clicked();
                ui.add(
                    egui::TextEdit::singleline(&mut c.filter.search)
                        .id_salt("cat-search")
                        .hint_text(tr!(lang, "cat.search_hint"))
                        .font(theme::body(16.0))
                        .desired_width(f32::INFINITY)
                        .margin(vec2(12.0, 8.0)),
                )
            },
        )
        .inner;
    if !search.has_focus() && !egui::Popup::is_any_open(ui.ctx()) {
        search.request_focus();
    }
    // an import's report runs to a few lines: across the page under the search line, not in the stats pane
    let long = st.message.as_ref().is_some_and(|n| n.secs > NOTE_SECS);
    if long {
        note_line(ui, &mut st.message);
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
    let links = &st.links;
    // the affinity column when the game's links are known
    let with_affinity = links.affinity.is_some();
    let mut strips = egui_extras::StripBuilder::new(ui).size(egui_extras::Size::remainder());
    if with_affinity {
        strips = strips.size(egui_extras::Size::exact(220.0));
    }
    strips
        .size(egui_extras::Size::exact(250.0))
        .horizontal(|mut strip| {
            strip.cell(|ui| {
                ui.horizontal(|ui| {
                    caps(ui, &tr!(lang, "cat.results", n = results.len()));
                });
                let sizing::Grid {
                    card: size,
                    cols: per_row,
                } = sizing::grid(ui.available_width(), ui.ctx().pixels_per_point());
                let gap = sizing::GAP;
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
            let shown = focus
                .filter(|i| results.contains(i))
                .or(results.first().copied())
                .and_then(|i| cards.get(i));
            if with_affinity {
                strip.cell(|ui| {
                    if let Some(card) = shown {
                        affinity_pane(ui, card, links, &on_table, lang);
                    }
                });
            }
            strip.cell(|ui| {
                if let Some(card) = shown {
                    let (added, opened) = stats_pane(ui, card, lang);
                    if added {
                        add = Some(card.number);
                    }
                    open |= opened;
                }
                if !long {
                    note_line(ui, message);
                }
            });
            // Tab or a click on the stats pane's face: the card view of the card shown
            if open && let Some(card) = shown {
                c.view = cards.iter().position(|k| k.number == card.number);
            }
        });
    st.add = st.add.or(add);
    st.import |= import;
}

/// The card view's caption row under the pictures, with its gap: the name, the club line and the roles beside TOTAL,
/// the position and ADD.
const CAPTION_ROW: f32 = 100.0;

/// The card view, in the catalogue's place under its heading: the card's front and back side by side, each at a whole
/// multiple of its own pixels ([`sizing::exact`]: x1, or x2 with nearest filtering when the page has room), never
/// scaled by a fraction or scrolled; one side with a FRONT | BACK toggle when the pair does not fit. Under them the
/// name, club line and roles, TOTAL, the position and ADD. Keys: Tab or Esc back to the list, arrows the previous and
/// next result, F turns the single side over, Enter adds.
fn card_view(ui: &mut Ui, st: &mut PanelState, lang: Lang) {
    use egui::emath::GuiRounding as _;
    let cards = st.cards.clone();
    let c = &mut st.catalogue;
    let Some(card) = c.view.and_then(|i| cards.get(i)) else {
        c.view = None;
        return;
    };
    let at = c.results.iter().position(|&i| Some(i) == c.view);
    let key = |k| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
    let (close, prev, next, flip, add) = (
        key(egui::Key::Tab),
        key(egui::Key::ArrowLeft),
        key(egui::Key::ArrowRight),
        key(egui::Key::F),
        key(egui::Key::Enter),
    );
    let ppp = ui.ctx().pixels_per_point();
    let body = ui.available_rect_before_wrap();
    // the pair at x2 or x1, else one side at x1, else (a window under about 330 pixels tall) one side at half; the head
    // row and its gap above the pictures, the caption row and its gap under them
    let room = vec2(body.width(), body.height() - 48.0 - CAPTION_ROW);
    let (n, k) = match sizing::exact(sizing::ALBUM, 2, room, ppp) {
        Some(k) => (2, k),
        None => (1, sizing::exact(sizing::ALBUM, 1, room, ppp).unwrap_or(0.5)),
    };
    let size = sizing::ALBUM * k / ppp;
    // the head row: back to the list, the side toggle (one side only), the place in the results
    let head = Rect::from_min_size(body.min, vec2(body.width(), 32.0));
    let mut back_to_list = close;
    let mut go: i32 = i32::from(next) - i32::from(prev);
    let mut flip = flip && n == 1;
    ui.scope_builder(egui::UiBuilder::new().max_rect(head), |ui| {
        ui.horizontal_centered(|ui| {
            back_to_list |= action(ui, &tr!(lang, "cat.view_list")).clicked();
        });
    });
    if n == 1 {
        let mut r = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_center_size(head.center(), vec2(240.0, 32.0))),
        );
        let mut i = usize::from(c.view_back);
        if segmented(
            &mut r,
            &mut i,
            &[&tr!(lang, "cat.front"), &tr!(lang, "cat.back")],
            240.0,
        ) {
            flip = (i == 1) != c.view_back;
        }
    }
    if let Some(at) = at {
        ui.scope_builder(egui::UiBuilder::new().max_rect(head), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                go += i32::from(action(ui, "›").clicked());
                ui.label(
                    egui::RichText::new(format!("{} / {}", at + 1, c.results.len()))
                        .font(theme::display(18.0))
                        .color(theme::TEXT_DIM),
                );
                go -= i32::from(action(ui, "‹").clicked());
            });
        });
    }
    if flip {
        c.view_back = !c.view_back;
    }
    // the pictures, centred, their top left on a whole pixel
    let w = n as f32 * size.x + (n as f32 - 1.0) * sizing::PAIR_GAP;
    let left = body.center().x - w / 2.0;
    let sides: &[bool] = if n == 2 {
        &[false, true]
    } else if c.view_back {
        &[true]
    } else {
        &[false]
    };
    for (j, &back) in sides.iter().enumerate() {
        let min = pos2(
            left + j as f32 * (size.x + sizing::PAIR_GAP),
            body.top() + 48.0,
        );
        let r = Rect::from_min_size(min.round_to_pixels(ppp), size);
        board::card_side(ui.painter(), r, card, back, k >= 2.0, lang);
    }
    // the caption row under them
    let cap = Rect::from_min_max(
        pos2(body.left(), body.top() + 48.0 + size.y + 16.0),
        body.max,
    )
    .intersect(Rect::from_center_size(
        pos2(body.center().x, body.center().y),
        vec2(w.max(560.0).min(body.width()), f32::INFINITY),
    ));
    let mut added = add;
    ui.scope_builder(egui::UiBuilder::new().max_rect(cap), |ui| {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.set_max_width(cap.width() - 330.0);
                ui.label(
                    egui::RichText::new(&card.name)
                        .font(theme::display(26.0))
                        .color(theme::TEXT),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{} · {} · {}",
                        card.club, card.country, card.season
                    ))
                    .font(theme::body(HINT))
                    .color(theme::TEXT_DIM),
                );
                let top = card.aptitude.top();
                if !top.is_empty() {
                    let roles = top
                        .iter()
                        .map(|(n, r)| format!("{n} {r}"))
                        .collect::<Vec<_>>()
                        .join(" · ");
                    ui.label(
                        egui::RichText::new(tr!(lang, "board.roles", roles = roles))
                            .font(theme::body(HINT))
                            .color(theme::TEXT_DIM),
                    );
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                added |=
                    cabinet(ui, vec2(200.0, 44.0), &tr!(lang, "cat.add"), Cab::Go, true).clicked();
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(card.total.to_string())
                            .font(theme::display(44.0))
                            .color(theme::AMBER),
                    );
                    chip(ui, card.pos.code(), true, Some(card.pos.color()));
                });
            });
        });
    });
    if added {
        st.add = Some(card.number);
    }
    // what ADD did (added, already on the table, refused): a toast over the pictures' lower edge, as the game page has
    let note = Rect::from_center_size(
        pos2(body.center().x, body.top() + 48.0 + size.y - 30.0),
        vec2(w.clamp(400.0, 520.0), 34.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(note), |ui| {
        note_line(ui, &mut st.message)
    });
    let c = &mut st.catalogue;
    if back_to_list {
        c.view = None;
    } else if go != 0
        && let Some(at) = at
    {
        let to = (at as i32 + go).clamp(0, c.results.len() as i32 - 1) as usize;
        c.view = c.results.get(to).copied();
    }
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

/// The card's partners by affinity, best first (from the game's own files), a tick on those already on the table.
fn affinity_pane(ui: &mut Ui, card: &Card, links: &board::Links, on_table: &[u32], lang: Lang) {
    egui::Frame::new()
        .fill(theme::WELL)
        .corner_radius(theme::RADIUS_BIG)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            caps(ui, &tr!(lang, "cat.affinity"));
            let w = ui.available_width();
            let partners: Vec<_> = links
                .affinity
                .iter()
                .flat_map(|a| a.partners(card.number))
                .collect();
            if partners.is_empty() {
                ui.label(
                    egui::RichText::new(tr!(lang, "cat.no_partners"))
                        .font(theme::body(HINT))
                        .color(theme::TEXT_DIM),
                );
            }
            egui::ScrollArea::vertical()
                .id_salt("affinity")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (p, v) in partners {
                        let r = ui.cursor();
                        board::partner_row(ui, &p.name, None, Some(v), w - 22.0);
                        if p.cards.iter().any(|n| on_table.contains(n)) {
                            super::widgets::tone_icon(
                                ui.painter(),
                                pos2(r.left() + w - 9.0, r.top() + 9.0),
                                Tone::Done,
                            );
                        }
                    }
                });
        });
}

/// The card's stats and its ADD button; `true` = ADD clicked.
/// The focused card's stats, ADD under them; returns (ADD clicked, the face clicked: the card view asked for).
fn stats_pane(ui: &mut Ui, card: &Card, lang: Lang) -> (bool, bool) {
    let face = sizing::hover_face(ui.ctx().pixels_per_point());
    let u = sizing::text_unit(face.x);
    egui::Frame::new()
        .fill(theme::WELL)
        .corner_radius(theme::RADIUS_BIG)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            let mut open = false;
            ui.horizontal(|ui| {
                let (r, resp) = ui.allocate_exact_size(face, Sense::click());
                open = resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked();
                board::card_face(ui.painter(), r, card, board::Look::default());
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(tr!(lang, "cat.total"))
                            .font(theme::display(14.0 * u))
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(
                        egui::RichText::new(card.total.to_string())
                            .font(theme::display(48.0 * u))
                            .color(theme::AMBER),
                    );
                    ui.horizontal(|ui| {
                        chip(ui, card.pos.code(), true, Some(card.pos.color()));
                        // the printed code: the original's, never translated
                        if !card.series().is_empty() {
                            chip(ui, card.series(), false, None);
                        }
                    });
                });
            });
            ui.label(
                egui::RichText::new(&card.name)
                    .font(theme::display(22.0 * u))
                    .color(theme::TEXT),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} · {} · {}",
                    card.club, card.country, card.season
                ))
                .font(theme::body(HINT * u))
                .color(theme::TEXT_DIM),
            );
            // every role at the card's top rating (a card rated from its position flags has them all at 8)
            let top = card.aptitude.roles.iter().copied().max().unwrap_or(0);
            let best: Vec<_> = board::ROLES
                .iter()
                .zip(card.aptitude.roles)
                .filter(|(_, r)| top > 0 && *r == top)
                .map(|(n, _)| *n)
                .collect();
            if !best.is_empty() {
                ui.label(
                    egui::RichText::new(tr!(lang, "cat.best_role", roles = best.join(" ")))
                        .font(theme::body(HINT * u))
                        .color(theme::TEXT_DIM),
                );
            }
            ui.add_space(4.0);
            let w = ui.available_width();
            let names = STATS.map(|k| tr!(lang, k));
            widgets::stat_bars(ui, card, &names, &tr!(lang, "cat.secret"), w);
            ui.add_space(6.0);
            let add = cabinet(ui, vec2(w, 44.0), &tr!(lang, "cat.add"), Cab::Go, true).clicked();
            (add, open)
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
                let key = forza::keys::key_label(vk);
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
        } else if alt || !forza::keys::is_allowed(i128::from(vk)) {
            let key = if alt && vk != 0x12 {
                "Alt".into()
            } else {
                forza::keys::key_label(vk)
            };
            st.note(Tone::Refused, tr!(lang, "keys.msg_refused", key = key));
        } else {
            let other = (0..ACTIONS.len()).find(|&j| j != k && st.keys[j] == vk);
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
        let other = (0..ACTIONS.len()).find(|&j| j != k && st.pads[j] == code);
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
        forza::keys::key_label(vk)
    }
}

fn pad_name(lang: Lang, code: u32) -> String {
    if code == 0 {
        tr!(lang, "keys.none")
    } else {
        forza::keys::pad_label(code)
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
    // an open Advanced group scrolls instead of pushing RESET KEYS off the page
    let room = (ui.available_height() - 56.0).max(120.0);
    egui::ScrollArea::vertical()
        .max_height(room)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("keys")
                .num_columns(4)
                .spacing(vec2(16.0, 3.0))
                .striped(false)
                .show(ui, |ui| {
                    caps(ui, &tr!(lang, "keys.action"));
                    ui.label("");
                    caps(ui, &tr!(lang, "keys.keyboard"));
                    caps(ui, &tr!(lang, "keys.controller"));
                    ui.end_row();
                    for i in (0..ACTIONS.len()).filter(|i| !OPERATOR.contains(i)) {
                        if i == forza::keys::PAUSE && !pause_on(st.edition) {
                            continue;
                        }
                        row(ui, st, lang, i, cell_w);
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            // the operator's switches: TEST, SERVICE, SW1, SW2
            let mut open = st.keys_advanced;
            widgets::fold(
                ui,
                &tr!(lang, "keys.advanced"),
                "",
                &mut open,
                theme::AMBER,
                |ui| {
                    egui::Grid::new("keys-advanced")
                        .num_columns(4)
                        .spacing(vec2(16.0, 3.0))
                        .striped(false)
                        .show(ui, |ui| {
                            for i in OPERATOR {
                                row(ui, st, lang, i, cell_w);
                                ui.end_row();
                            }
                        });
                },
            );
            st.keys_advanced = open;
        });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if action(ui, &tr!(lang, "keys.defaults")).clicked() {
            st.keys = forza::keys::defaults(st.edition == Edition::E1718);
            st.pads = [0; forza::keys::COUNT];
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
        forza::keys::key_label(st.keys[i]),
        forza::keys::pad_label(st.pads[i]),
    );
    let r1 = key_cell(ui, &k, ks, cell_w, lang);
    if i == forza::keys::PAUSE {
        ui.label(""); // the panel's own key: the key driver reads no controller for it
        if r1.clicked() {
            st.pad_capturing = None;
            st.capturing = (st.capturing != Some(i)).then_some(i);
            st.message = None;
        }
        return;
    }
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

/// The text key and colour of the club card's session.
fn card_text(card: CardState) -> (&'static str, Color32) {
    match card {
        CardState::Open => ("settings.card_open", theme::AMBER),
        CardState::Closed => ("settings.card_closed", theme::GREEN),
        CardState::Cut => ("settings.card_cut", theme::AMBER),
        CardState::New => ("settings.card_new", theme::TEXT_FAINT),
        CardState::Unreadable => ("settings.card_unreadable", theme::RED_TEXT),
        CardState::None => ("settings.card_none", theme::TEXT_FAINT),
    }
}

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
                    let (k, c) = card_text(s.card);
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
            const HOVER: [Option<u32>; 5] = [Some(500), Some(1000), Some(2000), Some(3000), None];
            let mut i = HOVER
                .iter()
                .position(|&m| m == st.file.hover_ms)
                .unwrap_or(usize::MAX);
            let off = tr!(lang, "settings.hover_off");
            if segmented(ui, &mut i, &["0.5 s", "1 s", "2 s", "3 s", &off], w) {
                st.file.hover_ms = HOVER[i];
                st.hover_delay = HOVER[i].map(|ms| ms as f32 / 1000.0);
                st.file_changed = true;
                match st.hover_delay {
                    Some(s) => {
                        let t = format!("{s}");
                        st.note(Tone::Done, tr!(lang, "settings.msg_hover", s = t));
                    }
                    None => st.say(Tone::Done, "settings.msg_hover_off"),
                }
            }
            field(ui, &tr!(lang, "settings.board"), w);
            let mut i = st.file.board_window as usize;
            if segmented(
                ui,
                &mut i,
                &[
                    &tr!(lang, "settings.board_here"),
                    &tr!(lang, "settings.board_window"),
                ],
                w,
            ) {
                st.file.board_window = i == 1;
                st.file_changed = true;
                st.say(
                    Tone::Done,
                    if i == 1 {
                        "settings.msg_board_window"
                    } else {
                        "settings.msg_board_docked"
                    },
                );
            }
            if widgets::toggle_row(
                ui,
                &mut st.file.board_buttons,
                &tr!(lang, "settings.board_buttons"),
                &tr!(lang, "settings.board_buttons_hint"),
            )
            .clicked()
            {
                st.file_changed = true;
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
                    .font(theme::body(HINT))
                    .color(c),
            );
        });
        widgets::section(right, &tr!(lang, "settings.logs"), accent, |ui| {
            let w = ui.available_width();
            let busy = st.logs_sent;
            if cabinet(
                ui,
                vec2(w, 44.0),
                &tr!(lang, "settings.save_logs"),
                Cab::Data,
                busy,
            )
            .clicked()
                && !busy
            {
                st.save_logs = true;
            }
            match s.logs.as_deref() {
                Some(r) if r.starts_with("ok ") => {
                    status_line(ui, Tone::Done, &tr!(lang, "logs.saved", path = &r[3..]));
                }
                Some(r) if r.starts_with("fail ") => {
                    status_line(
                        ui,
                        Tone::Refused,
                        &tr!(lang, "logs.save_failed", why = &r[5..]),
                    );
                }
                _ if busy => {
                    status_line(ui, Tone::Notice, &tr!(lang, "settings.msg_saving"));
                }
                _ => {}
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
                                .font(theme::body(HINT))
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
                                            .font(theme::body(HINT))
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
                                        .font(theme::body(HINT))
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
                                .font(theme::body(HINT))
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
    let g = widgets::fit(p, sub, theme::body(HINT), theme::TEXT_DIM, tw);
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
                    (Cab::Go, "cab.start"),
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
                    status_line(ui, Tone::Done, &tr!(lang, "panel.msg_saved"));
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
                        &tr!(lang, "launcher.skip_tutorial"),
                        &tr!(lang, "launcher.skip_tutorial_hint"),
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
    use crate::board::Aptitude;

    fn sheet_of(w: f32, h: f32) -> Rect {
        // what a window of w x h pixels shows, in points
        let ppp = scale(w, h, 96.0);
        Rect::from_min_size(Pos2::ZERO, vec2(w, h) / ppp)
    }

    #[test]
    fn layout_fits_every_window() {
        let windows = [
            (1440.0, 900.0),
            (1600.0, 900.0),
            (1280.0, 800.0),
            (1920.0, 1049.0),
            (1920.0, 1080.0),
            (2560.0, 1369.0),
            (2560.0, 1440.0),
            (3440.0, 1440.0),
            (3840.0, 2160.0),
        ];
        for (w, h) in windows {
            for compact in [false, true] {
                let sheet = sheet_of(w, h);
                assert!(sheet.width() >= SIZE.x - 0.01 && sheet.height() >= SIZE.y - 0.01);
                let l = layout(sheet, compact);
                let at = format!("{w}x{h} compact {compact}: {l:?}");
                // inside, and no part overlaps another
                assert!(
                    sheet.contains_rect(l.board) && l.board.width() >= BOARD_W,
                    "{at}"
                );
                assert!(l.middle.contains_rect(l.picture), "{at}");
                assert!(
                    l.middle.contains_rect(l.deck) && l.middle.contains_rect(l.tabs),
                    "{at}"
                );
                assert!(l.board.left() >= l.middle.right() - 0.01, "{at}");
                assert!(l.picture.bottom() <= l.tabs.top() + 0.01, "{at}");
                assert!(
                    l.deck.bottom() <= l.tabs.top() + 0.01
                        && l.picture.bottom() <= l.deck.top() + 0.01,
                    "{at}"
                );
                // 16:10, and the deck keeps its height
                assert!(
                    (l.picture.width() / l.picture.height() - 1.6).abs() < 1e-3,
                    "{at}"
                );
                assert!(compact || l.deck.height() >= DECK_MIN - 0.01, "{at}");
                // the board's pitch, bench and hint fit its height
                let bw = l.board.width() - 32.0;
                assert!(
                    compact || BOARD_PER_W * bw + BOARD_EXTRA <= l.board.height() + 1.0,
                    "{at}"
                );
            }
        }
    }

    /// The hover pane lies in the board's column at every window: docked under the hint where the column has the room
    /// (1920x1080: a strip; 2560x1440: large), else a strip over the pitch's half away from the hovered card; never on
    /// the game's picture or the deck.
    #[test]
    fn the_hover_pane_stays_in_the_board_column() {
        for (w, h) in [
            (1280.0, 800.0),
            (1440.0, 900.0),
            (1600.0, 900.0),
            (1920.0, 1009.0),
            (1920.0, 1080.0),
            (2560.0, 1373.0),
            (2560.0, 1440.0),
            (3440.0, 1440.0),
        ] {
            for compact in [false, true] {
                let l = layout(sheet_of(w, h), compact);
                let col = l.board.shrink(16.0);
                let pitch = board::pitch_rect(col.min, col.width());
                let tile = board::card_size(pitch.width());
                // the bench (8 + its well) and a line of hint (4 + 17), as board_strip lays them out
                let hint_bottom = pitch.bottom() + 8.0 + tile.y * 0.8 + 12.0 + 4.0 + 4.0 + 17.0;
                for card_y in [
                    pitch.top() + 40.0,
                    pitch.bottom() - 40.0,
                    hint_bottom - 30.0,
                ] {
                    let (pane, tier) =
                        board::pane_place(pitch, hint_bottom, col.bottom(), 425.0, 149.0, card_y);
                    let at =
                        format!("{w}x{h} compact {compact} card at {card_y}: {pane:?} {tier:?}");
                    assert!(l.board.contains_rect(pane), "{at}");
                    assert!(
                        !pane.intersects(l.picture) && !pane.intersects(l.deck),
                        "{at}"
                    );
                    // floating: on the other half of the pitch
                    if pane.top() < hint_bottom {
                        assert!(
                            pane.top() >= pitch.top() && pane.bottom() <= pitch.bottom(),
                            "{at}"
                        );
                        assert!(
                            (pane.center().y < pitch.center().y) != (card_y < pitch.center().y),
                            "{at}"
                        );
                    }
                }
                let docked = |card_y| {
                    board::pane_place(pitch, hint_bottom, col.bottom(), 425.0, 149.0, card_y)
                };
                match (w as u32, h as u32, compact) {
                    (1440, 900, _) => assert!(docked(pitch.top()).0.top() < hint_bottom),
                    (1920, 1080, false) => assert_eq!(docked(pitch.top()).1, board::Tier::Strip),
                    (2560, 1440, false) => assert_eq!(docked(pitch.top()).1, board::Tier::Large),
                    _ => {}
                }
            }
        }
    }

    /// A right-click latches KEY PLAYER or KEEPER, the next click on it lets it go; other buttons press as before;
    /// a stop or a focus loss lets it go.
    #[test]
    fn the_latch_holds_until_the_next_click() {
        assert_eq!(latch_click(None, "KEYPLAYER", true), Some("KEYPLAYER"));
        assert_eq!(latch_click(None, "KEEPER", true), Some("KEEPER"));
        assert_eq!(latch_click(None, "SHOOT", true), None);
        assert_eq!(latch_click(None, "KEYPLAYER", false), None); // an ordinary press
        let on = Some("KEYPLAYER");
        assert_eq!(latch_click(on, "SHOOT", false), on); // SHOOT while it is held
        assert_eq!(latch_click(on, "KEYPLAYER", false), None);
        assert_eq!(latch_click(on, "KEYPLAYER", true), None);
        assert_eq!(latch_click(on, "KEEPER", true), Some("KEEPER"));
        assert_eq!(latch_kept(on, false, true), on);
        assert_eq!(latch_kept(on, true, true), None);
        assert_eq!(latch_kept(on, false, false), None);
    }

    /// The game window, the board popped out: the left part holds the picture (centred, 16:10) over the toast line and
    /// the page bar; the column holds the room for a touched card over the stacked deck plates, which fit it.
    #[test]
    fn popped_layout_column() {
        for (w, h) in [
            (1440.0, 900.0),
            (1920.0, 1080.0),
            (2560.0, 1440.0),
            (1920.0, 1200.0),
            (3440.0, 1440.0),
        ] {
            let sheet = Rect::from_min_size(Pos2::ZERO, vec2(w, h));
            for both in [false, true] {
                let l = popped_layout(sheet, both);
                let at = format!("{w}x{h} {both}: {l:?}");
                let col = l.board.union(l.deck);
                assert!((col.width() + l.middle.width() - w).abs() < 0.01, "{at}");
                assert!(col.width() >= SIDE_COLUMN - 0.01, "{at}");
                assert!(l.pages.contains_rect(l.picture), "{at}");
                assert!(
                    l.picture.bottom() + SIDE_TOAST <= l.tabs.top() + 0.01,
                    "{at}"
                );
                assert!(
                    (l.picture.center().x - l.tabs.center().x).abs() < 0.01,
                    "{at}"
                );
                assert!((l.picture.aspect_ratio() - 1.6).abs() < 0.01, "{at}");
                assert_eq!(l.tabs.right(), l.board.left(), "{at}");
                assert_eq!(l.board.bottom(), l.deck.top(), "{at}");
                // room for at least a strip pane over the deck
                assert!(l.board.height() >= 200.0, "{at}");
                let plates = deck_plates(l.deck, both, l.deck_scale, true);
                let n = if both { 3 } else { 2 };
                assert!(plates[..n].iter().all(|r| l.deck.contains_rect(*r)), "{at}");
                assert!(plates[1].width() <= l.deck.width() - 31.99, "{at}");
                assert!(plates[0].bottom() < plates[1].top(), "{at}");
                assert!(l.deck_scale >= 1.0, "{at}");
            }
        }
    }

    /// The board window: the board as large as the window allows, centred (bars on two sides only), its bench beside
    /// the pitch from 2:3 wide, under it on taller windows.
    #[test]
    fn the_board_window_fills_its_shape() {
        for (w, h, beside) in [
            (960.0, 1232.0, true),
            (1080.0, 1920.0, false),
            (2360.0, 1640.0, true),
            (1640.0, 2360.0, true),
            (1920.0, 1080.0, true),
            (700.0, 1400.0, false),
        ] {
            let room = Rect::from_min_size(Pos2::ZERO, vec2(w, h) / board_scale(w, h, 96.0));
            let (b, side) = desk_board_rect(room);
            let at = format!("{w}x{h}: {room:?} {b:?}");
            assert_eq!(side, beside, "{at}");
            let inner = room.shrink(16.0);
            assert!(inner.expand(0.01).contains_rect(b), "{at}");
            assert!(b.bottom() + DESK_FOOT <= inner.bottom() + 0.01, "{at}");
            // filled across or top to bottom, centred the other way
            let across = (b.width() - inner.width()).abs() < 0.01;
            let down = (b.height() + DESK_FOOT - inner.height()).abs() < 0.01;
            assert!(across || down, "{at}");
            assert!((b.center().x - inner.center().x).abs() < 0.01, "{at}");
            let ratio = if side {
                board::COLUMN_ASPECT
            } else {
                board::row_height(b.width()) / b.width()
            };
            assert!((b.height() / b.width() - ratio).abs() < 1e-3, "{at}");
        }
    }

    /// `board_buttons=on`: the deck in a column at the right of a landscape board window, in a row under a portrait
    /// one, its plates inside it and apart from the board's room; off, the board has the whole window.
    #[test]
    fn the_board_window_buttons() {
        for (w, h, stacked) in [
            (1920.0, 1080.0, true),
            (2360.0, 1640.0, true),
            (1080.0, 1920.0, false),
            (960.0, 1232.0, false),
        ] {
            let sheet = Rect::from_min_size(Pos2::ZERO, vec2(w, h));
            for both in [false, true] {
                assert_eq!(desk_parts(sheet, false, both), (sheet, None));
                let (room, deck) = desk_parts(sheet, true, both);
                let (d, s, stack) = deck.expect("a deck");
                let at = format!("{w}x{h} {both}: {room:?} {d:?} {s}");
                assert_eq!(stack, stacked, "{at}");
                assert!(!room.intersects(d.shrink(0.5)), "{at}");
                assert!((room.union(d).area() - sheet.area()).abs() < 1.0, "{at}");
                let n = if both { 3 } else { 2 };
                let plates = deck_plates(d, both, s, stack);
                assert!(plates[..n].iter().all(|r| d.contains_rect(*r)), "{at}");
                assert!(s >= 1.0, "{at}");
                let (b, _) = desk_board_rect(room);
                assert!(room.contains_rect(b), "{at}");
            }
        }
    }

    /// One frame of `st` on `ctx`, a `w` x `h` sheet at time `t`.
    fn frame(
        ctx: &egui::Context,
        st: &mut PanelState,
        w: f32,
        h: f32,
        t: f64,
        events: Vec<egui::Event>,
    ) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, h))),
            time: Some(t),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| show(ui, st)).textures_delta.clear();
    }

    /// A card pressed on the board window is the game window's touched card: shown there at once, gone
    /// [`TOUCH_SECS`] after; the board window shows nothing big and selects nothing.
    #[test]
    fn a_card_touched_on_the_board_window_shows_on_the_game_window() {
        let mut st = PanelState::sample(Edition::E1718, Lang::En, Page::Game);
        st.file.board_window = true;
        let (board_ctx, game_ctx) = (egui::Context::default(), egui::Context::default());
        for c in [&board_ctx, &game_ctx] {
            theme::install(c, Edition::E1718, &theme::font_dir()).unwrap();
            c.run_ui(egui::RawInput::default(), |_| {})
                .textures_delta
                .clear();
        }
        let (w, h) = (960.0, 1232.0);
        let (at, beside) = desk_board_rect(Rect::from_min_size(Pos2::ZERO, vec2(w, h)));
        assert!(beside);
        let slot = (0..11).find(|&s| st.board.slots[s].is_some()).unwrap();
        let card = st.board.cards[st.board.slots[slot].unwrap()].number;
        let pos = board::Mat::column(at.min, at.width()).slot_rects(&st.board.spots)[slot].center();
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let both = |st: &mut PanelState, t: f64, events: Vec<egui::Event>| {
            st.desk = true;
            frame(&board_ctx, st, w, h, t, events);
            st.desk = false;
            st.popped = true;
            frame(&game_ctx, st, 1920.0, 1080.0, t, vec![]);
        };
        let big =
            |ctx: &egui::Context| ctx.memory(|m| m.area_rect(egui::Id::new("board-big")).is_some());
        both(&mut st, 0.0, vec![egui::Event::PointerMoved(pos)]);
        assert!(st.touched.is_none());
        both(&mut st, 0.1, vec![press(true)]);
        assert_eq!(st.touched.as_ref().map(|t| t.0.number), Some(card));
        assert!(big(&game_ctx), "the game window shows it");
        both(&mut st, 0.2, vec![press(false)]);
        // rested on past the hover delay: still no big card on the board window
        both(&mut st, 3.0, vec![]);
        assert!(!big(&board_ctx), "nothing big on the board window");
        assert!(st.touched.is_some(), "kept for a while");
        both(&mut st, 0.2 + TOUCH_SECS + 0.1, vec![]);
        assert!(st.touched.is_none(), "gone after TOUCH_SECS");
        // a dock forgets the card touched last
        both(&mut st, 10.0, vec![press(true)]);
        assert!(st.touched.is_some());
        st.set_popped(false);
        assert!(st.touched.is_none(), "docked: forgotten");
    }

    #[test]
    fn layout_numbers() {
        // 1440x900 as before: the picture 860 wide, the board 580
        let l = layout(sheet_of(1440.0, 900.0), false);
        assert!((l.picture.width() - 860.0).abs() < 0.01 && l.board.width() == 580.0);
        // a 2560x1440 client: the picture is width-bound (1980x1237.5), flush top, the deck row right under it
        let l = layout(sheet_of(2560.0, 1440.0), false);
        assert_eq!(l.picture.size(), vec2(1977.6, 1236.0)); // height-bound: 1440 - 56 - 148
        assert_eq!(l.picture.top(), 0.0);
        assert!((l.board.width() - 582.4).abs() < 0.01);
        assert_eq!(l.deck.top(), l.picture.bottom());
        assert!(l.deck.height() >= 148.0);
        // the share of the window the picture takes: 1080p and 1440p maximized (client 1920x1009, 2560x1373)
        let share = |w: f32, h: f32| {
            let l = layout(sheet_of(w, h), false);
            l.picture.area() * scale(w, h, 96.0).powi(2) / (w * h)
        };
        assert!(share(1920.0, 1080.0) > 0.54 && share(1920.0, 1009.0) > 0.52);
        assert!(share(2560.0, 1373.0) > 0.62 && share(2560.0, 1440.0) > 0.66);
        // 4K: the sheet is 2x
        assert_eq!(scale(3840.0, 2160.0, 96.0), 2.0);
        assert_eq!(scale(2560.0, 1369.0, 96.0), 1.0);
        assert!((scale(1280.0, 800.0, 96.0) - 0.8889).abs() < 1e-3);
    }

    #[test]
    fn scale_follows_the_monitor() {
        // 2560x1440 at 125%, maximized: 1.25, a 2048x1099 sheet drawn at the screen's pixels
        assert_eq!(scale(2560.0, 1374.0, 120.0), 1.25);
        assert_eq!(scale(3840.0, 2088.0, 144.0), 1.5);
        assert_eq!(scale(3840.0, 2160.0, 96.0), 2.0); // 100%: the 4K rule
        // the sheet stays at least 1440x900 points: 1600x1000 at 150% gets 1.111, a smaller window scales down
        assert!((scale(1600.0, 1000.0, 144.0) - 1.1111).abs() < 1e-3);
        assert!((scale(1280.0, 800.0, 120.0) - 0.8889).abs() < 1e-3);
        assert_eq!(scale(2560.0, 1440.0, 288.0), 1.6);
    }

    #[test]
    fn deck_layout_is_one_row() {
        // design choice: one row, KEY PLAYER left of SHOOT, unlike the cabinet's two plates (more room for the picture)
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(860.0, 148.0));
        let items = deck_layout(area);
        let at = |a: &str| items.iter().find(|i| i.action == a).unwrap().rect;
        assert_eq!(items.len(), 10);
        // the diamond: CENTRAL above PRESS above COUNTER, L-SIDE left of PRESS, R-SIDE right of it
        assert!(at("UP").bottom() <= at("PRESS").top() && at("PRESS").bottom() <= at("DOWN").top());
        assert!(
            at("LEFT").right() <= at("PRESS").left() && at("PRESS").right() <= at("RIGHT").left()
        );
        // the line: KEY PLAYER, SHOOT, KEEPER, START; SHOOT and KEEPER big, the other two small
        let line = ["KEYPLAYER", "SHOOT", "KEEPER", "START"].map(at);
        assert!(line.windows(2).all(|w| w[0].right() <= w[1].left()));
        assert!(
            at("SHOOT").width() >= 80.0
                && at("KEYPLAYER").width() < 50.0
                && at("START").width() < 50.0
        );
        assert!(
            at("DATA").left() >= at("RIGHT").right()
                && at("DATA").right() <= at("KEYPLAYER").left()
        );
        // one row: every centre on the same line; the plates and machine keys fit the area
        assert!(
            items
                .iter()
                .all(|i| (i.rect.center().y - area.center().y).abs() < 0.01 || i.tactic.is_some())
        );
        for (n, a) in items.iter().enumerate() {
            assert!(area.contains_rect(a.rect), "{} leaves the area", a.action);
            for b in &items[n + 1..] {
                assert!(
                    !a.rect.intersects(b.rect),
                    "{} overlaps {}",
                    a.action,
                    b.action
                );
            }
        }
        let plates = deck_plates(area, true, 1.0, false);
        assert!(plates.iter().all(|p| area.contains_rect(*p)));
        assert!(
            items
                .iter()
                .all(|i| plates.iter().any(|p| p.contains_rect(i.rect)))
        );
        assert!(plates[2].contains_rect(card_key(area, 1.0, false)));
        // without the machine plate (2017-18) the two plates drawn are centred
        let two = deck_plates(area, false, 1.0, false);
        assert!((two[0].left() - area.left() - (area.right() - two[1].right())).abs() < 0.01);
    }

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
            record: None,
            name: name.into(),
            short: name.rsplit(' ').next().unwrap_or(name).to_uppercase(),
            club: club.into(),
            country: country.into(),
            season: "2004-05".into(),
            born: String::new(),
            pos,
            rarity: if no.is_multiple_of(2) {
                Rarity::Legend
            } else {
                Rarity::Regular
            },
            code: String::new(),
            secret: false,
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
        // the printed code and the short season: "kaka mvp 0405" (a player's search for an MVP card, 2026-10-23)
        let mut c = c;
        c[0].code = "EMVP".into(); // Kakà
        assert_eq!(found(&c, &q("kaka mvp 0405")), [10]);
        assert_eq!(found(&c, &q("emvp")), [10]);
        assert_eq!(found(&c, &q("milano 0506")), Vec::<u32>::new());
        assert_eq!(found(&c, &q("kaka 04/05")), [10]);
        // the rarity in any language: 101 is REGULAR (odd), 10, 32 and 210 LEGEND
        assert_eq!(found(&c, &q("regular")), [101]);
        assert_eq!(found(&c, &q("normale")), [101]);
        assert_eq!(found(&c, &q("kaka leggenda")), [10]);
        assert_eq!(found(&c, &q("legend italia")), [32, 210]);
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
        assert_eq!(st.message.as_ref().unwrap().text, "KAKÀ is on the table");
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

    fn importer() -> (PanelState, Vec<Placed>) {
        let st = PanelState::new(
            Edition::E1011,
            Lang::En,
            cards::board(&[], &[]),
            cards().into(),
        );
        let old = vec![Placed {
            slot: 1,
            no: 10,
            x: 0.0,
            y: 0.9,
        }];
        (st, old)
    }

    #[test]
    fn import_is_refused_in_a_match_for_another_edition_and_when_empty() {
        let (mut st, old) = importer();
        let text = "game 1011\nP 1 50 90\n";
        for (lineup, in_match, said) in [
            (text, true, "Not during a match. Import between matches."),
            (
                "game 1718\nP 1 50 90\n",
                false,
                "This lineup is for 2017-18.",
            ),
            (
                "",
                false,
                "No lineup on the clipboard. Copy one on db.wccf.it first.",
            ),
        ] {
            assert_eq!(import_lineup(&mut st, &old, lineup, in_match, 0.0), None);
            let m = st.message.as_ref().unwrap();
            assert_eq!((m.tone, m.text.as_str()), (Tone::Refused, said));
            assert!(st.import_undo.is_none());
        }
        // nothing this game has: refused, the misses listed
        assert_eq!(import_lineup(&mut st, &old, "998 999", false, 0.0), None);
        assert_eq!(
            st.message.as_ref().unwrap().text,
            "None of these cards is in this game.\n998 (not in the catalogue), 999 (not in the catalogue)"
        );
    }

    #[test]
    fn import_replaces_the_table_and_undo_puts_it_back() {
        let (mut st, old) = importer();
        let text = "# WCCF DB lineup\ngame 1011\nname Test XI\nP 101 50 90 K\nP 210 50 30\nS 32\n\
                    S \"13/GES 3 \" # OLD · 13 GES 3\nS 999\n";
        let new = import_lineup(&mut st, &old, text, false, 5.0).unwrap();
        assert_eq!(
            new.iter().map(|p| (p.slot, p.no)).collect::<Vec<_>>(),
            [(1, 101), (2, 210), (3, 32)]
        );
        assert_eq!((new[0].x, new[0].y), (0.0, 0.8));
        assert!(new[2].on_bench());
        assert_eq!(st.board.cards[0].number, 101);
        let kp = st
            .cards
            .iter()
            .find(|c| c.number == 101)
            .unwrap()
            .short_name()
            .to_owned();
        let m = st.message.as_ref().unwrap();
        assert_eq!(m.tone, Tone::Notice);
        assert_eq!(
            m.text,
            format!(
                "Test XI: 3 cards on the table.\nKey player on the site: {kp}. Set it in the game.\n\
                 Skipped: OLD · 13 GES 3 (no number in this game), 999 (not in the catalogue)"
            )
        );
        assert_eq!(st.import_undo, Some((old.clone(), 5.0)));
        // a match began meanwhile: UNDO refused too
        assert_eq!(undo_import(&mut st, true), None);
        assert!(st.import_undo.is_some());
        assert_eq!(undo_import(&mut st, false), Some(old));
        assert_eq!(st.board.cards[0].number, 10);
        assert_eq!(undo_import(&mut st, false), None);
    }

    #[test]
    fn import_asks_first_cancel_keeps_the_table_confirm_imports_and_undo_restores() {
        let (mut st, old) = importer();
        let text = "game 1011\nP 101 50 90\nP 210 50 30\nS 999\n";
        // refused in a match: no question
        assert!(!ask_import(&mut st, text, true));
        assert!(st.import_ask.is_none());
        assert!(ask_import(&mut st, text, false));
        let ask = st.import_ask.clone().unwrap();
        assert_eq!((ask.cards, ask.skipped), (2, 1));
        // asking changes nothing; CANCEL drops the question
        assert!(st.import_undo.is_none() && st.board.cards.is_empty());
        st.import_ask = None;
        assert!(st.import_undo.is_none() && st.board.cards.is_empty());
        // CONFIRM: the panel DLL imports the asked text; UNDO puts the old table back
        assert!(ask_import(&mut st, text, false));
        let ask = st.import_ask.take().unwrap();
        let new = import_lineup(&mut st, &old, &ask.text, false, 1.0).unwrap();
        assert_eq!(new.iter().map(|p| p.no).collect::<Vec<_>>(), [101, 210]);
        assert_eq!(undo_import(&mut st, false), Some(old));
        assert_eq!(st.board.cards[0].number, 10);
    }

    #[test]
    fn add_card_refuses_the_players_second_card() {
        let mut v = cards();
        let mut kaka = v[0].clone(); // card 10, Ricardo Kakà 2004-05
        (kaka.number, kaka.season, kaka.name) = (11, "2005-06".into(), "RICARDO KAKA".into());
        v.push(kaka);
        let mut st = PanelState::new(Edition::E1011, Lang::En, cards::board(&[], &[]), v.into());
        let mut placed = Vec::new();
        assert!(add_card(&mut st, &mut placed, 10));
        assert!(!add_card(&mut st, &mut placed, 11));
        assert_eq!((placed.len(), placed[0].no), (1, 10));
        let m = st.message.as_ref().unwrap();
        assert_eq!(
            (m.tone, m.text.as_str()),
            (
                Tone::Refused,
                "KAKÀ: another card of this player is on the table (2004-05)"
            )
        );
    }

    #[test]
    fn the_advanced_group_is_the_operators_switches() {
        assert_eq!(
            forza::keys::NAMES[OPERATOR],
            ["TEST", "SERVICE", "SENSOR2", "SENSOR3"]
        );
    }

    #[test]
    fn deck_keycap_names_the_key_and_the_control() {
        let mut st = PanelState::new(
            Edition::E1011,
            Lang::En,
            cards::board(&[], &[]),
            cards().into(),
        );
        assert_eq!(deck_keycap(&st, "SHOOT", true), "C");
        assert_eq!(deck_keycap(&st, "SHOOT", false), ""); // hidden unless Alt is held
        assert_eq!(deck_keycap(&st, "UP", true), "Up");
        st.pads[2] = forza::keys::PAD_BUTTON + 3; // SHOOT: controller button 3
        assert_eq!(deck_keycap(&st, "SHOOT", true), "C"); // the key first
        st.keys[2] = 0;
        assert_eq!(deck_keycap(&st, "SHOOT", true), "Button 3");
        st.pads[2] = 0;
        assert_eq!(deck_keycap(&st, "SHOOT", true), "");
        assert_eq!(deck_keycap(&st, "NOPE", true), "");
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
        assert_eq!(st.message.as_ref().unwrap().text, "SHOOT = X; PRESS = C");
        // Esc and F10 (must stay free) and Alt+G: refused or cancelled, nothing changes
        st.capturing = Some(0);
        st.vk_down = vec![(0x79, false), (0x47, true)];
        keys_input(&mut st);
        assert_eq!(st.message.as_ref().unwrap().text, "Alt is not allowed");
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
            forza::keys::keys_text(false, &st.keys, &st.pads)
                .lines()
                .last(),
            Some("PAD DATA=B3")
        );
    }
}
