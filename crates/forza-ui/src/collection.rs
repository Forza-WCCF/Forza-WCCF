//! Create-a-Team's COLLECTION tab in the Launcher (2017-18, `PLAN.md` 1.4): the first start (choose a start, see the
//! 11 dealt, pick 1 of 3 RARE cards) and then the collection page (the cards held with their copies and marks, the
//! token balance, the match tokens left this week). [`show`] draws a [`CollectionView`] and returns what the player
//! asked for; Launcher.exe deals the cards and writes the log (`forza::collection`). From the page, PACKS opens the
//! pack store (`PLAN.md` 2.1): the three packs with their odds after the step-down, and a bought pack's 3 cards turned
//! over one by one.

use super::Lang;
use super::board::{self, Card, Look, Rarity};
use super::theme;
use super::widgets::{self, Cab};
use egui::{Rect, Role, Sense, Ui, WidgetInfo, vec2};
use forza::collection::{self, Copies, LEAGUES, NATIONS, Pack, Start};

/// A start the picker offers (`PLAN.md` 2.4): all seasons, one season (its first year), one league ([`LEAGUES`]' index).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Choice {
    AllSeasons,
    Season(u16),
    League(usize),
}

impl Choice {
    /// The draw's start.
    pub fn start(self) -> Start<'static> {
        match self {
            Choice::AllSeasons => Start::AllSeasons,
            Choice::Season(y) => Start::Season(y),
            Choice::League(i) => Start::League(LEAGUES[i].0),
        }
    }

    /// The log's `why` for its starter lines, in English as the log is.
    pub fn why(self) -> String {
        match self {
            Choice::AllSeasons => "starter All seasons".into(),
            Choice::Season(y) => format!("starter {}", season(y)),
            Choice::League(i) => format!("starter {}", LEAGUES[i].0),
        }
    }

    fn label(self, lang: Lang) -> String {
        match self {
            Choice::AllSeasons => tr!(lang, "collection.all_seasons"),
            Choice::Season(y) => season(y),
            Choice::League(i) => tr!(lang, LEAGUE_KEYS[i]),
        }
    }
}

/// The league names in the window, in [`LEAGUES`]' order.
const LEAGUE_KEYS: [&str; 9] = [
    "collection.league_italy",
    "collection.league_spain",
    "collection.league_england",
    "collection.league_germany",
    "collection.league_france",
    "collection.league_netherlands",
    "collection.league_portugal",
    "collection.league_japan",
    "collection.league_south_america",
];

/// A season as the cards print it: 2013 -> "2013-14".
fn season(y: u16) -> String {
    format!("{y}-{:02}", (y + 1) % 100)
}

/// One row of the picker: the choice and the mean TOTAL its 11 land on (`None`: a banded start, 80-82).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartRow {
    pub choice: Choice,
    pub mean: Option<u32>,
}

/// A pack the store offers: every season, one season (its first year), one nation ([`NATIONS`]' index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackChoice {
    Standard,
    Season(u16),
    Nation(usize),
}

impl PackChoice {
    pub fn pack(self) -> Pack<'static> {
        match self {
            PackChoice::Standard => Pack::Standard,
            PackChoice::Season(y) => Pack::Season(y),
            PackChoice::Nation(i) => Pack::Nation(NATIONS[i]),
        }
    }

    /// 0, 1 or 2: the kind's tile.
    fn kind(self) -> usize {
        match self {
            PackChoice::Standard => 0,
            PackChoice::Season(_) => 1,
            PackChoice::Nation(_) => 2,
        }
    }

    /// The pick's own name: a season as the cards print it, a nation as the catalogue spells it.
    fn label(self, lang: Lang) -> String {
        match self {
            PackChoice::Standard => tr!(lang, "collection.pack_standard"),
            PackChoice::Season(y) => season(y),
            PackChoice::Nation(i) => NATIONS[i].to_string(),
        }
    }
}

/// One pack on offer: its pool's size and one card's odds per tier after the step-down
/// (`forza::collection::pack_odds`, tenths of a percent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub pack: PackChoice,
    pub cards: u32,
    pub odds: [u32; 5],
}

/// One opened card: the card and how many copies of it the player holds now.
#[derive(Clone, Debug)]
pub struct Opened {
    pub card: Card,
    pub copies: u32,
}

