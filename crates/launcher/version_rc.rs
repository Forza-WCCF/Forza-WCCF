// The Windows version resource of a kit program (see crates/launcher/build.rs for why), shared by the build scripts of
// Launcher.exe, forza-server.exe and forza_panel.dll.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The compiled .res for `bin` (a program; `name.dll` or `NAME.DLL` for a DLL), or None (and a build warning) without an rc.exe.
/// `icon`: an .ico for the program's own icon (Explorer, taskbar, shortcuts), relative to the crate.
pub fn make(bin: &str, what: &str, icon: Option<&str>) -> Option<PathBuf> {
    let Some(rc) = find_rc() else {
        println!("cargo:warning=no rc.exe: {bin} has no version information");
        return None;
    };
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let script = out.join(format!("{bin}.rc"));
    let res = out.join(format!("{bin}.res"));
    let mut rc_text = version_rc(bin, what, &version);
    if let Some(icon) = icon {
        println!("cargo:rerun-if-changed={icon}");
        let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
        let path = dir.join(icon).display().to_string().replace('\\', "/");
        rc_text = format!("1 ICON \"{path}\"\n{rc_text}");
    }
    fs::write(&script, rc_text).expect("write the .rc");
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

/// An embedded asInvoker manifest for the program `bin` (MSVC linker): a program without one is left to Windows' installer
/// guesses, and antivirus heuristics weigh a missing manifest against an unsigned program.
#[allow(dead_code)] // not every build script that includes this file links a program
pub fn as_invoker(bin: &str) {
    println!("cargo:rustc-link-arg-bin={bin}=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bin={bin}=/MANIFESTUAC:level='asInvoker' uiAccess='false'");
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
    let dll = [".dll", ".DLL"].iter().find_map(|s| bin.strip_suffix(s));
    let (bin, file, kind) = match dll {
        Some(stem) => (stem, bin.to_owned(), "0x2"),
        None => (bin, format!("{bin}.exe"), "0x1"),
    };
    let nums: Vec<&str> = version.split('.').chain(["0", "0", "0"]).take(4).collect();
    let comma = nums.join(",");
    format!(
        "1 VERSIONINFO\nFILEVERSION {comma}\nPRODUCTVERSION {comma}\nFILEOS 0x40004\nFILETYPE {kind}\nBEGIN\n\
         BLOCK \"StringFileInfo\"\nBEGIN\nBLOCK \"040904b0\"\nBEGIN\n\
         VALUE \"CompanyName\", \"Forza-WCCF\"\n\
         VALUE \"FileDescription\", \"Forza-WCCF - {bin}: {what}\"\n\
         VALUE \"FileVersion\", \"{version}\"\n\
         VALUE \"InternalName\", \"{bin}\"\n\
         VALUE \"LegalCopyright\", \"Community-made tools for your own copy of the game; the game is not included\"\n\
         VALUE \"OriginalFilename\", \"{file}\"\n\
         VALUE \"ProductName\", \"Forza-WCCF\"\n\
         VALUE \"ProductVersion\", \"{version}\"\n\
         END\nEND\nBLOCK \"VarFileInfo\"\nBEGIN\nVALUE \"Translation\", 0x409, 1200\nEND\nEND\n"
    )
}
