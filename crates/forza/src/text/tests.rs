use super::names::*;
use super::*;

/// The package folder at run time first: nextest's `--workspace-remap` sets it for an archive built on another
/// machine (the pictures' tests use it too).
pub(super) fn manifest_dir() -> std::path::PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR").map_or_else(
        || env!("CARGO_MANIFEST_DIR").into(),
        std::path::PathBuf::from,
    )
}

/// A line's words in `lang`, from forza-ui's sheet: forza words nothing (`forza-ui` needs forza), so a test that
/// reads a note takes the text from the file the screens load.
pub(super) fn note_words(lang: &str, line: &Line) -> String {
    let sheet = include_str!("../../../forza-ui/locales/ui.yml");
    match line {
        Line::Raw(s) => s.clone(),
        Line::Key(key, args) => {
            let row = sheet
                .lines()
                .find(|l| l.starts_with(&format!("{key}: {{")))
                .unwrap_or_else(|| panic!("{key} is not in ui.yml"));
            let start = row.find(&format!("{lang}: \"")).expect("the language") + lang.len() + 3;
            let rest = &row[start..];
            let mut own = rest[..rest.find('"').expect("the closing quote")].to_string();
            for (name, value) in args {
                own = own.replace(&format!("%{{{name}}}"), &note_words(lang, value));
            }
            own
        }
    }
}

/// The note's pieces are `ui.yml` keys: the English reads as it did, and the Italian none of those words.
#[test]
fn the_player_note_is_keyed() {
    let faults = NameFaults { cut: 2, unknown: 1 };
    let note = faults.note(2, 3, &[]);
    assert_eq!(
        note_words("en", &note),
        "2 players, 3 CPU teams; 2 player names are too long for the field and are cut; 1 player names have a letter the game's font cannot show and got a ?"
    );
    let it = note_words("it", &note);
    for english in ["player names", "CPU teams", "too long", "cannot show"] {
        assert!(!it.contains(english), "{it}");
    }
    assert!(!it.contains("%{"), "every argument is filled: {it}");
    assert_ne!(note_words("ja", &note), it, "the Japanese is its own text");
}

/// A reason the FAIL line shows is keyed too: the Italian and the Japanese say it in their own words, with the
/// file as the only English left, and every argument filled.
#[test]
fn the_player_reason_is_keyed() {
    let reason =
        Line::key("text.fail_set_up_first").part("what", Line::key("text.fail_what_seat1"));
    let en = note_words("en", &reason);
    assert!(
        en.starts_with("seat 1's client_Release.exe (the copy setup makes)")
            && en.ends_with("is not the expected file - press SET UP in Launcher.exe first"),
        "{en}"
    );
    let it = note_words("it", &reason);
    for english in ["seat 1", "copy setup", "expected file", "press SET UP"] {
        assert!(!it.contains(english), "{it}");
    }
    assert!(!it.contains("%{"), "every argument is filled: {it}");
    let ja = note_words("ja", &reason);
    assert!(!ja.contains("%{"), "every argument is filled: {ja}");
    assert!(!ja.contains("expected file"), "{ja}");
    assert_ne!(ja, it, "the Japanese is its own text");
}

#[test]
fn rename_players_puts_the_initial_and_keeps_empty_records() {
    let s = spec(Edition::E1718);
    let mut pd = vec![0u8; 3 * s.rec];
    let (kana, ..) = encoding_rs::SHIFT_JIS.encode("Ｃ．");
    pd[s.short_kana..][..kana.len()].copy_from_slice(&kana);
    for (i, u) in "Ronaldo".encode_utf16().enumerate() {
        pd[s.short_latin + 2 * i..][..2].copy_from_slice(&u.to_le_bytes());
    }
    let d = 2 * s.rec + s.display.unwrap();
    pd[d..d + FIELD].fill(b'A'); // no end byte: the folded name is FIELD bytes, one too many
    let names = rename_players(&mut pd, &s);
    assert_eq!(first(&pd[s.short_kana..s.short_kana + FIELD]), b"C.Ronaldo");
    assert!(
        pd[s.rec..2 * s.rec].iter().all(|&b| b == 0),
        "the empty record keeps its bytes"
    );
    assert_eq!((names.cut, names.unknown), (1, 0));
}

#[test]
fn put_field_keeps_the_fill() {
    assert_eq!(put_field(b"ab\0\xAA\xAA\xAA", b"xyz", 6), b"xyz\0\xAA\xAA");
    assert_eq!(put_field(b"abcdef", b"toolong", 6), b"toolo\0");
    assert_eq!(put_field(b"abcde\0", b"q", 6), b"q\0\0\0\0\0");
}

#[test]
fn fold_drops_accents() {
    assert_eq!(fold("Kaká Øre ß"), "Kaka Ore SS");
    assert_eq!(fold("日"), "?");
}

#[test]
fn a_cut_or_unknown_player_name_is_counted() {
    let mut f = NameFaults::default();
    f.add("Del Piero");
    assert_eq!((f.cut, f.unknown), (0, 0), "a name that fits as it is");
    f.add(&"A".repeat(FIELD - 1));
    assert_eq!(
        (f.cut, f.unknown),
        (0, 0),
        "the longest name the field keeps whole"
    );
    f.add(&"A".repeat(FIELD));
    assert_eq!(
        (f.cut, f.unknown),
        (1, 0),
        "one byte too many for the field"
    );
    f.add(&fold("FC 東京"));
    assert_eq!(
        (f.cut, f.unknown),
        (1, 1),
        "a letter the font has no glyph for"
    );
    let n = note_words("en", &f.note(1, 0, &[]));
    assert!(
        n.contains("1 player names are too long for the field"),
        "{n}"
    );
    assert!(n.contains("1 player names have a letter"), "{n}");
}

