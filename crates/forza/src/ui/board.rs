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
    pub aptitude: Aptitude,
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

// ---- suitable positions ----

/// The 16 roles of a card's position ratings, in the order the original files keep them. The names are inferred by
/// the project from the players rated 10 in each, not the game's own.
pub const ROLES: [&str; 16] = [
    "CF", "TM", "WG", "SS", "OMF", "OH", "CMF", "DMF", "SMF", "WB", "SB", "CB", "STP", "CVR", "SW",
    "GK",
];

/// Where each role plays: 0..1 across, 0 (attack) .. 1 (own goal) down. Wide roles hold both flanks (the ratings name
/// no side). Inferred, like the names.
const ROLE_SPOTS: [&[(f32, f32)]; 16] = [
    &[(0.5, 0.12)],                // CF
    &[(0.5, 0.08)],                // TM
    &[(0.13, 0.18), (0.87, 0.18)], // WG
    &[(0.5, 0.25)],                // SS
    &[(0.5, 0.36)],                // OMF
    &[(0.22, 0.36), (0.78, 0.36)], // OH
    &[(0.5, 0.5)],                 // CMF
    &[(0.5, 0.62)],                // DMF
    &[(0.13, 0.48), (0.87, 0.48)], // SMF
    &[(0.12, 0.6), (0.88, 0.6)],   // WB
    &[(0.12, 0.74), (0.88, 0.74)], // SB
    &[(0.37, 0.78), (0.63, 0.78)], // CB
    &[(0.5, 0.74)],                // STP
    &[(0.5, 0.8)],                 // CVR
    &[(0.5, 0.86)],                // SW
    &[(0.5, 0.94)],                // GK
];

/// A card's suitable positions, as the back of the original card shows them: 16 role ratings in [`ROLES`] order, each
/// 1..10; 0 is no data (2017-18's own cards hold zeros there).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Aptitude {
    pub roles: [u8; 16],
}

/// How well a card fits a spot: rated 8 or more (the C panel's "good"), 5 or more, less.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Best,
    Fits,
    Out,
}

impl Fit {
    pub fn of(rating: f32) -> Fit {
        match rating {
            r if r >= 8.0 => Fit::Best,
            r if r >= 5.0 => Fit::Fits,
            _ => Fit::Out,
        }
    }
}

impl Aptitude {
    pub fn known(&self) -> bool {
        self.roles.iter().any(|&r| r > 0)
    }

    /// The rating at a pitch point (0..1 as in [`ROLE_SPOTS`]): the best role there, full within an eighth of the
    /// pitch's width of the role's spot, fading to nothing at three tenths.
    pub fn at(&self, p: Pos2) -> f32 {
        let mut best = 0.0f32;
        for (rating, spots) in self.roles.iter().zip(ROLE_SPOTS) {
            for &(x, y) in spots {
                // in pitch widths: the pitch is 1.25 times as tall as wide
                let d = vec2(p.x - x, (p.y - y) * PITCH_ASPECT).length();
                let t = ((0.3 - d) / 0.18).clamp(0.0, 1.0);
                best = best.max(*rating as f32 * t * t * (3.0 - 2.0 * t));
            }
        }
        best
    }

    /// The three best roles, best first, for the big card.
    pub fn top(&self) -> Vec<(&'static str, u8)> {
        let mut v: Vec<_> = ROLES
            .iter()
            .zip(self.roles)
            .filter(|(_, r)| *r > 0)
            .map(|(n, r)| (*n, r))
            .collect();
        v.sort_by_key(|(_, r)| std::cmp::Reverse(*r));
        v.truncate(3);
        v
    }
}

