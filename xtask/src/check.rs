// ---- check ----
use super::{
    BTreeSet, CheckFailed, Child, Command, Context, Duration, INSTALLER, InstallerWork, KEEP, MAP,
    NATIVE_CRATES, Output, PANEL_TARGET, PROGRAMS, PROGRAMS_1718, Path, PathBuf, Result, Stdio,
    TARGET, build_all, ci_1011_reason, ci_builds_1011, ensure, fs, hook_feature, hooks, in_ci,
    installer_checks, installer_exe, installer_work, kit_stale, no_kit_word, no_nul, no_python,
    no_script_host, profile, remap_config, run_ok, rust, scrub, shipped_docs_no_kit, stage,
};

/// What `check` collects: the failed checks' names.
pub(crate) struct Checks {
    kit: PathBuf,
    /// scratch folders of the checks, beside the kit
    tmp: PathBuf,
    failed: Vec<String>,
    /// a check's program could not start: `verify`'s ERROR, not FAIL
    unstarted: bool,
}

impl Checks {
    /// Run a check's command to its end; print its output (only the last line if `last`); a nonzero exit fails `what`.
    pub(crate) fn run(&mut self, what: &str, mut c: Command, last: bool) {
        match c.output() {
            Ok(o) => {
                self.report(what, &o, last);
            }
            Err(e) => {
                self.unstarted = true;
                self.fail(what, &format!("could not start: {e}"));
            }
        }
    }

    pub(crate) fn report(&mut self, what: &str, o: &Output, last: bool) {
        let out = String::from_utf8_lossy(&o.stdout);
        let lines: Vec<&str> = out.lines().collect();
        let shown = if last {
            &lines[lines.len().saturating_sub(1)..]
        } else {
            &lines[..]
        };
        for l in shown {
            println!("{l}");
        }
        if !o.status.success() {
            eprint!("{}", String::from_utf8_lossy(&o.stderr));
            self.failed.push(what.to_owned());
        }
    }

    pub(crate) fn fail(&mut self, what: &str, why: &str) {
        eprintln!("{what}: {why}");
        self.failed.push(what.to_owned());
    }
}

/// One check at a time on this PC (held until dropped).
pub(crate) fn check_lock() -> Result<std::fs::File> {
    let lock = std::fs::File::create(std::env::temp_dir().join("forza-xtask-check.lock"))?;
    if lock.try_lock().is_err() {
        println!("waiting for another `cargo xtask check` on this PC ...");
        lock.lock()?;
    }
    Ok(lock)
}

/// A Windows path spelled one way: `/`, lower case, no `\\?\` prefix, no trailing separator.
pub(crate) fn norm_path(p: &str) -> String {
    let p = p.replace('\\', "/").to_lowercase();
    let p = match p.strip_prefix("//?/unc/") {
        Some(unc) => format!("//{unc}"),
        None => p.trim_start_matches("//?/").to_owned(),
    };
    p.trim_end_matches('/').to_owned()
}

/// A package's directory relative to the repository root, written the way git prints paths (`/`, no `\\?\` prefix) and
/// in lower case (Windows ignores case; `changed_packages` lower-cases the files too). Cargo prints `\` on Windows, git
/// `/` everywhere: compared raw they matched no file, so `check quick` ran fmt only.
pub(crate) fn package_dir(root: &str, manifest: &str) -> Result<String> {
    let (root, manifest) = (norm_path(root), norm_path(manifest));
    let dir = manifest.rsplit_once('/').map_or("", |(d, _)| d);
    // the next character must end a folder name (not e:/r vs e:/rr)
    dir.strip_prefix(&root)
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
        .map(|rest| rest.trim_matches('/').to_owned())
        .with_context(|| format!("package {manifest} is outside the repository {root}"))
}

/// The files that differ from the base's merge-base with HEAD (committed, staged, unstaged or untracked), one per line.
/// `None` when `base` has no merge base with HEAD here (not fetched, unknown): the caller then checks everything.
pub(crate) fn changed_files(root: &Path, base: &str) -> Result<Option<String>> {
    let git = |args: &[&str]| -> Result<String> {
        let o = Command::new("git").args(args).current_dir(root).output()?;
        ensure!(o.status.success(), "git {} failed", args.join(" "));
        Ok(String::from_utf8(o.stdout)?)
    };
    let Ok(mb) = git(&["merge-base", base, "HEAD"]) else {
        return Ok(None);
    };
    let mb = mb.trim().to_owned();
    // no renames: the old path of a moved file is a change in its old package too
    let mut files = git(&["diff", "--name-only", "--no-renames", &mb])?;
    files += &git(&["ls-files", "--others", "--exclude-standard"])?;
    Ok(Some(files))
}

