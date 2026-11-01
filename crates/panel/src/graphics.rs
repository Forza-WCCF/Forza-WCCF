//! SETTINGS > TEXTURE FILTERING (2017-18): GAME / 4x / 8x / 16x anisotropic filtering, ported from the 2010-11 panel's
//! GRAPHICS page (`wccfpanel.c` hook_sampler, aniso_tick). The game device's SetSamplerState (vtable slot 69: every
//! call goes through the vtable) is replaced on the first frame: the game's MINFILTER and MAXANISOTROPY are kept per
//! pixel sampler, and with a level chosen a LINEAR minify filter goes out as ANISOTROPIC with the maximum anisotropy
//! raised to the level (capped at the card's). GAME sends the game's values as they are. A new choice sends the
//! game's last values again through it, so it applies at once. The panel's own drawing (inside `hook_present`) passes
//! straight through and is not kept.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed};
use windows::Win32::Graphics::Direct3D9::{
    D3DCAPS9, D3DPTFILTERCAPS_MINFANISOTROPIC, D3DSAMP_MAXANISOTROPY, D3DSAMP_MINFILTER,
    D3DTEXF_ANISOTROPIC, D3DTEXF_LINEAR, D3DTEXF_POINT, IDirect3DDevice9,
};
use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
use windows::core::{HRESULT, Interface};

type SetSamplerFn = unsafe extern "system" fn(*mut c_void, u32, u32, u32) -> HRESULT;
const SLOT: usize = 69;
const MINF: u32 = D3DSAMP_MINFILTER.0 as u32;
const MAXA: u32 = D3DSAMP_MAXANISOTROPY.0 as u32;

/// The choice (0, 4, 8, 16), set by the panel from panel.txt.
pub static WANT: AtomicU32 = AtomicU32::new(0);
static ORIG: AtomicUsize = AtomicUsize::new(0);
/// The card has none, or the slot could not be written: not tried again on this device.
static FAILED: AtomicBool = AtomicBool::new(false);
/// What this card can do (D3DCAPS9.MaxAnisotropy).
static CARD_MAX: AtomicU32 = AtomicU32::new(0);
/// The choice the samplers have now.
static SENT: AtomicU32 = AtomicU32::new(0);
static DEVICE: AtomicUsize = AtomicUsize::new(0);
/// The game's own MINFILTER / MAXANISOTROPY per pixel sampler once seen (a pure device cannot be asked), and the
/// MAXANISOTROPY each sampler has now.
static GAME_MINF: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];
static GAME_MAXA: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];
static SEEN_MINF: AtomicU32 = AtomicU32::new(0);
static SEEN_MAXA: AtomicU32 = AtomicU32::new(0);
static SENT_MAXA: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];

/// The filter and maximum anisotropy one sampler gets at `level` (0: the game's own) from the game's values (`None`:
/// not seen yet, the device defaults POINT and 1).
fn translate(level: u32, minf: Option<u32>, maxa: Option<u32>) -> (u32, u32) {
    let mut f = minf.unwrap_or(D3DTEXF_POINT.0 as u32);
    let gm = maxa.unwrap_or(1);
    if level != 0 && f == D3DTEXF_LINEAR.0 as u32 {
        f = D3DTEXF_ANISOTROPIC.0 as u32;
    }
    let m = if level != 0 && f == D3DTEXF_ANISOTROPIC.0 as u32 && gm < level {
        level
    } else {
        gm
    };
    (f, m)
}

fn orig() -> SetSamplerFn {
    // SAFETY: ORIG holds d3d9's SetSamplerState, read from the vtable before it was replaced
    unsafe { std::mem::transmute::<usize, SetSamplerFn>(ORIG.load(Relaxed)) }
}

/// The game's value for one state: kept, then sent through the choice (also the replay of a new choice).
unsafe fn sampler_game(dev: *mut c_void, smp: u32, t: u32, v: u32) -> HRESULT {
    if smp >= 16 || (t != MINF && t != MAXA) {
        // SAFETY: the original with the game's own arguments
        return unsafe { orig()(dev, smp, t, v) };
    }
    let i = smp as usize;
    let bit = 1u32 << smp;
    if t == MINF {
        GAME_MINF[i].store(v, Relaxed);
        SEEN_MINF.fetch_or(bit, Relaxed);
    } else {
        GAME_MAXA[i].store(v, Relaxed);
        SEEN_MAXA.fetch_or(bit, Relaxed);
    }
    let seen = |set: &AtomicU32, val: &AtomicU32| {
        (set.load(Relaxed) & bit != 0).then(|| val.load(Relaxed))
    };
    let level = WANT.load(Relaxed).min(CARD_MAX.load(Relaxed));
    let (f, m) = translate(
        level,
        seen(&SEEN_MINF, &GAME_MINF[i]),
        seen(&SEEN_MAXA, &GAME_MAXA[i]),
    );
    if SENT_MAXA[i].swap(m, Relaxed) != m {
        // SAFETY: as above
        let _ = unsafe { orig()(dev, smp, MAXA, m) };
    }
    if t == MINF {
        // SAFETY: as above
        unsafe { orig()(dev, smp, t, f) }
    } else {
        HRESULT(0)
    }
}