/// Where the tab is.
#[derive(Clone, Debug)]
pub enum Step {
    /// No starter yet: the starts, the one chosen.
    Choose {
        starts: Vec<StartRow>,
        chosen: Option<usize>,
    },
    /// The deal: the 11 and the 3 RARE cards, the one picked.
    Pick {
        choice: Choice,
        dealt: Vec<Card>,
        rares: Vec<Card>,
        picked: Option<usize>,
    },
    /// The collection page.
    Page,
    /// The pack store: the offer chosen ([`CollectionView::offers`]' index).
    Store { chosen: usize },
    /// A pack just bought: its cards turn over one by one from `since` (the frame time of the first frame), all at
    /// once when `all` (a click skips); `chosen` is the store's offer to go back to.
    Open {
        chosen: usize,
        cards: Vec<Opened>,
        since: Option<f64>,
        all: bool,
    },
}

/// The page's filters; `None` shows all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub season: Option<String>,
    pub nation: Option<String>,
    pub league: Option<usize>,
    pub rarity: Option<Rarity>,
}

#[derive(Clone, Debug)]
pub struct CollectionView {
    pub step: Step,
    /// The cards held (owned or listed), in catalogue order, with their copies.
    pub cards: Vec<(Card, Copies)>,
    pub tokens: i64,
    /// Match tokens still to earn this week.
    pub match_left: i64,
    pub filter: Filter,
    /// The card whose detail is open.
    pub open: Option<u32>,
    /// Why the tab cannot work (no catalogue, an unreadable log): shown instead.
    pub problem: Option<String>,
    /// The packs on offer: Standard, then each season, then the 24 nations.
    pub offers: Vec<Offer>,
}

/// What the player asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionAction {
    /// DEAL with the start chosen.
    Deal(Choice),
    /// TAKE THIS TEAM: the 11 and the RARE picked become the starter cards.
    Take,
    /// BUY the pack.
    Buy(PackChoice),
}

/// Which screen a sample shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Choose,
    Pick,
    Page,
    /// The page with a starter card's detail open.
    Detail,
    /// The store on a Nation pack, Korea Republic: its odds stepped down.
    Store,
    /// A pack opened, every card turned.
    Open,
}

impl CollectionView {
    pub fn sample(s: Scenario) -> Self {
        let cards = super::sample::cards();
        let starts = std::iter::once(StartRow {
            choice: Choice::AllSeasons,
            mean: None,
        })
        .chain((2001..2016).map(|y| StartRow {
            choice: Choice::Season(y),
            mean: Some(75 + u32::from(y - 2001) * 9 / 14),
        }))
        .chain((0..LEAGUES.len()).map(|i| StartRow {
            choice: Choice::League(i),
            mean: None,
        }))
        .collect();
        let held = |n: usize, c: Copies| (cards[n % cards.len()].clone(), c);
        let mut v = CollectionView {
            step: Step::Page,
            cards: vec![],
            tokens: 150,
            match_left: 1_000,
            filter: Filter::default(),
            open: None,
            problem: None,
            offers: sample_offers(),
        };
        match s {
            Scenario::Choose => {
                v.step = Step::Choose {
                    starts,
                    chosen: Some(0),
                };
            }
            Scenario::Pick => {
                let rares = cards
                    .iter()
                    .filter(|c| c.rarity == Rarity::Rare)
                    .take(3)
                    .cloned()
                    .collect();
                v.step = Step::Pick {
                    choice: Choice::AllSeasons,
                    dealt: cards
                        .iter()
                        .filter(|c| c.rarity != Rarity::Rare)
                        .take(11)
                        .cloned()
                        .collect(),
                    rares,
                    picked: Some(1),
                };
            }
            Scenario::Page | Scenario::Detail => {
                let c = |earned, starter, imported, listed| Copies {
                    earned,
                    starter,
                    imported,
                    listed,
                };
                v.cards = (0..12)
                    .map(|n| held(n, c(0, 1, 0, 0)))
                    .chain((12..15).map(|n| held(n, c(0, 0, 1, 0))))
                    .chain((15..21).map(|n| held(n, c(1 + (n as u32 % 2), 0, 0, 0))))
                    .collect();
                if s == Scenario::Detail {
                    v.open = Some(v.cards[0].0.number);
                }
            }
            Scenario::Store => {
                let korea = NATIONS.iter().position(|n| *n == "Korea Republic");
                v.step = Step::Store {
                    chosen: v
                        .offers
                        .iter()
                        .position(|o| Some(o.pack) == korea.map(PackChoice::Nation))
                        .unwrap_or(0),
                };
            }
            Scenario::Open => {
                v.step = Step::Open {
                    chosen: 0,
                    cards: [3, 30, 7]
                        .iter()
                        .enumerate()
                        .map(|(i, &n)| Opened {
                            card: cards[n % cards.len()].clone(),
                            copies: if i == 1 { 2 } else { 1 },
                        })
                        .collect(),
                    since: None,
                    all: true,
                };
            }
        }
        v
    }