/// Workspace packages that own one of `files` ([`changed_files`]), plus every package that depends on one by path. A
/// change to Cargo.lock, the toolchain or the root manifest means all.
pub(crate) fn changed_packages(root: &Path, files: &str) -> Result<Option<BTreeSet<String>>> {
    let meta = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .current_dir(root)
        .output()?;
    ensure!(meta.status.success(), "cargo metadata failed");
    let meta: serde_json::Value = serde_json::from_slice(&meta.stdout)?;
    let pkgs = meta["packages"].as_array().context("no packages")?;
    // (name, dir relative to the root, names of path dependencies)
    let info: Vec<(String, String, Vec<String>)> = pkgs
        .iter()
        .map(|p| -> Result<_> {
            let manifest = p["manifest_path"].as_str().unwrap_or_default();
            let dir = package_dir(&root.display().to_string(), manifest)?;
            let deps = p["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| !d["path"].is_null())
                .filter_map(|d| d["name"].as_str().map(str::to_owned))
                .collect();
            Ok((p["name"].as_str().unwrap_or_default().to_owned(), dir, deps))
        })
        .collect::<Result<_>>()?;
    Ok(affected(info, files))
}

/// The crates that embed or read a folder the workspace shares: a file under one of them is their change, not the whole
/// workspace's. `forza` and `forza-ui` read the fonts and the edition sheets in their tests, `forza-panel` embeds the
/// panel's text, Launcher, the installer, the server and FPR_Emu embed the logo, and `xtask` stages the fonts and the
/// editions and freezes `FPR_Emu.exe` on its icon.
pub(crate) const FOLDER_OWNERS: &[(&str, &[&str])] = &[
    (
        "assets/fonts",
        &[
            "forza",
            "forza-ui",
            "forza-panel",
            "wccf-installer",
            "xtask",
        ],
    ),
    (
        "assets/logo",
        &[
            "forza-ui",
            "launcher",
            "wccf-installer",
            "forza-server",
            "forza-programs",
            "xtask",
        ],
    ),
    ("assets/panel", &["forza-ui"]),
    ("editions/2010-11", &["forza", "xtask"]),
    ("editions/2017-18", &["forza", "forza-panel", "xtask"]),
];

/// A file more than one crate compiles, in the folder of the package that happens to hold it: its readers, by name.
/// `crates/launcher/version_rc.rs` is the launcher's and the other eight crates' build scripts' (`#[path]`).
pub(crate) const SHARED_FILES: &[(&str, &[&str])] = &[(
    "crates/launcher/version_rc.rs",
    &[
        "launcher",
        "wccf-installer",
        "forza-panel",
        "forza-server",
        "forza-hook",
        "forza-detmath",
        "forza-icmp",
        "forza-programs",
        "forza-wccfnet",
    ],
)];

/// [`changed_packages`] over `info`: (name, dir relative to the root, names of path dependencies) per package.
pub(crate) fn affected(
    mut info: Vec<(String, String, Vec<String>)>,
    files: &str,
) -> Option<BTreeSet<String>> {
    // files that other crates embed with `include_str!`: (the file's package, the package whose tests read it)
    const EMBEDDED_BY: &[(&str, &str)] = &[("forza-panel", "launcher")];
    for (_, _, deps) in info
        .iter_mut()
        .filter(|(n, _, _)| EMBEDDED_BY.iter().any(|(_, u)| u == n))
    {
        deps.extend(EMBEDDED_BY.iter().map(|(p, _)| (*p).to_owned()));
    }
    let mut hit = BTreeSet::new();
    for f in files.lines() {
        let f = f.to_lowercase(); // git prints the index's spelling, cargo the folder's
        // a folder the crates share: its readers, plus their dependents below
        if f.starts_with("assets/") || f.starts_with("editions/") {
            let owner = FOLDER_OWNERS
                .iter()
                .filter(|(d, _)| f.starts_with(&format!("{d}/")))
                .max_by_key(|(d, _)| d.len());
            match owner {
                // a folder no crate is mapped to: everything, safe until it has a row here
                None => return None,
                Some((_, owners)) => hit.extend(
                    owners
                        .iter()
                        .filter(|o| info.iter().any(|(n, _, _)| n == *o))
                        .map(|o| (*o).to_owned()),
                ),
            }
            continue;
        }
        // a file more than one crate compiles: its readers, plus their dependents below
        if let Some((_, readers)) = SHARED_FILES.iter().find(|(p, _)| f == *p) {
            hit.extend(
                readers
                    .iter()
                    .filter(|o| info.iter().any(|(n, _, _)| n == *o))
                    .map(|o| (*o).to_owned()),
            );
            continue;
        }
        // the locked crates, the toolchain and its config: any crate can differ
        if f == "cargo.lock" || f == "rust-toolchain.toml" || f.starts_with(".cargo/") {
            return None;
        }
        // native/clippy.toml: clippy reads the nearest file above a crate, so every crate under native/ is a reader
        if f == "native/clippy.toml" {
            hit.extend(
                info.iter()
                    .filter(|(_, d, _)| d.starts_with("native/"))
                    .map(|(n, _, _)| n.clone()),
            );
            continue;
        }
        // the owning package: the longest package dir that prefixes the file
        if let Some((n, _, _)) = info
            .iter()
            .filter(|(_, d, _)| !d.is_empty() && f.starts_with(&format!("{d}/")))
            .max_by_key(|(_, d, _)| d.len())
        {
            hit.insert(n.clone());
        } else if f == "cargo.toml" {
            return None;
        }
    }
    // dependents, to a fixed point
    loop {
        let more: Vec<String> = info
            .iter()
            .filter(|(n, _, deps)| !hit.contains(n) && deps.iter().any(|d| hit.contains(d)))
            .map(|(n, _, _)| n.clone())
            .collect();
        if more.is_empty() {
            return Some(hit);
        }
        hit.extend(more);
    }
}

/// The 32-bit crates of [`NATIVE_CRATES`] and the hook: a change to one also lints and tests them for [`PANEL_TARGET`].
pub(crate) const NATIVE_PKGS: &[&str] = &[
    "forza-native",
    "forza-hook",
    "forza-wccfnet",
    "forza-detmath",
    "forza-netguard",
    "forza-icmp",
    "forza-programs",
];

/// The cargo arguments of a scope (`--workspace` or `-p` pairs) split by target: forza-panel is a 32-bit crate, empty on
/// the 64-bit target, so its lints and tests (the fakegame stages too) run for [`PANEL_TARGET`] or not at all. The other
/// 32-bit crates ([`NATIVE_PKGS`]) stay on the 64-bit target (the hook's tests run there too) and, in scope, are added
/// to the 32-bit run as `build` builds them, the hook with `hook` (its feature).
pub(crate) fn target_scopes(scope: &[String], hook: &str) -> Vec<(&'static str, Vec<String>)> {
    let all = scope.first().is_some_and(|a| a == "--workspace");
    let has_panel = all || scope.iter().any(|a| a == "forza-panel");
    let has_native = all || scope.iter().any(|a| NATIVE_PKGS.contains(&a.as_str()));
    let rest: Vec<String> = if all {
        ["--workspace", "--exclude", "forza-panel"]
            .map(Into::into)
            .into()
    } else {
        scope
            .chunks(2)
            .filter(|c| c[1] != "forza-panel")
            .flatten()
            .cloned()
            .collect()
    };
    let mut x86: Vec<String> = vec![];
    if has_panel {
        x86.extend(["-p", "forza-panel"].map(String::from));
    }
    if has_native {
        x86.extend(["-p", "forza-hook", "--features", hook].map(String::from));
        x86.extend(NATIVE_CRATES.iter().map(ToString::to_string));
    }
    [(TARGET, rest), (PANEL_TARGET, x86)]
        .into_iter()
        .filter(|(_, s)| !s.is_empty())
        .collect()
}

/// `cargo check -p forza-server` on its own: in a workspace build another crate turns on forza's `full`, so a call
/// into a `full`-only module passed the workspace's checks and broke the server build (2026-11-17). `check quick`
/// and `check ci` both run it.
pub(crate) fn server_check(root: &Path) -> Command {
    let mut c = rust("check", root);
    c.args(["--locked", "-p", "forza-server"]);
    c
}

/// `cargo xtask check quick`: fmt, the whole workspace's clippy and the tests of the packages changed since `base`, then
/// the kit's self-tests when a 32-bit crate changed, the Launcher's imports and the installer's checks when theirs did.
/// The 2010-11 hooks only when [`touches_1011`]. Pull requests' CI runs it (in the `ci` profile); no release build. The
/// GPU snapshot tests are skipped wherever it runs: their pixels move in the `fast` profile this uses locally.
pub(crate) fn quick(
    root: &Path,
    stage_root: &Path,
    kit: &Path,
    kit_1718: &Path,
    base: &str,
) -> Result<()> {
    let _lock = check_lock()?;
    let files = changed_files(root, base)?;
    if files.is_none() {
        println!("quick: no merge base with {base} (not fetched?): everything");
    }
    let pkgs = match &files {
        Some(f) => changed_packages(root, f)?,
        None => None,
    };
    let e1011 = ci_builds_1011(false, files.as_deref());
    let scope: Vec<String> = match &pkgs {
        None => {
            println!("quick: Cargo.lock, the toolchain or a manifest changed: whole workspace");
            vec!["--workspace".into()]
        }
        Some(p) if p.is_empty() => {
            println!("quick: no crate changed since {base}: fmt only");
            vec![]
        }
        Some(p) => {
            println!(
                "quick: tests only for {}; clippy lints the whole workspace",
                p.iter().cloned().collect::<Vec<_>>().join(", ")
            );
            p.iter()
                .flat_map(|n| ["-p".to_owned(), n.clone()])
                .collect()
        }
    };
    println!(
        "quick: 2010-11 hooks {}",
        ci_1011_reason(false, Some(base), files.as_deref())
    );
    let has = |p: &str| pkgs.as_ref().is_none_or(|s| s.contains(p));
    let native = NATIVE_PKGS.iter().any(|p| has(p));
    // the installer's notices and smoke run only when the files they read changed, not for any crate it depends on,
    // and both when the whole workspace is in scope (`pkgs` is `None`: the lock, the toolchain or the root manifest)
    let installer = installer_work(files.as_deref(), pkgs.is_none());
    // the launcher's and the panel's tests start the stand-ins and the panel DLL that `cargo xtask build` puts in the kit,
    // and the installer's smoke the exe it builds there: one another profile's run left (`release` from `package`) is not
    // this run's, and neither is a missing one
    let exe_missing = installer.smoke && !installer_exe(root, profile()).is_file();
    let needs_kit = kit_needed(has) || exe_missing;
    let mut fmt = Command::new("cargo");
    fmt.args(["fmt", "--all", "--check"]).current_dir(root);
    run_ok(fmt, "cargo fmt")?;
    if scope.is_empty() {
        return Ok(());
    }
    // a kit built without the 2010-11 hook has no wccfk.dll (build_all)
    if needs_kit
        && (exe_missing
            || kit_stale(root, kit)?
            || e1011 && !kit.join("programs/wccfk.dll").is_file())
    {
        println!(
            "quick: the staged kit or installer is missing, older than the sources or from another profile: rebuilding"
        );
        build_all(root, kit, kit_1718, e1011)?;
    }
    let hook = hook_feature(e1011);
    // The changed crates only pick the targets: clippy lints the whole workspace of each, `check`'s scope, whose units
    // a cache warm from `check ci` holds. A `-p` set resolves features differently and re-checked the dependencies:
    // warm on 2026-11-21, `-p launcher -p wccf-installer` re-checked 104 crates in 51 s, where the old narrow clippy
    // took 73 s for the same two crates. `server_check` below is now the only step that builds a crate under its own
    // feature set.
    // shortcut: a crate that only builds because another workspace member enables a feature now passes here; a
    // `cargo check -p` per changed crate would cover it at those 51 s a run, so upgrade if that cost drops.
    let targets: Vec<&str> = target_scopes(&scope, hook)
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    for cmd in quick_clippy(root, &scope, hook) {
        run_ok(cmd, "cargo clippy")?;
    }
    // the server alone, which a workspace build's feature unification hides
    run_ok(server_check(root), "cargo check -p forza-server")?;
    // the hook's other features, as `check` lints them
    if native {
        for (feature, _, _) in hooks(e1011).skip(1) {
            let mut clippy = rust("clippy", root);
            clippy.args(["--locked", "--all-targets", "--target", PANEL_TARGET]);
            clippy.args([
                "-p",
                "forza-hook",
                "--features",
                feature,
                "--",
                "-D",
                "warnings",
            ]);
            run_ok(clippy, "cargo clippy (32-bit)")?;
        }
    }
    for &target in &targets {
        let only = match &pkgs {
            None => None,
            Some(p) => match test_filter(p, target) {
                Some(f) => Some(f),
                None => continue,
            },
        };
        let wide = wide_scope(target, hook)?;
        let wide: Vec<&str> = wide.iter().map(String::as_str).collect();
        run_ok(
            tests(root, true, &[], target, &wide, only.as_deref()),
            "cargo test",
        )?;
    }
    let mut ck = Checks {
        kit: kit.to_owned(),
        tmp: stage_root.join("check-tmp"),
        failed: vec![],
        unstarted: false,
    };
    if native || has("forza-panel") {
        windows_tests(&mut ck, kit, kit_1718, e1011)?;
    }
    if has("launcher") {
        launcher_imports(&mut ck, kit_1718);
    }
    if installer.any() {
        installer_checks(root, stage_root, installer, &mut ck.failed);
    }
    // a program that could not start is xtask's failure (plain error), one that ran and failed the check's
    ensure!(!ck.unstarted, "FAILED: {}", ck.failed.join(", "));
    if !ck.failed.is_empty() {
        return Err(CheckFailed {
            stage: ck.failed.join("+"),
            msg: format!("FAILED: {}", ck.failed.join(", ")),
        }
        .into());
    }
    println!("quick check passed (no release build; the full check and `check ci` run everything)");
    Ok(())
}

/// The nextest filterset of `pkgs` ([`changed_packages`]) built for `target` (the split of [`target_scopes`]): `None`
/// when none is, so no `package()`-less `()` reaches nextest.
pub(crate) fn test_filter(pkgs: &BTreeSet<String>, target: &str) -> Option<String> {
    let on = |p: &str| match target {
        PANEL_TARGET => p == "forza-panel" || NATIVE_PKGS.contains(&p),
        _ => p != "forza-panel",
    };
    let f: Vec<String> = pkgs
        .iter()
        .filter(|p| on(p))
        .map(|p| format!("package({p})"))
        .collect();
    (!f.is_empty()).then(|| f.join(" | "))
}

/// The staged programs `check` runs or ships; wccfk.dll only with `e1011` (`build_all` removes it without).
pub(crate) fn required_programs(e1011: bool) -> impl Iterator<Item = &'static str> {
    PROGRAMS
        .iter()
        .copied()
        .filter(move |p| e1011 || *p != "programs/wccfk.dll")
        .chain([
            "source/build/fakegame.exe",
            "source/build/fakegame_iphlp.exe",
            "source/build/test_dhcphook.exe",
            "source/build/crashme.exe",
        ])
}

