//! Bit-exact port of Pillow 11.3.0's DDS reading and DXT5 writing
//! (`DdsImagePlugin.py`, `BcnDecode.c`, `BcnEncode.c`).
//! Reads: DXT5, and uncompressed RGBA 32-bit (ARGB masks) / 16-bit (4444 masks).
//! Writes: DXT5 blocks from RGBA8 (no header).

fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn rgb565(x: u16) -> [u8; 3] {
    let (r, g, b) = (
        ((x & 0xf800) >> 8) as u8,
        ((x & 0x7e0) >> 3) as u8,
        ((x & 0x1f) << 3) as u8,
    );
    [r | r >> 5, g | g >> 6, b | b >> 5]
}

fn decode_dxt5_block(src: &[u8]) -> [[u8; 4]; 16] {
    let mut out = [[0u8; 4]; 16];
    let c0 = u16::from_le_bytes([src[8], src[9]]);
    let c1 = u16::from_le_bytes([src[10], src[11]]);
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    // BC3 colour block always acts like c0 > c1.
    let mut p = [[0u8; 4]; 4];
    p[0] = [p0[0], p0[1], p0[2], 255];
    p[1] = [p1[0], p1[1], p1[2], 255];
    for k in 0..3 {
        p[2][k] = ((2 * p0[k] as u32 + p1[k] as u32) / 3) as u8;
        p[3][k] = ((p0[k] as u32 + 2 * p1[k] as u32) / 3) as u8;
    }
    p[2][3] = 255;
    p[3][3] = 255;
    for n in 0..4 {
        for o in 0..4 {
            out[n * 4 + o] = p[(3 & (src[12 + n] >> (2 * o))) as usize];
        }
    }
    let (a0, a1) = (src[0] as u32, src[1] as u32);
    let mut a = [a0 as u8, a1 as u8, 0, 0, 0, 0, 0, 0];
    if a0 > a1 {
        for i in 1..7u32 {
            a[1 + i as usize] = (((7 - i) * a0 + i * a1) / 7) as u8;
        }
    } else {
        for i in 1..5u32 {
            a[1 + i as usize] = (((5 - i) * a0 + i * a1) / 5) as u8;
        }
        a[6] = 0;
        a[7] = 255;
    }
    let lut1 = src[2] as u32 | (src[3] as u32) << 8 | (src[4] as u32) << 16;
    let lut2 = src[5] as u32 | (src[6] as u32) << 8 | (src[7] as u32) << 16;
    for n in 0..8 {
        out[n][3] = a[(7 & (lut1 >> (3 * n))) as usize];
        out[8 + n][3] = a[(7 & (lut2 >> (3 * n))) as usize];
    }
    out
}

/// Pillow's Image.open(dds).convert("RGBA"): width, height and RGBA8 pixels (row-major, top row first) of the top mip.
pub fn decode_dds(dds: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if dds.len() < 128 || &dds[..4] != b"DDS " {
        return Err("not a DDS file".into());
    }
    if u32le(dds, 4) != 124 {
        return Err("unsupported header size".into());
    }
    let (h, w) = (u32le(dds, 12), u32le(dds, 16));
    let (pff, fourcc, bits) = (u32le(dds, 80), &dds[84..88], u32le(dds, 88));
    let masks = [
        u32le(dds, 92),
        u32le(dds, 96),
        u32le(dds, 100),
        u32le(dds, 104),
    ];
    let (wu, hu) = (w as usize, h as usize);
    if wu.checked_mul(hu).is_none_or(|n| n > 1 << 28) {
        return Err("image too large".into());
    }
    let data = &dds[128..];
    let mut px = vec![0u8; wu * hu * 4];
    if pff & 0x4 != 0 && fourcc == b"DXT5" {
        let (bw, bh) = (wu.div_ceil(4), hu.div_ceil(4));
        if data.len() < bw * bh * 16 {
            return Err("truncated DXT5 data".into());
        }
        for by in 0..bh {
            for bx in 0..bw {
                let blk = decode_dxt5_block(&data[(by * bw + bx) * 16..]);
                for (i, c) in blk.iter().enumerate() {
                    let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                    if x < wu && y < hu {
                        px[(y * wu + x) * 4..][..4].copy_from_slice(c);
                    }
                }
            }
        }
    } else if pff == 0x41
        && ((bits == 32 && masks == [0xff0000, 0xff00, 0xff, 0xff000000])
            || (bits == 16 && masks == [0xf00, 0xf0, 0xf, 0xf000]))
    {
        let bc = (bits / 8) as usize;
        if data.len() < wu * hu * bc {
            return Err("truncated pixel data".into());
        }
        let shift: Vec<u32> = masks.iter().map(|m| m.trailing_zeros()).collect();
        for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let mut v = 0u32;
            for (k, b) in data[i * bc..][..bc].iter().enumerate() {
                v |= (*b as u32) << (8 * k);
            }
            for c in 0..4 {
                let total = (masks[c] >> shift[c]) as f64;
                p[c] = ((((v & masks[c]) >> shift[c]) as f64 / total) * 255.0) as u8;
            }
        }
    } else {
        return Err("unsupported DDS pixel format".into());
    }
    Ok((w, h, px))
}

