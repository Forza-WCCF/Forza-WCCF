//! The shop tunnel (M2 plan 5.2): a seat on another PC reaches a 2017-18 shop's control, seat desk and match relay
//! through one iroh connection to the shop's PC, so the shop needs no inbound port (a hole punch, else the relay).
//!
//! - Join side: listens on its own loopback where the Launcher and the game look for a shop (TCP: the control and
//!   desk ports; UDP: the relay port). Each TCP connection becomes a QUIC bi-stream whose first byte is its kind
//!   ([`KIND_CONTROL`], [`KIND_DESK`]). Each local UDP socket that sends to the relay port is a flow: its datagrams
//!   cross as `[flow u32 LE][datagram]`, and the answers go back to that socket from the relay port.
//! - Host side: dials only its own loopback: the kind's port for a stream; for datagrams, the relay port, from one
//!   socket per (guest, flow), so the relay tells the guest's game socket and ping socket apart.
//!
//! The panel relay already routes every in-shop match packet through the relay port (`crates/panel/src/net.rs`), so
//! the seats' peer link needs nothing more. The streams are buffered on this side (all of a guest's on the host, all
//! of the join side's) up to [`STREAM_BUF`] together, so a slow path never blocks the game's own `send`; past that the
//! stream that overflows is closed and logged. A host takes [`MAX_STREAMS`] streams per guest, and closes a guest
//! that opens nothing usable within [`GUEST_GRACE`].
//! Join side ports: the shop's own where free, else any free one ([`ShopLink::ports`] says which).
//! The test-only delay ([`ShopConfig::delay`], the Launcher's `VSLINK_DELAY_MS` / 2) holds everything this side
//! sends and receives that much longer.

use crate::core::EndpointId;
use crate::net::{
    id_short, load_key, public_relays, relay_addr, relay_from, relay_names, watch_path,
};
use bytes::Bytes;
use forza::service::{Handle, Log};
use iroh::endpoint::{Connection, IdleTimeout, QuicTransportConfig, presets};
use iroh::tls::CaTlsConfig;
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMap, RelayMode};
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, interval, sleep, sleep_until, timeout};

pub const SHOP_ALPN: &[u8] = b"forza-wccf/shop/1";
/// The first byte of a stream: what the host dials for it.
pub const KIND_CONTROL: u8 = 1;
pub const KIND_DESK: u8 = 2;
/// The host's close code for a guest its room does not name: the join side stops dialling and says so.
pub const CLOSE_NOT_IN_ROOM: u8 = 4;
/// The host's close code for a guest it could not check now (its room's list did not come, too many at the door): the
/// join side dials again.
pub const CLOSE_NO_ANSWER: u8 = 5;
/// Bytes the streams may hold on this side together (per guest on the host) before the one that overflows is closed.
pub const STREAM_BUF: usize = 4 << 20;
/// Streams a guest may hold open at once (the control, the projector-less seat's reconnects, the desk).
pub const MAX_STREAMS: usize = 4;
/// A guest that opens no stream of a known kind and sends no datagram in this time is closed.
pub const GUEST_GRACE: Duration = Duration::from_secs(15);
/// Guests a host serves at once (8 seats), and flows per guest (a game socket and a ping socket each; restarts add).
const MAX_GUESTS: usize = 8;
/// Guests checked against the room at once; more at the door are told to come back without a check.
pub const MAX_ADMITTING: usize = MAX_GUESTS * 2;
const MAX_FLOWS: usize = 32;
/// Datagrams queued per direction before new ones are dropped.
const DGRAM_QUEUE: usize = 1024;
const KEEP_ALIVE: Duration = Duration::from_secs(5);
/// Above the game's 30 s silence error (2001).
const IDLE: Duration = Duration::from_secs(60);
/// How long a local connection waits for the link to the shop, and the join's first dial.
const LINK_WAIT: Duration = Duration::from_secs(20);

/// The shop's loopback ports: where the host dials them, where the join side listens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ports {
    pub control: u16,
    pub desk: u16,
    pub relay: u16,
}

