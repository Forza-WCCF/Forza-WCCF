//! `cargo xtask build | check | package 2010-11`: the 2010-11 kit's build, checks and ZIP, replacing
//! source/build.ps1, check.ps1 and package.ps1.
//!
//! Windows host only for now (MSVC `cl` for the 32-bit C programs, the kit's own Windows Python for the tests).
//! The WSL cross-build (`cargo xwin`, `clang-cl`) comes in a later step.
//!
//! The kit the players get is staged at `target/kit/2010-11/`, copied from the working tree by one table
//! ([`MAP`]); the build puts its programs there, `check` tests that folder, `package` zips it.

use anyhow::{Context, Result, ensure};
use clap::Parser;
use fs_err as fs;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;
use xshell::{Shell, cmd};

#[derive(Parser)]
#[command(about = "Build, check and package the kit")]
enum Task {
    /// Compile the C programs and the launchers into the staged kit
    Build,
    /// fmt, clippy, tests, the kit's Python and C self-tests, the scrub
    Check,
    /// The kit ZIP in dist/
    Package {
        /// The edition to package
        edition: String,
        /// File name without .zip; version.txt is made from it
        #[arg(long, default_value = TOP)]
        name: String,
    },
}

const TOP: &str = "WCCF-2010-11-kit";
const PRODUCT: &str = "WCCF 2010-11 kit";
const TARGET: &str = "x86_64-pc-windows-msvc";

/// The staged kit <- the repository: (kit path, repository path). Whatever is not here does not ship.
const MAP: &[(&str, &str)] = &[
    ("README.txt", "editions/2010-11/README.txt"),
    ("CHANGELOG.md", "editions/2010-11/CHANGELOG.md"),
    ("art", "editions/2010-11/art"),
    ("code", "code"),
    ("languages", "editions/2010-11/languages"),
    ("python", "python"),
    ("source", "source"),
    ("source/launcher", "crates/launcher"),
];
/// Built into the staged kit, so staging leaves them alone.
const KEEP: &[&str] = &["programs", "PLAY.exe", "SETUP.exe", "source/build"];
/// What the kit must hold after a build and a package (the ZIP also gets python.exe from git).
const PROGRAMS: &[&str] = &[
    "PLAY.exe",
    "SETUP.exe",
    "programs/wccfk.dll",
    "programs/FPR_Emu.exe",
    "programs/wccfpanel.dll",
    "programs/inject.exe",
];

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("no repository root")?
        .to_path_buf();
    let kit = root.join("target/kit/2010-11");
    match Task::parse() {
        Task::Build => build(&root, &kit),
        Task::Check => check(&root, &kit),
        Task::Package { edition, name } => {
            ensure!(
                edition == "2010-11",
                "only the 2010-11 kit is packaged here (got {edition})"
            );
            package(&root, &kit, &name)
        }
    }
}

// ---- the staged kit ----

