//! Painted widgets shared by every screen: cabinet buttons, segmented choices, chips, the edition badge, status lines,
//! stat bars, the range slider, key-binding cells, the step progress and the launcher's frameless title bar.

use super::Lang;
use super::theme::{self, Edition};
use egui::epaint::{Mesh, RectShape, TessellationOptions, Tessellator};
use egui::{
    Align2, Color32, CornerRadius, FontId, Galley, Id, Painter, Pos2, Rect, Response, Sense,
    Shadow, Stroke, StrokeKind, Ui, ViewportCommand, WidgetInfo, WidgetType, pos2, vec2,
};
use std::sync::Arc;

// ---- painting helpers ----

/// A rounded rectangle filled with a gradient from `a` to `b` (top to bottom, or left to right): the shape is
/// tessellated in white, then each vertex takes the colour of its position, keeping the anti-aliased edge's alpha.
pub fn gradient(
    painter: &Painter,
    rect: Rect,
    radius: impl Into<CornerRadius>,
    a: Color32,
    b: Color32,
    vertical: bool,
) {
    let mut mesh = Mesh::default();
    let mut t = Tessellator::new(
        painter.pixels_per_point(),
        TessellationOptions::default(),
        [1, 1],
        vec![],
    );
    t.tessellate_rect(&RectShape::filled(rect, radius, Color32::WHITE), &mut mesh);
    for v in &mut mesh.vertices {
        let f = if vertical {
            (v.pos.y - rect.top()) / rect.height().max(1.0)
        } else {
            (v.pos.x - rect.left()) / rect.width().max(1.0)
        };
        let c = a.lerp_to_gamma(b, f.clamp(0.0, 1.0));
        v.color = c.gamma_multiply(v.color.a() as f32 / 255.0);
    }
    painter.add(mesh);
}

/// A filled polygon around `center` (a star, a diamond, a hexagon): a triangle fan, so it need not be convex.
pub fn fan(painter: &Painter, center: Pos2, points: &[Pos2], color: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, color);
    for p in points {
        mesh.colored_vertex(*p, color);
    }
    let n = points.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(mesh);
}

/// Text laid out on one line, shrunk (down to 70 %) until it fits `max_width`: every label fits in EN, IT and JA.
pub fn fit(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let mut f = font.clone();
    loop {
        let g = painter.layout_no_wrap(text.to_owned(), f.clone(), color);
        if g.size().x <= max_width || f.size <= font.size * 0.7 {
            return g;
        }
        f.size -= 0.5;
    }
}

/// Paints `text` centred in `rect`, fitted to its width.
pub fn centered_text(painter: &Painter, rect: Rect, text: &str, font: FontId, color: Color32) {
    let g = fit(painter, text, font, color, rect.width());
    painter.galley(rect.center() - g.size() / 2.0, g, color);
}

/// A small capitals label in the display face, for field names and section headings.
pub fn caps(ui: &mut Ui, text: &str) -> Response {
    ui.label(
        egui::RichText::new(text)
            .font(theme::display(15.0))
            .color(theme::TEXT_DIM),
    )
}

/// A framed section with a heading and an accent tick: the building block of every screen.
pub fn section<R>(ui: &mut Ui, title: &str, accent: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::DEEP)
        .stroke(Stroke::new(1.0, theme::RIM))
        .corner_radius(theme::RADIUS_BIG)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(4.0, 16.0), Sense::hover());
                ui.painter().rect_filled(r, 2, accent);
                ui.label(
                    egui::RichText::new(title)
                        .font(theme::display(18.0))
                        .color(theme::TEXT),
                );
            });
            ui.add_space(4.0);
            add(ui)
        })
        .inner
}

// ---- cabinet buttons ----

/// What a cabinet button does decides its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cab {
    /// CENTRAL, COUNTER, L-SIDE, R-SIDE: the cabinet's yellow triangles; lit while the order is active.
    Tactic,
    /// PRESS: the red square at the diamond's centre.
    Press,
    Shoot,
    Keeper,
    KeyPlayer,
    Data,
    /// START, CARD, COIN, BACK.
    Plain,
    /// PLAY, INSTALL, UPDATE: the one action a screen is for.
    Go,
    /// STOP THE GAME.
    Stop,
}

impl Cab {
    pub fn color(self) -> Color32 {
        match self {
            Cab::Tactic => theme::AMBER,
            Cab::Shoot | Cab::Stop | Cab::Press => theme::RED,
            Cab::Keeper => theme::BLUE,
            Cab::KeyPlayer => theme::GOLD,
            Cab::Data => theme::GREEN,
            Cab::Plain => Color32::from_rgb(0x9A, 0xA8, 0xC0),
            Cab::Go => theme::GREEN,
        }
    }

