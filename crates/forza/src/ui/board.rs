//! Player cards and the FORMATION BOARD: the card face (81:58, upright), the rarity marks, and the pitch with its three
//! zones where cards are dragged, swapped, taken off and shown big on a resting mouse.

use super::Lang;
use super::theme;
use super::widgets::{centered_text, fan, fit, gradient};
use egui::{
    Align2, Color32, Id, Order, Painter, Pos2, Rect, Response, Sense, Shadow, Stroke, StrokeKind,
    Ui, pos2, vec2,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Gk,
    Df,
    Mf,
    Fw,
}

impl Position {
    pub const ALL: [Position; 4] = [Position::Gk, Position::Df, Position::Mf, Position::Fw];

    pub fn code(self) -> &'static str {
        match self {
            Position::Gk => "GK",
            Position::Df => "DF",
            Position::Mf => "MF",
            Position::Fw => "FW",
        }
    }

    pub fn color(self) -> Color32 {
        match self {
            Position::Gk => theme::POS_GK,
            Position::Df => theme::POS_DF,
            Position::Mf => theme::POS_MF,
            Position::Fw => theme::POS_FW,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rarity {
    AllTime,
    Legend,
    Rare,
    Special,
    Regular,
}

impl Rarity {
    pub const ALL: [Rarity; 5] = [
        Rarity::AllTime,
        Rarity::Legend,
        Rarity::Rare,
        Rarity::Special,
        Rarity::Regular,
    ];

    /// The rim of the card frame.
    pub fn rim(self) -> Color32 {
        match self {
            Rarity::AllTime => Color32::from_rgb(0xB9, 0x9C, 0xFF),
            Rarity::Legend => theme::GOLD,
            Rarity::Rare => Color32::from_rgb(0xC6, 0xD2, 0xE2),
            Rarity::Special => theme::CYAN,
            Rarity::Regular => theme::RIM_HI,
        }
    }
}

/// A player card: what the catalogue lists and the board holds. Stats in card-back order: OFF DEF TEC POW SPD STA.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub number: u16,
    pub name: String,
    pub club: String,
    pub country: String,
    pub season: String,
    pub pos: Position,
    pub rarity: Rarity,
    pub stats: [u8; 6],
}

impl Card {
    pub fn total(&self) -> u8 {
        // TOTAL on a 60-100 scale from the six 1-20 stats
        (60 + self.stats.iter().map(|&s| s as u32).sum::<u32>() * 40 / 120) as u8
    }

    fn short_name(&self) -> &str {
        self.name.rsplit(' ').next().unwrap_or(&self.name)
    }
}

/// Paints a rarity mark: ALL-TIME a ringed star, LEGEND a star, RARE a diamond, SPECIAL a hexagon, REGULAR nothing.
pub fn rarity_mark(p: &Painter, c: Pos2, size: f32, rarity: Rarity) {
    let col = rarity.rim();
    let ring = |n: usize, r_out: f32, r_in: f32, rot: f32| -> Vec<Pos2> {
        (0..n)
            .map(|i| {
                let a = rot + i as f32 * std::f32::consts::TAU / n as f32;
                let r = if i % 2 == 0 { r_out } else { r_in };
                c + vec2(a.cos(), a.sin()) * r
            })
            .collect()
    };
    let up = -std::f32::consts::FRAC_PI_2;
    match rarity {
        Rarity::AllTime => {
            p.circle_stroke(c, size * 0.62, Stroke::new(1.5, col));
            fan(p, c, &ring(10, size * 0.48, size * 0.2, up), Color32::WHITE);
        }
        Rarity::Legend => fan(p, c, &ring(10, size * 0.55, size * 0.23, up), col),
        Rarity::Rare => fan(p, c, &ring(4, size * 0.5, size * 0.32, up), col),
        Rarity::Special => fan(p, c, &ring(6, size * 0.42, size * 0.42, 0.0), col),
        Rarity::Regular => {}
    }
}

/// How a card is drawn: at rest, lifted (dragged), a ghost left in its slot, a swap target, out of position.
#[derive(Clone, Copy, Debug, Default)]
pub struct Look {
    pub lifted: bool,
    pub ghost: bool,
    pub target: bool,
    pub warn: bool,
    pub selected: bool,
}

/// The card face, upright 58:81. Small sizes show the position band, the name and TOTAL; large ones add the club,
/// the initial as a monogram and the season. No pictures: a neutral frame and text.
pub fn card_face(p: &Painter, rect: Rect, card: &Card, look: Look) {
    let big = rect.width() >= 90.0;
    let r = (rect.width() * 0.08) as u8;
    let alpha = if look.ghost { 0.3 } else { 1.0 };
    if look.lifted {
        let s = Shadow {
            offset: [0, 10],
            blur: 22,
            spread: 0,
            color: Color32::from_black_alpha(170),
        };
        p.add(s.as_shape(rect, r));
    }
    if look.target {
        let s = Shadow {
            offset: [0, 0],
            blur: 22,
            spread: 6,
            color: theme::AMBER.gamma_multiply(0.8),
        };
        p.add(s.as_shape(rect, r));
    }
    let rim = card.rarity.rim();
    gradient(
        p,
        rect,
        r,
        theme::RAISED_HI.gamma_multiply(alpha),
        theme::DEEP
            .lerp_to_gamma(theme::WELL, 0.5)
            .gamma_multiply(alpha),
        true,
    );
    // the position band
    let band_h = (rect.height() * 0.2).max(13.0);
    let band = Rect::from_min_size(rect.min, vec2(rect.width(), band_h));
    let pc = card.pos.color().gamma_multiply(alpha);
    p.rect_filled(
        band,
        egui::CornerRadius {
            nw: r,
            ne: r,
            sw: 0,
            se: 0,
        },
        pc,
    );
    let fs = band_h * 0.78;
    p.text(
        band.left_center() + vec2(4.0, 0.5),
        Align2::LEFT_CENTER,
        card.pos.code(),
        theme::display(fs),
        Color32::WHITE.gamma_multiply(alpha),
    );
    p.text(
        band.right_center() + vec2(-4.0, 0.5),
        Align2::RIGHT_CENTER,
        card.total().to_string(),
        theme::display(fs),
        Color32::WHITE.gamma_multiply(alpha),
    );

    let body = rect.with_min_y(band.bottom());
    if big {
        // a monogram where the original has a picture
        let initial: String = card.short_name().chars().take(1).collect();
        p.text(
            body.center() - vec2(0.0, body.height() * 0.14),
            Align2::CENTER_CENTER,
            initial,
            theme::display(body.height() * 0.5),
            rim.gamma_multiply(0.22 * alpha),
        );
        let name_y = body.bottom() - body.height() * 0.3;
        centered_text(
            p,
            Rect::from_center_size(
                pos2(body.center().x, name_y),
                vec2(rect.width() - 10.0, 20.0),
            ),
            &card.name,
            theme::display(rect.width() * 0.16),
            theme::TEXT.gamma_multiply(alpha),
        );
        centered_text(
            p,
            Rect::from_center_size(
                pos2(body.center().x, name_y + rect.width() * 0.15),
                vec2(rect.width() - 10.0, 14.0),
            ),
            &card.club,
            theme::body(rect.width() * 0.09),
            theme::TEXT_DIM.gamma_multiply(alpha),
        );
        p.text(
            body.left_bottom() + vec2(6.0, -4.0),
            Align2::LEFT_BOTTOM,
            &card.season,
            theme::display(rect.width() * 0.1),
            theme::TEXT_FAINT.gamma_multiply(alpha),
        );
        rarity_mark(
            p,
            body.right_bottom() + vec2(-11.0, -11.0),
            14.0,
            card.rarity,
        );
    } else {
        let name = card.short_name();
        let g = fit(
            p,
            name,
            theme::display(rect.width() * 0.3),
            theme::TEXT.gamma_multiply(alpha),
            rect.width() - 6.0,
        );
        p.galley(
            pos2(
                body.center().x - g.size().x / 2.0,
                body.bottom() - g.size().y - 3.0,
            ),
            g,
            theme::TEXT,
        );
        p.text(
            body.center() - vec2(0.0, body.height() * 0.16),
            Align2::CENTER_CENTER,
            card.number.to_string(),
            theme::display(rect.width() * 0.34),
            rim.gamma_multiply(0.5 * alpha),
        );
        rarity_mark(
            p,
            pos2(body.right() - 7.0, body.top() + 7.0),
            9.0,
            card.rarity,
        );
    }
    let rim_stroke = if look.target || look.selected {
        Stroke::new(if look.target { 3.0 } else { 2.0 }, theme::AMBER)
    } else {
        Stroke::new(
            if card.rarity == Rarity::Regular {
                1.0
            } else {
                1.5
            },
            rim.gamma_multiply(alpha),
        )
    };
    p.rect_stroke(rect, r, rim_stroke, StrokeKind::Inside);
    if look.warn && !look.ghost {
        // out of position: an amber corner with an exclamation
        let c = rect.left_bottom();
        let s = 14.0;
        fan(
            p,
            c + vec2(s * 0.33, -s * 0.33),
            &[c, c + vec2(0.0, -s), c + vec2(s, 0.0)],
            theme::AMBER,
        );
        p.text(
            c + vec2(3.5, -1.0),
            Align2::LEFT_BOTTOM,
            "!",
            theme::display(11.0),
            theme::INK,
        );
    }
}

// ---- the formation board ----

/// The card table: 16 slots, 0-10 on the pitch (0 the keeper), 11-15 on the bench; each holds a card index or nothing.
#[derive(Clone, Debug)]
pub struct Board {
    pub cards: Vec<Card>,
    pub slots: [Option<usize>; 16],
    /// Where each pitch slot sits, 0..1 across and 0 (attack) .. 1 (own goal) down.
    pub spots: [Pos2; 11],
}

/// The zone a pitch point is in: attack at the top, the centre, defence near the own goal, the keeper's box.
fn zone(y: f32) -> Position {
    match y {
        y if y < 0.34 => Position::Fw,
        y if y < 0.64 => Position::Mf,
        y if y < 0.88 => Position::Df,
        _ => Position::Gk,
    }
}

pub const CARD_SMALL: egui::Vec2 = egui::vec2(50.0, 70.0);

/// Draws the board and handles it: drag a card onto another to swap (the target is lit while dragging), right-click
/// takes a card off, a resting mouse shows the card big after `hover_delay` seconds. Returns true when it changed.
pub fn formation_board(ui: &mut Ui, board: &mut Board, hover_delay: f32, lang: Lang) -> bool {
    let width = ui.available_width();
    let pitch_h = width * 1.32;
    let (outer, _) = ui.allocate_exact_size(vec2(width, pitch_h), Sense::hover());
    let pitch = outer;
    let dragging = egui::DragAndDrop::payload::<usize>(ui.ctx()).map(|a| *a);
    let pointer = ui.ctx().pointer_latest_pos();
    let drag_zone = dragging
        .and(pointer)
        .filter(|p| pitch.contains(*p))
        .map(|p| zone((p.y - pitch.top()) / pitch.height()));
    paint_pitch(ui.painter(), pitch, drag_zone, lang);

    let mut changed = false;
    let mut rects = [Rect::NOTHING; 16];
    for (i, s) in board.spots.iter().enumerate() {
        rects[i] = Rect::from_center_size(
            pitch.min + vec2(s.x * pitch.width(), s.y * pitch.height()),
            CARD_SMALL,
        );
    }
    // the bench under the pitch
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(tr!(lang, "board.bench"))
            .font(theme::display(15.0))
            .color(theme::TEXT_DIM),
    );
    let (bench, _) = ui.allocate_exact_size(vec2(width, CARD_SMALL.y + 12.0), Sense::hover());
    ui.painter()
        .rect_filled(bench, theme::RADIUS_BIG, theme::WELL);
    let gap = (bench.width() - 5.0 * CARD_SMALL.x) / 6.0;
    for b in 0..5 {
        rects[11 + b] = Rect::from_min_size(
            bench.min + vec2(gap + b as f32 * (CARD_SMALL.x + gap), 6.0),
            CARD_SMALL,
        );
    }

    let hover_id = Id::new("board-hover");
    let mut hovered_card = None;
    for (slot, &rect) in rects.iter().enumerate() {
        let resp = ui.interact(rect, Id::new(("board-slot", slot)), Sense::click_and_drag());
        let p = ui.painter();
        match board.slots[slot] {
            Some(ci) => {
                resp.dnd_set_drag_payload(slot);
                let card = &board.cards[ci];
                let is_dragged = dragging == Some(slot);
                let target = dragging.is_some_and(|d| d != slot)
                    && resp.dnd_hover_payload::<usize>().is_some();
                let warn = slot < 11 && zone(board.spots[slot].y) != card.pos;
                card_face(
                    p,
                    rect,
                    card,
                    Look {
                        ghost: is_dragged,
                        target,
                        warn,
                        ..Look::default()
                    },
                );
                if resp.hovered() && dragging.is_none() {
                    hovered_card = Some((slot, ci, rect));
                }
                if is_dragged && let Some(pt) = pointer {
                    // the lifted card follows the pointer above everything
                    let lp = ui
                        .ctx()
                        .layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("board-lift")));
                    // up and to the left of the pointer, so the swap target under it stays in sight
                    let at = pt + vec2(-CARD_SMALL.x * 0.45, -CARD_SMALL.y * 0.55);
                    card_face(
                        &lp,
                        Rect::from_center_size(at, CARD_SMALL * 1.08),
                        card,
                        Look {
                            lifted: true,
                            ..Look::default()
                        },
                    );
                }
                if resp.secondary_clicked() {
                    board.slots[slot] = None;
                    changed = true;
                }
            }
            None => {
                let target = resp.dnd_hover_payload::<usize>().is_some();
                let col = if target {
                    theme::AMBER
                } else {
                    Color32::from_white_alpha(70)
                };
                let pts = [
                    rect.left_top(),
                    rect.right_top(),
                    rect.right_bottom(),
                    rect.left_bottom(),
                    rect.left_top(),
                ];
                p.extend(egui::Shape::dashed_line(
                    &pts,
                    Stroke::new(1.0, col),
                    4.0,
                    3.0,
                ));
                p.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "+",
                    theme::display(22.0),
                    col,
                );
            }
        }
        if let Some(from) = resp.dnd_release_payload::<usize>()
            && *from != slot
        {
            board.slots.swap(*from, slot);
            changed = true;
        }
    }

    // the big card after a resting mouse
    let now = ui.input(|i| i.time);
    match hovered_card {
        Some((slot, ci, rect)) => {
            let since = ui.data_mut(|d| {
                let e = d.get_temp_mut_or_insert_with(hover_id, || (slot, now));
                if e.0 != slot {
                    *e = (slot, now);
                }
                e.1
            });
            let left = hover_delay as f64 - (now - since);
            if left <= 0.0 {
                big_card(ui, &board.cards[ci], rect, ui.max_rect().left(), lang);
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(left));
            }
        }
        None => ui.data_mut(|d| d.remove::<(usize, f64)>(hover_id)),
    }
    changed
}

