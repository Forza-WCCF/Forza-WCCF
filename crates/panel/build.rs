// forza_panel.dll carries Windows version information like Launcher.exe (cargo xtask package refuses a kit program
// without it).
#[path = "../launcher/version_rc.rs"]
mod version_rc;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../launcher/version_rc.rs");
    let version = version_rc::release_version();
    if std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "x86")
        && std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows")
        && let Some(res) = version_rc::make_with(
            "forza_panel.dll",
            "the in-game panel: board, catalogue, keys, settings and club card in seat 1's window",
            None,
            &version,
        )
    {
        println!("cargo:rustc-cdylib-link-arg={}", res.display());
    }
}
