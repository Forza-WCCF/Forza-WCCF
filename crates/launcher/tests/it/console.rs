//! `Launcher.exe --no-quickedit PID` on a stand-in console: a hidden `ping` with a console of its own.

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The built Launcher: at run time first (cargo test and nextest set it; an archive's copy is elsewhere).
fn launcher_exe() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_Launcher").map_or_else(|| env!("CARGO_BIN_EXE_Launcher").into(), Into::into)
}

#[test]
fn quickedit_is_switched_off_in_another_programs_console() {
    let mut stand_in = Command::new("ping.exe")
        .args(["-n", "30", "127.0.0.1"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let out = Command::new(launcher_exe())
        .args(["--no-quickedit", &stand_in.id().to_string(), "x"])
        .output()
        .unwrap();
    let _ = stand_in.kill();
    let _ = stand_in.wait();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains(&format!("pid {}: QuickEdit OFF", stand_in.id())),
        "{text}"
    );
    assert!(text.contains("pid x: NOT changed (not a process id)"), "{text}");
    assert_eq!(out.status.code(), Some(1), "{text}");
}

/// Output redirected to a file lands in the file, not in the parent's console (`Launcher.exe status > out.txt`).
#[test]
fn redirected_output_goes_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("Launcher.exe");
    std::fs::copy(launcher_exe(), &exe).unwrap();
    let out = dir.path().join("out.txt");
    let status = Command::new(&exe)
        .arg("status")
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&out).unwrap())
        .status()
        .unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("Nothing running."), "{status:?} {text:?}");
}
