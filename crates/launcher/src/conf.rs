//! The game's settings files (a cabinet's client_user_option.conf and the like): "KEY=value" lines changed in place,
//! byte for byte as play.py's set_control_ip, set_satellite_no and set_conf_value do: the file's own line ends kept,
//! nothing else touched, written through a temporary file and read back.

use std::path::Path;
use std::{fs, io};

use forza_ui::lobby::{CUP_MAX_MINUTES, CUP_MIN_MINUTES, CupChoice, CupMode};
use regex::bytes::{Captures, Regex};

/// The CONTROL_IP= and MATCH_IP= lines of a cabinet's client_user_option.conf set to `ip`; true if the file changed.
/// An error if it has no CONTROL_IP line (setup writes one).
pub fn set_control_ip(conf: &Path, ip: &str) -> io::Result<bool> {
    let raw = fs::read(conf)?;
    need(conf, &raw, "CONTROL_IP")?;
    let pat = Regex::new(r"(?m-u)^(CONTROL_IP|MATCH_IP)=[^\r\n]*").expect("a fixed pattern");
    let out = pat.replace_all(&raw, |c: &Captures| [&c[1], b"=", ip.as_bytes()].concat());
    write(conf, &raw, &out)
}

/// The cabinet's seat in its client_user_option.conf (SATELLITE_NO=n: setup writes the line, as the game's command line
/// breaks on folder names with spaces); true if the file changed.
pub fn set_satellite_no(conf: &Path, n: u32) -> io::Result<bool> {
    let raw = fs::read(conf)?;
    need(conf, &raw, "SATELLITE_NO")?;
    let pat = Regex::new(r"(?m-u)^SATELLITE_NO=[^\r\n]*").expect("a fixed pattern");
    let line = format!("SATELLITE_NO={n}");
    let out = pat.replace_all(&raw, line.as_bytes());
    write(conf, &raw, &out)
}

/// KEY=value in one of the game's settings files: every KEY line replaced, or one added at the end in the file's own
/// line ends; true if the file changed.
pub fn set_conf_value(conf: &Path, key: &str, value: &str) -> io::Result<bool> {
    let raw = fs::read(conf)?;
    let out = conf_text_with(&raw, key, value)?;
    write(conf, &raw, &out)
}

/// The host dialog's Universal Cup choice in data\settings.json: "cup" ("off", "game", "in", "at"; else the game's
/// times), "cup_minutes" (In N min, kept within the dialog's range) and "cup_at" (HHMM).
pub fn cup_choice(settings: &serde_json::Value) -> CupChoice {
    let d = CupChoice::default();
    let num = |k: &str| settings[k].as_u64().and_then(|n| u16::try_from(n).ok());
    CupChoice {
        mode: match settings["cup"].as_str() {
            Some("off") => CupMode::Off,
            Some("in") => CupMode::In,
            Some("at") => CupMode::At,
            _ => CupMode::GameTimes,
        },
        minutes: num("cup_minutes").map_or(d.minutes, |m| m.clamp(CUP_MIN_MINUTES, CUP_MAX_MINUTES)),
        at: num("cup_at")
            .filter(|t| t / 100 < 24 && t % 100 < 60)
            .map_or(d.at, |t| t / 100 * 60 + t % 100),
    }
}