    /// Dark text on the light faces, white on the deep ones.
    fn ink(self) -> Color32 {
        match self {
            Cab::Tactic | Cab::KeyPlayer | Cab::Data | Cab::Plain => theme::INK,
            _ => Color32::WHITE,
        }
    }
}

/// A direction: a triangle's apex, or the side a label is printed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// The outline of a cabinet button: the modern key, or the shapes of the real cabinet's plates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Key,
    Round,
    Square,
    Tri(Dir),
}

fn tri_points(d: Dir, r: Rect) -> [Pos2; 3] {
    match d {
        Dir::Up => [r.center_top(), r.left_bottom(), r.right_bottom()],
        Dir::Down => [r.center_bottom(), r.left_top(), r.right_top()],
        Dir::Left => [r.left_center(), r.right_top(), r.right_bottom()],
        Dir::Right => [r.right_center(), r.left_top(), r.left_bottom()],
    }
}

/// Whether `p` lies on the button: the whole box for a key or square, the circle, or the triangle.
pub fn shape_contains(shape: Shape, r: Rect, p: Pos2) -> bool {
    match shape {
        Shape::Key | Shape::Square => r.contains(p),
        Shape::Round => (p - r.center()).length() <= r.width().min(r.height()) / 2.0,
        Shape::Tri(d) => {
            let [a, b, c] = tri_points(d, r);
            let side = |u: Pos2, v: Pos2| (v.x - u.x) * (p.y - u.y) - (v.y - u.y) * (p.x - u.x);
            let s = [side(a, b), side(b, c), side(c, a)];
            s.iter().all(|x| *x >= 0.0) || s.iter().all(|x| *x <= 0.0)
        }
    }
}

/// A chunky arcade button: a face on a darker skirt that travels 4 px down while held, a lamp window that glows when
/// `lit`, and a label in the condensed display face fitted to the width. Hold it: the response's
/// `is_pointer_button_down_on()` is the key's down state.
pub fn cabinet(ui: &mut Ui, size: egui::Vec2, label: &str, kind: Cab, lit: bool) -> Response {
    cabinet_shaped(ui, size, label, kind, Shape::Key, lit)
}

/// `cabinet` in any shape. Only a key or a shape at least 50 px across prints its label inside; the caller prints
/// the others' beside them. `label` is the accessible name either way.
pub fn cabinet_shaped(
    ui: &mut Ui,
    size: egui::Vec2,
    label: &str,
    kind: Cab,
    shape: Shape,
    lit: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, lit, label));
    if ui.is_rect_visible(rect) {
        let ctx = ui.ctx();
        let press = ctx.animate_bool_with_time(
            resp.id.with("press"),
            resp.is_pointer_button_down_on(),
            0.06,
        );
        let glow = ctx.animate_bool_with_time(resp.id.with("lit"), lit, 0.16);
        let over = resp
            .hover_pos()
            .is_some_and(|p| shape_contains(shape, rect, p));
        let hover = ctx.animate_bool_with_time(resp.id.with("hover"), over && enabled, 0.12);
        cabinet_shape_face(
            ui.painter(),
            rect,
            label,
            kind,
            shape,
            Face {
                lit,
                glow,
                press,
                hover,
                enabled,
            },
        );
        if resp.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(3.0),
                10,
                Stroke::new(2.0, theme::BLUE),
                StrokeKind::Outside,
            );
        }
    }
    resp
}

/// The animated state of a cabinet button face: `glow`, `press` and `hover` run 0..1.
#[derive(Clone, Copy, Debug)]
pub struct Face {
    pub lit: bool,
    pub glow: f32,
    pub press: f32,
    pub hover: f32,
    pub enabled: bool,
}

/// Paints a cabinet button in any state (the widget sheet shows a pressed one without a mouse).
pub fn cabinet_face(p: &Painter, rect: Rect, label: &str, kind: Cab, f: Face) {
    cabinet_shape_face(p, rect, label, kind, Shape::Key, f)
}

