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
    // FPR_Emu.exe, which the game starts, also has Forza-WCCF's icon and a manifest (2026-10-29): a known face for
    // Windows' file checks, run as the user who starts it, nothing more. The icon is forza-wccf.ico without its 64, 128
    // and 256 pixel images: the full one was 122 KB of the 389 KB exe (0.5.1: 268 KB), and a windowless helper shows 16-48
    let manifest = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("fpr_emu.manifest");
    std::fs::write(&manifest, FPR_EMU_MANIFEST).expect("write fpr_emu.manifest");
    println!("cargo:rustc-link-arg-bin=fpr_emu=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=fpr_emu=/MANIFESTINPUT:{}",
        manifest.display()
    );
    println!("cargo:rustc-link-arg-bin=fpr_emu=/MANIFESTUAC:level='asInvoker' uiAccess='false'");
    for (bin, exe, what, icon) in [
        (
            "inject",
            "inject",
            "loads the Forza-WCCF panel into the running game",
            None,
        ),
        (
            "fpr_emu",
            "FPR_Emu",
            "the card table's stand-in",
            Some("../../assets/logo/forza-wccf-small.ico"),
        ),
    ] {
        if let Some(res) = version_rc::make(exe, what, icon) {
            println!("cargo:rustc-link-arg-bin={bin}={}", res.display());
        }
    }
}

/// FPR_Emu.exe's manifest beside the linker's asInvoker one: Windows 10 and 11.
const FPR_EMU_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;