/// [`cup_choice`]'s settings for `c`.
pub fn cup_settings(c: CupChoice) -> [(&'static str, serde_json::Value); 3] {
    let mode = match c.mode {
        CupMode::Off => "off",
        CupMode::GameTimes => "game",
        CupMode::In => "in",
        CupMode::At => "at",
    };
    [
        ("cup", mode.into()),
        ("cup_minutes", c.minutes.into()),
        ("cup_at", (c.at / 60 * 100 + c.at % 60).into()),
    ]
}

/// The first line of the `schedule_option.conf` the Launcher writes (the control skips `#` lines, 0x4A0F40): a file
/// without it is someone else's and stays.
const SCHEDULE_MARK: &str = "# Forza-WCCF Universal Cup";

/// The control's competition list (local\schedule_option.conf, read once as it starts, 0x4A0DC0): `friendlies` friendly
/// slots, the Universal Cup, then the 21 friendlies the control's own timed insertion puts after a cup (0x45F030).
pub fn schedule_text(friendlies: u32) -> String {
    let mut out = format!("{SCHEDULE_MARK} (the Launcher rewrites this at each start)\r\n");
    out += &"FriendlyMatch\r\n".repeat(friendlies as usize);
    out += "UniversalCup\r\n";
    out += &"FriendlyMatch\r\n".repeat(21);
    out
}

/// local\schedule_option.conf for a cup after `friendlies` slots (anyone else's file kept as schedule_option.conf.bak);
/// `None` removes the Launcher's own file (the control then runs 21 friendlies, 0x4A0FE0) and leaves anyone else's.
pub fn set_cup_schedule(file: &Path, friendlies: Option<u32>) -> io::Result<()> {
    let ours = fs::read(file).map(|raw| raw.starts_with(SCHEDULE_MARK.as_bytes()));
    match (friendlies, ours) {
        (Some(n), Ok(false)) => {
            fs::rename(file, file.with_extension("conf.bak"))?;
            write(file, &[], schedule_text(n).as_bytes()).map(drop)
        }
        (Some(n), _) => write(file, &fs::read(file).unwrap_or_default(), schedule_text(n).as_bytes()).map(drop),
        (None, Ok(true)) => fs::remove_file(file),
        (None, _) => Ok(()),
    }
}

/// The control's own Universal Cup times in ctrl_user_option.conf, read once as it starts (control 0x49EE90, copied at
/// 0x45CF77): UNIVERSAL_CHAMPIONSHIP_OPEN_TIME_OPTION=1 and OPEN_TIME1..4 for `times` (all 0: no timed cup, 0x462820
/// takes only a time after now), or OPTION=0 for the game's times (then the control takes the stand-in's GAME_CTRL
/// times and ignores OPEN_TIME1..4, so they stay as they are).
pub fn cup_times_text(raw: &[u8], times: Option<[u32; 4]>) -> io::Result<Vec<u8>> {
    let on = if times.is_some() { "1" } else { "0" };
    let mut out = conf_text_with(raw, "UNIVERSAL_CHAMPIONSHIP_OPEN_TIME_OPTION", on)?;
    for (i, t) in times.iter().flatten().enumerate() {
        out = conf_text_with(
            &out,
            &format!("UNIVERSAL_CHAMPIONSHIP_OPEN_TIME{}", i + 1),
            &t.to_string(),
        )?;
    }
    Ok(out)
}

/// [`cup_times_text`] on the file; true if it changed.
pub fn set_cup_times(conf: &Path, times: Option<[u32; 4]>) -> io::Result<bool> {
    let raw = fs::read(conf)?;
    let out = cup_times_text(&raw, times)?;
    write(conf, &raw, &out)
}

/// 2017-18's control and its game server (ctrl_user_option.conf): GAME_SVR_IP, GAME_SVR_PORT and GAME_SVR_ENTRY,
/// 1 = online, the control's VS entries (243-246) on, else 0 (control_Release.exe reads it at +932 of its options).
/// `host` must be one the control can use: see [`game_server_host`].
pub fn game_server_text(raw: &[u8], host: &str, port: u16, entry: bool) -> io::Result<Vec<u8>> {
    let host = game_server_host(host)?;
    if port == 0 {
        return Err(io::Error::other("the game server port must be 1-65535"));
    }
    let t = conf_text_with(raw, "GAME_SVR_IP", host)?;
    let t = conf_text_with(&t, "GAME_SVR_PORT", &port.to_string())?;
    conf_text_with(&t, "GAME_SVR_ENTRY", if entry { "1" } else { "0" })
}

/// GAME_SVR_IP as the control reads it (control 0x49EE90, 0x440C80): at most 63 bytes (more crashes it), no blanks;
/// exactly `127.0.0.1` means "use the network's own host" (none here: never connects), so that one becomes
/// `localhost`. A numeric address goes to `inet_addr`, which reads leading zeros as octal, accepts short forms and
/// takes 255.255.255.255 as an error, so a numeric address must be a plain dotted IPv4 other than that one.
fn game_server_host(host: &str) -> io::Result<&str> {
    let bad = |why: &str| Err(io::Error::other(format!("game server address \"{host}\": {why}")));
    if host == "127.0.0.1" {
        return Ok("localhost");
    }
    if host.is_empty() || host.len() > 63 {
        return bad("empty or longer than 63 characters");
    }
    if host
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control() || b == b':')
    {
        return bad("blanks or a colon (IPv6 does not work)");
    }
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        // Ipv4Addr refuses leading zeros and short forms
        match host.parse::<std::net::Ipv4Addr>() {
            Ok(ip) if !ip.is_broadcast() => {}
            _ => return bad("not a plain dotted IPv4 address (four numbers 0-255, no leading zeros)"),
        }
    }
    Ok(host)
}