unsafe extern "system" fn hook_sampler(dev: *mut c_void, smp: u32, t: u32, v: u32) -> HRESULT {
    if crate::IN_PRESENT.get() {
        // SAFETY: the panel's own drawing, sent as it is
        return unsafe { orig()(dev, smp, t, v) };
    }
    // SAFETY: the game's call, its arguments passed on
    unsafe { sampler_game(dev, smp, t, v) }
}

/// Every frame on the render thread (`hook_present`, before the panel's state block): the hook put in on the first
/// frame, a new choice sent through the game's last filters.
pub fn tick(dev: &IDirect3DDevice9, log: fn(&str)) {
    let raw = dev.as_raw();
    // SAFETY: a COM object's first field is its vtable pointer
    let vt = unsafe { *(raw as *const *mut usize) };
    // another device: its samplers are at their defaults, so nothing kept is true of it
    if DEVICE.swap(raw as usize, Relaxed) != raw as usize && ORIG.load(Relaxed) != 0 {
        SENT_MAXA.iter().for_each(|a| a.store(0, Relaxed));
        SEEN_MINF.store(0, Relaxed);
        SEEN_MAXA.store(0, Relaxed);
        SENT.store(0, Relaxed);
        // SAFETY: the vtable's slot, readable while the device lives
        if unsafe { *vt.add(SLOT) } != hook_sampler as *const () as usize {
            ORIG.store(0, Relaxed);
            FAILED.store(false, Relaxed);
        }
        log("textures: another device - the samplers' states start again");
    }
    if ORIG.load(Relaxed) == 0 {
        if FAILED.swap(true, Relaxed) {
            return;
        }
        let mut caps = D3DCAPS9::default();
        // SAFETY: a device query into our own struct
        let max = if unsafe { dev.GetDeviceCaps(&mut caps) }.is_ok()
            && caps.TextureFilterCaps & D3DPTFILTERCAPS_MINFANISOTROPIC as u32 != 0
        {
            caps.MaxAnisotropy
        } else {
            0
        };
        CARD_MAX.store(max, Relaxed);
        if max < 2 {
            log("textures: this graphics card has no anisotropic filtering - the game's own");
            return;
        }
        // SAFETY: one pointer-sized vtable slot made writable, swapped, its protection put back; the game calls it
        // on this same thread, so no call is between the read and the write
        unsafe {
            let slot = vt.add(SLOT);
            let mut old = PAGE_PROTECTION_FLAGS(0);
            if VirtualProtect(
                slot as *const c_void,
                size_of::<usize>(),
                PAGE_READWRITE,
                &mut old,
            )
            .is_err()
            {
                log("textures: the vtable is not writable - the game's own");
                return;
            }
            ORIG.store(*slot, Relaxed);
            *slot = hook_sampler as *const () as usize;
            let _ = VirtualProtect(slot as *const c_void, size_of::<usize>(), old, &mut old);
        }
        FAILED.store(false, Relaxed);
        log(&format!(
            "textures: SetSamplerState replaced (card max {max}x)"
        ));
    }
    let want = WANT.load(Relaxed);
    if SENT.swap(want, Relaxed) == want {
        return;
    }
    let seen = SEEN_MINF.load(Relaxed);
    for i in 0..16u32 {
        if seen & (1 << i) != 0 {
            // SAFETY: the game's last filter for this sampler, sent through the new choice
            let _ = unsafe { sampler_game(raw, i, MINF, GAME_MINF[i as usize].load(Relaxed)) };
        }
    }
    log(&format!(
        "textures: {}",
        if want != 0 {
            format!("{want}x anisotropic (card max {}x)", CARD_MAX.load(Relaxed))
        } else {
            "the game's own filters".into()
        }
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_becomes_anisotropic_at_the_level() {
        let (point, linear, aniso) = (1, 2, 3);
        assert_eq!(translate(0, Some(linear), Some(1)), (linear, 1)); // GAME: as it is
        assert_eq!(translate(8, Some(linear), Some(1)), (aniso, 8));
        assert_eq!(translate(8, Some(point), Some(1)), (point, 1)); // POINT stays POINT
        assert_eq!(translate(4, Some(aniso), Some(16)), (aniso, 16)); // the game's higher level kept
        assert_eq!(translate(16, None, None), (point, 1)); // not seen: the device defaults
    }
}
