//! The project server's notice to players (`forza::online::lobby::ServerNotice`): a thread asks the project server's
//! `LOBBY` at the start and every [`EVERY`] (a game running too), the ONLINE lobby's own answers from that server feed
//! it as well, and each change goes to `data\notice.json` for the panel, which shows each id once. Only the project
//! server is asked (`lobby::project_server`); the ask carries the edition and nothing else.

use crate::lobby::{Wake, socket};
use forza::online::lobby::{self, Lobby, ServerNotice};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Between two asks of the watcher.
pub const EVERY: Duration = Duration::from_secs(60);
/// The watcher does not ask when an answer came this recently (the lobby's 5 s polls while ONLINE shows): the desk
/// answers a second `LOBBY` from one address within 2 s with `ERR slow`.
const FRESH: Duration = Duration::from_secs(50);

struct Inner {
    file: PathBuf,
    notice: Option<ServerNotice>,
    at: Option<Instant>,
}

/// The notice as last heard; shared by the window and the watcher thread.
#[derive(Clone)]
pub struct Notices(Arc<Mutex<Inner>>);

impl Notices {
    /// No notice yet; `file` is `data\notice.json`, removed until the server sends one (an old one is stale).
    pub fn new(file: PathBuf) -> Notices {
        let _ = std::fs::remove_file(&file);
        Notices(Arc::new(Mutex::new(Inner {
            file,
            notice: None,
            at: None,
        })))
    }

    /// The watcher: asks `server` for `edition` now and every `every`, while no other answer came within [`FRESH`];
    /// it ends when the last other copy of this box is dropped.
    pub fn watch(&self, server: String, edition: &'static str, every: Duration, wake: Wake) {
        let me = self.clone();
        thread::spawn(move || {
            loop {
                // the window's box is gone (a renderer that failed made it): this watcher too
                if Arc::strong_count(&me.0) == 1 {
                    return;
                }
                if me.lock().at.is_none_or(|t| t.elapsed() >= FRESH)
                    && let Some(a) = socket(&server)
                    && let Ok(l) = lobby::fetch(a, edition)
                    && me.heard(&l)
                {
                    wake();
                }
                thread::sleep(every);
            }
        });
    }

    /// An answer from the project server: its notice (cleaned again: it came over the network) replaces the last; a
    /// change is written to the file. True when it changed.
    pub fn heard(&self, l: &Lobby) -> bool {
        let n = l.notice.clone().and_then(ServerNotice::clean);
        let mut i = self.lock();
        i.at = Some(Instant::now());
        if i.notice == n {
            return false;
        }
        let _ = match &n {
            Some(n) => crate::choices::write(&i.file, &serde_json::to_string(n).expect("plain data serializes")),
            None => std::fs::remove_file(&i.file),
        };
        i.notice = n;
        true
    }

    /// The text in language `code`, else English; none without a notice.
    pub fn text(&self, code: &str) -> Option<String> {
        self.lock().notice.as_ref().map(|n| n.text(code).to_owned())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(notice: Option<ServerNotice>) -> Lobby {
        Lobby::parse(r#"{"v":1,"edition":"2017-18","at":1}"#)
            .map(|l| Lobby { notice, ..l })
            .unwrap()
    }

    #[test]
    fn a_notice_goes_to_the_panels_file_in_the_players_language() {
        let dir = std::env::temp_dir().join(format!("forza_notices_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notice.json");
        std::fs::write(&file, "stale").unwrap();
        let n = Notices::new(file.clone());
        assert!(!file.exists(), "a notice of an earlier run is gone");
        let sent = ServerNotice {
            id: "n1".into(),
            en: "Server update at 23:00".into(),
            it: None,
            ja: Some("23:00に\u{7}更新".into()),
        };
        assert!(n.heard(&answer(Some(sent.clone()))));
        assert_eq!(n.text("ja").as_deref(), Some("23:00に 更新"));
        assert_eq!(n.text("it").as_deref(), Some("Server update at 23:00"));
        let kept: ServerNotice = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(kept, sent.clean().unwrap());
        assert!(!n.heard(&answer(kept.clone().into())), "the same notice: no change");
        assert!(n.heard(&answer(None)));
        assert!(!file.exists() && n.text("en").is_none());
    }
}