/// `cabinet_face` in any shape. The cabinet's shapes are solid colour on a darker skirt, with no lamp window.
pub fn cabinet_shape_face(p: &Painter, rect: Rect, label: &str, kind: Cab, shape: Shape, f: Face) {
    if shape != Shape::Key {
        return solid_face(p, rect, label, kind, shape, f);
    }
    let base = if f.enabled {
        kind.color()
    } else {
        theme::TEXT_FAINT
    };
    let travel = 4.0;
    let r = if rect.height() >= 56.0 { 12 } else { 8 };
    let face = Rect::from_min_size(
        rect.min + vec2(0.0, f.press * travel),
        rect.size() - vec2(0.0, travel),
    );
    if f.glow > 0.0 {
        let s = Shadow {
            offset: [0, 0],
            blur: 18,
            spread: 1,
            color: base.gamma_multiply(0.55 * f.glow),
        };
        p.add(s.as_shape(face, r));
    }
    // the skirt: what shows below the face
    p.rect_filled(
        rect.with_min_y(rect.min.y + travel),
        r,
        theme::WELL.lerp_to_gamma(base, 0.22),
    );
    let (top, bottom) = if f.lit {
        (
            base.lerp_to_gamma(Color32::WHITE, 0.25),
            base.lerp_to_gamma(Color32::BLACK, 0.12),
        )
    } else {
        let t = base.lerp_to_gamma(theme::RAISED, 0.84);
        (
            t.lerp_to_gamma(Color32::WHITE, 0.06 + 0.06 * f.hover),
            theme::RAISED.lerp_to_gamma(theme::WELL, 0.3),
        )
    };
    gradient(p, face, r, top, bottom, true);
    // the lamp window across the top of the face
    let lamp = Rect::from_center_size(
        pos2(face.center().x, face.top() + 5.0),
        vec2(face.width() * 0.42, 3.0),
    );
    p.rect_filled(
        lamp,
        2,
        if f.lit {
            Color32::WHITE.gamma_multiply(0.85)
        } else {
            base.gamma_multiply(0.55)
        },
    );
    p.line_segment(
        [
            face.left_top() + vec2(r as f32, 1.0),
            face.right_top() + vec2(-(r as f32), 1.0),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(if f.lit { 90 } else { 30 })),
    );
    let rim = if f.lit {
        base.lerp_to_gamma(Color32::WHITE, 0.4)
    } else {
        base.gamma_multiply(0.45 + 0.35 * f.hover)
    };
    p.rect_stroke(face, r, Stroke::new(1.0, rim), StrokeKind::Inside);
    let fsize = (face.height() * 0.36).clamp(13.0, 30.0);
    let ink = if f.lit {
        kind.ink()
    } else if f.enabled {
        theme::TEXT
    } else {
        theme::TEXT_FAINT
    };
    centered_text(
        p,
        face.shrink2(vec2(6.0, 0.0)).translate(vec2(0.0, 2.0)),
        label,
        theme::display(fsize),
        ink,
    );
}

fn solid_face(p: &Painter, rect: Rect, label: &str, kind: Cab, shape: Shape, f: Face) {
    let base = if f.enabled {
        kind.color()
    } else {
        theme::TEXT_FAINT
    };
    let travel = 3.0;
    let skirt = rect.with_min_y(rect.min.y + travel);
    let face = Rect::from_min_size(
        rect.min + vec2(0.0, f.press * travel),
        rect.size() - vec2(0.0, travel),
    );
    let fill = if f.lit {
        base.lerp_to_gamma(Color32::WHITE, 0.3)
    } else {
        base.lerp_to_gamma(Color32::WHITE, 0.1 * f.hover)
    };
    let rim = Stroke::new(1.0, base.lerp_to_gamma(Color32::BLACK, 0.4));
    let draw = |r: Rect, fill: Color32, stroke: Stroke| match shape {
        Shape::Round => {
            p.circle(r.center(), r.width().min(r.height()) / 2.0, fill, stroke);
        }
        Shape::Tri(d) => {
            p.add(egui::Shape::convex_polygon(
                tri_points(d, r).to_vec(),
                fill,
                stroke,
            ));
        }
        _ => {
            p.rect(r, 4, fill, stroke, StrokeKind::Inside);
        }
    };
    draw(
        skirt,
        base.lerp_to_gamma(Color32::BLACK, 0.55),
        Stroke::NONE,
    );
    draw(face, fill, rim);
    if shape == Shape::Round {
        let hl = Rect::from_center_size(
            face.center() - vec2(0.0, face.height() * 0.08),
            face.size() * 0.66,
        );
        p.circle_filled(
            hl.center(),
            hl.width() / 2.0,
            fill.lerp_to_gamma(Color32::WHITE, 0.12),
        );
    }
    if face.width().min(face.height()) >= 50.0 {
        let fsize = (face.height() * 0.22).clamp(13.0, 24.0);
        centered_text(p, face, label, theme::display(fsize), Color32::WHITE);
    }
}

