use super::*;
use egui::epaint::Shadow;

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
    cabinet_shaped(ui, size, label, kind, Shape::Key, lit, true, false)
}

/// `cabinet` in any shape. Only a key or a shape at least 50 px across prints its label inside, unless `inside` is
/// false (the caller prints it beside); the caller prints the others' beside them. `label` is the accessible name
/// either way. `key_down`: its key is held, so it draws pressed as under the mouse.
#[allow(clippy::too_many_arguments)]
pub fn cabinet_shaped(
    ui: &mut Ui,
    size: egui::Vec2,
    label: &str,
    kind: Cab,
    shape: Shape,
    lit: bool,
    inside: bool,
    key_down: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(Role::Button, enabled, lit, label));
    if ui.is_rect_visible(rect) {
        let ctx = ui.ctx();
        let press = ctx.animate_bool_with_time(
            resp.id.with("press"),
            resp.is_pointer_button_down_on() || key_down,
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
            if inside { label } else { "" },
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

/// `cabinet_face` in any shape. The deck's shapes show the cabinet art in `assets/panel`; others are solid colour on
/// a darker skirt, with no lamp window.
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

/// The cabinet art for a deck button (`assets/panel`, cut from the project's button drawings), if it has one.
fn art(kind: Cab, shape: Shape) -> Option<egui::ImageSource<'static>> {
    use egui::include_image as img;
    Some(match (kind, shape) {
        (Cab::Tactic, Shape::Tri(Dir::Up)) => img!("../../assets/panel/arrow_up.png"),
        (Cab::Tactic, Shape::Tri(Dir::Down)) => img!("../../assets/panel/arrow_down.png"),
        (Cab::Tactic, Shape::Tri(Dir::Left)) => img!("../../assets/panel/arrow_left.png"),
        (Cab::Tactic, Shape::Tri(Dir::Right)) => img!("../../assets/panel/arrow_right.png"),
        (Cab::Press, Shape::Square) => img!("../../assets/panel/press.png"),
        (Cab::Shoot, Shape::Round) => img!("../../assets/panel/shoot.png"),
        (Cab::Keeper, Shape::Round) => img!("../../assets/panel/keeper.png"),
        (Cab::KeyPlayer, Shape::Round) => img!("../../assets/panel/key_player.png"),
        (Cab::Go, Shape::Round) => img!("../../assets/panel/start.png"),
        (Cab::Data, Shape::Square) => img!("../../assets/panel/data.png"),
        _ => return None,
    })
}

/// How much a deck button's art is brightened (0..1) and its tint: the toggle lamps (arrows, PRESS) dim when off and
/// show full colour when lit; the action buttons rest at full colour and brighten while pressed or held.
fn art_light(kind: Cab, f: Face) -> (Color32, f32) {
    if !f.enabled {
        return (Color32::from_gray(0x60), 0.0);
    }
    if matches!(kind, Cab::Tactic | Cab::Press) {
        let tint = Color32::from_gray(0x55).lerp_to_gamma(Color32::WHITE, f.glow);
        (tint, 0.1 * f.hover)
    } else {
        let up = (0.6 * f.press).max(0.35 * f.glow) + 0.1 * f.hover;
        (Color32::WHITE, up.min(1.0))
    }
}

const SMOOTH_ART: &str = "smooth_deck_art";

/// Whether the deck draws its art resampled to the exact drawn pixel size (2017-18). The game's painter samples
/// textures bilinearly without mipmaps, so art about twice its drawn size looks jagged; 2010-11 keeps egui's loader.
pub fn smooth_deck_art(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(Id::unique(SMOOTH_ART), on));
}

