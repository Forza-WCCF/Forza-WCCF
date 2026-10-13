// Launcher.exe is a window: Windows' current look for its controls (Common Controls 6, also the progress bar's).
//
// Launcher.exe carries Windows version information (2026-10-10): product, publisher, description and version,
// what a file's Properties show. Without it they were nameless unsigned programs, the kind Windows Defender's machine
// learning distrusts most: Kit 5.5's first SETUP.exe was quarantined as "Trojan:Win32/Sabsik.FL.A!ml" when it was
// unzipped. Made with the Windows SDK's rc.exe (on PATH after vcvarsall, as cargo xtask build runs it; otherwise the newest
// under Windows Kits) or, on WSL, llvm-rc. cargo xtask package refuses a kit whose programs lack it.
#[path = "version_rc.rs"]
mod version_rc;

fn main() {
    println!("cargo:rustc-link-arg-bin=Launcher=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=Launcher=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' \
         version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
    );
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=version_rc.rs");
    let what = "starts the game (server, projector and seat 1), sets up the game folder and updates the kit";
    if let Some(res) = version_rc::make("Launcher", what) {
        println!("cargo:rustc-link-arg-bin=Launcher={}", res.display());
    }
}