#[test]
fn codes_must_match() {
    let e = Edition::E1718;
    assert!(encoded(e, "%dポイント", "%d points", b"\n").is_ok());
    assert!(encoded(e, "%dポイント", "points", b"\n").is_err());
    // the codes in the game's order: a sort would let "%d ... %s" stand against a "%s ... %d" line and the
    // game would print each value in the other's slot
    assert!(encoded(e, "%sは%d点", "%d points by %s", b"\n").is_err());
    assert!(encoded(e, "%sは%d点", "%s: %d points", b"\n").is_ok());
    assert!(encoded(e, "$c[FF0000FF]赤$c", "red", b"\n").is_err());
    assert!(
        encoded(Edition::E1011, "$pあ$t", "a$p", b"\n").is_err(),
        "a layout code dropped"
    );
    assert!(encoded(Edition::E1011, "$pあ$t", "a$p$t", b"\n").is_ok());
    assert!(encoded(e, "あ", "<empty>", b"\n").unwrap().is_empty());
    assert!(encoded(e, "%dあ", "<empty>", b"\n").is_err());
    assert!(encoded(e, "あ", "100% sure", b"\n").is_err());
    assert_eq!(
        encoded(Edition::E1011, "あ", "a\\nb", b"\r\n").unwrap(),
        b"a\r\nb"
    );
}

/// string_list.bin: tag, length, body; .hf: a header, then `key slot0 slot1`.
fn strings() -> (Vec<u8>, Vec<u8>) {
    let mut body = b"\0".to_vec();
    for s in ["jp one", "second", "jp two"] {
        body.extend(s.as_bytes());
        body.push(0);
    }
    body.extend([0; 80]);
    let hf = b"header\r\nA_B_1 1 8\r\nA_B_2 15 8\r\n".to_vec();
    let mut bin = b"TAG!".to_vec();
    bin.extend((body.len() as u32).to_le_bytes());
    bin.extend(body);
    (bin, hf)
}

#[test]
fn a_line_without_two_numbers_is_refused() {
    let bin = strings().0;
    for hf in [&b"header\r\nA_B_1\r\n"[..], b"header\r\nA_B_1 1\r\n"] {
        assert!(
            read_strings(&bin, hf).is_err(),
            "{:?}",
            String::from_utf8_lossy(hf)
        );
    }
    assert!(read_strings(&bin, b"header\r\nA_B_1 1 8\r\n").is_ok());
}

#[test]
fn pack_strings_swaps_slot_zero() {
    let (bin, hf) = strings();
    let (b0, h0, pos) = pack_strings(&bin, &hf, &BTreeMap::new()).unwrap();
    assert_eq!((b0, h0, pos.len()), (bin.clone(), hf.clone(), 0));
    let enc = BTreeMap::from([
        ("A_B_1".to_string(), b"a much longer english line".to_vec()),
        ("A_B_2".to_string(), b"two".to_vec()),
    ]);
    let (nb, nh, pos) = pack_strings(&bin, &hf, &enc).unwrap();
    assert_eq!(nb.len(), bin.len());
    let (body, keys) = read_strings(&nb, &nh).unwrap();
    for (k, v) in &enc {
        assert_eq!(at(&body, keys[k].1[0]), &v[..], "{k}");
        assert_eq!(at(&body, keys[k].1[1]), b"second");
    }
    assert_eq!(pos["A_B_2"], 15, "a line that fits stays where it was");
}

#[test]
fn enum_round_trip_and_swap() {
    let f = |k: &str, jp: &str| -> Rec {
        (
            7,
            [k, "c", jp, "e", "i", "s", "f", "g"]
                .iter()
                .map(|x| x.as_bytes().to_vec())
                .collect(),
        )
    };
    let raw = write_enum(&[f("A_B_1", "jp one"), f("A_B_9", "other")]);
    assert_eq!(write_enum(&read_enum(&raw).unwrap()), raw);
    assert!(read_enum(&raw[..raw.len() - 1]).is_err(), "cut short");
    let (_, keys) = read_strings(&strings().0, &strings().1).unwrap();
    let body = strings().0[8..].to_vec();
    let (new, n) = pack_enum(
        &raw,
        &body,
        &keys,
        &BTreeMap::from([("A_B_1".to_string(), b"one".to_vec())]),
    )
    .unwrap();
    assert_eq!(n, 1);
    assert_eq!(read_enum(&new).unwrap()[0].1[2], b"one");
    assert_eq!(read_enum(&new).unwrap()[1].1[2], b"other");
}

/// A PE with one section holding an import table that names `dll`.
fn pe(dll: &[u8]) -> Vec<u8> {
    let mut d = vec![0; 0x400];
    d[0x3C] = 0x40;
    d[0x40 + 6] = 1; // one section
    d[0x40 + 20] = 0xE0; // optional header size
    let opt = 0x40 + 24;
    d[opt + 104..opt + 108].copy_from_slice(&0x1000u32.to_le_bytes()); // import table rva
    let sec = opt + 0xE0;
    for (o, v) in [(8, 0x200u32), (12, 0x1000), (16, 0x200), (20, 0x200)] {
        d[sec + o..sec + o + 4].copy_from_slice(&v.to_le_bytes());
    }
    d[0x200 + 12..0x200 + 16].copy_from_slice(&0x1100u32.to_le_bytes()); // name rva -> file 0x300
    d[0x300..0x300 + dll.len()].copy_from_slice(dll);
    d
}

#[test]
fn hook_import_renames_and_returns() {
    let orig = pe(SYS_DLL);
    let hooked = rename_import(&orig, SYS_DLL, HOOK_DLL).unwrap();
    assert_ne!(hooked, orig);
    assert!(crate::setup::imports(&hooked, HOOK_DLL));
    assert_eq!(norm(hooked, "client_Release.exe").unwrap(), orig);
    assert_eq!(norm(orig.clone(), "client_Release.exe").unwrap(), orig);
    assert!(rename_import(&orig, HOOK_DLL, SYS_DLL).is_err());
}

