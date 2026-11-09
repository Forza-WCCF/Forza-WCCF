//! forza-arcade: a server's 2017-18 arcade on the arcade tunnel, the Launcher's HOST ARCADE without a seat of its own
//! (the hourly Universal Cup arcade, crates/linux-host `cups`). `host` keeps the arcade's room on a lobby desk and runs
//! the tunnel's host side, which dials the arcade's control, desk and relay on this machine's loopback for the guests
//! the room names. `probe` is a test seat: it joins a room and checks the arcade's desk and control answer through
//! the tunnel.
//!
//!     forza-arcade host --lobby HOST:PORT --key FILE --control PORT --desk PORT --relay PORT [--name NAME] [--public] [--relay-url URL]
//!     forza-arcade probe --lobby HOST:PORT --join CODE|ROOM-ID --key FILE [--relay-url URL]

use forza::online::lobby::{self, Created, Peers, Reserved};
use forza::service::Log;
use std::io::Read;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use vslink::arcade::{self, ArcadeConfig, Ports, Role, Verdict};

static STOP: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn on_stop() {
    unsafe extern "C" {
        fn signal(number: i32, handler: extern "C" fn(i32)) -> usize;
    }
    extern "C" fn stop(_: i32) {
        STOP.store(true, Ordering::Relaxed);
    }
    // SAFETY: libc's signal() with a handler that only stores to an atomic
    unsafe {
        signal(2, stop);
        signal(15, stop);
    }
}
#[cfg(not(unix))]
fn on_stop() {}

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("forza-arcade: {e}");
            ExitCode::from(2)
        }
    }
}

fn value<'a>(args: &'a [String], key: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == key)?;
    args.get(i + 1).map(String::as_str)
}

fn need<'a>(args: &'a [String], key: &str) -> Result<&'a str, String> {
    value(args, key).ok_or_else(|| format!("{key} is missing"))
}

fn port(args: &[String], key: &str) -> Result<u16, String> {
    need(args, key)?
        .parse()
        .map_err(|_| format!("{key} takes a port"))
}

fn run(args: &[String]) -> Result<(), String> {
    let lobby_at = need(args, "--lobby")?;
    let addr = lobby_at
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .ok_or_else(|| format!("--lobby {lobby_at}: not an address"))?;
    let key = PathBuf::from(need(args, "--key")?);
    let log: Log = Arc::new(|s: &str| println!("{s}"));
    let cfg = |role, ports| -> Result<ArcadeConfig, String> {
        let mut c = ArcadeConfig::public(role, ports, key.clone(), log.clone());
        if let Some(url) = value(args, "--relay-url") {
            c.set_relay(url, None)?;
        }
        Ok(c)
    };
    match args.first().map(String::as_str) {
        Some("host") => {
            let ports = Ports {
                control: port(args, "--control")?,
                desk: port(args, "--desk")?,
                relay: port(args, "--relay")?,
            };
            let name = value(args, "--name").unwrap_or("");
            host(
                addr,
                cfg(Role::Host, ports)?,
                name,
                args.iter().any(|a| a == "--public"),
            )
        }
        Some("probe") => {
            let own = arcade::id_text(
                &arcade::own_id(&key).map_err(|e| format!("{}: {e}", key.display()))?,
            );
            let join = need(args, "--join")?;
            let ticket = match lobby::join_arcade(addr, join, None)? {
                Reserved::Ticket { ticket, .. } => ticket,
                other => return Err(format!("JOIN {join}: {}", other.line().trim())),
            };
            let result = (|| {
                let host = match lobby::arcade_peers(addr, &ticket, Some(&own))? {
                    Some(Peers::Host(h)) => h,
                    other => return Err(format!("SHOP: not a guest answer ({other:?})")),
                };
                let id =
                    arcade::parse_id(&host).ok_or_else(|| format!("{host}: not a tunnel id"))?;
                probe(
                    cfg(
                        Role::Join(id),
                        Ports {
                            control: 0,
                            desk: 0,
                            relay: 0,
                        },
                    )?,
                    &log,
                )
            })();
            let _ = lobby::release(addr, &ticket);
            result
        }
        _ => Err("usage: forza-arcade host|probe (see the file's header)".into()),
    }
}

