//! The match end and Create-a-Team's match card (plan step 1.2).
//!
//! [`MatchEnd`] turns the two match-end sources into one deal per match and logs each source it sees, so a guarded run
//! shows which fired. [`Granter`]: with `WCCF_COLLECTION` set (the Launcher's Create-a-Team run), the deal draws a card
//! through `forza::collection` and appends its `match` line to the profile's grant log. The panel draws inside the
//! game's Present, so the frame only sends a message and reads the answers: the draw, the log's time-boxed lock and the
//! write run on this module's thread. A failed write is logged and tried again later, and never lands twice. A card
//! not saved yet is kept in [`PENDING`] in the collection folder, so a game that ends first loses no grant: the next
//! Create-a-Team start saves it.

use std::collections::VecDeque;
use std::hash::BuildHasher as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use std::{fs, io};

use forza::board::Card;
use forza::card::Session;
use forza::collection::{self, Copies, Kind, Line, Log, OffsetDateTime};
use forza_ui::panel::{Deal, Grant};

/// A second trigger this soon after a grant, with no new club card session seen in between, is the same match.
// shortcut: 5 min is shorter than a match and longer than its end screens; the guarded measurement (plan 7.1 (a))
// confirms it, or the window changes.
const SAME_MATCH: Duration = Duration::from_secs(5 * 60);

/// The match ends the panel has seen: one deal per match, whichever source tells it first.
#[derive(Debug, Default)]
pub struct MatchEnd {
    /// The club card session at the last readable look.
    last: Option<Session>,
    /// When the last deal was made.
    dealt: Option<Instant>,
    /// A session opened since that deal: a new match is on.
    opened: bool,
}

impl MatchEnd {
    /// One look, once a second: `owed` = cards the game handed out since the last look ([`crate::patches::take_owed_new`]),
    /// `session` = the club card's session (`None` when unreadable), `counts` = the dispenser count works on this game.
    /// The trigger's name when a match finished and is not dealt yet; every source seen is said.
    pub fn look(
        &mut self,
        owed: u32,
        session: Option<Session>,
        counts: bool,
        at: Instant,
        say: &mut impl FnMut(String),
    ) -> Option<&'static str> {
        let was = session.and_then(|s| self.last.replace(s));
        let opened = session == Some(Session::Open) && was != Some(Session::Open);
        let closed = was == Some(Session::Open) && session == Some(Session::Closed);
        if opened {
            self.opened = true;
            say("deal: source club card session open (a match or Club Make started)".into());
        }
        if owed > 0 {
            say(format!(
                "deal: source dispenser, {owed} player card(s) handed out"
            ));
        }
        if closed {
            say(format!(
                "deal: source club card session closed{}",
                if counts {
                    " (not a trigger: the dispenser has counted a hand-out on this game)"
                } else {
                    ""
                }
            ));
        }
        // the dispenser counts each card the game hands out; the session's close is the fallback until it has counted
        // one on this game (it also closes after Club Make, which is no match)
        let why = if owed > 0 {
            "dispenser"
        } else if closed && !counts {
            "club card session"
        } else {
            return None;
        };
        if self
            .dealt
            .is_some_and(|d| !self.opened && at.duration_since(d) < SAME_MATCH)
        {
            say(format!(
                "deal: {why} again in the same match - no second deal"
            ));
            return None;
        }
        self.dealt = Some(at);
        self.opened = false;
        Some(why)
    }
}

/// The first wait after a failed write; it doubles on each failure up to [`RETRY_MAX`].
const RETRY_FIRST: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);

/// The grant thread's two ends, as the frame holds them.
pub struct Granter {
    ask: Sender<&'static str>,
    news: Receiver<(usize, Grant)>,
}

