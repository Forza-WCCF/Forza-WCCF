//! The caves: each approximate instruction (`rcpps`, `rsqrtps`, `rsqrtss`, whose low bits differ between CPU vendors)
//! is replaced by a 5-byte jump to code that computes the same value with correctly rounded SSE operations
//! (`sqrtps`/`divps`: identical on every CPU), runs the instructions the jump displaced and jumps back.
//!
//! The caves touch nothing but the destination register: a scratch register is kept on the stack (`lea` moves `esp`
//! without touching EFLAGS; Windows x86 has no red zone below `esp`), and no SSE operation changes EFLAGS.

use iced_x86::code_asm::{
    AsmRegisterXmm, CodeAssembler, eax, edi, esp, ptr, xmm0, xmm1, xmm2, xmm3, xmm4, xmm5, xmm6,
    xmm7, xmmword_ptr,
};
use iced_x86::{Decoder, DecoderOptions, FlowControl};

/// The approximate instruction at a site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Rcpps,
    Rsqrtps,
    Rsqrtss,
}

/// One site in match_Release.exe: `kind xmm{dst}, xmm{src}`, `len` bytes long, at `rva`; `old` holds the bytes the
/// 5-byte jump covers (the instruction and the whole instructions after it that the jump displaces).
pub struct Site {
    pub rva: u32,
    pub kind: Kind,
    pub dst: u8,
    pub src: u8,
    pub len: u8,
    pub old: &'static [u8],
}

const XMM: [AsmRegisterXmm; 8] = [xmm0, xmm1, xmm2, xmm3, xmm4, xmm5, xmm6, xmm7];

/// Where the engine draws its per-frame random number: `mov edx, [edi+70114h]` (VA 0x444EDF, right after the draw is
/// stored and before the frame counter at `[edi+70090h]` goes up). The frame log ([`frame_cave`]) hooks it.
pub const FRAME_RVA: u32 = 0x44edf;
pub const FRAME_OLD: &[u8] = &[0x8b, 0x97, 0x14, 0x01, 0x07, 0x00];

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 1.0f in every lane of `r`, built in the register: all ones, << 25 = 0xFE000000, >> 2 = 0x3F800000.
fn one(a: &mut CodeAssembler, r: AsmRegisterXmm) -> Result<(), String> {
    a.pcmpeqd(r, r).map_err(err)?;
    a.pslld(r, 25).map_err(err)?;
    a.psrld(r, 2).map_err(err)
}

/// The exact `kind xmm{d}, xmm{s}`: 1/x or 1/sqrt(x) per lane (`rsqrtss`: the low lane, the others kept).
/// `foreign` cuts the result to the 11 high mantissa bits, as another CPU's 12-bit estimate would differ (the one-PC
/// desync test, RUN.md V2).
pub fn exact(
    a: &mut CodeAssembler,
    kind: Kind,
    d: usize,
    s: usize,
    foreign: bool,
) -> Result<(), String> {
    let (dr, sr) = (XMM[d], XMM[s]);
    if kind == Kind::Rcpps && d != s && !foreign {
        one(a, dr)?;
        return a.divps(dr, sr).map_err(err);
    }
    let mut free = (0..8).filter(|&r| r != d && r != s).map(|r| XMM[r]);
    let (Some(t), Some(u)) = (free.next(), free.next()) else {
        unreachable!("8 registers, at most 2 taken")
    };
    a.lea(esp, ptr(esp - 32)).map_err(err)?;
    a.movups(xmmword_ptr(esp), t).map_err(err)?;
    a.movups(xmmword_ptr(esp + 16), u).map_err(err)?;
    match kind {
        Kind::Rcpps => {
            one(a, t)?;
            a.divps(t, sr).map_err(err)?;
        }
        Kind::Rsqrtps => {
            a.sqrtps(u, sr).map_err(err)?;
            one(a, t)?;
            a.divps(t, u).map_err(err)?;
        }
        Kind::Rsqrtss => {
            a.sqrtss(u, sr).map_err(err)?;
            one(a, t)?;
            a.divss(t, u).map_err(err)?;
        }
    }
    if foreign {
        a.pcmpeqd(u, u).map_err(err)?;
        a.pslld(u, 12).map_err(err)?;
        a.andps(t, u).map_err(err)?;
    }
    if kind == Kind::Rsqrtss {
        a.movss(dr, t).map_err(err)?;
    } else {
        a.movaps(dr, t).map_err(err)?;
    }
    a.movups(u, xmmword_ptr(esp + 16)).map_err(err)?;
    a.movups(t, xmmword_ptr(esp)).map_err(err)?;
    a.lea(esp, ptr(esp + 32)).map_err(err)
}

