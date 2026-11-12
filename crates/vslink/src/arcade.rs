//! The arcade tunnel (M2 plan 5.2): a seat on another PC reaches a 2017-18 arcade's control, seat desk and match relay
//! through one iroh connection to the arcade's PC, so the arcade needs no inbound port (a hole punch, else the relay).
//!
//! - Join side: listens on its own loopback where the Launcher and the game look for an arcade (TCP: the control and
//!   desk ports; UDP: the relay port). Each TCP connection becomes a QUIC bi-stream whose first byte is its kind
//!   ([`KIND_CONTROL`], [`KIND_DESK`]). Each local UDP socket that sends to the relay port is a flow: its datagrams
//!   cross as `[flow u32 LE][datagram]`, and the answers go back to that socket from the relay port.
//! - Host side: dials only its own loopback: the kind's port for a stream; for datagrams, the relay port, from one
//!   socket per (guest, flow), so the relay tells the guest's game socket and ping socket apart.
//!
//! The panel relay already routes every in-arcade match packet through the relay port (`crates/panel/src/net.rs`), so
//! the seats' peer link needs nothing more. The streams are buffered on this side (all of a guest's on the host, all
//! of the join side's) up to [`STREAM_BUF`] together, so a slow path never blocks the game's own `send`; past that the
//! stream that overflows is closed and logged. A host takes [`MAX_STREAMS`] streams per guest, and closes a guest
//! that opens nothing usable within [`GUEST_GRACE`].
//! Join side ports: the arcade's own where free, else any free one ([`ArcadeLink::ports`] says which).
//! Between two ends newer than 0.6.3 the streams cross in lz4 frames (`crate::pack`, negotiated by ALPN:
//! [`ARCADE_ALPN_LZ4`]); with an older end they stay as they are.
//! Between two ends newer than 0.7.1 ([`ARCADE_ALPN_3`]) the flows also cross on one ordered stream each way instead
//! of datagrams: the lockstep needs every frame in order, so a stall pauses the match and loses nothing. With an older
//! end the flows stay datagrams, as before.
//! The test-only delay ([`ArcadeConfig::delay`], the Launcher's `VSLINK_DELAY_MS` / 2) holds everything this side
//! sends and receives that much longer.

use crate::core::EndpointId;
use crate::net::{
    FALLBACK_AFTER, dial_relays, fallback_for, home_or_fall_back, id_short, load_key,
    public_relays, relay_addr, relay_from, relay_names, watch_path,
};
use crate::pack::{Packer, Unpacker, size};
use bytes::Bytes;
use forza::service::{Handle, Log};
use iroh::endpoint::{
    ConnectOptions, Connection, IdleTimeout, QuicTransportConfig, RecvStream, presets,
};
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

// the ALPN keeps "shop" from before the arcade rename: both tunnel ends must agree on it
/// Streams uncompressed, flows as datagrams (0.6.3 and older).
pub const ARCADE_ALPN: &[u8] = b"forza-wccf/shop/1";
/// The same tunnel with its streams in lz4 frames (`crate::pack`); the flows stay datagrams (0.7.0 and 0.7.1).
pub const ARCADE_ALPN_LZ4: &[u8] = b"forza-wccf/shop/2";
/// [`ARCADE_ALPN_LZ4`], and the flows on one ordered stream each way, as `[len u16 LE][flow u32 LE][datagram]`.
pub const ARCADE_ALPN_3: &[u8] = b"forza-wccf/shop/3";
/// Newest first: the host takes the first of these the guest offers, the guest offers them all. A link with an older
/// peer gets exactly what that peer speaks.
const ALPNS: [&[u8]; 3] = [ARCADE_ALPN_3, ARCADE_ALPN_LZ4, ARCADE_ALPN];
/// The first byte of a stream: what the host dials for it.
pub const KIND_CONTROL: u8 = 1;
pub const KIND_DESK: u8 = 2;
/// The host's close code for a guest its room does not name: the join side stops dialling and says so.
pub const CLOSE_NOT_IN_ROOM: u8 = 4;
/// The host's close code for a guest it could not check now (its room's list did not come, too many at the door): the
/// join side dials again.
pub const CLOSE_NO_ANSWER: u8 = 5;
/// The host's close code for every guest when it stops its arcade: the join side says the host closed it (and dials
/// again, in case the host comes back). Old peers: a 0.6.1 guest only acts on [`CLOSE_NOT_IN_ROOM`], so it takes this
/// as any drop and dials again, as it did after a 0.6.1 host's plain endpoint close (code 0).
pub const CLOSE_ARCADE_CLOSED: u8 = 6;
/// Either side's close code when the other side's flow stream ([`ARCADE_ALPN_3`] only) ends, breaks or sends a bad
/// frame: the join side dials again onto fresh streams, as after a host restart.
pub const CLOSE_BAD_FLOWS: u8 = 7;
/// Join, after a bad flow stream: the first wait before dialling again, doubled after each link that lived less than
/// [`HEALTHY_LINK`], up to [`BACKOFF_CAP`].
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(30);
const HEALTHY_LINK: Duration = Duration::from_secs(60);
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
/// Flow datagrams queued per direction: then datagram flows drop new ones, stream flows wait.
const DGRAM_QUEUE: usize = 1024;
/// The largest flow packet a side sends: a flow number and a loopback UDP datagram (65,507 bytes).
const MAX_FLOW_PACKET: usize = 4 + 65_507;
const KEEP_ALIVE: Duration = Duration::from_secs(5);
/// Above the game's 30 s silence error (2001).
const IDLE: Duration = Duration::from_secs(60);
/// How long a local connection waits for the link to the arcade, and the join's first dial.
const LINK_WAIT: Duration = Duration::from_secs(20);

