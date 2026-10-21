//! `cargo xtask build | check | package <edition> | stage <edition>`: the 2010-11 kit's build, checks and ZIP, replacing
//! source/build.ps1, check.ps1 and package.ps1; the 2017-18 kit is staged and zipped from its committed programs.
//!
//! WSL is the one build host: `cargo xwin` for the Rust, `clang-cl` + `lld-link` + `llvm-rc` against the MSVC CRT and SDK
//! that xwin caches for the 32-bit C programs; the tests run the Windows programs through WSL interop.
//! `FORZA_STAGE_ROOT` (default `target/`) is where the kits are staged and the checks run; Windows programs get Windows paths.
//!
//! The kit the players get is staged at `target/kit/2010-11/`, copied from the working tree by one table
//! ([`MAP`]); the build puts its programs there, `check` tests that folder, `package` zips it. The 2017-18 kit
//! ([`MAP_1718`]) is staged at `target/kit/2017-18/` the same way; `stage` only copies.

use anyhow::{Context, Result, ensure};
use clap::Parser;
use fs_err as fs;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use xshell::{Shell, cmd};

#[derive(Parser)]
#[command(about = "Build, check and package Forza-WCCF")]
enum Task {
    /// Compile the C programs and the launchers into the staged kits (2010-11 and 2017-18)
    Build,
    /// fmt, clippy, tests, the kit's C self-tests, the scrub; `check quick` = fmt, clippy and the tests of the crates changed since --base
    Check {
        /// `quick`: only the crates changed since the base (and their dependents); no release build, no kit, no smoke.
        /// `ci`: the Linux-side checks of the GitHub workflow (build, fmt, clippy, scrub, licenses, no Windows program runs).
        /// `text`: the scrub, the script-host check and the installer docs' word check; no build, runs anywhere (docs-only changes)
        /// `win`: the Windows-side checks, on a Windows PC against a built `target/kit/` (the CI artifact)
        #[arg(value_parser = ["quick", "ci", "win", "text"])]
        mode: Option<String>,
        /// What `quick` diffs against (its merge-base with HEAD, plus the working tree)
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
    /// Build and open the UI design gallery (crates/forza/examples/gallery.rs) through WSL interop
    Gallery,
    /// The kit ZIP in dist/
    Package {
        /// 2010-11, 2017-18 or installer
        edition: String,
        /// File name without .zip; version.txt is made from it
        #[arg(long)]
        name: Option<String>,
    },
}

const QUICK_BASE: &str = "main";
/// Set by `check` (both tiers): `rust()` then builds with `CARGO_INCREMENTAL=0`, so sccache can serve the workspace crates
/// across worktrees; builds while editing stay incremental.
static NO_INCREMENTAL: AtomicBool = AtomicBool::new(false);

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
    ("source/hook", "native/2010-11/hook"),
    ("source/panel", "native/2010-11/panel"),
    ("source/panel/inject.c", "native/common/inject.c"),
    ("source/crashme.c", "native/2010-11/crashme.c"),
    ("source/card_table/fpr_emu.c", "native/fpr_emu/fpr_emu.c"),
    ("source/launcher", "crates/launcher"),
];
/// The staged 2017-18 kit <- the repository, same rules as [`MAP`]; later rows overlay earlier ones and a row may be one
/// file. The kit's programs are not here: `build` compiles them into the staged kit ([`KEEP_1718`]).
const MAP_1718: &[(&str, &str)] = &[
    ("README.txt", "editions/2017-18/README.txt"),
    ("README.it.txt", "editions/2017-18/README.it.txt"),
    ("README.ja.txt", "editions/2017-18/README.ja.txt"),
    ("CHANGES.txt", "editions/2017-18/CHANGES.txt"),
    ("english", "editions/2017-18/english"),
    ("italian", "editions/2017-18/italian"), // the Italian text: key + Italian, the English behind each key
    ("fonts", "assets/fonts"),
    ("source/README.txt", "native/2017-18/README.txt"),
    ("source/mxhook", "native/2017-18/mxhook"),
    ("source/overlay", "native/2017-18/overlay"),
    ("source/logowin", "native/2017-18/logowin"),
    ("source/fpr_emu", "native/fpr_emu"),
];
/// Product name in the version information of programs both kits share.
const PRODUCT_COMMON: &str = "Forza-WCCF";
const TOP_1718: &str = "Forza-WCCF-2017-18";
/// Built into the staged kit, so staging leaves them alone.
const KEEP: &[&str] = &["programs", "Launcher.exe", "source/build"];
/// Built into the staged 2017-18 kit (`source/build` holds the guard tests, which the kit does not ship; the guard `bin/wccfnet.dll` ships, setup copies it into the game runtime).
const KEEP_1718: &[&str] = &[
    "Launcher.exe",
    "bin/forza-server.exe",
    "bin/winmm.dll",
    "bin/wccfnet.dll",
    "bin/ICMP.DLL",
    "bin/logowin.exe",
    "bin/FPR_Emu.exe",
    "overlay/forza_panel.dll",
    "source/build",
];
/// One C job: `src` is read from the edition's source folder, `out` is relative to the staged kit; no `what`, no version
/// resource. `link` and `compile` are extra clang-cl arguments.
struct Cj {
    src: &'static str,
    out: &'static str,
    what: &'static str,
    compile: &'static [&'static str],
    link: &'static [&'static str],
}

const fn cj(
    src: &'static str,
    out: &'static str,
    what: &'static str,
    compile: &'static [&'static str],
    link: &'static [&'static str],
) -> Cj {
    Cj {
        src,
        out,
        what,
        compile,
        link,
    }
}

/// What differs between the two kits: the staging table, the programs `build` keeps, the ZIP's top folder and product
/// name, where the C sources are read from (relative to the kit, else the repository) and which C programs are built.
struct Ed {
    map: &'static [(&'static str, &'static str)],
    keep: &'static [&'static str],
    top: &'static str,
    product: &'static str,
    /// the 2010-11 kit builds from its own staged `source/`, the 2017-18 kit from the repository's `native/2017-18`
    src: &'static str,
    in_kit: bool,
    /// folders `build` makes in the staged kit
    dirs: &'static [&'static str],
    jobs: &'static [Cj],
}

