//! The project server's notice to players (`forza::online::lobby::ServerNotice`): a thread asks the project server's
//! `LOBBY` at the start and every [`EVERY`] (a game running too), the ONLINE lobby's own answers from that server feed
//! it as well, and each change goes to `data\notice.json` for the panel, which shows each id once. The project server
//! is asked (`lobby::project_server`), or the LAN arcade's desk while a seat plays there ([`Notices::ask`]): a seat
//! shows the notice of the server it plays on. The ask carries the edition and nothing else.

use crate::lobby::{Wake, socket};
use forza::online::lobby::{self, Lobby, ServerNotice};
use forza::service::Log;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Between two asks of the watcher.
pub const EVERY: Duration = Duration::from_secs(60);
/// The watcher does not ask when an answer came this recently (the lobby's 5 s polls while ONLINE shows): the desk
/// answers a second `LOBBY` from one address within 2 s with `ERR slow`.
const FRESH: Duration = Duration::from_secs(50);
/// The first ask waits this long for the window's own first lobby poll, whose answer sets `at`; with no poll (never
/// on the ONLINE page) it asks then. Asking before that poll lands put both asks from one address inside the desk's
/// 2 s gap, and one of them got `ERR slow`.
const FIRST_WAIT: Duration = Duration::from_secs(8);
/// How often the first wait looks again for that poll's answer.
const FIRST_STEP: Duration = Duration::from_secs(1);

