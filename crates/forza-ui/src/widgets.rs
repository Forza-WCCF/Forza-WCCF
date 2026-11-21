//! Painted widgets shared by every screen: cabinet buttons, segmented choices, chips, the edition badge, status lines,
//! stat bars, the range slider, key-binding cells, the step progress and the launcher's frameless title bar.

use super::Lang;
use super::theme::{self, Edition};
use crate::theme::Accent;
use egui::epaint::{Mesh, RectShape, TessellationOptions, Tessellator};
use egui::{
    Align2, Color32, CornerRadius, FontId, Galley, Id, Painter, Pos2, Rect, Response, Role, Sense,
    Stroke, StrokeKind, Ui, ViewportCommand, WidgetInfo, pos2, vec2,
};
use std::ops::RangeInclusive;
use std::sync::Arc;

mod cab;
pub use cab::*;

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

/// Text laid out on one line, shrunk (down to 70 %) until it fits `max_width`. Text still too wide at 70 % is drawn
/// as it is and, while a test's [`probe`] is on, recorded for [`overflows`].
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
        if g.size().x <= max_width {
            return g;
        }
        if f.size <= font.size * 0.7 {
            if !text.is_empty() {
                record(painter.ctx(), text, max_width, g.size().x);
            }
            return g;
        }
        f.size -= 0.5;
    }
}

/// [`fit`], then cut with "…" when the text is still too wide at the 70 % floor: for names from the game data and
/// other text of any length, which may never run past its room.
pub fn fit_elided(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let g = fit(painter, text, font, color, max_width);
    if g.size().x <= max_width {
        return g;
    }
    let mut job = (*g.job).clone();
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width);
    painter.layout_job(job)
}

/// A text [`fit`] could not fit: its room and its width at the 70 % floor.
#[derive(Clone, Debug, PartialEq)]
pub struct Overflow {
    pub text: String,
    pub room: f32,
    pub width: f32,
}

#[derive(Clone, Default)]
struct Ledger(Vec<Overflow>);

fn ledger_id() -> Id {
    Id::unique("forza_ui::widgets::ledger")
}

/// Starts (or clears) the overflow ledger on `ctx`; without it [`fit`] records nothing. For tests.
pub fn probe(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(ledger_id(), Ledger::default()));
}

/// The texts [`fit`] could not fit since [`probe`], each once.
pub fn overflows(ctx: &egui::Context) -> Vec<Overflow> {
    ctx.data(|d| d.get_temp::<Ledger>(ledger_id()))
        .map(|l| l.0)
        .unwrap_or_default()
}

