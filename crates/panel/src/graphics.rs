//! SETTINGS > TEXTURE FILTERING (2017-18): GAME / 4x / 8x / 16x anisotropic filtering, ported from the 2010-11 panel's
//! GRAPHICS page (`wccfpanel.c` hook_sampler, aniso_tick). The game device's SetSamplerState (vtable slot 69: every
//! call goes through the vtable) is replaced on the first frame: the game's MINFILTER and MAXANISOTROPY are kept per
//! pixel sampler, and with a level chosen a LINEAR minify filter goes out as ANISOTROPIC with the maximum anisotropy
//! raised to the level (capped at the card's). GAME sends the game's values as they are. A new choice sends the
//! game's last values again through it, so it applies at once. The panel's own drawing (inside `hook_present`) passes
//! straight through and is not kept.

use std::ffi::c_void;
use std::sync::atomic::{
    AtomicBool, AtomicU32, AtomicUsize,
    Ordering::{Acquire, Relaxed, Release},
};
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
/// Each vtable replaced and the SetSamplerState it had, filled in order and never cleared: a replaced slot is never put
/// back, so an old device's calls keep reaching their own original after another device comes.
static VTABLES: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
static ORIGS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
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

/// The original SetSamplerState of the vtable `vt`, `None` if that vtable was never replaced.
fn orig_of(vt: usize) -> Option<SetSamplerFn> {
    let (_, orig) = VTABLES
        .iter()
        .zip(&ORIGS)
        .find(|(v, _)| v.load(Acquire) == vt)?;
    let f = orig.load(Relaxed);
    // SAFETY: a nonzero entry is the slot's value read before it was replaced: d3d9's SetSamplerState for that vtable
    (f != 0).then(|| unsafe { std::mem::transmute::<usize, SetSamplerFn>(f) })
}

/// The device's vtable.
fn vtable(dev: *mut c_void) -> usize {
    // SAFETY: a COM object's first field is its vtable pointer
    unsafe { *(dev as *const usize) }
}

fn level() -> u32 {
    WANT.load(Relaxed).min(CARD_MAX.load(Relaxed))
}

/// The game's value for one state: kept, then sent through the choice; GAME passes the call on as it is.
unsafe fn sampler_game(orig: SetSamplerFn, dev: *mut c_void, smp: u32, t: u32, v: u32) -> HRESULT {
    if smp >= 16 || (t != MINF && t != MAXA) {
        // SAFETY: the original with the game's own arguments
        return unsafe { orig(dev, smp, t, v) };
    }
    let i = smp as usize;
    let (Some(game_minf), Some(game_maxa), Some(sent_maxa)) =
        (GAME_MINF.get(i), GAME_MAXA.get(i), SENT_MAXA.get(i))
    else {
        // SAFETY: as above (smp < 16, so not reached)
        return unsafe { orig(dev, smp, t, v) };
    };
    let bit = 1u32 << smp;
    if t == MINF {
        game_minf.store(v, Relaxed);
        SEEN_MINF.fetch_or(bit, Relaxed);
    } else {
        game_maxa.store(v, Relaxed);
        SEEN_MAXA.fetch_or(bit, Relaxed);
    }
    if level() == 0 {
        if t == MAXA {
            sent_maxa.store(v, Relaxed);
        }
        // SAFETY: as above
        return unsafe { orig(dev, smp, t, v) };
    }
    // SAFETY: as above
    unsafe { apply(orig, dev, smp, t == MINF) }
}

/// One sampler set from the game's last values through the choice: its MAXANISOTROPY when that changes, and its
/// MINFILTER when `minf` (the game's call, or the replay of a new choice).
unsafe fn apply(orig: SetSamplerFn, dev: *mut c_void, smp: u32, minf: bool) -> HRESULT {
    let i = smp as usize;
    let (Some(game_minf), Some(game_maxa), Some(sent_maxa)) =
        (GAME_MINF.get(i), GAME_MAXA.get(i), SENT_MAXA.get(i))
    else {
        return HRESULT(0); // smp < 16 from both callers: not reached
    };
    let bit = 1u32 << smp;
    let seen = |set: &AtomicU32, val: &AtomicU32| {
        (set.load(Relaxed) & bit != 0).then(|| val.load(Relaxed))
    };
    let (f, m) = translate(
        level(),
        seen(&SEEN_MINF, game_minf),
        seen(&SEEN_MAXA, game_maxa),
    );
    if sent_maxa.swap(m, Relaxed) != m {
        // SAFETY: the original, on the device it was called for
        let _ = unsafe { orig(dev, smp, MAXA, m) };
    }
    if minf {
        // SAFETY: as above
        unsafe { orig(dev, smp, MINF, f) }
    } else {
        HRESULT(0)
    }
}

