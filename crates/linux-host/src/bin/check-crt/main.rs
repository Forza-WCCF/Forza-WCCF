//! `check-crt.exe [fatal]`: the CRT check under Wine (32-bit Windows only; README.md).
#[cfg(all(windows, target_arch = "x86"))]
mod check;

#[cfg(all(windows, target_arch = "x86"))]
fn main() {
    check::main()
}

#[cfg(not(all(windows, target_arch = "x86")))]
fn main() {
    eprintln!("check-crt is a 32-bit Windows program: build it with --target i686-pc-windows-msvc");
    std::process::exit(2);
}
