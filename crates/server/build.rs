// forza-server.exe carries Windows version information, the Forza-WCCF icon and an asInvoker manifest like Launcher.exe
// (cargo xtask package refuses a kit program without version information); the Linux build has none.
#[path = "../launcher/version_rc.rs"]
mod version_rc;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../launcher/version_rc.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    version_rc::as_invoker("forza-server");
    if let Some(res) = version_rc::make(
        "forza-server",
        "the online server: seat desk, match relay and game server",
        Some("../../assets/logo/forza-wccf.ico"),
    ) {
        println!("cargo:rustc-link-arg-bin=forza-server={}", res.display());
    }
}