/// The arcade's loopback ports: where the host dials them, where the join side listens.
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
    /// The arcade's PC: guests dial this endpoint.
    Host,
    /// A seat elsewhere: dials the host's EndpointId.
    Join(EndpointId),
}

/// Host: whether a guest's EndpointId may come in (an arcade room's `GUESTS`, `forza::online::lobby::arcade_peers`). It
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
pub struct ArcadeConfig {
    pub role: Role,
    pub ports: Ports,
    /// The relays the endpoint homes on; the host is dialled through these and their fallback (`net::dial_relays`)
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
    /// Test only: speak this ALPN and the older ones alone, as the Forza-WCCF whose newest it was; `None`: up to
    /// [`ARCADE_ALPN_3`].
    pub speak_up_to: Option<&'static [u8]>,
}

impl ArcadeConfig {
    /// What the Launcher runs: `net::public_relays` (n0's as the fallback), direct paths allowed, no delay.
    pub fn public(role: Role, ports: Ports, key: PathBuf, log: Log) -> Self {
        ArcadeConfig {
            role,
            ports,
            relay: public_relays(),
            ca: None,
            relay_only: false,
            key,
            log,
            delay: Duration::ZERO,
            admit: None,
            speak_up_to: None,
        }
    }

    /// Uses only `url` instead of `net::public_relays`, with no fallback; `ca_pem` (a PEM file) is then its only
    /// trusted root.
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
/// in its arcade room before it listens, a guest tells it before it dials.
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

/// Counts what is open while the guard lives (a flow's socket, a stream): one up here, one down at drop.
fn live_guard(
    n: &Arc<AtomicUsize>,
) -> core::mem::DropGuard<Arc<AtomicUsize>, fn(Arc<AtomicUsize>)> {
    n.fetch_add(1, Ordering::Relaxed);
    core::mem::DropGuard::new(n.clone(), |n| {
        n.fetch_sub(1, Ordering::Relaxed);
    })
}

/// Join: whether the link went down over a bad flow stream: the host's [`CLOSE_BAD_FLOWS`], or this side's own close
/// (the join closes a link only in [`recv_flow`], with that code).
fn bad_flows(why: &iroh::endpoint::ConnectionError) -> bool {
    use iroh::endpoint::ConnectionError::{ApplicationClosed, LocallyClosed};
    matches!(why, LocallyClosed)
        || matches!(why, ApplicationClosed(c) if c.error_code == CLOSE_BAD_FLOWS.into())
}

/// Join: whether the link went down because the host closed the arcade: its [`CLOSE_ARCADE_CLOSED`], or code 0, the
/// endpoint close a 0.6.1 host sends when it stops (iroh's `Endpoint::close`; every other close here has its own code).
pub fn closed_by_host(why: &iroh::endpoint::ConnectionError) -> bool {
    matches!(why, iroh::endpoint::ConnectionError::ApplicationClosed(c)
        if c.error_code == CLOSE_ARCADE_CLOSED.into() || c.error_code == 0u8.into())
}

/// A running arcade tunnel. Dropping `handle` stops it: up to about 3 s (the endpoint's close, then the runtime's).
pub struct ArcadeLink {
    pub handle: Handle,
    /// This endpoint's id (the host's is what guests type)
    pub id: EndpointId,
    /// Join: the loopback ports it listens on (the arcade's own unless taken). Host: the ports it dials.
    pub ports: Ports,
    flows: Arc<AtomicUsize>,
    streams: Arc<AtomicUsize>,
    guests: Arc<AtomicUsize>,
    host_closed: Arc<AtomicBool>,
}

impl ArcadeLink {
    /// Join: true while the link is down because the host closed the arcade ([`closed_by_host`]); false again once
    /// the link is back.
    pub fn host_closed(&self) -> Arc<AtomicBool> {
        self.host_closed.clone()
    }