const NO_CONSOLE: &[&str] = &["user32.lib", "/SUBSYSTEM:WINDOWS", "/ENTRY:mainCRTStartup"]; // the game starts it: no console window
const ED_1011: Ed = Ed {
    map: MAP,
    keep: KEEP,
    top: TOP,
    product: PRODUCT,
    src: "source",
    in_kit: true,
    dirs: &["programs", "source/build"],
    jobs: &[
        cj(
            "hook/wccfk.c",
            "programs/wccfk.dll",
            "Forza-WCCF's hook in the game: stands in for the cabinet's hardware",
            &["/LD"],
            &[],
        ),
        // tests only, not shipped
        cj(
            "panel/fakegame.c",
            "source/build/fakegame.exe",
            "",
            &[],
            &["d3d9.lib", "user32.lib"],
        ),
        // fakegame with an IP-helper import: the 2017-18 guard's tests rename it to wccfnet.dll as setup does
        cj(
            "panel/fakegame_iphlp.c",
            "source/build/fakegame_iphlp.exe",
            "",
            &[],
            &["d3d9.lib", "user32.lib", "iphlpapi.lib"],
        ),
        cj(
            "hook/test_wholemsg.c",
            "source/build/test_wholemsg.exe",
            "",
            &[],
            &[],
        ),
        cj(
            "hook/test_dhcphook.c",
            "source/build/test_dhcphook.exe",
            "",
            &[],
            &["iphlpapi.lib"],
        ),
        cj("crashme.c", "source/build/crashme.exe", "", &[], &[]),
    ],
};
const ED_1718: Ed = Ed {
    map: MAP_1718,
    keep: KEEP_1718,
    top: TOP_1718,
    product: PRODUCT_1718,
    src: "native/2017-18",
    in_kit: false,
    dirs: &["bin", "overlay", "source/build"],
    jobs: &[
        cj(
            "mxhook/mxhook.c",
            "bin/winmm.dll",
            "stands in for the cabinet's hardware, loaded by the game in place of winmm.dll",
            &["/LD"],
            &[],
        ),
        cj(
            "logowin/logowin_standin.c",
            "bin/logowin.exe",
            "the start-up notice window, replaced by one that only records the notice",
            &[],
            &["/SUBSYSTEM:WINDOWS"],
        ),
        // not in the 2017-18 kit: the Linux server's 2010-11 hook loads it (native/2010-11/server-hook)
        cj(
            "netguard/netguard.c",
            "source/build/netguard.dll",
            "",
            &["/LD"],
            &[],
        ),
        cj(
            "netguard/wccfnet.c",
            "bin/wccfnet.dll",
            "the network guard: the game programs import it instead of IPHLPAPI.DLL; it loads the panel when FORZA_PANEL is set",
            &["/LD"],
            &["user32.lib"],
        ),
        cj(
            "netguard/icmp.c",
            "bin/ICMP.DLL",
            "stand-in ICMP.DLL: the game's pings return at once instead of timing out",
            &["/LD"],
            &[],
        ),
        // tests only, not shipped
        cj(
            "netguard/test_icmp.c",
            "source/build/test_icmp.exe",
            "",
            &[],
            &[],
        ),
        cj(
            "netguard/test_wccfnet.c",
            "source/build/test_wccfnet.exe",
            "",
            &[],
            &[],
        ),
        cj(
            "netguard/test_netguard.c",
            "source/build/test_netguard.exe",
            "",
            &[],
            &["iphlpapi.lib"],
        ),
        cj(
            "netguard/panel_probe.c",
            "source/build/panel_probe.dll",
            "",
            &["/LD"],
            &[],
        ),
        cj(
            "netguard/test_panel.c",
            "source/build/test_panel.exe",
            "",
            &[],
            &["user32.lib"],
        ),
    ],
};
/// Programs both kits ship unchanged: compiled once from `native/` into the 2010-11 kit's `source/build`, which
/// `build_all` copies into both kits.
const COMMON: &[Cj] = &[
    cj(
        "common/inject.c",
        "source/build/inject.exe",
        "loads the Forza-WCCF panel into the running game",
        &[],
        &[],
    ),
    cj(
        "fpr_emu/fpr_emu.c",
        "source/build/FPR_Emu.exe",
        "the card table's stand-in",
        &[],
        NO_CONSOLE,
    ),
];

/// Shipped programs that may be console programs, each started only by Launcher.exe with CREATE_NO_WINDOW and its output
/// read through a file or a pipe: inject.exe (exit code and message read from a pipe), forza-server.exe (log file).  FPR_Emu.exe is not here: the game starts it.
const CONSOLE_OK: &[&str] = &[
    "programs/inject.exe",
    "bin/forza-server.exe",
    "programs/forza-server.exe",
];
/// The Subsystem field of a PE file's optional header (2 GUI, 3 console).
fn pe_subsystem(b: &[u8]) -> Option<u16> {
    let at = |o: usize, n: usize| b.get(o..o + n);
    let pe = u32::from_le_bytes(at(0x3c, 4)?.try_into().ok()?) as usize;
    (at(pe, 4)? == b"PE\0\0").then_some(())?;
    Some(u16::from_le_bytes(at(pe + 24 + 68, 2)?.try_into().ok()?))
}
const PROGRAMS_1718: &[&str] = &[
    "Launcher.exe",
    "bin/forza-server.exe",
    "bin/winmm.dll",
    "bin/wccfnet.dll",
    "bin/ICMP.DLL",
    "bin/logowin.exe",
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
    ensure!(
        cfg!(target_os = "linux")
            || matches!(&task, Task::Check { mode: Some(m), .. } if m == "win"),
        "cargo xtask runs in WSL (Linux); the Windows programs start through WSL interop (only `check win` runs on Windows)"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("no repository root")?
        .to_path_buf();
    let stage_root = stage_root(&root);
    let kit = stage_root.join("kit/2010-11");
    let kit_1718 = stage_root.join("kit/2017-18");
    match task {
        Task::Build => {
            let cc = Cc::new()?;
            build_all(&root, &kit, &kit_1718, &cc)
        }
        Task::Cap { root, outdir } => cap(&root, &outdir),
        Task::Gallery => gallery(&root),
        // check and package build first: both used whatever programs were staged last, and a ZIP shipped a Launcher.exe
        // older than its own scripts (2026-10-13, the first Rust run started a deleted panel.py)
        Task::Check { mode, base } => {
            NO_INCREMENTAL.store(true, Ordering::Relaxed);
            match mode.as_deref() {
                Some("quick") => return quick(&root, &stage_root, &base),
                Some("text") => return check_text(&root),
                Some("win") => return check_win(&root, &stage_root, &kit, &kit_1718),
                Some("ci") => {
                    return build_all(&root, &kit, &kit_1718, &Cc::new()?)
                        .and_then(|()| check_ci(&root, &stage_root, &kit, &kit_1718));
                }
                _ => {}
            }
            build_all(&root, &kit, &kit_1718, &Cc::new()?)
                .and_then(|()| check(&root, &stage_root, &kit, &kit_1718))
        }
        Task::Stage { edition } => match edition.as_str() {
            "2010-11" => stage(&root, &kit, ED_1011.map, ED_1011.keep),
            "2017-18" => stage(&root, &kit_1718, ED_1718.map, ED_1718.keep),
            _ => anyhow::bail!("unknown edition {edition} (2010-11 or 2017-18)"),
        },
        Task::Package { edition, name } => match edition.as_str() {
            "installer" => package_installer(&root, name),
            "2010-11" | "2017-18"
                if let Err(e) = build_all(&root, &kit, &kit_1718, &Cc::new()?) =>
            {
                Err(e)
            }
            "2010-11" => package(&root, &kit, &name.unwrap_or_else(|| TOP.into()), true),
            "2017-18" => package(
                &root,
                &kit_1718,
                &name.unwrap_or_else(|| TOP_1718.into()),
                false,
            ),
            _ => anyhow::bail!("unknown edition {edition} (2010-11 or 2017-18)"),
        },
    }
}

// ---- the host ----

/// Where the kits are staged and the checks run: `FORZA_STAGE_ROOT`, else `target/`.
fn stage_root(root: &Path) -> PathBuf {
    std::env::var_os("FORZA_STAGE_ROOT").map_or_else(|| root.join("target"), PathBuf::from)
}

/// A path as the Windows programs want it: `wslpath -w` (a /mnt/e path becomes E:\..., anything else \\wsl.localhost\...).
fn win(p: &Path) -> String {
    if let Ok(o) = Command::new("wslpath").arg("-w").arg(p).output()
        && o.status.success()
    {
        return String::from_utf8_lossy(&o.stdout).trim_end().to_owned();
    }
    p.display().to_string()
}

/// A command argument for a Windows program: an absolute Linux path becomes a Windows path (other arguments pass as they are).
fn win_arg(a: &str) -> String {
    if a.starts_with('/') {
        win(Path::new(a))
    } else {
        a.to_owned()
    }
}

/// Before a Windows program starts from Linux: WSL hands the program only the variables listed in WSLENV, so list those the
/// command sets (`/p`: a path, translated).
fn run_env(c: &mut Command) {
    let list: Vec<String> = c
        .get_envs()
        .filter_map(|(k, v)| {
            let v = v?.to_str()?;
            Some(format!(
                "{}{}",
                k.to_str()?,
                if v.starts_with('/') { "/p" } else { "" }
            ))
        })
        .collect();
    if !list.is_empty() {
        c.env("WSLENV", list.join(":"));
    }
}

/// `cargo xwin <sub>` for the Windows target (the MSVC CRT and SDK come from xwin's cache; Microsoft's license
/// for them is accepted by setting XWIN_ACCEPT_LICENSE).
fn rust(sub: &str, root: &Path) -> Command {
    let mut c = Command::new("cargo");
    c.args(["xwin", sub]).current_dir(root);
    if NO_INCREMENTAL.load(Ordering::Relaxed) {
        c.env("CARGO_INCREMENTAL", "0");
    }
    c.env("XWIN_ARCH", "x86,x86_64");
    xwin_env(&mut c);
    // `cargo xwin test` would run the test programs under wine; WSL runs them itself (interop), and winrun.sh starts
    // the launcher's run tests through explorer.exe (interop's "Prefer System32 images" would make the run refuse)
    c.env(
        "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER",
        root.join("xtask/winrun.sh"),
    );
    c
}

/// The MSVC CRT and Windows SDK xwin fetches: pinned, so a new release on Microsoft's manifest cannot change the build
/// (the CI cache is keyed by these two).
const XWIN_CRT_VERSION: &str = "14.44.17.14";
const XWIN_SDK_VERSION: &str = "10.0.26100";

/// The environment every `cargo xwin` run of xtask shares: licence accepted, xtask's cache, the pinned CRT and SDK.
fn xwin_env(c: &mut Command) {
    c.env("XWIN_ACCEPT_LICENSE", "1")
        .env("XWIN_CACHE_DIR", xwin_root())
        .env("XWIN_CRT_VERSION", XWIN_CRT_VERSION)
        .env("XWIN_SDK_VERSION", XWIN_SDK_VERSION);
}

fn run_ok(mut c: Command, what: &str) -> Result<()> {
    ensure!(
        c.status().with_context(|| what.to_owned())?.success(),
        "{what} failed"
    );
    Ok(())
}

/// xtask's own xwin cache (`XWIN_CACHE_DIR`), apart from the default `~/.cache/cargo-xwin` that a bare `cargo xwin`
/// uses: xwin re-downloads its cache whenever a run asks for architectures its `DONE` line lacks, deleting `dl\` and
/// `unpack\` under every build in flight, so one bare run with the default `x86_64,aarch64` broke two full checks
/// (2026-10-19). xtask always asks for `x86,x86_64`, so its cache never flips.
fn xwin_root() -> PathBuf {
    std::env::var_os("HOME").map_or_else(
        || PathBuf::from(".xwin-cache"),
        |h| Path::new(&h).join(".cache/cargo-xwin-xtask"),
    )
}

/// The C compiler: `clang-cl` + `lld-link` + `llvm-rc` against xwin's 32-bit CRT and SDK; holds the xwin cache folder.
struct Cc(PathBuf);

impl Cc {
    fn new() -> Result<Cc> {
        let cache = xwin_root().join("xwin");
        if !cache.join("sdk/lib/um/x86").is_dir() {
            let mut c = Command::new("cargo");
            c.args(["xwin", "cache", "xwin", "--xwin-arch", "x86,x86_64"]);
            xwin_env(&mut c);
            run_ok(c, "cargo xwin cache (the 32-bit CRT and SDK)")?;
        }
        Ok(Cc(cache))
    }
}

/// The design gallery built for Windows and started from the repository root, where it finds `assets/fonts/`.
/// `FORZA_CARDS` (a folder of card pictures, Linux or Windows path) passes on to it.
fn gallery(root: &Path) -> Result<()> {
    let mut c = rust("build", root);
    c.args(["-p", "forza", "--example", "gallery", "--target", TARGET]);
    run_ok(c, "cargo build --example gallery")?;
    let exe = root
        .join("target")
        .join(TARGET)
        .join("debug/examples/gallery.exe");
    let mut g = Command::new(exe);
    g.current_dir(root);
    if let Some(cards) = std::env::var_os("FORZA_CARDS") {
        g.env("FORZA_CARDS", cards);
    }
    run_env(&mut g);
    run_ok(g, "gallery.exe")
}

/// `cap.exe` (xtask/cap, in no kit) built for Windows, then run through WSL interop with Windows paths.
fn cap(game_root: &Path, outdir: &Path) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("no repository root")?;
    let mut b = rust("build", root);
    b.args(["--locked", "-p", "cap", "--target", TARGET]);
    run_ok(b, "cargo build -p cap")?;
    let exe = root.join("target").join(TARGET).join("debug/cap.exe");
    let mut c = Command::new(exe);
    c.arg(win(game_root)).arg(win(outdir));
    run_env(&mut c);
    run_ok(c, "cap")
}

