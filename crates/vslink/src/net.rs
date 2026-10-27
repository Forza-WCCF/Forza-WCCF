//! The iroh side (docs/IROH_VS_PLAN.md 2.2-2.4): the reflect socket the game probes, one socket per opponent token,
//! the gmsvr client (hello, register, lookup) and one QUIC connection per PC pair carrying `[seat][game datagram]`.
//! Runs on its own thread with its own tokio runtime; [`spawn`] returns once the sockets are bound and the first
//! hello went out.

use crate::core::{self, Pair, SEAT_BASE, Seats, Via};
use bytes::Bytes;
use forza::online::gamesvr;
use forza::online::vs::{Reflect, hello_payload, reflect};
use forza::service::{Handle, Log};
use iroh::endpoint::{Connection, IdleTimeout, QuicTransportConfig, presets};
use iroh::tls::CaTlsConfig;
use iroh::{
    Endpoint, EndpointAddr, PublicKey, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::task::AbortHandle;
use tokio::time::{interval, sleep, timeout};

pub const ALPN: &[u8] = b"forza-wccf/vs/1";
const HELLO_EVERY: Duration = Duration::from_secs(60);
const LOOKUP_EVERY: Duration = Duration::from_millis(500);
/// Paired seats look up every this many ticks (2 s): the next match on the same token brings a new opponent.
const PAIRED_EVERY: u64 = 4;
const ASK_TIMEOUT: Duration = Duration::from_secs(5);
const KEEP_ALIVE: Duration = Duration::from_secs(5);
/// Above the game's 30 s silence error (2001).
const IDLE: Duration = Duration::from_secs(60);
/// Small, so a stalled path drops old frames instead of queueing them.
const SEND_BUF: usize = 16 * 1024;

/// What [`spawn`] needs.
pub struct Config {
    /// gmsvr's TCP address
    pub gmsvr: SocketAddr,
    /// The reflect socket, `127.0.0.1:R` (port 0 picks one: tests)
    pub reflect: SocketAddrV4,
    /// The relays; peers are dialled through all of them ([`relay_addr`])
    pub relay: RelayMap,
    /// Trust for the relay's TLS certificate; None = the embedded web PKI roots
    pub ca: Option<CaTlsConfig>,
    /// No direct paths: everything through the relay (tests, the probe)
    pub relay_only: bool,
    /// The endpoint's secret key, made on first use
    pub key: PathBuf,
    pub log: Log,
    /// The lobby VS room's ticket, sent in every hello so gmsvr pairs this PC only within its room; None = the open
    /// pool
    pub ticket: Option<String>,
}

/// The project's relay (webpki roots) and the reflect port gmsvr's `--vslink-port` defaults to.
pub const RELAY_URL: &str = "https://relay.wccf.it:8443";
pub const REFLECT_PORT: u16 = 20090;

/// The relay list the Launcher uses: the project's relay plus n0's public relays as the backup. iroh homes on the
/// one with the lowest latency (sticky: it moves only for one a third faster) and a `RelayMap` keeps no order, so
/// "ours first" cannot be expressed; peers are dialled through all of them instead ([`relay_addr`]).
pub fn public_relays() -> RelayMap {
    let map = iroh::defaults::prod::default_relay_map();
    map.extend(&RelayMap::from(
        RELAY_URL.parse::<RelayUrl>().expect("RELAY_URL is a URL"),
    ));
    map
}

/// A peer reachable through every relay in `relays`: it homes on any one of them, and iroh sends the first QUIC
/// packets down every known path.
pub(crate) fn relay_addr(pk: PublicKey, relays: &RelayMap) -> EndpointAddr {
    EndpointAddr::from_parts(
        pk,
        relays
            .urls::<Vec<RelayUrl>>()
            .into_iter()
            .map(TransportAddr::Relay),
    )
}

/// The relays for a log line.
pub(crate) fn relay_names(relays: &RelayMap) -> String {
    let urls: Vec<String> = relays
        .urls::<Vec<RelayUrl>>()
        .iter()
        .map(|u| u.to_string())
        .collect();
    urls.join(" ")
}

impl Config {
    /// What the Launcher runs: [`public_relays`], direct paths allowed, reflect on `127.0.0.1:20090`.
    pub fn public(gmsvr: SocketAddr, key: PathBuf, log: Log) -> Self {
        Config {
            gmsvr,
            reflect: SocketAddrV4::new(Ipv4Addr::LOCALHOST, REFLECT_PORT),
            relay: public_relays(),
            ca: None,
            relay_only: false,
            key,
            log,
            ticket: None,
        }
    }
}

impl Config {
    /// Dials only `url` instead of [`public_relays`]; `ca_pem` (a PEM file) is then the only trusted root. For a
    /// self-hosted relay.
    pub fn set_relay(&mut self, url: &str, ca_pem: Option<&Path>) -> Result<(), String> {
        let (relay, ca) = relay_from(url, ca_pem)?;
        self.relay = relay;
        if ca.is_some() {
            self.ca = ca;
        }
        Ok(())
    }
}

/// A relay URL and, with `ca_pem` (a PEM file), its only trusted roots.
pub(crate) fn relay_from(
    url: &str,
    ca_pem: Option<&Path>,
) -> Result<(RelayMap, Option<CaTlsConfig>), String> {
    let u: RelayUrl = url
        .trim()
        .parse()
        .map_err(|e| format!("relay {url}: {e}"))?;
    let Some(path) = ca_pem else {
        return Ok((RelayMap::from(u), None));
    };
    use rustls_pki_types::{CertificateDer, pem::PemObject};
    let roots = CertificateDer::pem_file_iter(path)
        .and_then(|it| it.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("relay CA {}: {e}", path.display()))?;
    if roots.is_empty() {
        return Err(format!("relay CA {}: no certificate", path.display()));
    }
    Ok((RelayMap::from(u), Some(CaTlsConfig::custom_roots(roots))))
}

/// A running vslink. Dropping `handle` stops it within 100 ms.
pub struct Link {
    pub handle: Handle,
    /// Where the reflect socket is bound
    pub reflect: SocketAddrV4,
    pub id: core::EndpointId,
}

/// The game's peer socket for seat `s`: `127.0.0.1:20000+s`.
fn game_addr(seat: u8) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, SEAT_BASE + u16::from(seat))
}

