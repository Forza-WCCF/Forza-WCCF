//! REPORT A PROBLEM: the player's words and this kit's logs to the project's server ([`forza::online::report`] has the
//! wire and the ZIP). Each report is first kept in `data\reports\` (`DATE-TIME.zip` and the words in `.txt`); a sent
//! one is removed, one that could not go waits there and goes with the next report. Also the plain words for network
//! failures: players never see a tool's own error text, the log keeps it.

use crate::{Edition, Kit, lobby};
use forza::online::SEAT_PORT;
use forza::online::report;
use forza::online::send_logs::{self, Scrub, SendError};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Reports that waited in `data\reports\` and go after a new one, at most.
const BACKLOG: usize = 2;

/// Why a report did not go, as the player reads it: each a `ui.yml` key ([`Failure::key`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Offline,
    NoAnswer,
    Busy,
    TooMany,
    Refused,
    /// The logs could not be packed or kept (a full disk).
    Pack,
}

impl Failure {
    pub fn key(self) -> &'static str {
        match self {
            Failure::Offline => "net.offline",
            Failure::NoAnswer => "net.no_answer",
            Failure::Busy => "report.busy",
            Failure::TooMany => "report.too_many",
            Failure::Refused => "report.refused",
            Failure::Pack => "report.pack_failed",
        }
    }

    fn of(e: &SendError) -> Failure {
        match e {
            SendError::NoAnswer(_) | SendError::Garbled => Failure::NoAnswer,
            SendError::Refused(why) if why.starts_with("busy") => Failure::Busy,
            SendError::Refused(why) if why.starts_with("too many") => Failure::TooMany,
            SendError::Refused(_) => Failure::Refused,
        }
    }
}

/// A network tool's error text (curl's, a socket's, the lobby workers') as a `ui.yml` key: `net.offline` when no name
/// could be looked up, `net.no_answer` when nothing answered or the line broke, else `net.failed`.
pub fn plain(raw: &str) -> &'static str {
    let r = raw.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| r.contains(w));
    if has(&[
        "curl: (6)",
        "could not resolve",
        "no such host",
        "os error 11001",
        "name or service not known",
        "is not an address",
    ]) {
        "net.offline"
    } else if has(&[
        "curl: (7)",
        "curl: (28)",
        "curl: (52)",
        "curl: (56)",
        "timed out",
        "refused",
        "reset",
        "aborted",
        "unreachable",
        "did not answer",
        "os error 10060",
        "os error 10061",
        "os error 10065",
        "connectionrefused",
        "timedout",
    ]) {
        "net.no_answer"
    } else {
        "net.failed"
    }
}

/// The server reports go to: the edition's project server (`WCCF_LOBBY` overrides it, as for the lobby).
pub fn server(edition: Edition) -> String {
    lobby::project_server(edition)
}

/// What a report holds, by the names in its ZIP; `game` is the game folder (its logs and seat 1's).
pub fn contents(kit: &Kit, game: Option<&Path>) -> Vec<String> {
    let mut v = vec!["report.txt".to_owned()];
    v.extend(send_logs::names(kit.dir(), game));
    v
}

fn folder(kit: &Kit) -> PathBuf {
    kit.data().join("reports")
}

/// Packs a report and keeps it in `data\reports\`: the ZIP's path.
pub fn keep(kit: &Kit, game: Option<&Path>, words: &str, why: &str) -> io::Result<PathBuf> {
    let scrub = Scrub::from_env();
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let summary = format!(
        "edition: {}\nversion: {}\nlauncher: {}\nmade: {stamp}\nwhy: {why}\n",
        kit.edition().name(),
        send_logs::version(kit.dir()),
        env!("CARGO_PKG_VERSION"),
    );
    let zip = report::pack(kit.dir(), game, &scrub, &summary)?;
    let dir = folder(kit);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{stamp}.zip"));
    fs::write(path.with_extension("txt"), report::note(words, &scrub))?;
    fs::write(&path, zip)?;
    Ok(path)
}

/// Sends the kept report at `zip` (its words beside it); removed once the server took it.
fn send_kept(at: std::net::SocketAddr, kit: &Kit, zip: &Path) -> Result<String, SendError> {
    let io_err = |e: io::Error| SendError::NoAnswer(e.kind());
    let blob = fs::read(zip).map_err(io_err)?;
    let words = fs::read_to_string(zip.with_extension("txt")).unwrap_or_default();
    let id = report::send(
        at,
        &blob,
        &words,
        &send_logs::version(kit.dir()),
        Duration::from_secs(60),
    )?;
    let _ = fs::remove_file(zip);
    let _ = fs::remove_file(zip.with_extension("txt"));
    Ok(id)
}

/// The report kept at `zip` to the edition's server, then up to [`BACKLOG`] older ones that waited. The id the server
/// gave; on a failure the report stays kept, and the error's own text comes with the player's words for the log.
pub fn send(kit: &Kit, zip: &Path) -> Result<String, (Failure, String)> {
    let server = server(kit.edition());
    let Some(at) = crate::choices::resolve(&server, SEAT_PORT) else {
        return Err((Failure::Offline, format!("{server}: no address")));
    };
    let id = send_kept(at, kit, zip).map_err(|e| (Failure::of(&e), format!("{server}: {e}")))?;
    let mut older: Vec<PathBuf> = fs::read_dir(folder(kit))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    older.retain(|p| p.extension().is_some_and(|x| x == "zip"));
    older.sort();
    for p in older.into_iter().take(BACKLOG) {
        if send_kept(at, kit, &p).is_err() {
            break;
        }
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_errors_become_plain_words() {
        assert_eq!(plain("curl: (6) Could not resolve host: api.github.com"), "net.offline");
        assert_eq!(plain("lobby.wccf.it:20032 is not an address"), "net.offline");
        assert_eq!(
            plain("curl: (28) Connection timed out after 20001 milliseconds"),
            "net.no_answer"
        );
        assert_eq!(plain("curl: (7) Failed to connect to host port 443"), "net.no_answer");
        assert_eq!(
            plain("No connection could be made because the target machine actively refused it. (os error 10061)"),
            "net.no_answer"
        );
        assert_eq!(
            plain("the server's log inbox did not answer (TimedOut)"),
            "net.no_answer"
        );
        assert_eq!(
            plain("curl: (35) schannel: next InitializeSecurityContext failed"),
            "net.failed"
        );
    }

    #[test]
    fn server_refusals_have_their_own_words() {
        let f = |why: &str| Failure::of(&SendError::Refused(why.into()));
        assert_eq!(f("busy - try again in a minute"), Failure::Busy);
        assert_eq!(f("too many from this address - try again in an hour"), Failure::TooMany);
        assert_eq!(f("not a ZIP"), Failure::Refused);
        assert_eq!(
            Failure::of(&SendError::NoAnswer(io::ErrorKind::ConnectionRefused)),
            Failure::NoAnswer
        );
    }

    #[test]
    fn a_report_is_kept_then_sent_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::at(dir.path());
        fs::create_dir_all(kit.logs()).unwrap();
        fs::write(kit.logs().join("run_seat1.txt"), "crash in C:\\Users\\Someone\\x\n").unwrap();
        let zip = keep(&kit, None, "  it froze  ", "opened by the player").unwrap();
        assert_eq!(fs::read_to_string(zip.with_extension("txt")).unwrap(), "it froze");
        assert!(contents(&kit, None).contains(&"logs/run_seat1.txt".to_owned()));
        let mut c = forza_server::desk::DeskConfig::new(Edition::E1011);
        c.bind = "127.0.0.1".into();
        c.port = 0;
        let store = dir.path().join("server");
        c.reports = Some(std::sync::Arc::new(forza_server::reports::Reports::new(
            forza_server::reports::ReportConfig::new(store.clone()),
        )));
        let desk = forza_server::desk::start(c, std::sync::Arc::new(|_: &str| {})).unwrap();
        let id = send_kept(desk.addr(), &kit, &zip).unwrap();
        assert_eq!(id.len(), 6);
        assert!(!zip.exists() && !zip.with_extension("txt").exists());
        let kept: Vec<_> = fs::read_dir(&store).unwrap().flatten().collect();
        assert_eq!(kept.len(), 2);
    }
}