#[test]
fn on_then_off_restores_and_matches_the_python_manifest() {
    let t = tempfile::tempdir().unwrap();
    let (game, backup) = (t.path().join("game"), t.path().join("backup"));
    let rel = "prog_data/cpu_team/cpu_1.dat";
    let orig = vec![7u8; 0x200];
    fs::create_dir_all(game.join("prog_data/cpu_team")).unwrap();
    fs::write(game.join(rel), &orig).unwrap();
    let mut c = Ctx::new_bare(Edition::E1718, &game, &backup);
    c.cpu = vec![(rel.into(), sha(&orig), "x".into(), String::new())];
    c.sheets = t.path().join("kit");
    let sheet = c.sheets.join("italian/screen_text.tsv");
    fs::create_dir_all(sheet.parent().unwrap()).unwrap();
    fs::write(&sheet, "key\tit\nSYS_SEC_data_0006\tmese %d giorno %d\n").unwrap();
    let new = vec![(rel.to_string(), vec![9u8; 0x200])];
    assert!(
        !stale(Edition::E1718, &backup, &c.sheets),
        "no text in place"
    );
    c.turn_on(&new, Lang::It).unwrap();
    c.turn_on(&new, Lang::It).unwrap(); // again: builds from the backup, same result
    assert!(
        !stale(Edition::E1718, &backup, &c.sheets),
        "built from these sheets"
    );
    fs::write(&sheet, "key\tit\nSYS_SEC_data_0006\t%d/%d\n").unwrap();
    assert!(
        stale(Edition::E1718, &backup, &c.sheets),
        "a newer kit's sheets"
    );
    fs::remove_file(backup.join(SHEETS)).unwrap();
    assert!(
        stale(Edition::E1718, &backup, &c.sheets),
        "a text from before sheets.txt"
    );
    assert_eq!(fs::read(game.join(rel)).unwrap(), new[0].1);
    assert_eq!(
        fs::read_to_string(backup.join("language.txt")).unwrap(),
        "it\n"
    );
    let k = key(rel).replace('\\', "\\\\");
    assert_eq!(
        fs::read_to_string(backup.join("manifest.json")).unwrap(),
        format!(
            "{{\n \"{k}\": {{\n  \"original\": \"{}\",\n  \"english\": \"{}\"\n }}\n}}",
            sha(&orig),
            sha(&new[0].1)
        )
    );
    assert_eq!(c.turn_off().unwrap(), 1);
    assert_eq!(fs::read(game.join(rel)).unwrap(), orig);
    assert!(!backup.exists(), "the backup folder is gone");
}

#[test]
fn a_foreign_file_is_not_replaced() {
    let t = tempfile::tempdir().unwrap();
    let game = t.path().join("game");
    fs::create_dir_all(game.join("prog_data/cpu_team")).unwrap();
    fs::write(
        game.join("prog_data/cpu_team/cpu_1.dat"),
        b"not the original",
    )
    .unwrap();
    let mut c = Ctx::new_bare(Edition::E1718, &game, &t.path().join("backup"));
    c.cpu = vec![(
        "prog_data/cpu_team/cpu_1.dat".into(),
        sha(b"the original"),
        "x".into(),
        String::new(),
    )];
    assert!(
        c.turn_on(
            &[("prog_data/cpu_team/cpu_1.dat".into(), vec![1])],
            Lang::En
        )
        .is_err()
    );
    assert_eq!(
        fs::read(game.join("prog_data/cpu_team/cpu_1.dat")).unwrap(),
        b"not the original"
    );
}

#[test]
fn a_guest_team_keyed_through_cpu_team_by_0_3_2_is_rebuilt() {
    let t = tempfile::tempdir().unwrap();
    let (game, backup) = (t.path().join("game"), t.path().join("backup"));
    let rel = "prog_data/guest_team/cpu_0.dat";
    let (orig, old_en, new_en) = (vec![1u8; 64], vec![2u8; 64], vec![3u8; 64]);
    for (dir, data) in [(&game, &old_en), (&backup, &orig)] {
        fs::create_dir_all(dir.join("prog_data/guest_team")).unwrap();
        fs::write(dir.join(rel), data).unwrap();
    }
    let old_key = key("prog_data/cpu_team/../guest_team/cpu_0.dat");
    let man = serde_json::json!({ old_key: { "original": sha(&orig), "english": sha(&old_en) } });
    fs::write(backup.join("manifest.json"), man.to_string()).unwrap();
    let mut c = Ctx::new_bare(Edition::E1718, &game, &backup);
    c.cpu = vec![(rel.into(), sha(&orig), "x".into(), String::new())];
    c.sheets = t.path().join("kit");
    c.turn_on(&[(rel.into(), new_en.clone())], Lang::En)
        .unwrap();
    assert_eq!(fs::read(game.join(rel)).unwrap(), new_en);
    assert_eq!(
        fs::read(backup.join(rel)).unwrap(),
        orig,
        "the original stays backed up"
    );
    assert_eq!(
        c.load_manifest().unwrap().keys().collect::<Vec<_>>(),
        [&key(rel)]
    );
    assert_eq!(c.turn_off().unwrap(), 1);
    assert_eq!(fs::read(game.join(rel)).unwrap(), orig);
}

#[test]
fn original_of_reads_the_old_key_and_the_new() {
    let old = String::from_utf8(vec![0x73, 0x65, 0x67, 0x61]).unwrap(); // the name older kits wrote
    let mut cases = vec![
        serde_json::json!({ old.as_str(): "AB", "english": "CD" }),
        serde_json::json!({ "original": "AB", "english": "CD" }),
        serde_json::json!({ old.as_str(): "AB" }),
    ];
    cases.push(serde_json::json!({ "original": "AB" }));
    assert!(
        cases.iter().all(|e| original_of(e) == Some("AB")),
        "{cases:?}"
    );
    assert_eq!(original_of(&serde_json::json!({ "english": "CD" })), None);
}

/// Two files backed up and switched on by an older Python kit: the old key and Windows backslash keys. With
/// `half` the second file's "off" had already run (backup gone, game file original).
fn old_kit_state(half: bool) -> (tempfile::TempDir, Ctx, Vec<(String, Vec<u8>)>) {
    let t = tempfile::tempdir().unwrap();
    let (game, backup) = (t.path().join("game"), t.path().join("backup"));
    let old = String::from_utf8(vec![0x73, 0x65, 0x67, 0x61]).unwrap();
    let (mut man, mut files) = (Map::new(), vec![]);
    for (i, rel) in [
        "prog_data/cpu_team/cpu_1.dat",
        "data/string/string_list.bin",
    ]
    .into_iter()
    .enumerate()
    {
        let (orig, new) = (vec![i as u8 + 1; 64], vec![i as u8 + 9; 64]);
        let done = half && i == 1;
        fs::create_dir_all(game.join(rel).parent().unwrap()).unwrap();
        fs::write(game.join(rel), if done { &orig } else { &new }).unwrap();
        if !done {
            fs::create_dir_all(backup.join(rel).parent().unwrap()).unwrap();
            fs::write(backup.join(rel), &orig).unwrap();
        }
        let e = serde_json::json!({ old.as_str(): sha(&orig), "english": sha(&new) });
        man.insert(rel.replace('/', "\\"), e);
        files.push((rel.to_string(), orig));
    }
    fs::create_dir_all(&backup).unwrap();
    fs::write(
        backup.join("manifest.json"),
        serde_json::to_vec(&man).unwrap(),
    )
    .unwrap();
    fs::write(backup.join("language.txt"), "it\n").unwrap();
    (t, Ctx::new_bare(Edition::E1718, &game, &backup), files)
}

