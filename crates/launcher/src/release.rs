//! The kit's releases at UPDATE_URL (2026-10-09): Launcher.exe's UPDATE lists them and says when a newer release is out.
//! Test builds (pre-releases) are never "newer": only a release is offered that way.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

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

/// What UPDATE shows of a release's notes (GitHub's markdown body), in `lang` ("en", "it", "ja"): the `## English`,
/// `## Italiano` or `## 日本語` section, else the English one, else the whole body; up to the checksums line and the
/// VirusTotal table, headings without their `#`. Remote text: no HTML comments (the `needs-installer` marker), no `**`, no control or bidi/zero-width
/// characters, at most 16,000 characters. Tags stay literal text: the window draws it as a label, never as markup or links.
pub fn notes(body: &str, lang: &str) -> String {
    let heading = match lang {
        "it" => "Italiano",
        "ja" => "日本語",
        _ => "English",
    };
    let section = |name: &str| {
        let mut lines = body.lines().skip_while(|l| {
            !l.strip_prefix("## ")
                .is_some_and(|h| h.trim().eq_ignore_ascii_case(name))
        });
        lines.next()?;
        Some(lines.take_while(|l| !l.starts_with("## ")).collect::<Vec<_>>())
    };
    let lines = section(heading)
        .or_else(|| section("English"))
        .unwrap_or_else(|| body.lines().collect());
    let mut text: String = lines
        .into_iter()
        .take_while(|l| !l.starts_with("SHA-256 sums") && !l.starts_with("### VirusTotal"))
        // a heading's `### ` goes, its words stay
        .map(|l| match l.trim_start_matches('#') {
            t if t.len() < l.len() && t.starts_with(' ') => t.trim_start(),
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    while let Some(a) = text.find("<!--") {
        let b = text[a..].find("-->").map_or(text.len(), |b| a + b + 3);
        text.replace_range(a..b, "");
    }
    text.replace("**", "")
        .chars()
        .filter(|&c| {
            c == '\n'
                || !(c.is_control()
                    || matches!(c, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'))
        })
        .take(16_000)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The copy of a release's notes in `data`\release-notes\, named by its version (none for a tag of unknown shape).
fn notes_file(data: &Path, tag: &str) -> Option<PathBuf> {
    parse(tag).map(|v| data.join("release-notes").join(format!("{v}.md")))
}

/// Keeps the notes of release `tag` once: a version's first copy stays, so each is written at most once.  UPDATE shows
/// it when GitHub does not answer.
pub fn keep_notes(data: &Path, tag: &str, body: &str) {
    if let Some(f) = notes_file(data, tag)
        && !body.trim().is_empty()
        && !f.exists()
    {
        let _ = f.parent().map(fs::create_dir_all);
        // a self-hosted target has no size limit: GitHub's bodies stay under 125,000 characters
        let end = body.char_indices().nth(125_000).map_or(body.len(), |(i, _)| i);
        let _ = fs::write(f, &body[..end]);
    }
}

/// The notes of release `tag` kept by [`keep_notes`].
pub fn kept_notes(data: &Path, tag: &str) -> Option<String> {
    fs::read_to_string(notes_file(data, tag)?).ok()
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

    const BODY: &str = "## English\r\n\r\n**Big one.** 0.6.0 adds a cup.\r\n\r\n### For players\r\n\r\n- A cup.\r\n\
        <!-- needs-installer -->\r\n\r\n## Italiano\r\n\r\n- Una\u{202e} coppa.\u{7}\r\n\r\n## 日本語\r\n\r\n- カップ。\r\n\r\n\
        SHA-256 sums: `SHA256SUMS.txt`.\r\n\r\n### VirusTotal\r\n\r\n| File |\r\n";

    #[test]
    fn notes_in_the_players_language_as_plain_text() {
        assert_eq!(
            notes(BODY, "en"),
            "Big one. 0.6.0 adds a cup.\n\nFor players\n\n- A cup."
        );
        assert_eq!(notes(BODY, "it"), "- Una coppa."); // the control and bidi characters dropped
        assert_eq!(notes(BODY, "ja"), "- カップ。"); // no checksums, no VirusTotal table
        // no section in the language: English; no sections at all: the whole body
        let en_only = "## English\n- A cup.\n";
        assert_eq!(notes(en_only, "ja"), "- A cup.");
        assert_eq!(notes("Just <b>text</b> <!-- x", "it"), "Just <b>text</b>");
        assert_eq!(notes("#42 stays\n## Heading", "en"), "#42 stays\nHeading");
        assert_eq!(notes(&"x".repeat(20_000), "en").len(), 16_000);
    }

    #[test]
    fn notes_are_kept_once_per_version() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        assert_eq!(kept_notes(data, "v0.6.0"), None);
        keep_notes(data, "v0.6.0", "");
        assert_eq!(kept_notes(data, "v0.6.0"), None); // an empty body is not kept
        keep_notes(data, "v0.6.0", "first");
        keep_notes(data, "v0.6.0", "edited later");
        assert_eq!(kept_notes(data, "v0.6.0").as_deref(), Some("first"));
        assert_eq!(kept_notes(data, "kit-0.6.0").as_deref(), Some("first")); // version.txt's name for it
        keep_notes(data, "../../evil", "x");
        assert!(!data.join("release-notes").join("..").join("..").join("evil").exists());
        assert_eq!(fs::read_dir(data.join("release-notes")).unwrap().count(), 1);
    }

    #[test]
    fn startup_check_once_a_day() {
        assert!(check_due(1_000, None));
        assert!(!check_due(1_000 + 3_600, Some(1_000)));
        assert!(check_due(1_000 + 86_400, Some(1_000)));
        assert!(check_due(500, Some(1_000))); // clock set back
    }
}