/// [`game_server_text`] on the file; true if it changed.
pub fn set_game_server(conf: &Path, host: &str, port: u16, entry: bool) -> io::Result<bool> {
    let raw = fs::read(conf)?;
    let out = game_server_text(&raw, host, port, entry)?;
    write(conf, &raw, &out)
}

/// `set_conf_value` on the file's bytes.
pub fn conf_text_with(raw: &[u8], key: &str, value: &str) -> io::Result<Vec<u8>> {
    let nl: &[u8] = if raw.windows(2).any(|w| w == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    let line = format!("{key}={value}");
    let pat = Regex::new(&format!(r"(?m-u)^{}=[^\r\n]*", regex::escape(key))).map_err(io::Error::other)?;
    Ok(if pat.is_match(raw) {
        pat.replace_all(raw, regex::bytes::NoExpand(line.as_bytes()))
            .into_owned()
    } else {
        let sep: &[u8] = if raw.ends_with(nl) || raw.is_empty() { b"" } else { nl };
        [raw, sep, line.as_bytes(), nl].concat()
    })
}

fn need(conf: &Path, raw: &[u8], key: &str) -> io::Result<()> {
    let pat = Regex::new(&format!(r"(?m-u)^{key}=")).expect("a plain key");
    if pat.is_match(raw) {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "{} has no {key} line - press SET UP in Launcher.exe",
        conf.display()
    )))
}

/// `out` over `conf` when it differs from `raw`: a temporary file, one swap, then read back.
fn write(conf: &Path, raw: &[u8], out: &[u8]) -> io::Result<bool> {
    if out == raw {
        return Ok(false);
    }
    let mut tmp = conf.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, out)?;
    fs::rename(&tmp, conf)?;
    if fs::read(conf)? != out {
        return Err(io::Error::other(format!("could not write {}", conf.display())));
    }
    Ok(true)
}

#[cfg(test)]
mod club_tests {
    use super::{conf_text_with, game_server_text};

    /// Online 2017-18 points control at the server's game server with its VS entries on; this PC goes back to the
    /// stand-in with them off. The rest of setup's CONF_CONTROL stays as it was.
    #[test]
    fn game_server_online_then_this_pc() {
        let setup = b"FORCE_ONLINE=1\r\nGAME_SVR_IP=localhost\r\nGAME_SVR_PORT=20050\r\nWAIT_PROJECTOR=1\r\n";
        let on = game_server_text(setup, "203.0.113.7", 20060, true).unwrap();
        assert_eq!(
            on,
            b"FORCE_ONLINE=1\r\nGAME_SVR_IP=203.0.113.7\r\nGAME_SVR_PORT=20060\r\nWAIT_PROJECTOR=1\r\nGAME_SVR_ENTRY=1\r\n"
        );
        let off = game_server_text(&on, "localhost", 20050, false).unwrap();
        assert_eq!(
            off,
            b"FORCE_ONLINE=1\r\nGAME_SVR_IP=localhost\r\nGAME_SVR_PORT=20050\r\nWAIT_PROJECTOR=1\r\nGAME_SVR_ENTRY=0\r\n"
        );
    }

    #[test]
    fn game_server_address_is_one_the_control_can_use() {
        let raw = b"GAME_SVR_IP=localhost\r\n";
        let ip = |h: &str| game_server_text(raw, h, 20060, true).map(|t| String::from_utf8(t).unwrap());
        assert!(ip("127.0.0.1").unwrap().starts_with("GAME_SVR_IP=localhost\r\n"));
        for ok in ["203.0.113.10", "club.example.org", &"a".repeat(63)] {
            assert!(ip(ok).unwrap().starts_with(&format!("GAME_SVR_IP={ok}\r\n")), "{ok}");
        }
        for bad in [
            "",
            "203.0.113.010",
            "203.0.113",
            "255.255.255.255",
            "a b",
            "::1",
            &"a".repeat(64),
        ] {
            assert!(ip(bad).is_err(), "{bad}");
        }
        assert!(game_server_text(raw, "localhost", 0, true).is_err());
    }