// ---- choices ----

/// One choice of several, joined in a row; the chosen one is lit amber. Returns true when the choice changed.
pub fn segmented(ui: &mut Ui, selected: &mut usize, labels: &[&str], width: f32) -> bool {
    let h = 34.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let w = width / labels.len() as f32;
    let mut changed = false;
    ui.painter().rect_filled(rect, theme::RADIUS, theme::WELL);
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(w * i as f32, 0.0), vec2(w, h)).shrink(2.0);
        let resp = ui.interact(r, ui.id().with(("seg", label)), Sense::click());
        let on = *selected == i;
        resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, on, *label));
        if resp.clicked() && !on {
            *selected = i;
            changed = true;
        }
        let p = ui.painter();
        if on {
            gradient(
                p,
                r,
                3,
                theme::AMBER.lerp_to_gamma(Color32::WHITE, 0.2),
                theme::AMBER,
                true,
            );
        } else if resp.hovered() {
            p.rect_filled(r, 3, theme::RAISED_HI);
        }
        let color = if on { theme::INK } else { theme::TEXT_DIM };
        centered_text(
            p,
            r.shrink2(vec2(6.0, 0.0)),
            label,
            theme::bold(14.0),
            color,
        );
    }
    ui.painter().rect_stroke(
        rect,
        theme::RADIUS,
        Stroke::new(1.0, theme::RIM),
        StrokeKind::Inside,
    );
    changed
}

/// A filter chip: a pill that toggles. `color` marks a position chip (GK yellow, DF blue ...).
pub fn chip(ui: &mut Ui, text: &str, on: bool, color: Option<Color32>) -> Response {
    let font = theme::display(15.0);
    let g = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let size = vec2(g.size().x + 22.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, text));
    let p = ui.painter();
    let c = color.unwrap_or(theme::AMBER);
    let (fill, rim, ink) = if on {
        // dark text on the light colours (amber, gold, pearl), white on the deep ones
        let light = c.r() as u32 * 299 + c.g() as u32 * 587 + c.b() as u32 * 114 > 140_000;
        (c, c, if light { theme::INK } else { Color32::WHITE })
    } else if resp.hovered() {
        (theme::RAISED_HI, theme::RIM_HI, theme::TEXT)
    } else {
        (theme::RAISED, theme::RIM, theme::TEXT_DIM)
    };
    p.rect_filled(rect, 13, fill);
    p.rect_stroke(rect, 13, Stroke::new(1.0, rim), StrokeKind::Inside);
    if let (Some(c), false) = (color, on) {
        p.circle_filled(pos2(rect.left() + 9.0, rect.center().y), 3.0, c);
    }
    let x = if color.is_some() && !on { 4.0 } else { 0.0 };
    p.galley(rect.center() - g.size() / 2.0 + vec2(x, 0.0), g, ink);
    resp
}

/// The edition badge: which edition a screen belongs to.
pub fn badge(ui: &mut Ui, edition: Edition) -> Response {
    let a = edition.accent();
    let g = ui
        .painter()
        .layout_no_wrap(edition.label().to_owned(), theme::display(16.0), a);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 18.0, 22.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 11, a.gamma_multiply(0.16));
    p.rect_stroke(rect, 11, Stroke::new(1.0, a), StrokeKind::Inside);
    p.galley(rect.center() - g.size() / 2.0, g, a);
    resp
}

// ---- status lines ----

/// The three tones of a one-line message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Done,
    Refused,
    Notice,
}

impl Tone {
    pub fn color(self) -> Color32 {
        match self {
            Tone::Done => theme::GREEN,
            Tone::Refused => theme::RED_TEXT,
            Tone::Notice => theme::AMBER,
        }
    }
}

/// A painted tone mark: a tick, a cross or an exclamation, in a disc.
pub fn tone_icon(p: &Painter, c: Pos2, tone: Tone) {
    let col = tone.color();
    p.circle_filled(c, 9.0, col);
    let s = Stroke::new(2.2, theme::NIGHT);
    match tone {
        Tone::Done => {
            p.line_segment([c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.5)], s);
            p.line_segment([c + vec2(-1.0, 3.5), c + vec2(4.5, -3.5)], s);
        }
        Tone::Refused => {
            p.line_segment([c + vec2(-3.5, -3.5), c + vec2(3.5, 3.5)], s);
            p.line_segment([c + vec2(-3.5, 3.5), c + vec2(3.5, -3.5)], s);
        }
        Tone::Notice => {
            p.line_segment([c + vec2(0.0, -4.5), c + vec2(0.0, 1.0)], s);
            p.circle_filled(c + vec2(0.0, 4.2), 1.3, theme::NIGHT);
        }
    }
}

