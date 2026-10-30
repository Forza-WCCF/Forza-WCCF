// dmath.dll's version information and the four PSAPI calls match_Release.exe imports, forwarded to Windows' psapi (the
// guarded engine imports dmath.dll in PSAPI.DLL's place).
#[path = "../../crates/launcher/version_rc.rs"]
mod version_rc;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../crates/launcher/version_rc.rs");
    if !std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "x86")
        || !std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows")
    {
        return;
    }
    for f in [
        "GetModuleBaseNameA",
        "EnumProcessModules",
        "EnumProcesses",
        "GetProcessMemoryInfo",
    ] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{f}=psapi.{f}");
    }
    let what = "the match engine's deterministic math: the same results on every CPU";
    if let Some(res) = version_rc::make("dmath.dll", what, None) {
        println!("cargo:rustc-cdylib-link-arg={}", res.display());
    }
}
