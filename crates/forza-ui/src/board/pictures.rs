use super::*;

// ---- card pictures ----

/// Names the folder of card pictures, `<number>.png` each (the kit's `data\cache\cards`, extracted at setup on the
/// player's PC), or none for the drawn stand-in. Pictures load in the background through egui's loaders and the last
/// [`PICTURES_KEPT`] drawn stay cached, so scrolling the catalogue back seldom waits on a file.
pub fn set_picture_dir(ctx: &egui::Context, dir: Option<std::path::PathBuf>) {
    ctx.data_mut(|d| d.insert_temp(Id::unique("card-pictures"), dir));
}

/// The card face in a picture of `size`, whole (never cut): a square picture is the game's 128x128 one, its face
/// the left 88 columns; any other is the face itself (the pack's scans, the album's 256x357). Returns its uv part and its aspect.
pub(super) fn face_uv(size: Vec2) -> (Rect, f32) {
    let fw = if size.x == size.y { 88.0 / 128.0 } else { 1.0 };
    (
        Rect::from_min_max(Pos2::ZERO, pos2(fw, 1.0)),
        size.x * fw / size.y,
    )
}

/// The largest rect of `aspect` (width / height) centred in `r`: the whole picture, with bars where shapes differ.
pub(super) fn letterbox(r: Rect, aspect: f32) -> Rect {
    let w = r.width().min(r.height() * aspect);
    Rect::from_center_size(r.center(), vec2(w, w / aspect))
}

/// Where a picture of `aspect` (width / height) draws in the card slot `r`: a card-shaped one (within 5 % of the slot's
/// shape) fills it, so two sides of slightly different shapes share one rect (issue #265); another shape is letterboxed.
pub(super) fn picture_rect(r: Rect, aspect: f32) -> Rect {
    if (aspect / r.aspect_ratio() - 1.0).abs() < 0.05 {
        r
    } else {
        letterbox(r, aspect)
    }
}

fn picture_dir(ctx: &egui::Context) -> Option<std::path::PathBuf> {
    ctx.data(|d| d.get_temp::<Option<std::path::PathBuf>>(Id::unique("card-pictures")))
        .flatten()
}

/// The card's picture files in the order the board takes them: the downloaded high-quality picture, the game's album
/// art, the older pack's scan, the game's face.
pub(super) fn picture_names(number: u32) -> [String; 4] {
    [
        format!("{number}_hq.jpg"),
        format!("{number}_f.jpg"),
        format!("{number}.jpg"),
        format!("{number}.png"),
    ]
}

/// A picture file in the folder: a ready texture, still loading, or not there.
enum Pic {
    Ready(egui::TextureId, Vec2),
    Pending,
    Missing,
}

/// The card's picture: the first of [`picture_names`] that exists, once it is loaded. A file still loading ends
/// the search (no flash of a lesser picture, no texture loaded for nothing); `None` draws the stand-in.
pub(super) fn card_picture_texture(
    ctx: &egui::Context,
    dir: &std::path::Path,
    number: u32,
) -> Option<(egui::TextureId, Vec2)> {
    for f in picture_names(number) {
        match load_picture(ctx, dir, &f, egui::TextureOptions::LINEAR) {
            Pic::Ready(id, size) => return Some((id, size)),
            Pic::Pending => return None,
            Pic::Missing => {}
        }
    }
    None
}

/// The picture file `file` in the folder as a ready texture (a miss is remembered: the folder is made once, at
/// setup).
fn load_picture(
    ctx: &egui::Context,
    dir: &std::path::Path,
    file: &str,
    options: egui::TextureOptions,
) -> Pic {
    let file = dir.join(file);
    // `file:///C:\...` on Windows, `file:///home/...` elsewhere: what egui_extras' file loader parses
    let uri = format!(
        "file:///{}",
        file.display().to_string().trim_start_matches('/')
    );
    let miss = Id::unique(("card-picture-miss", &uri));
    if ctx.data(|d| d.get_temp::<bool>(miss)).is_some() {
        return Pic::Missing;
    }
    let loaded = ctx.try_load_texture(&uri, options, egui::SizeHint::default());
    match &loaded {
        Err(_) => {
            ctx.data_mut(|d| d.insert_temp(miss, true));
        }
        Ok(egui::load::TexturePoll::Ready { texture }) => {
            keep_recent(ctx, &uri, (texture.size.x * texture.size.y * 4.0) as usize);
        }
        Ok(_) => keep_recent(ctx, &uri, 0),
    }
    match loaded {
        Ok(egui::load::TexturePoll::Ready { texture }) => Pic::Ready(texture.id, texture.size),
        Ok(egui::load::TexturePoll::Pending { .. }) => Pic::Pending,
        Err(_) => Pic::Missing,
    }
}