fn enc565(p: &[u8]) -> u16 {
    ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3)
}

/// Pillow's `im.save(b, "DDS", pixel_format="DXT5"); b.getvalue()[128:]` for an RGBA image: the DXT5 blocks only.
pub fn encode_dxt5(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = Vec::with_capacity(w.div_ceil(4) * h.div_ceil(4) * 16);
    for by in (0..h).step_by(4) {
        for bx in (0..w).step_by(4) {
            // Pixels outside the image are zero and excluded from min/max.
            let mut blk = [[0u8; 4]; 16];
            let mut ins = [false; 16];
            for j in 0..4 {
                for i in 0..4 {
                    if bx + i < w && by + j < h {
                        blk[j * 4 + i].copy_from_slice(&rgba[((by + j) * w + bx + i) * 4..][..4]);
                        ins[j * 4 + i] = true;
                    }
                }
            }
            // Alpha block.
            let al = || (0..16).filter(|&k| ins[k]).map(|k| blk[k][3]);
            let (amin, amax) = (al().min().unwrap(), al().max().unwrap());
            out.extend([amin, amax]);
            let denom = (amax - amin) as f32;
            for half in 0..2 {
                let mut l = 0u32;
                for j in 0..8 {
                    let a = blk[half * 8 + j][3];
                    let idx = if a == 0 {
                        6
                    } else if a == 255 {
                        7
                    } else {
                        let d = if denom == 0.0 {
                            0.0
                        } else {
                            (a as i32 - amin as i32).abs() as f32 / denom * 10.0
                        };
                        if d < 3.0 {
                            2
                        } else if d < 5.0 {
                            3
                        } else if d < 7.0 {
                            4
                        } else {
                            5
                        }
                    };
                    l |= idx << (j * 3);
                }
                out.extend(&l.to_le_bytes()[..3]);
            }
            // Colour block.
            let cs = || (0..16).filter(|&k| ins[k]).map(|k| enc565(&blk[k]));
            let (cmin, cmax) = (cs().min().unwrap(), cs().max().unwrap());
            out.extend(cmax.to_le_bytes());
            out.extend(cmin.to_le_bytes());
            let (rmin, rmax) = (rgb565(cmin), rgb565(cmax));
            for i in 0..4 {
                let mut l = 0u8;
                for j in 0..4 {
                    let c = &blk[i * 4 + j];
                    let (mut dist, mut total) = (0f32, 0i32);
                    for k in 0..3 {
                        let denom = (rmax[k] as i32 - rmin[k] as i32).abs() as f32;
                        if denom != 0.0 {
                            dist += (c[k] as i32 - rmin[k] as i32).abs() as f32 / denom;
                            total += 1;
                        }
                    }
                    if total == 0 {
                        continue;
                    }
                    dist *= (6 / total) as f32;
                    if dist < 1.0 {
                        l |= 1 << (j * 2);
                    } else if dist < 3.0 {
                        l |= 3 << (j * 2);
                    } else if dist < 5.0 {
                        l |= 2 << (j * 2);
                    }
                }
                out.push(l);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dds(w: u32, h: u32, fourcc: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[..4].copy_from_slice(b"DDS ");
        d[4] = 124;
        d[12..16].copy_from_slice(&h.to_le_bytes());
        d[16..20].copy_from_slice(&w.to_le_bytes());
        d[76] = 32;
        d[80] = 4;
        d[84..88].copy_from_slice(fourcc);
        d.extend(body);
        d
    }

    #[test]
    fn solid_round_trip() {
        let img: Vec<u8> = (0..3 * 5).flat_map(|_| [255, 255, 255, 255]).collect();
        let enc = encode_dxt5(&img, 3, 5);
        assert_eq!(enc.len(), 2 * 16);
        let (w, h, px) = decode_dds(&dds(3, 5, b"DXT5", &enc)).unwrap();
        assert_eq!((w, h), (3, 5));
        assert_eq!(px, img);
    }

    #[test]
    fn known_block() {
        // a0=255 > a1=0 and index 0 everywhere; c0=0xf800 (red) c1=0; all colour index 0.
        let blk = [255, 0, 0, 0, 0, 0, 0, 0, 0x00, 0xf8, 0, 0, 0, 0, 0, 0];
        let (_, _, px) = decode_dds(&dds(4, 4, b"DXT5", &blk)).unwrap();
        assert!(px.chunks(4).all(|p| p == [255, 0, 0, 255]));
    }
}
