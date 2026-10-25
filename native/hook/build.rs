// The DLL's version information (cargo xtask package refuses a kit program without it) and its forwarded exports: the
// winmm calls the game imports from the hook go on to the real winmm.
#[path = "../../crates/launcher/version_rc.rs"]
mod version_rc;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=winmm_exports.txt");
    println!("cargo:rerun-if-changed=../../crates/launcher/version_rc.rs");
    let x86 = std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "x86")
        && std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    if !x86 {
        return;
    }
    let (dll, what, exports) = if std::env::var_os("CARGO_FEATURE_E1011").is_some() {
        // setup renames the game's WINMM.dll import to wccfk.dll: the three timer calls it imports go to Windows' winmm
        let timers =
            ["timeGetTime", "timeBeginPeriod", "timeEndPeriod"].map(|f| format!("{f}=winmm.{f}"));
        (
            "wccfk.dll",
            "Forza-WCCF's hook in the game: stands in for the cabinet's hardware",
            timers.to_vec(),
        )
    } else if let Some(what) = [
        (
            "CARGO_FEATURE_E1718",
            "stands in for the cabinet's hardware, loaded by the game in place of winmm.dll",
        ),
        (
            "CARGO_FEATURE_E1011_SERVER",
            "the 2010-11 server's hook under Wine, loaded by control in place of winmm.dll",
        ),
        (
            "CARGO_FEATURE_E1718_SERVER",
            "the 2017-18 server's hook under Wine, loaded by control in place of winmm.dll",
        ),
    ]
    .into_iter()
    .find_map(|(f, what)| std::env::var_os(f).map(|_| what))
    {
        // in place of winmm.dll in the game's folder: every winmm export goes to the real winmm, kept as winmm_orig.dll
        let list = std::fs::read_to_string("winmm_exports.txt").expect("winmm_exports.txt");
        let all = list
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|f| format!("{f}=winmm_orig.{f}"));
        ("winmm.dll", what, all.collect())
    } else {
        return; // lib.rs says which feature to pick
    };
    for e in exports {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{e}");
    }
    if let Some(res) = version_rc::make(dll, what, None) {
        println!("cargo:rustc-cdylib-link-arg={}", res.display());
    }
}