fn rand() -> u32 {
    getrandom::u32().unwrap_or(0x00C0_FFEE)
}

/// gmsvr answers: status u32 at +4, 1 = ok.
fn ok(b: &[u8]) -> bool {
    b.get(4..8) == Some(&1u32.to_le_bytes()[..])
}

fn token_bytes(t: SocketAddrV4) -> [u8; 6] {
    let mut b = [0u8; 6];
    b[..4].copy_from_slice(&t.ip().octets());
    b[4..].copy_from_slice(&t.port().to_le_bytes());
    b
}

/// A 905 answer: the opponent once paired and registered.
fn parse_lookup(b: &[u8]) -> Option<Pair> {
    if !ok(b) || b.len() < 48 {
        return None;
    }
    let id: core::EndpointId = b[16..48].try_into().ok()?;
    if id == [0; 32] {
        return None;
    }
    let ip = Ipv4Addr::new(b[8], b[9], b[10], b[11]);
    let pair = Pair {
        token: SocketAddrV4::new(ip, u16::from_le_bytes([b[12], b[13]])),
        id,
    };
    pair.remote_seat().map(|_| pair)
}

/// One TCP connection to gmsvr, opened on demand and dropped on any error.
struct Gmsvr {
    addr: SocketAddr,
    conn: tokio::sync::Mutex<Option<TcpStream>>,
    seq: AtomicU16,
}

