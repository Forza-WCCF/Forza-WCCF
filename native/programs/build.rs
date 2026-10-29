// The version information of the shipped programs (cargo xtask package refuses a kit program without it); the test
// programs have none.
#[path = "../../crates/launcher/version_rc.rs"]
mod version_rc;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../crates/launcher/version_rc.rs");
    if !std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "x86")
        || !std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows")
        || std::env::var_os("CARGO_FEATURE_PROGRAMS").is_none()
    {
        return;
    }
    for (bin, exe, what) in [
        (
            "inject",
            "inject",
            "loads the Forza-WCCF panel into the running game",
        ),
        ("fpr_emu", "FPR_Emu", "the card table's stand-in"),
    ] {
        if let Some(res) = version_rc::make(exe, what, None) {
            println!("cargo:rustc-link-arg-bin={bin}={}", res.display());
        }
    }
}
