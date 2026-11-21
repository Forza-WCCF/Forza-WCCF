//! The writing inside the game's pictures (`data/wccf_data.xaf`), ported from `legacy/2017-18/kit/scripts/
//! pictures1718.py` byte for byte: the same recipes (`english/pictures.json`, `italian/pictures.json`), the same
//! font (`fonts/BarlowCondensed-Bold.ttf`, drawn by FreeType as Pillow 11.3 draws it with its basic layout, the one
//! the kit's Windows Python had), the same pixels ([`super::pil`], [`super::bcn`]) and the same archive state file,
//! so a game switched by the Python scripts switches back here and the other way round.
//!
//! Each redrawn picture is ADDED at the archive's end and its table-of-contents entry pointed at it; the way back
//! (old entries, header, length) is saved first in `<backup>/wccf_data.xaf.json`; [`restore`] puts it back and cuts
//! the file to its old length. The 2010-11 script wrote the same state file (its pictures are off by default), so
//! [`restore`] serves both editions.

#![allow(clippy::useless_conversion)] // FreeType's FT_Pos is a C long: i64 here, i32 on Windows

use super::bcn::{self, u32le};
use super::{R, fail, pil, sha};
use crate::cards::pictures::lzw;
use freetype::face::LoadFlag;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const XAF: &str = "data/wccf_data.xaf";
pub const STATE: &str = "wccf_data.xaf.json";
const FONT: &str = "BarlowCondensed-Bold.ttf";
const SECTOR: u64 = 2048;
const ENTRY: usize = 0xB0;
const TOC_AT: u64 = 0x100;
/// words drawn 4x larger, then made small: smooth edges
const S: i64 = 4;

// ---------------------------------------------------------------- recipes
/// The recipes with each label's words in `lang` (Italian where `italian/pictures.json` has them, else English).
fn recipes(sheets: &Path, it: bool) -> R<Vec<Value>> {
    let read = |p: PathBuf| -> R<Value> {
        let d = fs::read(&p)?;
        serde_json::from_slice(&d).map_err(|e| super::Failed::in_file(p.display(), e))
    };
    let mut out = match read(sheets.join("english").join("pictures.json"))?
        .get_mut("pictures")
        .map(Value::take)
    {
        Some(Value::Array(a)) => a,
        _ => {
            return fail(super::Line::key("text.fail_pictures_list"));
        }
    };
    for r in &out {
        check_recipe(r).map_err(|e| {
            super::Failed::in_file(format!("english\\pictures.json: {}", r["id"]), e)
        })?;
    }
    let p = sheets.join("italian").join("pictures.json");
    let itw = if it && p.is_file() {
        read(p)?
    } else {
        json!({})
    };
    // a misspelt id would leave its picture in English with no word said
    if let Some(id) = itw
        .as_object()
        .into_iter()
        .flat_map(Map::keys)
        .find(|id| !out.iter().any(|r| r["id"].as_str() == Some(id.as_str())))
    {
        return fail(super::Line::key("text.fail_pictures_id").arg("id", id));
    }
    for r in &mut out {
        let id = r["id"].as_str().unwrap_or("").to_owned();
        let words = itw
            .get(&id)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let labels = r["labels"]
            .as_array_mut()
            .map_or(&mut [][..], Vec::as_mut_slice);
        if !words.is_empty() && words.len() != labels.len() {
            return fail(
                super::Line::key("text.fail_pictures_words")
                    .arg("id", id)
                    .arg("words", words.len())
                    .arg("labels", labels.len()),
            );
        }
        for (n, lab) in labels.iter_mut().enumerate() {
            let w = words
                .get(n)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let t = w
                .map(String::from)
                .or_else(|| lab["en"].as_str().map(String::from));
            lab["text"] = t.unwrap_or_default().into();
        }
    }
    Ok(out)
}

const RECIPE_KEYS: [&str; 9] = [
    "id", "where", "sha256", "files", "labels", "style", "clear", "erase", "reflect",
];
/// A label's own keys; a style key may sit on a label too (it wins there).
const LABEL_KEYS: [&str; 5] = ["en", "box", "clear", "at", "len"];
const STYLE_KEYS: [&str; 9] = [
    "core",
    "halo",
    "halo_px",
    "blur",
    "fill",
    "align",
    "squeeze",
    "size",
    "halo_alpha",
];
const STYLE_NUMBERS: [&str; 5] = ["halo_px", "blur", "squeeze", "size", "halo_alpha"];
const REFLECT_KEYS: [&str; 3] = ["src", "dst_y", "alpha"];