#[test]
fn an_older_kits_manifest_turns_off() {
    for half in [false, true] {
        let (t, c, files) = old_kit_state(half);
        assert_eq!(c.turn_off().unwrap(), 2, "half={half}");
        for (rel, orig) in files {
            assert_eq!(fs::read(t.path().join("game").join(rel)).unwrap(), orig);
        }
        assert!(!t.path().join("backup").exists(), "half={half}");
    }
}

/// "off" through `run`: the exit code and the keys it said.
fn run_off(game: &Path, backup: &Path, kit: &Path) -> (i32, Vec<String>) {
    let said = std::cell::RefCell::new(vec![]);
    let say: crate::Say = &|_, l| said.borrow_mut().push(format!("{l:?}"));
    let code = run(Edition::E1718, game, backup, kit, Mode::Off, Lang::En, say);
    (code, said.into_inner())
}

#[test]
fn a_damaged_backup_list_refuses_off_and_changes_nothing() {
    let (t, _, game, backup) = translated_game(false);
    fs::create_dir_all(&backup).unwrap();
    fs::write(backup.join("manifest.json"), b"{").unwrap();
    let before = fs::read(game.join(TEAM)).unwrap();
    let (code, said) = run_off(&game, &backup, &t.path().join("kit"));
    assert_eq!(code, 1, "{said:?}");
    assert!(
        !said.iter().any(|l| l.contains("already_original")),
        "{said:?}"
    );
    assert_eq!(fs::read(game.join(TEAM)).unwrap(), before);
    assert_eq!(fs::read(backup.join("manifest.json")).unwrap(), b"{");
}

#[test]
fn a_damaged_list_over_the_original_text_is_set_aside() {
    let (t, orig, game, backup) = translated_game(false);
    let kit = t.path().join("kit");
    fs::write(game.join(TEAM), &orig).unwrap();
    fs::remove_file(game.join(BIN)).unwrap();
    fs::create_dir_all(&backup).unwrap();
    fs::write(backup.join("manifest.json"), b"{").unwrap();
    let (code, said) = run_off(&game, &backup, &kit);
    assert_eq!(code, 0, "{said:?}");
    assert!(
        said.iter().any(|l| l.contains("text.already_original")),
        "{said:?}"
    );
    assert_eq!(fs::read(backup.join("manifest.json.bad")).unwrap(), b"{");
    let c = Ctx::new(Edition::E1718, &game, &backup, &kit).unwrap();
    c.turn_on(&[(TEAM.to_string(), vec![9u8; 64])], Lang::En)
        .unwrap();
    assert_eq!(fs::read(backup.join(TEAM)).unwrap(), orig);
    assert!(c.load_manifest().unwrap().contains_key(&key(TEAM)));
}

#[test]
fn a_missing_backup_list_is_not_an_error() {
    let (t, _, game, backup) = translated_game(false);
    let (code, said) = run_off(&game, &backup, &t.path().join("kit"));
    assert_eq!(code, 0, "{said:?}");
    assert!(
        said.iter().any(|l| l.contains("text.already_original")),
        "{said:?}"
    );
}

#[test]
fn the_hook_import_rename_is_a_pure_name_swap() {
    let orig = pe(SYS_DLL);
    let hooked = rename_import(&orig, SYS_DLL, HOOK_DLL).unwrap();
    assert_eq!(hooked.len(), orig.len());
    // The name's nine bytes change, not its NUL: a write over the terminator must fail this.
    assert!(orig.iter().zip(&hooked).filter(|(a, b)| a != b).count() < SYS_DLL.len());
    assert!(
        !hooked
            .to_ascii_lowercase()
            .windows(5)
            .any(|w| w == b"winmm")
    );
    assert_eq!(rename_import(&hooked, HOOK_DLL, SYS_DLL).unwrap(), orig);
    assert!(
        rename_import(&hooked, SYS_DLL, HOOK_DLL).is_err(),
        "already renamed"
    );
    assert!(
        rename_import(&pe(b"other.dll"), SYS_DLL, HOOK_DLL).is_err(),
        "no such import"
    );
}

/// Sheets in a scratch folder, 2017-18 layout (no `languages/`).
fn sheets_1718(files: &[(&str, &str)]) -> (tempfile::TempDir, Ctx) {
    let t = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let p = t.path().join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    let mut c = Ctx::new_bare(Edition::E1718, Path::new("g"), Path::new("b"));
    c.sheets = t.path().into();
    (t, c)
}

#[test]
fn game_text_1718_puts_italian_first_and_english_never_shows_italian() {
    let (_t, c) = sheets_1718(&[
        (
            "english/screen_text.tsv",
            "key\tenglish\nK_BOTH\tkit\nK_EN\tkit\n",
        ),
        ("english/original_rstring.tsv", "key\tenglish\n"),
        (
            "italian/screen_text.tsv",
            "key\titaliano\nK_BOTH\tkit Italian\n",
        ),
    ]);
    let none = Keys::new();
    let en = c.choices(b"\0", &none, Lang::En).unwrap();
    let it = c.choices(b"\0", &none, Lang::It).unwrap();
    assert_eq!(
        it["K_BOTH"],
        [
            ("kit Italian".into(), Source::KitItalian),
            ("kit".into(), Source::Kit)
        ]
    );
    assert_eq!(
        it["K_BOTH"][1], en["K_BOTH"][0],
        "the English stays behind it"
    );
    assert_eq!(
        it["K_EN"], en["K_EN"],
        "a key with no Italian stays English"
    );
    assert!(
        en.values().flatten().all(|(_, s)| *s == Source::Kit),
        "English shows no Italian"
    );
}

