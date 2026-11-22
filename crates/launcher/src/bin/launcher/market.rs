//! The market page's requests (step 4.1, `PLAN.md` 3.5): one job at a time on its own thread, so the window never
//! waits for the server. `forza::market::Client` keeps the outbox and writes the log lines; this file picks the words
//! and brings the page's lists back.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use forza::market::{Client, Fail, Row, Saved, StockList, Verb};
use forza_ui::collection::{CollectionAction, MarketView, Offered};
use forza_ui::widgets::Tone;
use launcher::Edition;

/// BROWSE pages one visit asks at most, and MORE after them.
const BROWSE_PAGES: usize = 3;

/// What a job brings back: its action, how it went, and the page's lists when it asked for them.
pub struct Done {
    pub action: CollectionAction,
    pub said: Result<(), Fail>,
    pub stock: Option<StockList>,
    pub sale: Option<Vec<Row>>,
    /// The server holds more listings past `sale`: MORE asks them.
    pub more: bool,
    /// A request kept from before, sent again and refused, as `VERB: why`.
    pub resent: Option<String>,
    /// The visit after a player's word that went through failed (#375): the word's own answer is `said`.
    pub visit: Option<Fail>,
    /// Sales paid into the log by this job's COLLECT.
    pub sold: usize,
}

/// The job running, if any.
#[derive(Default)]
pub struct Market {
    rx: Option<(mpsc::Receiver<Done>, CollectionAction)>,
}

impl Market {
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Runs `action` for the installation's `data` and the profile's `collection` folder; `false` while another job
    /// runs.
    pub fn start(&mut self, data: PathBuf, collection: PathBuf, action: CollectionAction) -> bool {
        if self.rx.is_some() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some((rx, action.clone()));
        thread::spawn(move || {
            let _ = tx.send(run(&data, &collection, action));
            super::poke();
        });
        true
    }

    /// The finished job, once; a worker that ended without an answer (a panic) is one that failed on this PC, so the
    /// page leaves its busy state with a note (#375).
    pub fn poll(&mut self) -> Option<Done> {
        let (rx, action) = self.rx.as_ref()?;
        let done = match rx.try_recv() {
            Ok(done) => done,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Done {
                said: Err(Fail::Disk("the market request stopped without an answer".into())),
                ..Done::new(action.clone())
            },
        };
        self.rx = None;
        Some(done)
    }
}

/// START OVER in the market: RESET into the outbox on disk before the profile is archived, so it goes before any later
/// request (the next job sends it) even after a crash between. Gives the state before, for [`put_back`] when the
/// archive fails. Never while a job runs: that job's own save would drop it.
pub fn queue_reset(data: &Path, collection: &Path) -> Result<(Saved, Option<Fail>), Fail> {
    // the desk is not used: queueing sends nothing
    Client::open(([0, 0, 0, 0], 0).into(), data, collection)?.queue_reset()
}

/// The market state before [`queue_reset`], back: the profile reset failed.
pub fn put_back(data: &Path, collection: &Path, before: &Saved) -> Result<(), Fail> {
    Client::open(([0, 0, 0, 0], 0).into(), data, collection)?.save(before)
}

impl Done {
    fn new(action: CollectionAction) -> Self {
        Self {
            action,
            said: Ok(()),
            stock: None,
            sale: None,
            more: false,
            resent: None,
            visit: None,
            sold: 0,
        }
    }
}

/// A player's word (BUY, LIST, TAKE BACK, JOIN, LEAVE), not the page's own visit or MORE.
pub fn is_word(a: &CollectionAction) -> bool {
    !matches!(a, CollectionAction::Market | CollectionAction::MarketMore(_))
}

