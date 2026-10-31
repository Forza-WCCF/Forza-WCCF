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

use super::{R, bcn, fail, pil, sha};
use crate::cards::pictures::lzw;
use freetype::face::LoadFlag;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const XAF: &str = "data/wccf_data.xaf";
pub const STATE: &str = "wccf_data.xaf.json";
pub const FONT: &str = "BarlowCondensed-Bold.ttf";
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
        serde_json::from_slice(&d).map_err(|e| super::Failed(format!("{}: {e}", p.display())))
    };
    let mut out = match read(sheets.join("english").join("pictures.json"))?
        .get_mut("pictures")
        .map(Value::take)
    {
        Some(Value::Array(a)) => a,
        _ => return fail("english\\pictures.json has no pictures list"),
    };
    let p = sheets.join("italian").join("pictures.json");
    let itw = if it && p.is_file() {
        read(p)?
    } else {
        json!({})
    };
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
            return fail(format!(
                "italian\\pictures.json: {id} has {} words for {} labels",
                words.len(),
                labels.len()
            ));
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

fn clear(im: &mut Img, a: (i64, i64, i64, i64), fill: &Value) {
    let (x0, y0, x1, y1) = a;
    match fill.as_str() {
        Some(f @ ("rows" | "cols")) => {
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
        let ft = |e: freetype::Error| super::Failed(format!("{FONT}: {e}"));
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
                .map_err(|e| super::Failed(format!("{FONT}: {e}")))?;
            let g = self
                .face
                .glyph()
                .get_glyph()
                .map_err(|e| super::Failed(format!("{FONT}: {e}")))?;
            let b = g.get_cbox(freetype::ffi::FT_GLYPH_BBOX_PIXELS);
            ymax = ymax.max(i64::from(b.yMax));
            ymin = ymin.min(i64::from(b.yMin));
        }
        Ok(ymax - ymin)
    }

    /// The words' coverage as Pillow's `ImageDraw.text` leaves it in an "L" image, cropped to the ink
    /// (`m.crop(m.getbbox())`): width, height, pixels.
    fn ink(&self) -> R<(usize, usize, Vec<u8>)> {
        let ft = |e: freetype::Error| super::Failed(format!("{FONT}: {e}"));
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

fn get<'a>(st: &'a Map<String, Value>, k: &str) -> Option<&'a Value> {
    st.get(k)
}

fn getf(st: &Map<String, Value>, k: &str, d: f64) -> f64 {
    get(st, k).map_or(d, num)
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
    let halo = get(st, "halo").filter(|v| truthy(Some(v)));
    let pad = if halo.is_some() {
        getf(st, "halo_px", 0.0) as i64
    } else {
        0
    };
    let w = x1 - x0 + 1 - 2 * pad;
    let h = if truthy(get(st, "size")) {
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
    let x = match get(st, "align").and_then(Value::as_str).unwrap_or("center") {
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
fn reflect(im: &mut Img, rf: &Value) {
    let (x0, y0, x1, y1) = boxv(&rf["src"]);
    let (a0, a1) = (num(&rf["alpha"][0]), num(&rf["alpha"][1]));
    let dst_y = rf["dst_y"].as_i64().unwrap_or(0);
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
}

/// Every label of a recipe cleared first, then drawn (a neighbour's halo is never cleared away).
fn redraw(im: &mut Img, r: &Value, font: &Font, refh: i64) -> R<()> {
    let orig = im.clone();
    let style = r["style"].as_object().cloned().unwrap_or_default();
    let fill = |st: &Map<String, Value>| st.get("fill").cloned().unwrap_or_else(|| "clear".into());
    for a in r["clear"].as_array().into_iter().flatten() {
        clear(im, boxv(a), &fill(&style));
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
        clear(im, boxv(area), &fill(&st));
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
    if r.get("reflect").is_some_and(|v| truthy(Some(v))) {
        reflect(im, &r["reflect"]);
    }
    Ok(())
}

// ---------------------------------------------------------------- DDS: the original's format, size and mips
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

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
    let nm = u32_at(orig, 28).max(1);
    let (pf_flags, fourcc, bits) = (u32_at(orig, 80), &orig[84..88], u32_at(orig, 88));
    let masks = [92, 96, 100, 104].map(|o| u32_at(orig, o));
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
                return fail(format!("DDS format {}", String::from_utf8_lossy(fourcc)));
            }
            out.extend(bcn::encode_dxt5(&lv.px, lv.w as u32, lv.h as u32));
        } else {
            out.extend(pack_pixels(&lv.px, bits, masks));
        }
    }
    if out.len() != orig.len() {
        return fail(format!(
            "the redrawn picture's size {} differs from the original's {}",
            out.len(),
            orig.len()
        ));
    }
    Ok(out)
}

