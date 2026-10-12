//! What the debug loop works out from a debuggee's memory, as plain functions over a read function, so the recordings
//! of debugger.py (tests/rec/debug.jsonl) replay them without a process.  `Read` is ReadProcessMemory: all `n` bytes at
//! an address, or nothing.

use windows_sys::Win32::Globalization::{MB_ERR_INVALID_CHARS, MultiByteToWideChar};

/// Reads `n` bytes of the debuggee at an address: all of them, or None.
pub type Read<'a> = &'a dyn Fn(u64, usize) -> Option<Vec<u8>>;

fn dword(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

// ---- control's message-reassembly crash guard (2026-10-08) -----------------------------------------------------------
// control_Release crashes while reassembling a per-connection message in FUN_00412860: it trusts a length/offset that
// came from the wire and was not bounds-checked, so a malformed or awkwardly split message makes that function touch
// memory far outside its buffer and the one-thread server dies, taking every cabinet and the projector with it (Error
// 3000).  Seen four times on 2026-10-08: twice the READ `movq xmm0,[ecx+esi]` at control+0x129C2, once a WRITE inside
// msvcr90 memcpy (~1 GB count) called from control+0x12A3D - the same bug, another instruction.  So no one instruction
// is guarded: on ANY first-chance access violation the loop looks for FUN_00412860's return into its caller on the
// stack (control+0x1226A, after the one call to FUN_00412860 in FUN_00412140).  If it is there, the fault happened while
// FUN_00412860 ran (itself or a memcpy/CRT call it made), so the whole function is unwound: the four callee-saved
// registers it pushed restored, the remaining length (*param_1) and offset (*param_3) zeroed so control abandons the bad
// chunk, and the thread resumed at the caller with eax=1 ("message consumed").  ASLR is off in control (ImageBase
// 0x400000).  Only this signature in control is touched; every other fault is left as it was.

/// control+0x1226A: FUN_00412140's instruction right after it calls FUN_00412860.
pub const MSGPARSE_RET_OFF: u32 = 0x1226A;
/// Bytes of stack scanned up from ESP for the return address.
pub const MSGPARSE_STACK_SCAN: usize = 0x120;
/// FUN_00412140's frame holds a ~512 KB (0x80000) receive buffer: its &len and &off live within this span.
const MSGPARSE_FRAME_SPAN: u64 = 0x90000;

/// How the loop makes FUN_00412860 return cleanly: the registers to set, the two dwords to zero.
#[derive(Debug, PartialEq, Eq)]
pub struct Recovery {
    pub edi: u32,
    pub esi: u32,
    pub ebp: u32,
    pub ebx: u32,
    pub eip: u32,
    pub esp: u32,
    pub eax: u32,
    /// The remaining length and the offset: zeroed so control drops the chunk.
    pub p_len: u32,
    pub p_off: u32,
    /// Where on the stack (from ESP) the return address was found.
    pub at: u32,
}

/// debugger.py's plan_412860_recovery: did this access violation happen while control's FUN_00412860 was on the stack,
/// and if so, how does it return cleanly?  The stack is scanned up from ESP for its return into the caller
/// (`base + ret_off`); at the slot A that holds it, FUN_00412860's prologue (push ebx/ebp/esi/edi) left the caller's
/// registers just below - ebx=[A-4] ebp=[A-8] esi=[A-12] edi=[A-16] - and its 3 arguments just above - &len=[A+4]
/// buf=[A+8] &off=[A+0xc].  A clean `ret 0xc` restores those registers, pops to the return address and drops the
/// arguments (esp -> A+0x10).  The argument pointers must point into the caller's frame (above A), or the slot is a
/// stale value, not the live frame.
pub fn plan_412860_recovery(esp: u32, read: Read, base: u32, ret_off: u32) -> Option<Recovery> {
    let ret140 = base.wrapping_add(ret_off);
    let stack = read(u64::from(esp), MSGPARSE_STACK_SCAN)?;
    if stack.len() < MSGPARSE_STACK_SCAN {
        return None;
    }
    // From 0x10: room for the 4 saved registers below the return slot.
    for off in (0x10..stack.len() - 0x10).step_by(4) {
        if dword(&stack, off) != ret140 {
            continue;
        }
        let a = esp.wrapping_add(off as u32);
        let (edi, esi, ebp, ebx) = (
            dword(&stack, off - 0x10),
            dword(&stack, off - 0xc),
            dword(&stack, off - 8),
            dword(&stack, off - 4),
        );
        let (p_len, p_off) = (dword(&stack, off + 4), dword(&stack, off + 0xc));
        let live = |p: u32| u64::from(a) < u64::from(p) && u64::from(p) <= u64::from(a) + MSGPARSE_FRAME_SPAN;
        if !(live(p_len) && live(p_off)) {
            continue; // not the live frame's arguments: keep scanning
        }
        return Some(Recovery {
            edi,
            esi,
            ebp,
            ebx,
            eip: ret140,
            esp: a.wrapping_add(0x10),
            eax: 1,
            p_len,
            p_off,
            at: off as u32,
        });
    }
    None
}

/// Why the guard found no plan, for the log: the stack it read, where the return address was (and the 3 dwords after
/// it), the first 16 dwords.
pub fn no_plan_note(esp: u32, stack: &[u8], ret: u32) -> String {
    let dw: Vec<u32> = stack
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    let hex = |v: &[u32]| v.iter().map(|v| format!("{v:08X}")).collect::<Vec<_>>().join(" ");
    let hits: Vec<String> = (0..dw.len())
        .filter(|&i| dw[i] == ret)
        .take(3)
        .map(|i| {
            format!(
                "+0x{:X} (then {})",
                i * 4,
                hex(&dw[(i + 1).min(dw.len())..(i + 4).min(dw.len())])
            )
        })
        .collect();
    format!(
        "no plan: {} stack bytes at ESP 0x{esp:08X}; return 0x{ret:08X} at {}; stack: {}",
        stack.len(),
        if hits.is_empty() {
            "nowhere".to_owned()
        } else {
            hits.join(", ")
        },
        hex(&dw[..dw.len().min(16)])
    )
}

/// The 7 bytes before an address end in a CALL instruction (so the address is a return address).
pub fn looks_like_return(b: &[u8]) -> bool {
    if b.len() < 7 {
        return false;
    }
    b[2] == 0xE8                                                      // call rel32
        || (b[1] == 0xFF && (b[2] == 0x15 || (b[2] & 0xF8) == 0x90))  // call [abs] / [reg+disp32]
        || (b[3] == 0xFF && b[4] == 0x54)                             // call [esp+disp8]
        || (b[4] == 0xFF && (b[5] & 0xF8) == 0x50)                    // call [reg+disp8]
        || (b[5] == 0xFF && ((b[6] & 0xF8) == 0xD0 || (b[6] & 0xF8) == 0x10)) // call reg / [reg]
}

/// Bytes as ASCII for the log (it is not UTF-8): printable characters as they are, any other byte (and `\`) as \xNN.
pub fn esc(b: &[u8]) -> String {
    b.iter()
        .map(|&c| {
            if (32..127).contains(&c) && c != b'\\' {
                char::from(c).to_string()
            } else {
                format!("\\x{c:02x}")
            }
        })
        .collect()
}

/// Shift-JIS text (code page 932, as Python's "cp932" reads it) to a string; `strict`: None if any byte is invalid,
/// else an invalid byte becomes U+FFFD.  Windows' code page 932 reads the double-byte characters; the single bytes
/// Windows refuses but Python's codec takes (0x80, 0xA0, 0xFD-0xFF) map as Python maps them.
pub fn cp932(b: &[u8], strict: bool) -> Option<String> {
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let single = match c {
            0..=0x80 => Some(char::from(c)),
            0xA0 => Some('\u{F8F0}'),
            0xA1..=0xDF => char::from_u32(0xFF61 + u32::from(c - 0xA1)),
            0xFD..=0xFF => char::from_u32(0xF8F1 + u32::from(c - 0xFD)),
            _ => None,
        };
        if let Some(ch) = single {
            out.push(ch);
            i += 1;
            continue;
        }
        let pair = b.get(i..i + 2).and_then(|p| {
            let mut w = [0u16; 2];
            // SAFETY: both buffers are live and their lengths passed.
            let n = unsafe { MultiByteToWideChar(932, MB_ERR_INVALID_CHARS, p.as_ptr(), 2, w.as_mut_ptr(), 2) };
            (n == 1).then(|| String::from_utf16_lossy(&w[..1]))
        });
        match pair {
            Some(s) => {
                out.push_str(&s);
                i += 2;
            }
            None if strict => return None,
            None => {
                out.push('\u{FFFD}');
                // a trail byte outside the trail range is a character of its own
                let trail = b.get(i + 1).is_some_and(|&t| (0x40..=0xFC).contains(&t) && t != 0x7F);
                i += if trail { 2 } else { 1 };
            }
        }
    }
    Some(out)
}

