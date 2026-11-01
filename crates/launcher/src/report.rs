//! REPORT A PROBLEM: the player's words and this kit's logs saved in one ZIP on this PC
//! ([`forza::online::save_logs`] packs it), for the player to send to whoever helps them. Also the plain words for
//! network failures: players never see a tool's own error text, the log keeps it.

use crate::Kit;
use forza::online::save_logs::{self, Scrub};
use std::io;
use std::path::{Path, PathBuf};

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

/// What a report holds, by the names in its ZIP; `game` is the game folder (its logs and seat 1's).
pub fn contents(kit: &Kit, game: Option<&Path>) -> Vec<String> {
    let mut v = vec!["report.txt".to_owned()];
    v.extend(save_logs::names(kit.dir(), game));
    v
}

/// [`save_logs::save`] after a lean snapshot of the game programs running from `game` (they ride along in `dumps/`).
pub fn save_zip(dir: &Path, kit: &Path, game: Option<&Path>, scrub: &Scrub, note: Option<&str>) -> io::Result<PathBuf> {
    if let Some(game) = game {
        crate::debug::dump_running(game, &kit.join("data").join("dumps"));
    }
    save_logs::save(dir, kit, game, scrub, note)
}

/// Saves a report in `dir`: `report.txt` holds a summary (edition, versions, why) and the player's words; the ZIP's
/// path.
pub fn save(kit: &Kit, game: Option<&Path>, words: &str, why: &str, dir: &Path) -> io::Result<PathBuf> {
    let scrub = Scrub::from_env();
    let summary = format!(
        "edition: {}\nversion: {}\nlauncher: {}\nwhy: {why}\n\n{}\n",
        kit.edition().name(),
        save_logs::version(kit.dir()),
        env!("FORZA_VERSION"),
        save_logs::note(words, &scrub),
    );
    save_zip(dir, kit.dir(), game, &scrub, Some(&summary))
}

/// GitHub's new-issue page; [`issue_url`] fills it in.
pub const NEW_ISSUE: &str = "https://github.com/Forza-WCCF/Forza-WCCF/issues/new";

/// A new-issue link with the title and body filled in: the versions, Windows', why, and the last error lines of the
/// Launcher's logs, scrubbed like the ZIP; the player attaches the ZIP by hand.
pub fn issue_url(kit: &Kit, why: &str) -> String {
    let scrub = Scrub::from_env();
    let mut errors = Vec::new();
    for log in ["launcher.txt", "run_launcher.txt"] {
        let text = std::fs::read(kit.logs().join(log)).unwrap_or_default();
        let text = String::from_utf8_lossy(&scrub.scrub(text)).into_owned();
        errors.extend(
            text.lines()
                .filter(|l| {
                    let l = l.to_ascii_lowercase();
                    l.contains("error") || l.contains("fail")
                })
                .map(|l| l.trim().chars().take(160).collect::<String>()),
        );
    }
    let errors = errors[errors.len().saturating_sub(5)..].join("\n");
    let version = save_logs::version(kit.dir());
    let body = format!(
        "Version: {version} (Launcher {}), {}\nWindows: {}\nWhy: {why}\n\nLast errors:\n```\n{errors}\n```\n\nWhat happened:\n\n\n(Attach the forza-wccf-logs ZIP here.)\n",
        env!("FORZA_VERSION"),
        kit.edition().name(),
        windows_version(),
    );
    format!(
        "{NEW_ISSUE}?title={}&body={}",
        encode(&format!("Problem: {} {version}", kit.edition().name())),
        encode(&body)
    )
}

/// `cmd /c ver`'s "Microsoft Windows [Version 10.0.26100.1]"; "?" when it gives nothing.
fn windows_version() -> String {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if let Ok(o) = std::process::Command::new("cmd")
            .args(["/c", "ver"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            let v = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if !v.is_empty() {
                return v;
            }
        }
    }
    "?".into()
}

/// Percent-encoding for a URL's query: everything but `A-Z a-z 0-9 - . _ ~`.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
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
    fn the_issue_link_is_encoded_and_carries_the_last_errors() {
        assert_eq!(encode("a b&c=é\n"), "a%20b%26c%3D%C3%A9%0A");
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::at(dir.path());
        std::fs::create_dir_all(kit.logs()).unwrap();
        std::fs::write(kit.logs().join("launcher.txt"), "ok\nerror: one\nfine\n").unwrap();
        let url = issue_url(&kit, "after a crash");
        assert!(url.starts_with(&format!("{NEW_ISSUE}?title=Problem%3A%20")), "{url}");
        assert!(url.contains("error%3A%20one") && !url.contains("fine"), "{url}");
        assert!(!url.contains(' ') && !url.contains('\n'), "{url}");
    }

    #[test]
    fn a_report_is_saved_with_the_words() {
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::at(dir.path());
        std::fs::create_dir_all(kit.logs()).unwrap();
        std::fs::write(kit.logs().join("run_seat1.txt"), "crash\n").unwrap();
        assert!(contents(&kit, None).contains(&"logs/run_seat1.txt".to_owned()));
        let out = dir.path().join("out");
        let zip = save(&kit, None, "  it froze  ", "opened by the player", &out).unwrap();
        assert_eq!(zip.parent(), Some(out.as_path()));
        let mut z = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
        let mut t = String::new();
        std::io::Read::read_to_string(&mut z.by_name("report.txt").unwrap(), &mut t).unwrap();
        assert!(
            t.contains("why: opened by the player") && t.ends_with("\n\nit froze\n"),
            "{t}"
        );
        assert!(z.by_name("logs/run_seat1.txt").is_ok());
    }
}