    /// The tab has its starter cards: the page, the store or an opened pack.
    pub fn started(&self) -> bool {
        !matches!(self.step, Step::Choose { .. } | Step::Pick { .. })
    }

    /// Show the pack just bought: `cards`' numbers, each with the copies [`Self::cards`] holds of it now. The store's
    /// offer stays chosen for the way back.
    pub fn open_pack(&mut self, cards: &[u32]) {
        let chosen = match self.step {
            Step::Store { chosen } | Step::Open { chosen, .. } => chosen,
            _ => 0,
        };
        let opened = cards
            .iter()
            .filter_map(|no| self.cards.iter().find(|(c, _)| c.number == *no))
            .map(|(card, n)| Opened {
                card: card.clone(),
                copies: n.earned + n.starter + n.imported,
            })
            .collect();
        self.step = Step::Open {
            chosen,
            cards: opened,
            since: None,
            all: false,
        };
    }
}

/// The sample's offers: the catalogue's card counts per nation (`PLAN.md` 2.3) and the table's odds, Korea Republic
/// stepped down (no RARE, LEGEND or ALL TIME LEGEND).
fn sample_offers() -> Vec<Offer> {
    const COUNTS: [u32; 24] = [
        1_264, 988, 957, 647, 644, 531, 497, 384, 375, 251, 129, 127, 99, 94, 89, 74, 72, 65, 52,
        51, 47, 44, 40, 27,
    ];
    let full = collection::Rarity::ALL.map(collection::Rarity::odds);
    std::iter::once(Offer {
        pack: PackChoice::Standard,
        cards: 8_396,
        odds: full,
    })
    .chain((2001..2016).map(|y| Offer {
        pack: PackChoice::Season(y),
        cards: 560,
        odds: full,
    }))
    .chain(COUNTS.iter().enumerate().map(|(i, &cards)| Offer {
        pack: PackChoice::Nation(i),
        cards,
        odds: if NATIONS[i] == "Korea Republic" {
            [680, 320, 0, 0, 0]
        } else {
            full
        },
    }))
    .collect()
}

/// The tab; what the player asked for, if anything.
pub fn show(ui: &mut Ui, v: &mut CollectionView, lang: Lang) -> Option<CollectionAction> {
    if let Some(p) = &v.problem {
        widgets::status_line(ui, widgets::Tone::Refused, p);
        return None;
    }
    let mut act = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| match &mut v.step {
            Step::Choose { starts, chosen } => choose(ui, starts, chosen, lang, &mut act),
            Step::Pick {
                dealt,
                rares,
                picked,
                ..
            } => pick(ui, dealt, rares, picked, lang, &mut act),
            Step::Page => page(ui, v, lang),
            Step::Store { chosen } => {
                if store(ui, &v.offers, chosen, v.tokens, lang, &mut act) {
                    v.step = Step::Page;
                }
            }
            Step::Open { .. } => open(ui, &mut v.step, lang),
        });
    act
}

/// A choice tile: a title and a line under it; amber when chosen.
fn tile(ui: &mut Ui, size: egui::Vec2, title: &str, sub: &str, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, title));
    let p = ui.painter();
    let (fill, rim, ink) = if on {
        (theme::AMBER, theme::AMBER, theme::INK)
    } else if resp.hovered() {
        (theme::RAISED_HI, theme::RIM_HI, theme::TEXT)
    } else {
        (theme::RAISED, theme::RIM, theme::TEXT)
    };
    p.rect_filled(rect, theme::RADIUS, fill);
    p.rect_stroke(
        rect,
        theme::RADIUS,
        egui::Stroke::new(1.0, rim),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(vec2(10.0, 6.0));
    let t = widgets::fit(p, title, theme::bold(14.0), ink, inner.width());
    let s = widgets::fit(
        p,
        sub,
        theme::body(12.0),
        ink.gamma_multiply(0.8),
        inner.width(),
    );
    p.galley(inner.left_top(), t, ink);
    p.galley(inner.left_bottom() - vec2(0.0, s.size().y), s, ink);
    resp
}