/// The heat ramp: nothing below 3, then dark orange, amber, pale yellow at 10. Brightness and opacity both rise, so
/// it reads on the green pitch without telling hues apart.
fn heat_color(v: f32) -> Color32 {
    let t = ((v - 3.0) / 7.0).clamp(0.0, 1.0);
    let stops = [
        (0.0, [176.0, 64.0, 0.0, 0.0]),
        (0.35, [224.0, 112.0, 0.0, 0.55]),
        (0.7, [255.0, 180.0, 0.0, 0.72]),
        (1.0, [255.0, 246.0, 208.0, 0.9]),
    ];
    let i = stops.iter().rposition(|s| s.0 <= t).unwrap_or(0).min(2);
    let ((t0, a), (t1, b)) = (stops[i], stops[i + 1]);
    let f = (t - t0) / (t1 - t0);
    let m = |k: usize| a[k] + (b[k] - a[k]) * f;
    Color32::from_rgba_unmultiplied(m(0) as u8, m(1) as u8, m(2) as u8, (m(3) * 255.0) as u8)
}

/// The heat zone over `pitch`: a vertex-coloured mesh sampled on a grid, the pitch dimmed under it so it stands out.
pub fn heat_zone(pitch: Rect, apt: &Aptitude) -> egui::Shape {
    let (nx, ny) = (28u32, 35u32);
    let mut mesh = egui::Mesh::default();
    for j in 0..=ny {
        for i in 0..=nx {
            let uv = vec2(i as f32 / nx as f32, j as f32 / ny as f32);
            mesh.colored_vertex(pitch.lerp_inside(uv), heat_color(apt.at(uv.to_pos2())));
        }
    }
    for j in 0..ny {
        for i in 0..nx {
            let a = j * (nx + 1) + i;
            mesh.add_triangle(a, a + 1, a + nx + 2);
            mesh.add_triangle(a, a + nx + 2, a + nx + 1);
        }
    }
    egui::Shape::Vec(vec![
        egui::Shape::rect_filled(pitch, theme::RADIUS_BIG, Color32::from_black_alpha(80)),
        egui::Shape::mesh(mesh),
    ])
}

// ---- card pictures ----

/// Names the folder of card pictures, `<number>.png` each (the kit's `data\cache\cards`, extracted at setup on the
/// player's PC), or none for the drawn stand-in. Pictures load in the background through egui's loaders and stay
/// cached, so scrolling the catalogue never waits on a file.
pub fn set_picture_dir(ctx: &egui::Context, dir: Option<std::path::PathBuf>) {
    ctx.data_mut(|d| d.insert_temp(Id::new("card-pictures"), dir));
}