/// The card pictures one egui context keeps loaded: the ones drawn last, at most this many and [`PICTURE_BYTES`] of
/// textures. A catalogue browsed for long would otherwise keep every picture it ever showed. One screen keeps at most
/// about 130: the catalogue grid (cards up to 170 points wide from `sizing::grid`; at most 110 in sight, partial rows
/// included, on a 4:3 to 16:9 window at 100-200 % scaling), the board's 16 slots and the card in hand, the stats pane's
/// two sides (its other one loads once the card has been in the pane for two frames) and the card view's two sides.
/// Worst case per context: 150 album pictures (256x357 RGBA, 365 KB) = 55 MB of textures, plus a second texture for the
/// few drawn both NEAREST and LINEAR (the card view, the large hover pane); the panel's contexts set
/// `reduce_texture_memory`, so no decoded copy sits beside them.
pub(super) const PICTURES_KEPT: usize = 150;
/// The texture bytes one context keeps for card pictures. The panel runs in the game's 32-bit process, and its
/// Direct3D 9 textures are MANAGED, so each also has a copy in that process. 150 album pictures (55 MB) fit as before;
/// the downloaded pictures (decoded at most [`HQ_MAX`]: 1.4 MB each) keep about 44 beside the ones on screen, where
/// 150 at their full 664x911 would be 363 MB.
pub(super) const PICTURE_BYTES: usize = 64 << 20;

/// The picture URIs loaded lately, each with the pass that last drew it and its texture's bytes (0 while it loads).
#[derive(Clone, Default)]
pub(super) struct RecentPictures(pub(super) HashMap<String, (u64, usize)>);

/// Marks the picture `uri` (`bytes` of texture) drawn in this pass; past [`PICTURES_KEPT`] or [`PICTURE_BYTES`],
/// forgets those drawn longest ago, never one drawn in this pass or the previous one: a card still on screen may come
/// later in this pass. With more than the bound on screen the set grows past it instead (no flicker, no reload loop).
fn keep_recent(ctx: &egui::Context, uri: &str, bytes: usize) {
    let now = ctx.cumulative_pass_nr();
    let gone = ctx.data_mut(|d| {
        let r = &mut d
            .get_temp_mut_or_default::<RecentPictures>(Id::unique("card-pictures-recent"))
            .0;
        r.insert(uri.to_owned(), (now, bytes));
        let mut gone = Vec::new();
        let mut total: usize = r.values().map(|(_, b)| b).sum();
        while r.len() > PICTURES_KEPT || total > PICTURE_BYTES {
            let Some((old, b)) = r
                .iter()
                .filter(|(_, (pass, _))| *pass + 1 < now)
                .min_by_key(|(_, (pass, _))| *pass)
                .map(|(u, (_, b))| (u.clone(), *b))
            else {
                break;
            };
            r.remove(&old);
            total -= b;
            gone.push(old);
        }
        gone
    });
    // outside `data_mut`: forgetting locks the context again
    for u in gone {
        ctx.forget_image(&u);
    }
}

/// The largest the board draws a downloaded picture, in pixels: the card view's album slot at 2x.
pub const HQ_MAX: (u32, u32) = (512, 714);

type Decoded = std::task::Poll<Result<Arc<egui::ColorImage>, String>>;

/// Decodes the downloaded pictures (`<number>_hq.jpg`, `<number>_hq_b.jpg`, 664x911) on a thread and shrinks them
/// to fit [`HQ_MAX`]: their textures cost 1.4 MB, not 2.4 MB. Other pictures go to egui's own loaders. Installed by
/// `theme::install_fonts`.
#[derive(Default)]
pub struct HqLoader(Arc<egui::mutex::Mutex<HashMap<String, Decoded>>>);

impl HqLoader {
    pub const ID: &'static str = egui::generate_loader_id!(HqLoader);
}

