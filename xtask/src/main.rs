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

/// `is_1011`: the 2010-11 kit, else the 2017-18 kit. Both hold the programs `cargo xtask build` made, and their version
/// information is checked; both ZIPs leave out `source/build` (the 2017-18 guard tests live there; its guard `bin/wccfnet.dll` ships).
/// Players read these files: the product is Forza-WCCF, never a "kit" (the whole word, any case).
/// The staged kits' player docs and the installer's say no "kit" (what `package` checks, without the ZIP).
fn shipped_docs_no_kit(root: &Path, kits: &[&Path]) -> Result<()> {
    let mut docs = vec![];
    for kit in kits {
        docs.extend(
            fs::read_dir(kit)?
                .filter_map(|e| Some(e.ok()?.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "txt" || e == "md")),
        );
    }
    docs.push(kits[0].join("source/README.txt")); // 2010-11 only: the 2017-18 ZIP has SOURCE.txt instead
    let src = root.join("crates/installer");
    docs.extend([src.join("README.md"), src.join("CHANGELOG.md")]);
    no_kit_word(&docs)
}

fn no_kit_word(files: &[PathBuf]) -> Result<()> {
    for f in files {
        for (n, line) in fs::read_to_string(f)?.lines().enumerate() {
            let low = line.to_ascii_lowercase();
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            let hit = low.match_indices("kit").any(|(i, _)| {
                !word(low[..i].chars().next_back()) && !word(low[i + 3..].chars().next())
            });
            ensure!(
                !hit,
                "{}:{}: the word \"kit\" in player text: {line}",
                f.display(),
                n + 1
            );
        }
    }
    Ok(())
}

/// The 2017-18 ZIP's SOURCE.txt: where the source of its programs is, at the commit it was built from.
fn source_txt(version: &str, commit: &str) -> String {
    let sha = commit.split(' ').next().unwrap_or(commit);
    // a release (1.2.3), not a dev build (1.2.4-dev+<sha>) or a test label
    let tag = if version.starts_with(|c: char| c.is_ascii_digit()) && !version.contains(['-', '+'])
    {
        format!("Release tag v{version}, commit")
    } else {
        "Commit".to_owned()
    };
    format!(
        "Forza-WCCF 2017-18 {version}\r\n\r\n\
         The source of every program in this folder (Launcher.exe, bin\\, overlay\\):\r\n\
         https://github.com/Forza-WCCF/Forza-WCCF/tree/{sha}\r\n\
         {tag} {commit}. Licence: MIT or Apache-2.0, your choice.\r\n\r\n\
         Il sorgente di ogni programma in questa cartella: il link qui sopra (licenza MIT o Apache-2.0).\r\n\
         このフォルダーのすべてのプログラムのソース：上のリンク（MIT または Apache-2.0 ライセンス）。\r\n"
    )
}

fn package(root: &Path, kit: &Path, name: &str, is_1011: bool) -> Result<()> {
    let Ed {
        top,
        map,
        keep,
        product,
        ..
    } = if is_1011 { ED_1011 } else { ED_1718 };
    let programs: &[&str] = if is_1011 { PROGRAMS } else { PROGRAMS_1718 };
    let sh = Shell::new()?;
    sh.change_dir(root);
    let mapped: Vec<&str> = map.iter().map(|m| m.1).collect();
    let dirty = cmd!(sh, "git status --porcelain -- {mapped...}").read()?;
    ensure!(
        dirty.trim().is_empty(),
        "uncommitted changes in files the kit ships; commit them first:\n{dirty}"
    );
    stage(root, kit, map, keep)?;
    let mut docs: Vec<PathBuf> = fs::read_dir(kit)?
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "txt" || e == "md"))
        .collect();
    if is_1011 {
        docs.push(kit.join("source/README.txt"));
    }
    no_kit_word(&docs)?;
    for p in programs {
        ensure!(
            kit.join(p).is_file(),
            "{p} is missing: run `cargo xtask build` first"
        );
    }

    if !is_1011 && let Err(e) = freeze_fpr_emu(root, kit) {
        println!("warning: FPR_Emu.exe is built fresh, not taken from the release: {e:#}");
    }

    // every program must carry its version information: a nameless unsigned program is what Windows Defender's
    // machine learning flagged (Kit 5.5's first SETUP.exe, then its winmm.dll)
    for p in programs {
        let v = version_strings(&fs::read(kit.join(p))?).unwrap_or_default();
        let get = |k: &str| v.get(k).map_or("", String::as_str);
        // the programs name the project (Forza-WCCF); an edition's own name is what the C programs carried up to 0.3.2
        ensure!(
            (get("ProductName") == product || get("ProductName") == PRODUCT_COMMON)
                && !get("FileVersion").is_empty(),
            "{p} has no version information - see xtask build"
        );
        println!(
            "{p} : {} {} - {}",
            get("ProductName"),
            get("FileVersion"),
            get("FileDescription")
        );
    }

    // Launcher.exe declares DPI awareness in its manifest (crates/launcher/build.rs): without it Windows stretches the
    // window blurry on a screen scaled above 100%
    for p in programs.iter().filter(|p| p.ends_with("Launcher.exe")) {
        let file = fs::read(kit.join(p))?;
        let manifest = resource(&file, object::pe::RT_MANIFEST).unwrap_or_default();
        ensure!(
            String::from_utf8_lossy(manifest).contains("dpiAware"),
            "{p} has no dpiAware manifest - see crates/launcher/build.rs"
        );
    }

    // Defender hygiene: no shipped exe/dll is packed (a UPX section is what machine-learning scanners flag), and no test
    // program (source/build, which `walk` leaves out) reaches the kit.
    // Optional signing hook: with FORZA_SIGN_CMD set (a cmd line, e.g. `signtool sign /f k.pfx /fd sha256 "{f}"`), every
    // shipped exe/dll goes through it, `{f}` replaced by the file's path, before files.txt and the ZIP are made. Unset: no signing.
    let (mut bins, mut dirs0) = (BTreeSet::new(), BTreeSet::new());
    walk(kit, "", &mut bins, &mut dirs0)?;
    for f in bins.iter().filter(|f| {
        let f = f.to_ascii_lowercase();
        f.ends_with(".exe") || f.ends_with(".dll")
    }) {
        let bytes = fs::read(kit.join(f))?;
        ensure!(
            !bytes.windows(4).any(|w| w == b"UPX!" || w == b"UPX0"),
            "{f} is UPX-packed: scanners flag packed programs"
        );
        ensure!(
            !has_user_path(&bytes),
            "{f} holds a user folder path - see rust_shipped"
        );
        // no console window may reach a player: every program is a GUI-subsystem one (a console program started by the
        // game itself gets a window of its own), except these, which the launcher starts with CREATE_NO_WINDOW
        if f.to_ascii_lowercase().ends_with(".exe") && !CONSOLE_OK.contains(&f.as_str()) {
            ensure!(
                pe_subsystem(&bytes) == Some(2),
                "{f} is a console program: link it /SUBSYSTEM:WINDOWS or list it in CONSOLE_OK with a reason"
            );
        }
        if let Some(cmdline) = std::env::var_os("FORZA_SIGN_CMD") {
            let path = kit.join(f).display().to_string();
            let line = cmdline.to_string_lossy().replace("{f}", &path);
            let mut sign = Command::new("cmd");
            sign.arg("/C");
            // raw: cmd parses the line itself, Rust's argument quoting would get in its way (cfg: xtask also builds
            // on Linux for `check text`)
            #[cfg(windows)]
            std::os::windows::process::CommandExt::raw_arg(&mut sign, &line);
            #[cfg(not(windows))]
            sign.arg(&line); // never runs: off Windows the host gate stops every task but `check text`
            ensure!(sign.status()?.success(), "FORZA_SIGN_CMD failed for {f}");
        }
    }

    // for SETUP.exe's update: version.txt, which kit this is, and files.txt, every file this kit ships - an update removes
    // what the old kit's list has and the new one's does not
    let version = build_version(root);
    fs::write(kit.join("version.txt"), format!("kit-{version}\r\n"))?;
    if !is_1011 {
        let mut commit = cmd!(sh, "git rev-parse HEAD").read()?.trim().to_owned();
        if !cmd!(sh, "git status --porcelain -- crates native xtask")
            .read()?
            .trim()
            .is_empty()
        {
            commit.push_str(" plus uncommitted changes"); // a test build: the programs are not exactly that commit
        }
        fs::write(kit.join("SOURCE.txt"), source_txt(version, &commit))?;
    }
    let mut shipped = BTreeSet::new();
    let mut dirs = BTreeSet::new();
    walk(kit, "", &mut shipped, &mut dirs)?;
    if !shipped.iter().any(|f| f.starts_with("source/")) {
        dirs.remove("source"); // only the unshipped source/build is left in it
    }
    // files.sha256: each file's SHA-256, for the Launcher's update to check the unzipped kit before it copies anything
    // (lists of an earlier run in the folder are not in them)
    shipped.retain(|f| f != "files.txt" && f != "files.sha256");
    let mut sums = String::new();
    for f in &shipped {
        let hash = sha256_hex(&fs::read(kit.join(f))?);
        let _ = write!(sums, "{hash}  {}\r\n", f.replace('/', "\\"));
    }
    fs::write(kit.join("files.sha256"), sums)?;
    shipped.insert("files.sha256".to_owned());
    shipped.insert("files.txt".to_owned());
    let mut files = String::from("\u{feff}");
    for f in &shipped {
        files.push_str(&f.replace('/', "\\"));
        files.push_str("\r\n");
    }
    fs::write(kit.join("files.txt"), files)?;

    // '/' in the entry names; a directory entry for every folder, as the kit was always shipped
    let dist = root.join("dist");
    fs::create_dir_all(&dist)?;
    let zip_path = dist.join(format!("{name}.zip"));
    let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path)?);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip_time(root)?);
    zip.add_directory(format!("{top}/"), opts)?;
    for d in &dirs {
        zip.add_directory(format!("{top}/{d}/"), opts)?;
    }
    for f in &shipped {
        zip.start_file(format!("{top}/{f}"), opts)?;
        std::io::copy(&mut fs::File::open(kit.join(f))?, &mut zip)?;
    }
    zip.finish()?;

    // read the ZIP back: every program must be in it, not only in the folder it was made from
    // (a kit without them cannot start)
    let mut ar = zip::ZipArchive::new(fs::File::open(&zip_path)?)?;
    let mut entries = HashSet::new();
    for i in 0..ar.len() {
        entries.insert(ar.by_index(i)?.name()?.into_owned()); // each entry's header reads
    }
    let need = programs
        .iter()
        .copied()
        .chain((!version.is_empty()).then_some("version.txt"));
    let missing: Vec<_> = need
        .filter(|n| !entries.contains(&format!("{top}/{n}")))
        .collect();
    ensure!(missing.is_empty(), "the ZIP lacks: {}", missing.join(", "));
    println!("the ZIP has every program: {}", programs.join(", "));

    let hash = sha256_hex(&fs::read(&zip_path)?);
    let mut sha = fs::File::create(dist.join(format!("{name}.zip.sha256")))?;
    write!(sha, "{hash}  {name}.zip\r\n")?;
    // a kit ZIP is far below 2^53 bytes, where f64 stops counting exactly
    #[allow(clippy::cast_precision_loss)]
    let mb = fs::metadata(&zip_path)?.len() as f64 / 1_048_576.0;
    println!(
        "packaged {} ({mb:.1} MB, sha256 {hash})",
        zip_path.display()
    );
    Ok(())
}

