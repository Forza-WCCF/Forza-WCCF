//! The kit's releases at UPDATE_URL (2026-10-09): Launcher.exe's UPDATE lists them and says when a newer release is out.
//! Test builds (pre-releases) are never "newer": only a release is offered that way.

use std::env;

use semver::Version;

/// Where UPDATE looks for releases, answered in the shape of GitHub's releases API (`releases?per_page=30`).
/// WCCF_UPDATE_URL points it elsewhere (a `.invalid/` address means no target at all).
pub const UPDATE_URL: &str = "https://api.github.com/repos/Forza-WCCF/Forza-WCCF/";

/// UPDATE_URL, or the environment variable WCCF_UPDATE_URL when it is set (a self-hosted release target).
pub fn update_url() -> String {
    update_url_from(env::var("WCCF_UPDATE_URL").ok())
}

fn update_url_from(env: Option<String>) -> String {
    env.map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| UPDATE_URL.to_owned())
}

/// The release target is a `.invalid/` address: UPDATE finds nothing and nobody is asked.
pub fn no_target() -> bool {
    invalid(&update_url())
}

/// `.invalid` never resolves (RFC 2606): an UPDATE_URL there is no release target.
pub fn invalid(url: &str) -> bool {
    url.contains(".invalid/")
}

pub use forza::curl::{curl, system32};

/// A version number: v0.3.0 or kit-0.3.0 -> 0.3.0; 8.2 -> 8.2.0; 0.4.0-rc.1 keeps its pre-release part.  Anything else
/// (a build of a commit: kit-1208d80) -> None.
pub fn parse(tag: &str) -> Option<Version> {
    let rest = tag
        .strip_prefix("kit-")
        .or_else(|| tag.strip_prefix('v'))
        .unwrap_or(tag);
    let (number, tail) = rest.find(['-', '+']).map_or((rest, ""), |i| rest.split_at(i));
    let mut parts: Vec<&str> = number.split('.').collect();
    if parts.len() > 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    while parts.len() < 3 {
        parts.push("0");
    }
    Version::parse(&format!("{}{tail}", parts.join("."))).ok()
}

/// A test build: a tag with a pre-release part (v0.4.0-rc.1).  GitHub's own prerelease flag does not count.
pub fn is_test(tag: &str) -> bool {
    parse(tag).is_some_and(|v| !v.pre.is_empty())
}

/// The release `tag` is newer than the version that is here.  A test build, or a version of unknown shape, is never
/// newer.
pub fn is_newer(tag: &str, installed: &str) -> bool {
    match (parse(tag), parse(installed)) {
        (Some(new), Some(old)) => new.pre.is_empty() && new > old,
        _ => false,
    }
}

/// This Launcher.exe (`own`, its build version) is not the version the folder's other files are (`files`,
/// version.txt): a copy made while Launcher was open replaces every file but the running Launcher.exe. The numbers
/// only (a test kit's `-label` aside); an unknown version on either side counts as no difference.
pub fn files_differ(own: &str, files: &str) -> bool {
    let core = |v: &str| parse(v).map(|v| (v.major, v.minor, v.patch));
    matches!((core(own), core(files)), (Some(a), Some(b)) if a != b)
}

/// At most one startup check a day: `last` and `now` are unix seconds.  A clock set back counts as due.
pub fn check_due(now: u64, last: Option<u64>) -> bool {
    last.is_none_or(|l| now < l || now - l >= 24 * 3600)
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
        assert!(is_newer("v0.4.0", "kit-0.3.0"));
        assert!(is_newer("v1.0.0", "kit-0.10.0"));
        assert!(is_newer("v0.10.0", "kit-0.9.0"));
        assert!(is_newer("v8.3", "kit-8.2")); // two-part numbers
        assert!(is_newer("v0.4.0", "kit-0.4.0-rc.1")); // the release after its test build
        assert!(!is_newer("v0.3.0", "kit-0.3.0")); // equal
        assert!(!is_newer("v0.3.0", "kit-0.3")); // 0.3 is 0.3.0
        assert!(!is_newer("v0.2.2", "kit-0.3.0")); // older
        assert!(!is_newer("v0.4.0", "kit-0.5.0-rc.1"));
        assert!(!is_newer("v0.5.0-rc.1", "kit-0.3.0")); // a test build is never offered
        assert!(!is_newer("v0.4.0", "kit-1208d80")); // a commit's build: unknown
        assert!(!is_newer("nonsense", "kit-0.3.0"));
        assert!(is_test("v0.4.0-rc.1") && !is_test("v0.4.0") && !is_test("nonsense"));
    }

    #[test]
    fn launcher_and_files_differ() {
        assert!(files_differ("0.3.2", "kit-0.3.3")); // copied 0.3.3 while the 0.3.2 Launcher was open
        assert!(files_differ("0.3.0", "kit-0.3.2"));
        assert!(!files_differ("0.3.2", "kit-0.3.2"));
        assert!(!files_differ("0.3.2", "kit-0.3.2-defendertest")); // a test kit's label
        assert!(!files_differ("0.3.2", "kit-1208d80")); // a build of a commit
        assert!(!files_differ("0.3.2", "not known"));
    }

    #[test]
    fn startup_check_once_a_day() {
        assert!(check_due(1_000, None));
        assert!(!check_due(1_000 + 3_600, Some(1_000)));
        assert!(check_due(1_000 + 86_400, Some(1_000)));
        assert!(check_due(500, Some(1_000))); // clock set back
    }
}
