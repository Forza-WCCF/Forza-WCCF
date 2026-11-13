//! Create-a-Team's COLLECTION tab (2017-18, `PLAN.md` 1.4): the profile's log folded into the tab's view, the first
//! start's deal and its lines. The economy is `forza::collection`'s; this file only reads and asks it.

use std::hash::BuildHasher;
use std::io;
use std::path::Path;
use std::time::Instant;

use forza::board::Card;
use forza::cards::collection_cards;
use forza::collection::{self as c, LEAGUES, Log, NATIONS, Rarity};
use forza_ui::collection::{Choice, CollectionView, Filter, Offer, PackChoice, StartRow, Step};

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
/// chosen or a deal not taken yet.
pub fn view(catalogue: &[Card], dir: &Path, old: Option<CollectionView>) -> io::Result<CollectionView> {
    let folded = c::fold(&Log::new(dir).lines()?);
    if folded.unreadable {
        return Err(io::Error::new(io::ErrorKind::InvalidData, folded.warnings.join("; ")));
    }
    let s = folded.state;
    let started = s.cards.values().any(|k| k.starter > 0);
    let (filter, open, step) = match old {
        Some(o) => (o.filter, o.open, o.step),
        None => (Filter::default(), None, Step::Page),
    };
    let step = match step {
        kept @ (Step::Store { .. } | Step::Open { .. }) if started => kept,
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
        offers: offers(catalogue),
    })
}

/// The store's packs (`PLAN.md` 2.3): Standard, each season of the catalogue, the 24 nations with their card counts;
/// each with one card's odds after the step-down.
pub fn offers(catalogue: &[Card]) -> Vec<Offer> {
    let pool = collection_cards(catalogue);
    let mut seasons: Vec<u16> = pool.iter().map(|c| c.season).filter(|&y| y > 0).collect();
    seasons.sort_unstable();
    seasons.dedup();
    std::iter::once(PackChoice::Standard)
        .chain(seasons.into_iter().map(PackChoice::Season))
        .chain((0..NATIONS.len()).map(PackChoice::Nation))
        .map(|pack| Offer {
            pack,
            cards: pool.iter().filter(|c| pack.pack().has(c)).count() as u32,
            odds: c::pack_odds(&pool, pack.pack()),
        })
        .collect()
}

/// BUY: the pack paid and drawn from a fresh seed under the log's lock; the cards' numbers, in the order drawn.
pub fn buy(catalogue: &[Card], dir: &Path, choice: PackChoice) -> io::Result<Vec<u32>> {
    let pool = collection_cards(catalogue);
    let mut rng = c::Rng::new(std::hash::RandomState::new().hash_one(Instant::now()));
    std::fs::create_dir_all(dir)?;
    let lines = c::buy_pack(&Log::new(dir), c::now(), &mut rng, &pool, choice.pack())?;
    Ok(lines.iter().filter_map(|e| e.card).collect())
}

/// The deal on disk as catalogue cards: `None` when there is none, or the catalogue no longer holds one of its numbers
/// (a rebuild).
fn pending(catalogue: &[Card], dir: &Path) -> Option<(Choice, Vec<Card>, Vec<Card>)> {
    let text = std::fs::read_to_string(dir.join(PENDING)).ok()?;
    let deal: Pending = serde_json::from_str(&text).ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use forza::board::{Position, Rarity as R};
    use forza::profile::Profile;
    use std::path::PathBuf;

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

    /// 2.1 through the tab: the store offers Standard, each season and the 24 nations (none of the 7 dropped); the
    /// welcome's 150 buy one Standard pack and not a second; the opened pack shows its 3 cards with their copies, and a
    /// view made again keeps the opened pack on the screen.
    #[test]
    fn the_store_sells_packs() {
        let cat = catalogue();
        let o = offers(&cat);
        assert_eq!((o[0].pack, o[0].cards), (PackChoice::Standard, cat.len() as u32));
        assert_eq!(
            o.iter()
                .filter(|x| matches!(x.pack, PackChoice::Season(_)))
                .map(|x| x.pack)
                .collect::<Vec<_>>(),
            [PackChoice::Season(2013), PackChoice::Season(2014)]
        );
        let nations: Vec<&str> = o
            .iter()
            .filter_map(|x| match x.pack {
                PackChoice::Nation(i) => Some(NATIONS[i]),
                _ => None,
            })
            .collect();
        assert_eq!(nations.len(), 24);
        for out in [
            "SAMURAI BLUE",
            "Mexico",
            "Romania",
            "Paraguay",
            "Cote d'Ivoire",
            "Ghana",
            "Greece",
            "Nigeria",
        ] {
            assert!(!nations.contains(&out), "{out}");
        }

        let dir = tempfile::tempdir().unwrap();
        let (coll, data) = profile(dir.path());
        deal(&cat, &coll, Choice::AllSeasons).unwrap();
        let Step::Pick {
            choice, dealt, rares, ..
        } = view(&cat, &coll, None).unwrap().step
        else {
            panic!("no deal on the screen")
        };
        let step = Step::Pick {
            choice,
            dealt,
            rares,
            picked: Some(0),
        };
        take(&coll, &data, &step).unwrap();
        let cards = buy(&cat, &coll, PackChoice::Standard).unwrap();
        assert_eq!(cards.len(), c::PACK_CARDS);
        let e = buy(&cat, &coll, PackChoice::Standard).unwrap_err();
        assert_eq!(e.to_string(), c::NOT_ENOUGH_TOKENS);
        let mut v = view(&cat, &coll, None).unwrap();
        assert_eq!(v.tokens, c::WELCOME - c::STANDARD_PACK);
        v.step = Step::Store { chosen: 0 };
        v.open_pack(&cards);
        let Step::Open { cards: opened, .. } = &v.step else {
            panic!("the pack is not open")
        };
        assert_eq!(opened.iter().map(|k| k.card.number).collect::<Vec<_>>(), cards);
        assert!(opened.iter().all(|k| k.copies >= 1));
        let again = view(&cat, &coll, Some(v)).unwrap();
        assert!(
            matches!(again.step, Step::Open { .. }),
            "a view made again keeps the pack open"
        );
    }
}
