//! REPORT A PROBLEM: the player's own words and this kit's logs, sent to the project's seat desk (the lobby port,
//! [`super::SEAT_PORT`] or the 2017-18 desk's), which keeps them for the people who run the server.
//!
//! Wire, one report per connection:
//! `REPORT ZIP NOTE VERSION\n` -> `GO\n` | `NO reason\n`; after `GO`, NOTE bytes of UTF-8 (the player's words) and then
//! ZIP bytes (the logs, [`pack`]) -> `OK ID\n` | `NO reason\n`. Refusals that need no payload (too big, too many,
//! busy) come before it is sent.
//!
//! The ZIP is [`send_logs::pack`]'s, scrubbed the same way (the user folder, the user and PC names), within
//! [`MAX_ZIP`]; `report.txt` in it holds the summary the Launcher writes (edition, versions, why).

use super::UploadReply;
use super::send_logs::{self, Scrub, SendError};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// The desk takes a ZIP up to this size.
pub const MAX_ZIP: usize = 4 * 1024 * 1024;
/// The player's words: at most this many characters (and 4 bytes each).
pub const MAX_NOTE_CHARS: usize = 2000;
pub const MAX_NOTE_BYTES: usize = MAX_NOTE_CHARS * 4;

/// The first line of a report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub zip: usize,
    pub note: usize,
    pub version: String,
}

impl Head {
    pub fn line(&self) -> String {
        format!("REPORT {} {} {}\n", self.zip, self.note, self.version)
    }

    /// Four words, the first `REPORT`, two counts of digits (the desk then checks them against the caps).
    pub fn parse(line: &str) -> Option<Head> {
        let num = |s: &str| {
            (!s.is_empty() && s.len() <= 12 && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse().ok())
                .flatten()
        };
        match line.split_whitespace().collect::<Vec<_>>()[..] {
            ["REPORT", zip, note, version] => Some(Head {
                zip: num(zip)?,
                note: num(note)?,
                version: version.into(),
            }),
            _ => None,
        }
    }
}

/// The player's words as sent: scrubbed like the logs, at most [`MAX_NOTE_CHARS`].
pub fn note(text: &str, scrub: &Scrub) -> String {
    let clean = scrub.scrub(text.trim().as_bytes().to_vec());
    String::from_utf8_lossy(&clean)
        .chars()
        .take(MAX_NOTE_CHARS)
        .collect()
}

/// The logs ZIP, `summary` as its `report.txt`.
pub fn pack(kit: &Path, game: Option<&Path>, scrub: &Scrub, summary: &str) -> io::Result<Vec<u8>> {
    let (blob, _) = send_logs::pack_within(
        kit,
        game,
        scrub,
        send_logs::TAIL,
        Some(summary),
        MAX_ZIP - 64 * 1024,
    )?;
    if blob.len() > MAX_ZIP {
        return Err(io::Error::other(format!(
            "the logs are too big ({} bytes)",
            blob.len()
        )));
    }
    Ok(blob)
}

fn line(s: &mut TcpStream) -> Result<String, SendError> {
    let mut answer = Vec::new();
    let mut b = [0u8; 1];
    while answer.len() < 200 {
        match s.read(&mut b) {
            Ok(0) => break,
            Ok(_) if b[0] == b'\n' => break,
            Ok(_) => answer.push(b[0]),
            Err(e) => return Err(SendError::NoAnswer(e.kind())),
        }
    }
    Ok(String::from_utf8_lossy(&answer).trim().to_owned())
}

/// Sends one report to the desk at `addr`; the id the desk gave it. `timeout` bounds each read and write.
pub fn send(
    addr: SocketAddr,
    zip: &[u8],
    note: &str,
    version: &str,
    timeout: Duration,
) -> Result<String, SendError> {
    let io_err = |e: io::Error| SendError::NoAnswer(e.kind());
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(15)).map_err(io_err)?;
    s.set_read_timeout(Some(timeout)).map_err(io_err)?;
    s.set_write_timeout(Some(timeout)).map_err(io_err)?;
    let version: String = version
        .chars()
        .map(|c| if c.is_ascii_graphic() { c } else { '?' })
        .collect();
    let head = Head {
        zip: zip.len(),
        note: note.len(),
        version: if version.is_empty() {
            "dev".into()
        } else {
            version
        },
    };
    s.write_all(head.line().as_bytes()).map_err(io_err)?;
    match line(&mut s)?.as_str() {
        "GO" => {}
        other => {
            return Err(match UploadReply::parse(other) {
                Some(UploadReply::No(why)) => SendError::Refused(why),
                _ => SendError::Garbled,
            });
        }
    }
    s.write_all(note.as_bytes()).map_err(io_err)?;
    s.write_all(zip).map_err(io_err)?;
    s.shutdown(Shutdown::Write).map_err(io_err)?;
    match UploadReply::parse(&line(&mut s)?) {
        Some(UploadReply::Ok(id)) => Ok(id),
        Some(UploadReply::No(why)) => Err(SendError::Refused(why)),
        None => Err(SendError::Garbled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use send_logs::user_folders;

    #[test]
    fn head_round_trips_and_refuses_junk() {
        let h = Head {
            zip: 1234,
            note: 56,
            version: "0.3.2".into(),
        };
        assert_eq!(Head::parse(h.line().trim_end()), Some(h));
        for bad in [
            "REPORT",
            "REPORT 1 2",
            "REPORT x 2 v",
            "REPORT 1 -2 v",
            "REPORT 1 2 v extra",
            "REPORT 9999999999999 2 v",
            "WANT",
        ] {
            assert_eq!(Head::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn user_folders_become_userprofile_on_any_drive_and_case() {
        let s = |t: &str| String::from_utf8(user_folders(t.as_bytes())).unwrap();
        assert_eq!(
            s(r"C:\Users\Mario\AppData\x.log"),
            r"%USERPROFILE%\AppData\x.log"
        );
        assert_eq!(s(r"d:\USERS\Ann Lee\Desktop"), r"%USERPROFILE%\Desktop");
        assert_eq!(
            s("e:/users/bob/x and E:\\\\Users\\\\bob\\\\y"),
            "%USERPROFILE%/x and %USERPROFILE%\\\\y"
        );
        assert_eq!(s("\"C:\\Users\\zed\" ok"), "\"%USERPROFILE%\" ok");
        assert_eq!(
            s(r"C:\Users\ then C:\Program Files\x"),
            r"C:\Users\ then C:\Program Files\x"
        );
        assert_eq!(s("no path here: Users"), "no path here: Users");
    }

    #[test]
    fn note_is_scrubbed_and_capped() {
        let scrub = Scrub {
            home: r"C:\Users\Mario".into(),
            user: "Mario".into(),
            pc: "DESKTOP-ABC".into(),
            folders: true,
        };
        let n = note(
            r" Mario here, on DESKTOP-ABC: crash in D:\Users\Mario\Games ",
            &scrub,
        );
        assert_eq!(n, r"USER here, on PC: crash in %USERPROFILE%\Games");
        assert_eq!(
            note(&"é".repeat(3000), &scrub).chars().count(),
            MAX_NOTE_CHARS
        );
    }
}
