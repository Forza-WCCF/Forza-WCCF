//! The shop tunnel (M2 plan 5.2): a seat on another PC reaches a 2017-18 shop's control, seat desk and match relay
//! through one iroh connection to the shop's PC, so the shop needs no inbound port (a hole punch, else the relay).
//!
//! - Join side: listens on its own loopback where the Launcher and the game look for a shop (TCP: the control and
//!   desk ports; UDP: the relay port). Each TCP connection becomes a QUIC bi-stream whose first byte is its kind
//!   ([`KIND_CONTROL`], [`KIND_DESK`]). Each local UDP socket that sends to the relay port is a flow: its datagrams
//!   cross as `[flow u16 LE][datagram]`, and the answers go back to that socket from the relay port.
//! - Host side: dials only its own loopback: the kind's port for a stream; for datagrams, the relay port, from one
//!   socket per (guest, flow), so the relay tells the guest's game socket and ping socket apart.
//!
//! The panel relay already routes every in-shop match packet through the relay port (`crates/panel/src/net.rs`), so
//! the seats' peer link needs nothing more. Each direction of a stream is buffered on this side up to
//! [`STREAM_BUF`], so a slow path never blocks the game's own `send`; past that the stream is closed and logged.
//! The test-only delay ([`ShopConfig::delay`], the Launcher's `VSLINK_DELAY_MS` / 2) holds everything this side
//! sends and receives that much longer.

use crate::core::EndpointId;
use crate::net::{RELAY_URL, id_short, load_key, relay_from, watch_path};
use bytes::Bytes;
use forza::service::{Handle, Log};
use iroh::endpoint::{Connection, IdleTimeout, QuicTransportConfig, presets};
use iroh::tls::CaTlsConfig;
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMap, RelayMode, RelayUrl};
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
use tokio::time::{Instant, interval, sleep, sleep_until, timeout};

pub const SHOP_ALPN: &[u8] = b"forza-wccf/shop/1";
/// The first byte of a stream: what the host dials for it.
pub const KIND_CONTROL: u8 = 1;
pub const KIND_DESK: u8 = 2;
/// Bytes one direction of a stream may hold on this side before it is closed.
pub const STREAM_BUF: usize = 4 << 20;
/// Guests a host serves at once (8 seats), and flows per guest (a game socket and a ping socket each; restarts add).
const MAX_GUESTS: usize = 8;
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

/// What [`spawn`] needs.
pub struct ShopConfig {
    pub role: Role,
    pub ports: Ports,
    /// The relay (the first URL is the one dialled and the one the host is reached through)
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
}

impl ShopConfig {
    /// What the Launcher runs: the public relay, direct paths allowed, no delay.
    pub fn public(role: Role, ports: Ports, key: PathBuf, log: Log) -> Self {
        let url: RelayUrl = RELAY_URL.parse().expect("RELAY_URL is a URL");
        ShopConfig {
            role,
            ports,
            relay: RelayMap::from(url),
            ca: None,
            relay_only: false,
            key,
            log,
            delay: Duration::ZERO,
        }
    }

    /// Dials `url` instead of the public relay; `ca_pem` (a PEM file) is then its only trusted root.
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

/// A datagram across: `[flow u16 LE][payload]`.
pub fn encode_flow(flow: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 2);
    v.extend_from_slice(&flow.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

pub fn decode_flow(d: &[u8]) -> Option<(u16, &[u8])> {
    let (f, rest) = d.split_first_chunk::<2>()?;
    Some((u16::from_le_bytes(*f), rest))
}

/// The join side's flows: each local socket that sends to the relay port gets a number; when all [`MAX_FLOWS`] are
/// taken, the least recently used one is given to the newcomer.
#[derive(Default)]
pub struct Flows {
    /// index = flow; (local socket, last use)
    slots: Vec<(SocketAddr, u64)>,
    tick: u64,
}

impl Flows {
    pub fn flow_of(&mut self, src: SocketAddr) -> u16 {
        self.tick += 1;
        let i = match self.slots.iter().position(|(a, _)| *a == src) {
            Some(i) => i,
            None if self.slots.len() < MAX_FLOWS => {
                self.slots.push((src, 0));
                self.slots.len() - 1
            }
            None => {
                let (i, _) = self
                    .slots
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, (_, t))| *t)
                    .expect("MAX_FLOWS > 0");
                self.slots[i].0 = src;
                i
            }
        };
        self.slots[i].1 = self.tick;
        i as u16
    }

