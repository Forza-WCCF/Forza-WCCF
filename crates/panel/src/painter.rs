//! egui on Direct3D 9: the tessellated meshes into one dynamic vertex and index buffer per frame, drawn as pre-transformed
//! (XYZRHW) triangles, one scissor rectangle and one texture per mesh. Colours are egui's premultiplied sRGB, blended
//! as egui expects (`ONE, INV_SRC_ALPHA`, no sRGB conversion). Textures are MANAGED (they survive a device reset); the
//! two buffers are DEFAULT pool and go in [`Painter::reset`]. The caller saves and restores the game's device state.

use egui::epaint::textures::TexturesDelta;
use egui::epaint::{ClippedPrimitive, ImageDelta, Primitive};
use egui::{TextureFilter, TextureId};
use std::collections::HashMap;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D9::*;
use windows::core::Result;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 4], // x, y in pixels, z, rhw
    colour: u32,   // D3DCOLOR, 0xAARRGGBB
    uv: [f32; 2],
}

const FVF: u32 = D3DFVF_XYZRHW | D3DFVF_DIFFUSE | D3DFVF_TEX1;

#[derive(Default)]
pub struct Painter {
    textures: HashMap<TextureId, (IDirect3DTexture9, bool)>, // the texture, linear filtering
    vb: Option<(IDirect3DVertexBuffer9, usize)>,             // the buffer, its capacity in vertices
    ib: Option<(IDirect3DIndexBuffer9, usize)>,              // in indices
}

impl Painter {
    /// A texture of our own (the game's frame) under an egui id.
    pub fn set_user(&mut self, id: TextureId, tex: IDirect3DTexture9) {
        self.textures.insert(id, (tex, true));
    }

    /// Before a device reset: the DEFAULT pool buffers and our own textures go (remade on the next frame).
    pub fn reset(&mut self) {
        self.vb = None;
        self.ib = None;
        self.textures
            .retain(|id, _| matches!(id, TextureId::Managed(_)));
    }

