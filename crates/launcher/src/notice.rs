//! The project server's notice to players (`forza::online::lobby::ServerNotice`): a thread asks the project server's
//! `LOBBY` at the start and every [`EVERY`] (a game running too), the ONLINE lobby's own answers from that server feed
//! it as well, and each change goes to `data\notice.json` for the panel, which shows each id once. The project server
//! is asked (`lobby::project_server`), or the LAN arcade's desk while a seat plays there ([`Notices::ask`]): a seat
//! shows the notice of the server it plays on. The ask carries the edition and nothing else. An arcade guest asks the
//! host's desk through the run's tunnel ([`TUNNEL_DESK`]); another server's notice shows under an "Arcade notice:"
//! prefix (added as shown: [`LAN_FILE`] keeps it as heard), and the project server's stays when that server has
//! none ([`round`]).

use crate::lobby::{Wake, socket};
use forza::online::SEAT_PORT;
use forza::online::lobby::{self, Lobby, ServerNotice};
use forza::service::Log;
use forza_ui::Lang;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
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

/// The loopback port of the host's desk behind the arcade tunnel this PC's seat plays through, 0 with none: the run
/// sets it once the tunnel is up (never assumed to be 20030: a server on this PC moves it) and clears it as it stops.
pub static TUNNEL_DESK: AtomicU16 = AtomicU16::new(0);

/// The host's desk through the run's arcade tunnel (`127.0.0.1:port`), while one is up ([`TUNNEL_DESK`]).
pub fn tunnel_desk() -> Option<String> {
    let port = TUNNEL_DESK.load(Ordering::Relaxed);
    (port != 0).then(|| format!("127.0.0.1:{port}"))
}

/// HOST ARCADE: the host's notice (`text`, data\settings.json `arcade_notice`, at most
/// [`lobby::HOST_NOTICE_LEN`] characters kept) into `file` for its desk (`WCCF_NOTICE`), under an id of this hosting
/// session (`now`, Unix seconds), so each guest's panel shows it once; the file is removed when the text is empty.
/// True when the file holds a notice.
pub fn write_host_notice(file: &Path, text: &str, now: u64) -> std::io::Result<bool> {
    let n = ServerNotice {
        id: format!("h{now}"),
        en: text.chars().take(lobby::HOST_NOTICE_LEN).collect(),
        ..ServerNotice::default()
    }
    .clean();
    match n {
        Some(n) => {
            crate::choices::write(file, &serde_json::to_string(&n).expect("plain data serializes")).map(|()| true)
        }
        None => remove(file).map(|()| false),
    }
}

/// `file` gone, whether it was there or not.
fn remove(file: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The file a HOST LAN ARCADE's desk serves its guests (`WCCF_NOTICE`), next to `notice.json`: the notice as heard,
/// never under the "Arcade notice:" prefix, which each guest adds itself (#419).
pub const LAN_FILE: &str = "lan_notice.json";

/// A round's notice, cleaned and as its server sent it; `desk`: it shows under the "Arcade notice:" prefix
/// ([`Heard::shown`]). No notice with `desk` set is a desk's "none" with the project server out of reach: it clears
/// only a desk's notice ([`Inner::hear`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Heard {
    notice: Option<ServerNotice>,
    desk: bool,
}

impl Heard {
    /// The project server's answer.
    fn project(l: &Lobby) -> Heard {
        Heard {
            notice: l.notice.clone().and_then(ServerNotice::clean),
            desk: false,
        }
    }

    /// The notice as the Launcher and the panel show it: a desk's under the prefix.
    fn shown(&self) -> Option<ServerNotice> {
        self.notice
            .as_ref()
            .and_then(|n| if self.desk { arcade_notice(n) } else { Some(n.clone()) })
    }
}

