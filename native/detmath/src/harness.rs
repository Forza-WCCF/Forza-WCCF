//! Runs a piece of generated code with every register set from [`Regs`] and read back after it: the cave tests and
//! the V4 check program (examples/v4.rs). The code is built for its own address and jumps to `back` when done; while
//! it runs, `ebx` holds the address of the harness state, whose [`MEM`] words it may use.

use iced_x86::code_asm::{
    AsmRegisterXmm, CodeAssembler, dword_ptr, eax, ebp, ebx, ecx, edi, edx, esi, esp, xmm0, xmm1,
    xmm2, xmm3, xmm4, xmm5, xmm6, xmm7, xmmword_ptr,
};
use std::mem::offset_of;

/// Executable memory for the harness (Windows: the cave tests; Linux: the V4 program on a test box).
#[cfg(windows)]
fn alloc(size: usize) -> *mut u8 {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc,
    };
    // SAFETY: a fresh private allocation
    unsafe {
        VirtualAlloc(
            std::ptr::null(),
            size,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_EXECUTE_READWRITE,
        )
        .cast()
    }
}

#[cfg(windows)]
fn free(mem: *mut u8, _size: usize) {
    use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFree};
    // SAFETY: an allocation of `alloc`
    unsafe { VirtualFree(mem.cast(), 0, MEM_RELEASE) };
}

#[cfg(unix)]
unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

#[cfg(unix)]
fn alloc(size: usize) -> *mut u8 {
    // PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANONYMOUS (musl's 64-bit off_t)
    // SAFETY: a fresh private anonymous mapping
    let p = unsafe { mmap(std::ptr::null_mut(), size, 7, 0x22, -1, 0) };
    if p as isize == -1 {
        std::ptr::null_mut()
    } else {
        p
    }
}

#[cfg(unix)]
fn free(mem: *mut u8, size: usize) {
    // SAFETY: a mapping of `alloc`
    unsafe { munmap(mem, size) };
}

/// xmm0-7, then eax ecx edx esi edi ebp, then EFLAGS.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Regs {
    pub xmm: [[u32; 4]; 8],
    pub gpr: [u32; 6],
    pub eflags: u32,
}

#[repr(C)]
struct State {
    inp: Regs,
    out: Regs,
    saved_esp: u32,
    out_esp: u32,
    mem: [u64; 8],
}

/// Offset of the harness's 8 scratch qwords from `ebx`.
pub const MEM: i32 = offset_of!(State, mem) as i32;
const ENTRY: usize = 0x400;
const BACK: usize = 0x800;
const CODE: usize = 0x1000;
const SIZE: usize = 0x4000;
const XMM: [AsmRegisterXmm; 8] = [xmm0, xmm1, xmm2, xmm3, xmm4, xmm5, xmm6, xmm7];

pub struct Harness {
    mem: *mut u8,
}

impl Harness {
    /// `code(ip, back)`: the code under test, assembled for `ip`, ending with a jump to `back`.
    pub fn new(code: impl FnOnce(u32, u32) -> Vec<u8>) -> Self {
        let mem = alloc(SIZE);
        assert!(!mem.is_null(), "no executable memory");
        let addr = |off: usize| mem as u32 + off as u32;
        let st = addr(0);
        let gpr_in = offset_of!(State, inp) + offset_of!(Regs, gpr);
        let gpr_out = offset_of!(State, out) + offset_of!(Regs, gpr);
        let xmm_in = offset_of!(State, inp) + offset_of!(Regs, xmm);
        let xmm_out = offset_of!(State, out) + offset_of!(Regs, xmm);
        let at = |off: usize| ebx + off as i32;
        let gprs = [eax, ecx, edx, esi, edi, ebp];

        let mut a = CodeAssembler::new(32).expect("32-bit");
        a.pushad().unwrap();
        a.pushfd().unwrap();
        a.mov(ebx, st).unwrap();
        a.mov(dword_ptr(at(offset_of!(State, saved_esp))), esp)
            .unwrap();
        for (i, r) in XMM.into_iter().enumerate() {
            a.movups(r, xmmword_ptr(at(xmm_in + 16 * i))).unwrap();
        }
        for (i, r) in gprs.into_iter().enumerate() {
            a.mov(r, dword_ptr(at(gpr_in + 4 * i))).unwrap();
        }
        a.push(dword_ptr(at(
            offset_of!(State, inp) + offset_of!(Regs, eflags)
        )))
        .unwrap();
        a.popfd().unwrap();
        a.jmp(u64::from(addr(CODE))).unwrap();
        let entry = a.assemble(u64::from(addr(ENTRY))).unwrap();

        let mut b = CodeAssembler::new(32).expect("32-bit");
        b.mov(dword_ptr(at(offset_of!(State, out_esp))), esp)
            .unwrap();
        for (i, r) in gprs.into_iter().enumerate() {
            b.mov(dword_ptr(at(gpr_out + 4 * i)), r).unwrap();
        }
        b.pushfd().unwrap();
        b.pop(dword_ptr(at(
            offset_of!(State, out) + offset_of!(Regs, eflags)
        )))
        .unwrap();
        for (i, r) in XMM.into_iter().enumerate() {
            b.movups(xmmword_ptr(at(xmm_out + 16 * i)), r).unwrap();
        }
        b.mov(esp, dword_ptr(at(offset_of!(State, saved_esp))))
            .unwrap();
        b.popfd().unwrap();
        b.popad().unwrap();
        b.ret().unwrap();
        let back = b.assemble(u64::from(addr(BACK))).unwrap();

        let code = code(addr(CODE), addr(BACK));
        assert!(
            entry.len() <= BACK - ENTRY && back.len() <= CODE - BACK && code.len() <= SIZE - CODE
        );
        // SAFETY: each piece fits its slot of the allocation (checked above)
        unsafe {
            std::ptr::copy_nonoverlapping(entry.as_ptr(), mem.add(ENTRY), entry.len());
            std::ptr::copy_nonoverlapping(back.as_ptr(), mem.add(BACK), back.len());
            std::ptr::copy_nonoverlapping(code.as_ptr(), mem.add(CODE), code.len());
        }
        Self { mem }
    }

    fn state(&self) -> *mut State {
        self.mem.cast()
    }

    /// Runs the code once from `inp`; the registers after it and whether `esp` came back where it was.
    pub fn run(&self, inp: &Regs) -> (Regs, bool) {
        // SAFETY: the code was assembled for this allocation and returns through `back`, which restores the caller's
        // registers, EFLAGS and stack
        unsafe {
            (*self.state()).inp = *inp;
            let f: extern "C" fn() = std::mem::transmute(self.mem.add(ENTRY));
            f();
            let s = &*self.state();
            (s.out, s.saved_esp == s.out_esp)
        }
    }

    /// Scratch qword `i` (the code reaches it at `ebx + MEM + 8 * i`).
    pub fn get(&self, i: usize) -> u64 {
        // SAFETY: the harness's own state, read while no code runs
        unsafe { (*self.state()).mem[i] }
    }

    pub fn put(&self, i: usize, v: u64) {
        // SAFETY: the harness's own state, written while no code runs
        unsafe { (*self.state()).mem[i] = v }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        free(self.mem, SIZE);
    }
}
