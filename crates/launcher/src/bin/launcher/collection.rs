//! Create-a-Team's COLLECTION tab (2017-18, `PLAN.md` 1.4): the profile's log folded into the tab's view, the first
//! start's deal and its lines. The economy is `forza::collection`'s; this file only reads and asks it.

use std::hash::BuildHasher;
use std::io;
use std::path::Path;
use std::time::Instant;

use forza::board::Card;
use forza::cards::collection_cards;
use forza::collection::{self as c, LEAGUES, Log, Rarity};
use forza_ui::collection::{Choice, CollectionView, Filter, StartRow, Step};

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
        _ if started => Step::Page,
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
    })
}

/// The first start's deal for `choice`: the 11 and the 3 RARE cards, from a fresh seed.
pub fn deal(catalogue: &[Card], choice: Choice) -> (Vec<Card>, Vec<Card>) {
    let pool = collection_cards(catalogue);
    let mut rng = c::Rng::new(std::hash::RandomState::new().hash_one(Instant::now()));
    let back = |picked: Vec<&c::Card>| -> Vec<Card> {
        picked
            .iter()
            .filter_map(|p| catalogue.iter().find(|k| k.number == p.card_no).cloned())
            .collect()
    };
    let eleven = back(c::starter(&mut rng, &pool, choice.start()));
    let rares = back(c::starter_rares(&mut rng, &pool, choice.start()));
    (eleven, rares)
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
    c::first_start(
        &Log::new(dir),
        &data.join(c::WELCOME_MARK),
        c::now(),
        &cards,
        &choice.why(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use forza::board::{Position, Rarity as R};

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

    /// 1.4 (a), (b) through the tab: DEAL, TAKE THIS TEAM writes 12 starter lines of the choice and the welcome, the view
    /// turns to the page with 12 starter cards and 150 tokens; a second TAKE writes nothing.
    #[test]
    fn the_first_start_takes_the_deal_once() {
        let cat = catalogue();
        let dir = tempfile::tempdir().unwrap();
        let (coll, data) = (dir.path().join("collection"), dir.path().to_path_buf());
        let v = view(&cat, &coll, None).unwrap();
        let Step::Choose { starts, .. } = &v.step else {
            panic!("a new profile opens on the picker")
        };
        assert_eq!(starts[0].choice, Choice::AllSeasons);
        assert_eq!(starts.len(), 1 + 2 + LEAGUES.len());
        let (dealt, rares) = deal(&cat, Choice::League(0));
        assert_eq!((dealt.len(), rares.len()), (11, 3));
        let step = Step::Pick {
            choice: Choice::League(0),
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
        let v = view(&cat, &coll, Some(v)).unwrap();
        assert!(matches!(v.step, Step::Page));
        assert_eq!((v.cards.len(), v.tokens), (12, c::WELCOME));
        assert!(v.cards.iter().all(|(_, n)| n.starter == 1));
        let before = std::fs::read(coll.join("grants.jsonl")).unwrap();
        assert!(take(&coll, &data, &step).unwrap().is_empty());
        assert_eq!(std::fs::read(coll.join("grants.jsonl")).unwrap(), before);
    }
}
