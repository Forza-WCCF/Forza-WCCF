// ---- package ----
use super::*;

/// `is_1011`: the 2010-11 kit, else the 2017-18 kit. Both hold the programs `cargo xtask build` made, and their version
/// information is checked; both ZIPs leave out `source/build` (the 2017-18 guard tests live there; its guard `bin/wccfnet.dll` ships).
/// Players read these files: the product is Forza-WCCF, never a "kit" (the whole word, any case).
/// The staged kits' player docs and the installer's say no "kit" (what `package` checks, without the ZIP).
pub(crate) fn shipped_docs_no_kit(root: &Path, kits: &[&Path]) -> Result<()> {
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

pub(crate) fn no_kit_word(files: &[PathBuf]) -> Result<()> {
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
pub(crate) fn source_txt(version: &str, commit: &str) -> String {
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

pub(crate) fn package(root: &Path, kit: &Path, name: &str, is_1011: bool) -> Result<()> {
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
pub(crate) const FREEZE_FILE: &str = "editions/2017-18/fpr_emu.sha256";
/// What FPR_Emu.exe is built from besides Cargo.lock and the toolchain: its crate, the common crate and its icon.
/// shortcut: `crates/launcher/version_rc.rs` (shared with every program) is not here, it changed in 0.6.2 without
/// changing this exe; add it back when its output for fpr_emu changes.
pub(crate) const FREEZE_SRC: &[&str] = &[
    "native/programs",
    "native/common",
    "assets/logo/forza-wccf-small.ico",
];

/// `<sha256> <tag> <commit> [pin]`: `pin` keeps the release's file even when the sources changed since `commit`.
pub(crate) fn parse_freeze(text: &str) -> Result<([&str; 3], bool)> {
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
pub(crate) fn keep_frozen(pinned: bool, changed: &str) -> bool {
    pinned || changed.trim().is_empty()
}

/// Replace the freshly built `bin/FPR_Emu.exe` of `kit` with the release's file when `FREEZE_SRC` is unchanged since the
/// recorded commit. `Ok` also when the sources changed (it says so and the fresh build stays); `Err` when the release
/// file cannot be had or does not match the hash (offline, a moved release): the caller warns and ships the fresh build.
pub(crate) fn freeze_fpr_emu(root: &Path, kit: &Path) -> Result<()> {
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
pub(crate) fn resource(file: &[u8], ty: u16) -> Result<&[u8]> {
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
pub(crate) fn version_strings(file: &[u8]) -> Result<HashMap<String, String>> {
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
pub(crate) fn walk(
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
