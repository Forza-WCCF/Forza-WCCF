//! The tablet formation board (issue #94, 2017-18, seat 1): a page on the home network that plays the formation by
//! touch. Off by default: it runs while panel.txt says `tablet=on` and the run is up. Launcher.exe serves it (no helper
//! program): one HTTP and WebSocket listener on this PC's private address only, a random token per start that every
//! request must carry (the panel shows it as a QR code), peers outside that network refused, one tablet at a time.
//!
//! Board moves go through `forza`'s own board code (`cards::board`, `Board::swap`/`move_to`, `cards::table_after`,
//! `TableFile::write`), as the panel's do; the table file is watched and pushed to the tablet when someone else writes
//! it. The buttons go straight into the key driver's held set (`forza::keys::TABLET`), whichever window is in front.
//! Windows' firewall is never touched: Windows asks the player the first time, and the panel says what to answer.

use crate::kit::Kit;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path as UrlPath, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use forza::board::Card;
use forza::cards::{self, Placed, TableFile};
use forza::panel_txt::PanelFile;
use forza::service::{Handle, Log};
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// The port the QR names when it is free (else one Windows picks).
pub const PORT: u16 = 20080;
/// The file the panel reads the QR's address from: `http://...` or `!` and why there is none.
pub const URL_FILE: &str = "tablet.url";
/// A button press lasts at least this long, as the panel's own buttons.
const MIN_HOLD: Duration = Duration::from_millis(140);
/// How often the table file is checked for someone else's write.
const WATCH: Duration = Duration::from_millis(30);
/// The tablet's buttons: the cabinet's play buttons and tactics; never CARD, COIN or the operator's switches.
pub const BUTTONS: [&str; 10] = [
    "START",
    "PRESS",
    "SHOOT",
    "KEEPER",
    "DATA",
    "KEYPLAYER",
    "UP",
    "DOWN",
    "LEFT",
    "RIGHT",
];

/// What the board serves: the seat's table, the player's catalogue and pictures, the key driver's tablet set.
pub struct Board {
    /// The game's folder: `fpr_table0.txt` (seat1\).
    pub dir: PathBuf,
    pub catalogue: Arc<[Card]>,
    /// `<number>.png` card pictures.
    pub pictures: PathBuf,
    pub keys: Arc<AtomicU16>,
    pub log: Log,
}

/// One board message from the tablet, checked: slots 0-15 (0-10 the pitch, 11-15 the bench boxes), table
/// coordinates -1..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Swap { from: usize, to: usize },
    Move { from: usize, x: f32, y: f32 },
    Off { slot: usize },
    Key { action: usize, down: bool },
}

/// A message's JSON to an [`Op`]; `None` for anything malformed, out of range or not allowed.
pub fn parse(text: &str) -> Option<Op> {
    let v: Value = serde_json::from_str(text).ok()?;
    let slot = |k: &str| v.get(k)?.as_u64().filter(|&s| s < 16).map(|s| s as usize);
    let coord = |k: &str| {
        v.get(k)?
            .as_f64()
            .filter(|f| f.is_finite())
            .map(|f| f.clamp(-1.0, 1.0) as f32)
    };
    Some(match v.get("t")?.as_str()? {
        "swap" => Op::Swap {
            from: slot("from")?,
            to: slot("to")?,
        },
        "move" => Op::Move {
            from: slot("from")?,
            x: coord("x")?,
            y: coord("y")?,
        },
        "off" => Op::Off { slot: slot("slot")? },
        "key" => {
            let name = v.get("a")?.as_str()?;
            if !BUTTONS.contains(&name) {
                return None;
            }
            Op::Key {
                action: forza::keys::action_index(name)?,
                down: v.get("down")?.as_bool()?,
            }
        }
        _ => return None,
    })
}

/// A board move on the table, as the panel makes it: the board built from the table, the move, the table after it.
/// `None` when it moves nothing (an empty slot, a full pitch).
pub fn apply(placed: &[Placed], catalogue: &[Card], op: Op) -> Option<Vec<Placed>> {
    let mut b = cards::board(placed, catalogue);
    match op {
        Op::Swap { from, to } => {
            b.slots[from]?;
            b.swap(from, to);
        }
        Op::Move { from, x, y } => {
            b.slots[from]?;
            if !b.move_to(from, eframe::egui::pos2((x + 1.0) / 2.0, (y + 1.0) / 2.0)) {
                return None;
            }
        }
        Op::Off { slot } => {
            b.slots[slot].take()?;
        }
        Op::Key { .. } => return None,
    }
    Some(cards::table_after(placed, &b))
}

