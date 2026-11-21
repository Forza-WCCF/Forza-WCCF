//! The market page's requests (step 4.1, `PLAN.md` 3.5): one job at a time on its own thread, so the window never
//! waits for the server. `forza::market::Client` keeps the outbox and writes the log lines; this file picks the words
//! and brings the page's lists back.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use forza::market::{Client, Fail, Row, Saved, StockList, Verb};
use forza_ui::collection::CollectionAction;
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
}

/// The job running, if any.
#[derive(Default)]
pub struct Market {
    rx: Option<mpsc::Receiver<Done>>,
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
        self.rx = Some(rx);
        thread::spawn(move || {
            let _ = tx.send(run(&data, &collection, action));
            super::poke();
        });
        true
    }

    /// The finished job, once.
    pub fn poll(&mut self) -> Option<Done> {
        match self.rx.as_ref()?.try_recv() {
            Ok(done) => {
                self.rx = None;
                Some(done)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.rx = None;
                None
            }
        }
    }
}

/// START OVER in the market: RESET into the outbox on disk before the profile is archived, so it goes before any later
/// request (the next job sends it) even after a crash between. Gives the state before, for [`put_back`] when the
/// archive fails. Never while a job runs: that job's own save would drop it.
pub fn queue_reset(data: &Path, collection: &Path) -> Result<Saved, Fail> {
    // the desk is not used: queueing sends nothing
    Client::open(([0, 0, 0, 0], 0).into(), data, collection)?.queue_reset()
}

/// The market state before [`queue_reset`], back: the profile reset failed.
pub fn put_back(data: &Path, collection: &Path, before: &Saved) -> Result<(), Fail> {
    Client::open(([0, 0, 0, 0], 0).into(), data, collection)?.save(before)
}

fn run(data: &Path, collection: &Path, action: CollectionAction) -> Done {
    let mut done = Done {
        action: action.clone(),
        said: Ok(()),
        stock: None,
        sale: None,
        more: false,
        resent: None,
    };
    let c = desk(&launcher::lobby::project_server(Edition::E1718)).and_then(|d| Client::open(d, data, collection));
    let c = match c {
        Ok(c) => c,
        Err(e) => {
            done.said = Err(e);
            return done;
        }
    };
    done.said = (|| {
        let n = |x: u32| u64::from(x);
        match action {
            CollectionAction::MarketJoin => c.ask(Verb::Join, &[]).map(drop)?,
            CollectionAction::MarketLeave => return c.leave().map(drop),
            CollectionAction::StockBuy(slot, card, price) => {
                c.ask(Verb::StockBuy, &[slot, n(card), n(price)]).map(drop)?;
            }
            CollectionAction::MarketBuy(id, card, price) => {
                c.ask(Verb::Buy, &[id, n(card), n(price)]).map(drop)?;
            }
            CollectionAction::List(card, price) => c.ask(Verb::List, &[n(card), n(price)]).map(drop)?,
            CollectionAction::Unlist(id) => c.ask(Verb::Cancel, &[id]).map(drop)?,
            CollectionAction::MarketMore(after) => {
                let (sale, more) = c.browse_pages(after, BROWSE_PAGES)?;
                (done.sale, done.more) = (Some(sale), more);
                return Ok(());
            }
            _ => c.flush()?,
        }
        let v = c.visit(BROWSE_PAGES)?;
        (done.stock, done.sale, done.more) = (v.stock, v.sale, v.more);
        Ok(())
    })();
    done.resent = c.resent_refused();
    done
}

/// The market desk at `server` (the lobby's name or address); a name that does not resolve sends nothing.
fn desk(server: &str) -> Result<std::net::SocketAddr, Fail> {
    launcher::lobby::socket(server).ok_or_else(|| Fail::Unresolved(server.to_owned()))
}

/// The server's refusal reasons a player meets, in the server's words (`crates/server/src/market.rs`, the desk's
/// [`forza::market::NOT_SET_UP`]), each with its `ui.yml` key; `{name}` stands for a number. A reason not here (a log
/// the server's check refuses, line by line) shows in English as the server said it.
const REASONS: [(&str, &str); 23] = [
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
}