// ---- the staged kit ----

/// Wipe the kit except the built outputs (`keep`), then copy every tracked file of `map` from the working tree.
fn stage(root: &Path, kit: &Path, map: &[(&str, &str)], keep: &[&str]) -> Result<()> {
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
            // clippy and the tests build xtask for the Windows target too, where there is no mode to set
            #[cfg(unix)]
            if matches!(
                dst.extension().and_then(|e| e.to_str()),
                Some("exe" | "dll" | "pyd")
            ) {
                use std::os::unix::fs::PermissionsExt; // WSL starts a Windows program only from an executable file
                fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755))?;
            }
        }
    }
    println!("staged {}", kit.display());
    Ok(())
}

fn wipe(dir: &Path, rel: &str, keep: &[&str]) -> Result<()> {
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

/// The workspace version: the first `version = "..."` line of the root Cargo.toml ([workspace.package]).
fn workspace_version(root: &Path) -> Result<String> {
    let text = fs::read_to_string(root.join("Cargo.toml"))?;
    text.lines()
        .find_map(|l| {
            l.strip_prefix("version = \"")?
                .strip_suffix('"')
                .map(str::to_owned)
        })
        .context("no version in the root Cargo.toml")
}

// ---- build ----

/// Compile jobs running at once: every C program of both editions and the launcher, joined by `build_all`.
type Jobs<'s> = Vec<std::thread::ScopedJoinHandle<'s, (String, Result<()>)>>;

/// Stage both kits, build everything at once (about 20 processes, no two writing the same file: each output stem is
/// unique in its edition's `source/build`), then report every failure.
fn build_all(root: &Path, kit: &Path, kit_1718: &Path, cc: &Cc) -> Result<()> {
    let mut failed = vec![];
    let res = std::thread::scope(|s| {
        let mut jobs = build(s, root, kit, cc)?;
        jobs.extend(build_c_jobs(s, root, kit_1718, cc, &ED_1718)?);
        jobs.extend(spawn_c(
            s,
            root,
            kit,
            cc,
            PRODUCT_COMMON,
            &root.join("native"),
            COMMON,
        )?);
        for j in jobs {
            let (what, r) = j.join().expect("build thread");
            if let Err(e) = r {
                failed.push(format!("{what}: {e:#}"));
            }
        }
        Ok::<_, anyhow::Error>(())
    });
    res?;
    ensure!(
        failed.is_empty(),
        "{} build step(s) failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
    // the same three programs in both kits: Launcher.exe runs either edition (launcher::kit::Edition, from the layout),
    // forza_panel.dll finds its edition the same way
    let release = root.join("target").join(TARGET).join("release");
    let panel = kit.join("source/build/forza_panel.dll");
    let common = |f: &str| kit.join("source/build").join(f);
    for (from, to) in [
        (release.join("Launcher.exe"), kit.join("Launcher.exe")),
        (
            release.join("forza-server.exe"),
            kit.join("programs/forza-server.exe"),
        ),
        (panel.clone(), kit.join("programs/forza_panel.dll")),
        (release.join("Launcher.exe"), kit_1718.join("Launcher.exe")),
        (
            release.join("forza-server.exe"),
            kit_1718.join("bin/forza-server.exe"),
        ),
        (panel, kit_1718.join("overlay/forza_panel.dll")),
        (common("inject.exe"), kit.join("programs/inject.exe")),
        (common("FPR_Emu.exe"), kit.join("programs/FPR_Emu.exe")),
        (common("FPR_Emu.exe"), kit_1718.join("bin/FPR_Emu.exe")),
    ] {
        fs::create_dir_all(to.parent().unwrap())?;
        fs::copy(from, &to)?;
        println!("built {}", to.display());
    }
    Ok(())
}

fn build<'s>(
    s: &'s std::thread::Scope<'s, '_>,
    root: &Path,
    kit: &Path,
    cc: &'s Cc,
) -> Result<Jobs<'s>> {
    let mut jobs = build_c_jobs(s, root, kit, cc, &ED_1011)?;
    // programs\ is kept between builds: the C panel it held up to step 9.4c must not ship beside forza_panel.dll
    let _ = fs::remove_file(kit.join("programs/wccfpanel.dll"));
    let obj = kit.join("source/build");

    // the toolchain is pinned by rust-toolchain.toml, the crates by Cargo.lock, the static runtime by .cargo/config.toml
    let mut c = rust("build", root);
    c.args([
        "--release",
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
    // the panel (step 9.4c: in place of the C wccfpanel.dll), 32-bit like the game; build_all copies it into both kits
    let mut c = rust("build", root);
    c.args([
        "--release",
        "--locked",
        "-p",
        "forza-panel",
        "--target",
        PANEL_TARGET,
    ]);
    let (dll, to) = (
        root.join("target")
            .join(PANEL_TARGET)
            .join("release/forza_panel.dll"),
        obj.join("forza_panel.dll"),
    );
    // one thread for both: two `cargo xwin` at once race on cargo-xwin's clang-cl symlink ("failed to remove file")
    jobs.push(s.spawn(move || {
        let r = run_ok(
            launcher,
            "cargo build -p launcher -p forza-server -p wccf-installer",
        )
        .and_then(|()| run_ok(c, "cargo build -p forza-panel"))
        .and_then(|()| fs::copy(&dll, &to).map(drop).map_err(Into::into));
        (
            "cargo build -p launcher -p forza-server -p wccf-installer -p forza-panel".into(),
            r,
        )
    }));
    Ok(jobs)
}

/// Stages the edition's kit, makes its folders and spawns one thread per C job.
fn build_c_jobs<'s>(
    s: &'s std::thread::Scope<'s, '_>,
    root: &Path,
    kit: &Path,
    cc: &'s Cc,
    ed: &Ed,
) -> Result<Jobs<'s>> {
    stage(root, kit, ed.map, ed.keep)?;
    for d in ed.dirs {
        fs::create_dir_all(kit.join(d))?;
    }
    let src = if ed.in_kit {
        kit.join(ed.src)
    } else {
        root.join(ed.src)
    };
    spawn_c(s, root, kit, cc, ed.product, &src, ed.jobs)
}

