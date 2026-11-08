//! `win32-helper.exe RUNTIME`: started by the supervisor under Wine (32-bit Windows only; README.md).
// No call that can panic (tests may, crates/clippy.toml)
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
#[cfg(all(windows, target_arch = "x86"))]
mod helper;

#[cfg(all(windows, target_arch = "x86"))]
fn main() -> std::io::Result<()> {
    helper::main()
}

#[cfg(not(all(windows, target_arch = "x86")))]
fn main() {
    eprintln!(
        "win32-helper is a 32-bit Windows program: build it with --target i686-pc-windows-msvc"
    );
    std::process::exit(2);
}