/// The table as the page draws it: each slot's card, pitch cards at their spots (table coordinates).
fn state_json(rev: u64, placed: &[Placed], catalogue: &[Card]) -> String {
    let b = cards::board(placed, catalogue);
    let cards: Vec<Value> = (0..16)
        .filter_map(|s| {
            let c = &b.cards[b.slots[s]?];
            let (x, y) = if s < 11 {
                (b.spots[s].x * 2.0 - 1.0, b.spots[s].y * 2.0 - 1.0)
            } else {
                (cards::BENCH_X, cards::BENCH_Y[s - 11])
            };
            Some(json!({
                "slot": s, "no": c.number, "x": x, "y": y, "name": c.name, "short": c.short_name(),
                "pos": c.pos.code(), "rarity": format!("{:?}", c.rarity), "stats": c.stats,
            }))
        })
        .collect();
    json!({"t": "state", "rev": rev, "cards": cards}).to_string()
}

struct Table {
    file: TableFile,
    placed: Vec<Placed>,
    rev: u64,
}

struct Shared {
    token: String,
    /// The network peers must be on: its address and mask.
    net: (u32, u32),
    table: Mutex<Table>,
    catalogue: Arc<[Card]>,
    pictures: PathBuf,
    keys: Arc<AtomicU16>,
    /// Each button's presses so far and its last press (the minimum hold).
    presses: Mutex<[(u64, Option<Instant>); 16]>,
    state: watch::Sender<String>,
    /// The tablet that may play (a new one takes over); `u64::MAX` = the board is stopping.
    owner: watch::Sender<u64>,
    next_id: AtomicU64,
    log: Log,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Equal without telling by its time how much of it was right.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

impl Shared {
    fn peer_ok(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V4(v) => v,
            IpAddr::V6(v) => match v.to_ipv4_mapped() {
                Some(v) => v,
                None => return false,
            },
        };
        u32::from(ip) & self.net.1 == self.net.0 & self.net.1
    }

    /// The table read again when someone else wrote it; true when it was.
    fn reread(&self, t: &mut Table) -> bool {
        if !t.file.changed() {
            return false;
        }
        let Some(p) = t.file.read() else { return false };
        t.placed = p;
        t.rev += 1;
        true
    }

    fn push(&self, t: &Table) {
        self.state.send_replace(state_json(t.rev, &t.placed, &self.catalogue));
    }

    fn board_op(&self, op: Op) {
        let t0 = Instant::now();
        let mut t = lock(&self.table);
        self.reread(&mut t);
        match apply(&t.placed, &self.catalogue, op) {
            Some(after) => match t.file.write(&after) {
                Ok(()) => {
                    t.placed = after;
                    t.rev += 1;
                    (self.log)(&format!(
                        "tablet: {op:?} written in {:.1} ms",
                        t0.elapsed().as_secs_f64() * 1000.0
                    ));
                }
                Err(e) => (self.log)(&format!("tablet: {op:?} not written ({e})")),
            },
            None => (self.log)(&format!("tablet: {op:?} moves nothing")),
        }
        // the page's own move answered: it drops what it guessed
        self.push(&t);
    }

    fn key(self: &Arc<Self>, action: usize, down: bool) {
        let bit = 1u16 << action;
        let mut p = lock(&self.presses);
        if down {
            p[action] = (p[action].0 + 1, Some(Instant::now()));
            self.keys.fetch_or(bit, Ordering::Relaxed);
            return;
        }
        let (n, at) = p[action];
        let left = at.map_or(Duration::ZERO, |t| MIN_HOLD.saturating_sub(t.elapsed()));
        if left.is_zero() {
            self.keys.fetch_and(!bit, Ordering::Relaxed);
            return;
        }
        let s = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(left).await;
            // not if it was pressed again meanwhile
            if lock(&s.presses)[action].0 == n {
                s.keys.fetch_and(!bit, Ordering::Relaxed);
            }
        });
    }
}

/// The token in `t=` of the query, if any.
fn query_token(q: Option<&str>) -> &str {
    q.unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("t="))
        .unwrap_or("")
}