    pub fn addr_of(&self, flow: u16) -> Option<SocketAddr> {
        self.slots.get(usize::from(flow)).map(|(a, _)| *a)
    }
}

/// A running shop tunnel. Dropping `handle` stops it within 100 ms.
pub struct ShopLink {
    pub handle: Handle,
    /// This endpoint's id (the host's is what guests type)
    pub id: EndpointId,
}

struct Sh {
    log: Log,
    ep: Endpoint,
    relay: RelayUrl,
    ports: Ports,
    delay: Duration,
    guests: AtomicUsize,
    oversize: AtomicU64,
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
/// Returns the bytes copied.
async fn pipe<R, W>(mut r: R, mut w: W, delay: Duration, what: String, log: Log) -> u64
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let held = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    let h = held.clone();
    let writer = tokio::spawn(async move {
        let mut n = 0u64;
        while let Some((at, chunk)) = rx.recv().await {
            sleep_until(at + delay).await;
            if w.write_all(&chunk).await.is_err() {
                return n;
            }
            n += chunk.len() as u64;
            h.fetch_sub(chunk.len(), Ordering::Relaxed);
        }
        let _ = w.shutdown().await;
        n
    });
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if held.fetch_add(n, Ordering::Relaxed) + n > STREAM_BUF {
            log(&format!(
                "{what}: more than {} MiB waiting to cross, closed",
                STREAM_BUF >> 20
            ));
            writer.abort();
            return 0;
        }
        if tx.send((Instant::now(), buf[..n].to_vec())).is_err() {
            break;
        }
    }
    drop(tx);
    writer.await.unwrap_or(0)
}

/// Both directions between a local TCP connection and a QUIC stream; logs the totals when both are done.
async fn splice<QR, QW>(tcp: TcpStream, qr: QR, qw: QW, sh: Arc<Sh>, what: String)
where
    QR: AsyncRead + Unpin + Send + 'static,
    QW: AsyncWrite + Unpin + Send + 'static,
{
    let (tr, tw) = tcp.into_split();
    let up = tokio::spawn(pipe(tr, qw, sh.delay, what.clone(), sh.log.clone()));
    let down = tokio::spawn(pipe(qr, tw, sh.delay, what.clone(), sh.log.clone()));
    let (up, down) = (up.await.unwrap_or(0), down.await.unwrap_or(0));
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
        if self.guests.fetch_add(1, Ordering::Relaxed) >= MAX_GUESTS {
            self.guests.fetch_sub(1, Ordering::Relaxed);
            (self.log)(&format!("guest {who} refused: {MAX_GUESTS} guests already"));
            conn.close(1u8.into(), b"full");
            return;
        }
        (self.log)(&format!("guest {who} joined"));
        tokio::spawn(watch_path(self.log.clone(), conn.clone()));
        let streams = tokio::spawn(self.clone().guest_streams(conn.clone(), who.clone()));
        let dgrams = tokio::spawn(self.clone().guest_datagrams(conn.clone(), who.clone()));
        let why = conn.closed().await;
        streams.abort();
        dgrams.abort();
        self.guests.fetch_sub(1, Ordering::Relaxed);
        (self.log)(&format!("guest {who} left: {why}"));
    }

    async fn guest_streams(self: Arc<Self>, conn: Connection, who: String) {
        while let Ok((qw, mut qr)) = conn.accept_bi().await {
            let me = self.clone();
            let who = who.clone();
            tokio::spawn(async move {
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
                let what = format!("guest {who}: {} connection", kind_name(k[0]));
                match TcpStream::connect(loopback(port)).await {
                    Ok(tcp) => {
                        let _ = tcp.set_nodelay(true);
                        (me.log)(&format!("{what} to port {port}"));
                        splice(tcp, qr, qw, me.clone(), what).await;
                    }
                    Err(e) => (me.log)(&format!("{what}: port {port}: {e}")),
                }
            });
        }
    }

    /// The guest's flows: one socket each on this loopback, talking to the relay port.
    async fn guest_datagrams(self: Arc<Self>, conn: Connection, who: String) {
        let out = datagram_line(&self, conn.clone());
        let mut flows: HashMap<u16, mpsc::Sender<(Instant, Vec<u8>, SocketAddr)>> = HashMap::new();
        let relay = loopback(self.ports.relay);
        while let Ok(d) = conn.read_datagram().await {
            let Some((flow, payload)) = decode_flow(&d) else {
                continue;
            };
            if !flows.contains_key(&flow) {
                if flows.len() >= MAX_FLOWS {
                    continue;
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
                let (sock, tx) = (s.clone(), out.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    loop {
                        match sock.recv_from(&mut buf).await {
                            Ok((n, src)) if src == relay => {
                                let d = Bytes::from(encode_flow(flow, &buf[..n]));
                                if tx.try_send((Instant::now(), d)).is_err() && tx.is_closed() {
                                    return;
                                }
                            }
                            Ok(_) => {}
                            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
                            Err(_) => return,
                        }
                    }
                });
                flows.insert(flow, udp_line(s, self.delay));
            }
            let _ = flows[&flow].try_send((Instant::now(), payload.to_vec(), relay));
        }
    }
}

