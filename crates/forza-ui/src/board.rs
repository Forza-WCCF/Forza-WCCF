//! Player cards and the FORMATION BOARD: the card face (upright), the rarity marks, and the pitch where cards lie
//! anywhere, as on the cabinet's table (no fixed spots), and are dragged, swapped, taken off and shown big on a
//! resting mouse.

use super::Lang;
use super::theme;
use super::widgets::{fan, fit, fit_elided, gradient};
use egui::{
    Align2, Color32, Galley, Id, Order, Painter, Pos2, Rect, Response, Sense, Shadow, Stroke,
    StrokeKind, Ui, Vec2, pos2, vec2,
};
use forza::cards::hq::{self, Hq};
use forza::cards::links::Affinity;
use std::collections::HashMap;
use std::sync::Arc;

pub use forza::board::*;

mod pictures;
pub use pictures::*;

/// A line's colour (the cards, the position map).
pub trait PositionColor {
    fn color(self) -> Color32;
}

impl PositionColor for Position {
    fn color(self) -> Color32 {
        match self {
            Position::Gk => theme::POS_GK,
            Position::Df => theme::POS_DF,
            Position::Mf => theme::POS_MF,
            Position::Fw => theme::POS_FW,
        }
    }
}

/// The rim of a card's frame by rarity.
pub trait RarityRim {
    fn rim(self) -> Color32;
}

impl RarityRim for Rarity {
    /// The rim of the card frame.
    fn rim(self) -> Color32 {
        match self {
            Rarity::AllTime => Color32::from_rgb(0xB9, 0x9C, 0xFF),
            Rarity::Legend => theme::GOLD,
            Rarity::Rare => Color32::from_rgb(0xC6, 0xD2, 0xE2),
            Rarity::Special => theme::CYAN,
            Rarity::Regular => theme::RIM_HI,
        }
    }
}

/// An area's rect within `field`.
fn area_rect(field: Rect, [x, y, w, h]: [f32; 4]) -> Rect {
    Rect::from_min_size(field.lerp_inside(vec2(x, y)), field.size() * vec2(w, h))
}

/// The position map over `field`, as the fan database draws it: a dark pitch, one rounded tile per area tinted by
/// its line's colour, solid at 8 or more, half at 5 or more, dark below; each labelled with its best role and rating
/// in `size` points, or the rating alone, or nothing, as room allows.
pub fn heat_zone(p: &Painter, pitch: Rect, field: Rect, apt: &Aptitude, size: f32) -> egui::Shape {
    let mut v = vec![egui::Shape::rect_filled(
        pitch,
        theme::RADIUS_BIG,
        theme::MAP_PITCH,
    )];
    let font = theme::bold(size);
    for (i, (area, roles, line)) in AREAS.iter().enumerate() {
        let r = area_rect(field, *area);
        let (name, rating) = apt.best(roles);
        let lc = line.color();
        let (fill, rim, text) = match Fit::of(rating as f32) {
            Fit::Best => (lc, Color32::WHITE, Color32::WHITE),
            Fit::Fits => (theme::MAP_PITCH.lerp_to_gamma(lc, 0.45), lc, Color32::WHITE),
            Fit::Out => (
                theme::MAP_PITCH,
                Color32::from_white_alpha(0x30),
                Color32::from_white_alpha(0x80),
            ),
        };
        v.push(egui::Shape::rect_filled(r, 3, fill));
        v.push(egui::Shape::rect_stroke(
            r,
            3,
            Stroke::new(1.0, rim),
            StrokeKind::Inside,
        ));
        let label = match rating {
            0 => vec![name.to_string()],
            n => vec![format!("{name} {n}"), n.to_string()],
        };
        let Some(g) = label
            .into_iter()
            .map(|t| p.layout_no_wrap(t, font.clone(), text))
            .find(|g| g.size().x <= r.width() - 4.0 && g.size().y <= r.height() - 2.0)
        else {
            continue;
        };
        // CF's label goes to its top edge: SS sits inside it (AREAS[6] is CF)
        let at = if i == 6 {
            pos2(r.center().x, r.top() + 2.0 + g.size().y / 2.0)
        } else {
            r.center()
        } - g.size() / 2.0;
        v.push(egui::Shape::galley_with_override_text_color(
            at + vec2(0.0, 1.0),
            g.clone(),
            Color32::from_black_alpha(160),
        ));
        v.push(egui::Shape::galley(at, g, text));
    }
    v.push(egui::Shape::rect_stroke(
        pitch,
        theme::RADIUS_BIG,
        Stroke::new(2.0, Color32::from_white_alpha(0x66)),
        StrokeKind::Inside,
    ));
    egui::Shape::Vec(v)
}

// ---- links ----

/// What the board knows of links: the club's link (0-255) between two of its cards, from the club card; the players'
/// affinity (1-5), from the game's own files ([`forza::cards::links`]). Empty: no lines, no lists.
#[derive(Clone, Debug, Default)]
pub struct Links {
    /// Smaller card number first.
    pub club: HashMap<(u32, u32), u8>,
    pub affinity: Option<Arc<Affinity>>,
}

impl Links {
    pub fn link(&self, a: u32, b: u32) -> u8 {
        self.club.get(&(a.min(b), a.max(b))).copied().unwrap_or(0)
    }

    pub fn affinity(&self, a: u32, b: u32) -> Option<u8> {
        self.affinity.as_ref()?.level(a, b)
    }

    /// In `a`'s group: a line drawn between them, or the best affinity (4-5, the game's "best affinity").
    fn grouped(&self, a: u32, b: u32) -> bool {
        a == b
            || Stage::of(self.link(a, b)).is_some()
            || self.affinity(a, b).is_some_and(|v| v >= 4)
    }
}

/// The lines the game draws for a link: none below 150, a thin white line from 150, a thick white line from 200, the
/// golden link at 255.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Thin,
    Thick,
    Gold,
}

impl Stage {
    pub fn of(link: u8) -> Option<Stage> {
        match link {
            255 => Some(Stage::Gold),
            200.. => Some(Stage::Thick),
            150.. => Some(Stage::Thin),
            _ => None,
        }
    }
}

/// A link line: a dark casing so it reads on the pitch, then thin white 2, thick white 4.5, or gold 6 with a dark rail
/// down its middle - width and the rail tell the stages apart without colour.
pub fn link_line(shapes: &mut Vec<egui::Shape>, a: Pos2, b: Pos2, stage: Stage, alpha: f32) {
    let (w, col) = match stage {
        Stage::Thin => (2.0, Color32::WHITE),
        Stage::Thick => (4.5, Color32::WHITE),
        Stage::Gold => (6.0, theme::GOLD),
    };
    shapes.push(egui::Shape::line_segment(
        [a, b],
        Stroke::new(w + 2.5, Color32::from_black_alpha((120.0 * alpha) as u8)),
    ));
    shapes.push(egui::Shape::line_segment(
        [a, b],
        Stroke::new(w, col.gamma_multiply(alpha)),
    ));
    if stage == Stage::Gold {
        shapes.push(egui::Shape::line_segment(
            [a, b],
            Stroke::new(1.5, theme::INK.gamma_multiply(alpha)),
        ));
    }
}

/// Affinity as five dots from `left_center`, the level's filled.
pub fn affinity_dots(p: &Painter, left_center: Pos2, level: u8) {
    for k in 0..5u8 {
        let c = left_center + vec2(4.0 + f32::from(k) * 9.0, 0.0);
        if k < level {
            p.circle_filled(c, 3.5, theme::AMBER);
        } else {
            p.circle_stroke(c, 3.0, Stroke::new(1.0, theme::TEXT_FAINT));
        }
    }
}

// ---- the formation board ----

/// The game's two lines, as a spot's height (0 the far goal line, 1 the own): the FW band ends 25 m past halfway, the
/// MF band 15 m inside the own half, on the 105 m pitch. From the game's own area table (2017-18 `AI/AI_DATA/area.adb`,
/// area 10 at 0x194: z 25..67 m, area 11 at 0x1BC: z -15..25 m; 2010-11's line function FUN_004db340 asks the same
/// areas, read from a running seat), FW asked first and the edges inclusive.
const LINE_FW: f32 = (52.5 - 25.0) / 105.0;
const LINE_MF: f32 = (52.5 + 15.0) / 105.0;

/// The line a spot plays in, by the game's own rule (2017-18 client 0x5D2B40, 2010-11 FUN_004db340): the keeper's box
/// first (table y > 0.8 and -0.2 < x < 0.2), then the FW band, the MF band, else DF ([`LINE_FW`], [`LINE_MF`]).
fn zone(spot: Pos2) -> Position {
    if spot.y > 0.9 && (spot.x - 0.5).abs() < 0.1 {
        Position::Gk
    } else if spot.y <= LINE_FW {
        Position::Fw
    } else if spot.y <= LINE_MF {
        Position::Mf
    } else {
        Position::Df
    }
}

/// A card's width over its height: the pack's scans (180x254, the printed card's shape; a 63x88 mm card is 0.716).
pub const CARD_ASPECT: f32 = 180.0 / 254.0;
/// The catalogue's and the widget sheet's card size; the board scales its cards with the pitch ([`card_size`]).
pub const CARD_SMALL: egui::Vec2 = egui::vec2(50.0, 50.0 / CARD_ASPECT);
/// The pitch is this many times as tall as it is wide.
pub const PITCH_ASPECT: f32 = 1.25;
/// The bench's cards against the pitch's.
const BENCH_SCALE: f32 = 0.8;
/// The pop-out board (pitch and bench column, [`Mat::column`]) is this many times as tall as it is wide.
pub const COLUMN_ASPECT: f32 =
    art::W * PITCH_ASPECT / (art::SHOWN[1] - art::SHOWN[0] + art::COLUMN_W);

/// A pitch card for a pitch this wide: the card's shape ([`CARD_ASPECT`]), about a seventh of the width.
pub fn card_size(pitch_w: f32) -> egui::Vec2 {
    let w = (pitch_w * 0.145).round();
    vec2(w, (w / CARD_ASPECT).round())
}

/// Where a pitch card sits: centred on its spot, pushed back inside when its edge would leave the pitch (the spot
/// itself, what the table holds, is kept). The keeper's card (a spot in the keeper's zone, [`zone`]) sits lower, as
/// on the cabinet's mat: on the mat's GK box, half below the own goal line.
pub fn spot_rect(pitch: Rect, spot: Pos2) -> Rect {
    spot_in(pitch, pitch.shrink(4.0), spot)
}