impl Gmsvr {
    async fn ask(&self, cmd: u16, payload: &[u8]) -> io::Result<Vec<u8>> {
        let mut conn = self.conn.lock().await;
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let r = timeout(
            ASK_TIMEOUT,
            Self::exchange(&mut conn, self.addr, cmd, seq, payload),
        )
        .await
        .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()));
        if r.is_err() {
            *conn = None;
        }
        r
    }

    async fn exchange(
        conn: &mut Option<TcpStream>,
        addr: SocketAddr,
        cmd: u16,
        seq: u16,
        payload: &[u8],
    ) -> io::Result<Vec<u8>> {
        if conn.is_none() {
            *conn = Some(TcpStream::connect(addr).await?);
        }
        let c = conn.as_mut().expect("just set");
        let mut frame = gamesvr::header(cmd, seq, 0, payload.len(), 0).to_vec();
        frame.extend_from_slice(payload);
        c.write_all(&frame).await?;
        let mut h = [0u8; 40];
        c.read_exact(&mut h).await?;
        let n = u32::from_le_bytes(h[16..20].try_into().expect("4 bytes")) as usize;
        if u16::from_le_bytes([h[8], h[9]]) != cmd + 1 || !(40..=gamesvr::MAX_FRAME).contains(&n) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bad answer to {cmd}"),
            ));
        }
        let mut body = vec![0u8; n - 40];
        c.read_exact(&mut body).await?;
        Ok(body)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Reg {
    Pending,
    Done,
}

struct Peer {
    sock: Arc<UdpSocket>,
    task: AbortHandle,
}

#[derive(Default)]
struct State {
    seats: Seats,
    reg: HashMap<u8, Reg>,
    /// The socket bound on each seat's opponent token
    peers: HashMap<u8, Peer>,
    conns: HashMap<core::EndpointId, Connection>,
    dialing: HashSet<core::EndpointId>,
}

struct Shared {
    log: Log,
    ep: Endpoint,
    id: core::EndpointId,
    relay: RelayMap,
    reflect: Arc<UdpSocket>,
    gm: Gmsvr,
    st: Mutex<State>,
    oversize: AtomicU64,
    ticket: Option<String>,
}

impl Shared {
    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn hello(&self) {
        let p = hello_payload(&self.id, self.ticket.as_deref());
        match self.gm.ask(900, &p).await {
            Ok(b) if ok(&b) => {}
            Ok(_) => (self.log)("gmsvr refused the hello (no VS there?)"),
            Err(e) => (self.log)(&format!("gmsvr hello: {e}")),
        }
    }

    /// One datagram on the reflect socket.
    async fn on_reflect(self: &Arc<Self>, pkt: &[u8], src: SocketAddr) {
        let SocketAddr::V4(src) = src else { return };
        let Some(seat) = core::seat_of(src.port()).filter(|_| *src.ip() == Ipv4Addr::LOCALHOST)
        else {
            return;
        };
        if !matches!(reflect(pkt, src), Reflect::Echo(_)) {
            // relay mode: route by seat, the header's destination is ignored
            self.forward(seat, pkt);
            return;
        }
        let echo = {
            let mut st = self.st();
            match st.reg.get(&seat) {
                Some(Reg::Done) => st.seats.probe_echo(pkt, src, rand),
                Some(Reg::Pending) => None,
                None => {
                    st.reg.insert(seat, Reg::Pending);
                    let token = st.seats.token_or_mint(seat, rand);
                    tokio::spawn(self.clone().register(seat, token, pkt.to_vec(), src));
                    None
                }
            }
        };
        if let Some((_, e)) = echo {
            let _ = self.reflect.send_to(&e, src).await;
        }
    }

    /// Registers the seat's first token (a new one on a collision), then answers the probe that asked.
    async fn register(
        self: Arc<Self>,
        seat: u8,
        mut token: SocketAddrV4,
        probe: Vec<u8>,
        src: SocketAddrV4,
    ) {
        loop {
            let mut p = [0u8; 40];
            p[..6].copy_from_slice(&token_bytes(token));
            p[8..].copy_from_slice(&self.id);
            match self.gm.ask(902, &p).await {
                Ok(b) if ok(&b) => break,
                Ok(_) => {
                    (self.log)(&format!(
                        "seat {seat}: token {token} taken, drawing another"
                    ));
                    token = self.st().seats.remint(seat, rand);
                }
                Err(e) => {
                    (self.log)(&format!("seat {seat}: register at gmsvr: {e}"));
                    self.st().reg.remove(&seat); // the next probe tries again
                    return;
                }
            }
        }
        (self.log)(&format!("seat {seat}: token {token}"));
        let echo = {
            let mut st = self.st();
            st.reg.insert(seat, Reg::Done);
            st.seats.probe_echo(&probe, src, rand)
        };
        if let Some((_, e)) = echo {
            let _ = self.reflect.send_to(&e, src).await;
        }
    }

