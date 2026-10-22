//! Player cards and the FORMATION BOARD: the card face (upright), the rarity marks, and the pitch where cards lie
//! anywhere, as on the cabinet's table (no fixed spots), and are dragged, swapped, taken off and shown big on a
//! resting mouse.

use super::Lang;
use super::theme;
use super::widgets::{centered_text, fan, fit, gradient};
use crate::cards::hq::{self, Hq};
use crate::cards::links::Affinity;
use egui::{
    Align2, Color32, Id, Order, Painter, Pos2, Rect, Response, Sense, Shadow, Stroke, StrokeKind,
    Ui, Vec2, pos2, vec2,
};
use std::collections::HashMap;
use std::sync::Arc;

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
    pub number: u32,
    /// The card's record in the game's player_data.bin (the catalogue's record_index): its links' key.
    pub record: Option<u32>,
    pub name: String,
    /// The name the card's face shows (the catalogue's short name); empty = the full name's last word.
    pub short: String,
    pub club: String,
    pub country: String,
    pub season: String,
    pub pos: Position,
    pub rarity: Rarity,
    /// The code printed on the card (the catalogue's card_code: HOLE, WGK-3, ATLE-RE-4 ...); empty for 2010-11.
    pub code: String,
    /// A card the 2017-18 game treats as a legend: its back prints SECRET for the six stats (they grow by training).
    pub secret: bool,
    pub stats: [u8; 6],
    /// TOTAL as the card prints it.
    pub total: u8,
    pub aptitude: Aptitude,
}

impl Card {
    /// The card's series: its printed code without the trailing number (WGK-3 -> WGK, MVP-1 -> MVP, KLE stays); a
    /// code of digits only gives none.
    pub fn series(&self) -> &str {
        series(&self.code)
    }

    pub fn short_name(&self) -> &str {
        if self.short.is_empty() {
            self.name.rsplit(' ').next().unwrap_or(&self.name)
        } else {
            &self.short
        }
    }
}

/// [`Card::series`] of a code.
pub fn series(code: &str) -> &str {
    code.trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches(['-', ' '])
}

// ---- suitable positions ----

/// The 16 roles of a card's position ratings, in the order the original files keep them. The names are inferred by
/// the project from the players rated 10 in each, not the game's own.
pub const ROLES: [&str; 16] = [
    "CF", "TM", "WG", "SS", "OMF", "OH", "CMF", "DMF", "SMF", "WB", "SB", "CB", "STP", "CVR", "SW",
    "GK",
];

/// The position areas of the game's own table (as the fan database maps them): left, top, width, height in 0..1 of
/// the field, attack at the top, own goal at the bottom; the roles each area rates (the best of them counts) and its
/// line. Big areas first, so the nested ones (SS in CF, GK in CB) come later and win: [`Aptitude::at`] reads the last
/// area holding a point. CB's area holds SW's, SH's holds WB's (the game tells them apart by the rest of the team).
const AREAS: [([f32; 4], &[usize], Position); 13] = [
    (
        [0.206, 0.643, 0.588, 0.357],
        &[11, 12, 13, 14],
        Position::Df,
    ), // CB STP CVR SW
    ([0.0, 0.643, 0.206, 0.357], &[10], Position::Df), // SB
    ([0.794, 0.643, 0.206, 0.357], &[10], Position::Df),
    ([0.206, 0.519, 0.588, 0.124], &[7], Position::Mf), // DMF
    ([0.206, 0.386, 0.588, 0.133], &[6], Position::Mf), // CMF
    ([0.206, 0.262, 0.588, 0.124], &[5], Position::Mf), // OH
    ([0.206, 0.0, 0.588, 0.262], &[0, 1], Position::Fw), // CF TM
    ([0.0, 0.0, 0.206, 0.262], &[2], Position::Fw),     // WG
    ([0.794, 0.0, 0.206, 0.262], &[2], Position::Fw),
    ([0.206, 0.081, 0.588, 0.181], &[3, 4], Position::Fw), // SS OMF
    ([0.0, 0.262, 0.206, 0.381], &[8, 9], Position::Mf),   // SMF WB
    ([0.794, 0.262, 0.206, 0.381], &[8, 9], Position::Mf),
    ([0.4, 0.9, 0.2, 0.1], &[15], Position::Gk), // GK
];

/// An area's rect within `field`.
fn area_rect(field: Rect, [x, y, w, h]: [f32; 4]) -> Rect {
    Rect::from_min_size(field.lerp_inside(vec2(x, y)), field.size() * vec2(w, h))
}

