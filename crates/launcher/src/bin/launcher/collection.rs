//! Create-a-Team's COLLECTION tab (2017-18, `PLAN.md` 1.4): the profile's log folded into the tab's view, the first
//! start's deal and its lines. The economy is `forza::collection`'s; this file only reads and asks it.

use std::hash::BuildHasher;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use forza::board::Card;
use forza::cards::collection_cards;
use forza::collection::{self as c, LEAGUES, Log, Rarity};
use forza_ui::collection::{Choice, CollectionView, Filter, StartRow, Step};
use launcher::Kit;
use launcher::run;

/// The deal drawn and not taken yet, in the profile's collection folder: DEAL writes it, TAKE removes it, so a restart
/// or a trip to FREE and back shows the same 12 cards (`PLAN.md` 2.4: "You choose once"). A reset archives the folder
/// with the log, which is what brings the picker back.
pub const PENDING: &str = "starter.pending.json";

/// The deal as the file holds it: the numbers, so a catalogue rebuild or a new `starter_rares` cannot reroll it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Pending {
    choice: Choice,
    dealt: Vec<u32>,
    rares: Vec<u32>,
}

/// The picker's rows (`PLAN.md` 2.4): All seasons first, then every season of the catalogue with the mean TOTAL of its
/// REGULAR cards (a season start is unbanded, so the mean is what the 11 land near), then the 9 leagues (banded).
pub fn starts(catalogue: &[Card]) -> Vec<StartRow> {
    let pool = collection_cards(catalogue);
    let mut seasons: Vec<u16> = pool.iter().map(|c| c.season).filter(|&y| y > 0).collect();
    seasons.sort_unstable();
    seasons.dedup();
    let mean = |y: u16| {
        let t: Vec<u32> = pool
            .iter()
            .filter(|c| c.season == y && c.rarity == Rarity::Regular && !c.secret)
            .map(|c| c.total)
            .collect();
        (!t.is_empty()).then(|| (t.iter().sum::<u32>() as f32 / t.len() as f32).round() as u32)
    };
    std::iter::once(StartRow {
        choice: Choice::AllSeasons,
        mean: None,
    })
    .chain(seasons.into_iter().filter_map(|y| {
        mean(y).map(|m| StartRow {
            choice: Choice::Season(y),
            mean: Some(m),
        })
    }))
    .chain((0..LEAGUES.len()).map(|i| StartRow {
        choice: Choice::League(i),
        mean: None,
    }))
    .collect()
}

/// The view of the log in `dir` against the catalogue. `old` keeps the page's filter and open card, and a start being
/// chosen or a deal not taken yet. A line that cannot be read shows the cards before it, marked damaged, so a started
/// collection still plays (reset, `PLAN.md` 1.5, is the way to a whole file again).
pub fn view(catalogue: &[Card], dir: &Path, old: Option<CollectionView>) -> io::Result<CollectionView> {
    let folded = c::fold(&Log::new(dir).lines()?);
    let s = folded.state;
    let started = s.cards.values().any(|k| k.starter > 0);
    let (filter, open, step) = match old {
        Some(o) => (o.filter, o.open, o.step),
        None => (Filter::default(), None, Step::Page),
    };
    let step = match step {
        _ if started => Step::Page,
        _ if let Some((choice, dealt, rares)) = pending(catalogue, dir) => Step::Pick {
            choice,
            dealt,
            rares,
            picked: None,
        },
        Step::Page => Step::Choose {
            starts: starts(catalogue),
            chosen: None,
        },
        kept => kept,
    };
    Ok(CollectionView {
        step,
        cards: catalogue
            .iter()
            .filter_map(|k| {
                s.cards
                    .get(&k.number)
                    .filter(|n| n.earned + n.starter + n.imported > 0)
                    .map(|n| (k.clone(), *n))
            })
            .collect(),
        tokens: s.tokens,
        match_left: c::WEEK_CAP - s.match_week,
        filter,
        open,
        problem: None,
        damaged: folded.unreadable,
    })
}