/// A recipe as the drawing reads it, or why not: the drawing reads a missing value as 0, so a misspelt key or a
/// box that is not four whole numbers would draw at (0,0,0,0) instead of stopping.
fn check_recipe(r: &Value) -> Result<(), String> {
    fn keys(v: &Value, allowed: &[&str], at: &str) -> Result<(), String> {
        let o = v
            .as_object()
            .ok_or_else(|| format!("{at} is not an object"))?;
        match o.keys().find(|k| !allowed.contains(&k.as_str())) {
            Some(k) => Err(format!("{at} has an unknown key {k:?}")),
            None => Ok(()),
        }
    }
    fn styled(st: &Value, at: &str) -> Result<(), String> {
        if let Some(k) = STYLE_NUMBERS
            .iter()
            .find(|k| st.get(**k).is_some_and(|v| !v.is_number()))
        {
            return Err(format!("{at}: {k} is not a number"));
        }
        match st.get("align") {
            Some(a) if !matches!(a.as_str(), Some("left" | "right" | "center")) => {
                Err(format!("{at}: align {a} is not left, right or center"))
            }
            _ => Ok(()),
        }
    }
    let is_box = |v: &Value| {
        v.as_array()
            .is_some_and(|a| a.len() == 4 && a.iter().all(|n| n.as_i64().is_some()))
    };
    keys(r, &RECIPE_KEYS, "the recipe")?;
    if r["id"].as_str().is_none() || r["sha256"].as_str().is_none() {
        return Err("no id or sha256".into());
    }
    let files = r["files"]
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or("no files")?;
    let Some(files) = files.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
        return Err("a file that is not a name".into());
    };
    let scene = files.iter().any(|f| f.ends_with(".sbscene"));
    let picture = files.iter().any(|f| !f.ends_with(".sbscene"));
    if let Some(st) = r.get("style") {
        keys(st, &STYLE_KEYS, "its style")?;
        styled(st, "its style")?;
    }
    if r.get("clear").is_some() && !r["clear"].as_array().is_some_and(|a| a.iter().all(is_box)) {
        return Err("clear is not a list of boxes".into());
    }
    if r.get("erase").is_some() && !r["erase"].as_array().is_some_and(|a| a.iter().all(is_box)) {
        return Err("erase is not a list of boxes".into());
    }
    if let Some(rf) = r.get("reflect") {
        keys(rf, &REFLECT_KEYS, "its reflect")?;
        let alpha = rf["alpha"].as_array();
        if !is_box(&rf["src"])
            || rf["dst_y"].as_i64().is_none()
            || !alpha.is_some_and(|a| a.len() == 2 && a.iter().all(Value::is_number))
        {
            return Err("reflect needs a src box, a dst_y and two alphas".into());
        }
    }
    // no labels is fine: a recipe may only clear (licence_caution_notes_end)
    let labels = r["labels"].as_array().ok_or("labels is not a list")?;
    let label_keys: Vec<&str> = LABEL_KEYS.iter().chain(&STYLE_KEYS).copied().collect();
    for (n, lab) in labels.iter().enumerate() {
        let at = format!("label {n}");
        keys(lab, &label_keys, &at)?;
        styled(lab, &at)?;
        if lab.get("en").is_some_and(|v| !v.is_string()) {
            return Err(format!("{at}: en is not words"));
        }
        if scene && (lab["at"].as_u64().is_none() || lab["len"].as_u64().is_none()) {
            return Err(format!("{at} needs at and len (a scene's text)"));
        }
        if picture && (!is_box(&lab["box"]) || lab.get("clear").is_some_and(|c| !is_box(c))) {
            return Err(format!("{at} needs a box of four whole numbers"));
        }
    }
    Ok(())
}

/// None = every picture, else the names (lower case); "none" = the empty set (`WCCF_PICTURES`, as the script).
fn wanted(spec: Option<&str>) -> Option<HashSet<String>> {
    let s = spec.unwrap_or("all").trim().to_lowercase();
    match s.as_str() {
        "all" | "on" | "1" | "" => None,
        "none" | "off" | "0" => Some(HashSet::new()),
        _ => Some(
            s.split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(String::from)
                .collect(),
        ),
    }
}

fn chosen(id: &str, path: &str, names: Option<&HashSet<String>>) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    let base = file
        .rsplit_once('.')
        .map_or(file, |(b, _)| b)
        .to_lowercase();
    names.is_none_or(|n| n.contains(&id.to_lowercase()) || n.contains(&base))
}

// ---------------------------------------------------------------- an RGBA picture
#[derive(Clone)]
struct Img {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Img {
    fn get(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.w + x) * 4;
        self.px[i..i + 4].try_into().unwrap()
    }
    fn set(&mut self, x: usize, y: usize, p: [u8; 4]) {
        let i = (y * self.w + x) * 4;
        self.px[i..i + 4].copy_from_slice(&p);
    }
    /// The box (inclusive) lies inside the picture: the pixels it names can be read.
    fn holds(&self, (x0, y0, x1, y1): (i64, i64, i64, i64)) -> bool {
        0 <= x0 && x0 <= x1 && x1 < self.w as i64 && 0 <= y0 && y0 <= y1 && y1 < self.h as i64
    }
    /// Pillow's `paste(colour, (x0, y0, x1, y1))`, clipped to the picture.
    fn fill(&mut self, (x0, y0, x1, y1): (i64, i64, i64, i64), p: [u8; 4]) {
        for y in y0.max(0)..y1.min(self.h as i64) {
            for x in x0.max(0)..x1.min(self.w as i64) {
                self.set(x as usize, y as usize, p);
            }
        }
    }
}

/// A recipe's number (int or float in the JSON).
fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// Python's truthiness for a style value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn boxv(v: &Value) -> (i64, i64, i64, i64) {
    let a = |i: usize| v[i].as_i64().unwrap_or(0);
    (a(0), a(1), a(2), a(3))
}

fn rgb(v: &Value) -> [u8; 3] {
    [0, 1, 2].map(|i| v[i].as_u64().unwrap_or(0) as u8)
}

// ---------------------------------------------------------------- drawing
/// The original writing's colour: the median of the brightest 40% ("auto") or the darkest 14% ("dark") of the
/// opaque pixels in the box; or the recipe's [r,g,b].
fn colour(im: &Img, b: (i64, i64, i64, i64), how: &Value) -> [u8; 3] {
    if how.is_array() {
        return rgb(how);
    }
    let dark = how.as_str() == Some("dark");
    let mut px = vec![];
    for y in b.1..=b.3 {
        for x in b.0..=b.2 {
            if (0..im.w as i64).contains(&x) && (0..im.h as i64).contains(&y) {
                let p = im.get(x as usize, y as usize);
                if p[3] > 200 {
                    px.push(p);
                }
            }
        }
    }
    if px.is_empty() {
        return [255; 3];
    }
    let key = |p: &[u8; 4]| u32::from(p[0]) * 3 + u32::from(p[1]) * 6 + u32::from(p[2]);
    if dark {
        px.sort_by_key(key);
    } else {
        px.sort_by_key(|p| std::cmp::Reverse(key(p))); // stable, as Python's reverse sort
    }
    let n = if dark { px.len() / 7 } else { px.len() * 2 / 5 };
    px.truncate(n.max(1));
    [0, 1, 2].map(|c| {
        let mut v: Vec<u8> = px.iter().map(|p| p[c]).collect();
        v.sort_unstable();
        v[v.len() / 2]
    })
}

