#![deny(clippy::pedantic)]
// the doc comments are the `cargo xtask --help` text, which clap shows as it is, and quotes of file and API names
#![allow(clippy::doc_markdown)]
// each comparison uses a name xtask itself spells, or one already lowercased above it
#![allow(clippy::case_sensitive_file_extension_comparisons)]
// build, package and staging steps read top to bottom; a line limit would split them for its own sake
#![allow(clippy::too_many_lines)]

//! `cargo xtask build | check | package <edition> | stage <edition>`: the 2010-11 kit's build, checks and ZIP, replacing
//! source/build.ps1, check.ps1 and package.ps1; the 2017-18 kit is staged and zipped from its committed programs.
//!
//! Windows is the build host: plain `cargo` with MSVC for every program, the 32-bit ones (the game's DLLs and the programs
//! beside it, `native/`) for i686; the tests run the programs natively.
//! `FORZA_STAGE_ROOT` (default `target/`) is where the kits are staged and the checks run.
//!
//! The kit the players get is staged at `target/kit/2010-11/`, copied from the working tree by one table
//! ([`MAP`]); the build puts its programs there, `check` tests that folder, `package` zips it. The 2017-18 kit
//! ([`MAP_1718`]) is staged at `target/kit/2017-18/` the same way; `stage` only copies.

use anyhow::{Context, Result, ensure};
use clap::Parser;
use fs_err as fs;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use xshell::{Shell, cmd};

#[derive(Parser)]
#[command(about = "Build, check and package Forza-WCCF")]
enum Task {
    /// Build every program (the 32-bit ones beside the game and the launchers) into the staged kits (2010-11 and 2017-18);
    /// the `fast` profile (release without LTO) unless CI is set or `--release`
    Build {
        /// The `fast` profile even under CI (it is the default without CI); `package` always builds release
        #[arg(long, conflicts_with = "release")]
        fast: bool,
        /// The release profile (thin LTO), what `package` builds
        #[arg(long)]
        release: bool,
    },
    /// fmt, clippy, tests, the kit's self-test programs, the scrub; `check quick` = fmt, clippy and the tests of the crates changed since --base (pull requests' CI)
    /// (in the `fast` profile unless CI is set, a wrapper such as sccache is on (`ci` then) or `--release`; it rebuilds the
    /// staged kit first when it is older than the sources)
    ///
    /// sccache is used when it is installed and RUSTC_WRAPPER is unset (not under CI; FORZA_NO_SCCACHE=1 turns it off)
    Check {
        /// `quick`: only the crates changed since the base (and their dependents), the kit tests and smoke of those; the
        /// 2010-11 hooks only when a 2010-11 path changed (PATHS_1011); no release build, no GPU snapshot tests (the
        /// `snapshots` CI job and `cargo nextest run -p forza-ui` run them, in the dev profile).
        /// `ci`: the full check minus the GPU snapshot tests (the GitHub runner has no GPU), in the `ci` profile (no LTO;
        /// `--release` for release)
        /// `text`: the scrub, the script-host check and the installer docs' word check; no build (docs-only changes)
        #[arg(value_parser = ["quick", "ci", "text"])]
        mode: Option<String>,
        /// What `quick` diffs against (its merge-base with HEAD, plus the working tree; default `main`). `ci`: the
        /// 2010-11 hooks only when a 2010-11 path changed since it; without it (the weekly run) or with `--release`, always
        #[arg(long)]
        base: Option<String>,
        /// Build the kits and run the tests in the `fast` profile (the default for `quick` without CI); the full check and `ci` use release and `ci`
        #[arg(long, conflicts_with = "release")]
        fast: bool,
        /// `ci`: the release profile, what players get (otherwise `ci` builds the `ci` profile: no LTO); `quick`: not `fast`
        #[arg(long)]
        release: bool,
        /// Passed on to the test runner, e.g. `-- --partition count:1/2` with cargo-nextest
        #[arg(last = true)]
        test_args: Vec<String>,
    },
    /// The tests of the crates named, built as `check quick` builds them (its target, profile and rustflags, and the wide
    /// workspace scope it builds with): a per-commit test run then reuses the gate's build tree instead of compiling a
    /// second one; the 32-bit hook builds with the 2017-18 feature, as `quick` does when no 2010-11 path changed. The GPU
    /// snapshot tests are skipped, as in `quick` (their pixels move in the `fast` profile): `cargo test --locked -p
    /// forza-ui --test it --target x86_64-pc-windows-msvc -- snapshots:: --test-threads=1` runs them in the dev profile,
    /// as CI does.
    Test {
        /// The crates to test, each as cargo's `-p`; a 32-bit one (forza-panel, the native crates) runs for its own target
        #[arg(short = 'p', long = "package", required = true)]
        package: Vec<String>,
        /// Passed on to the test runner, e.g. `-- --partition count:1/2` with cargo-nextest
        #[arg(last = true)]
        test_args: Vec<String>,
    },
    /// `check quick` behind a preflight, with fixed exit codes and last line for scripts and agents: 0 `VERIFY OK <secs>s`,
    /// 1 `VERIFY FAIL <stage> <secs>s`, 3 `VERIFY NOT-RUN <reason>` (nothing ran: a game runs, "Prefer System32 images"
    /// is on (a shell started from WSL), or the build drive has under 20 GB free), 4 `VERIFY ERROR <msg>`. Each run
    /// appends `date,worktree,base,result,secs` to FORZA_VERIFY_CSV, else to `wccf-builds/verify-runs.csv` beside the
    /// repository when that folder exists
    Verify {
        /// What `quick` diffs against, as `check quick --base`
        #[arg(long, default_value = QUICK_BASE)]
        base: String,
    },
    /// Copy an edition's files into target/kit/<edition>/ (copies only)
    Stage {
        /// 2010-11 or 2017-18
        edition: String,
    },
    /// Save the game windows running from ROOT as PNGs in OUTDIR (builds xtask/cap; Windows.Graphics.Capture per window, PrintWindow fallback, never the screen)
    Cap {
        /// The runtime folder: only game processes whose exe is under it are captured
        root: PathBuf,
        outdir: PathBuf,
    },
    /// Build and open the UI design gallery (crates/forza-ui/examples/gallery.rs)
    Gallery,
    /// The kit ZIP in dist/
    Package {
        /// 2010-11, 2017-18 or installer
        edition: String,
        /// File name without .zip (2017-18 and installer: the version's by default); version.txt comes from git
        #[arg(long)]
        name: Option<String>,
    },
}

