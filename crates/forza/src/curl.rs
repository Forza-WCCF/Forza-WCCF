//! Windows' own curl.exe, by full path: HTTPS with Windows' certificates, never a curl.exe that lies in a kit folder.
//! Shared by the Launcher's UPDATE and the installer's package download. Off Windows it is the `curl` on PATH.

use std::process::{Command, Stdio};

/// Windows' own program, by full path.
pub fn system32(exe: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let mut cmd = Command::new(std::path::Path::new(&root).join("System32").join(exe));
        // CREATE_NO_WINDOW
        cmd.stdin(Stdio::null()).creation_flags(0x0800_0000);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new(exe.trim_end_matches(".exe"));
        cmd.stdin(Stdio::null());
        cmd
    }
}

/// curl.exe that fails on an HTTP error and gives up on a connection that stays silent for a minute.
pub fn curl() -> Command {
    let mut cmd = system32("curl.exe");
    cmd.args([
        "-sSfL",
        "--connect-timeout",
        "20",
        "--speed-limit",
        "1",
        "--speed-time",
        "60",
    ])
    .args(["-A", "wccf-kit"]);
    cmd
}