fn record(ctx: &egui::Context, text: &str, room: f32, width: f32) {
    ctx.data_mut(|d| {
        let Some(mut l) = d.get_temp::<Ledger>(ledger_id()) else {
            return;
        };
        let o = Overflow {
            text: text.to_owned(),
            room,
            width,
        };
        if !l.0.contains(&o) {
            l.0.push(o);
            d.insert_temp(ledger_id(), l);
        }
    });
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

/// The size of a dialog's buttons (CONTINUE and BACK, OK and CANCEL).
pub const DIALOG_BUTTON: egui::Vec2 = vec2(196.0, 48.0);

/// A dialog over the dimmed window in the look the Launcher's, the installer's and the lobby's dialogs share.
pub fn modal<R>(
    ctx: &egui::Context,
    id: Id,
    add: impl FnOnce(&mut Ui) -> R,
) -> egui::ModalResponse<R> {
    egui::Modal::new(id)
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(
            egui::Frame::new()
                .fill(theme::DEEP)
                .stroke(Stroke::new(1.0, theme::RIM))
                .corner_radius(theme::RADIUS_BIG)
                .inner_margin(20),
        )
        .show(ctx, add)
}

// ---- choices ----

/// A [`segmented`] switch's height.
pub const SEGMENTED_H: f32 = 34.0;

/// One choice of several, joined in a row; the chosen one is lit amber. Returns true when the choice changed.
pub fn segmented(ui: &mut Ui, selected: &mut usize, labels: &[&str], width: f32) -> bool {
    let h = SEGMENTED_H;
    // the row's own id, not the ui's: two rows in one ui would share each segment's id
    let (rect, row) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let w = width / labels.len() as f32;
    let segs: Vec<(Rect, Response)> = (0..labels.len())
        .map(|i| {
            let r = Rect::from_min_size(rect.min + vec2(w * i as f32, 0.0), vec2(w, h)).shrink(2.0);
            (r, ui.interact(r, row.id.with(("seg", i)), Sense::click()))
        })
        .collect();
    // the click lands before the highlight is read, so the frame that turned the switch already lights the new side
    let mut changed = false;
    for (i, (_, resp)) in segs.iter().enumerate() {
        if resp.clicked() && *selected != i {
            *selected = i;
            changed = true;
        }
    }
    ui.painter().rect_filled(rect, theme::RADIUS, theme::WELL);
    for (i, (r, resp)) in segs.iter().enumerate() {
        let label = labels[i];
        let on = *selected == i;
        resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, label));
        let p = ui.painter();
        if on {
            gradient(
                p,
                *r,
                3,
                theme::AMBER.lerp_to_gamma(Color32::WHITE, 0.2),
                theme::AMBER,
                true,
            );
        } else if resp.hovered() {
            p.rect_filled(*r, 3, theme::RAISED_HI);
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

/// One step of [`stepper`]: `up` or down to the next multiple of `step` (8 -> 10 -> 15, 10 -> 8), kept in `range`;
/// `wrap` goes round to the other end instead (a clock: 23:55 -> 00:00).
pub fn step_value(v: u16, up: bool, range: &RangeInclusive<u16>, step: u16, wrap: bool) -> u16 {
    assert!(step > 0, "a stepper needs a step");
    let (lo, hi, s) = (
        i32::from(*range.start()),
        i32::from(*range.end()),
        i32::from(step),
    );
    let v = i32::from(v);
    let n = if up {
        (v.div_euclid(s) + 1) * s
    } else {
        (v - 1).div_euclid(s) * s
    };
    let n = match (n < lo, n > hi, wrap) {
        (true, _, true) => hi.div_euclid(s) * s,
        (_, true, true) => lo,
        _ => n.clamp(lo, hi),
    };
    n as u16
}

/// Hold an end of a [`stepper`] this long before it repeats, then repeat this often (seconds).
const STEP_HOLD: f64 = 0.4;
const STEP_REPEAT: f64 = 0.1;

/// A number with − and + at its ends, in the [`segmented`] well: a click steps once, holding an end repeats. `text`
/// is the value as shown, `names` the ends' spoken names. True when the value changed.
#[allow(clippy::too_many_arguments)]
pub fn stepper(
    ui: &mut Ui,
    value: &mut u16,
    range: RangeInclusive<u16>,
    step: u16,
    wrap: bool,
    text: &str,
    names: [&str; 2],
    width: f32,
) -> bool {
    let h = 34.0;
    let (rect, mid) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let old = *value;
    mid.widget_info(|| WidgetInfo::slider(true, f64::from(old), text));
    ui.painter().rect_filled(rect, theme::RADIUS, theme::WELL);
    let now = ui.input(|i| i.time);
    for (up, r, glyph, name) in [
        (
            false,
            Rect::from_min_size(rect.min, vec2(h, h)),
            "−",
            names[0],
        ),
        (
            true,
            Rect::from_min_size(rect.right_top() - vec2(h, 0.0), vec2(h, h)),
            "+",
            names[1],
        ),
    ] {
        let r = r.shrink(2.0);
        let live = wrap || step_value(*value, up, &range, step, wrap) != *value;
        let resp = ui.interact(r, mid.id.with(up), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, live, name));
        // (press time, repeats so far) while held: a release after a repeat is not one more step
        let key = resp.id.with("held");
        let held = live && resp.is_pointer_button_down_on();
        let mut stepped = false;
        if held {
            let (t0, n) = ui
                .data(|d| d.get_temp::<(f64, u32)>(key))
                .unwrap_or((now, 0));
            let due = t0 + STEP_HOLD + f64::from(n) * STEP_REPEAT;
            let n = if now >= due {
                stepped = true;
                n + 1
            } else {
                n
            };
            ui.data_mut(|d| d.insert_temp(key, (t0, n)));
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64((due - now).max(0.0)));
        } else {
            let n = ui
                .data_mut(|d| d.remove_temp::<(f64, u32)>(key))
                .map_or(0, |s| s.1);
            stepped = live && resp.clicked() && n == 0;
        }
        if stepped {
            *value = step_value(*value, up, &range, step, wrap);
        }
        let p = ui.painter();
        if held {
            gradient(
                p,
                r,
                3,
                theme::AMBER.lerp_to_gamma(Color32::WHITE, 0.2),
                theme::AMBER,
                true,
            );
        } else {
            let fill = if live && resp.hovered() {
                theme::RAISED_HI
            } else {
                theme::RAISED
            };
            p.rect_filled(r, 3, fill);
        }
        let ink = match (live, held) {
            (false, _) => theme::TEXT_FAINT,
            (_, true) => theme::INK,
            _ => theme::TEXT,
        };
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            glyph,
            theme::bold(18.0),
            ink,
        );
    }
    let shown = rect.shrink2(vec2(h + 4.0, 0.0));
    centered_text(ui.painter(), shown, text, theme::bold(15.0), theme::TEXT);
    ui.painter().rect_stroke(
        rect,
        theme::RADIUS,
        Stroke::new(1.0, theme::RIM),
        StrokeKind::Inside,
    );
    *value != old
}