/// Python's `round()` of a float to an int: halves to even.
fn pyround(v: f64) -> i64 {
    v.round_ties_even() as i64
}

fn clear(im: &mut Img, a: (i64, i64, i64, i64), fill: &Value) -> R<()> {
    let (x0, y0, x1, y1) = a;
    match fill.as_str() {
        Some(f @ ("rows" | "cols")) => {
            if !im.holds(a) {
                return fail(
                    super::Line::key("text.fail_picture_area")
                        .arg("fill", f)
                        .arg("area", format!("{a:?}")),
                );
            }
            let src = im.clone();
            let (w, h) = (im.w as i64, im.h as i64);
            for x in x0..=x1 {
                for y in y0..=y1 {
                    let (p, q, t) = if f == "rows" {
                        (
                            src.get(x as usize, (y0 - 1).max(0) as usize),
                            src.get(x as usize, (y1 + 1).min(h - 1) as usize),
                            (y - y0 + 1) as f64 / (y1 - y0 + 2) as f64,
                        )
                    } else {
                        (
                            src.get((x0 - 1).max(0) as usize, y as usize),
                            src.get((x1 + 1).min(w - 1) as usize, y as usize),
                            (x - x0 + 1) as f64 / (x1 - x0 + 2) as f64,
                        )
                    };
                    let v = [0, 1, 2, 3].map(|c| {
                        let (p, q) = (f64::from(p[c]), f64::from(q[c]));
                        pyround(p + (q - p) * t) as u8
                    });
                    im.set(x as usize, y as usize, v);
                }
            }
        }
        _ => {
            let p = if fill.is_array() {
                [0, 1, 2, 3].map(|i| fill[i].as_u64().unwrap_or(0) as u8)
            } else {
                [0; 4]
            };
            im.fill((x0, y0, x1 + 1, y1 + 1), p);
        }
    }
    Ok(())
}

/// The font, sized as Pillow's `ImageFont.truetype(path, size)`.
struct Font {
    lib: freetype::Library,
    data: std::rc::Rc<Vec<u8>>,
}

const PIXEL: fn(i64) -> i64 = |x| ((x + 32) & -64) >> 6;

/// One line laid out as Pillow's basic layout: glyph indices and advances (26.6), kerning as Pillow adds it.
struct Line {
    face: freetype::Face,
    glyphs: Vec<(u32, i64)>,
}

impl Font {
    fn line(&self, size: i64, text: &str) -> R<Line> {
        let ft = |e: freetype::Error| super::Failed::in_file(FONT, e);
        let face = self.lib.new_memory_face(self.data.clone(), 0).map_err(ft)?;
        face.set_char_size((size * 64) as isize, (size * 64) as isize, 0, 0)
            .map_err(ft)?;
        let kerning = face.has_kerning();
        let mut glyphs: Vec<(u32, i64)> = vec![];
        let mut last = 0;
        for ch in text.chars() {
            let idx = face.get_char_index(ch as usize).unwrap_or(0);
            face.load_glyph(idx, LoadFlag::DEFAULT).map_err(ft)?;
            if kerning
                && last != 0
                && idx != 0
                && let Ok(d) =
                    face.get_kerning(last, idx, freetype::face::KerningMode::KerningDefault)
                && let Some(prev) = glyphs.last_mut()
            {
                prev.1 += PIXEL(i64::from(d.x)); // as Pillow: pixels added to a 26.6 advance
            }
            glyphs.push((idx, i64::from(face.glyph().metrics().horiAdvance)));
            last = idx;
        }
        Ok(Line { face, glyphs })
    }
}

impl Line {
    /// `font.getbbox(text, anchor="ls")` height: the glyphs' pixel boxes and the pen line, top to bottom.
    fn height(&self) -> R<i64> {
        let (mut ymin, mut ymax) = (0i64, 0i64);
        for &(idx, _) in &self.glyphs {
            self.face
                .load_glyph(idx, LoadFlag::DEFAULT)
                .map_err(|e| super::Failed::in_file(FONT, e))?;
            let g = self
                .face
                .glyph()
                .get_glyph()
                .map_err(|e| super::Failed::in_file(FONT, e))?;
            let b = g.get_cbox(freetype::ffi::FT_GLYPH_BBOX_PIXELS);
            ymax = ymax.max(i64::from(b.yMax));
            ymin = ymin.min(i64::from(b.yMin));
        }
        Ok(ymax - ymin)
    }

