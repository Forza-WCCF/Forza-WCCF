//! `forza-server`'s command line, shared by the Linux `forza-server` binary (`main.rs`) and, on Windows, by
//! `Launcher.exe server ...` (the 2017-18 kit ships no `forza-server.exe`). Every online service in one program:
//! `desk | relay | gmsvr | reflect | all`, and `market-table` (the Python
//! `server/seat_desk.py` and `relay.py` it replaced each took `LIFE_SECONDS [LOG_FILE]`, as `desk` and `relay` still
//! do). Ports are flags (`--port`, and `--desk-port` ... for `all`) so both editions share
//! one host; a flag wins over the Python's environment variables, which win over the defaults:
//! `WCCF_BIND` (0.0.0.0), `WCCF_BROKER_PORT` (20030), `WCCF_BROKER_SILENT` (75), `WCCF_CONTROL_LOG`, `WCCF_HOLD_FILE`,
//! `WCCF_HOLD_FW`, `WCCF_PROJECTOR_PORT` (20031), `WCCF_RELAY_PORT` (20040), `WCCF_RELAY_FORGET` (30). Exit 0 = ran its time, 1 = a port could not be bound.
//!
//! The 2010-11 kit ships the Windows build (`cargo xtask build` puts it in `programs\forza-server.exe`; its
//! `Launcher.exe server` starts `forza-server all`). A Linux build is manual: `cargo build --release -p forza-server`.

use crate::{Edition, Handle, Log, desk, file_log, init_clock, relay};
use clap::Args;
use clap::{Parser, Subcommand, ValueEnum};
use forza::online::sessions::Sessions;
use forza::online::{gamesvr, vs};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser)]
#[command(
    version,
    about = "The Forza-WCCF online server: seat desk, match relay, game server, reflect"
)]
struct Cli {
    /// Whose clients: the seat desk and the relay differ a little (2017-18: the first PC shows the projector, no
    /// in-game check, no hold, any address may take a seat on the relay)
    #[arg(long, value_enum, default_value = "2010-11", global = true)]
    edition: Ed,
    #[command(subcommand)]
    service: Service,
}

#[derive(Clone, Copy, ValueEnum)]
enum Ed {
    #[value(name = "2010-11")]
    E1011,
    #[value(name = "2017-18")]
    E1718,
}

#[derive(Subcommand)]
enum Service {
    /// The seat desk (TCP 20030)
    Desk {
        /// TCP port (else `WCCF_BROKER_PORT`, else 20030)
        #[arg(long)]
        port: Option<u16>,
        life: Option<f64>,
        log_file: Option<PathBuf>,
    },
    /// The match relay (UDP 20040)
    Relay {
        /// UDP port (else `WCCF_RELAY_PORT`, else 20040)
        #[arg(long)]
        port: Option<u16>,
        life: Option<f64>,
        log_file: Option<PathBuf>,
    },
    /// The 2017-18 game server (TCP, SQLite clubs and saves; needs `--edition 2017-18`). Binds `WCCF_BIND`, else
    /// 127.0.0.1 (not 0.0.0.0: it has no authentication). Refuses every client but loopback and the allowlist
    /// (`--allow`, `--allow-file`) unless `--allow-any`. 8 connections per client IP, request dumps off.
    Gmsvr {
        #[arg(long, default_value_t = 20060)]
        port: u16,
        #[command(flatten)]
        g: GmsvrOpts,
    },
    /// The 2017-18 VS reflect service alone (UDP; the echo that tells a PC its public address, and the NAT relay
    /// between probed addresses). `gmsvr --vs-public-ip` runs its own.
    Reflect {
        #[arg(long, default_value_t = 20070)]
        port: u16,
        life: Option<f64>,
        log_file: Option<PathBuf>,
    },
    /// The arcade's services together: the desk, the relay and (2017-18) the game server sharing its VS matcher with the
    /// desk's lobby. LOGS_DIR gets seat_broker.txt, match_relay.txt, gmsvr.txt
    All {
        #[arg(long)]
        desk_port: Option<u16>,
        #[arg(long)]
        relay_port: Option<u16>,
        /// 2017-18 only
        #[arg(long, default_value_t = 20060)]
        gmsvr_port: u16,
        /// 2017-18: no game server (the Launcher's server mode runs its own stand-in on the PC)
        #[arg(long)]
        no_gmsvr: bool,
        /// 2017-18: answer LAN arcade searches (UDP 20035, `forza::online::lan`) with this arcade name
        #[arg(long)]
        lan: Option<String>,
        #[command(flatten)]
        g: GmsvrOpts,
        /// 2017-18: Create-a-Team's market on the desk, with this value table (`market-table` writes it) and
        /// `market.db` beside `gamesvr.db`; without it every MARKET word answers that the market is not set up
        #[arg(long)]
        market_table: Option<PathBuf>,
        life: Option<f64>,
        logs_dir: Option<PathBuf>,
    },
    /// Writes the market's value table (card number, tier, TOTAL, series, secret mark) from an installed 2017-18
    /// `catalogue.tsv`, for `all --market-table`. The table is card data: it stays on this host and out of git
    MarketTable { catalogue: PathBuf, out: PathBuf },
}

