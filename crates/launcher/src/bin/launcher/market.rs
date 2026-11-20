//! The market page's requests (step 4.1, `PLAN.md` 3.5): one job at a time on its own thread, so the window never
//! waits for the server. `forza::market::Client` keeps the outbox and writes the log lines; this file picks the words
//! and brings the page's lists back.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use forza::market::{Client, Fail, Row, Saved, StockList, Verb};
use forza_ui::collection::CollectionAction;
use launcher::Edition;

/// BROWSE pages one visit asks at most.
// shortcut: 3 pages of 40 listings; a MORE button when the market holds more than 120 at once
const BROWSE_PAGES: usize = 3;

/// What a job brings back: its action, how it went, and the page's lists when it asked for them.
pub struct Done {
    pub action: CollectionAction,
    pub said: Result<(), Fail>,
    pub stock: Option<StockList>,
    pub sale: Option<Vec<Row>>,
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
        resent: None,
    };
    let server = launcher::lobby::project_server(Edition::E1718);
    let Some(desk) = launcher::lobby::socket(&server) else {
        done.said = Err(Fail::Unanswered(format!("{server} could not be resolved")));
        return done;
    };
    let c = match Client::open(desk, data, collection) {
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
            CollectionAction::MarketLeave => return c.ask(Verb::Leave, &[]).map(drop),
            CollectionAction::StockBuy(slot, card, price) => {
                c.ask(Verb::StockBuy, &[slot, n(card), n(price)]).map(drop)?;
            }
            CollectionAction::MarketBuy(id, card, price) => {
                c.ask(Verb::Buy, &[id, n(card), n(price)]).map(drop)?;
            }
            CollectionAction::List(card, price) => c.ask(Verb::List, &[n(card), n(price)]).map(drop)?,
            CollectionAction::Unlist(id) => c.ask(Verb::Cancel, &[id]).map(drop)?,
            _ => c.flush()?,
        }
        // the page: the sales paid and the listings back first, then the lists
        let saved = Client::saved(data);
        if saved.joined || saved.waiting > 0 {
            c.collect()?;
        }
        if saved.joined {
            done.stock = Some(c.stock()?);
            let mut sale: Vec<Row> = Vec::new();
            for _ in 0..BROWSE_PAGES {
                let page = c.browse(sale.last().map_or(0, |r| r.id))?;
                let end = page.len() < forza::market::BROWSE_PAGE;
                sale.extend(page);
                if end {
                    break;
                }
            }
            done.sale = Some(sale);
        }
        Ok(())
    })();
    done.resent = c.resent_refused();
    done
}
