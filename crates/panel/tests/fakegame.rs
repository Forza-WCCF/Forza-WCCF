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
        .env("FORZA_PANEL_DUMP", "1")
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
