//! V4 (det-math RUN.md): the deterministic math's results on this CPU, as hashes to compare between PCs. It runs the
//! dmath.dll caves (exact rcpps / rsqrtps / rsqrtss) and its atan2 on 10^7 inputs each (fixed pseudo-random bit
//! patterns: denormals, infinities and NaNs included) and prints one hash per operation and one for all. The same
//! "det-math" hash on two PCs means the same results. The "native" lines hash the CPU's own estimates: they are
//! expected to differ between CPU vendors (that is the bug the caves fix). Starts no game; needs nothing beside it.

#[cfg(target_arch = "x86")]
fn main() {
    use core::arch::x86::{__cpuid, __m128, _mm_rcp_ps, _mm_rsqrt_ps};
    use dmath::cave::{Kind, exact};
    use dmath::harness::{Harness, Regs};
    use iced_x86::code_asm::CodeAssembler;
    use sha2::{Digest, Sha256};

    const N: usize = 10_000_000;
    let hex = |h: Sha256| -> String { h.finalize().iter().map(|b| format!("{b:02x}")).collect() };
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    // cpuid's brand leaves: every CPU that runs SSE2 code has them
    let brand: String = {
        (0x8000_0002..=0x8000_0004u32)
            .flat_map(|l| {
                let r = __cpuid(l);
                [r.eax, r.ebx, r.ecx, r.edx]
            })
            .flat_map(u32::to_le_bytes)
            .map(char::from)
            .collect()
    };
    println!("CPU: {}", brand.trim_matches(['\0', ' ']));
    let mut all = Sha256::new();
    for kind in [Kind::Rcpps, Kind::Rsqrtps, Kind::Rsqrtss] {
        let h = Harness::new(|ip, back| {
            let mut a = CodeAssembler::new(32).unwrap();
            exact(&mut a, kind, 0, 1, false).unwrap();
            a.jmp(u64::from(back)).unwrap();
            a.assemble(u64::from(ip)).unwrap()
        });
        let lanes = if kind == Kind::Rsqrtss { 1 } else { 4 };
        let (mut ours, mut native) = (Sha256::new(), Sha256::new());
        let mut inp = Regs::default();
        for _ in 0..N / lanes {
            let r = next();
            let s = next();
            inp.xmm[1] = [r as u32, (r >> 32) as u32, s as u32, (s >> 32) as u32];
            let (out, _) = h.run(&inp);
            for l in 0..lanes {
                ours.update(out.xmm[0][l].to_le_bytes());
            }
            // SAFETY: SSE is the target's baseline; plain bits in and out
            let est: [u32; 4] = unsafe {
                let x: __m128 = std::mem::transmute(inp.xmm[1]);
                std::mem::transmute(if kind == Kind::Rcpps {
                    _mm_rcp_ps(x)
                } else {
                    _mm_rsqrt_ps(x)
                })
            };
            for e in &est[..lanes] {
                native.update(e.to_le_bytes());
            }
        }
        let ours = hex(ours);
        all.update(&ours);
        println!("det-math {kind:?}: {ours}");
        println!(
            "native   {kind:?}: {} (CPU-dependent, information only)",
            hex(native)
        );
    }
    let mut at = Sha256::new();
    for _ in 0..N {
        let (y, x) = (f64::from_bits(next()), f64::from_bits(next()));
        at.update(dmath::det_atan2(y, x).to_bits().to_le_bytes());
    }
    let at = hex(at);
    all.update(&at);
    println!("det-math atan2: {at}");
    println!("det-math ALL: {}", hex(all));
}

#[cfg(not(target_arch = "x86"))]
fn main() {
    eprintln!("build it for i686-pc-windows-msvc");
}