/// The 2017-18 `bin/FPR_Emu.exe` keeps the bytes of the last release while its sources are unchanged, so a release does
/// not put a new binary in front of the antivirus engines (docs/RELEASING.md, "Antivirus verdicts"). The file holds the
/// sha256 of that release's file, its tag and the commit it was built from.
const FREEZE_FILE: &str = "editions/2017-18/fpr_emu.sha256";
/// What FPR_Emu.exe is built from besides Cargo.lock and the toolchain: its crate, the common crate and its icon.
/// shortcut: `crates/launcher/version_rc.rs` (shared with every program) is not here, it changed in 0.6.2 without
/// changing this exe; add it back when its output for fpr_emu changes.
const FREEZE_SRC: &[&str] = &[
    "native/programs",
    "native/common",
    "assets/logo/forza-wccf-small.ico",
];

/// `<sha256> <tag> <commit> [pin]`: `pin` keeps the release's file even when the sources changed since `commit`.
fn parse_freeze(text: &str) -> Result<([&str; 3], bool)> {
    let words = text.split_whitespace().collect::<Vec<_>>();
    let pinned = match words.get(3) {
        None => false,
        Some(&"pin") => true,
        Some(w) => anyhow::bail!("{FREEZE_FILE} wants `<sha256> <tag> <commit> [pin]`, not `{w}`"),
    };
    let Ok([hash, tag, commit]) = <[&str; 3]>::try_from(&words[..3.min(words.len())]) else {
        anyhow::bail!("{FREEZE_FILE} wants `<sha256> <tag> <commit> [pin]`");
    };
    ensure!(
        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "{FREEZE_FILE}: {hash} is not a sha256"
    );
    Ok(([hash, tag, commit], pinned))
}

/// Whether the release's file ships: a pin always wins, otherwise only while the sources are unchanged.
fn keep_frozen(pinned: bool, changed: &str) -> bool {
    pinned || changed.trim().is_empty()
}

/// Replace the freshly built `bin/FPR_Emu.exe` of `kit` with the release's file when `FREEZE_SRC` is unchanged since the
/// recorded commit. `Ok` also when the sources changed (it says so and the fresh build stays); `Err` when the release
/// file cannot be had or does not match the hash (offline, a moved release): the caller warns and ships the fresh build.
fn freeze_fpr_emu(root: &Path, kit: &Path) -> Result<()> {
    use std::io::Read;
    let text = fs::read_to_string(root.join(FREEZE_FILE))?;
    let ([hash, tag, commit], pinned) = parse_freeze(&text)?;
    let sh = Shell::new()?;
    sh.change_dir(root);
    // committed, uncommitted and new files since the commit
    let changed = cmd!(sh, "git diff --name-only {commit} -- {FREEZE_SRC...}")
        .read()
        .with_context(|| format!("git cannot compare with {commit} (a shallow clone?)"))?;
    let new = cmd!(
        sh,
        "git ls-files --others --exclude-standard -- {FREEZE_SRC...}"
    )
    .read()?;
    let changed = format!("{changed}\n{new}");
    if !keep_frozen(pinned, &changed) {
        println!(
            "FPR_Emu.exe: the frozen hash is stale, its sources changed since {commit}; built fresh:\n{}\nAfter the next release, put the new file's hash, tag and commit in {FREEZE_FILE}",
            changed.trim()
        );
        return Ok(());
    }
    if pinned && !changed.trim().is_empty() {
        println!(
            "FPR_Emu.exe: pinned to {tag} in {FREEZE_FILE}, its sources changed since {commit}:\n{}",
            changed.trim()
        );
    }
    let zip_path = kit.with_file_name("frozen-release.zip");
    let url = format!(
        "https://github.com/Forza-WCCF/Forza-WCCF/releases/download/{tag}/{TOP_1718}-{}.zip",
        tag.trim_start_matches('v')
    );
    cmd!(
        sh,
        "curl -fsSL --retry 2 --max-time 300 -o {zip_path} {url}"
    )
    .quiet()
    .run()
    .with_context(|| format!("cannot download {url}"))?;
    let mut bytes = Vec::new();
    let read = || -> Result<usize> {
        let mut ar = zip::ZipArchive::new(fs::File::open(&zip_path)?)?;
        Ok(ar
            .by_name(&format!("{TOP_1718}/bin/FPR_Emu.exe"))?
            .read_to_end(&mut bytes)?)
    }();
    let _ = fs::remove_file(&zip_path);
    read?;
    let got = sha256_hex(&bytes);
    ensure!(
        got == hash,
        "the {tag} file is sha256 {got}, {FREEZE_FILE} says {hash}"
    );
    fs::write(kit.join("bin/FPR_Emu.exe"), bytes)?;
    let why = if pinned {
        format!("pinned in {FREEZE_FILE}")
    } else {
        format!("sources unchanged since {commit}")
    };
    println!("FPR_Emu.exe: the {tag} file, sha256 {got} ({why})");
    Ok(())
}