    /// A game datagram from seat `seat` to its opponent.
    fn forward(&self, seat: u8, game: &[u8]) {
        let (conn, remote_seat) = {
            let st = self.st();
            let Some(p) = st.seats.pair(seat) else { return };
            let (Some(c), Some(rs)) = (st.conns.get(&p.id), p.remote_seat()) else {
                return;
            };
            (c.clone(), rs)
        };
        let d = core::encode(remote_seat, game);
        let max = conn.max_datagram_size().unwrap_or(0);
        if d.len() > max {
            let n = self.oversize.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() {
                (self.log)(&format!(
                    "seat {seat}: dropped a {}-byte datagram over the {max}-byte limit ({n} so far)",
                    game.len()
                ));
            }
            return;
        }
        let _ = conn.send_datagram(Bytes::from(d));
    }

    /// The seat's opponent from a lookup: bind its token locally (replacing an earlier opponent's).
    async fn pair(self: &Arc<Self>, seat: u8, pair: Pair) {
        let sock = match UdpSocket::bind(pair.token).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                (self.log)(&format!("seat {seat}: bind {}: {e}", pair.token));
                return;
            }
        };
        let task = tokio::spawn(self.clone().pump_peer(seat, sock.clone())).abort_handle();
        let short = id_short(&pair.id);
        let old = {
            let mut st = self.st();
            st.seats.set_pair(seat, pair);
            st.peers.insert(seat, Peer { sock, task })
        };
        if let Some(o) = old {
            o.task.abort();
        }
        (self.log)(&format!(
            "seat {seat}: paired with {} on {short}",
            pair.token
        ));
    }

    /// What the game sends to the opponent's token.
    async fn pump_peer(self: Arc<Self>, seat: u8, sock: Arc<UdpSocket>) {
        let from = SocketAddr::V4(game_addr(seat));
        let mut buf = vec![0u8; 65536];
        loop {
            match sock.recv_from(&mut buf).await {
                Ok((n, src)) if src == from => self.forward(seat, &buf[..n]),
                Ok(_) => {}
                // Windows: an earlier send hit a closed port
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
                Err(e) => {
                    (self.log)(&format!("seat {seat}: peer socket: {e}"));
                    return;
                }
            }
        }
    }

    /// Takes a connection (dialled or accepted) as the one for its PC.
    fn adopt(self: &Arc<Self>, conn: Connection) {
        let id = *conn.remote_id().as_bytes();
        (self.log)(&format!("link to {} up ({:?})", id_short(&id), conn.side()));
        {
            // a stranger holds a connection only so long: capped, and closed unless a seat pairs with it in time
            let st = self.st();
            if st.conns.len() >= MAX_LINKS
                && !st.conns.contains_key(&id)
                && !st.seats.is_paired(&id)
            {
                drop(st);
                conn.close(1u8.into(), b"busy");
                return;
            }
            if !st.seats.is_paired(&id) {
                let (me, c) = (self.clone(), conn.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(UNPAIRED_GRACE).await;
                    if !me.st().seats.is_paired(&id) {
                        c.close(2u8.into(), b"not paired");
                    }
                });
            }
        }
        let old = self.st().conns.insert(id, conn.clone());
        if let Some(o) = old {
            o.close(0u8.into(), b"replaced");
        }
        tokio::spawn(watch_path(self.log.clone(), conn.clone()));
        tokio::spawn(self.clone().pump_conn(conn));
    }

    async fn pump_conn(self: Arc<Self>, conn: Connection) {
        let remote = *conn.remote_id().as_bytes();
        while let Ok(d) = conn.read_datagram().await {
            let Some((seat, game)) = core::decode(&d) else {
                continue;
            };
            let sock = {
                let st = self.st();
                if !st.seats.accepts(seat, &remote) {
                    continue;
                }
                match core::deliver_from(game) {
                    Via::Reflect => Some(self.reflect.clone()),
                    Via::Peer => st.peers.get(&seat).map(|p| p.sock.clone()),
                }
            };
            if let Some(s) = sock {
                let _ = s.send_to(game, game_addr(seat)).await;
            }
        }
        let why = conn
            .close_reason()
            .map(|r| r.to_string())
            .unwrap_or_default();
        (self.log)(&format!("link to {} down: {why}", id_short(&remote)));
        let mut st = self.st();
        if st
            .conns
            .get(&remote)
            .is_some_and(|c| c.stable_id() == conn.stable_id())
        {
            st.conns.remove(&remote);
        }
    }
}

