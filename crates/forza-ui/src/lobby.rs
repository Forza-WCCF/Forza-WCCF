//! The online lobby (ROADMAP, "Online lobby view"): the shops and VS rooms a server lists, one row each, drawn from
//! [`LobbyView`] alone. [`show`] reads no socket and keeps no thread: it returns the [`LobbyAction`] the player chose
//! and the caller does the rest. Milestone L0 runs on [`LobbyView::sample`] data.

use super::Lang;
use super::theme;
use super::widgets::{
    Cab, Tone, action, cabinet, caps, check_chip, field, fit, fold, section, segmented,
    status_line, toggle_row,
};
use egui::{Key, Modifiers, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// A window this view fits.
pub const SIZE: egui::Vec2 = egui::vec2(600.0, 700.0);
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
    /// A shop room a host listed (2017-18): joined from its row even where SHOPS are view-only.
    pub room: bool,
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
    /// No room has the code typed.
    NoRoom,
    /// The room of the code typed has no free seat.
    RoomFull,
    /// The shop's host left before this PC got in.
    ShopClosed,
    /// HOST SHOP: the lobby no longer holds this PC's room.
    RoomLost,
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
    /// This PC's seat, as an index into `seats`.
    pub mine: Option<usize>,
    /// Seconds left before the run starts (VS: both ready, or no opponent in time).
    pub countdown: Option<u32>,
    /// No opponent came in time: the game gives this PC a CPU team.
    pub cpu: bool,
}

impl Room {
    /// A room of `n` empty seats.
    pub fn new(title: &str, kind: Kind, n: usize) -> Room {
        Room {
            title: title.into(),
            kind,
            seats: vec![None; n],
            next_entry: None,
            mine: None,
            countdown: None,
            cpu: false,
        }
    }

    /// Every seat taken and ready.
    pub fn all_ready(&self) -> bool {
        !self.seats.is_empty()
            && self
                .seats
                .iter()
                .all(|s| s.as_ref().is_some_and(|o| o.ready))
    }

    /// This PC pressed READY.
    pub fn ready_here(&self) -> bool {
        self.mine
            .and_then(|i| self.seats.get(i)?.as_ref())
            .is_some_and(|o| o.ready)
    }
}

/// The gamepad, read with winmm by Launcher.exe and fed to [`show`] as these keys: D-pad (and the left stick) the
/// arrows, A Enter, B Escape, LB/RB the kinds (Left/Right), X refresh (F5), Y HOST VS (H). `code` is a
/// `forza::keys` control number (`PAD_BUTTON + n` for button n, `PAD_HAT + 0..4` up right down left, `PAD_AXIS +
/// 2 * axis + plus`). Buttons as an XInput pad reports them through winmm: 1 A, 2 B, 3 X, 4 Y, 5 LB, 6 RB.
pub fn pad_key(code: u32) -> Option<Key> {
    use forza::keys::{PAD_AXIS, PAD_BUTTON, PAD_HAT};
    Some(match code {
        c if c == PAD_HAT || c == PAD_AXIS + 2 => Key::ArrowUp,
        c if c == PAD_HAT + 1 || c == PAD_AXIS + 1 => Key::ArrowRight,
        c if c == PAD_HAT + 2 || c == PAD_AXIS + 3 => Key::ArrowDown,
        c if c == PAD_HAT + 3 || c == PAD_AXIS => Key::ArrowLeft,
        c if c == PAD_BUTTON + 1 => Key::Enter,
        c if c == PAD_BUTTON + 2 => Key::Escape,
        c if c == PAD_BUTTON + 3 => Key::F5,
        c if c == PAD_BUTTON + 4 => Key::H,
        c if c == PAD_BUTTON + 5 => Key::ArrowLeft,
        c if c == PAD_BUTTON + 6 => Key::ArrowRight,
        _ => return None,
    })
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
    /// No opponent: wait for one again instead of the CPU team.
    KeepWaiting,
    /// No opponent: start the CPU match now.
    CpuNow,
    /// The typed address, on Enter in the Direct address field.
    DirectAddress(String),
    /// 2017-18: open this PC's shop to others (a shop room with a code).
    HostShop,
    /// A room code, typed: a shop room's (6 letters) or a listed VS room's (4).
    JoinCode(String),
    /// 2017-18: the "List my shop" switch, set to this (HOST SHOP lists the room in SHOPS).
    ListShop(bool),
    /// 2017-18: open this PC's shop to PCs on its network (no internet).
    HostLan,
    /// 2017-18: join the shop at this address on the network (found, or typed).
    JoinLan(String),
}