/// A filter chip: a pill that toggles. `color` marks a position chip (GK yellow, DF blue ...).
pub fn chip(ui: &mut Ui, text: &str, on: bool, color: Option<Color32>) -> Response {
    let font = theme::display(15.0);
    let g = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let size = vec2(g.size().x + 22.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, on, text));
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

/// A list filter: an outlined pill with a check box (`radio`: a round mark, one of several) that fills blue with a
/// tick when on, so it never reads as a tab or a button.
pub fn check_chip(ui: &mut Ui, text: &str, on: bool, radio: bool) -> Response {
    let font = theme::bold(13.0);
    let g = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let size = vec2(g.size().x + 40.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let kind = if radio {
        Role::RadioButton
    } else {
        Role::CheckBox
    };
    resp.widget_info(|| WidgetInfo::selected(kind, true, on, text));
    let p = ui.painter();
    let hot = resp.hovered() || resp.has_focus();
    let (fill, rim, ink) = if on {
        (theme::BLUE.gamma_multiply(0.25), theme::BLUE, theme::TEXT)
    } else if hot {
        (theme::RAISED_HI, theme::RIM_HI, theme::TEXT)
    } else {
        (theme::WELL, theme::RIM, theme::TEXT_DIM)
    };
    p.rect_filled(rect, 13, fill);
    p.rect_stroke(rect, 13, Stroke::new(1.0, rim), StrokeKind::Inside);
    let c = pos2(rect.left() + 16.0, rect.center().y);
    let mark = Rect::from_center_size(c, vec2(14.0, 14.0));
    let r = if radio { 7 } else { 3 };
    if on {
        p.rect_filled(mark, r, theme::BLUE);
        if radio {
            p.circle_filled(c, 3.0, Color32::WHITE);
        } else {
            let tick = [
                c + vec2(-4.0, 0.0),
                c + vec2(-1.0, 3.0),
                c + vec2(4.0, -3.5),
            ];
            p.line(tick.to_vec(), Stroke::new(2.0, Color32::WHITE));
        }
    } else {
        p.rect_stroke(mark, r, Stroke::new(1.5, theme::RIM_HI), StrokeKind::Inside);
    }
    p.galley(
        pos2(rect.left() + 30.0, rect.center().y - g.size().y / 2.0),
        g,
        ink,
    );
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

/// The room [`status_line`] takes around its text: left (the mark's place), right, top and bottom.
pub const STATUS_LINE_MARGIN: [i8; 4] = [34, 10, 7, 7];

/// One message line: a tone bar, a mark and the text (wrapped if it must).
pub fn status_line(ui: &mut Ui, tone: Tone, text: &str) -> Response {
    let col = tone.color();
    let [left, right, top, bottom] = STATUS_LINE_MARGIN;
    let r = egui::Frame::new()
        .fill(col.gamma_multiply(0.12))
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin {
            left,
            right,
            top,
            bottom,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // left-aligned even in a justified column: a long path must not spread the words across the line
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.label(
                    egui::RichText::new(text)
                        .color(theme::TEXT)
                        .font(theme::body(14.0)),
                );
            });
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
/// The six stat bars of `card`, or for a legend one block as tall that says `secret` (SECRET): its stats are not printed.
pub fn stat_bars(
    ui: &mut Ui,
    card: &super::board::Card,
    names: &[String; 6],
    secret: &str,
    width: f32,
) {
    if card.secret {
        let h = 6.0 * 22.0 + 5.0 * ui.spacing().item_spacing.y;
        let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 4, theme::WELL);
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            secret,
            theme::display(28.0),
            theme::TEXT_DIM,
        );
        return;
    }
    for (k, v) in names.iter().zip(card.stats) {
        stat_bar(ui, k, v, width);
    }
}

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