/// One message line: a tone bar, a mark and the text (wrapped if it must).
pub fn status_line(ui: &mut Ui, tone: Tone, text: &str) -> Response {
    let col = tone.color();
    let r = egui::Frame::new()
        .fill(col.gamma_multiply(0.12))
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin {
            left: 34,
            right: 10,
            top: 7,
            bottom: 7,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(text)
                    .color(theme::TEXT)
                    .font(theme::body(14.0)),
            );
        })
        .response;
    let p = ui.painter();
    p.rect_filled(
        Rect::from_min_size(r.rect.min, vec2(3.0, r.rect.height())),
        1,
        col,
    );
    tone_icon(p, pos2(r.rect.left() + 17.0, r.rect.top() + 16.5), tone);
    r
}

// ---- numbers ----

/// The colour of a 1-20 stat: dim when low, cyan, green, amber at the top.
pub fn stat_color(v: u8) -> Color32 {
    match v {
        0..=9 => Color32::from_rgb(0x4A, 0x7B, 0xA8),
        10..=14 => theme::CYAN,
        15..=17 => theme::GREEN,
        _ => theme::AMBER,
    }
}

/// A stat as an LED meter of 20 segments with its value, in the card-back order of the stats pane.
pub fn stat_bar(ui: &mut Ui, label: &str, value: u8, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
    let p = ui.painter();
    let lab = Rect::from_min_size(rect.min, vec2(40.0, rect.height()));
    let g = fit(p, label, theme::display(15.0), theme::TEXT_DIM, 40.0);
    p.galley(
        pos2(lab.left(), lab.center().y - g.size().y / 2.0),
        g,
        theme::TEXT_DIM,
    );
    p.text(
        rect.right_center(),
        Align2::RIGHT_CENTER,
        value.to_string(),
        theme::display(20.0),
        theme::TEXT,
    );
    let meter = Rect::from_min_max(
        pos2(lab.right() + 4.0, rect.top() + 5.0),
        pos2(rect.right() - 30.0, rect.bottom() - 5.0),
    );
    let n = 20;
    let w = meter.width() / n as f32;
    let col = stat_color(value);
    for i in 0..n {
        let s = Rect::from_min_size(
            meter.min + vec2(w * i as f32, 0.0),
            vec2(w - 1.5, meter.height()),
        );
        let on = i < value as usize;
        p.rect_filled(s, 1, if on { col } else { theme::WELL });
    }
    resp
}

/// A two-handled range on a 1-20 (or 60-100) scale: drag either handle; a press on the track moves the nearer one.
pub fn range_slider(
    ui: &mut Ui,
    label: &str,
    range: &mut (u8, u8),
    lim: (u8, u8),
    width: f32,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click_and_drag());
    let p = ui.painter();
    let g = fit(p, label, theme::display(15.0), theme::TEXT_DIM, 48.0);
    p.galley(
        pos2(rect.left(), rect.center().y - g.size().y / 2.0),
        g,
        theme::TEXT_DIM,
    );
    let track = Rect::from_min_max(
        pos2(rect.left() + 56.0, rect.center().y - 3.0),
        pos2(rect.right() - 54.0, rect.center().y + 3.0),
    );
    let span = (lim.1 - lim.0) as f32;
    let x_of = |v: u8| track.left() + track.width() * (v - lim.0) as f32 / span;
    let v_of = |x: f32| {
        (lim.0 as f32 + ((x - track.left()) / track.width()).clamp(0.0, 1.0) * span).round() as u8
    };
    let grab = resp.id.with("grab");
    if let Some(pos) = resp.interact_pointer_pos() {
        let which = if resp.drag_started()
            || resp.clicked()
            || !ui.data(|d| d.get_temp::<bool>(grab).is_some())
        {
            let hi = (pos.x - x_of(range.1)).abs() < (pos.x - x_of(range.0)).abs();
            ui.data_mut(|d| d.insert_temp(grab, hi));
            hi
        } else {
            ui.data(|d| d.get_temp::<bool>(grab).unwrap_or(false))
        };
        let v = v_of(pos.x);
        if which {
            range.1 = v.max(range.0);
        } else {
            range.0 = v.min(range.1);
        }
    } else {
        ui.data_mut(|d| d.remove::<bool>(grab));
    }
    let p = ui.painter();
    p.rect_filled(track, 3, theme::WELL);
    let fill = Rect::from_x_y_ranges(x_of(range.0)..=x_of(range.1), track.y_range());
    p.rect_filled(fill, 3, theme::CYAN);
    let active = resp.dragged() || resp.hovered();
    for v in [range.0, range.1] {
        let h = Rect::from_center_size(pos2(x_of(v), rect.center().y), vec2(10.0, 20.0));
        p.rect_filled(h, 3, if active { theme::AMBER } else { theme::TEXT });
        p.line_segment(
            [
                h.center_top() + vec2(0.0, 5.0),
                h.center_bottom() - vec2(0.0, 5.0),
            ],
            Stroke::new(1.0, theme::NIGHT),
        );
    }
    let full = *range == lim;
    p.text(
        rect.right_center(),
        Align2::RIGHT_CENTER,
        format!("{}–{}", range.0, range.1),
        theme::display(17.0),
        if full { theme::TEXT_DIM } else { theme::TEXT },
    );
    resp
}