/// [`spot_rect`], the card kept inside `inside`.
fn spot_in(pitch: Rect, inside: Rect, spot: Pos2) -> Rect {
    let f = field(pitch);
    let y = if zone(spot) == Position::Gk {
        art::rect(pitch, art::GK).center().y
    } else {
        f.top() + spot.y * f.height()
    };
    let r = Rect::from_center_size(
        pos2(f.left() + spot.x * f.width(), y),
        card_size(pitch.width()),
    );
    let push = |lo: f32, hi: f32, min: f32, max: f32| (min - lo).max(0.0) + (max - hi).min(0.0);
    r.translate(vec2(
        push(r.left(), r.right(), inside.left(), inside.right()),
        push(r.top(), r.bottom(), inside.top(), inside.bottom()),
    ))
}

/// The board window's board height `width` wide ([`Mat::row`]): the pitch, its gap of 12 (8 and the desk's item
/// spacing of 4), the bench strip.
pub fn row_height(width: f32) -> f32 {
    width * PITCH_ASPECT + 12.0 + art::bench_h(width)
}

/// Where every slot's card sits: the 11 pitch slots at their spots (empty ones too, unused), the 5 bench slots on the
/// artwork's numbered boxes, the bench from `bench_top` (the board puts it 12 points under the pitch).
pub fn slot_rects(pitch: Rect, bench_top: f32, spots: &[Pos2; 11]) -> [Rect; 16] {
    Mat::row(pitch, bench_top).slot_rects(spots)
}

/// Where the board's parts lie: `pitch` the pitch artwork's rect (cards and spots go by it), `shown` the part of it
/// drawn, `bench` the bench, a strip under the pitch ([`Mat::row`]) or a column at its right ([`Mat::column`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat {
    pub pitch: Rect,
    pub shown: Rect,
    pub bench: Rect,
    pub column: bool,
}

impl Mat {
    /// The in-game board: the whole pitch, the bench strip under it from `bench_top`.
    pub fn row(pitch: Rect, bench_top: f32) -> Mat {
        let bench = bench_rect(pitch, bench_top);
        Mat {
            pitch,
            shown: pitch,
            bench,
            column: false,
        }
    }

    /// The pop-out board, `width` wide from `top_left` ([`COLUMN_ASPECT`] tall): the pitch from a touchline margin to
    /// the other, the bench column flush at its right, as tall as the pitch. The artwork's side margins run under the
    /// column and off the left edge, undrawn.
    pub fn column(top_left: Pos2, width: f32) -> Mat {
        let s = width / (art::SHOWN[1] - art::SHOWN[0] + art::COLUMN_W);
        let pitch = pitch_rect(top_left - vec2(art::SHOWN[0] * s, 0.0), art::W * s);
        let shown = Rect::from_min_size(
            top_left,
            vec2((art::SHOWN[1] - art::SHOWN[0]) * s, pitch.height()),
        );
        let bench = Rect::from_min_max(shown.right_top(), pos2(top_left.x + width, shown.bottom()));
        Mat {
            pitch,
            shown,
            bench,
            column: true,
        }
    }

    /// The bench's numbered box `i` (0-4).
    pub fn bench_box(&self, i: usize) -> Rect {
        if self.column {
            art::column_slot(self.bench, i)
        } else {
            art::slot(self.bench, i)
        }
    }

    /// Where every slot's card sits ([`slot_rects`]); pitch cards stay on the pitch as shown.
    pub fn slot_rects(&self, spots: &[Pos2; 11]) -> [Rect; 16] {
        let mut rects = [Rect::NOTHING; 16];
        for (i, s) in spots.iter().enumerate() {
            rects[i] = spot_in(self.pitch, self.shown.shrink(4.0), *s);
        }
        let small = card_size(self.pitch.width()) * BENCH_SCALE;
        for b in 0..5 {
            rects[11 + b] = Rect::from_center_size(self.bench_box(b).center(), small);
        }
        rects
    }
}

/// The bench strip under a pitch, from `bench_top`: the artwork's, as wide as the pitch.
pub fn bench_rect(pitch: Rect, bench_top: f32) -> Rect {
    Rect::from_min_size(
        pos2(pitch.left(), bench_top),
        vec2(pitch.width(), art::bench_h(pitch.width())),
    )
}

/// A sheet point on `pitch` as a spot (the inverse of [`spot_rect`]'s centre, kept on the pitch).
fn spot_at(pitch: Rect, p: Pos2) -> Pos2 {
    let f = field(pitch);
    pos2(
        ((p.x - f.left()) / f.width()).clamp(0.0, 1.0),
        ((p.y - f.top()) / f.height()).clamp(0.0, 1.0),
    )
}

/// The board's drawn field lines, the pitch artwork's ([`art`]): touchline to touchline is the table's x -1..+1, the
/// far goal line to the own goal line its y -1..+1 (the 2010-11 C panel's FIELD_L..FIELD_B), so a card on a line lies
/// on it in the table too. The mat goes on below the own goal line, where the keeper's box lies.
fn field(pitch: Rect) -> Rect {
    art::rect(pitch, art::FIELD)
}

/// The project's pitch artwork (`campo wccf`, a 2964x4419 picture of the cabinet's mat), traced into shapes so it is
/// sharp at any size: its pitch part, from 20 px under the top, 2964x3705 px ([`PITCH_ASPECT`] tall). Positions are
/// that picture's pixels (line centres), colours sampled from it.
mod art {
    use egui::{Color32, Pos2, Rect, pos2};

    /// The picture's width, and where the board's pitch starts in it.
    pub const W: f32 = 2964.0;
    pub const TOP: f32 = 20.0;
    /// Touchlines and goal lines: left, far goal line, right, own goal line.
    pub const FIELD: [f32; 4] = [359.5, 64.5, 2606.0, 3361.5];
    pub const LINE: f32 = 21.0;
    /// Mown stripes across the field, the first (at the left touchline) light.
    pub const STRIPES: usize = 17;
    /// The midfield band, mown a yellower green.
    pub const MF_BAND: [f32; 2] = [945.0, 2171.0];
    /// The penalty box and the goal area: left and right edge, depth from the goal line; the penalty spot's distance.
    pub const BOX: [f32; 3] = [878.5, 2086.0, 495.0];
    pub const GOAL_AREA: [f32; 3] = [1208.5, 1756.5, 165.0];
    pub const SPOT: f32 = 330.0;
    pub const CIRCLE: f32 = 266.25;
    pub const CORNER: f32 = 103.0;
    pub const CENTRE_SPOT: f32 = 20.0;
    /// The keeper's box, half under the own goal line: left, top, right, bottom.
    pub const GK: [f32; 4] = [1311.0, 3232.0, 1656.0, 3716.0];
    /// The zone names' centre heights and their capital letters' height.
    pub const LABELS: [(&str, f32, Color32); 3] = [
        ("FW", 396.0, Color32::from_rgb(0x8F, 0x97, 0x27)),
        ("MF", 1121.0, Color32::from_rgb(0x5E, 0x6B, 0x32)),
        ("DF", 2372.0, Color32::from_rgb(0x93, 0x99, 0x29)),
    ];
    pub const CAPS: f32 = 112.0;
    pub const GRASS: Color32 = Color32::from_rgb(0x76, 0x7E, 0x31);
    /// Stripes (light, dark) outside the midfield band and in it.
    pub const MOWN: [Color32; 2] = [
        Color32::from_rgb(0x79, 0x7C, 0x30),
        Color32::from_rgb(0x68, 0x72, 0x33),
    ];
    pub const MOWN_MF: [Color32; 2] = [
        Color32::from_rgb(0x85, 0x88, 0x29),
        Color32::from_rgb(0x74, 0x7C, 0x2F),
    ];
    pub const CHALK: Color32 = Color32::from_rgb(0xB1, 0xAB, 0x67);
    /// The bench strip under the pitch: from its cream top line to the picture's foot; the line's foot.
    pub const BENCH: [f32; 2] = [3812.0, 4419.0];
    pub const BENCH_LINE: f32 = 3830.0;
    /// The five numbered boxes (left, right), their top and foot, their cream rim; the numbers' capital height.
    pub const SLOTS: [[f32; 2]; 5] = [
        [268.0, 646.0],
        [784.0, 1160.0],
        [1298.0, 1676.0],
        [1816.0, 2194.0],
        [2337.0, 2715.0],
    ];
    pub const SLOT_Y: [f32; 2] = [3870.0, 4389.0];
    pub const SLOT_RIM: f32 = 10.0;
    pub const DIGIT: f32 = 256.0;
    /// The SUB label (left, top, right, bottom) and the badge's box.
    pub const SUB: [f32; 4] = [40.0, 3940.0, 131.0, 4318.0];
    pub const BADGE: [f32; 4] = [2748.0, 4036.0, 2944.0, 4214.0];
    /// The pop-out board: the pitch as shown (left, right: the touchlines and a margin), and the bench column at its
    /// right, its own x from its left edge and y from its top (the pitch's): its width, the cream line down its left edge, the
    /// crest's and the SUB label's boxes (left, top, right, bottom), the first box's top and the step between boxes.
    pub const SHOWN: [f32; 2] = [269.5, 2696.0];
    pub const COLUMN_W: f32 = 459.0;
    pub const COLUMN_LINE: f32 = 21.0;
    pub const CREST: [f32; 4] = [55.0, 60.0, 425.0, 430.0];
    pub const COLUMN_SUB: [f32; 4] = [51.0, 470.0, 429.0, 560.0];
    pub const COLUMN_SLOT: [f32; 2] = [590.0, 546.0];
    pub const SOIL: Color32 = Color32::from_rgb(0x6C, 0x57, 0x20);
    pub const CREAM: Color32 = Color32::from_rgb(0xE2, 0xD4, 0xAD);
    pub const SLOT_FILL: Color32 = Color32::from_rgb(0xB9, 0xB1, 0x73);
    pub const DIGITS: Color32 = Color32::from_rgb(0xE2, 0xD5, 0xA9);
    pub const GK_BOX: Color32 = Color32::from_rgb(0xB7, 0xAE, 0x6F);
    pub const GK_TEXT: Color32 = Color32::from_rgb(0x65, 0x78, 0x3C);

    /// A point of the picture on a pitch drawn in `pitch`.
    pub fn at(pitch: Rect, x: f32, y: f32) -> Pos2 {
        pitch.min + egui::vec2(x, y - TOP) * (pitch.width() / W)
    }

    /// A rectangle of the picture (left, top, right, bottom) on the pitch.
    pub fn rect(pitch: Rect, [l, t, r, b]: [f32; 4]) -> Rect {
        Rect::from_min_max(at(pitch, l, t), at(pitch, r, b))
    }