impl Granter {
    /// The thread for the collection in `dir`, drawing from `cards` (the panel's catalogue).
    // shortcut: the thread lives as long as the process; only a Create-a-Team run sets WCCF_COLLECTION, and the game
    // never unloads the panel (the unload test runs without it)
    pub fn start(dir: PathBuf, cards: Arc<[Card]>, log: fn(&str)) -> Granter {
        let (ask, asks) = mpsc::channel();
        let (tell, news) = mpsc::channel();
        std::thread::spawn(move || run(&dir, &cards, &asks, &tell, log));
        Granter { ask, news }
    }

    /// A match finished (`why`: its trigger). Never waits.
    pub fn ask(&self, why: &'static str) {
        let _ = self.ask.send(why);
    }

    /// The thread's next news, never waiting: a card drawn ([`Grant::Saving`]), then how its save went.
    pub fn news(&self) -> Option<(usize, Grant)> {
        self.news.try_recv().ok()
    }
}

/// The thread's news into the deal window: `Saving` opens the card's window, later news update it. A `Saved` whose
/// window is gone (the save outlasted [`forza_ui::panel::DEAL_SECS`], or a card kept from an earlier run) opens it
/// again; another card's window is left alone. The log's note when the window had closed.
pub fn show(deal: &mut Option<Deal>, card: usize, grant: Grant) -> Option<&'static str> {
    if let Some(d) = deal
        .as_mut()
        .filter(|d| d.card == card && grant != Grant::Saving)
    {
        d.grant = Some(grant);
        return None;
    }
    let late = matches!(grant, Grant::Saved { .. });
    if grant != Grant::Saving && (!late || deal.is_some()) {
        // a retry under a closed window goes on unseen; a late save under another card's window is only logged
        return late.then_some(
            "saved after its window closed (another card is shown: only this log says so)",
        );
    }
    let mut d = Deal::new(card);
    d.grant = Some(grant);
    *deal = Some(d);
    late.then_some("saved after its window closed - shown again")
}

/// The cards drawn and not saved yet, one [`collection::Entry`] line each (its `card`, `at` and `why`; the rest
/// unused), in the collection folder: written before a card's first append, removed after the last save, read by
/// the next [`Granter::start`].
pub const PENDING: &str = "grants.pending";

/// A drawn card on its way into the log.
struct Job<'a> {
    at: OffsetDateTime,
    /// The card's index in the catalogue (and in the draw's pool), and its row there.
    card: usize,
    drawn: collection::Card<'a>,
    why: String,
    tries: u32,
}

/// [`PENDING`] as `pending` says: written whole (a temporary file, then renamed over it), removed when empty.
fn keep_pending(dir: &Path, pending: &VecDeque<Job>) -> io::Result<()> {
    let path = dir.join(PENDING);
    if pending.is_empty() {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let text: String = pending
        .iter()
        .map(|j| {
            let e = collection::Entry {
                seq: 0,
                at: j.at,
                rules: 0,
                kind: Kind::Match,
                card: Some(j.drawn.card_no),
                tokens: 0,
                why: j.why.clone(),
                prev: String::new(),
            };
            e.line() + "\n"
        })
        .collect();
    fs::create_dir_all(dir)?;
    let tmp = dir.join("grants.pending.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, &path)
}

/// The cards an earlier run left in [`PENDING`]; each first looks for its line in the log (`tries` 1), so a card whose
/// line landed before the game ended is not written twice.
fn replay<'a>(dir: &Path, pool: &[collection::Card<'a>], log: fn(&str)) -> VecDeque<Job<'a>> {
    let Ok(text) = fs::read_to_string(dir.join(PENDING)) else {
        return VecDeque::new();
    };
    text.lines()
        .filter_map(|l| {
            let job = serde_json::from_str::<collection::Entry>(l)
                .ok()
                .and_then(|e| {
                    let (i, drawn) = pool
                        .iter()
                        .enumerate()
                        .find(|(_, p)| Some(p.card_no) == e.card)?;
                    Some(Job {
                        at: e.at,
                        card: i,
                        drawn: *drawn,
                        why: e.why,
                        tries: 1,
                    })
                });
            match &job {
                Some(j) => log(&format!(
                    "deal: card {} drawn at {} in an earlier run, not saved then - saving it now",
                    j.drawn.card_no, j.at
                )),
                None => log(&format!(
                    "deal: a card kept in {PENDING} cannot be read or is not in the catalogue - dropped: {l}"
                )),
            }
            job
        })
        .collect()
}