const QUICK_BASE: &str = "main";
/// Whether the version a build stamps carries the commit id: the package and tag builds' (a ZIP from between tags names
/// the commit it came from), not an ordinary build's, which then keeps one version string between commits instead of
/// relinking every program with it (SCCACHE-PLAN step 3).
static STAMP_SHA: AtomicBool = AtomicBool::new(false);
/// Set with a wrapper (sccache): `rust()` then builds with `CARGO_INCREMENTAL=0`, so sccache can serve the workspace
/// crates across worktrees.
static NO_INCREMENTAL: AtomicBool = AtomicBool::new(false);
/// The sccache that `use_sccache` found, which `rust()` passes to cargo as RUSTC_WRAPPER.
static WRAPPER: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
/// The cargo profile of the staged programs and `check`'s tests: see [`pick_profile`]; unset, `release`.
static PROFILE: std::sync::OnceLock<&str> = std::sync::OnceLock::new();

fn profile() -> &'static str {
    PROFILE.get().copied().unwrap_or("release")
}

/// The profile of `build` (`mode` "build") and `check <mode>`: `--fast` is `fast`; `ci` is `ci` (`--release`: release);
/// `build` and `check quick` are `fast` without CI or a wrapper and `--release` (`quick` under either: `ci`, which `test`
/// takes too, so a per-commit run shares the gate's build tree); the rest (`package`, the full check) is release.
fn pick_profile(
    mode: Option<&str>,
    fast: bool,
    release: bool,
    ci_or_wrapper: bool,
) -> &'static str {
    match mode {
        _ if fast => "fast",
        Some("ci") if !release => "ci",
        // pull requests' CI and a user's sccache: the `ci` profile main's `check ci` fills the cache with
        Some("quick") if !release && ci_or_wrapper => "ci",
        Some("build" | "quick") if !release && !ci_or_wrapper => "fast",
        _ => "release",
    }
}

/// The build runs under CI (GitHub sets `CI=true`); an empty or `0` value is not CI.
fn in_ci() -> bool {
    std::env::var_os("CI").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Sets `NO_INCREMENTAL` when cargo will run with a wrapper (except in the `fast` profile, see `rust`): RUSTC_WRAPPER from the environment (empty = off), else
/// sccache from PATH or the cargo home's `bin`, unless CI is set or `FORZA_NO_SCCACHE=1` (Windows processes started from
/// WSL do not see the user's `setx` variables, so the variable alone left most local builds without it).
fn use_sccache() {
    let wrapper = match std::env::var_os("RUSTC_WRAPPER") {
        Some(w) => !w.is_empty(),
        None if in_ci()
            || std::env::var_os("FORZA_NO_SCCACHE").is_some_and(|v| v != "0" && !v.is_empty()) =>
        {
            false
        }
        None => {
            let cargo_bin = std::env::var_os("CARGO_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".cargo"))
                })
                .map(|h| h.join("bin"));
            let path = std::env::var_os("PATH").unwrap_or_default();
            let found = std::env::split_paths(&path)
                .chain(cargo_bin)
                .map(|d| d.join("sccache.exe"))
                .find(|p| p.is_file());
            if let Some(p) = &found {
                println!("sccache: {} (FORZA_NO_SCCACHE=1 turns it off)", p.display());
            }
            found.is_some_and(|p| WRAPPER.set(p).is_ok())
        }
    };
    NO_INCREMENTAL.store(wrapper, Ordering::Relaxed);
}