    /// A point of the picture's bench strip on a bench drawn in `bench` (its top at [`BENCH`]'s).
    pub fn bench_at(bench: Rect, x: f32, y: f32) -> Pos2 {
        bench.min + egui::vec2(x, y - BENCH[0]) * (bench.width() / W)
    }

    /// The bench strip's height for a pitch `width` wide.
    pub fn bench_h(width: f32) -> f32 {
        (BENCH[1] - BENCH[0]) * width / W
    }

    /// A slot's box on the bench.
    pub fn slot(bench: Rect, i: usize) -> Rect {
        let [l, r] = SLOTS[i];
        Rect::from_min_max(bench_at(bench, l, SLOT_Y[0]), bench_at(bench, r, SLOT_Y[1]))
    }

    /// A point of the pop-out board's bench column drawn in `bench` (from its left top corner).
    pub fn column_at(bench: Rect, x: f32, y: f32) -> Pos2 {
        bench.min + egui::vec2(x, y) * (bench.width() / COLUMN_W)
    }

    /// A slot's box on the bench column: the strip's box size, stacked.
    pub fn column_slot(bench: Rect, i: usize) -> Rect {
        let l = COLUMN_SUB[0];
        let t = COLUMN_SLOT[0] + COLUMN_SLOT[1] * i as f32;
        let (w, h) = (SLOTS[0][1] - SLOTS[0][0], SLOT_Y[1] - SLOT_Y[0]);
        Rect::from_min_max(column_at(bench, l, t), column_at(bench, l + w, t + h))
    }

    /// A length of the picture on the pitch.
    pub fn len(pitch: Rect, v: f32) -> f32 {
        v * pitch.width() / W
    }

    /// An arc round `c` of radius `r` from angle `a0` to `a1` (radians, y down), as a polyline.
    pub fn arc(c: Pos2, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
        let n = 24;
        (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / n as f32;
                pos2(c.x + r * a.cos(), c.y + r * a.sin())
            })
            .collect()
    }
}

/// The pitch a board of this width draws, from `top_left`.
pub fn pitch_rect(top_left: Pos2, width: f32) -> Rect {
    Rect::from_min_size(top_left, vec2(width, width * PITCH_ASPECT))
}

/// Where a card in hand sits: its centre keeps the offset from the pointer that it had at the press (`grab`, as a
/// fraction of the card), so it stays under the pointer where it was taken, and lands where it is shown.
fn drag_centre(pointer: Pos2, grab: Vec2, card: Vec2) -> Pos2 {
    pointer + grab * card
}

/// The slot whose card lies under `centre`, other than the dragged one: the drop target. `shown` says which slots
/// are drawn (an empty pitch slot is no place).
fn target_slot(
    rects: &[Rect; 16],
    shown: impl Fn(usize) -> bool,
    dragged: Option<usize>,
    centre: Pos2,
) -> Option<usize> {
    (0..16)
        .rev()
        .find(|&s| Some(s) != dragged && shown(s) && rects[s].contains(centre))
}

/// The grab of the drag in progress: the card centre less the press point, as a fraction of the card. Board drags
/// store it on their first frame; a catalogue card stores it when its drag starts ([`catalogue_card`]).
fn grab_id() -> Id {
    Id::unique("board-grab")
}

/// A card taken off the board (dragged off it and let go): what UNDO puts back.
#[derive(Clone, Debug, PartialEq)]
pub struct Removed {
    pub card: Card,
    /// Its slot (0-10 the pitch, 11-15 the bench) and, on the pitch, its spot before the lift.
    pub slot: usize,
    pub spot: Pos2,
    /// Where the pointer let go, from the board's top left, and the board's size (for the log).
    pub at: Pos2,
    pub size: Vec2,
}

fn removed_id() -> Id {
    Id::unique("board-removed")
}

fn keep_id() -> Id {
    Id::unique("board-keep")
}

fn kept_id() -> Id {
    Id::unique("board-kept")
}

/// While `keep` (a match runs), a card let go off the board stays on the table and snaps back to its spot
/// ([`take_kept`]); set it before each [`formation_board`] call.
pub fn keep_on_table(ui: &Ui, keep: bool) {
    ui.data_mut(|d| d.insert_temp(keep_id(), keep));
}

fn group_on_id() -> Id {
    Id::unique("board-group-on")
}

fn group_id() -> Id {
    Id::unique("board-group")
}

/// With `on` (2017-18), a drag on open pitch boxes pitch cards into a group and Ctrl- or Shift-click adds or removes
/// one; a group member in hand moves the whole group. Off by default; set it before each [`formation_board`] call.
pub fn group_moves(ui: &Ui, on: bool) {
    ui.data_mut(|d| d.insert_temp(group_on_id(), on));
}

/// Whether the card on `slot` may join a group: a pitch card off the keeper's spot (the keeper is drawn on its box
/// whatever its spot, so a moved keeper would show still while the game sees it move).
fn groupable(board: &Board, slot: usize) -> bool {
    slot < 11 && board.slots[slot].is_some() && zone(board.spots[slot]) != Position::Gk
}

/// The pitch slots whose card centre lies in `area` (the box let go on open pitch).
fn boxed(rects: &[Rect], filled: impl Fn(usize) -> bool, area: Rect) -> Vec<usize> {
    (0..11)
        .filter(|&s| filled(s) && area.contains(rects[s].center()))
        .collect()
}

/// A group's spots moved by `want` from `origins`: the offset clamped once for the whole group so every spot stays in
/// 0..1 and the group keeps its shape. An origin past 0..1 (an odd table file) is first brought onto the pitch.
fn group_spots(origins: &[Pos2], want: Vec2) -> Vec<Pos2> {
    let on: Vec<Pos2> = origins
        .iter()
        .map(|o| o.clamp(Pos2::ZERO, pos2(1.0, 1.0)))
        .collect();
    let lo = on.iter().fold(pos2(1.0, 1.0), |a, o| a.min(*o));
    let hi = on.iter().fold(Pos2::ZERO, |a, o| a.max(*o));
    let by = vec2(
        want.x.clamp(-lo.x, 1.0 - hi.x),
        want.y.clamp(-lo.y, 1.0 - hi.y),
    );
    on.into_iter().map(|o| o + by).collect()
}

/// The card [`formation_board`] refused to take off (a match runs) since the last call, if any.
pub fn take_kept(ui: &Ui) -> Option<u32> {
    ui.data_mut(|d| d.remove_temp::<u32>(kept_id()))
}

fn touched_id() -> Id {
    Id::unique("board-touched")
}

/// The card pressed or in hand on a `pure` [`formation_board`] this frame, if any.
pub fn take_touched(ui: &Ui) -> Option<Card> {
    ui.data_mut(|d| {
        let c = d.get_temp::<Card>(touched_id());
        d.remove::<Card>(touched_id());
        c
    })
}

/// The card [`formation_board`] took off this frame, if any.
pub fn take_removed(ui: &Ui) -> Option<Removed> {
    ui.data_mut(|d| {
        let r = d.get_temp::<Removed>(removed_id());
        d.remove::<Removed>(removed_id());
        r
    })
}

/// UNDO: the card back where it was, a pitch card on its spot, a bench card in its box (the next free one when that
/// was taken since). False when the table holds it again or has no room.
pub fn undo(board: &mut Board, r: &Removed) -> bool {
    if r.slot < 11 {
        return board.put_free(&r.card, r.spot);
    }
    (r.slot..16)
        .chain(11..r.slot)
        .find(|&s| board.slots[s].is_none())
        .is_some_and(|s| board.put(&r.card, s))
}

/// Seconds after the last touch of the board that a selected card stays selected.
const SELECT_HOLD: f64 = 3.0;

