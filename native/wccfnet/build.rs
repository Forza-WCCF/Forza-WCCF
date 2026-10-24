// wccfnet.dll's version information and the six read-only IP helper calls it forwards to Windows' iphlpapi (the four
// it stubs are its own exports, lib.rs).
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
        "GetInterfaceInfo",
        "GetIpAddrTable",
        "GetNetworkParams",
        "GetAdaptersInfo",
        "GetIpForwardTable",
        "GetAdaptersAddresses",
    ] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{f}=iphlpapi.{f}");
    }
    let what = "the network guard: the game programs import it instead of IPHLPAPI.DLL; it loads the panel when FORZA_PANEL is set";
    if let Some(res) = version_rc::make("wccfnet.dll", what, None) {
        println!("cargo:rustc-cdylib-link-arg={}", res.display());
    }
}