#[test]
fn the_string_list_is_the_first_original_and_kit_sources_are_the_kit_s() {
    for ed in [Edition::E1011, Edition::E1718] {
        assert_eq!(spec(ed).originals[STRINGS].0, BIN, "{ed:?}");
    }
    assert!(Source::KitItalian.kit() && !Source::OriginalItalian.kit());
}

#[test]
fn game_text_1718_falls_back_when_a_line_loses_a_code_or_a_letter() {
    let (body, _, _) = encoding_rs::SHIFT_JIS.encode("あ$c[FF6633FF]%d$c\0");
    let keys = Keys::from([("CLM_X_0000".to_string(), (1, vec![0, 0]))]);
    let c = |first: &str| -> Choices {
        Choices::from([(
            "CLM_X_0000".to_string(),
            vec![
                (first.to_string(), Source::KitItalian),
                ("$c[FF6633FF]%d$c points".to_string(), Source::Kit),
            ],
        )])
    };
    let e = Edition::E1718;
    let (enc, used, misfit, _) = encode_lines(e, &body, &keys, &c("%d punti"), b"\n");
    assert_eq!(
        enc["CLM_X_0000"], b"$c[FF6633FF]%d$c points",
        "lost the colour code"
    );
    assert_eq!((used["CLM_X_0000"], misfit.len()), (Source::Kit, 1));
    let (enc, ..) = encode_lines(e, &body, &keys, &c("$c[FF6633FF]%d$c punti"), b"\n");
    assert_eq!(enc["CLM_X_0000"], b"$c[FF6633FF]%d$c punti");
    // a letter outside the game's font (cp932) also falls back
    let (enc, _, misfit, _) = encode_lines(e, &body, &keys, &c("$c[FF6633FF]%d$c \u{141}"), b"\n");
    assert_eq!(
        (enc["CLM_X_0000"].as_slice(), misfit.len()),
        (&b"$c[FF6633FF]%d$c points"[..], 1)
    );
}

/// Every CPU and guest team file of the original game has its row, with an English and (if any) an Italian
/// name that fit; a new or emptied row fails here.
#[test]
fn every_team_has_english_and_italian_names() {
    for (ed, dir, files) in [
        (Edition::E1011, "2010-11", 252),
        (Edition::E1718, "2017-18", 599 + 11),
    ] {
        let sheets = manifest_dir().join("../../editions").join(dir);
        let c = Ctx::new(ed, Path::new("g"), Path::new("b"), &sheets).unwrap();
        let mut seen = HashSet::new();
        for (rel, h, en, it) in &c.cpu {
            assert!(seen.insert(rel), "{dir} {rel}: twice");
            assert_eq!(h.len(), 64, "{dir} {rel}: no original hash");
            assert!(team_name_fits(en), "{dir} {rel}: English {en:?}");
            assert!(
                it.is_empty() || team_name_fits(it),
                "{dir} {rel}: Italian {it:?}"
            );
        }
        assert_eq!(seen.len(), files, "{dir}: one row per team file");
    }
}

#[test]
fn the_italian_tables_2010_11_match_the_english() {
    let dir = manifest_dir().join("../../editions/2010-11");
    let mut c = Ctx::new_bare(Edition::E1011, Path::new("g"), Path::new("b"));
    c.sheets = dir;
    let mut english = HashMap::new(); // the kit's own English wins, as in `choices`
    for name in ["original_rstring.tsv", "screen_text.tsv"] {
        for r in c.sheet(name, "english").unwrap() {
            if !cell(&r, "english").trim().is_empty() {
                english.insert(cell(&r, "key").to_string(), cell(&r, "english").to_string());
            }
        }
    }
    let (mut bad, mut n) = (vec![], 0);
    for name in ["screen_text.tsv", "original_rstring.tsv"] {
        let rows = c.sheet(name, "italian").unwrap();
        let mut seen = HashSet::new();
        for r in &rows {
            let (k, it) = (cell(r, "key"), cell(r, "italiano"));
            n += 1;
            if !seen.insert(k) {
                bad.push(format!("{name} {k}: twice"));
            }
            if encoding_rs::SHIFT_JIS.encode(it).2 {
                bad.push(format!("{name} {k}: a letter the game's font cannot show"));
            }
            let Some(en) = english
                .get(k)
                .filter(|e| name == "screen_text.tsv" && e.trim() != "<empty>")
            else {
                continue; // the original's lines: a misfit is left for the English
            };
            let fmt = |s: &str| {
                FMT.find_iter(s)
                    .map(|m| m.as_str().to_string())
                    .collect::<Vec<_>>()
            };
            if fmt(it) != fmt(en) || it.matches("$c").count() != en.matches("$c").count() {
                bad.push(format!(
                    "{name} {k}: codes {:?}, the English has {:?}",
                    fmt(it),
                    fmt(en)
                ));
            }
        }
    }
    assert!(
        n > 0 && bad.is_empty(),
        "{} problem(s): {:?}",
        bad.len(),
        &bad[..bad.len().min(5)]
    );
}

#[test]
fn the_italian_tables_2017_18_match_the_english() {
    let mut c = Ctx::new_bare(Edition::E1718, Path::new("g"), Path::new("b"));
    c.sheets = manifest_dir().join("../../editions/2017-18");
    // the kit's English, else the original European English (the line the game shows without the kit's)
    let mut english: HashMap<String, String> = HashMap::new();
    for name in ["original_rstring.tsv", "screen_text.tsv"] {
        for r in c.sheet(name, "english").unwrap() {
            if !cell(&r, "english").trim().is_empty() {
                english.insert(cell(&r, "key").into(), cell(&r, "english").into());
            }
        }
    }
    let codes = |s: &str| {
        let mut v: Vec<String> = CODES.find_iter(s).map(|m| m.as_str().into()).collect();
        v.sort();
        let f: Vec<String> = FMT.find_iter(s).map(|m| m.as_str().into()).collect();
        (f, v)
    };
    let (mut bad, mut n) = (vec![], 0);
    for r in c.sheet("screen_text.tsv", "italian").unwrap() {
        let (k, it) = (cell(&r, "key"), cell(&r, "italiano"));
        n += 1;
        match english.get(k).filter(|e| !e.trim().is_empty()) {
            None => bad.push(format!("{k}: no English row")),
            Some(en) if en.trim() != "<empty>" && codes(en) != codes(it) => bad.push(format!(
                "{k}: codes {:?}, the English has {:?}",
                codes(it),
                codes(en)
            )),
            _ => {}
        }
    }
    assert!(
        n > 0 && bad.is_empty(),
        "{} problem(s): {:?}",
        bad.len(),
        &bad[..bad.len().min(5)]
    );
}

