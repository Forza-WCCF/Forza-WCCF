//! `Launcher.exe --no-quickedit PID` on a stand-in console: a hidden `ping` with a console of its own.

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
    let out = Command::new(env!("CARGO_BIN_EXE_Launcher"))
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
