// forza-server.exe carries Windows version information like Launcher.exe (cargo xtask package refuses a kit program
// without it); the Linux build has none.
#[path = "../launcher/version_rc.rs"]
mod version_rc;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../launcher/version_rc.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows")
        && let Some(res) = version_rc::make(
            "forza-server",
            "the online server: seat desk, match relay and log inbox",
            None,
        )
    {
        println!("cargo:rustc-link-arg-bin=forza-server={}", res.display());
    }
}