/// A card's suitable positions, as the back of the original card shows them: 16 role ratings in [`ROLES`] order, each
/// 1..10; 0 is no data (editions 36+ get flat 8s from their position flags, see [`Aptitude::from_flags`]).
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
    /// From the 12 position flags as 24 hex digits (CF SS WG OH CH DH SH WB SB CB SW GK, one byte each): every set
    /// flag rates its role 8, flat. Left over from the flags: TM, OMF, STP, CVR stay 0.
    pub fn from_flags(hex: &str) -> Aptitude {
        // flag -> index in ROLES
        const AT: [usize; 12] = [0, 3, 2, 5, 6, 7, 8, 9, 10, 11, 14, 15];
        let mut a = Aptitude::default();
        for (i, &role) in AT.iter().enumerate() {
            if hex.get(i * 2..i * 2 + 2).is_some_and(|b| b != "00") {
                a.roles[role] = 8;
            }
        }
        a
    }

    pub fn known(&self) -> bool {
        self.roles.iter().any(|&r| r > 0)
    }

    /// The best role of an area and its rating (the first named on a tie).
    fn best(&self, roles: &[usize]) -> (&'static str, u8) {
        let i = roles
            .iter()
            .copied()
            .rev()
            .max_by_key(|&i| self.roles[i])
            .unwrap_or(0);
        (ROLES[i], self.roles[i])
    }

    /// The rating at a field point (0..1 as in [`AREAS`]): the best role of the last area holding it.
    pub fn at(&self, p: Pos2) -> f32 {
        AREAS
            .iter()
            .rev()
            .find(|([x, y, w, h], ..)| Rect::from_min_size(pos2(*x, *y), vec2(*w, *h)).contains(p))
            .map_or(0.0, |(_, roles, _)| self.best(roles).1 as f32)
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

// ---- card pictures ----

/// Names the folder of card pictures, `<number>.png` each (the kit's `data\cache\cards`, extracted at setup on the
/// player's PC), or none for the drawn stand-in. Pictures load in the background through egui's loaders and stay
/// cached, so scrolling the catalogue never waits on a file.
pub fn set_picture_dir(ctx: &egui::Context, dir: Option<std::path::PathBuf>) {
    ctx.data_mut(|d| d.insert_temp(Id::new("card-pictures"), dir));
}

/// The card face in a picture of `size`, whole (never cut): a square picture is the game's 128x128 one, its face
/// the left 88 columns; any other is the face itself (the pack's scans). Returns its uv part and its aspect.
fn face_uv(size: Vec2) -> (Rect, f32) {
    let fw = if size.x == size.y { 88.0 / 128.0 } else { 1.0 };
    (
        Rect::from_min_max(Pos2::ZERO, pos2(fw, 1.0)),
        size.x * fw / size.y,
    )
}

/// The largest rect of `aspect` (width / height) centred in `r`: the whole picture, with bars where shapes differ.
fn letterbox(r: Rect, aspect: f32) -> Rect {
    let w = r.width().min(r.height() * aspect);
    Rect::from_center_size(r.center(), vec2(w, w / aspect))
}

fn picture_dir(ctx: &egui::Context) -> Option<std::path::PathBuf> {
    ctx.data(|d| d.get_temp::<Option<std::path::PathBuf>>(Id::new("card-pictures")))
        .flatten()
}

/// The card's picture file `<number>.<ext>` in the folder as a ready texture; `None` while it loads or when there is
/// no such file (a miss is remembered: the folder is made once, at setup).
fn load_picture(
    ctx: &egui::Context,
    dir: &std::path::Path,
    number: u32,
    ext: &str,
) -> Option<(egui::TextureId, Vec2)> {
    let file = dir.join(format!("{number}.{ext}"));
    // `file:///C:\...` on Windows, `file:///home/...` elsewhere: what egui_extras' file loader parses
    let uri = format!(
        "file:///{}",
        file.display().to_string().trim_start_matches('/')
    );
    let miss = Id::new(("card-picture-miss", &uri));
    if ctx.data(|d| d.get_temp::<bool>(miss)).is_some() {
        return None;
    }
    let loaded = ctx.try_load_texture(
        &uri,
        egui::TextureOptions::LINEAR,
        egui::SizeHint::default(),
    );
    if loaded.is_err() {
        ctx.data_mut(|d| d.insert_temp(miss, true));
    }
    match loaded {
        Ok(egui::load::TexturePoll::Ready { texture }) => Some((texture.id, texture.size)),
        _ => None,
    }
}

/// The pack's real values by card number (`hq_stats.tsv` beside the pictures), read once per folder.
fn hq_map(ctx: &egui::Context) -> Option<std::sync::Arc<std::collections::HashMap<u32, Hq>>> {
    let dir = picture_dir(ctx)?;
    let id = Id::new(("card-hq", &dir));
    if let Some(m) =
        ctx.data(|d| d.get_temp::<std::sync::Arc<std::collections::HashMap<u32, Hq>>>(id))
    {
        return Some(m);
    }
    let m = std::sync::Arc::new(hq::load(&dir.join(hq::FILE)));
    ctx.data_mut(|d| d.insert_temp(id, m.clone()));
    Some(m)
}

/// The card with the pack's real position, stats and TOTAL when it has them (the game catalogue's numbers
/// otherwise): what the board draws and lists.
pub fn real<'a>(ctx: &egui::Context, card: &'a Card) -> std::borrow::Cow<'a, Card> {
    let Some(h) = hq_map(ctx).and_then(|m| m.get(&card.number).copied()) else {
        return std::borrow::Cow::Borrowed(card);
    };
    let mut c = card.clone();
    c.stats = h.stats;
    c.total = h.total;
    c.pos = match &h.pos {
        b"GK" => Position::Gk,
        b"DF" => Position::Df,
        b"MF" => Position::Mf,
        b"FW" => Position::Fw,
        _ => c.pos,
    };
    std::borrow::Cow::Owned(c)
}