impl Ports {
    /// The TCP port a stream of `kind` is for; `None`: not a kind the host dials.
    pub fn of_kind(&self, kind: u8) -> Option<u16> {
        match kind {
            KIND_CONTROL => Some(self.control),
            KIND_DESK => Some(self.desk),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The shop's PC: guests dial this endpoint.
    Host,
    /// A seat elsewhere: dials the host's EndpointId.
    Join(EndpointId),
}

/// Host: whether a guest's EndpointId may come in (a shop room's `GUESTS`, `forza::online::lobby::shop_peers`). It
/// may block (it runs off the network threads, at most [`MAX_ADMITTING`] at once); None lets in anyone who knows the
/// host's id.
pub type Admit = Arc<dyn Fn(&EndpointId) -> Verdict + Send + Sync>;

/// What an [`Admit`] rule says of a guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    In,
    /// Definitely not in the room (or the room is gone): closed with [`CLOSE_NOT_IN_ROOM`], the guest stops dialling.
    NotInRoom,
    /// No answer to go by in time: closed with [`CLOSE_NO_ANSWER`], the guest dials again.
    NoAnswer,
}

/// What [`spawn`] needs.
pub struct ShopConfig {
    pub role: Role,
    pub ports: Ports,
    /// The relays; the host is dialled through all of them (`net::relay_addr`)
    pub relay: RelayMap,
    /// Trust for the relay's TLS certificate; None = the embedded web PKI roots
    pub ca: Option<CaTlsConfig>,
    /// No direct paths: everything through the relay (tests)
    pub relay_only: bool,
    /// The endpoint's secret key, made on first use (the host's id stays the same across runs)
    pub key: PathBuf,
    pub log: Log,
    /// Test only: held this long more in each direction on this side (`VSLINK_DELAY_MS` / 2); zero in play.
    pub delay: Duration,
    /// Host: who may come in ([`Admit`]).
    pub admit: Option<Admit>,
}

impl ShopConfig {
    /// What the Launcher runs: `net::public_relays`, direct paths allowed, no delay.
    pub fn public(role: Role, ports: Ports, key: PathBuf, log: Log) -> Self {
        ShopConfig {
            role,
            ports,
            relay: public_relays(),
            ca: None,
            relay_only: false,
            key,
            log,
            delay: Duration::ZERO,
            admit: None,
        }
    }