impl egui::load::ImageLoader for HqLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(
        &self,
        ctx: &egui::Context,
        uri: &str,
        _: egui::SizeHint,
    ) -> egui::load::ImageLoadResult {
        use egui::load::{BytesPoll, ImagePoll, LoadError};
        use std::task::Poll;
        if !(uri.ends_with("_hq.jpg") || uri.ends_with("_hq_b.jpg")) {
            return Err(LoadError::NotSupported);
        }
        if let Some(e) = self.0.lock().get(uri).cloned() {
            return match e {
                Poll::Ready(Ok(image)) => Ok(ImagePoll::Ready { image }),
                Poll::Ready(Err(e)) => Err(LoadError::Loading(e)),
                Poll::Pending => Ok(ImagePoll::Pending { size: None }),
            };
        }
        match ctx.try_load_bytes(uri)? {
            BytesPoll::Pending { size } => Ok(ImagePoll::Pending { size }),
            BytesPoll::Ready { bytes, .. } => {
                self.0.lock().insert(uri.to_owned(), Poll::Pending);
                let (ctx, cache, uri) = (ctx.clone(), Arc::clone(&self.0), uri.to_owned());
                std::thread::spawn(move || {
                    let r = image::load_from_memory(&bytes)
                        .map(|i| {
                            let i = if i.width() > HQ_MAX.0 || i.height() > HQ_MAX.1 {
                                i.resize(HQ_MAX.0, HQ_MAX.1, image::imageops::FilterType::Triangle)
                            } else {
                                i
                            };
                            let i = i.to_rgba8();
                            let size = [i.width() as usize, i.height() as usize];
                            Arc::new(egui::ColorImage::from_rgba_unmultiplied(size, i.as_raw()))
                        })
                        .map_err(|e| e.to_string());
                    // forgotten while it decoded: nothing to keep; the context is locked only after the cache
                    let kept = cache
                        .lock()
                        .get_mut(&uri)
                        .map(|e| *e = Poll::Ready(r))
                        .is_some();
                    if kept {
                        ctx.request_repaint();
                    }
                });
                Ok(ImagePoll::Pending { size: None })
            }
        }
    }

    fn forget(&self, uri: &str) {
        self.0.lock().remove(uri);
    }

    fn forget_all(&self) {
        self.0.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.0
            .lock()
            .values()
            .map(|e| match e {
                std::task::Poll::Ready(Ok(i)) => i.pixels.len() * 4,
                _ => 0,
            })
            .sum()
    }
}

/// A card's picture for the 1:1 views, at its own pixels: the front (the downloaded `<number>_hq.jpg`, else the album's
/// `<number>_f.jpg`, else the pack's scan `<number>.jpg`) or the back (`<number>_hq_b.jpg`, else the album's
/// `<number>_b.jpg`). `nearest`: a texture filtered by nearest pixel, for
/// whole multiples past 1x. `None` while it loads or when there is no file: the drawn face or back stands in.
pub fn card_picture_px(
    ctx: &egui::Context,
    number: u32,
    back: bool,
    nearest: bool,
) -> Option<(egui::TextureId, Vec2)> {
    let dir = picture_dir(ctx)?;
    let options = if nearest {
        egui::TextureOptions::NEAREST
    } else {
        egui::TextureOptions::LINEAR
    };
    let names = if back {
        back_names(number).to_vec()
    } else {
        vec![
            format!("{number}_hq.jpg"),
            format!("{number}_f.jpg"),
            format!("{number}.jpg"),
        ]
    };
    for f in names {
        match load_picture(ctx, &dir, &f, options) {
            Pic::Ready(id, size) => return Some((id, size)),
            Pic::Pending => return None,
            Pic::Missing => {}
        }
    }
    None
}

/// The card's back picture files: the downloaded one, then the album's.
fn back_names(number: u32) -> [String; 2] {
    [format!("{number}_hq_b.jpg"), format!("{number}_b.jpg")]
}

/// Whether the card has a back picture in the folder ([`back_names`]): `None` while one loads, false without a folder.
/// Asked per card at run time, since a pack may add backs; without one nothing offers BACK (#333).
pub fn has_back(ctx: &egui::Context, number: u32) -> Option<bool> {
    let Some(dir) = picture_dir(ctx) else {
        return Some(false);
    };
    for f in back_names(number) {
        match load_picture(ctx, &dir, &f, egui::TextureOptions::LINEAR) {
            Pic::Ready(..) => return Some(true),
            Pic::Pending => return None,
            Pic::Missing => {}
        }
    }
    Some(false)
}