    /// The words' coverage as Pillow's `ImageDraw.text` leaves it in an "L" image, cropped to the ink
    /// (`m.crop(m.getbbox())`): width, height, pixels.
    fn ink(&self) -> R<(usize, usize, Vec<u8>)> {
        let ft = |e: freetype::Error| super::Failed::in_file(FONT, e);
        let mut parts = vec![];
        let mut x = 0i64;
        for &(idx, adv) in &self.glyphs {
            self.face
                .load_glyph(idx, LoadFlag::DEFAULT | LoadFlag::RENDER)
                .map_err(ft)?;
            let g = self.face.glyph();
            let bm = g.bitmap();
            let (bw, rows, pitch) = (bm.width() as usize, bm.rows() as usize, bm.pitch() as isize);
            let buf = bm.buffer();
            let mut rowv = Vec::with_capacity(bw * rows);
            for r in 0..rows {
                let start = if pitch >= 0 {
                    r * pitch as usize
                } else {
                    (rows - 1 - r) * (-pitch) as usize
                };
                rowv.extend_from_slice(&buf[start..start + bw]);
            }
            parts.push((
                PIXEL(x) + i64::from(g.bitmap_left()),
                -i64::from(g.bitmap_top()),
                bw,
                rows,
                rowv,
            ));
            x += adv;
        }
        let x0 = parts.iter().map(|p| p.0).min().unwrap_or(0);
        let y0 = parts.iter().map(|p| p.1).min().unwrap_or(0);
        let x1 = parts
            .iter()
            .map(|p| p.0 + p.2 as i64)
            .max()
            .unwrap_or(1)
            .max(x0 + 1);
        let y1 = parts
            .iter()
            .map(|p| p.1 + p.3 as i64)
            .max()
            .unwrap_or(1)
            .max(y0 + 1);
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut m = vec![0u8; w * h];
        for (px, py, bw, rows, src) in &parts {
            for r in 0..*rows {
                for k in 0..*bw {
                    let s = u32::from(src[r * bw + k]);
                    if s == 0 {
                        continue;
                    }
                    let t = &mut m[(*py - y0) as usize * w + r * w + (*px - x0) as usize + k];
                    *t = if *t > 0 {
                        let v = u32::from(*t) * (255 - s) + 128;
                        (s + (((v >> 8) + v) >> 8)).min(255) as u8
                    } else {
                        s as u8
                    };
                }
            }
        }
        // the ink only (getbbox: the non-zero pixels; none: one pixel)
        let rows_on: Vec<usize> = (0..h)
            .filter(|&y| m[y * w..(y + 1) * w].iter().any(|&v| v > 0))
            .collect();
        let cols_on: Vec<usize> = (0..w)
            .filter(|&x| (0..h).any(|y| m[y * w + x] > 0))
            .collect();
        let (Some(&ya), Some(&yb), Some(&xa), Some(&xb)) = (
            rows_on.first(),
            rows_on.last(),
            cols_on.first(),
            cols_on.last(),
        ) else {
            return Ok((1, 1, vec![m.first().copied().unwrap_or(0)]));
        };
        let (cw, ch) = (xb - xa + 1, yb - ya + 1);
        let mut out = Vec::with_capacity(cw * ch);
        for y in ya..=yb {
            out.extend_from_slice(&m[y * w + xa..=y * w + xb]);
        }
        Ok((cw, ch, out))
    }
}

fn getf(st: &Map<String, Value>, k: &str, d: f64) -> f64 {
    st.get(k).map_or(d, num)
}

/// One picture of RGB `c` with the alpha `a` composited over `im` (Pillow `Image.merge` + `alpha_composite`).
fn composite(im: &mut Img, c: [u8; 3], a: &[u8]) {
    let mut layer = Vec::with_capacity(a.len() * 4);
    for &v in a {
        layer.extend_from_slice(&[c[0], c[1], c[2], v]);
    }
    pil::alpha_composite(
        &mut im.px,
        im.w as u32,
        &layer,
        im.w as u32,
        im.h as u32,
        0,
        0,
    );
}

/// The words in the box: as tall as the box allows (inside the halo), squeezed sideways down to the style's
/// squeeze, then smaller.
fn draw(
    im: &mut Img,
    b: (i64, i64, i64, i64),
    text: &str,
    st: &Map<String, Value>,
    core: [u8; 3],
    font: &Font,
    refh: i64,
) -> R<()> {
    let (x0, y0, x1, y1) = b;
    let halo = st.get("halo").filter(|v| truthy(Some(v)));
    let pad = if halo.is_some() {
        getf(st, "halo_px", 0.0) as i64
    } else {
        0
    };
    let w = x1 - x0 + 1 - 2 * pad;
    let h = if truthy(st.get("size")) {
        getf(st, "size", 0.0) as i64
    } else {
        y1 - y0 + 1 - 2 * pad
    };
    let size = pyround((100 * h * S) as f64 / refh as f64).max(4);
    let (mw0, mh0, m) = font.line(size, text)?.ink()?;
    let squeeze = getf(st, "squeeze", 0.62);
    let mut sx = ((w * S) as f64 / mw0 as f64).min(1.0);
    let u = (sx / squeeze).min(1.0);
    sx = sx.max(squeeze) * u;
    let mw = pyround(mw0 as f64 * sx / S as f64).max(1);
    let mh = pyround(mh0 as f64 * u / S as f64).max(1);
    let m = pil::resize_lanczos_l(&m, mw0 as u32, mh0 as u32, mw as u32, mh as u32);
    let x = match st.get("align").and_then(Value::as_str).unwrap_or("center") {
        "left" => x0 + pad,
        "right" => x1 - pad + 1 - mw,
        _ => x0 + (x1 - x0 + 1 - mw).div_euclid(2),
    };
    let y = y0 + (y1 - y0 + 1 - mh).div_euclid(2);
    let mut mask = vec![0u8; im.w * im.h];
    for r in 0..mh {
        for c in 0..mw {
            let (tx, ty) = (x + c, y + r);
            if (0..im.w as i64).contains(&tx) && (0..im.h as i64).contains(&ty) {
                mask[ty as usize * im.w + tx as usize] = m[(r * mw + c) as usize];
            }
        }
    }
    if let Some(halo) = halo {
        let hc: [u8; 4] = if halo.as_str() == Some("core") {
            [
                core[0],
                core[1],
                core[2],
                getf(st, "halo_alpha", 160.0) as u8,
            ]
        } else {
            [0, 1, 2, 3].map(|i| halo[i].as_u64().unwrap_or(0) as u8)
        };
        let (iw, ih) = (im.w as u32, im.h as u32);
        let mut g = pil::max_filter_l(&mask, iw, ih, 2 * getf(st, "halo_px", 1.0) as u32 + 1);
        let blur = getf(st, "blur", 0.6);
        if blur != 0.0 {
            g = pil::gaussian_blur_l(&g, iw, ih, blur);
        }
        let a: Vec<u8> = g
            .iter()
            .map(|&v| (u32::from(v) * u32::from(hc[3]) / 255) as u8)
            .collect();
        composite(im, [hc[0], hc[1], hc[2]], &a);
    }
    composite(im, core, &mask);
    Ok(())
}

