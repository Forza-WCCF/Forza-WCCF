//! The Windows side: DllMain, the checks and the in-memory patch of the engine, the atan2 import hook, the logs.

use std::ffi::c_void;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Mutex, OnceLock};

use crate::{FOREIGN, cave, det_atan2, sites};
use forza_native::{By, imports, redirect};
use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{HINSTANCE, HMODULE};
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::LibraryLoader::{
    DisableThreadLibraryCalls, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExA,
    GetModuleHandleW,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
};
use windows_sys::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// The engine's import as setup renames it (same length as the original `PSAPI.DLL`, NUL included).
pub const IMPORT: &[u8] = b"dmath.dll\0";
pub const ORIGINAL_IMPORT: &[u8] = b"PSAPI.DLL\0";
/// Room for the caves (about 9 KB; one allocation granule).
const CAVES: usize = 0x1_0000;

static LOG: OnceLock<PathBuf> = OnceLock::new();
static FRAMES: Mutex<Option<BufWriter<std::fs::File>>> = Mutex::new(None);

/// One line in dmath.log (every engine of the folder appends to the same file).
fn log(text: &str) {
    let Some(path) = LOG.get() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
    {
        let t = forza_native::local_time();
        let line = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} pid={} {text}\r\n",
            t.wYear,
            t.wMonth,
            t.wDay,
            t.wHour,
            t.wMinute,
            t.wSecond,
            std::process::id()
        );
        let _ = f.write_all(line.as_bytes()); // one write: whole lines
    }
}

/// MXCSR and the x87 control word of the calling thread (expected 0x1F80 and 0x027F).
pub fn control_words() -> (u32, u16) {
    let (mut m, mut c) = (0u32, 0u16);
    // SAFETY: both store into our own locals; nothing changes
    unsafe {
        core::arch::asm!("stmxcsr [{0}]", "fnstcw [{1}]", in(reg) &raw mut m, in(reg) &raw mut c, options(nostack));
    }
    (m, c)
}

/// The engine file is the pinned original once its one renamed import is named PSAPI.DLL again.
pub fn verify_file(file: &[u8]) -> Result<(), String> {
    let hits: Vec<usize> = file
        .windows(IMPORT.len())
        .enumerate()
        .filter(|(_, w)| *w == IMPORT)
        .map(|(i, _)| i)
        .collect();
    let [at] = hits[..] else {
        return Err(format!(
            "the engine names dmath.dll {} times, expected once",
            hits.len()
        ));
    };
    let mut orig = file.to_vec();
    orig[at..at + IMPORT.len()].copy_from_slice(ORIGINAL_IMPORT);
    let got: String = Sha256::digest(&orig)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got != sites::EXE_SHA256 {
        return Err(format!(
            "the engine is not the pinned original (SHA-256 {got} with PSAPI.DLL, expected {})",
            sites::EXE_SHA256
        ));
    }
    Ok(())
}

/// `_CIatan2` as the engine calls it: y in ST1, x in ST0, both popped, the result in ST0 (msvcr90's x87 intrinsic
/// convention; caller-saved registers may change, as the original's do).
///
/// # Safety
/// Only through the engine's `_CIatan2` import, with two values on the x87 stack.
#[unsafe(naked)]
pub unsafe extern "C" fn ci_atan2() {
    core::arch::naked_asm!(
        "sub esp, 16",
        "fstp qword ptr [esp + 8]",
        "fstp qword ptr [esp]",
        "call {f}",
        "add esp, 16",
        "ret",
        f = sym det_atan2,
    );
}

/// One line per sim frame: the frame counter and the random number drawn for it ([`cave::frame_cave`] calls it with
/// the engine's `edi`).
extern "C" fn log_frame(sim: u32) {
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| {
        let (m, c) = control_words();
        log(&format!(
            "det-math frame log on the sim thread: MXCSR {m:#06X}, CW {c:#06X}"
        ));
    });
    // SAFETY: the engine's own frame object (its edi at the hooked instruction, which reads the same field)
    let (frame, value) = unsafe {
        (
            ((sim + 0x70090) as *const u32).read_unaligned(),
            ((sim + 0x70114) as *const u32).read_unaligned(),
        )
    };
    if let Ok(mut g) = FRAMES.lock()
        && let Some(w) = g.as_mut()
    {
        let _ = writeln!(w, "{frame} {value:08x}");
        if frame % 60 == 0 {
            let _ = w.flush();
        }
    }
}

