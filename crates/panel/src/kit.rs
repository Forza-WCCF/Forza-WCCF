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

    /// The key of a cabinet action (its name in keys.txt) as keys.txt has it now, by the key driver's rules
    /// (`launcher::keys::load_keys`); 0 = none.
    pub fn key_of(&self, action: &str) -> u32 {
        let (keys, _) = launcher::keys::load_keys(&std::fs::read(&self.keys).unwrap_or_default());
        launcher::keys::ACTIONS
            .iter()
            .position(|a| a.name == action)
            .map_or(0, |i| keys[i])
    }
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
