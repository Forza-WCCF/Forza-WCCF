//! Wine-side motion/scene provider and debugger. No Python runtime required.
// nested ifs kept as written when the helper ran on the servers
#![allow(clippy::collapsible_if)]
use std::{
    collections::HashMap,
    ffi::{CString, c_void},
    fs, io,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    ptr, thread,
    time::Duration,
};
type Handle = *mut c_void;
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileMappingA(
        file: Handle,
        attrs: Handle,
        protect: u32,
        hi: u32,
        size: u32,
        name: *const i8,
    ) -> Handle;
    fn MapViewOfFile(handle: Handle, access: u32, hi: u32, lo: u32, size: usize) -> Handle;
    fn UnmapViewOfFile(view: Handle) -> i32;
    fn CreateMutexA(attrs: Handle, owned: i32, name: *const i8) -> Handle;
    fn WaitForSingleObject(handle: Handle, ms: u32) -> u32;
    fn ReleaseMutex(handle: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetLastError() -> u32;
    fn WaitForDebugEvent(event: *mut DebugEvent, ms: u32) -> i32;
    fn ContinueDebugEvent(pid: u32, tid: u32, status: u32) -> i32;
    fn ReadProcessMemory(
        handle: Handle,
        address: Handle,
        buffer: Handle,
        size: usize,
        read: *mut usize,
    ) -> i32;
    fn QueryFullProcessImageNameA(handle: Handle, flags: u32, path: *mut u8, len: *mut u32) -> i32;
    fn TerminateProcess(handle: Handle, code: u32) -> i32;
    fn GetThreadContext(thread: Handle, context: *mut Context32) -> i32;
    fn SetThreadContext(thread: Handle, context: *const Context32) -> i32;
    fn WriteProcessMemory(
        handle: Handle,
        address: Handle,
        buffer: *const u8,
        size: usize,
        written: *mut usize,
    ) -> i32;
    fn FlushInstructionCache(handle: Handle, address: Handle, size: usize) -> i32;
    fn VirtualProtectEx(
        handle: Handle,
        address: Handle,
        size: usize,
        protect: u32,
        old: *mut u32,
    ) -> i32;
    fn GetFinalPathNameByHandleA(handle: Handle, path: *mut u8, len: u32, flags: u32) -> u32;
}
#[repr(C)]
struct Context32 {
    words: [u32; 179],
}
const _: () = assert!(std::mem::size_of::<Context32>() == 716);
#[repr(C)]
struct DebugEvent {
    code: u32,
    pid: u32,
    tid: u32,
    data: [u32; 21],
}