/// The deal on disk as catalogue cards: `None` when there is none, the catalogue no longer holds one of its numbers
/// (a rebuild), or an edited file names a league past [`LEAGUES`] (its name would index past the list).
fn pending(catalogue: &[Card], dir: &Path) -> Option<(Choice, Vec<Card>, Vec<Card>)> {
    let text = std::fs::read_to_string(dir.join(PENDING)).ok()?;
    let deal: Pending = serde_json::from_str(&text).ok()?;
    if matches!(deal.choice, Choice::League(i) if i >= LEAGUES.len()) {
        return None;
    }
    let back = |nos: &[u32]| -> Option<Vec<Card>> {
        nos.iter()
            .map(|n| catalogue.iter().find(|k| k.number == *n).cloned())
            .collect()
    };
    Some((deal.choice, back(&deal.dealt)?, back(&deal.rares)?))
}

/// DEAL: the first start's 11 and 3 RARE cards for `choice` from a fresh seed, written down until TAKE removes them.
pub fn deal(catalogue: &[Card], dir: &Path, choice: Choice) -> io::Result<()> {
    let pool = collection_cards(catalogue);
    let mut rng = c::Rng::new(std::hash::RandomState::new().hash_one(Instant::now()));
    let deal = Pending {
        choice,
        dealt: c::starter(&mut rng, &pool, choice.start())
            .iter()
            .map(|p| p.card_no)
            .collect(),
        rares: c::starter_rares(&mut rng, &pool, choice.start())
            .iter()
            .map(|p| p.card_no)
            .collect(),
    };
    let text = serde_json::to_string(&deal).map_err(io::Error::other)?;
    launcher::choices::write(&dir.join(PENDING), &text)
}

/// TAKE THIS TEAM: the 11 and the RARE picked as `starter` lines, and the welcome once per installation (its mark in
/// `data`, beside `market.key`). Nothing is written when the log already holds a starter.
pub fn take(dir: &Path, data: &Path, step: &Step) -> io::Result<Vec<c::Entry>> {
    let Step::Pick {
        choice,
        dealt,
        rares,
        picked: Some(i),
    } = step
    else {
        return Ok(vec![]);
    };
    let cards: Vec<u32> = dealt.iter().chain(rares.get(*i)).map(|k| k.number).collect();
    std::fs::create_dir_all(dir)?;
    let out = c::first_start(
        &Log::new(dir),
        &data.join(c::WELCOME_MARK),
        c::now(),
        &cards,
        &choice.why(),
    )?;
    // the deal is spent: only a reset brings the picker back, and the reset takes the file with the folder
    let _ = std::fs::remove_file(dir.join(PENDING));
    Ok(out)
}

/// Why START OVER did not run; the profile is unchanged in each case.
#[derive(Debug)]
pub enum ResetRefused {
    /// A start between its checks and its game, a run, a server, or a SET UP holds the kit.
    Busy,
    Failed(io::Error),
}

/// START OVER for `kit`: under the kit's admission for the whole reset, as SET UP takes it (`setup.rs` `admitted`). A
/// start holds it from its checks until its game is on, so a reset never lands under a start, and a start meanwhile
/// is refused (`launcher.run_busy`).
pub fn reset_kit(kit: &Kit) -> Result<PathBuf, ResetRefused> {
    let Some(_admitted) = run::admit(kit).filter(|_| !run::any_on(kit)) else {
        return Err(ResetRefused::Busy);
    };
    reset(&kit.data(), run::seat_of(kit).as_deref()).map_err(ResetRefused::Failed)
}

