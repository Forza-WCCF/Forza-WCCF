use super::*;

#[test]
fn each_disc_maps_back_to_its_edition() {
    assert_eq!(edition_of("XXXX"), None);
    // the install roots players already have on disk
    assert_eq!(Edition::E1011.folder(), "2010-2011");
    assert_eq!(Edition::E1718.folder(), "2017-2018");
    for d in &disc::DISCS {
        assert_eq!(
            edition_of(d.edition.game_id()),
            Some(d.edition),
            "{}",
            d.name
        );
    }
}

/// A failed step's detail is its `FAIL` line, not an ok line that only mentions the word.
#[test]
fn a_failed_kit_step_reports_its_fail_line() {
    use forza::Line::Raw;
    let detail = |fail: bool| {
        let e = kit_step(&|_| {}, |say| {
            say("  ok    ", &Raw("no FAIL".into()));
            if fail {
                say("  FAIL  ", &Raw("x".into()));
            }
            1
        })
        .unwrap_err();
        match e.downcast_ref::<E>() {
            Some(E::Script { detail }) => detail.clone(),
            other => panic!("{other:?}"),
        }
    };
    assert!(detail(true).ends_with('x'), "{}", detail(true));
    assert_eq!(detail(false), "exit 1");
}

#[cfg(windows)]
#[test]
fn the_shortcut_points_at_the_launcher() {
    let t = std::env::temp_dir().join(format!("install-lnk-test-{}", std::process::id()));
    fs::create_dir_all(&t).unwrap();
    let exe = t.join("Launcher.exe");
    fs::write(&exe, b"").unwrap();
    let link = t.join("Forza-WCCF 2017-18.lnk");
    lnk::write(&link, &exe, &t).unwrap();
    let (target, dir) = lnk::read(&link).unwrap();
    let same = |a: &Path, b: &Path| fs::canonicalize(a).unwrap() == fs::canonicalize(b).unwrap(); // 8.3 names expand
    assert!(same(&target, &exe) && same(&dir, &t), "{target:?} {dir:?}");
    fs::remove_dir_all(&t).unwrap();
}

#[test]
fn a_picked_folder_gets_its_own_forza_wccf_folder() {
    let t = std::env::temp_dir().join(format!("install-folder-test-{}", std::process::id()));
    fs::create_dir_all(t.join("old/2017-2018")).unwrap();
    assert_eq!(install_folder(&t), t.join("Forza-WCCF")); // a plain folder (or a drive root) gets its own
    assert_eq!(install_folder(&t.join("forza-wccf")), t.join("forza-wccf"));
    assert_eq!(install_folder(&t.join("old")), t.join("old")); // an existing install stays where it is
    fs::remove_dir_all(&t).unwrap();
}

/// A cancelled or failed extract takes its half tree with it: nothing is left at `game.partial` to fill the drive
/// until the next install (the next run removes it, but only if one comes).
#[test]
fn a_cancelled_extract_leaves_no_partial() {
    let t = std::env::temp_dir().join(format!("install-partial-test-{}", std::process::id()));
    let partial = t.join("game.partial");
    let done = partial_tree(&partial, || {
        assert!(partial.is_dir());
        fs::write(partial.join("client_Release.exe"), b"half")?;
        Err(E::Cancelled.into())
    });
    assert!(
        done.is_err() && !partial.exists(),
        "{partial:?} is still there"
    );
    fs::remove_dir_all(&t).unwrap();
}

/// A name from the disc stays inside the destination: no separator, no drive or stream colon, no dot entry, and
/// nothing Windows rewrites.
#[test]
fn a_name_from_the_disc_stays_inside_the_destination() {
    for good in [
        "data",
        "client_Release.exe",
        "WCCF 2017-2018",
        "local",
        "$MFT",
        // not the devices these begin with
        "CONSOLE",
        "COM0",
        "COM10",
        "LPT0",
    ] {
        assert!(safe(good), "{good}");
    }
    for bad in ["", ".", "..", "a/b", r"a\b", "C:", "x:y"] {
        assert!(!safe(bad), "{bad}");
    }
    // Windows drops a trailing space or dot and opens the device, never the file
    for bad in [
        "data ", "data.", ".. ", "CON", "con", "nul.txt", "CON .txt", "PRN.log", "aux.", "COM1",
        "com9.log", "LPT1", "lpt9.dat",
    ] {
        assert!(!safe(bad), "{bad:?}");
    }
}