// ---- join ----

struct Join {
    sh: Arc<Sh>,
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
                            splice(tcp, qr, qw, me.sh.clone(), what).await;
                        }
                    }
                    Err(e) => (me.sh.log)(&format!("{what}: {e}")),
                }
            });
        }
    }
}

/// The join side as [`start`] leaves it: the first link up, the control and desk listeners bound.
type Joined = (Arc<Join>, Connection, [TcpListener; 2]);

async fn start(cfg: ShopConfig) -> io::Result<(Arc<Sh>, Option<Joined>)> {
    let relay = cfg
        .relay
        .urls::<Vec<_>>()
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no relay URL"))?;
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
        guests: AtomicUsize::new(0),
        oversize: AtomicU64::new(0),
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
            sh.relay,
            id_text(sh.ep.id().as_bytes())
        ));
        return Ok((sh, None));
    };
    // the local side first: a port in use fails the start before anything dials
    let p = cfg.ports;
    let control = TcpListener::bind(loopback(p.control)).await?;
    let desk = TcpListener::bind(loopback(p.desk)).await?;
    let udp = Arc::new(UdpSocket::bind(loopback(p.relay)).await?);
    let pk = PublicKey::from_bytes(&host).map_err(io::Error::other)?;
    let join = Arc::new(Join {
        host: EndpointAddr::new(pk).with_relay_url(sh.relay.clone()),
        sh: sh.clone(),
        link: watch::channel(None).0,
        udp,
        flows: Mutex::default(),
    });
    (sh.log)(&format!(
        "shop join {} to {} (relay {}{only}), local ports TCP {} and {}, UDP {}",
        sh.ep.id().fmt_short(),
        id_short(&host),
        sh.relay,
        p.control,
        p.desk,
        p.relay
    ));
    let first = join.dial().await.map_err(|e| {
        io::Error::other(format!("the shop {} did not answer: {e}", id_short(&host)))
    })?;
    (sh.log)("link to the shop up");
    Ok((sh, Some((join, first, [control, desk]))))
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
            let _ = tx.send(Ok(*sh.ep.id().as_bytes()));
            rt.block_on(async {
                match join {
                    None => {
                        tokio::spawn(sh.clone().host());
                    }
                    Some((j, first, [control, desk])) => {
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
    let id = rx
        .recv()
        .map_err(|_| io::Error::other("the shop tunnel's thread ended at start"))??;
    Ok(ShopLink {
        handle: Handle::new(stop, thread),
        id,
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
        let d = encode_flow(0x0102, b"ab");
        assert_eq!(d, [2, 1, b'a', b'b']);
        assert_eq!(decode_flow(&d), Some((0x0102, &b"ab"[..])));
        assert_eq!(decode_flow(&[7, 0]), Some((7, &[][..])));
        assert_eq!(decode_flow(&[7]), None);
    }

    #[test]
    fn flows_are_stable_and_reuse_the_oldest_when_full() {
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
        assert_eq!(f.flow_of(a(5000)), 1);
        assert_eq!(f.addr_of(1), Some(a(5000)));
        assert_eq!(f.addr_of(0), Some(a(1000)));
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