/// Tiles in rows of `cols`, the full width.
fn tiles(
    ui: &mut Ui,
    n: usize,
    cols: usize,
    h: f32,
    mut one: impl FnMut(&mut Ui, usize, egui::Vec2),
) {
    let gap = ui.spacing().item_spacing.x;
    let w = ((ui.available_width() - gap * (cols - 1) as f32) / cols as f32).floor();
    for row in (0..n).collect::<Vec<_>>().chunks(cols) {
        ui.horizontal(|ui| {
            for &i in row {
                one(ui, i, vec2(w, h));
            }
        });
    }
}

fn choose(
    ui: &mut Ui,
    starts: &[StartRow],
    chosen: &mut Option<usize>,
    lang: Lang,
    act: &mut Option<CollectionAction>,
) {
    title(ui, &tr!(lang, "collection.choose_title"));
    hint(ui, &tr!(lang, "collection.choose_hint"));
    let mut group = |ui: &mut Ui, head: Option<&str>, rows: Vec<usize>, cols: usize| {
        if let Some(head) = head {
            widgets::caps(ui, head);
        }
        tiles(ui, rows.len(), cols, 46.0, |ui, k, size| {
            let i = rows[k];
            let row = &starts[i];
            let sub = match row.mean {
                Some(m) => tr!(lang, "collection.mean", n = m.to_string().as_str()),
                // the leagues and All seasons are banded: 80-82 is what the deal aims at, not a result
                None => tr!(lang, "collection.band"),
            };
            let sub = if row.choice == Choice::AllSeasons {
                tr!(lang, "collection.recommended", mean = sub.as_str())
            } else {
                sub
            };
            if tile(ui, size, &row.choice.label(lang), &sub, *chosen == Some(i)).clicked() {
                *chosen = Some(i);
            }
        });
    };
    let of = |f: fn(&Choice) -> bool| -> Vec<usize> {
        (0..starts.len())
            .filter(|&i| f(&starts[i].choice))
            .collect()
    };
    group(ui, None, of(|c| *c == Choice::AllSeasons), 1);
    group(
        ui,
        Some(&tr!(lang, "collection.by_season")),
        of(|c| matches!(c, Choice::Season(_))),
        3,
    );
    group(
        ui,
        Some(&tr!(lang, "collection.by_league")),
        of(|c| matches!(c, Choice::League(_))),
        3,
    );
    ui.add_space(4.0);
    let w = ui.available_width();
    let on = chosen.is_some();
    if ui
        .add_enabled_ui(on, |ui| {
            widgets::cabinet(
                ui,
                vec2(w, 48.0),
                &tr!(lang, "collection.deal"),
                Cab::Go,
                on,
            )
        })
        .inner
        .clicked()
        && let Some(i) = *chosen
    {
        *act = Some(CollectionAction::Deal(starts[i].choice));
    }
}

fn pick(
    ui: &mut Ui,
    dealt: &[Card],
    rares: &[Card],
    picked: &mut Option<usize>,
    lang: Lang,
    act: &mut Option<CollectionAction>,
) {
    title(ui, &tr!(lang, "collection.dealt_title"));
    let gap = 4.0;
    let w = ((ui.available_width() - gap * 10.0) / 11.0).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for c in dealt {
            let (r, resp) = ui.allocate_exact_size(card_size(w), Sense::hover());
            resp.widget_info(|| WidgetInfo::labeled(Role::Image, true, &c.name));
            board::card_face(ui.painter(), r, c, Look::default());
        }
    });
    title(ui, &tr!(lang, "collection.pick_title"));
    hint(ui, &tr!(lang, "collection.pick_hint"));
    ui.horizontal(|ui| {
        let w = 130.0;
        let gap = ((ui.available_width() - w * 3.0) / 2.0).floor();
        ui.spacing_mut().item_spacing.x = gap;
        for (i, c) in rares.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(card_size(w), Sense::click());
            resp.widget_info(|| {
                WidgetInfo::selected(Role::RadioButton, true, *picked == Some(i), &c.name)
            });
            let look = Look {
                selected: *picked == Some(i),
                lifted: resp.hovered(),
                ..Look::default()
            };
            board::card_face(ui.painter(), r, c, look);
            if resp.clicked() {
                *picked = Some(i);
            }
        }
    });
    ui.add_space(4.0);
    let w = ui.available_width();
    let on = picked.is_some();
    if ui
        .add_enabled_ui(on, |ui| {
            widgets::cabinet(
                ui,
                vec2(w, 48.0),
                &tr!(lang, "collection.take"),
                Cab::Go,
                on,
            )
        })
        .inner
        .clicked()
        && on
    {
        *act = Some(CollectionAction::Take);
    }
}