fn spawn_c<'s>(
    s: &'s std::thread::Scope<'s, '_>,
    root: &Path,
    kit: &Path,
    cc: &'s Cc,
    product: &'static str,
    src: &Path,
    jobs: &'static [Cj],
) -> Result<Jobs<'s>> {
    let (version, obj) = (workspace_version(root)?, kit.join("source/build"));
    Ok(jobs
        .iter()
        .map(|j| {
            let (version, src, obj, out) = (
                version.clone(),
                src.to_owned(),
                obj.clone(),
                kit.join(j.out),
            );
            s.spawn(move || {
                let r = Shell::new().map_err(Into::into).and_then(|sh| {
                    build_c(
                        &sh, cc, product, &version, &src, &obj, j.src, &out, j.what, j.compile,
                        j.link,
                    )
                });
                (j.src.to_owned(), r)
            })
        })
        .collect())
}

/// The runtime linked in (/MT), so no Visual C++ runtime is needed on the player's PC; /Brepro: the same source, the same file.
/// `what` becomes the program's version information (what a file's Properties show); no `what`, no resource.
/// Kit 5.5's winmm.dll, nameless and unsigned, was quarantined as a trojan when it was unzipped (2026-10-10): `package` refuses
/// a kit whose programs lack it.
#[allow(clippy::too_many_arguments)]
fn build_c(
    sh: &Shell,
    cc: &Cc,
    product: &str,
    version: &str,
    src: &Path,
    obj: &Path,
    source: &str,
    out: &Path,
    what: &str,
    compile: &[&str],
    link: &[&str],
) -> Result<()> {
    let file = out.file_name().unwrap().to_string_lossy().into_owned();
    let name = out.file_stem().unwrap().to_string_lossy().into_owned();
    let mut parts: Vec<&str> = version.split('.').collect();
    parts.resize(4, "0");
    let comma = parts.join(",");
    let kind = if file.ends_with(".dll") { "0x2" } else { "0x1" };
    let mut res = vec![];
    if !what.is_empty() {
        let rc = format!(
            "1 VERSIONINFO\r\nFILEVERSION {comma}\r\nPRODUCTVERSION {comma}\r\nFILEOS 0x40004\r\nFILETYPE {kind}\r\nBEGIN\r\nBLOCK \"StringFileInfo\"\r\nBEGIN\r\nBLOCK \"040904b0\"\r\nBEGIN\r\n\
             VALUE \"CompanyName\", \"Forza-WCCF community\"\r\nVALUE \"FileDescription\", \"{product} - {file}: {what}\"\r\nVALUE \"FileVersion\", \"{version}\"\r\nVALUE \"InternalName\", \"{name}\"\r\n\
             VALUE \"LegalCopyright\", \"Community-made tools for your own copy of the game; the game is not included\"\r\nVALUE \"OriginalFilename\", \"{file}\"\r\nVALUE \"ProductName\", \"{product}\"\r\n\
             VALUE \"ProductVersion\", \"{version}\"\r\nEND\r\nEND\r\nBLOCK \"VarFileInfo\"\r\nBEGIN\r\nVALUE \"Translation\", 0x409, 1200\r\nEND\r\nEND\r\n"
        );
        let (rc_path, res_path) = (
            obj.join(format!("{name}.rc")),
            obj.join(format!("{name}.res")),
        );
        fs::write(&rc_path, rc)?;
        cmd!(sh, "llvm-rc /FO {res_path} {rc_path}")
            .run()
            .with_context(|| format!("rc failed for {source}"))?;
        res.push(res_path);
    }
    let input = src.join(source);
    let fe = format!("/Fe{}", out.display());
    {
        let x = &cc.0;
            // -Wno-unused-function: clang warns about the panel's two dead statics (draw_overlay, draw_text), which cl only does at /W4
            // the 32-bit CRT and SDK from xwin; no PDB (cl wrote none either: no /DEBUG), so the same source gives the same file
            let inc: Vec<String> = ["crt/include", "sdk/include/ucrt", "sdk/include/um", "sdk/include/shared"]
                .iter()
                .flat_map(|d| ["-imsvc".to_owned(), x.join(d).display().to_string()])
                .collect();
            let lib: Vec<String> = ["crt/lib/x86", "sdk/lib/ucrt/x86", "sdk/lib/um/x86"]
                .iter()
                .map(|d| format!("/libpath:{}", x.join(d).display()))
                .collect();
            let implib = format!("/IMPLIB:{}", obj.join(format!("{name}.lib")).display());
            let fo = format!("/Fo{}/", obj.display());
            cmd!(sh, "clang-cl-19 --target=i686-pc-windows-msvc -fuse-ld=lld /nologo /O2 /MT /W3 -Wno-unused-function /Brepro /D_CRT_SECURE_NO_WARNINGS {compile...} {inc...} {input} {fo} {fe} /link /Brepro {implib} {lib...} {res...} {link...}")
                .run()
    }
    .with_context(|| format!("compiling {source} failed"))?;
    println!("built {}", out.display());
    Ok(())
}