    /// Draw one egui frame into the bound render target of `size` pixels.
    ///
    /// # Safety
    /// `dev` is the device the painter's textures were made on, on the thread that renders with it.
    pub unsafe fn paint(
        &mut self,
        dev: &IDirect3DDevice9,
        size: [u32; 2],
        ppp: f32,
        prims: &[ClippedPrimitive],
        delta: &TexturesDelta,
    ) -> Result<()> {
        for (id, d) in delta
            .set
            .iter()
            .flat_map(|(id, ds)| ds.iter().map(move |d| (id, d)))
        {
            // SAFETY: same device and thread, as the caller promises
            let tex = unsafe { upload(dev, self.textures.get(id).map(|t| &t.0), d)? };
            let linear = d.options.magnification == TextureFilter::Linear;
            self.textures.insert(*id, (tex, linear));
        }
        let meshes: Vec<_> = prims
            .iter()
            .filter_map(|p| match &p.primitive {
                Primitive::Mesh(m) if !m.indices.is_empty() => Some((p.clip_rect, m)),
                _ => None, // paint callbacks: none in the panel
            })
            .collect();
        let nv: usize = meshes.iter().map(|(_, m)| m.vertices.len()).sum();
        let ni: usize = meshes.iter().map(|(_, m)| m.indices.len()).sum();
        if ni > 0 {
            // SAFETY: as above
            unsafe { self.draw(dev, size, ppp, &meshes, nv, ni)? };
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
        Ok(())
    }

    unsafe fn draw(
        &mut self,
        dev: &IDirect3DDevice9,
        size: [u32; 2],
        ppp: f32,
        meshes: &[(egui::Rect, &egui::Mesh)],
        nv: usize,
        ni: usize,
    ) -> Result<()> {
        // SAFETY: the device calls take valid pointers and out-parameters; each lock is written within its length and
        // unlocked before the draw
        unsafe {
            if self.vb.as_ref().is_none_or(|b| b.1 < nv) {
                let cap = nv.next_power_of_two().max(4096);
                let mut vb = None;
                dev.CreateVertexBuffer(
                    (cap * size_of::<Vertex>()) as u32,
                    (D3DUSAGE_DYNAMIC | D3DUSAGE_WRITEONLY) as u32,
                    FVF,
                    D3DPOOL_DEFAULT,
                    &mut vb,
                    std::ptr::null_mut(),
                )?;
                self.vb = vb.map(|b| (b, cap));
            }
            if self.ib.as_ref().is_none_or(|b| b.1 < ni) {
                let cap = ni.next_power_of_two().max(8192);
                let mut ib = None;
                dev.CreateIndexBuffer(
                    (cap * 4) as u32,
                    (D3DUSAGE_DYNAMIC | D3DUSAGE_WRITEONLY) as u32,
                    D3DFMT_INDEX32,
                    D3DPOOL_DEFAULT,
                    &mut ib,
                    std::ptr::null_mut(),
                )?;
                self.ib = ib.map(|b| (b, cap));
            }
            let (Some((vb, _)), Some((ib, _))) = (&self.vb, &self.ib) else {
                return Ok(());
            };
            let mut p = std::ptr::null_mut();
            vb.Lock(
                0,
                (nv * size_of::<Vertex>()) as u32,
                &mut p,
                D3DLOCK_DISCARD as u32,
            )?;
            let vs = std::slice::from_raw_parts_mut(p.cast::<Vertex>(), nv);
            let mut q = std::ptr::null_mut();
            if let Err(e) = ib.Lock(0, (ni * 4) as u32, &mut q, D3DLOCK_DISCARD as u32) {
                vb.Unlock()?;
                return Err(e);
            }
            let is = std::slice::from_raw_parts_mut(q.cast::<u32>(), ni);
            let (mut v, mut i) = (0, 0);
            for (_, m) in meshes {
                for (dst, src) in vs[v..].iter_mut().zip(&m.vertices) {
                    let [r, g, b, a] = src.color.to_array();
                    *dst = Vertex {
                        // D3D9 puts pixel centres on whole numbers, egui between them
                        pos: [src.pos.x * ppp - 0.5, src.pos.y * ppp - 0.5, 0.0, 1.0],
                        colour: u32::from_be_bytes([a, r, g, b]),
                        uv: [src.uv.x, src.uv.y],
                    };
                }
                is[i..i + m.indices.len()].copy_from_slice(&m.indices);
                v += m.vertices.len();
                i += m.indices.len();
            }
            vb.Unlock()?;
            ib.Unlock()?;

            states(dev)?;
            dev.SetFVF(FVF)?;
            dev.SetStreamSource(0, vb, 0, size_of::<Vertex>() as u32)?;
            dev.SetIndices(ib)?;
            let (mut v, mut i) = (0, 0);
            for (clip, m) in meshes {
                let (nv, ni) = (m.vertices.len(), m.indices.len());
                let r = RECT {
                    left: (clip.min.x * ppp).floor().clamp(0.0, size[0] as f32) as i32,
                    top: (clip.min.y * ppp).floor().clamp(0.0, size[1] as f32) as i32,
                    right: (clip.max.x * ppp).ceil().clamp(0.0, size[0] as f32) as i32,
                    bottom: (clip.max.y * ppp).ceil().clamp(0.0, size[1] as f32) as i32,
                };
                if let Some((tex, linear)) = self.textures.get(&m.texture_id)
                    && r.right > r.left
                    && r.bottom > r.top
                {
                    let f = if *linear {
                        D3DTEXF_LINEAR
                    } else {
                        D3DTEXF_POINT
                    };
                    dev.SetSamplerState(0, D3DSAMP_MAGFILTER, f.0 as u32)?;
                    dev.SetSamplerState(0, D3DSAMP_MINFILTER, f.0 as u32)?;
                    dev.SetTexture(0, tex)?;
                    dev.SetScissorRect(&r)?;
                    dev.DrawIndexedPrimitive(
                        D3DPT_TRIANGLELIST,
                        v as i32,
                        0,
                        nv as u32,
                        i as u32,
                        (ni / 3) as u32,
                    )?;
                }
                v += nv;
                i += ni;
            }
            dev.SetTexture(0, None)?; // the game's own texture comes back with its state block; ours is not held
        }
        Ok(())
    }
}

/// Every state the drawing depends on, set: nothing is left to whatever the game set.
unsafe fn states(dev: &IDirect3DDevice9) -> Result<()> {
    let rs = [
        (D3DRS_ZENABLE, 0),
        (D3DRS_ZWRITEENABLE, 0),
        (D3DRS_ALPHATESTENABLE, 0),
        (D3DRS_STENCILENABLE, 0),
        (D3DRS_FOGENABLE, 0),
        (D3DRS_LIGHTING, 0),
        (D3DRS_SPECULARENABLE, 0),
        (D3DRS_CULLMODE, D3DCULL_NONE.0 as u32),
        (D3DRS_FILLMODE, D3DFILL_SOLID.0 as u32),
        (D3DRS_SHADEMODE, D3DSHADE_GOURAUD.0 as u32),
        (D3DRS_CLIPPING, 1),
        (D3DRS_SCISSORTESTENABLE, 1),
        (D3DRS_ALPHABLENDENABLE, 1),
        (D3DRS_SEPARATEALPHABLENDENABLE, 0),
        (D3DRS_BLENDOP, D3DBLENDOP_ADD.0 as u32),
        (D3DRS_SRCBLEND, D3DBLEND_ONE.0 as u32),
        (D3DRS_DESTBLEND, D3DBLEND_INVSRCALPHA.0 as u32),
        (D3DRS_SRGBWRITEENABLE, 0),
        (D3DRS_COLORWRITEENABLE, 0xF),
        (D3DRS_MULTISAMPLEANTIALIAS, 0),
    ];
    let ts = [
        (0, D3DTSS_COLOROP, D3DTOP_MODULATE.0 as u32),
        (0, D3DTSS_COLORARG1, D3DTA_TEXTURE),
        (0, D3DTSS_COLORARG2, D3DTA_DIFFUSE),
        (0, D3DTSS_ALPHAOP, D3DTOP_MODULATE.0 as u32),
        (0, D3DTSS_ALPHAARG1, D3DTA_TEXTURE),
        (0, D3DTSS_ALPHAARG2, D3DTA_DIFFUSE),
        (0, D3DTSS_TEXCOORDINDEX, 0),
        (0, D3DTSS_TEXTURETRANSFORMFLAGS, D3DTTFF_DISABLE.0 as u32),
        (1, D3DTSS_COLOROP, D3DTOP_DISABLE.0 as u32),
        (1, D3DTSS_ALPHAOP, D3DTOP_DISABLE.0 as u32),
    ];
    let ss = [
        (D3DSAMP_ADDRESSU, D3DTADDRESS_CLAMP.0 as u32),
        (D3DSAMP_ADDRESSV, D3DTADDRESS_CLAMP.0 as u32),
        (D3DSAMP_MIPFILTER, D3DTEXF_NONE.0 as u32),
        (D3DSAMP_SRGBTEXTURE, 0),
    ];
    // SAFETY: plain state calls on a live device
    unsafe {
        for (s, v) in rs {
            dev.SetRenderState(s, v)?;
        }
        for (st, s, v) in ts {
            dev.SetTextureStageState(st, s, v)?;
        }
        for (s, v) in ss {
            dev.SetSamplerState(0, s, v)?;
        }
        dev.SetVertexShader(None)?;
        dev.SetPixelShader(None)?;
    }
    Ok(())
}

/// A whole texture made, or a patch of an existing one written (egui's font atlas grows by patches).
unsafe fn upload(
    dev: &IDirect3DDevice9,
    old: Option<&IDirect3DTexture9>,
    d: &ImageDelta,
) -> Result<IDirect3DTexture9> {
    let egui::ImageData::Color(img) = &d.image;
    let [w, h] = img.size;
    // SAFETY: the texture is made by the device; the locked rectangle is `w`×`h` 32-bit texels at `pitch` bytes a row
    unsafe {
        let tex = match (d.pos, old) {
            (Some(_), Some(t)) => t.clone(),
            _ => {
                let mut t = None;
                dev.CreateTexture(
                    w as u32,
                    h as u32,
                    1,
                    0,
                    D3DFMT_A8R8G8B8,
                    D3DPOOL_MANAGED,
                    &mut t,
                    std::ptr::null_mut(),
                )?;
                t.ok_or_else(windows::core::Error::empty)?
            }
        };
        let [x, y] = d.pos.unwrap_or([0, 0]);
        let r = RECT {
            left: x as i32,
            top: y as i32,
            right: (x + w) as i32,
            bottom: (y + h) as i32,
        };
        let mut lr = D3DLOCKED_RECT::default();
        tex.LockRect(0, &mut lr, &r, 0)?;
        for (row, px) in img.pixels.chunks_exact(w).enumerate() {
            let dst = std::slice::from_raw_parts_mut(
                lr.pBits.cast::<u8>().add(row * lr.Pitch as usize),
                w * 4,
            );
            for (o, c) in dst.as_chunks_mut::<4>().0.iter_mut().zip(px) {
                let [r, g, b, a] = c.to_array();
                o.copy_from_slice(&[b, g, r, a]); // A8R8G8B8 in memory: B, G, R, A
            }
        }
        tex.UnlockRect(0)?;
        Ok(tex)
    }
}
