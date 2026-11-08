//! `forza-server`: the Linux server (ovh1) and the 2010-11 kit's `programs\forza-server.exe`; the command line is
//! [`forza_server::cli`], which the 2017-18 kit runs as `Launcher.exe server ...`.
// The public server: no call that can panic (tests may, crates/clippy.toml)
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

fn main() {
    forza_server::cli::main(std::env::args_os())
}