/// A start's six steps in seconds on a PC that has not measured its own yet (the server's scenes, the server up, the
/// projector, the cabinet, the client's boot to its window, the panels): the bar's weights until it has.
pub const START_SECS: [f32; 6] = [2.0, 3.0, 1.0, 1.0, 3.0, 6.0];

/// The start bar's fill after `done` step lines, `stage_secs` into the next stage: the stages weigh what they take on
/// this PC (`secs`, all positive), and inside one the fill creeps toward the next boundary, never reaching it before
/// the line arrives.
pub fn start_fraction(done: usize, stage_secs: f32, secs: [f32; 6]) -> f32 {
    let total: f32 = secs.iter().sum::<f32>().max(0.001);
    let before: f32 = secs.iter().take(done).sum();
    match secs.get(done) {
        Some(d) => (before + d * 0.97 * (1.0 - (-stage_secs / d).exp())) / total,
        None => 1.0,
    }
}

/// A game start: the time-weighted bar (a light band runs over its fill while a step is on) and the elapsed time
/// (`clock` = seconds in this stage, in all).
pub fn start_bar(
    ui: &mut Ui,
    done: usize,
    clock: (f32, f32),
    secs: [f32; 6],
    width: f32,
) -> Response {
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(50));
    let s = clock.1 as u32;
    let label = format!("{}:{:02}", s / 60, s % 60);
    bar(
        ui,
        start_fraction(done, clock.0, secs),
        width,
        label,
        Some(clock.1),
    )
}

/// A job that reports no progress (a setup, the game text): a band sweeps the bar beside the time since it began,
/// instead of a percentage nobody knows.
pub fn working(ui: &mut Ui, secs: f32, width: f32) -> Response {
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(50));
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());
    let p = ui.painter();
    let bar = rect.with_max_x(rect.right() - 44.0);
    p.rect_filled(bar, 3, theme::WELL);
    let band = bar.width() * 0.25;
    // one sweep in 2 s, entering at the left and leaving at the right
    let x = bar.left() - band + (secs * 0.5).fract() * (bar.width() + band);
    let f = Rect::from_x_y_ranges(
        x.max(bar.left())..=(x + band).min(bar.right()),
        bar.y_range(),
    );
    gradient(
        p,
        f,
        3,
        theme::GREEN.lerp_to_gamma(Color32::WHITE, 0.2),
        theme::GREEN,
        true,
    );
    let s = secs as u32;
    p.text(
        rect.right_center(),
        Align2::RIGHT_CENTER,
        format!("{}:{:02}", s / 60, s % 60),
        theme::display(17.0),
        theme::TEXT,
    );
    resp
}

/// A plain progress bar with a percentage, for a download or an install stage.
pub fn progress(ui: &mut Ui, fraction: f32, width: f32) -> Response {
    bar(
        ui,
        fraction,
        width,
        format!("{:.0} %", fraction * 100.0),
        None,
    )
}

/// The band a bar's `sweep` shows over its fill at `secs` since the step began: it enters at the fill's left, runs over
/// it and leaves at its right, one sweep in 2 s; off the fill it is empty, so the bar looks alive, never fuller.
fn sweep_rect(fill: Rect, secs: f32) -> Rect {
    let band = fill.width() * 0.5;
    let x = fill.left() - band + (secs * 0.5).fract() * (fill.width() + band);
    Rect::from_x_y_ranges(x..=(x + band), fill.y_range()).intersect(fill)
}