/// Steps of a start or an install as chunky blocks: done green, the current one amber, the rest dark; `N/M` beside.
pub fn steps(ui: &mut Ui, done: usize, total: usize, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
    let p = ui.painter();
    let bar = rect.with_max_x(rect.right() - 44.0);
    let w = bar.width() / total.max(1) as f32;
    for i in 0..total {
        let s = Rect::from_min_size(
            bar.min + vec2(w * i as f32, 0.0),
            vec2(w - 3.0, bar.height()),
        );
        let c = if i < done {
            theme::GREEN
        } else if i == done {
            theme::AMBER
        } else {
            theme::WELL
        };
        p.rect_filled(s, 2, c);
    }
    p.text(
        rect.right_center(),
        Align2::RIGHT_CENTER,
        format!("{done}/{total}"),
        theme::display(18.0),
        theme::TEXT,
    );
    resp
}

/// A plain progress bar with a percentage, for a download or an install stage.
pub fn progress(ui: &mut Ui, fraction: f32, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());
    let p = ui.painter();
    let bar = rect.with_max_x(rect.right() - 44.0);
    p.rect_filled(bar, 3, theme::WELL);
    let f = Rect::from_min_size(
        bar.min,
        vec2(bar.width() * fraction.clamp(0.0, 1.0), bar.height()),
    );
    gradient(
        p,
        f,
        3,
        theme::GREEN.lerp_to_gamma(Color32::WHITE, 0.2),
        theme::GREEN,
        true,
    );
    p.text(
        rect.right_center(),
        Align2::RIGHT_CENTER,
        format!("{:.0} %", fraction * 100.0),
        theme::display(17.0),
        theme::TEXT,
    );
    resp
}

// ---- key bindings ----

/// The state of a binding cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    Bound,
    /// Clicked: the next key pressed is taken.
    Capturing,
    Empty,
    /// Just changed (a swap from another action shows here too).
    Changed,
}

/// A keycap: the bound key, "press a key" while capturing (an amber pulsing rim), a dash when cleared.
pub fn key_cell(ui: &mut Ui, text: &str, state: KeyState, width: f32, lang: Lang) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, text));
    let p = ui.painter();
    p.rect_filled(rect, 5, theme::WELL);
    let cap = rect.shrink(2.0).with_max_y(rect.bottom() - 4.0);
    let (face, label, ink) = match state {
        KeyState::Capturing => (
            theme::AMBER.gamma_multiply(0.18),
            tr!(lang, "keys.press_a_key"),
            theme::AMBER,
        ),
        KeyState::Empty => (theme::DEEP, "—".to_owned(), theme::TEXT_FAINT),
        KeyState::Changed => (
            theme::GREEN.gamma_multiply(0.25),
            text.to_owned(),
            Color32::WHITE,
        ),
        KeyState::Bound if resp.hovered() => (theme::RAISED_HI, text.to_owned(), Color32::WHITE),
        KeyState::Bound => (theme::RAISED, text.to_owned(), theme::TEXT),
    };
    if state != KeyState::Capturing {
        p.rect_filled(rect.shrink(2.0).translate(vec2(0.0, 2.0)), 4, theme::NIGHT);
    }
    gradient(
        p,
        cap,
        4,
        face.lerp_to_gamma(Color32::WHITE, 0.06),
        face,
        true,
    );
    if state == KeyState::Capturing {
        let t = ui.input(|i| i.time) as f32;
        let a = 0.55 + 0.45 * (t * 5.0).sin().abs();
        p.rect_stroke(
            rect,
            5,
            Stroke::new(2.0, theme::AMBER.gamma_multiply(a)),
            StrokeKind::Inside,
        );
        ui.ctx().request_repaint();
    } else {
        p.rect_stroke(cap, 4, Stroke::new(1.0, theme::RIM), StrokeKind::Inside);
    }
    centered_text(
        p,
        cap.shrink2(vec2(6.0, 0.0)),
        &label,
        theme::display(17.0),
        ink,
    );
    resp
}

