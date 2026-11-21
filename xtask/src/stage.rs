// ---- the staged kit ----
use super::*;

/// Wipe the kit except the built outputs (`keep`), then copy every tracked file of `map` from the working tree.
pub(crate) fn stage(root: &Path, kit: &Path, map: &[(&str, &str)], keep: &[&str]) -> Result<()> {
    fs::create_dir_all(kit)?;
    wipe(kit, "", keep)?;
    let sh = Shell::new()?;
    sh.change_dir(root);
    for (to, from) in map {
        let files = cmd!(sh, "git ls-files -z -- {from}").read()?;
        for f in files.split('\0').filter(|f| !f.is_empty()) {
            let src = root.join(f);
            if !src.is_file() {
                continue; // tracked, deleted in the working tree
            }
            let rest = f[from.len()..].trim_start_matches('/');
            let dst = if rest.is_empty() {
                kit.join(to)
            } else {
                kit.join(to).join(rest)
            };
            fs::create_dir_all(dst.parent().unwrap())?;
            fs::copy(&src, &dst)?;
        }
    }
    println!("staged {}", kit.display());
    Ok(())
}

pub(crate) fn wipe(dir: &Path, rel: &str, keep: &[&str]) -> Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let r = if rel.is_empty() {
            e.file_name().to_string_lossy().into_owned()
        } else {
            format!("{rel}/{}", e.file_name().to_string_lossy())
        };
        if keep.contains(&r.as_str()) {
            continue;
        }
        if e.file_type()?.is_dir() {
            if keep.iter().any(|k| k.starts_with(&format!("{r}/"))) {
                wipe(&e.path(), &r, keep)?;
            } else {
                fs::remove_dir_all(e.path())?;
            }
        } else {
            fs::remove_file(e.path())?;
        }
    }
    Ok(())
}

/// The date of every ZIP entry: `SOURCE_DATE_EPOCH`, else the commit's time, else now. A ZIP stays the same for a
/// commit, and unzipped files show a real date (the zip crate's default, 1980-01-01, left Explorer's column empty).
pub(crate) fn zip_time(root: &Path) -> Result<zip::DateTime> {
    let commit = || {
        let o = Command::new("git")
            .args(["log", "-1", "--format=%ct", "HEAD"])
            .current_dir(root)
            .output()
            .ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .or_else(commit)
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs().cast_signed())
        });
    dos_time(secs)
}

/// Unix seconds as a ZIP date, in UTC (ZIP dates have no time zone); days to a date by Howard Hinnant's civil_from_days.
pub(crate) fn dos_time(secs: i64) -> Result<zip::DateTime> {
    let (days, s) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let n = |v: i64| u8::try_from(v).unwrap_or(0);
    zip::DateTime::from_date_and_time(
        u16::try_from(year).unwrap_or(0),
        n(month),
        n(day),
        n(s / 3600),
        n(s % 3600 / 60),
        n(s % 60),
    )
    .map_err(|e| anyhow::anyhow!("{secs} is no ZIP date: {e:?}"))
}

/// The release number a build stamps: [`version`] in package and tag builds (the ZIP names the commit it came from),
/// and without the commit id otherwise, so an ordinary build keeps one version string between commits: the build
/// scripts rerun on `FORZA_VERSION`, and every program showing the version would otherwise relink with each commit.
pub(crate) fn build_version(root: &Path) -> &'static str {
    static DEV: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let v = version(root);
    if STAMP_SHA.load(Ordering::Relaxed) {
        return v;
    }
    DEV.get_or_init(|| {
        v.split_once("-dev+")
            .map_or_else(|| v.to_owned(), |(next, _)| format!("{next}-dev"))
    })
    .as_str()
}

/// The release number from git: `FORZA_VERSION` for the build scripts of Launcher, installer and panel
/// (crates/launcher/version_rc.rs), version.txt and the file names. Asked once per run. A `FORZA_VERSION` already in
/// the environment wins over git (a release candidate built before its tag, release.yml); else `git describe`.
/// [`build_version`] is what a build stamps.
pub(crate) fn version(root: &Path) -> &'static str {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        match version_override(std::env::var("FORZA_VERSION").ok().as_deref()) {
            Ok(Some(v)) => return v,
            Ok(None) => {}
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(2);
            }
        }
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        };
        git(&[
            "describe",
            "--tags",
            "--long",
            "--abbrev=7",
            "--match",
            "v[0-9]*.[0-9]*.[0-9]*",
        ])
        .and_then(|d| version_from_describe(&d))
        // no tag reachable (a shallow clone): a dev version of the commit
        .or_else(|| git(&["rev-parse", "--short=7", "HEAD"]).map(|sha| format!("0.0.0-dev+{sha}")))
        .unwrap_or_else(|| "0.0.0-dev".to_owned())
    })
}

/// A `FORZA_VERSION` from the environment: unset or empty -> none (git decides), a semver such as `0.7.0` -> it, else an
/// error, so a typo never ends up in version.txt and the file names.
pub(crate) fn version_override(env: Option<&str>) -> Result<Option<String>> {
    let Some(v) = env.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    semver::Version::parse(v)
        .map_err(|e| anyhow::anyhow!("FORZA_VERSION={v:?} is no semver version like 0.7.0: {e}"))?;
    Ok(Some(v.to_owned()))
}

/// `git describe --tags --long` -> the version: on the tag `v1.2.3` (`v1.2.3-0-g<sha>`) `1.2.3`; on a later commit the
/// next patch as a dev build, `1.2.4-dev+<sha>`, so the Launcher offers the next release and never the one it came from
/// (semver: 1.2.4-dev < 1.2.4). After a pre-release tag `v1.3.0-rc.1`: `1.3.0-rc.1-dev+<sha>`.
pub(crate) fn version_from_describe(d: &str) -> Option<String> {
    let (rest, sha) = d.rsplit_once("-g")?;
    let (tag, n) = rest.rsplit_once('-')?;
    let v = tag.strip_prefix('v')?;
    if n == "0" {
        return Some(v.to_owned());
    }
    let next = match v.rsplit_once('.') {
        Some((head, patch)) if !v.contains(['-', '+']) => {
            format!("{head}.{}", patch.parse::<u64>().ok()? + 1)
        }
        _ => v.to_owned(),
    };
    Some(format!("{next}-dev+{sha}"))
}