/// The mirror image under a line of writing, as the original has it: src flipped to dst_y, fading a0 -> a1.
fn reflect(im: &mut Img, rf: &Value) -> R<()> {
    let (x0, y0, x1, y1) = boxv(&rf["src"]);
    let (a0, a1) = (num(&rf["alpha"][0]), num(&rf["alpha"][1]));
    let dst_y = rf["dst_y"].as_i64().unwrap_or(0);
    if !im.holds((x0, y0, x1, y1)) || !im.holds((x0, dst_y, x1, dst_y + y1 - y0)) {
        return fail(super::Line::key("text.fail_picture_reflection"));
    }
    let (mw, h) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
    let mut m = Vec::with_capacity(mw * h * 4);
    for r in 0..h {
        let sy = (y1 as usize) - r; // flipped top to bottom
        let f = pyround(255.0 * (a0 + (a1 - a0) * (r as f64 / (h.max(2) - 1) as f64))) as u32;
        for c in 0..mw {
            let mut p = im.get(x0 as usize + c, sy);
            p[3] = (u32::from(p[3]) * f / 255) as u8;
            m.extend_from_slice(&p);
        }
    }
    im.fill((x0, dst_y, x1 + 1, dst_y + h as i64), [0; 4]);
    pil::alpha_composite(
        &mut im.px,
        im.w as u32,
        &m,
        mw as u32,
        h as u32,
        x0 as u32,
        dst_y as u32,
    );
    Ok(())
}

/// Every label of a recipe cleared first, then drawn (a neighbour's halo is never cleared away).
fn redraw(im: &mut Img, r: &Value, font: &Font, refh: i64) -> R<()> {
    let orig = im.clone();
    let style = r["style"].as_object().cloned().unwrap_or_default();
    let fill = |st: &Map<String, Value>| st.get("fill").cloned().unwrap_or_else(|| "clear".into());
    for a in r["clear"].as_array().into_iter().flatten() {
        clear(im, boxv(a), &fill(&style))?;
    }
    let labels = r["labels"].as_array().cloned().unwrap_or_default();
    let merged = |lab: &Value| {
        let mut st = style.clone();
        for (k, v) in lab.as_object().into_iter().flatten() {
            st.insert(k.clone(), v.clone());
        }
        st
    };
    for lab in &labels {
        let st = merged(lab);
        let area = lab.get("clear").unwrap_or(&lab["box"]);
        clear(im, boxv(area), &fill(&st))?;
    }
    for lab in &labels {
        let st = merged(lab);
        let core = colour(
            &orig,
            boxv(&lab["box"]),
            st.get("core").unwrap_or(&Value::from("auto")),
        );
        draw(
            im,
            boxv(&lab["box"]),
            lab["text"].as_str().unwrap_or(""),
            &st,
            core,
            font,
            refh,
        )?;
    }
    // a strip another picture already shows (a title over two textures): made transparent after drawing, so it
    // shows once. Its colour stays: the game filters across the seam, and black there draws a grey line (#331).
    // shortcut: the colour holds at level 0 only (`encode`'s smaller levels premultiply it away), enough while each
    // picture with an erase has one level; keep the colour in the mips when one ships with more
    for a in r["erase"].as_array().into_iter().flatten() {
        let (x0, y0, x1, y1) = boxv(a);
        for y in y0.max(0)..=y1.min(im.h as i64 - 1) {
            for x in x0.max(0)..=x1.min(im.w as i64 - 1) {
                im.px[(y as usize * im.w + x as usize) * 4 + 3] = 0;
            }
        }
    }
    if r.get("reflect").is_some_and(|v| truthy(Some(v))) {
        reflect(im, &r["reflect"])?;
    }
    Ok(())
}

/// The entry with our words: a picture redrawn, or a scene's text replaced ([`retext`]).
fn remake(
    data: &[u8],
    r: &Value,
    font: &Font,
    refh: i64,
    p: &str,
    notes: &mut Vec<super::Line>,
) -> R<Vec<u8>> {
    if p.ends_with(".sbscene") {
        let mut d = data.to_vec();
        retext(&mut d, r, notes)?;
        return Ok(d);
    }
    let (w, h, px) = bcn::decode_dds(data).map_err(|e| super::Failed::in_file(p, e))?;
    let mut im = Img {
        w: w as usize,
        h: h as usize,
        px,
    };
    redraw(&mut im, r, font, refh).map_err(|e| e.after(p))?;
    encode(data, &im)
}