struct Inner {
    file: PathBuf,
    /// The server asked: the project server, or a LAN arcade's desk while a seat plays there.
    server: String,
    notice: Option<ServerNotice>,
    at: Option<Instant>,
    /// The file holds `notice`: `false` after a write that failed, so the next answer tries again.
    saved: bool,
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
            server: String::new(),
            notice: None,
            at: None,
            saved: true,
        })))
    }

    /// The watcher: asks `server` (later the one [`Notices::ask`] names) for `edition` at the start and every `every`,
    /// while no other answer came within [`FRESH`]; it ends when the last other copy of this box is dropped. `log`
    /// hears about each change and each write that failed.
    pub fn watch(&self, server: String, edition: &'static str, every: Duration, wake: Wake, log: Log) {
        self.lock().server = server;
        let me = self.clone();
        thread::spawn(move || {
            let started = Instant::now();
            while me.lock().waiting_for_the_poll(started, Instant::now()) {
                thread::sleep(FIRST_STEP);
            }
            loop {
                // the window's box is gone (a renderer that failed made it): this watcher too
                if Arc::strong_count(&me.0) == 1 {
                    return;
                }
                let (server, due) = {
                    let i = me.lock();
                    (i.server.clone(), i.at.is_none_or(|t| t.elapsed() >= FRESH))
                };
                if due
                    && let Some(a) = socket(&server)
                    && let Ok(l) = lobby::fetch(a, edition)
                    && me.heard(&l, log.as_ref())
                {
                    wake();
                }
                thread::sleep(every);
            }
        });
    }

    /// An answer from the project server: its notice (cleaned again: it came over the network) replaces the last; a
    /// change is written to the file, and a write that failed is tried again on the next answer. True when the
    /// notice changed.
    pub fn heard(&self, l: &Lobby, log: &dyn Fn(&str)) -> bool {
        let n = l.notice.clone().and_then(ServerNotice::clean);
        let mut i = self.lock();
        i.at = Some(Instant::now());
        let changed = i.notice != n;
        if !changed && i.saved {
            return false;
        }
        i.notice = n;
        match i.save() {
            Ok(()) => {
                i.saved = true;
                if changed {
                    match &i.notice {
                        Some(n) => log(&format!(
                            "notice: {} from the server, written to {}",
                            n.id,
                            i.file.display()
                        )),
                        None => log(&format!("notice: none from the server, {} removed", i.file.display())),
                    }
                }
            }
            Err(e) => {
                i.saved = false;
                log(&format!(
                    "notice: {} not written ({e}), again on the next answer",
                    i.file.display()
                ));
            }
        }
        changed
    }

    /// Asks `server` from the watcher's next round on (a LAN arcade's desk while a seat plays there, the project server
    /// after); its first answer replaces the notice of the server asked before.
    pub fn ask(&self, server: &str) {
        let mut i = self.lock();
        if i.server != server {
            i.server = server.to_owned();
            i.at = None;
        }
    }

    /// The server asked now ([`Notices::ask`]).
    pub fn server(&self) -> String {
        self.lock().server.clone()
    }

    /// The text in language `code`, else English; none without a notice.
    pub fn text(&self, code: &str) -> Option<String> {
        self.lock().notice.as_ref().map(|n| n.text(code).to_owned())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Inner {
    /// The notice into the file, or the file removed when the server sends none.
    fn save(&self) -> std::io::Result<()> {
        match &self.notice {
            Some(n) => crate::choices::write(&self.file, &serde_json::to_string(n).expect("plain data serializes")),
            None => match std::fs::remove_file(&self.file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        }
    }

    /// The first ask waits for the window's own lobby poll (its answer sets `at`), no longer than [`FIRST_WAIT`].
    fn waiting_for_the_poll(&self, started: Instant, now: Instant) -> bool {
        self.at.is_none() && now.duration_since(started) < FIRST_WAIT
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

    fn inner(at: Option<Instant>) -> Inner {
        Inner {
            file: PathBuf::new(),
            server: String::new(),
            notice: None,
            at,
            saved: true,
        }
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forza_notice_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_notice_goes_to_the_panels_file_in_the_players_language() {
        let dir = dir("lang");
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
        assert!(n.heard(&answer(Some(sent.clone())), &|_| {}));
        assert_eq!(n.text("ja").as_deref(), Some("23:00に 更新"));
        assert_eq!(n.text("it").as_deref(), Some("Server update at 23:00"));
        let kept: ServerNotice = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(kept, sent.clean().unwrap());
        assert!(
            !n.heard(&answer(kept.clone().into()), &|_| {}),
            "the same notice: no change"
        );
        assert!(n.heard(&answer(None), &|_| {}));
        assert!(!file.exists() && n.text("en").is_none());
    }

    /// A write that fails (the file held, a scanner on the fresh file) must not lose the notice: the next lobby
    /// answer tries again, even though the notice itself did not change.
    #[test]
    fn a_failed_write_is_tried_again_on_the_next_answer() {
        let dir = dir("retry");
        let file = dir.join("notice.json");
        let n = Notices::new(file.clone());
        std::fs::create_dir_all(&file).unwrap(); // the rename onto it fails
        let a = answer(Some(ServerNotice {
            id: "n7".into(),
            en: "Server update at 23:00".into(),
            ..ServerNotice::default()
        }));
        let said = Mutex::new(Vec::new());
        let log = |line: &str| said.lock().unwrap().push(line.to_owned());
        assert!(n.heard(&a, &log), "the notice changed");
        assert!(!file.is_file(), "the write failed, so the file is not there");
        assert!(said.lock().unwrap().iter().any(|l| l.contains("not written")));
        std::fs::remove_dir(&file).unwrap();
        assert!(!n.heard(&a, &log), "the same notice, not a change");
        let kept: ServerNotice = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(kept.id, "n7", "the retry wrote it");
        let _ = std::fs::remove_file(&file);
    }

    /// A LAN seat shows its own server's notice: `ask` turns the watcher to that desk at once (no wait for
    /// [`FRESH`]), and back again after the run.
    #[test]
    fn ask_turns_to_the_server_the_seat_plays_on() {
        let n = Notices::new(dir("ask").join("notice.json"));
        n.lock().server = "lobby.wccf.it:20032".into();
        n.heard(&answer(None), &|_| {});
        assert!(n.lock().at.is_some());
        n.ask("192.168.1.20");
        assert_eq!(n.server(), "192.168.1.20");
        assert!(n.lock().at.is_none(), "the LAN desk is asked on the next round");
        n.heard(&answer(None), &|_| {});
        n.ask("192.168.1.20");
        assert!(n.lock().at.is_some(), "the same server: no extra ask");
        n.ask("lobby.wccf.it:20032");
        assert_eq!(n.server(), "lobby.wccf.it:20032");
    }

    /// The one-ask-first race: the watcher waits for the window's own poll before its first ask, so the two never
    /// land inside the desk's 2 s gap (one of them got `ERR slow`).
    #[test]
    fn the_first_ask_waits_for_the_windows_poll() {
        let t = Instant::now();
        let quiet = inner(None);
        assert!(quiet.waiting_for_the_poll(t, t), "the poll goes first");
        assert!(quiet.waiting_for_the_poll(t, t + FIRST_WAIT - FIRST_STEP));
        assert!(
            !quiet.waiting_for_the_poll(t, t + FIRST_WAIT),
            "no poll answered: this ask goes"
        );
        let polled = inner(Some(t));
        assert!(
            !polled.waiting_for_the_poll(t, t + FIRST_STEP),
            "the poll answered: this ask stands back"
        );
    }
}