/// The engine's `_CIatan2` import slot, once the loader has filled it (it points into a loaded DLL).
///
/// # Safety
/// `base` is a loaded module.
unsafe fn atan2_slot(base: *const u8) -> Result<*mut usize, String> {
    // SAFETY: the caller's contract
    let dll = unsafe { imports(base) }
        .find(|d| d.name().to_bytes().eq_ignore_ascii_case(b"msvcr90.dll"))
        .ok_or("the engine imports no MSVCR90.dll")?;
    let slot = dll
        .find(By::Name(c"_CIatan2"))
        .ok_or("the engine imports no _CIatan2")?;
    let mut owner: HMODULE = std::ptr::null_mut();
    let flags =
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    // SAFETY: an import slot of the loaded module; only its value is looked up
    if unsafe { GetModuleHandleExA(flags, slot.read() as *const u8, &mut owner) } == 0 {
        return Err("the _CIatan2 import is not resolved yet".into());
    }
    Ok(slot)
}

/// Patches the engine loaded at `base` whose file is `file`: every check first, then the caves, the 140 jumps (the
/// frame log's too with `frames`) and the atan2 import. Ok: the log line; Err: why nothing was patched.
///
/// # Safety
/// `base` is the loaded `file`, no other thread runs its code, and it stays loaded for the life of the process.
pub unsafe fn patch(
    base: *mut u8,
    file: &[u8],
    foreign: bool,
    frames: bool,
) -> Result<String, String> {
    verify_file(file)?;
    // SAFETY: a loaded image's headers: e_lfanew, then SizeOfImage at +0x50 of the PE32 headers
    let image = unsafe {
        let nt = base.add(0x3c).cast::<u32>().read_unaligned() as usize;
        let size = base.add(nt + 0x50).cast::<u32>().read_unaligned() as usize;
        std::slice::from_raw_parts(base, size)
    };
    // SAFETY: the caller's contract
    let slot = unsafe { atan2_slot(base)? };
    // SAFETY: a fresh private allocation, freed below unless the patch goes in
    let mem = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            CAVES,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if mem.is_null() {
        return Err("no memory for the caves".into());
    }
    let free = || {
        // SAFETY: the allocation above, not in use
        unsafe { VirtualFree(mem, 0, MEM_RELEASE) };
    };
    let log_fn = frames.then_some(log_frame as extern "C" fn(u32) as usize as u32);
    let p = match cave::plan(
        image,
        base as u32,
        mem as u32,
        &sites::SITES,
        foreign,
        log_fn,
    ) {
        Ok(p) if p.caves.len() <= CAVES => p,
        Ok(_) => {
            free();
            return Err("the caves do not fit".into());
        }
        Err(e) => {
            free();
            return Err(e);
        }
    };
    let lo = p.jumps.iter().map(|j| j.0).min().unwrap_or(0) as usize;
    let hi = p
        .jumps
        .iter()
        .map(|j| j.0 as usize + j.1.len())
        .max()
        .unwrap_or(0);
    let mut prot: PAGE_PROTECTION_FLAGS = 0;
    // SAFETY: the caves go into our allocation; the jumps into the engine's code, whose bytes plan() checked, made
    // writable for the writes and executable-read-only again after
    unsafe {
        std::ptr::copy_nonoverlapping(p.caves.as_ptr(), mem.cast::<u8>(), p.caves.len());
        if VirtualProtect(mem, CAVES, PAGE_EXECUTE_READ, &mut prot) == 0
            || VirtualProtect(
                base.add(lo).cast(),
                hi - lo,
                PAGE_EXECUTE_READWRITE,
                &mut prot,
            ) == 0
        {
            free();
            return Err("the engine's code cannot be made writable".into());
        }
        for (rva, bytes) in &p.jumps {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(*rva as usize), bytes.len());
        }
        VirtualProtect(base.add(lo).cast(), hi - lo, PAGE_EXECUTE_READ, &mut prot);
        FlushInstructionCache(GetCurrentProcess(), base.add(lo).cast(), hi - lo);
    }
    // SAFETY: the slot is the engine's resolved `_CIatan2` import; ci_atan2 keeps its convention
    let hooked = unsafe { redirect(slot, ci_atan2 as unsafe extern "C" fn() as usize) }.is_some();
    let (m, c) = control_words();
    Ok(format!(
        "det-math {n}/{n}, atan2 {}, MXCSR {m:#06X}, CW {c:#06X}{}{}",
        if hooked { "hooked" } else { "NOT hooked" },
        if foreign {
            ", FOREIGN estimate (test)"
        } else {
            ""
        },
        if frames { ", frame log on" } else { "" },
        n = sites::SITES.len(),
    ))
}