/// The staged kits must hold the built programs and the test programs.
pub(crate) fn ensure_built(kit: &Path, kit_1718: &Path, e1011: bool) -> Result<()> {
    for p in required_programs(e1011) {
        ensure!(
            kit.join(p).is_file(),
            "{p} is missing: run `cargo xtask build` first"
        );
    }
    for p in PROGRAMS_1718.iter().chain(&[
        "source/build/test_wccfnet.exe",
        "source/build/netguard.dll",
        "source/build/test_netguard.exe",
        "source/build/test_panel.exe",
        "source/build/panel_probe.dll",
        "source/build/test_icmp.exe",
        "source/build/test_logowin.exe",
    ]) {
        ensure!(
            kit_1718.join(p).is_file(),
            "2017-18 {p} is missing: run `cargo xtask build` first"
        );
    }
    // bin\winmm.dll answers control's start of logowin.exe (2026-10-29): no stand-in exe in the kit
    ensure!(
        !kit_1718.join("bin/logowin.exe").exists(),
        "2017-18 bin/logowin.exe must not be in the kit"
    );
    Ok(())
}

/// The 2010-11 hook's kit test, only with `e1011`: without the hooks there is no wccfk.dll to point the imports at.
pub(crate) fn hook_test(kit: &Path, e1011: bool) -> Option<(&'static str, Command)> {
    e1011.then(|| {
        let mut c = Command::new(kit.join("source/build/test_dhcphook.exe"));
        c.arg(kit.join("programs/wccfk.dll"));
        (
            "2010-11 hook: the IpReleaseAddress and IpRenewAddress imports now point into wccfk.dll",
            c,
        )
    })
}