/// A shop found on this PC's network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanShop {
    pub name: String,
    /// `IP`, or `IP:PORT` when its seat desk is not on the usual port.
    pub address: String,
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
    /// 2017-18: the shops are listed; only shop rooms join (see [`shop_rooms`](Self::shop_rooms)).
    pub shops_view_only: bool,
    pub address: String,
    pub direct_open: bool,
    /// Scroll the selected row into view on the next frame.
    pub scroll: bool,
    /// 2017-18: the room code box.
    pub codes: bool,
    /// 2017-18 with the shared shop on: HOST SHOP, and shop codes in the box.
    pub shop_rooms: bool,
    /// The code typed in the box.
    pub code: String,
    /// The code being joined, until the run starts.
    pub joining: Option<String>,
    /// HOST SHOP is on: this PC's shop, with its room code once the lobby gave one.
    pub hosting: bool,
    pub host_code: Option<String>,
    /// 2017-18: HOST SHOP lists the room in others' SHOPS (the "List my shop" switch, data\settings.json `list_shop`).
    pub list_shop: bool,
    /// The lobby server shop rooms are on (`host:port`): the host's room's, or the one a code was asked of.
    pub desk: String,
    /// 2017-18 with the shared shop on: the LAN fold, the shops found on this network and the address typed there.
    pub lan_open: bool,
    pub lan: Vec<LanShop>,
    pub lan_address: String,
    /// HOST LAN SHOP is on: this PC's addresses on its network (shown instead of a room code).
    pub host_lan: Option<Vec<String>>,
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
    /// The VS ROOMS list, open rooms only.
    VsList,
    /// HOST VS just made a room: its code, this PC alone and not ready yet.
    Host,
    /// Both ready: the 5 s countdown.
    RoomReady,
    /// No opponent in time: the CPU line and the countdown.
    Cpu,
    /// HOST SHOP: the room code and its copy button.
    ShopHost,
    /// A shop code typed and being joined.
    ShopJoining,
    /// A shop code no room has.
    ShopNoRoom,
    /// HOST SHOP after the lobby lost the room: no code, the line that says so.
    ShopLost,
    /// The LAN fold open with two shops found.
    LanList,
    /// The LAN fold open, nothing found: the hint.
    LanNone,
    /// HOST LAN SHOP: this PC's addresses.
    LanHost,
}