/// A scene's literal text, same byte length: each label's `at` is the length byte (`len`) of a KEY text the game
/// draws as it is; the words, centred in spaces, take the original bytes' place, so no chunk size changes. A label
/// with no words keeps the original text, and `notes` says so.
fn retext(d: &mut [u8], r: &Value, notes: &mut Vec<super::Line>) -> R<()> {
    for lab in r["labels"].as_array().into_iter().flatten() {
        let num = |k: &str| usize::try_from(lab[k].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
        let (at, len) = (num("at"), num("len"));
        let t = lab["text"].as_str().unwrap_or("");
        let fits = at.checked_add(len).is_some_and(|e| e < d.len()) && usize::from(d[at]) == len;
        if !t.is_ascii() || t.len() > len || !fits {
            return fail(
                super::Line::key("text.fail_scene_text")
                    .arg("id", r["id"].as_str().unwrap_or(""))
                    .arg("text", t)
                    .arg("at", at),
            );
        }
        if t.is_empty() {
            notes.push(
                super::Line::key("text.note_picture_no_words")
                    .arg("id", r["id"].as_str().unwrap_or(""))
                    .arg("at", at),
            );
            continue;
        }
        let pad = len - t.len();
        let s = format!("{}{t}{}", " ".repeat(pad / 2), " ".repeat(pad - pad / 2));
        d[at + 1..at + 1 + len].copy_from_slice(s.as_bytes());
    }
    Ok(())
}

// ---------------------------------------------------------------- DDS: the original's format, size and mips
fn pack_pixels(im: &[u8], bits: u32, masks: [u32; 4]) -> Vec<u8> {
    if bits == 32 && masks == [0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000] {
        return im
            .chunks(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
    }
    let luts: Vec<(usize, [u32; 256])> = masks
        .iter()
        .enumerate()
        .filter(|(_, m)| **m != 0)
        .map(|(c, &m)| {
            let (sh, n) = (m.trailing_zeros(), m.count_ones());
            (
                c,
                std::array::from_fn(|v| ((v as u32 * ((1 << n) - 1) + 127) / 255) << sh),
            )
        })
        .collect();
    let nb = (bits / 8) as usize;
    let mut out = Vec::with_capacity(im.len() / 4 * nb);
    for p in im.chunks(4) {
        let v: u32 = luts.iter().map(|(c, t)| t[p[*c] as usize]).sum();
        out.extend_from_slice(&v.to_le_bytes()[..nb]);
    }
    out
}

/// A DDS like `orig` (its 128-byte header kept): same format, size and mip count; the mips made from `im`.
fn encode(orig: &[u8], im: &Img) -> R<Vec<u8>> {
    let nm = u32le(orig, 28).max(1);
    let (pf_flags, fourcc, bits) = (u32le(orig, 80), &orig[84..88], u32le(orig, 88));
    let masks = [92, 96, 100, 104].map(|o| u32le(orig, o));
    let mut out = orig[..128].to_vec();
    let mut lv = im.clone();
    for n in 0..nm {
        if n > 0 {
            let (w, h) = ((lv.w / 2).max(1), (lv.h / 2).max(1));
            lv = Img {
                w,
                h,
                px: pil::resize_box_rgba(&lv.px, lv.w as u32, lv.h as u32, w as u32, h as u32),
            };
        }
        if pf_flags & 4 != 0 {
            if fourcc != b"DXT5" {
                return fail(super::Line::raw(format!(
                    "DDS format {}",
                    String::from_utf8_lossy(fourcc)
                )));
            }
            out.extend(bcn::encode_dxt5(&lv.px, lv.w as u32, lv.h as u32));
        } else {
            out.extend(pack_pixels(&lv.px, bits, masks));
        }
    }
    if out.len() != orig.len() {
        return fail(
            super::Line::key("text.fail_picture_size")
                .arg("new", out.len())
                .arg("old", orig.len()),
        );
    }
    Ok(out)
}

// ---------------------------------------------------------------- YS packing (the exact inverse of lzw)
/// YS LZW as the archive packs: early width change, a fresh table after 3,840 codes.
fn ys_pack(data: &[u8]) -> Vec<u8> {
    let mut out = b"YS\x02\x00".to_vec();
    let (mut acc, mut nb) = (0u64, 0u32);
    let (mut i, n) = (0, data.len());
    while i < n {
        let mut table: HashMap<(u16, u8), u16> = HashMap::new();
        let (mut nxt, mut dn, mut dw, mut j) = (256u16, 256u32, 9u32, 0);
        while i < n && j < 3840 {
            let mut code = u16::from(data[i]);
            i += 1;
            while i < n {
                match table.get(&(code, data[i])) {
                    Some(&c) => {
                        code = c;
                        i += 1;
                    }
                    None => break,
                }
            }
            acc = (acc << dw) | u64::from(code);
            nb += dw;
            j += 1;
            while nb >= 8 {
                nb -= 8;
                out.push((acc >> nb) as u8);
            }
            acc &= (1 << nb) - 1;
            if i < n {
                table.insert((code, data[i]), nxt);
                nxt += 1;
            }
            if j >= 2 {
                dn += 1;
                if dn + 1 == 1 << dw && dw < 12 {
                    dw += 1;
                }
            }
        }
    }
    if nb > 0 {
        out.push((acc << (8 - nb)) as u8);
    }
    out
}

// ---------------------------------------------------------------- the archive
/// The pictures' way back: `None` when missing, an error when unreadable or damaged (never "nothing to restore").
fn load_state(path: &Path) -> R<Option<Map<String, Value>>> {
    let d = match fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        r => r?,
    };
    serde_json::from_slice(&d).map(Some).map_err(|e| {
        super::Failed(
            super::Line::key("text.fail_state_damaged_detail")
                .arg("file", STATE)
                .arg("detail", e),
        )
    })
}

/// `{path: entry number}` for every file in the archive.
fn toc(f: &mut File) -> R<HashMap<String, usize>> {
    let mut hdr = [0u8; TOC_AT as usize];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut hdr)?;
    if &hdr[..4] != b"xaf0" {
        return fail(super::Line::key("text.fail_xaf_archive").arg("file", "data\\wccf_data.xaf"));
    }
    let n = u32le(&hdr, 12) as usize;
    // a damaged count must not size the allocation: the table has to fit in the file
    let len = f.seek(SeekFrom::End(0))?;
    if (n as u64).saturating_mul(ENTRY as u64) > len.saturating_sub(TOC_AT) {
        return fail(super::Line::key("text.fail_xaf_count").arg("file", "data\\wccf_data.xaf"));
    }
    f.seek(SeekFrom::Start(TOC_AT))?;
    let mut raw = vec![0u8; n * ENTRY];
    f.read_exact(&mut raw)?;
    let e = |i: usize| &raw[i * ENTRY..(i + 1) * ENTRY];
    let name = |i: usize| {
        let s = &e(i)[..0x80];
        s[..s.iter().position(|&c| c == 0).unwrap_or(0x80)]
            .iter()
            .map(|&b| char::from(b))
            .collect::<String>()
    };
    let parent = |i: usize| i32::from_le_bytes(e(i)[0x84..0x88].try_into().unwrap());
    let mut out = HashMap::new();
    for i in (0..n).filter(|&i| e(i)[0x80] != 0) {
        let (mut parts, mut p) = (vec![name(i)], parent(i));
        while p >= 0 && (p as usize) < n && parts.len() <= n {
            parts.push(name(p as usize));
            p = parent(p as usize);
        }
        parts.reverse();
        out.insert(parts.join("/"), i);
    }
    Ok(out)
}

fn read_entry(f: &mut File, e: &[u8]) -> R<Vec<u8>> {
    if e.len() != ENTRY {
        return Err(super::Failed(
            super::Line::key("text.fail_state_short").arg("file", STATE),
        ));
    }
    let (size, stored) = (u32le(e, 0x94) as usize, u32le(e, 0x98) as usize);
    let at = u64::from(u32le(e, 0xA0)) * SECTOR;
    // a damaged size must not size the allocation: the data has to fit in the file
    if at.saturating_add(stored as u64) > f.seek(SeekFrom::End(0))? {
        return fail(super::Line::key("text.fail_xaf_size").arg("file", "data\\wccf_data.xaf"));
    }
    f.seek(SeekFrom::Start(at))?;
    let mut d = vec![0u8; stored];
    f.read_exact(&mut d)?;
    Ok(if stored < size { lzw(&d, size)? } else { d })
}