/// The instructions in `old[from..]` (at `at + from` in the program), moved, then a jump back to `at + old.len()`.
/// Only whole, straight-line, non-IP-relative instructions move.
fn tail(a: &mut CodeAssembler, old: &[u8], from: usize, at: u32) -> Result<(), String> {
    let mut d = Decoder::with_ip(
        32,
        &old[from..],
        u64::from(at) + from as u64,
        DecoderOptions::NONE,
    );
    while d.can_decode() {
        let i = d.decode();
        if i.is_invalid() || i.flow_control() != FlowControl::Next || i.is_ip_rel_memory_operand() {
            return Err(format!("the instruction at {:#x} cannot be moved", i.ip()));
        }
        a.add_instruction(i).map_err(err)?;
    }
    a.jmp(u64::from(at) + old.len() as u64).map_err(err)
}

/// The cave for `site` found at `at` in the running program, placed at `ip`.
pub fn cave(site: &Site, at: u32, ip: u32, foreign: bool) -> Result<Vec<u8>, String> {
    let mut a = CodeAssembler::new(32).map_err(err)?;
    exact(&mut a, site.kind, site.dst.into(), site.src.into(), foreign)?;
    tail(&mut a, site.old, site.len.into(), at)?;
    a.assemble(u64::from(ip)).map_err(err)
}

/// The frame log's cave: every register, EFLAGS and xmm0-7 kept, `log(edi)` called (cdecl), then the displaced
/// `mov edx, [edi+70114h]` and the jump back.
pub fn frame_cave(at: u32, ip: u32, log: u32) -> Result<Vec<u8>, String> {
    let mut a = CodeAssembler::new(32).map_err(err)?;
    a.pushad().map_err(err)?;
    a.pushfd().map_err(err)?;
    a.lea(esp, ptr(esp - 128)).map_err(err)?;
    for (i, r) in XMM.into_iter().enumerate() {
        a.movups(xmmword_ptr(esp + 16 * i as i32), r).map_err(err)?;
    }
    a.push(edi).map_err(err)?;
    a.mov(eax, log).map_err(err)?;
    a.call(eax).map_err(err)?;
    a.lea(esp, ptr(esp + 4)).map_err(err)?;
    for (i, r) in XMM.into_iter().enumerate() {
        a.movups(r, xmmword_ptr(esp + 16 * i as i32)).map_err(err)?;
    }
    a.lea(esp, ptr(esp + 128)).map_err(err)?;
    a.popfd().map_err(err)?;
    a.popad().map_err(err)?;
    tail(&mut a, FRAME_OLD, 0, at)?;
    a.assemble(u64::from(ip)).map_err(err)
}

/// `jmp rel32` from `at` to `to`, the rest of the `cover` bytes int3 (never run: the cave jumps back past them).
pub fn jump(at: u32, to: u32, cover: usize) -> Vec<u8> {
    let mut j = vec![0xe9];
    j.extend(to.wrapping_sub(at.wrapping_add(5)).to_le_bytes());
    j.resize(cover, 0xcc);
    j
}

/// The caves (to be placed at `caves_at`) and the jumps (RVA, bytes) that patch the program loaded at `base`.
pub struct Plan {
    pub caves: Vec<u8>,
    pub jumps: Vec<(u32, Vec<u8>)>,
}