/// Cargo's output folder: `CARGO_TARGET_DIR` (one folder shared by several worktrees), else `target/`.
fn target_dir(root: &Path) -> PathBuf {
    shared_dir().unwrap_or_else(|| root.join("target"))
}

/// `CARGO_TARGET_DIR` without a trailing separator: `remap_config` replaces it as a literal prefix.
fn shared_dir() -> Option<PathBuf> {
    std::env::var_os("CARGO_TARGET_DIR").map(|t| Path::new(&t).components().collect())
}

/// What a run in worktree `here` (an xtask.exe built from `root`) does to a shared folder last built from `last`.
#[derive(Debug, PartialEq, Eq)]
enum Switch {
    /// the folder holds this worktree's build
    Same,
    /// another worktree built last: mark this one's files new, then build
    Touch,
    /// this xtask.exe comes from another worktree: mark the files new and stop, the next `cargo run` rebuilds it
    TouchAndRerun,
}

fn switch(here: &str, root: &str, last: &str) -> Switch {
    let (here, root, last) = (norm_path(here), norm_path(root), norm_path(last.trim()));
    if here != root {
        Switch::TouchAndRerun
    } else if last != here {
        Switch::Touch
    } else {
        Switch::Same
    }
}

/// A shared folder serves one `cargo xtask` run at a time (held until dropped): another worktree's cargo writing there
/// after this worktree marked its files new would leave artifacts newer than them, which cargo calls fresh.
fn target_lock(dir: &Path) -> Result<std::fs::File> {
    fs::create_dir_all(dir)?;
    let path = dir.join(".forza-worktree.lock");
    let lock = std::fs::File::options()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| path.display().to_string())?;
    let (mut waited, mut say_at) = (0u64, 30u64);
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            // no contention: the folder implements no byte-range locks (a share) or a filter denies access, and the
            // wait below would never end
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("cannot lock {}", path.display()));
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
        }
        // a hung holder blocks every worktree on the folder, and a build takes a while: keep waiting, say so now and
        // then (the holder writes the marker only once it has marked its files new)
        if waited == 0 {
            let last = std::fs::read_to_string(dir.join(".forza-worktree")).unwrap_or_default();
            println!(
                "waiting for another `cargo xtask` run on the shared CARGO_TARGET_DIR {} (last built from {}) ...",
                dir.display(),
                last.trim()
            );
            println!(
                "  lock: {}; it frees when that run ends (kill a hung one, the OS frees the lock with it; deleting \
                 the file does not)",
                path.display()
            );
        } else if waited >= say_at {
            say_at += 30;
            println!(
                "still waiting ({waited} s) for another `cargo xtask` run on {} (lock: {})",
                dir.display(),
                path.display()
            );
        }
        std::thread::sleep(Duration::from_secs(1));
        waited += 1;
    }
}

/// A `CARGO_TARGET_DIR` shared by several worktrees holds one copy of each workspace crate: cargo names the artifacts by
/// the crate's path inside the workspace and calls them fresh when the sources are older, so a worktree with older files
/// would get the last worktree's code (cargo 1.99, tried 2026-11-07 and 2026-11-08). The run holds `target_lock`; on a
/// change of worktree (`.forza-worktree` in that folder) every tracked file gets the current time, and cargo rebuilds
/// the workspace crates; dependencies stay.
fn shared_target(root: &Path) -> Result<Option<std::fs::File>> {
    let Some(dir) = shared_dir() else {
        return Ok(None);
    };
    ensure!(
        dir.is_absolute(),
        "CARGO_TARGET_DIR must be a full path, not {}",
        dir.display()
    );
    // `cargo run` names the worktree it runs in; this xtask.exe may have been built from another one, so the root it
    // was built with is no fallback
    let here = std::env::var_os("CARGO_MANIFEST_DIR")
        .and_then(|m| Path::new(&m).parent().map(Path::to_path_buf))
        .context("CARGO_MANIFEST_DIR is not set: with a shared CARGO_TARGET_DIR start xtask with `cargo xtask`")?;
    shared_target_in(&dir, root, &here).map(Some)
}

