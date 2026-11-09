//! `wccf-linux-server start|stop|status|serve|cups [--edition 2010-11|2017-18]`: the arcades' supervisor (Linux only; README.md).
// The arcades' supervisor: no call that can panic (tests may, crates/clippy.toml)
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::string_slice
)]
#[cfg(target_os = "linux")]
mod cups;
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