// ---- check ----

/// What `check` collects: the failed checks' names.
struct Checks {
    kit: PathBuf,
    /// scratch folders of the checks, beside the kit (a Windows drive when the kit is on one)
    tmp: PathBuf,
    failed: Vec<String>,
}

impl Checks {
    /// Run a check's command to its end; print its output (only the last line if `last`); a nonzero exit fails `what`.
    fn run(&mut self, what: &str, mut c: Command, last: bool) {
        run_env(&mut c);
        match c.output() {
            Ok(o) => {
                self.report(what, &o, last);
            }
            Err(e) => self.fail(what, &format!("could not start: {e}")),
        }
    }

    fn report(&mut self, what: &str, o: &Output, last: bool) {
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

    fn fail(&mut self, what: &str, why: &str) {
        eprintln!("{what}: {why}");
        self.failed.push(what.to_owned());
    }
}

/// One check at a time on this PC (held until dropped).
fn check_lock() -> Result<std::fs::File> {
    let lock = std::fs::File::create(std::env::temp_dir().join("forza-xtask-check.lock"))?;
    if lock.try_lock().is_err() {
        println!("waiting for another `cargo xtask check` on this PC ...");
        lock.lock()?;
    }
    Ok(lock)
}

/// Workspace packages whose files differ from the base's merge-base with HEAD (committed, staged, unstaged or untracked),
/// plus every package that depends on one by path. A change to xtask, Cargo.lock, a Cargo.toml or the toolchain means all.
fn changed_packages(root: &Path, base: &str) -> Result<Option<BTreeSet<String>>> {
    let git = |args: &[&str]| -> Result<String> {
        let o = Command::new("git").args(args).current_dir(root).output()?;
        ensure!(o.status.success(), "git {} failed", args.join(" "));
        Ok(String::from_utf8(o.stdout)?)
    };
    let mb = git(&["merge-base", base, "HEAD"])?.trim().to_owned();
    let mut files = git(&["diff", "--name-only", &mb])?;
    files += &git(&["ls-files", "--others", "--exclude-standard"])?;
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
        .map(|p| {
            let dir = Path::new(p["manifest_path"].as_str().unwrap_or_default())
                .parent()
                .and_then(|d| d.strip_prefix(root).ok())
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            let deps = p["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| !d["path"].is_null())
                .filter_map(|d| d["name"].as_str().map(str::to_owned))
                .collect();
            (p["name"].as_str().unwrap_or_default().to_owned(), dir, deps)
        })
        .collect();
    let mut hit = BTreeSet::new();
    for f in files.lines() {
        let xtask = f == "xtask" || f.starts_with("xtask/");
        if xtask || f == "Cargo.lock" || f == "rust-toolchain.toml" || f.starts_with(".cargo/") {
            return Ok(None);
        }
        // the owning package: the longest package dir that prefixes the file
        if let Some((n, _, _)) = info
            .iter()
            .filter(|(_, d, _)| !d.is_empty() && f.starts_with(&format!("{d}/")))
            .max_by_key(|(_, d, _)| d.len())
        {
            hit.insert(n.clone());
        } else if f == "Cargo.toml" {
            return Ok(None);
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
            return Ok(Some(hit));
        }
        hit.extend(more);
    }
}

/// `cargo xtask check quick`: fmt, clippy and tests for the packages changed since `base`; no release build, kit or smoke.
fn quick(root: &Path, stage_root: &Path, base: &str) -> Result<()> {
    let _lock = check_lock()?;
    let pkgs = changed_packages(root, base)?;
    let scope: Vec<String> = match &pkgs {
        None => {
            println!("quick: xtask, Cargo.lock or a manifest changed: whole workspace");
            vec!["--workspace".into()]
        }
        Some(p) if p.is_empty() => {
            println!("quick: no crate changed since {base}: fmt only");
            vec![]
        }
        Some(p) => {
            println!(
                "quick: {}",
                p.iter().cloned().collect::<Vec<_>>().join(", ")
            );
            p.iter()
                .flat_map(|n| ["-p".to_owned(), n.clone()])
                .collect()
        }
    };
    // the launcher's and the panel's tests start the C stand-ins and the panel DLL that `cargo xtask build` puts in the kit
    let needs_kit = pkgs
        .as_ref()
        .is_none_or(|p| p.contains("launcher") || p.contains("forza-panel"));
    let stand_in = stage_root.join("kit/2010-11/source/build/fakegame.exe");
    ensure!(
        !needs_kit || stand_in.is_file(),
        "{} is missing: the launcher and panel tests need the stand-ins; run `cargo xtask build` once",
        stand_in.display()
    );
    let mut fmt = Command::new("cargo");
    fmt.args(["fmt", "--all", "--check"]).current_dir(root);
    run_ok(fmt, "cargo fmt")?;
    if scope.is_empty() {
        return Ok(());
    }
    let mut clippy = rust("clippy", root);
    clippy.args(["--locked", "--all-targets", "--target", TARGET]);
    clippy.args(&scope).args(["--", "-D", "warnings"]);
    run_ok(clippy, "cargo clippy")?;
    let mut test = rust("test", root);
    test.args(["--locked", "--target", TARGET]).args(&scope);
    run_ok(test, "cargo test")?;
    println!("quick check passed (no release build, no installer smoke, no kit tests)");
    Ok(())
}

/// The staged kits must hold the built programs and the C test programs.
fn ensure_built(kit: &Path, kit_1718: &Path) -> Result<()> {
    for p in PROGRAMS.iter().chain(&[
        "source/build/fakegame.exe",
        "source/build/fakegame_iphlp.exe",
        "source/build/test_wholemsg.exe",
        "source/build/test_dhcphook.exe",
        "source/build/crashme.exe",
    ]) {
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
    ]) {
        ensure!(
            kit_1718.join(p).is_file(),
            "2017-18 {p} is missing: run `cargo xtask build` first"
        );
    }
    Ok(())
}

/// The checks that start Windows programs from the staged kits (WSL interop, or a Windows PC).
fn windows_tests(ck: &mut Checks, kit: &Path, kit_1718: &Path) -> Result<()> {
    ck.run(
        "whole messages: a batch in pieces waits for its rest, bytes that are no batch are skipped",
        Command::new(kit.join("source/build/test_wholemsg.exe")),
        false,
    );

    {
        let mut c = Command::new(kit.join("source/build/test_dhcphook.exe"));
        c.arg(win(&kit.join("programs/wccfk.dll")));
        ck.run(
            "2010-11 hook: the IpReleaseAddress and IpRenewAddress imports now point into wccfk.dll",
            c,
            false,
        );
    }
    panel_selftests(ck)?;
    guard_tests(ck, kit_1718)
}

/// Runs a cargo command; a failure adds `what` to `failed`.
fn cargo_step(what: &str, mut c: Command, failed: &mut Vec<String>) {
    if !c.status().is_ok_and(|s| s.success()) {
        failed.push(what.to_owned());
    }
}

/// The workspace's format and lint (the toolchain from rust-toolchain.toml, read in the root).
fn fmt_clippy(root: &Path, failed: &mut Vec<String>) {
    let mut fmt = Command::new("cargo");
    fmt.args(["fmt", "--all", "--check"]).current_dir(root);
    cargo_step("workspace: cargo fmt", fmt, failed);
    let mut clippy = rust("clippy", root);
    clippy.args([
        "--locked",
        "--workspace",
        "--all-targets",
        "--target",
        TARGET,
        "--",
        "-D",
        "warnings",
    ]);
    cargo_step("workspace: cargo clippy", clippy, failed);
}

fn check(root: &Path, stage_root: &Path, kit: &Path, kit_1718: &Path) -> Result<()> {
    // one check at a time on this PC: the tests use fixed local ports (the relay on 20941, the seat desk on 20932-20936),
    // and two checks at once (two worktrees) failed each other's relay checks
    let _lock = check_lock()?;
    ensure_built(kit, kit_1718)?;
    stage(root, kit, MAP, KEEP)?;
    let mut ck = Checks {
        kit: kit.to_owned(),
        tmp: stage_root.join("check-tmp"),
        failed: vec![],
    };
    windows_tests(&mut ck, kit, kit_1718)?;
    launcher_imports(&mut ck, kit_1718);

    // the Rust workspace, xtask included: format, lint, tests
    fmt_clippy(root, &mut ck.failed);
    let mut test = rust("test", root);
    test.args(["--locked", "--workspace", "--target", TARGET]);
    cargo_step("workspace: cargo test", test, &mut ck.failed);

    scrub(root, &mut ck.failed)?;
    installer_checks(root, stage_root, &mut ck.failed);
    no_script_host(root, &mut ck.failed)?;
    no_python(&[kit, kit_1718], &mut ck.failed)?;

    ensure!(ck.failed.is_empty(), "FAILED: {}", ck.failed.join(", "));
    println!("all checks passed");
    Ok(())
}

/// `cargo xtask check ci` (after the build): everything that needs no Windows program to run, for the Linux CI job.
/// The Windows tests are `check win`, on a Windows runner against this job's kits.
fn check_ci(root: &Path, stage_root: &Path, kit: &Path, kit_1718: &Path) -> Result<()> {
    ensure_built(kit, kit_1718)?;
    stage(root, kit, MAP, KEEP)?;
    let mut ck = Checks {
        kit: kit.to_owned(),
        tmp: stage_root.join("check-tmp"),
        failed: vec![],
    };
    launcher_imports(&mut ck, kit_1718);
    fmt_clippy(root, &mut ck.failed);
    scrub(root, &mut ck.failed)?;
    for (what, r) in [
        ("installer licenses", check_licenses(root)),
        (
            "shipped docs say kit",
            shipped_docs_no_kit(root, &[kit, kit_1718]),
        ),
        ("installer build", installer_built(root).map(drop)),
    ] {
        if let Err(e) = r {
            eprintln!("{what}: {e:#}");
            ck.failed.push(what.to_owned());
        }
    }
    no_script_host(root, &mut ck.failed)?;
    no_python(&[kit, kit_1718], &mut ck.failed)?;
    ensure!(ck.failed.is_empty(), "FAILED: {}", ck.failed.join(", "));
    println!("ci checks passed (the Windows tests are `check win`)");
    Ok(())
}

/// `cargo xtask check text`: the checks that read only tracked text; no build, no kit, no xwin.
fn check_text(root: &Path) -> Result<()> {
    let mut failed = vec![];
    scrub(root, &mut failed)?;
    no_script_host(root, &mut failed)?;
    let src = root.join("crates/installer");
    if let Err(e) = no_kit_word(&[src.join("README.md"), src.join("CHANGELOG.md")]) {
        eprintln!("installer docs say kit: {e:#}");
        failed.push("installer docs say kit".into());
    }
    ensure!(failed.is_empty(), "FAILED: {}", failed.join(", "));
    println!("text checks passed");
    Ok(())
}

/// `cargo xtask check win`: on a Windows PC, against the kits and smoke installer that `check ci` built on Linux
/// (target/kit/** and target/x86_64-pc-windows-msvc/release/Forza-WCCF-Installer.exe, as the CI artifact holds them).
fn check_win(root: &Path, stage_root: &Path, kit: &Path, kit_1718: &Path) -> Result<()> {
    ensure_built(kit, kit_1718)?;
    let exe = installer_built(root)?;
    let mut ck = Checks {
        kit: kit.to_owned(),
        tmp: stage_root.join("check-tmp"),
        failed: vec![],
    };
    fs::create_dir_all(&ck.tmp)?;
    windows_tests(&mut ck, kit, kit_1718)?;
    if let Err(e) = smoke_run(root, stage_root, &exe) {
        eprintln!("installer smoke: {e:#}");
        ck.failed.push("installer smoke".into());
    }
    ensure!(ck.failed.is_empty(), "FAILED: {}", ck.failed.join(", "));
    println!("windows checks passed");
    Ok(())
}

/// The panel's money and relay self-tests: forza_panel.dll loaded by fakegame.exe in a scratch folder (the 2010-11 kit's
/// copy, so its fonts are found above it); the relay test plays two seats through `forza-server relay` on 127.0.0.1:20941 -
/// the panel's and the server's sides of the match relay checked against each other; the panel must have drawn a frame
/// without a panic. The deal is not here: its odds are forza::cards' unit test, its window crates/panel's fakegame stage 6.
/// The frame-wait timing stays out: a busy machine fails it (crates/panel's unit tests cover its arithmetic).
fn panel_selftests(ck: &mut Checks) -> Result<()> {
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
    relay.args(["relay", "30"]).env("WCCF_RELAY_PORT", "20941");
    run_env(&mut relay);
    let mut relay: Child = relay.stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    // waits for its "listening" line (a fixed second lost the race under load, 2026-10-13); the rest of its output is
    // drained, so its prints never block or break
    let out = std::io::BufReader::new(relay.stdout.take().context("the relay's output")?);
    let (up, listening) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut up = Some(up);
        for l in std::io::BufRead::lines(out).map_while(Result::ok) {
            if l.contains("listening")
                && let Some(u) = up.take()
            {
                let _ = u.send(());
            }
        }
    });
    let _ = listening.recv_timeout(Duration::from_secs(20));
    let mut game = Command::new(t.join("fakegame.exe"));
    game.current_dir(&t)
        .arg(win(&kit.join("programs/forza_panel.dll")))
        .arg("wait:12000")
        .env("WCCFPANEL_ROLE", "seat1") // the cabinet's code path (the panel would take a folder named "extracted" for the projector)
        .env("WCCFPANEL_DRYKEYS", "1") // keys logged, never pressed
        .env("WCCFPANEL_FULLSCREEN", "0") // never maximized
        .env("WCCF_DATA", t.join("data"))
        .env("WCCF_KEYS", t.join("data/keys.txt"))
        .env("WCCFPANEL_MONEYTEST", "1")
        .env("WCCFPANEL_RELAYTEST", "1")
        .env("WCCF_RELAY_PORT", "20941");
    run_env(&mut game);
    let run = game.status();
    // a kill would only reach the interop shim: the relay ends by itself after its 30 s
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
/// window shows, and that without the variable nothing loads.
fn guard_tests(ck: &mut Checks, kit: &Path) -> Result<()> {
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
        c.arg(win(&t.join(dll))).current_dir(&t);
        ck.run(what, c, false);
    }
    for (what, mode) in [
        ("wccfnet.dll loads the panel when FORZA_PANEL is set", "on"),
        ("wccfnet.dll loads nothing without FORZA_PANEL", "off"),
    ] {
        let mut c = Command::new(t.join("test_panel.exe"));
        c.arg(win(&t.join("wccfnet.dll")))
            .arg(win(&t.join("panel_probe.dll")))
            .arg(mode)
            .current_dir(&t)
            .env_remove("FORZA_PANEL");
        if mode == "on" {
            c.env("FORZA_PANEL", win(&t.join("panel_probe.dll")));
        }
        ck.run(what, c, false);
    }
    let _ = fs::remove_dir_all(&t);
    Ok(())
}