/// Wipe the kit except the built outputs, then copy every tracked file of [`MAP`] from the working tree.
fn stage(root: &Path, kit: &Path) -> Result<()> {
    fs::create_dir_all(kit)?;
    wipe(kit, "")?;
    let sh = Shell::new()?;
    sh.change_dir(root);
    for (to, from) in MAP {
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

fn wipe(dir: &Path, rel: &str) -> Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let r = if rel.is_empty() {
            e.file_name().to_string_lossy().into_owned()
        } else {
            format!("{rel}/{}", e.file_name().to_string_lossy())
        };
        if KEEP.contains(&r.as_str()) {
            continue;
        }
        if e.file_type()?.is_dir() {
            if KEEP.iter().any(|k| k.starts_with(&format!("{r}/"))) {
                wipe(&e.path(), &r)?;
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

fn build(root: &Path, kit: &Path) -> Result<()> {
    stage(root, kit)?;
    let obj = kit.join("source/build");
    fs::create_dir_all(&obj)?;
    fs::create_dir_all(kit.join("programs"))?;
    let env = msvc_env(root)?;
    let version = workspace_version(root)?;
    let sh = Shell::new()?;
    sh.change_dir(root);
    let c = |source: &str, out: PathBuf, what: &str, compile: &[&str], link: &[&str]| {
        build_c(
            &sh,
            &env,
            &version,
            &root.join("source"),
            &obj,
            source,
            &out,
            what,
            compile,
            link,
        )
    };
    let p = kit.join("programs");
    c(
        "hook/wccfk.c",
        p.join("wccfk.dll"),
        "the kit's hook in the game: stands in for the cabinet's hardware",
        &["/LD"],
        &[],
    )?;
    c(
        "card_table/fpr_emu.c",
        p.join("FPR_Emu.exe"),
        "the card table's stand-in",
        &[],
        &["user32.lib"],
    )?;
    c(
        "panel/wccfpanel.c",
        p.join("wccfpanel.dll"),
        "the kit's on-screen panel",
        &["/LD"],
        &[
            "d3d9.lib",
            "gdi32.lib",
            "user32.lib",
            "ole32.lib",
            "windowscodecs.lib",
        ],
    )?;
    c(
        "panel/inject.c",
        p.join("inject.exe"),
        "loads the kit's panel into the running game",
        &[],
        &[],
    )?;
    // tests only, not shipped
    c(
        "panel/fakegame.c",
        obj.join("fakegame.exe"),
        "",
        &[],
        &["d3d9.lib", "user32.lib"],
    )?;
    c(
        "hook/test_wholemsg.c",
        obj.join("test_wholemsg.exe"),
        "",
        &[],
        &[],
    )?;
    c("crashme.c", obj.join("crashme.exe"), "", &[], &[])?;

    // the toolchain is pinned by rust-toolchain.toml, the crates by Cargo.lock, the static runtime by .cargo/config.toml
    cmd!(
        sh,
        "cargo build --release --locked -p launcher --target {TARGET}"
    )
    .run()?;
    for exe in ["PLAY.exe", "SETUP.exe"] {
        fs::copy(
            root.join("target").join(TARGET).join("release").join(exe),
            kit.join(exe),
        )?;
        println!("built {}", kit.join(exe).display());
    }
    Ok(())
}

/// MSVC for 32-bit x86 (the game is 32-bit, so its hook, panel and injector must be): the environment `vcvarsall x86` makes.
fn msvc_env(root: &Path) -> Result<Vec<(String, String)>> {
    let sh = Shell::new()?;
    let pf =
        std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let vswhere = format!(r"{pf}\Microsoft Visual Studio\Installer\vswhere.exe");
    let vs = cmd!(sh, "{vswhere} -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath")
        .read()
        .context("vswhere failed")?;
    ensure!(
        !vs.trim().is_empty(),
        "no Visual Studio with the C++ tools (vswhere found none)"
    );
    let bat = root.join("target/vcvars-x86.bat");
    fs::create_dir_all(bat.parent().unwrap())?;
    fs::write(
        &bat,
        format!(
            "@call \"{}\\VC\\Auxiliary\\Build\\vcvarsall.bat\" x86 >nul\r\n@set\r\n",
            vs.trim()
        ),
    )?;
    let out = cmd!(sh, "cmd /c {bat}").read()?;
    Ok(out
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.trim_end().to_owned()))
        .collect())
}

/// The runtime linked in (/MT), so no Visual C++ runtime is needed on the player's PC; /Brepro: the same source, the same file.
/// `what` becomes the program's version information (what a file's Properties show); no `what`, no resource.
/// Kit 5.5's winmm.dll, nameless and unsigned, was quarantined as a trojan when it was unzipped (2026-10-10): `package` refuses
/// a kit whose programs lack it.
#[allow(clippy::too_many_arguments)]
fn build_c(
    sh: &Shell,
    env: &[(String, String)],
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
             VALUE \"CompanyName\", \"Forza-WCCF community\"\r\nVALUE \"FileDescription\", \"{PRODUCT} - {file}: {what}\"\r\nVALUE \"FileVersion\", \"{version}\"\r\nVALUE \"InternalName\", \"{name}\"\r\n\
             VALUE \"LegalCopyright\", \"Community-made tools for your own copy of the game; the game is not included\"\r\nVALUE \"OriginalFilename\", \"{file}\"\r\nVALUE \"ProductName\", \"{PRODUCT}\"\r\n\
             VALUE \"ProductVersion\", \"{version}\"\r\nEND\r\nEND\r\nBLOCK \"VarFileInfo\"\r\nBEGIN\r\nVALUE \"Translation\", 0x409, 1200\r\nEND\r\nEND\r\n"
        );
        let (rc_path, res_path) = (
            obj.join(format!("{name}.rc")),
            obj.join(format!("{name}.res")),
        );
        fs::write(&rc_path, rc)?;
        cmd!(sh, "rc /nologo /fo {res_path} {rc_path}")
            .envs(env.iter().cloned())
            .run()
            .with_context(|| format!("rc failed for {source}"))?;
        res.push(res_path);
    }
    let input = src.join(source);
    let fo = format!("/Fo{}\\", obj.display());
    let fe = format!("/Fe{}", out.display());
    let implib = format!("/IMPLIB:{}", obj.join(format!("{name}.lib")).display());
    let pdb = format!("/PDB:{}", obj.join(format!("{name}.pdb")).display());
    cmd!(sh, "cl /nologo /O2 /MT /W3 /Brepro /D_CRT_SECURE_NO_WARNINGS {compile...} {input} {fo} {fe} /link /Brepro {implib} {pdb} {res...} {link...}")
        .envs(env.iter().cloned())
        .run()
        .with_context(|| format!("cl failed for {source}"))?;
    println!("built {}", out.display());
    Ok(())
}