/// A card side in a 1:1 slot `rect` (its top left on a whole pixel): its picture filling the slot, letterboxed when its
/// shape is not a card's, else the drawn face (a back without its picture draws the front: there is no drawn back).
/// `true`: the picture was drawn, not the stand-in.
pub fn card_side(p: &Painter, rect: Rect, card: &Card, back: bool, nearest: bool) -> bool {
    // nearest filtering only when the slot's width is a whole multiple of the picture's pixels; any other picture
    // stays smooth. A card-shaped picture fills the slot ([`picture_rect`]) up to 5 % off its own shape, so its
    // height may not be whole pixels of the picture
    let ppp = p.pixels_per_point();
    // a NEAREST texture still loading: the LINEAR one, not the stand-in
    let got = card_picture_px(p.ctx(), card.number, back, nearest)
        .filter(|(_, size)| !nearest || (rect.width() * ppp / size.x).fract().abs() <= 0.01)
        .or_else(|| card_picture_px(p.ctx(), card.number, back, false));
    match got {
        Some((id, size)) => {
            p.rect_filled(rect, 0, theme::DEEP);
            // a plain mesh, not an image shape: the tessellator's feathering would move its edges off the pixel grid
            let at = picture_rect(rect, size.x / size.y);
            let at = Rect::from_min_size(
                rect.min
                    + ((at.min - rect.min) * p.pixels_per_point()).round() / p.pixels_per_point(),
                at.size(),
            );
            let mut mesh = egui::Mesh::with_texture(id);
            let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
            mesh.add_rect_with_uv(at, uv, Color32::WHITE);
            p.add(egui::Shape::mesh(mesh));
            return true;
        }
        None if back => return card_side(p, rect, card, false, nearest),
        None => {
            card_face(
                p,
                rect,
                card,
                Look {
                    square: true,
                    ..Look::default()
                },
            );
        }
    }
    false
}

/// The pack's real values by card number (`hq_stats.tsv` beside the pictures), read once per folder.
fn hq_map(ctx: &egui::Context) -> Option<std::sync::Arc<std::collections::HashMap<u32, Hq>>> {
    let dir = picture_dir(ctx)?;
    let id = Id::unique(("card-hq", &dir));
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
    let (r, g) = chip(p, right, h, code, theme::TEXT.gamma_multiply(alpha));
    p.rect_filled(r, 3, Color32::from_black_alpha((180.0 * alpha) as u8));
    p.galley(r.min + vec2(h * 0.35, h * 0.1), g, theme::TEXT);
}

/// [`code_chip`]'s box and its text.
fn chip(p: &Painter, right: Pos2, h: f32, code: &str, color: Color32) -> (Rect, Arc<Galley>) {
    let g = p.layout_no_wrap(code.to_owned(), theme::display(h), color);
    let pad = vec2(h * 0.35, h * 0.1);
    let r = Rect::from_min_size(
        right - vec2(g.size().x + 2.0 * pad.x, g.size().y / 2.0 + pad.y),
        g.size() + 2.0 * pad,
    );
    (r, g)
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
    /// Square corners (the big hover card).
    pub square: bool,
}