unsafe extern "system" fn hook_sampler(dev: *mut c_void, smp: u32, t: u32, v: u32) -> HRESULT {
    // only vtables in VTABLES ever point here; a miss would be a bug, and dropping one state beats calling nothing
    let Some(orig) = orig_of(vtable(dev)) else {
        return HRESULT(0);
    };
    if crate::IN_PRESENT.get() {
        // SAFETY: the panel's own drawing, sent as it is
        return unsafe { orig(dev, smp, t, v) };
    }
    // SAFETY: the game's call, its arguments passed on
    unsafe { sampler_game(orig, dev, smp, t, v) }
}

/// Nothing kept is true any more (a Reset or another device: every sampler at its default): nothing is replayed until
/// the game sends its states again.
fn forget() {
    SENT_MAXA.iter().for_each(|a| a.store(0, Relaxed));
    SEEN_MINF.store(0, Relaxed);
    SEEN_MAXA.store(0, Relaxed);
}

/// A device Reset puts every sampler state back at its default.
pub fn reset() {
    forget();
}

/// The vtable's SetSamplerState slot replaced by [`hook_sampler`], its original kept in [`VTABLES`] / [`ORIGS`]; false
/// when the table is full or the slot is not writable.
///
/// # Safety
/// `vt` points at a live vtable of at least `SLOT + 1` pointers, called only on this thread.
unsafe fn install(vt: *mut usize) -> bool {
    let Some((vt_kept, orig_kept)) = VTABLES
        .iter()
        .zip(&ORIGS)
        .find(|(v, _)| v.load(Relaxed) == 0)
    else {
        return false;
    };
    // SAFETY: one pointer-sized vtable slot made writable, swapped, its protection put back; the game calls it on this
    // same thread, so no call is between the read and the write
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
            return false;
        }
        orig_kept.store(*slot, Relaxed);
        vt_kept.store(vt as usize, Release);
        *slot = hook_sampler as *const () as usize;
        let _ = VirtualProtect(slot as *const c_void, size_of::<usize>(), old, &mut old);
    }
    true
}

/// Every frame on the render thread (`hook_present`, before the panel's state block): the hook put in on the first
/// frame, a new choice sent through the game's last filters.
pub fn tick(dev: &IDirect3DDevice9, log: fn(&str)) {
    let raw = dev.as_raw();
    let vt = vtable(raw) as *mut usize;
    // another device: its samplers are at their defaults, so nothing kept is true of it
    let first = DEVICE.load(Relaxed) == 0;
    if DEVICE.swap(raw as usize, Relaxed) != raw as usize {
        forget();
        SENT.store(0, Relaxed);
        FAILED.store(false, Relaxed);
        if !first {
            log("textures: another device - the samplers' states start again");
        }
    }
    // SAFETY: the vtable's slot, readable while the device lives
    if unsafe { *vt.add(SLOT) } != hook_sampler as *const () as usize {
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
        // SAFETY: the live device's vtable, on its render thread
        if !unsafe { install(vt) } {
            log("textures: the vtable could not be replaced - the game's own");
            return;
        }
        FAILED.store(false, Relaxed);
        log(&format!(
            "textures: SetSamplerState replaced (card max {max}x)"
        ));
    }
    let Some(orig) = orig_of(vt as usize) else {
        return;
    };
    let want = WANT.load(Relaxed);
    if SENT.swap(want, Relaxed) == want {
        return;
    }
    // SAFETY: the game's last states for this device, sent through the new choice
    unsafe { replay(orig, raw) };
    log(&format!(
        "textures: {}",
        if want != 0 {
            format!("{want}x anisotropic (card max {}x)", CARD_MAX.load(Relaxed))
        } else {
            "the game's own filters".into()
        }
    ));
}