/// Every request: a peer on the home network, with the token.
async fn guard(
    State(s): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    if !s.peer_ok(peer.ip()) {
        (s.log)(&format!("tablet: {peer} refused (not on this network)"));
        return StatusCode::FORBIDDEN.into_response();
    }
    if !same(query_token(req.uri().query()).as_bytes(), s.token.as_bytes()) {
        (s.log)(&format!("tablet: {peer} refused (no or wrong token)"));
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

async fn page() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        include_str!("tablet/page.html"),
    )
        .into_response()
}

async fn picture(State(s): State<Arc<Shared>>, UrlPath(no): UrlPath<String>) -> Response {
    // digits only: no path from the network ever reaches the file system
    let Ok(no) = no.parse::<u32>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::read(s.pictures.join(format!("{no}.png"))).await {
        Ok(b) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "max-age=86400"),
            ],
            b,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn ws(State(s): State<Arc<Shared>>, up: WebSocketUpgrade) -> Response {
    up.max_message_size(1024).on_upgrade(move |sock| tablet(s, sock))
}

/// One tablet's connection: it takes over from any other, gets the table on each change, sends moves and buttons.
async fn tablet(s: Arc<Shared>, mut sock: WebSocket) {
    let id = s.next_id.fetch_add(1, Ordering::Relaxed);
    if *s.owner.borrow() == u64::MAX {
        return;
    }
    s.owner.send_replace(id);
    // the other tablet's buttons are let go
    s.keys.store(0, Ordering::Relaxed);
    (s.log)(&format!("tablet: tablet {id} connected"));
    let (mut owner, mut state) = (s.owner.subscribe(), s.state.subscribe());
    let first = state.borrow_and_update().clone();
    if sock.send(Message::Text(first.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            m = sock.recv() => match m {
                Some(Ok(Message::Text(t))) => match parse(t.as_str()) {
                    Some(Op::Key { action, down }) => s.key(action, down),
                    Some(op) => s.board_op(op),
                    None => (s.log)("tablet: a message not understood - ignored"),
                },
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            r = state.changed() => {
                let j = state.borrow_and_update().clone();
                if r.is_err() || sock.send(Message::Text(j.into())).await.is_err() {
                    break;
                }
            },
            _ = owner.changed() => {
                let now = *owner.borrow_and_update();
                if now != id {
                    let why = if now == u64::MAX { "stopped" } else { "other" };
                    let _ = sock.send(Message::Text(json!({"t": "bye", "why": why}).to_string().into())).await;
                    (s.log)(&format!("tablet: tablet {id} let go ({why})"));
                    return;
                }
            },
        }
    }
    if *s.owner.borrow() == id {
        s.keys.store(0, Ordering::Relaxed);
    }
    (s.log)(&format!("tablet: tablet {id} gone"));
}

/// Serves the board on `listener` until `stop` ends: peers on `net` (address, prefix length) with `token` only.
pub async fn serve(
    listener: tokio::net::TcpListener,
    token: String,
    net: (Ipv4Addr, u8),
    board: Board,
    stop: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let mut file = TableFile::in_dir(&board.dir);
    let placed = file.read().unwrap_or_default();
    let mask = u32::MAX.checked_shl(32 - u32::from(net.1.min(32))).unwrap_or(0);
    let s = Arc::new(Shared {
        token,
        net: (u32::from(net.0), mask),
        state: watch::channel(state_json(0, &placed, &board.catalogue)).0,
        table: Mutex::new(Table { file, placed, rev: 0 }),
        catalogue: board.catalogue,
        pictures: board.pictures,
        keys: board.keys,
        presses: Mutex::new([(0, None); 16]),
        owner: watch::channel(0).0,
        next_id: AtomicU64::new(1),
        log: board.log,
    });
    let w = s.clone();
    let watcher = tokio::spawn(async move {
        loop {
            tokio::time::sleep(WATCH).await;
            let mut t = lock(&w.table);
            if w.reread(&mut t) {
                w.push(&t);
            }
        }
    });
    let app = Router::new()
        .route("/", get(page))
        .route("/ws", get(ws))
        .route("/c/{no}", get(picture))
        .layer(middleware::from_fn_with_state(s.clone(), guard))
        .with_state(s.clone());
    let o = s.clone();
    let r = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            stop.await;
            // every tablet told and let go, so the shutdown does not wait for them
            o.owner.send_replace(u64::MAX);
        })
        .await;
    watcher.abort();
    s.keys.store(0, Ordering::Relaxed);
    r
}