/// The game server's switches, for `gmsvr` and `all`.
#[derive(Args)]
struct GmsvrOpts {
    /// Where `gamesvr.db` lives
    #[arg(long, default_value = "gmsvr-data")]
    store: PathBuf,
    /// The game folder: pre-registers its 2D-code cards (`player_data.bin`) and feeds `--skip-tutorial`
    #[arg(long)]
    game_data: Option<PathBuf>,
    /// A first COACH_LIST for an unknown Aime id makes a club from `cpu_50.dat`, skipping the tutorial
    #[arg(long)]
    skip_tutorial: bool,
    /// Clients allowed besides loopback: an IP or CIDR, repeatable
    #[arg(long)]
    allow: Vec<ipnet::IpNet>,
    /// File of allowed IPs/CIDRs, one per line (`#` comments); re-read when it changes
    #[arg(long)]
    allow_file: Option<PathBuf>,
    /// Accept every client (no allowlist)
    #[arg(long)]
    allow_any: bool,
    /// Keep the last 200 request payloads as `req_*.bin` in the store folder
    #[arg(long)]
    dump: bool,
    /// Direct VS (M3): the public IPv4 the clients reach the reflect service at; enables VS in the game server
    #[arg(long, requires = "reflect_port")]
    vs_public_ip: Option<std::net::Ipv4Addr>,
    /// UDP port of the reflect service (bound on `WCCF_BIND`)
    #[arg(long, requires = "vs_public_ip")]
    reflect_port: Option<u16>,
    /// UDP port of the loopback reflect address given to PCs that run vslink (`127.0.0.1:<port>`)
    #[arg(long, default_value_t = 20090)]
    vslink_port: u16,
    /// GAME_CTRL 1172 = 0: the control schedules no cup at its open times and never opens the Universal Championship;
    /// cups come only from its `schedule_option.conf` (the hourly arcade, `wccf-linux-server cups`)
    #[arg(long)]
    no_timed_cups: bool,
}

