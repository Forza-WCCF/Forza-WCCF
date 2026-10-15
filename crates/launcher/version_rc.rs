// The Windows version resource of a kit program (see crates/launcher/build.rs for why), shared by the build scripts of
// Launcher.exe, forza-server.exe and forza_panel.dll.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The compiled .res for `bin` (a program; `name.dll` for a DLL), or None (and a build warning) without an rc.exe.
pub fn make(bin: &str, what: &str) -> Option<PathBuf> {
    let Some(rc) = find_rc() else {
        println!("cargo:warning=no rc.exe: {bin} has no version information");
        return None;
    };
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let script = out.join(format!("{bin}.rc"));
    let res = out.join(format!("{bin}.res"));
    fs::write(&script, version_rc(bin, what, &version)).expect("write the .rc");
    let ok = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&script)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "rc.exe could not make {}", res.display());
    Some(res)
}

/// rc.exe on PATH (after vcvarsall), else llvm-rc (WSL), else the newest x64 rc.exe under the Windows Kits.
fn find_rc() -> Option<PathBuf> {
    for rc in ["rc.exe", "llvm-rc"] {
        if Command::new(rc).arg("/?").output().is_ok() {
            return Some(PathBuf::from(rc));
        }
    }
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut found: Vec<PathBuf> = fs::read_dir(kits)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path().join("x64").join("rc.exe")))
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found.pop()
}

/// The version resource of one program: 5.5.0 -> FILEVERSION 5,5,0,0.
fn version_rc(bin: &str, what: &str, version: &str) -> String {
    let (bin, file, kind) = match bin.strip_suffix(".dll") {
        Some(stem) => (stem, bin.to_owned(), "0x2"),
        None => (bin, format!("{bin}.exe"), "0x1"),
    };
    let nums: Vec<&str> = version.split('.').chain(["0", "0", "0"]).take(4).collect();
    let comma = nums.join(",");
    format!(
        "1 VERSIONINFO\nFILEVERSION {comma}\nPRODUCTVERSION {comma}\nFILEOS 0x40004\nFILETYPE {kind}\nBEGIN\n\
         BLOCK \"StringFileInfo\"\nBEGIN\nBLOCK \"040904b0\"\nBEGIN\n\
         VALUE \"CompanyName\", \"Forza-WCCF community\"\n\
         VALUE \"FileDescription\", \"WCCF 2010-11 kit - {bin}: {what}\"\n\
         VALUE \"FileVersion\", \"{version}\"\n\
         VALUE \"InternalName\", \"{bin}\"\n\
         VALUE \"LegalCopyright\", \"Community-made tools for your own copy of the game; the game is not included\"\n\
         VALUE \"OriginalFilename\", \"{file}\"\n\
         VALUE \"ProductName\", \"WCCF 2010-11 kit\"\n\
         VALUE \"ProductVersion\", \"{version}\"\n\
         END\nEND\nBLOCK \"VarFileInfo\"\nBEGIN\nVALUE \"Translation\", 0x409, 1200\nEND\nEND\n"
    )
}