/// [`shared_target`] with the folder and the worktree given. The lock comes before the marker is read or written, and
/// only a run that goes on to build this worktree writes it.
fn shared_target_in(dir: &Path, root: &Path, here: &Path) -> Result<std::fs::File> {
    let lock = target_lock(dir)?;
    let marker = dir.join(".forza-worktree");
    let last = std::fs::read_to_string(&marker).unwrap_or_default();
    let todo = switch(
        &real_path(here).display().to_string(),
        &real_path(root).display().to_string(),
        &last,
    );
    if todo == Switch::Same {
        return Ok(lock);
    }
    touch_tracked(here)?;
    // the marker is written only by a run that goes on to build this worktree
    ensure!(
        todo == Switch::Touch,
        "this xtask.exe was built from {} (shared CARGO_TARGET_DIR); {} is marked newer now: run the command again",
        root.display(),
        here.display()
    );
    fs::write(&marker, real_path(here).display().to_string())?;
    Ok(lock)
}

/// `p` in the filesystem's own spelling: a junction, a `subst` drive or an 8.3 name (`RUNNER~1`) is the same folder
/// under another name; else `p` as given. The `\\?\` form `canonicalize` returns is dropped for messages and the
/// folder's marker, which `norm_path` reads the same way.
fn real_path(p: &Path) -> PathBuf {
    let real = fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    let shown = real.display().to_string();
    let Some(rest) = shown.strip_prefix("\\\\?\\") else {
        return real;
    };
    PathBuf::from(match rest.strip_prefix("UNC\\") {
        Some(share) => format!("\\\\{share}"),
        None => rest.to_owned(),
    })
}

/// Sets every tracked file of the worktree to the current time.
fn touch_tracked(here: &Path) -> Result<()> {
    let o = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(here)
        .output()?;
    ensure!(
        o.status.success(),
        "git ls-files failed in {}",
        here.display()
    );
    let now = std::time::SystemTime::now();
    let mut skipped = Vec::new();
    for f in o.stdout.split(|b| *b == 0).filter(|f| !f.is_empty()) {
        let path = here.join(String::from_utf8_lossy(f).as_ref());
        match std::fs::File::options().write(true).open(&path) {
            Ok(file) => file
                .set_modified(now)
                .with_context(|| format!("cannot mark {} new", path.display()))?,
            // deleted in the working tree: nothing to build
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => skipped.push(format!("{} ({e})", path.display())),
        }
    }
    if !skipped.is_empty() {
        eprintln!(
            "warning: not marked new, so cargo may use another worktree's build of them:\n  {}",
            skipped.join("\n  ")
        );
    }
    Ok(())
}

const TOP: &str = "Forza-WCCF-2010-11";
const PRODUCT: &str = "Forza-WCCF 2010-11";
const PRODUCT_1718: &str = "Forza-WCCF 2017-18";
const TARGET: &str = "x86_64-pc-windows-msvc";
/// The game is 32-bit, so is a DLL that goes into it.
const PANEL_TARGET: &str = "i686-pc-windows-msvc";

/// The staged kit <- the repository: (kit path, repository path). Whatever is not here does not ship.
const MAP: &[(&str, &str)] = &[
    ("README.txt", "editions/2010-11/README.txt"),
    ("CHANGELOG.md", "editions/2010-11/CHANGELOG.md"),
    ("fonts", "assets/fonts"), // the panel's fonts (forza-panel font_dir: a fonts\ above its DLL) and their licences
    ("languages", "editions/2010-11/languages"),
    ("source/README.txt", "native/2010-11/README.txt"),
    // the sources of the kit's 32-bit programs (wccfk.dll, inject.exe, FPR_Emu.exe)
    ("source/native/common", "native/common"),
    ("source/native/hook", "native/hook"),
    ("source/native/programs", "native/programs"),
    ("source/launcher", "crates/launcher"),
];
/// The staged 2017-18 kit <- the repository, same rules as [`MAP`]; later rows overlay earlier ones and a row may be one
/// file. The kit's programs are not here: `build` compiles them into the staged kit ([`KEEP_1718`]).
const MAP_1718: &[(&str, &str)] = &[
    ("README.txt", "editions/2017-18/README.txt"),
    ("README.it.txt", "editions/2017-18/README.it.txt"),
    ("README.ja.txt", "editions/2017-18/README.ja.txt"),
    ("english", "editions/2017-18/english"),
    ("italian", "editions/2017-18/italian"), // the Italian text: key + Italian, the English behind each key
    ("fonts", "assets/fonts"),
    // no source\ (2026-10-29): its plain-text process-injection code got the ZIP flagged; `package` writes SOURCE.txt,
    // the repository at the exact commit, instead
];
/// Product name in the version information of programs both kits share.
const PRODUCT_COMMON: &str = "Forza-WCCF";
const TOP_1718: &str = "Forza-WCCF-2017-18";
/// Built into the staged kit, so staging leaves them alone.
const KEEP: &[&str] = &["programs", "Launcher.exe", "source/build"];
/// Built into the staged 2017-18 kit (`source/build` holds the guard tests, which the kit does not ship; the guard `bin/wccfnet.dll` ships, setup copies it into the game runtime).
const KEEP_1718: &[&str] = &[
    "Launcher.exe",
    "bin/winmm.dll",
    "bin/wccfnet.dll",
    "bin/dmath.dll",
    "bin/ICMP.DLL",
    "bin/FPR_Emu.exe",
    "overlay/forza_panel.dll",
    "source/build",
];
/// What differs between the two kits: the staging table, the programs `build` keeps, the ZIP's top folder and product
/// name, and the folders `build` makes in the staged kit.
struct Ed {
    map: &'static [(&'static str, &'static str)],
    keep: &'static [&'static str],
    top: &'static str,
    product: &'static str,
    dirs: &'static [&'static str],
}