fn paint_pitch(p: &Painter, pitch: Rect, lit_zone: Option<Position>, lang: Lang) {
    // mowing stripes
    let n = 10;
    let h = pitch.height() / n as f32;
    p.rect_filled(pitch, theme::RADIUS_BIG, theme::PITCH);
    for i in (0..n).step_by(2) {
        let r = Rect::from_min_size(pitch.min + vec2(0.0, h * i as f32), vec2(pitch.width(), h));
        let cr = egui::CornerRadius {
            nw: if i == 0 { theme::RADIUS_BIG } else { 0 },
            ne: if i == 0 { theme::RADIUS_BIG } else { 0 },
            sw: 0,
            se: 0,
        };
        p.rect_filled(r, cr, theme::PITCH_STRIPE);
    }
    // the three zones: a tint and a name on the left edge
    let bands = [
        (Position::Fw, 0.0, 0.34, tr!(lang, "board.zone_attack")),
        (Position::Mf, 0.34, 0.64, tr!(lang, "board.zone_centre")),
        (Position::Df, 0.64, 1.0, tr!(lang, "board.zone_defence")),
    ];
    for (z, a, b, text) in bands {
        let r = Rect::from_x_y_ranges(
            pitch.x_range(),
            (pitch.top() + a * pitch.height())..=(pitch.top() + b * pitch.height()),
        );
        let lit = lit_zone == Some(z) || (lit_zone == Some(Position::Gk) && z == Position::Df);
        if lit {
            p.rect_filled(r, 0, Color32::from_white_alpha(22));
        }
        if a > 0.0 {
            let y = r.top();
            p.extend(egui::Shape::dashed_line(
                &[pos2(pitch.left(), y), pos2(pitch.right(), y)],
                Stroke::new(1.0, Color32::from_black_alpha(70)),
                6.0,
                6.0,
            ));
        }
        // the zone's name runs up the free margin left of the cards
        let col = Color32::from_white_alpha(if lit { 220 } else { 110 });
        let g = fit(p, &text, theme::display(13.0), col, r.height() - 16.0);
        let at = pos2(pitch.left() + 9.0, r.center().y + g.size().x / 2.0);
        p.add(egui::epaint::TextShape::new(at, g, col).with_angle(-std::f32::consts::FRAC_PI_2));
    }
    // markings, attacking upwards
    let line = Stroke::new(1.5, theme::PITCH_LINE.gamma_multiply(0.75));
    let f = pitch.shrink(8.0);
    p.rect_stroke(f, 2, line, StrokeKind::Middle);
    p.line_segment([f.left_center(), f.right_center()], line);
    p.circle_stroke(f.center(), f.width() * 0.14, line);
    p.circle_filled(f.center(), 2.5, line.color);
    for (y, dir) in [(f.top(), 1.0), (f.bottom(), -1.0)] {
        let bw = f.width() * 0.56;
        let bh = f.height() * 0.13 * dir;
        let gw = f.width() * 0.26;
        let gh = f.height() * 0.05 * dir;
        let box_ = Rect::from_two_pos(
            pos2(f.center().x - bw / 2.0, y),
            pos2(f.center().x + bw / 2.0, y + bh),
        );
        let goal = Rect::from_two_pos(
            pos2(f.center().x - gw / 2.0, y),
            pos2(f.center().x + gw / 2.0, y + gh),
        );
        p.rect_stroke(box_, 0, line, StrokeKind::Middle);
        p.rect_stroke(goal, 0, line, StrokeKind::Middle);
    }
}