// ---- window chrome ----

/// What the title bar asked for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleAction {
    None,
    Minimize,
    Close,
}

/// The launcher's own title bar (its window has no system frame): our mark, the name, the edition badge, the
/// launcher language, minimize and close. Dragging the bar moves the window.
pub fn title_bar(ui: &mut Ui, edition: Edition, lang: &mut Lang) -> TitleAction {
    let rect = ui.max_rect().with_max_y(ui.max_rect().top() + 44.0);
    let drag = ui.interact(rect, Id::new("title-drag"), Sense::click_and_drag());
    if drag.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    let p = ui.painter();
    gradient(p, rect, 0, theme::DEEP, theme::NIGHT, true);
    let accent = edition.accent();
    let stripe = Rect::from_min_size(rect.left_bottom() - vec2(0.0, 2.0), vec2(rect.width(), 2.0));
    gradient(p, stripe, 0, accent, accent.gamma_multiply(0.0), false);
    mark(p, pos2(rect.left() + 26.0, rect.center().y), accent);
    let name = p.layout_no_wrap("FORZA-WCCF".to_owned(), theme::display(22.0), theme::TEXT);
    let name_w = name.size().x;
    p.galley(
        pos2(rect.left() + 46.0, rect.center().y - name.size().y / 2.0),
        name,
        theme::TEXT,
    );

    let mut action = TitleAction::None;
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(12.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.add_space(34.0 + name_w + 10.0);
    badge(&mut child, edition);
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (glyph, act) in [(1, TitleAction::Close), (0, TitleAction::Minimize)] {
            let (r, resp) = ui.allocate_exact_size(vec2(40.0, 32.0), Sense::click());
            let hot = resp.hovered();
            let p = ui.painter();
            if hot {
                p.rect_filled(
                    r,
                    4,
                    if glyph == 1 {
                        theme::RED
                    } else {
                        theme::RAISED_HI
                    },
                );
            }
            let c = r.center();
            let s = Stroke::new(1.5, if hot { Color32::WHITE } else { theme::TEXT_DIM });
            if glyph == 1 {
                p.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], s);
                p.line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], s);
            } else {
                p.line_segment([c + vec2(-6.0, 0.0), c + vec2(6.0, 0.0)], s);
            }
            if resp.clicked() {
                action = act;
            }
        }
        ui.add_space(10.0);
        let mut i = Lang::ALL.iter().position(|l| l == lang).unwrap_or(0);
        ui.allocate_ui(vec2(132.0, 30.0), |ui| {
            if segmented(ui, &mut i, &["EN", "IT", "日本語"], 132.0) {
                *lang = Lang::ALL[i];
            }
        });
    });
    match action {
        TitleAction::Minimize => ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true)),
        TitleAction::Close => ui.ctx().send_viewport_cmd(ViewportCommand::Close),
        TitleAction::None => {}
    }
    ui.advance_cursor_after_rect(rect);
    action
}

/// Forza-WCCF's own mark: a small pitch with a centre line and circle, in the edition's accent.
pub fn mark(p: &Painter, c: Pos2, accent: Color32) {
    let r = Rect::from_center_size(c, vec2(22.0, 28.0));
    gradient(p, r, 4, theme::PITCH_STRIPE, theme::PITCH, true);
    p.rect_stroke(r, 4, Stroke::new(1.5, accent), StrokeKind::Inside);
    p.line_segment(
        [
            r.left_center() + vec2(3.0, 0.0),
            r.right_center() - vec2(3.0, 0.0),
        ],
        Stroke::new(1.0, theme::PITCH_LINE),
    );
    p.circle_stroke(c, 4.5, Stroke::new(1.0, theme::PITCH_LINE));
}

