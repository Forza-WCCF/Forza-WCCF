//! `cap <root> <outdir>`: save every visible top-level window of the game processes (`client_Release.exe`,
//! `control_Release.exe`) whose exe lies under `root` as `<kind>-<pid>.png`, one line per capture naming the method.
//! Windows.Graphics.Capture on the window handle (`wgc`) takes the window's own composed content, Direct3D included;
//! PrintWindow with PW_RENDERFULLCONTENT (`printwindow`, blank for the game's Direct3D 9 windows) is the fallback when
//! WGC is unavailable. Both read one window, never the screen, so other apps never end up in a capture.
//! Replaces research/cap.ps1. Windows only; `cargo xtask cap` builds and runs it.

mod wgc;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, SelectObject,
};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SW_RESTORE, ShowWindow,
};

const GAMES: [&str; 2] = ["client_Release.exe", "control_Release.exe"];
const PW_RENDERFULLCONTENT: u32 = 2;

/// The capture's name prefix: a client under `\seat1\` is a seat, a control is `control`, any other client the projector.
fn kind(exe: &str, path: &str) -> &'static str {
    if path.to_ascii_lowercase().contains("\\seat1\\") {
        "seat1"
    } else if exe.eq_ignore_ascii_case("control_Release.exe") {
        "control"
    } else {
        "projector"
    }
}

/// Is `path` inside `root` (case-insensitive, whole folder names)?
fn under(root: &str, path: &str) -> bool {
    let root = format!("{}\\", root.trim_end_matches('\\')).to_ascii_lowercase();
    path.to_ascii_lowercase().starts_with(&root)
}

fn wide(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}

/// (pid, exe name, full path) of the running game processes under `root`.
fn games(root: &str) -> Vec<(u32, String, String)> {
    let mut found = vec![];
    // SAFETY: plain Win32 calls on handles and buffers owned here; the entry's dwSize is set before the first call.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let exe = wide(&e.szExeFile);
            if GAMES.iter().any(|g| g.eq_ignore_ascii_case(&exe)) {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID);
                if !h.is_null() {
                    let mut buf = [0u16; 1024];
                    let mut n = buf.len() as u32;
                    if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut n) != 0 {
                        let path = String::from_utf16_lossy(&buf[..n as usize]);
                        if under(root, &path) {
                            found.push((e.th32ProcessID, exe, path));
                        }
                    }
                    CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    found
}

unsafe extern "system" fn collect(h: HWND, l: LPARAM) -> i32 {
    // SAFETY: `l` is the `&mut Vec<(HWND, u32)>` that `windows_of` passes to EnumWindows, alive for the whole call.
    let v = unsafe { &mut *(l as *mut Vec<(HWND, u32)>) };
    let mut pid = 0;
    // SAFETY: `h` comes from EnumWindows.
    unsafe {
        GetWindowThreadProcessId(h, &mut pid);
        if IsWindowVisible(h) != 0 {
            v.push((h, pid));
        }
    }
    1
}

/// Every visible top-level window with its owner pid.
fn windows_of() -> Vec<(HWND, u32)> {
    let mut v: Vec<(HWND, u32)> = vec![];
    // SAFETY: `collect` only uses the pointer during this call.
    unsafe { EnumWindows(Some(collect), &mut v as *mut _ as LPARAM) };
    v
}

/// The window's own pixels as (width, height, RGBA, method), restoring it first when minimized: WGC, else PrintWindow.
fn grab(h: HWND) -> Option<(u32, u32, Vec<u8>, &'static str)> {
    // SAFETY: `h` is a window handle; IsIconic and ShowWindow accept any value.
    unsafe {
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
    }
    match wgc::grab(h) {
        Ok((w, ht, px)) => Some((w, ht, px, "wgc")),
        Err(e) => {
            eprintln!("cap: WGC failed ({e}), PrintWindow instead");
            print_window(h).map(|(w, ht, px)| (w, ht, px, "printwindow"))
        }
    }
}

/// PrintWindow(PW_RENDERFULLCONTENT) into a DIB section: (width, height, RGBA).
fn print_window(h: HWND) -> Option<(u32, u32, Vec<u8>)> {
    // SAFETY: GDI objects are created, used and freed here; the DIB section memory is read only while the bitmap is alive.
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut r);
        let (w, ht) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || ht <= 0 {
            return None;
        }
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -ht, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bmp.is_null() {
            DeleteDC(dc);
            return None;
        }
        let old = SelectObject(dc, bmp);
        let ok = PrintWindow(h, dc, PW_RENDERFULLCONTENT) != 0;
        let mut px = std::slice::from_raw_parts(bits as *const u8, (w * ht * 4) as usize).to_vec();
        SelectObject(dc, old);
        DeleteObject(bmp);
        DeleteDC(dc);
        for p in px.as_chunks_mut::<4>().0 {
            p.swap(0, 2); // BGRA -> RGBA
            p[3] = 255; // PrintWindow leaves alpha at 0
        }
        ok.then_some((w as u32, ht as u32, px))
    }
}

fn title(h: HWND) -> String {
    let mut b = [0u16; 256];
    // SAFETY: the buffer is 256 wide, as passed.
    unsafe { GetWindowTextW(h, b.as_mut_ptr(), 256) };
    wide(&b)
}