/// A card's size at width `w`, the real card's shape.
fn card_size(w: f32) -> egui::Vec2 {
    vec2(w, (w / board::CARD_ASPECT).round())
}

/// The cards a filter keeps, as indexes into `cards`.
pub fn shown(cards: &[(Card, Copies)], f: &Filter) -> Vec<usize> {
    (0..cards.len())
        .filter(|&i| {
            let c = &cards[i].0;
            f.season.as_ref().is_none_or(|s| *s == c.season)
                && f.nation.as_ref().is_none_or(|n| *n == c.country)
                && f.league
                    .is_none_or(|l| collection::league_of(&c.club) == Some(LEAGUES[l].0))
                && f.rarity.is_none_or(|r| r == c.rarity)
        })
        .collect()
}

/// The rarity's filter name.
fn rarity_key(r: Rarity) -> &'static str {
    match r {
        Rarity::AllTime => "cat.all_time",
        Rarity::Legend => "cat.legend",
        Rarity::Rare => "cat.rare",
        Rarity::Special => "cat.special",
        Rarity::Regular => "cat.regular",
    }
}

/// The page's card width: 6 to a row in the Launcher's window.
pub const COLUMNS: usize = 6;

fn page(ui: &mut Ui, v: &mut CollectionView, lang: Lang) {
    ui.horizontal_wrapped(|ui| {
        let n = v.tokens.to_string();
        ui.label(
            egui::RichText::new(tr!(lang, "collection.tokens", n = n.as_str()))
                .font(theme::display(18.0))
                .color(theme::AMBER),
        );
        ui.add_space(12.0);
        let n = v.match_left.max(0).to_string();
        hint(ui, &tr!(lang, "collection.match_left", n = n.as_str()));
    });
    if !v.offers.is_empty() && widgets::action(ui, &tr!(lang, "collection.packs")).clicked() {
        v.step = Step::Store { chosen: 0 };
    }
    filters(ui, v, lang);
    if let Some(no) = v.open
        && let Some((card, copies)) = v.cards.iter().find(|(c, _)| c.number == no)
        && detail(ui, card, copies, lang)
    {
        v.open = None;
    }
    let list = shown(&v.cards, &v.filter);
    let gap = ui.spacing().item_spacing.x;
    let w = ((ui.available_width() - gap * (COLUMNS - 1) as f32) / COLUMNS as f32).floor();
    for row in list.chunks(COLUMNS) {
        ui.horizontal(|ui| {
            for &i in row {
                let (card, copies) = &v.cards[i];
                let mark = card_mark(copies).map(|k| tr!(lang, k));
                let resp = board::catalogue_card(
                    ui,
                    card,
                    card_size(w),
                    v.open == Some(card.number),
                    false,
                    mark.as_deref(),
                );
                resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &card.name));
                let n = copies.earned + copies.starter + copies.imported;
                if n > 1 {
                    let r = resp.rect;
                    let at =
                        Rect::from_min_size(r.right_top() + vec2(-26.0, 4.0), vec2(22.0, 16.0));
                    ui.painter()
                        .rect_filled(at, 3, theme::NIGHT.gamma_multiply(0.85));
                    ui.painter().text(
                        at.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("x{n}"),
                        theme::bold(12.0),
                        theme::TEXT,
                    );
                }
                if resp.clicked() {
                    v.open = Some(card.number);
                }
            }
        });
    }
}

/// A card's mark key: STARTER or IMPORT when no copy held can be sold (`crate::panel::card_mark`'s rule).
fn card_mark(c: &Copies) -> Option<&'static str> {
    let one = std::collections::BTreeMap::from([(0, *c)]);
    crate::panel::card_mark(Some(&one), 0)
}

