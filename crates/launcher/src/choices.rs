//! What the window's choices read and write, in the files the rest of the kit already uses:
//!   data\panel.txt      play_on= / server= (the in-game SETTINGS panel writes the same lines; a plain start reads
//!                       them) and english=on|it|off, a change play.py makes at the start and then takes out
//!   data\settings.json  "language": the window's own words (en, it, ja); setup.py keeps "game" and "server" there
//! Lines "name=value" with "#" starting a note, by play.py read_panel()'s rules.  A file is written whole: a temporary
//! file, then one swap.

use std::path::Path;
use std::{fs, io};

use serde_json::Value;

/// The value of `name` in a panel.txt text, if the line is there.
pub fn panel_value(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.split('#').next()?.split_once('=')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_owned())
    })
}

/// `text` with the line for `name` set to `value` (in place of the first, others dropped; added at the end if there
/// was none), or taken out (None).  Every other line stays as it was.
pub fn set_panel_value(text: &str, name: &str, value: Option<&str>) -> String {
    let mut out = String::new();
    let mut done = false;
    for line in text.lines() {
        let named = line
            .split('#')
            .next()
            .and_then(|l| l.split_once('='))
            .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case(name));
        if !named {
            out.push_str(line);
            out.push('\n');
        } else if let Some(v) = value.filter(|_| !done) {
            out.push_str(&format!("{name}={v}\n"));
            done = true;
        }
    }
    if let (Some(v), false) = (value, done) {
        out.push_str(&format!("{name}={v}\n"));
    }
    out
}

pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// A temporary file beside `path`, then one swap: never a half-written file for the game or the panel.
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

/// settings.json with `key` set; every other key stays (setup.py's "game" and "server").
pub fn set_setting_bool(path: &Path, key: &str, value: bool) -> io::Result<()> {
    set_value(path, key, Value::Bool(value))
}

pub fn set_setting(path: &Path, key: &str, value: &str) -> io::Result<()> {
    set_value(path, key, Value::String(value.to_owned()))
}

/// Any JSON value under `key` (the `servers` list, the last lobby seat).
pub fn set_setting_json(path: &Path, key: &str, value: Value) -> io::Result<()> {
    set_value(path, key, value)
}

pub fn setting_json(path: &Path, key: &str) -> Option<Value> {
    serde_json::from_str::<Value>(&read(path)).ok()?.get(key).cloned()
}

fn set_value(path: &Path, key: &str, value: Value) -> io::Result<()> {
    let mut json = serde_json::from_str::<Value>(&read(path))
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    json[key] = value;
    write(path, &serde_json::to_string_pretty(&json).map_err(io::Error::other)?)
}

pub fn setting(path: &Path, key: &str) -> Option<String> {
    let json = serde_json::from_str::<Value>(&read(path)).ok()?;
    json[key].as_str().map(str::to_owned)
}

pub use forza::panel_txt::valid_address;

/// The 2017-18 club server's default port (`forza-server --edition 2017-18 gmsvr`).
pub const CLUB_PORT: u16 = 20060;

/// data\\settings.json "club_server": "" = this PC; "HOST" or "HOST:PORT" (host: a name or IPv4 address).
pub fn parse_club_server(s: &str) -> Option<(String, u16)> {
    let s = s.trim();
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok().filter(|&p| p != 0)?),
        None => (s, CLUB_PORT),
    };
    let ok = !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|l| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    ok.then(|| (host.to_owned(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn club_server_parse() {
        assert_eq!(
            parse_club_server("club.example.org"),
            Some(("club.example.org".into(), 20060))
        );
        assert_eq!(parse_club_server(" 10.0.0.5:2000 "), Some(("10.0.0.5".into(), 2000)));
        for bad in [
            "",
            "host:",
            "host:0",
            "host:70000",
            ":20060",
            "a b",
            "-x.org",
            "a..b",
            "h:1:2",
        ] {
            assert_eq!(parse_club_server(bad), None, "{bad}");
        }
    }

    #[test]
    fn panel_lines() {
        let panel = "# note\nplay_on=online\nserver=1.2.3.4  # the host\nlayout=compact\n";
        assert_eq!(panel_value(panel, "PLAY_ON").as_deref(), Some("online"));
        assert_eq!(panel_value(panel, "server").as_deref(), Some("1.2.3.4"));
        assert_eq!(panel_value(panel, "english"), None);
        assert_eq!(panel_value("# server=9.9.9.9\n", "server"), None);
        let set = set_panel_value(panel, "play_on", Some("this_pc"));
        assert_eq!(
            set,
            "# note\nplay_on=this_pc\nserver=1.2.3.4  # the host\nlayout=compact\n"
        );
        let added = set_panel_value(&set, "english", Some("on"));
        assert!(added.ends_with("layout=compact\nenglish=on\n"), "{added}");
        assert_eq!(set_panel_value(&added, "english", None), set);
        assert_eq!(set_panel_value("a=1\r\na=2\n", "a", Some("3")), "a=3\n");
        assert_eq!(set_panel_value("", "server", Some("1.2.3.4")), "server=1.2.3.4\n");
    }

    #[test]
    fn addresses() {
        assert!(valid_address("192.168.1.20"));
        assert!(valid_address("0.0.0.0"));
        for bad in [
            "",
            "1.2.3",
            "1.2.3.4.5",
            "256.1.1.1",
            "a.b.c.d",
            "1..2.3",
            "1.2.3.4 ",
            "1234.1.1.1",
            "١.2.3.4",
        ] {
            assert!(!valid_address(bad), "{bad}");
        }
    }

    #[test]
    fn settings_keep_other_keys() {
        let dir = std::env::temp_dir().join(format!("wccf-choices-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        write(&path, r#"{"game": "D:\\WCCF\\extracted", "server": "1.2.3.4"}"#).unwrap();
        set_setting_bool(&path, "skip_tutorial", true).unwrap();
        assert!(read(&path).contains("\"skip_tutorial\": true"));
        set_setting(&path, "language", "it").unwrap();
        assert_eq!(setting(&path, "language").as_deref(), Some("it"));
        assert_eq!(setting(&path, "game").as_deref(), Some(r"D:\WCCF\extracted"));
        write(&path, "not json").unwrap();
        set_setting(&path, "language", "ja").unwrap();
        assert_eq!(setting(&path, "language").as_deref(), Some("ja"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
