//! Where the panel's files are and what they say, by the C panels' rules (`wccfpanel.c`: `board_paths`,
//! `keys_init_path`, `set_init`, `set_load_locked`, `send_key`). Nothing is fixed: the kit is unpacked anywhere.
//!
//! - The edition: `WCCF_EDITION` (2010-11 or 2017-18, for tests), else the kit's layout above the DLL
//!   (`launcher::Kit::edition`: `scripts\` without `code\` is the 2017-18 kit).
//! - `data\`: `WCCF_DATA`, else beside the DLL's folder (`programs\..\data`, 2017-18 `overlay\..\data`).
//! - The catalogue and the card pictures: `WCCF_CATALOGUE` / `WCCF_CARDS` (the run sets them), else 2010-11's
//!   `data\cache\catalogue.tsv` and `data\cache\cards\`, 2017-18's beside the DLL.
//! - The keys: `WCCF_KEYS`, else `data\keys.txt` when `data\` is there, else beside the DLL.
//! - The card table and its formation copy: the game's own folder (the program's), never configured.
//! - The language: `WCCF_KIT_LANG`, else `data\settings.json`'s `language`, else Windows' display language, else
//!   English (the launcher's order).

use forza::ui::Lang;
use forza::ui::theme::Edition;
use std::env;
use std::path::{Path, PathBuf};
use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::Media::Multimedia::{JOY_RETURNPOVCTS, JOYCAPSW, JOYINFOEX};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY,
};

pub struct Files {
    pub edition: Edition,
    pub data: PathBuf,
    pub catalogue: PathBuf,
    pub cards: PathBuf,
    pub keys: PathBuf,
}