fn filters(ui: &mut Ui, v: &mut CollectionView, lang: Lang) {
    let all = tr!(lang, "cat.all");
    let mut seasons: Vec<String> = v.cards.iter().map(|(c, _)| c.season.clone()).collect();
    seasons.sort();
    seasons.dedup();
    let mut nations: Vec<String> = v.cards.iter().map(|(c, _)| c.country.clone()).collect();
    nations.sort();
    nations.dedup();
    let f = &mut v.filter;
    ui.horizontal_wrapped(|ui| {
        let combo = |ui: &mut Ui, id: &str, label: String, w: f32, add: &mut dyn FnMut(&mut Ui)| {
            egui::ComboBox::from_id_salt(id)
                .selected_text(&label)
                .width(w)
                .show_ui(ui, add)
                .response
                .widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, &label));
        };
        let w = ((ui.available_width() - ui.spacing().item_spacing.x * 3.0) / 4.0).floor() - 24.0;
        let label = f
            .season
            .clone()
            .unwrap_or_else(|| tr!(lang, "collection.season"));
        combo(ui, "coll-season", label, w, &mut |ui| {
            ui.selectable_value(&mut f.season, None, &all);
            for s in &seasons {
                ui.selectable_value(&mut f.season, Some(s.clone()), s);
            }
        });
        let label = f.nation.clone().unwrap_or_else(|| tr!(lang, "cat.country"));
        combo(ui, "coll-nation", label, w, &mut |ui| {
            ui.selectable_value(&mut f.nation, None, &all);
            for n in &nations {
                ui.selectable_value(&mut f.nation, Some(n.clone()), n);
            }
        });
        let label = f.league.map_or_else(
            || tr!(lang, "collection.league"),
            |l| tr!(lang, LEAGUE_KEYS[l]),
        );
        combo(ui, "coll-league", label, w, &mut |ui| {
            ui.selectable_value(&mut f.league, None, &all);
            for (l, key) in LEAGUE_KEYS.iter().enumerate() {
                ui.selectable_value(&mut f.league, Some(l), tr!(lang, key));
            }
        });
        let label = f
            .rarity
            .map_or_else(|| tr!(lang, "cat.rarity"), |r| tr!(lang, rarity_key(r)));
        combo(ui, "coll-rarity", label, w, &mut |ui| {
            ui.selectable_value(&mut f.rarity, None, &all);
            for r in [
                Rarity::Regular,
                Rarity::Special,
                Rarity::Rare,
                Rarity::Legend,
                Rarity::AllTime,
            ] {
                ui.selectable_value(&mut f.rarity, Some(r), tr!(lang, rarity_key(r)));
            }
        });
    });
}

/// A card's detail: its value and how it is made, its copies by source and its mark. No CONVERT and no SELL here:
/// CONVERT comes with step 2.3 (spare earned copies only) and selling with the market (4.1). `true`: CLOSE pressed.
fn detail(ui: &mut Ui, card: &Card, copies: &Copies, lang: Lang) -> bool {
    let row = &forza::cards::collection_cards(std::slice::from_ref(card))[0];
    let mut close = false;
    egui::Frame::new()
        .fill(theme::DEEP)
        .stroke(egui::Stroke::new(1.0, theme::RIM))
        .corner_radius(theme::RADIUS_BIG)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(card_size(84.0), Sense::hover());
                board::card_face(ui.painter(), r, card, Look::default());
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&card.name)
                            .font(theme::bold(16.0))
                            .color(theme::TEXT),
                    );
                    hint(
                        ui,
                        &format!("{} · {} · {}", card.season, card.club, card.country),
                    );
                    let value = collection::value(row).to_string();
                    ui.label(
                        egui::RichText::new(tr!(lang, "collection.value", n = value.as_str()))
                            .font(theme::bold(14.0))
                            .color(theme::AMBER),
                    );
                    if row.secret {
                        hint(ui, &tr!(lang, "collection.value_secret"));
                    } else {
                        // a multiplier, printed with the language's decimal mark (it: 1,5)
                        let pct = |p: u32| {
                            let s = format!("{}", p as f32 / 100.0);
                            if lang == Lang::It {
                                s.replace('.', ",")
                            } else {
                                s
                            }
                        };
                        let (base, step, factor) = (
                            row.rarity.base().to_string(),
                            pct(collection::step(row.total)),
                            pct(collection::factor(row.rarity, row.series)),
                        );
                        hint(
                            ui,
                            &tr!(
                                lang,
                                "collection.value_parts",
                                base = base.as_str(),
                                step = step.as_str(),
                                factor = factor.as_str()
                            ),
                        );
                    }
                    let n = |x: u32| x.to_string();
                    let (e, s, i, l) = (
                        n(copies.earned),
                        n(copies.starter),
                        n(copies.imported),
                        n(copies.listed),
                    );
                    hint(
                        ui,
                        &tr!(
                            lang,
                            "collection.copies",
                            earned = e.as_str(),
                            starter = s.as_str(),
                            imported = i.as_str(),
                            listed = l.as_str()
                        ),
                    );
                    if card_mark(copies).is_some() {
                        hint(ui, &tr!(lang, "collection.no_sell"));
                    }
                    if widgets::action(ui, &tr!(lang, "collection.close")).clicked() {
                        close = true;
                    }
                });
            });
        });
    close
}