/// The root's own metadata is skipped; a `$` name below the root is game data and stays.
#[test]
fn the_root_metadata_is_skipped() {
    let root = Path::new("");
    let below = Path::new("data");
    for name in ["$MFT", "$Bitmap", "$Extend", "System Volume Information"] {
        assert!(!keep(root, name), "{name}");
        assert!(keep(below, name), "{name}");
    }
    for name in ["data", "client_Release.exe"] {
        assert!(keep(root, name), "{name}");
    }
    assert!(!keep(root, ".."), "a bad name is no game data either");
    assert!(!keep(below, ".."), "nor below the root");
    assert!(!keep(below, r"a\b"), "nor below the root");
}

#[test]
fn pictures_step_runs_ensure_and_survives_failure() {
    let dir = std::env::temp_dir().join(format!("pictures-step-test-{}", std::process::id()));
    let game = dir.join("game");
    fs::create_dir_all(&game).unwrap();
    let lines = std::cell::RefCell::new(Vec::new());
    let emit = |e: Event| {
        if let Event::Missed(l) = e {
            lines.borrow_mut().push(l(forza_ui::Lang::En));
        }
    };
    // no game files: the step reports and returns instead of failing the install
    pictures(&dir, Edition::E1718, &game.to_string_lossy(), &emit);
    assert!(
        lines
            .borrow()
            .iter()
            .any(|l| l.contains("Card pictures failed"))
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The lines a player must act on read in each language, none of them a bare key (a typo or a missing language).
#[test]
fn the_lines_a_player_must_act_on_are_in_three_languages() {
    let root = std::env::temp_dir().join(format!("player-lines-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let game = version_paths(&root, "SDDY").unwrap().1;
    fs::create_dir_all(game.join("local")).unwrap();
    fs::write(game.join("client_Release.exe"), b"").unwrap();
    fs::write(game.join("local/SDDY_Table.dat"), b"SDDY").unwrap();
    let plan = Plan {
        dest: root.clone(),
        iso: None,
        key: None,
        game_id: Some("SDDY"),
        verify: true,
        setup: true,
        english: false,
        shortcuts: true,
        kit: None,
        latest: false,
    };
    let empty = root.join("no-game");
    fs::create_dir_all(&empty).unwrap();
    let (mut extract_only, mut no_pictures) = (vec![], vec![]);
    for lang in forza_ui::Lang::ALL {
        let last = std::cell::RefCell::new(String::new());
        run(&plan, &AtomicBool::new(false), lang, &|e| {
            if let Event::Log(l) = e {
                *last.borrow_mut() = l;
            }
        })
        .unwrap();
        extract_only.push(last.into_inner());
        let missed = std::cell::RefCell::new(String::new());
        pictures(&root, Edition::E1718, &empty.to_string_lossy(), &|e| {
            if let Event::Missed(l) = e {
                *missed.borrow_mut() = l(lang);
            }
        });
        no_pictures.push(missed.into_inner());
    }
    let three = |key: &str, args: &[(&str, &str)]| -> Vec<String> {
        forza_ui::Lang::ALL
            .into_iter()
            .map(|l| forza_ui::text(l, key, args))
            .collect()
    };
    assert!(
        extract_only[0].contains("installer again"),
        "the next step: {extract_only:?}"
    );
    // the Done page's note, which the player sees (the log sits in a closed fold), names the next step too
    let note = forza_ui::text(forza_ui::Lang::En, "installer.extract_only_note", &[]);
    assert!(note.contains("installer again"), "{note}");
    for (key, texts) in [
        ("installer.log_extract_only", extract_only),
        ("installer.log_pictures_no_game", no_pictures),
        (
            "launcher.run_pictures_failed",
            three("launcher.run_pictures_failed", &[("error", "E")]),
        ),
        (
            "installer.log_stuck",
            three("installer.log_stuck", &[("path", "P")]),
        ),
        (
            "installer.log_shortcut_failed",
            three("installer.log_shortcut_failed", &[]),
        ),
    ] {
        assert!(
            texts.iter().all(|t| !t.is_empty() && !t.contains(key)),
            "{key}: {texts:?}"
        );
        assert!(
            texts[1] != texts[0] && texts[2] != texts[0] && texts[2] != texts[1],
            "{key}: {texts:?}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

/// An extracted 2017-18 game (exe, table, a file beside it) under a fresh root, and a plan that reuses it.
fn sddy_game(name: &str) -> (PathBuf, Plan, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let (edition_root, game) = version_paths(&root, "SDDY").unwrap();
    fs::create_dir_all(game.join("local")).unwrap();
    fs::write(game.join("client_Release.exe"), b"unchanged").unwrap();
    fs::write(game.join("local/SDDY_Table.dat"), b"SDDY").unwrap();
    fs::write(
        edition_root.join(".probe-a"),
        b"do not touch on extraction-only installs",
    )
    .unwrap();
    let plan = Plan {
        dest: root.clone(),
        iso: None,
        key: None,
        game_id: Some("SDDY"),
        verify: true,
        setup: true,
        english: false,
        shortcuts: true,
        kit: None,
        latest: false,
    };
    (root, plan, edition_root, game)
}

/// A 2010-11 game beside it, and a save in each edition's tree. Returns its edition root and game folder.
fn add_sbwg(root: &Path) -> (PathBuf, PathBuf) {
    let (old_root, old_game) = version_paths(root, "SBWG").unwrap();
    fs::create_dir_all(old_game.join("local")).unwrap();
    fs::write(
        old_game.join("client_Release.exe"),
        b"old edition unchanged",
    )
    .unwrap();
    fs::write(old_game.join("local/SBWG_Table.dat"), b"SBWG").unwrap();
    let edition_root = version_paths(root, "SDDY").unwrap().0;
    for (dir, bytes) in [
        (&old_root, b"old save".as_slice()),
        (&edition_root, b"new save".as_slice()),
    ] {
        fs::create_dir_all(dir.join("data/save")).unwrap();
        fs::write(dir.join("data/save/club.bin"), bytes).unwrap();
    }
    (old_root, old_game)
}

/// A 2010-11 kit (never run: it is refused or ignored first).
fn sbwg_kit(root: &Path) -> PathBuf {
    let kit = root.join("kit.zip");
    let mut zip = zip::ZipWriter::new(File::create(&kit).unwrap());
    for n in ["Launcher.exe", "programs/wccfk.dll"] {
        zip.start_file(n, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"not executed").unwrap();
    }
    zip.finish().unwrap();
    kit
}

#[test]
fn no_kit_install_runs_with_setup_on() {
    let (root, plan, ..) = sddy_game("no-kit-test");
    let cancel = AtomicBool::new(false);
    run(&plan, &cancel, forza_ui::Lang::En, &|_| {}).unwrap(); // no kit, even with setup's default true
    assert_eq!(GAME_DIR, "game");
    assert!(game_present(&root, "SDDY"));
    assert_eq!(installed_game_id(&root), Some("SDDY"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn two_editions_need_game() {
    let (root, mut plan, edition_root, _) = sddy_game("two-editions-test");
    let (old_root, _) = add_sbwg(&root);
    assert_ne!(old_root, edition_root);
    assert!(game_present(&root, "SBWG"));
    assert_eq!(installed_game_id(&root), None); // never choose an edition silently
    plan.game_id = None;
    let cancel = AtomicBool::new(false);
    assert!(
        run(&plan, &cancel, forza_ui::Lang::En, &|_| {})
            .unwrap_err()
            .to_string()
            .contains("--game")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reuse_keeps_each_editions_files() {
    let (root, mut plan, edition_root, _) = sddy_game("reuse-both-test");
    let (old_root, old_game) = add_sbwg(&root);
    let cancel = AtomicBool::new(false);
    plan.game_id = Some("SBWG");
    run(&plan, &cancel, forza_ui::Lang::En, &|_| {}).unwrap();
    plan.game_id = Some("SDDY");
    run(&plan, &cancel, forza_ui::Lang::En, &|_| {}).unwrap();
    assert_eq!(
        fs::read(old_game.join("client_Release.exe")).unwrap(),
        b"old edition unchanged"
    );
    assert_eq!(
        fs::read(old_root.join("data/save/club.bin")).unwrap(),
        b"old save"
    );
    assert_eq!(
        fs::read(edition_root.join("data/save/club.bin")).unwrap(),
        b"new save"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn kit_of_other_edition_is_refused() {
    let (root, mut plan, edition_root, _) = sddy_game("other-kit-test");
    plan.kit = Some(sbwg_kit(&root));
    let cancel = AtomicBool::new(false);
    assert!(
        run(&plan, &cancel, forza_ui::Lang::En, &|_| {})
            .unwrap_err()
            .to_string()
            .contains("is for 2010–11")
    );
    assert!(!edition_root.join("scripts").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn no_setup_ignores_kit_and_english_needs_setup() {
    let (root, mut plan, ..) = sddy_game("no-setup-test");
    plan.kit = Some(sbwg_kit(&root));
    plan.setup = false;
    let cancel = AtomicBool::new(false);
    run(&plan, &cancel, forza_ui::Lang::En, &|_| {}).unwrap(); // --no-setup ignores a supplied kit
    plan.english = true;
    assert!(run(&plan, &cancel, forza_ui::Lang::En, &|_| {}).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_never_replaces_a_game() {
    let (root, mut plan, edition_root, game) = sddy_game("no-replace-test");
    plan.kit = Some(sbwg_kit(&root));
    plan.setup = false;
    plan.iso = Some(root.join("not-read.iso"));
    plan.key = Some(root.join("not-read.key"));
    let cancel = AtomicBool::new(false);
    assert!(
        run(&plan, &cancel, forza_ui::Lang::En, &|_| {})
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    assert!(!edition_root.join(format!("{GAME_DIR}.partial")).exists());
    assert_eq!(
        fs::read(game.join("client_Release.exe")).unwrap(),
        b"unchanged"
    );
    assert_eq!(
        fs::read(edition_root.join(".probe-a")).unwrap(),
        b"do not touch on extraction-only installs"
    );
    fs::remove_dir_all(root).unwrap();
}

/// The old flat install remains usable; installing the other edition gets its own root.
#[test]
fn legacy_flat_install_is_reused() {
    let (root, mut plan, edition_root, game) = sddy_game("legacy-flat-test");
    let (old_root, _) = add_sbwg(&root);
    let legacy = root.join(LEGACY_GAME_DIR);
    fs::rename(&game, &legacy).unwrap();
    fs::remove_dir_all(&edition_root).unwrap();
    assert_eq!(
        version_paths(&root, "SDDY").unwrap(),
        (root.clone(), legacy.clone())
    );
    assert!(game_present(&root, "SDDY"));
    plan.setup = false;
    let cancel = AtomicBool::new(false);
    run(&plan, &cancel, forza_ui::Lang::En, &|_| {}).unwrap();
    assert_eq!(version_paths(&root, "SBWG").unwrap().0, old_root);
    assert_eq!(
        fs::read(legacy.join("client_Release.exe")).unwrap(),
        b"unchanged"
    );
    fs::remove_dir_all(root).unwrap();
}

/// A zip whose every file holds its own name as bytes.
fn kit_zip(dir: &Path, name: &str, files: &[&str]) -> PathBuf {
    let path = dir.join(name);
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    for n in files {
        zip.start_file(*n, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(n.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    path
}

/// Every file under `dir` with its bytes, by path relative to `dir`.
fn tree(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.strip_prefix(dir).unwrap().into(), fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// Both editions side by side, each with its game files and the player's save; the 2017-18 root returned.
fn two_editions(root: &Path) -> PathBuf {
    for (id, save) in [("SBWG", "2010-11 save"), ("SDDY", "2017-18 save")] {
        let (ed, game) = version_paths(root, id).unwrap();
        fs::create_dir_all(game.join("local")).unwrap();
        fs::write(game.join("client_Release.exe"), id).unwrap();
        fs::write(game.join(format!("local/{id}_Table.dat")), id).unwrap();
        fs::create_dir_all(ed.join("data/save")).unwrap();
        fs::write(ed.join("data/save/club.bin"), save).unwrap();
    }
    version_paths(root, "SDDY").unwrap().0
}

/// The package goes over the 2017-18 install: its top folder stripped, the player's data\ and the 2010-11 tree
/// untouched (#302).
#[test]
fn unpack_kit_keeps_data_and_the_other_edition() {
    let root = std::env::temp_dir().join(format!("unpack-kit-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dest = two_editions(&root);
    let other = version_paths(&root, "SBWG").unwrap().0;
    let before = tree(&other);
    let kit = kit_zip(
        &root,
        "k.zip",
        &[
            "Forza-WCCF-2017-18/Launcher.exe",
            "Forza-WCCF-2017-18/bin/x.dll",
            "Forza-WCCF-2017-18/data/save/club.bin",
        ],
    );
    unpack_kit(&kit, &dest, forza_ui::Lang::En, &|_| {}).unwrap();
    assert_eq!(
        fs::read(dest.join("Launcher.exe")).unwrap(),
        b"Forza-WCCF-2017-18/Launcher.exe"
    );
    assert!(dest.join("bin/x.dll").is_file() && !dest.join("Forza-WCCF-2017-18").exists());
    assert_eq!(
        fs::read(dest.join("data/save/club.bin")).unwrap(),
        b"2017-18 save"
    );
    assert_eq!(tree(&other), before);
    fs::remove_dir_all(root).unwrap();
}

/// A package whose files.txt lists a data\ file applies over an install with data: the file is unpacked for the
/// check, the player's copy kept.
#[test]
fn a_listed_data_file_applies_over_an_install_with_data() {
    let root = std::env::temp_dir().join(format!("unpack-listed-data-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dest = two_editions(&root);
    let kit = root.join("k.zip");
    let mut zip = zip::ZipWriter::new(File::create(&kit).unwrap());
    for (n, b) in [
        ("Launcher.exe", "new"),
        ("bin/x.dll", "new"),
        ("data/save/club.bin", "package"),
        (
            "files.txt",
            "Launcher.exe\r\nbin\\x.dll\r\ndata\\save\\club.bin\r\nfiles.txt\r\n",
        ),
    ] {
        zip.start_file(n, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    unpack_kit(&kit, &dest, forza_ui::Lang::En, &|_| {}).unwrap();
    assert_eq!(fs::read(dest.join("Launcher.exe")).unwrap(), b"new");
    assert_eq!(
        fs::read(dest.join("data/save/club.bin")).unwrap(),
        b"2017-18 save"
    );
    assert!(!dest.join("data/update").exists());
    fs::remove_dir_all(root).unwrap();
}

/// A package entry under `Data/` (other capitals) is the player's data\ too: kept, never written (#280).
#[test]
fn a_data_folder_in_other_capitals_is_kept() {
    let root = std::env::temp_dir().join(format!("unpack-data-case-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dest = root.join("2017-2018");
    fs::create_dir_all(dest.join("data/save")).unwrap();
    fs::write(dest.join("data/save/club.bin"), b"player").unwrap();
    let kit = kit_zip(
        &root,
        "k.zip",
        &[
            "Launcher.exe",
            "bin/x.dll",
            "Data/save/club.bin",
            "DATA/keys.txt",
        ],
    );
    unpack_kit(&kit, &dest, forza_ui::Lang::En, &|_| {}).unwrap();
    // one folder on Windows, two elsewhere: either way no package byte lands in it
    for p in ["data/save/club.bin", "Data/save/club.bin"] {
        let p = dest.join(p);
        assert!(!p.exists() || fs::read(&p).unwrap() == b"player", "{p:?}");
    }
    assert!(!dest.join("DATA/keys.txt").exists() && !dest.join("data/keys.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

/// A folder the package cannot write (here a file stands where its folder goes) names the path in the player's
/// language, not bare OS text; a full drive is the free-space message (#301).
#[test]
fn a_write_error_names_the_path_in_three_languages() {
    let root = std::env::temp_dir().join(format!("unpack-write-err-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dest = root.join("2017-2018");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("scripts"), b"a file, not a folder").unwrap();
    let kit = kit_zip(
        &root,
        "k.zip",
        &["Launcher.exe", "bin/x.dll", "scripts/setup.py"],
    );
    let e = unpack_kit(&kit, &dest, forza_ui::Lang::En, &|_| {}).unwrap_err();
    let path = shown(&dest.join("scripts"));
    let texts: Vec<String> = forza_ui::Lang::ALL
        .into_iter()
        .map(|l| crate::err::localize(&e, l))
        .collect();
    for t in &texts {
        assert!(t.contains(&path), "{t}");
    }
    assert!(texts.iter().skip(1).all(|t| *t != texts[0]), "{texts:?}");
    assert_eq!(
        write_err(&dest)(std::io::ErrorKind::StorageFull.into()),
        E::NoSpace
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unpack_kit_refuses_a_path_out_of_the_folder() {
    let root = std::env::temp_dir().join(format!("unpack-unsafe-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dest = root.join("in");
    fs::create_dir_all(&dest).unwrap();
    let kit = kit_zip(&root, "k.zip", &["Launcher.exe", "bin/x.dll", "../x"]);
    let e = unpack_kit(&kit, &dest, forza_ui::Lang::En, &|_| {}).unwrap_err();
    assert_eq!(e.downcast_ref::<E>(), Some(&E::KitUnsafePath));
    assert!(!root.join("x").exists());
    fs::remove_dir_all(root).unwrap();
}

/// Updating the 2017-18 install from a package (reuse + setup) in a folder with a space and non-ASCII letters: the
/// 2010-11 tree and both saves stay byte-identical.  Setup fails here (no control_Release.exe), after the package
/// was applied whole: its files stay.
#[test]
fn an_update_leaves_the_other_edition_identical() {
    let root = std::env::temp_dir()
        .join(format!("update-coexist-test-{}", std::process::id()))
        .join("Forza WCCF àé 日本");
    let _ = fs::remove_dir_all(&root);
    let dest = two_editions(&root);
    let other = version_paths(&root, "SBWG").unwrap().0;
    let before = tree(&other);
    let kit = kit_zip(
        root.parent().unwrap(),
        "k.zip",
        &["Launcher.exe", "bin/x.dll", "data/save/club.bin"],
    );
    let plan = Plan {
        dest: root.clone(),
        iso: None,
        key: None,
        game_id: Some("SDDY"),
        verify: false,
        setup: true,
        english: false,
        shortcuts: false,
        kit: Some(kit),
        latest: false,
    };
    let e = run(&plan, &AtomicBool::new(false), forza_ui::Lang::En, &|_| {}).unwrap_err();
    assert!(
        matches!(e.downcast_ref::<E>(), Some(E::Script { .. })),
        "{e:#}"
    );
    assert_eq!(tree(&other), before);
    assert_eq!(
        fs::read(dest.join("data/save/club.bin")).unwrap(),
        b"2017-18 save"
    );
    // the failed setup leaves the applied package's files in place
    assert!(dest.join("Launcher.exe").is_file() && dest.join("bin/x.dll").is_file());
    fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

/// A write that fails half way through a package over an existing install (a file stands where the package's
/// last folder goes): every file already replaced comes back byte for byte, the other edition untouched (#276).
#[test]
fn a_failed_write_puts_the_old_kit_back() {
    let root = std::env::temp_dir().join(format!("unpack-rollback-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dest = two_editions(&root);
    fs::write(dest.join("Launcher.exe"), b"old launcher").unwrap();
    fs::create_dir_all(dest.join("bin")).unwrap();
    fs::write(dest.join("bin/x.dll"), b"old dll").unwrap();
    fs::write(dest.join("zz"), b"a file, not a folder").unwrap();
    let kit = kit_zip(
        &root,
        "k.zip",
        &["Launcher.exe", "bin/x.dll", "bin/new.dll", "zz/last.txt"],
    );
    let before = tree(&root);
    let e = unpack_kit(&kit, &dest, forza_ui::Lang::En, &|_| {}).unwrap_err();
    assert!(
        matches!(e.downcast_ref::<E>(), Some(E::CannotWrite { .. })),
        "{e:#}"
    );
    assert_eq!(tree(&root), before);
    assert!(!dest.join("data/update").exists());
    fs::remove_dir_all(root).unwrap();
}

/// A program running from the install (here this test binary, copied in as a game program) refuses the package
/// before anything is written (#276).
#[cfg(windows)]
#[test]
fn a_running_program_refuses_the_package() {
    let root = std::env::temp_dir().join(format!("running-refuse-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dest = two_editions(&root);
    let exe = dest.join("game/control_Release.exe");
    fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
    let mut child = std::process::Command::new(&exe)
        .args(["install::tests::a_program_that_waits", "--exact"])
        .env("WCCF_TEST_WAIT", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let kit = kit_zip(&root, "k.zip", &["Launcher.exe", "bin/x.dll"]);
    let before = tree(&root);
    let plan = Plan {
        dest: root.clone(),
        iso: None,
        key: None,
        game_id: Some("SDDY"),
        verify: false,
        setup: true,
        english: false,
        shortcuts: false,
        kit: Some(kit),
        latest: false,
    };
    let r = run(&plan, &AtomicBool::new(false), forza_ui::Lang::En, &|_| {});
    let _ = (child.kill(), child.wait());
    let e = r.unwrap_err();
    assert!(
        matches!(e.downcast_ref::<E>(), Some(E::ProgramRunning { name }) if name.eq_ignore_ascii_case("control_Release.exe")),
        "{e:#}"
    );
    assert_eq!(tree(&root), before);
    fs::remove_dir_all(root).unwrap();
}

/// A program running from the install refuses the package in `forza::apply` too (here past the installer's own
/// check): the staging copy in data\update goes with the refusal.
#[cfg(windows)]
#[test]
fn a_running_refusal_leaves_no_staging_copy() {
    let root = std::env::temp_dir().join(format!("running-staging-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dest = two_editions(&root);
    let exe = dest.join("game/control_Release.exe");
    fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
    let mut child = std::process::Command::new(&exe)
        .args(["install::tests::a_program_that_waits", "--exact"])
        .env("WCCF_TEST_WAIT", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let kit = kit_zip(&root, "k.zip", &["Launcher.exe", "bin/x.dll"]);
    let r = unpack_kit(&kit, &dest, forza_ui::Lang::En, &|_| {});
    let _ = (child.kill(), child.wait());
    let e = r.unwrap_err();
    assert!(
        matches!(e.downcast_ref::<E>(), Some(E::ProgramRunning { .. })),
        "{e:#}"
    );
    assert!(!dest.join("data/update").exists());
    fs::remove_dir_all(root).unwrap();
}

/// The program `a_running_program_refuses_the_package` starts: this test binary, waiting when asked to.
#[test]
fn a_program_that_waits() {
    if std::env::var_os("WCCF_TEST_WAIT").is_some() {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

/// Too little free space for the game is refused before the disc is read: no verify pass, nothing written (#277).
#[test]
fn too_little_space_is_refused_before_the_verify_pass() {
    let root = std::env::temp_dir().join(format!("space-extract-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let (iso, key) = (root.join("disc.iso"), root.join("keys.bin"));
    fs::write(&iso, b"never read").unwrap();
    fs::write(&key, b"never read").unwrap();
    FREE.set(Some(1 << 30)); // 1 GB: less than the 2017-18 game
    let plan = Plan {
        dest: root.clone(),
        iso: Some(iso),
        key: Some(key),
        game_id: Some("SDDY"),
        verify: true,
        setup: false,
        english: false,
        shortcuts: false,
        kit: None,
        latest: false,
    };
    let steps = std::cell::Cell::new(0);
    let e = run(&plan, &AtomicBool::new(false), forza_ui::Lang::En, &|e| {
        if !matches!(e, Event::Log(_)) {
            steps.set(steps.get() + 1); // a stage or the verify pass's progress
        }
    })
    .unwrap_err();
    assert_eq!(e.downcast_ref::<E>(), Some(&E::NoSpace), "{e:#}");
    assert_eq!(steps.get(), 0);
    assert!(!root.join("2017-2018").exists());
    fs::remove_dir_all(root).unwrap();
}

/// Reusing an install with setup checks the free space too: the package, and setup's headroom only when the
/// install lacks its card pictures (#277).
#[test]
fn reuse_and_setup_needs_the_package_and_headroom_only_without_pictures() {
    let root = std::env::temp_dir().join(format!("space-reuse-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dest = two_editions(&root);
    let kit = kit_zip(&root, "k.zip", &["Launcher.exe", "bin/x.dll"]);
    let need = kit_room(&kit);
    let plan = Plan {
        dest: root.clone(),
        iso: None,
        key: None,
        game_id: Some("SDDY"),
        verify: false,
        setup: true,
        english: false,
        shortcuts: false,
        kit: Some(kit),
        latest: false,
    };
    let run_with = |free: u64| {
        FREE.set(Some(free));
        run(&plan, &AtomicBool::new(false), forza_ui::Lang::En, &|_| {}).unwrap_err()
    };
    // no pictures yet: setup makes them, the headroom counts
    let before = tree(&root);
    let e = run_with(need + HEADROOM);
    assert_eq!(e.downcast_ref::<E>(), Some(&E::NoSpace), "{e:#}");
    assert_eq!(tree(&root), before);
    // pictures there: the package alone
    fs::create_dir_all(dest.join(Edition::E1718.layout().cache).join("cards")).unwrap();
    let before = tree(&root);
    let e = run_with(need - 1);
    assert_eq!(e.downcast_ref::<E>(), Some(&E::NoSpace), "{e:#}");
    assert_eq!(tree(&root), before);
    assert!(!dest.join("Launcher.exe").exists());
    // just enough: the package applies (setup then fails here, no game program)
    let e = run_with(need + 1);
    assert!(
        matches!(e.downcast_ref::<E>(), Some(E::Script { .. })),
        "{e:#}"
    );
    assert!(dest.join("Launcher.exe").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn kit_zip_edition_and_prefix() {
    let dir = std::env::temp_dir().join(format!("kit-zip-test-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let prefix = |files: &[&str]| {
        let p = kit_zip(&dir, "k.zip", files);
        kit_prefix(&mut zip::ZipArchive::new(File::open(p).unwrap()).unwrap())
            .map_err(|e| e.to_string())
    };
    let a = ["Launcher.exe", "programs/wccfk.dll"];
    let b = ["Launcher.exe", "bin/x.dll"];
    assert_eq!(prefix(&a), Ok((Edition::E1011, String::new())));
    assert_eq!(prefix(&b), Ok((Edition::E1718, String::new())));
    assert_eq!(
        prefix(&["K/Launcher.exe", "K/programs/wccfk.dll"]),
        Ok((Edition::E1011, "K/".into()))
    );
    assert_eq!(
        prefix(&["K/Launcher.exe", "K/overlay/forza_panel.dll"]),
        Ok((Edition::E1718, "K/".into()))
    );
    // an old kit with only scripts/, a missing Launcher.exe and a foreign zip are not kits
    for files in [
        &["Launcher.exe", "scripts/setup.py"][..],
        &["programs/wccfk.dll"],
        &["Launcher.exe", "xprograms/wccfk.dll"],
        &["programs/wccfk.dll", "Launcher.exe/child"],
        &["K/Launcher.exe", "programs/wccfk.dll"],
        &["readme.txt"],
    ] {
        assert!(
            prefix(files)
                .unwrap_err()
                .contains("not a Forza-WCCF package"),
            "{files:?}"
        );
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn kit_edition_must_match_the_game() {
    assert!(check_setup(Some("SBWG"), Edition::E1011).is_ok());
    assert!(check_setup(Some("SDDY"), Edition::E1718).is_ok());
    assert!(check_setup(Some("SDDY"), Edition::E1011).is_err());
    assert!(check_setup(Some("SBWG"), Edition::E1718).is_err());
    assert!(check_setup(None, Edition::E1011).is_err());
}