// ---------------------------------------------------------------- YS packing (the exact inverse of lzw)
/// YS LZW as the archive packs: early width change, a fresh table after 3,840 codes.
pub fn ys_pack(data: &[u8]) -> Vec<u8> {
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
fn load_state(path: &Path) -> Option<Map<String, Value>> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// `{path: entry number}` for every file in the archive.
fn toc(f: &mut File) -> R<HashMap<String, usize>> {
    let mut hdr = [0u8; TOC_AT as usize];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut hdr)?;
    if &hdr[..4] != b"xaf0" {
        return fail("data\\wccf_data.xaf is not an xaf0 archive");
    }
    let n = u32_at(&hdr, 12) as usize;
    // a damaged count must not size the allocation: the table has to fit in the file
    let len = f.seek(SeekFrom::End(0))?;
    if (n as u64).saturating_mul(ENTRY as u64) > len.saturating_sub(TOC_AT) {
        return fail("data\\wccf_data.xaf is cut short or damaged (its entry count)");
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
    let (size, stored) = (u32_at(e, 0x94) as usize, u32_at(e, 0x98) as usize);
    f.seek(SeekFrom::Start(u64::from(u32_at(e, 0xA0)) * SECTOR))?;
    let mut d = vec![0u8; stored];
    f.read_exact(&mut d)?;
    Ok(if stored < size { lzw(&d, size)? } else { d })
}

/// The redrawn pictures in the recipes' order: (entry number, path, new entry data).
pub type Pics = Vec<(usize, String, Vec<u8>)>;

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
) -> R<(Pics, Vec<String>)> {
    let names = wanted(spec);
    if names.as_ref().is_some_and(HashSet::is_empty) {
        return Ok((
            Pics::new(),
            vec!["pictures: none chosen, they stay as the original".into()],
        ));
    }
    let Some(fp) = font_path(sheets) else {
        return Ok((
            Pics::new(),
            vec![format!(
                "pictures stay as the original: fonts\\{FONT} is missing"
            )],
        ));
    };
    let font = Font {
        lib: freetype::Library::init().map_err(|e| super::Failed(format!("FreeType: {e}")))?,
        data: std::rc::Rc::new(fs::read(fp)?),
    };
    let refh = font.line(100, "Hgjy")?.height()?;
    let saved = load_state(state)
        .and_then(|s| s.get("entries").and_then(Value::as_object).cloned())
        .unwrap_or_default();
    let mut f = File::open(game.join(XAF))?;
    let index = toc(&mut f)?;
    let (mut out, mut skipped, mut seen) = (Pics::new(), vec![], false);
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
                skipped.push(format!("{p} (not in the archive)"));
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
                skipped.push(format!("{p} (not the picture the recipe is for)"));
                continue;
            }
            let (w, h, px) =
                bcn::decode_dds(&data).map_err(|e| super::Failed(format!("{p}: {e}")))?;
            let mut im = Img {
                w: w as usize,
                h: h as usize,
                px,
            };
            redraw(&mut im, &r, &font, refh)?;
            out.push((i, p.to_owned(), encode(&data, &im)?));
        }
    }
    if names.is_some() && !seen {
        return fail(format!(
            "pictures={} names no picture of english\\pictures.json",
            spec.unwrap_or("")
        ));
    }
    let mut notes = vec![format!(
        "{} pictures with their writing in {}",
        out.len(),
        if it { "Italian" } else { "English" }
    )];
    if let Some(s) = skipped.first() {
        notes.push(format!(
            "{} pictures stay as they are: {s}{}",
            skipped.len(),
            if skipped.len() > 1 { ", ..." } else { "" }
        ));
    }
    Ok((out, notes))
}

fn hex(s: &str) -> R<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return Err(super::Failed(format!("{STATE}: not hex")));
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2).unwrap_or("zz"), 16))
        .collect::<Result<_, _>>()
        .map_err(|_| super::Failed(format!("{STATE}: not hex")))
}

