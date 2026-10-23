// Forza-WCCF-Installer.exe, like Launcher.exe (crates/launcher/build.rs): DPI-aware per monitor through its manifest (the
// screen's own pixels, no blurry stretch at 125% or 150%), and Windows version information (a nameless unsigned program is what
// Defender's machine learning distrusts most). Windows targets only: the tests also build it for Linux.
#[path = "../launcher/version_rc.rs"]
mod version_rc;

const DPI_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../launcher/version_rc.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let bin = "Forza-WCCF-Installer";
    let dpi =
        std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("dpi.manifest");
    std::fs::write(&dpi, DPI_MANIFEST).expect("write dpi.manifest");
    println!("cargo:rustc-link-arg-bin={bin}=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin={bin}=/MANIFESTINPUT:{}",
        dpi.display()
    );
    if let Some(res) = version_rc::make(
        bin,
        "installs Forza-WCCF from your game disc",
        Some("../../assets/logo/forza-wccf.ico"),
    ) {
        println!("cargo:rustc-link-arg-bin={bin}={}", res.display());
    }
}
