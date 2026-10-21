// Launcher.exe is a window: Windows' current look for its controls (Common Controls 6, also the progress bar's).
//
// Launcher.exe carries Windows version information (2026-10-10): product, publisher, description and version,
// what a file's Properties show. Without it they were nameless unsigned programs, the kind Windows Defender's machine
// learning distrusts most: Kit 5.5's first SETUP.exe was quarantined as "Trojan:Win32/Sabsik.FL.A!ml" when it was
// unzipped. Made with the Windows SDK's rc.exe (on PATH after vcvarsall, as cargo xtask build runs it; otherwise the newest
// under Windows Kits) or, on WSL, llvm-rc. cargo xtask package refuses a kit whose programs lack it.
#[path = "version_rc.rs"]
mod version_rc;

// Launcher.exe is DPI-aware (2026-10-17): its manifest says so, Windows then gives it the screen's own pixels (no blurry
// stretch at 125%). Per monitor since the window is eframe (step 9.4): egui scales to each screen's dpi as it moves;
// `dpiAware` stays for Windows before 10 1607.
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
    let dpi = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("dpi.manifest");
    std::fs::write(&dpi, DPI_MANIFEST).expect("write dpi.manifest");
    println!("cargo:rustc-link-arg-bin=Launcher=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bin=Launcher=/MANIFESTINPUT:{}", dpi.display());
    println!(
        "cargo:rustc-link-arg-bin=Launcher=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' \
         version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
    );
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=version_rc.rs");
    let what = "starts the game (server, projector and seat 1), sets up the game folder and updates Forza-WCCF";
    if let Some(res) = version_rc::make("Launcher", what) {
        println!("cargo:rustc-link-arg-bin=Launcher={}", res.display());
    }
}