const ED_1011: Ed = Ed {
    map: MAP,
    keep: KEEP,
    top: TOP,
    product: PRODUCT,
    dirs: &["programs", "source/build"],
};
const ED_1718: Ed = Ed {
    map: MAP_1718,
    keep: KEEP_1718,
    top: TOP_1718,
    product: PRODUCT_1718,
    dirs: &[
        "bin",
        "overlay",
        "source/build",
        "source/build/server-hook",
        "source/build/server-hook-1718",
    ],
};

/// The 32-bit programs (`native/`, built for [`PANEL_TARGET`]): cargo's file -> where a kit has it (`true` the 2010-11
/// kit, `false` the 2017-18 kit). The hook, forza_hook.dll, is built once per edition ([`HOOKS`]).
const NATIVE: &[(&str, bool, &str)] = &[
    ("inject.exe", true, "programs/inject.exe"),
    ("fpr_emu.exe", true, "programs/FPR_Emu.exe"),
    ("fpr_emu.exe", false, "bin/FPR_Emu.exe"),
    ("wccfnet.dll", false, "bin/wccfnet.dll"),
    ("dmath.dll", false, "bin/dmath.dll"),
    ("icmp.dll", false, "bin/ICMP.DLL"),
    // not in the 2017-18 kit: the Linux servers' hooks load it (forza-hook e1011-server and e1718-server, [`HOOKS`])
    ("netguard.dll", false, "source/build/netguard.dll"),
    // the server hook's self-tests, run under Wine beside it (native/hook/src/server.rs); a folder of their own, so
    // no test program here loads the server's winmm.dll
    (
        "msgguard_test.exe",
        false,
        "source/build/server-hook/msgguard_test.exe",
    ),
    (
        "sleepfix_test.exe",
        false,
        "source/build/server-hook/sleepfix_test.exe",
    ),
    (
        "spinfix_test.exe",
        false,
        "source/build/server-hook/spinfix_test.exe",
    ),
    // the 2017-18 server hook's self-test, beside it
    (
        "server1718_test.exe",
        false,
        "source/build/server-hook-1718/server1718_test.exe",
    ),
    // the 2017-18 Linux arcade's stand-in for the Launcher's shared memory (crates/linux-host), beside its hook
    (
        "sddy_maps.exe",
        false,
        "source/build/server-hook-1718/sddy_maps.exe",
    ),
    // tests only, not shipped; fakegame_iphlp: the 2017-18 guard's tests rename its IP-helper import as setup does
    ("fakegame.exe", true, "source/build/fakegame.exe"),
    (
        "fakegame_iphlp.exe",
        true,
        "source/build/fakegame_iphlp.exe",
    ),
    ("test_dhcphook.exe", true, "source/build/test_dhcphook.exe"),
    ("crashme.exe", true, "source/build/crashme.exe"),
    ("test_icmp.exe", false, "source/build/test_icmp.exe"),
    ("test_wccfnet.exe", false, "source/build/test_wccfnet.exe"),
    ("test_logowin.exe", false, "source/build/test_logowin.exe"),
    ("test_netguard.exe", false, "source/build/test_netguard.exe"),
    ("test_panel.exe", false, "source/build/test_panel.exe"),
    // test_panel's stand-in for the panel: a DLL that does nothing when it loads
    ("icmp.dll", false, "source/build/panel_probe.dll"),
];
/// forza_hook.dll per edition: (its feature, `true` the 2010-11 kit, where the kit has it). The Linux server's hooks
/// are in neither ZIP: the 2017-18 kit's source/build, which is not zipped, holds them beside netguard.dll.
const HOOKS: [(&str, bool, &str); 4] = [
    ("forza-hook/e1011", true, "programs/wccfk.dll"),
    ("forza-hook/e1718", false, "bin/winmm.dll"),
    (
        "forza-hook/e1011-server",
        false,
        "source/build/server-hook/winmm.dll",
    ),
    (
        "forza-hook/e1718-server",
        false,
        "source/build/server-hook-1718/winmm.dll",
    ),
];
/// [`HOOKS`], without the 2010-11 ones unless `e1011`.
fn hooks(e1011: bool) -> impl Iterator<Item = (&'static str, bool, &'static str)> {
    HOOKS
        .into_iter()
        .filter(move |(f, _, _)| e1011 || !f.starts_with("forza-hook/e1011"))
}
/// The hook feature a target builds with: the 2010-11 hook's only when `e1011` ([`hooks`]). `check quick` decides it per
/// branch, and a per-commit test run takes the 2017-18 one, so both build the gate's tree.
fn hook_feature(e1011: bool) -> &'static str {
    let (feature, _, _) = hooks(e1011)
        .next()
        .expect("HOOKS holds both editions' hooks");
    feature
}
/// What only the 2010-11 edition builds from (git's paths, lower case; a folder ends in `/`). `check quick`, which
/// pull requests' CI runs, builds and tests the 2010-11 hooks ([`HOOKS`]: wccfk.dll and the 2010-11 Linux server's
/// winmm.dll) only when a change touches one of these, and so does `check ci --base` ([`ci_builds_1011`]); the full
/// check, `check ci` without `--base` or with `--release` (the weekly run, releases), `build` and `package` always do.
/// The hook crate and its dependency count whole (both editions' hooks share their code), and so do the toolchain, the
/// locked crates and the settings that profile or lint every crate: a break in those that only the 2010-11 hooks meet
/// must not wait for the weekly `--release` run. Package-less 2010-11 files (README, deploy scripts) are only staged or
/// linted, by `check ci` and the lint job alike.
const PATHS_1011: &[&str] = &[
    "editions/2010-11/",
    "legacy/2010-11/",
    "native/2010-11/",
    "native/hook/",
    "native/common/",
    // test_dhcphook.exe, the 2010-11 hook's kit test, and the 2010-11 kit's inject.exe and FPR_Emu.exe
    "native/programs/",
    "crates/forza/src/setup/e1011.rs",
    "crates/forza/examples/setup1011.rs",
    "deploy/linux/wccf-1011",
    // the root manifest (profiles, workspace lints) and native/'s clippy settings: the hooks are built and linted by them
    "cargo.toml",
    "native/clippy.toml",
    "cargo.lock",
    "rust-toolchain.toml",
    ".cargo/",
];