/// A section that folds away: a header row with a turning chevron and a one-line summary on the right.
pub fn fold(
    ui: &mut Ui,
    title: &str,
    summary: &str,
    open: &mut bool,
    accent: Color32,
    add: impl FnOnce(&mut Ui),
) {
    egui::Frame::new()
        .fill(theme::DEEP)
        .stroke(Stroke::new(1.0, theme::RIM))
        .corner_radius(theme::RADIUS_BIG)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (rect, resp) =
                ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
            resp.widget_info(|| {
                WidgetInfo::selected(WidgetType::CollapsingHeader, true, *open, title)
            });
            if resp.clicked() {
                *open = !*open;
            }
            let turn = ui.ctx().animate_bool_with_time(resp.id, *open, 0.14);
            let p = ui.painter();
            let c = pos2(rect.left() + 8.0, rect.center().y);
            let a = turn * std::f32::consts::FRAC_PI_2;
            let rot = |v: egui::Vec2| {
                c + vec2(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos())
            };
            let col = if resp.hovered() { theme::TEXT } else { accent };
            fan(
                p,
                c,
                &[
                    rot(vec2(-3.0, -5.0)),
                    rot(vec2(5.0, 0.0)),
                    rot(vec2(-3.0, 5.0)),
                ],
                col,
            );
            p.text(
                pos2(rect.left() + 22.0, rect.center().y),
                Align2::LEFT_CENTER,
                title,
                theme::display(18.0),
                theme::TEXT,
            );
            let sx = rect.left()
                + 22.0
                + p.layout_no_wrap(title.to_owned(), theme::display(18.0), theme::TEXT)
                    .size()
                    .x
                + 16.0;
            let g = fit(
                p,
                summary,
                theme::body(13.0),
                theme::TEXT_DIM,
                rect.right() - sx,
            );
            p.galley(
                pos2(
                    rect.right() - g.size().x,
                    rect.center().y - g.size().y / 2.0,
                ),
                g,
                theme::TEXT_DIM,
            );
            if *open {
                ui.add_space(6.0);
                add(ui);
            }
        });
}

/// A secondary action: a flat raised button with the bold face, for everything that is not the screen's one action.
pub fn action(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).font(theme::bold(14.0)))
            .min_size(vec2(0.0, 32.0)),
    )
}

/// A field label of a fixed width in front of a control, so the controls of a section line up.
pub fn field(ui: &mut Ui, label: &str, width: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
    let g = fit(
        ui.painter(),
        label,
        theme::display(15.0),
        theme::TEXT_DIM,
        width - 8.0,
    );
    ui.painter().galley(
        pos2(r.left(), r.center().y - g.size().y / 2.0),
        g,
        theme::TEXT_DIM,
    );
}

/// An option as a row: a painted switch, a title and a one-line hint. The whole row toggles.
pub fn toggle_row(ui: &mut Ui, on: &mut bool, title: &str, hint: &str) -> Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, *on, title));
    if resp.clicked() {
        *on = !*on;
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, theme::RADIUS, theme::RAISED.gamma_multiply(0.6));
    }
    let sw = Rect::from_min_size(
        pos2(rect.left() + 6.0, rect.center().y - 10.0),
        vec2(38.0, 20.0),
    );
    p.rect_filled(sw, 10, theme::RAISED.lerp_to_gamma(theme::GREEN, t));
    p.rect_stroke(
        sw,
        10,
        Stroke::new(1.0, theme::RIM.lerp_to_gamma(theme::GREEN, t)),
        StrokeKind::Inside,
    );
    p.circle_filled(
        pos2(sw.left() + 10.0 + 18.0 * t, sw.center().y),
        7.0,
        Color32::WHITE,
    );
    let x = sw.right() + 12.0;
    let g = fit(p, title, theme::bold(14.0), theme::TEXT, rect.right() - x);
    p.galley(pos2(x, rect.top() + 3.0), g, theme::TEXT);
    let g = fit(
        p,
        hint,
        theme::body(12.0),
        theme::TEXT_DIM,
        rect.right() - x,
    );
    p.galley(pos2(x, rect.top() + 23.0), g, theme::TEXT_DIM);
    resp
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    #[test]
    fn hit_test_follows_the_shape() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(40.0, 40.0));
        let corner = pos2(1.0, 1.0);
        assert!(shape_contains(Shape::Square, r, corner));
        assert!(!shape_contains(Shape::Round, r, corner));
        assert!(shape_contains(Shape::Round, r, r.center()));
        // an up triangle: apex at the top middle, so the top corners are outside, the bottom ones inside
        assert!(!shape_contains(Shape::Tri(Dir::Up), r, corner));
        assert!(shape_contains(Shape::Tri(Dir::Up), r, pos2(1.0, 39.0)));
        assert!(shape_contains(Shape::Tri(Dir::Up), r, r.center()));
        // a left triangle: apex at the left middle
        assert!(!shape_contains(Shape::Tri(Dir::Left), r, corner));
        assert!(shape_contains(Shape::Tri(Dir::Left), r, pos2(39.0, 1.0)));
    }
}
