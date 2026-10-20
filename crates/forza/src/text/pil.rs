//! Bit-exact port of a few Pillow 11.3.0 operations on "L" and "RGBA" images.
//!
//! Mirrors `PIL/Image.py` (`resize`, `alpha_composite`), `PIL/ImageFilter.py`
//! (`RankFilter`, `GaussianBlur`), `libImaging/Resample.c`, `BoxBlur.c`,
//! `RankFilter.c`, `Filter.c` (`ImagingExpand`), `AlphaComposite.c` and
//! `Convert.c` (`RGBA` <-> `RGBa`). Images are row-major, top row first.

use std::f64::consts::PI;

// ---------------------------------------------------------------- resample

const PRECISION_BITS: u32 = 32 - 8 - 2;

struct Filter {
    f: fn(f64) -> f64,
    support: f64,
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let x = x * PI;
    x.sin() / x
}

fn lanczos(x: f64) -> f64 {
    if (-3.0..3.0).contains(&x) {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

fn box_filter(x: f64) -> f64 {
    if x > -0.5 && x <= 0.5 { 1.0 } else { 0.0 }
}

const LANCZOS: Filter = Filter {
    f: lanczos,
    support: 3.0,
};
const BOX: Filter = Filter {
    f: box_filter,
    support: 0.5,
};

/// Per-output-pixel (first source index, tap count), plus the fixed-point kernel.
type Coeffs = (usize, Vec<(usize, usize)>, Vec<i32>);

/// `precompute_coeffs` plus `normalize_coeffs_8bpc`; returns (ksize, bounds, kernel).
fn coeffs(in_size: usize, in0: f32, in1: f32, out_size: usize, filt: &Filter) -> Coeffs {
    let scale = f64::from(in1 - in0) / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = filt.support * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(out_size);
    let mut kk = vec![0i32; out_size * ksize];
    for xx in 0..out_size {
        let center = f64::from(in0) + (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let xmin = ((center - support + 0.5) as i32).max(0);
        let xmax = (((center + support + 0.5) as i32).min(in_size as i32) - xmin).max(0);
        let mut k: Vec<f64> = (0..xmax)
            .map(|x| (filt.f)((f64::from(x + xmin) - center + 0.5) * ss))
            .collect();
        let ww: f64 = k.iter().sum();
        if ww != 0.0 {
            k.iter_mut().for_each(|v| *v /= ww);
        }
        for (x, v) in k.iter().enumerate() {
            let p = v * f64::from(1u32 << PRECISION_BITS);
            kk[xx * ksize + x] = if *v < 0.0 {
                (-0.5 + p) as i32
            } else {
                (0.5 + p) as i32
            };
        }
        bounds.push((xmin as usize, xmax as usize));
    }
    (ksize, bounds, kk)
}

fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// Two-pass resample of an `n`-channel 8-bit image, as `ImagingResampleInner`
/// with box = (0, 0, sw, sh).
fn resample(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32, n: usize, filt: &Filter) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    if sw == dw && sh == dh {
        return src.to_vec(); // Image.resize returns self.copy()
    }
    let (kh, bh, kkh) = coeffs(sw, 0.0, sw as f32, dw, filt);
    let (kv, mut bv, kkv) = coeffs(sh, 0.0, sh as f32, dh, filt);
    let first = bv[0].0;
    let last = bv[dh - 1].0 + bv[dh - 1].1;
    let half = 1i32 << (PRECISION_BITS - 1);

    // Horizontal pass, only over the source rows the vertical pass reads.
    let tmp;
    let mid: &[u8] = if dw != sw {
        for b in &mut bv {
            b.0 -= first;
        }
        let mut t = vec![0u8; (last - first) * dw * n];
        for y in first..last {
            let row = &src[y * sw * n..(y + 1) * sw * n];
            for xx in 0..dw {
                let (xmin, xmax) = bh[xx];
                let k = &kkh[xx * kh..];
                for c in 0..n {
                    let mut ss = half;
                    for x in 0..xmax {
                        ss += i32::from(row[(x + xmin) * n + c]) * k[x];
                    }
                    t[((y - first) * dw + xx) * n + c] = clip8(ss);
                }
            }
        }
        tmp = t;
        &tmp
    } else {
        src
    };
    if dh == sh {
        return mid.to_vec();
    }
    // Vertical pass.
    let mut out = vec![0u8; dw * dh * n];
    for yy in 0..dh {
        let (ymin, ymax) = bv[yy];
        let k = &kkv[yy * kv..];
        for xx in 0..dw {
            for c in 0..n {
                let mut ss = half;
                for y in 0..ymax {
                    ss += i32::from(mid[((y + ymin) * dw + xx) * n + c]) * k[y];
                }
                out[(yy * dw + xx) * n + c] = clip8(ss);
            }
        }
    }
    out
}

/// `Image.resize((dw, dh), Image.LANCZOS)` on an "L" image (no box, reducing_gap None: whatever Image.resize does by default).
pub fn resize_lanczos_l(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    resample(src, sw, sh, dw, dh, 1, &LANCZOS)
}

fn muldiv255(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    ((t >> 8) + t) >> 8
}

/// `Image.resize((dw, dh), Image.BOX)` on an "RGBA" image (Image.resize converts RGBA to RGBa and back around the resample: port that conversion exactly, Convert.c).
pub fn resize_box_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    if sw == dw && sh == dh {
        return src.to_vec();
    }
    // RGBA -> RGBa (rgbA2rgba)
    let mut pre = src.to_vec();
    for p in pre.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        for c in &mut p[..3] {
            *c = muldiv255(u32::from(*c), a) as u8;
        }
    }
    let mut out = resample(&pre, sw, sh, dw, dh, 4, &BOX);
    // RGBa -> RGBA (rgba2rgbA)
    for p in out.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((255 * u32::from(*c)) / a).min(255) as u8;
            }
        }
    }
    out
}