/// Draws the board and handles it: a card dropped on open pitch lies exactly there (the cabinet's table has no fixed
/// spots), a card dropped onto another swaps with it (the target is lit while dragging), a drag off the board takes
/// a card off ([`take_removed`]; no right-click: a long press on a touch screen is one), a resting mouse shows the card big after `hover_delay` seconds (None: never). While a card is dragged or shown
/// big, the pitch shows its suitable positions as a heat zone, and the drop target says how well it fits there.
/// With `links`, lines join every two linked pitch cards by the club's link stage; a selected card (clicked, or in
/// hand) brings its group forward - its lines bright, the others and the cards outside its group dimmed - and a list
/// beside the board names its partners, on the team and not. With `column` the bench is a column at the pitch's right
/// ([`Mat::column`]), else a strip under it. `pure` (the pop-out board): moves only, no big card, no link selection
/// and no lines (a box group still moves, with [`group_moves`]); the card pressed or in hand is kept for
/// [`take_touched`]. Returns true when it changed.
pub fn formation_board(
    ui: &mut Ui,
    board: &mut Board,
    links: &Links,
    hover_delay: Option<f32>,
    lang: Lang,
    column: bool,
    pure: bool,
) -> bool {
    let none = Links::default();
    let (links, hover_delay) = if pure {
        (&none, None)
    } else {
        (links, hover_delay)
    };
    let width = ui.available_width();
    let mat = if column {
        let (r, _) = ui.allocate_exact_size(vec2(width, width * COLUMN_ASPECT), Sense::hover());
        Mat::column(r.min, width)
    } else {
        let (pitch, _) = ui.allocate_exact_size(vec2(width, width * PITCH_ASPECT), Sense::hover());
        // the bench under the pitch: a well with its name running up the left like the zones
        ui.add_space(8.0);
        let (bench, _) = ui.allocate_exact_size(vec2(width, art::bench_h(width)), Sense::hover());
        Mat::row(pitch, bench.top())
    };
    let (pitch, shown, bench) = (mat.pitch, mat.shown, mat.bench);
    let dragging = egui::DragAndDrop::payload::<usize>(ui.ctx()).map(|a| *a);
    // a card dragged out of the catalogue
    let incoming = egui::DragAndDrop::payload::<Card>(ui.ctx());
    let pointer = ui.ctx().pointer_latest_pos();
    paint_pitch(&ui.painter().with_clip_rect(shown), pitch, true);
    // the heat zone goes over the pitch and under the cards, once we know whose it is
    let heat_at = ui.painter().add(egui::Shape::Noop);
    // the link lines over it, under the cards
    let lines_at = ui.painter().add(egui::Shape::Noop);
    let card = card_size(pitch.width());
    // the selection: the card in hand, else the card clicked last (while it is on the board)
    let select_id = Id::unique("board-select");
    let clicked_card = ui
        .data(|d| d.get_temp::<Option<u32>>(select_id))
        .flatten()
        .filter(|n| (0..16).any(|s| board.number(s) == Some(*n)));
    // no links known: no selection, the board as it was
    let has_links = links.affinity.is_some() || !links.club.is_empty();
    let selected = dragging
        .and_then(|d| board.number(d))
        .or(clicked_card)
        .filter(|_| has_links);
    // the group (box or Ctrl-click): card numbers, which survive a swap, of cards still on the pitch
    let group_on = ui.data(|d| d.get_temp::<bool>(group_on_id())) == Some(true);
    let mut group: Vec<u32> = ui
        .data(|d| d.get_temp::<Vec<u32>>(group_id()))
        .unwrap_or_default()
        .into_iter()
        .filter(|n| {
            // a card dropped on the keeper's spot leaves the group once let go
            group_on
                && (0..11).any(|s| {
                    board.number(s) == Some(*n) && (dragging.is_some() || groupable(board, s))
                })
        })
        .collect();
    let in_group =
        |b: &Board, g: &[u32], s: usize| s < 11 && b.number(s).is_some_and(|n| g.contains(&n));
    // a drag of a card outside the group lets the group go
    if dragging.is_some_and(|d| d < 16 && board.slots[d].is_some() && !in_group(board, &group, d)) {
        group.clear();
    }
    let group_drag = group.len() >= 2 && dragging.is_some_and(|d| in_group(board, &group, d));

    let mut changed = false;
    if column {
        mat_column(ui, bench);
    } else {
        mat_bench(ui, bench);
    }
    // a pitch card in hand moves live: the table follows it every frame, as the C panels' board_move, so the game
    // sees the card slide and rub, not one jump on release; its spot before the lift is kept for a swap
    let origin_id = Id::unique("board-origin");
    if dragging.is_some() || incoming.is_some() {
        // the first frame of a board drag: the grab is measured on the card's resting rect, before it moves
        if let Some(from) = dragging.filter(|&d| d < 16 && board.slots[d].is_some())
            && ui.data(|d| d.get_temp::<Vec2>(grab_id())).is_none()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            let rest = mat.slot_rects(&board.spots)[from];
            ui.data_mut(|d| d.insert_temp(grab_id(), (rest.center() - press) / rest.size()));
        }
    } else {
        ui.data_mut(|d| d.remove::<Vec2>(grab_id()));
    }
    let grab = ui
        .data(|d| d.get_temp::<Vec2>(grab_id()))
        .unwrap_or(Vec2::ZERO);
    // a group in hand: every member's spot at the lift, kept until it is let go (or Esc)
    let group_origin_id = Id::unique("board-group-origin");
    let group_origin: Vec<(usize, Pos2)> = if group_drag {
        let o = ui.data_mut(|d| {
            d.get_temp_mut_or_insert_with(group_origin_id, || {
                (0..11)
                    .filter(|&s| in_group(board, &group, s))
                    .map(|s| (s, board.spots[s]))
                    .collect::<Vec<_>>()
            })
            .clone()
        });
        if ui.input(|i| i.pointer.any_released()) {
            ui.data_mut(|d| d.remove::<Vec<(usize, Pos2)>>(group_origin_id));
        }
        o
    } else {
        changed |= ui
            .data_mut(|d| d.remove_temp::<Vec<(usize, Pos2)>>(group_origin_id))
            .is_some();
        Vec::new()
    };
    let origin = match dragging.filter(|&d| d < 11 && board.slots[d].is_some()) {
        Some(from) => {
            let o = ui.data_mut(|d| {
                *d.get_temp_mut_or_insert_with(origin_id, || (from, board.spots[from]))
            });
            if let Some(pt) = pointer.map(|p| drag_centre(p, grab, card))
                && shown.contains(pt)
            {
                let spot = spot_at(pitch, pt);
                if group_drag {
                    // the whole group by one offset, clamped once so it stops together at an edge
                    let f = field(pitch);
                    let want = pos2((pt.x - f.left()) / f.width(), (pt.y - f.top()) / f.height());
                    let spots: Vec<Pos2> = group_origin.iter().map(|m| m.1).collect();
                    let to = group_spots(&spots, want - o.1);
                    for (&(m, _), at) in group_origin.iter().zip(to) {
                        if board.spots[m] != at {
                            board.spots[m] = at;
                            changed = true;
                        }
                    }
                } else if spot != board.spots[from] {
                    board.spots[from] = spot;
                    changed = true;
                }
            }
            // let go anywhere: one last change, so the board's table is rebuilt and logged
            if ui.input(|i| i.pointer.any_released()) {
                changed = true;
                ui.data_mut(|d| d.remove::<(usize, Pos2)>(origin_id));
            }
            Some(o.1)
        }
        None => {
            // Esc clears egui's payload without a release: the card moved live, so rebuild the table
            changed |= ui
                .data_mut(|d| d.remove_temp::<(usize, Pos2)>(origin_id))
                .is_some();
            None
        }
    };
    // a bench card in hand rubs inside its box: one signed scalar from the pointer's travel since the press
    let rub = dragging
        .filter(|&d| d >= 11 && board.slots[d].is_some())
        .and_then(|d| {
            let (p, o) = (pointer?, ui.input(|i| i.pointer.press_origin())?);
            let v = p - o;
            let t = if v.x.abs() >= v.y.abs() { v.x } else { v.y };
            Some((d, (t / card.x).clamp(-1.0, 1.0)))
        })
        .filter(|_| !ui.input(|i| i.pointer.any_released()));
    if board.rub != rub {
        board.rub = rub;
        changed = true;
    }
    let rects = mat.slot_rects(&board.spots);
    // the card in hand a card's width or more off the board: let go there, it is taken off (a bench rub stays
    // within a card's width of its box)
    // The pop-out board (`pure`) has a footer under it inside its window, and a card low on the pitch is a short drag
    // from it: there only a release outside the window's client area takes a card off.
    let off = pointer.is_some_and(|p| {
        if pure {
            !ui.ctx().content_rect().contains(p)
        } else {
            !shown
                .union(bench)
                .expand(card.x)
                .contains(drag_centre(p, grab, card))
        }
    });
    let mut removed: Option<(usize, Pos2)> = None;

    let hover_id = Id::unique("board-hover");
    // the card clicked last, pinned in the info box ([`next_pin`])
    let pin_id = Id::unique("board-pin");
    let mut hovered_card = None;
    let mut drop_target = None;
    // the swap target is the card under the held card's centre (as drawn), not under the pointer
    // a group in hand never swaps: its cards lie where shown
    let target = pointer.filter(|_| !group_drag).and_then(|p| {
        target_slot(
            &rects,
            |s| s >= 11 || board.slots[s].is_some(),
            dragging,
            drag_centre(p, grab, card),
        )
    });
    // the box: a drag on open pitch, interacted before the cards so they stay on top of it; a click there lets the
    // group go
    let box_start_id = Id::unique("board-box");
    let mut box_area = None;
    if group_on {
        let r = ui.interact(shown, Id::unique("board-box"), Sense::click_and_drag());
        if r.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin())
        {
            ui.data_mut(|d| d.insert_temp(box_start_id, p));
        }
        let start = ui.data(|d| d.get_temp::<Pos2>(box_start_id));
        if let (Some(a), Some(b)) = (start, pointer) {
            box_area = Some(Rect::from_two_pos(a, b));
        }
        if r.drag_stopped() {
            ui.data_mut(|d| d.remove::<Pos2>(box_start_id));
            if let Some(area) = box_area.take() {
                group = boxed(&rects, |s| groupable(board, s), area)
                    .into_iter()
                    .filter_map(|s| board.number(s))
                    .collect();
            }
        } else if !r.dragged() {
            box_area = None;
        }
        if r.clicked() {
            group.clear();
        }
    }
    for (slot, &rect) in rects.iter().enumerate() {
        // no spot boxes on the pitch: an empty pitch slot is no place and takes no drop
        if slot < 11 && board.slots[slot].is_none() {
            continue;
        }
        let resp = ui.interact(
            rect,
            Id::unique(("board-slot", slot)),
            Sense::click_and_drag(),
        );
        let p = ui.painter();
        let hovered_by_drag = (dragging.is_some() || incoming.is_some()) && target == Some(slot);
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
                        Fit::of(c.aptitude.at(board.spots[slot])) == Fit::Out
                    } else {
                        zone(board.spots[slot]) != c.pos
                    };
                let corners = card_face(
                    p,
                    rect,
                    c,
                    Look {
                        ghost: is_dragged,
                        selected: selected == Some(c.number) || in_group(board, &group, slot),
                        target: hovered_by_drag,
                        warn,
                        ..Look::default()
                    },
                );
                if let Some(s) = selected
                    && !is_dragged
                    && !links.grouped(s, c.number)
                    && !in_group(board, &group, slot)
                {
                    p.rect_filled(rect, corners, Color32::from_black_alpha(140));
                }
                if pure && (resp.is_pointer_button_down_on() || is_dragged || resp.clicked()) {
                    ui.data_mut(|d| d.insert_temp(touched_id(), c.clone()));
                }
                if resp.hovered() && dragging.is_none() && !pure {
                    hovered_card = Some((slot, ci, rect));
                }
                // Ctrl- or Shift-click: the pitch card joins or leaves the group, nothing shows big; a plain click on
                // a card outside the group lets the group go
                let toggle = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                if resp.clicked() && group_on && groupable(board, slot) && toggle {
                    if let Some(i) = group.iter().position(|&n| n == c.number) {
                        group.remove(i);
                    } else {
                        group.push(c.number);
                    }
                } else if resp.clicked() && group_on && !toggle && !in_group(board, &group, slot) {
                    group.clear();
                }
                // a click shows the card big and selects it (and lets both go); only the big card turns over
                if resp.clicked() && !pure && !(group_on && groupable(board, slot) && toggle) {
                    let no = c.number;
                    ui.data_mut(|d| {
                        let f = d.get_temp_mut_or_default::<Option<u32>>(pin_id);
                        *f = next_pin(*f, PinEvent::Click(no));
                        let s = d.get_temp_mut_or_default::<Option<u32>>(select_id);
                        *s = if clicked_card == Some(no) {
                            None
                        } else {
                            Some(no)
                        };
                    });
                }
                if is_dragged && let Some(pt) = pointer {
                    // the lifted card follows the pointer above everything
                    let lp = ui.ctx().layer_painter(egui::LayerId::new(
                        Order::Tooltip,
                        Id::unique("board-lift"),
                    ));
                    let at = drag_centre(pt, grab, card);
                    card_face(
                        &lp,
                        Rect::from_center_size(at, card * 1.08),
                        c,
                        Look {
                            lifted: true,
                            held: true,
                            ghost: off,
                            ..Look::default()
                        },
                    );
                }
            }
            // an empty bench box: the artwork's numbered box shows; lit while a card is held over it
            None if hovered_by_drag => {
                p.rect_stroke(
                    mat.bench_box(slot - 11),
                    0,
                    Stroke::new(3.0, theme::AMBER),
                    StrokeKind::Inside,
                );
            }
            None => {}
        }
    }

    // a group let go: on open pitch it lies where shown; over the bench or off the board it goes back to its spots
    // from before the lift (no group take-off, no group benching)
    if group_drag
        && ui.input(|i| i.pointer.any_released())
        && egui::DragAndDrop::take_payload::<usize>(ui.ctx()).is_some()
    {
        // by the pointer, not the card's centre: a card held by its lower edge has its centre on the pitch while the
        // pointer is over the bench already
        if !pointer.is_some_and(|p| shown.contains(p)) {
            for &(m, at) in &group_origin {
                board.spots[m] = at;
            }
        }
        changed = true;
    }
    // dragged off the board and let go: taken off (a pitch card's spot from before the lift)
    if let Some(from) = dragging.filter(|&d| board.slots[d].is_some())
        && off
        && ui.input(|i| i.pointer.any_released())
        && egui::DragAndDrop::take_payload::<usize>(ui.ctx()).is_some()
    {
        let spot = origin
            .or(board.spots.get(from).copied())
            .unwrap_or_default();
        if ui.data(|d| d.get_temp::<bool>(keep_id())) == Some(true) {
            // a match runs: the card goes back to its spot from before the lift (a bench card never moved)
            if from < 11 {
                board.spots[from] = spot;
            }
            changed = true;
            let n = board.number(from).unwrap_or(0);
            ui.data_mut(|d| d.insert_temp(kept_id(), n));
        } else {
            removed = Some((from, spot));
        }
    }
    // a release: onto the target card (swap, or put the catalogue card there), else on open pitch
    if ui.input(|i| i.pointer.any_released()) {
        if let (Some(from), Some(t)) = (dragging, target)
            && egui::DragAndDrop::take_payload::<usize>(ui.ctx()).is_some()
        {
            if let Some(o) = origin {
                // the other card goes where this one was lifted from
                board.spots[from] = o;
            }
            board.swap(from, t);
            changed = true;
        } else if let (Some(c), Some(t)) = (incoming.clone(), target)
            && egui::DragAndDrop::take_payload::<Card>(ui.ctx()).is_some()
        {
            changed |= board.put(&c, t);
        }
    }

    // dropped on open pitch: the card lies where it was shown, from the pitch, the bench or the catalogue
    if (dragging.is_some() || incoming.is_some())
        && pointer.is_some_and(|p| shown.contains(drag_centre(p, grab, card)))
        && ui.input(|i| i.pointer.any_released())
        && let Some(pt) = pointer
    {
        let spot = spot_at(pitch, drag_centre(pt, grab, card));
        if let Some(from) = dragging
            && egui::DragAndDrop::take_payload::<usize>(ui.ctx()).is_some()
        {
            changed |= board.move_to(from, spot);
        } else if let Some(c) = incoming.clone()
            && egui::DragAndDrop::take_payload::<Card>(ui.ctx()).is_some()
        {
            changed |= board.put_free(&c, spot);
        }
    }
    // a catalogue card in hand follows the pointer too
    if let (Some(c), Some(pt)) = (incoming.as_ref(), pointer) {
        let lp = ui
            .ctx()
            .layer_painter(egui::LayerId::new(Order::Tooltip, Id::unique("board-lift")));
        card_face(
            &lp,
            Rect::from_center_size(drag_centre(pt, grab, card), card * 1.08),
            c,
            Look {
                lifted: true,
                held: true,
                ..Look::default()
            },
        );
    }

    // the box being drawn, over the cards
    if let Some(area) = box_area {
        let p = ui.painter().with_clip_rect(shown);
        p.rect_filled(area, 0, theme::AMBER.gamma_multiply(0.12));
        p.rect_stroke(area, 0, Stroke::new(2.0, theme::AMBER), StrokeKind::Inside);
    }
    // the link lines between pitch cards: the selection's group last, so it lies on top
    let mut back = Vec::new();
    let mut front = Vec::new();
    for i in 0..11 {
        for j in i + 1..11 {
            let (Some(a), Some(b)) = (board.number(i), board.number(j)) else {
                continue;
            };
            let Some(stage) = Stage::of(links.link(a, b)) else {
                continue;
            };
            // the group's lines stay bright beside the selection's
            let lit = selected.is_none_or(|s| s == a || s == b)
                || in_group(board, &group, i)
                || in_group(board, &group, j);
            let (to, alpha) = if lit {
                (&mut front, 1.0)
            } else {
                (&mut back, 0.3)
            };
            link_line(to, rects[i].center(), rects[j].center(), stage, alpha);
        }
    }
    back.append(&mut front);
    ui.painter().set(lines_at, egui::Shape::Vec(back));
    // a selection lets go by itself `SELECT_HOLD` seconds after the last touch of the board: a card in hand, a press
    // on the board, the mouse on the selected card, a key. A drag in progress keeps it, whatever its length.
    let now = ui.input(|i| i.time);
    let touch_id = Id::unique("board-select-touch");
    let on_board = |p: Pos2| shown.union(bench).contains(p);
    let over_selected = clicked_card.is_some_and(|s| {
        (0..16).any(|i| board.number(i) == Some(s) && pointer.is_some_and(|p| rects[i].contains(p)))
    });
    let over_group = (0..11)
        .any(|i| in_group(board, &group, i) && pointer.is_some_and(|p| rects[i].contains(p)));
    let touched = dragging.is_some()
        || box_area.is_some()
        || over_selected
        || over_group
        || ui.input(|i| {
            i.events.iter().any(|e| match e {
                egui::Event::Key { .. } => true,
                egui::Event::PointerButton { pos, .. } => on_board(*pos),
                _ => false,
            })
        });
    if touched {
        ui.data_mut(|d| d.insert_temp(touch_id, now));
        // a frame when the hold runs out, though nothing else happens
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(SELECT_HOLD));
    } else if clicked_card.is_some() || !group.is_empty() {
        let last = ui.data(|d| d.get_temp::<f64>(touch_id)).unwrap_or(now);
        if now - last >= SELECT_HOLD {
            ui.data_mut(|d| d.insert_temp(select_id, None::<u32>));
            group.clear();
            ui.ctx().request_repaint();
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(
                    SELECT_HOLD - (now - last),
                ));
        }
    }
    // the list while the card is in hand or the mouse is on the board, drawn by [`info_panes`] in the board's column
    if let Some(s) = selected
        && (dragging.is_some() || pointer.is_some_and(|p| shown.union(bench).contains(p)))
    {
        let at = (0..16)
            .find(|&i| board.number(i) == Some(s))
            .map_or(shown, |i| rects[i]);
        ui.data_mut(|d| d.insert_temp(partners_id(), (s, at)));
    }

    // the info box (#333): the card clicked last stays pinned where it opened, the card the mouse rests on (past the
    // hover delay) shows in a preview at the cursor ([`boxes`])
    let slot_of = |n: u32| (0..16).find(|&s| board.number(s) == Some(n));
    let mut pin = ui
        .data(|d| d.get_temp::<Option<u32>>(pin_id))
        .flatten()
        .filter(|&n| slot_of(n).is_some()); // the card left the table: the pin goes with it
    let mut rested = None;
    match hovered_card {
        Some((slot, ci, _)) => {
            let since = ui.data_mut(|d| {
                let e = d.get_temp_mut_or_insert_with(hover_id, || (slot, now));
                if e.0 != slot {
                    *e = (slot, now);
                }
                e.1
            });
            // None: CARD HOVER DELAY off, no preview
            let left = hover_delay.map(|d| d as f64 - (now - since));
            if left.is_some_and(|l| l <= 0.0) {
                rested = Some(board.cards[ci].number);
            } else if let Some(left) = left {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(left));
            }
        }
        None => {
            ui.data_mut(|d| d.remove::<(usize, f64)>(hover_id));
            // a press off the cards and off the pinned card (as drawn last frame) lets it go
            let big = ui.ctx().memory(|m| m.area_rect(Id::unique("board-big")));
            let off_big = ui.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .press_origin()
                        .is_some_and(|p| !big.is_some_and(|r| r.contains(p)))
            });
            if off_big {
                pin = next_pin(pin, PinEvent::Close);
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(pin_id, pin));
    let busy = dragging.is_some() || incoming.is_some() || group_drag || box_area.is_some();
    let (preview, pinned) = boxes(pin, rested, busy);
    let ask = |n: u32| {
        slot_of(n).and_then(|s| {
            board.slots[s].map(|ci| PaneAsk {
                card: board.cards[ci].clone(),
                at: rects[s],
            })
        })
    };
    if let Some(a) = pinned.and_then(ask) {
        ui.data_mut(|d| d.insert_temp(pane_id(), a));
    }
    if let (Some(a), Some(p)) = (preview.and_then(ask), pointer) {
        ui.data_mut(|d| d.insert_temp(preview_id(), (a, p)));
    }
    let shown_big = preview
        .or(pinned)
        .and_then(slot_of)
        .and_then(|s| board.slots[s]);

    // the heat zone of the card in hand (or shown big), and the drop target's fit
    let held = dragging
        .and_then(|d| board.slots[d])
        .or(shown_big)
        .map(|ci| board.cards[ci].aptitude);
    if let Some(apt) = held {
        if apt.known() {
            let size = (pitch.width() * 0.025).clamp(9.0, 12.0);
            let zone = heat_zone(ui.painter(), pitch, field(pitch), &apt, size);
            ui.painter().set(heat_at, zone);
            if let Some((slot, rect)) = drop_target
                && slot < 11
            {
                let fit = Fit::of(apt.at(board.spots[slot]));
                let lp = ui
                    .ctx()
                    .layer_painter(egui::LayerId::new(Order::Tooltip, Id::unique("board-fit")));
                fit_badge(&lp, pos2(rect.center().x, rect.bottom() + 4.0), fit, lang);
            }
        } else {
            // a card without ratings: say so instead of an empty heat zone
            let p = ui.painter();
            let g = p.layout_no_wrap(tr!(lang, "board.no_data"), theme::bold(13.0), theme::TEXT);
            let r = Rect::from_center_size(
                pos2(shown.center().x, shown.top() + 22.0),
                g.size() + vec2(20.0, 8.0),
            );
            p.rect_filled(r, 12, Color32::from_black_alpha(190));
            p.galley(r.center() - g.size() / 2.0, g, theme::TEXT);
        }
    }
    if let Some((slot, spot)) = removed
        && let Some(ci) = board.slots[slot].take()
    {
        let card = board.cards[ci].clone();
        let whole = shown.union(bench);
        let at = pointer.map_or(Pos2::ZERO, |p| (p - whole.min).to_pos2());
        let size = whole.size();
        ui.data_mut(|d| {
            d.insert_temp(
                removed_id(),
                Removed {
                    card,
                    slot,
                    spot,
                    at,
                    size,
                },
            )
        });
        changed = true;
    }
    ui.data_mut(|d| d.insert_temp(group_id(), group));
    changed
}