// ---- check ----

/// What `check` collects: the failed checks' names.
struct Checks {
    py: PathBuf,
    kit: PathBuf,
    failed: Vec<String>,
}

impl Checks {
    fn python(&self, args: &[&str]) -> Command {
        let mut c = Command::new(&self.py);
        c.arg("-I").args(args).current_dir(&self.kit);
        c
    }

    /// Run a check's command to its end; print its output (only the last line if `last`); a nonzero exit fails `what`.
    fn run(&mut self, what: &str, mut c: Command, last: bool) {
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

    /// A kit script `source/NAME` run as a check; `last`: show only its last output line.
    fn test(&mut self, what: &str, script: &str, last: bool) {
        let path = self.kit.join("source").join(script);
        let c = self.python(&[path.to_str().unwrap()]);
        self.run(what, c, last);
    }
}

fn check(root: &Path, kit: &Path) -> Result<()> {
    for p in PROGRAMS.iter().chain(&[
        "source/build/fakegame.exe",
        "source/build/test_wholemsg.exe",
        "source/build/crashme.exe",
    ]) {
        ensure!(
            kit.join(p).is_file(),
            "{p} is missing: run `cargo xtask build` first"
        );
    }
    stage(root, kit)?;
    let mut ck = Checks {
        py: kit.join("python/python.exe"),
        kit: kit.to_owned(),
        failed: vec![],
    };
    let code = kit.join("code");
    let code_s = code.to_str().unwrap();

    let c = ck.python(&["-c", "import pathlib, sys; [compile(p.read_bytes(), str(p), 'exec') for p in sorted(pathlib.Path(sys.argv[1]).rglob('*.py'))]; print('code: all compile (every folder)')", code_s]);
    ck.run("code compile", c, false);

    // the launchers run python\python.exe code\<script>: the kit's modules are found ONLY through python\python313._pth
    // (2026-10-11: scripts\ was renamed to code\ but the _pth still said ..\scripts - every test adds code\ to the path
    // itself, so they all passed while a real PLAY.exe could not import kit_common). Every file, every folder.
    ck.test(
        "every file in code\\ imports through python313._pth, as the launchers run them",
        "test_imports.py",
        true,
    );

    // the key driver's live file is where play.py's stop guard reads the card state, and the I/O board's outputs file is
    // beside it (hardware\io_board.py derives it): if they ever disagree, the stop guard reads "card out" with a card in
    let c = ck.python(&["-c", r"import os, play; from hardware import keyboard as k; n = os.path.normcase; assert n(play.KEYS_INPUT) == n(k.LIVE), (play.KEYS_INPUT, k.LIVE); assert n(k.OUTPUTS) == n(k.LIVE[:-len('_input.txt')] + '_outputs.txt'); assert os.path.basename(os.path.dirname(k.LIVE)) == 'data'; print('key driver files: in data\\, the same for play.py and the I/O board')"]);
    ck.run(
        "the key driver, the I/O board and play.py name the same live files in data\\",
        c,
        false,
    );

    // Kit 5.5 reused the name PANEL for wccfpanel.dll: a plain start read the dll as data\panel.txt and never went online
    let c = ck.python(&["-c", "import sys; sys.path.insert(0, sys.argv[1]); import play; assert play.PANEL.endswith('panel.txt'), play.PANEL; print('play.py: PANEL is panel.txt')", code_s]);
    ck.run("play.py PANEL", c, false);

    let c = ck.python(&[
        code.join("hardware/keyboard.py").to_str().unwrap(),
        "--selftest",
    ]);
    ck.run("key driver self-test", c, true);
    ck.test(
        "play.py ended: a closed window vs a crash",
        "test_ended.py",
        false,
    );
    let c = ck.python(&[code.join("cards/edit.py").to_str().unwrap(), "--self-test"]);
    ck.run("club card editor self-test", c, true);
    ck.test(
        "a missing card helper file must not stop a start",
        "test_card_fix.py",
        false,
    );
    ck.test(
        "the card reader: a failed save never writes in place; a bad card does not lock out",
        "test_icc_reader.py",
        false,
    );
    ck.test(
        "the seat decision: only \"no broker\" falls back to seat 1; other answers stop",
        "test_take_seat.py",
        false,
    );
    ck.test(
        "the seat lease reconnects and re-claims its seat after a break",
        "test_seat_lease.py",
        true,
    );
    ck.test(
        "the match relay: a live seat cannot be taken over by another address",
        "test_relay.py",
        true,
    );
    ck.test(
        "log rotation never stops a start when a log file is held open",
        "test_rotate_logs.py",
        false,
    );
    ck.test(
        "a missing card catalogue does not stop a start",
        "test_setup_state.py",
        true,
    );
    ck.test(
        "an older kit's scripts\\ bin\\ overlay\\ tidied, the catalogue moved to data\\cache\\",
        "test_old_layout.py",
        true,
    );
    ck.test(
        "the hook-import patcher: winmm -> wccfk, valid checksum, reversible",
        "test_patch_hook_import.py",
        false,
    );
    ck.test(
        "logs: the run before last zipped, the archive kept to its size",
        "test_log_archive.py",
        false,
    );
    ck.test(
        "a transferred card: listed as such, never played again",
        "test_wallet_transferred.py",
        false,
    );

    // The checks that wait on their own servers (the seat desk, SEND LOGS) run side by side, beside the panel's
    // self-tests below (each on its own ports); their last line and exit code are read once they end.
    let side: Vec<_> = [
        (
            "the seat desk: never a seat that is in the game",
            "test_seat_desk_game.py",
        ),
        (
            "the projector box: held until a player is in",
            "test_hold_projector.py",
        ),
        (
            "SEND LOGS: packed, sent, kept - and refused when it must be",
            "test_send_logs.py",
        ),
    ]
    .into_iter()
    .map(|(what, script)| {
        let mut c = ck.python(&[kit.join("source").join(script).to_str().unwrap()]);
        (what, std::thread::spawn(move || c.output()))
    })
    .collect();

    ck.test(
        "English pictures: added to the archive, taken back out byte for byte",
        "test_english_pictures.py",
        false,
    );
    ck.test(
        "the Italian tables: keys, font and codes",
        "test_italian_tables.py",
        false,
    );
    ck.test(
        "the english_backup list: entries of older kits still read",
        "test_backup_manifest.py",
        false,
    );

    ck.run(
        "whole messages: a batch in pieces waits for its rest, bytes that are no batch are skipped",
        Command::new(kit.join("source/build/test_wholemsg.exe")),
        false,
    );
    ck.test(
        "a crash leaves its memory snapshots (lean + full) in data\\dumps",
        "test_crash_dump.py",
        false,
    );

    panel_selftests(&mut ck)?;

    for (what, t) in side {
        match t.join().expect("check thread") {
            Ok(o) if o.status.success() => ck.report(what, &o, true),
            Ok(o) => {
                let all = [o.stdout.as_slice(), o.stderr.as_slice()].concat();
                let text = String::from_utf8_lossy(&all).into_owned();
                let lines: Vec<&str> = text.lines().collect();
                for l in &lines[lines.len().saturating_sub(20)..] {
                    println!("  {l}");
                }
                ck.failed.push(what.to_owned());
            }
            Err(e) => ck.fail(what, &e.to_string()),
        }
    }

    // the Rust workspace, xtask included: format, lint, tests - the toolchain from rust-toolchain.toml, read in the root
    let cargo = |what: &str, args: &[&str], failed: &mut Vec<String>| {
        if !Command::new("cargo")
            .args(args)
            .current_dir(root)
            .status()
            .is_ok_and(|s| s.success())
        {
            failed.push(what.to_owned());
        }
    };
    cargo(
        "workspace: cargo fmt",
        &["fmt", "--all", "--check"],
        &mut ck.failed,
    );
    cargo(
        "workspace: cargo clippy",
        &[
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--target",
            TARGET,
            "--",
            "-D",
            "warnings",
        ],
        &mut ck.failed,
    );
    cargo(
        "workspace: cargo test",
        &["test", "--locked", "--workspace", "--target", TARGET],
        &mut ck.failed,
    );

    scrub(root, &mut ck.failed)?;

    ensure!(ck.failed.is_empty(), "FAILED: {}", ck.failed.join(", "));
    println!("all checks passed");
    Ok(())
}

/// The panel's money, relay and dealt-card self-tests: wccfpanel.dll loaded by fakegame.exe in a scratch folder; the relay test plays
/// two seats through code\server\relay.py on 127.0.0.1:20941 - the C side and the Python side of the match relay checked against each other.
/// The frame-wait test (WCCFPANEL_LIMITERTEST) measures milliseconds and stays out: a busy machine fails it.
fn panel_selftests(ck: &mut Checks) -> Result<()> {
    let what = "panel self-tests";
    let kit = ck.kit.clone();
    let t = std::env::temp_dir().join(format!("wccfpanel-check-{}", std::process::id()));
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(t.join("data"))?;
    fs::copy(
        kit.join("source/build/fakegame.exe"),
        t.join("fakegame.exe"),
    )?;
    let mut relay: Child = ck
        .python(&[kit.join("code/server/relay.py").to_str().unwrap(), "60"])
        .env("WCCF_RELAY_PORT", "20941")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::sleep(Duration::from_secs(1));
    let run = Command::new(t.join("fakegame.exe"))
        .current_dir(&t)
        .arg(kit.join("programs/wccfpanel.dll"))
        .arg("wait:12000")
        .env("WCCFPANEL_ROLE", "seat1") // the cabinet's code path (the panel would take a folder named "extracted" for the projector)
        .env("WCCFPANEL_DRYKEYS", "1") // keys logged, never pressed
        .env("WCCFPANEL_FULLSCREEN", "0") // never maximized
        .env("WCCF_DATA", t.join("data"))
        .env("WCCF_KEYS", t.join("data/keys.txt"))
        .env("WCCFPANEL_MONEYTEST", "1")
        .env("WCCFPANEL_RELAYTEST", "1")
        .env("WCCFPANEL_DEALTEST", "1") // the dealt card's odds
        .env("WCCFPANEL_SETTEST", "1") // a panel.txt save keeps auto_logs=0
        .env("WCCF_RELAY_PORT", "20941")
        .env("WCCF_SKIN", kit.join("art/skin.tex")) // the panel's art (play.py passes it)
        .status();
    let _ = relay.kill();
    let _ = relay.wait();
    let log = String::from_utf8_lossy(&fs::read(t.join("wccfpanel.log")).unwrap_or_default())
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
    let done = money && log.iter().any(|l| l.contains("relay test: done"));
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

/// Every tracked text file (not python/, not binaries): no line names the publisher except the class-C lines of
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
    let own = cmd!(
        sh,
        "git grep -n -I -i -e {name} -e {kana} -- . :!python :!import/1718/kit/python"
    )
    .ignore_status()
    .read()?;
    bad.extend(
        own.lines()
            .filter(|l| {
                !allow.contains(mask(l.splitn(3, ':').nth(2).unwrap_or("").trim()).as_str())
            })
            .map(str::to_owned),
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
        let hits = cmd!(
            sh,
            "git grep -n -I -i -E {pats...} -- . :!python :!import/1718/kit/python"
        )
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

// ---- package ----

fn package(root: &Path, kit: &Path, name: &str) -> Result<()> {
    let sh = Shell::new()?;
    sh.change_dir(root);
    let mapped: Vec<&str> = MAP.iter().map(|m| m.1).collect();
    let dirty = cmd!(sh, "git status --porcelain -- {mapped...}").read()?;
    ensure!(
        dirty.trim().is_empty(),
        "uncommitted changes in files the kit ships; commit them first:\n{dirty}"
    );
    stage(root, kit)?;
    for p in PROGRAMS {
        ensure!(
            kit.join(p).is_file(),
            "{p} is missing: run `cargo xtask build` first"
        );
    }

    // every program must carry its version information: a nameless unsigned program is what Windows Defender's
    // machine learning flagged (Kit 5.5's first SETUP.exe, then its winmm.dll)
    let list = PROGRAMS
        .iter()
        .map(|p| format!("'{}'", kit.join(p).display()))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "{list} | ForEach-Object {{ $v = (Get-Item $_).VersionInfo; \"$($v.ProductName)|$($v.FileVersion)|$($v.FileDescription)\" }}"
    );
    let info = cmd!(sh, "powershell -NoProfile -Command {script}").read()?;
    for (p, line) in PROGRAMS.iter().zip(info.lines()) {
        let f: Vec<&str> = line.trim().splitn(3, '|').collect();
        ensure!(
            f.len() == 3 && f[0] == PRODUCT && !f[1].is_empty(),
            "{p} has no version information - see xtask build"
        );
        println!("{p} : {} {} - {}", f[0], f[1], f[2]);
    }

    // for SETUP.exe's update: version.txt, which kit this is, and files.txt, every file this kit ships - an update removes
    // what the old kit's list has and the new one's does not
    let version = name
        .strip_prefix(TOP)
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
    zip.add_directory(format!("{TOP}/"), opts)?;
    for d in &dirs {
        zip.add_directory(format!("{TOP}/{d}/"), opts)?;
    }
    for f in &shipped {
        zip.start_file(format!("{TOP}/{f}"), opts)?;
        std::io::copy(&mut fs::File::open(kit.join(f))?, &mut zip)?;
    }
    zip.finish()?;

    // read the ZIP back: every program must be in it, not only in the folder it was made from
    // (a kit without them cannot start); python.exe comes from git, the rest from the build
    let mut ar = zip::ZipArchive::new(fs::File::open(&zip_path)?)?;
    let mut entries = HashSet::new();
    for i in 0..ar.len() {
        entries.insert(ar.by_index(i)?.name()?.into_owned()); // each entry's header reads
    }
    let need = PROGRAMS
        .iter()
        .copied()
        .chain(["python/python.exe"])
        .chain((!version.is_empty()).then_some("version.txt"));
    let missing: Vec<_> = need
        .filter(|n| !entries.contains(&format!("{TOP}/{n}")))
        .collect();
    ensure!(missing.is_empty(), "the ZIP lacks: {}", missing.join(", "));
    println!(
        "the ZIP has every program: {}, python/python.exe",
        PROGRAMS.join(", ")
    );

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
    fn mask_hides_the_name_in_any_case() {
        let [p, k] = publisher();
        assert_eq!(
            mask(&format!("a {} b {}{}", p.to_uppercase(), k, p)),
            "a <P> b <P><P>"
        );
        assert_eq!(mask("nothing here"), "nothing here");
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
