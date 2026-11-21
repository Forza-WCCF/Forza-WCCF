// ---- the host ----
use super::*;

/// Where the kits are staged and the checks run: `FORZA_STAGE_ROOT`, else `target/`.
pub(crate) fn stage_root(root: &Path) -> PathBuf {
    std::env::var_os("FORZA_STAGE_ROOT").map_or_else(|| root.join("target"), PathBuf::from)
}

/// `cargo <sub>` in the repository (MSVC's linker, the CRT and SDK Visual Studio installed).
pub(crate) fn rust(sub: &str, root: &Path) -> Command {
    let mut c = Command::new("cargo");
    c.arg(sub).current_dir(root);
    if STAMP_SHA.load(Ordering::Relaxed) {
        // `package` (the only task that stamps the sha): no wrapper from the caller's environment or its config, empty = off
        // (cargo's `does_not_use_empty_rustc_wrapper`). Cargo hashes the wrapper into every unit, so the empty one rebuilds
        // all and no shipped program comes from the cache; the workspace wrapper caches the workspace crates themselves.
        c.env("RUSTC_WRAPPER", "");
        c.env("RUSTC_WORKSPACE_WRAPPER", "");
    } else if let Some(w) = WRAPPER.get() {
        c.env("RUSTC_WRAPPER", w);
    }
    c.env("FORZA_VERSION", build_version(root));
    // the `fast` profile stays incremental: sccache serves the dependencies either way (cargo never compiles those
    // incrementally), and an incremental rebuild of the changed workspace crate beats a whole one (measured)
    if NO_INCREMENTAL.load(Ordering::Relaxed) && profile() != "fast" {
        c.env("CARGO_INCREMENTAL", "0");
    }
    c
}

/// `cargo build` of programs a player gets, with no path of the build PC in them: the panic messages of crates.io code
/// carry their source paths, and 0.3.0's Launcher.exe held `C:\Users\<builder>\.cargo\registry\...` 2,718 times.
/// rustc applies the last matching `--remap-path-prefix`, so the user folder goes before the cargo home inside it.
/// `--config` arrays are joined with .cargo/config.toml's rustflags, not put in their place.
pub(crate) fn rust_shipped(root: &Path) -> Command {
    let mut c = rust("build", root);
    c.args(remap_config(root));
    c
}

/// The `--config` arguments of `rust_shipped`. The tests in the kits' profile pass them too: other target rustflags give
/// every crate another hash, and the test build would compile the whole workspace a second time after the kits.
pub(crate) fn remap_config(root: &Path) -> Vec<String> {
    remap_flags(shared_dir().as_deref(), build_dir(root).as_deref(), root)
}

/// Cargo's `build-dir` (`cargo metadata`'s `build_directory`), when the PC sets one outside `target\`: this one builds
/// into `C:/forza-build/{workspace-path-hash}`, and the build scripts' `OUT_DIR` under it reaches the shipped programs.
/// `None` when it is the target folder itself, as on the runners (the remap above covers that one already).
pub(crate) fn build_dir(root: &Path) -> Option<PathBuf> {
    static ONCE: OnceLock<Option<PathBuf>> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .current_dir(root)
            .output()
            .ok()
            .and_then(|out| serde_json::from_slice::<serde_json::Value>(&out.stdout).ok())
            .and_then(|meta| meta["build_directory"].as_str().map(PathBuf::from));
        let Some(dir) = dir else {
            // the local bytes then differ from a runner's, with a `build-dir` path in them
            eprintln!(
                "warning: cargo metadata gave no build_directory; a build-dir outside target\\ is not remapped"
            );
            return None;
        };
        outside_target(dir, &target_dir(root))
    })
    .clone()
}

/// `dir` when it is not the target folder, compared the way paths are spelled on Windows (either separator, any
/// case): a runner, whose build folder is `target\`, gets no extra flag and keeps its crate hashes.
pub(crate) fn outside_target(dir: PathBuf, target: &Path) -> Option<PathBuf> {
    (norm_path(&dir.display().to_string()) != norm_path(&target.display().to_string()))
        .then_some(dir)
}

