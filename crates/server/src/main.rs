//! `forza-server`: the online server's three services in one program (the Python `server/seat_desk.py`, `relay.py`,
//! `log_inbox.py` it replaces each took `LIFE_SECONDS [LOG_FILE]`). Environment variables and defaults as the Python's:
//! `WCCF_BIND` (0.0.0.0), `WCCF_BROKER_PORT` (20030), `WCCF_BROKER_SILENT` (75), `WCCF_CONTROL_LOG`, `WCCF_HOLD_FILE`,
//! `WCCF_HOLD_FW`, `WCCF_RELAY_PORT` (20040), `WCCF_RELAY_FORGET` (30), `WCCF_INBOX_PORT` (20050),
//! `WCCF_INBOX_PER_HOUR` (6), `WCCF_INBOX_KEEP` (1 GB). Exit 0 = ran its time, 1 = a port could not be bound.

use clap::{Parser, Subcommand, ValueEnum};
use forza_server::{Edition, Handle, Log, desk, file_log, inbox, init_clock, relay};
use std::path::{Path, PathBuf};
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
    /// All three, as `play.py server` starts them; LOGS_DIR gets seat_broker.txt, match_relay.txt, log_inbox.txt
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
        Service::All { life, logs_dir } => {
            let file = |n: &str| logs_dir.as_ref().map(|d| d.join(n));
            let (desk_log, relay_log, inbox_log) = (
                file("seat_broker.txt"),
                file("match_relay.txt"),
                file("log_inbox.txt"),
            );
            (
                life.unwrap_or(day),
                vec![
                    start_desk(ed, desk_log.as_deref(), file_log(desk_log.clone())),
                    start_relay(ed, file_log(relay_log)),
                    start_inbox(inbox_log.as_deref(), None, file_log(inbox_log.clone())),
                ],
            )
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