    /// Dials only `url` instead of `net::public_relays`; `ca_pem` (a PEM file) is then its only trusted root.
    pub fn set_relay(&mut self, url: &str, ca_pem: Option<&Path>) -> Result<(), String> {
        let (relay, ca) = relay_from(url, ca_pem)?;
        self.relay = relay;
        if ca.is_some() {
            self.ca = ca;
        }
        Ok(())
    }
}

/// A host's EndpointId as written (64 hex digits, as the host logs it); `None` for anything else.
pub fn parse_id(s: &str) -> Option<EndpointId> {
    let s = s.trim();
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut id = [0u8; 32];
    for (b, pair) in id.iter_mut().zip(s.as_bytes().chunks(2)) {
        *b = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    // a point on the curve, as iroh takes it
    PublicKey::from_bytes(&id).ok().map(|_| id)
}

/// The id as [`parse_id`] reads it.
pub fn id_text(id: &EndpointId) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

/// The id an endpoint with the key at `key` has (the key made on first use, as [`spawn`] does): a host registers it
/// in its shop room before it listens, a guest tells it before it dials.
pub fn own_id(key: &Path) -> io::Result<EndpointId> {
    Ok(*load_key(key)?.public().as_bytes())
}

/// A datagram across: `[flow u32 LE][payload]`. Flow numbers are never reused within a run, so a late datagram of a
/// flow that is gone finds no socket instead of the wrong one.
pub fn encode_flow(flow: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 4);
    v.extend_from_slice(&flow.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

pub fn decode_flow(d: &[u8]) -> Option<(u32, &[u8])> {
    let (f, rest) = d.split_first_chunk::<4>()?;
    Some((u32::from_le_bytes(*f), rest))
}

/// The join side's flows: each local socket that sends to the relay port gets the next number; past [`MAX_FLOWS`]
/// the least recently used one is forgotten (its number is never handed out again).
#[derive(Default)]
pub struct Flows {
    by_addr: HashMap<SocketAddr, (u32, u64)>,
    by_flow: HashMap<u32, SocketAddr>,
    next: u32,
    tick: u64,
}

impl Flows {
    pub fn flow_of(&mut self, src: SocketAddr) -> u32 {
        self.tick += 1;
        if let Some(e) = self.by_addr.get_mut(&src) {
            e.1 = self.tick;
            return e.0;
        }
        if self.by_addr.len() >= MAX_FLOWS
            && let Some((&old, &(f, _))) = self.by_addr.iter().min_by_key(|(_, (_, t))| *t)
        {
            self.by_addr.remove(&old);
            self.by_flow.remove(&f);
        }
        let f = self.next;
        self.next = self.next.wrapping_add(1);
        self.by_addr.insert(src, (f, self.tick));
        self.by_flow.insert(f, src);
        f
    }

    pub fn addr_of(&self, flow: u32) -> Option<SocketAddr> {
        self.by_flow.get(&flow).copied()
    }
}

/// Counts what is open while it lives (a flow's socket, a stream): one up at `new`, one down at drop.
struct Live(Arc<AtomicUsize>);

impl Live {
    fn new(n: &Arc<AtomicUsize>) -> Self {
        n.fetch_add(1, Ordering::Relaxed);
        Live(n.clone())
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A running shop tunnel. Dropping `handle` stops it: up to about 3 s (the endpoint's close, then the runtime's).
pub struct ShopLink {
    pub handle: Handle,
    /// This endpoint's id (the host's is what guests type)
    pub id: EndpointId,
    /// Join: the loopback ports it listens on (the shop's own unless taken). Host: the ports it dials.
    pub ports: Ports,
    flows: Arc<AtomicUsize>,
    streams: Arc<AtomicUsize>,
    guests: Arc<AtomicUsize>,
}

impl ShopLink {
    /// Host: the guests connected now (admitted, link up), live: a shop server with guests holds their matches.
    pub fn guests(&self) -> Arc<AtomicUsize> {
        self.guests.clone()
    }

    /// The host's guest flows (a socket each) and the streams open now: both 0 once every guest has left.
    pub fn open(&self) -> (usize, usize) {
        (
            self.flows.load(Ordering::Relaxed),
            self.streams.load(Ordering::Relaxed),
        )
    }
}

struct Sh {
    log: Log,
    ep: Endpoint,
    relay: RelayMap,
    ports: Ports,
    delay: Duration,
    guests: Arc<AtomicUsize>,
    /// Guests being checked against the room now ([`MAX_ADMITTING`]).
    admitting: AtomicUsize,
    oversize: AtomicU64,
    flows: Arc<AtomicUsize>,
    streams: Arc<AtomicUsize>,
    admit: Option<Admit>,
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

/// Sends datagrams on `conn` after the test delay, in order; full or too big: dropped (the big ones logged).
fn datagram_line(sh: &Arc<Sh>, conn: Connection) -> mpsc::Sender<(Instant, Bytes)> {
    let (tx, mut rx) = mpsc::channel::<(Instant, Bytes)>(DGRAM_QUEUE);
    let sh = sh.clone();
    tokio::spawn(async move {
        while let Some((at, d)) = rx.recv().await {
            sleep_until(at + sh.delay).await;
            let max = conn.max_datagram_size().unwrap_or(0);
            if d.len() > max {
                let n = sh.oversize.fetch_add(1, Ordering::Relaxed) + 1;
                if n.is_power_of_two() {
                    (sh.log)(&format!(
                        "dropped a {}-byte datagram over the {max}-byte limit ({n} so far)",
                        d.len()
                    ));
                }
                continue;
            }
            if conn.send_datagram(d).is_err() {
                return;
            }
        }
    });
    tx
}

/// Sends datagrams from `sock` after the test delay, in order; full: dropped.
fn udp_line(sock: Arc<UdpSocket>, delay: Duration) -> mpsc::Sender<(Instant, Vec<u8>, SocketAddr)> {
    let (tx, mut rx) = mpsc::channel::<(Instant, Vec<u8>, SocketAddr)>(DGRAM_QUEUE);
    tokio::spawn(async move {
        while let Some((at, d, to)) = rx.recv().await {
            sleep_until(at + delay).await;
            let _ = sock.send_to(&d, to).await;
        }
    });
    tx
}

/// Copies `r` to `w`, holding each chunk the test delay and at most [`STREAM_BUF`] bytes; then shuts `w` down.
/// Returns the bytes copied. Nothing is spawned: dropping the future ends both halves.
async fn pipe<R, W>(
    mut r: R,
    mut w: W,
    delay: Duration,
    budget: Arc<AtomicUsize>,
    what: String,
    log: Log,
) -> u64
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let held = Held {
        budget,
        mine: AtomicUsize::new(0),
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    // true: over the limit
    let reader = async {
        let tx = tx;
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = match r.read(&mut buf).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => n,
            };
            if held.add(n) > STREAM_BUF {
                return true;
            }
            if tx.send((Instant::now(), buf[..n].to_vec())).is_err() {
                return false;
            }
        }
    };
    let writer = async {
        let mut n = 0u64;
        while let Some((at, chunk)) = rx.recv().await {
            sleep_until(at + delay).await;
            if w.write_all(&chunk).await.is_err() {
                return n;
            }
            n += chunk.len() as u64;
            held.sub(chunk.len());
        }
        let _ = w.shutdown().await;
        n
    };
    tokio::pin!(reader, writer);
    let mut reading = true;
    loop {
        tokio::select! {
            over = &mut reader, if reading => {
                if over {
                    log(&format!("{what}: more than {} MiB waiting to cross, closed", STREAM_BUF >> 20));
                    return 0;
                }
                reading = false; // the sender is gone: the writer drains, then shuts down
            }
            n = &mut writer => return n,
        }
    }
}

/// Bytes a pipe holds, counted in the shared budget too; what is still held when the pipe ends leaves the budget.
struct Held {
    budget: Arc<AtomicUsize>,
    mine: AtomicUsize,
}

impl Held {
    /// The budget's total after `n` more.
    fn add(&self, n: usize) -> usize {
        self.mine.fetch_add(n, Ordering::Relaxed);
        self.budget.fetch_add(n, Ordering::Relaxed) + n
    }

    fn sub(&self, n: usize) {
        self.mine.fetch_sub(n, Ordering::Relaxed);
        self.budget.fetch_sub(n, Ordering::Relaxed);
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.budget
            .fetch_sub(*self.mine.get_mut(), Ordering::Relaxed);
    }
}

/// Both directions between a local TCP connection and a QUIC stream; logs the totals when both are done.
async fn splice<QR, QW>(
    tcp: TcpStream,
    qr: QR,
    qw: QW,
    sh: Arc<Sh>,
    budget: Arc<AtomicUsize>,
    what: String,
) where
    QR: AsyncRead + Unpin,
    QW: AsyncWrite + Unpin,
{
    let _live = Live::new(&sh.streams);
    let (tr, tw) = tcp.into_split();
    let (up, down) = tokio::join!(
        pipe(
            tr,
            qw,
            sh.delay,
            budget.clone(),
            what.clone(),
            sh.log.clone()
        ),
        pipe(qr, tw, sh.delay, budget, what.clone(), sh.log.clone())
    );
    (sh.log)(&format!("{what} closed ({up} bytes out, {down} in)"));
}

fn kind_name(kind: u8) -> &'static str {
    match kind {
        KIND_CONTROL => "control",
        KIND_DESK => "desk",
        _ => "?",
    }
}

// ---- host ----

impl Sh {
    async fn host(self: Arc<Self>) {
        while let Some(inc) = self.ep.accept().await {
            let me = self.clone();
            tokio::spawn(async move {
                match inc.await {
                    Ok(c) => me.serve_guest(c).await,
                    Err(e) => (me.log)(&format!("incoming guest: {e}")),
                }
            });
        }
    }

    async fn serve_guest(self: Arc<Self>, conn: Connection) {
        let who = conn.remote_id().fmt_short().to_string();
        // the room first, so a stranger never holds one of the guests' places while it is checked
        if let Some(admit) = self.admit.clone() {
            if self.admitting.fetch_add(1, Ordering::Relaxed) >= MAX_ADMITTING {
                self.admitting.fetch_sub(1, Ordering::Relaxed);
                (self.log)(&format!(
                    "guest {who}: {MAX_ADMITTING} at the door already, told to come back"
                ));
                conn.close(CLOSE_NO_ANSWER.into(), b"busy");
                return;
            }
            let id = *conn.remote_id().as_bytes();
            let v = tokio::task::spawn_blocking(move || admit(&id))
                .await
                .unwrap_or(Verdict::NoAnswer);
            self.admitting.fetch_sub(1, Ordering::Relaxed);
            match v {
                Verdict::In => {}
                Verdict::NotInRoom => {
                    (self.log)(&format!("guest {who} refused: not in the shop's room"));
                    conn.close(CLOSE_NOT_IN_ROOM.into(), b"not in the room");
                    return;
                }
                Verdict::NoAnswer => {
                    (self.log)(&format!(
                        "guest {who}: the room's list did not come in time, told to come back"
                    ));
                    conn.close(CLOSE_NO_ANSWER.into(), b"no answer");
                    return;
                }
            }
        }
        if self.guests.fetch_add(1, Ordering::Relaxed) >= MAX_GUESTS {
            self.guests.fetch_sub(1, Ordering::Relaxed);
            (self.log)(&format!("guest {who} refused: {MAX_GUESTS} guests already"));
            conn.close(1u8.into(), b"full");
            return;
        }
        (self.log)(&format!("guest {who} joined"));
        tokio::spawn(watch_path(self.log.clone(), conn.clone()));
        let used = Arc::new(AtomicBool::new(false));
        let streams = tokio::spawn(self.clone().guest_streams(
            conn.clone(),
            who.clone(),
            used.clone(),
        ));
        let dgrams = tokio::spawn(self.clone().guest_datagrams(
            conn.clone(),
            who.clone(),
            used.clone(),
        ));
        let (c, log, w) = (conn.clone(), self.log.clone(), who.clone());
        let grace = tokio::spawn(async move {
            sleep(GUEST_GRACE).await;
            if !used.load(Ordering::Relaxed) {
                log(&format!(
                    "guest {w}: nothing usable in {} s, closed",
                    GUEST_GRACE.as_secs()
                ));
                c.close(3u8.into(), b"idle");
            }
        });
        let why = conn.closed().await;
        streams.abort();
        dgrams.abort();
        grace.abort();
        self.guests.fetch_sub(1, Ordering::Relaxed);
        (self.log)(&format!("guest {who} left: {why}"));
    }

    /// The guest's streams; aborted with the guest, which aborts every stream task (the set drops).
    async fn guest_streams(self: Arc<Self>, conn: Connection, who: String, used: Arc<AtomicBool>) {
        let mut tasks = JoinSet::new();
        let budget = Arc::new(AtomicUsize::new(0));
        while let Ok((qw, mut qr)) = conn.accept_bi().await {
            while tasks.try_join_next().is_some() {}
            if tasks.len() >= MAX_STREAMS {
                (self.log)(&format!(
                    "guest {who}: more than {MAX_STREAMS} streams, one refused"
                ));
                continue; // dropped: the guest sees the stream reset
            }
            let (me, who, used, budget) = (self.clone(), who.clone(), used.clone(), budget.clone());
            tasks.spawn(async move {
                let mut k = [0u8; 1];
                if timeout(LINK_WAIT, qr.read_exact(&mut k)).await.is_err() {
                    return;
                }
                let Some(port) = me.ports.of_kind(k[0]) else {
                    (me.log)(&format!(
                        "guest {who}: stream of unknown kind {} refused",
                        k[0]
                    ));
                    return;
                };
                used.store(true, Ordering::Relaxed);
                let what = format!("guest {who}: {} connection", kind_name(k[0]));
                match TcpStream::connect(loopback(port)).await {
                    Ok(tcp) => {
                        let _ = tcp.set_nodelay(true);
                        (me.log)(&format!("{what} to port {port}"));
                        splice(tcp, qr, qw, me.clone(), budget, what).await;
                    }
                    Err(e) => (me.log)(&format!("{what}: port {port}: {e}")),
                }
            });
        }
    }

    /// The guest's flows: one socket each on this loopback, talking to the relay port. Aborted with the guest, which
    /// ends every flow (the set drops); past [`MAX_FLOWS`] the least recently used flow is closed.
    async fn guest_datagrams(
        self: Arc<Self>,
        conn: Connection,
        who: String,
        used: Arc<AtomicBool>,
    ) {
        type Line = mpsc::Sender<(Instant, Vec<u8>, SocketAddr)>;
        let out = datagram_line(&self, conn.clone());
        let mut tasks = JoinSet::new();
        let mut flows: HashMap<u32, (Line, tokio::task::AbortHandle, u64)> = HashMap::new();
        let relay = loopback(self.ports.relay);
        let mut tick = 0u64;
        while let Ok(d) = conn.read_datagram().await {
            let Some((flow, payload)) = decode_flow(&d) else {
                continue;
            };
            used.store(true, Ordering::Relaxed);
            tick += 1;
            if !flows.contains_key(&flow) {
                // a flow whose socket failed is forgotten (its task returned its number)
                while let Some(done) = tasks.try_join_next() {
                    if let Ok(f) = done {
                        flows.remove(&f);
                    }
                }
                if flows.len() >= MAX_FLOWS
                    && let Some(&old) = flows.iter().min_by_key(|(_, (_, _, t))| *t).map(|(f, _)| f)
                    && let Some((_, task, _)) = flows.remove(&old)
                {
                    task.abort();
                }
                let Ok(s) = UdpSocket::bind(loopback(0)).await else {
                    continue;
                };
                let s = Arc::new(s);
                if let Ok(a) = s.local_addr() {
                    (self.log)(&format!(
                        "guest {who}: flow {flow} from {a} to the relay port {}",
                        relay.port()
                    ));
                }
                let (sock, tx, live) = (s.clone(), out.clone(), Live::new(&self.flows));
                let task = tasks.spawn(async move {
                    let _live = live;
                    let mut buf = vec![0u8; 65536];
                    loop {
                        match sock.recv_from(&mut buf).await {
                            Ok((n, src)) if src == relay => {
                                let d = Bytes::from(encode_flow(flow, &buf[..n]));
                                if tx.try_send((Instant::now(), d)).is_err() && tx.is_closed() {
                                    return flow;
                                }
                            }
                            Ok(_) => {}
                            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
                            Err(_) => return flow,
                        }
                    }
                });
                flows.insert(flow, (udp_line(s, self.delay), task, tick));
            }
            if let Some((line, _, used)) = flows.get_mut(&flow) {
                *used = tick;
                let _ = line.try_send((Instant::now(), payload.to_vec(), relay));
            }
        }
    }
}

// ---- join ----

struct Join {
    sh: Arc<Sh>,
    /// What all the join side's streams hold ([`STREAM_BUF`])
    budget: Arc<AtomicUsize>,
    host: EndpointAddr,
    link: watch::Sender<Option<Connection>>,
    udp: Arc<UdpSocket>,
    flows: Mutex<Flows>,
}

impl Join {
    fn flows(&self) -> std::sync::MutexGuard<'_, Flows> {
        self.flows.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn dial(&self) -> Result<Connection, String> {
        match timeout(LINK_WAIT, self.sh.ep.connect(self.host.clone(), SHOP_ALPN)).await {
            Ok(Ok(c)) => Ok(c),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err(format!("no answer in {} s", LINK_WAIT.as_secs())),
        }
    }

    /// Keeps the link to the host up: `first` is the one [`spawn`] made; a dropped link is dialled again.
    async fn keep(self: Arc<Self>, first: Connection) {
        let mut conn = first;
        loop {
            self.link.send_replace(Some(conn.clone()));
            tokio::spawn(watch_path(self.sh.log.clone(), conn.clone()));
            let inbound = tokio::spawn(self.clone().from_host(conn.clone()));
            let why = conn.closed().await;
            inbound.abort();
            self.link.send_replace(None);
            if let iroh::endpoint::ConnectionError::ApplicationClosed(c) = &why
                && c.error_code == CLOSE_NOT_IN_ROOM.into()
            {
                // dialling again would be refused again: the game finds no shop and says so
                (self.sh.log)(
                    "the shop refused this PC: not in its room (join with the room code, in ONLINE)",
                );
                return;
            }
            (self.sh.log)(&format!("link to the shop down: {why}; dialling again"));
            conn = loop {
                match self.dial().await {
                    Ok(c) => break c,
                    Err(e) => {
                        (self.sh.log)(&format!("shop: {e}"));
                        sleep(Duration::from_secs(2)).await;
                    }
                }
            };
            (self.sh.log)("link to the shop up again");
        }
    }

    /// The relay's answers, to the local socket of their flow, from the relay port.
    async fn from_host(self: Arc<Self>, conn: Connection) {
        let line = udp_line(self.udp.clone(), self.sh.delay);
        while let Ok(d) = conn.read_datagram().await {
            let Some((flow, payload)) = decode_flow(&d) else {
                continue;
            };
            let Some(to) = self.flows().addr_of(flow) else {
                continue;
            };
            let _ = line.try_send((Instant::now(), payload.to_vec(), to));
        }
    }

    /// Local sockets to the relay port: each is a flow to the host.
    async fn to_host(self: Arc<Self>) {
        let mut rx = self.link.subscribe();
        let mut line: Option<(usize, mpsc::Sender<(Instant, Bytes)>)> = None;
        let mut buf = vec![0u8; 65536];
        loop {
            let (n, src) = match self.udp.recv_from(&mut buf).await {
                Ok(x) => x,
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(e) => {
                    (self.sh.log)(&format!("relay port: {e}"));
                    return;
                }
            };
            let Some(conn) = rx.borrow_and_update().clone() else {
                continue; // no link: dropped, as a lost datagram
            };
            let id = conn.stable_id();
            if line.as_ref().is_none_or(|(i, _)| *i != id) {
                line = Some((id, datagram_line(&self.sh, conn)));
            }
            let flow = self.flows().flow_of(src);
            let d = Bytes::from(encode_flow(flow, &buf[..n]));
            if let Some((_, tx)) = &line {
                let _ = tx.try_send((Instant::now(), d));
            }
        }
    }

    /// Local TCP connections of `kind`, each a stream to the host.
    async fn accept(self: Arc<Self>, l: TcpListener, kind: u8) {
        loop {
            let Ok((tcp, _)) = l.accept().await else {
                continue;
            };
            let me = self.clone();
            tokio::spawn(async move {
                let _ = tcp.set_nodelay(true);
                let mut rx = me.link.subscribe();
                let conn = match timeout(LINK_WAIT, rx.wait_for(Option::is_some)).await {
                    Ok(Ok(c)) => c.clone().expect("waited for Some"),
                    _ => {
                        (me.sh.log)(&format!(
                            "{} connection dropped: no link to the shop",
                            kind_name(kind)
                        ));
                        return;
                    }
                };
                let what = format!("{} connection", kind_name(kind));
                match conn.open_bi().await {
                    Ok((mut qw, qr)) => {
                        if qw.write_all(&[kind]).await.is_ok() {
                            splice(tcp, qr, qw, me.sh.clone(), me.budget.clone(), what).await;
                        }
                    }
                    Err(e) => (me.sh.log)(&format!("{what}: {e}")),
                }
            });
        }
    }
}

/// The join side as [`start`] leaves it: the first link up, the control and desk listeners bound, the ports bound.
type Joined = (Arc<Join>, Connection, [TcpListener; 2], Ports);

async fn start(cfg: ShopConfig) -> io::Result<(Arc<Sh>, Option<Joined>)> {
    if cfg.relay.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no relay URL"));
    }
    let relay = cfg.relay.clone();
    let transport = QuicTransportConfig::builder()
        .keep_alive_interval(KEEP_ALIVE)
        .max_idle_timeout(Some(IdleTimeout::try_from(IDLE).expect("60 s fits")))
        .build();
    let mut b = Endpoint::builder(presets::Minimal)
        .secret_key(load_key(&cfg.key)?)
        .relay_mode(RelayMode::Custom(cfg.relay.clone()))
        .clear_address_lookup()
        .transport_config(transport);
    if cfg.role == Role::Host {
        b = b.alpns(vec![SHOP_ALPN.to_vec()]);
    }
    if let Some(ca) = cfg.ca {
        b = b.ca_tls_config(ca);
    }
    if cfg.relay_only {
        b = b.clear_ip_transports();
    }
    let ep = b.bind().await.map_err(io::Error::other)?;
    let sh = Arc::new(Sh {
        log: cfg.log,
        ep,
        relay,
        ports: cfg.ports,
        delay: cfg.delay,
        guests: Arc::default(),
        admitting: AtomicUsize::new(0),
        oversize: AtomicU64::new(0),
        flows: Arc::default(),
        streams: Arc::default(),
        admit: cfg.admit,
    });
    let only = if cfg.relay_only { ", relay only" } else { "" };
    if !sh.delay.is_zero() {
        (sh.log)(&format!(
            "test delay: {} ms more each way on this side (VSLINK_DELAY_MS)",
            sh.delay.as_millis()
        ));
    }
    let Role::Join(host) = cfg.role else {
        (sh.log)(&format!(
            "shop host {} (relay {}{only}): guests run Launcher.exe remote {}",
            sh.ep.id().fmt_short(),
            relay_names(&sh.relay),
            id_text(sh.ep.id().as_bytes())
        ));
        return Ok((sh, None));
    };
    // the local side first, before anything dials: the shop's own ports, else any free one (this PC runs a server of
    // its own); the Launcher points the game at what was bound
    let want = cfg.ports;
    let control = bind_tcp(want.control).await?;
    let desk = bind_tcp(want.desk).await?;
    let udp = match UdpSocket::bind(loopback(want.relay)).await {
        Ok(u) => u,
        Err(_) => UdpSocket::bind(loopback(0)).await?,
    };
    let p = Ports {
        control: control.local_addr()?.port(),
        desk: desk.local_addr()?.port(),
        relay: udp.local_addr()?.port(),
    };
    let udp = Arc::new(udp);
    let pk = PublicKey::from_bytes(&host).map_err(io::Error::other)?;
    let join = Arc::new(Join {
        host: relay_addr(pk, &sh.relay),
        sh: sh.clone(),
        budget: Arc::default(),
        link: watch::channel(None).0,
        udp,
        flows: Mutex::default(),
    });
    (sh.log)(&format!(
        "shop join {} to {} (relay {}{only}), local ports TCP {} and {}, UDP {}",
        sh.ep.id().fmt_short(),
        id_short(&host),
        relay_names(&sh.relay),
        p.control,
        p.desk,
        p.relay
    ));
    let first = join.dial().await.map_err(|e| {
        io::Error::other(format!("the shop {} did not answer: {e}", id_short(&host)))
    })?;
    (sh.log)("link to the shop up");
    Ok((sh, Some((join, first, [control, desk], p))))
}

/// A TCP listener on the loopback `port`, else on any free loopback port.
async fn bind_tcp(port: u16) -> io::Result<TcpListener> {
    match TcpListener::bind(loopback(port)).await {
        Ok(l) => Ok(l),
        Err(_) => TcpListener::bind(loopback(0)).await,
    }
}

/// Starts the tunnel on its own thread. Host: returns once the endpoint is bound. Join: once its local ports are
/// bound and the first link to the host is up (an error if the host does not answer within 20 s).
pub fn spawn(cfg: ShopConfig) -> io::Result<ShopLink> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("vslink-shop".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("vslink-shop-rt")
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let (sh, join) = match rt.block_on(start(cfg)) {
                Ok(x) => x,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let ports = join.as_ref().map_or(sh.ports, |j| j.3);
            let _ = tx.send(Ok((
                *sh.ep.id().as_bytes(),
                sh.flows.clone(),
                sh.streams.clone(),
                sh.guests.clone(),
                ports,
            )));
            rt.block_on(async {
                match join {
                    None => {
                        tokio::spawn(sh.clone().host());
                    }
                    Some((j, first, [control, desk], _)) => {
                        tokio::spawn(j.clone().keep(first));
                        tokio::spawn(j.clone().to_host());
                        tokio::spawn(j.clone().accept(control, KIND_CONTROL));
                        tokio::spawn(j.accept(desk, KIND_DESK));
                    }
                }
                let mut t = interval(Duration::from_millis(100));
                while !flag.load(Ordering::Relaxed) {
                    t.tick().await;
                }
                let _ = timeout(Duration::from_secs(2), sh.ep.close()).await;
            });
            rt.shutdown_timeout(Duration::from_secs(1));
        })?;
    let (id, flows, streams, guests, ports) = rx
        .recv()
        .map_err(|_| io::Error::other("the shop tunnel's thread ended at start"))??;
    Ok(ShopLink {
        handle: Handle::new(stop, thread),
        id,
        ports,
        flows,
        streams,
        guests,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_the_shop_ports_only() {
        let p = Ports {
            control: 20002,
            desk: 20030,
            relay: 20040,
        };
        assert_eq!(p.of_kind(KIND_CONTROL), Some(20002));
        assert_eq!(p.of_kind(KIND_DESK), Some(20030));
        for k in [0u8, 3, 255] {
            assert_eq!(p.of_kind(k), None, "kind {k}");
        }
    }

    #[test]
    fn flow_framing() {
        let d = encode_flow(0x0403_0201, b"ab");
        assert_eq!(d, [1, 2, 3, 4, b'a', b'b']);
        assert_eq!(decode_flow(&d), Some((0x0403_0201, &b"ab"[..])));
        assert_eq!(decode_flow(&[7, 0, 0, 0]), Some((7, &[][..])));
        assert_eq!(decode_flow(&[7, 0, 0]), None);
    }

    /// A socket keeps its number; past MAX_FLOWS the least recently used is forgotten, and no number comes back, so a
    /// late answer for a forgotten flow reaches no socket rather than the newcomer.
    #[test]
    fn flow_numbers_are_stable_and_never_reused() {
        let a = |p: u16| SocketAddr::from((Ipv4Addr::LOCALHOST, p));
        let mut f = Flows::default();
        assert_eq!(f.flow_of(a(1000)), 0);
        assert_eq!(f.flow_of(a(1001)), 1);
        assert_eq!(f.flow_of(a(1000)), 0);
        assert_eq!(f.addr_of(1), Some(a(1001)));
        assert_eq!(f.addr_of(2), None);
        for p in 2..MAX_FLOWS as u16 {
            f.flow_of(a(1000 + p));
        }
        f.flow_of(a(1000)); // 0 used last; 1 is now the oldest
        let n = f.flow_of(a(5000));
        assert_eq!(n, MAX_FLOWS as u32, "a new number, not the forgotten 1");
        assert_eq!(f.addr_of(1), None, "flow 1's late answers go nowhere");
        assert_eq!(f.addr_of(n), Some(a(5000)));
        assert_eq!(f.addr_of(0), Some(a(1000)));
        assert_eq!(
            f.flow_of(a(1001)),
            n + 1,
            "the forgotten socket comes back as a new flow"
        );
    }

    #[test]
    fn ids_are_64_hex_digits() {
        let key = iroh::SecretKey::generate();
        let id = *key.public().as_bytes();
        let text = id_text(&id);
        assert_eq!(text.len(), 64);
        assert_eq!(text, key.public().to_string());
        assert_eq!(parse_id(&format!(" {text}\n")), Some(id));
        assert_eq!(parse_id(&text.to_uppercase()), Some(id));
        for bad in [
            "",
            "127.0.0.1",
            &text[..63],
            &format!("{text}0"),
            &text.replace(&text[..1], "g"),
        ] {
            assert_eq!(parse_id(bad), None, "{bad:?}");
        }
    }
}