/// The checks that start Windows programs from the staged kits; the 2010-11 hook's only with `e1011`.
pub(crate) fn windows_tests(
    ck: &mut Checks,
    kit: &Path,
    kit_1718: &Path,
    e1011: bool,
) -> Result<()> {
    if let Some((what, c)) = hook_test(kit, e1011) {
        ck.run(what, c, false);
    }
    panel_selftests(ck)?;
    guard_tests(ck, kit_1718)
}

/// Runs a cargo command; a failure adds `what` to `failed`.
pub(crate) fn cargo_step(what: &str, mut c: Command, failed: &mut Vec<String>) {
    if !c.status().is_ok_and(|s| s.success()) {
        failed.push(what.to_owned());
    }
}

/// The workspace's format and lint (the toolchain from rust-toolchain.toml, read in the root).
pub(crate) fn fmt_clippy(root: &Path, e1011: bool, failed: &mut Vec<String>) {
    let mut fmt = Command::new("cargo");
    fmt.args(["fmt", "--all", "--check"]).current_dir(root);
    cargo_step("workspace: cargo fmt", fmt, failed);
    // `check quick` lints the same two scopes through the same function, so the cache entry keeps its shape
    let hook = hook_feature(e1011);
    let mut wide = quick_clippy(root, &["--workspace".to_owned()], hook).into_iter();
    cargo_step(
        "workspace: cargo clippy",
        wide.next().expect("the 64-bit workspace's scope"),
        failed,
    );
    cargo_step(
        &format!("32-bit: cargo clippy ({hook})"),
        wide.next().expect("the 32-bit scope"),
        failed,
    );
    // the hook's other features: as `build` builds them, once per edition
    for (feature, _, _) in hooks(e1011).skip(1) {
        let mut clippy = rust("clippy", root);
        clippy.args([
            "--locked",
            "--all-targets",
            "--target",
            PANEL_TARGET,
            "-p",
            "forza-hook",
            "--features",
            feature,
            "--",
            "-D",
            "warnings",
        ]);
        cargo_step(&format!("32-bit: cargo clippy ({feature})"), clippy, failed);
    }
}