/// A new token: 128 random bits in hex.
pub fn token() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the system's random numbers");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The run's tablet board: a thread that reads data\panel.txt each second and serves the board while it says
/// `tablet=on`, writing its address for the panel's QR to data\tablet.url (removed when it stops). Dropping the
/// handle stops it.
pub fn spawn(kit: &Kit, seat: PathBuf, keys: Arc<AtomicU16>, log: Log) -> Handle {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let (data, cache) = (kit.data(), kit.cache());
    let thread = std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return log("tablet: no runtime - no tablet board");
        };
        let url_file = data.join(URL_FILE);
        let panel = data.join("panel.txt");
        rt.block_on(async {
            // the server's stop and its task; `tried`: started once for this tablet=on (a failure is not retried)
            let mut server: Option<(tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<()>)> = None;
            let mut tried = false;
            let mut next = Instant::now();
            while !flag.load(Ordering::Relaxed) {
                if Instant::now() >= next {
                    next = Instant::now() + Duration::from_secs(1);
                    let on = PanelFile::parse(&std::fs::read(&panel).unwrap_or_default()).tablet;
                    if on && !tried {
                        tried = true;
                        let board = Board {
                            dir: seat.clone(),
                            catalogue: cards::load_catalogue(&cache.join("catalogue.tsv"))
                                .unwrap_or_default()
                                .into(),
                            pictures: cache.join("cards"),
                            keys: keys.clone(),
                            log: log.clone(),
                        };
                        match start(board).await {
                            Ok((url, s)) => {
                                log(&format!("tablet: serving {}", url.split("?t=").next().unwrap_or("")));
                                let _ = std::fs::write(&url_file, url);
                                server = Some(s);
                            }
                            Err(why) => {
                                log(&format!("tablet: not started ({why})"));
                                let _ = std::fs::write(&url_file, format!("!{why}"));
                            }
                        }
                    } else if !on && tried {
                        tried = false;
                        stop_server(server.take()).await;
                        let _ = std::fs::remove_file(&url_file);
                        log("tablet: stopped (turned off)");
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            stop_server(server.take()).await;
            let _ = std::fs::remove_file(&url_file);
        });
    });
    Handle::new(stop, thread)
}

type Running = (tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<()>);

async fn stop_server(s: Option<Running>) {
    if let Some((tx, task)) = s {
        let _ = tx.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
    }
}

/// The listener on this PC's private address, the server started: the QR's address and the server's stop.
async fn start(board: Board) -> Result<(String, Running), String> {
    let (ip, prefix) = forza::online::lan::private_net().ok_or("no home network")?;
    let listener = match tokio::net::TcpListener::bind((ip, PORT)).await {
        Ok(l) => l,
        Err(_) => tokio::net::TcpListener::bind((ip, 0))
            .await
            .map_err(|e| format!("cannot listen on {ip}: {e}"))?,
    };
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = token();
    let url = format!("http://{ip}:{port}/?t={token}");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let log = board.log.clone();
    let task = tokio::spawn(async move {
        if let Err(e) = serve(listener, token, (ip, prefix), board, async move {
            let _ = rx.await;
        })
        .await
        {
            log(&format!("tablet: server ended ({e})"));
        }
    });
    Ok((url, (tx, task)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_checked() {
        assert_eq!(
            parse(r#"{"t":"swap","from":3,"to":12}"#),
            Some(Op::Swap { from: 3, to: 12 })
        );
        assert_eq!(
            parse(r#"{"t":"move","from":1,"x":5,"y":-0.5}"#),
            Some(Op::Move {
                from: 1,
                x: 1.0,
                y: -0.5
            })
        );
        let shoot = forza::keys::action_index("SHOOT").unwrap();
        assert_eq!(
            parse(r#"{"t":"key","a":"SHOOT","down":true}"#),
            Some(Op::Key {
                action: shoot,
                down: true
            })
        );
        for bad in [
            r#"{"t":"swap","from":16,"to":1}"#,
            r#"{"t":"swap","from":-1,"to":1}"#,
            r#"{"t":"move","from":1,"x":"a","y":0}"#,
            r#"{"t":"off"}"#,
            r#"{"t":"key","a":"CARD","down":true}"#,
            r#"{"t":"key","a":"TEST","down":true}"#,
            r#"{"t":"nope"}"#,
            "not json",
            "[]",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert!(same(b"abc", b"abc") && !same(b"abc", b"abd") && !same(b"abc", b"ab"));
        assert_eq!(query_token(Some("x=1&t=ab")), "ab");
        assert_eq!(query_token(None), "");
        assert_eq!(token().len(), 32);
    }
}