/// One round's answer from `asked` (`get` asks a server). From a server other than `project` (an arcade's desk, a
/// LAN arcade, a listed server's seat), its notice is a desk's (shown prefixed); with none there, the project
/// server is asked in the same round and its notice kept. A notice without a host's id (`h<unix>`,
/// [`write_host_notice`]) that equals the project server's is that one: an older host's desk serves its Launcher's
/// copy of the project's notice. With the project server out of reach (a LAN arcade with no internet) that copy
/// cannot be told from the desk's own, so it is a desk's rather than not shown at all, and the desk's "none" clears a
/// desk's notice (#419). `None` when the desk's ask failed: the last notice stays.
fn round(asked: &str, project: &str, get: impl Fn(&str) -> Option<Lobby>) -> Option<Heard> {
    let l = get(asked)?;
    if same_server(asked, project) {
        return Some(Heard::project(&l));
    }
    let ours = || get(project).map(|l| Heard::project(&l));
    Some(match l.notice.and_then(ServerNotice::clean) {
        Some(n) if host_id(&n.id) => Heard {
            notice: Some(n),
            desk: true,
        },
        Some(n) => {
            let desk = ours().and_then(|h| h.notice).as_ref() != Some(&n);
            Heard { notice: Some(n), desk }
        }
        None => ours().unwrap_or(Heard {
            notice: None,
            desk: true,
        }),
    })
}

/// `a` and `b` name one server as [`socket`] reads them: the seat desk's port by default, the name in any case. A
/// saved `servers` entry spelled another way than the project server would ask that desk twice inside its gap.
pub(crate) fn same_server(a: &str, b: &str) -> bool {
    let key = |s: &str| crate::choices::host_port(s, SEAT_PORT).map(|(h, p)| (h.to_ascii_lowercase(), p));
    a == b || key(a).is_some_and(|k| key(b) == Some(k))
}

