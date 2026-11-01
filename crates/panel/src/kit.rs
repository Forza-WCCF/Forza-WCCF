//! Where the panel's files are and what they say, by the C panels' rules (`wccfpanel.c`: `board_paths`,
//! `keys_init_path`, `set_init`, `set_load_locked`, `send_key`). Nothing is fixed: the kit is unpacked anywhere.
//!
//! - The edition: `WCCF_EDITION` (2010-11 or 2017-18, for tests), else the kit's layout above the DLL
//!   (`forza::kit::Kit::edition`: `scripts\` without `code\` is the 2017-18 kit).
//! - `data\`: `WCCF_DATA`, else beside the DLL's folder (`programs\..\data`, 2017-18 `overlay\..\data`).
//! - The catalogue and the card pictures: `WCCF_CATALOGUE` / `WCCF_CARDS` (the run sets them), else 2010-11's
//!   `data\cache\catalogue.tsv` and `data\cache\cards\`, 2017-18's beside the DLL.
//! - The keys: `WCCF_KEYS`, else `data\keys.txt` when `data\` is there, else beside the DLL.
//! - The card table and its formation copy: the game's own folder (the program's), never configured.
//! - Links (`lib.rs` `affinity`, `links_in`): `prog_data\player_data\` in the game's own folder and the program itself,
//!   read once; the club card `data\save\seat1_club.bin`, read again when it changes (2010-11 only).
//! - The language: `WCCF_KIT_LANG`, else `data\settings.json`'s `language`, else Windows' display language, else
//!   English (the launcher's order).

use forza::keys::{COUNT, PAUSE};
use forza::panel_txt::PanelFile;
use forza_ui::Lang;
use forza_ui::panel::{CardState, Link, Status, when};
use forza_ui::theme::Edition;
use std::env;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
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
            _ => forza::kit::Kit::at(kit).edition(),
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

    /// data\panel.txt's choices (missing: this PC, no request, 1 s hover).
    pub fn panel_file(&self) -> PanelFile {
        PanelFile::parse(&std::fs::read(self.data.join("panel.txt")).unwrap_or_default())
    }

    pub fn save_panel(&self, f: &PanelFile) -> std::io::Result<()> {
        swap_write(
            &self.data.join("panel.txt"),
            &f.text(self.edition == Edition::E1718),
        )
    }

    /// RESTART NOW's and SAVE LOGS' requests to the run (`data\restart.request`, `data\savelogs.request`) and SAVE
    /// LOGS' answer (`data\savelogs.result`).
    pub fn request(&self, what: &str) -> PathBuf {
        self.data.join(what)
    }

    /// SETTINGS' NOW, as `wccfpanel.c` `set_refresh` reads it: the link to the server, the club card's session and bad
    /// endings (the card being played, a manager transfer's new card included; open only if written since this run
    /// started, running.json), its last save and backups, the game's text, the version.
    pub fn status(&self) -> Status {
        use forza::card::{self, Session};
        let since = std::fs::metadata(self.data.join("running.json"))
            .and_then(|m| m.modified())
            .ok();
        let save = self.data.join("save");
        let (session, bad) =
            card::club_session(&save.join(card::SLOT), &save.join(card::NEW), since);
        let now = SystemTime::now();
        let saved = std::fs::metadata(save.join(card::SLOT))
            .and_then(|m| m.modified())
            .ok()
            .filter(|_| session != Session::None);
        let backups: Vec<SystemTime> = std::fs::read_dir(save.join("backup"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with("seat1_club.bin.") && n.ends_with(".bak")
            })
            .filter_map(|e| e.metadata().ok()?.modified().ok())
            .collect();
        let kit = forza::kit::Kit::at(self.data.parent().unwrap_or(&self.data));
        let version = kit
            .release()
            .unwrap_or_else(|| env!("FORZA_VERSION").to_owned());
        Status {
            link: server_link(),
            card: match session {
                Session::None => CardState::None,
                Session::Unreadable => CardState::Unreadable,
                Session::New => CardState::New,
                Session::Open => CardState::Open,
                Session::Cut => CardState::Cut,
                Session::Closed => CardState::Closed,
            },
            bad,
            saved: saved.map(|t| when(t, now)),
            backup: backups.iter().max().map(|&t| (when(t, now), backups.len())),
            text: Some(kit.game_language()),
            version,
            logs: None,
            seats: forza::keys::seats_here(),
        }
    }

    /// keys.txt's keys and controller controls in the KEYS page's order (`forza::keys::NAMES`), by the key driver's
    /// rules (`forza::keys::load_keys_with`, `load_pads`) and this edition's defaults.
    pub fn load_keys(&self) -> ([u32; COUNT], [u32; COUNT]) {
        use forza::keys::{ACTIONS, load_keys_with, load_pads};
        let raw = std::fs::read(&self.keys).unwrap_or_default();
        let ui = forza::keys::defaults(self.edition == Edition::E1718);
        // forza::keys keeps the actions in its own order: by name both ways
        let at = |name: &str| {
            forza::keys::NAMES
                .iter()
                .position(|n| *n == name)
                .unwrap_or(0)
        };
        let (keys, _) = load_keys_with(&raw, std::array::from_fn(|i| ui[at(ACTIONS[i].name)]));
        let (pads, _) = load_pads(&raw);
        let back = |v: [u32; 16]| -> [u32; COUNT] {
            std::array::from_fn(|i| {
                ACTIONS
                    .iter()
                    .position(|a| a.name == forza::keys::NAMES[i])
                    .map_or(0, |j| v[j])
            })
        };
        let mut keys = back(keys);
        // PAUSE is the panel's own: the key driver skips its line
        keys[PAUSE] = forza::keys::pause_key(&raw, ui[PAUSE], &keys[..PAUSE]);
        (keys, back(pads))
    }

    /// keys.txt as the KEYS page writes it, whole: `keys.txt.panel`, then one swap (the key driver may be reading it
    /// this instant: 40 tries 5 ms apart, as `wccfpanel.c`).
    pub fn save_keys(&self, keys: &[u32; COUNT], pads: &[u32; COUNT]) -> std::io::Result<()> {
        swap_write(
            &self.keys,
            &forza::keys::keys_text(self.edition == Edition::E1718, keys, pads),
        )
    }
}