/// A tier's chance in tenths of a percent as the store prints it: 680 -> "68 %", 2 -> "0.2 %" (it: "0,2 %").
fn chance(tenths: u32, lang: Lang) -> String {
    let s = if tenths.is_multiple_of(10) {
        (tenths / 10).to_string()
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    };
    let s = if lang == Lang::It {
        s.replace('.', ",")
    } else {
        s
    };
    format!("{s} %")
}

/// The pack store: the three kinds, the chosen pack's odds and BUY, then the season or nation picker (BUY stays in
/// view while the 24 nations scroll). `true`: back to the page.
fn store(
    ui: &mut Ui,
    offers: &[Offer],
    chosen: &mut usize,
    tokens: i64,
    lang: Lang,
    act: &mut Option<CollectionAction>,
) -> bool {
    let Some(offer) = offers.get(*chosen).cloned() else {
        *chosen = 0;
        return false;
    };
    let mut back = false;
    ui.horizontal(|ui| {
        back = widgets::action(ui, &tr!(lang, "collection.to_page")).clicked();
        ui.add_space(8.0);
        title(ui, &tr!(lang, "collection.store_title"));
        ui.add_space(12.0);
        let n = tokens.to_string();
        ui.label(
            egui::RichText::new(tr!(lang, "collection.tokens", n = n.as_str()))
                .font(theme::display(18.0))
                .color(theme::AMBER),
        );
    });
    hint(ui, &tr!(lang, "collection.store_hint"));
    const KINDS: [(PackChoice, &str, &str); 3] = [
        (
            PackChoice::Standard,
            "collection.pack_standard",
            "collection.pack_standard_hint",
        ),
        (
            PackChoice::Season(0),
            "collection.pack_season",
            "collection.pack_season_hint",
        ),
        (
            PackChoice::Nation(0),
            "collection.pack_nation",
            "collection.pack_nation_hint",
        ),
    ];
    tiles(ui, KINDS.len(), 3, 46.0, |ui, k, size| {
        let (kind, name, sub) = KINDS[k];
        let price = kind.pack().price().to_string();
        let sub = tr!(lang, sub, n = price.as_str());
        if tile(ui, size, &tr!(lang, name), &sub, offer.pack.kind() == k).clicked()
            && offer.pack.kind() != k
            && let Some(i) = offers.iter().position(|o| o.pack.kind() == kind.kind())
        {
            *chosen = i;
        }
    });
    ui.add_space(4.0);
    widgets::caps(
        ui,
        &tr!(
            lang,
            "collection.odds_title",
            pack = offer.pack.label(lang).as_str()
        ),
    );
    let tiers = [
        Rarity::Regular,
        Rarity::Special,
        Rarity::Rare,
        Rarity::Legend,
        Rarity::AllTime,
    ];
    ui.horizontal_wrapped(|ui| {
        for (r, &o) in tiers.iter().zip(&offer.odds) {
            let text = format!(
                "{} {}",
                tr!(lang, rarity_key(*r)),
                if o == 0 {
                    "–".into()
                } else {
                    chance(o, lang)
                }
            );
            ui.label(
                egui::RichText::new(text)
                    .font(theme::bold(13.0))
                    .color(if o == 0 { theme::TEXT_DIM } else { theme::TEXT }),
            );
            ui.add_space(8.0);
        }
    });
    hint(ui, &tr!(lang, "collection.odds_guarantee"));
    if offer.odds.contains(&0) {
        hint(ui, &tr!(lang, "collection.odds_step_down"));
    }
    ui.add_space(4.0);
    let price = offer.pack.pack().price();
    let can = tokens >= price && offer.cards as usize >= collection::PACK_CARDS;
    let n = price.to_string();
    let w = ui.available_width();
    if ui
        .add_enabled_ui(can, |ui| {
            widgets::cabinet(
                ui,
                vec2(w, 48.0),
                &tr!(lang, "collection.buy", n = n.as_str()),
                Cab::Go,
                can,
            )
        })
        .inner
        .clicked()
        && can
    {
        *act = Some(CollectionAction::Buy(offer.pack));
    }
    if tokens < price {
        hint(ui, &tr!(lang, "collection.buy_short", n = n.as_str()));
    }
    let picks: Vec<usize> = (0..offers.len())
        .filter(|&i| {
            offer.pack != PackChoice::Standard && offers[i].pack.kind() == offer.pack.kind()
        })
        .collect();
    if !picks.is_empty() {
        let head = if offer.pack.kind() == 1 {
            "collection.by_season"
        } else {
            "collection.pick_nation"
        };
        widgets::caps(ui, &tr!(lang, head));
        tiles(ui, picks.len(), 4, 46.0, |ui, k, size| {
            let i = picks[k];
            let n = offers[i].cards.to_string();
            let sub = tr!(lang, "collection.cards_n", n = n.as_str());
            if tile(ui, size, &offers[i].pack.label(lang), &sub, *chosen == i).clicked() {
                *chosen = i;
            }
        });
    }
    back
}

