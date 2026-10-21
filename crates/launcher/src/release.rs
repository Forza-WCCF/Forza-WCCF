//! The kit's releases at UPDATE_URL (2026-10-09): Launcher.exe's UPDATE lists them and says when a newer release is out.
//! Test builds (pre-releases) are never "newer": only a release is offered that way.

use std::env;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::Kit;

/// Where UPDATE looks for releases, answered in the shape of GitHub's releases API (`releases?per_page=30`). A
/// placeholder until Forza-WCCF has a release target (ROADMAP step 2): `.invalid` never resolves (RFC 2606). Pointing
/// UPDATE at a real target is this one line.
pub const UPDATE_URL: &str = "https://updates.invalid/wccf/";

/// UPDATE_URL, or the environment variable WCCF_UPDATE_URL when it is set (a self-hosted release target).
pub fn update_url() -> String {
    update_url_from(env::var("WCCF_UPDATE_URL").ok())
}

fn update_url_from(env: Option<String>) -> String {
    env.map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| UPDATE_URL.to_owned())
}

/// No release target yet: UPDATE finds nothing and nobody is asked.
pub fn no_target() -> bool {
    update_url().contains(".invalid/")
}

/// Windows' own program, by full path: never a curl.exe or tar.exe that lies in the kit folder.
pub fn system32(exe: &str) -> Command {
    let root = env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let mut cmd = Command::new(Path::new(&root).join("System32").join(exe));
    cmd.stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW);
    cmd
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

impl Kit {
    /// version.txt from the kit ZIP (cargo xtask package writes it): kit-5.4, kit-5.5-test1, kit-6-test-<commit>.
    pub fn version(&self) -> Option<String> {
        let v = fs::read_to_string(self.dir.join("version.txt")).ok()?;
        Some(v.trim().to_owned()).filter(|v| !v.is_empty())
    }
}

/// kit-5.4 -> ([5, 4], release); kit-5.5-test1 -> ([5, 5], test build); a build of a commit (kit-1208d80) -> None.
fn parse(tag: &str) -> Option<(Vec<u32>, bool)> {
    let rest = tag.strip_prefix("kit-")?;
    let (number, test) = rest.split_once('-').map_or((rest, false), |(n, _)| (n, true));
    let mut parts: Vec<u32> = number.split('.').map(str::parse).collect::<Result<_, _>>().ok()?;
    while parts.len() > 1 && parts.last() == Some(&0) {
        parts.pop(); // kit-5.0 is kit-5
    }
    Some((parts, test))
}

/// The release `tag` is newer than the kit that is here: a higher number, or the release of the test build that is
/// here (kit-5.5 after kit-5.5-test1).  A test build, or a kit of unknown version, is never newer / never older.
pub fn is_newer(tag: &str, installed: &str) -> bool {
    match (parse(tag), parse(installed)) {
        (Some((new, false)), Some((old, old_test))) => new > old || (new == old && old_test),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_url_env_overrides_the_const() {
        assert_eq!(update_url_from(None), UPDATE_URL);
        assert_eq!(update_url_from(Some(" ".into())), UPDATE_URL);
        assert_eq!(update_url_from(Some("https://x.test/r/".into())), "https://x.test/r/");
    }

    #[test]
    fn newer_releases_only() {
        assert!(is_newer("kit-5.5", "kit-5.4"));
        assert!(is_newer("kit-6", "kit-5.10"));
        assert!(is_newer("kit-5.10", "kit-5.9"));
        assert!(is_newer("kit-5.5", "kit-5.5-test1"));
        assert!(!is_newer("kit-5.4", "kit-5.4"));
        assert!(!is_newer("kit-5.0", "kit-5"));
        assert!(!is_newer("kit-5.3", "kit-5.4"));
        assert!(!is_newer("kit-5.4", "kit-5.5-test1"));
        assert!(!is_newer("kit-5.6-test1", "kit-5.4")); // a test build is never offered
        assert!(!is_newer("kit-5.5", "kit-1208d80")); // a commit's build: unknown
        assert!(!is_newer("v1", "kit-5.4"));
        // CI's build of a commit since 2026-10-11 (kit.yml): kit-6-test-<commit>, a test build of 6
        assert!(is_newer("kit-6", "kit-6-test-af17138"));
        assert!(!is_newer("kit-5.5.1", "kit-6-test-af17138"));
        assert!(is_newer("kit-6.1", "kit-6-test-af17138"));
    }
}