/// The redrawn pictures in the recipes' order: (entry number, path, new entry data).
type Pics = Vec<(usize, String, Vec<u8>)>;

/// The font file: the kit's `fonts\`, or the repository's `assets/fonts` beside `editions/<ed>`.
fn font_path(sheets: &Path) -> Option<PathBuf> {
    [sheets.join("fonts"), sheets.join("../../assets/fonts")]
        .into_iter()
        .map(|d| d.join(FONT))
        .find(|p| p.is_file())
}

/// Every chosen picture whose original matches its recipe, redrawn, and the notes. Reads the original entries (from
/// the saved way back while pictures are on).
pub fn build(
    game: &Path,
    sheets: &Path,
    it: bool,
    spec: Option<&str>,
    state: &Path,
) -> R<(Pics, Vec<super::Line>)> {
    let names = wanted(spec);
    if names.as_ref().is_some_and(HashSet::is_empty) {
        return Ok((Pics::new(), vec![super::Line::key("text.pictures_none")]));
    }
    let Some(fp) = font_path(sheets) else {
        return Ok((
            Pics::new(),
            vec![super::Line::key("text.pictures_no_font").arg("font", FONT)],
        ));
    };
    let font = Font {
        lib: freetype::Library::init().map_err(|e| super::Failed::in_file("FreeType", e))?,
        data: std::rc::Rc::new(fs::read(fp)?),
    };
    let refh = font.line(100, "Hgjy")?.height()?;
    let saved = load_state(state)?
        .and_then(|s| s.get("entries").and_then(Value::as_object).cloned())
        .unwrap_or_default();
    let mut f = File::open(game.join(XAF))?;
    let index = toc(&mut f)?;
    let (mut out, mut skipped, mut seen) = (Pics::new(), vec![], false);
    // notes of a scene label left as the original, ahead of the summary
    let mut notes = vec![];
    for r in recipes(sheets, it)? {
        let id = r["id"].as_str().unwrap_or("");
        for p in r["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !chosen(id, p, names.as_ref()) {
                continue;
            }
            seen = true;
            let Some(&i) = index.get(p) else {
                skipped.push(super::Line::key("text.why_not_in_archive").arg("picture", p));
                continue;
            };
            let e = match saved.get(&i.to_string()).and_then(Value::as_str) {
                Some(h) => hex(h)?,
                None => {
                    let mut e = vec![0u8; ENTRY];
                    f.seek(SeekFrom::Start(TOC_AT + (i * ENTRY) as u64))?;
                    f.read_exact(&mut e)?;
                    e
                }
            };
            let data = read_entry(&mut f, &e)?;
            if sha(&data) != r["sha256"].as_str().unwrap_or("").to_uppercase() {
                skipped.push(super::Line::key("text.why_not_the_recipe").arg("picture", p));
                continue;
            }
            out.push((
                i,
                p.to_owned(),
                remake(&data, &r, &font, refh, p, &mut notes)?,
            ));
        }
    }
    if names.is_some() && !seen {
        return fail(super::Line::key("text.fail_pictures_name").arg("name", spec.unwrap_or("")));
    }
    let lang = if it { super::Lang::It } else { super::Lang::En };
    notes.insert(
        0,
        super::Line::key("text.note_pictures")
            .arg("count", out.len())
            .part("lang", lang.line()),
    );
    if let Some(s) = skipped.first() {
        let key = if skipped.len() > 1 {
            "text.note_pictures_kept_more"
        } else {
            "text.note_pictures_kept"
        };
        notes.push(
            super::Line::key(key)
                .arg("count", skipped.len())
                .part("first", s.clone()),
        );
    }
    Ok((out, notes))
}

fn hex(s: &str) -> R<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return Err(super::Failed(
            super::Line::key("text.fail_state_hex").arg("file", STATE),
        ));
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2).unwrap_or("zz"), 16))
        .collect::<Result<_, _>>()
        .map_err(|_| super::Failed(super::Line::key("text.fail_state_hex").arg("file", STATE)))
}

fn to_hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// The original entries and header back, the added pictures cut off; 0 if none were added.
pub fn restore(game: &Path, state_path: &Path) -> R<usize> {
    let Some(state) = load_state(state_path)? else {
        return Ok(0);
    };
    let length = state.get("length").and_then(Value::as_u64);
    let header = state
        .get("header")
        .and_then(Value::as_str)
        .map(hex)
        .transpose()?;
    let entries = state.get("entries").and_then(Value::as_object);
    let (Some(length), Some(header), Some(entries)) = (length, header, entries) else {
        return fail(super::Line::key("text.fail_state_way_back").arg("file", STATE));
    };
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(game.join(XAF))?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)?;
    if &magic != b"xaf0" || f.seek(SeekFrom::End(0))? < length {
        return fail(super::Line::key("text.fail_xaf_shorter").arg("file", "data\\wccf_data.xaf"));
    }
    // everything is checked before the first byte is written; the state file stays when anything is wrong
    let bad = || super::Failed(super::Line::key("text.fail_state_damaged").arg("file", STATE));
    if header.len() != 0x18 {
        return Err(bad());
    }
    let mut fixes = Vec::new();
    for (i, raw) in entries {
        let i: u64 = i.parse().map_err(|_| bad())?;
        let e = hex(raw.as_str().unwrap_or(""))?;
        if e.len() != ENTRY || TOC_AT + (i + 1) * ENTRY as u64 > length {
            return Err(bad());
        }
        fixes.push((i, e));
    }
    for (i, e) in fixes {
        f.seek(SeekFrom::Start(TOC_AT + i * ENTRY as u64))?;
        f.write_all(&e)?;
    }
    f.seek(SeekFrom::Start(0x18))?;
    f.write_all(&header)?;
    f.sync_all()?;
    f.set_len(length)?;
    f.sync_all()?;
    drop(f);
    fs::remove_file(state_path)?;
    Ok(entries.len())
}