/// The Launcher (64-bit, with vslink and iroh in it) must not import the IP helper calls wccfnet.dll stubs: iroh's netwatch
/// reads adapters and routes (GetAdaptersAddresses and the like), which is fine, but a release, renew or route write never.
fn launcher_imports(ck: &mut Checks, kit: &Path) {
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

// ---- scrub ----

/// The publisher's name (and its katakana), built from bytes so this file does not spell it.
fn publisher() -> [String; 2] {
    [
        String::from_utf8(vec![0x73, 0x65, 0x67, 0x61]).unwrap(),
        String::from_utf8(vec![0xE3, 0x82, 0xBB, 0xE3, 0x82, 0xAC]).unwrap(),
    ]
}

/// The line with every case-insensitive occurrence of the publisher's name replaced by `<P>`: the form xtask/scrub-allow.txt
/// stores, so the allow-list does not spell the name either.
fn mask(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    while i < line.len() {
        match publisher()
            .iter()
            .find(|p| lower[i..].starts_with(p.as_str()))
        {
            Some(p) => {
                out.push_str("<P>");
                i += p.len();
            }
            None => {
                let c = line[i..].chars().next().unwrap();
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

/// Whether `text` names the publisher outside words that only contain the name: the 2017-18 secretary's surname
/// Hasegawa does ("Ha<P>wa").
fn names_publisher(text: &str) -> bool {
    mask(&text.to_ascii_lowercase().replace("hasegawa", "")).contains("<P>")
}

/// Every tracked text file (not binaries): no line names the publisher except the class-C lines of
/// xtask/scrub-allow.txt (the JVS board ID, the card reader's AES key, keychip strings, third-party tool and host names, the logowin argument
/// name in comments, an in-game club name, and the docs quoting them), and no line matches ~/.config/forza-wccf/scrub.txt
/// (personal data, kept out of the repository). Matches are compared by line text, not by file.
fn scrub(root: &Path, failed: &mut Vec<String>) -> Result<()> {
    let sh = Shell::new()?;
    sh.change_dir(root);
    let allow: HashSet<&str> = include_str!("../scrub-allow.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let [name, kana] = publisher();
    let mut bad = vec![];
    let own = cmd!(sh, "git grep -n -I -i -e {name} -e {kana} -- .")
        .ignore_status()
        .read()?;
    bad.extend(
        own.lines()
            .filter(|l| {
                let text = l.splitn(3, ':').nth(2).unwrap_or("").trim();
                names_publisher(text) && !allow.contains(mask(text).as_str())
            })
            .map(str::to_owned),
    );
    // originals and derived data never get committed: no tracked binary, disc image, IDA database or log
    // (nothing is allowed today; a legitimate fixture would be listed here)
    let tracked = cmd!(
        sh,
        "git ls-files -- *.exe *.dll *.iso *.chd *.bin *.i64 *.idb *.id0 *.til *.nam *.log :!vendor"
    )
    .read()?;
    bad.extend(
        tracked
            .lines()
            .map(|l| format!("tracked binary or log: {l}")),
    );
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    if let Some(list) = home
        .map(|h| h.join(".config/forza-wccf/scrub.txt"))
        .filter(|p| p.is_file())
    {
        let pats: Vec<String> = fs::read_to_string(list)?
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .flat_map(|l| ["-e".to_owned(), l.to_owned()])
            .collect();
        let hits = cmd!(sh, "git grep -n -I -i -E {pats...} -- .")
            .ignore_status()
            .read()?;
        bad.extend(hits.lines().map(str::to_owned));
    }
    for l in &bad {
        println!("scrub: {}", l.chars().take(200).collect::<String>());
    }
    if bad.is_empty() {
        println!("scrub: clean");
    } else {
        failed.push(format!("scrub ({} lines)", bad.len()));
    }
    Ok(())
}

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

// ---- the installer ----

const INSTALLER: &str = "wccf-installer"; // the package; its exe is Forza-WCCF-Installer.exe
const INSTALLER_EXE: &str = "Forza-WCCF-Installer.exe";
const LICENSES: &str = "crates/installer/THIRD-PARTY-LICENSES.json";

/// `profile`: "release" (the ZIP's, built with launcher and forza-server).
fn installer_exe(root: &Path, profile: &str) -> PathBuf {
    root.join("target")
        .join(TARGET)
        .join(profile)
        .join(INSTALLER_EXE)
}

/// The installer's own checks (they were crates/installer/scripts/*.ps1): the license notices, then a synthetic headless run
/// of the release build.
fn installer_checks(root: &Path, stage_root: &Path, failed: &mut Vec<String>) {
    for (what, r) in [
        ("installer licenses", check_licenses(root)),
        ("installer smoke", smoke(root, stage_root)),
    ] {
        if let Err(e) = r {
            eprintln!("{what}: {e:#}");
            failed.push(what.to_owned());
        }
    }
}

/// THIRD-PARTY-LICENSES.json holds a notice with text for every normal/build dependency of the installer on Windows.
fn check_licenses(root: &Path) -> Result<()> {
    let bundle: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(LICENSES))?)?;
    ensure!(bundle["target"] == TARGET, "Wrong license-bundle target.");
    let mut notices = HashSet::new();
    for lib in bundle["third_party_libraries"]
        .as_array()
        .context("no third_party_libraries")?
    {
        let (name, ver) = (
            lib["package_name"].as_str().unwrap_or("?"),
            lib["package_version"].as_str().unwrap_or("?"),
        );
        let licenses = lib["licenses"].as_array().filter(|l| !l.is_empty());
        ensure!(
            lib["license"].as_str().is_some_and(|l| !l.is_empty()) && licenses.is_some(),
            "Missing notices: {name}"
        );
        for n in licenses.unwrap() {
            let t = n["text"].as_str().unwrap_or("").trim();
            ensure!(
                !t.is_empty() && t != "NOT FOUND",
                "Missing license text: {name}"
            );
        }
        notices.insert(format!("{name} v{ver}"));
    }
    let sh = Shell::new()?;
    sh.change_dir(root);
    let fmt = "{p}";
    let tree = cmd!(sh, "cargo tree --locked -p {INSTALLER} --target {TARGET} -e normal,build --prefix none --format {fmt}")
        .read()
        .context("Cannot resolve Windows dependency tree.")?;
    for line in tree.lines() {
        let mut w = line.split_whitespace();
        if let (Some(name), Some(ver)) = (w.next(), w.next().and_then(|v| v.strip_prefix('v'))) {
            ensure!(
                name == INSTALLER || notices.contains(&format!("{name} v{ver}")),
                "Refresh and review {LICENSES}: missing {name} v{ver}"
            );
        }
    }
    println!("installer: Windows dependency license notices are complete");
    Ok(())
}

/// Synthetic headless run of the installer in a scratch folder: inert placeholder files only, no media, keys, kit or game.
/// The exe is the release build that `build_all` makes (a Windows exe; WSL interop runs it).
fn smoke(root: &Path, stage_root: &Path) -> Result<()> {
    let exe = installer_built(root)?;
    smoke_run(root, stage_root, &exe)
}

/// The release installer that `build_all` built; returns its path.
fn installer_built(root: &Path) -> Result<PathBuf> {
    let exe = installer_exe(root, "release");
    ensure!(exe.is_file(), "{} is missing", exe.display());
    Ok(exe)
}

/// The headless run of a built installer.
fn smoke_run(root: &Path, stage_root: &Path, exe: &Path) -> Result<()> {
    let dir = stage_root.join(format!("installer-smoke-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let r = smoke_in(root, exe, &dir);
    let _ = fs::remove_dir_all(&dir);
    r?;
    println!("Headless smoke check passed; no game was launched.");
    Ok(())
}

fn smoke_in(root: &Path, exe: &Path, dir: &Path) -> Result<()> {
    // runs the installer; its exit code must be `expected`; returns stdout
    let run = |args: &[&Path], expected: i32| -> Result<String> {
        let args: Vec<String> = args.iter().map(|a| win_arg(&a.to_string_lossy())).collect();
        let o = Command::new(exe).args(&args).output()?;
        ensure!(
            o.status.code() == Some(expected),
            "Installer returned {:?}, expected {expected}: {args:?}",
            o.status.code()
        );
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let p = Path::new;
    let help = run(&[p("--help")], 0)?;
    ensure!(
        help.contains("Forza-WCCF Installer")
            && help.contains("--game")
            && !help.contains("Revival"),
        "Wrong branding or edition help."
    );
    ensure!(
        run(&[p("--version")], 0)?.trim()
            == format!("Forza-WCCF Installer {}", workspace_version(root)?),
        "Wrong release version/branding."
    );
    run(&[p("--dest")], 1)?;
    run(&[p("--unknown")], 1)?;
    let missing = dir.join("absent");
    run(&[p("--dest"), &missing], 1)?;
    ensure!(!missing.exists(), "Invalid input wrote a destination.");

    let read = |f: PathBuf| -> Result<String> { Ok(fs::read_to_string(f)?.trim().to_owned()) };
    let edition = dir.join("2017-2018");
    let game = edition.join("game");
    fs::create_dir_all(game.join("local"))?;
    fs::write(game.join("client_Release.exe"), "inert test placeholder")?;
    fs::write(game.join("local/SDDY_Table.dat"), "SDDY")?;
    fs::write(edition.join(".probe-a"), "untouched")?;
    run(&[p("--dest"), dir], 0)?;
    let old_edition = dir.join("2010-2011");
    let old_game = old_edition.join("game");
    fs::create_dir_all(old_game.join("local"))?;
    fs::write(old_game.join("client_Release.exe"), "old edition unchanged")?;
    fs::write(old_game.join("local/SBWG_Table.dat"), "SBWG")?;
    for f in [&old_edition, &edition] {
        fs::create_dir_all(f.join("data/save"))?;
        fs::write(f.join("data/save/club.bin"), f.to_string_lossy().as_bytes())?;
    }
    run(&[p("--dest"), dir], 1)?; // both present: require an explicit edition
    run(&[p("--dest"), dir, p("--game"), p("SBWG")], 0)?;
    run(&[p("--dest"), dir, p("--game"), p("SDDY")], 0)?;
    run(
        &[p("--dest"), dir, p("--game"), p("SDDY"), p("--english")],
        1,
    )?;
    run(
        &[
            p("--dest"),
            dir,
            p("--game"),
            p("SDDY"),
            p("--iso"),
            p("missing.iso"),
            p("--key"),
            p("missing.key"),
        ],
        1,
    )?;
    ensure!(
        read(old_game.join("client_Release.exe"))? == "old edition unchanged",
        "Other edition changed."
    );
    for f in [&old_edition, &edition] {
        ensure!(
            read(f.join("data/save/club.bin"))? == f.to_string_lossy(),
            "Edition save changed."
        );
    }
    ensure!(
        read(game.join("client_Release.exe"))? == "inert test placeholder",
        "Existing game files changed."
    );
    ensure!(
        read(edition.join(".probe-a"))? == "untouched",
        "Extraction-only mode ran the hard-link probe."
    );
    Ok(())
}

/// `dist/`: the installer exe, its notices, `NAME.zip` of exactly those files, and SHA256SUMS.txt for the exe and the ZIP.
/// NAME defaults to `Forza-WCCF-Installer-<version>-windows-x64`. Run `cargo xtask check` first (it builds the exe).
fn package_installer(root: &Path, name: Option<String>) -> Result<()> {
    check_licenses(root)?;
    let mut c = rust("build", root);
    c.args(["--release", "--locked", "-p", INSTALLER, "--target", TARGET]);
    run_ok(c, "cargo build --release -p wccf-installer")?;
    let exe = installer_exe(root, "release");
    ensure!(exe.is_file(), "{} is missing", exe.display());
    let name = name.unwrap_or_else(|| {
        format!(
            "Forza-WCCF-Installer-{}-windows-x64",
            workspace_version(root).unwrap_or_default()
        )
    });
    let dist = root.join("dist");
    fs::create_dir_all(&dist)?;
    // explicit payload: never archive the repository, media, kit, saves or target tree
    let src = root.join("crates/installer");
    let docs = [
        "README.md",
        "CHANGELOG.md",
        "LICENSE",
        "THIRD-PARTY-LICENSES.json",
    ];
    no_kit_word(&[src.join("README.md"), src.join("CHANGELOG.md")])?;
    let exe_name = INSTALLER_EXE.to_owned();
    let payload: Vec<(String, PathBuf)> = std::iter::once((exe_name.clone(), exe))
        .chain(docs.iter().map(|d| (d.to_string(), src.join(d))))
        .collect();
    let zip_name = format!("{name}.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(dist.join(&zip_name))?);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (n, path) in &payload {
        fs::copy(path, dist.join(n))?;
        zip.start_file(n, opts)?;
        std::io::copy(&mut fs::File::open(path)?, &mut zip)?;
    }
    zip.finish()?;
    let mut sums = String::new();
    for n in [&exe_name, &zip_name] {
        let hash: String = Sha256::digest(fs::read(dist.join(n))?)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        sums.push_str(&format!("{hash}  {n}\n"));
    }
    fs::write(dist.join("SHA256SUMS.txt"), sums)?;
    let mut ar = zip::ZipArchive::new(fs::File::open(dist.join(&zip_name))?)?;
    let mut got: Vec<String> = (0..ar.len())
        .map(|i| Ok(ar.by_index(i)?.name()?.into_owned()))
        .collect::<Result<_>>()?;
    let mut want: Vec<String> = payload.into_iter().map(|p| p.0).collect();
    got.sort();
    want.sort();
    ensure!(got == want, "Unexpected release archive contents.");
    println!("Prepared {}", dist.join(zip_name).display());
    Ok(())
}

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
        docs.push(kit.join("source/README.txt"));
    }
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
    docs.push(kit.join("source/README.txt"));
    no_kit_word(&docs)?;
    for p in programs {
        ensure!(
            kit.join(p).is_file(),
            "{p} is missing: run `cargo xtask build` first"
        );
    }

    // every program must carry its version information: a nameless unsigned program is what Windows Defender's
    // machine learning flagged (Kit 5.5's first SETUP.exe, then its winmm.dll)
    for p in programs {
        let v = version_strings(&fs::read(kit.join(p))?).unwrap_or_default();
        let get = |k: &str| v.get(k).map_or("", String::as_str);
        // programs built once for both editions (Launcher.exe, forza-server.exe, forza_panel.dll, COMMON) name the project
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
    // Optional signing hook: with FORZA_SIGN_CMD set (e.g. `osslsigncode sign -pkcs12 k.pfx -in {f} -out {f}.s && mv {f}.s {f}`
    // or `signtool sign /f k.pfx /fd sha256 {f}`), every shipped exe/dll goes through it, `{f}` replaced by the file's path,
    // before files.txt and the ZIP are made. Unset: no signing.
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
            ensure!(
                Command::new("sh").arg("-c").arg(&line).status()?.success(),
                "FORZA_SIGN_CMD failed for {f}"
            );
        }
    }

    // for SETUP.exe's update: version.txt, which kit this is, and files.txt, every file this kit ships - an update removes
    // what the old kit's list has and the new one's does not
    let version = name
        .strip_prefix(top)
        .map_or(name, |v| v.strip_prefix('-').unwrap_or(v));
    if !version.is_empty() {
        fs::write(kit.join("version.txt"), format!("kit-{version}\r\n"))?;
    }
    let mut shipped = BTreeSet::new();
    let mut dirs = BTreeSet::new();
    walk(kit, "", &mut shipped, &mut dirs)?;
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
        .compression_method(zip::CompressionMethod::Deflated);
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

    let hash: String = Sha256::digest(fs::read(&zip_path)?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut sha = fs::File::create(dist.join(format!("{name}.zip.sha256")))?;
    write!(sha, "{hash}  {name}.zip\r\n")?;
    println!(
        "packaged {} ({:.1} MB, sha256 {hash})",
        zip_path.display(),
        fs::metadata(&zip_path)?.len() as f64 / 1048576.0
    );
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
    let data = resource(file, object::pe::RT_VERSION)?;
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
        assert!(files.len() > 8);
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
