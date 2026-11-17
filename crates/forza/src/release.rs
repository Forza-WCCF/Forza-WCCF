//! Release tags: their version and which are test builds.  Shared by the Launcher's UPDATE and the installer's package
//! pick, so both skip the same test builds.

pub use semver::Version;

/// A version number: v0.3.0 or kit-0.3.0 -> 0.3.0; 8.2 -> 8.2.0; 0.4.0-rc.1 keeps its pre-release part.  Anything else
/// (a build of a commit: kit-1208d80) -> None.
pub fn parse(tag: &str) -> Option<Version> {
    let rest = tag
        .strip_prefix("kit-")
        .or_else(|| tag.strip_prefix('v'))
        .unwrap_or(tag);
    let (number, tail) = rest
        .find(['-', '+'])
        .map_or((rest, ""), |i| rest.split_at(i));
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
