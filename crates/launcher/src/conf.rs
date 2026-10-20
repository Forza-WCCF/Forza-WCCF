//! The game's settings files (a cabinet's client_user_option.conf and the like): "KEY=value" lines changed in place,
//! byte for byte as play.py's set_control_ip, set_satellite_no and set_conf_value do: the file's own line ends kept,
//! nothing else touched, written through a temporary file and read back.

use std::path::Path;
use std::{fs, io};

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

/// 2017-18's control and its game server (ctrl_user_option.conf): GAME_SVR_IP, GAME_SVR_PORT and GAME_SVR_ENTRY,
/// 1 = online, the control's VS entries (243-246) on, else 0 (control_Release.exe reads it at +932 of its options).
pub fn game_server_text(raw: &[u8], host: &str, port: u16, entry: bool) -> io::Result<Vec<u8>> {
    let t = conf_text_with(raw, "GAME_SVR_IP", host)?;
    let t = conf_text_with(&t, "GAME_SVR_PORT", &port.to_string())?;
    conf_text_with(&t, "GAME_SVR_ENTRY", if entry { "1" } else { "0" })
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
        "{} has no {key} line - set up again (Launcher.exe, Setup and updates)",
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
    fn rewrite_keeps_other_lines_and_line_ends() {
        let t = b"A=1\r\nGAME_SVR_IP=localhost\r\nGAME_SVR_PORT=20050\r\n";
        let t = conf_text_with(t, "GAME_SVR_IP", "club.example.org").unwrap();
        let t = conf_text_with(&t, "GAME_SVR_PORT", "20060").unwrap();
        assert_eq!(t, b"A=1\r\nGAME_SVR_IP=club.example.org\r\nGAME_SVR_PORT=20060\r\n");
        assert_eq!(conf_text_with(b"A=1", "K", "v").unwrap(), b"A=1\nK=v\n");
    }
}