/// The `market_sell` lines of the log: the sales paid into it.
fn sales(collection: &Path) -> usize {
    forza::collection::Log::new(collection)
        .lines()
        .map_or(0, |l| l.matches(r#""kind":"market_sell""#).count())
}

fn run(data: &Path, collection: &Path, action: CollectionAction) -> Done {
    let mut done = Done::new(action.clone());
    let c = desk(&launcher::lobby::project_server(Edition::E1718)).and_then(|d| Client::open(d, data, collection));
    let c = match c {
        Ok(c) => c,
        Err(e) => {
            done.said = Err(e);
            return done;
        }
    };
    let sold = sales(collection);
    let n = |x: u32| u64::from(x);
    let (said, visit) = match action {
        CollectionAction::MarketJoin => (c.ask(Verb::Join, &[]).map(drop), true),
        CollectionAction::MarketLeave => (c.leave().map(drop), false),
        CollectionAction::StockBuy(slot, card, price) => {
            (c.ask(Verb::StockBuy, &[slot, n(card), n(price)]).map(drop), true)
        }
        CollectionAction::MarketBuy(id, card, price) => (c.ask(Verb::Buy, &[id, n(card), n(price)]).map(drop), true),
        CollectionAction::List(card, price) => (c.ask(Verb::List, &[n(card), n(price)]).map(drop), true),
        CollectionAction::Unlist(id) => (c.ask(Verb::Cancel, &[id]).map(drop), true),
        CollectionAction::MarketMore(after) => (
            c.browse_pages(after, BROWSE_PAGES)
                .map(|(sale, more)| (done.sale, done.more) = (Some(sale), more)),
            false,
        ),
        _ => (c.flush(), true),
    };
    done.said = said;
    if visit && done.said.is_ok() {
        match c.visit(BROWSE_PAGES) {
            Ok(v) => (done.stock, done.sale, done.more) = (v.stock, v.sale, v.more),
            // a word that went through stays a success when the visit after it is refused (`slow`)
            Err(e) if is_word(&action) => done.visit = Some(e),
            Err(e) => done.said = Err(e),
        }
    }
    done.sold = sales(collection).saturating_sub(sold);
    done.resent = c.resent_refused();
    done
}

/// The footer line for a finished job (#375): a player's word refused or unanswered says `note` (the market page's
/// own line), one that went through says so, and sales paid say so; the page's own visit says nothing else.
pub fn footer(lang: forza_ui::Lang, d: &Done, note: Option<&str>) -> Option<(Tone, String)> {
    if d.said.is_err() && is_word(&d.action) {
        return note.map(|n| (Tone::Refused, n.to_owned()));
    }
    let key = match d.action {
        _ if d.said.is_err() => None,
        CollectionAction::StockBuy(..) | CollectionAction::MarketBuy(..) => Some("collection.market_said_bought"),
        CollectionAction::List(..) => Some("collection.market_said_listed"),
        CollectionAction::Unlist(_) => Some("collection.market_said_unlisted"),
        _ => None,
    };
    let sold = (d.sold > 0).then(|| forza_ui::text(lang, "collection.market_said_sold", &[("n", &d.sold.to_string())]));
    let line = [key.map(|k| forza_ui::text(lang, k, &[])), sold]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    (!line.is_empty()).then_some((Tone::Done, line))
}

/// A kept request's `VERB: why` with the verb as the page's button reads in `lang` (#375).
pub fn resent_in(lang: forza_ui::Lang, said: &str) -> String {
    let Some((verb, why)) = said.split_once(": ") else {
        return said.to_owned();
    };
    let key = match verb {
        "BUY" | "STOCKBUY" => "collection.market_word_buy",
        "LIST" => "collection.market_word_list",
        "CANCEL" => "collection.market_unlist",
        "COLLECT" => "collection.market_refresh",
        "RESET" => "collection.reset",
        _ => return format!("{verb}: {}", reason_in(lang, why)),
    };
    format!("{}: {}", forza_ui::text(lang, key, &[]), reason_in(lang, why))
}

/// Whether the page's `a` asks the server: a player's word always; the page's own visit (`Market`) only in the market,
/// while a sale or a request waits, or while the log holds open listings (`open`: outside the market they came back
/// with a LEAVE whose answer was lost, and the visit writes them).
pub fn asks(a: &CollectionAction, saved: &Saved, open: bool) -> bool {
    *a != CollectionAction::Market || saved.joined || saved.waiting > 0 || saved.outbox.is_some() || open
}

/// A job's listings on the page: MORE's after the ones shown, any other visit's in their place.
pub fn show_sale(m: &mut MarketView, action: &CollectionAction, rows: Vec<Offered>, more: bool) {
    if matches!(action, CollectionAction::MarketMore(_)) {
        m.sale.extend(rows);
    } else {
        m.sale = rows;
    }
    m.more = more;
}

/// The market desk at `server` (the lobby's name or address); a name that does not resolve sends nothing.
fn desk(server: &str) -> Result<std::net::SocketAddr, Fail> {
    launcher::lobby::socket(server).ok_or_else(|| Fail::Unresolved(server.to_owned()))
}

/// The server's refusal reasons a player meets, in the server's words (`crates/server/src/market.rs`, the desk's
/// [`forza::market::NOT_SET_UP`]), each with its `ui.yml` key; `{name}` stands for a number. A reason not here (a log
/// the server's check refuses, line by line) shows in English as the server said it.
const REASONS: [(&str, &str); 26] = [
    ("this installation is not in the market", "collection.market_why_not_in"),
    ("that listing is no longer on the market", "collection.market_why_gone"),
    ("that listing sold: COLLECT pays it", "collection.market_why_sold"),
    (
        "that slot is sold: each sells once a day",
        "collection.market_why_slot_sold",
    ),
    (
        "this installation has no listing with that id",
        "collection.market_why_no_listing",
    ),
    (
        "that is this installation's own listing: CANCEL takes it back",
        "collection.market_why_own",
    ),
    (
        "{n} listings are open: one must sell, come back or be cancelled first",
        "collection.market_why_open",
    ),
    (
        "that listing's seller sold {n} cards this week: it sells again next week",
        "collection.market_why_seller_week",
    ),
    (
        "{tokens} tokens to spend, the listing costs {price}",
        "collection.market_why_tokens",
    ),
    (
        "{tokens} tokens to spend, the slot costs {price}",
        "collection.market_why_tokens",
    ),
    (
        "a price of {price} for card {card}: its band is {lo} to {hi} tokens",
        "collection.market_why_band",
    ),
    (
        "card {card} is held only as a starter or imported copy, which never lists",
        "collection.market_why_starter",
    ),
    (
        "no earned copy of card {card} is left off the market",
        "collection.market_why_no_copy",
    ),
    (
        "today's stock list has no slot {slot} of card {card} at {price}: the list turns at 04:00 UTC",
        "collection.market_why_list_changed",
    ),
    (
        "{n} stock purchases today, this server's limit: the list turns at 04:00 UTC",
        "collection.market_why_limit",
    ),
    (
        "this PC's clock is more than a day off the server's",
        "collection.market_why_clock",
    ),
    (
        "update first: this Launcher's market rules are older than the server's",
        "collection.market_why_update",
    ),
    (
        "this server's market rules are older than this Launcher's",
        "collection.market_why_server_old",
    ),
    (
        "the market is not set up on this server",
        "collection.market_why_not_set_up",
    ),
    (
        "this log is in the market under another installation's key",
        "collection.market_why_another_key",
    ),
    (
        "a second welcome: this installation had its welcome tokens before",
        "collection.market_why_welcome",
    ),
    (
        "card {card} is not in this server's table",
        "collection.market_why_no_card",
    ),
    (
        "an old request (sent twice?), the last at {last}",
        "collection.market_why_old",
    ),
    (
        "a fork: this log drops {n} of the {total} lines the server accepted (a profile reset sends MARKET RESET first)",
        "collection.market_why_fork",
    ),
    ("busy, try again in a minute", "collection.market_why_busy"),
    // the desk's request limit per address
    ("slow", "collection.market_why_slow"),
];

/// The server's refusal `why` in the player's language, for the "refused" lines.
pub fn reason(why: &str) -> String {
    reason_in(crate::text::lang(), why)
}

fn reason_in(lang: forza_ui::Lang, why: &str) -> String {
    REASONS
        .iter()
        .find_map(|(shape, key)| Some(forza_ui::text(lang, key, &numbers(shape, why)?)))
        .unwrap_or_else(|| why.to_owned())
}

/// `why`'s numbers by name when it reads as `shape`, whose `{name}` parts each stand for one number.
fn numbers<'a, 'b>(shape: &'a str, why: &'b str) -> Option<Vec<(&'a str, &'b str)>> {
    let mut parts = shape.split('{');
    let mut rest = why.strip_prefix(parts.next()?)?;
    let mut out = vec![];
    for part in parts {
        let (name, words) = part.split_once('}')?;
        let end = rest
            .char_indices()
            .find(|&(i, c)| !(c.is_ascii_digit() || i == 0 && c == '-'))
            .map_or(rest.len(), |(i, _)| i);
        if end == 0 {
            return None;
        }
        out.push((name, &rest[..end]));
        rest = rest[end..].strip_prefix(words)?;
    }
    rest.is_empty().then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #375: a server name that does not resolve is told apart from a server that does not answer.
    #[test]
    fn a_name_that_does_not_resolve_is_not_unanswered() {
        assert_eq!(
            desk("wccf-market.invalid"),
            Err(Fail::Unresolved("wccf-market.invalid".into()))
        );
        assert!(desk("127.0.0.1").is_ok());
    }

    /// #375: the server's refusals read in the player's language, numbers in place; one not in the table stays as the
    /// server said it.
    #[test]
    fn a_refusal_reads_in_the_players_language() {
        use forza_ui::Lang;
        assert_eq!(
            reason_in(Lang::It, "this installation is not in the market"),
            "questa installazione non è nel mercato."
        );
        assert_eq!(
            reason_in(Lang::En, "-5 tokens to spend, the slot costs 216"),
            "you have -5 tokens to spend, and it costs 216."
        );
        assert_eq!(
            reason_in(Lang::Ja, "a price of 300 for card 57: its band is 192 to 228 tokens"),
            "300 トークンはカード 57 の価格の範囲（192〜228 トークン）の外です。"
        );
        assert_eq!(
            reason_in(Lang::It, "an old request (sent twice?), the last at 1763700000000"),
            "una richiesta è arrivata due volte. Riprova."
        );
        // the desk's request limit (#375): no longer "refused: slow"
        for lang in Lang::ALL {
            assert_ne!(reason_in(lang, "slow"), "slow", "{lang:?}");
        }
        let check = "line 15: a match 179 s after the one before";
        assert_eq!(reason_in(Lang::It, check), check);
        assert_eq!(
            reason_in(Lang::It, "that listing sold: COLLECT pays it twice"),
            "that listing sold: COLLECT pays it twice"
        );
        // every shape, its numbers filled, reads translated in every language with nothing left unfilled
        for (shape, key) in REASONS {
            let why = shape
                .split('{')
                .enumerate()
                .map(|(i, p)| {
                    if i == 0 {
                        p.to_owned()
                    } else {
                        format!("7{}", p.split_once('}').unwrap().1)
                    }
                })
                .collect::<String>();
            for lang in Lang::ALL {
                let said = reason_in(lang, &why);
                assert_eq!(said, forza_ui::text(lang, key, &numbers(shape, &why).unwrap()), "{why}");
                assert!(said != why && !said.contains("%{"), "{lang:?} {why}: {said}");
            }
        }
    }

    /// #375: a worker that ends without an answer (a panic) is a finished job carrying an error, so the page leaves its
    /// busy state with a note.
    #[test]
    fn a_worker_gone_without_an_answer_is_a_failed_job() {
        let (tx, rx) = mpsc::channel();
        let mut m = Market {
            rx: Some((rx, CollectionAction::List(7, 40))),
        };
        assert!(m.busy() && m.poll().is_none(), "still running");
        drop(tx);
        let d = m.poll().expect("a Done");
        assert_eq!(d.action, CollectionAction::List(7, 40));
        assert!(matches!(d.said, Err(Fail::Disk(_))), "{:?}", d.said);
        assert!(!m.busy() && m.poll().is_none(), "once");
    }

    /// #375: a refused word says its note in the footer (LIST is pressed on the collection page, which has no market
    /// line); one that went through says so, also when the visit after it was refused; sales paid say so; the page's
    /// own visit says nothing else.
    #[test]
    fn the_footer_says_each_refusal_and_success() {
        use forza_ui::Lang;
        let done = |action, said| Done {
            said,
            ..Done::new(action)
        };
        let refused = done(CollectionAction::List(7, 40), Err(Fail::Refused("slow".into())));
        assert_eq!(
            footer(Lang::En, &refused, Some("why")),
            Some((Tone::Refused, "why".into()))
        );
        let visit = done(CollectionAction::Market, Err(Fail::Refused("slow".into())));
        assert_eq!(footer(Lang::En, &visit, Some("why")), None, "the page's line only");
        assert_eq!(footer(Lang::En, &done(CollectionAction::Market, Ok(())), None), None);
        for lang in Lang::ALL {
            for (a, key) in [
                (CollectionAction::MarketBuy(1, 7, 40), "collection.market_said_bought"),
                (CollectionAction::StockBuy(1, 7, 40), "collection.market_said_bought"),
                (CollectionAction::List(7, 40), "collection.market_said_listed"),
                (CollectionAction::Unlist(1), "collection.market_said_unlisted"),
            ] {
                let mut d = done(a, Ok(()));
                d.visit = Some(Fail::Refused("slow".into()));
                let (tone, line) = footer(lang, &d, None).expect("a line");
                assert_eq!((tone, line), (Tone::Done, forza_ui::text(lang, key, &[])));
                assert!(
                    !forza_ui::text(lang, key, &[]).starts_with("collection."),
                    "{lang:?} {key}"
                );
            }
            let mut d = done(CollectionAction::Market, Ok(()));
            d.sold = 2;
            let (tone, line) = footer(lang, &d, None).expect("a sale paid");
            assert!(
                tone == Tone::Done && line.contains('2') && !line.contains("%{"),
                "{lang:?}: {line}"
            );
        }
    }

    /// #375: a kept request's refusal names the action as the page's button reads, not the protocol word.
    #[test]
    fn a_kept_request_names_its_button() {
        use forza_ui::Lang;
        let said = "CANCEL: this installation has no listing with that id";
        assert_eq!(
            resent_in(Lang::It, said),
            "RITIRA: questa installazione non ha questa inserzione."
        );
        for lang in Lang::ALL {
            for verb in ["BUY", "STOCKBUY", "LIST", "CANCEL", "COLLECT", "RESET"] {
                let line = resent_in(lang, &format!("{verb}: slow"));
                assert!(!line.starts_with(verb) || lang == Lang::En, "{lang:?}: {line}");
                assert!(
                    !line.contains("collection.") && !line.ends_with("slow"),
                    "{lang:?}: {line}"
                );
            }
        }
    }

    /// #375: the page's own visit goes out in the market, while a sale or a request waits, or while the log holds open
    /// listings outside the market (a lost LEAVE answer); a player's word always.
    #[test]
    fn the_page_asks_only_with_something_to_ask() {
        let out = Saved::default();
        assert!(!asks(&CollectionAction::Market, &out, false));
        assert!(asks(&CollectionAction::Market, &out, true), "open listings outside");
        let waiting = Saved {
            waiting: 1,
            ..Saved::default()
        };
        assert!(asks(&CollectionAction::Market, &waiting, false));
        assert!(asks(&CollectionAction::MarketJoin, &out, false));
    }

    /// #375: MORE's listings go after the ones shown and set MORE from its answer; a visit's replace them.
    #[test]
    fn more_adds_its_listings_after_the_ones_shown() {
        let row = |id| Offered {
            id,
            card: forza_ui::sample::cards()[0].clone(),
            price: 40,
            sold: false,
        };
        let ids = |m: &MarketView| m.sale.iter().map(|o| o.id).collect::<Vec<_>>();
        let mut m = MarketView::default();
        show_sale(&mut m, &CollectionAction::Market, vec![row(1), row(2)], true);
        assert_eq!((ids(&m), m.more), (vec![1, 2], true));
        show_sale(&mut m, &CollectionAction::MarketMore(2), vec![row(3)], false);
        assert_eq!((ids(&m), m.more), (vec![1, 2, 3], false));
        show_sale(&mut m, &CollectionAction::Market, vec![row(4)], false);
        assert_eq!(ids(&m), [4]);
    }

    /// #375: each shape's words are the server's own (its source files), so a server rewording fails here instead of
    /// showing in English.
    #[test]
    fn every_reason_is_in_the_servers_words() {
        let server = [
            include_str!("../../../../server/src/market.rs"),
            include_str!("../../../../server/src/desk.rs"),
            include_str!("../../../../forza/src/market.rs"),
        ]
        .concat()
        .replace("{OLD}", forza::market::OLD);
        for (shape, _) in REASONS {
            for part in shape.split('{').map(|p| p.split_once('}').map_or(p, |(_, w)| w)) {
                assert!(server.contains(part), "{part:?} of {shape:?}");
            }
        }
    }
}