/// `ci`: the GPU snapshot tests are skipped. `e1011`: the 2010-11 hooks were built and are linted and tested.
pub(crate) fn check(
    root: &Path,
    stage_root: &Path,
    kit: &Path,
    kit_1718: &Path,
    ci: bool,
    e1011: bool,
    test_args: &[String],
) -> Result<()> {
    // one check at a time on this PC: the tests use fixed local ports (the relay on 20941, the seat desk on 20932-20936),
    // and two checks at once (two worktrees) failed each other's relay checks
    let _lock = check_lock()?;
    ensure_built(kit, kit_1718, e1011)?;
    stage(root, kit, MAP, KEEP)?;
    let mut ck = Checks {
        kit: kit.to_owned(),
        tmp: stage_root.join("check-tmp"),
        failed: vec![],
        unstarted: false,
    };
    windows_tests(&mut ck, kit, kit_1718, e1011)?;
    launcher_imports(&mut ck, kit_1718);

    // the Rust workspace, xtask included: format, lint, tests
    fmt_clippy(root, e1011, &mut ck.failed);
    // the server alone, as `check quick` runs it: the workspace build turns on forza's `full`, which hides a server
    // that no longer builds without it
    cargo_step(
        "cargo check -p forza-server",
        server_check(root),
        &mut ck.failed,
    );
    // the 64-bit workspace, then the 32-bit crates (the panel; FPR_Emu's table file and frame; the hook's tests run on
    // x86_64 too)
    let native: Vec<&str> = [
        "-p",
        "forza-panel",
        "-p",
        "forza-hook",
        "--features",
        hook_feature(e1011),
    ]
    .into_iter()
    .chain(NATIVE_CRATES.iter().copied())
    .collect();
    for (what, target, scope) in [
        (
            "workspace: cargo test",
            TARGET,
            ["--workspace", "--exclude", "forza-panel"].as_slice(),
        ),
        ("32-bit: cargo test", PANEL_TARGET, native.as_slice()),
    ] {
        cargo_step(
            what,
            tests(root, ci, test_args, target, scope, None),
            &mut ck.failed,
        );
    }

    scrub(root, &mut ck.failed)?;
    installer_checks(root, stage_root, InstallerWork::ALL, &mut ck.failed);
    if let Err(e) = shipped_docs_no_kit(root, &[kit, kit_1718]) {
        eprintln!("shipped docs say kit: {e:#}");
        ck.failed.push("shipped docs say kit".into());
    }
    no_script_host(root, &mut ck.failed)?;
    no_python(&[kit, kit_1718], &mut ck.failed)?;

    ensure!(ck.failed.is_empty(), "FAILED: {}", ck.failed.join(", "));
    println!("all checks passed");
    Ok(())
}