/// Up to `n` bytes from an address, page by page, stopping at the first page that cannot be read (the stack's top): one
/// read of the whole range would fail entirely there.
pub fn rd_upto(read: Read, addr: u64, n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    while out.len() < n {
        let a = addr + out.len() as u64;
        let want = (n - out.len()).min(0x1000 - (a & 0xFFF) as usize);
        match read(a, want) {
            Some(b) if !b.is_empty() => {
                let short = b.len() < want;
                out.extend_from_slice(&b);
                if short {
                    break;
                }
            }
            _ => break,
        }
    }
    out
}

/// The NUL-ended text at an address, or None when it does not look like text: under 3 bytes, a control character other
/// than tab / CR / LF, or not valid Shift-JIS.
pub fn text_at(read: Read, va: u64) -> Option<Vec<u8>> {
    if va < 0x10000 {
        return None;
    }
    let mut s = rd_upto(read, va, 200);
    if let Some(end) = s.iter().position(|&c| c == 0) {
        s.truncate(end);
    }
    if s.len() < 3 || s.iter().any(|&c| c < 32 && ![9, 10, 13].contains(&c)) {
        return None;
    }
    cp932(&s, true)?;
    Some(s)
}

/// The end of the image at `base` (base + SizeOfImage), or base + 0x1000 when its header cannot be read.
pub fn image_end(read: Read, base: u64) -> u64 {
    let Some(hdr) = read(base, 0x400).filter(|h| h.starts_with(b"MZ")) else {
        return base + 0x1000;
    };
    let lfa = dword(&hdr, 0x3C) as usize;
    if lfa + 0x54 > hdr.len() {
        return base + 0x1000;
    }
    base + u64::from(dword(&hdr, lfa + 0x50))
}

