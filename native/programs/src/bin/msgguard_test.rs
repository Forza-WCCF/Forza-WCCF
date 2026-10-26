//! Not shipped. The server hook's MSGPARSE guard ([forza-hook e1011-server]): control's FUN_00412140 -> FUN_00412860
//! call as the game makes it. The caller keeps len at esp+0xC, off at +0x10, buf at +0x28, pushes &off, buf, &len,
//! ebx = this, esi = seat; the victim pushes ebx/ebp/esi/edi, overwrites its &len slot as FUN_00412860 does at
//! 0x41287F, then faults. The guard must return 1 to the caller with len/off zeroed, ebx/esi intact, and the seat's
//! carried-over count dropped only when no buffer is left. Run under Wine beside the server's winmm.dll,
//! winmm_orig.dll and netguard.dll; prints PASS.
use std::process::ExitCode;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, LoadLibraryA};

std::arch::global_asm!(
    ".globl _victim",
    "_victim:",
    " push %ebx",
    " push %ebp",
    " mov 0xc(%esp), %ebp",
    " push %esi",
    " push %edi",
    " movl $0x1C, 0x14(%esp)", // the reused arg slot
    " xor %ebx, %ebx",
    " xor %esi, %esi",
    " xor %edi, %edi",
    " xor %ebp, %ebp",
    " mov (%ebx), %eax", // the bad read
    " pop %edi",
    " pop %esi",
    " pop %ebp",
    " pop %ebx",
    " ret $0xc",
    // run(this, seat, out): out = {ret, len, off, ebx after, esi after}
    ".globl _run",
    "_run:",
    " push %ebx",
    " push %esi",
    " push %edi",
    " push %ebp",
    " mov 20(%esp), %ebx",
    " mov 24(%esp), %esi",
    " mov 28(%esp), %edi",
    " sub $0x40, %esp",
    " movl $5, 0xC(%esp)",
    " movl $7, 0x10(%esp)",
    " lea 0x10(%esp), %eax",
    " push %eax",
    " lea 0x2C(%esp), %eax",
    " push %eax",
    " lea 0x14(%esp), %eax",
    " push %eax",
    " mov %esi, %eax",
    " mov %ebx, %ecx",
    " call _victim",
    ".globl _after_call",
    "_after_call:",
    " mov %eax, (%edi)",
    " mov 0xC(%esp), %eax",
    " mov %eax, 4(%edi)",
    " mov 0x10(%esp), %eax",
    " mov %eax, 8(%edi)",
    " mov %ebx, 12(%edi)",
    " mov %esi, 16(%edi)",
    " add $0x40, %esp",
    " pop %ebp",
    " pop %edi",
    " pop %esi",
    " pop %ebx",
    " ret",
    options(att_syntax)
);

unsafe extern "C" {
    fn run(this: *mut u32, seat: u32, out: *mut u32);
    static after_call: u8;
}

fn check(name: &str, pend: u32, want_count: u32) -> bool {
    let (mut counts, mut pends, mut this, mut out) = ([0u32; 8], [0u32; 8], [0u32; 19], [0u32; 5]);
    counts[2] = 44571;
    pends[2] = pend;
    this[17] = counts.as_mut_ptr() as u32;
    this[18] = pends.as_mut_ptr() as u32;
    // SAFETY: `run` builds control's frame on its own stack; the guard writes only `out` and `counts`
    unsafe { run(this.as_mut_ptr(), 2, out.as_mut_ptr()) };
    // SAFETY: written behind the compiler's back, through the address in this[17]
    let count = unsafe { std::ptr::read_volatile(&counts[2]) };
    let ebx_ok = out[3] == this.as_ptr() as u32;
    let ok =
        out[0] == 1 && out[1] == 0 && out[2] == 0 && ebx_ok && out[4] == 2 && count == want_count;
    println!(
        "{} {name}: ret={} len={} off={} ebx={} esi={} count={count}",
        if ok { "PASS" } else { "FAIL" },
        out[0],
        out[1],
        out[2],
        if ebx_ok { "this" } else { "WRONG" },
        out[4]
    );
    ok
}

fn main() -> ExitCode {
    // SAFETY: this program's own module and a label in its code
    let off =
        unsafe { &raw const after_call as usize - GetModuleHandleA(std::ptr::null()) as usize };
    // SAFETY: one thread so far
    unsafe { std::env::set_var("MXHOOK_MSGGUARD_RET", format!("{off:x}")) };
    // SAFETY: a NUL-terminated name
    if unsafe { LoadLibraryA(c"winmm.dll".as_ptr().cast()) }.is_null() {
        println!("FAIL: winmm.dll did not load");
        return ExitCode::from(1);
    }
    let ok = check("no buffer left", 0, 0) & check("buffer kept", 0x1234, 44571);
    println!("{}", if ok { "PASS" } else { "FAIL" });
    ExitCode::from(u8::from(!ok))
}