/// `id` is a host's ([`write_host_notice`]): `h` and Unix seconds, nothing else.
fn host_id(id: &str) -> bool {
    id.strip_prefix('h')
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// A peer's notice under the prefix that says whose it is, in each language, cut to a notice's length again.
fn arcade_notice(n: &ServerNotice) -> Option<ServerNotice> {
    let say = |lang: Lang| forza_ui::text(lang, "launcher.arcade_notice", &[("text", n.text(lang.code()))]);
    ServerNotice {
        id: n.id.clone(),
        en: say(Lang::En),
        it: Some(say(Lang::It)),
        ja: Some(say(Lang::Ja)),
    }
    .clean()
}

struct Inner {
    /// `data\notice.json`, the panel's (the notice as shown); [`LAN_FILE`] next to it holds it as heard.
    file: PathBuf,
    /// The server asked: the project server, or a LAN arcade's desk while a seat plays there.
    server: String,
    notice: Heard,
    at: Option<Instant>,
    /// The file holds `notice`: `false` after a write that failed, so the next answer tries again.
    saved: bool,
}

/// The notice as last heard; shared by the window and the watcher thread.
#[derive(Clone)]
pub struct Notices(Arc<Mutex<Inner>>);

impl Notices {
    /// No notice yet; `file` is `data\notice.json`, removed until the server sends one (an old one is stale), and
    /// [`LAN_FILE`] with it.
    pub fn new(file: PathBuf) -> Notices {
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(file.with_file_name(LAN_FILE));
        Notices(Arc::new(Mutex::new(Inner {
            file,
            server: String::new(),
            notice: Heard::default(),
            at: None,
            saved: true,
        })))
    }

    /// The watcher: asks `server` (later the one [`Notices::ask`] names) for `edition` at the start and every `every`,
    /// while no other answer came within [`FRESH`]; it ends when the last other copy of this box is dropped. `log`
    /// hears about each change and each write that failed.
    pub fn watch(&self, server: String, edition: &'static str, every: Duration, wake: Wake, log: Log) {
        let project = server.clone();
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
                let get = |s: &str| socket(s).and_then(|a| lobby::fetch(a, edition).ok());
                if due
                    && let Some(h) = round(&server, &project, get)
                    && me.heard_from(&server, h, log.as_ref())
                {
                    wake();
                }
                // an `ask` for another server ends the wait: the run's end brings the project's notice back at once
                let napped = Instant::now();
                while me.lock().napping(&server, napped, Instant::now(), every) {
                    thread::sleep(FIRST_STEP.min(every));
                }
            }
        });
    }

    /// An answer from the server asked ([`round`]) or the ONLINE lobby's polls: its notice (cleaned again: it came over
    /// the network) replaces the last; a change is written to the file, and a write that failed is tried again on the
    /// next answer. True when the notice changed.
    pub fn heard(&self, l: &Lobby, log: &dyn Fn(&str)) -> bool {
        self.lock().hear(Heard::project(l), log)
    }

    /// The watcher's answer from `asked`, dropped when [`Notices::ask`] named another server while it was out (#419):
    /// a run that ended then would otherwise keep its host's notice until the next round.
    fn heard_from(&self, asked: &str, h: Heard, log: &dyn Fn(&str)) -> bool {
        let mut i = self.lock();
        i.server == asked && i.hear(h, log)
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

    /// The text in language `code`, else English, a desk's under the prefix; none without a notice.
    pub fn text(&self, code: &str) -> Option<String> {
        self.lock().notice.shown().map(|n| n.text(code).to_owned())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Inner {
    /// [`Notices::heard`] under the lock. A desk's "none" with the project server out of reach keeps the project's
    /// notice ([`round`]).
    fn hear(&mut self, n: Heard, log: &dyn Fn(&str)) -> bool {
        let i = self;
        i.at = Some(Instant::now());
        let n = if n.notice.is_none() && n.desk && !i.notice.desk {
            i.notice.clone()
        } else {
            Heard {
                desk: n.desk && n.notice.is_some(),
                ..n
            }
        };
        let changed = i.notice != n;
        if !changed && i.saved {
            return false;
        }
        i.notice = n;
        match i.save() {
            Ok(()) => {
                i.saved = true;
                if changed {
                    match &i.notice.notice {
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

    /// The notice as shown into the panel's file and as heard into [`LAN_FILE`], or both removed with none.
    fn save(&self) -> std::io::Result<()> {
        let put = |file: &Path, n: Option<ServerNotice>| match n {
            Some(n) => crate::choices::write(file, &serde_json::to_string(&n).expect("plain data serializes")),
            None => remove(file),
        };
        put(&self.file.with_file_name(LAN_FILE), self.notice.notice.clone())?;
        put(&self.file, self.notice.shown())
    }

    /// The watcher waits `every` after a round that asked `asked`, less when [`Notices::ask`] named another server.
    fn napping(&self, asked: &str, started: Instant, now: Instant, every: Duration) -> bool {
        self.server == asked && now.duration_since(started) < every
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
            notice: Heard::default(),
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

    /// An arcade guest (#328): the host's notice replaces the project server's, under the prefix in each language; a
    /// desk with none keeps the project's; after the run the project's is back.
    #[test]
    fn a_seat_shows_its_arcades_notice_over_the_projects() {
        let project = "lobby.wccf.it:20032";
        let desk = "127.0.0.1:20041";
        let ours = ServerNotice {
            id: "n1".into(),
            en: "Server update at 23:00".into(),
            ..ServerNotice::default()
        };
        let host = ServerNotice {
            id: "h17".into(),
            en: "Open till 23:00".into(),
            ..ServerNotice::default()
        };
        // the project server answers with `ours`, the desk with `hosts`
        fn answers<'a>(
            project: &'a str,
            ours: &'a ServerNotice,
            hosts: Option<&'a ServerNotice>,
        ) -> impl Fn(&str) -> Option<Lobby> + 'a {
            move |s| {
                Some(answer(if s == project {
                    Some(ours.clone())
                } else {
                    hosts.cloned()
                }))
            }
        }
        let n = Notices::new(dir("arcade").join("notice.json"));
        n.lock().hear(
            round(project, project, answers(project, &ours, Some(&host))).unwrap(),
            &|_| {},
        );
        assert_eq!(n.text("en").as_deref(), Some("Server update at 23:00"));
        n.ask(desk);
        assert!(n.lock().hear(
            round(desk, project, answers(project, &ours, Some(&host))).unwrap(),
            &|_| {}
        ));
        assert_eq!(n.text("en").as_deref(), Some("Arcade notice: Open till 23:00"));
        assert_eq!(n.text("it").as_deref(), Some("Avviso della sala: Open till 23:00"));
        assert_eq!(
            n.text("ja").as_deref(),
            Some("ゲームセンターのお知らせ：Open till 23:00")
        );
        assert_eq!(n.lock().notice.notice.as_ref().map(|n| n.id.as_str()), Some("h17"));
        // a full host notice and the longest prefix still fit a notice
        let long = ServerNotice {
            en: "x".repeat(lobby::HOST_NOTICE_LEN),
            ..host.clone()
        };
        let l = round(desk, project, answers(project, &ours, Some(&long))).unwrap();
        assert!(
            l.shown()
                .unwrap()
                .text("it")
                .ends_with(&"x".repeat(lobby::HOST_NOTICE_LEN))
        );
        // an older host's desk serves its copy of the project's notice: the project's, unprefixed; another server's
        // own notice keeps the prefix
        let l = round(desk, project, answers(project, &ours, Some(&ours))).unwrap();
        assert_eq!(l.shown().as_ref(), Some(&ours));
        let theirs = ServerNotice {
            id: "n2".into(),
            ..host.clone()
        };
        let l = round(desk, project, answers(project, &ours, Some(&theirs))).unwrap();
        assert_eq!(l.shown().unwrap().en, "Arcade notice: Open till 23:00");
        // the desk sets none: the project's stays; the desk not answering keeps the last
        let l = round(desk, project, answers(project, &ours, None)).unwrap();
        assert_eq!(l.shown().as_ref().map(|n| n.id.as_str()), Some("n1"));
        assert!(round(desk, project, |s| (s != desk).then(|| answer(None))).is_none());
        n.ask(project);
        assert!(n.lock().hear(
            round(project, project, answers(project, &ours, Some(&host))).unwrap(),
            &|_| {}
        ));
        assert_eq!(
            n.text("en").as_deref(),
            Some("Server update at 23:00"),
            "the run is over"
        );
    }

    /// Only `h<digits>` is a host's id (#419): a project notice whose id starts with `h` is still the project's.
    #[test]
    fn only_a_hosts_id_gets_the_arcade_prefix() {
        let project = "lobby.wccf.it:20032";
        let ours = ServerNotice {
            id: "hello".into(),
            en: "Server update at 23:00".into(),
            ..ServerNotice::default()
        };
        let host = ServerNotice {
            id: "h1795816800".into(),
            ..ours.clone()
        };
        let l = round("127.0.0.1:20041", project, |_| Some(answer(Some(ours.clone())))).unwrap();
        assert_eq!(l.shown().as_ref(), Some(&ours), "the project's notice, unprefixed");
        let l = round("127.0.0.1:20041", project, |s| {
            Some(answer(Some(if s == project { ours.clone() } else { host.clone() })))
        })
        .unwrap();
        assert_eq!(l.shown().unwrap().en, "Arcade notice: Server update at 23:00");
    }

    /// The project server under another spelling (no default port, another case) is asked once a round (#419): a
    /// second ask within the desk's gap got `ERR slow`.
    #[test]
    fn the_project_server_spelled_another_way_is_asked_once() {
        let ours = ServerNotice {
            id: "n1".into(),
            en: "Server update at 23:00".into(),
            ..ServerNotice::default()
        };
        let asks = std::cell::Cell::new(0);
        let get = |_: &str| {
            asks.set(asks.get() + 1);
            Some(answer(Some(ours.clone())))
        };
        let l = round("LOBBY.wccf.it", "lobby.wccf.it:20030", get).unwrap();
        assert_eq!(l.notice.as_ref(), Some(&ours));
        assert_eq!(asks.get(), 1, "one ask");
        asks.set(0);
        round("127.0.0.1:20041", "lobby.wccf.it:20030", get).unwrap();
        assert_eq!(asks.get(), 2, "another server: the desk and the project server");
    }

    /// A LAN arcade with no internet (#419): the desk's copy of the project's notice still shows, under the prefix
    /// (it cannot be checked), and is saved as heard for a later HOST LAN ARCADE's guests; the desk's "none" clears it
    /// but keeps a notice the project server sent.
    #[test]
    fn a_desks_notice_shows_when_the_project_server_is_out_of_reach() {
        let desk = "192.168.1.20";
        let project = "lobby.wccf.it:20032";
        let copy = ServerNotice {
            id: "n1".into(),
            en: "Server update at 23:00".into(),
            ..ServerNotice::default()
        };
        let offline = |sent: Option<ServerNotice>| round(desk, project, |s| (s == desk).then(|| answer(sent.clone())));
        let file = dir("offline").join("notice.json");
        let lan = file.with_file_name(LAN_FILE);
        let n = Notices::new(file.clone());
        assert!(n.lock().hear(offline(Some(copy.clone())).unwrap(), &|_| {}));
        assert_eq!(n.text("en").as_deref(), Some("Arcade notice: Server update at 23:00"));
        let read = |f: &Path| serde_json::from_slice::<ServerNotice>(&std::fs::read(f).unwrap()).unwrap();
        assert_eq!(
            read(&file).en,
            "Arcade notice: Server update at 23:00",
            "the panel shows it prefixed"
        );
        assert_eq!(
            read(&lan),
            copy,
            "a LAN arcade hosted next serves it as heard, never prefixed"
        );
        assert!(
            n.lock().hear(offline(None).unwrap(), &|_| {}),
            "the desk's none clears it"
        );
        assert!(n.text("en").is_none() && !file.exists() && !lan.exists());
        assert!(n.heard(&answer(Some(copy.clone())), &|_| {}));
        assert!(
            !n.lock().hear(offline(None).unwrap(), &|_| {}),
            "the project's notice stays"
        );
        assert_eq!(n.text("en").as_deref(), Some("Server update at 23:00"));
        assert_eq!(read(&lan), copy);
    }

    /// A desk with no notice: the project server's is kept cleaned like the other arms of [`round`] (#419).
    #[test]
    fn the_projects_notice_behind_a_quiet_desk_is_cleaned() {
        let project = "lobby.wccf.it:20032";
        let sent = ServerNotice {
            id: "n1".into(),
            en: "  Server update\u{202E} at 23:00 ".into(),
            ..ServerNotice::default()
        };
        let l = round("127.0.0.1:20041", project, |s| {
            Some(answer((s == project).then(|| sent.clone())))
        })
        .unwrap();
        assert_eq!(l.notice, sent.clone().clean());
        assert_ne!(l.notice.as_ref(), Some(&sent), "the raw notice changed when cleaned");
    }

    /// HOST ARCADE's notice file: a fresh id and the text as typed (bounded), removed when the box is empty.
    #[test]
    fn the_host_writes_its_notice_for_its_desk() {
        let file = dir("host").join("host_notice.json");
        assert!(write_host_notice(&file, &format!("  Open till 23:00\u{202E}{}", "y".repeat(300)), 17).unwrap());
        let n: ServerNotice = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(n.id, "h17");
        assert!(n.en.starts_with("Open till 23:00y"), "{}", n.en);
        assert_eq!(
            n.en.chars().count(),
            lobby::HOST_NOTICE_LEN - 3,
            "cut as typed, then trimmed and stripped"
        );
        assert!(!write_host_notice(&file, " \t ", 18).unwrap());
        assert!(!file.exists(), "no notice: no file");
        assert!(
            !write_host_notice(&file, "", 19).unwrap(),
            "removing a missing file is fine"
        );
    }

    /// The run ends while the watcher's ask of the host's desk is out (#419): that answer is dropped, so the host's
    /// notice is not stored after the run, and the project server is asked at once.
    #[test]
    fn an_answer_from_the_desk_after_the_run_is_dropped() {
        let desk = "127.0.0.1:20041";
        let n = Notices::new(dir("late").join("notice.json"));
        n.ask(desk);
        let host = Heard {
            notice: Some(ServerNotice {
                id: "h17".into(),
                en: "Open till 23:00".into(),
                ..ServerNotice::default()
            }),
            desk: true,
        };
        n.ask("lobby.wccf.it:20032"); // the run ended before the desk answered
        assert!(!n.heard_from(desk, host.clone(), &|_| {}));
        assert!(n.text("en").is_none(), "no host notice after the run");
        assert!(n.lock().at.is_none(), "the project server is asked on the next round");
        n.ask(desk);
        assert!(
            n.heard_from(desk, host, &|_| {}),
            "the desk still asked: its answer counts"
        );
    }

    /// The run's end (`ask` for the project server) cuts the watcher's minute short: the host's notice goes at once.
    #[test]
    fn an_ask_for_another_server_ends_the_wait() {
        let t = Instant::now();
        let mut i = inner(Some(t));
        i.server = "127.0.0.1:20041".into();
        assert!(i.napping("127.0.0.1:20041", t, t + FIRST_STEP, EVERY));
        assert!(
            !i.napping("127.0.0.1:20041", t, t + EVERY, EVERY),
            "a minute on: the next round"
        );
        i.server = "lobby.wccf.it:20032".into();
        assert!(
            !i.napping("127.0.0.1:20041", t, t + FIRST_STEP, EVERY),
            "another server: asked now"
        );
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
