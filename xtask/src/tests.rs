use super::*;
use proptest::prelude::*;

use super::build::{built_stamp_text, other_profile, sources_changed};
use super::check::{
    affected, clippy, hook_test, package_dir, required_programs, target_scopes, test_commands,
    test_filter, tests,
};
use super::installer::LICENSES;
use super::package::{keep_frozen, parse_freeze, source_txt};
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
