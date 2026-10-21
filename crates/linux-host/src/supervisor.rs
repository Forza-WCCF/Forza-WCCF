//! The 2010-11 server supervisor: Xvfb, the Wine prefix, control and its four match engines, plus a built-in seat broker
//! and match relay (off with `WCCF_EXTERNAL_DESK=1`, when `forza-server` serves TCP 20030 and UDP 20040).
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
fn match_count(root: &Path) -> usize {
    prefix_processes(root)
        .into_iter()
        .filter(|pid| {
            fs::read(format!("/proc/{pid}/comm"))
                .unwrap_or_default()
                .starts_with(b"match_Release")
        })
        .count()
}
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
fn serve(root: &Path) -> io::Result<()> {
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
        snapshot(root, "before")?;
        // WCCF_EXTERNAL_DESK=1: the seat desk and match relay run outside (`forza-server desk` and `relay`
        // beside the projector relay), so this supervisor leaves TCP 20030 and UDP 20040 to them
        let external = std::env::var_os("WCCF_EXTERNAL_DESK").is_some_and(|v| v == "1");
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
        let display = ":97";
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
            ("WINEDEBUG", "-all".into()),
            (
                "WINEDLLOVERRIDES",
                "winmm,winmm_orig,wccfnet,netguard=n;mscoree,mshtml=".into(),
            ),
            ("MXHOOK_LOG_DIR", winpath(&logs)),
            ("MXHOOK_NET_IP", "127.0.0.1".into()),
            (
                "WINESERVER",
                wine_tool(root, "wineserver").display().to_string(),
            ),
        ];
        env.envs(wine_env.clone()).args(["-u"]);
        run_bounded(&mut env, &logs.join("wineboot.log"), 45)?;
        let mut guard = Command::new(wine_tool(root, "wine"));
        guard
            .envs(wine_env.clone())
            .arg(root.join("runtime/test_wccfnet.exe"));
        run_bounded(&mut guard, &logs.join("guard-test.log"), 20)?;
        let mut helper = Command::new(wine_tool(root, "wine"));
        helper
            .envs(wine_env)
            .arg(root.join("run/win32-helper.exe"))
            .arg(winpath(&root.join("runtime")))
            .current_dir(root.join("runtime"));
        children.push(spawn_log(&mut helper, &logs.join("game.log"))?);
        let deadline = Instant::now() + Duration::from_secs(180);
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
                    &"127.0.0.1:20002".parse().unwrap(),
                    Duration::from_millis(200),
                )
                .is_ok()
            {
                break;
            }
            if Instant::now() >= deadline
                || STOP.load(Ordering::Relaxed)
                || root.join("run/stop").exists()
            {
                return Err(io::Error::other(
                    "controller/four match engines not ready within 180s or stop requested",
                ));
            }
            thread::sleep(Duration::from_millis(500));
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
        println!(
            "READY: TCP 20002 controller, TCP 20030 seats, UDP 20040 relay; logs {}",
            logs.display()
        );
        let mut waited: u64 = 0;
        while !root.join("run/stop").exists() && !STOP.load(Ordering::Relaxed) {
            if children
                .iter_mut()
                .any(|p| p.try_wait().ok().flatten().is_some())
            {
                return Err(io::Error::other("server process exited"));
            }
            record_engine_health(root, match_count(root))?;
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
    match std::env::args().nth(1).as_deref().unwrap_or("status") {
        "serve" => serve(&root),
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
            "usage: wccf-linux-server start|stop|status|serve",
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
}
