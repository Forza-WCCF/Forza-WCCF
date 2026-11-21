// ---- the installer ----
use super::*;

pub(crate) const INSTALLER: &str = "wccf-installer"; // the package; its exe is Forza-WCCF-Installer.exe
pub(crate) const INSTALLER_EXE: &str = "Forza-WCCF-Installer.exe";
pub(crate) const LICENSES: &str = "crates/installer/THIRD-PARTY-LICENSES.json";

/// `profile`: "release" (the ZIP's, built with launcher and forza-server).
pub(crate) fn installer_exe(root: &Path, profile: &str) -> PathBuf {
    target_dir(root)
        .join(TARGET)
        .join(profile)
        .join(INSTALLER_EXE)
}

/// The installer's own checks (they were crates/installer/scripts/*.ps1): the license notices, then a synthetic headless run
/// of the release build. `work` says which of the two a change can break.
pub(crate) fn installer_checks(
    root: &Path,
    stage_root: &Path,
    work: InstallerWork,
    failed: &mut Vec<String>,
) {
    for (what, r) in [
        (
            "installer licenses",
            work.licenses.then(|| check_licenses(root)),
        ),
        (
            "installer smoke",
            work.smoke.then(|| smoke(root, stage_root)),
        ),
    ] {
        if let Some(Err(e)) = r {
            eprintln!("{what}: {e:#}");
            failed.push(what.to_owned());
        }
    }
}

/// Which of the installer's checks the changed files ([`changed_files`]) can break: `licenses` is the dependency tree
/// Cargo.lock and the crate's manifest resolve, `smoke` the exe the crate, the packaging code and the files it embeds
/// make. Its unit tests still run with the crates it depends on (`affected`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InstallerWork {
    pub(crate) licenses: bool,
    pub(crate) smoke: bool,
}

impl InstallerWork {
    /// Both, when no file list narrowed them down (the whole workspace is in scope).
    pub(crate) const ALL: Self = Self {
        licenses: true,
        smoke: true,
    };

    pub(crate) fn any(self) -> bool {
        self.licenses || self.smoke
    }
}

/// The work of `files`; `everything` is the whole workspace's scope (`pkgs` is `None`), where the files do not narrow it.
pub(crate) fn installer_work(files: Option<&str>, everything: bool) -> InstallerWork {
    let Some(files) = files.filter(|_| !everything) else {
        return InstallerWork::ALL;
    };
    let mut work = InstallerWork {
        licenses: false,
        smoke: false,
    };
    for f in files.lines().map(str::to_lowercase) {
        work.licenses |= f == "cargo.lock"
            || f == "crates/installer/cargo.toml"
            || f.eq_ignore_ascii_case(LICENSES);
        work.smoke |= [
            "crates/installer/",
            // the version string the smoke asserts
            "crates/launcher/version_rc.rs",
            "xtask/",
            "assets/fonts/",
            "assets/logo/",
        ]
        .iter()
        .any(|p| f.starts_with(p));
    }
    work
}

/// THIRD-PARTY-LICENSES.json holds a notice with text for every normal/build dependency of the installer on Windows.
pub(crate) fn check_licenses(root: &Path) -> Result<()> {
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
/// The exe is the one that `build_all` makes in this run's profile (`quick`'s `ci` under a wrapper, else `release`).
pub(crate) fn smoke(root: &Path, stage_root: &Path) -> Result<()> {
    let exe = installer_built(root)?;
    smoke_run(root, stage_root, &exe)
}

/// The installer that `build_all` built in this run's profile; returns its path.
pub(crate) fn installer_built(root: &Path) -> Result<PathBuf> {
    let exe = installer_exe(root, profile());
    ensure!(exe.is_file(), "{} is missing", exe.display());
    Ok(exe)
}

/// The headless run of a built installer.
pub(crate) fn smoke_run(root: &Path, stage_root: &Path, exe: &Path) -> Result<()> {
    let dir = stage_root.join(format!("installer-smoke-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let r = smoke_in(root, exe, &dir);
    let _ = fs::remove_dir_all(&dir);
    r?;
    println!("Headless smoke check passed; no game was launched.");
    Ok(())
}

pub(crate) fn smoke_in(root: &Path, exe: &Path, dir: &Path) -> Result<()> {
    // runs the installer; its exit code must be `expected`; returns stdout
    let run = |args: &[&Path], expected: i32| -> Result<String> {
        let o = Command::new(exe).args(args).output()?;
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
    // the exe may be a package build from an earlier run (stamped with the commit id) or this one's
    let got = run(&[p("--version")], 0)?;
    let want = |v: &str| format!("Forza-WCCF Installer {v}");
    ensure!(
        [version(root), build_version(root)]
            .iter()
            .any(|v| got.trim() == want(v)),
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

/// The bytes' SHA-256 as lowercase hex: the spelling SHA256SUMS.txt, files.sha256 and the release's sums use.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for b in Sha256::digest(bytes) {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// `dist/`: the installer exe, its notices, `NAME.zip` of exactly those files, and SHA256SUMS.txt for the exe and the ZIP.
/// NAME defaults to `Forza-WCCF-Installer-<version>-windows-x64`. Run `cargo xtask check` first (it builds the exe).
pub(crate) fn package_installer(root: &Path, name: Option<String>) -> Result<()> {
    check_licenses(root)?;
    let mut c = rust_shipped(root);
    c.args(["--release", "--locked", "-p", INSTALLER, "--target", TARGET]);
    run_ok(c, "cargo build --release -p wccf-installer")?;
    let exe = installer_exe(root, "release");
    ensure!(exe.is_file(), "{} is missing", exe.display());
    ensure!(
        !has_user_path(&fs::read(&exe)?),
        "{} holds a user folder path - see rust_shipped",
        exe.display()
    );
    let name =
        name.unwrap_or_else(|| format!("Forza-WCCF-Installer-{}-windows-x64", build_version(root)));
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
    // the release asset carries the version too, like the ZIP: Forza-WCCF-Installer-<version>-windows-x64.exe
    let exe_name = format!("{name}.exe");
    let payload: Vec<(String, PathBuf)> = std::iter::once((exe_name.clone(), exe))
        .chain(docs.iter().map(|d| (d.to_string(), src.join(d))))
        .collect();
    let zip_name = format!("{name}.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(dist.join(&zip_name))?);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip_time(root)?);
    for (n, path) in &payload {
        fs::copy(path, dist.join(n))?;
        zip.start_file(n, opts)?;
        std::io::copy(&mut fs::File::open(path)?, &mut zip)?;
    }
    zip.finish()?;
    let mut sums = String::new();
    for n in [&exe_name, &zip_name] {
        let hash = sha256_hex(&fs::read(dist.join(n))?);
        let _ = writeln!(sums, "{hash}  {n}");
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