/// The big card beside the board with its caption: a card at 2.6x and its line of facts.
fn big_card(ui: &Ui, card: &Card, near: Rect, board_left: f32, lang: Lang) {
    let size = CARD_SMALL * 2.6;
    let pos = pos2(
        board_left - 16.0,
        (near.center().y - size.y / 2.0).max(16.0),
    );
    egui::Area::new(Id::new("board-big"))
        .order(Order::Tooltip)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(pos)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::DEEP)
                .stroke(Stroke::new(1.0, theme::RIM_HI))
                .corner_radius(theme::RADIUS_BIG)
                .inner_margin(10)
                .shadow(Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(160),
                })
                .show(ui, |ui| {
                    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                    card_face(ui.painter(), r, card, Look::default());
                    ui.set_max_width(size.x);
                    ui.label(
                        egui::RichText::new(&card.name)
                            .font(theme::display(20.0))
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
                    let stats = [
                        "cat.off", "cat.def", "cat.tec", "cat.pow", "cat.spd", "cat.sta",
                    ];
                    egui::Grid::new("big-stats")
                        .num_columns(6)
                        .spacing(vec2(6.0, 0.0))
                        .show(ui, |ui| {
                            for k in stats {
                                ui.label(
                                    egui::RichText::new(tr!(lang, k))
                                        .font(theme::display(12.0))
                                        .color(theme::TEXT_DIM),
                                );
                            }
                            ui.end_row();
                            for v in card.stats {
                                ui.label(
                                    egui::RichText::new(v.to_string())
                                        .font(theme::display(18.0))
                                        .color(super::widgets::stat_color(v)),
                                );
                            }
                        });
                });
        });
}

/// A card in the catalogue's results: lifts a little under the mouse; `on_table` dims it with a tick.
pub fn catalogue_card(
    ui: &mut Ui,
    card: &Card,
    size: egui::Vec2,
    selected: bool,
    on_table: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let lift = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let r = rect.translate(vec2(0.0, -3.0 * lift));
    card_face(
        ui.painter(),
        r,
        card,
        Look {
            lifted: lift > 0.5,
            selected,
            ..Look::default()
        },
    );
    if on_table {
        // already on the table: dimmed, with a tick in the corner
        ui.painter()
            .rect_filled(r, 6, theme::WELL.gamma_multiply(0.55));
        super::widgets::tone_icon(
            ui.painter(),
            r.left_top() + vec2(12.0, 34.0),
            super::widgets::Tone::Done,
        );
    }
    resp
}
