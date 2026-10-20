//! The Rust panel in a real Direct3D 9 window: `fakegame.exe` and `forza_panel.dll` (both from `cargo xtask build`) in a
//! scratch folder with the fonts, the panel's frame dumps checked (the window's size, the formation board's pitch, the
//! game's own frame in the picture, a page opened by a click), then the DLL unloaded while the frames go on, and the
//! game's device as it left it on every frame.
#![cfg(windows)]

use std::path::Path;
use std::process::Command;
use std::{env, fs};

const BUILD: &str = "../../target/kit/2010-11/source/build";

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
}

/// How many pixels of `rect` (x0, y0, x1, y1) are within 2 of one of `colours`.
fn count(
    img: &image::RgbImage,
    (x0, y0, x1, y1): (u32, u32, u32, u32),
    colours: &[[u8; 3]],
) -> u32 {
    let near =
        |p: &image::Rgb<u8>, c: &[u8; 3]| p.0.iter().zip(c).all(|(a, b)| a.abs_diff(*b) <= 2);
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            n += u32::from(colours.iter().any(|c| near(img.get_pixel(x, y), c)));
        }
    }
    n
}

#[test]
fn panel_draws_in_fakegame() {
    for f in ["fakegame.exe", "forza_panel.dll"] {
        assert!(
            Path::new(BUILD).join(f).is_file(),
            "{BUILD}/{f} is missing: run cargo xtask build first"
        );
    }
    let t = env::temp_dir().join(format!("forza-panel-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(&t).unwrap();
    for f in ["fakegame.exe", "forza_panel.dll"] {
        fs::copy(Path::new(BUILD).join(f), t.join(f)).unwrap();
    }
    copy_dir(Path::new("../../assets/fonts"), &t.join("fonts")); // the kit's layout: fonts/ beside the programs

    // KEYS on the bar of page buttons (forza::ui::panel: the middle's bottom 60 points), at 1440x900 one point a pixel
    let out = Command::new(t.join("fakegame.exe"))
        .current_dir(&t)
        .env("WCCFPANEL_FULLSCREEN", "0") // never maximized over the player's screen
        .env("FORZA_PANEL_DUMP", "1")
        .env("WCCF_DATA", t.join("data")) // no data: the panel's empty board, nothing read from above the folder
        .args([
            "forza_panel.dll",
            "wait:3000",
            "copy:game.bmp",
            "click:336,870",
            "size:1280,800",
            "wait:1500",
            "copy:keys.bmp",
            "unload",
            "wait:1000",
        ])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    let log = fs::read_to_string(t.join("forza_panel.log")).unwrap_or_default();
    println!("fakegame:\n{said}\nforza_panel.log:\n{log}");
    assert!(out.status.success(), "fakegame ended with {}", out.status);
    assert!(said.contains("unloaded the panel"), "no unload");
    assert!(
        !said.contains("device changed"),
        "the panel left the game's device changed"
    );
    for line in [
        "Present detoured",
        "frame dump 1440x900",
        "frame dump 1280x800",
        "unloaded",
    ] {
        assert!(log.contains(line), "the panel's log has no '{line}'");
    }

    let game = image::open(t.join("game.bmp")).unwrap().to_rgb8();
    assert_eq!(game.dimensions(), (1440, 900));
    // the formation board's pitch (theme::PITCH, PITCH_STRIPE) fills much of the right 580 pixels
    let pitch = count(
        &game,
        (860, 0, 1440, 900),
        &[[0x1E, 0x7A, 0x3C], [0x22, 0x8A, 0x43]],
    );
    assert!(
        pitch > 580 * 900 / 4,
        "only {pitch} pitch pixels in the board strip"
    );
    // the game's frame (fakegame's lighter band in its middle third) is the picture, top centre of the middle
    let band = [[34, 120, 58]];
    let seen = count(&game, (330, 230, 530, 310), &band);
    assert!(
        seen > 200 * 80 * 9 / 10,
        "the game's frame is not in the picture ({seen} band pixels)"
    );

    // the click opened KEYS over the picture; the dump follows the window's new size
    let keys = image::open(t.join("keys.bmp")).unwrap().to_rgb8();
    assert_eq!(keys.dimensions(), (1280, 800));
    let seen = count(&keys, (290, 200, 470, 280), &band);
    assert!(
        seen < 180 * 80 / 10,
        "KEYS did not open over the picture ({seen} band pixels left)"
    );
    let _ = fs::remove_dir_all(&t);
}

/// The panel with real data in a scratch kit: a synthetic catalogue (the real column layout, made-up cards) and
/// made-up pictures, keys.txt, panel.txt and settings.json, the card table in the game's folder. Checked: a card's
/// picture on its card, the heat zone while a card is dragged, a drag from the bench to the pitch and a right-click
/// written to fpr_table0.txt as the C panel writes it, a cabinet button pressing its key from keys.txt (dry run), and
/// a click on the game's picture reaching the game, moved into its client area, while clicks on the panel do not.
#[test]
#[cfg(target_arch = "x86")] // forza is the 32-bit panel's own dependency
fn board_and_buttons_with_real_data() {
    use forza::cards;
    use forza::ui::panel;

    for f in ["fakegame.exe", "forza_panel.dll"] {
        assert!(
            Path::new(BUILD).join(f).is_file(),
            "{BUILD}/{f} is missing: run cargo xtask build first"
        );
    }
    let t = env::temp_dir().join(format!("forza-panel-data-{}", std::process::id()));
    let _ = fs::remove_dir_all(&t);
    // the 2010-11 kit's layout: programs\ holds the panel, data\ beside it, the game's own folder seat1\
    for d in ["programs", "data/cache/cards", "seat1"] {
        fs::create_dir_all(t.join(d)).unwrap();
    }
    fs::copy(
        Path::new(BUILD).join("forza_panel.dll"),
        t.join("programs/forza_panel.dll"),
    )
    .unwrap();
    fs::copy(
        Path::new(BUILD).join("fakegame.exe"),
        t.join("seat1/fakegame.exe"),
    )
    .unwrap();
    copy_dir(Path::new("../../assets/fonts"), &t.join("fonts"));
    let data = t.join("data");
    fs::write(data.join("panel.txt"), "hover_delay_ms=300\n").unwrap();
    fs::write(data.join("keys.txt"), "SHOOT=0x47\n").unwrap();
    fs::write(data.join("settings.json"), "{\"language\": \"it\"}").unwrap();
    // made-up cards in the catalogue's columns; 103 a striker by its 16 role ratings (CF 10, SS 8, TM 7)
    let head = "card_no\trecord_index\tset_no\tedition\tedition_label\tseason\trarity\trarity_name\trarity_frame\t\
                is_placeholder\tname_full_latin\tname_short_latin\tname_display_ascii\tname_full_kana\tname_short_kana\t\
                position_name\tclub_id\tclub_name\tnationality_id\tnationality_name\tshirt_numbers\tbirth_date\t\
                height_cm\tweight_kg\tstat_offence\tstat_defence\tstat_technique\tstat_power\tstat_speed\tstat_stamina\t\
                stats_total\tskill_en\tskill_jp\tmodel_key\timage_archive_path\timage_xaf_offset\t\
                image_xaf_stored_size\timage_size\timage_packed\talt_png_2005_06_build\thidden_params\n";
    let row = |no: u32, name: &str, pos: &str, roles: &str| {
        format!(
            "{no}\t{}\t1\t1\tx\t10-11\t4\tLEGEND\tGold\t0\t{name} Test\t{name}\t{name}\t\t\t{pos}\t1\tNowhere FC\t1\t\
             Nowhere\t9\t1980-01-01\t180\t75\t12\t12\t12\t12\t12\t12\t72\t\t\t\t\t0\t0\t0\t0\t\t{roles} 5 5\n",
            no - 101 // its record in the made-up player_data.bin
        )
    };
    let low = "1 1 1 1 1 1 1 1 1 1 1 1 1 1 1";
    fs::write(
        data.join("cache/catalogue.tsv"),
        format!(
            "{head}{}{}{}{}{}",
            row(101, "KEEPA", "GK", &format!("{low} 10")),
            row(102, "BACKA", "DF", "1 1 1 1 1 1 1 4 1 7 9 10 8 8 7 1"),
            row(103, "STRIKA", "FW", "10 7 4 8 3 2 1 1 2 1 1 1 1 1 1 1"),
            // a white and a black card, for the deal's every tier
            row(104, "WHITA", "MF", &format!("{low} 1")).replace("\t4\tLEGEND\t", "\t1\tREGULAR\t"),
            row(105, "BLACKA", "MF", &format!("{low} 1"))
                .replace("\t4\tLEGEND\t", "\t2\tSPECIAL\t"),
        ),
    )
    .unwrap();
    // card 101's picture: plain magenta, plainly not a photo
    let magenta = image::Rgba([255u8, 0, 255, 255]);
    for (no, c) in [(101, magenta), (102, image::Rgba([0, 255, 255, 255]))] {
        image::RgbaImage::from_pixel(128, 128, c)
            .save(data.join(format!("cache/cards/{no}.png")))
            .unwrap();
    }
    let seat = t.join("seat1");
    // links (synthetic, the 2010-11 layouts): each card its own player (key at record +0x177), 101 and 103 listed at
    // affinity 5; the club card holds 101, 102, 103 in slots 0-2 with a thin link (160) between 101 and 102
    let pd = seat.join("prog_data/player_data");
    fs::create_dir_all(&pd).unwrap();
    let mut players = vec![0u8; 0x328 * 5];
    for k in 0..5 {
        players[k * 0x328 + 0x177..][..3].copy_from_slice(format!("K{k}0").as_bytes());
    }
    fs::write(pd.join("player_data.bin"), players).unwrap();
    let mut pair = vec![0u8; 129];
    pair[..3].copy_from_slice(b"K00");
    pair[64..67].copy_from_slice(b"K20");
    pair[128] = 5;
    fs::write(pd.join("partnership.bin"), pair).unwrap();
    fs::create_dir_all(data.join("save")).unwrap();
    fs::write(
        data.join("save/seat1_club.bin"),
        club_card(&[101, 102, 103], 160),
    )
    .unwrap();
    let table = seat.join("fpr_table0.txt");
    let start = [
        cards::Placed {
            slot: 1,
            no: 101,
            x: 0.0,
            y: 0.9,
        },
        cards::Placed {
            slot: 2,
            no: 102,
            x: -0.6,
            y: 0.55,
        },
        cards::Placed {
            slot: 3,
            no: 103,
            x: 1.24,
            y: -0.68,
        },
    ];
    fs::write(&table, cards::table_text(&start)).unwrap();
    let spots = cards::board(&start, &[]).spots;
    let at = |slot: usize| {
        let p = panel::board_slot_center(1440.0, &spots, slot);
        format!("{},{}", p.x as i32, p.y as i32)
    };
    let run_with = |env: &[(&str, &str)], steps: &[String]| {
        let out = Command::new(seat.join("fakegame.exe"))
            .env("WCCFPANEL_FULLSCREEN", "0")
            .envs(env.iter().copied())
            .current_dir(&seat)
            .env("FORZA_PANEL_DUMP", "1")
            .env("WCCFPANEL_DRYKEYS", "1")
            .env_remove("WCCF_KIT_LANG")
            .arg("../programs/forza_panel.dll")
            .args(steps)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout).into_owned();
        println!("fakegame:\n{said}");
        assert!(out.status.success(), "fakegame ended with {}", out.status);
        said
    };
    let run = |steps: &[String]| run_with(&[], steps);

    // 1. the bench card (slot 11) dragged onto the pitch where 4-4-2 has a free spot (-0.2, 0.6); it lies where dropped
    let mid = panel::board_slot_center(1440.0, &spots, 11)
        .lerp(panel::board_slot_center(1440.0, &spots, 2), 0.5);
    let said = run(&[
        "wait:3000".into(),
        "copy:start.bmp".into(),
        format!("down:{}", at(11)),
        format!("move:{},{}", mid.x as i32, mid.y as i32),
        format!("move:{}", at(2)),
        "wait:700".into(),
        "copy:drag.bmp".into(),
        format!("up:{}", at(2)),
        "wait:500".into(),
    ]);
    let log = fs::read_to_string(seat.join("forza_panel.log")).unwrap_or_default();
    println!("forza_panel.log:\n{log}");
    assert!(
        !said.contains("game:"),
        "a press on the board reached the game"
    );
    for line in [
        "edition 2010–11",
        "catalogue 5 cards",
        "board: table",
        "language it, hover delay 0.3 s",
        "links: affinity 1 listed pair(s) of 5 player(s), default rule off",
        "- 3 pair(s), 1 drawn (150+), 0 golden",
    ] {
        assert!(log.contains(line), "the panel's log has no '{line}'");
    }
    let header =
        "# written by the in-game card board (wccfpanel.dll) - read by FPR_Emu.exe (table 0)\n";
    // free placement: the card lies where it was shown when let go - on the pitch, near the empty 4-4-2 spot
    let after = fs::read_to_string(&table).unwrap();
    let l103 = after
        .lines()
        .find(|l| l.starts_with("3 103 "))
        .unwrap_or_else(|| panic!("card 103 is not in the table:\n{after}"))
        .to_owned();
    let xy: Vec<f32> = l103
        .split(' ')
        .skip(2)
        .map(|v| v.parse().unwrap())
        .collect();
    assert!(
        (xy[0] + 0.2).abs() < 0.2 && (xy[1] - 0.6).abs() < 0.2,
        "card 103 landed at {xy:?}, not near -0.2, 0.6"
    );
    assert_eq!(
        after,
        format!("{header}1 101 0.000 0.900\n2 102 -0.600 0.550\n{l103}\n")
    );
    assert_eq!(
        fs::read_to_string(seat.join("fpr_panel_state.json")).unwrap(),
        format!(
            "{{\n \"placed\": [\n  {{\"slot\": 1, \"no\": 101, \"x\": 0.000, \"y\": 0.900}},\n  \
             {{\"slot\": 2, \"no\": 102, \"x\": -0.600, \"y\": 0.550}},\n  \
             {{\"slot\": 3, \"no\": 103, \"x\": {:.3}, \"y\": {:.3}}}\n ],\n \"mirror\": false\n}}\n",
            xy[0], xy[1]
        )
    );
    // card 101's magenta picture on its card (slot 0, the keeper's spot)
    let start_img = image::open(seat.join("start.bmp")).unwrap().to_rgb8();
    let k = panel::board_slot_center(1440.0, &spots, 0);
    let (kx, ky) = (k.x as u32, k.y as u32);
    let pink = count(
        &start_img,
        (kx - 40, ky - 55, kx + 40, ky + 55),
        &[[255, 0, 255]],
    );
    assert!(
        pink > 1500,
        "card 101's picture is not on its card ({pink} pixels)"
    );
    // the striker in hand: the heat zone (amber to pale yellow) over the pitch, little of it at rest
    let heat = |img: &image::RgbImage| {
        let mut n = 0;
        for y in 50..740 {
            for x in 890..1424 {
                let [r, g, b] = img.get_pixel(x, y).0;
                n += u32::from(r > 170 && g > 100 && r >= g && r > b.saturating_add(40));
            }
        }
        n
    };
    let drag_img = image::open(seat.join("drag.bmp")).unwrap().to_rgb8();
    let (rest, held) = (heat(&start_img), heat(&drag_img));
    assert!(
        held > rest + 8000,
        "no heat zone while dragging: {rest} at rest, {held} held"
    );

    // 2. a right-click takes card 102 off; SHOOT (keys.txt: G) presses its key; a click on the picture is the game's
    let _ = fs::remove_file(seat.join("forza_panel.log"));
    // SHOOT's centre from the panel's own layout at 1440x900 (one point per pixel), so the click follows the deck
    let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0));
    let deck = panel::layout(sheet, false).deck;
    let shoot = panel::deck_layout(deck) // the deck row under the picture, as show() has it
        .into_iter()
        .find(|it| it.action == "SHOOT")
        .expect("the deck has SHOOT")
        .rect
        .center();
    let said = run(&[
        "wait:2500".into(),
        format!("rclick:{}", at(1)),
        "wait:300".into(),
        format!("click:{},{}", shoot.x as i32, shoot.y as i32), // SHOOT, in the cabinet deck under the picture
        "wait:400".into(),
        "click:430,268".into(), // the middle of the picture (0..860 x 0..537.5)
        "wait:300".into(),
    ]);
    let log = fs::read_to_string(seat.join("forza_panel.log")).unwrap_or_default();
    println!("forza_panel.log:\n{log}");
    assert_eq!(
        fs::read_to_string(&table).unwrap(),
        format!("{header}1 101 0.000 0.900\n{l103}\n")
    );
    for line in [
        "button SHOOT -> key 0x47",
        "key 0x47 down (dry run - not pressed)",
        "key 0x47 up (dry run - not pressed)",
        "click at 430,268 -> the game at 720,448",
    ] {
        assert!(log.contains(line), "the panel's log has no '{line}'");
    }
    // the game saw the picture's click, moved into its 1440x900 client area, and nothing else
    assert_eq!(
        said.lines()
            .filter(|l| l.starts_with("game:"))
            .collect::<Vec<_>>(),
        ["game: left down at 720,448", "game: left up at 720,448"]
    );

    // 3. CATALOGUE from the bar (+ CARD), a typed search, Enter: the first card found goes on the table where the C
    // panel puts it (the free DF spot, the lowest free slot); Esc closes the page
    let _ = fs::remove_file(seat.join("forza_panel.log"));
    run(&[
        "wait:2500".into(),
        "click:206,870".into(),
        "wait:400".into(),
        "type:backa".into(),
        "wait:400".into(),
        "key:13".into(),
        "wait:400".into(),
        "key:27".into(),
        "wait:400".into(),
    ]);
    let log = fs::read_to_string(seat.join("forza_panel.log")).unwrap_or_default();
    println!("forza_panel.log:\n{log}");
    for line in [
        "page catalogue",
        "catalogue: card 102 added - 3 card(s)",
        "page game",
    ] {
        assert!(log.contains(line), "the panel's log has no '{line}'");
    }
    assert_eq!(
        fs::read_to_string(&table).unwrap(),
        format!("{header}1 101 0.000 0.900\n{l103}\n2 102 -0.600 0.550\n")
    );

    // 4. KEYS from the bar: SHOOT's key cell clicked, then H; KEEPER's controller cell clicked, then a (fake)
    // controller's button 3 pressed after the controller was found at rest; keys.txt written as the C panel writes it
    let _ = fs::remove_file(seat.join("forza_panel.log"));
    let pad = t.join("pad.txt");
    let at_rest = "0 0 2 0 32767 32767 0 0 0 0 65535 Test Pad\n";
    fs::write(&pad, at_rest).unwrap();
    let game = Command::new(seat.join("fakegame.exe"))
        .env("WCCFPANEL_FULLSCREEN", "0")
        .current_dir(&seat)
        .env("WCCFPANEL_FAKEPAD", &pad)
        .env_remove("WCCF_KIT_LANG")
        .arg("../programs/forza_panel.dll")
        .args([
            "wait:2500",
            "click:336,870", // KEYS
            "wait:600",
            "click:335,300", // SHOOT's key
            "wait:300",
            "key:72", // H
            "wait:300",
            "click:551,335", // KEEPER's controller cell
            "wait:6000",
            "key:27",
            "wait:400",
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // the button goes down once H has been taken and the controller cell clicked (300 ms later)
    for _ in 0..200 {
        let log = fs::read_to_string(seat.join("forza_panel.log")).unwrap_or_default();
        if log.contains("keys: key 0x48") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::thread::sleep(std::time::Duration::from_millis(2000));
    fs::write(&pad, at_rest.replacen("0 0 2", "0 4 2", 1)).unwrap(); // button 3 down
    let out = game.wait_with_output().unwrap();
    assert!(out.status.success(), "fakegame ended with {}", out.status);
    let log = fs::read_to_string(seat.join("forza_panel.log")).unwrap_or_default();
    println!("forza_panel.log:\n{log}");
    for line in [
        "page keys",
        "pads: FAKE controllers",
        "controller found: Test Pad",
        "keys: controller Button 3",
        "keys: saved",
        "page game",
    ] {
        assert!(log.contains(line), "the panel's log has no '{line}'");
    }
    let mut keys = forza::keys::defaults(false);
    keys[2] = 0x48;
    let mut pads = [0; forza::keys::COUNT];
    pads[3] = 0x103;
    assert_eq!(
        fs::read_to_string(data.join("keys.txt")).unwrap(),
        forza::keys::keys_text(false, &keys, &pads)
    );

    // 5. SETTINGS from the bar: COMPACT and a 2 s hover written to data\panel.txt as the C panel writes it; RESTART
    // NOW clicked twice writes data\restart.request, which no run takes here: after 3 s it is taken back and said
    let _ = fs::remove_file(seat.join("forza_panel.log"));
    run(&[
        "wait:2500".into(),
        "click:466,870".into(), // SETTINGS
        "wait:1500".into(),
        "click:321,559".into(), // COMPACT
        "click:275,647".into(), // 2 s
        "wait:300".into(),
        "click:632,392".into(), // RESTART NOW
        "wait:300".into(),
        "click:632,392".into(), // again
        "wait:4000".into(),
        "key:27".into(),
        "wait:400".into(),
    ]);
    let log = fs::read_to_string(seat.join("forza_panel.log")).unwrap_or_default();
    println!("forza_panel.log:\n{log}");
    for line in [
        "page settings",
        "settings: saved play_on=this_pc server=- english=- layout=compact hover=2000 auto_logs=on",
        "settings: restart requested",
        "settings: restart.request not picked up - removed",
        "page game",
    ] {
        assert!(log.contains(line), "the panel's log has no '{line}'");
    }
    let want = forza::panel_txt::PanelFile {
        compact: true,
        hover_ms: 2000,
        ..Default::default()
    };
    assert_eq!(
        fs::read_to_string(data.join("panel.txt")).unwrap(),
        want.text(false)
    );
    assert!(!data.join("restart.request").exists());

    // 6. a card dealt (WCCFPANEL_DEALTEST: at once, as after a match) over the picture: a click on the card turns it
    // over, a click beside it closes the window; neither reaches the game
    let _ = fs::remove_file(seat.join("forza_panel.log"));
    let said = run_with(
        &[("WCCFPANEL_DEALTEST", "1")],
        &[
            "wait:2500".into(),
            "copy:deal.bmp".into(),
            "click:430,300".into(), // the card (forza::ui::panel's deal window at 1440x900)
            "wait:400".into(),
            "click:430,700".into(), // under it
            "wait:400".into(),
        ],
    );
    let log = fs::read_to_string(seat.join("forza_panel.log")).unwrap_or_default();
    println!("forza_panel.log:\n{log}");
    assert!(log.contains("deal: card 10"), "no card dealt");
    assert!(
        log.contains("deal: closed"),
        "the click did not close the deal"
    );
    assert!(
        !said.contains("game: left down"),
        "the click went through to the game"
    );
    if env::var_os("FORZA_PANEL_KEEP").is_none() {
        let _ = fs::remove_dir_all(&t);
    }
}

/// A club card file with `cards` in its first squad slots (PLAYER_CARD_NO, 14 bits from payload bit 6688 + 312 i)
/// and the link of slots 0 and 1 (pair 0 of CLUB_PLAYER_COOPERATION_INFO, 8 bits from bit 3650); both copies good.
#[cfg(target_arch = "x86")]
fn club_card(cards: &[u32], link: u8) -> Vec<u8> {
    let mut raw = vec![0u8; 16 + 256 * 16];
    let put = |buf: &mut [u8], start: usize, width: usize, v: u32| {
        for b in 0..width {
            if (v >> (width - 1 - b)) & 1 == 1 {
                buf[(start + b) >> 3] |= 0x80 >> ((start + b) & 7);
            }
        }
    };
    for start in [16 + 8 * 16, 16 + 8 * 16 + 0x690] {
        let copy = &mut raw[start..start + 0x690];
        for (i, &c) in cards.iter().enumerate() {
            put(copy, 6688 + 312 * i, 14, c);
        }
        put(copy, 3650, 8, u32::from(link));
        let sum = copy[..0x68C].chunks(4).fold(0u32, |t, w| {
            t.wrapping_add(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        });
        copy[0x68C..].copy_from_slice(&sum.to_le_bytes());
    }
    raw
}

/// A club card file whose session is open (the flag set in both copies, their checksums good): launcher::card's
/// layout (a 16-byte header, copies A and B of 0x690 bytes from block 8, USER_INJUSTICE_FLAG at bit 595).
fn open_card() -> Vec<u8> {
    let mut raw = vec![0u8; 16 + 256 * 16];
    for start in [16 + 8 * 16, 16 + 8 * 16 + 0x690] {
        raw[start + 595 / 8] |= 0x80 >> (595 % 8);
        let sum = raw[start..start + 0x68C].chunks(4).fold(0u32, |t, w| {
            t.wrapping_add(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        });
        raw[start + 0x68C..start + 0x690].copy_from_slice(&sum.to_le_bytes());
    }
    raw
}

/// The window's own behaviour, in a scratch kit: maximized at the start onto a pretend screen (WCCFPANEL_FS_RECT, off
/// every monitor) and F11 back to the game's size; during a card session the first close only warns and a second
/// within 8 s closes; the server link (the panel's own test link to a port of the test's, WCCFPANEL_LINKTEST) written
/// UP to WCCF_LOGS\server_link.txt and, dropped after the player's close, ENDED - not LOST.
#[test]
fn window_close_and_server_link() {
    for f in ["fakegame.exe", "forza_panel.dll"] {
        assert!(
            Path::new(BUILD).join(f).is_file(),
            "{BUILD}/{f} is missing: run cargo xtask build first"
        );
    }
    let t = env::temp_dir().join(format!("forza-panel-close-{}", std::process::id()));
    let _ = fs::remove_dir_all(&t);
    for d in ["programs", "data/save", "seat1", "logs"] {
        fs::create_dir_all(t.join(d)).unwrap();
    }
    fs::copy(
        Path::new(BUILD).join("forza_panel.dll"),
        t.join("programs/forza_panel.dll"),
    )
    .unwrap();
    fs::copy(
        Path::new(BUILD).join("fakegame.exe"),
        t.join("seat1/fakegame.exe"),
    )
    .unwrap();
    copy_dir(Path::new("../../assets/fonts"), &t.join("fonts"));
    fs::write(t.join("data/save/seat1_club.bin"), open_card()).unwrap();
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    let started = std::time::Instant::now();
    // the link: accepted, held until after the player's second close (8.2 s), then dropped
    let link = std::thread::spawn(move || {
        let (conn, _) = server.accept().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(10).saturating_sub(started.elapsed()));
        drop(conn);
    });
    let seat = t.join("seat1");
    let out = Command::new(seat.join("fakegame.exe"))
        .current_dir(&seat)
        .env("WCCFPANEL_FS_RECT", "-4000,-4000,1700,1000")
        .env("WCCF_LOGS", t.join("logs"))
        .env("WCCFPANEL_CONTROL_PORT", port.to_string())
        .env("WCCFPANEL_LINKTEST", format!("127.0.0.1:{port}"))
        .env("WCCFPANEL_DRYKEYS", "1")
        .env_remove("WCCF_KIT_LANG")
        .args([
            "../programs/forza_panel.dll",
            "wait:2500",
            "key:122", // F11
            "wait:1000",
            "close",
            "wait:1500",
            "close",
            "wait:6000",
        ])
        .output()
        .unwrap();
    link.join().unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    let log = fs::read_to_string(seat.join("forza_panel.log")).unwrap_or_default();
    let links = fs::read_to_string(t.join("logs/server_link.txt")).unwrap_or_default();
    println!("fakegame:\n{said}\nforza_panel.log:\n{log}\nserver_link.txt:\n{links}");
    assert!(out.status.success(), "fakegame ended with {}", out.status);
    for line in [
        "maximized: client now ",
        "window: client now 1440x900",
        "close: a card session is open - warned, not closed",
        "close: closed again during the card session - the game ends",
    ] {
        assert!(log.contains(line), "the panel's log has no '{line}'");
    }
    let lines: Vec<&str> = links.lines().collect();
    assert_eq!(lines.len(), 2, "{links}");
    assert!(lines[0].ends_with(" UP 127.0.0.1"), "{links}");
    assert!(lines[1].contains(" ENDED (it was up "), "{links}");
    if env::var_os("FORZA_PANEL_KEEP").is_none() {
        let _ = fs::remove_dir_all(&t);
    }
}

/// The projector (WCCFPANEL_ROLE=projector): no panel drawn - its Present copies the frame for seat 1's PROJECTOR - money with commas
/// and the frame wait, which check that they are in the real game and stay off in fakegame; its log goes to
/// WCCF_LOGS\forza_panel_projector.log. The money display's self-test (WCCFPANEL_MONEYTEST: every place's role with
/// the game's arguments faked, amounts with thousands separators cut to their boxes) passes there.
#[test]
fn projector_role_and_money() {
    for f in ["fakegame.exe", "forza_panel.dll"] {
        assert!(
            Path::new(BUILD).join(f).is_file(),
            "{BUILD}/{f} is missing: run cargo xtask build first"
        );
    }
    let t = env::temp_dir().join(format!("forza-panel-projector-{}", std::process::id()));
    let _ = fs::remove_dir_all(&t);
    for d in ["programs", "data", "extracted", "logs"] {
        fs::create_dir_all(t.join(d)).unwrap();
    }
    fs::copy(
        Path::new(BUILD).join("forza_panel.dll"),
        t.join("programs/forza_panel.dll"),
    )
    .unwrap();
    fs::copy(
        Path::new(BUILD).join("fakegame.exe"),
        t.join("extracted/fakegame.exe"),
    )
    .unwrap();
    let game = t.join("extracted");
    let out = Command::new(game.join("fakegame.exe"))
        .current_dir(&game)
        .env("WCCFPANEL_FULLSCREEN", "0")
        .env_remove("WCCFPANEL_ROLE") // the folder named "extracted" says it, as in the 2010-11 kit
        .env("WCCF_LOGS", t.join("logs"))
        .env("WCCFPANEL_MONEYTEST", "1")
        .args(["../programs/forza_panel.dll", "wait:3000"])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    let log = fs::read_to_string(t.join("logs/forza_panel_projector.log")).unwrap_or_default();
    println!("fakegame:\n{said}\nforza_panel_projector.log:\n{log}");
    assert!(out.status.success(), "fakegame ended with {}", out.status);
    assert!(log.contains("the PROJECTOR"), "not taken for the projector");
    assert!(
        log.contains("money: off - not the game (fakegame.exe)"),
        "money not off"
    );
    let tests = log
        .lines()
        .find(|l| l.contains("money test: ") && l.contains(" checks, "));
    assert!(
        tests.is_some_and(|l| l.ends_with(" checks, 0 failed")),
        "money test: {tests:?}"
    );
    assert!(!log.contains("FAIL"), "a money test failed");
    // its frames go to seat 1's PROJECTOR through the shared section; no panel drawn, no seat log
    assert!(
        log.contains("projector: frames shared ") || said.contains("no Direct3D 9 device"),
        "no frames shared"
    );
    assert!(!log.contains("first frame shown") && !game.join("forza_panel.log").exists());
    assert!(
        !log.contains("window moved off-screen"),
        "moved with no seat 1 reading"
    );
    assert!(!said.contains("device changed"));
    if env::var_os("FORZA_PANEL_KEEP").is_none() {
        let _ = fs::remove_dir_all(&t);
    }
}

/// Pause (WCCFPANEL_PAUSETEST=1500: the gate passed with made-up facts, each pause 1.5 s instead of the 20 s cap): P
/// holds the game inside its Present while the panel draws the PAUSED page and pumps the window's messages (the key's
/// release arrives during the hold and is kept from the game), then the game goes on; P again within 10 s is refused,
/// and the game's device is as it left it. The window's Move command during the hold (which would enter Windows'
/// own move loop) does not hold it past its cap.
#[test]
fn pause_holds_the_game() {
    for f in ["fakegame.exe", "forza_panel.dll"] {
        assert!(
            Path::new(BUILD).join(f).is_file(),
            "{BUILD}/{f} is missing: run cargo xtask build first"
        );
    }
    let t = env::temp_dir().join(format!("forza-panel-pause-{}", std::process::id()));
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(t.join("data")).unwrap();
    for f in ["fakegame.exe", "forza_panel.dll"] {
        fs::copy(Path::new(BUILD).join(f), t.join(f)).unwrap();
    }
    copy_dir(Path::new("../../assets/fonts"), &t.join("fonts"));
    let started = std::time::Instant::now();
    let out = Command::new(t.join("fakegame.exe"))
        .current_dir(&t)
        .env("WCCFPANEL_FULLSCREEN", "0")
        .env("WCCF_DATA", t.join("data"))
        .env("WCCFPANEL_PAUSETEST", "1500")
        .env_remove("WCCF_RELAY")
        .args([
            "forza_panel.dll",
            "wait:2500",
            "sysmove:500", // inside the hold
            "key:80",      // P
            "wait:500",
            "key:80", // within the 10 s gap
            "wait:500",
        ])
        .output()
        .unwrap();
    let took = started.elapsed();
    let said = String::from_utf8_lossy(&out.stdout);
    let log = fs::read_to_string(t.join("forza_panel.log")).unwrap_or_default();
    println!("fakegame:\n{said}\nforza_panel.log:\n{log}");
    assert!(out.status.success(), "fakegame ended with {}", out.status);
    assert!(
        !said.contains("device changed"),
        "the hold left the game's device changed"
    );
    assert!(log.contains("pause: on ("), "no pause");
    let off = log
        .lines()
        .find(|l| l.contains("pause: off (cap) after "))
        .expect("the pause did not end at its cap");
    let nums: Vec<u64> = off
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect();
    assert!(nums[0] >= 1500, "held only {} ms: {off}", nums[0]);
    assert!(
        nums[0] < 2300,
        "held {} ms: past its cap (the Move command's loop?): {off}",
        nums[0]
    );
    assert!(
        nums[1] >= 10,
        "the PAUSED page drew only {} frames: {off}",
        nums[1]
    );
    assert!(
        log.contains("pause: refused - TooSoon("),
        "the gap did not refuse"
    );
    // the hold adds its 1.5 s to fakegame's own 2.5 + 0.5 + 0.15 + 0.5 + 0.15 s of steps
    assert!(took.as_millis() >= 5000, "fakegame ran only {took:?}");
    if env::var_os("FORZA_PANEL_KEEP").is_none() {
        let _ = fs::remove_dir_all(&t);
    }
}