/// The card's picture in `r`; the stand-in while it loads, when the file is missing, or with no folder set.
fn card_picture(p: &Painter, r: Rect, card: &Card, rim: Color32, alpha: f32) {
    gradient(
        p,
        r,
        2,
        theme::RAISED_HI.gamma_multiply(alpha),
        theme::DEEP.gamma_multiply(alpha),
        true,
    );
    let dir = p
        .ctx()
        .data(|d| d.get_temp::<Option<std::path::PathBuf>>(Id::new("card-pictures")))
        .flatten();
    if let Some(dir) = dir {
        // `file:///C:\...` on Windows, `file:///home/...` elsewhere: what egui_extras' file loader parses
        let file = dir.join(format!("{}.png", card.number));
        let uri = format!(
            "file:///{}",
            file.display().to_string().trim_start_matches('/')
        );
        if let Ok(egui::load::TexturePoll::Ready { texture }) = p.ctx().try_load_texture(
            &uri,
            egui::TextureOptions::LINEAR,
            egui::SizeHint::default(),
        ) {
            p.image(
                texture.id,
                r,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE.gamma_multiply(alpha),
            );
            return;
        }
    }
    // the stand-in: a neutral head-and-shoulders silhouette, plainly not a photo
    let col = rim
        .lerp_to_gamma(theme::RAISED_HI, 0.55)
        .gamma_multiply(0.75 * alpha);
    let w = r.width();
    let base = r.center_bottom();
    let arc: Vec<Pos2> = (0..=16)
        .map(|i| {
            let a = std::f32::consts::PI * (1.0 + i as f32 / 16.0);
            base + vec2(a.cos() * w * 0.4, a.sin() * w * 0.34)
        })
        .collect();
    fan(p, base - vec2(0.0, w * 0.1), &arc, col);
    p.circle_filled(base - vec2(0.0, w * 0.55), w * 0.18, col);
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

/// The card face, upright 58:81: the position band with TOTAL, the player's picture, the name on its foot; large ones
/// add the club and the season. The picture comes from the folder [`set_picture_dir`] names, else a drawn stand-in.
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
    let band_h = (rect.height() * 0.17).max(13.0);
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

    // the picture: a square under the band, as wide as the card allows (the original pictures are 128x128)
    let inset = if big { 6.0 } else { 3.0 };
    let side = rect.width() - 2.0 * inset;
    let pic = Rect::from_min_size(
        band.left_bottom() + vec2(inset, inset * 0.7),
        vec2(side, side),
    );
    card_picture(p, pic, card, rim, alpha);
    // a scrim under the name, so it reads on any picture
    let scrim = pic.with_min_y(pic.bottom() - pic.height() * 0.38);
    gradient(
        p,
        scrim,
        0,
        Color32::TRANSPARENT,
        Color32::from_black_alpha((200.0 * alpha) as u8),
        true,
    );
    if big {
        let g = fit(
            p,
            &card.name,
            theme::display(rect.width() * 0.15),
            theme::TEXT.gamma_multiply(alpha),
            pic.width() - 8.0,
        );
        p.galley(
            pos2(
                pic.center().x - g.size().x / 2.0,
                pic.bottom() - g.size().y - 2.0,
            ),
            g,
            theme::TEXT,
        );
        centered_text(
            p,
            Rect::from_min_max(
                pos2(rect.left() + 6.0, pic.bottom() + 1.0),
                pos2(rect.right() - 6.0, pic.bottom() + rect.width() * 0.12),
            ),
            &card.club,
            theme::body(rect.width() * 0.09),
            theme::TEXT_DIM.gamma_multiply(alpha),
        );
        p.text(
            rect.left_bottom() + vec2(6.0, -4.0),
            Align2::LEFT_BOTTOM,
            &card.season,
            theme::display(rect.width() * 0.1),
            theme::TEXT_FAINT.gamma_multiply(alpha),
        );
        rarity_mark(
            p,
            rect.right_bottom() + vec2(-11.0, -11.0),
            14.0,
            card.rarity,
        );
    } else {
        let g = fit(
            p,
            card.short_name(),
            theme::display(rect.width() * 0.26),
            theme::TEXT.gamma_multiply(alpha),
            rect.width() - 6.0,
        );
        // the name sits on the picture's foot and the card's bottom edge
        p.galley(
            pos2(
                rect.center().x - g.size().x / 2.0,
                rect.bottom() - g.size().y - 2.0,
            ),
            g,
            theme::TEXT,
        );
        rarity_mark(
            p,
            pos2(pic.right() - 6.0, pic.top() + 6.0),
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
    /// Where each pitch slot sits, 0..1 across the card lane and 0 (attack) .. 1 (own goal) down.
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

/// The catalogue's and the widget sheet's card size; the board scales its cards with the pitch ([`card_size`]).
pub const CARD_SMALL: egui::Vec2 = egui::vec2(50.0, 70.0);
/// The pitch is this many times as tall as it is wide.
pub const PITCH_ASPECT: f32 = 1.25;
/// The strip down the pitch's left edge that holds the zone names; no card reaches into it.
const LABEL_LANE: f32 = 26.0;
/// The bench's cards against the pitch's.
const BENCH_SCALE: f32 = 0.8;

/// A pitch card for a pitch this wide: 58:81, about a seventh of the width.
pub fn card_size(pitch_w: f32) -> egui::Vec2 {
    let w = (pitch_w * 0.145).round();
    vec2(w, (w * 1.4).round())
}

/// Where a pitch slot's card sits: spots run across the pitch right of the zone names, and no card leaves the pitch.
pub fn spot_rect(pitch: Rect, spot: Pos2) -> Rect {
    let lane = pitch.with_min_x(pitch.left() + LABEL_LANE);
    let r = Rect::from_center_size(
        pos2(
            lane.left() + spot.x * lane.width(),
            pitch.top() + spot.y * pitch.height(),
        ),
        card_size(pitch.width()),
    );
    r.translate(vec2(0.0, (pitch.bottom() - 4.0 - r.bottom()).min(0.0)))
}

/// The pitch a board of this width draws, from `top_left`.
pub fn pitch_rect(top_left: Pos2, width: f32) -> Rect {
    Rect::from_min_size(top_left, vec2(width, width * PITCH_ASPECT))
}

/// A point of the sheet as a 0..1 pitch point, for [`Aptitude::at`].
fn on_pitch(pitch: Rect, p: Pos2) -> Pos2 {
    ((p - pitch.min) / pitch.size()).to_pos2()
}

/// Draws the board and handles it: drag a card onto another to swap (the target is lit while dragging), right-click
/// takes a card off, a resting mouse shows the card big after `hover_delay` seconds. While a card is dragged or shown
/// big, the pitch shows its suitable positions as a heat zone, and the drop target says how well it fits there.
/// Returns true when it changed.
pub fn formation_board(ui: &mut Ui, board: &mut Board, hover_delay: f32, lang: Lang) -> bool {
    let width = ui.available_width();
    let (pitch, _) = ui.allocate_exact_size(vec2(width, width * PITCH_ASPECT), Sense::hover());
    let dragging = egui::DragAndDrop::payload::<usize>(ui.ctx()).map(|a| *a);
    let pointer = ui.ctx().pointer_latest_pos();
    paint_pitch(ui.painter(), pitch, Some(lang));
    // the heat zone goes over the pitch and under the cards, once we know whose it is
    let heat_at = ui.painter().add(egui::Shape::Noop);
    let card = card_size(pitch.width());

    let mut changed = false;
    let mut rects = [Rect::NOTHING; 16];
    for (i, s) in board.spots.iter().enumerate() {
        rects[i] = spot_rect(pitch, *s);
    }
    // the bench under the pitch: a well with its name running up the left like the zones
    ui.add_space(8.0);
    let small = card * BENCH_SCALE;
    let (bench, _) = ui.allocate_exact_size(vec2(width, small.y + 12.0), Sense::hover());
    ui.painter()
        .rect_filled(bench, theme::RADIUS_BIG, theme::WELL);
    up_text(
        ui.painter(),
        bench.with_max_x(bench.left() + LABEL_LANE),
        &tr!(lang, "board.bench"),
        theme::TEXT_DIM,
    );
    let row = bench.with_min_x(bench.left() + LABEL_LANE);
    let gap = (row.width() - 5.0 * small.x) / 6.0;
    for b in 0..5 {
        rects[11 + b] =
            Rect::from_min_size(row.min + vec2(gap + b as f32 * (small.x + gap), 6.0), small);
    }

    let hover_id = Id::new("board-hover");
    let mut hovered_card = None;
    let mut drop_target = None;
    for (slot, &rect) in rects.iter().enumerate() {
        let resp = ui.interact(rect, Id::new(("board-slot", slot)), Sense::click_and_drag());
        let p = ui.painter();
        let hovered_by_drag =
            dragging.is_some_and(|d| d != slot) && resp.dnd_hover_payload::<usize>().is_some();
        if hovered_by_drag {
            drop_target = Some((slot, rect));
        }
        match board.slots[slot] {
            Some(ci) => {
                resp.dnd_set_drag_payload(slot);
                let c = &board.cards[ci];
                let is_dragged = dragging == Some(slot);
                let warn = slot < 11
                    && if c.aptitude.known() {
                        Fit::of(c.aptitude.at(on_pitch(pitch, rect.center()))) == Fit::Out
                    } else {
                        zone(board.spots[slot].y) != c.pos
                    };
                card_face(
                    p,
                    rect,
                    c,
                    Look {
                        ghost: is_dragged,
                        target: hovered_by_drag,
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
                    let at = pt + vec2(-card.x * 0.45, -card.y * 0.55);
                    card_face(
                        &lp,
                        Rect::from_center_size(at, card * 1.08),
                        c,
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
                let col = if hovered_by_drag {
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
    let mut shown_big = None;
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
                shown_big = Some(ci);
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(left));
            }
        }
        None => ui.data_mut(|d| d.remove::<(usize, f64)>(hover_id)),
    }

    // the heat zone of the card in hand (or shown big), and the drop target's fit
    let held = dragging
        .and_then(|d| board.slots[d])
        .or(shown_big)
        .map(|ci| board.cards[ci].aptitude);
    if let Some(apt) = held {
        if apt.known() {
            ui.painter().set(heat_at, heat_zone(pitch, &apt));
            if let Some((slot, rect)) = drop_target
                && slot < 11
            {
                let fit = Fit::of(apt.at(on_pitch(pitch, rect.center())));
                let lp = ui
                    .ctx()
                    .layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("board-fit")));
                fit_badge(&lp, pos2(rect.center().x, rect.bottom() + 4.0), fit, lang);
            }
        } else {
            // 2017-18's own cards carry no ratings: say so instead of an empty heat zone
            let p = ui.painter();
            let g = p.layout_no_wrap(tr!(lang, "board.no_data"), theme::bold(13.0), theme::TEXT);
            let r = Rect::from_center_size(
                pos2(pitch.center().x, pitch.top() + 22.0),
                g.size() + vec2(20.0, 8.0),
            );
            p.rect_filled(r, 12, Color32::from_black_alpha(190));
            p.galley(r.center() - g.size() / 2.0, g, theme::TEXT);
        }
    }
    changed
}

/// A pill under the drop target: BEST POSITION pale with dark ink, FITS amber, OUT OF POSITION red. The words say it,
/// so colour is never the only cue.
pub fn fit_badge(p: &Painter, top_center: Pos2, fit: Fit, lang: Lang) {
    let (key, fill, ink) = match fit {
        Fit::Best => (
            "board.fit_best",
            Color32::from_rgb(0xFF, 0xF6, 0xD0),
            theme::INK,
        ),
        Fit::Fits => ("board.fit_ok", theme::AMBER, theme::INK),
        Fit::Out => ("board.fit_out", theme::RED, Color32::WHITE),
    };
    let g = p.layout_no_wrap(tr!(lang, key), theme::display(15.0), ink);
    let r = Rect::from_min_size(
        top_center - vec2(g.size().x / 2.0 + 9.0, 0.0),
        g.size() + vec2(18.0, 6.0),
    );
    p.add(
        Shadow {
            offset: [0, 3],
            blur: 8,
            spread: 0,
            color: Color32::from_black_alpha(150),
        }
        .as_shape(r, 11),
    );
    p.rect_filled(r, 11, fill);
    p.galley(r.center() - g.size() / 2.0, g, ink);
}

/// Text running up a narrow strip, centred in it and fitted to its height. Japanese stands upright, one character
/// under the other, as vertical Japanese is written.
fn up_text(p: &Painter, strip: Rect, text: &str, col: Color32) {
    if !text.chars().any(|c| c.is_ascii_alphabetic()) {
        let size = (strip.width() - 8.0).min(16.0);
        let n = text.chars().count() as f32;
        let top = strip.center().y - n * size / 2.0;
        for (i, c) in text.chars().enumerate() {
            p.text(
                pos2(strip.center().x, top + (i as f32 + 0.5) * size),
                Align2::CENTER_CENTER,
                c,
                theme::bold(size - 2.0),
                col,
            );
        }
        return;
    }
    let g = fit(p, text, theme::display(14.0), col, strip.height() - 12.0);
    let at = pos2(
        strip.center().x - g.size().y / 2.0,
        strip.center().y + g.size().x / 2.0,
    );
    p.add(egui::epaint::TextShape::new(at, g, col).with_angle(-std::f32::consts::FRAC_PI_2));
}

/// The pitch, attacking upwards. With `labels`, the three zones carry their names up the left edge.
fn paint_pitch(p: &Painter, pitch: Rect, labels: Option<Lang>) {
    // mowing stripes
    let n = 10;
    let h = pitch.height() / n as f32;
    let radius = if labels.is_some() {
        theme::RADIUS_BIG
    } else {
        theme::RADIUS
    };
    p.rect_filled(pitch, radius, theme::PITCH);
    for i in (0..n).step_by(2) {
        let r = Rect::from_min_size(pitch.min + vec2(0.0, h * i as f32), vec2(pitch.width(), h));
        let cr = egui::CornerRadius {
            nw: if i == 0 { radius } else { 0 },
            ne: if i == 0 { radius } else { 0 },
            sw: 0,
            se: 0,
        };
        p.rect_filled(r, cr, theme::PITCH_STRIPE);
    }
    if let Some(lang) = labels {
        // the three zones: a dashed edge and a name up the label lane
        let bands = [
            (0.0, 0.34, tr!(lang, "board.zone_attack")),
            (0.34, 0.64, tr!(lang, "board.zone_centre")),
            (0.64, 1.0, tr!(lang, "board.zone_defence")),
        ];
        for (a, b, text) in bands {
            let r = Rect::from_x_y_ranges(
                pitch.left()..=pitch.left() + LABEL_LANE,
                (pitch.top() + a * pitch.height())..=(pitch.top() + b * pitch.height()),
            );
            if a > 0.0 {
                p.extend(egui::Shape::dashed_line(
                    &[pos2(pitch.left(), r.top()), pos2(pitch.right(), r.top())],
                    Stroke::new(1.0, Color32::from_black_alpha(70)),
                    6.0,
                    6.0,
                ));
            }
            up_text(p, r, &text, Color32::from_white_alpha(130));
        }
    }
    // markings
    let line = Stroke::new(1.5, theme::PITCH_LINE.gamma_multiply(0.75));
    let f = pitch.shrink((pitch.width() * 0.025).max(3.0));
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

/// The card back's position map: a small pitch with the heat zone, or "no position data".
pub fn aptitude_map(p: &Painter, rect: Rect, apt: &Aptitude, lang: Lang) {
    paint_pitch(p, rect, None);
    if apt.known() {
        p.add(heat_zone(rect, apt));
    } else {
        p.rect_filled(rect, theme::RADIUS, Color32::from_black_alpha(140));
        let g = p.layout(
            tr!(lang, "board.no_data"),
            theme::body(12.0),
            theme::TEXT,
            rect.width() - 12.0,
        );
        p.galley(rect.center() - g.size() / 2.0, g, theme::TEXT);
    }
}

/// The big card beside the board with its caption: a card at 2.6x and its line of facts, and the card back's
/// suitable positions beside it.
fn big_card(ui: &Ui, card: &Card, near: Rect, board_left: f32, lang: Lang) {
    let size = CARD_SMALL * 2.6;
    // the map under its 22 px heading, as tall as the card
    let map = vec2((size.y - 22.0) / PITCH_ASPECT, size.y - 22.0);
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
                    let wide = size.x + 10.0 + map.x;
                    ui.set_max_width(wide);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                        card_face(ui.painter(), r, card, Look::default());
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            ui.label(
                                egui::RichText::new(tr!(lang, "board.positions"))
                                    .font(theme::display(14.0))
                                    .color(theme::TEXT_DIM),
                            );
                            let (m, _) = ui.allocate_exact_size(map, Sense::hover());
                            aptitude_map(ui.painter(), m, &card.aptitude, lang);
                        });
                    });
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
                    let top = card.aptitude.top();
                    if !top.is_empty() {
                        let roles = top
                            .iter()
                            .map(|(n, r)| format!("{n} {r}"))
                            .collect::<Vec<_>>()
                            .join(" · ");
                        ui.label(
                            egui::RichText::new(tr!(lang, "board.roles", roles = roles))
                                .font(theme::body(12.0))
                                .italics()
                                .color(theme::TEXT_DIM),
                        );
                    }
                    let stats = [
                        "cat.off", "cat.def", "cat.tec", "cat.pow", "cat.spd", "cat.sta",
                    ];
                    egui::Grid::new("big-stats")
                        .num_columns(6)
                        .spacing(vec2(14.0, 0.0))
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