/// Decoded art (premultiplied) and its textures, one per art and drawn pixel size.
#[derive(Clone, Default)]
struct ArtCache {
    decoded: std::collections::HashMap<&'static str, Arc<image::Rgba32FImage>>,
    sized: std::collections::HashMap<(&'static str, [usize; 2]), egui::TextureHandle>,
}

/// `src` resampled (Lanczos3, premultiplied so transparent edges keep no dark fringe) to the physical pixel size of
/// `rect` at `ppp`; made once per size and reused.
fn sized_art(
    ctx: &egui::Context,
    src: &egui::ImageSource<'static>,
    rect: Rect,
    ppp: f32,
) -> Option<egui::TextureHandle> {
    let egui::ImageSource::Bytes {
        uri: std::borrow::Cow::Borrowed(uri),
        bytes: egui::load::Bytes::Static(bytes),
    } = src
    else {
        return None;
    };
    let size = [
        (rect.width() * ppp).round() as usize,
        (rect.height() * ppp).round() as usize,
    ];
    if size.contains(&0) {
        return None;
    }
    let id = Id::unique(SMOOTH_ART);
    let (cached, decoded) = ctx.data_mut(|d| {
        let c = d.get_temp_mut_or_default::<ArtCache>(id);
        (
            c.sized.get(&(*uri, size)).cloned(),
            c.decoded.get(uri).cloned(),
        )
    });
    if cached.is_some() {
        return cached;
    }
    let decoded = match decoded {
        Some(d) => d,
        None => {
            let mut img = image::load_from_memory(bytes).ok()?.to_rgba32f();
            for px in img.pixels_mut() {
                let a = px[3];
                px[0] *= a;
                px[1] *= a;
                px[2] *= a;
            }
            Arc::new(img)
        }
    };
    let small = image::imageops::resize(
        &*decoded,
        size[0] as u32,
        size[1] as u32,
        image::imageops::FilterType::Lanczos3,
    );
    // Lanczos rings: clamp so each colour stays within its alpha (valid premultiplied)
    let rgba: Vec<u8> = small
        .pixels()
        .flat_map(|px| {
            let a = px[3].clamp(0.0, 1.0);
            let c = |v: f32| (v.clamp(0.0, a) * 255.0).round() as u8;
            [c(px[0]), c(px[1]), c(px[2]), (a * 255.0).round() as u8]
        })
        .collect();
    let tex = ctx.load_texture(
        format!("{uri}@{}x{}", size[0], size[1]),
        egui::ColorImage::from_rgba_premultiplied(size, &rgba),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| {
        let c = d.get_temp_mut_or_default::<ArtCache>(id);
        if c.sized.len() >= 32 {
            c.sized.clear(); // a window resize leaves old sizes behind
        }
        c.sized.insert((*uri, size), tex.clone());
        c.decoded.insert(uri, decoded);
    });
    Some(tex)
}

fn solid_face(p: &Painter, rect: Rect, label: &str, kind: Cab, shape: Shape, f: Face) {
    // the art, once its loader has it (the first frames draw the plain shape); it carries no label
    let smooth = p.ctx().data(|d| d.get_temp::<bool>(Id::unique(SMOOTH_ART))) == Some(true);
    let tex = art(kind, shape).and_then(|src| {
        if smooth {
            sized_art(p.ctx(), &src, rect, p.ctx().pixels_per_point()).map(|t| t.id())
        } else if let Ok(egui::load::TexturePoll::Ready { texture }) = src.load(
            p.ctx(),
            egui::TextureOptions::LINEAR,
            egui::SizeHint::default(),
        ) {
            Some(texture.id)
        } else {
            None
        }
    });
    if let Some(tex) = tex {
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let (tint, up) = art_light(kind, f);
        p.image(tex, rect, uv, tint);
        if up > 0.0 {
            // the art again with zero alpha: premultiplied blending adds it, so the face brightens inside its outline
            let k = (255.0 * up) as u8;
            p.image(tex, rect, uv, Color32::from_rgba_premultiplied(k, k, k, 0));
        }
        return;
    }
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
    // only the tactic lamps grey out when off (lit = full colour); the action buttons keep their colour at rest
    let lamp = matches!(kind, Cab::Tactic | Cab::Press);
    let tone = if lamp && !f.lit {
        base.lerp_to_gamma(Color32::from_gray(0x80), 0.65)
    } else {
        base
    };
    let wash = if !lamp && f.lit { 0.3 } else { 0.1 * f.hover };
    let fill = tone.lerp_to_gamma(Color32::WHITE, wash);
    let rim = Stroke::new(1.0, tone.lerp_to_gamma(Color32::BLACK, 0.4));
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
        tone.lerp_to_gamma(Color32::BLACK, 0.55),
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

#[cfg(test)]
mod shape_tests {
    use super::*;

    /// A start bar's band runs over its fill while a step is on, and never wider than the fill.
    #[test]
    fn the_start_bar_sweeps_its_fill_and_never_grows_it() {
        let fill = Rect::from_min_size(pos2(10.0, 0.0), vec2(200.0, 14.0));
        assert_eq!(sweep_rect(fill, 0.0).width(), 0.0, "it starts off the fill");
        let mut seen = 0;
        for i in 0..40 {
            let s = sweep_rect(fill, i as f32 * 0.1);
            if s.width() > 0.0 {
                assert!(fill.contains_rect(s), "{s:?} is not inside the fill");
                seen += 1;
            }
        }
        assert!(
            seen >= 20,
            "the band is over the fill half of the time ({seen})"
        );
    }

    #[test]
    fn start_bar_creeps_and_never_passes_the_next_stage() {
        // this PC's measured steps, the estimate, and a start whose steps ran far from any guess
        for secs in [START_SECS, [1.0; 6], [0.3, 9.0, 2.0, 0.4, 5.5, 3.0]] {
            assert_eq!(start_fraction(0, 0.0, secs), 0.0);
            for done in 1..6 {
                let at_line = start_fraction(done, 0.0, secs);
                let crept = start_fraction(done, 1e4, secs);
                assert!(crept >= at_line, "the fill never goes back ({done})");
                assert!(
                    crept < start_fraction(done + 1, 0.0, secs),
                    "the fill stays below the next mark until its line ({done})"
                );
                assert!(start_fraction(done, 2.0, secs) <= crept);
            }
            assert_eq!(start_fraction(6, 0.0, secs), 1.0);
            assert_eq!(start_fraction(6, 99.0, secs), 1.0);
        }
    }

    #[test]
    fn stepper_steps_snap_clamp_and_wrap() {
        let min = 8..=150;
        let walk = |mut v, up, n| {
            (0..n)
                .map(|_| {
                    v = step_value(v, up, &min, 5, false);
                    v
                })
                .collect::<Vec<u16>>()
        };
        assert_eq!(walk(8, true, 3), [10, 15, 20]);
        assert_eq!(walk(15, false, 3), [10, 8, 8]);
        // a hand-edited 12 snaps to the 5s; the top holds
        assert_eq!(walk(12, true, 1), [15]);
        assert_eq!(walk(12, false, 1), [10]);
        assert_eq!(walk(145, true, 2), [150, 150]);
        let day = 0..=24 * 60 - 1;
        assert_eq!(step_value(23 * 60 + 55, true, &day, 5, true), 0);
        assert_eq!(step_value(0, false, &day, 5, true), 23 * 60 + 55);
        assert_eq!(step_value(21 * 60 + 2, true, &day, 5, true), 21 * 60 + 5);
    }

    /// A click steps once; holding + repeats after the hold delay and the release adds nothing.
    #[test]
    fn stepper_click_and_hold() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let v = std::cell::Cell::new(8u16);
        let mut fresh = true;
        let mut h = Harness::builder().with_step_dt(0.05).build_ui(|ui| {
            if fresh {
                // the bold face is ours: it takes effect from the next frame
                crate::theme::install(ui.ctx(), Edition::E1718, &crate::theme::font_dir()).unwrap();
                fresh = false;
                return;
            }
            let mut x = v.get();
            let text = format!("{x} min");
            stepper(
                ui,
                &mut x,
                8..=150,
                5,
                false,
                &text,
                ["earlier", "later"],
                190.0,
            );
            v.set(x);
        });
        h.run();
        h.get_by_label("later").click();
        h.run();
        assert_eq!(v.get(), 10);
        h.get_by_label("earlier").click();
        h.run();
        assert_eq!(v.get(), 8);
        // at the floor − does nothing
        h.get_by_label("earlier").click();
        h.run();
        assert_eq!(v.get(), 8);
        let at = h.get_by_label("later").rect().center();
        h.hover_at(at);
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        // about 1 s held (frames of 0.05 s): repeats from 0.4 s, every 0.1 s, so several steps but not the whole range
        for _ in 0..20 {
            h.step();
        }
        let held = v.get();
        assert!((20..=45).contains(&held), "{held}");
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
        h.run();
        assert_eq!(v.get(), held, "the release after a hold is no extra step");
    }

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

    /// The 2017-18 deck art is resampled to exactly its drawn pixel size, once per size.
    #[test]
    fn deck_art_matches_its_drawn_pixels_and_is_cached() {
        let ctx = egui::Context::default();
        let src = art(Cab::Shoot, Shape::Round).unwrap();
        let rect = Rect::from_min_size(pos2(10.3, 20.0), vec2(90.0, 91.0));
        for (ppp, want) in [(1.0, [90, 91]), (1.25, [113, 114])] {
            let t = sized_art(&ctx, &src, rect, ppp).unwrap();
            assert_eq!(t.size(), want);
            let n = ctx.tex_manager().read().num_allocated();
            let again = sized_art(&ctx, &src, rect, ppp).unwrap();
            assert_eq!(again.id(), t.id());
            assert_eq!(ctx.tex_manager().read().num_allocated(), n);
        }
    }

    #[test]
    fn art_lamps_dim_and_actions_brighten() {
        let f = |lit: bool, press: f32| Face {
            lit,
            glow: if lit { 1.0 } else { 0.0 },
            press,
            hover: 0.0,
            enabled: true,
        };
        // a toggle lamp off is darker than lit; pressing it does not brighten it
        let (off, _) = art_light(Cab::Tactic, f(false, 0.0));
        let (on, up) = art_light(Cab::Press, f(true, 1.0));
        assert!(off.r() < on.r() && up == 0.0);
        // an action button rests at full colour and brightens while pressed
        for kind in [Cab::Shoot, Cab::Keeper, Cab::KeyPlayer, Cab::Go, Cab::Data] {
            let (rest, still) = art_light(kind, f(false, 0.0));
            let (_, pressed) = art_light(kind, f(false, 1.0));
            assert!(
                rest == Color32::WHITE && still == 0.0 && pressed > 0.3,
                "{kind:?}"
            );
            assert!(
                art(
                    kind,
                    if kind == Cab::Data {
                        Shape::Square
                    } else {
                        Shape::Round
                    }
                )
                .is_some()
            );
        }
    }
}