/// START OVER (`PLAN.md` 1.5): the profile's folder into its archive (`forza::profile::reset`) under this PC's time.
pub fn reset(data: &Path, seat: Option<&Path>) -> io::Result<PathBuf> {
    let t = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    );
    forza::profile::reset(data, seat, &stamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forza::board::{Position, Rarity as R};
    use forza::profile::Profile;

    /// A catalogue of 2 seasons: per season 2 GK, 6 DF, 6 MF, 4 FW REGULAR at TOTAL 80-82 and 4 RARE.
    fn catalogue() -> Vec<Card> {
        let mut out = vec![];
        for (season, year) in [("2013-14", 13u32), ("2014-15", 14)] {
            let mut add = |pos, rarity, n: u32| {
                for k in 0..n {
                    let number = year * 1000 + out.len() as u32;
                    let mut card = forza_ui::sample::cards()[0].clone();
                    card.number = number;
                    card.name = format!("P{number}");
                    card.born = format!("19{k:02}-01-01");
                    card.season = season.into();
                    card.club = "Juventus".into();
                    card.pos = pos;
                    card.rarity = rarity;
                    card.code = String::new();
                    card.secret = false;
                    card.total = 80 + (k % 3) as u8;
                    out.push(card);
                }
            };
            add(Position::Gk, R::Regular, 2);
            add(Position::Df, R::Regular, 6);
            add(Position::Mf, R::Regular, 6);
            add(Position::Fw, R::Regular, 4);
            add(Position::Mf, R::Rare, 4);
        }
        out
    }

    /// The profile's collection folder and the kit's `data`, built as the app builds them (a reset and the welcome
    /// mark's place both hang off these two paths).
    fn profile(data: &Path) -> (PathBuf, PathBuf) {
        let coll = Profile::CreateATeam.collection(data).unwrap();
        (coll, data.to_path_buf())
    }

    /// The numbers of a view's deal, for comparing what a restart shows with what was written.
    fn numbers(step: &Step) -> (Vec<u32>, Vec<u32>) {
        let Step::Pick { dealt, rares, .. } = step else {
            panic!("no deal on the screen")
        };
        let nos = |cards: &[Card]| cards.iter().map(|k| k.number).collect::<Vec<_>>();
        (nos(dealt), nos(rares))
    }

    /// 1.4 (a), (b) through the tab: DEAL, TAKE THIS TEAM writes 12 starter lines of the choice and the welcome, the view
    /// turns to the page with 12 starter cards and 150 tokens; a second TAKE writes nothing. The welcome's mark lands
    /// beside `data`, outside the profile's folder, which a reset never touches.
    #[test]
    fn the_first_start_takes_the_deal_once() {
        let cat = catalogue();
        let dir = tempfile::tempdir().unwrap();
        let (coll, data) = profile(dir.path());
        let v = view(&cat, &coll, None).unwrap();
        let Step::Choose { starts, .. } = &v.step else {
            panic!("a new profile opens on the picker")
        };
        assert_eq!(starts[0].choice, Choice::AllSeasons);
        assert_eq!(starts.len(), 1 + 2 + LEAGUES.len());
        deal(&cat, &coll, Choice::League(0)).unwrap();
        let Step::Pick {
            choice, dealt, rares, ..
        } = view(&cat, &coll, None).unwrap().step
        else {
            panic!("DEAL leaves the deal on the screen")
        };
        assert_eq!(choice, Choice::League(0));
        assert_eq!((dealt.len(), rares.len()), (11, 3));
        let step = Step::Pick {
            choice,
            dealt,
            rares,
            picked: Some(2),
        };
        let lines = take(&coll, &data, &step).unwrap();
        assert_eq!(lines.iter().filter(|e| e.kind == c::Kind::Starter).count(), 12);
        assert!(
            lines
                .iter()
                .all(|e| e.kind != c::Kind::Starter || e.why == "starter Italy (Serie A)")
        );
        assert_eq!(lines.iter().filter(|e| e.kind == c::Kind::Welcome).count(), 1);
        assert!(data.join(c::WELCOME_MARK).is_file());
        assert!(!coll.join(c::WELCOME_MARK).exists(), "the mark is outside the profile");
        let v = view(&cat, &coll, Some(v)).unwrap();
        assert!(matches!(v.step, Step::Page));
        assert_eq!((v.cards.len(), v.tokens), (12, c::WELCOME));
        assert!(v.cards.iter().all(|(_, n)| n.starter == 1));
        let before = std::fs::read(coll.join("grants.jsonl")).unwrap();
        assert!(take(&coll, &data, &step).unwrap().is_empty());
        assert_eq!(std::fs::read(coll.join("grants.jsonl")).unwrap(), before);
    }

    /// 1.4 review #1: DEAL writes the deal in the profile's own folder and a view made again (a Launcher restart; FREE
    /// and back to CREATE-A-TEAM, which drops free play's view) shows the same 12 numbers; TAKE removes the file; a reset
    /// shows the picker again.
    #[test]
    fn the_deal_stays_until_it_is_taken() {
        let cat = catalogue();
        let dir = tempfile::tempdir().unwrap();
        let (coll, data) = profile(dir.path());
        deal(&cat, &coll, Choice::Season(2013)).unwrap();
        let written: Pending = serde_json::from_str(&std::fs::read_to_string(coll.join(PENDING)).unwrap()).unwrap();
        assert_eq!((written.dealt.len(), written.rares.len()), (11, 3));
        assert_eq!(written.choice, Choice::Season(2013));

        // the view made again from the log alone: a restart, and FREE -> CREATE-A-TEAM
        let v = view(&cat, &coll, None).unwrap();
        assert_eq!(numbers(&v.step), (written.dealt.clone(), written.rares.clone()));
        assert!(Profile::Free.collection(&data).is_none());
        let back = view(&cat, &Profile::CreateATeam.collection(&data).unwrap(), None).unwrap();
        assert_eq!(numbers(&back.step), numbers(&v.step));

        // TAKE: the 12 starter lines and the file gone
        let Step::Pick {
            choice, dealt, rares, ..
        } = back.step
        else {
            panic!("no deal on the screen")
        };
        let step = Step::Pick {
            choice,
            dealt,
            rares,
            picked: Some(0),
        };
        let lines = take(&coll, &data, &step).unwrap();
        assert_eq!(lines.iter().filter(|e| e.kind == c::Kind::Starter).count(), 12);
        assert!(!coll.join(PENDING).exists(), "TAKE removes the deal");
        assert!(matches!(view(&cat, &coll, None).unwrap().step, Step::Page));

        // a reset: the folder goes, so the picker is back
        std::fs::remove_dir_all(&coll).unwrap();
        let v = view(&cat, &coll, None).unwrap();
        assert!(matches!(v.step, Step::Choose { .. }));
    }

    /// #374: an edited deal file whose league is past the list is no deal: the picker comes back instead of a crash on
    /// the league's name.
    #[test]
    fn an_edited_deal_out_of_range_is_dropped() {
        let cat = catalogue();
        let dir = tempfile::tempdir().unwrap();
        let (coll, _) = profile(dir.path());
        deal(&cat, &coll, Choice::League(0)).unwrap();
        let file = coll.join(PENDING);
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, text.replace("{\"League\":0}", "{\"League\":99}")).unwrap();
        let v = view(&cat, &coll, None).unwrap();
        assert!(matches!(v.step, Step::Choose { .. }), "{:?}", v.step);
    }

    /// #374 B1: a last line cut off by an interrupted write is dropped as the next write drops it, so the page opens
    /// with the cards before it and PLAY is not held back.
    #[test]
    fn a_cut_off_last_line_opens_the_collection_page() {
        let cat = catalogue();
        let dir = tempfile::tempdir().unwrap();
        let (coll, data) = profile(dir.path());
        deal(&cat, &coll, Choice::Season(2013)).unwrap();
        let mut step = view(&cat, &coll, None).unwrap().step;
        if let Step::Pick { picked, .. } = &mut step {
            *picked = Some(0);
        }
        take(&coll, &data, &step).unwrap();
        let log = coll.join("grants.jsonl");
        let mut text = std::fs::read_to_string(&log).unwrap();
        text.push_str("{\"seq\": 14, \"at\": \"2026-11-1");
        std::fs::write(&log, text).unwrap();
        let v = view(&cat, &coll, None).unwrap();
        assert!(matches!(v.step, Step::Page), "the page, not the picker");
        assert_eq!(v.cards.len(), 12);
        assert!(!v.damaged);
    }

    /// #374 B1: a line in the middle that cannot be read shows the cards before it and the page, so PLAY starts; the
    /// page says the file is damaged.
    #[test]
    fn a_broken_middle_line_shows_the_cards_before_it() {
        let cat = catalogue();
        let dir = tempfile::tempdir().unwrap();
        let (coll, data) = profile(dir.path());
        deal(&cat, &coll, Choice::Season(2013)).unwrap();
        let mut step = view(&cat, &coll, None).unwrap().step;
        if let Step::Pick { picked, .. } = &mut step {
            *picked = Some(0);
        }
        take(&coll, &data, &step).unwrap();
        let log = coll.join("grants.jsonl");
        let text = std::fs::read_to_string(&log).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        lines[5] = "not a line"; // the 6th of the 12 starter lines
        std::fs::write(&log, lines.join("\n") + "\n").unwrap();
        let v = view(&cat, &coll, None).unwrap();
        assert!(matches!(v.step, Step::Page), "the page, not the picker");
        assert!(v.damaged);
        assert_eq!(v.cards.len(), 5, "the cards before the break");
    }

    /// 1.5 (1), (4), 1.5 (f) through the tab: a log with a broken middle line, START OVER confirmed: the archive holds
    /// the broken log byte-identical, the tab offers the first start again, and the new start writes 12 starter lines
    /// and no second welcome (its mark stays byte-identical).
    #[test]
    fn a_damaged_log_starts_over_from_the_collection_tab() {
        let cat = catalogue();
        let dir = tempfile::tempdir().unwrap();
        let (coll, data) = profile(dir.path());
        let first = |coll: &Path| {
            deal(&cat, coll, Choice::Season(2013)).unwrap();
            let mut step = view(&cat, coll, None).unwrap().step;
            if let Step::Pick { picked, .. } = &mut step {
                *picked = Some(0);
            }
            take(coll, &data, &step).unwrap()
        };
        assert!(first(&coll).iter().any(|e| e.kind == c::Kind::Welcome));
        let log = coll.join("grants.jsonl");
        let text = std::fs::read_to_string(&log).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        lines[5] = "not a line";
        std::fs::write(&log, lines.join("\n") + "\n").unwrap();
        let broken = std::fs::read(&log).unwrap();
        let mark = std::fs::read(data.join(c::WELCOME_MARK)).unwrap();
        assert!(view(&cat, &coll, None).unwrap().damaged);
        let to = reset(&data, None).unwrap();
        assert_eq!(
            std::fs::read(to.join("collection").join("grants.jsonl")).unwrap(),
            broken
        );
        let v = view(&cat, &coll, None).unwrap();
        assert!(matches!(v.step, Step::Choose { .. }), "the first start again");
        assert!(!v.damaged && v.cards.is_empty() && v.tokens == 0);
        let lines = first(&coll);
        assert_eq!(lines.len(), 12);
        assert!(lines.iter().all(|e| e.kind == c::Kind::Starter), "no second welcome");
        assert_eq!(std::fs::read(data.join(c::WELCOME_MARK)).unwrap(), mark);
    }

    /// 1.5 review M1: START OVER is refused while a start holds the kit's admission (between its checks and its game,
    /// the window where it could land under the start); the profile stays where it is.
    #[test]
    fn start_over_is_refused_under_a_start() {
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::at(dir.path());
        let (coll, _) = profile(&kit.data());
        std::fs::create_dir_all(&coll).unwrap();
        // the start's admission on its own thread, as the start's job holds it (the mutex is the thread's)
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let start = std::thread::spawn({
            let kit = Kit::at(dir.path());
            move || {
                let admitted = run::admit(&kit).expect("the start's admission");
                held_tx.send(()).unwrap();
                done_rx.recv().unwrap();
                drop(admitted);
            }
        });
        held_rx.recv().unwrap();
        let r = reset_kit(&kit);
        assert!(matches!(r, Err(ResetRefused::Busy)), "{r:?}");
        assert!(coll.is_dir(), "the profile stays under a start");
        done_tx.send(()).unwrap();
        start.join().unwrap();
        let r = reset_kit(&kit);
        assert!(!matches!(r, Err(ResetRefused::Busy)), "the start is done: {r:?}");
    }
}