/// The locker-room match report joins two table lines with no separator: opponent + score, team play + its
/// result, player play + his. A lost space ran them together on screen ("inferiore.Vittoria", "mondialeHa"),
/// so the second line of each pair carries the space at its start and the first never ends one.
#[test]
fn the_1718_match_report_lines_join_with_one_space() {
    let mut c = Ctx::new_bare(Edition::E1718, Path::new("g"), Path::new("b"));
    c.sheets = manifest_dir().join("../../editions/2017-18");
    let glue = Regex::new(r"[a-z][A-Z]|[.!?,][A-Z]").unwrap();
    let pairs = [
        (
            "MAN_MATCH_report_team_enemy_",
            "MAN_MATCH_report_team_score_",
        ),
        (
            "MAN_MATCH_report_team_perform_",
            "MAN_MATCH_report_team_value_",
        ),
        (
            "MAN_MATCH_report_player_perform_",
            "MAN_MATCH_report_player_value_",
        ),
    ];
    let (mut bad, mut n) = (vec![], 0);
    for (folder, column) in [("english", "english"), ("italian", "italiano")] {
        let table: HashMap<String, String> = c
            .sheet("screen_text.tsv", folder)
            .unwrap()
            .iter()
            .map(|r| (cell(r, "key").into(), cell(r, column).into()))
            .collect();
        let with = |pre: &str| {
            let mut v: Vec<(&String, &String)> =
                table.iter().filter(|(k, _)| k.starts_with(pre)).collect();
            v.sort();
            v
        };
        for (first, second) in pairs {
            for (k1, a) in with(first) {
                for (k2, b) in with(second) {
                    n += 1;
                    let joined = format!("{a}{b}");
                    if a.ends_with(' ') || !b.starts_with(' ') || b.starts_with("  ") {
                        bad.push(format!("{folder} {k1} + {k2}: the join is {joined:?}"));
                    } else if glue.is_match(&joined) {
                        bad.push(format!("{folder} {k1} + {k2}: {joined:?}"));
                    }
                }
            }
        }
    }
    assert!(
        n > 3000 && bad.is_empty(),
        "{n} pairs, {} problem(s): {:?}",
        bad.len(),
        &bad[..bad.len().min(5)]
    );
}

/// Texts the 2017-18 client prints with sprintf_s into a fixed buffer: (key, buffer bytes with its NUL, widest
/// number for each %d). A text that does not fit ends the game with 0xC0000417 (Italian "mese %d giorno %d" in the
/// secretary's 10-byte birthday, sub_500910, 2026-10-22). The friend-mission notice goes into 128 bytes at 0x6D0EEE
/// (sub_6CEBE0); its Italian was 139. The other-stores note goes into 64 bytes at 0x6F5B12 (sub_6F5750). The sweep
/// of 2026-10-22 found no other sheet text within 48 bytes of its buffer.
const FIXED_BUFFERS: &[(&str, usize, &str)] = &[
    ("SYS_SEC_data_0006", 10, "99"),
    ("INF_094_mission_0000", 128, ""),
    ("INF_084_TOPICS_message_0001", 64, "-2147483648"),
];

/// The bytes sprintf_s writes for `text`, its NUL included, with every `%d` as wide as `widest`; any other
/// conversion has no widest value here, so it is refused rather than counted as its own letters.
fn fixed_len(text: &str, widest: &str) -> Result<usize, String> {
    match FMT.find_iter(text).find(|m| m.as_str() != "%d") {
        Some(m) => Err(format!(
            "{} has no widest value in FIXED_BUFFERS",
            m.as_str()
        )),
        None => Ok(text.replace("%d", widest).len() + 1),
    }
}

#[test]
fn fixed_len_counts_every_conversion() {
    assert_eq!(fixed_len("%d/%d", "99"), Ok(6));
    for t in ["%s", "%u", "%2d"] {
        assert!(fixed_len(t, "99").is_err(), "{t}");
    }
}