impl Scenario {
    pub const ALL: [Scenario; 17] = [
        Scenario::List,
        Scenario::Empty,
        Scenario::Full,
        Scenario::Offline,
        Scenario::RoomWait,
        Scenario::Greyed,
        Scenario::VsList,
        Scenario::Host,
        Scenario::RoomReady,
        Scenario::Cpu,
        Scenario::ShopHost,
        Scenario::ShopJoining,
        Scenario::ShopNoRoom,
        Scenario::ShopLost,
        Scenario::LanList,
        Scenario::LanNone,
        Scenario::LanHost,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            Scenario::List => "list",
            Scenario::Empty => "empty",
            Scenario::Full => "full",
            Scenario::Offline => "offline",
            Scenario::RoomWait => "room_wait",
            Scenario::Greyed => "greyed",
            Scenario::VsList => "vs_list",
            Scenario::Host => "host",
            Scenario::RoomReady => "room_ready",
            Scenario::Cpu => "cpu",
            Scenario::ShopHost => "shop_host",
            Scenario::ShopJoining => "shop_joining",
            Scenario::ShopNoRoom => "shop_no_room",
            Scenario::ShopLost => "shop_lost",
            Scenario::LanList => "lan_list",
            Scenario::LanNone => "lan_none",
            Scenario::LanHost => "lan_host",
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
        room: false,
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
            shops_view_only: false,
            address: "192.168.1.20".into(),
            direct_open: false,
            scroll: true,
            codes: false,
            shop_rooms: false,
            code: String::new(),
            joining: None,
            hosting: false,
            host_code: None,
            list_shop: false,
            desk: String::new(),
            lan_open: false,
            lan: vec![],
            lan_address: String::new(),
            host_lan: None,
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
                "FORZA ROMA",
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
                    "FORZA ROMA",
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
            Scenario::RoomWait | Scenario::Host | Scenario::RoomReady | Scenario::Cpu => {
                let club = |c: &str, ready| {
                    Some(Occupant {
                        club: c.into(),
                        ready,
                    })
                };
                v.kind = Kind::Vs;
                let mut r = Room::new("K7QM", Kind::Vs, 2);
                r.mine = Some(0);
                r.seats[0] = club("A7K2", s != Scenario::Host);
                if s == Scenario::RoomReady {
                    r.seats[1] = club("Q3ZD", true);
                    r.countdown = Some(3);
                }
                if s == Scenario::Cpu {
                    (r.cpu, r.countdown) = (true, Some(5));
                }
                v.room = Some(r);
            }
            // a filter on, so the snapshots show its checked state
            Scenario::VsList => (v.kind, v.open_only) = (Kind::Vs, true),
            Scenario::Greyed => {
                (v.grey[Kind::Vs as usize], v.kind) = (Some(Grey::Vs1011), Kind::Vs);
                v.vs.clear();
            }
            Scenario::ShopHost
            | Scenario::ShopJoining
            | Scenario::ShopNoRoom
            | Scenario::ShopLost => {
                // 2017-18 with the shared shop on: one public room listed, the rest by code
                (v.codes, v.shop_rooms, v.shops_view_only) = (true, true, true);
                let mut room = row("p1", "Shop room", 2, 8, Some(24), None, Open, None);
                room.room = true;
                v.shops = vec![room];
                v.code = "K7QM2XPD".into();
                v.desk = "lobby.wccf.it:20032".into();
                match s {
                    Scenario::ShopHost => {
                        (v.hosting, v.host_code) = (true, Some("K7QM2XPD".into()))
                    }
                    Scenario::ShopJoining => {
                        (v.joining, v.list_shop) = (Some("K7QM2XPD".into()), true)
                    }
                    Scenario::ShopLost => (v.hosting, v.notice) = (true, Some(Notice::RoomLost)),
                    _ => v.notice = Some(Notice::NoRoom),
                }
            }
            Scenario::LanList | Scenario::LanNone | Scenario::LanHost => {
                // no internet: the lobby does not answer, the LAN fold does the work
                (v.codes, v.shop_rooms, v.shops_view_only) = (true, true, true);
                (v.offline, v.updated_s) = (true, None);
                v.shops.clear();
                v.lan_open = true;
                if s == Scenario::LanList {
                    let shop = |name: &str, address: &str| LanShop {
                        name: name.into(),
                        address: address.into(),
                    };
                    v.lan = vec![
                        shop("SALOTTO-PC", "192.168.1.20"),
                        shop("MARCO-LAPTOP", "192.168.1.34"),
                    ];
                }
                if s == Scenario::LanHost {
                    v.hosting = true;
                    v.host_lan = Some(vec!["192.168.1.20".into()]);
                }
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

    /// The SHOPS tab shows its rows but cannot join.
    pub fn view_only_now(&self) -> bool {
        self.shops_view_only && self.kind == Kind::Shops
    }

    /// Row `r` is shown but cannot be joined: a view-only SHOPS row, unless it is a listed shop room.
    pub fn view_only(&self, r: &Row) -> bool {
        self.view_only_now() && !(self.shop_rooms && r.room)
    }

    /// The selected row, whatever its state.
    fn selected_row(&self) -> Option<&Row> {
        Some(&self.rows()[*self.visible().get(self.selected)?])
    }

    pub fn grey_now(&self) -> Option<Grey> {
        self.grey[self.kind as usize]
    }

    fn stale(&self) -> bool {
        self.updated_s.is_some_and(|s| s > STALE_S)
    }

    /// A row listed on its tab: 2017-18 lists only shop rooms (no desk runs a 2017-18 shop of its own to join).
    fn listed(&self, r: &Row) -> bool {
        !self.view_only_now() || r.room
    }

    /// The rows the filters let through, as indexes into [`rows`](Self::rows).
    pub fn visible(&self) -> Vec<usize> {
        let want = [None, Some(Lang::En), Some(Lang::It), Some(Lang::Ja)][self.lang_filter.min(3)];
        self.rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                self.listed(r)
                    && want.is_none_or(|l| r.lang == Some(l))
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
        let r = self.selected_row()?;
        (r.joinable() && !self.view_only(r)).then_some(r)
    }

    /// Why JOIN is greyed, in a few words; `None` when it can join or the greyed list already says why.
    pub fn why_not_join(&self, lang: Lang) -> Option<String> {
        if self.joinable().is_some() || self.grey_now().is_some() {
            return None;
        }
        Some(if self.offline {
            tr!(lang, "lobby.why_offline")
        } else if self.stale() {
            tr!(lang, "lobby.why_stale")
        } else {
            match self.selected_row().map(|r| &r.state) {
                None => tr!(lang, "lobby.why_pick"),
                Some(RowState::Full) => tr!(lang, "lobby.full"),
                Some(RowState::Closed) => tr!(lang, "lobby.closed"),
                Some(RowState::NoAnswer) => tr!(lang, "lobby.no_answer"),
                Some(RowState::NeedsVersion(x)) => tr!(lang, "lobby.needs_version", v = x),
                Some(RowState::Open | RowState::InMatch) => return None,
            }
        })
    }

    /// The SHOPS tab offers HOST SHOP (2017-18 with the shared shop on) instead of HOST VS.
    pub fn host_shop_now(&self) -> bool {
        self.shop_rooms && self.kind == Kind::Shops
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
    if v.hosting {
        return None;
    }
    if let Some(r) = &v.room {
        let can = !r.ready_here() && r.countdown.is_none();
        return (pressed(Key::Enter, none) && can).then_some(LobbyAction::Ready);
    }
    if pressed(Key::H, none) && !v.offline {
        if v.host_shop_now() {
            return Some(LobbyAction::HostShop);
        }
        if v.grey[Kind::Vs as usize].is_none() {
            return Some(LobbyAction::HostVs);
        }
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
    show_with(ui, v, lang, |_| {})
}

/// [`show`], with `more` drawn under the address in the Direct address fold (the 2017-18 club server rows).
pub fn show_with(
    ui: &mut Ui,
    v: &mut LobbyView,
    lang: Lang,
    more: impl FnOnce(&mut Ui),
) -> Option<LobbyAction> {
    let mut act = if ui.ctx().egui_wants_keyboard_input() {
        None
    } else {
        keys(ui, v)
    };
    let title = tr!(lang, "lobby.title");
    section(ui, &title, theme::AMBER, |ui| {
        if v.hosting {
            host_view(ui, v, lang, &mut act);
        } else if v.room.is_some() {
            room_view(ui, v, lang, &mut act);
        } else {
            list_view(ui, v, lang, &mut act);
        }
    });
    ui.add_space(8.0);
    let summary = v.address.clone();
    let direct = tr!(lang, "lobby.direct");
    // the server and club settings stay as they are while this PC hosts
    ui.add_enabled_ui(!v.hosting, |ui| {
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
                more(ui);
            },
        );
    });
    // a host's own LAN addresses are in the view above
    if v.shop_rooms && !v.hosting {
        ui.add_space(8.0);
        let summary = tr!(lang, "lobby.lan_found", n = v.lan.len());
        let mut open = v.lan_open;
        fold(
            ui,
            &tr!(lang, "lobby.lan"),
            &summary,
            &mut open,
            theme::BLUE,
            |ui| {
                lan_view(ui, v, lang, &mut act);
            },
        );
        v.lan_open = open;
    }
    act
}

/// The LAN fold (2017-18, no internet needed): HOST LAN SHOP, the shops found on this network, an address typed.
fn lan_view(ui: &mut Ui, v: &mut LobbyView, lang: Lang, act: &mut Option<LobbyAction>) {
    if action(ui, &tr!(lang, "lobby.host_lan")).clicked() {
        *act = Some(LobbyAction::HostLan);
    }
    ui.add_space(6.0);
    for s in &v.lan {
        ui.horizontal(|ui| {
            if action(ui, &tr!(lang, "lobby.join")).clicked() {
                *act = Some(LobbyAction::JoinLan(s.address.clone()));
            }
            ui.label(
                egui::RichText::new(&s.name)
                    .font(theme::body(15.0))
                    .color(theme::TEXT),
            );
            ui.label(
                egui::RichText::new(&s.address)
                    .font(theme::mono(14.0))
                    .color(theme::TEXT_DIM),
            );
        });
    }
    if v.lan.is_empty() {
        ui.add(egui::Label::new(
            egui::RichText::new(tr!(lang, "lobby.lan_none"))
                .font(theme::body(13.0))
                .color(theme::TEXT_DIM),
        ));
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field(ui, &tr!(lang, "lobby.lan_type"), 130.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut v.lan_address)
                .font(theme::mono(15.0))
                .desired_width(180.0)
                .margin(vec2(10.0, 6.0)),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let typed = !v.lan_address.trim().is_empty();
        if ui
            .add_enabled_ui(typed, |ui| action(ui, &tr!(lang, "lobby.join")))
            .inner
            .clicked()
            || enter && typed
        {
            *act = Some(LobbyAction::JoinLan(v.lan_address.trim().to_owned()));
        }
    });
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

/// The room language filter and LANG column are hidden: few players yet, and no way to set a room's language.
const SHOW_LANG: bool = false;

/// The columns of a row of width `w`: x offsets and widths of NAME, PLAYERS, PING, LANG, STATE (LANG empty while
/// hidden, its width given to NAME).
fn columns(w: f32) -> [(f32, f32); 5] {
    let pad = 10.0;
    let lang = if SHOW_LANG { w * 0.10 } else { 0.0 };
    let (name, players, ping) = (w * 0.44 - lang, w * 0.14, w * 0.14);
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
        // offline, the counts are old news: the names only
        let count = |n: usize| {
            if v.offline {
                String::new()
            } else {
                format!(" {n}")
            }
        };
        let labels = [
            format!(
                "{}{}",
                tr!(lang, "lobby.shops"),
                count(
                    v.shops
                        .iter()
                        .filter(|r| !v.shops_view_only || r.room)
                        .count()
                )
            ),
            format!("{}{}", tr!(lang, "lobby.vs"), count(v.vs.len())),
        ];
        if segmented(ui, &mut k, &[&labels[0], &labels[1]], w * 0.52) {
            v.set_kind(if k == 0 { Kind::Shops } else { Kind::Vs });
        }
        let retry = v.offline
            && cabinet(
                ui,
                vec2(110.0, 30.0),
                &tr!(lang, "lobby.retry"),
                Cab::Go,
                true,
            )
            .clicked();
        if retry || (!v.offline && action(ui, &tr!(lang, "lobby.refresh")).clicked()) {
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
    // filters: check boxes under "Show:", the language as one choice of four (hidden, see SHOW_LANG)
    ui.horizontal(|ui| {
        field(ui, &tr!(lang, "lobby.show"), 100.0);
        let open = if v.kind == Kind::Shops {
            "lobby.open_shops"
        } else {
            "lobby.open_rooms"
        };
        if check_chip(ui, &tr!(lang, open), v.open_only, false).clicked() {
            v.open_only = !v.open_only;
            v.selected = 0;
        }
        if check_chip(ui, &tr!(lang, "lobby.ping_under"), v.ping_ok, false).clicked() {
            v.ping_ok = !v.ping_ok;
            v.selected = 0;
        }
    });
    if SHOW_LANG {
        ui.horizontal(|ui| {
            field(ui, &tr!(lang, "lobby.language"), 100.0);
            let all = tr!(lang, "lobby.all_langs");
            for (i, l) in [all.as_str(), "EN", "IT", "JA"].into_iter().enumerate() {
                if check_chip(ui, l, v.lang_filter == i, true).clicked() && v.lang_filter != i {
                    v.lang_filter = i;
                    v.selected = 0;
                }
            }
        });
    }
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
    .filter(|&(i, _)| SHOW_LANG || i != 3)
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
    let offline = v.offline;
    let vis = v.visible();
    v.selected = v.selected.min(vis.len().saturating_sub(1));
    egui::ScrollArea::vertical()
        .id_salt("lobby_rows")
        .min_scrolled_height(LIST_H)
        .max_height(LIST_H)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(Grey::Vs1011) = v.grey_now() {
                hint(ui, &tr!(lang, "lobby.grey_vs_1011"));
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
                let dim = offline || v.view_only(r);
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
                let seats = if v.offline {
                    "-".into()
                } else {
                    format!("{}/{}", r.players, r.max)
                };
                cell(ui, rect, cols[1], &seats, theme::bold(14.0), ink);
                let ping = r
                    .ping_ms
                    .filter(|_| !v.offline)
                    .map_or("-".into(), |p| format!("{p} ms"));
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
                let l = r
                    .lang
                    .filter(|_| SHOW_LANG)
                    .map_or(String::new(), |l| l.code().to_uppercase());
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
    if v.codes {
        code_row(ui, v, lang, act);
        ui.add_space(8.0);
    }
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
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let size = vec2(bw, 40.0);
        let on = |ui: &mut Ui, ok: bool, label: String, kind: Cab| {
            ui.add_enabled_ui(ok, |ui| cabinet(ui, size, &label, kind, false))
                .inner
                .clicked()
        };
        if on(
            ui,
            live && !v.view_only_now(),
            tr!(lang, "lobby.quick"),
            Cab::Plain,
        ) {
            *act = Some(LobbyAction::QuickMatch);
        }
        let join = ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let clicked = on(ui, can_join, tr!(lang, "lobby.join"), Cab::Go);
            if let Some(why) = v.why_not_join(lang) {
                ui.label(
                    egui::RichText::new(why)
                        .font(theme::body(12.0))
                        .color(theme::TEXT_DIM),
                );
            }
            clicked
        });
        if join.inner {
            *act = v.joinable().map(|r| LobbyAction::Join(r.id.clone()));
        }
        if v.host_shop_now() {
            if on(
                ui,
                !v.offline && v.joining.is_none(),
                tr!(lang, "lobby.host_shop"),
                Cab::Plain,
            ) {
                *act = Some(LobbyAction::HostShop);
            }
        } else if on(ui, live, tr!(lang, "lobby.host"), Cab::Plain) {
            *act = Some(LobbyAction::HostVs);
        }
    });
    // HOST SHOP's one setting, right under it
    if v.host_shop_now() {
        ui.add_space(4.0);
        let hint = tr!(
            lang,
            if v.list_shop {
                "lobby.list_shop_on"
            } else {
                "lobby.list_shop_off"
            }
        );
        let title = tr!(lang, "lobby.list_shop");
        let r = ui.add_enabled_ui(v.joining.is_none(), |ui| {
            toggle_row(ui, &mut v.list_shop, &title, &hint)
        });
        if r.inner.clicked() {
            *act = Some(LobbyAction::ListShop(v.list_shop));
        }
    }
}