/// The folder of this DLL.
fn dll_dir(inst: HINSTANCE) -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    // SAFETY: our own module handle and a buffer we own
    let n = unsafe { GetModuleFileNameW(inst, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| {
        let p = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
        p.parent().map(PathBuf::from).unwrap_or_default()
    })
}

fn start(inst: HINSTANCE) {
    let Some(dir) = dll_dir(inst) else { return };
    let _ = LOG.set(dir.join("dmath.log"));
    let cfg = std::fs::read_to_string(dir.join("dmath.cfg"))
        .unwrap_or_default()
        .to_lowercase();
    let has = |w: &str| cfg.split_whitespace().any(|x| x == w);
    if has("off") {
        log("det-math off (dmath.cfg): nothing patched");
        return;
    }
    let (foreign, frames) = (has("foreign"), has("frames"));
    FOREIGN.store(foreign, Relaxed);
    if frames {
        let path = dir.join(format!("dmath_frames_{}.log", std::process::id()));
        match std::fs::File::create(&path) {
            Ok(f) => *FRAMES.lock().unwrap_or_else(|e| e.into_inner()) = Some(BufWriter::new(f)),
            Err(e) => log(&format!("det-math: no frame log ({}: {e})", path.display())),
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string());
    let file = exe.and_then(|p| std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display())));
    // SAFETY: DllMain of an import of the engine: its image is loaded and none of its code has run yet
    let r = file.and_then(|f| unsafe {
        patch(
            GetModuleHandleW(std::ptr::null()).cast(),
            &f,
            foreign,
            frames,
        )
    });
    match r {
        Ok(line) => log(&line),
        Err(why) => log(&format!("det-math NOT applied, nothing patched: {why}")),
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllMain(
    inst: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> windows_sys::core::BOOL {
    if reason == DLL_PROCESS_ATTACH {
        // SAFETY: our own module handle
        unsafe { DisableThreadLibraryCalls(inst) };
        start(inst);
    } else if reason == DLL_PROCESS_DETACH {
        // try_lock: at process exit a killed thread may still hold it
        if let Ok(mut g) = FRAMES.try_lock()
            && let Some(w) = g.as_mut()
        {
            let _ = w.flush();
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{Harness, MEM, Regs};
    use iced_x86::code_asm::{CodeAssembler, ebx, qword_ptr, word_ptr};
    use windows_sys::Win32::Foundation::FreeLibrary;
    use windows_sys::Win32::System::LibraryLoader::{DONT_RESOLVE_DLL_REFERENCES, LoadLibraryExW};

    /// `ci_atan2` called as the engine calls `_CIatan2`, x87 status word before and after.
    fn x87_atan2(h: &Harness, y: f64, x: f64) -> (f64, u64, u64) {
        h.put(0, y.to_bits());
        h.put(1, x.to_bits());
        h.run(&Regs::default());
        (
            f64::from_bits(h.get(2)),
            h.get(3) & 0x3800,
            h.get(4) & 0x3800,
        )
    }

    #[test]
    fn atan2_takes_the_x87_convention_and_is_libm() {
        let h = Harness::new(|ip, back| {
            let m = |i: i32| ebx + (MEM + 8 * i);
            let mut a = CodeAssembler::new(32).unwrap();
            a.fnstsw(word_ptr(m(3))).unwrap();
            a.fld(qword_ptr(m(0))).unwrap(); // y -> ST1
            a.fld(qword_ptr(m(1))).unwrap(); // x -> ST0
            a.call(ci_atan2 as unsafe extern "C" fn() as usize as u64)
                .unwrap();
            a.fstp(qword_ptr(m(2))).unwrap();
            a.fnstsw(word_ptr(m(4))).unwrap();
            a.jmp(u64::from(back)).unwrap();
            a.assemble(u64::from(ip)).unwrap()
        });
        let cases = [
            (1.0, 1.0),
            (1.0, 0.0),
            (0.0, -1.0),
            (-0.0, -1.0),
            (0.0, 0.0),
            (-0.0, -0.0),
            (f64::INFINITY, f64::NEG_INFINITY),
            (f64::NAN, 1.0),
            (-3.5, 2.25),
            (1e-310, -7.0),
        ];
        for (y, x) in cases {
            let (r, top0, top1) = x87_atan2(&h, y, x);
            assert_eq!(top0, top1, "the x87 stack is balanced");
            let want = libm::atan2(y, x);
            assert!(
                r.to_bits() == want.to_bits() || (r.is_nan() && want.is_nan()),
                "atan2({y}, {x}) = {r}, not {want}"
            );
        }
        assert_eq!(x87_atan2(&h, 1.0, 1.0).0, std::f64::consts::FRAC_PI_4);
        assert_eq!(x87_atan2(&h, 1.0, 0.0).0, std::f64::consts::FRAC_PI_2);
        assert_eq!(
            control_words().0 & !0x3f,
            0x1f80,
            "default MXCSR, the shim changed nothing"
        );
    }

    #[test]
    fn the_engine_file_check() {
        let mut f = b"MZ....PSAPI.DLL\0....".to_vec();
        assert!(verify_file(&f).unwrap_err().contains("0 times"));
        f[6..16].copy_from_slice(IMPORT);
        assert!(
            verify_file(&f)
                .unwrap_err()
                .contains("not the pinned original")
        );
        f.extend(IMPORT);
        assert!(verify_file(&f).unwrap_err().contains("2 times"));
    }

    /// Acceptance check 2, with the real engine: DETMATH_MATCH_EXE names an original match_Release.exe (never
    /// written). A scratch copy with the import renamed is mapped (not run) in this process and patched in memory.
    #[test]
    fn the_real_engine_patches_140_of_140_or_nothing() {
        let Some(orig) = std::env::var_os("DETMATH_MATCH_EXE") else {
            eprintln!("DETMATH_MATCH_EXE not set: skipped");
            return;
        };
        let data = std::fs::read(&orig).unwrap();
        let at = data.windows(10).position(|w| w == ORIGINAL_IMPORT).unwrap();
        let mut copy = data.clone();
        copy[at..at + 10].copy_from_slice(IMPORT);
        let dir = std::env::temp_dir().join(format!("detmath-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("match_Release.exe");
        std::fs::write(&path, &copy).unwrap();
        let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
        // SAFETY: mapped as an image only, no code of it runs
        let m = unsafe {
            LoadLibraryExW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                DONT_RESOLVE_DLL_REFERENCES,
            )
        };
        assert!(!m.is_null(), "the scratch copy maps");
        let base = m.cast::<u8>();
        // SAFETY: the mapped image; nothing runs it
        unsafe {
            // the loader's job, undone by DONT_RESOLVE_DLL_REFERENCES: _CIatan2's slot points into a loaded DLL
            let dll = imports(base)
                .find(|d| d.name().to_bytes().eq_ignore_ascii_case(b"msvcr90.dll"))
                .unwrap();
            let slot = dll.find(By::Name(c"_CIatan2")).unwrap();
            assert!(
                atan2_slot(base).is_err(),
                "an unresolved slot is left alone"
            );
            redirect(
                slot,
                GetModuleHandleW as unsafe extern "system" fn(*const u16) -> HMODULE as usize,
            )
            .unwrap();
            let site = |i: usize| {
                std::slice::from_raw_parts(
                    base.add(sites::SITES[i].rva as usize),
                    sites::SITES[i].old.len(),
                )
            };

            // wrong file: nothing changes
            let e = patch(base, &data, false, true).unwrap_err();
            assert!(e.contains("0 times"), "{e}");
            let mut other = copy.clone();
            other[0x400] ^= 1;
            assert!(
                patch(base, &other, false, true)
                    .unwrap_err()
                    .contains("not the pinned original")
            );
            // one site not the original: nothing changes
            let one = base.add(sites::SITES[9].rva as usize);
            let flip = || {
                let mut prot = 0;
                VirtualProtect(one.cast(), 1, PAGE_EXECUTE_READWRITE, &mut prot);
                *one ^= 0xff;
                VirtualProtect(one.cast(), 1, prot, &mut prot);
            };
            flip();
            let e = patch(base, &copy, false, true).unwrap_err();
            assert!(e.starts_with("139/140"), "{e}");
            assert!(
                (0..140)
                    .filter(|&i| i != 9)
                    .all(|i| site(i) == sites::SITES[i].old)
            );
            assert_eq!(
                slot.read(),
                GetModuleHandleW as unsafe extern "system" fn(*const u16) -> HMODULE as usize
            );
            flip();

            let line = patch(base, &copy, false, true).unwrap();
            eprintln!("{line}");
            assert!(
                line.starts_with("det-math 140/140, atan2 hooked, MXCSR 0x1F80"),
                "{line}"
            );
            assert_eq!(slot.read(), ci_atan2 as unsafe extern "C" fn() as usize);
            for i in 0..140 {
                assert_eq!(site(i)[0], 0xe9, "site {i} jumps");
            }
            assert_eq!(*base.add(cave::FRAME_RVA as usize), 0xe9);
            FreeLibrary(m);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