/// One row of a partner list: the link's line (when one is drawn), the name, the affinity dots.
pub fn partner_row(ui: &mut Ui, name: &str, link: Option<Stage>, affinity: Option<u8>, width: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
    let p = ui.painter();
    if let Some(stage) = link {
        let mut v = Vec::new();
        let y = r.center().y;
        link_line(
            &mut v,
            pos2(r.left() + 2.0, y),
            pos2(r.left() + 26.0, y),
            stage,
            1.0,
        );
        p.extend(v);
    }
    let g = fit(p, name, theme::body(12.0), theme::TEXT, width - 32.0 - 50.0);
    p.galley(
        pos2(r.left() + 32.0, r.center().y - g.size().y / 2.0),
        g,
        theme::TEXT,
    );
    if let Some(v) = affinity {
        affinity_dots(p, pos2(r.right() - 46.0, r.center().y), v);
    }
}

/// The selected card's partners in the board's column, the top left at `at`: team-mates it is linked with (a line) or
/// has the best affinity with, then the best partners not on the team (at most six).
fn partner_list(ui: &Ui, board: &Board, links: &Links, sel: u32, at: Pos2, lang: Lang) {
    let on: Vec<&Card> = board
        .slots
        .iter()
        .flatten()
        .map(|&ci| &board.cards[ci])
        .collect();
    let Some(me) = on.iter().find(|c| c.number == sel) else {
        return;
    };
    let mut mates: Vec<(&Card, u8, Option<u8>)> = on
        .iter()
        .filter(|c| c.number != sel && links.grouped(sel, c.number))
        .map(|c| (*c, links.link(sel, c.number), links.affinity(sel, c.number)))
        .collect();
    mates.sort_by_key(|&(c, v, a)| (std::cmp::Reverse((v, a)), c.number));
    let away: Vec<(&str, u8)> = links
        .affinity
        .iter()
        .flat_map(|a| a.partners(sel))
        .filter(|(p, v)| *v >= 4 && !p.cards.iter().any(|n| on.iter().any(|c| c.number == *n)))
        .map(|(p, v)| (p.name.as_str(), v))
        .collect();
    let w = 230.0;
    egui::Area::new(Id::unique("board-partners"))
        .order(Order::Foreground)
        .fixed_pos(at)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::DEEP)
                .stroke(Stroke::new(1.0, theme::RIM_HI))
                .corner_radius(theme::RADIUS_BIG)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(w);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let head = |ui: &mut Ui, t: String| {
                        ui.label(
                            egui::RichText::new(t)
                                .font(theme::display(13.0))
                                .color(theme::TEXT_DIM),
                        );
                    };
                    ui.label(
                        egui::RichText::new(tr!(lang, "board.links_of", name = me.short_name()))
                            .font(theme::display(16.0))
                            .color(theme::TEXT),
                    );
                    head(ui, tr!(lang, "board.on_team"));
                    if mates.is_empty() {
                        ui.label(
                            egui::RichText::new(tr!(lang, "board.no_links"))
                                .font(theme::body(12.0))
                                .color(theme::TEXT_FAINT),
                        );
                    }
                    for (c, v, a) in &mates {
                        partner_row(ui, c.short_name(), Stage::of(*v), *a, w);
                    }
                    if !away.is_empty() {
                        head(ui, tr!(lang, "board.not_on_team"));
                        for (name, v) in away.iter().take(6) {
                            partner_row(ui, name, None, Some(*v), w);
                        }
                        if away.len() > 6 {
                            ui.label(
                                egui::RichText::new(tr!(lang, "board.more", n = away.len() - 6))
                                    .font(theme::body(11.0))
                                    .color(theme::TEXT_FAINT),
                            );
                        }
                    }
                });
        });
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

