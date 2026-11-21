// ---- build ----
use super::{
    CheckFailed, ED_1011, ED_1718, INSTALLER, NATIVE, NATIVE_CRATES, PANEL_TARGET, Path, PathBuf,
    Result, Shell, TARGET, cmd, fs, hooks, profile, run_ok, rust_shipped, stage, target_dir,
};

/// Build jobs running at once: the 64-bit programs and the 32-bit ones, joined by `build_all`.
pub(crate) type Jobs<'s> = Vec<std::thread::ScopedJoinHandle<'s, (String, Result<()>)>>;

/// Stage both kits, build the 64-bit and the 32-bit programs at once, then report every failure. `e1011`: also the
/// 2010-11 hooks ([`HOOKS`]); without them the 2010-11 kit holds only the shared programs and the test programs.
pub(crate) fn build_all(root: &Path, kit: &Path, kit_1718: &Path, e1011: bool) -> Result<()> {
    let started = std::time::SystemTime::now();
    for (k, ed) in [(kit, &ED_1011), (kit_1718, &ED_1718)] {
        stage(root, k, ed.map, ed.keep)?;
        for d in ed.dirs {
            fs::create_dir_all(k.join(d))?;
        }
    }
    // programs\ is kept between builds: the C panel it held up to step 9.4c must not ship beside forza_panel.dll
    let _ = fs::remove_file(kit.join("programs/wccfpanel.dll"));
    if !e1011 {
        // no stale hook: `check` and a `quick` that needs it rebuild (ensure_built, quick's kit test)
        let _ = fs::remove_file(kit.join("programs/wccfk.dll"));
    }
    let mut failed = vec![];
    // a build that ran and failed is `verify`'s FAIL; one that could not start, its ERROR
    let mut ran = true;
    std::thread::scope(|s| {
        for j in build(s, root, kit, kit_1718, e1011) {
            let (what, r) = j.join().expect("build thread");
            if let Err(e) = r {
                ran &= e.downcast_ref::<CheckFailed>().is_some();
                failed.push(format!("{what}: {e:#}"));
            }
        }
    });
    if !failed.is_empty() {
        let msg = format!(
            "{} build step(s) failed:\n{}",
            failed.len(),
            failed.join("\n")
        );
        if ran {
            return Err(CheckFailed {
                stage: "build".into(),
                msg,
            }
            .into());
        }
        anyhow::bail!(msg);
    }
    // the same three programs in both kits: Launcher.exe runs either edition (launcher::kit::Edition, from the layout),
    // forza_panel.dll finds its edition the same way
    let release = target_dir(root).join(TARGET).join(profile());
    let panel = target_dir(root)
        .join(PANEL_TARGET)
        .join(profile())
        .join("forza_panel.dll");
    for (from, to) in [
        (release.join("Launcher.exe"), kit.join("Launcher.exe")),
        (
            release.join("forza-server.exe"),
            kit.join("programs/forza-server.exe"),
        ),
        (panel.clone(), kit.join("programs/forza_panel.dll")),
        // beside fakegame.exe: the panel's tests and self-tests load it from there
        (panel.clone(), kit.join("source/build/forza_panel.dll")),
        (release.join("Launcher.exe"), kit_1718.join("Launcher.exe")),
        (panel, kit_1718.join("overlay/forza_panel.dll")),
    ] {
        fs::copy(from, &to)?;
        println!("built {}", to.display());
    }
    // dated at the start (a source saved during the build is newer than the stamp, so the next `quick` rebuilds) and
    // holding the profile and the list of sources (a deleted or renamed one is then not a build with a file that is gone)
    let stamp = built_stamp(kit);
    std::fs::write(&stamp, built_stamp_text(profile(), &sources(root)?))?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&stamp)?
        .set_modified(started)?;
    Ok(())
}

/// Written by [`build_all`] beside the kits; [`kit_stale`] compares the profile and the sources with it.
pub(crate) fn built_stamp(kit: &Path) -> PathBuf {
    kit.with_file_name("built.stamp")
}

/// What [`build_all`] writes in the stamp: the profile the kits were built with, then the sources, one per line.
pub(crate) fn built_stamp_text(profile: &str, sources: &str) -> String {
    format!("{profile}\n{sources}")
}

/// Whether the stamp names another profile than `now`: `check quick` builds `ci` when a wrapper is on, `cargo xtask build`
/// `fast`, and the gate must not test the other one's kit or installer (a stamp from before the profile line is stale too).
pub(crate) fn other_profile(stamp: &str, now: &str) -> bool {
    stamp.split_once('\n').map(|(built, _)| built) != Some(now)
}