/// Every site of `sites` checked in `image` (the loaded program's bytes from its base) before anything is planned:
/// one site that is not the original makes the whole plan an error. `frames`: the frame log function's address, to
/// hook [`FRAME_RVA`] as well.
pub fn plan(
    image: &[u8],
    base: u32,
    caves_at: u32,
    sites: &[Site],
    foreign: bool,
    frames: Option<u32>,
) -> Result<Plan, String> {
    let at = |rva: u32, n: usize| image.get(rva as usize..rva as usize + n);
    let bad: Vec<u32> = sites
        .iter()
        .filter(|s| at(s.rva, s.old.len()) != Some(s.old))
        .map(|s| s.rva)
        .collect();
    if let Some(first) = bad.first() {
        return Err(format!(
            "{}/{} sites hold the original bytes (first different at RVA {first:#x})",
            sites.len() - bad.len(),
            sites.len()
        ));
    }
    if frames.is_some() && at(FRAME_RVA, FRAME_OLD.len()) != Some(FRAME_OLD) {
        return Err(format!(
            "the frame log's site (RVA {FRAME_RVA:#x}) is not the original"
        ));
    }
    let mut p = Plan {
        caves: Vec::new(),
        jumps: Vec::new(),
    };
    let mut add = |rva: u32, cover: usize, make: &dyn Fn(u32, u32) -> Result<Vec<u8>, String>| {
        p.caves.resize(p.caves.len().next_multiple_of(16), 0xcc);
        let ip = caves_at + p.caves.len() as u32;
        p.caves.extend(make(base + rva, ip)?);
        p.jumps.push((rva, jump(base + rva, ip, cover)));
        Ok::<(), String>(())
    };
    for s in sites {
        add(s.rva, s.old.len(), &|at, ip| cave(s, at, ip, foreign))?;
    }
    if let Some(log) = frames {
        add(FRAME_RVA, FRAME_OLD.len(), &|at, ip| {
            frame_cave(at, ip, log)
        })?;
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{Harness, Regs};
    use crate::sites::SITES;
    use core::arch::x86::*;
    use iced_x86::{Mnemonic, Register};

    /// The edge inputs: zeros, denormals, infinities, NaNs, negatives, extremes; then random bit patterns.
    fn inputs() -> Vec<u32> {
        let mut v = vec![
            0x0000_0000,
            0x8000_0000,
            0x0000_0001,
            0x007f_ffff,
            0x8000_0001,
            0x8040_0000,
            0x0080_0000,
            0x7f7f_ffff,
            0xff7f_ffff,
            0x7f80_0000,
            0xff80_0000,
            0x7fc0_0000,
            0xffc0_0000,
            0x7f80_0001,
            0x7fbf_ffff,
            0x3f80_0000,
            0xbf80_0000,
            0x4000_0000,
            0x3e80_0000,
            0x5f00_0000,
            0x2000_0000,
            0x1f00_0000,
            0x6000_0000,
        ];
        let mut x = 0x2545_f491u32;
        for _ in 0..4000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            v.push(x);
        }
        v
    }

    fn lanes(r: __m128) -> [u32; 4] {
        // SAFETY: same size, plain bits
        unsafe { std::mem::transmute(r) }
    }

    fn m128(l: [u32; 4]) -> __m128 {
        // SAFETY: same size, plain bits
        unsafe { std::mem::transmute(l) }
    }

    /// The reference: divps / sqrtps, as Rust compiles the intrinsics.
    fn reference(kind: Kind, x: [u32; 4]) -> [u32; 4] {
        // SAFETY: SSE2 is the target's baseline
        unsafe {
            let (one, x) = (_mm_set1_ps(1.0), m128(x));
            match kind {
                Kind::Rcpps => lanes(_mm_div_ps(one, x)),
                Kind::Rsqrtps | Kind::Rsqrtss => lanes(_mm_div_ps(one, _mm_sqrt_ps(x))),
            }
        }
    }

    /// The hardware estimate, for the sanity bound only.
    fn estimate(kind: Kind, x: [u32; 4]) -> [u32; 4] {
        // SAFETY: SSE is the target's baseline
        unsafe {
            match kind {
                Kind::Rcpps => lanes(_mm_rcp_ps(m128(x))),
                Kind::Rsqrtps | Kind::Rsqrtss => lanes(_mm_rsqrt_ps(m128(x))),
            }
        }
    }

    const FLAGS: u32 = 0x8d5; // CF PF AF ZF SF OF

    /// A site-like byte string: the instruction, then `tail` (displaced instructions).
    fn synthetic(kind: Kind, d: u8, s: u8, tail: &[u8]) -> Site {
        let modrm = 0xc0 | (d << 3) | s;
        let mut b: Vec<u8> = match kind {
            Kind::Rcpps => vec![0x0f, 0x53, modrm],
            Kind::Rsqrtps => vec![0x0f, 0x52, modrm],
            Kind::Rsqrtss => vec![0xf3, 0x0f, 0x52, modrm],
        };
        let len = b.len() as u8;
        b.extend(tail);
        Site {
            rva: 0,
            kind,
            dst: d,
            src: s,
            len,
            old: Box::leak(b.into_boxed_slice()),
        }
    }

    /// Runs the cave of `site` over every input in the source lanes and checks it against the reference, with every
    /// other register, EFLAGS and esp unchanged.
    fn check(site: &Site, foreign: bool) {
        let h = Harness::new(|ip, back| {
            let at = back - site.old.len() as u32;
            cave(site, at, ip, foreign).unwrap()
        });
        let (d, s) = (usize::from(site.dst), usize::from(site.src));
        let xs = inputs();
        for (n, chunk) in xs.chunks(4).enumerate() {
            let mut x = [0x4040_0000; 4];
            x[..chunk.len()].copy_from_slice(chunk);
            let mut inp = Regs::default();
            for (r, reg) in inp.xmm.iter_mut().enumerate() {
                *reg = [
                    0x1111_0000 + r as u32,
                    0x2222_0000 + r as u32,
                    0x3333_0000,
                    0x4444_0000 + n as u32,
                ];
            }
            inp.xmm[s] = x;
            inp.gpr = [1, 2, 3, 4, 5, 6];
            inp.eflags = if n % 2 == 0 { FLAGS | 2 } else { 2 };
            let (out, esp_same) = h.run(&inp);
            assert!(esp_same, "esp moved");
            assert_eq!(out.eflags & FLAGS, inp.eflags & FLAGS, "EFLAGS changed");
            assert_eq!(out.gpr, inp.gpr, "a general register changed");
            let mut want = reference(site.kind, x);
            if foreign {
                want = want.map(|l| l & 0xffff_f000);
            }
            let mut expect = inp.xmm;
            if site.kind == Kind::Rsqrtss {
                expect[d][0] = want[0];
            } else {
                expect[d] = want;
            }
            assert_eq!(out.xmm, expect, "{:?} d={d} s={s} x={x:08x?}", site.kind);
            // within the estimate's error of the hardware's own answer (finite, normal results)
            let est = estimate(site.kind, x);
            for l in 0..4 {
                let (w, e) = (f32::from_bits(want[l]), f32::from_bits(est[l]));
                if w.is_normal() && e.is_normal() && !foreign {
                    assert!(((w - e) / w).abs() < 1.0 / 2048.0, "{w} vs estimate {e}");
                }
            }
        }
    }

    #[test]
    fn every_template_is_exact_and_keeps_the_rest() {
        for foreign in [false, true] {
            for kind in [Kind::Rcpps, Kind::Rsqrtps, Kind::Rsqrtss] {
                // d != s (rcpps: the no-spill template), d == s, and registers at both ends
                for (d, s) in [(0, 1), (2, 1), (1, 1), (0, 0), (7, 6), (6, 7), (5, 5)] {
                    check(&synthetic(kind, d, s, &[0x90, 0x90]), foreign);
                }
            }
        }
    }

    #[test]
    fn a_displaced_neighbour_runs_after_the_exact_value() {
        // the third site: rcpps xmm0, xmm1; movaps xmm2, xmm0
        let s = &SITES[2];
        assert_eq!(s.old, [0x0f, 0x53, 0xc1, 0x0f, 0x28, 0xd0]);
        let h = Harness::new(|ip, back| cave(s, back - 6, ip, false).unwrap());
        let mut inp = Regs::default();
        inp.xmm[1] = [0x4000_0000, 0x3f80_0000, 0xc080_0000, 0];
        let (out, esp_same) = h.run(&inp);
        let want = reference(Kind::Rcpps, inp.xmm[1]);
        assert!(esp_same);
        assert_eq!((out.xmm[0], out.xmm[2]), (want, want));
        assert_eq!(f32::from_bits(want[0]), 0.5);
    }

    #[test]
    fn the_frame_cave_calls_the_log_and_keeps_every_register() {
        static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        extern "C" fn log(sim: u32) {
            SEEN.store(sim, std::sync::atomic::Ordering::Relaxed);
        }
        let h = Harness::new(|ip, back| {
            frame_cave(back - 6, ip, log as extern "C" fn(u32) as usize as u32).unwrap()
        });
        // edi points 0x70114 below a value the displaced `mov edx, [edi+70114h]` reads
        let value = Box::leak(Box::new(0xdead_beefu32));
        let sim = (value as *mut u32 as u32).wrapping_sub(0x70114);
        let mut inp = Regs::default();
        inp.xmm[3] = [1, 2, 3, 4];
        inp.gpr = [10, 11, 12, 13, sim, 15];
        inp.eflags = FLAGS | 2;
        let (out, esp_same) = h.run(&inp);
        assert!(esp_same);
        assert_eq!(SEEN.load(std::sync::atomic::Ordering::Relaxed), sim);
        assert_eq!(
            out.gpr,
            [10, 11, 0xdead_beef, 13, sim, 15],
            "edx = the value, the rest kept"
        );
        assert_eq!(out.xmm, inp.xmm);
        assert_eq!(out.eflags & FLAGS, FLAGS);
    }

    #[test]
    fn the_table_is_what_it_says() {
        assert_eq!(SITES.len(), 140);
        let count = |k| SITES.iter().filter(|s| s.kind == k).count();
        assert_eq!(
            (
                count(Kind::Rsqrtss),
                count(Kind::Rsqrtps),
                count(Kind::Rcpps)
            ),
            (25, 21, 94)
        );
        let mut end = 0;
        for s in &SITES {
            assert!(s.rva >= end, "sites overlap at {:#x}", s.rva);
            end = s.rva + s.old.len() as u32;
            assert!(s.old.len() >= 5);
            let mut dec = Decoder::with_ip(32, s.old, u64::from(s.rva), DecoderOptions::NONE);
            let i = dec.decode();
            let m = match s.kind {
                Kind::Rcpps => Mnemonic::Rcpps,
                Kind::Rsqrtps => Mnemonic::Rsqrtps,
                Kind::Rsqrtss => Mnemonic::Rsqrtss,
            };
            assert_eq!(i.mnemonic(), m, "{:#x}", s.rva);
            assert_eq!(i.len(), usize::from(s.len));
            let reg = |n: u8| Register::XMM0 as u32 + u32::from(n);
            assert_eq!(
                (i.op0_register() as u32, i.op1_register() as u32),
                (reg(s.dst), reg(s.src)),
                "{:#x}",
                s.rva
            );
            // every cave assembles, its displaced instructions whole, at a high and a low base
            for base in [0x0040_0000u32, 0x7000_0000] {
                cave(s, base + s.rva, 0x1000_0000, false).unwrap();
            }
        }
    }

    #[test]
    fn a_plan_needs_every_original_byte() {
        let base = 0x0100_0000;
        let mut image = vec![0u8; 0x40_0000];
        for s in &SITES {
            image[s.rva as usize..][..s.old.len()].copy_from_slice(s.old);
        }
        image[FRAME_RVA as usize..][..FRAME_OLD.len()].copy_from_slice(FRAME_OLD);
        let p = plan(&image, base, 0x2000_0000, &SITES, false, Some(0x3000_0000)).unwrap();
        assert_eq!(p.jumps.len(), 141);
        for (rva, j) in &p.jumps {
            assert_eq!(j[0], 0xe9);
            let to = (base + rva + 5).wrapping_add(u32::from_le_bytes(j[1..5].try_into().unwrap()));
            assert!((0x2000_0000..0x2000_0000 + p.caves.len() as u32).contains(&to));
        }
        let first = SITES[7].rva as usize;
        image[first + 1] ^= 1;
        let e = plan(&image, base, 0x2000_0000, &SITES, false, None)
            .err()
            .unwrap();
        assert!(e.starts_with("139/140 sites"), "{e}");
        assert!(e.contains(&format!("{first:#x}")), "{e}");
        image[first + 1] ^= 1;
        image[FRAME_RVA as usize] = 0x90;
        assert!(
            plan(&image, base, 0x2000_0000, &SITES, false, None).is_ok(),
            "no frame log, no frame check"
        );
        assert!(plan(&image, base, 0x2000_0000, &SITES, false, Some(1)).is_err());
    }
}