/// Logs the selected path when it changes (direct or relay) and its RTT every 30 s.
pub(crate) async fn watch_path(log: Log, conn: Connection) {
    let short = conn.remote_id().fmt_short();
    let mut last = None;
    let mut tick = interval(Duration::from_secs(1));
    for n in 0u64.. {
        tokio::select! {
            _ = tick.tick() => {}
            _ = conn.closed() => return,
        }
        let sel = conn
            .paths()
            .iter()
            .find(|p| p.is_selected())
            .map(|p| (p.is_relay(), p.rtt()));
        if let Some((relay, rtt)) = sel
            && (last != Some(relay) || n % 30 == 0)
        {
            let kind = if relay { "relay" } else { "direct" };
            (log)(&format!(
                "link to {short}: path {kind}, rtt {:.1} ms",
                rtt.as_secs_f64() * 1e3
            ));
            last = Some(relay);
        }
    }
}

impl Shared {
    async fn dial(self: Arc<Self>, id: core::EndpointId) {
        let addr = PublicKey::from_bytes(&id).map(|pk| relay_addr(pk, &self.relay));
        match addr {
            Ok(a) => match self.ep.connect(a, ALPN).await {
                Ok(c) => self.adopt(c),
                Err(e) => (self.log)(&format!("dial {}: {e}", id_short(&id))),
            },
            Err(e) => (self.log)(&format!("bad EndpointId from gmsvr: {e}")),
        }
        self.st().dialing.remove(&id);
    }

    /// Lookups for registered seats (unpaired every 500 ms, paired every 2 s) and dials for paired seats without a
    /// connection.
    async fn poll(self: Arc<Self>) {
        let mut tick = interval(LOOKUP_EVERY);
        let mut gm_down = false;
        let mut n = 0u64;
        loop {
            tick.tick().await;
            n += 1;
            let todo: Vec<_> = {
                let st = self.st();
                st.reg
                    .iter()
                    .filter(|(_, r)| **r == Reg::Done)
                    .filter_map(|(s, _)| {
                        Some((*s, st.seats.token(*s)?, st.seats.pair(*s).copied()))
                    })
                    .collect()
            };
            for (seat, token, cur) in todo {
                if cur.is_some() && !n.is_multiple_of(PAIRED_EVERY) {
                    continue;
                }
                match self.gm.ask(904, &token_bytes(token)).await {
                    Ok(b) => {
                        gm_down = false;
                        if let Some(p) = parse_lookup(&b)
                            && Some(p) != cur
                            && self.st().seats.token(seat) == Some(token)
                        {
                            self.pair(seat, p).await;
                        }
                    }
                    Err(e) if !gm_down => {
                        gm_down = true;
                        (self.log)(&format!("gmsvr lookup: {e}"));
                    }
                    Err(_) => {}
                }
            }
            let dial: Vec<_> = {
                let mut st = self.st();
                let want: Vec<_> = st
                    .reg
                    .keys()
                    .filter_map(|s| st.seats.pair(*s).map(|p| p.id))
                    .filter(|id| {
                        core::dials(&self.id, id)
                            && !st.conns.contains_key(id)
                            && !st.dialing.contains(id)
                    })
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect();
                st.dialing.extend(want.iter().copied());
                want
            };
            for id in dial {
                tokio::spawn(self.clone().dial(id));
            }
        }
    }

