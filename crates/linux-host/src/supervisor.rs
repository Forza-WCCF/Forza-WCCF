//! The server supervisor: Xvfb, the Wine prefix, control and its four match engines. 2010-11 (the default) also has a
//! built-in seat broker and match relay (off with `WCCF_EXTERNAL_DESK=1`, when `forza-server` serves TCP 20030 and UDP
//! 20040). `--edition 2017-18` runs a 2017-18 arcade from its own tree (own Wine prefix and display): the two editions use
//! the same shared-memory names, so they never share a prefix.
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

static STOP: AtomicBool = AtomicBool::new(false);
unsafe extern "C" {
    fn signal(number: i32, handler: extern "C" fn(i32)) -> usize;
}
extern "C" fn stop_signal(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}

#[derive(Default)]
struct Seats {
    players: [bool; 8],
    projector: Option<usize>,
}
impl Seats {
    fn take(&mut self, request: &str) -> Result<(usize, bool), String> {
        let words: Vec<_> = request.split_whitespace().collect();
        let seat = match words.as_slice() {
            ["WANT"] => self.players.iter().position(|&p| !p).ok_or("FULL")?,
            ["WANT", "SEAT", n] => {
                let n: usize = n.parse().map_err(|_| "ERR invalid seat")?;
                if !(1..=8).contains(&n) {
                    return Err("ERR invalid seat".into());
                }
                if self.players[n - 1] {
                    return Err(format!("TAKEN {n}"));
                }
                n - 1
            }
            _ => return Err("ERR say WANT or WANT SEAT n".into()),
        };
        self.players[seat] = true;
        let show = self.projector.is_none();
        if show {
            self.projector = Some(seat);
        }
        Ok((seat, show))
    }
    fn release(&mut self, seat: usize) {
        self.players[seat] = false;
        if self.projector == Some(seat) {
            self.projector = None;
        }
    }
}
fn line(reader: &mut BufReader<TcpStream>) -> io::Result<String> {
    let mut out = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Err(io::Error::other("connection closed"));
        }
        let n = buf
            .iter()
            .position(|&b| b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        if out.len() + n > 256 {
            return Err(io::Error::other("line too long"));
        }
        out.extend_from_slice(&buf[..n]);
        reader.consume(n);
        if out.last() == Some(&b'\n') {
            return String::from_utf8(out).map_err(io::Error::other);
        }
    }
}
/// Where the current run's game log is, from the ready record the supervisor wrote.
fn current_game_log(root: &Path) -> Option<PathBuf> {
    let ready = fs::read_to_string(root.join("run/ready")).ok()?;
    let dir = ready
        .split_whitespace()
        .find_map(|w| w.strip_prefix("logs="))?;
    Some(PathBuf::from(dir).join("game.log"))
}
/// The controller waits at PRJ_BOOT_WAIT until a projector connects when the server
/// option WAIT_PROJECTOR is 1 (measured 2026-10-07: still waiting after 150 s with no
/// projector, against ENTRY within 5 s with the option 0).
fn projector_state(root: &Path) -> String {
    let Some(log) = current_game_log(root) else {
        return "no run".into();
    };
    let Ok(bytes) = fs::read(&log) else {
        return "log unreadable".into();
    };
    let text = String::from_utf8_lossy(&bytes);
    if text.contains("state  PRJ_BOOT_WAIT(0) -> ENTRY(1)") {
        "connected - rounds running".into()
    } else if text.contains("CoStateManage::initialize") {
        "WAITING: controller holds at boot until a projector connects".into()
    } else {
        "boot not reached".into()
    }
}
fn lease(
    mut stream: TcpStream,
    seats: Arc<Mutex<Seats>>,
    root: PathBuf,
    seen: Arc<AtomicBool>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(75)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let peer = stream.peer_addr()?;
    let request = line(&mut reader)?;
    let result = seats.lock().unwrap().take(&request);
    match result {
        Err(e) => writeln!(stream, "{e}"),
        Ok((seat, show)) => {
            println!("seat {} leased to {peer}", seat + 1);
            if show {
                seen.store(true, Ordering::Relaxed);
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let _ = fs::write(
                    root.join("run/projector_seen.txt"),
                    format!("projector (seat {}) leased to {peer} at {secs}\n", seat + 1),
                );
                println!("projector connected: the controller can leave PRJ_BOOT_WAIT");
            }
            let result = (|| {
                writeln!(stream, "SEAT {} PROJECTOR {}", seat + 1, u8::from(show))?;
                loop {
                    if line(&mut reader)?.trim() != "PING" {
                        return Err(io::Error::other("expected PING"));
                    }
                    stream.write_all(b"PONG\n")?;
                }
            })();
            seats.lock().unwrap().release(seat);
            println!("seat {} released", seat + 1);
            result
        }
    }
}
fn broker(listener: TcpListener, root: PathBuf, seen: Arc<AtomicBool>) {
    let seats = Arc::new(Mutex::new(Seats::default()));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let seats = seats.clone();
                let (root, seen) = (root.clone(), seen.clone());
                thread::spawn(move || {
                    let _ = lease(stream, seats, root, seen);
                });
            }
            Err(e) => {
                eprintln!("broker failed: {e}");
                std::process::exit(1);
            }
        }
    }
}
#[derive(Default)]
struct Relay {
    seats: HashMap<u8, (SocketAddr, Instant)>,
}
impl Relay {
    fn packet(
        &mut self,
        data: &[u8],
        addr: SocketAddr,
        now: Instant,
    ) -> Option<(Vec<u8>, SocketAddr)> {
        if data.len() < 6 || &data[..4] != b"WRL1" || !addr.is_ipv4() {
            return None;
        }
        match data[4] {
            b'R' if data.len() == 6 && (1..=8).contains(&data[5]) => {
                self.seats.insert(data[5], (addr, now));
                None
            }
            b'D' if data.len() >= 7
                && data.len() <= 4103
                && (1..=8).contains(&data[5])
                && (1..=8).contains(&data[6]) =>
            {
                // The kit protocol has no token; use a private network, not public internet exposure.
                self.seats.insert(data[6], (addr, now));
                let &(target, last) = self.seats.get(&data[5])?;
                if now.duration_since(last) > Duration::from_secs(30) {
                    return None;
                }
                let SocketAddr::V4(from) = addr else {
                    return None;
                };
                let mut out = b"WRL1F".to_vec();
                out.push(data[6]);
                out.extend(from.ip().octets());
                out.extend(&data[7..]);
                Some((out, target))
            }
            // The panel's ping meter (kit 5.x _relay.py): its bytes straight back as "Q".
            b'P' if data.len() <= 64 => {
                let mut out = b"WRL1Q".to_vec();
                out.extend(&data[5..]);
                Some((out, addr))
            }
            _ => None,
        }
    }
}
fn relay(socket: UdpSocket) {
    let mut state = Relay::default();
    let mut buf = [0u8; 4200];
    loop {
        match socket.recv_from(&mut buf) {
            Ok((n, addr)) => {
                let now = Instant::now();
                state
                    .seats
                    .retain(|_, (_, last)| now.duration_since(*last) <= Duration::from_secs(30));
                if let Some((out, target)) = state.packet(&buf[..n], addr, now) {
                    let _ = socket.send_to(&out, target);
                }
            }
            Err(e) => {
                eprintln!("relay failed: {e}");
                std::process::exit(1);
            }
        }
    }
}
/// The two arcades. Each runs from its own `ROOT` (`WCCF_ROOT`), so each has its own Wine prefix and wineserver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edition {
    E1011,
    E1718,
}
impl Edition {
    /// `--edition 2010-11|2017-18` among the arguments after the command; 2010-11 when absent.
    fn from_args(args: &[String]) -> Result<Self, String> {
        let Some(i) = args.iter().position(|a| a == "--edition") else {
            return Ok(Self::E1011);
        };
        match args.get(i + 1).map(String::as_str) {
            Some("2010-11") => Ok(Self::E1011),
            Some("2017-18") => Ok(Self::E1718),
            other => Err(format!("--edition takes 2010-11 or 2017-18, not {other:?}")),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::E1011 => "2010-11",
            Self::E1718 => "2017-18",
        }
    }
    /// Xvfb's display; apart per edition so both arcades can run on one host.
    fn display(self) -> &'static str {
        match self {
            Self::E1011 => ":97",
            Self::E1718 => ":96",
        }
    }
    fn dll_overrides(self) -> &'static str {
        match self {
            Self::E1011 => "winmm,winmm_orig,wccfnet,netguard=n;mscoree,mshtml=",
            Self::E1718 => "winmm,winmm_orig,netguard=n;mscoree,mshtml=",
        }
    }
    /// 2017-18 has no helper to log a crash: Wine's own backtrace and the control's OutputDebugString go to game.log.
    fn wine_debug(self) -> &'static str {
        match self {
            Self::E1011 => "-all",
            Self::E1718 => "fixme-all,+debugstr,+pid,+timestamp",
        }
    }
    /// The line each hooked process's `mxhook_<pid>.log` must carry before the arcade counts as up (`native/hook`'s
    /// `EDITION`). 2010-11 is not checked here: the hook's own netguard check ends a process that is not guarded.
    fn hook_line(self) -> Option<&'static str> {
        match self {
            Self::E1011 => None,
            Self::E1718 => Some("edition: 2017-18 server"),
        }
    }
    /// The seat desk and match relay of 2017-18 are always `forza-server`'s; the built-in ones speak the 2010-11 protocol.
    fn external_desk(self, env_switch: bool) -> bool {
        self == Self::E1718 || env_switch
    }
    /// TCP port of control: fixed for 2010-11; 2017-18's tree says (`CONTROL_PORT`), as two arcades cannot share 20002.
    fn control_port(self, conf: &str) -> Result<u16, String> {
        match self {
            Self::E1011 => Ok(20002),
            // required, and not 20002: the 2010-11 arcade on this host holds that port, and ready must be this control's
            Self::E1718 => match conf_values(conf, "CONTROL_PORT").as_slice() {
                [v] => match v.parse::<u16>() {
                    Ok(20002) => {
                        Err("CONTROL_PORT=20002 is the 2010-11 arcade's port: use another".into())
                    }
                    Ok(p) if p > 0 => {
                        // the control's other ports in the same conf must differ from its listening port
                        for other in ["MATCH_PORT", "GAME_SVR_PORT"] {
                            if conf_values(conf, other).iter().any(|o| o.parse() == Ok(p)) {
                                return Err(format!("CONTROL_PORT={p} is also {other}"));
                            }
                        }
                        Ok(p)
                    }
                    _ => Err(format!("CONTROL_PORT={v} is not a port")),
                },
                [] => {
                    Err("runtime/local/ctrl_user_option.conf needs CONTROL_PORT (not 20002)".into())
                }
                _ => Err("CONTROL_PORT is set more than once".into()),
            },
        }
    }
}
/// Every `KEY=value` line of `ctrl_user_option.conf` (CRLF or LF), in order.
fn conf_values(conf: &str, key: &str) -> Vec<String> {
    conf.lines()
        .filter_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))
        .map(|v| v.trim().to_string())
        .collect()
}
/// A 2017-18 control must only reach this host's own game server, never the publisher's: GAME_SVR_IP is set once, to
/// `localhost`. A literal `127.0.0.1` would make the control use the network's own host (the publisher's server;
/// `crates/launcher/src/conf.rs` `game_server_host`), and the control's reader of duplicates is unknown.
fn game_server_is_local(conf: &str) -> bool {
    conf_values(conf, "GAME_SVR_IP") == ["localhost"]
}
/// Inodes of the IPv4 sockets listening on `port`, from `/proc/net/tcp`.
fn listen_inodes(tcp: &str, port: u16) -> Vec<u64> {
    tcp.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<_> = l.split_whitespace().collect();
            let local_port = u16::from_str_radix(f.get(1)?.rsplit(':').next()?, 16).ok()?;
            (f.get(3) == Some(&"0A") && local_port == port).then(|| f.get(9)?.parse().ok())?
        })
        .collect()
}
/// Whether one of `pids` (the prefix's processes, wineserver included: Wine keeps sockets there) holds a socket
/// listening on `port`: ready must be this arcade's control, not another arcade's.
fn owns_listener(pids: &[u32], port: u16) -> bool {
    // IPv4 and IPv6 sockets (Wine's may be either)
    let mut inodes = Vec::new();
    for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
        inodes.extend(listen_inodes(
            &fs::read_to_string(file).unwrap_or_default(),
            port,
        ));
    }
    pids.iter().any(|pid| {
        fs::read_dir(format!("/proc/{pid}/fd"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|fd| {
                fs::read_link(fd.path()).is_ok_and(|l| {
                    let l = l.to_string_lossy();
                    inodes.iter().any(|i| l == format!("socket:[{i}]"))
                })
            })
    })
}
/// What the hook logs of a start show.
#[derive(Debug, PartialEq, Eq)]
enum HookLogs {
    /// this many logs carry the edition line, none refused
    Ok(usize),
    /// a log says REFUSED or REFUSING: that process is not guarded
    Refused(String),
    /// fewer logs than `need` carry the line so far
    Short(usize),
}
/// The per-process guard check: `need` `mxhook_*.log` files in `dir` (control and four engines) say `line`, and none says
/// `REFUSED` or `REFUSING TO RUN`.
fn hook_logs(dir: &Path, line: &str, need: usize) -> io::Result<HookLogs> {
    let mut ok = 0;
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if !(name.starts_with("mxhook_") && name.ends_with(".log")) {
            continue;
        }
        // a transient read error must not stop a healthy arcade: the log counts as not OK, and the next check retries
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("hook log {} unreadable: {e}", path.display());
                continue;
            }
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if text.contains("REFUSED") || text.contains("REFUSING") {
            return Ok(HookLogs::Refused(name));
        }
        ok += usize::from(text.contains(line));
    }
    Ok(if ok >= need {
        HookLogs::Ok(ok)
    } else {
        HookLogs::Short(ok)
    })
}
/// Waits up to `seconds` for [`hook_logs`] to pass; a refusal fails at once.
fn wait_hook_logs(
    dir: &Path,
    line: &str,
    need: impl Fn() -> usize,
    seconds: u64,
) -> io::Result<usize> {
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        let need = need();
        match hook_logs(dir, line, need)? {
            HookLogs::Ok(n) => return Ok(n),
            HookLogs::Refused(name) => {
                return Err(io::Error::other(format!(
                    "{name} says the process was refused by the guard"
                )));
            }
            HookLogs::Short(n) if Instant::now() >= end || STOP.load(Ordering::Relaxed) => {
                return Err(io::Error::other(format!(
                    "only {n} hook logs say \"{line}\" (need {need}) in {}",
                    dir.display()
                )));
            }
            HookLogs::Short(_) => thread::sleep(Duration::from_millis(500)),
        }
    }
}
/// The periodic guard check: a refusal fails at once; too few logs for the live processes fail on the third check in a
/// row (a process just started has not logged yet).
/// shortcut: hook logs are named by Windows pids, which /proc does not show, so processes are matched by count (a live
/// process without a log shows as a missing log); upgrade by reading the pid line when a Wine pid map exists.
fn guard_step(dir: &Path, line: &str, need: usize, short: &mut u32) -> io::Result<()> {
    match hook_logs(dir, line, need)? {
        HookLogs::Ok(_) => *short = 0,
        HookLogs::Refused(name) => {
            return Err(io::Error::other(format!(
                "{name} says the process was refused by the guard"
            )));
        }
        HookLogs::Short(n) => {
            *short += 1;
            if *short >= 3 {
                return Err(io::Error::other(format!(
                    "{need} guarded processes run but only {n} hook logs say \"{line}\""
                )));
            }
        }
    }
    Ok(())
}
/// The server tree (`runtime/`, `wineprefix/`, `run/`, `logs/`, `evidence/`): `WCCF_ROOT`, else three folders above the
/// binary (`ROOT/target/release/wccf-linux-server`).
fn root() -> io::Result<PathBuf> {
    if let Some(r) = std::env::var_os("WCCF_ROOT").filter(|r| !r.is_empty()) {
        return Ok(PathBuf::from(r));
    }
    std::env::current_exe()?
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("binary must be in ROOT/target/release"))
}
/// `sddy_maps.exe` under Wine. Its working directory is in the tree, as `verify_prefix` requires of every prefix process.
fn sddy_command(root: &Path, wine_env: &[(&str, String)]) -> Command {
    let mut maps = Command::new(wine_tool(root, "wine"));
    maps.envs(wine_env.iter().map(|(k, v)| (k, v)))
        .arg(root.join("run/sddy_maps.exe"))
        .arg(winpath(&root.join("runtime")))
        .current_dir(root.join("runtime"));
    maps
}
fn winpath(path: &Path) -> String {
    format!("Z:{}", path.to_string_lossy().replace('/', "\\"))
}
fn prefix_processes(root: &Path) -> Vec<u32> {
    let prefix = format!("WINEPREFIX={}", root.join("wineprefix").display());
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
            let env = fs::read(entry.path().join("environ")).ok()?;
            env.split(|&b| b == 0)
                .any(|s| s == prefix.as_bytes())
                .then_some(pid)
        })
        .collect()
}
fn engine_pids(root: &Path) -> Vec<u32> {
    prefix_processes(root)
        .into_iter()
        .filter(|pid| {
            fs::read(format!("/proc/{pid}/comm"))
                .unwrap_or_default()
                .starts_with(b"match_Release")
        })
        .collect()
}
fn match_count(root: &Path) -> usize {
    engine_pids(root).len()
}
/// Guarded processes a 2017-18 arcade must have logged: control and its four match engines.
const GUARDED: usize = 5;
fn record_engine_health(root: &Path, engines: usize) -> io::Result<()> {
    if engines != 4 && root.join("run/ready").exists() {
        fs::remove_file(root.join("run/ready"))?;
        fs::write(
            root.join("run/degraded"),
            format!("only {engines}/4 match engines; no automatic restart during player session\n"),
        )?;
        eprintln!("DEGRADED: {engines}/4 match engines; keeping connected sessions untouched");
    }
    Ok(())
}
fn verify_prefix(root: &Path) -> io::Result<()> {
    for pid in prefix_processes(root) {
        let proc = PathBuf::from(format!("/proc/{pid}"));
        let exe = match fs::read_link(proc.join("exe")) {
            Ok(path) => path,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        let cwd = fs::read_link(proc.join("cwd"))?;
        // Debian 13 starts wineserver64 beside the 32-bit loader, its server dir under /tmp/wine-XXXXXX/
        let wine = [
            "/usr/lib/wine/wine",
            "/usr/lib/wine/wineserver32",
            "/usr/lib/wine/wineserver64",
        ];
        let boxed = exe == root.join("tools/box64");
        let server_dir = cwd.to_string_lossy();
        let allowed = cwd.starts_with(root)
            || ((exe == Path::new(wine[1]) || exe == Path::new(wine[2]) || boxed)
                && ((server_dir.starts_with("/run/user/")
                    && server_dir.contains("/wine/server-"))
                    || ((server_dir.starts_with("/tmp/.wine-")
                        || server_dir.starts_with("/tmp/wine-"))
                        && server_dir.contains("/server-"))));
        if (!wine.iter().any(|s| exe == Path::new(s)) && !boxed) || !allowed {
            return Err(io::Error::other(format!(
                "refusing cleanup of unowned pid={pid} exe={} cwd={}",
                exe.display(),
                cwd.display()
            )));
        }
        println!(
            "verified Wine process pid={pid} exe={} cwd={}",
            exe.display(),
            cwd.display()
        );
    }
    Ok(())
}
fn alive(pid: u32, binary: &Path) -> bool {
    fs::read_link(format!("/proc/{pid}/exe")).ok().as_deref() == Some(binary)
}
fn snapshot(root: &Path, label: &str) -> io::Result<()> {
    let out = Command::new("ip").args(["-4", "route"]).output()?;
    fs::write(
        root.join("evidence").join(format!("routes-{label}.txt")),
        out.stdout,
    )
}
fn spawn_log(cmd: &mut Command, path: &Path) -> io::Result<Child> {
    let log = OpenOptions::new().create_new(true).write(true).open(path)?;
    cmd.stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .stdin(Stdio::null())
        .spawn()
}
fn wine_tool(root: &Path, name: &str) -> PathBuf {
    let private = root.join("tools/bin").join(name);
    if private.exists() {
        private
    } else {
        PathBuf::from(name)
    }
}
fn run_bounded(cmd: &mut Command, log: &Path, seconds: u64) -> io::Result<()> {
    let mut child = spawn_log(cmd, log)?;
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "command failed; see {}",
                    log.display()
                )))
            };
        }
        if Instant::now() >= end || STOP.load(Ordering::Relaxed) {
            child.kill()?;
            let _ = child.wait();
            return Err(io::Error::other(format!(
                "command timed out/stopped; see {}",
                log.display()
            )));
        }
        thread::sleep(Duration::from_millis(100));
    }
}
fn serve(root: &Path, edition: Edition) -> io::Result<()> {
    // SAFETY: libc's signal() with a handler that only stores to an atomic, which is async-signal-safe
    unsafe {
        signal(2, stop_signal);
        signal(15, stop_signal);
    }
    let mut lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("run/server.pid"))?;
    writeln!(lock, "{}", std::process::id())?;
    let mut children: Vec<Child> = Vec::new();
    let result = (|| {
        let check = Command::new("sha256sum")
            .args(["-c", "evidence/runtime.sha256"])
            .current_dir(root)
            .output()?;
        if !check.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&check.stdout).into_owned(),
            ));
        }
        // 2017-18: the tree's conf names control's port, and must point its game server at this host
        let conf = if edition == Edition::E1718 {
            let text = fs::read_to_string(root.join("runtime/local/ctrl_user_option.conf"))?;
            if !game_server_is_local(&text) {
                return Err(io::Error::other(
                    "runtime/local/ctrl_user_option.conf: GAME_SVR_IP must be localhost",
                ));
            }
            text
        } else {
            String::new()
        };
        let control_port = edition.control_port(&conf).map_err(io::Error::other)?;
        snapshot(root, "before")?;
        // WCCF_EXTERNAL_DESK=1: the seat desk and match relay run outside (`forza-server desk` and `relay`
        // beside the projector relay), so this supervisor leaves TCP 20030 and UDP 20040 to them
        let external =
            edition.external_desk(std::env::var_os("WCCF_EXTERNAL_DESK").is_some_and(|v| v == "1"));
        let listener = if external {
            None
        } else {
            Some(TcpListener::bind("0.0.0.0:20030")?)
        };
        let udp = if external {
            None
        } else {
            Some(UdpSocket::bind("0.0.0.0:20040")?)
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let logs = root.join("logs").join(stamp.to_string());
        fs::create_dir_all(&logs)?;
        let display = edition.display();
        let mut xvfb = Command::new(root.join("tools/usr/bin/Xvfb"));
        xvfb.current_dir(root)
            .args([
                display,
                "-screen",
                "0",
                "1024x768x24",
                "-nolisten",
                "tcp",
                "-noreset",
                "-ac",
            ])
            .env(
                "LD_LIBRARY_PATH",
                root.join(if cfg!(target_arch = "aarch64") {
                    "tools/usr/lib/aarch64-linux-gnu"
                } else {
                    "tools/usr/lib/x86_64-linux-gnu"
                }),
            )
            .env("XKB_BINDIR", root.join("tools/usr/bin"))
            .args([
                "-xkbdir",
                &root.join("tools/usr/share/X11/xkb").display().to_string(),
            ]);
        children.push(spawn_log(&mut xvfb, &logs.join("xvfb.log"))?);
        let mut env = Command::new(wine_tool(root, "wineboot"));
        let private_wine = root.join("tools/bin/wine").exists();
        let wine_env = [
            ("WINEPREFIX", root.join("wineprefix").display().to_string()),
            (
                "WINEARCH",
                if private_wine { "win64" } else { "win32" }.into(),
            ),
            ("DISPLAY", display.into()),
            ("WINEDEBUG", edition.wine_debug().into()),
            ("WINEDLLOVERRIDES", edition.dll_overrides().into()),
            ("MXHOOK_LOG_DIR", winpath(&logs)),
            ("MXHOOK_NET_IP", "127.0.0.1".into()),
            (
                "WINESERVER",
                wine_tool(root, "wineserver").display().to_string(),
            ),
        ];
        // every Wine process runs with its working directory in the tree, or verify_prefix refuses cleanup at stop
        env.envs(wine_env.clone()).args(["-u"]).current_dir(root);
        run_bounded(&mut env, &logs.join("wineboot.log"), 45)?;
        if edition == Edition::E1718 {
            // no helper catches a crash: winedbg prints its backtrace into game.log, with no dialog on the display
            let mut reg = Command::new(wine_tool(root, "wine"));
            reg.envs(wine_env.clone()).args([
                "reg",
                "add",
                r"HKCU\Software\Wine\WineDbg",
                "/v",
                "ShowCrashDialog",
                "/t",
                "REG_DWORD",
                "/d",
                "0",
                "/f",
            ]);
            reg.current_dir(root);
            run_bounded(&mut reg, &logs.join("wine-reg.log"), 20)?;
        }
        let mut guard = Command::new(wine_tool(root, "wine"));
        guard
            .envs(wine_env.clone())
            .arg(root.join("runtime/test_wccfnet.exe"))
            // the guard's path: the repository's test_wccfnet.exe needs it, older builds ignore it
            .arg(winpath(&root.join("runtime/wccfnet.dll")))
            .current_dir(root.join("runtime"));
        run_bounded(&mut guard, &logs.join("guard-test.log"), 20)?;
        let mut helper = Command::new(wine_tool(root, "wine"));
        match edition {
            Edition::E1011 => helper
                .envs(wine_env)
                .arg(root.join("run/win32-helper.exe"))
                .arg(winpath(&root.join("runtime"))),
            Edition::E1718 => {
                // control waits 60 s for the SDDY_* shared memory the Launcher makes; sddy_maps.exe makes it
                // in this prefix and holds it until the prefix stops
                let mut maps = sddy_command(root, &wine_env);
                let maps_log = logs.join("sddy-maps.log");
                children.push(spawn_log(&mut maps, &maps_log)?);
                let end = Instant::now() + Duration::from_secs(60);
                while !fs::read_to_string(&maps_log)
                    .is_ok_and(|t| t.lines().any(|l| l == "holding"))
                {
                    if children
                        .iter_mut()
                        .any(|p| p.try_wait().ok().flatten().is_some())
                        || Instant::now() >= end
                        || STOP.load(Ordering::Relaxed)
                    {
                        return Err(io::Error::other(format!(
                            "sddy_maps did not hold the mappings; see {}",
                            maps_log.display()
                        )));
                    }
                    thread::sleep(Duration::from_millis(250));
                }
                helper.envs(wine_env).arg("control_Release.exe")
            }
        }
        .current_dir(root.join("runtime"));
        children.push(spawn_log(&mut helper, &logs.join("game.log"))?);
        let deadline = Instant::now() + Duration::from_secs(180);
        let mut foreign = false;
        loop {
            if children
                .iter_mut()
                .any(|p| p.try_wait().ok().flatten().is_some())
            {
                return Err(io::Error::other("server/display exited; inspect logs"));
            }
            let matches = match_count(root);
            if matches >= 4
                && TcpStream::connect_timeout(
                    &SocketAddr::from(([127, 0, 0, 1], control_port)),
                    Duration::from_millis(200),
                )
                .is_ok()
            {
                if edition == Edition::E1011 || owns_listener(&prefix_processes(root), control_port)
                {
                    break;
                }
                foreign = true;
            }
            if Instant::now() >= deadline
                || STOP.load(Ordering::Relaxed)
                || root.join("run/stop").exists()
            {
                return Err(io::Error::other(if foreign {
                    format!(
                        "port {control_port} listened on, but not by this prefix (another arcade's control?)"
                    )
                } else {
                    "controller/four match engines not ready within 180s or stop requested".into()
                }));
            }
            thread::sleep(Duration::from_millis(500));
        }
        // the guard, per process: control and four engines each log the edition, none refused
        if let Some(line) = edition.hook_line() {
            let n = wait_hook_logs(&logs, line, || GUARDED, 30)?;
            println!("guard: {n} hook logs say \"{line}\", none refused");
        }
        let projector_seen = Arc::new(AtomicBool::new(false));
        let broker_root = root.to_path_buf();
        let broker_seen = projector_seen.clone();
        if let Some(listener) = listener {
            thread::spawn(move || broker(listener, broker_root, broker_seen));
        }
        if let Some(udp) = udp {
            thread::spawn(move || relay(udp));
        }
        fs::write(
            root.join("run/ready"),
            format!("ready pid={} logs={}\n", std::process::id(), logs.display()),
        )?;
        if edition == Edition::E1718 {
            println!(
                "READY {}: TCP {control_port} controller; logs {}",
                edition.name(),
                logs.display()
            );
        } else {
            println!(
                "READY: TCP 20002 controller, TCP 20030 seats, UDP 20040 relay; logs {}",
                logs.display()
            );
        }
        let mut waited: u64 = 0;
        let mut short = 0;
        while !root.join("run/stop").exists() && !STOP.load(Ordering::Relaxed) {
            if children
                .iter_mut()
                .any(|p| p.try_wait().ok().flatten().is_some())
            {
                return Err(io::Error::other("server process exited"));
            }
            record_engine_health(root, match_count(root))?;
            if let Some(line) = edition.hook_line().filter(|_| waited.is_multiple_of(10)) {
                guard_step(&logs, line, GUARDED, &mut short)?;
                // surplus engines (a leftover of an earlier run) are reported, not counted against the guard
                let pids = engine_pids(root);
                if pids.len() > 4 {
                    let msg = format!("{} match engines (expected 4), pids {pids:?}\n", pids.len());
                    eprintln!("DEGRADED: {msg}");
                    fs::write(root.join("run/degraded"), msg)?;
                }
            }
            if !external && !projector_seen.load(Ordering::Relaxed) && waited.is_multiple_of(60) {
                eprintln!(
                    "WAITING for a projector ({}): start the projector client now; it needs \
                     the server up first. WAIT_PROJECTOR=0 in runtime/local/ctrl_user_option.conf \
                     runs the server without one.",
                    projector_state(root)
                );
            }
            waited += 1;
            thread::sleep(Duration::from_secs(1));
        }
        Ok(())
    })();
    // Prefix identity is checked immediately before prefix-scoped cleanup. No name-based kill.
    if !prefix_processes(root).is_empty() {
        verify_prefix(root)?;
        let _ = Command::new(wine_tool(root, "wineserver"))
            .env("WINEPREFIX", root.join("wineprefix"))
            .arg("-k")
            .status();
    }
    for child in &mut children {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = snapshot(root, "after");
    let _ = fs::remove_file(root.join("run/ready"));
    let _ = fs::remove_file(root.join("run/server.pid"));
    let _ = fs::remove_file(root.join("run/degraded"));
    let _ = fs::remove_file(root.join("run/stop"));
    result
}
pub fn main() -> io::Result<()> {
    let root = root()?;
    let binary = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let edition = Edition::from_args(&args).map_err(io::Error::other)?;
    match args.first().map_or("status", String::as_str) {
        "serve" => serve(&root, edition),
        "start" => {
            if let Ok(pid) = fs::read_to_string(root.join("run/server.pid")) {
                if alive(pid.trim().parse().unwrap_or(0), &binary) {
                    return Err(io::Error::other("already running"));
                }
                fs::remove_file(root.join("run/server.pid"))?;
            }
            let _ = fs::remove_file(root.join("run/ready"));
            let _ = fs::remove_file(root.join("run/degraded"));
            let _ = fs::remove_file(root.join("run/projector_seen.txt"));
            let _ = fs::remove_file(root.join("run/stop"));
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("logs/supervisor.log"))?;
            let mut child = Command::new(&binary)
                .arg("serve")
                .args(if edition == Edition::E1718 {
                    &["--edition", "2017-18"][..]
                } else {
                    &[]
                })
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log))
                .spawn()?;
            let deadline = Instant::now() + Duration::from_secs(270);
            loop {
                if root.join("run/ready").is_file() {
                    print!("{}", fs::read_to_string(root.join("run/ready"))?);
                    return Ok(());
                }
                if child.try_wait()?.is_some() {
                    return Err(io::Error::other("startup failed; read logs/supervisor.log"));
                }
                if Instant::now() >= deadline {
                    fs::write(root.join("run/stop"), "startup timeout\n")?;
                    return Err(io::Error::other("startup timed out; stop requested"));
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
        "stop" => {
            let pid: u32 = fs::read_to_string(root.join("run/server.pid"))?
                .trim()
                .parse()
                .map_err(io::Error::other)?;
            if !alive(pid, &binary) {
                return Err(io::Error::other(
                    "refusing stale PID; no verified supervisor",
                ));
            }
            fs::write(root.join("run/stop"), "operator stop\n")?;
            for _ in 0..30 {
                if !alive(pid, &binary) {
                    println!("stopped");
                    return Ok(());
                }
                thread::sleep(Duration::from_secs(1));
            }
            Err(io::Error::other("stop still in progress"))
        }
        "status" => {
            let pid = fs::read_to_string(root.join("run/server.pid"))
                .unwrap_or_default()
                .trim()
                .parse()
                .unwrap_or(0);
            println!(
                "supervisor pid={pid} alive={} ready={}",
                alive(pid, &binary),
                alive(pid, &binary) && root.join("run/ready").exists() && match_count(&root) == 4
            );
            println!("match engines: {}/4", match_count(&root));
            println!("projector: {}", projector_state(&root));
            if let Ok(reason) = fs::read_to_string(root.join("run/degraded")) {
                print!("DEGRADED: {reason}");
            }
            println!("Wine-prefix processes: {:?}", prefix_processes(&root));
            Ok(())
        }
        _ => Err(io::Error::other(
            "usage: wccf-linux-server start|stop|status|serve [--edition 2010-11|2017-18]",
        )),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kit_protocols() {
        let root = std::env::temp_dir().join(format!("wccf-health-check-{}", std::process::id()));
        fs::create_dir_all(root.join("run")).unwrap();
        fs::write(root.join("run/ready"), "fixture").unwrap();
        record_engine_health(&root, 4).unwrap();
        assert!(root.join("run/ready").exists());
        assert!(!root.join("run/degraded").exists());
        record_engine_health(&root, 3).unwrap();
        assert!(!root.join("run/ready").exists());
        assert!(
            fs::read_to_string(root.join("run/degraded"))
                .unwrap()
                .contains("3/4")
        );
        record_engine_health(&root, 4).unwrap();
        assert!(!root.join("run/ready").exists()); // no dishonest automatic recovery
        // projector_state reads the run's own log; waiting versus started must be distinguishable
        let logdir = root.join("logs/1");
        fs::create_dir_all(&logdir).unwrap();
        fs::write(
            root.join("run/ready"),
            format!("ready pid=1 logs={}\n", logdir.display()),
        )
        .unwrap();
        fs::write(
            logdir.join("game.log"),
            "CoStateManage::initialize() : ENTRY:40sec\n",
        )
        .unwrap();
        assert!(projector_state(&root).starts_with("WAITING"));
        fs::write(
            logdir.join("game.log"),
            "CoStateManage::initialize()\nstate  PRJ_BOOT_WAIT(0) -> ENTRY(1)\n",
        )
        .unwrap();
        assert!(projector_state(&root).starts_with("connected"));
        fs::remove_dir_all(root).unwrap();
        let mut seats = Seats::default();
        assert_eq!(seats.take("WANT\n").unwrap(), (0, true));
        assert_eq!(seats.take("WANT SEAT 1").unwrap_err(), "TAKEN 1");
        assert!(seats.take("WANT SEAT 9").is_err());
        for n in 1..8 {
            assert_eq!(seats.take("WANT").unwrap(), (n, false));
        }
        assert_eq!(seats.take("WANT").unwrap_err(), "FULL");
        seats.release(0);
        assert_eq!(seats.take("WANT").unwrap(), (0, true));
        let mut r = Relay::default();
        let now = Instant::now();
        let a: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        let b: SocketAddr = "127.0.0.1:5678".parse().unwrap();
        assert!(r.packet(b"garbage", a, now).is_none());
        assert_eq!(
            r.packet(b"WRL1Ptok", a, now).unwrap(),
            (b"WRL1Qtok".to_vec(), a)
        );
        assert!(r.packet(b"WRL1D\x02\x01lost", a, now).is_none());
        assert!(r.packet(b"WRL1R\x02", b, now).is_none());
        assert_eq!(
            r.packet(b"WRL1D\x02\x01hello", a, now).unwrap(),
            (b"WRL1F\x01\x7f\x00\x00\x01hello".to_vec(), b)
        );
        assert!(
            r.packet(b"WRL1D\x02\x01old", a, now + Duration::from_secs(31))
                .is_none()
        );
    }
    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn edition_selection() {
        assert_eq!(Edition::from_args(&args(&["start"])), Ok(Edition::E1011));
        assert_eq!(
            Edition::from_args(&args(&["start", "--edition", "2010-11"])),
            Ok(Edition::E1011)
        );
        assert_eq!(
            Edition::from_args(&args(&["serve", "--edition", "2017-18"])),
            Ok(Edition::E1718)
        );
        assert!(Edition::from_args(&args(&["start", "--edition"])).is_err());
        assert!(Edition::from_args(&args(&["start", "--edition", "2018"])).is_err());
    }
    /// 2010-11 keeps today's values exactly: display, Wine environment, port, desk, and no hook-log gate.
    #[test]
    fn e1011_is_unchanged() {
        let e = Edition::E1011;
        assert_eq!(e.display(), ":97");
        assert_eq!(
            e.dll_overrides(),
            "winmm,winmm_orig,wccfnet,netguard=n;mscoree,mshtml="
        );
        assert_eq!(e.wine_debug(), "-all");
        assert_eq!(e.hook_line(), None);
        assert_eq!(e.control_port("CONTROL_PORT=20009\r\n"), Ok(20002));
        assert!(!e.external_desk(false));
        assert!(e.external_desk(true));
    }
    #[test]
    fn e1718_config() {
        let e = Edition::E1718;
        assert_ne!(e.display(), Edition::E1011.display());
        assert_eq!(
            e.dll_overrides(),
            "winmm,winmm_orig,netguard=n;mscoree,mshtml="
        );
        assert_eq!(e.hook_line(), Some("edition: 2017-18 server"));
        assert!(
            e.external_desk(false),
            "the built-in desk speaks the 2010-11 protocol"
        );
        let conf =
            "DHCP=0\r\nCONTROL_PORT=20012\r\nGAME_SVR_IP=localhost\r\nGAME_SVR_PORT=20060\r\n";
        assert_eq!(e.control_port(conf), Ok(20012));
        // required, never the 2010-11 arcade's port, set once
        assert!(e.control_port("DHCP=0\n").is_err());
        assert!(e.control_port("CONTROL_PORT=20002\n").is_err());
        assert!(e.control_port("CONTROL_PORT=0\n").is_err());
        assert!(e.control_port("CONTROL_PORT=x\n").is_err());
        assert!(
            e.control_port("CONTROL_PORT=20012\nCONTROL_PORT=20013\n")
                .is_err()
        );
        assert!(
            e.control_port("CONTROL_PORT=20012\nMATCH_PORT=20012\n")
                .is_err()
        );
        assert!(
            e.control_port("CONTROL_PORT=20060\nGAME_SVR_PORT=20060\n")
                .is_err()
        );
        assert_eq!(
            e.control_port("CONTROL_PORT=20012\nMATCH_PORT=20013\nGAME_SVR_PORT=20060\n"),
            Ok(20012)
        );
        assert!(game_server_is_local(conf));
        // a literal 127.0.0.1 makes the control use the network's own host: the publisher's server
        assert!(!game_server_is_local("GAME_SVR_IP=127.0.0.1\n"));
        assert!(!game_server_is_local("GAME_SVR_IP=127.evil.example\n"));
        assert!(!game_server_is_local("GAME_SVR_IP=127.0.0.2\n"));
        assert!(!game_server_is_local(
            "GAME_SVR_IP=localhost\nGAME_SVR_IP=203.0.113.7\n"
        ));
        assert!(!game_server_is_local(
            "GAME_SVR_IP=203.0.113.7\nGAME_SVR_IP=localhost\n"
        ));
        assert!(!game_server_is_local("GAME_SVR_IP=203.0.113.7\n"));
        assert!(
            !game_server_is_local("DHCP=0\n"),
            "no GAME_SVR_IP: control would use its built-in host"
        );
        assert!(!game_server_is_local("XGAME_SVR_IP=localhost\n"));
    }
    #[test]
    fn hook_log_guard() {
        let dir = std::env::temp_dir().join(format!("wccf-hooklogs-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let line = Edition::E1718.hook_line().unwrap();
        let good = format!("==== mxhook attached (pid 1) ====\n{line}\n");
        assert_eq!(hook_logs(&dir, line, 5).unwrap(), HookLogs::Short(0));
        for pid in 1..=4 {
            fs::write(dir.join(format!("mxhook_{pid}.log")), &good).unwrap();
        }
        fs::write(dir.join("game.log"), line).unwrap(); // not a hook log
        assert_eq!(hook_logs(&dir, line, 5).unwrap(), HookLogs::Short(4));
        fs::write(dir.join("mxhook_5.log"), "edition: 2010-11 server\n").unwrap(); // the wrong hook
        assert_eq!(hook_logs(&dir, line, 5).unwrap(), HookLogs::Short(4));
        fs::write(dir.join("mxhook_5.log"), &good).unwrap();
        assert_eq!(hook_logs(&dir, line, 5).unwrap(), HookLogs::Ok(5));
        // an unreadable entry (a directory with a log's name) is skipped, not fatal, and not counted
        fs::create_dir(dir.join("mxhook_9.log")).unwrap();
        assert_eq!(hook_logs(&dir, line, 5).unwrap(), HookLogs::Ok(5));
        fs::remove_dir(dir.join("mxhook_9.log")).unwrap();
        assert_eq!(wait_hook_logs(&dir, line, || 5, 0).unwrap(), 5);
        let refused = format!("{good}REFUSED: secondary netguard.dll did not load\n");
        fs::write(dir.join("mxhook_6.log"), refused).unwrap();
        assert_eq!(
            hook_logs(&dir, line, 5).unwrap(),
            HookLogs::Refused("mxhook_6.log".into())
        );
        assert!(wait_hook_logs(&dir, line, || 5, 0).is_err());
        fs::write(
            dir.join("mxhook_6.log"),
            "REFUSING TO RUN: the IP helper imports could not be patched\n",
        )
        .unwrap();
        assert!(matches!(
            hook_logs(&dir, line, 5).unwrap(),
            HookLogs::Refused(_)
        ));
        fs::remove_dir_all(dir).unwrap();
    }
    /// The periodic check: a refusal is fatal at once, a missing log only on the third check in a row.
    #[test]
    fn guard_step_streak() {
        let dir = std::env::temp_dir().join(format!("wccf-guardstep-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let line = Edition::E1718.hook_line().unwrap();
        for pid in 1..=5 {
            fs::write(dir.join(format!("mxhook_{pid}.log")), line).unwrap();
        }
        let mut short = 0;
        guard_step(&dir, line, 5, &mut short).unwrap();
        // a sixth process is alive but has no log
        assert!(guard_step(&dir, line, 6, &mut short).is_ok());
        assert!(guard_step(&dir, line, 6, &mut short).is_ok());
        assert!(guard_step(&dir, line, 6, &mut short).is_err());
        short = 2;
        guard_step(&dir, line, 5, &mut short).unwrap();
        assert_eq!(short, 0, "a good check resets the streak");
        fs::write(
            dir.join("mxhook_6.log"),
            "REFUSED: secondary netguard.dll did not load",
        )
        .unwrap();
        assert!(guard_step(&dir, line, 5, &mut short).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn listener_ownership() {
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
            0: 0100007F:4E32 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0 100 0 0 10 0\n\
            1: 0100007F:4E33 0100007F:9999 01 00000000:00000000 00:00000000 00000000  1000        0 777 1 0 100 0 0 10 0\n";
        assert_eq!(listen_inodes(tcp, 20018), [4242]);
        assert!(
            listen_inodes(tcp, 20019).is_empty(),
            "established, not listening"
        );
        assert!(listen_inodes(tcp, 20002).is_empty());
        let tcp6 = "  sl  local_address rem_address st\n\
            0: 00000000000000000000000001000000:4E32 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 5151 1 0\n";
        assert_eq!(listen_inodes(tcp6, 20018), [5151]);
        // this test process holds real listeners (IPv4 and IPv6); another pid does not
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(owns_listener(&[std::process::id()], port));
        if let Ok(l6) = TcpListener::bind("[::1]:0") {
            assert!(owns_listener(
                &[std::process::id()],
                l6.local_addr().unwrap().port()
            ));
        }
        assert!(!owns_listener(&[], port));
        assert!(!owns_listener(&[1], port));
    }
    /// verify_prefix refuses cleanup of a prefix process whose cwd is outside the tree, which would leak Xvfb and the
    /// wineserver at stop: every Wine process the supervisor starts runs inside the tree.
    #[test]
    fn sddy_runs_inside_the_tree() {
        let root = Path::new("/srv/x/1718-arcade");
        let cmd = sddy_command(
            root,
            &[("WINEPREFIX", "/srv/x/1718-arcade/wineprefix".into())],
        );
        assert_eq!(cmd.get_current_dir(), Some(root.join("runtime").as_path()));
        assert!(
            cmd.get_envs()
                .any(|(k, v)| k == "WINEPREFIX" && v.is_some())
        );
    }
}