/// The drawn stand-in for a card with no picture (still loading, no file, or no folder set).
fn card_picture(p: &Painter, r: Rect, rim: Color32, alpha: f32) {
    gradient(
        p,
        r,
        2,
        theme::RAISED_HI.gamma_multiply(alpha),
        theme::DEEP.gamma_multiply(alpha),
        true,
    );
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

/// The card's printed series as a small dark chip ending at `right` (its right edge's middle), text `h` high; none
/// for an empty code.
pub fn code_chip(p: &Painter, right: Pos2, h: f32, code: &str, alpha: f32) {
    if code.is_empty() {
        return;
    }
    let g = p.layout_no_wrap(
        code.to_owned(),
        theme::display(h),
        theme::TEXT.gamma_multiply(alpha),
    );
    let pad = vec2(h * 0.35, h * 0.1);
    let r = Rect::from_min_size(
        right - vec2(g.size().x + 2.0 * pad.x, g.size().y / 2.0 + pad.y),
        g.size() + 2.0 * pad,
    );
    p.rect_filled(r, 3, Color32::from_black_alpha((180.0 * alpha) as u8));
    p.galley(r.min + pad, g, theme::TEXT);
}

/// How a card is drawn: at rest, lifted (dragged), a ghost left in its slot, a swap target, out of position.
#[derive(Clone, Copy, Debug, Default)]
pub struct Look {
    pub lifted: bool,
    /// In hand under the pointer: drawn a little see-through, so the card under it shows.
    pub held: bool,
    pub ghost: bool,
    pub target: bool,
    pub warn: bool,
    pub selected: bool,
}

/// The card face, upright at the real card's shape ([`CARD_ASPECT`]): the card's picture whole, never cut (the pack's
/// scan `<number>.jpg`, else the game's face `<number>.png`, letterboxed on a dark ground when its shape differs),
/// from the folder [`set_picture_dir`] names; else a drawn face: the position band with TOTAL, a stand-in, the name on
/// its foot, and on large ones the club and the season.
pub fn card_face(p: &Painter, rect: Rect, card: &Card, look: Look) {
    let card = &*real(p.ctx(), card);
    let big = rect.width() >= 90.0;
    let r = (rect.width() * 0.08) as u8;
    let alpha = if look.ghost {
        0.3
    } else if look.held {
        0.85
    } else {
        1.0
    };
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
    // a picture is the whole card face (its own band, photo, name and numbers): shown whole
    if let Some(dir) = picture_dir(p.ctx())
        && let Some((id, size)) = load_picture(p.ctx(), &dir, card.number, "jpg")
            .or_else(|| load_picture(p.ctx(), &dir, card.number, "png"))
    {
        let (uv, aspect) = face_uv(size);
        p.rect_filled(rect, r, theme::DEEP.gamma_multiply(alpha));
        p.image(
            id,
            letterbox(rect, aspect),
            uv,
            Color32::WHITE.gamma_multiply(alpha),
        );
        face_marks(p, rect, r, card, rim, alpha, look);
        return;
    }
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
        card.total.to_string(),
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
    card_picture(p, pic, rim, alpha);
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
        code_chip(
            p,
            rect.right_bottom() + vec2(-22.0, -11.0),
            rect.width() * 0.09,
            card.series(),
            alpha,
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
        code_chip(
            p,
            pos2(pic.right() - 13.0, pic.top() + 6.0),
            (rect.width() * 0.13).max(8.0),
            card.series(),
            alpha,
        );
    }
    face_marks(p, rect, r, card, rim, alpha, look);
}

/// The rim (amber when selected or a drop target) and the out-of-position corner, over the face or the scan.
fn face_marks(p: &Painter, rect: Rect, r: u8, card: &Card, rim: Color32, alpha: f32, look: Look) {
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

// ---- links ----

/// What the board knows of links: the club's link (0-255) between two of its cards, from the club card; the players'
/// affinity (1-5), from the game's own files ([`crate::cards::links`]). Empty: no lines, no lists.
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

/// The card table: 16 slots, 0-10 on the pitch, 11-15 the bench's five boxes; each holds a card index or nothing. A
/// pitch slot is not a place: its card lies at its spot, anywhere on the pitch, and an empty pitch slot is not drawn.
#[derive(Clone, Debug)]
pub struct Board {
    pub cards: Vec<Card>,
    pub slots: [Option<usize>; 16],
    /// Where each pitch slot sits, 0..1 across the card lane and 0 (attack) .. 1 (own goal) down.
    pub spots: [Pos2; 11],
}

impl Board {
    /// The card number in a slot.
    pub fn number(&self, slot: usize) -> Option<u32> {
        self.slots[slot].map(|ci| self.cards[ci].number)
    }
}

impl Board {
    fn first_free(&self, slots: std::ops::Range<usize>) -> Option<usize> {
        slots.into_iter().find(|&s| self.slots[s].is_none())
    }

    /// A card on the table already (by number).
    fn holds(&self, number: u32) -> bool {
        (0..16).any(|s| self.number(s) == Some(number))
    }

    /// A card dragged within the board dropped on `slot`: the two trade places (an empty slot takes the card).
    pub fn swap(&mut self, from: usize, slot: usize) {
        self.slots.swap(from, slot);
    }

    /// A card dropped on open pitch lies exactly at `spot`: a pitch card moves there, a bench card goes on the pitch
    /// there (11 on the pitch already: it stays). Returns whether anything moved.
    pub fn move_to(&mut self, from: usize, spot: Pos2) -> bool {
        let to = if from < 11 {
            from
        } else if let Some(s) = self.first_free(0..11) {
            s
        } else {
            return false;
        };
        self.slots.swap(from, to);
        self.spots[to] = spot;
        true
    }

    /// A catalogue card dropped on `slot`: it takes the place, and the card there goes to the first free bench box
    /// (or off the table when the bench is full). A card already on the table is left alone.
    pub fn put(&mut self, card: &Card, slot: usize) -> bool {
        if self.holds(card.number) {
            return false;
        }
        self.cards.push(card.clone());
        if let Some(old) = self.slots[slot].take()
            && let Some(b) = self.first_free(11..16)
        {
            self.slots[b] = Some(old);
        }
        self.slots[slot] = Some(self.cards.len() - 1);
        true
    }

    /// A catalogue card dropped on open pitch lies exactly at `spot` (11 on the pitch already: the first free bench
    /// box); nothing when the table is full or holds the card already.
    pub fn put_free(&mut self, card: &Card, spot: Pos2) -> bool {
        if self.holds(card.number) {
            return false;
        }
        let Some(s) = self.first_free(0..16) else {
            return false;
        };
        self.cards.push(card.clone());
        self.slots[s] = Some(self.cards.len() - 1);
        if s < 11 {
            self.spots[s] = spot;
        }
        true
    }
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

/// A card's width over its height: the pack's scans (180x254, the printed card's shape; a 63x88 mm card is 0.716).
pub const CARD_ASPECT: f32 = 180.0 / 254.0;
/// The catalogue's and the widget sheet's card size; the board scales its cards with the pitch ([`card_size`]).
pub const CARD_SMALL: egui::Vec2 = egui::vec2(50.0, 50.0 / CARD_ASPECT);
/// The pitch is this many times as tall as it is wide.
pub const PITCH_ASPECT: f32 = 1.25;
/// The strip down the pitch's left edge that holds the zone names; no card reaches into it.
const LABEL_LANE: f32 = 26.0;
/// The bench's cards against the pitch's.
const BENCH_SCALE: f32 = 0.8;

/// A pitch card for a pitch this wide: the card's shape ([`CARD_ASPECT`]), about a seventh of the width.
pub fn card_size(pitch_w: f32) -> egui::Vec2 {
    let w = (pitch_w * 0.145).round();
    vec2(w, (w / CARD_ASPECT).round())
}

/// Where a pitch card sits: centred on its spot across the pitch right of the zone names, pushed back inside when its
/// edge would leave the pitch (the spot itself, what the table holds, is kept).
pub fn spot_rect(pitch: Rect, spot: Pos2) -> Rect {
    let lane = pitch.with_min_x(pitch.left() + LABEL_LANE);
    let f = field(pitch);
    let r = Rect::from_center_size(
        pos2(f.left() + spot.x * f.width(), f.top() + spot.y * f.height()),
        card_size(pitch.width()),
    );
    let inside = lane.shrink(4.0);
    let push = |lo: f32, hi: f32, min: f32, max: f32| (min - lo).max(0.0) + (max - hi).min(0.0);
    r.translate(vec2(
        push(r.left(), r.right(), inside.left(), inside.right()),
        push(r.top(), r.bottom(), inside.top(), inside.bottom()),
    ))
}

/// Where every slot's card sits: the 11 pitch slots at their spots (empty ones too, unused), the 5 bench slots in a row from `bench_top`
/// (the board puts the bench 12 points under the pitch).
pub fn slot_rects(pitch: Rect, bench_top: f32, spots: &[Pos2; 11]) -> [Rect; 16] {
    let mut rects = [Rect::NOTHING; 16];
    for (i, s) in spots.iter().enumerate() {
        rects[i] = spot_rect(pitch, *s);
    }
    let small = card_size(pitch.width()) * BENCH_SCALE;
    let row = Rect::from_min_size(
        pos2(pitch.left() + LABEL_LANE, bench_top),
        vec2(pitch.width() - LABEL_LANE, small.y + 12.0),
    );
    let gap = (row.width() - 5.0 * small.x) / 6.0;
    for b in 0..5 {
        rects[11 + b] =
            Rect::from_min_size(row.min + vec2(gap + b as f32 * (small.x + gap), 6.0), small);
    }
    rects
}

/// A sheet point on `pitch` as a spot (the inverse of [`spot_rect`]'s centre, kept on the pitch).
fn spot_at(pitch: Rect, p: Pos2) -> Pos2 {
    let f = field(pitch);
    pos2(
        ((p.x - f.left()) / f.width()).clamp(0.0, 1.0),
        ((p.y - f.top()) / f.height()).clamp(0.0, 1.0),
    )
}

/// The board's drawn field lines, right of the zone names: touchline to touchline is the table's x -1..+1, the far
/// goal line to the own goal line its y -1..+1 (the 2010-11 C panel's FIELD_L..FIELD_B), so a card on a line lies on
/// it in the table too.
fn field(pitch: Rect) -> Rect {
    pitch
        .with_min_x(pitch.left() + LABEL_LANE)
        .shrink((pitch.width() * 0.025).max(3.0))
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
    Id::new("board-grab")
}

/// Draws the board and handles it: a card dropped on open pitch lies exactly there (the cabinet's table has no fixed
/// spots), a card dropped onto another swaps with it (the target is lit while dragging), right-click
/// takes a card off, a resting mouse shows the card big after `hover_delay` seconds. While a card is dragged or shown
/// big, the pitch shows its suitable positions as a heat zone, and the drop target says how well it fits there.
/// With `links`, lines join every two linked pitch cards by the club's link stage; a selected card (clicked, or in
/// hand) brings its group forward - its lines bright, the others and the cards outside its group dimmed - and a list
/// beside the board names its partners, on the team and not. Returns true when it changed.
pub fn formation_board(
    ui: &mut Ui,
    board: &mut Board,
    links: &Links,
    hover_delay: f32,
    lang: Lang,
) -> bool {
    let width = ui.available_width();
    let (pitch, _) = ui.allocate_exact_size(vec2(width, width * PITCH_ASPECT), Sense::hover());
    let dragging = egui::DragAndDrop::payload::<usize>(ui.ctx()).map(|a| *a);
    // a card dragged out of the catalogue
    let incoming = egui::DragAndDrop::payload::<Card>(ui.ctx());
    let pointer = ui.ctx().pointer_latest_pos();
    paint_pitch(ui.painter(), pitch, Some(lang));
    // the heat zone goes over the pitch and under the cards, once we know whose it is
    let heat_at = ui.painter().add(egui::Shape::Noop);
    // the link lines over it, under the cards
    let lines_at = ui.painter().add(egui::Shape::Noop);
    let card = card_size(pitch.width());
    // the selection: the card in hand, else the card clicked last (while it is on the board)
    let select_id = Id::new("board-select");
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

    let mut changed = false;
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
    // a pitch card in hand moves live: the table follows it every frame, as the C panels' board_move, so the game
    // sees the card slide and rub, not one jump on release; its spot before the lift is kept for a swap
    let origin_id = Id::new("board-origin");
    if dragging.is_some() || incoming.is_some() {
        // the first frame of a board drag: the grab is measured on the card's resting rect, before it moves
        if let Some(from) = dragging.filter(|&d| d < 16 && board.slots[d].is_some())
            && ui.data(|d| d.get_temp::<Vec2>(grab_id())).is_none()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            let rest = slot_rects(pitch, bench.top(), &board.spots)[from];
            ui.data_mut(|d| d.insert_temp(grab_id(), (rest.center() - press) / rest.size()));
        }
    } else {
        ui.data_mut(|d| d.remove::<Vec2>(grab_id()));
    }
    let grab = ui
        .data(|d| d.get_temp::<Vec2>(grab_id()))
        .unwrap_or(Vec2::ZERO);
    let origin = match dragging.filter(|&d| d < 11 && board.slots[d].is_some()) {
        Some(from) => {
            let o = ui.data_mut(|d| {
                *d.get_temp_mut_or_insert_with(origin_id, || (from, board.spots[from]))
            });
            if let Some(pt) = pointer.map(|p| drag_centre(p, grab, card))
                && pitch.contains(pt)
            {
                let spot = spot_at(pitch, pt);
                if spot != board.spots[from] {
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
    let rects = slot_rects(pitch, bench.top(), &board.spots);

    let hover_id = Id::new("board-hover");
    let flip_id = Id::new("board-flip");
    let mut hovered_card = None;
    let mut drop_target = None;
    // the swap target is the card under the held card's centre (as drawn), not under the pointer
    let target = pointer.and_then(|p| {
        target_slot(
            &rects,
            |s| s >= 11 || board.slots[s].is_some(),
            dragging,
            drag_centre(p, grab, card),
        )
    });
    for (slot, &rect) in rects.iter().enumerate() {
        // no spot boxes on the pitch: an empty pitch slot is no place and takes no drop
        if slot < 11 && board.slots[slot].is_none() {
            continue;
        }
        let resp = ui.interact(rect, Id::new(("board-slot", slot)), Sense::click_and_drag());
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
                        zone(board.spots[slot].y) != c.pos
                    };
                card_face(
                    p,
                    rect,
                    c,
                    Look {
                        ghost: is_dragged,
                        selected: selected == Some(c.number),
                        target: hovered_by_drag,
                        warn,
                        ..Look::default()
                    },
                );
                if let Some(s) = selected
                    && !is_dragged
                    && !links.grouped(s, c.number)
                {
                    p.rect_filled(
                        rect,
                        (rect.width() * 0.08) as u8,
                        Color32::from_black_alpha(140),
                    );
                }
                if resp.hovered() && dragging.is_none() {
                    hovered_card = Some((slot, ci, rect));
                }
                // a click turns the big card over (and back) and selects it (and lets it go)
                if resp.clicked() {
                    let no = c.number;
                    ui.data_mut(|d| {
                        let f = d.get_temp_mut_or_default::<Option<usize>>(flip_id);
                        *f = if *f == Some(slot) { None } else { Some(slot) };
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
                    let lp = ui
                        .ctx()
                        .layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("board-lift")));
                    let at = drag_centre(pt, grab, card);
                    card_face(
                        &lp,
                        Rect::from_center_size(at, card * 1.08),
                        c,
                        Look {
                            lifted: true,
                            held: true,
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
        && pointer.is_some_and(|p| pitch.contains(drag_centre(p, grab, card)))
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
            .layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("board-lift")));
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
            let lit = selected.is_none_or(|s| s == a || s == b);
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
    // the list while the card is in hand or the mouse is on the board: elsewhere it would cover the pages
    if let Some(s) = selected
        && (dragging.is_some() || pointer.is_some_and(|p| pitch.union(bench).contains(p)))
    {
        partner_list(
            ui,
            board,
            links,
            s,
            pos2(ui.max_rect().left() - 16.0, bench.bottom()),
            lang,
        );
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
            // turned over until the mouse rests on another card
            let back = ui.data_mut(|d| {
                let f = d.get_temp_mut_or_default::<Option<usize>>(flip_id);
                if f.is_some_and(|s| s != slot) {
                    *f = None;
                }
                *f == Some(slot)
            });
            if left <= 0.0 || back {
                let left = ui.max_rect().left();
                big_card(ui, &board.cards[ci], rect, left, card, back, lang);
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
            let size = (pitch.width() * 0.025).clamp(9.0, 12.0);
            let zone = heat_zone(ui.painter(), pitch, field(pitch), &apt, size);
            ui.painter().set(heat_at, zone);
            if let Some((slot, rect)) = drop_target
                && slot < 11
            {
                let fit = Fit::of(apt.at(board.spots[slot]));
                let lp = ui
                    .ctx()
                    .layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("board-fit")));
                fit_badge(&lp, pos2(rect.center().x, rect.bottom() + 4.0), fit, lang);
            }
        } else {
            // a card without ratings: say so instead of an empty heat zone
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

/// The selected card's partners beside the board, its bottom right at `anchor`: team-mates it is linked with (a line)
/// or has the best affinity with, then the best partners not on the team (at most six).
fn partner_list(ui: &Ui, board: &Board, links: &Links, sel: u32, anchor: Pos2, lang: Lang) {
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
    egui::Area::new(Id::new("board-partners"))
        .order(Order::Foreground)
        .pivot(Align2::RIGHT_BOTTOM)
        .fixed_pos(anchor)
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
        // in spot units, as `zone` reads a card's line: the field's height, not the pitch's
        let f = field(pitch);
        let y = |t: f32| {
            if t <= 0.0 {
                pitch.top()
            } else if t >= 1.0 {
                pitch.bottom()
            } else {
                f.top() + t * f.height()
            }
        };
        for (a, b, text) in bands {
            let r = Rect::from_x_y_ranges(pitch.left()..=pitch.left() + LABEL_LANE, y(a)..=y(b));
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
    let f = if labels.is_some() {
        field(pitch)
    } else {
        pitch.shrink((pitch.width() * 0.025).max(3.0))
    };
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

/// The card back's position map: a tile per position area ([`heat_zone`]), or "no position data" on a small pitch.
pub fn aptitude_map(p: &Painter, rect: Rect, apt: &Aptitude, lang: Lang) {
    if apt.known() {
        let field = rect.shrink((rect.width() * 0.025).max(3.0));
        let size = (rect.width() * 0.055).clamp(8.0, 12.0);
        p.add(heat_zone(p, rect, field, apt, size));
    } else {
        paint_pitch(p, rect, None);
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

/// The card's back (the players' request, 2026-10-14), upright as the face: the number, name and line on a band; the
/// suitable positions as the original card's back maps them, a small pitch with a tile per position area
/// ([`heat_zone`]); the six stats and TOTAL beside it. From the 16 role ratings
/// (the names and spots are the project's inference: [`ROLES`]).
pub fn card_back(p: &Painter, rect: Rect, card: &Card, lang: Lang) {
    let card = &*real(p.ctx(), card);
    let r = (rect.width() * 0.08) as u8;
    let rim = card.rarity.rim();
    p.add(
        Shadow {
            offset: [0, 6],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(150),
        }
        .as_shape(rect, r),
    );
    gradient(
        p,
        rect,
        r,
        Color32::from_rgb(0x1A, 0x27, 0x3D),
        theme::WELL,
        true,
    );
    p.rect_stroke(rect, r, Stroke::new(2.0, rim), StrokeKind::Inside);
    let w = rect.width();
    let unit = w / 200.0; // drawn for a card 200 wide, scaled
    // the band: number, name, line
    let band = Rect::from_min_size(
        rect.min + vec2(6.0, 6.0) * unit,
        vec2(w - 12.0 * unit, 26.0 * unit),
    );
    p.rect_filled(
        band,
        (4.0 * unit) as u8,
        card.pos.color().gamma_multiply(0.85),
    );
    p.text(
        band.left_center() + vec2(6.0 * unit, 0.0),
        Align2::LEFT_CENTER,
        format!("#{}", card.number),
        theme::display(13.0 * unit),
        Color32::WHITE,
    );
    p.text(
        band.right_center() - vec2(6.0 * unit, 0.0),
        Align2::RIGHT_CENTER,
        card.pos.code(),
        theme::display(15.0 * unit),
        Color32::WHITE,
    );
    let g = fit(
        p,
        card.short_name(),
        theme::display(15.0 * unit),
        Color32::WHITE,
        band.width() - 90.0 * unit,
    );
    p.galley(band.center() - g.size() / 2.0, g, Color32::WHITE);
    // the position map, left
    let map_w = w * 0.58;
    let map = Rect::from_min_size(
        pos2(rect.left() + 8.0 * unit, band.bottom() + 8.0 * unit),
        vec2(map_w, map_w * PITCH_ASPECT),
    );
    aptitude_map(p, map, &card.aptitude, lang);
    // the stats, right of the map
    let col = Rect::from_min_max(
        pos2(map.right() + 8.0 * unit, map.top()),
        pos2(rect.right() - 8.0 * unit, map.bottom()),
    );
    let names = [
        "cat.off", "cat.def", "cat.tec", "cat.pow", "cat.spd", "cat.sta",
    ];
    let row = col.height() / 8.0;
    if card.secret {
        // a legend's back: SECRET across the six rows, as the original prints it
        p.text(
            pos2(col.right(), col.top() + row * 3.0),
            Align2::RIGHT_CENTER,
            tr!(lang, "cat.secret"),
            theme::display(14.0 * unit),
            theme::TEXT_DIM,
        );
    }
    for (i, (k, v)) in names.iter().zip(card.stats).enumerate() {
        let y = col.top() + row * (i as f32 + 0.5);
        p.text(
            pos2(col.left(), y),
            Align2::LEFT_CENTER,
            tr!(lang, *k),
            theme::display(11.0 * unit),
            theme::TEXT_DIM,
        );
        if !card.secret {
            p.text(
                pos2(col.right(), y),
                Align2::RIGHT_CENTER,
                v.to_string(),
                theme::display(16.0 * unit),
                super::widgets::stat_color(v),
            );
        }
    }
    let y = col.top() + row * 6.0;
    p.line_segment(
        [pos2(col.left(), y), pos2(col.right(), y)],
        Stroke::new(1.0, theme::RIM),
    );
    p.text(
        pos2(col.left(), y + row),
        Align2::LEFT_CENTER,
        tr!(lang, "cat.total"),
        theme::display(11.0 * unit),
        theme::TEXT_DIM,
    );
    p.text(
        pos2(col.right(), y + row),
        Align2::RIGHT_CENTER,
        card.total.to_string(),
        theme::display(22.0 * unit),
        theme::AMBER,
    );
    // under the map: the 16 ratings, four to a row (the best lit), and the season
    let grid = Rect::from_min_max(
        pos2(rect.left() + 10.0 * unit, map.bottom() + 6.0 * unit),
        pos2(rect.right() - 10.0 * unit, rect.bottom() - 20.0 * unit),
    );
    if card.aptitude.known() {
        let (cw, rh) = (grid.width() / 4.0, grid.height() / 4.0);
        for (i, (name, rating)) in ROLES.iter().zip(card.aptitude.roles).enumerate() {
            let c = pos2(
                grid.left() + cw * (i % 4) as f32,
                grid.top() + rh * ((i / 4) as f32 + 0.5),
            );
            let ink = match rating {
                8.. => Color32::from_rgb(0xFF, 0xF6, 0xD0),
                5.. => theme::AMBER,
                _ => theme::TEXT_FAINT,
            };
            p.text(
                c,
                Align2::LEFT_CENTER,
                *name,
                theme::display(10.0 * unit),
                theme::TEXT_DIM,
            );
            p.text(
                c + vec2(cw - 8.0 * unit, 0.0),
                Align2::RIGHT_CENTER,
                rating.to_string(),
                theme::display(12.0 * unit),
                ink,
            );
        }
    } else {
        let g = p.layout(
            tr!(lang, "board.no_data"),
            theme::body(11.0 * unit),
            theme::TEXT_DIM,
            grid.width(),
        );
        p.galley(grid.center() - g.size() / 2.0, g, theme::TEXT_DIM);
    }
    p.text(
        pos2(rect.right() - 10.0 * unit, rect.bottom() - 11.0 * unit),
        Align2::RIGHT_CENTER,
        card.season.as_str(),
        theme::display(11.0 * unit),
        theme::TEXT_DIM,
    );
}

/// Room around the big card's face: the frame's margins, and its caption under it (name, facts, roles, stats).
const BIG_FRAME: f32 = 22.0;
const BIG_CAPTION: f32 = 150.0;

/// How much the big card enlarges a `tile`: 4x, or the most that keeps the face, the position map beside it and the
/// caption inside `room` (the space left of the board, the window's height); never under 1x.
fn big_scale(tile: Vec2, room: Vec2) -> f32 {
    // the face k * tile, 10 apart the map (k * tile.y - 22) / PITCH_ASPECT wide, the frame round them
    let by_w = (room.x - BIG_FRAME - 10.0 + 22.0 / PITCH_ASPECT) / (tile.x + tile.y / PITCH_ASPECT);
    let by_h = (room.y - BIG_FRAME - BIG_CAPTION) / tile.y;
    by_w.min(by_h).clamp(1.0, 4.0)
}

/// The big card beside the board, kept inside the window, with its caption: the whole card face at 4x the board's
/// `tile` (less when the window is too small, [`big_scale`]), the card back's suitable positions beside it, and its
/// line of facts.
fn big_card(ui: &Ui, card: &Card, near: Rect, board_left: f32, tile: Vec2, back: bool, lang: Lang) {
    let card = &*real(ui.ctx(), card);
    let screen = ui.ctx().content_rect();
    let room = vec2(board_left - 32.0 - screen.left(), screen.height() - 32.0);
    let size = tile * big_scale(tile, room);
    // the map under its 22 px heading, as tall as the card
    let map = vec2((size.y - 22.0) / PITCH_ASPECT, size.y - 22.0);
    // turned over: the back, as wide as the face and the map unless that is too tall
    let back_w = (size.x + 10.0 + map.x).min((room.y - BIG_FRAME - BIG_CAPTION) * CARD_ASPECT);
    let body_h = if back { back_w / CARD_ASPECT } else { size.y };
    let pos = pos2(
        board_left - 16.0,
        (near.center().y - size.y / 2.0)
            .min(screen.bottom() - 16.0 - BIG_FRAME - BIG_CAPTION - body_h)
            .max(screen.top() + 16.0),
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
                    if back {
                        // turned over: the back, as wide as the face and the map
                        let w = back_w;
                        let (r, _) =
                            ui.allocate_exact_size(vec2(w, w / CARD_ASPECT), Sense::hover());
                        card_back(ui.painter(), r, card, lang);
                    } else {
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
                    }
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
                    if card.secret {
                        ui.label(
                            egui::RichText::new(tr!(lang, "cat.secret"))
                                .font(theme::display(18.0))
                                .color(theme::TEXT_DIM),
                        );
                    } else {
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
                    }
                    ui.label(
                        egui::RichText::new(tr!(
                            lang,
                            if back {
                                "board.flip_back"
                            } else {
                                "board.flip"
                            }
                        ))
                        .font(theme::body(11.0))
                        .color(theme::TEXT_FAINT),
                    );
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
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rating_comes_from_the_area_drawn_on_top() {
        let mut a = Aptitude::default();
        a.roles[0] = 9; // CF
        a.roles[3] = 4; // SS
        a.roles[11] = 8; // CB
        a.roles[14] = 6; // SW: CB's area, the better role counts
        assert_eq!(a.at(pos2(0.5, 0.03)), 9.0); // CF's top strip
        assert_eq!(a.at(pos2(0.5, 0.2)), 4.0); // SS inside CF
        assert_eq!(a.at(pos2(0.5, 0.8)), 8.0);
        assert_eq!(a.at(pos2(0.5, 0.95)), 0.0); // the keeper's box inside CB's
        assert_eq!(a.at(pos2(0.1, 0.1)), 0.0); // WG
    }

    #[test]
    fn a_card_dropped_on_the_next_one_targets_it() {
        // slot 0 follows the pointer and overlaps slot 1: the target is slot 1, never the dragged slot itself
        let mut rects = [Rect::NOTHING; 16];
        rects[0] = Rect::from_min_size(pos2(90.0, 0.0), vec2(80.0, 120.0));
        rects[1] = Rect::from_min_size(pos2(100.0, 0.0), vec2(80.0, 120.0));
        assert_eq!(
            target_slot(&rects, |_| true, Some(0), pos2(150.0, 50.0)),
            Some(1)
        );
        assert_eq!(
            target_slot(&rects, |_| true, Some(0), pos2(95.0, 50.0)),
            None
        );
    }

    #[test]
    fn the_top_drawn_of_two_overlapping_cards_is_the_target() {
        let mut rects = [Rect::NOTHING; 16];
        rects[3] = Rect::from_min_size(pos2(0.0, 0.0), vec2(80.0, 120.0));
        rects[7] = Rect::from_min_size(pos2(40.0, 0.0), vec2(80.0, 120.0));
        assert_eq!(
            target_slot(&rects, |_| true, None, pos2(60.0, 50.0)),
            Some(7)
        );
    }

    #[test]
    fn a_dragged_card_stays_where_it_was_taken() {
        // taken at its bottom-right corner: the first frame draws it where it was, and it follows the pointer
        let card = vec2(80.0, 120.0);
        let rest = pos2(300.0, 200.0);
        let press = rest + card / 2.0;
        let grab = (rest - press) / card;
        assert_eq!(drag_centre(press, grab, card), rest);
        let moved = press + vec2(10.0, -5.0);
        assert_eq!(drag_centre(moved, grab, card), rest + vec2(10.0, -5.0));
    }

    #[test]
    fn a_card_picture_is_shown_whole() {
        // the game's whole 128x128 picture: its face, the left 88 columns, all its height
        let (uv, a) = face_uv(vec2(128.0, 128.0));
        assert_eq!(
            (uv.min, uv.max, a),
            (pos2(0.0, 0.0), pos2(0.6875, 1.0), 0.6875)
        );
        // the pack's 180x254 scan: all of it, and the tile has its shape
        let (uv, a) = face_uv(vec2(180.0, 254.0));
        assert_eq!(
            (uv.min, uv.max, a),
            (pos2(0.0, 0.0), pos2(1.0, 1.0), CARD_ASPECT)
        );
        let tile = card_size(400.0);
        assert!((tile.x / tile.y - CARD_ASPECT).abs() < 0.01, "{tile:?}");
        // the game's narrower face in a card tile: full height, bars at the sides, nothing cut
        let r = Rect::from_min_size(pos2(10.0, 20.0), tile);
        let l = letterbox(r, 0.6875);
        assert!(r.contains_rect(l) && (l.height() - r.height()).abs() < 1e-3);
        assert!((l.width() / l.height() - 0.6875).abs() < 1e-4 && l.center() == r.center());
        // a wider picture: full width, bars above and below
        let l = letterbox(r, 1.0);
        assert!((l.width() - r.width()).abs() < 1e-3 && l.height() < r.height());
    }

    fn card(n: u32) -> Card {
        Card {
            number: n,
            record: None,
            name: n.to_string(),
            short: String::new(),
            club: String::new(),
            country: String::new(),
            season: String::new(),
            pos: Position::Mf,
            rarity: Rarity::Regular,
            code: String::new(),
            secret: false,
            stats: [0; 6],
            total: 0,
            aptitude: Aptitude::default(),
        }
    }

    /// Cards 1..=3 on pitch slots 0..=2, card 4 on the first bench box.
    fn table() -> Board {
        let mut b = Board {
            cards: (1..=4).map(card).collect(),
            slots: [None; 16],
            spots: [pos2(0.5, 0.5); 11],
        };
        b.slots[..3].copy_from_slice(&[Some(0), Some(1), Some(2)]);
        b.slots[11] = Some(3);
        b
    }

    #[test]
    fn a_card_moves_swaps_and_goes_to_the_bench() {
        let mut b = table();
        // free move on the pitch: the spot changes, the slot stays
        assert!(b.move_to(1, pos2(0.1, 0.2)));
        assert_eq!((b.spots[1], b.number(1)), (pos2(0.1, 0.2), Some(2)));
        // swap with another card, a bench card, and an empty bench box
        b.swap(0, 2);
        assert_eq!((b.number(0), b.number(2)), (Some(3), Some(1)));
        b.swap(0, 11);
        assert_eq!((b.number(0), b.number(11)), (Some(4), Some(3)));
        b.swap(1, 13);
        assert_eq!((b.number(1), b.number(13)), (None, Some(2)));
        // a bench card dropped on the pitch takes the first free pitch slot, at the drop spot
        assert!(b.move_to(13, pos2(0.9, 0.9)));
        assert_eq!((b.number(1), b.spots[1]), (Some(2), pos2(0.9, 0.9)));
        assert_eq!(b.number(13), None);
    }

    #[test]
    fn a_catalogue_card_goes_on_the_pitch_or_over_a_place() {
        let mut b = table();
        assert!(b.put_free(&card(9), pos2(0.3, 0.3)));
        assert_eq!((b.number(3), b.spots[3]), (Some(9), pos2(0.3, 0.3)));
        // already on the table: refused
        assert!(!b.put_free(&card(9), pos2(0.0, 0.0)) && !b.put(&card(1), 5));
        // over a card: it takes the place and the old card goes to the first free bench box
        assert!(b.put(&card(10), 0));
        assert_eq!((b.number(0), b.number(12)), (Some(10), Some(1)));
        // a full bench: the old card goes off
        for s in 11..16 {
            b.slots[s].get_or_insert(0);
        }
        assert!(b.put(&card(11), 2));
        assert!(!b.holds(3) && b.number(2) == Some(11));
    }

    #[test]
    fn what_the_board_took_in_is_written_to_the_table() {
        use crate::cards::{Placed, table_after};
        let before: Vec<Placed> = (1..=3)
            .map(|i| Placed {
                slot: i,
                no: 100 + i as u32,
                x: -0.5 + i as f32 / 4.0,
                y: 0.1,
            })
            .collect();
        let mut b = crate::cards::board(&before, &[]);
        assert!(b.put_free(&card(555), pos2(0.5, 0.75)));
        let after = table_after(&before, &b);
        assert_eq!(after[..3], before[..]);
        assert_eq!((after[3].slot, after[3].no), (4, 555));
        assert_eq!((after[3].x, after[3].y), (0.0, 0.5));
    }

    /// Free placement through to the table file: a card dropped anywhere on the pitch keeps that exact place (three
    /// decimals), a drop past the pitch is clamped to its lines, a bench card and a catalogue card land where dropped,
    /// a drop onto a card swaps the two, and a card dropped on a bench box takes the box's band.
    #[test]
    fn cards_lie_where_they_are_dropped() {
        use crate::cards::{Placed, table_after, table_text};
        let pitch = pitch_rect(pos2(100.0, 50.0), 400.0);
        let f = field(pitch);
        let at = |x: f32, y: f32| pos2(f.left() + x * f.width(), f.top() + y * f.height());
        let pl = |slot, no, x, y| Placed { slot, no, x, y };
        let before = vec![
            pl(1, 101, 0.0, 0.9),
            pl(2, 102, -0.6, 0.55),
            pl(3, 103, 1.24, 0.34),
        ];
        let mut b = crate::cards::board(&before, &[]);
        // anywhere on the pitch: exactly there
        assert!(b.move_to(0, spot_at(pitch, at(0.3215, 0.8))));
        // past the pitch: clamped to the touchline and the far goal line
        assert!(b.move_to(1, spot_at(pitch, pitch.right_top() + vec2(90.0, -90.0))));
        let t = table_after(&before, &b);
        assert_eq!(
            t,
            [pl(1, 101, -0.357, 0.6), pl(2, 102, 1.0, -1.0), before[2]]
        );
        // the bench card (box 14, band 0.34) onto open pitch: there, not on a 4-4-2 spot
        assert_eq!(b.slots[14], Some(2));
        assert!(b.move_to(14, spot_at(pitch, at(0.75, 0.25))));
        // a catalogue card dropped on open pitch: there too
        assert!(b.put_free(&card(555), spot_at(pitch, at(0.1, 0.5))));
        let t = table_after(&before, &b);
        assert_eq!(t[2], pl(3, 103, 0.5, -0.5));
        assert_eq!(t[3], pl(4, 555, -0.8, 0.0));
        assert!(table_text(&t).ends_with("3 103 0.500 -0.500\n4 555 -0.800 0.000\n"));
        // onto another card: the two trade places
        let (s103, s555) = (b.slots.iter().position(|&s| s == Some(2)).unwrap(), 3);
        b.swap(s103, s555);
        let t2 = table_after(&t, &b);
        assert_eq!((t2[2].x, t2[2].y, t2[3].x, t2[3].y), (-0.8, 0.0, 0.5, -0.5));
        // onto the last bench box: that box's band
        b.swap(0, 15);
        let t3 = table_after(&t2, &b);
        assert_eq!((t3[0].x, t3[0].y), (crate::cards::BENCH_X, 0.68));
    }

    /// A pitch card in hand moves the table while the button is still down, back and forth (a rub), then once more on
    /// release; the board keeps its slots throughout.
    #[test]
    fn a_card_in_hand_moves_the_table_before_release() {
        use crate::cards::{Placed, table_after};
        use egui::{Event, PointerButton};
        let before = vec![Placed {
            slot: 1,
            no: 101,
            x: 0.0,
            y: 0.0,
        }];
        let mut b = crate::cards::board(&before, &[]);
        let ctx = egui::Context::default();
        theme::install(&ctx, theme::Edition::E1718, &theme::font_dir()).unwrap();
        // the fonts take effect from the next frame
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let mut pitch = Rect::NOTHING;
        let frame = |events: Vec<Event>, b: &mut Board, pitch: &mut Rect| {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 900.0))),
                events,
                ..Default::default()
            };
            let mut changed = false;
            ctx.run_ui(raw, |ui| {
                *pitch = pitch_rect(ui.cursor().min, ui.available_width());
                changed = formation_board(ui, b, &Links::default(), 3.0, crate::ui::Lang::En);
            })
            .textures_delta
            .clear();
            changed
        };
        frame(vec![], &mut b, &mut pitch);
        let at = spot_rect(pitch, b.spots[0]).center();
        let press = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            vec![Event::PointerMoved(at), press(at, true)],
            &mut b,
            &mut pitch,
        );
        let mut xs = Vec::new();
        for dx in [10.0, 20.0, 40.0, 60.0, 20.0, 60.0, 20.0] {
            let changed = frame(
                vec![Event::PointerMoved(at + vec2(dx, 0.0))],
                &mut b,
                &mut pitch,
            );
            if changed {
                xs.push(table_after(&before, &b)[0].x);
            }
        }
        assert_eq!(b.slots[0], Some(0));
        // the held card moved right, back, right, back: four turns the game can see
        let turns = xs
            .windows(3)
            .filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < 0.0)
            .count();
        assert!(xs.len() >= 4 && turns >= 2, "{xs:?}");
        // let go: one more change, where it was shown
        assert!(frame(
            vec![press(at + vec2(20.0, 0.0), false)],
            &mut b,
            &mut pitch
        ));
        assert_eq!(table_after(&before, &b)[0].x, *xs.last().unwrap());
    }

    /// The big card is 4x the tile when the window has room, else the most that keeps it, its map and caption inside.
    #[test]
    fn the_big_card_is_four_times_the_tile_when_it_fits() {
        let tile = card_size(400.0);
        assert_eq!(big_scale(tile, vec2(1200.0, 1400.0)), 4.0);
        let room = vec2(400.0, 600.0);
        let k = big_scale(tile, room);
        assert!(k > 1.0 && k < 4.0, "{k}");
        let face = tile * k;
        let need_w = face.x + 10.0 + (face.y - 22.0) / PITCH_ASPECT + BIG_FRAME;
        let need_h = face.y + BIG_FRAME + BIG_CAPTION;
        assert!(
            need_w <= room.x + 0.01 && need_h <= room.y + 0.01,
            "{need_w} {need_h}"
        );
        assert_eq!(big_scale(tile, vec2(10.0, 10.0)), 1.0);
    }

    /// A card's tile stays on the pitch at any spot, and lies centred on its spot away from the edges.
    #[test]
    fn a_card_never_leaves_the_pitch() {
        let pitch = pitch_rect(pos2(0.0, 0.0), 400.0);
        for s in [
            pos2(0.0, 0.0),
            pos2(1.0, 1.0),
            pos2(0.0, 1.0),
            pos2(1.0, 0.0),
        ] {
            assert!(pitch.contains_rect(spot_rect(pitch, s)), "{s:?}");
        }
        let lane = pitch.with_min_x(LABEL_LANE);
        let mid = spot_rect(pitch, pos2(0.5, 0.5)).center();
        assert!((mid - lane.center()).length() < 0.01);
    }

    #[test]
    fn the_packs_real_stats_replace_the_catalogues() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join(hq::FILE),
            "7\tFW\t19\t6\t17\t13\t18\t15\t88\n",
        )
        .unwrap();
        let ctx = egui::Context::default();
        set_picture_dir(&ctx, Some(d.path().to_path_buf()));
        let seven = card(7);
        let c = real(&ctx, &seven);
        assert_eq!(
            (c.pos, c.stats, c.total),
            (Position::Fw, [19, 6, 17, 13, 18, 15], 88)
        );
        // a card the pack lacks, and no pack at all: the game's numbers
        assert!(matches!(
            real(&ctx, &card(8)),
            std::borrow::Cow::Borrowed(_)
        ));
        set_picture_dir(&ctx, None);
        assert!(matches!(
            real(&ctx, &card(7)),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