    async fn pump_reflect(self: Arc<Self>) {
        let mut buf = vec![0u8; 65536];
        loop {
            match self.reflect.recv_from(&mut buf).await {
                Ok((n, src)) => self.on_reflect(&buf[..n], src).await,
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
                Err(e) => {
                    (self.log)(&format!("reflect socket: {e}"));
                    return;
                }
            }
        }
    }

    async fn accept(self: Arc<Self>) {
        while let Some(inc) = self.ep.accept().await {
            let me = self.clone();
            tokio::spawn(async move {
                match inc.await {
                    Ok(c) => me.adopt(c),
                    Err(e) => (me.log)(&format!("incoming link: {e}")),
                }
            });
        }
    }
}

/// Links held at once, and how long one from an id no seat pairs with may stay (the opponent can dial before our
/// lookup shows the pairing).
const MAX_LINKS: usize = 64;
const UNPAIRED_GRACE: Duration = Duration::from_secs(15);

pub(crate) fn id_short(id: &core::EndpointId) -> String {
    PublicKey::from_bytes(id).map_or_else(|_| "?".into(), |pk| pk.fmt_short().to_string())
}

/// The endpoint's secret key from `path`, made and saved there on first use.
pub(crate) fn load_key(path: &Path) -> io::Result<SecretKey> {
    match std::fs::read(path) {
        Ok(b) => <[u8; 32]>::try_from(b)
            .map(|b| SecretKey::from_bytes(&b))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: not 32 bytes", path.display()),
                )
            }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let k = SecretKey::generate();
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, k.to_bytes())?;
            Ok(k)
        }
        Err(e) => Err(e),
    }
}

async fn start(cfg: Config) -> io::Result<Arc<Shared>> {
    if cfg.relay.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no relay URL"));
    }
    let relay = cfg.relay.clone();
    let transport = QuicTransportConfig::builder()
        .keep_alive_interval(KEEP_ALIVE)
        .max_idle_timeout(Some(IdleTimeout::try_from(IDLE).expect("60 s fits")))
        .datagram_send_buffer_size(SEND_BUF)
        .build();
    let mut b = Endpoint::builder(presets::Minimal)
        .secret_key(load_key(&cfg.key)?)
        .relay_mode(RelayMode::Custom(cfg.relay.clone()))
        .clear_address_lookup()
        .alpns(vec![ALPN.to_vec()])
        .transport_config(transport);
    if let Some(ca) = cfg.ca {
        b = b.ca_tls_config(ca);
    }
    if cfg.relay_only {
        b = b.clear_ip_transports();
    }
    let ep = b.bind().await.map_err(io::Error::other)?;
    let reflect = Arc::new(UdpSocket::bind(cfg.reflect).await?);
    let sh = Arc::new(Shared {
        log: cfg.log,
        id: *ep.id().as_bytes(),
        ep,
        relay,
        reflect,
        gm: Gmsvr {
            addr: cfg.gmsvr,
            conn: Default::default(),
            seq: AtomicU16::new(0),
        },
        st: Default::default(),
        oversize: AtomicU64::new(0),
        ticket: cfg.ticket,
    });
    sh.hello().await;
    (sh.log)(&format!(
        "vslink {} on {} (relay {}{})",
        sh.ep.id().fmt_short(),
        sh.reflect.local_addr()?,
        relay_names(&sh.relay),
        if cfg.relay_only { ", relay only" } else { "" }
    ));
    Ok(sh)
}