fn error() -> io::Error {
    // SAFETY: GetLastError takes no arguments and only reads this thread's last error
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}
fn mapping(name: &str, data: &[u8]) -> io::Result<Handle> {
    let name = CString::new(name).unwrap();
    // SAFETY: the name is a live NUL-terminated string; the view is checked, written within data.len() and unmapped
    unsafe {
        let h = CreateFileMappingA(
            -1isize as Handle,
            ptr::null_mut(),
            4,
            0,
            u32::try_from(data.len()).map_err(|_| io::Error::other("mapping too large"))?,
            name.as_ptr(),
        );
        if h.is_null() {
            return Err(error());
        }
        if GetLastError() == 183 {
            CloseHandle(h);
            return Err(io::Error::other("mapping already exists; another runtime?"));
        }
        let view = MapViewOfFile(h, 0xF001F, 0, 0, data.len());
        if view.is_null() {
            CloseHandle(h);
            return Err(error());
        }
        ptr::copy_nonoverlapping(data.as_ptr(), view.cast::<u8>(), data.len());
        UnmapViewOfFile(view);
        Ok(h)
    }
}
fn sized(data: Vec<u8>) -> Vec<u8> {
    let mut out = (data.len() as u32).to_le_bytes().to_vec();
    out.extend(data);
    out
}
fn motion(game: &Path) -> io::Result<Vec<Handle>> {
    let mut handles = Vec::new();
    for (name, path) in [
        (
            "game_motion_data.moai",
            "AI/motion_data/game_motion_data.moai",
        ),
        (
            "management_motion_data.moai",
            "data/motion/launcherMotion/management_motion_data.moai",
        ),
        (
            "scene_motion_data.moai",
            "data/motion/launcherMotion/scene_motion_data.moai",
        ),
    ] {
        let data = sized(fs::read(game.join(path))?);
        handles.push(mapping(&format!("SBWG_{name}"), &data)?);
        println!("mapping SBWG_{name}: {} bytes", data.len());
    }
    let mut body = vec![0; 16];
    for (i, name) in [
        "game_motion_table_moai.bin",
        "game_actionsetup_table_moai.bin",
        "game_movepack_table_moai.bin",
        "game_request_table_moai.bin",
    ]
    .iter()
    .enumerate()
    {
        let offset = body.len() as u32;
        body[i * 4..i * 4 + 4].copy_from_slice(&offset.to_le_bytes());
        body.extend(fs::read(game.join("AI/motion_data").join(name))?);
        while !body.len().is_multiple_of(4) {
            body.push(0);
        }
    }
    handles.push(mapping("SBWG_moSuppl", &body)?);
    let mut status = [0u8; 64];
    status[0] = 1;
    handles.push(mapping("SBWG_status", &status)?);
    Ok(handles)
}
fn scenes(game: PathBuf) -> io::Result<()> {
    let mut index = HashMap::new();
    for entry in fs::read_dir(game.join("data/motion/launcherMotion/xscn"))? {
        let path = entry?.path();
        let name = path.file_name().unwrap().to_string_lossy();
        if name.ends_with(".xscn") {
            if let Ok(n) = name
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
            {
                index.insert(n, path);
            }
        }
    }
    let mut board = vec![0u8; 0x5CC];
    board[4..8].copy_from_slice(&16u32.to_le_bytes());
    board[8..12].copy_from_slice(&32u32.to_le_bytes());
    let h = mapping("xscnWatch::ComBuf", &board)?;
    // SAFETY: the mutex name is NUL-terminated; the view covers board.len() bytes of the mapping made just above
    unsafe {
        let name = CString::new("xscnWatch::Mutex").unwrap();
        let mutex = CreateMutexA(ptr::null_mut(), 0, name.as_ptr());
        let view = MapViewOfFile(h, 0xF001F, 0, 0, board.len()).cast::<u32>();
        if mutex.is_null() || view.is_null() {
            return Err(error());
        }
        println!("scene provider ready: {} scenes", index.len());
        let mut loaded: Vec<(u32, Handle)> = Vec::new();
        loop {
            let wait = WaitForSingleObject(mutex, 250);
            if wait != 0 && wait != 0x80 {
                continue;
            }
            let mut pending = Vec::new();
            for i in 0..16 {
                let slot = view.add(3 + i * 3);
                let typ = ptr::read_volatile(slot);
                let pid = ptr::read_volatile(slot.add(1));
                let scene = ptr::read_volatile(slot.add(2));
                if typ == 1 && pid != 0 && scene > 0 && scene < 0x80000000 {
                    pending.push(scene);
                }
                ptr::write_bytes(slot, 0, 3);
            }
            ptr::write_bytes(view.add(0xCC / 4), 0, 32 * 10);
            for (i, (scene, _)) in loaded.iter().take(32).enumerate() {
                ptr::write_volatile(view.add(0xCC / 4 + i * 10), *scene);
                ptr::write_volatile(view.add(0xCC / 4 + i * 10 + 1), 2);
            }
            ReleaseMutex(mutex);
            // ponytail: cache the finite shipped scene set; add eviction only if memory measurements require it.
            for scene in pending {
                if loaded.iter().any(|(n, _)| *n == scene) {
                    continue;
                }
                let path = index
                    .get(&scene)
                    .ok_or_else(|| io::Error::other(format!("missing scene {scene}")))?;
                let data = sized(fs::read(path)?);
                loaded.push((scene, mapping(&format!("{scene:06}.xscn"), &data)?));
                println!("scene {scene} loaded: {} bytes", data.len());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}
fn image(handle: Handle) -> String {
    let mut path = [0u8; 1024];
    let mut n = path.len() as u32;
    // SAFETY: the buffer and its length go together; the call writes at most n bytes
    unsafe {
        if QueryFullProcessImageNameA(handle, 0, path.as_mut_ptr(), &mut n) == 0 {
            return String::new();
        }
    }
    String::from_utf8_lossy(&path[..n as usize]).to_string()
}
fn read_memory(process: Handle, address: u32, size: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; size];
    let mut n = 0;
    // SAFETY: the local buffer holds `size` bytes, the length passed; a bad remote address only fails the call
    unsafe {
        ReadProcessMemory(
            process,
            address as Handle,
            bytes.as_mut_ptr().cast(),
            size,
            &mut n,
        );
    }
    bytes.truncate(n);
    bytes
}
fn write_memory(process: Handle, address: u32, data: &[u8]) -> bool {
    let mut n = 0;
    // SAFETY: only the debugged process's memory changes; data's pointer and length go together
    unsafe {
        let mut old = 0;
        if VirtualProtectEx(process, address as Handle, data.len(), 0x40, &mut old) == 0 {
            return false;
        }
        let ok = WriteProcessMemory(
            process,
            address as Handle,
            data.as_ptr(),
            data.len(),
            &mut n,
        );
        FlushInstructionCache(process, address as Handle, data.len());
        let mut previous = 0;
        let restored = VirtualProtectEx(process, address as Handle, data.len(), old, &mut previous);
        ok != 0 && n == data.len() && restored != 0
    }
}
fn dword(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}
fn export(process: Handle, base: u32, want: &[u8]) -> Option<u32> {
    let hdr = read_memory(process, base, 1024);
    if hdr.get(..2)? != b"MZ" {
        return None;
    }
    let nt = dword(&hdr, 0x3c)? as usize;
    if dword(&hdr, nt + 24)? & 0xffff != 0x10b {
        return None;
    }
    let rva = dword(&hdr, nt + 0x78)?;
    if rva == 0 {
        return None;
    }
    let dir = read_memory(process, base + rva, 40);
    let count = dword(&dir, 24)?.min(4096);
    let funcs = dword(&dir, 28)?;
    let names = dword(&dir, 32)?;
    let ords = dword(&dir, 36)?;
    for i in 0..count {
        let rva = dword(&read_memory(process, base + names + i * 4, 4), 0)?;
        let name = read_memory(process, base + rva, want.len() + 1);
        if name.get(..want.len()) == Some(want) && name.get(want.len()) == Some(&0) {
            let ord = read_memory(process, base + ords + i * 2, 2);
            let ord = u16::from_le_bytes(ord.get(..2)?.try_into().ok()?) as u32;
            return Some(base + dword(&read_memory(process, base + funcs + ord * 4, 4), 0)?);
        }
    }
    None
}
fn file_name(handle: Handle) -> String {
    let mut path = [0u8; 1024];
    // SAFETY: the buffer and its length go together
    let n = unsafe { GetFinalPathNameByHandleA(handle, path.as_mut_ptr(), path.len() as u32, 0) };
    if n == 0 || n as usize >= path.len() {
        return String::new();
    }
    String::from_utf8_lossy(&path[..n as usize]).to_string()
}
fn dump(process: Handle, thread: Handle, pid: u32, modules: &[(u32, String)]) -> Option<Context32> {
    let mut context = Context32 { words: [0; 179] };
    context.words[0] = 0x10007;
    // SAFETY: context is a CONTEXT-sized buffer with ContextFlags set, for a thread stopped at a debug event
    if unsafe { GetThreadContext(thread, &mut context) } == 0 {
        eprintln!("context failed pid={pid}: {}", error());
        return None;
    }
    let c = &context.words;
    eprintln!(
        "CRT DIAGNOSTIC pid={pid} EIP={:08x} EAX={:08x} EBX={:08x} ECX={:08x} EDX={:08x} EBP={:08x} ESP={:08x}",
        c[46], c[44], c[41], c[43], c[42], c[45], c[49]
    );
    for (base, name) in modules {
        eprintln!("MODULE {base:08x} {name}");
    }
    let stack = read_memory(process, c[49], 0x2000);
    for (i, bytes) in stack.as_chunks::<4>().0.iter().enumerate().take(1024) {
        let addr = u32::from_le_bytes(*bytes);
        if (0x400000..0x800000).contains(&addr) || i < 48 {
            eprintln!("STACK +{:04x} = {addr:08x}", i * 4);
        }
        if (0x10000..0x70000000).contains(&addr) {
            let text = read_memory(process, addr, 160);
            let text = text.split(|&c| c == 0).next().unwrap_or_default();
            if text.len() >= 3
                && text
                    .iter()
                    .all(|&b| (0x20..0x7f).contains(&b) || b == b'\n' || b == b'\r' || b == b'\t')
            {
                eprintln!(
                    "TEXT +{:04x} -> {addr:08x} {:?}",
                    i * 4,
                    String::from_utf8_lossy(text)
                );
            }
        }
    }
    Some(context)
}
pub fn main() -> io::Result<()> {
    let game = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| io::Error::other("usage: helper GAME"))?,
    );
    let fixture = std::env::args().nth(2);
    let _maps = if fixture.is_none() {
        motion(&game)?
    } else {
        Vec::new()
    };
    if fixture.is_none() {
        let scene_game = game.clone();
        thread::spawn(move || {
            if let Err(e) = scenes(scene_game) {
                eprintln!("scene provider failed: {e}");
                std::process::exit(1);
            }
        });
        thread::sleep(Duration::from_millis(200));
    }
    let target = fixture.as_deref().unwrap_or("control_Release.exe");
    let mut child = Command::new(game.join(target))
        .current_dir(&game)
        .creation_flags(1)
        .spawn()?;
    println!("controller Win32 PID {}", child.id());
    let mut processes: HashMap<u32, Handle> = HashMap::new();
    let mut threads: HashMap<u32, Handle> = HashMap::new();
    let mut modules: HashMap<u32, Vec<(u32, String)>> = HashMap::new();
    let mut watson: HashMap<u32, (u32, u8)> = HashMap::new();
    let mut event = DebugEvent {
        code: 0,
        pid: 0,
        tid: 0,
        data: [0; 21],
    };
    let root = game
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
        + "\\";
    // SAFETY: the debug loop: event is a DEBUG_EVENT-sized buffer the calls fill; handles come from those events
    unsafe {
        loop {
            if WaitForDebugEvent(&mut event, 500) == 0 {
                continue;
            }
            let mut status = 0x00010002;
            let d = event.data;
            match event.code {
                3 => {
                    let h = d[1] as Handle;
                    let path = image(h);
                    println!("process started pid={} path={path}", event.pid);
                    if path.to_ascii_lowercase().ends_with("\\control_release.exe") {
                        // The shared logger copies this child argument into char[12].
                        // Keep mode 2/seat 0; shorten only its diagnostic label.
                        let address = d[3] + 0x1b03f0;
                        let original = b"Match[Control]\0";
                        let replacement = b"Match[Ctl]\0\0\0\0\0";
                        if read_memory(h, address, original.len()) != original
                            || !write_memory(h, address, replacement)
                            || read_memory(h, address, replacement.len()) != replacement
                        {
                            TerminateProcess(h, 1);
                            return Err(io::Error::other(
                                "REFUSED: controller log label preimage/write check failed",
                            ));
                        }
                        eprintln!(
                            "FIX: checked controller child log label Match[Control] -> Match[Ctl] (char[12]); game modes unchanged"
                        );
                    }
                    processes.insert(event.pid, h);
                    modules.entry(event.pid).or_default().push((d[3], image(h)));
                    if d[0] != 0 {
                        CloseHandle(d[0] as Handle);
                    }
                    if d[2] != 0 {
                        threads.insert(event.tid, d[2] as Handle);
                    }
                }
                2 => {
                    if d[0] != 0 {
                        threads.insert(event.tid, d[0] as Handle);
                    }
                }
                4 => {
                    if let Some(h) = threads.remove(&event.tid) {
                        CloseHandle(h);
                    }
                }
                6 => {
                    let name = file_name(d[0] as Handle);
                    modules
                        .entry(event.pid)
                        .or_default()
                        .push((d[1], name.clone()));
                    if name.to_ascii_lowercase().ends_with("\\msvcr90.dll") {
                        eprintln!("CRT module pid={} base={:08x} path={name}", event.pid, d[1]);
                        if let Some(&process) = processes.get(&event.pid) {
                            if let Some(va) = export(process, d[1], b"_invoke_watson") {
                                if let Some(&old) = read_memory(process, va, 1).first() {
                                    if write_memory(process, va, &[0xcc]) {
                                        watson.insert(event.pid, (va, old));
                                        eprintln!(
                                            "Watson capture armed pid={} address={va:08x}",
                                            event.pid
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if d[0] != 0 {
                        CloseHandle(d[0] as Handle);
                    }
                }
                8 => {
                    if let Some(h) = processes.get(&event.pid) {
                        let unicode = d[1] & 0xFFFF != 0;
                        let len = ((d[1] >> 16) as usize * if unicode { 2 } else { 1 }).min(8192);
                        let mut bytes = vec![0u8; len];
                        let mut n = 0;
                        if ReadProcessMemory(
                            *h,
                            d[0] as Handle,
                            bytes.as_mut_ptr().cast(),
                            len,
                            &mut n,
                        ) != 0
                        {
                            bytes.truncate(n);
                            let text = if unicode {
                                String::from_utf16_lossy(
                                    &bytes
                                        .as_chunks::<2>()
                                        .0
                                        .iter()
                                        .map(|b| u16::from_le_bytes(*b))
                                        .collect::<Vec<_>>(),
                                )
                            } else {
                                String::from_utf8_lossy(&bytes).to_string()
                            };
                            let text = text.trim_end_matches(['\0', '\r', '\n']);
                            if !text.starts_with("inst(") {
                                println!("pid={} {text}", event.pid);
                            }
                        }
                    }
                }
                1 => {
                    if d[0] == 0x80000003
                        && watson
                            .get(&event.pid)
                            .is_some_and(|&(va, _)| d[3] == va || d[3] == va + 1)
                    {
                        if let (Some(&process), Some(&thread), Some((va, old))) = (
                            processes.get(&event.pid),
                            threads.get(&event.tid),
                            watson.remove(&event.pid),
                        ) {
                            let mut context = dump(
                                process,
                                thread,
                                event.pid,
                                modules
                                    .get(&event.pid)
                                    .map(Vec::as_slice)
                                    .unwrap_or_default(),
                            )
                            .ok_or_else(error)?;
                            if !write_memory(process, va, &[old]) {
                                return Err(error());
                            }
                            context.words[46] = va;
                            if SetThreadContext(thread, &context) == 0 {
                                return Err(error());
                            }
                        }
                    } else if ![0x80000003, 0x4000001F].contains(&d[0]) {
                        status = 0x80010001;
                        if d[0] == 0xc0000417 {
                            if let (Some(&process), Some(&thread)) =
                                (processes.get(&event.pid), threads.get(&event.tid))
                            {
                                dump(
                                    process,
                                    thread,
                                    event.pid,
                                    modules
                                        .get(&event.pid)
                                        .map(Vec::as_slice)
                                        .unwrap_or_default(),
                                );
                            }
                        }
                        if d[20] == 0 {
                            eprintln!(
                                "FATAL pid={} code={:#x} address={:#x}",
                                event.pid, d[0], d[3]
                            );
                        }
                    }
                }
                5 => {
                    println!("process exited pid={} code={:#x}", event.pid, d[0]);
                    if let Some(h) = processes.remove(&event.pid) {
                        CloseHandle(h);
                    }
                    if event.pid == child.id() {
                        ContinueDebugEvent(event.pid, event.tid, status);
                        for h in processes.values() {
                            if image(*h).to_ascii_lowercase().starts_with(&root) {
                                TerminateProcess(*h, 1);
                            }
                        }
                        let _ = child.wait();
                        std::process::exit(if d[0] == 0 { 0 } else { 1 });
                    }
                }
                _ => {}
            }
            if ContinueDebugEvent(event.pid, event.tid, status) == 0 {
                return Err(error());
            }
        }
    }
}