impl GmsvrOpts {
    fn vs(&self) -> Option<vs::VsConfig> {
        let (public_ip, reflect_port) = self.vs_public_ip.zip(self.reflect_port)?;
        Some(vs::VsConfig {
            public_ip,
            reflect_port,
            vslink_port: self.vslink_port,
        })
    }
}

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn bind() -> String {
    std::env::var("WCCF_BIND").unwrap_or_else(|_| "0.0.0.0".into())
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// `data\` as the Python derived it: the folder above the log file's folder (the cwd without a log file).
fn data_dir(log_file: Option<&Path>) -> PathBuf {
    log_file
        .and_then(|f| absolute(f).parent()?.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)] // sessions (#164) and store (#167) each add one
fn start_desk(
    ed: Edition,
    port: Option<u16>,
    log_file: Option<&Path>,
    vs: Option<Arc<vs::Matcher>>,
    club: Option<(PathBuf, Option<desk::Allow>)>,
    sessions: Option<Arc<Sessions>>,
    store: Option<&Path>,
    market_table: Option<&Path>,
    log: Log,
) -> std::io::Result<Handle> {
    let mut c = desk::DeskConfig::new(ed);
    c.sessions = sessions;
    // the market's store beside the game server's (`all`); a table that does not load leaves the market off
    if let (Some(table), Some(store)) = (market_table, store) {
        match crate::market::Market::open(table, &store.join("market.db")) {
            Ok(m) => {
                log(&format!(
                    "market on: {} cards, rules {}, catalogue {}",
                    m.table().len(),
                    m.table().rules,
                    m.table().catalogue
                ));
                c.market = Some(Arc::new(m));
            }
            Err(e) => log(&format!("market not set up: {e}")),
        }
    }
    // the operator's notice: beside the game server's store (`all`), else in data\ beside hold_projector.txt
    c.notice = c.notice.or_else(|| {
        Some(
            store
                .map_or_else(|| data_dir(log_file), Path::to_path_buf)
                .join("notice.json"),
        )
    });
    if let Some((db, allow)) = club {
        c.club_db = Some(db);
        c.club_public = true;
        c.club_allow = allow;
        c.club_cards = None; // a server has no cards of its own
    }
    c.bind = bind();
    c.port = port.unwrap_or_else(|| env("WCCF_BROKER_PORT", c.port));
    c.vs = vs;
    c.silent = Duration::from_secs_f64(env("WCCF_BROKER_SILENT", 75.0));
    c.projector_port = env("WCCF_PROJECTOR_PORT", c.projector_port);
    c.control_log = env_path("WCCF_CONTROL_LOG").or_else(|| {
        log_file.map(|f| {
            absolute(f)
                .parent()
                .unwrap_or(Path::new(""))
                .join("_dbg_control.txt")
        })
    });
    c.hold_file = Some(
        env_path("WCCF_HOLD_FILE").unwrap_or_else(|| data_dir(log_file).join("hold_projector.txt")),
    );
    c.hold_fw_test = env_path("WCCF_HOLD_FW");
    let port = c.port;
    desk::start(c, log.clone()).inspect_err(|e| {
        log(&format!(
            "cannot listen on TCP {port} ({e}) - is a seat broker already running?"
        ));
    })
}

fn start_relay(ed: Edition, port: Option<u16>, log: Log) -> std::io::Result<Handle> {
    let mut c = relay::RelayConfig::new(ed);
    c.bind = bind();
    c.port = port.unwrap_or_else(|| env("WCCF_RELAY_PORT", c.port));
    c.forget = Duration::from_secs_f64(env("WCCF_RELAY_FORGET", 30.0));
    c.stall_file = env_path("FORZA_RELAY_STALL_FILE"); // tests only (the Launcher passes no WCCF* variable on)
    let port = c.port;
    relay::start(c, log.clone()).inspect_err(|e| {
        log(&format!(
            "cannot listen on UDP {port} ({e}) - is a relay already running?"
        ));
    })
}

/// `--allow` entries plus the lines of `file`, which is re-read when its modified time changes.
fn allowlist(
    fixed: Vec<ipnet::IpNet>,
    file: Option<PathBuf>,
) -> Arc<dyn Fn(std::net::IpAddr) -> bool + Send + Sync> {
    type Cache = Option<(std::time::SystemTime, Vec<ipnet::IpNet>)>;
    let cache = Mutex::new(Cache::None);
    Arc::new(move |ip| {
        if ip.is_loopback() || fixed.iter().any(|n| n.contains(&ip)) {
            return true;
        }
        let Some(f) = &file else { return false };
        let Ok(m) = std::fs::metadata(f).and_then(|m| m.modified()) else {
            return false;
        };
        let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
        if c.as_ref().is_none_or(|(t, _)| *t != m) {
            let nets = std::fs::read_to_string(f).unwrap_or_default();
            let nets = nets
                .lines()
                .filter_map(|l| {
                    l.split('#')
                        .next()?
                        .trim()
                        .parse::<ipnet::IpNet>()
                        .ok()
                        .or_else(|| {
                            l.split('#')
                                .next()?
                                .trim()
                                .parse::<std::net::IpAddr>()
                                .ok()
                                .map(ipnet::IpNet::from)
                        })
                })
                .collect();
            *c = Some((m, nets));
        }
        c.as_ref()
            .is_some_and(|(_, v)| v.iter().any(|n| n.contains(&ip)))
    })
}

/// The game server's store for the clubs players bring (`all`), with its allowlist unless `--allow-any`.
fn club_store(g: &GmsvrOpts) -> (PathBuf, Option<desk::Allow>) {
    let allow =
        (!g.allow_any).then(|| desk::Allow(allowlist(g.allow.clone(), g.allow_file.clone())));
    (g.store.join("gamesvr.db"), allow)
}

/// Binds and starts the reflect service on `host:port`.
fn start_reflect(host: &str, port: u16, log: Log) -> std::io::Result<forza::service::Handle> {
    let sock = std::net::UdpSocket::bind((host, port)).inspect_err(|e| {
        log(&format!("cannot listen on UDP {host}:{port} ({e})"));
    })?;
    log(&format!("reflect on UDP {host}:{port}"));
    vs::spawn_reflect(sock, log)
}

/// The game server on `port` (and its reflect service when VS is on); `matcher` is shared with the desk in `all`.
fn start_gmsvr(
    g: &GmsvrOpts,
    port: u16,
    matcher: Option<Arc<vs::Matcher>>,
    sessions: Option<Arc<Sessions>>,
    log: Log,
) -> std::io::Result<Vec<forza::service::Handle>> {
    let host = std::env::var("WCCF_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let listener = std::net::TcpListener::bind((host.as_str(), port)).inspect_err(|e| {
        log(&format!("cannot listen on TCP {host}:{port} ({e})"));
    })?;
    log(&format!(
        "game server on {host}:{port}, store {}",
        g.store.display()
    ));
    let o = gamesvr::Options {
        allow: (!g.allow_any).then(|| allowlist(g.allow.clone(), g.allow_file.clone())),
        game: g.game_data.clone(),
        skip_tutorial: g.skip_tutorial,
        dump: g.dump,
        max_per_ip: 8,
        vs: g.vs(),
        matcher,
        sessions,
        no_timed_cups: g.no_timed_cups,
    };
    let mut handles = Vec::new();
    if let Some(cfg) = o.vs {
        log(&format!("VS on: reflect advertised as {}", cfg.public_ip));
        handles.push(start_reflect(&host, cfg.reflect_port, log.clone())?);
    }
    handles.push(gamesvr::spawn_opts(listener, g.store.clone(), log, o)?);
    Ok(handles)
}