// -------------------------------------------------------------- max filter

/// `im.filter(ImageFilter.MaxFilter(size))` on "L" (size odd: 3, 5, 7), edges exactly as Pillow (it expands the image first).
pub fn max_filter_l(src: &[u8], w: u32, h: u32, size: u32) -> Vec<u8> {
    let (w, h, m) = (i64::from(w), i64::from(h), i64::from(size / 2));
    // ImagingExpand replicates edge pixels, which is the same as clamping coordinates.
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let mut best = 0u8;
            for dy in -m..=m {
                for dx in -m..=m {
                    let (xx, yy) = ((x + dx).clamp(0, w - 1), (y + dy).clamp(0, h - 1));
                    best = best.max(src[(yy * w + xx) as usize]);
                }
            }
            out.push(best);
        }
    }
    out
}

// ----------------------------------------------------------- gaussian blur

/// `_gaussian_blur_radius` from BoxBlur.c: `float` arithmetic with `double` sqrt/floor.
fn gaussian_box_radius(radius: f32, passes: i32) -> f32 {
    let sigma2 = radius * radius / passes as f32;
    let big_l = (12.0 * f64::from(sigma2) + 1.0).sqrt() as f32;
    let l = ((f64::from(big_l) - 1.0) / 2.0).floor() as f32;
    let mut a = (2.0 * l + 1.0) * (l * (l + 1.0) - 3.0 * sigma2);
    a /= 6.0 * (sigma2 - (l + 1.0) * (l + 1.0));
    l + a
}

/// `ImagingLineBoxBlur8`.
fn line_box_blur(
    inp: &[u8],
    out: &mut [u8],
    radius: i32,
    edge_a: i32,
    edge_b: i32,
    ww: u32,
    fw: u32,
) {
    let lastx = inp.len() as i32 - 1;
    let px = |i: i32| u32::from(inp[i as usize]);
    let mut acc: u32 = px(0).wrapping_mul(radius as u32 + 1);
    for x in 0..edge_a - 1 {
        acc = acc.wrapping_add(px(x));
    }
    acc = acc.wrapping_add(px(lastx).wrapping_mul((radius - edge_a + 1) as u32));
    let mut step = |x: i32, sub: i32, add: i32, left: i32, right: i32| {
        acc = acc.wrapping_add(px(add)).wrapping_sub(px(sub));
        let bulk = acc
            .wrapping_mul(ww)
            .wrapping_add((px(left) + px(right)).wrapping_mul(fw));
        out[x as usize] = (bulk.wrapping_add(1 << 23) >> 24) as u8;
    };
    if edge_a <= edge_b {
        for x in 0..edge_a {
            step(x, 0, x + radius, 0, x + radius + 1);
        }
        for x in edge_a..edge_b {
            step(
                x,
                x - radius - 1,
                x + radius,
                x - radius - 1,
                x + radius + 1,
            );
        }
        for x in edge_b..=lastx {
            step(x, x - radius - 1, lastx, x - radius - 1, lastx);
        }
    } else {
        for x in 0..edge_b {
            step(x, 0, x + radius, 0, x + radius + 1);
        }
        for x in edge_b..edge_a {
            step(x, 0, lastx, 0, lastx);
        }
        for x in edge_a..=lastx {
            step(x, x - radius - 1, lastx, x - radius - 1, lastx);
        }
    }
}

/// `ImagingHorizontalBoxBlur`: one pass over every row of a `w`-wide image.
fn horizontal_box_blur(img: &[u8], w: usize, float_radius: f32) -> Vec<u8> {
    let radius = float_radius as i32;
    let ww = ((1u32 << 24) as f32 / (float_radius * 2.0 + 1.0)) as u32;
    let fw = (1u32 << 24).wrapping_sub((radius as u32 * 2 + 1).wrapping_mul(ww)) / 2;
    let edge_a = (radius + 1).min(w as i32);
    let edge_b = (w as i32 - radius - 1).max(0);
    let mut out = vec![0u8; img.len()];
    for (i, o) in img.chunks_exact(w).zip(out.chunks_exact_mut(w)) {
        line_box_blur(i, o, radius, edge_a, edge_b, ww, fw);
    }
    out
}

