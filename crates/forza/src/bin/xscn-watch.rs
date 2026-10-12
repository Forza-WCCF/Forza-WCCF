//! `xscn-watch.exe SECONDS [LOGFILE]`: the xscnWatch scene service on its own, with the arguments of the vendored
//! `_xscn_watch.py` it replaces (the Linux server runs it under wine beside the game). The game's `extracted` folder
//! comes from the environment variable `WCCF_GAME`.
//! `XSCN_WATCH_TAG` (tests only) swaps the game's mutex, board and mapping names for scratch ones. Exit 0 = ran its time, 2 = refused or bad arguments.

#[cfg(windows)]
fn main() {
    use forza::standins::scenes::{Names, spawn};
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(seconds) = args.first().and_then(|s| s.parse::<u64>().ok()) else {
        eprintln!(
            "usage: xscn-watch SECONDS [LOGFILE]    (WCCF_GAME = the game's extracted folder)"
        );
        std::process::exit(2);
    };
    let file = args.get(1).map(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap_or_else(|e| {
                eprintln!("cannot open {p}: {e}");
                std::process::exit(2);
            })
    });
    let (t0, file) = (Instant::now(), Mutex::new(file));
    let log: forza::standins::Log = Arc::new(move |msg| {
        let line = format!("{:7.2}  {msg}", t0.elapsed().as_secs_f64());
        println!("{line}");
        if let Some(f) = file.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{line}");
        }
    });
    let game = std::env::var_os("WCCF_GAME").filter(|g| !g.is_empty());
    let dir = game.map(|g| std::path::Path::new(&g).join(r"data\motion\launcherMotion\xscn"));
    let Some(dir) = dir.filter(|d| d.is_dir()) else {
        log("REFUSED: WCCF_GAME must be the game's extracted folder");
        std::process::exit(2);
    };
    let names =
        std::env::var("XSCN_WATCH_TAG").map_or_else(|_| Names::game(), |t| Names::scratch(&t));
    match spawn(&dir, &names, log) {
        Ok(handle) => {
            std::thread::sleep(Duration::from_secs(seconds));
            drop(handle);
        }
        Err(_) => std::process::exit(2), // the service has logged why
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("xscn-watch runs on Windows (or under wine)");
    std::process::exit(2);
}