/// A bar; `sweep` (seconds since the step began) runs a light band over the fill, for a wait whose end is not known.
fn bar(ui: &mut Ui, fraction: f32, width: f32, label: String, sweep: Option<f32>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());
    let p = ui.painter();
    let bar = rect.with_max_x(rect.right() - 44.0);
    p.rect_filled(bar, 3, theme::WELL);
    let fill = fraction.clamp(0.0, 1.0);
    let f = Rect::from_min_size(bar.min, vec2(bar.width() * fill, bar.height()));
    gradient(
        p,
        f,
        3,
        theme::GREEN.lerp_to_gamma(Color32::WHITE, 0.2),
        theme::GREEN,
        true,
    );
    if let Some(secs) = sweep.filter(|_| fill < 1.0) {
        let s = sweep_rect(f, secs);
        if s.width() > 0.0 {
            gradient(
                p,
                s,
                3,
                Color32::TRANSPARENT,
                Color32::from_white_alpha(30),
                true,
            );
        }
    }
    p.text(
        rect.right_center(),
        Align2::RIGHT_CENTER,
        label,
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
    let drawn = match state {
        KeyState::Capturing => tr!(lang, "keys.press_a_key"),
        KeyState::Empty => "—".to_owned(),
        _ => text.to_owned(),
    };
    // the name a screen reader reads is the cell's own drawn text ("press a key", "—"), not the empty binding
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &drawn));
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
pub fn title_bar(ui: &mut Ui, edition: Edition, version: &str, lang: &mut Lang) -> TitleAction {
    let rect = ui.max_rect().with_max_y(ui.max_rect().top() + 44.0);
    let drag = ui.interact(rect, Id::unique("title-drag"), Sense::click_and_drag());
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
    // the edition badge, then the Forza-WCCF version beside it on the badge's line, shown on every tab (a version
    // that is not a number, "not known", is left out: the UPDATE tab says it)
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        pos2(
            rect.left() + 12.0 + 34.0 + name_w + 10.0,
            rect.center().y - 11.0,
        ),
        vec2(120.0, 22.0),
    )));
    let pill = badge(&mut b, edition);
    if version.starts_with(|c: char| c.is_ascii_digit()) {
        ui.painter().text(
            pos2(pill.rect.right() + 8.0, pill.rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("v{version}"),
            theme::display(15.0),
            theme::TEXT_DIM,
        );
    }
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        action = window_buttons(ui);
        ui.add_space(10.0);
        let mut i = Lang::ALL.iter().position(|l| l == lang).unwrap_or(0);
        // narrow enough for the version beside the badge in the 600 px Launcher
        ui.allocate_ui(vec2(216.0, 30.0), |ui| {
            // each language in its own words, whatever the UI language
            if segmented(ui, &mut i, &["English", "Italiano", "日本語"], 216.0) {
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

/// Minimize and close, drawn right to left from the current position (call inside a right-to-left layout).
pub fn window_buttons(ui: &mut Ui) -> TitleAction {
    let mut action = TitleAction::None;
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
            resp.widget_info(|| WidgetInfo::selected(Role::DisclosureTriangle, true, *open, title));
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

/// The width of a secondary action's label.
pub fn text_w(ui: &Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), theme::bold(14.0), theme::TEXT)
        .size()
        .x
}

/// A field label of a fixed width in front of a control, so the controls of a section line up; the response names
/// that control for screen readers (`labelled_by`).
pub fn field(ui: &mut Ui, label: &str, width: f32) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, label));
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
    resp
}

/// An option as a row: a painted switch, a title and a hint, wrapped onto more lines when it needs them (the row grows).
/// The whole row toggles.
pub fn toggle_row(ui: &mut Ui, on: &mut bool, title: &str, hint: &str) -> Response {
    let w = ui.available_width();
    let x = 6.0 + 38.0 + 12.0; // the switch's margin, its width, the gap
    let hint = ui
        .painter()
        .layout(hint.to_owned(), theme::body(12.0), theme::TEXT_DIM, w - x);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(w, (27.0 + hint.size().y).max(44.0)), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, *on, title));
    if resp.clicked() {
        *on = !*on;
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, theme::RADIUS, theme::RAISED.gamma_multiply(0.6));
    }
    let sw = Rect::from_min_size(pos2(rect.left() + 6.0, rect.top() + 12.0), vec2(38.0, 20.0));
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
    let x = rect.left() + x;
    let g = fit(p, title, theme::bold(14.0), theme::TEXT, rect.right() - x);
    p.galley(pos2(x, rect.top() + 3.0), g, theme::TEXT);
    p.galley(pos2(x, rect.top() + 23.0), hint, theme::TEXT_DIM);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stat scale the catalogue and the board share: 1-9 dim, 10-14 cyan, 15-17 green, 18-20 amber.
    #[test]
    fn stat_colours_follow_the_scale() {
        let dim = Color32::from_rgb(0x4A, 0x7B, 0xA8);
        for (v, c) in [
            (1, dim),
            (9, dim),
            (10, theme::CYAN),
            (14, theme::CYAN),
            (15, theme::GREEN),
            (17, theme::GREEN),
            (18, theme::AMBER),
            (20, theme::AMBER),
        ] {
            assert_eq!(stat_color(v), c, "{v}");
        }
    }
}