/// Starts vslink on its own thread. Returns once the endpoint and the reflect socket are bound and the first hello
/// went to gmsvr (a gmsvr that does not answer is logged, not an error).
pub fn spawn(cfg: Config) -> io::Result<Link> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("vslink".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("vslink-rt")
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let sh = match rt.block_on(start(cfg)) {
                Ok(sh) => sh,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let addr = match sh.reflect.local_addr() {
                Ok(SocketAddr::V4(a)) => a,
                _ => SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
            };
            let _ = tx.send(Ok((addr, sh.id)));
            rt.block_on(async {
                tokio::spawn(sh.clone().pump_reflect());
                tokio::spawn(sh.clone().poll());
                tokio::spawn(sh.clone().accept());
                let hello = sh.clone();
                tokio::spawn(async move {
                    let mut t = interval(HELLO_EVERY);
                    t.tick().await; // the first went out in start
                    loop {
                        t.tick().await;
                        hello.hello().await;
                    }
                });
                while !flag.load(Ordering::Relaxed) {
                    sleep(Duration::from_millis(100)).await;
                }
                let _ = timeout(Duration::from_secs(2), sh.ep.close()).await;
            });
            rt.shutdown_timeout(Duration::from_secs(1));
        })?;
    let (reflect, id) = rx
        .recv()
        .map_err(|_| io::Error::other("vslink thread ended at start"))??;
    Ok(Link {
        handle: Handle::new(stop, thread),
        reflect,
        id,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn set_relay_takes_a_url_and_refuses_a_missing_ca() {
        let mut c = Config::public("127.0.0.1:1".parse().unwrap(), "k".into(), Arc::new(|_| {}));
        assert!(c.set_relay("not a url", None).is_err());
        c.set_relay("https://relay.example:8443", None).unwrap();
        assert!(
            c.relay.urls::<Vec<_>>()[0]
                .as_str()
                .starts_with("https://relay.example:8443")
        );
        assert!(
            c.set_relay("https://relay.example:8443", Some(Path::new("no-such.pem")))
                .is_err()
        );
    }

    use super::*;

    #[test]
    fn public_relays_are_ours_plus_n0_and_peers_are_dialled_through_all() {
        let map = public_relays();
        let ours: RelayUrl = RELAY_URL.parse().unwrap();
        assert!(map.contains(&ours));
        for url in iroh::defaults::prod::default_relay_map().urls::<Vec<RelayUrl>>() {
            assert!(map.contains(&url), "{url}");
        }
        assert_eq!(map.len(), 5);
        let c = Config::public("127.0.0.1:1".parse().unwrap(), "k".into(), Arc::new(|_| {}));
        assert_eq!(c.relay, map);
        let pk = SecretKey::generate().public();
        assert_eq!(relay_addr(pk, &map).relay_urls().count(), 5);
    }

    #[test]
    fn hello_with_and_without_a_room_ticket() {
        let id = [7u8; 32];
        let open = hello_payload(&id, None);
        assert_eq!(
            (open.len(), &open[..4], &open[4..]),
            (36, &[0u8; 4][..], &id[..])
        );
        let room = hello_payload(&id, Some("0123456789abcdef"));
        assert_eq!(
            (room.len(), &room[..36], &room[36..]),
            (52, &open[..], &b"0123456789abcdef"[..])
        );
        let short = hello_payload(&id, Some("ab12"));
        assert_eq!(&short[36..], b"ab12\0\0\0\0\0\0\0\0\0\0\0\0");
    }

    #[test]
    fn lookup_answers() {
        let mut b = vec![0u8; 48];
        assert_eq!(parse_lookup(&b), None, "unpaired");
        b[4] = 1;
        b[8..12].copy_from_slice(&[127, 9, 8, 7]);
        b[12..14].copy_from_slice(&20003u16.to_le_bytes());
        assert_eq!(parse_lookup(&b), None, "opponent not registered");
        b[16..].fill(5);
        let p = parse_lookup(&b).unwrap();
        assert_eq!(p.token, "127.9.8.7:20003".parse().unwrap());
        assert_eq!((p.id, p.remote_seat()), ([5; 32], Some(3)));
        assert_eq!(token_bytes(p.token), [127, 9, 8, 7, 0x23, 0x4E]);
    }
}