/// A new choice: every sampler the game has set, sent again through it.
unsafe fn replay(orig: SetSamplerFn, dev: *mut c_void) {
    let (minf, maxa) = (SEEN_MINF.load(Relaxed), SEEN_MAXA.load(Relaxed));
    for i in (0..16u32).filter(|i| (minf | maxa) & (1 << i) != 0) {
        // SAFETY: the original, on the device the states were kept for
        let _ = unsafe { apply(orig, dev, i, minf & (1 << i) != 0) };
    }
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

    static CALLS: std::sync::Mutex<Vec<(u8, u32, u32, u32)>> = std::sync::Mutex::new(Vec::new());
    unsafe extern "system" fn orig_a(_: *mut c_void, smp: u32, t: u32, v: u32) -> HRESULT {
        CALLS.lock().unwrap().push((b'a', smp, t, v));
        HRESULT(0)
    }
    unsafe extern "system" fn orig_b(_: *mut c_void, smp: u32, t: u32, v: u32) -> HRESULT {
        CALLS.lock().unwrap().push((b'b', smp, t, v));
        HRESULT(0)
    }
    fn calls() -> Vec<(u8, u32, u32, u32)> {
        std::mem::take(&mut *CALLS.lock().unwrap())
    }
    /// A vtable with `orig` in the SetSamplerState slot, and a device object pointing at it.
    fn fake(orig: SetSamplerFn) -> (Box<[usize; SLOT + 1]>, Box<usize>) {
        let mut vt = Box::new([0usize; SLOT + 1]);
        vt[SLOT] = orig as *const () as usize;
        let dev = Box::new(vt.as_ptr() as usize);
        (vt, dev)
    }
    fn raw(dev: &mut usize) -> *mut c_void {
        dev as *mut usize as *mut c_void
    }

    /// One test: the hook's state is global.
    #[test]
    fn hook_paths() {
        let (lin, aniso, addru) = (D3DTEXF_LINEAR.0 as u32, D3DTEXF_ANISOTROPIC.0 as u32, 1);
        let (mut vt_a, mut dev_a) = fake(orig_a);
        // SAFETY: our own vtable, on this thread
        assert!(unsafe { install(vt_a.as_mut_ptr()) });
        assert_eq!(vt_a[SLOT], hook_sampler as *const () as usize);
        let a = raw(&mut dev_a);
        CARD_MAX.store(16, Relaxed);
        // SAFETY: (all calls below) fake devices whose vtables hold our recording originals
        unsafe {
            // GAME: the game's calls go on as they are, no MAXANISOTROPY of our own
            WANT.store(0, Relaxed);
            let _ = hook_sampler(a, 0, MAXA, 16);
            let _ = hook_sampler(a, 0, MINF, lin);
            let _ = hook_sampler(a, 1, MINF, lin);
            assert_eq!(
                calls(),
                [
                    (b'a', 0, MAXA, 16),
                    (b'a', 0, MINF, lin),
                    (b'a', 1, MINF, lin)
                ]
            );
            // 8x: the game's last states sent again through it (the game's own 16 kept)
            WANT.store(8, Relaxed);
            replay(orig_a, a);
            assert_eq!(
                calls(),
                [
                    (b'a', 0, MINF, aniso),
                    (b'a', 1, MAXA, 8),
                    (b'a', 1, MINF, aniso)
                ]
            );
            // back to GAME: the game's own values, the raised anisotropy put back
            WANT.store(0, Relaxed);
            replay(orig_a, a);
            assert_eq!(
                calls(),
                [
                    (b'a', 0, MINF, lin),
                    (b'a', 1, MAXA, 1),
                    (b'a', 1, MINF, lin)
                ]
            );
            // a Reset: nothing from before it is sent
            reset();
            WANT.store(16, Relaxed);
            replay(orig_a, a);
            assert_eq!(calls(), []);
            // another vtable replaced: the first one's calls still reach their own original
            let (mut vt_b, mut dev_b) = fake(orig_b);
            assert!(install(vt_b.as_mut_ptr()));
            let _ = hook_sampler(a, 2, addru, 3);
            let _ = hook_sampler(raw(&mut dev_b), 2, addru, 3);
            assert_eq!(calls(), [(b'a', 2, addru, 3), (b'b', 2, addru, 3)]);
            // a vtable never replaced has no original: nothing is called, never a null pointer
            let (_vt_c, mut dev_c) = fake(orig_a);
            assert_eq!(hook_sampler(raw(&mut dev_c), 0, MINF, lin), HRESULT(0));
            assert_eq!(calls(), []);
        }
    }
}