/// The workspace's tests: cargo-nextest when it is installed (one process per test, all cores, `--partition` for
/// split CI jobs), else `cargo test`. `skip_snapshots` leaves out the GPU snapshot tests; `--fast` runs them in the
/// kits' profile. nextest runs no doctests; the workspace has none.
/// `only`: a nextest filterset of the tests to run (all of `scope` without nextest).
pub(crate) fn tests(
    root: &Path,
    skip_snapshots: bool,
    extra: &[String],
    target: &str,
    scope: &[&str],
    only: Option<&str>,
) -> Command {
    let nextest = Command::new("cargo")
        .args(["nextest", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    let mut test = rust(if nextest { "nextest" } else { "test" }, root);
    if nextest {
        test.arg("run");
    }
    test.args(["--locked", "--target", target]).args(scope);
    // the kits' profile and rustflags, so the tests reuse their crates; release tests in the usual test profile
    if profile() != "release" {
        test.args(remap_config(root));
        test.args([
            if nextest {
                "--cargo-profile"
            } else {
                "--profile"
            },
            profile(),
        ]);
    }
    if nextest {
        // CI selects nextest's own `ci` profile (retries and no fail-fast, .config/nextest.toml), not a cargo one
        if in_ci() {
            test.args(["--profile", "ci"]);
        }
        // one expression: nextest runs the union of several -E
        let expr: Vec<String> = only
            .map(|o| format!("({o})"))
            .into_iter()
            .chain(skip_snapshots.then(|| "not test(/^snapshots::/)".to_owned()))
            .collect();
        if !expr.is_empty() {
            test.args(["-E", &expr.join(" & ")]);
        }
        // a filtered package may have no tests (forza-icmp has none): nothing to run is a pass
        if only.is_some() {
            test.arg("--no-tests=pass");
        }
        test.args(extra);
    } else {
        test.arg("--");
        if skip_snapshots {
            test.args(["--skip", "snapshots::"]);
        }
        test.args(extra);
    }
    test
}

/// `cargo clippy` of one target over `scope`: `check` and `check quick` both pass the whole workspace of each target.
pub(crate) fn clippy(root: &Path, target: &str, scope: &[String]) -> Command {
    let mut c = rust("clippy", root);
    c.args(["--locked", "--all-targets", "--target", target]);
    c.args(scope).args(["--", "-D", "warnings"]);
    c
}

/// The whole workspace's scope for `target`, which is what `check` lints and tests: a CI cache warm from `check ci`
/// holds those units, so cargo only re-checks the crates that changed and their dependents.
pub(crate) fn wide_scope(target: &str, hook: &str) -> Result<Vec<String>> {
    target_scopes(&["--workspace".to_owned()], hook)
        .into_iter()
        .find(|(t, _)| *t == target)
        .map(|(_, s)| s)
        .with_context(|| format!("no workspace scope for {target}"))
}

/// The clippy commands `check quick` runs for the crates in `scope`: the whole workspace of each target they are built
/// for, never a `-p` set of just those crates. A `-p` set resolves features differently from main's cached `--workspace`
/// clippy, so cargo re-checked the dependencies (2026-11-21: 104 crates, 51 s warm for a two-crate change); with that
/// cache only the changed crates and their dependents are linted either way.
///
/// The 32-bit scope's hook feature is `hook`: on a branch that skips the 2010-11 hooks it is `forza-hook/e1718`, where
/// main's `check ci` lints `forza-hook/e1011`, so that target re-checks forza-hook and its dependents; the panel's stack
/// does not depend on the hook and stays cached.
pub(crate) fn quick_clippy(root: &Path, scope: &[String], hook: &str) -> Vec<Command> {
    let wide = target_scopes(&["--workspace".to_owned()], hook);
    target_scopes(scope, hook)
        .into_iter()
        .filter_map(|(target, _)| {
            let (_, scope) = wide.iter().find(|(t, _)| *t == target)?;
            Some(clippy(root, target, scope))
        })
        .collect()
}

/// `cargo xtask test -p <crate>`: the tests of the crates named, built as `check quick` builds them (the wide
/// workspace scope of each target, same profile and rustflags), so a per-commit run reuses the gate's build tree
/// instead of making a second one; nextest's filterset then picks the crates named.
pub(crate) fn test_packages(
    root: &Path,
    kit: &Path,
    packages: &[String],
    extra: &[String],
) -> Result<()> {
    // a rebuild on every run would defeat the per-commit use, and a kit older than the sources only runs the tests'
    // programs from before the change: one line names the fix, as a missing kit fails in the tests themselves
    let has = |p: &str| packages.iter().any(|x| x == p);
    if kit_needed(has) && kit_stale(root, kit)? {
        println!(
            "test: the staged kit is missing or older than the sources: `cargo xtask build` first, or the tests start the old programs"
        );
    }
    // one test run at a time on this PC: the tests use fixed local ports (the relay on 20941, the seat desk on 20932-20936)
    let _lock = check_lock()?;
    for c in test_commands(root, packages, extra) {
        run_ok(c, "cargo test")?;
    }
    Ok(())
}

/// The tests that start the programs `cargo xtask build` stages in the kit: the launcher's and the panel's stand-ins,
/// the installer's and the native crates' own.
pub(crate) fn kit_needed(has: impl Fn(&str) -> bool) -> bool {
    has("launcher") || has("forza-panel") || has(INSTALLER) || NATIVE_PKGS.iter().any(|p| has(p))
}

/// The commands [`test_packages`] runs: one per target the crates named build for, as `check quick`'s test step.
pub(crate) fn test_commands(root: &Path, packages: &[String], extra: &[String]) -> Vec<Command> {
    let pkgs: BTreeSet<String> = packages.iter().cloned().collect();
    let scope: Vec<String> = pkgs
        .iter()
        .flat_map(|p| ["-p".to_owned(), p.clone()])
        .collect();
    let hook = hook_feature(false);
    let wide = target_scopes(&["--workspace".to_owned()], hook);
    target_scopes(&scope, hook)
        .into_iter()
        .filter_map(|(target, _)| {
            let only = test_filter(&pkgs, target)?;
            let wide: Vec<&str> = wide
                .iter()
                .find(|(t, _)| *t == target)?
                .1
                .iter()
                .map(String::as_str)
                .collect();
            // the GPU snapshot tests are skipped, as in `quick`: their pixels move in the `fast` profile
            Some(tests(root, true, extra, target, &wide, Some(&only)))
        })
        .collect()
}

/// `cargo xtask check text`: the checks that read only tracked text; no build, no kit.
pub(crate) fn check_text(root: &Path) -> Result<()> {
    let mut failed = vec![];
    scrub(root, &mut failed)?;
    no_script_host(root, &mut failed)?;
    no_nul(root, &mut failed)?;
    let src = root.join("crates/installer");
    if let Err(e) = no_kit_word(&[src.join("README.md"), src.join("CHANGELOG.md")]) {
        eprintln!("installer docs say kit: {e:#}");
        failed.push("installer docs say kit".into());
    }
    ensure!(failed.is_empty(), "FAILED: {}", failed.join(", "));
    println!("text checks passed");
    Ok(())
}

/// The panel's money and relay self-tests: forza_panel.dll loaded by fakegame.exe in a scratch folder (the 2010-11 kit's
/// copy, so its fonts are found above it); the relay test plays two seats through `forza-server relay` on 127.0.0.1 (port 0) -
/// the panel's and the server's sides of the match relay checked against each other; the panel must have drawn a frame
/// without a panic. The deal is not here: its odds are forza::cards' unit test, its window crates/panel's fakegame stage 6.
/// The frame-wait timing stays out: a busy machine fails it (crates/panel's unit tests cover its arithmetic).
pub(crate) fn panel_selftests(ck: &mut Checks) -> Result<()> {
    let what = "panel self-tests";
    let kit = ck.kit.clone();
    let t = ck
        .tmp
        .join(format!("forza-panel-check-{}", std::process::id()));
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(t.join("data"))?;
    fs::copy(
        kit.join("source/build/fakegame.exe"),
        t.join("fakegame.exe"),
    )?;
    let mut relay = Command::new(kit.join("programs/forza-server.exe"));
    // port 0: two checks on this PC (two worktrees) never share a relay; the panel gets the port it bound
    relay.args(["relay", "30"]).env("WCCF_RELAY_PORT", "0");
    let mut relay: Child = relay.stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    // waits for its "listening on UDP <port>" line (a fixed second lost the race under load, 2026-10-13); the rest of its
    // output is drained, so its prints never block or break
    let out = std::io::BufReader::new(relay.stdout.take().context("the relay's output")?);
    let (up, listening) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut up = Some(up);
        for l in std::io::BufRead::lines(out).map_while(Result::ok) {
            if let Some(port) = l.split("listening on UDP ").nth(1)
                && let Some(u) = up.take()
            {
                let _ = u.send(port.trim().to_owned());
            }
        }
    });
    // no line: port 0 makes the panel's relay test fail and say so
    let port = listening
        .recv_timeout(Duration::from_secs(20))
        .unwrap_or_else(|_| "0".into());
    let mut game = Command::new(t.join("fakegame.exe"));
    game.current_dir(&t)
        .arg(kit.join("programs/forza_panel.dll"))
        .arg("wait:12000")
        .env("WCCFPANEL_ROLE", "seat1") // the cabinet's code path (the panel would take a folder named "extracted" for the projector)
        .env("WCCFPANEL_DRYKEYS", "1") // keys logged, never pressed
        .env("WCCFPANEL_FULLSCREEN", "0") // never maximized
        .env("WCCF_DATA", t.join("data"))
        .env("WCCF_KEYS", t.join("data/keys.txt"))
        .env("WCCFPANEL_MONEYTEST", "1")
        .env("WCCFPANEL_RELAYTEST", "1")
        .env("WCCF_RELAY_PORT", port);
    let run = game.status();
    let _ = relay.kill();
    let _ = relay.wait();
    let log = String::from_utf8_lossy(&fs::read(t.join("forza_panel.log")).unwrap_or_default())
        .into_owned();
    let log: Vec<&str> = log.lines().collect();
    let tests = log
        .iter()
        .filter(|l| l.contains(" test: PASS ") || l.contains(" test: FAIL "))
        .count();
    let bad: Vec<_> = log.iter().filter(|l| l.contains(" test: FAIL ")).collect();
    let money = log.iter().any(|l| {
        l.split_once("money test: ").is_some_and(|(_, r)| {
            r.trim_start_matches(|c: char| c.is_ascii_digit())
                .starts_with(" checks")
                && !r.starts_with(' ')
        })
    });
    let done = money
        && log.iter().any(|l| l.contains("relay test: done"))
        && log.iter().any(|l| l.contains("first frame shown"))
        && !log.iter().any(|l| l.contains("panicked"));
    for l in &bad {
        println!("{l}");
    }
    println!(
        "panel self-tests: {tests} checks, {} failed{}",
        bad.len(),
        if done { "" } else { " - DID NOT FINISH" }
    );
    if !run.is_ok_and(|s| s.success()) || !bad.is_empty() || !done || tests == 0 {
        for l in &log[log.len().saturating_sub(30)..] {
            println!("  log: {l}");
        }
        ck.failed.push(what.to_owned());
    }
    let _ = fs::remove_dir_all(&t);
    Ok(())
}