#[test]
fn the_2017_18_texts_fit_the_game_s_fixed_buffers() {
    let mut c = Ctx::new_bare(Edition::E1718, Path::new("g"), Path::new("b"));
    c.sheets = manifest_dir().join("../../editions/2017-18");
    let mut bad = vec![];
    for (lang, column) in [("english", "english"), ("italian", "italiano")] {
        let rows = c.sheet("screen_text.tsv", lang).unwrap();
        for (key, size, widest) in FIXED_BUFFERS {
            let Some(r) = rows.iter().find(|r| cell(r, "key") == *key) else {
                bad.push(format!("{lang} {key}: no row"));
                continue;
            };
            match fixed_len(cell(r, column), widest) {
                Ok(n) if n <= *size => {}
                Ok(n) => bad.push(format!(
                    "{lang} {key}: needs {n} bytes, the game has {size}"
                )),
                Err(e) => bad.push(format!("{lang} {key}: {e}")),
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

const TEAM: &str = "prog_data/cpu_team/cpu_1.dat";

/// A kit folder (its sheets name `TEAM`'s original), a game whose text an older kit changed (a changed
/// `string_list.bin`, `TEAM` in "old" text) and, with `kept`, the originals beside the game. Returns the root,
/// the original `TEAM`, the game and the fresh kit's backup folder (not there yet).
fn translated_game(kept: bool) -> (tempfile::TempDir, Vec<u8>, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let (orig, old) = (vec![7u8; 64], vec![8u8; 64]);
    let game = t.path().join("game");
    for (rel, d) in [(TEAM, &old), (BIN, &old)] {
        fs::create_dir_all(game.join(rel).parent().unwrap()).unwrap();
        fs::write(game.join(rel), d).unwrap();
    }
    let sheet = t.path().join("kit/english/cpu_names.tsv");
    fs::create_dir_all(sheet.parent().unwrap()).unwrap();
    fs::write(
        &sheet,
        format!(
            "file\toriginal_sha256\tenglish\titaliano\ncpu_1.dat\t{}\tx\t\n",
            sha(&orig)
        ),
    )
    .unwrap();
    if kept {
        let b = beside(&game);
        fs::create_dir_all(b.join(TEAM).parent().unwrap()).unwrap();
        fs::write(b.join(TEAM), &orig).unwrap();
        let k = key(TEAM);
        let man = serde_json::json!({ k: {"original": sha(&orig), "english": sha(&old)} });
        fs::write(b.join("manifest.json"), serde_json::to_vec(&man).unwrap()).unwrap();
        fs::write(b.join("language.txt"), "en\n").unwrap();
    }
    let backup = t.path().join("fresh/data/english_backup");
    (t, orig, game, backup)
}

#[test]
fn a_fresh_folder_takes_the_backup_beside_the_game_and_builds_current_text() {
    let (t, orig, game, backup) = translated_game(true);
    let kit = t.path().join("kit");
    assert_eq!(
        sync_beside(Edition::E1718, &game, &backup, &kit),
        Beside::Fine
    );
    assert!(
        stale(Edition::E1718, &backup, &kit),
        "the old text is rebuilt"
    );
    let c = Ctx::new(Edition::E1718, &game, &backup, &kit).unwrap();
    c.turn_on(&[(TEAM.to_string(), vec![9u8; 64])], Lang::En)
        .unwrap();
    assert_eq!(
        fs::read(game.join(TEAM)).unwrap(),
        vec![9u8; 64],
        "the current text"
    );
    assert_eq!(fs::read(backup.join(TEAM)).unwrap(), orig);
    assert!(!stale(Edition::E1718, &backup, &kit));
    c.save_beside().unwrap(); // what `run` does after an `on`
    assert_eq!(fs::read(beside(&game).join(TEAM)).unwrap(), orig);
    assert_eq!(c.turn_off().unwrap(), 1);
    assert_eq!(fs::read(game.join(TEAM)).unwrap(), orig);
}

#[test]
fn a_backup_that_is_not_the_original_is_refused() {
    let (t, _, game, backup) = translated_game(true);
    let kit = t.path().join("kit");
    fs::write(beside(&game).join(TEAM), vec![1u8; 64]).unwrap();
    assert_eq!(
        sync_beside(Edition::E1718, &game, &backup, &kit),
        Beside::Foreign
    );
    assert!(!backup.exists(), "nothing taken");
    assert_eq!(
        fs::read(game.join(TEAM)).unwrap(),
        vec![8u8; 64],
        "the text untouched"
    );
    let (t, _, game, backup) = translated_game(false); // no backup anywhere
    let kit = t.path().join("kit");
    assert_eq!(
        sync_beside(Edition::E1718, &game, &backup, &kit),
        Beside::Foreign
    );
    assert!(!backup.exists());
}

#[test]
fn the_same_folder_update_keeps_its_own_backup() {
    let (t, orig, game, backup) = translated_game(false);
    let kit = t.path().join("kit");
    let c = Ctx::new(Edition::E1718, &game, &backup, &kit).unwrap();
    fs::write(game.join(TEAM), &orig).unwrap();
    c.turn_on(&[(TEAM.to_string(), vec![8u8; 64])], Lang::It)
        .unwrap();
    fs::remove_file(backup.join(SHEETS)).unwrap(); // text from an older kit
    let before = fs::read(backup.join("manifest.json")).unwrap();
    assert_eq!(
        sync_beside(Edition::E1718, &game, &backup, &kit),
        Beside::Fine
    );
    assert_eq!(fs::read(backup.join("manifest.json")).unwrap(), before);
    assert_eq!(fs::read(backup.join("language.txt")).unwrap(), b"it\n");
    assert!(
        stale(Edition::E1718, &backup, &kit),
        "still rebuilt by the normal path"
    );
    assert_eq!(
        fs::read(beside(&game).join(TEAM)).unwrap(),
        orig,
        "kept beside the game"
    );
    // a backup beside the game is not read when this folder has its own
    fs::write(beside(&game).join(TEAM), vec![1u8; 64]).unwrap();
    assert_eq!(
        sync_beside(Edition::E1718, &game, &backup, &kit),
        Beside::Fine
    );
    assert_eq!(fs::read(backup.join(TEAM)).unwrap(), orig);
    // an original game with no backup: nothing to say
    let (t, _, game, backup) = translated_game(false);
    fs::remove_file(game.join(BIN)).unwrap();
    assert_eq!(
        sync_beside(Edition::E1718, &game, &backup, &t.path().join("kit")),
        Beside::Fine
    );
}

const TEAM2: &str = "prog_data/cpu_team/cpu_2.dat";

/// A second kit folder `name` whose sheet pins `TEAM` and `TEAM2` (both originals `orig`).
fn bigger_kit(root: &Path, name: &str, orig: &[u8]) -> PathBuf {
    let kit = root.join(name);
    fs::create_dir_all(kit.join("english")).unwrap();
    fs::write(
        kit.join("english/cpu_names.tsv"),
        format!(
            "file\toriginal_sha256\tenglish\titaliano\ncpu_1.dat\t{h}\tx\t\ncpu_2.dat\t{h}\ty\t\n",
            h = sha(orig)
        ),
    )
    .unwrap();
    kit
}

#[test]
fn an_older_folder_turning_off_keeps_the_originals_a_newer_one_left_beside_the_game() {
    let (t, orig, game, _) = translated_game(false);
    fs::write(game.join(TEAM), &orig).unwrap();
    fs::write(game.join(TEAM2), &orig).unwrap();
    // B (older, pins TEAM only) turns English on first and keeps its backup beside the game
    let (kit_b, bak_b) = (t.path().join("kit"), t.path().join("b/backup"));
    let b = Ctx::new(Edition::E1718, &game, &bak_b, &kit_b).unwrap();
    b.turn_on(&[(TEAM.to_string(), vec![9u8; 64])], Lang::En)
        .unwrap();
    b.save_beside().unwrap();
    // A (newer, pins both) takes it, switches both and keeps the bigger backup there; then A is deleted
    let (kit_a, bak_a) = (
        bigger_kit(t.path(), "kit_a", &orig),
        t.path().join("a/backup"),
    );
    assert_eq!(
        sync_beside(Edition::E1718, &game, &bak_a, &kit_a),
        Beside::Fine
    );
    let a = Ctx::new(Edition::E1718, &game, &bak_a, &kit_a).unwrap();
    a.turn_on(
        &[
            (TEAM.to_string(), vec![9u8; 64]),
            (TEAM2.to_string(), vec![9u8; 64]),
        ],
        Lang::En,
    )
    .unwrap();
    a.save_beside().unwrap();
    fs::remove_dir_all(t.path().join("a")).unwrap();
    // B turns the text off: its file back, A's original of TEAM2 still beside the game
    let say: crate::Say = &|_, _| {};
    assert_eq!(
        run(
            Edition::E1718,
            &game,
            &bak_b,
            &kit_b,
            Mode::Off,
            Lang::En,
            say
        ),
        0
    );
    assert_eq!(fs::read(game.join(TEAM)).unwrap(), orig);
    assert_eq!(fs::read(beside(&game).join(TEAM2)).unwrap(), orig);
    // B turning on again does not drop TEAM2 from the list beside the game
    b.turn_on(&[(TEAM.to_string(), vec![9u8; 64])], Lang::En)
        .unwrap();
    b.save_beside().unwrap();
    assert!(
        read_manifest(&beside(&game).join("manifest.json"))
            .unwrap()
            .contains_key(&key(TEAM2))
    );
    // a fresh folder with A's sheets takes both and puts TEAM2 back
    let (kit_c, bak_c) = (
        bigger_kit(t.path(), "kit_c", &orig),
        t.path().join("c/backup"),
    );
    assert_eq!(
        sync_beside(Edition::E1718, &game, &bak_c, &kit_c),
        Beside::Fine
    );
    let c = Ctx::new(Edition::E1718, &game, &bak_c, &kit_c).unwrap();
    assert_eq!(c.turn_off().unwrap(), 2);
    assert_eq!(fs::read(game.join(TEAM2)).unwrap(), orig);
}

#[test]
fn a_key_this_folder_does_not_pin_is_skipped_and_the_rest_taken() {
    let (t, orig, game, backup) = translated_game(true);
    let b = beside(&game);
    let mut man = read_manifest(&b.join("manifest.json")).unwrap();
    man.insert(
        key(TEAM2),
        serde_json::json!({"original": "00", "english": "11"}),
    );
    fs::write(b.join("manifest.json"), serde_json::to_vec(&man).unwrap()).unwrap();
    fs::write(b.join(TEAM2), vec![5u8; 64]).unwrap();
    let kit = t.path().join("kit");
    assert_eq!(
        sync_beside(Edition::E1718, &game, &backup, &kit),
        Beside::Fine
    );
    let c = Ctx::new(Edition::E1718, &game, &backup, &kit).unwrap();
    let got = c.load_manifest().unwrap();
    assert!(got.contains_key(&key(TEAM)) && !got.contains_key(&key(TEAM2)));
    assert_eq!(fs::read(backup.join(TEAM)).unwrap(), orig);
    assert!(!backup.join(TEAM2).exists());
}

#[test]
fn a_pictures_state_that_does_not_match_the_archive_is_not_taken() {
    let (t, _, game, backup) = translated_game(true);
    let b = beside(&game);
    // an archive as the original: its one entry points inside the original length
    let mut xaf = vec![0u8; 4096];
    xaf[..4].copy_from_slice(b"xaf0");
    fs::create_dir_all(game.join(pictures::XAF).parent().unwrap()).unwrap();
    fs::write(game.join(pictures::XAF), &xaf).unwrap();
    let state = |total: &str| {
        let h = format!("{total:0<48}");
        let st =
            serde_json::json!({"length": 2048, "header": h, "entries": {"0": "00".repeat(0xB0)}});
        fs::write(b.join(pictures::STATE), serde_json::to_vec(&st).unwrap()).unwrap();
    };
    state("01"); // a header of one sector: 2048 bytes
    let kit = t.path().join("kit");
    assert_eq!(
        sync_beside(Edition::E1718, &game, &backup, &kit),
        Beside::Fine
    );
    assert!(
        !backup.join(pictures::STATE).exists(),
        "foreign state left out"
    );
    // the same state once the entry points at pictures past the original length: taken
    fs::remove_dir_all(&backup).unwrap();
    xaf[0x100 + 0xA0..0x100 + 0xA4].copy_from_slice(&1u32.to_le_bytes());
    fs::write(game.join(pictures::XAF), &xaf).unwrap();
    state("02"); // its header disagrees with its length: restore would break the archive
    assert_eq!(
        sync_beside(Edition::E1718, &game, &backup, &kit),
        Beside::Fine
    );
    assert!(!backup.join(pictures::STATE).exists());
    fs::remove_dir_all(&backup).unwrap();
    state("01");
    assert_eq!(
        sync_beside(Edition::E1718, &game, &backup, &kit),
        Beside::Fine
    );
    assert!(backup.join(pictures::STATE).is_file());
}
/// The check mode for a player's own original 2017-18 game: `FORZA_GAME=<game folder> FORZA_SHEETS=<editions/
/// 2017-18> cargo nextest run -p forza --run-ignored all -E 'test(the_real_text_has_no_line_that_gives_way)'`.
/// A line that does not fit its place, a line that stays Japanese, and a player name that is cut or that gets a
/// `?` all fail here.
#[test]
#[ignore]
fn the_real_text_has_no_line_that_gives_way() {
    let game = std::path::PathBuf::from(std::env::var("FORZA_GAME").unwrap());
    let sheets = std::path::PathBuf::from(std::env::var("FORZA_SHEETS").unwrap());
    let c = Ctx::new(Edition::E1718, &game, Path::new("none"), &sheets).unwrap();
    for lang in [Lang::En, Lang::It] {
        let (_, notes) = c.build_all(lang).unwrap();
        let note = notes
            .iter()
            .map(|l| note_words("en", l))
            .collect::<Vec<_>>()
            .join("\n");
        println!("{}: {note}", lang.name());
        for bad in [
            "do not fit",
            "still Japanese",
            "too long for the field",
            "cannot show",
        ] {
            assert!(!note.contains(bad), "{}: {note}", lang.name());
        }
    }
}

impl Ctx {
    fn new_bare(ed: Edition, game: &Path, backup: &Path) -> Self {
        Self {
            ed,
            spec: spec(ed),
            game: game.into(),
            backup: backup.into(),
            sheets: PathBuf::new(),
            cpu: vec![],
        }
    }
}