/// Whether `check ci` builds the 2010-11 hooks: always for `--release` (what players get) or when the changes are
/// unknown (`None`: no `--base`, as in the weekly run, or no merge base with it), else when [`touches_1011`].
fn ci_builds_1011(release: bool, files: Option<&str>) -> bool {
    release || files.is_none_or(touches_1011)
}

/// A changed file (one per line, git's spelling) is in [`PATHS_1011`].
fn touches_1011(files: &str) -> bool {
    files
        .lines()
        .map(str::to_lowercase)
        .any(|f| PATHS_1011.iter().any(|p| f.starts_with(p)))
}

/// The log line for [`ci_builds_1011`]: why `check ci` built or skipped the 2010-11 hooks. `--base` is a manual run's
/// (CI's `check ci` passes none: no base, everything); one the clone does not have (a force-push, a shallow clone)
/// says so, and must not read as a run with `--base` whose diff was empty: without it the skip is off.
fn ci_1011_reason(release: bool, base: Option<&str>, files: Option<&str>) -> String {
    match (release, base, files) {
        (true, _, _) => "in (--release: what players get)".to_owned(),
        (false, None, _) => "in (no --base: everything)".to_owned(),
        (false, Some(b), None) => format!("in (no merge base with {b}: not fetched?)"),
        (false, Some(_), Some(f)) if touches_1011(f) => "in (a 2010-11 path changed)".to_owned(),
        (false, Some(b), Some(_)) => format!("skipped (no 2010-11 path changed since {b})"),
    }
}

/// The 32-bit crates besides the panel, as `build`, clippy and the tests take them (the hook gets its feature apart).
const NATIVE_CRATES: &[&str] = &[
    "-p",
    "forza-native",
    "-p",
    "forza-wccfnet",
    "-p",
    "forza-detmath",
    "-p",
    "forza-netguard",
    "-p",
    "forza-icmp",
    "-p",
    "forza-programs",
    "--features",
    "forza-programs/programs",
];