/// The 2017-18 network guard, never the real IP helper calls: test_wccfnet.exe loads wccfnet.dll from a scratch folder and calls
/// its four stubs (each must return 0 and log a line); test_netguard.exe imports the four IP helper calls and checks netguard.dll
/// (not shipped: the 2010-11 Linux server loads it) moved them into itself before calling anything; test_panel.exe checks that FORZA_PANEL makes it load a probe DLL once a
/// window shows, and that without the variable nothing loads; test_logowin.exe checks that the 2017-18 hook answers a start
/// of logowin.exe without starting anything.
pub(crate) fn guard_tests(ck: &mut Checks, kit: &Path) -> Result<()> {
    let t = ck.tmp.join(format!("wccfnet-check-{}", std::process::id()));
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(&t)?;
    for (from, to) in [
        ("bin/wccfnet.dll", "wccfnet.dll"),
        ("source/build/test_wccfnet.exe", "test_wccfnet.exe"),
        ("source/build/netguard.dll", "netguard.dll"),
        ("source/build/test_netguard.exe", "test_netguard.exe"),
        ("source/build/test_panel.exe", "test_panel.exe"),
        ("source/build/panel_probe.dll", "panel_probe.dll"),
        ("bin/ICMP.DLL", "ICMP.DLL"),
        ("source/build/test_icmp.exe", "test_icmp.exe"),
    ] {
        fs::copy(kit.join(from), t.join(to))?;
    }
    for (what, exe, dll) in [
        (
            "the guard wccfnet.dll: four stubs return 0 and log, nothing reaches IPHLPAPI",
            "test_wccfnet.exe",
            "wccfnet.dll",
        ),
        (
            "netguard.dll (the Linux server's second guard): the four IP helper imports now point into it",
            "test_netguard.exe",
            "netguard.dll",
        ),
        (
            "ICMP.DLL stand-in: three exports, IcmpSendEcho returns 0 at once",
            "test_icmp.exe",
            "ICMP.DLL",
        ),
    ] {
        let mut c = Command::new(t.join(exe));
        // the debug trace would add its own lines to wccfnet.log
        c.arg(t.join(dll))
            .current_dir(&t)
            .env_remove("WCCF_TRACE_NET");
        ck.run(what, c, false);
    }
    for (what, mode) in [
        ("wccfnet.dll loads the panel when FORZA_PANEL is set", "on"),
        ("wccfnet.dll loads nothing without FORZA_PANEL", "off"),
    ] {
        let mut c = Command::new(t.join("test_panel.exe"));
        c.arg(t.join("wccfnet.dll"))
            .arg(t.join("panel_probe.dll"))
            .arg(mode)
            .current_dir(&t)
            .env_remove("FORZA_PANEL");
        if mode == "on" {
            c.env("FORZA_PANEL", t.join("panel_probe.dll"));
        }
        ck.run(what, c, false);
    }
    // a folder of its own: no other test program there loads the hook as its winmm.dll
    let lw = t.join("logowin");
    fs::create_dir_all(&lw)?;
    fs::copy(kit.join("bin/winmm.dll"), lw.join("winmm.dll"))?;
    fs::copy(
        kit.join("source/build/test_logowin.exe"),
        lw.join("test_logowin.exe"),
    )?;
    let mut c = Command::new(lw.join("test_logowin.exe"));
    c.arg(lw.join("winmm.dll"))
        .current_dir(&lw)
        .env_remove("MXHOOK_LOG_DIR");
    ck.run(
        "2017-18 hook winmm.dll: a start of logowin.exe is answered in-process, no new process",
        c,
        false,
    );
    let _ = fs::remove_dir_all(&t);
    Ok(())
}