/// The first resource of type `ty` (RT_VERSION, RT_MANIFEST ...) in a PE file; `object` reaches the resource directory, so
/// `pelite` is not needed.
fn resource(file: &[u8], ty: u16) -> Result<&[u8]> {
    use object::read::pe::{ImageNtHeaders, PeFile};
    use object::{FileKind, pe};
    fn rsrc<P: ImageNtHeaders>(file: &[u8], ty: u16) -> Result<&[u8]> {
        let pe = PeFile::<P>::parse(file)?;
        let sections = pe.section_table();
        let dir = pe
            .data_directories()
            .resource_directory(file, &sections)?
            .context("no resources")?;
        let mut table = dir.root()?;
        // the type, then the first name/id, then the first language
        for level in 0..3 {
            let entry = table
                .entries
                .iter()
                .find(|e| level > 0 || e.name_or_id.get(object::LittleEndian) == u32::from(ty))
                .context("no such resource")?;
            match entry.data(dir)? {
                object::read::pe::ResourceDirectoryEntryData::Table(t) => table = t,
                object::read::pe::ResourceDirectoryEntryData::Data(d) => {
                    let rva = d.offset_to_data.get(object::LittleEndian);
                    let size = d.size.get(object::LittleEndian) as usize;
                    let data = sections
                        .pe_data_at(file, rva)
                        .context("resource outside the file")?;
                    return data.get(..size).context("short resource");
                }
            }
        }
        anyhow::bail!("resource too deep")
    }
    match FileKind::parse(file)? {
        FileKind::Pe32 => rsrc::<pe::ImageNtHeaders32>(file, ty),
        FileKind::Pe64 => rsrc::<pe::ImageNtHeaders64>(file, ty),
        _ => anyhow::bail!("not a PE file"),
    }
}