    /// Host: the guests connected now (admitted, link up), live: an arcade server with guests holds their matches.
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
    /// Times a flow producer waited for room on a stream link ([`feed`]).
    waits: AtomicU64,
    flows: Arc<AtomicUsize>,
    streams: Arc<AtomicUsize>,
    admit: Option<Admit>,
    /// Host: the guests' connections, told [`CLOSE_ARCADE_CLOSED`] when the arcade stops.
    conns: Mutex<Vec<Connection>>,
    /// Join: [`ArcadeLink::host_closed`].
    host_closed: Arc<AtomicBool>,
    /// What this side speaks, newest first ([`ALPNS`], or fewer as [`ArcadeConfig::speak_up_to`] says).
    alpns: Vec<Vec<u8>>,
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

/// [`ALPNS`] from `newest` down (all of them for `None`); an ALPN not among them is an error.
fn alpns_up_to(newest: Option<&[u8]>) -> io::Result<Vec<Vec<u8>>> {
    let from = match newest {
        None => 0,
        Some(a) => ALPNS.iter().position(|&x| x == a).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "speak_up_to: {:?} is no arcade ALPN",
                    String::from_utf8_lossy(a)
                ),
            )
        })?,
    };
    Ok(ALPNS[from..].iter().map(|a| a.to_vec()).collect())
}

/// Does `conn` carry flows on streams ([`ARCADE_ALPN_3`])?
fn on_streams(conn: &Connection) -> bool {
    conn.alpn() == ARCADE_ALPN_3
}

/// `d` (a flow and its datagram) as a frame on the flow stream; `None` over [`MAX_FLOW_PACKET`].
fn flow_frame(d: &[u8]) -> Option<Vec<u8>> {
    let len = u16::try_from(d.len())
        .ok()
        .filter(|&n| usize::from(n) <= MAX_FLOW_PACKET)?;
    Some([&len.to_le_bytes()[..], d].concat())
}

/// The next frame's flow and datagram from a flow stream; an error, for the log, when the stream ends or breaks
/// (between or inside frames) or sends a frame under 4 bytes (no flow number) or over [`MAX_FLOW_PACKET`].
async fn read_flow<R: AsyncRead + Unpin>(r: &mut R) -> Result<Vec<u8>, String> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len)
        .await
        .map_err(|e| format!("between frames: {e}"))?;
    let n = usize::from(u16::from_le_bytes(len));
    if n < 4 {
        return Err(format!("a {n}-byte frame, under the 4-byte flow number"));
    }
    if n > MAX_FLOW_PACKET {
        return Err(format!(
            "a {n}-byte frame, over the {MAX_FLOW_PACKET}-byte cap"
        ));
    }
    let mut d = vec![0u8; n];
    r.read_exact(&mut d)
        .await
        .map_err(|e| format!("inside a {n}-byte frame: {e}"))?;
    Ok(d)
}