fn transpose(img: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; img.len()];
    for y in 0..h {
        for x in 0..w {
            out[x * h + y] = img[y * w + x];
        }
    }
    out
}

/// `im.filter(ImageFilter.GaussianBlur(radius))` on "L"; radius is a Python float such as 0.6, 0.7, 1.0, 2.5, 3, 4 (keep the float/double types Pillow uses in BoxBlur.c and _imaging.c exactly, e.g. float vs double for the radius).
pub fn gaussian_blur_l(src: &[u8], w: u32, h: u32, radius: f64) -> Vec<u8> {
    const PASSES: i32 = 3;
    if radius == 0.0 || src.is_empty() {
        return src.to_vec();
    }
    let (w, h) = (w as usize, h as usize);
    let r = gaussian_box_radius(radius as f32, PASSES); // _imaging.c parses "ff"
    let mut cur = src.to_vec();
    if r != 0.0 {
        for _ in 0..PASSES {
            cur = horizontal_box_blur(&cur, w, r);
        }
        let mut t = transpose(&cur, w, h);
        for _ in 0..PASSES {
            t = horizontal_box_blur(&t, h, r);
        }
        cur = transpose(&t, h, w);
    }
    cur
}

// --------------------------------------------------------- alpha composite

/// `dst.alpha_composite(src, (x, y))` on "RGBA" images, in place: src (sw x sh) composited over dst at (x, y), x/y >= 0 and the src rectangle inside dst (Image.alpha_composite / ImagingAlphaComposite exactly).
pub fn alpha_composite(dst: &mut [u8], dw: u32, src: &[u8], sw: u32, sh: u32, x: u32, y: u32) {
    const P: u32 = 7;
    let div255 = |a: u32| ((a >> 8) + a) >> 8;
    let (dw, sw) = (dw as usize, sw as usize);
    for row in 0..sh as usize {
        let d0 = ((y as usize + row) * dw + x as usize) * 4;
        let drow = &mut dst[d0..d0 + sw * 4];
        let srow = &src[row * sw * 4..(row + 1) * sw * 4];
        for (d, s) in drow
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(srow.as_chunks::<4>().0)
        {
            let sa = u32::from(s[3]);
            if sa == 0 {
                continue;
            }
            let outa255 = sa * 255 + u32::from(d[3]) * (255 - sa);
            let coef1 = sa * 255 * 255 * (1 << P) / outa255;
            let coef2 = 255 * (1 << P) - coef1;
            for c in 0..3 {
                let t = u32::from(s[c]) * coef1 + u32::from(d[c]) * coef2;
                d[c] = (div255(t + (0x80 << P)) >> P) as u8;
            }
            d[3] = div255(outa255 + 0x80) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_images_stay_constant() {
        let l = vec![77u8; 40 * 30];
        assert!(resize_lanczos_l(&l, 40, 30, 10, 8).iter().all(|&v| v == 77));
        assert!(max_filter_l(&l, 40, 30, 5).iter().all(|&v| v == 77));
        assert!(gaussian_blur_l(&l, 40, 30, 2.5).iter().all(|&v| v == 77));
        let rgba = [200u8, 100, 50, 255].repeat(16 * 8);
        let out = resize_box_rgba(&rgba, 16, 8, 4, 2);
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [200, 100, 50, 255])
        );
    }

    #[test]
    fn max_filter_grows_single_pixel() {
        let mut img = vec![0u8; 9 * 9];
        img[4 * 9 + 4] = 200;
        let out = max_filter_l(&img, 9, 9, 5);
        for y in 0..9 {
            for x in 0..9 {
                let inside = (2..=6).contains(&x) && (2..=6).contains(&y);
                assert_eq!(out[y * 9 + x], if inside { 200 } else { 0 });
            }
        }
    }

    #[test]
    fn opaque_src_replaces_dst() {
        let mut dst = [9u8, 8, 7, 255].repeat(4 * 4);
        let src = [1u8, 2, 3, 255].repeat(2 * 2);
        alpha_composite(&mut dst, 4, &src, 2, 2, 1, 1);
        assert_eq!(dst[(4 + 1) * 4..(4 + 1) * 4 + 4], [1, 2, 3, 255]);
        assert_eq!(dst[0..4], [9, 8, 7, 255]);
    }

    #[test]
    fn same_size_resize_is_copy() {
        let img: Vec<u8> = (0..20).collect();
        assert_eq!(resize_lanczos_l(&img, 5, 4, 5, 4), img);
    }
}