/// The pitch, attacking upwards. With `mat`, the formation board's: the project's artwork of the cabinet's mat
/// ([`mat_pitch`]); without, the card back's small plain pitch.
fn paint_pitch(p: &Painter, pitch: Rect, mat: bool) {
    if mat {
        return mat_pitch(p, pitch);
    }
    let bands = 10;
    let h = pitch.height() / bands as f32;
    p.rect_filled(pitch, theme::RADIUS, theme::PITCH);
    for i in (0..bands).step_by(2) {
        let r = Rect::from_min_size(pitch.min + vec2(0.0, h * i as f32), vec2(pitch.width(), h));
        let cr = egui::CornerRadius {
            nw: if i == 0 { theme::RADIUS } else { 0 },
            ne: if i == 0 { theme::RADIUS } else { 0 },
            sw: 0,
            se: 0,
        };
        p.rect_filled(r, cr, theme::PITCH_STRIPE);
    }
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

/// The formation board's pitch: the project's artwork ([`art`]), mown stripes, the yellower midfield band, the zone names
/// FW, MF and DF down the middle, the markings in chalk and the keeper's GK box half under the own goal line.
fn mat_pitch(p: &Painter, pitch: Rect) {
    use std::f32::consts::{FRAC_PI_2, PI};
    p.rect_filled(pitch, theme::RADIUS_BIG, art::GRASS);
    let f = field(pitch);
    let sw = f.width() / art::STRIPES as f32;
    let band = art::rect(pitch, [0.0, art::MF_BAND[0], art::W, art::MF_BAND[1]]);
    for i in 0..art::STRIPES {
        let x = f.left() + sw * i as f32;
        let r = Rect::from_x_y_ranges(x..=x + sw, f.y_range());
        p.rect_filled(r, 0, art::MOWN[i % 2]);
        p.rect_filled(r.intersect(band), 0, art::MOWN_MF[i % 2]);
    }
    let size = art::len(pitch, art::CAPS) / 0.7; // the display font's capitals are about 0.7 of its size
    for (name, y, col) in art::LABELS {
        p.text(
            art::at(pitch, art::W / 2.0 + 5.0, y),
            Align2::CENTER_CENTER,
            name,
            theme::display(size),
            col,
        );
    }
    let line = Stroke::new(art::len(pitch, art::LINE).max(1.0), art::CHALK);
    p.rect_stroke(f, 0, line, StrokeKind::Middle);
    p.line_segment([f.left_center(), f.right_center()], line);
    p.circle_stroke(f.center(), art::len(pitch, art::CIRCLE), line);
    p.circle_filled(f.center(), art::len(pitch, art::CENTRE_SPOT), art::CHALK);
    let corner = art::len(pitch, art::CORNER);
    for (c, a) in [
        (f.left_top(), 0.0),
        (f.right_top(), FRAC_PI_2),
        (f.right_bottom(), PI),
        (f.left_bottom(), -FRAC_PI_2),
    ] {
        p.add(egui::Shape::line(
            art::arc(c, corner, a, a + FRAC_PI_2),
            line,
        ));
    }
    let [bl, br, bd] = art::BOX;
    let [gl, gr, gd] = art::GOAL_AREA;
    // the penalty arc: the part of the circle round the spot outside the box
    let half = ((bd - art::SPOT) / art::CIRCLE).acos();
    for (goal, dir) in [(art::FIELD[1], 1.0), (art::FIELD[3], -1.0)] {
        let rect = |l, r, d: f32| {
            Rect::from_two_pos(art::at(pitch, l, goal), art::at(pitch, r, goal + d * dir))
        };
        p.rect_stroke(rect(bl, br, bd), 0, line, StrokeKind::Middle);
        p.rect_stroke(rect(gl, gr, gd), 0, line, StrokeKind::Middle);
        let spot = art::at(pitch, art::W / 2.0, goal + art::SPOT * dir);
        let mid = if dir > 0.0 { FRAC_PI_2 } else { -FRAC_PI_2 };
        p.add(egui::Shape::line(
            art::arc(spot, art::len(pitch, art::CIRCLE), mid - half, mid + half),
            line,
        ));
    }
    let gk = art::rect(pitch, art::GK);
    p.rect_filled(gk, 0, art::GK_BOX);
    p.text(
        gk.center(),
        Align2::CENTER_CENTER,
        "GK",
        theme::display(size),
        art::GK_TEXT,
    );
}

/// The formation board's bench: the artwork's strip (a cream line, the soil, SUB, the five numbered boxes and the
/// Forza-WCCF badge), drawn as the source file has it, untranslated.
fn mat_bench(ui: &Ui, bench: Rect) {
    let p = ui.painter();
    let at = |x, y| art::bench_at(bench, x, y);
    let len = |v| art::len(bench, v);
    p.rect_filled(bench, 0, art::SOIL);
    p.rect_filled(
        Rect::from_min_max(bench.min, at(art::W, art::BENCH_LINE)),
        0,
        art::CREAM,
    );
    let [l, t, r, b] = art::SUB;
    let sub = Rect::from_min_max(at(l, t), at(r, b));
    p.rect_filled(sub, 0, art::CREAM);
    let g = p.layout_no_wrap("SUB".into(), theme::display(len(78.0)), art::SOIL);
    let pos = sub.center() + vec2(-g.size().y / 2.0, g.size().x / 2.0);
    p.add(egui::epaint::TextShape::new(pos, g, art::SOIL).with_angle(-std::f32::consts::FRAC_PI_2));
    for i in 0..5 {
        mat_box(p, art::slot(bench, i), i, len(1.0));
    }
    let [l, t, r, b] = art::BADGE;
    egui::Image::new(egui::include_image!(
        "../../../assets/logo/forza-wccf-badge-128.png"
    ))
    .paint_at(ui, Rect::from_min_max(at(l, t), at(r, b)));
}

/// A bench box `r`, numbered `i + 1`, `px` points a picture pixel.
fn mat_box(p: &Painter, r: Rect, i: usize, px: f32) {
    p.rect_filled(r, 0, art::CREAM);
    p.rect_filled(r.shrink(art::SLOT_RIM * px), 0, art::SLOT_FILL);
    p.text(
        r.center(),
        Align2::CENTER_CENTER,
        (i + 1).to_string(),
        theme::display(art::DIGIT * px / 0.7),
        art::DIGITS,
    );
}

/// The pop-out board's bench column: the soil, a cream line down its left edge, the Forza-WCCF crest, SUB and the
/// five numbered boxes stacked, in the bench strip's colours and box size.
fn mat_column(ui: &Ui, bench: Rect) {
    let p = ui.painter();
    let px = bench.width() / art::COLUMN_W;
    let at = |[l, t, r, b]: [f32; 4]| {
        Rect::from_min_max(art::column_at(bench, l, t), art::column_at(bench, r, b))
    };
    p.rect_filled(bench, 0, art::SOIL);
    p.rect_filled(
        bench.with_max_x(bench.left() + art::COLUMN_LINE * px),
        0,
        art::CREAM,
    );
    let sub = at(art::COLUMN_SUB);
    p.rect_filled(sub, 0, art::CREAM);
    p.text(
        sub.center(),
        Align2::CENTER_CENTER,
        "SUB",
        theme::display(78.0 * px),
        art::SOIL,
    );
    for i in 0..5 {
        mat_box(p, art::column_slot(bench, i), i, px);
    }
    egui::Image::new(egui::include_image!(
        "../../../assets/logo/forza-wccf-256.png"
    ))
    .paint_at(ui, at(art::CREST));
}

/// The card back's position map: a tile per position area ([`heat_zone`]), or "no position data" on a small pitch.
pub fn aptitude_map(p: &Painter, rect: Rect, apt: &Aptitude, lang: Lang) {
    if apt.known() {
        let field = rect.shrink((rect.width() * 0.025).max(3.0));
        let size = (rect.width() * 0.055).clamp(8.0, 12.0);
        p.add(heat_zone(p, rect, field, apt, size));
    } else {
        paint_pitch(p, rect, false);
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

/// Room around the info pane's face: the frame's margins, and the large tier's caption under it (name, facts, roles,
/// stats).
const BIG_FRAME: f32 = 22.0;
const BIG_CAPTION: f32 = 150.0;
/// The FRONT | BACK switch under the big card: its height and the gap over it.
pub const FLIP_ROW: f32 = 34.0 + 6.0;

/// The strip pane's height at `ppp` pixels a point: the frame's margins, the card's face and the FRONT | BACK switch
/// under it.
pub fn strip_pane(ppp: f32) -> f32 {
    BIG_FRAME + crate::sizing::hover_face(ppp).y + FLIP_ROW
}

/// What the board asks [`info_panes`] to show this frame: a card in the info box and where it lies.
#[derive(Clone)]
struct PaneAsk {
    card: Card,
    at: Rect,
}

/// The pinned card asked for.
fn pane_id() -> Id {
    Id::unique("board-pane-ask")
}

/// The previewed card asked for, and the cursor.
fn preview_id() -> Id {
    Id::unique("board-preview-ask")
}

/// What changes the info box's pinned card (#333).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinEvent {
    /// A plain click on a card.
    Click(u32),
    /// Esc, a press off the cards and the pinned card, or the card leaving the table.
    Close,
}

/// The pinned card after `e`: a click pins the card, a click on the pinned card lets it go; a close lets it go.
pub fn next_pin(pin: Option<u32>, e: PinEvent) -> Option<u32> {
    match e {
        PinEvent::Click(n) if pin != Some(n) => Some(n),
        _ => None,
    }
}

/// What the info box shows this frame: (the preview's card, the pinned card). Nothing while a card, a group or a box is
/// in hand (`busy`); the preview is the card rested on past the hover delay (`rested`), unless it is the pinned one.
fn boxes(pin: Option<u32>, rested: Option<u32>, busy: bool) -> (Option<u32>, Option<u32>) {
    if busy {
        return (None, None);
    }
    (rested.filter(|&n| Some(n) != pin), pin)
}

/// Lets the pinned card go (Esc on the game window).
pub fn close_pin(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(Id::unique("board-pin"), None::<u32>));
}

/// The hover preview's caption width: the six stats' columns with their Japanese names ([`caption`]).
const PREVIEW_CAPTION: f32 = 280.0;

/// The hover preview's size at `ppp`, at most `width` wide: the frame, the strip's face and the caption beside it; no
/// switch row.
pub fn preview_size(ppp: f32, width: f32) -> Vec2 {
    let face = crate::sizing::hover_face(ppp);
    vec2(
        (BIG_FRAME + face.x + 10.0 + PREVIEW_CAPTION).min(width),
        BIG_FRAME + face.y,
    )
}

/// The hover preview's column: the board's, `pitch` wide, from its top to the board's foot at `board_bottom`, so the
/// preview keeps off the buttons and the hint under the board (and the pinned pane docked under them).
pub fn preview_column(pitch: Rect, board_bottom: f32) -> Rect {
    Rect::from_min_max(pitch.min, pos2(pitch.right(), board_bottom))
}

/// Where the hover preview (`size`) goes in the board's `column` ([`preview_column`]): 16 points right of and under the `cursor`, on the
/// first side of the hovered `card` where it fits whole (under, over, right, left), following the cursor along that
/// side (turned to the cursor's left when it would leave the column) and kept in the column; so it never covers the
/// card, and never leaves the column for the game's picture or the deck.
pub fn preview_place(column: Rect, cursor: Pos2, card: Rect, size: Vec2) -> Rect {
    const GAP: f32 = 16.0;
    let fit = |v: f32, lo: f32, hi: f32, len: f32| v.clamp(lo, (hi - len).max(lo));
    // along an axis: after the cursor, or before it when that leaves the column
    let along = |c: f32, lo: f32, hi: f32, len: f32| {
        let v = if c + GAP + len <= hi {
            c + GAP
        } else {
            c - GAP - len
        };
        fit(v, lo, hi, len)
    };
    let x = along(cursor.x, column.left(), column.right(), size.x);
    let y = along(cursor.y, column.top(), column.bottom(), size.y);
    let tries = [
        pos2(x, (card.bottom() + 8.0).max(cursor.y + GAP)),
        pos2(x, (card.top() - 8.0).min(cursor.y - GAP) - size.y),
        pos2((card.right() + 8.0).max(cursor.x + GAP), y),
        pos2((card.left() - 8.0).min(cursor.x - GAP) - size.x, y),
    ];
    tries
        .into_iter()
        .map(|m| Rect::from_min_size(m, size))
        .find(|r| column.contains_rect(*r))
        .unwrap_or_else(|| {
            Rect::from_min_size(
                pos2(
                    fit(x, column.left(), column.right(), size.x),
                    fit(y, column.top(), column.bottom(), size.y),
                ),
                size,
            )
        })
}

/// The selected card's partner list asked for: its number and where it lies.
fn partners_id() -> Id {
    Id::unique("board-partners-ask")
}

/// The info pane's two sizes: large (the face at its picture's own pixels, the position map beside it, the caption
/// under them) or a strip (a smaller face, the caption beside it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Large,
    Strip,
}