fn need_1718(ed: Edition, what: &str) {
    if ed != Edition::E1718 {
        eprintln!("{what} is a 2017-18 service: pass --edition 2017-18");
        std::process::exit(2);
    }
}

/// Runs the server from `args` (the first one is the program name clap shows in its usage) until its life ends; never
/// returns.
pub fn main<I, T>(args: I) -> !
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    init_clock();
    let cli = Cli::parse_from(args);
    let ed = match cli.edition {
        Ed::E1011 => Edition::E1011,
        Ed::E1718 => Edition::E1718,
    };
    let day = 12.0 * 3600.0;
    // the game server and the reflect service are forza's services, the rest this crate's: kept apart, stopped alike
    let mut forza_svcs: Vec<std::io::Result<forza::service::Handle>> = Vec::new();
    let (life, services): (f64, Vec<std::io::Result<Handle>>) = match &cli.service {
        Service::Desk {
            port,
            life,
            log_file,
        } => (
            life.unwrap_or(day),
            vec![start_desk(
                ed,
                *port,
                log_file.as_deref(),
                None,
                None,
                None,
                None,
                None,
                file_log(log_file.clone()),
            )],
        ),
        Service::Relay {
            port,
            life,
            log_file,
        } => (
            life.unwrap_or(day),
            vec![start_relay(ed, *port, file_log(log_file.clone()))],
        ),
        Service::Gmsvr { port, g } => {
            need_1718(ed, "gmsvr");
            match start_gmsvr(g, *port, None, None, file_log(None)) {
                Ok(h) => forza_svcs.extend(h.into_iter().map(Ok)),
                Err(_) => std::process::exit(1),
            }
            (f64::INFINITY, vec![])
        }
        Service::Reflect {
            port,
            life,
            log_file,
        } => {
            need_1718(ed, "reflect");
            forza_svcs.push(start_reflect(&bind(), *port, file_log(log_file.clone())));
            (life.unwrap_or(day), vec![])
        }
        Service::All {
            desk_port,
            relay_port,
            gmsvr_port,
            no_gmsvr,
            lan,
            g,
            market_table,
            life,
            logs_dir,
        } => {
            if market_table.is_some() {
                need_1718(ed, "--market-table");
            }
            let file = |n: &str| logs_dir.as_ref().map(|d| d.join(n));
            let (desk_log, relay_log) = (file("seat_broker.txt"), file("match_relay.txt"));
            let gmsvr = ed == Edition::E1718 && !no_gmsvr;
            // one matcher: the game server pairs the 243 entries, the desk's LOBBY lists them as VS rooms
            let matcher = (gmsvr && g.vs().is_some()).then(Arc::<vs::Matcher>::default);
            // and one view of who is on the game server: the desk's lobby row counts its cards
            let sessions = gmsvr.then(Arc::<Sessions>::default);
            // players bring their clubs to the game server beside the desk (forza::online::club), unless
            // WCCF_CLUB_DB names another store; the game server's allowlist holds there too
            let club = (gmsvr && std::env::var_os("WCCF_CLUB_DB").is_none()).then(|| club_store(g));
            let services = vec![
                start_desk(
                    ed,
                    *desk_port,
                    desk_log.as_deref(),
                    matcher.clone(),
                    club,
                    sessions.clone(),
                    Some(&g.store),
                    market_table.as_deref(),
                    file_log(desk_log.clone()),
                ),
                start_relay(ed, *relay_port, file_log(relay_log)),
            ];
            if let Some(name) = lan {
                need_1718(ed, "--lan");
                let desk =
                    desk_port.unwrap_or_else(|| env("WCCF_BROKER_PORT", forza::online::SEAT_PORT));
                let log = file_log(desk_log.clone());
                forza_svcs.push(
                    forza::online::lan::serve(
                        &bind(),
                        forza::online::lan::PORT,
                        forza::online::lan::E1718,
                        desk,
                        name,
                        log.clone(),
                    )
                    .inspect_err(|e| {
                        log(&format!(
                            "cannot answer LAN searches on UDP {} ({e})",
                            forza::online::lan::PORT
                        ))
                    }),
                );
            }
            if gmsvr {
                match start_gmsvr(
                    g,
                    *gmsvr_port,
                    matcher,
                    sessions,
                    file_log(file("gmsvr.txt")),
                ) {
                    Ok(h) => forza_svcs.extend(h.into_iter().map(Ok)),
                    Err(e) => forza_svcs.push(Err(e)),
                }
            }
            (life.unwrap_or(day), services)
        }
        Service::MarketTable { catalogue, out } => {
            match crate::market::write_table(catalogue, out) {
                Ok(n) => println!("{}: {n} cards", out.display()),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
            std::process::exit(0);
        }
    };
    let failed = services.iter().any(Result::is_err) || forza_svcs.iter().any(Result::is_err);
    let running: Vec<Handle> = services.into_iter().flatten().collect();
    let forza_running: Vec<forza::service::Handle> = forza_svcs.into_iter().flatten().collect();
    if running.is_empty() && forza_running.is_empty() {
        std::process::exit(1);
    }
    std::thread::sleep(
        Duration::try_from_secs_f64(life).unwrap_or(Duration::from_secs(u32::MAX.into())),
    );
    drop((running, forza_running));
    std::process::exit(i32::from(failed));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_loopback_cidr_and_file() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        let f = std::env::temp_dir().join(format!("forza_allow_{}.txt", std::process::id()));
        std::fs::write(
            &f,
            "# players\n203.0.113.7\n198.51.100.0/24 # club\nnonsense\n",
        )
        .unwrap();
        let a = allowlist(vec!["192.0.2.0/30".parse().unwrap()], Some(f.clone()));
        assert!(a(ip("127.0.0.1")) && a(ip("192.0.2.3")));
        assert!(a(ip("203.0.113.7")) && a(ip("198.51.100.200")));
        assert!(!a(ip("203.0.113.8")) && !a(ip("192.0.2.4")));
        assert!(
            !allowlist(vec![], None)(ip("8.8.8.8")),
            "empty list refuses"
        );
        let _ = std::fs::remove_file(f);
    }

    /// `all`'s club store takes the game server's real allowlist: a stranger is refused, a listed address and
    /// loopback are let in, and only `--allow-any` leaves it open.
    #[test]
    fn the_club_store_follows_the_game_servers_allowlist() {
        #[derive(clap::Parser)]
        struct T {
            #[command(flatten)]
            g: GmsvrOpts,
        }
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        let opts = |args: &[&str]| <T as clap::Parser>::parse_from([&["t"][..], args].concat()).g;
        let (db, allow) = club_store(&opts(&["--store", "s", "--allow", "203.0.113.7/32"]));
        assert_eq!(db, Path::new("s").join("gamesvr.db"));
        let allow = allow.expect("an allowlist");
        assert!(allow.0(ip("203.0.113.7")) && allow.0(ip("127.0.0.1")));
        assert!(!allow.0(ip("203.0.113.8")), "a stranger is refused");
        assert!(club_store(&opts(&["--allow-any"])).1.is_none());
    }
}