/// Shipped programs that may be console programs, each started only by Launcher.exe with CREATE_NO_WINDOW and its output
/// read through a file or a pipe: inject.exe (exit code and message read from a pipe), forza-server.exe (log file; 2010-11 only,
/// 2017-18 runs the server as `Launcher.exe server`).  FPR_Emu.exe is not here: the game starts it.
const CONSOLE_OK: &[&str] = &["programs/inject.exe", "programs/forza-server.exe"];
/// The Subsystem field of a PE file's optional header (2 GUI, 3 console).
fn pe_subsystem(b: &[u8]) -> Option<u16> {
    let at = |o: usize, n: usize| b.get(o..o + n);
    let pe = u32::from_le_bytes(at(0x3c, 4)?.try_into().ok()?) as usize;
    (at(pe, 4)? == b"PE\0\0").then_some(())?;
    Some(u16::from_le_bytes(at(pe + 24 + 68, 2)?.try_into().ok()?))
}
const PROGRAMS_1718: &[&str] = &[
    "Launcher.exe",
    "bin/winmm.dll",
    "bin/wccfnet.dll",
    "bin/dmath.dll",
    "bin/ICMP.DLL",
    "bin/FPR_Emu.exe",
    "overlay/forza_panel.dll",
];
/// What the kit must hold after a build and a package.
const PROGRAMS: &[&str] = &[
    "Launcher.exe",
    "programs/forza-server.exe",
    "programs/wccfk.dll",
    "programs/FPR_Emu.exe",
    "programs/forza_panel.dll",
    "programs/inject.exe",
];

fn main() -> Result<()> {
    let task = Task::parse();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("no repository root")?
        .to_path_buf();
    if let Task::Verify { base } = task {
        std::process::exit(verify(&root, &base));
    }
    run(task, &root)
}

fn run(task: Task, root: &Path) -> Result<()> {
    let root = root.to_path_buf();
    // `check text` reads only tracked text: CI's Linux lint job runs it
    ensure!(
        cfg!(windows) || matches!(&task, Task::Check { mode: Some(m), .. } if m == "text"),
        "cargo xtask runs on Windows (only `check text` runs anywhere)"
    );
    // `check text` and `stage` run no compiler
    let text = matches!(&task, Task::Check { mode: Some(m), .. } if m == "text");
    // `package` neither: shipped programs never come from the cache (see `rust`)
    if !text && !matches!(task, Task::Stage { .. } | Task::Package { .. }) {
        use_sccache();
    }
    // held until `run` returns
    let _shared = shared_target(&root)?;
    let stage_root = stage_root(&root);
    let kit = stage_root.join("kit/2010-11");
    let kit_1718 = stage_root.join("kit/2017-18");
    match task {
        Task::Build { fast, release } => {
            let _ = PROFILE.set(pick_profile(Some("build"), fast, release, in_ci()));
            build_all(&root, &kit, &kit_1718, true)
        }
        Task::Cap { root, outdir } => cap(&root, &outdir),
        Task::Gallery => gallery(&root),
        // check and package build first: both used whatever programs were staged last, and a ZIP shipped a Launcher.exe
        // older than its own scripts (2026-10-13, the first Rust run started a deleted panel.py)
        Task::Check {
            mode,
            base,
            fast,
            release,
            test_args,
        } => {
            let ci = mode.as_deref() == Some("ci");
            // with a wrapper `quick` takes CI's non-incremental `ci` profile: sccache serves its workspace crates across
            // worktrees, never the incremental `fast` ones (`test` takes the same profile, sharing this build tree)
            let wrapped = NO_INCREMENTAL.load(Ordering::Relaxed);
            let _ = PROFILE.set(pick_profile(
                mode.as_deref(),
                fast,
                release,
                in_ci() || wrapped,
            ));
            match mode.as_deref() {
                Some("quick") => {
                    let base = base.as_deref().unwrap_or(QUICK_BASE);
                    return quick(&root, &stage_root, &kit, &kit_1718, base);
                }
                Some("text") => return check_text(&root),
                _ => {}
            }
            // the full check always builds the 2010-11 hooks; `ci` only for a 2010-11 change since `--base`
            let files = match base.as_deref() {
                Some(b) if ci => changed_files(&root, b)?,
                _ => None,
            };
            let e1011 = !ci || ci_builds_1011(release, files.as_deref());
            if ci {
                println!(
                    "ci: 2010-11 hooks {}",
                    ci_1011_reason(release, base.as_deref(), files.as_deref())
                );
            }
            build_all(&root, &kit, &kit_1718, e1011)
                .and_then(|()| check(&root, &stage_root, &kit, &kit_1718, ci, e1011, &test_args))
        }
        Task::Test { package, test_args } => {
            // the profile `quick` picks, so a per-commit run reuses the gate's build tree instead of compiling a `fast` one
            // beside it (with a wrapper that is `ci`; `test_commands` builds the same targets and rustflags)
            let wrapped = NO_INCREMENTAL.load(Ordering::Relaxed);
            let _ = PROFILE.set(pick_profile(
                Some("quick"),
                false,
                false,
                in_ci() || wrapped,
            ));
            test_packages(&root, &kit, &package, &test_args)
        }
        Task::Verify { .. } => unreachable!("main runs verify"),
        Task::Stage { edition } => match edition.as_str() {
            "2010-11" => stage(&root, &kit, ED_1011.map, ED_1011.keep),
            "2017-18" => stage(&root, &kit_1718, ED_1718.map, ED_1718.keep),
            _ => anyhow::bail!("unknown edition {edition} (2010-11 or 2017-18)"),
        },
        Task::Package { edition, name } => {
            STAMP_SHA.store(true, Ordering::Relaxed);
            match edition.as_str() {
                "installer" => package_installer(&root, name),
                "2010-11" | "2017-18" if let Err(e) = build_all(&root, &kit, &kit_1718, true) => {
                    Err(e)
                }
                "2010-11" => package(&root, &kit, &name.unwrap_or_else(|| TOP.into()), true),
                "2017-18" => package(
                    &root,
                    &kit_1718,
                    &name.unwrap_or_else(|| format!("{TOP_1718}-{}", build_version(&root))),
                    false,
                ),
                _ => anyhow::bail!("unknown edition {edition} (2010-11 or 2017-18)"),
            }
        }
    }
}