fn to_hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// The original entries and header back, the added pictures cut off; 0 if none were added.
pub fn restore(game: &Path, state_path: &Path) -> R<usize> {
    let Some(state) = load_state(state_path) else {
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
        return fail(format!(
            "{STATE} is not the archive's way back - left as it is"
        ));
    };
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(game.join(XAF))?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)?;
    if &magic != b"xaf0" || f.seek(SeekFrom::End(0))? < length {
        return fail("data\\wccf_data.xaf is shorter than before the pictures - left as it is");
    }
    // everything is checked before the first byte is written; the state file stays when anything is wrong
    let bad = || super::Failed(format!("{STATE} is damaged - left as it is"));
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
/// points past that length, where [`apply`] put the pictures. Another folder's state is taken only then.
pub fn state_matches(game: &Path, state_path: &Path) -> bool {
    let Some(s) = load_state(state_path) else {
        return false;
    };
    let (Some(length), Some(entries)) = (
        s.get("length").and_then(Value::as_u64),
        s.get("entries").and_then(Value::as_object),
    ) else {
        return false;
    };
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
                && u64::from(u32_at(&e, 0xA0)) * SECTOR >= length
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
        return fail(
            "data\\wccf_data.xaf is not as the original was made (its length) - pictures left as they are",
        );
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
            return fail(format!(
                "{p} did not write correctly - switch the game text back to Japanese"
            ));
        }
    }
    Ok(pics.len())
}

#[cfg(test)]
mod tests {
    //! The retired `test_pictures1718.py` and `test_english_pictures.py` on a scratch archive (no game).
    use super::*;

    fn font_file() -> PathBuf {
        super::super::tests::manifest_dir()
            .join("../../assets/fonts")
            .join(FONT)
    }

    #[test]
    fn freetype_draws_as_pillow_11_3() {
        // Pillow 11.3 (FreeType 2.13.3, basic layout) drew this row of "D" at 72 px as 231 208 208 208 226; the
        // FreeType 2.13.2 of crates.io's freetype-sys drew 229 204 204 204 222 (vendor/freetype-sys)
        let font = Font {
            lib: freetype::Library::init().unwrap(),
            data: std::rc::Rc::new(fs::read(font_file()).unwrap()),
        };
        let (w, h, m) = font.line(72, "D").unwrap().ink().unwrap();
        assert_eq!((w, h), (29, 51));
        assert_eq!(&m[8 * w + 9..8 * w + 15], &[255, 231, 208, 208, 208, 226]);
        assert_eq!(font.line(100, "Hgjy").unwrap().height().unwrap(), 94); // Pillow: getbbox -73..21
    }

    #[test]
    fn ys_pack_round_trips_across_a_table_reset() {
        let mut d: Vec<u8> = (0..40000u32)
            .map(|i| ((i * 7 + (i >> 5)) & 255) as u8)
            .collect();
        d.extend(b"WCCF".repeat(3000));
        assert_eq!(lzw(&ys_pack(&d), d.len()).unwrap(), d);
        assert_eq!(lzw(&ys_pack(b"a"), 1).unwrap(), b"a");
    }

    fn dds_header(
        w: u32,
        h: u32,
        mips: u32,
        fourcc: &[u8; 4],
        bits: u32,
        masks: [u32; 4],
    ) -> Vec<u8> {
        let pf = if fourcc != &[0; 4] {
            4
        } else if masks[3] != 0 {
            0x41
        } else {
            0x40
        };
        let mut d = b"DDS ".to_vec();
        for v in [
            124,
            0x1007 | if mips > 1 { 0x20000 } else { 0 },
            h,
            w,
            0,
            0,
            mips,
        ] {
            d.extend(u32::to_le_bytes(v));
        }
        d.resize(76, 0);
        d.extend(32u32.to_le_bytes());
        d.extend(u32::to_le_bytes(pf));
        d.extend(fourcc);
        d.extend(bits.to_le_bytes());
        for m in masks {
            d.extend(m.to_le_bytes());
        }
        d.extend(0x1000u32.to_le_bytes());
        d.resize(128, 0);
        d
    }