/// The card face, upright at the real card's shape ([`CARD_ASPECT`]): the card's picture whole, never cut (the game's
/// downloaded picture `<number>_hq.jpg`, else album art `<number>_f.jpg`, else the pack's scan `<number>.jpg`, else the game's face `<number>.png`, letterboxed on a dark ground when its shape differs),
/// from the folder [`set_picture_dir`] names; else a drawn face: the position band with TOTAL, a stand-in, the name on
/// its foot, and on large ones the club and the season. Returns the corner radius it drew with, for whatever a caller
/// paints over the face (a square picture has none).
pub fn card_face(p: &Painter, rect: Rect, card: &Card, look: Look) -> u8 {
    let card = &*real(p.ctx(), card);
    let big = rect.width() >= 90.0;
    // a picture is the whole card face (its own band, photo, name and numbers): shown whole, so square
    let picture =
        picture_dir(p.ctx()).and_then(|dir| card_picture_texture(p.ctx(), &dir, card.number));
    let r = if look.square || picture.is_some() {
        0
    } else {
        (rect.width() * 0.08) as u8
    };
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
    if let Some((id, size)) = picture {
        let (uv, aspect) = face_uv(size);
        p.rect_filled(rect, r, theme::DEEP.gamma_multiply(alpha));
        p.image(
            id,
            letterbox(rect, aspect),
            uv,
            Color32::WHITE.gamma_multiply(alpha),
        );
        face_marks(p, rect, r, card, rim, alpha, look);
        return r;
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
    let band_h = band_height(rect);
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
    let parts = big.then(|| face_parts(p, rect, card));
    let pic = parts.as_ref().map_or_else(
        || {
            let inset = 3.0;
            Rect::from_min_size(
                band.left_bottom() + vec2(inset, inset * 0.7),
                Vec2::splat(rect.width() - 2.0 * inset),
            )
        },
        |f| f.pic,
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
    if let Some(f) = parts {
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
        let club = club_galley(p, rect, card, theme::TEXT_DIM.gamma_multiply(alpha));
        p.galley(f.club.min, club, theme::TEXT_DIM);
        let season = season_galley(
            p,
            rect,
            card,
            theme::TEXT_FAINT.gamma_multiply(alpha),
            f.chip,
            f.mark,
        );
        p.galley(f.season.min, season, theme::TEXT_FAINT);
        rarity_mark(p, f.mark.center(), MARK, card.rarity);
        code_chip(
            p,
            f.chip.right_center(),
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
    r
}

/// The rarity mark's size on a big face.
const MARK: f32 = 14.0;

/// A drawn face's position band height.
fn band_height(rect: Rect) -> f32 {
    (rect.height() * 0.17).max(13.0)
}

/// A big face's club line, shrunk to its room, cut with "…" past it.
pub fn club_galley(p: &Painter, rect: Rect, card: &Card, color: Color32) -> Arc<Galley> {
    fit_elided(
        p,
        &card.club,
        theme::body(rect.width() * 0.09),
        color,
        rect.width() - 12.0,
    )
}

/// A big face's season at the foot's left, shrunk to its room before the series `chip` (or the rarity `mark` when the
/// chip is [`Rect::NOTHING`]), cut with "…" past it (#306: a long season ran under the chip).
pub fn season_galley(
    p: &Painter,
    rect: Rect,
    card: &Card,
    color: Color32,
    chip: Rect,
    mark: Rect,
) -> Arc<Galley> {
    let end = if chip == Rect::NOTHING { mark } else { chip };
    fit_elided(
        p,
        &card.season,
        theme::display(rect.width() * 0.1),
        color,
        end.left() - 2.0 - (rect.left() + 6.0),
    )
}

/// Where a big drawn face ([`card_face`]) puts its parts under the band: the picture, the club line, the season, the
/// series chip and the rarity mark (the box the mark's widest shape covers; [`Rect::NOTHING`] for a chip without a code).
#[derive(Clone, Copy, Debug)]
pub struct FaceParts {
    pub pic: Rect,
    pub club: Rect,
    pub season: Rect,
    pub chip: Rect,
    pub mark: Rect,
}

/// [`FaceParts`] for `card` on a big face in `rect`: the foot row on the card's foot (the season at the
/// left, the chip and the mark at the right), the club line over it, and the picture as large as the room above
/// allows (#376: the chip and the mark sat on the club line, and the card's foot cut it).
pub fn face_parts(p: &Painter, rect: Rect, card: &Card) -> FaceParts {
    let card = &*real(p.ctx(), card);
    let h = rect.width() * 0.09;
    let chip_h = chip(p, Pos2::ZERO, h, card.series(), theme::TEXT)
        .0
        .height();
    let season = p
        .layout_no_wrap(
            card.season.clone(),
            theme::display(rect.width() * 0.1),
            theme::TEXT,
        )
        .size();
    // the row's middle: its tallest part 2 points over the card's foot
    let row = rect.bottom() - 2.0 - (MARK * 1.35).max(chip_h).max(season.y) / 2.0;
    let mark = Rect::from_center_size(pos2(rect.right() - 11.0, row), Vec2::splat(MARK * 1.35));
    let chip = if card.series().is_empty() {
        Rect::NOTHING
    } else {
        chip(
            p,
            pos2(mark.left() - 2.0, row),
            h,
            card.series(),
            theme::TEXT,
        )
        .0
    };
    let fitted = season_galley(p, rect, card, theme::TEXT, chip, mark).size();
    let season = Rect::from_min_size(pos2(rect.left() + 6.0, row - fitted.y / 2.0), fitted);
    let club = club_galley(p, rect, card, theme::TEXT_DIM).size();
    let club_top = mark.top().min(chip.top()).min(season.top()) - 1.0 - club.y;
    let club = Rect::from_min_size(pos2(rect.center().x - club.x / 2.0, club_top), club);
    let top = rect.top() + band_height(rect) + 4.2;
    let side = (rect.width() - 12.0).min(club.top() - 1.0 - top);
    FaceParts {
        pic: Rect::from_min_size(pos2(rect.center().x - side / 2.0, top), Vec2::splat(side)),
        club,
        season,
        chip,
        mark,
    }
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