// ---- the host ----
mod the_host;
use self::the_host::{
    cap, gallery, has_user_path, remap_config, run_ok, rust, rust_shipped, stage_root,
};

// ---- the staged kit ----
mod stage;
use self::stage::{build_version, dos_time, stage, version, zip_time};

// ---- build ----
mod build;
use self::build::{build_all, kit_stale};

// ---- verify ----
mod verify;
use self::verify::{CheckFailed, verify};

// ---- check ----
mod check;
use self::check::{changed_files, check, check_text, norm_path, quick, test_packages};

// ---- scrub ----
mod scrub;
use self::scrub::scrub;

// ---- no Python ----

/// The staged kits ship no Python (2026-10-20: setup, the game's text and the card steps are Launcher.exe's own): no
/// python.exe and no .py file anywhere in them.
fn no_python(kits: &[&Path], failed: &mut Vec<String>) -> Result<()> {
    for kit in kits {
        let (mut files, mut dirs) = (BTreeSet::new(), BTreeSet::new());
        walk(kit, "", &mut files, &mut dirs)?;
        let found: Vec<&String> = files
            .iter()
            .filter(|f| {
                let f = f.to_lowercase();
                f.ends_with(".py") || f.ends_with("python.exe")
            })
            .collect();
        if found.is_empty() {
            println!(
                "no Python: {} holds no python.exe and no .py",
                kit.display()
            );
        } else {
            failed.push(format!("no Python: {} holds {found:?}", kit.display()));
        }
    }
    Ok(())
}

// ---- no script host ----

/// Keeps the Windows script host out (ROADMAP 4.7): no mention of it in xtask's source, and no tracked `*.ps1`.
/// The word is built from two halves so that this file does not spell it either.
fn no_script_host(root: &Path, failed: &mut Vec<String>) -> Result<()> {
    let word = ["power", "shell"].concat();
    let mut bad = vec![];
    for e in fs::read_dir(root.join("xtask/src"))? {
        let path = e?.path();
        for (n, l) in fs::read_to_string(&path)?.lines().enumerate() {
            if l.to_ascii_lowercase().contains(&word) {
                bad.push(format!("{}:{}", path.display(), n + 1));
            }
        }
    }
    let sh = Shell::new()?;
    sh.change_dir(root);
    let scripts = cmd!(sh, "git ls-files -- *.ps1").read()?;
    bad.extend(scripts.lines().map(str::to_owned));
    for b in &bad {
        println!("script host: {b}");
    }
    if bad.is_empty() {
        println!("script host: none in xtask, no tracked .ps1");
    } else {
        failed.push(format!("script host ({} places)", bad.len()));
    }
    Ok(())
}

/// A NUL byte in a tracked Rust file makes grep call it binary (#308): write `\0` instead.
fn no_nul(root: &Path, failed: &mut Vec<String>) -> Result<()> {
    let sh = Shell::new()?;
    sh.change_dir(root);
    let files = cmd!(sh, "git ls-files -- *.rs").read()?;
    let bad: Vec<&str> = files
        .lines()
        .filter(|f| fs::read(root.join(f)).is_ok_and(|b| b.contains(&0)))
        .collect();
    for b in &bad {
        println!("NUL byte: {b}");
    }
    if !bad.is_empty() {
        failed.push(format!("NUL bytes ({} files)", bad.len()));
    }
    Ok(())
}

// ---- the installer ----
mod installer;
use self::installer::{
    INSTALLER, InstallerWork, installer_checks, installer_exe, installer_work, package_installer,
    sha256_hex,
};

// ---- package ----
mod package;
use self::package::{no_kit_word, package, shipped_docs_no_kit, walk};

#[cfg(test)]
mod tests;
