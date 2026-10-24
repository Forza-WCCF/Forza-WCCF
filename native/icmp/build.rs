// ICMP.DLL's version information.
#[path = "../../crates/launcher/version_rc.rs"]
mod version_rc;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../crates/launcher/version_rc.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "x86")
        && std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows")
        && let Some(res) = version_rc::make(
            "ICMP.DLL",
            "stand-in ICMP.DLL: the game's pings return at once instead of timing out",
            None,
        )
    {
        println!("cargo:rustc-cdylib-link-arg={}", res.display());
    }
}
