//! `forza-server`: the online server's three services in one program (the Python `server/seat_desk.py`, `relay.py`,
//! `log_inbox.py` it replaces each took `LIFE_SECONDS [LOG_FILE]`). Environment variables and defaults as the Python's:
//! `WCCF_BIND` (0.0.0.0), `WCCF_BROKER_PORT` (20030), `WCCF_BROKER_SILENT` (75), `WCCF_CONTROL_LOG`, `WCCF_HOLD_FILE`,
//! `WCCF_HOLD_FW`, `WCCF_RELAY_PORT` (20040), `WCCF_RELAY_FORGET` (30), `WCCF_INBOX_PORT` (20050),
//! `WCCF_INBOX_PER_HOUR` (6), `WCCF_INBOX_KEEP` (1 GB). Exit 0 = ran its time, 1 = a port could not be bound.
//!
//! The 2010-11 kit ships the Windows build (`cargo xtask build` puts it in `programs\forza-server.exe`; `Launcher.exe server`
//! starts `forza-server all`). A Linux build is manual: `cargo build --release -p forza-server`.

use clap::{Parser, Subcommand, ValueEnum};
use forza_server::{Edition, Handle, Log, desk, file_log, inbox, init_clock, relay};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser)]
#[command(
    version,
    about = "The Forza-WCCF online server: seat desk, match relay, log inbox"
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
        life: Option<f64>,
        log_file: Option<PathBuf>,
    },
    /// The match relay (UDP 20040)
    Relay {
        life: Option<f64>,
        log_file: Option<PathBuf>,
    },
    /// The log inbox (TCP 20050); FOLDER defaults to `player_logs` beside the log file's folder
    Inbox {
        life: f64,
        log_file: Option<PathBuf>,
        folder: Option<PathBuf>,
    },
    /// The 2017-18 game server stand-in (TCP, SQLite clubs and saves; needs `--edition 2017-18`). Binds `WCCF_BIND`,
    /// else 127.0.0.1 (not 0.0.0.0: it has no authentication). Refuses every client but loopback and the allowlist
    /// (`--allow`, `--allow-file`) unless `--allow-any`. 8 connections per client IP, request dumps off.
    Gmsvr {
        #[arg(long, default_value_t = 20060)]
        port: u16,
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
    },
    /// All three (2017-18: the desk and the relay), as `play.py server` started them; LOGS_DIR gets seat_broker.txt,
    /// match_relay.txt, log_inbox.txt
    All {
        life: Option<f64>,
        logs_dir: Option<PathBuf>,
    },
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