/// A file written whole: `NAME.panel`, then one swap; the reader may hold it this instant: 40 tries 5 ms apart, as
/// `wccfpanel.c`.
pub fn swap_write(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".panel");
    std::fs::write(&tmp, text)?;
    let mut last = Ok(());
    for _ in 0..40 {
        last = std::fs::rename(&tmp, path);
        if last.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    last
}

/// This process's TCP link to the game server's control port (20002), as `wccfpanel.c` `server_link`: the IP-helper
/// table read through iphlpapi.dll loaded by name (read only; the game's own IP-helper calls stay guarded).
pub fn server_link() -> Link {
    let Some(rows) = tcp_rows() else {
        return Link::Unknown;
    };
    let me = std::process::id();
    let want = control_port();
    // MIB_TCPROW_OWNER_PID: state, local address, local port, remote address, remote port, process
    for row in rows {
        let port = u16::from_be((row[4] & 0xFFFF) as u16);
        if row[5] == me && port == want && row[0] == 5 {
            let [a, b, c, d] = row[3].to_le_bytes();
            return Link::Up(format!("{a}.{b}.{c}.{d}"));
        }
    }
    Link::Down
}

/// The local port of this process's live link to the control port, to find what is left of it once it drops.
pub fn link_port() -> Option<u16> {
    let (me, want) = (std::process::id(), control_port());
    let rows = tcp_rows()?;
    rows.iter()
        .find(|r| r[5] == me && r[0] == 5 && u16::from_be((r[4] & 0xFFFF) as u16) == want)
        .map(|r| u16::from_be((r[2] & 0xFFFF) as u16))
}

/// The TCP state the dropped link from local port `local` left behind (a TIME_WAIT row has no process), if any.
pub fn link_left(local: u16) -> Option<u32> {
    let want = control_port();
    let rows = tcp_rows()?;
    rows.iter()
        .find(|r| {
            r[0] != 5
                && u16::from_be((r[2] & 0xFFFF) as u16) == local
                && u16::from_be((r[4] & 0xFFFF) as u16) == want
        })
        .map(|r| r[0])
}

/// Who closed the link, from the state it left ([`link_left`], MIB_TCP_STATE): the side that sends its FIN first
/// waits in FIN_WAIT, CLOSING or TIME_WAIT; the one that got the FIN (an EOF on its socket) in CLOSE_WAIT or LAST_ACK.
/// Nothing left: a reset, or a close by the control seen too late to tell (the row goes within the 2 s poll).
pub fn close_reason(state: Option<u32>) -> &'static str {
    match state {
        Some(6 | 7 | 9 | 11) => "this seat closed it",
        Some(8 | 10) => "the control closed it (EOF)",
        _ => "no close seen: the control closed it or the link was reset",
    }
}