/// "Room code" [______] JOIN: Enter in the box or the button joins the code typed.
fn code_row(ui: &mut Ui, v: &mut LobbyView, lang: Lang, act: &mut Option<LobbyAction>) {
    ui.add_enabled_ui(v.joining.is_none(), |ui| {
        ui.horizontal(|ui| {
            field(ui, &tr!(lang, "lobby.room_code"), 130.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut v.code)
                    .font(theme::mono(17.0))
                    .char_limit(forza::online::lobby::SHOP_CODE_LEN)
                    .desired_width(110.0)
                    .margin(vec2(10.0, 6.0)),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let typed = !v.code.trim().is_empty();
            if ui
                .add_enabled_ui(typed, |ui| action(ui, &tr!(lang, "lobby.join")))
                .inner
                .clicked()
                || enter && typed
            {
                *act = Some(LobbyAction::JoinCode(v.code.trim().to_ascii_uppercase()));
            }
        });
    });
}

/// HOST SHOP: the room code to pass on, its copy button, LEAVE (the shop closes).
fn host_view(ui: &mut Ui, v: &mut LobbyView, lang: Lang, act: &mut Option<LobbyAction>) {
    let w = ui.available_width();
    let lan = v.host_lan.is_some();
    caps(
        ui,
        &tr!(
            lang,
            if lan {
                "lobby.lan_address"
            } else {
                "lobby.room_code"
            }
        ),
    );
    // LAN: this PC's addresses; else the room's code
    let shown = match &v.host_lan {
        Some(a) => (!a.is_empty()).then(|| a.join("\n")),
        None => v.host_code.clone(),
    };
    ui.label(
        egui::RichText::new(shown.as_deref().unwrap_or("--------"))
            .font(theme::mono(34.0))
            .color(if shown.is_some() {
                theme::TEXT
            } else {
                theme::TEXT_FAINT
            }),
    );
    if lan {
        ui.label(
            egui::RichText::new(tr!(lang, "lobby.lan_host_hint"))
                .font(theme::body(13.0))
                .color(theme::TEXT_DIM),
        );
    } else if !v.desk.is_empty() {
        ui.label(
            egui::RichText::new(tr!(lang, "lobby.room_desk", desk = desk_host(&v.desk)))
                .font(theme::body(13.0))
                .color(theme::TEXT_DIM),
        );
    }
    ui.add_space(8.0);
    let (tone, text) = match (&v.host_code, v.notice) {
        _ if lan => (Tone::Done, tr!(lang, "lobby.shop_howto")),
        (_, Some(Notice::RoomLost)) => (Tone::Refused, tr!(lang, "lobby.room_lost")),
        (Some(_), _) => (Tone::Done, tr!(lang, "lobby.shop_howto")),
        (None, _) => (Tone::Notice, tr!(lang, "lobby.opening")),
    };
    status_line(ui, tone, &text);
    ui.add_space(8.0);
    let gap = 8.0;
    let size = vec2((w - gap) / 2.0, 40.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let code = shown.and_then(|s| s.lines().next().map(str::to_owned));
        let copy = ui.add_enabled_ui(code.is_some(), |ui| {
            cabinet(ui, size, &tr!(lang, "launcher.copy"), Cab::Plain, false)
        });
        if let (true, Some(c)) = (copy.inner.clicked(), code) {
            ui.ctx().copy_text(c);
        }
        if cabinet(ui, size, &tr!(lang, "lobby.leave"), Cab::Stop, false).clicked() {
            *act = Some(LobbyAction::Leave);
        }
    });
}