fn var(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl Files {
    pub fn find(dll_dir: &Path) -> Files {
        let kit = dll_dir.parent().unwrap_or(dll_dir);
        let edition = match env::var("WCCF_EDITION").as_deref() {
            Ok("2017-18" | "1718") => Edition::E1718,
            Ok("2010-11" | "1011") => Edition::E1011,
            _ => match launcher::Kit::at(kit).edition() {
                launcher::Edition::E1718 => Edition::E1718,
                launcher::Edition::E1011 => Edition::E1011,
            },
        };
        let data = var("WCCF_DATA").unwrap_or_else(|| kit.join("data"));
        let cache = match edition {
            Edition::E1011 => data.join("cache"),
            Edition::E1718 => dll_dir.to_owned(),
        };
        let keys = var("WCCF_KEYS").unwrap_or_else(|| {
            if data.is_dir() {
                data.join("keys.txt")
            } else {
                dll_dir.join("keys.txt")
            }
        });
        Files {
            edition,
            catalogue: var("WCCF_CATALOGUE").unwrap_or_else(|| cache.join("catalogue.tsv")),
            cards: var("WCCF_CARDS").unwrap_or_else(|| cache.join("cards")),
            keys,
            data,
        }
    }

    pub fn lang(&self) -> Lang {
        let saved = std::fs::read(self.data.join("settings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("language")?.as_str().map(str::to_owned));
        // SAFETY: a plain query without arguments
        let windows = match unsafe { GetUserDefaultUILanguage() } & 0x3ff {
            0x10 => "it",
            0x11 => "ja",
            _ => "en",
        };
        [env::var("WCCF_KIT_LANG").ok(), saved]
            .into_iter()
            .flatten()
            .chain([windows.to_owned()])
            .find_map(|c| Lang::ALL.into_iter().find(|l| l.code() == c))
            .unwrap_or(Lang::En)
    }

    /// `hover_delay_ms` in data\panel.txt, in seconds: 0-5000 ms; missing, not a number or out of range: 1000.
    pub fn hover_delay(&self) -> f32 {
        let text = std::fs::read(self.data.join("panel.txt")).unwrap_or_default();
        let mut ms = 1000;
        for line in String::from_utf8_lossy(&text).lines() {
            let line = line.split('#').next().unwrap_or("");
            if let Some((name, val)) = line.split_once('=')
                && name.trim().eq_ignore_ascii_case("hover_delay_ms")
            {
                ms = val
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|v| *v <= 5000)
                    .unwrap_or(1000);
            }
        }
        ms as f32 / 1000.0
    }

    /// keys.txt's keys and controller controls in the KEYS page's order (`forza::keys::NAMES`), by the key driver's
    /// rules (`launcher::keys::load_keys_with`, `load_pads`) and this edition's defaults.
    pub fn load_keys(&self) -> ([u32; 16], [u32; 16]) {
        use launcher::keys::{ACTIONS, load_keys_with, load_pads};
        let raw = std::fs::read(&self.keys).unwrap_or_default();
        let ui = forza::keys::defaults(self.edition == Edition::E1718);
        // launcher::keys keeps the actions in its own order: by name both ways
        let at = |name: &str| {
            forza::keys::NAMES
                .iter()
                .position(|n| *n == name)
                .unwrap_or(0)
        };
        let (keys, _) = load_keys_with(&raw, std::array::from_fn(|i| ui[at(ACTIONS[i].name)]));
        let (pads, _) = load_pads(&raw);
        let back = |v: [u32; 16]| {
            std::array::from_fn(|i| {
                ACTIONS
                    .iter()
                    .position(|a| a.name == forza::keys::NAMES[i])
                    .map_or(0, |j| v[j])
            })
        };
        (back(keys), back(pads))
    }

    /// keys.txt as the KEYS page writes it, whole: `keys.txt.panel`, then one swap (the key driver may be reading it
    /// this instant: 40 tries 5 ms apart, as `wccfpanel.c`).
    pub fn save_keys(&self, keys: &[u32; 16], pads: &[u32; 16]) -> std::io::Result<()> {
        let text = forza::keys::keys_text(self.edition == Edition::E1718, keys, pads);
        let mut tmp = self.keys.clone().into_os_string();
        tmp.push(".panel");
        std::fs::write(&tmp, text)?;
        let mut last = Ok(());
        for _ in 0..40 {
            last = std::fs::rename(&tmp, &self.keys);
            if last.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        last
    }
}

/// winmm's joystick calls from the system folder, as `wccfpanel.c` `pad_api` takes them: seat 1's own winmm.dll is the
/// hook, which (2010-11) passes no joystick calls on, so they are never imported by name.
pub struct SysWinmm {
    caps: unsafe extern "system" fn(usize, *mut JOYCAPSW, u32) -> u32,
    pos: unsafe extern "system" fn(u32, *mut JOYINFOEX) -> u32,
    changed: Option<unsafe extern "system" fn(u32) -> u32>,
}

type Proc = unsafe extern "system" fn() -> isize;

impl SysWinmm {
    pub fn load() -> Option<SysWinmm> {
        let mut dir = [0u16; 260];
        // SAFETY: a buffer we own; the module stays loaded for the process (never freed), so the functions stay valid
        unsafe {
            let n = GetSystemDirectoryW(Some(&mut dir)) as usize;
            let path: Vec<u16> = dir[..n.min(260)]
                .iter()
                .copied()
                .chain("\\winmm.dll\0".encode_utf16())
                .collect();
            let m = LoadLibraryW(windows::core::PCWSTR(path.as_ptr())).ok()?;
            let caps = GetProcAddress(m, windows::core::s!("joyGetDevCapsW"))?;
            let pos = GetProcAddress(m, windows::core::s!("joyGetPosEx"))?;
            let changed = GetProcAddress(m, windows::core::s!("joyConfigChanged"));
            Some(SysWinmm {
                caps: std::mem::transmute::<
                    Proc,
                    unsafe extern "system" fn(usize, *mut JOYCAPSW, u32) -> u32,
                >(caps),
                pos: std::mem::transmute::<
                    Proc,
                    unsafe extern "system" fn(u32, *mut JOYINFOEX) -> u32,
                >(pos),
                changed: changed
                    .map(|f| std::mem::transmute::<Proc, unsafe extern "system" fn(u32) -> u32>(f)),
            })
        }
    }
}

impl launcher::keys::PadApi for SysWinmm {
    fn caps(&self, jid: u32) -> Option<launcher::keys::Caps> {
        let mut c = JOYCAPSW::default();
        // SAFETY: winmm's own call with a structure of its size
        if unsafe { (self.caps)(jid as usize, &mut c, size_of::<JOYCAPSW>() as u32) } != 0 {
            return None;
        }
        Some(launcher::keys::Caps {
            num_buttons: c.wNumButtons,
            num_axes: c.wNumAxes,
            flags: c.wCaps,
            mid: c.wMid,
            pid: c.wPid,
            lim: [
                (c.wXmin, c.wXmax),
                (c.wYmin, c.wYmax),
                (c.wZmin, c.wZmax),
                (c.wRmin, c.wRmax),
                (c.wUmin, c.wUmax),
                (c.wVmin, c.wVmax),
            ],
        })
    }

    fn pos(&self, jid: u32) -> Option<launcher::keys::Info> {
        let mut i = JOYINFOEX {
            dwSize: size_of::<JOYINFOEX>() as u32,
            dwFlags: 0xFF | JOY_RETURNPOVCTS as u32, // JOY_RETURNALL
            ..Default::default()
        };
        // SAFETY: as above
        if unsafe { (self.pos)(jid, &mut i) } != 0 {
            return None;
        }
        Some(launcher::keys::Info {
            buttons: i.dwButtons,
            pos: [i.dwXpos, i.dwYpos, i.dwZpos, i.dwRpos, i.dwUpos, i.dwVpos],
            pov: i.dwPOV,
        })
    }

    fn config_changed(&self) {
        if let Some(f) = self.changed {
            // SAFETY: winmm's own call
            unsafe { f(0) };
        }
    }
}

/// No controllers to be had.
pub struct NoPads;

impl launcher::keys::PadApi for NoPads {
    fn caps(&self, _: u32) -> Option<launcher::keys::Caps> {
        None
    }
    fn pos(&self, _: u32) -> Option<launcher::keys::Info> {
        None
    }
    fn config_changed(&self) {}
}

/// The test's controllers (`WCCFPANEL_FAKEPAD=file`, as `wccfpanel.c`): one line each, "SLOT BUTTONS AXES CAPS X Y Z R
/// U V POV NAME" (axes 0-65535 as winmm's), read again at every call.
pub struct FakePads(pub PathBuf);

impl FakePads {
    fn line(&self, jid: u32) -> Option<([u32; 11], String)> {
        let text = std::fs::read_to_string(&self.0).ok()?;
        text.lines().find_map(|l| {
            let mut w = l.split_whitespace();
            let mut v = [0u32; 11];
            for x in &mut v {
                let t = w.next()?;
                *x = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                    Some(h) => u32::from_str_radix(h, 16).ok()?,
                    None => t.parse().ok()?,
                };
            }
            (v[0] == jid).then(|| (v, w.collect::<Vec<_>>().join(" ")))
        })
    }

    pub fn name(&self, caps: &launcher::keys::Caps) -> String {
        self.line(u32::from(caps.pid))
            .map(|(_, n)| n)
            .unwrap_or_default()
    }
}

impl launcher::keys::PadApi for FakePads {
    fn caps(&self, jid: u32) -> Option<launcher::keys::Caps> {
        let (v, _) = self.line(jid)?;
        Some(launcher::keys::Caps {
            num_buttons: 12,
            num_axes: v[2],
            flags: v[3],
            mid: 0xFFFF,
            pid: jid as u16,
            lim: [(0, 65535); 6],
        })
    }

    fn pos(&self, jid: u32) -> Option<launcher::keys::Info> {
        let (v, _) = self.line(jid)?;
        Some(launcher::keys::Info {
            buttons: v[1],
            pos: [v[4], v[5], v[6], v[7], v[8], v[9]],
            pov: v[10],
        })
    }

    fn config_changed(&self) {}
}

/// A key down or up, as the C panel's `send_key`: none for 0; with WCCFPANEL_DRYKEYS=1 only logged.
pub fn send_key(vk: u32, up: bool, log: impl Fn(&str)) {
    if vk == 0 {
        return;
    }
    if env::var("WCCFPANEL_DRYKEYS").is_ok_and(|v| v.starts_with('1')) {
        log(&format!(
            "key 0x{vk:02X} {} (dry run - not pressed)",
            if up { "up" } else { "down" }
        ));
        return;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk as u16),
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                ..Default::default()
            },
        },
    };
    // SAFETY: one keyboard input of our own
    unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
}