/// The room on the lobby desk, made again until the desk answers with one.
fn open_room(addr: SocketAddr, id: &str, public: bool, name: &str) -> Option<(String, String)> {
    let mut said = String::new();
    while !STOP.load(Ordering::Relaxed) {
        // no room seat for this host, so 8 guests fit; a desk before `CREATE seatless` answers ERR: the old room
        let made = lobby::create_seatless_arcade(addr, id, public, name)
            .or_else(|_| lobby::create_arcade(addr, id, None, public, name));
        let why = match made {
            Ok(Created::Room {
                id: room,
                code,
                ticket,
                ..
            }) => {
                println!(
                    "arcade room {room} on {addr}: code {code}, {}",
                    if public { "listed" } else { "not listed" }
                );
                return Some((room, ticket));
            }
            Ok(Created::Full) => "the desk is full".to_owned(),
            Err(e) => e,
        };
        if why != said {
            println!("no arcade room on {addr} ({why}); trying every 30 s");
            said = why;
        }
        for _ in 0..30 {
            if STOP.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    None
}

fn host(addr: SocketAddr, mut cfg: ArcadeConfig, name: &str, public: bool) -> Result<(), String> {
    on_stop();
    let id = arcade::id_text(
        &arcade::own_id(&cfg.key).map_err(|e| format!("{}: {e}", cfg.key.display()))?,
    );
    // the room first: its guests are the only ones the tunnel lets in
    let Some(first) = open_room(addr, &id, public, name) else {
        return Ok(());
    };
    let room = Arc::new(Mutex::new(first));
    let held = room.clone();
    // a guest says its id on the desk (`SHOP t ID`) before it dials, so a fresh ask names it
    cfg.admit = Some(Arc::new(move |guest: &vslink::core::EndpointId| {
        let ticket = held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1
            .clone();
        match lobby::arcade_peers(addr, &ticket, None) {
            Ok(Some(Peers::Guests(g))) if g.contains(&arcade::id_text(guest)) => Verdict::In,
            Ok(Some(Peers::Guests(_))) => Verdict::NotInRoom,
            _ => Verdict::NoAnswer,
        }
    }));
    let ports = cfg.ports;
    let link = arcade::spawn(cfg).map_err(|e| format!("the tunnel: {e}"))?;
    println!(
        "arcade tunnel {id}: control {}, desk {}, relay {} on loopback",
        ports.control, ports.desk, ports.relay
    );
    let guests = link.guests();
    let (mut seen, mut failing) = (0, false);
    while !STOP.load(Ordering::Relaxed) {
        for _ in 0..lobby::HEARTBEAT_EVERY {
            if STOP.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        let now = guests.load(Ordering::Relaxed);
        if now != seen {
            println!("guests connected: {now}");
            seen = now;
        }
        let ticket = room
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1
            .clone();
        match lobby::heartbeat(addr, &ticket) {
            Ok(true) => failing = false,
            Ok(false) => {
                println!("the desk no longer holds the room: a new one");
                match open_room(addr, &id, public, name) {
                    Some(r) => *room.lock().unwrap_or_else(PoisonError::into_inner) = r,
                    None => break,
                }
            }
            Err(e) if !failing => {
                println!("heartbeat: {e}; trying again");
                failing = true;
            }
            Err(_) => {}
        }
    }
    let (room_id, ticket) = room.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let _ = lobby::close(addr, &room_id, &ticket);
    println!("arcade room {room_id} closed");
    drop(link);
    Ok(())
}

/// The arcade's desk and control, reached through the tunnel's join side.
fn probe(cfg: ArcadeConfig, log: &Log) -> Result<(), String> {
    let link = arcade::spawn(cfg).map_err(|e| format!("the tunnel: {e}"))?;
    let local = |p: u16| SocketAddr::from(([127, 0, 0, 1], p));
    let rows = lobby::fetch(local(link.ports.desk), "2017-18")
        .map_err(|e| format!("the arcade's desk: {e}"))?;
    log(&format!(
        "the arcade's desk answered LOBBY through the tunnel ({} arcade rows)",
        rows.arcades.len()
    ));
    // the host dials control for this stream; a control that is not there closes it at once
    let mut c =
        TcpStream::connect(local(link.ports.control)).map_err(|e| format!("control: {e}"))?;
    c.set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut b = [0u8; 64];
    match c.read(&mut b) {
        Ok(0) => Err("control: the tunnel closed the connection (control not listening?)".into()),
        Ok(n) => {
            log(&format!("control answered {n} bytes through the tunnel"));
            Ok(())
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            log("control holds the connection through the tunnel (5 s, nothing sent yet)");
            Ok(())
        }
        Err(e) => Err(format!("control: {e}")),
    }
}