/// Sends flow datagrams on `conn` after the test delay, in order: on its stream ([`ARCADE_ALPN_3`]), where a stall
/// holds them (feed it with [`feed`]); else as datagrams, where too big drops them (logged).
fn datagram_line(sh: &Arc<Sh>, conn: Connection) -> mpsc::Sender<(Instant, Bytes)> {
    let (tx, mut rx) = mpsc::channel::<(Instant, Bytes)>(DGRAM_QUEUE);
    let sh = sh.clone();
    tokio::spawn(async move {
        let mut stream = None; // opened with the first datagram
        while let Some((at, d)) = rx.recv().await {
            sleep_until(at + sh.delay).await;
            if on_streams(&conn) {
                if stream.is_none() {
                    stream = Some(match conn.open_uni().await {
                        Ok(s) => s,
                        Err(_) => return,
                    });
                }
                // a loopback datagram and its flow always fit; shortcut: the queues count frames, not bytes, so a
                // peer sending 64 KB frames to a stalled side may hold up to DGRAM_QUEUE x 64 KB here: bound by bytes if
                // that ever shows
                let Some(f) = flow_frame(&d) else {
                    continue;
                };
                let s = stream.as_mut().expect("opened above");
                if s.write_all(&f).await.is_err() {
                    return;
                }
                continue;
            }
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

/// Queues `item` on a [`datagram_line`] or [`udp_line`] of `conn`'s flows: on a stream it waits for room (a stall holds
/// the sender, not drops; counted in `sh.waits` and logged at powers of two); as datagrams a full queue drops it, as a
/// lost datagram. `false`: the line is gone.
async fn feed<T>(sh: &Sh, conn: &Connection, tx: &mpsc::Sender<T>, item: T) -> bool {
    match tx.try_send(item) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(item)) if on_streams(conn) => {
            let n = sh.waits.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() {
                (sh.log)(&format!(
                    "flows: a queue is full, waiting for room ({n} so far)"
                ));
            }
            tx.send(item).await.is_ok()
        }
        Err(e) => matches!(e, mpsc::error::TrySendError::Full(_)),
    }
}

/// The next flow datagram from the other side: on its stream ([`ARCADE_ALPN_3`], `stream` holds it once it came), else
/// as a datagram. `None`: the link is gone. A flow stream that ends, breaks or sends a bad frame while the link is up
/// closes the link ([`CLOSE_BAD_FLOWS`], logged as `{who}: ...`), so the join side dials again onto fresh streams.
async fn recv_flow(
    sh: &Sh,
    who: &str,
    conn: &Connection,
    stream: &mut Option<RecvStream>,
) -> Option<Bytes> {
    if !on_streams(conn) {
        return conn.read_datagram().await.ok();
    }
    if stream.is_none() {
        *stream = Some(conn.accept_uni().await.ok()?);
    }
    let s = stream.as_mut()?;
    match read_flow(s).await {
        Ok(d) => Some(d.into()),
        Err(why) => {
            if conn.close_reason().is_none() {
                (sh.log)(&format!(
                    "{who}: the flow stream failed ({why}), closing the link"
                ));
                conn.close(CLOSE_BAD_FLOWS.into(), b"bad flow stream");
            }
            None
        }
    }
}

/// Sends datagrams from `sock` after the test delay, in order; full: dropped (callers on a stream wait instead).
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

/// What a [`pipe`] does to the bytes on their way.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    Plain,
    /// Reads raw bytes, writes `crate::pack` frames (TCP to QUIC on a compressed link)
    Pack,
    /// Reads frames, writes raw bytes (QUIC to TCP)
    Unpack,
}

/// How a [`pipe`]'s reading ended.
enum End {
    Done,
    Over,
    Bad(io::Error),
}

/// Copies `r` to `w` through `codec`, holding each chunk the test delay and at most [`STREAM_BUF`] bytes (as they
/// are held: frames when packed, raw when unpacked); then shuts `w` down. Returns the bytes read and written, both
/// as far as a stream over the limit or with a bad frame got before it is logged and dropped there. Nothing is
/// spawned: dropping the future ends both halves.
async fn pipe<R, W>(
    mut r: R,
    mut w: W,
    codec: Codec,
    delay: Duration,
    budget: Arc<AtomicUsize>,
    what: String,
    log: Log,
) -> (u64, u64)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let held = Held {
        budget,
        mine: AtomicUsize::new(0),
    };
    let read = AtomicU64::new(0);
    let written = AtomicU64::new(0);
    let (tx, mut rx) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    let reader = async {
        let tx = tx;
        let mut buf = vec![0u8; crate::pack::MAX_RAW];
        let (mut packer, mut unpacker) = (Packer::default(), Unpacker::default());
        loop {
            let chunk = if codec == Codec::Unpack {
                match unpacker.next(&mut r).await {
                    Ok(Some(raw)) => {
                        read.store(unpacker.wire, Ordering::Relaxed);
                        raw
                    }
                    Ok(None) => return End::Done,
                    Err(e) => {
                        read.store(unpacker.wire, Ordering::Relaxed); // a cut frame's bytes count too
                        return match e.kind() {
                            io::ErrorKind::InvalidData => End::Bad(e),
                            _ => End::Done,
                        };
                    }
                }
            } else {
                let n = match r.read(&mut buf).await {
                    Ok(0) | Err(_) => return End::Done,
                    Ok(n) => n,
                };
                read.fetch_add(n as u64, Ordering::Relaxed);
                match codec {
                    Codec::Pack => packer.pack(&buf[..n]),
                    _ => buf[..n].to_vec(),
                }
            };
            if held.add(chunk.len()) > STREAM_BUF {
                return End::Over;
            }
            if tx.send((Instant::now(), chunk)).is_err() {
                return End::Done;
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
            written.store(n, Ordering::Relaxed);
            held.sub(chunk.len());
        }
        let _ = w.shutdown().await;
        n
    };
    tokio::pin!(reader, writer);
    let mut reading = true;
    loop {
        tokio::select! {
            end = &mut reader, if reading => match end {
                End::Done => reading = false, // the sender is gone: the writer drains, then shuts down
                End::Over => {
                    log(&format!("{what}: more than {} MiB waiting to cross, closed", STREAM_BUF >> 20));
                    return (
                        read.load(Ordering::Relaxed),
                        written.load(Ordering::Relaxed),
                    );
                }
                End::Bad(e) => {
                    log(&format!("{what}: a bad frame from the other side ({e}), closed"));
                    return (
                        read.load(Ordering::Relaxed),
                        written.load(Ordering::Relaxed),
                    );
                }
            },
            n = &mut writer => return (read.load(Ordering::Relaxed), n),
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

/// Both directions between a local TCP connection and a QUIC stream, in lz4 frames if `packed`; logs the totals when
/// both are done (the game's bytes, and on a packed stream what crossed for them).
async fn splice<QR, QW>(
    tcp: TcpStream,
    qr: QR,
    qw: QW,
    packed: bool,
    sh: Arc<Sh>,
    budget: Arc<AtomicUsize>,
    what: String,
) where
    QR: AsyncRead + Unpin,
    QW: AsyncWrite + Unpin,
{
    let _live = live_guard(&sh.streams);
    let (tr, tw) = tcp.into_split();
    let (out, inn) = match packed {
        true => (Codec::Pack, Codec::Unpack),
        false => (Codec::Plain, Codec::Plain),
    };
    let ((up, up_wire), (down_wire, down)) = tokio::join!(
        pipe(
            tr,
            qw,
            out,
            sh.delay,
            budget.clone(),
            what.clone(),
            sh.log.clone()
        ),
        pipe(qr, tw, inn, sh.delay, budget, what.clone(), sh.log.clone())
    );
    (sh.log)(&format!("{what} closed ({up} bytes out, {down} in)"));
    if packed {
        (sh.log)(&packed_line(&what, (up, up_wire), (down, down_wire)));
    }
}

/// `{what} lz4: out <game bytes> -> <bytes across>, in <game bytes> -> <bytes across>`.
fn packed_line(what: &str, out: (u64, u64), inn: (u64, u64)) -> String {
    format!(
        "{what} lz4: out {} -> {}, in {} -> {}",
        size(out.0),
        size(out.1),
        size(inn.0),
        size(inn.1)
    )
}

/// Whether the link's streams are in lz4 frames: both ends offered [`ARCADE_ALPN_LZ4`] or [`ARCADE_ALPN_3`].
fn packs(conn: &Connection) -> bool {
    conn.alpn() == ARCADE_ALPN_LZ4 || on_streams(conn)
}

/// What a link carries, for its "joined" / "up" line.
fn link_text(conn: &Connection) -> &'static str {
    if on_streams(conn) {
        "streams in lz4, flows on a stream"
    } else if packs(conn) {
        "streams in lz4, flows as datagrams (the other PC runs Forza-WCCF 0.7.1 or older)"
    } else {
        "streams uncompressed, flows as datagrams (the other PC runs Forza-WCCF 0.6.3 or older)"
    }
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
                    (self.log)(&format!("guest {who} refused: not in the arcade's room"));
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
        (self.log)(&format!("guest {who} joined ({})", link_text(&conn)));
        {
            let mut conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
            conns.retain(|c| c.close_reason().is_none());
            conns.push(conn.clone());
        }
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
            let (me, who, used, budget, conn) = (
                self.clone(),
                who.clone(),
                used.clone(),
                budget.clone(),
                conn.clone(),
            );
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
                        splice(tcp, qr, qw, packs(&conn), me.clone(), budget, what).await;
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
        let mut inbound = None;
        let guest = format!("guest {who}");
        while let Some(d) = recv_flow(&self, &guest, &conn, &mut inbound).await {
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
                let (sock, tx, live, conn, sh) = (
                    s.clone(),
                    out.clone(),
                    live_guard(&self.flows),
                    conn.clone(),
                    self.clone(),
                );
                let task = tasks.spawn(async move {
                    let _live = live;
                    let mut buf = vec![0u8; 65536];
                    loop {
                        match sock.recv_from(&mut buf).await {
                            Ok((n, src)) if src == relay => {
                                let d = Bytes::from(encode_flow(flow, &buf[..n]));
                                if !feed(&sh, &conn, &tx, (Instant::now(), d)).await {
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
                // on a stream: waits rather than drops, so a burst after a stall reaches the relay whole
                feed(
                    &self,
                    &conn,
                    line,
                    (Instant::now(), payload.to_vec(), relay),
                )
                .await;
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

    /// Offers every ALPN this side speaks: the host picks the newest it knows.
    async fn dial(&self) -> Result<Connection, String> {
        let opts = ConnectOptions::new().with_additional_alpns(self.sh.alpns[1..].to_vec());
        let dial = async {
            let connecting = self
                .sh
                .ep
                .connect_with_opts(self.host.clone(), &self.sh.alpns[0], opts)
                .await
                .map_err(|e| e.to_string())?;
            connecting.await.map_err(|e| e.to_string())
        };
        match timeout(LINK_WAIT, dial).await {
            Ok(r) => r,
            Err(_) => Err(format!("no answer in {} s", LINK_WAIT.as_secs())),
        }
    }

    /// Keeps the link to the host up: `first` is the one [`spawn`] made; a dropped link is dialled again.
    async fn keep(self: Arc<Self>, first: Connection) {
        let mut conn = first;
        let mut wait = Duration::ZERO;
        let mut bad = 0u32;
        loop {
            let up = Instant::now();
            self.link.send_replace(Some(conn.clone()));
            tokio::spawn(watch_path(self.sh.log.clone(), conn.clone()));
            let inbound = tokio::spawn(self.clone().from_host(conn.clone()));
            let why = conn.closed().await;
            inbound.abort();
            self.link.send_replace(None);
            if closed_by_host(&why) {
                self.sh.host_closed.store(true, Ordering::Relaxed);
                (self.sh.log)("the host closed the arcade");
            }
            if let iroh::endpoint::ConnectionError::ApplicationClosed(c) = &why
                && c.error_code == CLOSE_NOT_IN_ROOM.into()
            {
                // dialling again would be refused again: the game finds no arcade and says so
                (self.sh.log)(
                    "the arcade refused this PC: not in its room (join with the room code, in ONLINE)",
                );
                return;
            }
            if bad_flows(&why) {
                // a peer that keeps breaking its flow stream: dial again ever slower, so the log and the relay stay calm
                wait = if up.elapsed() >= HEALTHY_LINK {
                    BACKOFF_FIRST
                } else {
                    (wait * 2).clamp(BACKOFF_FIRST, BACKOFF_CAP)
                };
                bad += 1;
                (self.sh.log)(&format!(
                    "link to the arcade down: {why}; dialling again in {} s (bad flow stream {bad})",
                    wait.as_secs()
                ));
                sleep(wait).await;
            } else {
                (self.sh.log)(&format!("link to the arcade down: {why}; dialling again"));
            }
            conn = loop {
                match self.dial().await {
                    Ok(c) => break c,
                    Err(e) => {
                        (self.sh.log)(&format!("arcade: {e}"));
                        sleep(Duration::from_secs(2)).await;
                    }
                }
            };
            self.sh.host_closed.store(false, Ordering::Relaxed);
            (self.sh.log)(&format!(
                "link to the arcade up again ({})",
                link_text(&conn)
            ));
        }
    }

    /// The relay's answers, to the local socket of their flow, from the relay port.
    async fn from_host(self: Arc<Self>, conn: Connection) {
        let line = udp_line(self.udp.clone(), self.sh.delay);
        let mut inbound = None;
        while let Some(d) = recv_flow(&self.sh, "arcade link", &conn, &mut inbound).await {
            let Some((flow, payload)) = decode_flow(&d) else {
                continue;
            };
            let Some(to) = self.flows().addr_of(flow) else {
                continue;
            };
            feed(
                &self.sh,
                &conn,
                &line,
                (Instant::now(), payload.to_vec(), to),
            )
            .await;
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
                line = Some((id, datagram_line(&self.sh, conn.clone())));
            }
            let flow = self.flows().flow_of(src);
            let d = Bytes::from(encode_flow(flow, &buf[..n]));
            if let Some((_, tx)) = &line {
                feed(&self.sh, &conn, tx, (Instant::now(), d)).await;
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
                            "{} connection dropped: no link to the arcade",
                            kind_name(kind)
                        ));
                        return;
                    }
                };
                let what = format!("{} connection", kind_name(kind));
                match conn.open_bi().await {
                    Ok((mut qw, qr)) => {
                        if qw.write_all(&[kind]).await.is_ok() {
                            let packed = packs(&conn);
                            splice(tcp, qr, qw, packed, me.sh.clone(), me.budget.clone(), what)
                                .await;
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

async fn start(cfg: ArcadeConfig) -> io::Result<(Arc<Sh>, Option<Joined>)> {
    if cfg.relay.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no relay URL"));
    }
    let relay = dial_relays(&cfg.relay);
    let transport = QuicTransportConfig::builder()
        .keep_alive_interval(KEEP_ALIVE)
        .max_idle_timeout(Some(IdleTimeout::try_from(IDLE).expect("60 s fits")))
        .build();
    let alpns = alpns_up_to(cfg.speak_up_to)?;
    let mut b = Endpoint::builder(presets::Minimal)
        .secret_key(load_key(&cfg.key)?)
        .relay_mode(RelayMode::Custom(cfg.relay.clone()))
        .clear_address_lookup()
        .transport_config(transport);
    if cfg.role == Role::Host {
        // the first of these the guest offers wins
        b = b.alpns(alpns.clone());
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
        waits: AtomicU64::new(0),
        flows: Arc::default(),
        streams: Arc::default(),
        admit: cfg.admit,
        conns: Mutex::default(),
        host_closed: Arc::default(),
        alpns,
    });
    let fallback = fallback_for(&cfg.relay);
    let only = format!(
        "{}{}",
        if fallback.is_empty() {
            ""
        } else {
            ", public relays if it does not answer"
        },
        if cfg.relay_only { ", relay only" } else { "" }
    );
    let (ep, log) = (sh.ep.clone(), sh.log.clone());
    tokio::spawn(async move { home_or_fall_back(&ep, &fallback, FALLBACK_AFTER, &log).await });
    if !sh.delay.is_zero() {
        (sh.log)(&format!(
            "test delay: {} ms more each way on this side (VSLINK_DELAY_MS)",
            sh.delay.as_millis()
        ));
    }
    let Role::Join(host) = cfg.role else {
        (sh.log)(&format!(
            "arcade host {} (relay {}{only}): guests run Launcher.exe remote {}",
            sh.ep.id().fmt_short(),
            relay_names(&cfg.relay),
            id_text(sh.ep.id().as_bytes())
        ));
        return Ok((sh, None));
    };
    // the local side first, before anything dials: the arcade's own ports, else any free one (this PC runs a server of
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
        "arcade join {} to {} (relay {}{only}), local ports TCP {} and {}, UDP {}",
        sh.ep.id().fmt_short(),
        id_short(&host),
        relay_names(&cfg.relay),
        p.control,
        p.desk,
        p.relay
    ));
    let first = join.dial().await.map_err(|e| {
        io::Error::other(format!(
            "the arcade {} did not answer: {e}",
            id_short(&host)
        ))
    })?;
    (sh.log)(&format!("link to the arcade up ({})", link_text(&first)));
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
pub fn spawn(cfg: ArcadeConfig) -> io::Result<ArcadeLink> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("vslink-arcade".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("vslink-arcade-rt")
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
                sh.host_closed.clone(),
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
                // the guests hear why before the endpoint closes (its close flushes these frames)
                for c in sh
                    .conns
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .drain(..)
                {
                    c.close(CLOSE_ARCADE_CLOSED.into(), b"arcade closed");
                }
                let _ = timeout(Duration::from_secs(2), sh.ep.close()).await;
            });
            rt.shutdown_timeout(Duration::from_secs(1));
        })?;
    let (id, flows, streams, guests, host_closed, ports) = rx
        .recv()
        .map_err(|_| io::Error::other("the arcade tunnel's thread ended at start"))??;
    Ok(ArcadeLink {
        handle: Handle::new(stop, thread),
        id,
        ports,
        flows,
        streams,
        guests,
        host_closed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_the_arcade_ports_only() {
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

    /// The line a packed stream adds beside its "closed" line: the game's bytes, then what crossed for them.
    #[test]
    fn the_lz4_line() {
        assert_eq!(
            packed_line(
                "guest 9581768412: control connection",
                (1_792_094, 41_234),
                (6_251, 2_020)
            ),
            "guest 9581768412: control connection lz4: out 1.79 MB -> 41 KB, in 6 KB -> 2 KB"
        );
    }

    /// A frame cut by the other side's end is a bad frame, not a half-close: the line names it, and the counts keep
    /// what crossed before the drop.
    #[tokio::test]
    async fn a_cut_frame_is_logged_and_counted() {
        let mut p = crate::pack::Packer::default();
        let mut wire = p.pack(&[7u8; 4000]);
        let first = wire.len();
        let mut cut = p.pack(&[9u8; 4000]);
        cut.truncate(cut.len() - 3); // the second frame's body, short
        wire.extend_from_slice(&cut);
        let (mut feed, rd) = tokio::io::duplex(64 << 10);
        let (wr, mut down) = tokio::io::duplex(64 << 10);
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let log: Log = {
            let lines = lines.clone();
            Arc::new(move |s: &str| lines.lock().unwrap().push(s.to_owned()))
        };
        let budget = Arc::new(AtomicUsize::new(0));
        let run = pipe(
            rd,
            wr,
            Codec::Unpack,
            Duration::ZERO,
            budget.clone(),
            "control connection".into(),
            log,
        );
        // the cut frame only once the first one has crossed, so the counts are the ones the pipe returns with
        let wire_len = wire.len() as u64;
        let feed_it = async move {
            feed.write_all(&wire[..first]).await.unwrap();
            let mut got = [0u8; 4000];
            down.read_exact(&mut got).await.unwrap();
            assert!(got.iter().all(|&b| b == 7));
            feed.write_all(&wire[first..]).await.unwrap();
            drop(feed); // the other side hangs up inside the second frame
            let mut rest = Vec::new();
            down.read_to_end(&mut rest).await.unwrap();
            rest
        };
        let (counts, rest) = tokio::join!(run, feed_it);
        assert_eq!(
            counts,
            (wire_len, 4000),
            "the cut frame's bytes count, the raw bytes it never brought do not"
        );
        assert!(rest.is_empty(), "the cut frame reached the game");
        assert_eq!(budget.load(Ordering::Relaxed), 0, "nothing still held");
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with(
                "control connection: a bad frame from the other side (frame cut after "
            ),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn speak_up_to_takes_only_arcade_alpns() {
        assert_eq!(alpns_up_to(None).unwrap().len(), 3);
        assert_eq!(
            alpns_up_to(Some(ARCADE_ALPN_LZ4)).unwrap(),
            [ARCADE_ALPN_LZ4.to_vec(), ARCADE_ALPN.to_vec()]
        );
        let e = alpns_up_to(Some(b"forza-wccf/shop/9")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn flow_framing() {
        let d = encode_flow(0x0403_0201, b"ab");
        assert_eq!(d, [1, 2, 3, 4, b'a', b'b']);
        assert_eq!(decode_flow(&d), Some((0x0403_0201, &b"ab"[..])));
        assert_eq!(decode_flow(&[7, 0, 0, 0]), Some((7, &[][..])));
        assert_eq!(decode_flow(&[7, 0, 0]), None);
    }

    /// The flow stream's frames: the largest crosses whole and the next one after it, one over the cap is not sent, and
    /// a frame whose length is over the cap or that the stream's end cuts gives nothing (the caller stops the stream).
    #[tokio::test]
    async fn flow_stream_frames() {
        let big = vec![7u8; MAX_FLOW_PACKET];
        let mut wire = flow_frame(&big).expect("the largest frame");
        wire.extend(flow_frame(b"abcd").unwrap());
        let mut r = &wire[..];
        assert_eq!(read_flow(&mut r).await.as_deref(), Ok(&big[..]));
        assert_eq!(read_flow(&mut r).await.as_deref(), Ok(&b"abcd"[..]));
        assert_eq!(
            read_flow(&mut &flow_frame(b"abc").unwrap()[..]).await,
            Err("a 3-byte frame, under the 4-byte flow number".into())
        );
        let end = read_flow(&mut r).await.unwrap_err();
        assert!(
            end.starts_with("between frames: "),
            "the stream's end: {end}"
        );

        assert_eq!(
            flow_frame(&[0u8; MAX_FLOW_PACKET + 1]),
            None,
            "over the cap"
        );
        let mut over = u16::try_from(MAX_FLOW_PACKET + 1)
            .unwrap()
            .to_le_bytes()
            .to_vec();
        over.extend([0u8; 64]);
        assert_eq!(
            read_flow(&mut &over[..]).await,
            Err("a 65512-byte frame, over the 65511-byte cap".into())
        );

        let f = flow_frame(b"abcdef").unwrap();
        let cut = read_flow(&mut &f[..f.len() - 1]).await.unwrap_err();
        assert!(
            cut.starts_with("inside a 6-byte frame: "),
            "cut in the body: {cut}"
        );
        let cut = read_flow(&mut &f[..1]).await.unwrap_err();
        assert!(
            cut.starts_with("between frames: "),
            "cut in the length: {cut}"
        );
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

    /// A new guest reads the host's close as "the host closed the arcade" from a new host (code 6) and from a 0.6.1
    /// host (its endpoint close, code 0); the refusals and drops are not that. A 0.6.1 guest only acts on code 4, so
    /// code 6 must stay apart from it (it then dials again, as after code 0).
    #[test]
    fn the_hosts_close_is_told_apart() {
        use iroh::endpoint::{ApplicationClose, ConnectionError};
        let app = |code: u8| {
            ConnectionError::ApplicationClosed(ApplicationClose {
                error_code: code.into(),
                reason: Bytes::new(),
            })
        };
        assert!(closed_by_host(&app(CLOSE_ARCADE_CLOSED)));
        assert!(closed_by_host(&app(0)), "a 0.6.1 host's endpoint close");
        for code in [1, 3, CLOSE_NOT_IN_ROOM, CLOSE_NO_ANSWER, CLOSE_BAD_FLOWS] {
            assert!(!closed_by_host(&app(code)), "code {code}");
        }
        assert!(!closed_by_host(&ConnectionError::TimedOut));
        assert_ne!(CLOSE_ARCADE_CLOSED, CLOSE_NOT_IN_ROOM);
        assert!(
            bad_flows(&app(CLOSE_BAD_FLOWS)),
            "the host's close over a bad flow stream"
        );
        assert!(
            bad_flows(&ConnectionError::LocallyClosed),
            "this side's own"
        );
        assert!(!bad_flows(&app(CLOSE_ARCADE_CLOSED)));
    }

    /// The real closes, not made-up ones: a 0.6.1 host's stop (iroh's `Endpoint::close`, code 0) reads as the host
    /// closing the arcade; the guest's own stop (`LocallyClosed` on its side) does not.
    #[tokio::test]
    async fn real_closes_are_told_apart() {
        let ep = || {
            Endpoint::builder(presets::Minimal)
                .alpns(vec![ARCADE_ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .bind()
        };
        async fn link(guest: &Endpoint, host: &Endpoint) -> (Connection, Connection) {
            let (c, h) = tokio::join!(guest.connect(host.addr(), ARCADE_ALPN), async {
                host.accept().await.unwrap().await.unwrap()
            });
            (c.unwrap(), h)
        }
        let (host, guest) = (ep().await.unwrap(), ep().await.unwrap());
        let (c, _h) = link(&guest, &host).await;
        host.close().await;
        let why = c.closed().await;
        assert!(closed_by_host(&why), "{why:?}");

        let host = ep().await.unwrap();
        let (c, _h) = link(&guest, &host).await;
        guest.close().await;
        let why = c.closed().await;
        assert!(!closed_by_host(&why), "{why:?}");
        host.close().await;
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