/// Seconds between two cards turning over, and the turn itself.
const OPEN_GAP: f64 = 0.4;
const OPEN_TURN: f64 = 0.25;

/// A bought pack: its cards turn over one by one (about a second for the three; a click shows them all), each with
/// NEW or the copies held now.
fn open(ui: &mut Ui, step: &mut Step, lang: Lang) {
    let Step::Open {
        chosen,
        cards,
        since,
        all,
    } = step
    else {
        return;
    };
    let now = ui.input(|i| i.time);
    let t = now - *since.get_or_insert(now);
    // 0 before the turn, 1 after; the card at `i` starts at OPEN_GAP * (i + 1)
    let turned = |i: usize| -> f32 {
        if *all {
            1.0
        } else {
            ((t - OPEN_GAP * (i + 1) as f64) / OPEN_TURN).clamp(0.0, 1.0) as f32
        }
    };
    let done = (0..cards.len()).all(|i| turned(i) >= 1.0);
    title(ui, &tr!(lang, "collection.open_title"));
    let w = 150.0;
    let mut skip = false;
    ui.horizontal(|ui| {
        let gap =
            ((ui.available_width() - w * cards.len() as f32) / (cards.len() + 1) as f32).floor();
        ui.add_space(gap);
        ui.spacing_mut().item_spacing.x = gap;
        for (i, o) in cards.iter().enumerate() {
            ui.vertical(|ui| {
                let (r, resp) = ui.allocate_exact_size(card_size(w), Sense::click());
                resp.widget_info(|| WidgetInfo::labeled(Role::Image, true, &o.card.name));
                skip |= resp.clicked();
                let p = turned(i);
                // a turn: the back narrows to nothing, then the face widens back
                let squeeze = (2.0 * p - 1.0).abs();
                let shown =
                    Rect::from_center_size(r.center(), vec2(r.width() * squeeze, r.height()));
                if p < 0.5 {
                    ui.painter()
                        .rect_filled(shown, theme::RADIUS, theme::RAISED);
                    ui.painter().rect_stroke(
                        shown,
                        theme::RADIUS,
                        egui::Stroke::new(2.0, theme::AMBER),
                        egui::StrokeKind::Inside,
                    );
                } else {
                    board::card_face(ui.painter(), shown, &o.card, Look::default());
                }
                let badge = if p < 1.0 {
                    String::new()
                } else if o.copies <= 1 {
                    tr!(lang, "collection.new_card")
                } else {
                    let n = o.copies.to_string();
                    tr!(lang, "collection.copy_n", n = n.as_str())
                };
                ui.label(egui::RichText::new(badge).font(theme::bold(14.0)).color(
                    if o.copies <= 1 {
                        theme::AMBER
                    } else {
                        theme::TEXT
                    },
                ));
            });
        }
    });
    ui.add_space(8.0);
    if !done {
        skip |= widgets::action(ui, &tr!(lang, "collection.show_all")).clicked();
        ui.ctx().request_repaint();
    } else if widgets::action(ui, &tr!(lang, "collection.to_store")).clicked() {
        *step = Step::Store { chosen: *chosen };
        return;
    }
    if skip {
        *all = true;
    }
}

fn title(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(theme::display(18.0))
            .color(theme::TEXT),
    );
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(theme::body(13.0))
            .color(theme::TEXT_DIM),
    );
}