/// Where an info pane goes in the board's column, `pitch` wide: docked under the hint (its foot at `hint_bottom`) when
/// the column has the room before `foot`, large (`large_h` tall) if it can, else a strip (`strip_h`); when not even a
/// strip fits there, a strip over the half of the pitch away from the card it is about (its centre at `card_y`; a bench
/// card counts as low). It never leaves the column, so it never covers the game's picture or the deck.
pub fn pane_place(
    pitch: Rect,
    hint_bottom: f32,
    foot: f32,
    large_h: f32,
    strip_h: f32,
    card_y: f32,
) -> (Rect, Tier) {
    let free = foot - hint_bottom - 8.0;
    let docked = |h| {
        Rect::from_min_size(
            pos2(pitch.left(), hint_bottom + 8.0),
            vec2(pitch.width(), h),
        )
    };
    if free >= large_h {
        return (docked(large_h), Tier::Large);
    }
    if free >= strip_h {
        return (docked(strip_h), Tier::Strip);
    }
    let r = if card_y > pitch.center().y {
        Rect::from_min_size(pitch.left_top(), vec2(pitch.width(), strip_h))
    } else {
        Rect::from_min_max(
            pos2(pitch.left(), pitch.bottom() - strip_h),
            pitch.right_bottom(),
        )
    };
    (r, Tier::Strip)
}

/// The large tier's face in a pane `width` wide: the album art's 256x357 at one device pixel each
/// ([`crate::sizing::ALBUM`]), when the face and a position map at least 120 points wide fit the width; else none
/// (the strip).
fn large_face(width: f32, ppp: f32) -> Option<Vec2> {
    let face = crate::sizing::ALBUM / ppp;
    (face.x + 10.0 + 120.0 + BIG_FRAME <= width).then_some(face)
}

/// The board's panes, drawn once the hint under the bench is laid out (the board ends at `board_bottom`, the hint's
/// foot at `hint_bottom`; the column ends at `foot`): the preview of the card the mouse rests on (after the hover delay) at the cursor ([`preview_place`]), and
/// the pinned card ([`pane_place`]), else the selected card's partners. All stay inside the board's column. `turned`:
/// the pinned card on its back ([`flip_side`]); `flip`: the KEYS page's FLIP pressed this frame.
#[allow(clippy::too_many_arguments)]
pub fn info_panes(
    ui: &Ui,
    board: &Board,
    links: &Links,
    pitch: Rect,
    board_bottom: f32,
    hint_bottom: f32,
    foot: f32,
    turned: &mut Option<u32>,
    flip: bool,
    lang: Lang,
) {
    let (ask, preview, partners) = ui.data_mut(|d| {
        let ask = d.get_temp::<PaneAsk>(pane_id());
        let preview = d.get_temp::<(PaneAsk, Pos2)>(preview_id());
        let partners = d.get_temp::<(u32, Rect)>(partners_id());
        d.remove::<PaneAsk>(pane_id());
        d.remove::<(PaneAsk, Pos2)>(preview_id());
        d.remove::<(u32, Rect)>(partners_id());
        (ask, preview, partners)
    });
    let ppp = ui.ctx().pixels_per_point();
    if let Some((a, cursor)) = preview {
        let column = preview_column(pitch, board_bottom);
        let r = preview_place(column, cursor, a.at, preview_size(ppp, column.width()));
        let face = crate::sizing::hover_face(ppp);
        info_pane(ui, &a.card, r, Tier::Strip, face, None, true, lang);
    }
    if let Some(a) = ask {
        let large = large_face(pitch.width(), ppp);
        let strip = crate::sizing::hover_face(ppp);
        let (pane, tier) = pane_place(
            pitch,
            hint_bottom,
            foot,
            large.map_or(f32::INFINITY, |f| BIG_FRAME + f.y + FLIP_ROW + BIG_CAPTION),
            strip_pane(ppp),
            a.at.center().y,
        );
        let face = match (tier, large) {
            (Tier::Large, Some(f)) => f,
            _ => strip,
        };
        let has = has_back(ui.ctx(), a.card.number) == Some(true);
        let back = flip_side(turned, a.card.number, flip && has);
        if info_pane(
            ui,
            &a.card,
            pane,
            tier,
            face,
            has.then_some(back),
            false,
            lang,
        ) {
            flip_side(turned, a.card.number, true);
        }
        return;
    }
    *turned = None; // no card big: the next one opens on its front
    if let Some((sel, at)) = partners {
        // as tall as it was last frame (about 220 points the first time)
        let h = ui
            .ctx()
            .memory(|m| m.area_rect(Id::unique("board-partners")))
            .map_or(220.0, |r| r.height());
        let (r, _) = pane_place(pitch, hint_bottom, foot, f32::INFINITY, h, at.center().y);
        partner_list(ui, board, links, sel, r.min, lang);
    }
}