/// The Launcher (64-bit, with vslink and iroh in it) must not import the IP helper calls wccfnet.dll stubs: iroh's netwatch
/// reads adapters and routes (GetAdaptersAddresses and the like), which is fine, but a release, renew or route write never.
pub(crate) fn launcher_imports(ck: &mut Checks, kit: &Path) {
    const BLOCKED: [&str; 4] = [
        "IpReleaseAddress",
        "IpRenewAddress",
        "CreateIpForwardEntry",
        "SetIpForwardEntry",
    ];
    let what = "Launcher.exe imports none of the four guarded IP helper calls";
    let found = || -> Result<Vec<String>> {
        use object::LittleEndian as LE;
        use object::pe::ImageNtHeaders64;
        use object::read::pe::{Import, PeFile64};
        let data = fs::read(kit.join("Launcher.exe"))?;
        let pe = PeFile64::parse(&*data)?;
        let mut found = vec![];
        let Some(table) = pe.import_table()? else {
            return Ok(found);
        };
        let mut descs = table.descriptors()?;
        while let Some(d) = descs.next()? {
            let lib = String::from_utf8_lossy(table.name(d.name.get(LE))?).into_owned();
            let first = match d.original_first_thunk.get(LE) {
                0 => d.first_thunk.get(LE),
                rva => rva,
            };
            let mut thunks = table.thunks(first)?;
            while let Some(t) = thunks.next::<ImageNtHeaders64>()? {
                if let Import::Name(_, n) = table.import::<ImageNtHeaders64>(t)? {
                    let n = String::from_utf8_lossy(n);
                    if BLOCKED.contains(&&*n) {
                        found.push(format!("{n} from {lib}"));
                    }
                }
            }
        }
        Ok(found)
    };
    match found() {
        Ok(f) if f.is_empty() => println!("ok: {what}"),
        Ok(f) => ck.fail(what, &f.join(", ")),
        Err(e) => ck.fail(what, &format!("{e:#}")),
    }
}