fn start_desk(ed: Edition, log_file: Option<&Path>, log: Log) -> std::io::Result<Handle> {
    let mut c = desk::DeskConfig::new(ed);
    c.bind = bind();
    c.port = env("WCCF_BROKER_PORT", c.port);
    c.silent = Duration::from_secs_f64(env("WCCF_BROKER_SILENT", 75.0));
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

fn start_relay(ed: Edition, log: Log) -> std::io::Result<Handle> {
    let mut c = relay::RelayConfig::new(ed);
    c.bind = bind();
    c.port = env("WCCF_RELAY_PORT", c.port);
    c.forget = Duration::from_secs_f64(env("WCCF_RELAY_FORGET", 30.0));
    let port = c.port;
    relay::start(c, log.clone()).inspect_err(|e| {
        log(&format!(
            "cannot listen on UDP {port} ({e}) - is a relay already running?"
        ));
    })
}

fn start_inbox(
    log_file: Option<&Path>,
    folder: Option<&Path>,
    log: Log,
) -> std::io::Result<Handle> {
    let folder = folder.map(Path::to_path_buf).unwrap_or_else(|| {
        let base = if log_file.is_some() {
            data_dir(log_file)
        } else {
            std::env::current_dir().unwrap_or_default()
        };
        base.join("player_logs")
    });
    let mut c = inbox::InboxConfig::new(folder);
    c.bind = bind();
    c.port = env("WCCF_INBOX_PORT", c.port);
    c.per_hour = env("WCCF_INBOX_PER_HOUR", c.per_hour);
    c.keep = env("WCCF_INBOX_KEEP", c.keep);
    let port = c.port;
    inbox::start(c, log.clone()).inspect_err(|e| {
        log(&format!(
            "cannot listen on TCP {port} ({e}) - is a log inbox already running?"
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

#[allow(clippy::too_many_arguments)]
fn start_gmsvr(
    port: u16,
    store: &Path,
    game: Option<&Path>,
    skip_tutorial: bool,
    dump: bool,
    allow: Option<Arc<dyn Fn(std::net::IpAddr) -> bool + Send + Sync>>,
    vs: Option<forza::standins::vs::VsConfig>,
    log: Log,
) -> std::io::Result<Vec<forza::standins::Handle>> {
    let host = std::env::var("WCCF_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let listener = std::net::TcpListener::bind((host.as_str(), port)).inspect_err(|e| {
        log(&format!("cannot listen on TCP {host}:{port} ({e})"));
    })?;
    log(&format!(
        "game server on {host}:{port}, store {}",
        store.display()
    ));
    let o = forza::standins::gamesvr::Options {
        allow,
        game: game.map(Path::to_path_buf),
        skip_tutorial,
        dump,
        max_per_ip: 8,
        vs,
    };
    let mut handles = Vec::new();
    if let Some(cfg) = vs {
        let sock =
            std::net::UdpSocket::bind((host.as_str(), cfg.reflect_port)).inspect_err(|e| {
                log(&format!(
                    "cannot listen on UDP {host}:{} ({e})",
                    cfg.reflect_port
                ));
            })?;
        log(&format!(
            "VS on: reflect {host}:{} advertised as {}",
            cfg.reflect_port, cfg.public_ip
        ));
        handles.push(forza::standins::vs::spawn_reflect(sock, log.clone())?);
    }
    handles.push(forza::standins::gamesvr::spawn_opts(
        listener,
        store.to_path_buf(),
        log,
        o,
    )?);
    Ok(handles)
}

fn main() {
    init_clock();
    let cli = Cli::parse();
    let ed = match cli.edition {
        Ed::E1011 => Edition::E1011,
        Ed::E1718 => Edition::E1718,
    };
    let day = 12.0 * 3600.0;
    let (life, services): (f64, Vec<std::io::Result<Handle>>) = match &cli.service {
        Service::Desk { life, log_file } => (
            life.unwrap_or(day),
            vec![start_desk(
                ed,
                log_file.as_deref(),
                file_log(log_file.clone()),
            )],
        ),
        Service::Relay { life, log_file } => (
            life.unwrap_or(day),
            vec![start_relay(ed, file_log(log_file.clone()))],
        ),
        Service::Inbox {
            life,
            log_file,
            folder,
        } => (
            *life,
            vec![start_inbox(
                log_file.as_deref(),
                folder.as_deref(),
                file_log(log_file.clone()),
            )],
        ),
        Service::Gmsvr {
            port,
            store,
            game_data,
            skip_tutorial,
            dump,
            allow,
            allow_file,
            allow_any,
            vs_public_ip,
            reflect_port,
            vslink_port,
        } => {
            if ed != Edition::E1718 {
                eprintln!("gmsvr is a 2017-18 service: pass --edition 2017-18");
                std::process::exit(2);
            }
            let Ok(_svc) = start_gmsvr(
                *port,
                store,
                game_data.as_deref(),
                *skip_tutorial,
                *dump,
                (!allow_any).then(|| allowlist(allow.clone(), allow_file.clone())),
                vs_public_ip
                    .zip(*reflect_port)
                    .map(|(public_ip, reflect_port)| forza::standins::vs::VsConfig {
                        public_ip,
                        reflect_port,
                        vslink_port: *vslink_port,
                    }),
                file_log(None),
            ) else {
                std::process::exit(1);
            };
            loop {
                std::thread::park();
            }
        }
        Service::All { life, logs_dir } => {
            let file = |n: &str| logs_dir.as_ref().map(|d| d.join(n));
            let (desk_log, relay_log, inbox_log) = (
                file("seat_broker.txt"),
                file("match_relay.txt"),
                file("log_inbox.txt"),
            );
            let mut services = vec![
                start_desk(ed, desk_log.as_deref(), file_log(desk_log.clone())),
                start_relay(ed, file_log(relay_log)),
            ];
            // 2017-18 has no log inbox, and its game server stand-in holds the inbox's port 20050
            if ed == Edition::E1011 {
                services.push(start_inbox(
                    inbox_log.as_deref(),
                    None,
                    file_log(inbox_log.clone()),
                ));
            }
            (life.unwrap_or(day), services)
        }
    };
    let failed = services.iter().any(Result::is_err);
    let running: Vec<Handle> = services.into_iter().flatten().collect();
    if running.is_empty() {
        std::process::exit(1);
    }
    std::thread::sleep(
        Duration::try_from_secs_f64(life).unwrap_or(Duration::from_secs(u32::MAX.into())),
    );
    drop(running);
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
}