fn run(
    dir: &Path,
    cards: &[Card],
    asks: &Receiver<&'static str>,
    tell: &Sender<(usize, Grant)>,
    log: fn(&str),
) {
    let pool = forza::cards::collection_cards(cards);
    let mut rng = collection::Rng::new(std::hash::RandomState::new().hash_one(Instant::now()));
    let mut pending = replay(dir, &pool, log);
    let (mut retry, mut wait) = (RETRY_FIRST, RETRY_FIRST);
    loop {
        while let Some(job) = pending.front_mut() {
            match save(dir, job) {
                Ok(grant) => {
                    log(&format!(
                        "deal: card {} saved to {} ({grant:?})",
                        job.drawn.card_no,
                        dir.display()
                    ));
                    let _ = tell.send((job.card, grant));
                    pending.pop_front();
                    wait = RETRY_FIRST;
                    if let Err(e) = keep_pending(dir, &pending) {
                        log(&format!("deal: {PENDING} not updated ({e})"));
                    }
                }
                Err(e) => {
                    job.tries += 1;
                    retry = wait;
                    wait = (wait * 2).min(RETRY_MAX);
                    log(&format!(
                        "deal: card {} not saved yet (try {}: {e}) - again in {} s",
                        job.drawn.card_no,
                        job.tries,
                        retry.as_secs()
                    ));
                    let _ = tell.send((job.card, Grant::Retrying));
                    break;
                }
            }
        }
        let asked = if pending.is_empty() {
            asks.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            asks.recv_timeout(retry)
        };
        match asked {
            Ok(why) => {
                let Some((i, drawn)) = collection::match_card(&mut rng, &pool)
                    .and_then(|c| Some((pool.iter().position(|p| std::ptr::eq(p, c))?, *c)))
                else {
                    log("deal: a match finished, but no catalogue card to draw");
                    continue;
                };
                log(&format!(
                    "deal: card {} {} ({:?}) drawn for the collection ({why})",
                    drawn.card_no,
                    cards.get(i).map_or("", Card::short_name),
                    drawn.rarity
                ));
                let _ = tell.send((i, Grant::Saving));
                pending.push_back(Job {
                    at: collection::now(),
                    card: i,
                    drawn,
                    why: why.into(),
                    tries: 0,
                });
                // before the first append: a game that ends while the card waits for the log keeps it for the next start
                if let Err(e) = keep_pending(dir, &pending) {
                    log(&format!(
                        "deal: {PENDING} not written ({e}) - the card is lost if the game ends before it is saved"
                    ));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if !pending.is_empty() {
                    log(&format!(
                        "deal: the panel stopped with {} card(s) not saved - kept in {PENDING} for the next start",
                        pending.len()
                    ));
                }
                return;
            }
        }
    }
}

/// The job's `match` line appended under the log's lock, or found already there (a retry after a write that landed but
/// reported an error).
fn save(dir: &Path, job: &Job) -> io::Result<Grant> {
    let card = &job.drawn;
    fs::create_dir_all(dir)?;
    let log = Log::new(dir);
    if job.tries > 0
        && let Some(grant) = landed(&log, card, job.at)?
    {
        return Ok(grant);
    }
    let mut got = None;
    log.append_lines_at(job.at, |state| {
        let tokens = state.match_pays();
        let mut copies = state.copies(card.card_no);
        copies.earned += 1;
        got = Some((tokens, copies));
        Ok(vec![Line {
            kind: Kind::Match,
            card: Some(card.card_no),
            tokens,
            why: format!("match end ({})", job.why),
        }])
    })?;
    let (tokens, copies) = got.ok_or_else(|| io::Error::other("the log asked for no line"))?;
    Ok(saved(card, tokens, copies))
}

/// The grant of `card` at `at` when the log already has its `match` line.
fn landed(log: &Log, card: &collection::Card, at: OffsetDateTime) -> io::Result<Option<Grant>> {
    let text = log.lines()?;
    let line = text
        .lines()
        .filter_map(|l| serde_json::from_str::<collection::Entry>(l).ok())
        .find(|e| e.kind == Kind::Match && e.card == Some(card.card_no) && e.at == at);
    Ok(line.map(|e| {
        let copies = collection::fold(&text).state.copies(card.card_no);
        saved(card, e.tokens, copies)
    }))
}

fn saved(card: &collection::Card, tokens: i64, copies: Copies) -> Grant {
    Grant::Saved {
        tokens,
        // listed copies are on the market, not held (`Copies::owned`)
        copies: (copies.earned + copies.starter + copies.imported).saturating_sub(copies.listed),
        convert: collection::convert(&copies, card).ok(),
        floor: collection::floor_value(collection::value(card)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    fn look(
        m: &mut MatchEnd,
        owed: u32,
        s: Option<Session>,
        counts: bool,
        at: Instant,
    ) -> Option<&'static str> {
        m.look(owed, s, counts, at, &mut |_| {})
    }

    /// One deal per match: the dispenser where it counts (the session's close after it, or after Club Make, deals
    /// nothing), the session's close where it does not; a second hand-out in the same match deals nothing; a new
    /// session or 5 minutes start the next match.
    #[test]
    fn each_match_deals_once() {
        use Session::{Closed, Open};
        let t = Instant::now();
        let s = |n: u64| t + Duration::from_secs(n);
        // 2017-18, the dispenser counts: club make, then two matches
        let mut m = MatchEnd::default();
        assert_eq!(look(&mut m, 0, Some(Closed), true, s(0)), None);
        assert_eq!(look(&mut m, 0, Some(Open), true, s(1)), None);
        assert_eq!(look(&mut m, 0, Some(Closed), true, s(2)), None, "Club Make");
        assert_eq!(look(&mut m, 0, Some(Open), true, s(3)), None);
        assert_eq!(look(&mut m, 1, Some(Open), true, s(600)), Some("dispenser"));
        assert_eq!(
            look(&mut m, 1, Some(Open), true, s(601)),
            None,
            "a 2nd card"
        );
        assert_eq!(look(&mut m, 0, None, true, s(602)), None);
        assert_eq!(look(&mut m, 0, Some(Closed), true, s(630)), None);
        assert_eq!(look(&mut m, 0, Some(Open), true, s(700)), None);
        assert_eq!(look(&mut m, 2, Some(Open), true, s(800)), Some("dispenser"));
        // no session to read: 5 minutes apart is another match
        let mut m = MatchEnd::default();
        assert_eq!(look(&mut m, 1, None, true, s(0)), Some("dispenser"));
        assert_eq!(look(&mut m, 1, None, true, s(299)), None);
        assert_eq!(look(&mut m, 1, None, true, s(301)), Some("dispenser"));
        // the dispenser does not count (2010-11): the session's close, once
        let mut m = MatchEnd::default();
        assert_eq!(look(&mut m, 0, Some(Open), false, s(0)), None);
        assert_eq!(look(&mut m, 0, None, false, s(1)), None, "mid-write");
        assert_eq!(
            look(&mut m, 0, Some(Closed), false, s(2)),
            Some("club card session")
        );
        assert_eq!(look(&mut m, 0, Some(Closed), false, s(3)), None);
        assert_eq!(look(&mut m, 0, Some(Open), false, s(60)), None);
        assert_eq!(
            look(&mut m, 0, Some(Closed), false, s(61)),
            Some("club card session")
        );
        // the hook is in but has counted nothing yet (counts false): the close deals, and the hand-out the same match
        // counts after it does not
        let mut m = MatchEnd::default();
        assert_eq!(look(&mut m, 0, Some(Open), false, s(0)), None);
        assert_eq!(
            look(&mut m, 0, Some(Closed), false, s(1)),
            Some("club card session")
        );
        assert_eq!(
            look(&mut m, 1, Some(Closed), true, s(30)),
            None,
            "the same match's hand-out"
        );
        // every source is said, also the ones that deal nothing
        let mut said = vec![];
        let mut m = MatchEnd::default();
        m.look(0, Some(Open), true, s(0), &mut |l| said.push(l));
        m.look(1, Some(Closed), true, s(1), &mut |l| said.push(l));
        assert_eq!(said.len(), 3, "{said:?}");
        assert!(said[2].contains("not a trigger"), "{said:?}");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forza-panel-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn cards() -> Arc<[Card]> {
        forza_ui::sample::cards().into()
    }

    /// The news until `done` says so, or `limit`.
    fn wait_for(
        g: &Granter,
        limit: Duration,
        done: impl Fn(&Grant) -> bool,
    ) -> Vec<(usize, Grant)> {
        let t = Instant::now();
        let mut news = vec![];
        while t.elapsed() < limit {
            while let Some(n) = g.news() {
                news.push(n);
            }
            if news.last().is_some_and(|(_, g)| done(g)) {
                break;
            }
            sleep(Duration::from_millis(20));
        }
        news
    }

    /// (e) The log held by another handle for 10 s: asking and reading the news never wait (a frame's few ms), the
    /// card is said not saved yet, and its one `match` line lands after the hold.
    #[test]
    fn a_held_log_stalls_no_frame_and_the_grant_lands_after() {
        let dir = scratch("grant-held");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("grants.jsonl");
        let held = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        held.lock().unwrap();
        let g = Granter::start(dir.clone(), cards(), |_| {});
        let t = Instant::now();
        g.ask("test");
        let mut worst = t.elapsed();
        let mut news = vec![];
        while t.elapsed() < Duration::from_secs(10) {
            let frame = Instant::now();
            while let Some(n) = g.news() {
                news.push(n);
            }
            worst = worst.max(frame.elapsed());
            sleep(Duration::from_millis(16));
        }
        assert!(worst < Duration::from_millis(5), "a frame waited {worst:?}");
        assert_eq!(news.first().map(|n| &n.1), Some(&Grant::Saving), "{news:?}");
        assert!(news.iter().any(|n| n.1 == Grant::Retrying), "{news:?}");
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
        assert!(dir.join(PENDING).is_file(), "the waiting card is not kept");
        drop(held);
        let after = wait_for(&g, Duration::from_secs(40), |g| {
            matches!(g, Grant::Saved { .. })
        });
        let Some((_, Grant::Saved { tokens, copies, .. })) = after.last() else {
            panic!("not saved after the hold: {after:?}");
        };
        assert_eq!((*tokens, *copies), (collection::MATCH_TOKENS, 1));
        let text = fs::read_to_string(&path).unwrap();
        let folded = collection::fold(&text);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
        assert_eq!(folded.state.tokens, collection::MATCH_TOKENS);
        assert!(!dir.join(PENDING).exists(), "kept after the save");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Finding 1: a card drawn and not saved when the game ended is saved by the next start, once; a card whose line
    /// landed before the end (the pending file not removed yet) is not written again.
    #[test]
    fn a_card_pending_when_the_game_ended_is_saved_at_the_next_start_once() {
        let dir = scratch("grant-pending");
        let sample = forza_ui::sample::cards();
        let pool = forza::cards::collection_cards(&sample);
        let job = |at| Job {
            at,
            card: 0,
            drawn: pool[0],
            why: "dispenser".into(),
            tries: 0,
        };
        let next_start = || {
            let g = Granter::start(dir.clone(), cards(), |_| {});
            let news = wait_for(&g, Duration::from_secs(10), |g| {
                matches!(g, Grant::Saved { .. })
            });
            assert!(
                matches!(news.last(), Some((_, Grant::Saved { .. }))),
                "{news:?}"
            );
        };
        keep_pending(&dir, &VecDeque::from([job(collection::now())])).unwrap();
        next_start();
        let log = Log::new(&dir);
        let text = log.lines().unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        let e: collection::Entry = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(
            (e.card, e.why.as_str()),
            (Some(pool[0].card_no), "match end (dispenser)")
        );
        assert!(!dir.join(PENDING).exists());
        keep_pending(&dir, &VecDeque::from([job(e.at)])).unwrap();
        next_start();
        assert_eq!(log.lines().unwrap(), text, "written again");
        assert!(!dir.join(PENDING).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Finding 1: a save that lands after its window closed opens it again; a retry does not, and another card's window
    /// stays.
    #[test]
    fn a_save_after_its_window_closed_is_shown_again() {
        let done = Grant::Saved {
            tokens: 40,
            copies: 1,
            convert: None,
            floor: 1,
        };
        let mut deal = None;
        assert_eq!(show(&mut deal, 3, Grant::Saving), None);
        assert_eq!(deal.as_ref().map(|d| d.card), Some(3));
        deal = None; // DEAL_SECS passed
        assert_eq!(show(&mut deal, 3, Grant::Retrying), None);
        assert!(deal.is_none(), "a retry opened a window");
        assert!(show(&mut deal, 3, done.clone()).is_some());
        assert_eq!(
            deal.as_ref().map(|d| (d.card, d.grant.clone(), d.until)),
            Some((3, Some(done.clone()), None))
        );
        let mut other = Some(Deal::new(5));
        assert!(show(&mut other, 3, done).is_some());
        assert_eq!(other, Some(Deal::new(5)));
    }

    /// Finding 3: the reveal counts the copies held, not the ones out on the market.
    #[test]
    fn the_copy_count_leaves_out_listed_copies() {
        let sample = forza_ui::sample::cards();
        let card = forza::cards::collection_cards(&sample)[0];
        let copies = Copies {
            earned: 3,
            listed: 1,
            ..Copies::default()
        };
        assert!(
            matches!(saved(&card, 15, copies), Grant::Saved { copies: 2, .. }),
            "{:?}",
            saved(&card, 15, copies)
        );
    }

    /// Two matches: each line pays the match tokens; a retry that finds its line already written writes none.
    #[test]
    fn two_matches_pay_and_a_landed_line_is_not_written_again() {
        let dir = scratch("grant-two");
        let g = Granter::start(dir.clone(), cards(), |_| {});
        for want in [collection::MATCH_TOKENS, collection::MATCH_TOKENS] {
            g.ask("test");
            let news = wait_for(&g, Duration::from_secs(10), |g| {
                matches!(g, Grant::Saved { .. })
            });
            assert!(
                matches!(news.last(), Some((_, Grant::Saved { tokens, .. })) if *tokens == want),
                "{news:?}"
            );
        }
        let log = Log::new(&dir);
        let text = log.lines().unwrap();
        assert_eq!(text.lines().count(), 2);
        let last: collection::Entry = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        let sample = forza_ui::sample::cards();
        let drawn = forza::cards::collection_cards(&sample)
            .into_iter()
            .find(|c| Some(c.card_no) == last.card)
            .unwrap();
        let job = Job {
            at: last.at,
            card: 0,
            drawn,
            why: "test".into(),
            tries: 1,
        };
        assert!(
            matches!(save(&dir, &job), Ok(Grant::Saved { tokens, .. }) if tokens == collection::MATCH_TOKENS)
        );
        assert_eq!(log.lines().unwrap().lines().count(), 2, "written again");
        let _ = fs::remove_dir_all(&dir);
    }
}