/// The seats linked to the control `pid` on this PC: its connections on the control port (the projector counts).
pub fn seats_on(pid: u32) -> Option<usize> {
    let want = control_port();
    let rows = tcp_rows()?;
    Some(
        rows.iter()
            .filter(|r| r[5] == pid && r[0] == 5 && u16::from_be((r[2] & 0xFFFF) as u16) == want)
            .count(),
    )
}

/// The control port: 20002, or the tests' own (WCCFPANEL_CONTROL_PORT), never the server's on a PC that may be playing.
fn control_port() -> u16 {
    env::var("WCCFPANEL_CONTROL_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20002u16)
}

/// Every IPv4 TCP connection with its process (MIB_TCPROW_OWNER_PID: state, local address, local port, remote address,
/// remote port, process).
fn tcp_rows() -> Option<Vec<[u32; 6]>> {
    type GetTable = unsafe extern "system" fn(*mut c_void, *mut u32, i32, u32, i32, u32) -> u32;
    static GET: std::sync::OnceLock<Option<GetTable>> = std::sync::OnceLock::new();
    // SAFETY: the system's own module, never freed; GetExtendedTcpTable has this signature
    let get = GET.get_or_init(|| unsafe {
        let m = LoadLibraryW(windows::core::w!("iphlpapi.dll")).ok()?;
        let f = GetProcAddress(m, windows::core::s!("GetExtendedTcpTable"))?;
        Some(std::mem::transmute::<Proc, GetTable>(f))
    });
    let get = (*get)?;
    let mut size = 0u32;
    // SAFETY: first the size, then a buffer of it (and some): AF_INET, TCP_TABLE_OWNER_PID_ALL
    unsafe {
        get(std::ptr::null_mut(), &mut size, 0, 2, 5, 0);
        let mut buf = vec![0u32; (size as usize + 4096) / 4];
        let mut n = (buf.len() * 4) as u32;
        if get(buf.as_mut_ptr().cast(), &mut n, 0, 2, 5, 0) != 0 {
            return None;
        }
        Some(
            buf[1..]
                .as_chunks::<6>()
                .0
                .iter()
                .take(buf[0] as usize)
                .copied()
                .collect(),
        )
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

impl forza::keys::PadApi for SysWinmm {
    fn caps(&self, jid: u32) -> Option<forza::keys::Caps> {
        let mut c = JOYCAPSW::default();
        // SAFETY: winmm's own call with a structure of its size
        if unsafe { (self.caps)(jid as usize, &mut c, size_of::<JOYCAPSW>() as u32) } != 0 {
            return None;
        }
        Some(forza::keys::Caps {
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

    fn pos(&self, jid: u32) -> Option<forza::keys::Info> {
        let mut i = JOYINFOEX {
            dwSize: size_of::<JOYINFOEX>() as u32,
            dwFlags: 0xFF | JOY_RETURNPOVCTS as u32, // JOY_RETURNALL
            ..Default::default()
        };
        // SAFETY: as above
        if unsafe { (self.pos)(jid, &mut i) } != 0 {
            return None;
        }
        Some(forza::keys::Info {
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

impl forza::keys::PadApi for NoPads {
    fn caps(&self, _: u32) -> Option<forza::keys::Caps> {
        None
    }
    fn pos(&self, _: u32) -> Option<forza::keys::Info> {
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

    pub fn name(&self, caps: &forza::keys::Caps) -> String {
        self.line(u32::from(caps.pid))
            .map(|(_, n)| n)
            .unwrap_or_default()
    }
}

impl forza::keys::PadApi for FakePads {
    fn caps(&self, jid: u32) -> Option<forza::keys::Caps> {
        let (v, _) = self.line(jid)?;
        Some(forza::keys::Caps {
            num_buttons: 12,
            num_axes: v[2],
            flags: v[3],
            mid: 0xFFFF,
            pid: jid as u16,
            lim: [(0, 65535); 6],
        })
    }

    fn pos(&self, jid: u32) -> Option<forza::keys::Info> {
        let (v, _) = self.line(jid)?;
        Some(forza::keys::Info {
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

#[cfg(test)]
mod tests {
    use super::close_reason;

    #[test]
    fn a_dropped_link_says_who_closed_it() {
        assert_eq!(close_reason(Some(11)), "this seat closed it");
        assert_eq!(close_reason(Some(7)), "this seat closed it");
        assert_eq!(close_reason(Some(8)), "the control closed it (EOF)");
        assert_eq!(
            close_reason(Some(10)),
            "the control closed it (EOF)",
            "LAST_ACK"
        );
        assert_eq!(close_reason(Some(9)), "this seat closed it", "CLOSING");
        assert_eq!(
            close_reason(None),
            "no close seen: the control closed it or the link was reset"
        );
    }
}
