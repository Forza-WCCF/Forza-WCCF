//! `wccf-linux-server start|stop|status|serve [--edition 2010-11|2017-18]`: the shops' supervisor (Linux only; README.md).
#[cfg(target_os = "linux")]
mod supervisor;

#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
    supervisor::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("wccf-linux-server runs on Linux only");
    std::process::exit(2);
}
