//! Create-a-Team's COLLECTION tab in the Launcher (2017-18, `PLAN.md` 1.4): the first start (choose a start, see the
//! 11 dealt, pick 1 of 3 RARE cards) and then the collection page (the cards held with their copies and marks, the
//! token balance, the match tokens left this week). [`show`] draws a [`CollectionView`] and returns what the player
//! asked for; Launcher.exe deals the cards and writes the log (`forza::collection`).

use super::Lang;
use super::board::{self, Card, Look, Rarity};
use super::theme;
use super::widgets::{self, Cab};
use egui::{Rect, Role, Sense, Ui, WidgetInfo, vec2};
use forza::collection::{self, Copies, LEAGUES, Start};

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
}

/// What the player asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionAction {
    /// DEAL with the start chosen.
    Deal(Choice),
    /// TAKE THIS TEAM: the 11 and the RARE picked become the starter cards.
    Take,
}

/// Which screen a sample shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Choose,
    Pick,
    Page,
    /// The page with a starter card's detail open.
    Detail,
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
        }
        v
    }
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