    #[test]
    fn rewrite_keeps_other_lines_and_line_ends() {
        let t = b"A=1\r\nGAME_SVR_IP=localhost\r\nGAME_SVR_PORT=20050\r\n";
        let t = conf_text_with(t, "GAME_SVR_IP", "club.example.org").unwrap();
        let t = conf_text_with(&t, "GAME_SVR_PORT", "20060").unwrap();
        assert_eq!(t, b"A=1\r\nGAME_SVR_IP=club.example.org\r\nGAME_SVR_PORT=20060\r\n");
        assert_eq!(conf_text_with(b"A=1", "K", "v").unwrap(), b"A=1\nK=v\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cup_choice_becomes_the_controls_schedule_and_open_time_keys() {
        let text = schedule_text(2);
        assert!(text.starts_with("# Forza-WCCF Universal Cup "));
        let lines: Vec<&str> = text.split_terminator("\r\n").skip(1).collect();
        assert_eq!(lines.len(), 2 + 1 + 21);
        assert_eq!(lines[..3], ["FriendlyMatch", "FriendlyMatch", "UniversalCup"]);
        assert!(lines[3..].iter().all(|l| *l == "FriendlyMatch"));
        // written, rewritten, removed; someone else's file stays
        let dir = std::env::temp_dir().join(format!("forza-cup-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("schedule_option.conf");
        set_cup_schedule(&f, Some(1)).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), schedule_text(1));
        set_cup_schedule(&f, None).unwrap();
        assert!(!f.exists());
        fs::write(&f, "UniversalCup\r\n").unwrap();
        set_cup_schedule(&f, None).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "UniversalCup\r\n");
        set_cup_schedule(&f, Some(1)).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), schedule_text(1));
        let bak = dir.join("schedule_option.conf.bak");
        assert_eq!(fs::read_to_string(bak).unwrap(), "UniversalCup\r\n");
        fs::remove_dir_all(&dir).unwrap();
        // the host's choice as saved, then as the control gets it: In 25 min at 14:00 = 3 friendlies, then the cup
        let mut st = serde_json::json!({});
        for (k, v) in cup_settings(CupChoice {
            mode: CupMode::In,
            minutes: 25,
            at: 21 * 60 + 30,
        }) {
            st[k] = v;
        }
        assert_eq!(st, serde_json::json!({"cup": "in", "cup_minutes": 25, "cup_at": 2130}));
        let c = cup_choice(&st);
        assert_eq!((c.mode, c.minutes, c.at), (CupMode::In, 25, 21 * 60 + 30));
        let Ok(forza_ui::lobby::CupPlan::Slot { friendlies, .. }) = c.plan(14 * 3600) else {
            panic!("{c:?}")
        };
        assert_eq!(schedule_text(friendlies), schedule_text(3));
        // the dialog's minimum holds for a hand-edited file; no "cup" = the game's times
        let c = cup_choice(&serde_json::json!({"cup_minutes": 2, "cup_at": 2575}));
        assert_eq!(
            (c.mode, c.minutes, c.at),
            (CupMode::GameTimes, CUP_MIN_MINUTES, 21 * 60)
        );
        let raw = b"GAME_SVR_IP=localhost\r\nUNIVERSAL_CHAMPIONSHIP_OPEN_TIME1=900\r\n";
        assert_eq!(
            cup_times_text(raw, Some([0; 4])).unwrap(),
            b"GAME_SVR_IP=localhost\r\nUNIVERSAL_CHAMPIONSHIP_OPEN_TIME1=0\r\n\
UNIVERSAL_CHAMPIONSHIP_OPEN_TIME_OPTION=1\r\nUNIVERSAL_CHAMPIONSHIP_OPEN_TIME2=0\r\n\
UNIVERSAL_CHAMPIONSHIP_OPEN_TIME3=0\r\nUNIVERSAL_CHAMPIONSHIP_OPEN_TIME4=0\r\n"
        );
        // the game's times: the option off, the old times left for nobody to read
        assert_eq!(
            cup_times_text(
                b"UNIVERSAL_CHAMPIONSHIP_OPEN_TIME_OPTION=1\nUNIVERSAL_CHAMPIONSHIP_OPEN_TIME1=1435\n",
                None
            )
            .unwrap(),
            b"UNIVERSAL_CHAMPIONSHIP_OPEN_TIME_OPTION=0\nUNIVERSAL_CHAMPIONSHIP_OPEN_TIME1=1435\n"
        );
    }
}