/// The address of the export `want` of the 32-bit DLL at `base`, read from its memory (the names are sorted: a binary
/// search); None if absent or unreadable.
pub fn export_va(read: Read, base: u64, want: &str) -> Option<u64> {
    let hdr = read(base, 0x400).filter(|h| h.starts_with(b"MZ"))?;
    let lfa = dword(&hdr, 0x3C) as usize;
    if lfa + 0x80 > hdr.len() {
        return None;
    }
    let ed = read(base + u64::from(dword(&hdr, lfa + 0x78)), 40).filter(|e| e.len() >= 40)?;
    let (count, funcs, names, ords) = (dword(&ed, 0x18), dword(&ed, 0x1c), dword(&ed, 0x20), dword(&ed, 0x24));
    let key = want.as_bytes();
    let (mut lo, mut hi) = (0i64, i64::from(count) - 1);
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let ptr = read(base + u64::from(names) + 4 * mid as u64, 4)?;
        let mut name = read(base + u64::from(dword(&ptr, 0)), 128)?;
        if let Some(end) = name.iter().position(|&c| c == 0) {
            name.truncate(end);
        }
        match name.as_slice().cmp(key) {
            std::cmp::Ordering::Equal => {
                let o = read(base + u64::from(ords) + 2 * mid as u64, 2)?;
                let ord = u64::from(u16::from_le_bytes([o[0], o[1]]));
                let f = read(base + u64::from(funcs) + 4 * ord, 4)?;
                return Some(base + u64::from(dword(&f, 0)));
            }
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid - 1,
        }
    }
    None
}

/// A function index of one program (from Ghidra): sorted start addresses, names, body sizes.
#[derive(Debug, Default)]
pub struct Index {
    pub starts: Vec<u64>,
    pub names: Vec<String>,
    pub sizes: Vec<u64>,
}

impl Index {
    /// From `_index.csv` (columns addr in hex, full_name, body_bytes); empty on any error.
    pub fn load(path: &std::path::Path) -> Option<Self> {
        let mut rows = Vec::new();
        for r in csv::Reader::from_path(path)
            .ok()?
            .deserialize::<std::collections::HashMap<String, String>>()
        {
            let r = r.ok()?;
            let addr = r.get("addr")?;
            let addr = u64::from_str_radix(addr.trim_start_matches("0x").trim_start_matches("0X"), 16).ok()?;
            let size = r
                .get("body_bytes")
                .filter(|s| !s.is_empty())
                .map_or(Some(0), |s| s.parse().ok())?;
            rows.push((addr, r.get("full_name")?.clone(), size));
        }
        rows.sort();
        Some(Self {
            starts: rows.iter().map(|r| r.0).collect(),
            names: rows.iter().map(|r| r.1.clone()).collect(),
            sizes: rows.iter().map(|r| r.2).collect(),
        })
    }
}

/// Names an address: a function of the program's index (+offset, " (?)" past its body), else the program or a DLL
/// +offset, else "?".
pub fn name_of(va: u64, index: &Index, base: u64, end: u64, name: &str, dlls: &[(u64, u64, String)]) -> String {
    if base <= va && va < end {
        let i = index.starts.partition_point(|&s| s <= va);
        if i > 0 {
            let off = va - index.starts[i - 1];
            let past = if off < index.sizes[i - 1] { "" } else { " (?)" };
            return format!("{}+0x{off:X}{past}", index.names[i - 1]);
        }
        return format!("{name}+0x{:X}", va - base);
    }
    dlls.iter()
        .find(|(b, e, _)| *b <= va && va < *e)
        .map_or_else(|| "?".to_owned(), |(b, _, nm)| format!("{nm}+0x{:X}", va - b))
}