/// The staged kit is missing, or a source it is built from changed after the last `build_all` started (a branch switch
/// counts: git writes the files), or the list of sources is not the one it was built from, or it was built in another
/// profile.
pub(crate) fn kit_stale(root: &Path, kit: &Path) -> Result<bool> {
    // the stamp sits outside the edition folders: a kit folder deleted by hand leaves it behind
    let stamp = built_stamp(kit);
    let Ok(built) = fs::metadata(&stamp).and_then(|m| m.modified()) else {
        return Ok(true);
    };
    if !kit.join("source/build/fakegame.exe").is_file()
        || !kit.join("programs/forza_panel.dll").is_file()
    {
        return Ok(true);
    }
    let files = sources(root)?;
    let stamped = fs::read_to_string(&stamp).unwrap_or_default();
    let (_, listed) = stamped.split_once('\n').unwrap_or_default();
    Ok(other_profile(&stamped, profile()) || sources_changed(root, built, listed, &files))
}

/// The files the kits are built from, sorted, as `git ls-files` lists them (the stamp holds this).
pub(crate) fn sources(root: &Path) -> Result<String> {
    let sh = Shell::new()?;
    sh.change_dir(root);
    let dirs = [
        "assets",
        "crates",
        "editions",
        "legacy",
        "native",
        "vendor",
        ".cargo",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
    ];
    let files = cmd!(
        sh,
        "git ls-files -z --cached --others --exclude-standard -- {dirs...}"
    )
    .read()?;
    let mut list: Vec<&str> = files.split('\0').filter(|f| !f.is_empty()).collect();
    list.sort_unstable();
    Ok(list.join("\n"))
}

/// Whether the sources are not the build's: a listed file newer than it, a listed file gone from the tree (deleted and
/// not staged), or another list than the stamp's (deleted and staged, or renamed).
pub(crate) fn sources_changed(
    root: &Path,
    built: std::time::SystemTime,
    listed: &str,
    now: &str,
) -> bool {
    listed.trim_end() != now
        || now
            .split('\n')
            .filter(|f| !f.is_empty())
            .any(|f| match fs::metadata(root.join(f)) {
                Ok(m) => m.modified().is_ok_and(|m| m > built),
                Err(_) => true,
            })
}

pub(crate) fn build<'s>(
    s: &'s std::thread::Scope<'s, '_>,
    root: &'s Path,
    kit: &Path,
    kit_1718: &Path,
    e1011: bool,
) -> Jobs<'s> {
    // the toolchain is pinned by rust-toolchain.toml, the crates by Cargo.lock, the static runtime by .cargo/config.toml
    let mut c = rust_shipped(root);
    c.args([
        "--profile",
        profile(),
        "--locked",
        "-p",
        "launcher",
        "-p",
        "forza-server",
        "-p",
        INSTALLER,
        "--target",
        TARGET,
    ]);
    let launcher = c;
    // 32-bit like the game: the panel (build_all copies it into both kits), the hook once per edition, the guards and
    // the programs beside the game, one build after the other (the hook's builds share its output file)
    let native = |extra: &[&str]| {
        let mut c = rust_shipped(root);
        c.args(["--profile", profile(), "--locked", "--target", PANEL_TARGET])
            .args(extra);
        c
    };
    let hooks: Vec<_> = hooks(e1011).collect();
    let mut first = native(&[
        "-p",
        "forza-panel",
        "-p",
        "forza-hook",
        "--features",
        hooks[0].0,
    ]);
    first.args(NATIVE_CRATES);
    let out = target_dir(root).join(PANEL_TARGET).join(profile());
    let (kit, kit_1718) = (kit.to_owned(), kit_1718.to_owned());
    let to = move |in_1011: bool, rel: &str| (if in_1011 { &kit } else { &kit_1718 }).join(rel);
    let natives = move || -> Result<()> {
        run_ok(first, "cargo build (32-bit)")?;
        for (from, in_1011, rel) in NATIVE {
            fs::copy(out.join(from), to(*in_1011, rel))?;
        }
        for (i, (feature, in_1011, rel)) in hooks.into_iter().enumerate() {
            if i > 0 {
                run_ok(
                    native(&["-p", "forza-hook", "--features", feature]),
                    "cargo build -p forza-hook",
                )?;
            }
            fs::copy(out.join("forza_hook.dll"), to(in_1011, rel))?;
        }
        Ok(())
    };
    vec![
        s.spawn(move || {
            let what = "cargo build -p launcher -p forza-server -p wccf-installer";
            (what.into(), run_ok(launcher, what))
        }),
        s.spawn(move || {
            (
                "the 32-bit programs (native/, forza-panel)".into(),
                natives(),
            )
        }),
    ]
}