    #[test]
    fn uncompressed_pictures_encode_back_to_their_bytes() {
        // A8R8G8B8, 4 mips: the mips are Pillow's BOX halvings of the top level
        let top: Vec<u8> = (0..64)
            .flat_map(|i| {
                let (x, y) = (i % 8, i / 8);
                [
                    (x * 32) as u8,
                    (y * 32) as u8,
                    (255 - x * 16) as u8,
                    if (x + y) % 3 != 0 { 255 } else { 0 },
                ]
            })
            .collect();
        let mut argb = dds_header(8, 8, 4, &[0; 4], 32, [0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000]);
        let (mut lv, mut w) = (top.clone(), 8);
        for n in 0..4 {
            if n > 0 {
                lv = pil::resize_box_rgba(&lv, w, w, w / 2, w / 2);
                w /= 2;
            }
            argb.extend(lv.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]));
        }
        let im = Img {
            w: 8,
            h: 8,
            px: top,
        };
        assert_eq!(encode(&argb, &im).unwrap(), argb);
        // A4R4G4B4: 4-bit values read and written back
        let mut a4 = dds_header(8, 4, 0, &[0; 4], 16, [0xF00, 0xF0, 0xF, 0xF000]);
        for r in (0..16u16).chain(0..16) {
            let (cr, cg, cb, ca) = (r, r * 5 % 16, 15 - r, r * 3 % 16);
            a4.extend((ca << 12 | cr << 8 | cg << 4 | cb).to_le_bytes());
        }
        let (w, h, px) = bcn::decode_dds(&a4).unwrap();
        assert_eq!(
            encode(
                &a4,
                &Img {
                    w: w as usize,
                    h: h as usize,
                    px
                }
            )
            .unwrap(),
            a4
        );
        assert_eq!(
            pack_pixels(&[0, 136, 255, 17], 16, [0xF00, 0xF0, 0xF, 0xF000]),
            [0x8F, 0x10] // A 1, R 0, G 8, B 15
        );
    }

    #[test]
    fn subsets_by_id_or_file_name() {
        assert!(wanted(None).is_none() && wanted(Some("all")).is_none());
        assert_eq!(wanted(Some("none")), Some(HashSet::new()));
        let n = wanted(Some(" Pic , other"));
        assert!(chosen("x", "ui/pic.dds", n.as_ref()) && chosen("OTHER", "ui/a.dds", n.as_ref()));
        assert!(!chosen("x", "ui/a.dds", n.as_ref()));
    }

    #[test]
    fn colour_is_the_median_of_the_brightest_opaque_pixels() {
        let px = [
            [10, 10, 10, 255],
            [200, 100, 50, 255],
            [250, 250, 250, 255],
            [255, 255, 255, 0],
            [90, 90, 90, 255],
        ];
        let im = Img {
            w: 5,
            h: 1,
            px: px.concat(),
        };
        // 4 opaque, the brightest 40% = 1 pixel
        assert_eq!(colour(&im, (0, 0, 4, 0), &"auto".into()), [250, 250, 250]);
        assert_eq!(colour(&im, (0, 0, 4, 0), &"dark".into()), [10, 10, 10]);
        assert_eq!(colour(&im, (0, 0, 4, 0), &json!([1, 2, 3])), [1, 2, 3]);
        assert_eq!(pyround(2.5), 2);
        assert_eq!(pyround(3.5), 4);
    }

    /// A scratch archive: a folder "ui" and its file "pic.dds" (YS packed) in sector 1.
    struct Scratch {
        _t: tempfile::TempDir,
        game: PathBuf,
        kit: PathBuf,
        state: PathBuf,
        original: Vec<u8>,
        dds: Vec<u8>,
        entry: Vec<u8>,
        hdr: Vec<u8>,
    }

    impl Scratch {
        fn new(packed_dds: bool) -> Self {
            let t = tempfile::tempdir().unwrap();
            let (game, kit) = (t.path().join("game"), t.path().join("kit"));
            for d in [
                game.join("data"),
                kit.join("english"),
                kit.join("italian"),
                kit.join("fonts"),
            ] {
                fs::create_dir_all(d).unwrap();
            }
            fs::copy(font_file(), kit.join("fonts").join(FONT)).unwrap();
            // "the original writing": a white block
            let mut px = vec![0u8; 64 * 32 * 4];
            for y in 8..24 {
                for x in 8..56 {
                    px[(y * 64 + x) * 4..][..4].copy_from_slice(&[255; 4]);
                }
            }
            let mut dds = dds_header(64, 32, 0, b"DXT5", 0, [0; 4]);
            dds.extend(bcn::encode_dxt5(&px, 64, 32));
            let stored = if packed_dds {
                ys_pack(&dds)
            } else {
                dds.clone()
            };
            let mut toc = vec![0u8; 2 * ENTRY];
            toc[..2].copy_from_slice(b"ui");
            toc[0x84..0x88].copy_from_slice(&(-1i32).to_le_bytes());
            let e = &mut toc[ENTRY..];
            e[..7].copy_from_slice(b"pic.dds");
            (e[0x80], e[0x81]) = (1, u8::from(packed_dds));
            e[0x84..0x88].copy_from_slice(&0i32.to_le_bytes());
            e[0x94..0x98].copy_from_slice(&(dds.len() as u32).to_le_bytes());
            e[0x98..0x9C].copy_from_slice(&(stored.len() as u32).to_le_bytes());
            e[0xA0..0xA4].copy_from_slice(&1u32.to_le_bytes());
            let mut hdr = vec![0u8; TOC_AT as usize];
            hdr[..4].copy_from_slice(b"xaf0");
            for (o, v) in [(4, 2u32), (8, SECTOR as u32), (12, 2)] {
                hdr[o..o + 4].copy_from_slice(&v.to_le_bytes());
            }
            let sectors = 1 + stored.len().div_ceil(SECTOR as usize) as u64;
            for (o, v) in [(0x18, sectors), (0x20, 1), (0x28, sectors - 1)] {
                hdr[o..o + 8].copy_from_slice(&v.to_le_bytes());
            }
            let mut original = [hdr.clone(), toc.clone()].concat();
            original.resize(SECTOR as usize, 0);
            original.extend(&stored);
            original.resize(sectors as usize * SECTOR as usize, 0);
            fs::write(game.join(XAF), &original).unwrap();
            let s = Self {
                state: t.path().join("english_backup").join(STATE),
                entry: toc[ENTRY..].to_vec(),
                _t: t,
                game,
                kit,
                original,
                dds,
                hdr,
            };
            s.recipe(&sha(&s.dds));
            fs::write(
                s.kit.join("italian/pictures.json"),
                r#"{"test_pic": ["Sì"]}"#,
            )
            .unwrap();
            s
        }

        fn recipe(&self, h: &str) {
            let r = json!({"pictures": [{"id": "test_pic", "sha256": h, "files": ["ui/pic.dds"],
                "style": {"core": "auto", "halo": [0, 0, 0, 200], "halo_px": 1},
                "labels": [{"box": [6, 6, 57, 25], "en": "OK"}]}]});
            fs::write(self.kit.join("english/pictures.json"), r.to_string()).unwrap();
        }

        fn archive(&self) -> Vec<u8> {
            fs::read(self.game.join(XAF)).unwrap()
        }

        fn build(&self, it: bool, spec: &str) -> R<Pics> {
            build(&self.game, &self.kit, it, Some(spec), &self.state).map(|(p, _)| p)
        }
    }

    #[test]
    fn on_adds_at_the_end_and_off_gives_back_the_original() {
        let s = Scratch::new(true);
        let pics = s.build(false, "all").unwrap();
        assert_eq!(
            pics.iter()
                .map(|(i, p, _)| (*i, p.as_str()))
                .collect::<Vec<_>>(),
            [(1, "ui/pic.dds")]
        );
        assert_eq!(apply(&s.game, &pics, &s.state).unwrap(), 1);
        let on = s.archive();
        let n = s.original.len();
        assert!(
            on.len() > n && on[SECTOR as usize..n] == s.original[SECTOR as usize..],
            "added at the end"
        );
        assert_eq!(
            u64::from_le_bytes(on[0x18..0x20].try_into().unwrap()),
            on.len() as u64 / SECTOR
        );
        let ne = &on[TOC_AT as usize + ENTRY..][..ENTRY];
        assert_eq!(
            u32_at(ne, 0xA0) as usize,
            n / SECTOR as usize,
            "the entry points at the added picture"
        );
        let mut f = File::open(s.game.join(XAF)).unwrap();
        let new = read_entry(&mut f, ne).unwrap();
        assert_eq!((new.len(), &new[..128]), (s.dds.len(), &s.dds[..128]));
        let (_, _, px) = bcn::decode_dds(&new).unwrap();
        assert!(px[(9 * 64 + 9) * 4 + 3] < 100, "the old writing is cleared");
        assert!(px.chunks(4).any(|p| p[3] > 0), "the words are drawn");
        // the way back as Python's json.dump(indent=1)
        let st = fs::read_to_string(&s.state).unwrap();
        assert!(
            st.starts_with(&format!("{{\n \"length\": {n},\n \"header\": \"")),
            "{st}"
        );
        assert!(
            st.ends_with(&format!("\"1\": \"{}\"\n }}\n}}", to_hex(&s.entry))),
            "{st}"
        );

        assert_eq!(
            apply(&s.game, &s.build(false, "all").unwrap(), &s.state).unwrap(),
            1
        );
        assert_eq!(s.archive(), on, "on twice: the same file");
        assert_ne!(
            s.build(true, "all").unwrap()[0].2,
            pics[0].2,
            "Italian words differ"
        );
        assert_eq!(
            s.build(false, "pic").unwrap().len(),
            1,
            "a subset by file name"
        );
        assert!(s.build(false, "none").unwrap().is_empty());
        assert!(s.build(false, "no_such_picture").is_err());

        assert_eq!(restore(&s.game, &s.state).unwrap(), 1);
        assert_eq!(
            s.archive(),
            s.original,
            "off: the original archive byte for byte"
        );
        assert!(!s.state.exists() && restore(&s.game, &s.state).unwrap() == 0);
    }

    #[test]
    fn off_after_an_interrupted_on_and_a_foreign_picture() {
        let s = Scratch::new(true);
        let pics = s.build(false, "all").unwrap();
        apply(&s.game, &pics, &s.state).unwrap();
        // stopped half way: the way back saved, the entry and header not yet written
        let mut f = OpenOptions::new()
            .write(true)
            .open(s.game.join(XAF))
            .unwrap();
        f.seek(SeekFrom::Start(TOC_AT + ENTRY as u64)).unwrap();
        f.write_all(&s.entry).unwrap();
        f.seek(SeekFrom::Start(0x18)).unwrap();
        f.write_all(&s.hdr[0x18..0x30]).unwrap();
        drop(f);
        assert_eq!(restore(&s.game, &s.state).unwrap(), 1);
        assert_eq!(s.archive(), s.original);
        s.recipe(&"00".repeat(32)); // not the recipe's picture: skipped, nothing written
        assert!(s.build(false, "all").unwrap().is_empty());
        assert_eq!(apply(&s.game, &vec![], &s.state).unwrap(), 0);
        assert_eq!(s.archive(), s.original);
    }

    #[test]
    fn a_damaged_way_back_writes_nothing_and_stays() {
        let s = Scratch::new(true);
        apply(&s.game, &s.build(false, "all").unwrap(), &s.state).unwrap();
        let on = s.archive();
        let good = fs::read_to_string(&s.state).unwrap();
        let short = good.replace(&to_hex(&s.entry), &to_hex(&s.entry[..ENTRY - 1]));
        for bad in [short, good.replace("\"header\": \"", "\"header\": \"0")] {
            fs::write(&s.state, &bad).unwrap();
            assert!(restore(&s.game, &s.state).is_err());
            assert_eq!(s.archive(), on, "nothing written");
            assert!(s.state.is_file(), "the state stays");
        }
        // a huge entry count is refused, not allocated
        let mut huge = s.original.clone();
        huge[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        fs::write(s.game.join(XAF), &huge).unwrap();
        assert!(toc(&mut File::open(s.game.join(XAF)).unwrap()).is_err());
    }

    #[test]
    fn a_2010_11_state_is_taken_back() {
        // the 2010-11 script's way of adding a picture: stored as is, entry flags cleared, the same state file
        let s = Scratch::new(false);
        let blob = [s.dds.clone(), vec![7; 3000]].concat();
        let n = s.original.len() as u64;
        let mut e = s.entry.clone();
        (e[0x81], e[0x82]) = (0, 0);
        e[0x94..0x98].copy_from_slice(&(blob.len() as u32).to_le_bytes());
        e[0x98..0x9C].copy_from_slice(&(blob.len() as u32).to_le_bytes());
        e[0xA0..0xA4].copy_from_slice(&((n / SECTOR) as u32).to_le_bytes());
        let mut on = s.archive();
        on[TOC_AT as usize + ENTRY..][..ENTRY].copy_from_slice(&e);
        on.extend(&blob);
        on.resize(on.len().div_ceil(SECTOR as usize) * SECTOR as usize, 0);
        fs::write(s.game.join(XAF), &on).unwrap();
        let st = json!({"length": n, "header": to_hex(&s.hdr[0x18..0x30]), "entries": {"1": to_hex(&s.entry)}});
        fs::create_dir_all(s.state.parent().unwrap()).unwrap();
        fs::write(&s.state, serde_json::to_string_pretty(&st).unwrap()).unwrap();
        assert_eq!(restore(&s.game, &s.state).unwrap(), 1);
        assert_eq!(s.archive(), s.original);
        // an archive cut shorter than the way back says: left as it is
        fs::write(&s.state, st.to_string()).unwrap();
        fs::write(s.game.join(XAF), &s.original[..SECTOR as usize]).unwrap();
        assert!(restore(&s.game, &s.state).is_err() && s.state.exists());
    }
}