fn save(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

/// Capture the game windows under `root` into `out`; one line per capture.
pub fn capture(root: &str, out: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(out)?;
    let procs = games(root);
    let wins = windows_of();
    let mut lines = vec![];
    let mut seen: HashMap<u32, u32> = HashMap::new();
    for (pid, exe, path) in procs {
        for &(h, _) in wins.iter().filter(|w| w.1 == pid) {
            let Some((w, ht, px, how)) = grab(h) else {
                continue;
            };
            let n = seen.entry(pid).and_modify(|n| *n += 1).or_insert(1);
            let k = kind(&exe, &path);
            // a second window of the same process gets a suffix, so no capture overwrites another
            let name = if *n == 1 {
                format!("{k}-{pid}.png")
            } else {
                format!("{k}-{pid}-{n}.png")
            };
            let f: PathBuf = out.join(name);
            save(&f, w, ht, &px)?;
            lines.push(format!(
                "{k} pid {pid} '{}' {w}x{ht} {how} -> {}",
                title(h),
                f.display()
            ));
        }
    }
    Ok(lines)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [root, out] = &a[..] else {
        eprintln!("usage: cap <root> <outdir>");
        std::process::exit(2);
    };
    match capture(root, Path::new(out)) {
        Ok(lines) => lines.iter().for_each(|l| println!("{l}")),
        Err(e) => {
            eprintln!("cap: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            kind("client_Release.exe", r"D:\k\seat1\client_Release.exe"),
            "seat1"
        );
        assert_eq!(
            kind("control_Release.exe", r"D:\k\control_Release.exe"),
            "control"
        );
        assert_eq!(
            kind("client_Release.exe", r"D:\k\client_Release.exe"),
            "projector"
        );
    }

    #[test]
    fn root_match() {
        assert!(under(r"D:\K\", r"d:\k\game\client_Release.exe"));
        assert!(!under(r"D:\k", r"D:\kit2\client_Release.exe"));
    }

    use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, UpdateWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE,
        PeekMessageW, RegisterClassW, WNDCLASSW, WS_POPUP, WS_VISIBLE,
    };

    /// A 240x160 popup window of our own at (x, 100), its class background `bg` (a COLORREF, 0 = none).
    fn test_window(class: &str, x: i32, bg: u32) -> HWND {
        let name: Vec<u16> = class.encode_utf16().chain([0]).collect();
        // SAFETY: the class name outlives both calls; DefWindowProcW handles every message.
        unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                lpszClassName: name.as_ptr(),
                hbrBackground: if bg == 0 {
                    std::ptr::null_mut()
                } else {
                    CreateSolidBrush(bg)
                },
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let h = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                WS_POPUP | WS_VISIBLE,
                x,
                100,
                240,
                160,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!h.is_null(), "CreateWindowExW");
            UpdateWindow(h);
            h
        }
    }

    /// Let the window thread answer its messages for `ms` so the compositor has its content.
    fn pump(ms: u64) {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < end {
            // SAFETY: a zeroed MSG filled by PeekMessageW and dispatched on this, the windows' thread.
            unsafe {
                let mut m: MSG = std::mem::zeroed();
                while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    DispatchMessageW(&m);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// grab() on `h`: the method used and the centre pixel's RGB.
    fn centre(h: HWND) -> (&'static str, [u8; 3]) {
        let (w, ht, px, how) = grab(h).expect("grab");
        let i = ((ht / 2 * w + w / 2) * 4) as usize;
        (how, [px[i], px[i + 1], px[i + 2]])
    }

    /// A GDI-painted window: its brush colour at the centre (RGB(10, 200, 30) is COLORREF 0x1EC80A).
    #[test]
    fn gdi_window() {
        let h = test_window("cap-test-gdi", 100, 0x001E_C80A);
        pump(300);
        let (how, rgb) = centre(h);
        println!("gdi window: {how} {rgb:?}");
        // SAFETY: our own window, destroyed on its thread.
        unsafe { DestroyWindow(h) };
        assert_eq!(rgb, [10, 200, 30], "via {how}");
    }

    /// A window presented by a D3D11 flip-model swap chain (cleared to RGB(51, 102, 204)): the Direct3D path the game's
    /// windows need, where PrintWindow comes back blank.
    #[test]
    fn d3d_window() {
        use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
        };
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC1,
            DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2,
        };
        let h = test_window("cap-test-d3d", 400, 0);
        let (dev, ctx) = wgc::device().expect("D3D11 device");
        // SAFETY: D3D11/DXGI calls on objects created here, for our own window.
        unsafe {
            let f: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: 240,
                Height: 160,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                ..Default::default()
            };
            let sc = f
                .CreateSwapChainForHwnd(
                    &dev,
                    windows::Win32::Foundation::HWND(h),
                    &desc,
                    None,
                    None,
                )
                .unwrap();
            let back: ID3D11Texture2D = sc.GetBuffer(0).unwrap();
            let mut rtv = None;
            dev.CreateRenderTargetView(&back, None, Some(&mut rtv))
                .unwrap();
            ctx.ClearRenderTargetView(rtv.as_ref().unwrap(), &[0.2, 0.4, 0.8, 1.0]);
            sc.Present(0, DXGI_PRESENT(0)).unwrap();
            pump(300);
            let (how, rgb) = centre(h);
            println!("d3d window: {how} {rgb:?}");
            DestroyWindow(h);
            assert_eq!(rgb, [51, 102, 204], "via {how}");
        }
    }

    /// A root that matches no game process: zero captures, no error.
    #[test]
    fn no_match_no_capture() {
        let out = std::env::temp_dir().join(format!("cap-test-{}", std::process::id()));
        assert!(capture(r"Z:\no\such\root", &out).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&out);
    }
}