/// A lobby server as players name it: its host, without the port.
fn desk_host(desk: &str) -> &str {
    desk.rsplit_once(':').map_or(desk, |(h, _)| h)
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
    if let Some(c) = &v.joining {
        return Some((Tone::Notice, tr!(lang, "lobby.joining", code = c)));
    }
    let key = match v.notice {
        Some(Notice::Taken) => Some("lobby.taken"),
        Some(Notice::RoomClosed) => Some("lobby.room_closed"),
        Some(Notice::NoRoom) => {
            return Some((
                Tone::Notice,
                tr!(lang, "lobby.no_room", desk = desk_host(&v.desk)),
            ));
        }
        Some(Notice::RoomLost) => Some("lobby.room_lost"),
        Some(Notice::RoomFull) => Some("lobby.room_full"),
        Some(Notice::ShopClosed) => Some("lobby.shop_closed"),
        None => None,
    };
    if let Some(k) = key {
        return Some((Tone::Notice, tr!(lang, k)));
    }
    if let (true, Some(s)) = (v.stale(), v.updated_s) {
        return Some((Tone::Notice, tr!(lang, "lobby.stale", s = s)));
    }
    // with shop rooms on, listed rooms join from their row and the rest by code: the box says it
    if v.view_only_now() && !v.shop_rooms {
        return Some((Tone::Notice, tr!(lang, "lobby.shop_rooms_off")));
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
                // no club id yet (this PC's seat before the next LOBBY, a shop seat): nothing
                let mut t = if o.club.is_empty() {
                    String::new()
                } else {
                    tr!(lang, "lobby.club", id = o.club)
                };
                if o.ready {
                    t = format!("{t}  {}", tr!(lang, "lobby.ready"))
                        .trim_start()
                        .to_owned();
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
    let vs = room.kind == Kind::Vs;
    let mut lines = vec![];
    if room.cpu {
        lines.push((Tone::Notice, tr!(lang, "lobby.cpu")));
    }
    match room.countdown {
        Some(s) if room.cpu => lines.push((Tone::Done, tr!(lang, "lobby.starting_in", s = s))),
        Some(s) => lines.push((Tone::Done, tr!(lang, "lobby.both_ready_in", s = s))),
        None if vs && room.all_ready() => lines.push((Tone::Done, tr!(lang, "lobby.starting"))),
        None if vs && room.seats.iter().any(Option::is_none) => {
            lines.push((Tone::Notice, tr!(lang, "lobby.wait_opponent")))
        }
        None => {
            if let Some(s) = room.next_entry {
                let t = clock(s);
                lines.push((Tone::Notice, tr!(lang, "lobby.next_entry", t = t)));
            }
        }
    }
    if let Some(Notice::RoomClosed) = v.notice {
        lines.push((Tone::Notice, tr!(lang, "lobby.room_closed")));
    }
    if let (true, Some(s)) = (v.stale(), v.updated_s) {
        lines.push((Tone::Notice, tr!(lang, "lobby.stale_room", s = s)));
    }
    for (tone, text) in lines {
        status_line(ui, tone, &text);
        ui.add_space(8.0);
    }
    let gap = 8.0;
    let size = vec2((w - gap) / 2.0, 40.0);
    if room.cpu && room.countdown.is_some() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let wait = tr!(lang, "lobby.keep_waiting");
            if cabinet(ui, size, &wait, Cab::Plain, false).clicked() {
                *act = Some(LobbyAction::KeepWaiting);
            }
            if cabinet(ui, size, &tr!(lang, "lobby.cpu_now"), Cab::Go, false).clicked() {
                *act = Some(LobbyAction::CpuNow);
            }
        });
        ui.add_space(8.0);
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let can = !room.ready_here() && room.countdown.is_none();
        let ready = ui.add_enabled_ui(can, |ui| {
            cabinet(ui, size, &tr!(lang, "lobby.ready"), Cab::Go, can)
        });
        if ready.inner.clicked() {
            *act = Some(LobbyAction::Ready);
        }
        if cabinet(ui, size, &tr!(lang, "lobby.leave"), Cab::Stop, false).clicked() {
            *act = Some(LobbyAction::Leave);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use forza::keys::{PAD_AXIS, PAD_BUTTON, PAD_HAT};

    #[test]
    fn pad_buttons_are_the_lobby_keys() {
        for (code, key) in [
            (PAD_HAT, Key::ArrowUp),
            (PAD_HAT + 1, Key::ArrowRight),
            (PAD_HAT + 2, Key::ArrowDown),
            (PAD_HAT + 3, Key::ArrowLeft),
            (PAD_AXIS, Key::ArrowLeft),
            (PAD_AXIS + 1, Key::ArrowRight),
            (PAD_AXIS + 2, Key::ArrowUp),
            (PAD_AXIS + 3, Key::ArrowDown),
            (PAD_BUTTON + 1, Key::Enter),
            (PAD_BUTTON + 2, Key::Escape),
            (PAD_BUTTON + 3, Key::F5),
            (PAD_BUTTON + 4, Key::H),
            (PAD_BUTTON + 5, Key::ArrowLeft),
            (PAD_BUTTON + 6, Key::ArrowRight),
        ] {
            assert_eq!(pad_key(code), Some(key), "{code:#x}");
        }
        // Back, Start, the sticks' clicks, the triggers and the right stick do nothing here
        for code in [
            PAD_BUTTON + 7,
            PAD_BUTTON + 8,
            PAD_BUTTON + 9,
            PAD_AXIS + 4,
            PAD_AXIS + 6,
            0x0D,
        ] {
            assert_eq!(pad_key(code), None, "{code:#x}");
        }
    }

    /// 2017-18 SHOPS are view-only, but a listed shop room joins from its row while shop rooms are on.
    #[test]
    fn listed_shop_rooms_join_from_their_row() {
        let mut v = LobbyView::sample(Scenario::ShopNoRoom);
        assert_eq!(v.joinable().map(|r| r.id.as_str()), Some("p1"));
        v.shops[0].room = false; // a desk's own shop, or a desk too old to mark rooms: not listed
        assert!(v.visible().is_empty() && v.joinable().is_none());
        assert_eq!(v.why_not_join(Lang::En).as_deref(), Some("Pick a room"));
        v.shops[0].room = true;
        v.shop_rooms = false;
        assert!(v.joinable().is_none());
    }

    /// Each filter on its own lets through the rows it did before, and off again shows them all.
    #[test]
    fn filters_pick_the_rows() {
        let mut v = LobbyView::sample(Scenario::List);
        let names = |v: &LobbyView| -> Vec<String> {
            v.visible()
                .iter()
                .map(|&i| v.rows()[i].name.clone())
                .collect()
        };
        assert_eq!(names(&v), ["FORZA ROMA", "TOKYO NIGHT", "MILANO CUP"]);
        v.open_only = true;
        assert_eq!(names(&v), ["FORZA ROMA"]);
        v.open_only = false;
        v.ping_ok = true;
        assert_eq!(names(&v), ["FORZA ROMA", "MILANO CUP"]);
        v.ping_ok = false;
        for (f, want) in [
            (1, vec![]),
            (2, vec!["FORZA ROMA", "MILANO CUP"]),
            (3, vec!["TOKYO NIGHT"]),
        ] {
            v.lang_filter = f;
            assert_eq!(names(&v), want, "language {f}");
        }
        v.lang_filter = 0;
        assert_eq!(names(&v).len(), 3);
        v.kind = Kind::Vs;
        v.open_only = true;
        assert_eq!(names(&v), ["K7QM", "TQ4D"]);
    }

    /// JOIN's grey says why, in a few words, in the order `joinable` checks: offline, old list, the row.
    #[test]
    fn join_says_why_it_is_grey() {
        let why = |v: &LobbyView| v.why_not_join(Lang::En);
        let mut v = LobbyView::sample(Scenario::List);
        assert_eq!((v.joinable().is_some(), why(&v)), (true, None));
        v.selected = 1; // TOKYO NIGHT, full
        assert_eq!(why(&v).as_deref(), Some("FULL"));
        v.updated_s = Some(STALE_S + 1);
        assert_eq!(why(&v).as_deref(), Some("List out of date"));
        v.offline = true;
        assert_eq!(why(&v).as_deref(), Some("Server offline"));
        assert_eq!(
            LobbyView::sample(Scenario::Offline)
                .why_not_join(Lang::En)
                .as_deref(),
            Some("Server offline")
        );
    }
}