/// [`remap_config`] with the shared folder (`CARGO_TARGET_DIR`) and the build folder given.
pub(crate) fn remap_flags(shared: Option<&Path>, build: Option<&Path>, root: &Path) -> Vec<String> {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let cargo = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".cargo")));
    // the build scripts' OUT_DIR is the only path of the worktree left in the programs (cargo passes the workspace crates'
    // sources as relative paths); a shared CARGO_TARGET_DIR is mapped to where `target\` would be, so the bytes stay the
    // same and the flags name no worktree (they are part of every crate's hash: one copy of the dependencies per slot)
    let local = match shared {
        Some(t) => (t.to_owned(), Path::new("forza-wccf").join("target")),
        None => (root.to_owned(), PathBuf::from("forza-wccf")),
    };
    let in_both_separators = dir_spellings(&local.0)
        .into_iter()
        .map(|from| (Some(from), local.1.clone()));
    // a `build-dir` is mapped to where `target\` is, so a program built on a PC that sets one spells the same paths as
    // a runner's, and the folder of this PC appears nowhere in it
    let target_as = Path::new("forza-wccf").join("target");
    let in_build_dir: Vec<(Option<PathBuf>, PathBuf)> = build
        .into_iter()
        .flat_map(|dir| {
            dir_spellings(dir)
                .into_iter()
                .map(|from| (Some(from), target_as.clone()))
        })
        .collect();
    let flags: Vec<String> = [(home, PathBuf::from("~")), (cargo, PathBuf::from("cargo"))]
        .into_iter()
        .chain(in_both_separators)
        .chain(in_build_dir)
        .filter_map(|(from, to)| {
            Some(format!(
                "'--remap-path-prefix={}={}'",
                from?.display(),
                to.display()
            ))
        })
        .collect();
    [TARGET, PANEL_TARGET]
        .into_iter()
        .flat_map(|t| {
            [
                "--config".to_owned(),
                format!("target.{t}.rustflags=[{}]", flags.join(", ")),
            ]
        })
        .collect()
}

/// The spellings of a shared folder that `--remap-path-prefix` must match: cargo builds `OUT_DIR` from the raw
/// `CARGO_TARGET_DIR` value, so a `/` there stays in the prefix, and the flag compares it as a literal string.
pub(crate) fn dir_spellings(dir: &Path) -> Vec<PathBuf> {
    let mut seen = vec![dir.to_owned()];
    if cfg!(windows) {
        let slash = PathBuf::from(dir.display().to_string().replace('\\', "/"));
        if slash.display().to_string() != dir.display().to_string() {
            seen.push(slash);
        }
    }
    seen
}

/// A shipped program that still holds a user folder path (see `rust_shipped`).
pub(crate) fn has_user_path(bytes: &[u8]) -> bool {
    bytes
        .windows(7)
        .any(|w| w.eq_ignore_ascii_case(b":\\Users"))
}

pub(crate) fn run_ok(mut c: Command, what: &str) -> Result<()> {
    if c.status().with_context(|| what.to_owned())?.success() {
        Ok(())
    } else {
        Err(CheckFailed {
            stage: what.to_owned(),
            msg: format!("{what} failed"),
        }
        .into())
    }
}

/// The design gallery built for Windows and started from the repository root, where it finds `assets/fonts/`.
/// `FORZA_CARDS` (a folder of card pictures) passes on to it.
pub(crate) fn gallery(root: &Path) -> Result<()> {
    let mut c = rust("build", root);
    c.args(["-p", "forza-ui", "--example", "gallery", "--target", TARGET]);
    run_ok(c, "cargo build --example gallery")?;
    let exe = target_dir(root)
        .join(TARGET)
        .join("debug/examples/gallery.exe");
    let mut g = Command::new(exe);
    g.current_dir(root);
    if let Some(cards) = std::env::var_os("FORZA_CARDS") {
        g.env("FORZA_CARDS", cards);
    }
    run_ok(g, "gallery.exe")
}

/// `cap.exe` (xtask/cap, in no kit) built, then run.
pub(crate) fn cap(game_root: &Path, outdir: &Path) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("no repository root")?;
    let mut b = rust("build", root);
    b.args(["--locked", "-p", "cap", "--target", TARGET]);
    run_ok(b, "cargo build -p cap")?;
    let exe = target_dir(root).join(TARGET).join("debug/cap.exe");
    let mut c = Command::new(exe);
    c.arg(game_root).arg(outdir);
    run_ok(c, "cap")
}