/// The side a card shown big is on: its back when `turned` holds its number. Another card than the one turned opens on
/// its front; `flip` turns it over (and back).
pub fn flip_side(turned: &mut Option<u32>, card: u32, flip: bool) -> bool {
    *turned = turned.filter(|n| *n == card);
    if flip {
        *turned = if turned.is_some() { None } else { Some(card) };
    }
    turned.is_some()
}

/// A card's info pane at the top of `area` (the game window's column, the board popped out): large when it fits, else
/// the strip. `back`: on its back, when it has a back picture ([`has_back`]); true when its FRONT | BACK switch or the
/// big card was clicked.
pub fn card_view(ui: &Ui, card: &Card, area: Rect, back: bool, lang: Lang) -> bool {
    let (pane, tier, face) = card_view_place(area, ui.ctx().pixels_per_point());
    let has = has_back(ui.ctx(), card.number) == Some(true);
    info_pane(ui, card, pane, tier, face, has.then_some(back), false, lang)
}

/// [`card_view`]'s pane in `area`, its tier and its face's size.
pub fn card_view_place(area: Rect, ppp: f32) -> (Rect, Tier, Vec2) {
    let strip = crate::sizing::hover_face(ppp);
    let large_h = |f: Vec2| BIG_FRAME + f.y + FLIP_ROW + BIG_CAPTION;
    match large_face(area.width(), ppp) {
        Some(f) if large_h(f) <= area.height() => (
            Rect::from_min_size(area.min, vec2(area.width(), large_h(f))),
            Tier::Large,
            f,
        ),
        _ => (
            Rect::from_min_size(area.min, vec2(area.width(), strip_pane(ppp))),
            Tier::Strip,
            strip,
        ),
    }
}

/// The info pane in `pane`: the card's face (its back when turned over) `face` big, the FRONT | BACK switch under it;
/// large, the position map beside them and the caption under them; a strip, the caption beside them. `back`: the side
/// shown, `None` for a card without a back picture (the switch's row stays, empty, so a late back moves nothing).
/// `preview`: the hover preview, a strip with no switch row that takes no pointer. True when the switch or the big
/// card was clicked: the caller turns it over.
#[allow(clippy::too_many_arguments)]
fn info_pane(
    ui: &Ui,
    card: &Card,
    pane: Rect,
    tier: Tier,
    face: Vec2,
    back: Option<bool>,
    preview: bool,
    lang: Lang,
) -> bool {
    let (has, back) = (back.is_some(), back == Some(true));
    let card = &*real(ui.ctx(), card);
    let large = tier == Tier::Large;
    let t = if large { 1.0 } else { 0.9 };
    // the map under its 22 pt heading, as tall as the card (narrower when the pane is)
    let map_w = ((face.y - 22.0) / PITCH_ASPECT).min(pane.width() - BIG_FRAME - 10.0 - face.x);
    let map = vec2(map_w, map_w * PITCH_ASPECT);
    let ppp = ui.ctx().pixels_per_point();
    let (mut card_r, mut switch_r) = (Rect::NOTHING, Rect::NOTHING);
    // click-through: the mat cards under the pane keep their hover, clicks and drags
    let name = if preview {
        "board-preview"
    } else {
        "board-big"
    };
    let pane_layer = egui::Area::new(Id::unique(name))
        .order(Order::Tooltip)
        .fixed_pos(pane.min)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::DEEP)
                .stroke(Stroke::new(1.0, theme::RIM_HI))
                .corner_radius(0)
                .inner_margin(10)
                .shadow(Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(160),
                })
                .show(ui, |ui| {
                    let inner = pane.size() - Vec2::splat(BIG_FRAME);
                    ui.set_min_size(inner);
                    ui.set_max_width(inner.x);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 6.0;
                            let (r, _) = ui.allocate_exact_size(face, Sense::hover());
                            card_r = r;
                            if large {
                                // the album art at its own pixels: its top left on a whole device pixel
                                use egui::emath::GuiRounding as _;
                                let r = Rect::from_min_size(r.min.round_to_pixels(ppp), face);
                                card_side(ui.painter(), r, card, back, false);
                            } else if back {
                                card_side(ui.painter(), r, card, true, false);
                            } else {
                                card_face(
                                    ui.painter(),
                                    r,
                                    card,
                                    Look {
                                        square: true,
                                        ..Look::default()
                                    },
                                );
                            }
                            // the switch is drawn on its own layer below
                            if !preview {
                                switch_r = ui
                                    .allocate_exact_size(
                                        vec2(face.x, FLIP_ROW - 6.0),
                                        Sense::hover(),
                                    )
                                    .0;
                            }
                        });
                        ui.vertical(|ui| {
                            if large {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                ui.label(
                                    egui::RichText::new(tr!(lang, "board.positions"))
                                        .font(theme::display(14.0))
                                        .color(theme::TEXT_DIM),
                                );
                                let (m, _) = ui.allocate_exact_size(map, Sense::hover());
                                aptitude_map(ui.painter(), m, &card.aptitude, lang);
                            } else {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                caption(ui, card, t, lang);
                            }
                        });
                    });
                    if large {
                        caption(ui, card, t, lang);
                    }
                });
        })
        .response
        .layer_id;
    if preview {
        ui.ctx().move_to_top(pane_layer); // over the pinned card
        return false;
    }
    // without a back picture nothing turns it over
    if !has {
        return false;
    }
    // only the big card and its switch take the pointer, each on a layer just above the pane
    let mut flip = false;
    let mut over = |name: &'static str, at: Rect, add: &mut dyn FnMut(&mut Ui) -> bool| {
        let shown = egui::Area::new(Id::unique(name))
            .order(Order::Tooltip)
            .fixed_pos(at.min)
            .show(ui.ctx(), |ui| add(ui));
        ui.ctx().set_sublayer(pane_layer, shown.response.layer_id);
        flip |= shown.inner;
    };
    over("board-big-card", card_r, &mut |ui| {
        ui.allocate_exact_size(face, Sense::click()).1.clicked()
    });
    over("board-big-switch", switch_r, &mut |ui| {
        let mut side = usize::from(back);
        crate::widgets::segmented(
            ui,
            &mut side,
            &[&tr!(lang, "cat.front"), &tr!(lang, "cat.back")],
            face.x,
        )
    });
    flip
}

/// The card's number, position and TOTAL on one line: `#4321 · MF · TOTAL 81`.
fn facts(card: &Card, lang: Lang) -> String {
    format!(
        "#{} · {} · {} {}",
        card.number,
        card.pos.code(),
        tr!(lang, "cat.total"),
        card.total
    )
}

/// The info pane's words: the name, club · country · season, number · position · TOTAL, the best roles and the stats (or SECRET); `t` scales the
/// text.
fn caption(ui: &mut Ui, card: &Card, t: f32, lang: Lang) {
    ui.label(
        egui::RichText::new(&card.name)
            .font(theme::display(20.0 * t))
            .color(theme::TEXT),
    );
    ui.label(
        egui::RichText::new(format!(
            "{} · {} · {}",
            card.club, card.country, card.season
        ))
        .font(theme::body(12.0 * t))
        .color(theme::TEXT_DIM),
    );
    // the number, position and TOTAL the drawn back carried, now that only a picture shows a back (#333)
    ui.label(
        egui::RichText::new(facts(card, lang))
            .font(theme::display(14.0 * t))
            .color(theme::AMBER),
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
                .font(theme::body(12.0 * t))
                .italics()
                .color(theme::TEXT_DIM),
        );
    }
    let stats = [
        "cat.off", "cat.def", "cat.tec", "cat.pow", "cat.spd", "cat.sta",
    ];
    if card.secret {
        ui.label(
            egui::RichText::new(tr!(lang, "cat.secret"))
                .font(theme::display(18.0 * t))
                .color(theme::TEXT_DIM),
        );
    } else {
        // columns as wide as their words, not the theme's 40-point least: six fit beside the strip's face on a
        // 420-point board
        egui::Grid::new("big-stats")
            .num_columns(6)
            .min_col_width(28.0)
            .spacing(vec2(14.0, 0.0))
            .show(ui, |ui| {
                for k in stats {
                    ui.label(
                        egui::RichText::new(tr!(lang, k))
                            .font(theme::display(12.0 * t))
                            .color(theme::TEXT_DIM),
                    );
                }
                ui.end_row();
                for v in card.stats {
                    ui.label(
                        egui::RichText::new(v.to_string())
                            .font(theme::display(18.0 * t))
                            .color(super::widgets::stat_color(v)),
                    );
                }
            });
    }
}

/// A card in the catalogue's results: lifts a little under the mouse; `on_table` dims it with a tick.
pub fn catalogue_card(
    ui: &mut Ui,
    card: &Card,
    size: egui::Vec2,
    selected: bool,
    on_table: bool,
    mark: Option<&str>,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    resp.dnd_set_drag_payload(card.clone());
    if resp.drag_started()
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
    {
        ui.data_mut(|d| d.insert_temp(grab_id(), (rect.center() - press) / size));
    }
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
    if let Some(mark) = mark {
        // Create-a-Team: STARTER or IMPORT, a copy that plays but never sells, on the picture's left under the band
        let p = ui.painter();
        let g = p.layout_no_wrap(mark.to_owned(), theme::display(11.0), theme::INK);
        let at = Rect::from_min_size(
            r.left_top() + vec2(3.0, (r.height() * 0.62).round()),
            g.size() + vec2(8.0, 2.0),
        );
        p.rect_filled(at, 3, theme::AMBER);
        p.galley(at.min + vec2(4.0, 1.0), g, theme::INK);
    }
    resp
}

#[cfg(test)]
mod tests;