/// The string table of a PE file's version resource (ProductName, FileVersion, ...), read in Rust so it works on any host;
/// the VS_VERSIONINFO block layout inside the data is parsed here.
fn version_strings(file: &[u8]) -> Result<HashMap<String, String>> {
    // VS_VERSIONINFO: blocks of (length, value length, type, UTF-16 key, 4-aligned value, 4-aligned child blocks)
    fn blocks(d: &[u8], out: &mut HashMap<String, String>) {
        let u16_at = |i: usize| {
            d.get(i..i + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
        };
        let (mut at, a4) = (0, |n: usize| n.div_ceil(4) * 4);
        while let (Some(len), Some(vlen), Some(ty)) = (u16_at(at), u16_at(at + 2), u16_at(at + 4)) {
            let Some(block) = d.get(at..at + len).filter(|_| len >= 6) else {
                return;
            };
            let key_end = (6..block.len() - 1)
                .step_by(2)
                .find(|&i| block[i] == 0 && block[i + 1] == 0)
                .unwrap_or(block.len() - 1);
            let units = |b: &[u8]| {
                b.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect::<Vec<_>>()
            };
            let key = String::from_utf16_lossy(&units(&block[6..key_end]));
            let value = a4(key_end + 2);
            let size = if ty == 1 { vlen * 2 } else { vlen };
            if ty == 1
                && vlen > 0
                && let Some(v) = block.get(value..value + size)
            {
                out.insert(
                    key,
                    String::from_utf16_lossy(&units(v))
                        .trim_end_matches('\0')
                        .to_owned(),
                );
            }
            blocks(block.get(a4(value + size)..).unwrap_or_default(), out);
            at += a4(len);
        }
    }
    let data = resource(file, object::pe::RT_VERSION)?;
    let mut out = HashMap::new();
    blocks(data, &mut out);
    Ok(out)
}

/// Files and folders of the kit, as '/' paths under it, without the build's leftovers (source/build).
fn walk(
    dir: &Path,
    rel: &str,
    files: &mut BTreeSet<String>,
    dirs: &mut BTreeSet<String>,
) -> Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let r = if rel.is_empty() {
            e.file_name().to_string_lossy().into_owned()
        } else {
            format!("{rel}/{}", e.file_name().to_string_lossy())
        };
        if r == "source/build" {
            continue;
        }
        if e.file_type()?.is_dir() {
            dirs.insert(r.clone());
            walk(&e.path(), &r, files, dirs)?;
        } else {
            files.insert(r);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    use super::build::{built_stamp_text, other_profile, sources_changed};
    use super::check::{
        affected, clippy, hook_test, package_dir, required_programs, target_scopes, test_commands,
        test_filter, tests,
    };
    use super::installer::LICENSES;
    use super::scrub::{mask, names_publisher, publisher};
    use super::stage::{version_from_describe, version_override};
    use super::the_host::{outside_target, remap_flags};
    use super::verify::{Verdict, blocks_verify};

    // The scrub check's two halves on any text: `mask` walks `line[i..]` by hand, so it must not panic on any UTF-8
    // (a cut inside a character), and a line it masked already holds no name, so the check run through its own mask
    // finds nothing. The third case is the surname that only holds the name (Hasegawa), which the name list allows.
    proptest! {
        #[test]
        fn mask_never_panics_and_leaves_nothing_for_the_check(
            pre in "\\PC{0,30}",
            post in "\\PC{0,30}",
            which in 0usize..3,
        ) {
            let [p, k] = publisher();
            let name = match which {
                0 => p.clone(),
                1 => p.to_uppercase(),
                _ => format!("Ha{p}wa"),
            };
            let text = format!("{pre}{name}{post}");
            let masked = mask(&text);
            prop_assert!(!masked.to_ascii_lowercase().contains(&p), "{masked:?}");
            prop_assert!(!masked.contains(&k), "{masked:?}");
            prop_assert!(!names_publisher(&masked), "{masked:?}");
        }
    }

    #[test]
    fn verify_maps_each_end_to_its_exit_code_and_last_line() {
        let failed: Result<()> = Err(anyhow::Error::from(CheckFailed {
            stage: "cargo clippy (32-bit)".into(),
            msg: "cargo clippy (32-bit) failed".into(),
        })
        .context("around it"));
        let v = Verdict::of(&failed);
        assert_eq!(
            (v.code(), v.line(7)),
            (1, "VERIFY FAIL cargo-clippy-(32-bit) 7s".into())
        );
        assert_eq!(v.result(), "FAIL:cargo-clippy-(32-bit)");
        let v = Verdict::of(&Ok(()));
        assert_eq!((v.code(), v.line(12)), (0, "VERIFY OK 12s".into()));
        let v = Verdict::of(&Err(anyhow::anyhow!("git\nnot found")));
        assert_eq!(
            (v.code(), v.line(1)),
            (4, "VERIFY ERROR git not found".into())
        );
        let v = Verdict::NotRun("a game process runs".into());
        assert_eq!(
            (v.code(), v.line(0)),
            (3, "VERIFY NOT-RUN a game process runs".into())
        );
    }

    #[test]
    fn verify_stops_for_the_game_and_its_launcher_not_powertoys() {
        assert!(blocks_verify("client_Release.exe", ""));
        assert!(blocks_verify(
            "Launcher.exe",
            r"C:\Games\Forza-WCCF\2017-2018\Launcher.exe"
        ));
        assert!(blocks_verify("Launcher.exe", ""));
        assert!(!blocks_verify(
            "Launcher.exe",
            r"C:\Program Files\PowerToys\Launcher.exe"
        ));
        assert!(!blocks_verify("cargo.exe", ""));
    }

    /// A kit built in another profile is not the gate's: `check quick` builds `ci` when a wrapper is on, `cargo xtask build`
    /// `fast`, and the smoke of the installer plus the kit tests must not run the other one's programs.
    #[test]
    fn a_kit_built_in_another_profile_is_stale() {
        let stamp = built_stamp_text("ci", "a.rs\nb.rs");
        assert_eq!(stamp.lines().next(), Some("ci"));
        assert!(!other_profile(&stamp, "ci"));
        assert!(other_profile(&stamp, "fast"));
        assert!(other_profile(&stamp, "release"));
        // a stamp from before the profile line: its first line is a source path, not a profile
        assert!(other_profile("a.rs\nb.rs", "ci"));
        assert!(other_profile("", "release"));
    }

    /// `check quick` must rebuild when a source is gone: deleted and not staged, git still lists it and the mtime
    /// comparison skips it (the 2026-11-06 sweep); deleted and staged, it is not in the list at all.
    #[test]
    fn a_deleted_source_makes_the_kit_stale() {
        let root = std::env::temp_dir().join(format!("xtask-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.rs"), b"").unwrap();
        let built = std::fs::metadata(root.join("a.rs"))
            .unwrap()
            .modified()
            .unwrap();
        assert!(!sources_changed(&root, built, "a.rs\n", "a.rs"));
        assert!(sources_changed(&root, built, "a.rs\nb.rs\n", "a.rs\nb.rs"));
        assert!(sources_changed(&root, built, "a.rs\nb.rs\n", "a.rs"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn source_txt_names_the_tag_only_for_a_version() {
        let r = source_txt("0.5.1", "abc");
        assert!(r.contains("/tree/abc\r\n") && r.contains("Release tag v0.5.1, commit abc."));
        let t = source_txt("test", "abc plus uncommitted changes");
        assert!(t.contains("/tree/abc\r\n") && !t.contains("Release tag"));
        assert!(!source_txt("0.5.2-dev+abc", "abc").contains("Release tag"));
    }

    #[test]
    fn the_version_is_the_tag_or_a_dev_build() {
        assert_eq!(
            version_from_describe("v1.2.3-0-gabc1234").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            version_from_describe("v1.2.3-5-gabc1234").as_deref(),
            Some("1.2.4-dev+abc1234")
        );
        assert_eq!(
            version_from_describe("v1.3.0-rc.1-2-gabc1234").as_deref(),
            Some("1.3.0-rc.1-dev+abc1234")
        );
        assert_eq!(
            version_from_describe("v1.3.0-rc.1-0-gabc1234").as_deref(),
            Some("1.3.0-rc.1")
        );
        assert_eq!(version_from_describe("nonsense"), None);
    }

    #[test]
    fn a_build_that_ships_no_zip_keeps_the_commit_out_of_its_version() {
        // `STAMP_SHA` is off for every task but `package`: an ordinary build's version must not move with each commit,
        // or cargo relinks Launcher, the panel and the installer after every one
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert!(
            !build_version(root).contains("-dev+"),
            "{}",
            build_version(root)
        );
    }

    #[test]
    fn a_forza_version_in_the_environment_wins_and_junk_is_refused() {
        assert_eq!(version_override(None).unwrap(), None);
        assert_eq!(version_override(Some(" ")).unwrap(), None);
        assert_eq!(
            version_override(Some("0.7.0")).unwrap().as_deref(),
            Some("0.7.0")
        );
        assert_eq!(
            version_override(Some("1.3.0-rc.1")).unwrap().as_deref(),
            Some("1.3.0-rc.1")
        );
        for junk in ["v0.7.0", "0.7", "0.7.0 beta", "latest"] {
            assert!(version_override(Some(junk)).is_err(), "{junk}");
        }
    }

    #[test]
    fn profiles_are_fast_locally_and_release_for_the_gate() {
        let p = |mode, fast, release, ci| pick_profile(Some(mode), fast, release, ci);
        assert_eq!(p("build", false, false, false), "fast");
        assert_eq!(p("quick", false, false, false), "fast");
        assert_eq!(p("build", false, true, false), "release");
        assert_eq!(p("quick", false, true, false), "release");
        assert_eq!(p("build", false, false, true), "release");
        assert_eq!(p("quick", false, false, true), "ci");
        assert_eq!(p("build", true, false, true), "fast");
        assert_eq!(p("ci", false, false, false), "ci");
        assert_eq!(p("ci", false, true, false), "release");
        assert_eq!(pick_profile(None, false, false, false), "release");
        assert_eq!(pick_profile(None, true, false, false), "fast");
        // the fourth input is CI or a wrapper: `quick` builds `ci` under either, and nothing else moves with it
        assert_eq!(p("ci", false, false, true), "ci");
        assert_eq!(p("quick", true, false, true), "fast");
        assert_eq!(p("quick", false, true, true), "release");
    }

    /// `package` builds every shipped program under no wrapper: `RUSTC_WRAPPER` and the workspace wrapper empty (empty is
    /// off in cargo, and overrides the config file too), so no cache-built object with another flag set ships.
    #[test]
    fn a_package_build_runs_under_no_wrapper() {
        let env = |c: &Command, k: &str| {
            c.get_envs()
                .find(|(e, _)| *e == std::ffi::OsStr::new(k))
                .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        };
        let root = Path::new("C:/repo");
        let was = STAMP_SHA.swap(true, Ordering::Relaxed);
        let package = rust("build", root);
        STAMP_SHA.store(was, Ordering::Relaxed);
        for key in ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"] {
            assert_eq!(env(&package, key), Some(Some(String::new())), "{key}");
        }
        // an ordinary build leaves the workspace wrapper to the caller: only `package` clears it
        assert_eq!(env(&rust("build", root), "RUSTC_WORKSPACE_WRAPPER"), None);
    }

    #[test]
    fn the_tests_build_with_the_kits_rustflags() {
        // other rustflags give every crate another hash: the test build would compile the workspace a second time
        let _ = PROFILE.set("ci"); // the release tests build in the test profile, which shares nothing with the kits
        let root = Path::new("C:/repo");
        let args = |c: &Command| c.get_args().map(ToOwned::to_owned).collect::<Vec<_>>();
        let config = |c: Command| {
            let a = args(&c);
            a.iter()
                .zip(a.iter().skip(1))
                .filter(|(f, _)| *f == "--config")
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>()
        };
        let want = config(rust_shipped(root));
        assert_eq!(want.len(), 2);
        assert_eq!(config(tests(root, true, &[], TARGET, &[], None)), want);
        assert_eq!(
            config(tests(root, true, &[], PANEL_TARGET, &[], None)),
            want
        );
    }

    #[test]
    fn a_skipped_check_passes_no_snapshot_test_to_the_runner() {
        let line = |c: Command| {
            c.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        // nextest's filterset, else cargo test's --skip: whichever runner this PC has
        let skipped = line(tests(Path::new("C:/repo"), true, &[], TARGET, &[], None));
        assert!(
            skipped.contains("not test(/^snapshots::/)") || skipped.contains("--skip snapshots::"),
            "{skipped}"
        );
        let run = line(tests(Path::new("C:/repo"), false, &[], TARGET, &[], None));
        assert!(!run.contains("snapshots"), "{run}");
    }

    #[test]
    fn a_per_commit_test_builds_what_check_quick_builds() {
        let line = |c: &Command| {
            c.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let root = Path::new("C:/repo");
        // `quick` builds the whole workspace of each target and picks the tests with a filterset: the per-commit run
        // must ask for the same build, or it compiles the workspace a second time
        let x64 = test_commands(root, &["forza".to_owned()], &[]);
        assert_eq!(x64.len(), 1);
        let x64 = line(&x64[0]);
        assert!(
            x64.contains("--target x86_64-pc-windows-msvc --workspace --exclude forza-panel"),
            "{x64}"
        );
        // a 32-bit crate is tested for its own target
        let panel = test_commands(root, &["forza-panel".to_owned()], &[]);
        assert_eq!(panel.len(), 1);
        let panel = line(&panel[0]);
        assert!(panel.contains("--target i686-pc-windows-msvc"), "{panel}");
        assert!(!panel.contains("--workspace"), "{panel}");
        // the 32-bit hook builds with the 2017-18 feature, as `quick` does on a branch that changes no 2010-11 path:
        // the gate's build tree, and the 2017-18 hook's cfg'd tests
        let hook = test_commands(root, &["forza-hook".to_owned()], &[]);
        let hook = line(
            hook.iter()
                .find(|c| line(c).contains("i686"))
                .expect("a 32-bit command"),
        );
        assert!(hook.contains("--features forza-hook/e1718"), "{hook}");
    }

    #[test]
    fn the_hook_feature_is_the_2017_18_one_unless_2010_11_changed() {
        assert_eq!(hook_feature(false), "forza-hook/e1718");
        assert_eq!(hook_feature(true), "forza-hook/e1011");
    }

    #[test]
    fn the_32_bit_panel_is_linted_and_tested_for_its_own_target() {
        let v = |a: &[&str]| a.iter().map(ToString::to_string).collect::<Vec<_>>();
        let native: Vec<String> = ["-p", "forza-hook", "--features", "f"]
            .into_iter()
            .chain(NATIVE_CRATES.iter().copied())
            .map(String::from)
            .collect();
        let all = target_scopes(&v(&["--workspace"]), "f");
        assert_eq!(
            all[0],
            (TARGET, v(&["--workspace", "--exclude", "forza-panel"]))
        );
        let panel_native: Vec<String> = v(&["-p", "forza-panel"])
            .into_iter()
            .chain(native.clone())
            .collect();
        assert_eq!(all[1], (PANEL_TARGET, panel_native));
        let some = target_scopes(&v(&["-p", "forza", "-p", "forza-panel"]), "f");
        assert_eq!(some[0], (TARGET, v(&["-p", "forza"])));
        assert_eq!(some[1], (PANEL_TARGET, v(&["-p", "forza-panel"])));
        assert_eq!(
            target_scopes(&v(&["-p", "forza"]), "f"),
            [(TARGET, v(&["-p", "forza"]))]
        );
        assert_eq!(
            target_scopes(&v(&["-p", "forza-panel"]), "f"),
            [(PANEL_TARGET, v(&["-p", "forza-panel"]))]
        );
        let hook = target_scopes(&v(&["-p", "forza-hook"]), "f");
        assert_eq!(
            hook,
            [(TARGET, v(&["-p", "forza-hook"])), (PANEL_TARGET, native)]
        );
    }

    #[test]
    fn the_installer_checks_follow_the_files_they_read() {
        let work = |files: &str| installer_work(Some(files), false);
        let both = InstallerWork::ALL;
        let licenses = InstallerWork {
            licenses: true,
            smoke: false,
        };
        let smoke = InstallerWork {
            licenses: false,
            smoke: true,
        };
        // no file list (no merge base, so the whole workspace): both
        assert_eq!(installer_work(None, false), both);
        assert!(both.any());
        // rule 5's files put every crate in scope: both, whatever their own rows say
        for f in [
            "Cargo.lock\n",
            "rust-toolchain.toml\n",
            ".cargo/config.toml\n",
            "Cargo.toml\n",
        ] {
            assert_eq!(installer_work(Some(f), true), both, "{f}");
        }
        // the tree the notices cover: the lock, or the manifest (which makes the exe too)
        assert_eq!(work("crates/installer/Cargo.toml\n"), both);
        assert_eq!(work("Cargo.lock\n"), licenses);
        // the bundle the notices check reads: its folder smokes, and a change to the bundle itself runs the check
        assert_eq!(work(&format!("{LICENSES}\n")), both);
        // the exe: its crate and build script, the version file, the files it embeds
        for f in [
            "crates/installer/src/main.rs\n",
            "crates/launcher/version_rc.rs\n",
            "assets/fonts/Hack-Regular.ttf\n",
            "assets/logo/forza-wccf.ico\n",
        ] {
            assert_eq!(work(f), smoke, "{f}");
        }
        // a crate it depends on: neither check, only its unit tests (through `affected`)
        let depends = work("crates/forza/src/lib.rs\n");
        assert_eq!(
            depends,
            InstallerWork {
                licenses: false,
                smoke: false
            }
        );
        assert!(!depends.any());
    }

    #[test]
    fn an_xtask_change_smokes_the_installer_it_builds() {
        // xtask is the packaging code that builds the exe, not a crate of its: `quick` runs the smoke for this file
        // alone (`if installer.any()`, without asking the crate scope) and builds the exe for it (`needs_kit`)
        assert_eq!(
            installer_work(Some("xtask/src/main.rs\n"), false),
            InstallerWork {
                licenses: false,
                smoke: true
            }
        );
    }

    #[test]
    fn a_shared_folder_is_the_change_of_the_crates_that_read_it() {
        let info = || {
            [
                ("forza", "crates/forza", &[][..]),
                ("forza-ui", "crates/ui", &["forza"][..]),
                ("launcher", "crates/launcher", &["forza", "forza-ui"][..]),
                ("forza-panel", "crates/panel", &["forza"][..]),
                (
                    "wccf-installer",
                    "crates/installer",
                    &["forza", "forza-ui"][..],
                ),
                ("xtask", "xtask", &[][..]),
            ]
            .map(|(n, d, deps)| {
                let deps = deps.iter().map(ToString::to_string).collect();
                (n.to_owned(), d.to_owned(), deps)
            })
            .to_vec()
        };
        let set = |a: &[&str]| a.iter().map(ToString::to_string).collect::<BTreeSet<_>>();
        // the fonts: forza's and forza-ui's tests, the panel's and the installer's, whose exe the smoke runs
        assert_eq!(
            affected(info(), "assets/fonts/Hack-Regular.ttf\n").unwrap(),
            set(&[
                "forza",
                "forza-ui",
                "launcher",
                "forza-panel",
                "wccf-installer",
                "xtask"
            ])
        );
        // the panel's art is forza-ui's alone
        assert_eq!(
            affected(info(), "assets/panel/shoot.png\n").unwrap(),
            set(&["forza-ui", "launcher", "wccf-installer"])
        );
        // the sheets: forza's text tests and the panel's embedded English text
        assert_eq!(
            affected(info(), "editions/2017-18/english/exe_text.tsv\n").unwrap(),
            set(&[
                "forza",
                "forza-ui",
                "launcher",
                "forza-panel",
                "wccf-installer",
                "xtask"
            ])
        );
        // a folder with no row yet keeps the old answer: everything
        assert_eq!(affected(info(), "assets/new-shared/logo.png\n"), None);
        assert_eq!(affected(info(), "editions/next-edition/README.txt\n"), None);
    }

    #[test]
    fn the_version_file_is_the_change_of_every_crate_that_compiles_it() {
        let names = [
            "launcher",
            "wccf-installer",
            "forza-panel",
            "forza-server",
            "forza-hook",
            "forza-detmath",
            "forza-icmp",
            "forza-programs",
            "forza-wccfnet",
        ];
        // the row answers before any folder does: these are the packages' names, not their folders
        let info = names
            .iter()
            .map(|n| ((*n).to_owned(), format!("crates/{n}"), vec![]))
            .collect();
        let set = |a: &[&str]| a.iter().map(ToString::to_string).collect::<BTreeSet<_>>();
        assert_eq!(
            affected(info, "crates/launcher/version_rc.rs\n").unwrap(),
            set(&names)
        );
    }

    #[test]
    fn the_lock_the_toolchain_and_the_root_manifest_check_everything() {
        let info = || {
            let rows: [(&str, &str, &[&str]); 2] =
                [("forza", "crates/forza", &[]), ("xtask", "xtask", &[])];
            rows.map(|(n, d, deps)| {
                let deps = deps.iter().map(ToString::to_string).collect();
                (n.to_owned(), d.to_owned(), deps)
            })
            .to_vec()
        };
        // the locked crates, the toolchain and its config, and the root manifest: every crate can differ
        for f in [
            "Cargo.lock",
            "rust-toolchain.toml",
            ".cargo/config.toml",
            "Cargo.toml",
        ] {
            assert_eq!(affected(info(), &format!("{f}\n")), None, "{f}");
        }
        // a crate's own manifest is that crate's change
        let set = |a: &[&str]| a.iter().map(ToString::to_string).collect::<BTreeSet<_>>();
        assert_eq!(
            affected(info(), "crates/forza/Cargo.toml\n"),
            Some(set(&["forza"]))
        );
    }

    #[test]
    fn quick_lints_the_changed_crates_and_their_dependents() {
        let line = |c: Command| {
            c.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        // the scope `quick` hands each target is `target_scopes`' `-p` set of the changed crates, never the workspace
        let scope = ["-p", "forza", "-p", "launcher"].map(String::from);
        let args = line(clippy(Path::new("C:/repo"), TARGET, &scope));
        assert!(
            args.contains("--locked --all-targets --target x86_64-pc-windows-msvc -p forza -p launcher -- -D warnings"),
            "{args}"
        );
        assert!(!args.contains("--workspace"), "{args}");
    }

    #[test]
    fn quick_tests_the_changed_crates_and_their_dependents() {
        let info = || {
            [
                ("forza", "crates/forza", &[][..]),
                ("forza-ui", "crates/ui", &["forza"][..]),
                ("launcher", "crates/launcher", &["forza", "forza-ui"][..]),
                ("forza-tools", "crates/tools", &[][..]),
                ("forza-panel", "native/panel", &[][..]),
                ("xtask", "xtask", &["cap"][..]),
                ("cap", "xtask/cap", &[][..]),
            ]
            .map(|(n, d, deps)| {
                let deps = deps.iter().map(ToString::to_string).collect();
                (n.to_owned(), d.to_owned(), deps)
            })
            .to_vec()
        };
        let set = |a: &[&str]| a.iter().map(ToString::to_string).collect::<BTreeSet<_>>();
        assert_eq!(affected(info(), ""), Some(set(&[])));
        assert_eq!(affected(info(), "docs/README.md\n"), Some(set(&[])));
        // a leaf: itself only
        let leaf = affected(info(), "crates/launcher/src/main.rs\n").unwrap();
        assert_eq!(leaf, set(&["launcher"]));
        // the shared library: its dependents too, through forza-ui
        let lib = affected(info(), "crates/forza/src/lib.rs\n").unwrap();
        assert_eq!(lib, set(&["forza", "forza-ui", "launcher"]));
        // the launcher embeds the panel's files
        let panel = affected(info(), "native/panel/src/lib.rs\n").unwrap();
        assert_eq!(panel, set(&["forza-panel", "launcher"]));
        // native/clippy.toml is the clippy config of every crate under native/ (the nearest file above a crate)
        let native = || {
            let rows: [(&str, &str, &[&str]); 2] = [
                ("forza-native", "native/common", &[]),
                ("forza-panel", "native/panel", &[]),
            ];
            rows.map(|(n, d, deps)| {
                let deps = deps.iter().map(ToString::to_string).collect();
                (n.to_owned(), d.to_owned(), deps)
            })
            .to_vec()
        };
        assert_eq!(
            affected(native(), "native/clippy.toml\n"),
            Some(set(&["forza-native", "forza-panel"]))
        );
        assert_eq!(affected(info(), "Cargo.lock\n"), None);
        // xtask's own crate: its tests and lints, not the workspace's
        assert_eq!(
            affected(info(), "xtask/src/main.rs\n"),
            Some(set(&["xtask"]))
        );
        assert_eq!(
            affected(info(), "xtask/cap/src/lib.rs\n"),
            Some(set(&["cap", "xtask"]))
        );
        // filters per target, never an empty one
        assert_eq!(test_filter(&set(&[]), TARGET), None);
        assert_eq!(test_filter(&set(&["forza-panel"]), TARGET), None);
        assert_eq!(
            test_filter(&panel, TARGET).as_deref(),
            Some("package(launcher)")
        );
        assert_eq!(
            test_filter(&panel, PANEL_TARGET).as_deref(),
            Some("package(forza-panel)")
        );
        assert_eq!(
            test_filter(&set(&["forza-hook", "forza"]), PANEL_TARGET).as_deref(),
            Some("package(forza-hook)")
        );
        // a base git cannot resolve: no file list, so quick checks everything
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(
            changed_files(root, "0123456789abcdef0123456789abcdef01234567").unwrap(),
            None
        );
    }

    #[test]
    fn the_frozen_hash_file_parses() {
        let ([hash, tag, commit], _) =
            parse_freeze(include_str!("../../editions/2017-18/fpr_emu.sha256")).unwrap();
        assert!(tag.starts_with('v') && commit.len() == 40 && hash.len() == 64);
        assert!(parse_freeze("abc v0.6.1 33f4591e").is_err());
        assert!(parse_freeze("").is_err());
        assert!(parse_freeze(&format!("{hash} {tag} {commit} freeze")).is_err());
    }

    #[test]
    fn a_pinned_freeze_file_wins_over_changed_sources() {
        let hash = "2c9a7a4372a6c21e7de4bbbb1d951f9f33e4d0154b352f7c46508bd73f9c6e00";
        let commit = "bf787185b98b61beebbb52e0d4adff33cb8a4c18";
        let changed = "native/common/src/lib.rs\n";
        let (_, pinned) = parse_freeze(&format!("{hash} v0.7.2 {commit} pin")).unwrap();
        assert!(pinned && keep_frozen(pinned, changed));
        let (_, pinned) = parse_freeze(&format!("{hash} v0.7.2 {commit}")).unwrap();
        assert!(!pinned && !keep_frozen(pinned, changed));
        assert!(keep_frozen(pinned, "\n"));
    }

    #[test]
    fn zip_dates_are_the_utc_date_of_the_seconds() {
        let d = dos_time(1_793_281_530).unwrap(); // 2026-10-29 13:45:30 UTC
        let got = (
            d.year(),
            d.month(),
            d.day(),
            d.hour(),
            d.minute(),
            d.second(),
        );
        assert_eq!(got, (2026, 10, 29, 13, 45, 30));
        assert!(dos_time(0).is_err()); // 1970 is before ZIP's 1980
    }

    #[test]
    fn only_2010_11_paths_bring_in_the_2010_11_hooks() {
        assert!(!touches_1011(
            "crates/launcher/src/lib.rs\ncrates/forza/src/setup/e1718.rs\n"
        ));
        assert!(!touches_1011("editions/2017-18/README.txt\n"));
        assert!(touches_1011(
            "crates/launcher/src/lib.rs\nEditions/2010-11/x.txt\n"
        ));
        assert!(touches_1011("crates/forza/src/setup/e1011.rs\n"));
        assert!(touches_1011("native/hook/src/lib.rs\n"));
        assert!(touches_1011("Cargo.lock\n"));
        // the files that profile or lint every crate reach the 2010-11 hooks, so a break only they meet cannot wait
        // for the weekly --release run; a crate's own manifest is that crate's
        assert!(touches_1011("Cargo.toml\n"));
        assert!(touches_1011("native/clippy.toml\n"));
        assert!(!touches_1011("crates/forza/Cargo.toml\n"));
        assert_eq!(hooks(false).count(), 2);
        assert!(hooks(false).all(|(f, _, _)| f.contains("e1718")));
        assert_eq!(hooks(true).count(), HOOKS.len());
    }

    #[test]
    fn check_ci_skips_the_2010_11_hooks_only_for_a_known_2017_18_change() {
        let only_1718 = Some("crates/launcher/src/lib.rs\n");
        assert!(!ci_builds_1011(false, only_1718));
        assert!(ci_builds_1011(false, Some("native/hook/src/lib.rs\n")));
        // no --base (the weekly run) or no merge base: everything; --release: what players get
        assert!(ci_builds_1011(false, None));
        assert!(ci_builds_1011(true, only_1718));
    }

    #[test]
    fn check_ci_builds_the_2010_11_hooks_where_its_line_says_in() {
        // the line and what is built must agree, on a manual run's `--base`, over what `changed_files` returns: a base
        // the clone does not have gives no list, a known one the changed files
        for (release, files) in [
            (false, None),
            (false, Some("crates/launcher/src/lib.rs\n")),
            (false, Some("native/hook/src/lib.rs\n")),
            (true, None),
            (true, Some("crates/launcher/src/lib.rs\n")),
        ] {
            let reason = ci_1011_reason(release, Some("origin/main"), files);
            assert_eq!(
                ci_builds_1011(release, files),
                reason.starts_with("in ("),
                "{reason}"
            );
        }
        // CI's `check ci` has no `--base`: everything, as its line says
        assert!(ci_builds_1011(false, None));
        assert!(ci_1011_reason(false, None, None).starts_with("in ("));
    }

    #[test]
    fn a_run_without_the_2010_11_hooks_demands_no_wccfk_dll() {
        // ensure_built's list: a skipped run has no wccfk.dll (build_all removed it) and must not fail for it
        assert!(!required_programs(false).any(|p| p == "programs/wccfk.dll"));
        assert!(required_programs(true).any(|p| p == "programs/wccfk.dll"));
        assert_eq!(
            required_programs(true).count(),
            required_programs(false).count() + 1
        );
        // windows_tests starts the hook's kit test only with e1011
        let kit = Path::new("kit/2010-11");
        assert!(hook_test(kit, false).is_none());
        let (what, c) = hook_test(kit, true).expect("the 2010-11 hook's kit test");
        assert!(what.starts_with("2010-11 hook"), "{what}");
        assert_eq!(
            c.get_program(),
            kit.join("source/build/test_dhcphook.exe").as_os_str()
        );
        // its argument is the point: the 2010-11 hook whose imports the test checks
        let dll = kit.join("programs/wccfk.dll");
        assert_eq!(c.get_args().collect::<Vec<_>>(), [dll.as_os_str()]);
    }

    #[test]
    fn check_ci_says_why_a_base_is_missing() {
        let only_1718 = Some("crates/launcher/src/lib.rs\n");
        assert_eq!(
            ci_1011_reason(false, Some("origin/main"), only_1718),
            "skipped (no 2010-11 path changed since origin/main)"
        );
        assert_eq!(
            ci_1011_reason(false, Some("origin/main"), Some("native/hook/src/lib.rs\n")),
            "in (a 2010-11 path changed)"
        );
        assert_eq!(
            ci_1011_reason(false, None, None),
            "in (no --base: everything)"
        );
        // a base the clone does not have (a force-push, a shallow clone): its own line, not the no-`--base` one
        assert_eq!(
            ci_1011_reason(false, Some("0f00ba5"), None),
            "in (no merge base with 0f00ba5: not fetched?)"
        );
        assert_eq!(
            ci_1011_reason(true, Some("origin/main"), only_1718),
            "in (--release: what players get)"
        );
    }

    #[test]
    fn a_shared_target_is_touched_only_on_a_worktree_switch() {
        let w1 = r"E:\wccf\forza-wccf-w1";
        assert_eq!(switch(w1, w1, w1), Switch::Same);
        // another spelling of the same folder (a shell's case, cargo's `\\?\`, a newline) is no switch
        assert_eq!(
            switch(w1, r"\\?\E:\wccf\forza-wccf-w1\", "e:/WCCF/forza-wccf-w1\n"),
            Switch::Same
        );
        assert_eq!(switch(w1, w1, r"E:\wccf\forza-wccf-w2"), Switch::Touch);
        assert_eq!(switch(w1, w1, ""), Switch::Touch);
        // an xtask.exe built from w2 run in w1 builds nothing, whatever the marker says
        assert_eq!(
            switch(w1, r"E:\wccf\forza-wccf-w2", w1),
            Switch::TouchAndRerun
        );
    }

    #[test]
    fn a_shared_target_serves_one_run_at_a_time() {
        let dir = std::env::temp_dir().join(format!("forza-xtask-lock-{}", std::process::id()));
        let held = target_lock(&dir.join("t")).unwrap();
        let other = std::fs::File::open(dir.join("t/.forza-worktree.lock")).unwrap();
        assert!(other.try_lock().is_err());
        drop(held);
        assert!(other.try_lock().is_ok());
        drop(other);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_wait_ends_when_the_other_run_lets_go() {
        let dir = std::env::temp_dir().join(format!("forza-xtask-wait-{}", std::process::id()));
        let held = target_lock(&dir).unwrap();
        let sooner = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(held);
        });
        let got = target_lock(&dir).unwrap();
        sooner.join().unwrap();
        drop(got);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_run_locks_the_folder_before_it_writes_the_marker() {
        let dir = std::env::temp_dir().join(format!("forza-xtask-order-{}", std::process::id()));
        let (shared, here) = (dir.join("t"), dir.join("w"));
        fs::create_dir_all(&here).unwrap();
        // a repository with no tracked file: the touch writes no mtime
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&here)
            .status()
            .unwrap();
        assert!(init.success());
        let held = target_lock(&shared).unwrap();
        let (d, h) = (shared.clone(), here.clone());
        let waiter = std::thread::spawn(move || shared_target_in(&d, &h, &h));
        std::thread::sleep(Duration::from_millis(500));
        // the waiting run has marked nothing: the marker is written under the lock
        assert!(!shared.join(".forza-worktree").exists());
        drop(held);
        waiter.join().unwrap().unwrap();
        assert_eq!(
            fs::read_to_string(shared.join(".forza-worktree")).unwrap(),
            real_path(&here).display().to_string()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_folder_under_two_spellings_is_no_worktree_switch() {
        let dir = std::env::temp_dir().join(format!("forza-xtask-canon-{}", std::process::id()));
        let (shared, here) = (dir.join("t"), dir.join("w"));
        fs::create_dir_all(&here).unwrap();
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&here)
            .status()
            .unwrap();
        assert!(init.success());
        // first run: marks the folder as this worktree's, in the folder's own spelling
        drop(shared_target_in(&shared, &here, &here).unwrap());
        fs::remove_dir_all(here.join(".git")).unwrap();
        // `w\.` is the same folder (so is a junction or a `subst` drive), so this is Same and runs no `git ls-files`
        let again = shared_target_in(&shared, &here.join("."), &here);
        assert!(again.is_ok(), "{:?}", again.err());
        drop(again);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_shared_folder_is_remapped_in_both_separators() {
        let flags = remap_flags(
            Some(Path::new(r"E:\cargo-target")),
            None,
            Path::new(r"E:\wccf"),
        );
        let remap = |from: &str| {
            flags
                .iter()
                .any(|f| f.contains(&format!("--remap-path-prefix={from}=")))
        };
        assert!(remap(r"E:\cargo-target"), "{flags:?}");
        // cargo keeps the raw variable's `/` in OUT_DIR
        #[cfg(windows)]
        assert!(remap("E:/cargo-target"), "{flags:?}");
    }

    #[test]
    fn a_build_dir_outside_the_target_folder_is_remapped() {
        // the spelling cargo metadata prints here (2026-11-15) and the shipped Launcher holds: `/` up to the folder
        // the hash goes in, `\` before the hash
        let build = Path::new(r"C:/forza-build/e8\969756f5df38da");
        let flags = remap_flags(None, Some(build), Path::new(r"E:\wccf"));
        let remap = |from: &str| {
            flags
                .iter()
                .any(|f| f.contains(&format!("--remap-path-prefix={from}=")))
        };
        assert!(remap(r"C:/forza-build/e8\969756f5df38da"), "{flags:?}");
        // a `build-dir` the user config spells with `\`
        #[cfg(windows)]
        assert!(remap("C:/forza-build/e8/969756f5df38da"), "{flags:?}");
        // where the runner's `target\` is, so a local program spells what a runner's spells
        let to = Path::new("forza-wccf").join("target").display().to_string();
        assert!(
            flags.iter().any(|f| f.contains(&format!("={to}'"))),
            "{flags:?}"
        );
        // the runners set no `build-dir`: nothing is added there
        let runner = remap_flags(None, None, Path::new(r"E:\wccf"));
        assert!(
            !runner.iter().any(|f| f.contains("forza-build")),
            "{runner:?}"
        );
    }

    #[test]
    fn only_a_build_dir_other_than_the_target_folder_counts() {
        let target = Path::new(r"D:\a\Forza-WCCF\Forza-WCCF\target");
        // a runner: cargo reports the target folder itself, in either spelling or case
        for same in [
            r"D:\a\Forza-WCCF\Forza-WCCF\target",
            "D:/a/Forza-WCCF/Forza-WCCF/target/",
            r"d:\a\forza-wccf\forza-wccf\TARGET",
        ] {
            assert_eq!(outside_target(PathBuf::from(same), target), None, "{same}");
        }
        let build = PathBuf::from(r"C:/forza-build/e8\969756f5df38da");
        assert_eq!(outside_target(build.clone(), target), Some(build));
    }

    #[test]
    fn package_dirs_match_git_paths_from_windows_and_git_style_roots() {
        let manifest = r"E:\forza-wccf\crates\launcher\Cargo.toml";
        for (root, manifest) in [
            (r"E:\forza-wccf", manifest),
            (r"E:\forza-wccf", "E:/forza-wccf/crates/launcher/Cargo.toml"),
            ("E:/forza-wccf", manifest),
            (r"e:\Forza-WCCF", manifest),
            (r"\\?\E:\forza-wccf", manifest),
        ] {
            assert_eq!(package_dir(root, manifest).unwrap(), "crates/launcher");
        }
        // a drive-root checkout, a verbatim UNC manifest under a plain UNC root, a folder spelled in another case than git's
        assert_eq!(
            package_dir(r"E:\", r"E:\crates\launcher\Cargo.toml").unwrap(),
            "crates/launcher"
        );
        assert_eq!(
            package_dir(
                r"\\srv\share\repo",
                r"\\?\UNC\srv\share\repo\crates\launcher\Cargo.toml"
            )
            .unwrap(),
            "crates/launcher"
        );
        let dir = package_dir(r"E:\r", r"E:\r\crates\Forza\Cargo.toml").unwrap();
        assert!("crates/forza/src/lib.rs".starts_with(&format!("{dir}/")));
        // git prints `crates/launcher/src/main.rs`; the old code compared it with `crates\launcher/`
        let dir = package_dir(r"E:\r", r"E:\r\crates\launcher\Cargo.toml").unwrap();
        assert!("crates/launcher/src/main.rs".starts_with(&format!("{dir}/")));
        assert_eq!(package_dir(r"E:\r", r"E:\r\Cargo.toml").unwrap(), "");
        assert!(package_dir(r"E:\r", r"D:\x\Cargo.toml").is_err());
        assert!(package_dir(r"E:\r", r"E:\rr\Cargo.toml").is_err());
    }

    #[test]
    fn shipped_docs_never_say_kit() {
        // at run time first: nextest's `--workspace-remap` sets it for test programs built on another machine
        let root = std::env::var_os("CARGO_MANIFEST_DIR")
            .map_or_else(|| env!("CARGO_MANIFEST_DIR").into(), PathBuf::from)
            .join("..");
        let files: Vec<PathBuf> = MAP
            .iter()
            .chain(MAP_1718)
            .filter(|(to, _)| to.ends_with(".txt") || to.ends_with(".md"))
            .map(|(_, from)| root.join(from))
            .chain(
                [
                    "crates/installer/README.md",
                    "crates/installer/CHANGELOG.md",
                ]
                .map(|f| root.join(f)),
            )
            .collect();
        assert!(files.len() >= 8);
        no_kit_word(&files).unwrap();
        let bad = std::env::temp_dir().join("no-kit-word-test.txt");
        fs::write(&bad, "fine\nthe Kit-5 zip\nskit kitchen\n").unwrap();
        assert!(no_kit_word(std::slice::from_ref(&bad)).is_err());
        fs::write(&bad, "skit kitchen _kit_\n").unwrap();
        no_kit_word(&[bad]).unwrap();
    }

    #[test]
    fn mask_hides_the_name_in_any_case() {
        let [p, k] = publisher();
        assert_eq!(
            mask(&format!("a {} b {}{}", p.to_uppercase(), k, p)),
            "a <P> b <P><P>"
        );
        assert_eq!(mask("nothing here"), "nothing here");
    }

    #[test]
    fn a_surname_that_contains_the_name_is_no_mention() {
        let [p, _] = publisher();
        assert!(!names_publisher("I'm Yu Hasegawa, your secretary."));
        assert!(names_publisher(&format!(
            "Hasegawa of {}",
            p.to_uppercase()
        )));
    }

    #[test]
    fn the_allow_list_is_masked_and_clean() {
        let [p, k] = publisher();
        for l in include_str!("../scrub-allow.txt").lines() {
            assert!(
                !l.to_ascii_lowercase().contains(&p) && !l.contains(&k),
                "{l}"
            );
        }
    }
}