/// The state is this archive's way back: the archive is longer than the state's `length` and every entry it names
/// points past that length, where [`apply`] put the pictures; and it holds what [`restore`] needs (a header that
/// agrees with that length, whole entries). Another folder's state is taken only then.
pub fn state_matches(game: &Path, state_path: &Path) -> bool {
    let Ok(Some(s)) = load_state(state_path) else {
        return false;
    };
    let (Some(length), Some(header), Some(entries)) = (
        s.get("length").and_then(Value::as_u64),
        s.get("header")
            .and_then(Value::as_str)
            .and_then(|h| hex(h).ok()),
        s.get("entries").and_then(Value::as_object),
    ) else {
        return false;
    };
    if header.len() != 0x18
        || u64::from_le_bytes(header[..8].try_into().unwrap()) * SECTOR != length
        || !entries.values().all(|e| {
            e.as_str()
                .and_then(|e| hex(e).ok())
                .is_some_and(|e| e.len() == ENTRY)
        })
    {
        return false;
    }
    let Ok(mut f) = File::open(game.join(XAF)) else {
        return false;
    };
    if f.seek(SeekFrom::End(0)).map_or(true, |end| end <= length) || entries.is_empty() {
        return false;
    }
    entries.keys().all(|i| {
        let mut e = [0u8; ENTRY];
        i.parse::<u64>().is_ok_and(|i| {
            TOC_AT + (i + 1) * ENTRY as u64 <= length
                && f.seek(SeekFrom::Start(TOC_AT + i * ENTRY as u64)).is_ok()
                && f.read_exact(&mut e).is_ok()
                && u64::from(u32le(&e, 0xA0)) * SECTOR >= length
        })
    })
}

/// From the original archive every time (never stacked): the way back saved, the pictures added at the end (YS
/// packed when that is smaller), the header grown, the entries pointed at them, each read back.
pub fn apply(game: &Path, pics: &Pics, state_path: &Path) -> R<usize> {
    restore(game, state_path)?;
    if pics.is_empty() {
        return Ok(0);
    }
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(game.join(XAF))?;
    let mut hdr = [0u8; TOC_AT as usize];
    f.read_exact(&mut hdr)?;
    let length = f.seek(SeekFrom::End(0))?;
    let q = |o: usize| u64::from_le_bytes(hdr[o..o + 8].try_into().unwrap());
    let (total, toc_sectors, data_sectors) = (q(0x18), q(0x20), q(0x28));
    if &hdr[..4] != b"xaf0" || length % SECTOR != 0 || total * SECTOR != length {
        return fail(super::Line::key("text.fail_xaf_length").arg("file", "data\\wccf_data.xaf"));
    }
    let mut old = vec![];
    for (i, ..) in pics {
        let mut e = vec![0u8; ENTRY];
        f.seek(SeekFrom::Start(TOC_AT + (i * ENTRY) as u64))?;
        f.read_exact(&mut e)?;
        old.push((*i, e));
    }
    // the way back, before the first byte is written (Python's json.dump(indent=1), keys in the same order)
    let mut st = String::from("{\n \"length\": ");
    st += &format!(
        "{length},\n \"header\": \"{}\",\n \"entries\": {{",
        to_hex(&hdr[0x18..0x30])
    );
    for (n, (i, e)) in old.iter().enumerate() {
        st += &format!(
            "{}\n  \"{i}\": \"{}\"",
            if n > 0 { "," } else { "" },
            to_hex(e)
        );
    }
    st += "\n }\n}";
    super::put(state_path, st.as_bytes())?;
    let (mut pos, mut entries) = (length, std::collections::BTreeMap::new());
    let mut sorted: Vec<_> = pics.iter().zip(&old).collect();
    sorted.sort_by_key(|((i, ..), _)| *i);
    for ((i, _, blob), (_, e)) in sorted {
        let mut packed = ys_pack(blob);
        if packed.len() >= blob.len() || lzw(&packed, blob.len()).ok().as_ref() != Some(blob) {
            packed.clone_from(blob);
        }
        let stored_as_is = &packed == blob;
        let pad = (SECTOR - packed.len() as u64 % SECTOR) % SECTOR;
        f.seek(SeekFrom::Start(pos))?;
        f.write_all(&packed)?;
        f.write_all(&vec![0u8; pad as usize])?;
        let mut e = e.clone();
        if stored_as_is {
            (e[0x81], e[0x82]) = (0, 0); // stored as is (3,541 original entries are)
        }
        e[0x94..0x98].copy_from_slice(&(blob.len() as u32).to_le_bytes());
        e[0x98..0x9C].copy_from_slice(&(packed.len() as u32).to_le_bytes());
        e[0xA0..0xA4].copy_from_slice(&((pos / SECTOR) as u32).to_le_bytes());
        entries.insert(*i, e);
        pos += packed.len() as u64 + pad;
    }
    f.sync_all()?;
    let added = (pos - length) / SECTOR;
    f.seek(SeekFrom::Start(0x18))?;
    for v in [total + added, toc_sectors, data_sectors + added] {
        f.write_all(&v.to_le_bytes())?;
    }
    for (i, e) in &entries {
        f.seek(SeekFrom::Start(TOC_AT + (i * ENTRY) as u64))?;
        f.write_all(e)?;
    }
    f.sync_all()?;
    for (i, p, blob) in pics {
        let mut e = vec![0u8; ENTRY];
        f.seek(SeekFrom::Start(TOC_AT + (i * ENTRY) as u64))?;
        f.read_exact(&mut e)?;
        if e != entries[i] || &read_entry(&mut f, &e)? != blob {
            return fail(super::Line::key("text.fail_not_written").arg("file", p));
        }
    }
    Ok(pics.len())
}

#[cfg(test)]
mod tests;
