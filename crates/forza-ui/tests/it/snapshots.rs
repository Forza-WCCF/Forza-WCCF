//! Every screen rendered to PNG in English, Italian and Japanese: what a reviewer looks at, then the regression check
//! (`UPDATE_SNAPSHOTS=1` writes new ones). Rendered with wgpu on the machine's GPU.

use egui::{Ui, Vec2};
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, SnapshotResults};
use forza_ui::Lang;
use forza_ui::theme::{self, Edition};
use forza_ui::{installer, launcher, lobby, panel};

/// One test on the GPU at a time: wgpu devices made in parallel crashed the test process now and then (exit 116,
/// about 1 run in 4 on 2026-10-12); one after another they never did.
fn gpu() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn harness<'a>(
    size: Vec2,
    pixels_per_point: f32,
    edition: Edition,
    mut show: impl FnMut(&mut Ui) + 'a,
) -> Harness<'a> {
    let dir = theme::font_dir();
    let mut fresh = true;
    Harness::builder()
        .with_size(size)
        .with_pixels_per_point(pixels_per_point)
        .build_ui(move |ui| {
            if fresh {
                // our fonts take effect from the next frame: draw nothing on this one
                theme::install(ui.ctx(), edition, &dir).expect("the fonts in assets/fonts");
                fresh = false;
                return;
            }
            // kittest puts the closure in an 8 px margin; the screens own the whole window
            let full = ui.ctx().content_rect();
            let mut root = ui.new_child(egui::UiBuilder::new().max_rect(full));
            root.set_clip_rect(full);
            show(&mut root);
        })
}

/// Renders one screen into the test's results (kittest wants one result set per test).
fn snap(
    all: &mut SnapshotResults,
    name: &str,
    size: Vec2,
    edition: Edition,
    show: impl FnMut(&mut Ui),
) {
    let mut h = harness(size, 1.0, edition, show);
    h.run_steps(8);
    h.snapshot(name);
    all.extend_harness(&mut h);
}

#[test]
fn launcher_screens() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    use launcher::Scenario::*;
    for lang in Lang::ALL {
        for (s, ed, tag) in [
            (Ready, Edition::E1011, "ready"),
            (Running, Edition::E1718, "running"),
            (StopArmed, Edition::E1718, "stop"),
            (NotSetUp, Edition::E1011, "setup"),
            (Update, Edition::E1011, "update"),
            (Busy, Edition::E1718, "busy"),
            (Report, Edition::E1718, "report"),
        ] {
            let mut st = launcher::LauncherState::sample(ed, lang, s);
            snap(
                &mut all,
                &format!("launcher_{tag}_{}", lang.code()),
                launcher::SIZE,
                ed,
                |ui| {
                    launcher::show(ui, &mut st);
                },
            );
        }
    }
    all.unwrap();
}

/// ONLINE chosen: the lobby inside the launcher (taller than the window, which scrolls).
#[test]
fn launcher_online() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let mut st =
        launcher::LauncherState::sample(Edition::E1011, Lang::En, launcher::Scenario::Online);
    snap(
        &mut all,
        "launcher_online_en",
        Vec2::new(launcher::SIZE.x, 1000.0),
        Edition::E1011,
        |ui| {
            launcher::show(ui, &mut st);
        },
    );
    all.unwrap();
}

/// ONLINE on 2017-18 with the Direct address fold open: the club server and the VS link switch live there now (SETUP
/// no longer runs past the window).
#[test]
fn launcher_online_1718() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Online);
        st.lobby.direct_open = true;
        snap(
            &mut all,
            &format!("launcher_online_1718_{}", lang.code()),
            Vec2::new(launcher::SIZE.x, 1100.0),
            Edition::E1718,
            |ui| {
                launcher::show(ui, &mut st);
            },
        );
    }
    all.unwrap();
}

#[test]
fn installer_pages() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        for page in installer::Page::ALL {
            let mut st = installer::InstallerState::sample(lang, page);
            let name = format!("installer_{}_{}", page.tag(), lang.code());
            snap(&mut all, &name, installer::SIZE, Edition::E1011, |ui| {
                installer::show(ui, &mut st);
            });
        }
        let mut st = installer::InstallerState::sample(lang, installer::Page::Files);
        st.confirm = true;
        let name = format!("installer_confirm_{}", lang.code());
        snap(&mut all, &name, installer::SIZE, Edition::E1011, |ui| {
            installer::show(ui, &mut st);
        });
    }
    all.unwrap();
}

#[test]
fn panel_pages() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        for page in panel::Page::ALL {
            let ed = if page == panel::Page::Keys {
                Edition::E1718
            } else {
                Edition::E1011
            };
            let mut st = panel::PanelState::sample(ed, lang, page);
            let name = format!("panel_{}_{}", page.tag(), lang.code());
            snap(&mut all, &name, panel::SIZE, ed, |ui| {
                panel::show(ui, &mut st)
            });
        }
    }
    all.unwrap();
}

#[test]
fn panel_2017_18_and_compact() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    snap(
        &mut all,
        "panel_game_1718_en",
        panel::SIZE,
        Edition::E1718,
        |ui| panel::show(ui, &mut st),
    );
    st = panel::PanelState::sample(Edition::E1011, Lang::It, panel::Page::Game);
    st.file.compact = true;
    snap(
        &mut all,
        "panel_compact_it",
        panel::SIZE,
        Edition::E1011,
        |ui| panel::show(ui, &mut st),
    );
    all.unwrap();
}

#[test]
fn widget_sheet() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in [Lang::En, Lang::Ja] {
        let mut st = panel::WidgetSheet::sample(lang);
        snap(
            &mut all,
            &format!("widgets_{}", lang.code()),
            panel::SHEET_SIZE,
            Edition::E1011,
            |ui| panel::widget_sheet(ui, &mut st),
        );
    }
    all.unwrap();
}

#[test]
fn board_interaction() {
    let _gpu = gpu();
    let mut st = panel::PanelState::sample(Edition::E1011, Lang::En, panel::Page::Game);
    let spots = st.board.spots;
    let at = |slot| panel::board_slot_center(panel::SIZE.x, &spots, slot);
    let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st)
    });
    h.run_steps(4);
    // a box-to-box midfielder in hand over the striker on the left of midfield: the heat zone and a FITS badge
    let from = at(6);
    let to = at(5);
    h.drag_at(from);
    h.run_steps(2);
    h.hover_at(to);
    h.run_steps(3);
    h.snapshot("board_drag_en");
    h.drop_at(to);
    h.run_steps(2);
    // the big card of a centre back, with its position map and the heat zone on the pitch
    h.hover_at(at(2));
    h.run_steps(8); // past the 1 s hover delay at 0.25 s a step
    h.snapshot("board_hover_en");
    // a click turns the big card over: its back, the position map with the ratings and the stats
    h.drag_at(at(2));
    h.run_steps(1);
    h.drop_at(at(2));
    h.hover_at(at(2));
    h.run_steps(3);
    h.snapshot("board_back_en");
}

/// The hover pane stays in the board's column: docked under the hint at 1920x1080 (a strip) and 2560x1440 (large), over
/// the pitch's far half at 1440x900 (the card in the top half: the pane at the pitch's foot).
#[test]
fn hover_pane() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (size, slot, name) in [
        (Vec2::new(1920.0, 1080.0), 2, "panel_hover_1080_en"),
        (Vec2::new(2560.0, 1440.0), 2, "panel_hover_1440p_en"),
        (panel::SIZE, 9, "panel_hover_1440x900_top_en"),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1011, Lang::En, panel::Page::Game);
        let col = panel::layout(egui::Rect::from_min_size(egui::Pos2::ZERO, size), false)
            .board
            .shrink(16.0);
        let pitch = forza_ui::board::pitch_rect(col.min, col.width());
        let at = forza_ui::board::slot_rects(pitch, pitch.bottom() + 12.0, &st.board.spots)[slot]
            .center();
        st.hover_delay = 0.0;
        let mut h = harness(size, 1.0, Edition::E1011, |ui| panel::show(ui, &mut st));
        h.run_steps(4);
        h.hover_at(at);
        h.run_steps(4);
        h.snapshot(name);
        all.extend_harness(&mut h);
    }
    all.unwrap();
}

/// The pop-out board's own window: wide (the board beside the deck), tall (the board over the deck and the page bar),
/// and tall with a page open over the board.
#[test]
fn desk_window() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (size, ed, lang, page, name) in [
        (
            Vec2::new(1600.0, 1000.0),
            Edition::E1011,
            Lang::En,
            panel::Page::Game,
            "panel_desk_wide_en",
        ),
        (
            Vec2::new(1000.0, 1500.0),
            Edition::E1718,
            Lang::Ja,
            panel::Page::Game,
            "panel_desk_tall_ja",
        ),
        (
            Vec2::new(1000.0, 1500.0),
            Edition::E1011,
            Lang::It,
            panel::Page::Catalogue,
            "panel_desk_tall_catalogue_it",
        ),
        (
            Vec2::new(1920.0, 1080.0),
            Edition::E1011,
            Lang::En,
            panel::Page::Game,
            "panel_desk_1920x1080_en",
        ),
        (
            Vec2::new(1080.0, 1920.0),
            Edition::E1718,
            Lang::En,
            panel::Page::Game,
            "panel_desk_1080x1920_en",
        ),
    ] {
        let mut st = panel::PanelState::sample(ed, lang, page);
        st.desk = true;
        st.file.board_window = true;
        snap(&mut all, name, size, ed, |ui| panel::show(ui, &mut st));
    }
    all.unwrap();
}

/// The catalogue's card view: front and back side by side at their own pixels on the 1440x900 sheet (x1), one side at
/// x1 with the FRONT | BACK toggle on a 900x563 window (0.625 px a point), the pair at x2 at 175 %. The pictures are
/// made-up 256x357 patterns of single-pixel lines (blurred or moiré if ever scaled by a fraction), never card art.
#[test]
fn card_view() {
    let _gpu = gpu();
    let dir = tempfile::tempdir().expect("a temp folder");
    let cards = forza_ui::sample::cards();
    let ci = 2;
    let no = cards[ci].number;
    for (side, tint) in [("f", [200u8, 60, 40]), ("b", [40, 90, 200])] {
        let img = image::RgbaImage::from_fn(256, 357, |x, y| {
            let line = (x + y) % 2 == 0 || x % 32 == 0 || y % 32 == 0;
            let [r, g, b] = tint;
            if line {
                image::Rgba([r, g, b, 255])
            } else {
                image::Rgba([235, 235, 225, 255])
            }
        });
        img.save_with_format(
            dir.path().join(format!("{no}_{side}.jpg")),
            image::ImageFormat::Png,
        )
        .expect("a picture written");
    }
    let mut all = SnapshotResults::new();
    for (ppp, lang, name) in [
        (1.0, Lang::En, "panel_card_view_en"),
        (0.625, Lang::Ja, "panel_card_view_single_ja"),
        (1.75, Lang::En, "panel_card_view_175_x2_en"),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1011, lang, panel::Page::Catalogue);
        st.catalogue.filter = Default::default();
        st.catalogue.view = Some(ci);
        let path = dir.path().to_owned();
        let mut first = true;
        let mut h = harness(panel::SIZE, ppp, Edition::E1011, |ui| {
            if first {
                forza_ui::board::set_picture_dir(ui.ctx(), Some(path.clone()));
                first = false;
            }
            panel::show(ui, &mut st)
        });
        // the files load on a thread: step until they show
        for _ in 0..40 {
            h.run_steps(1);
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        h.snapshot(name);
        all.extend_harness(&mut h);
    }
    all.unwrap();
}

/// The pop-out board's window at 1024x768 (4:3): the wide layout, the board full height, drawn at the scale the panel
/// DLL picks for it.
#[test]
fn desk_window_4_3() {
    let _gpu = gpu();
    let ppp = panel::desk_scale(1024.0, 768.0, 96.0);
    let mut st = panel::PanelState::sample(Edition::E1011, Lang::En, panel::Page::Game);
    st.desk = true;
    st.file.board_window = true;
    let mut h = harness(Vec2::new(1024.0, 768.0) / ppp, ppp, Edition::E1011, |ui| {
        panel::show(ui, &mut st)
    });
    h.run_steps(8);
    h.snapshot("panel_desk_1024x768_en");
}

/// The 2017-18 formation board as the cabinet's mat: at rest, a card in hand, and with the keeper's slot empty.
#[test]
fn board_mat() {
    let _gpu = gpu();
    let st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    let spots = st.board.spots;
    let at = |slot| panel::board_slot_center(panel::SIZE.x, &spots, slot);
    let st = std::cell::RefCell::new(st);
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    h.snapshot("board_mat_en");
    h.drag_at(at(6));
    h.run_steps(2);
    h.hover_at(at(5));
    h.run_steps(3);
    h.snapshot("board_mat_drag_en");
    h.drop_at(at(5));
    h.hover_at(egui::pos2(1150.0, 280.0));
    st.borrow_mut().board.slots[0] = None;
    h.run_steps(3);
    h.snapshot("board_mat_no_gk_en");
}

/// Links on the board (synthetic): every stage between pitch cards at rest; a click on card 8 (slot 7) brings its group
/// forward with its partner list; the catalogue's stats pane lists the same card's affinity, ticks on the team.
#[test]
fn board_links() {
    let _gpu = gpu();
    let mut st = panel::PanelState::sample(Edition::E1011, Lang::En, panel::Page::Game);
    st.links = forza_ui::sample::links();
    let spots = st.board.spots;
    let at = |slot| panel::board_slot_center(panel::SIZE.x, &spots, slot);
    let st = std::cell::RefCell::new(st);
    let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    h.snapshot("board_links_en");
    h.drag_at(at(7));
    h.run_steps(1);
    h.drop_at(at(7));
    // the mouse on the pitch off the cards, so no big card covers the list
    h.hover_at(egui::pos2(1150.0, 280.0));
    h.run_steps(3);
    h.snapshot("board_links_selected_en");
    {
        let mut s = st.borrow_mut();
        s.page = panel::Page::Catalogue;
        s.catalogue.filter = Default::default();
        s.catalogue.focus = Some(7);
    }
    h.hover_at(egui::pos2(500.0, 440.0));
    h.run_steps(4);
    h.snapshot("panel_catalogue_affinity_en");
}

/// A 1920×1080 screen: one point a pixel, a 1920×1080 sheet; the picture grows past the game's size, the board keeps its height's width.
#[test]
fn panel_1920x1080() {
    let _gpu = gpu();
    let mut st = panel::PanelState::sample(Edition::E1011, Lang::It, panel::Page::Game);
    let mut h = harness(Vec2::new(1920.0, 1080.0), 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st)
    });
    h.run_steps(8);
    h.snapshot("panel_game_1080_it");
}

/// A 2560×1440 screen (the design target): the picture at 1.2x (1728×1080), the board in the rest, the deck under the picture.
#[test]
fn panel_2560x1440() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (ed, lang, name) in [
        (Edition::E1011, Lang::En, "panel_game_1440p_1011_en"),
        (Edition::E1718, Lang::En, "panel_game_1440p_1718_en"),
    ] {
        let mut st = panel::PanelState::sample(ed, lang, panel::Page::Game);
        snap(&mut all, name, Vec2::new(2560.0, 1440.0), ed, |ui| {
            panel::show(ui, &mut st)
        });
    }
    all.unwrap();
}

/// Card pictures load from the folder `set_picture_dir` names: a made-up 128×128 PNG (the original pictures' size; the
/// slot shows the card face, its left 88 columns, cut square from the top)
/// for card 6, none for card 7, which keeps the drawn stand-in. Board, bench, catalogue and big sizes.
#[test]
fn card_pictures() {
    let _gpu = gpu();
    let dir = tempfile::tempdir().expect("a temp folder");
    let png = image::RgbaImage::from_fn(128, 128, |x, y| {
        image::Rgba([(x * 2) as u8, 90, (y * 2) as u8, 255])
    });
    png.save(dir.path().join("6.png")).expect("a PNG written");
    let cards = forza_ui::sample::cards();
    let path = dir.path().to_owned();
    let mut first = true;
    let mut h = harness(Vec2::new(860.0, 200.0), 1.0, Edition::E1011, |ui| {
        if first {
            forza_ui::board::set_picture_dir(ui.ctx(), Some(path.clone()));
            first = false;
        }
        ui.horizontal(|ui| {
            for card in [&cards[5], &cards[6]] {
                for w in [63.0, 79.0, 84.0, 130.0] {
                    let (r, _) = ui.allocate_exact_size(
                        Vec2::new(w, (w * 1.4f32).round()),
                        egui::Sense::hover(),
                    );
                    forza_ui::board::card_face(ui.painter(), r, card, Default::default());
                }
            }
        });
    });
    // the file loads on a thread: step until it shows
    for _ in 0..40 {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    h.snapshot("card_pictures");
}

/// IMPORT LINEUP done: the catalogue's message names the lineup, the site's key player and the cards skipped; the board
/// strip offers UNDO in its hint's place.
#[test]
fn catalogue_imported() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let mut st = panel::PanelState::sample(Edition::E1011, Lang::En, panel::Page::Catalogue);
    st.catalogue.filter = panel::Filter::default();
    let text = "# WCCF DB lineup\ngame 1011\nname Sunday XI\n\
                P 1 50 92 K\nP 2 15 70\nP 3 38 74\nP 4 62 74\nP 5 85 70\nP 6 30 50\nP 7 50 55\n\
                P 8 70 50\nP 9 25 25\nP 10 50 20\nP 11 75 25\nS 12\nS 13\n\
                S \"13/GES 3 \" # GULLIT · 13 GES 3\nS 999\n";
    assert!(panel::import_lineup(&mut st, &[], text, false, 0.0).is_some());
    snap(
        &mut all,
        "panel_catalogue_imported_en",
        panel::SIZE,
        Edition::E1011,
        |ui| panel::show(ui, &mut st),
    );
    all.unwrap();
}

/// CATALOGUE as a player uses it: the page opens with the search line taking the keys, typed words narrow the list,
/// Enter asks for the first card listed, and the table takes it with a message; Esc closes the page.
#[test]
fn catalogue_search_and_add() {
    use std::cell::RefCell;
    let _gpu = gpu();
    let st = RefCell::new(panel::PanelState::sample(
        Edition::E1011,
        Lang::En,
        panel::Page::Catalogue,
    ));
    {
        let mut s = st.borrow_mut();
        s.catalogue.filter = panel::Filter::default();
        s.message = None;
    }
    let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    assert_eq!(
        st.borrow().catalogue.results.len(),
        forza_ui::sample::cards().len()
    );
    h.event(egui::Event::Text("italia porto".into()));
    h.run_steps(2);
    let first = {
        let s = st.borrow();
        assert_eq!(s.catalogue.filter.search, "italia porto");
        let names: Vec<&str> = s
            .catalogue
            .results
            .iter()
            .map(|&i| s.cards[i].name.as_str())
            .collect();
        assert!(!names.is_empty() && names.len() < 4, "{names:?}");
        s.cards[s.catalogue.results[0]].number
    };
    h.key_press(egui::Key::Enter);
    h.run_steps(1);
    let mut placed = Vec::new();
    {
        let mut s = st.borrow_mut();
        let no = s.add.take();
        assert_eq!(no, Some(first));
        assert!(panel::add_card(&mut s, &mut placed, first));
    }
    h.run_steps(2);
    h.snapshot("panel_catalogue_added_en");
    assert_eq!(placed.len(), 1);
    assert!(
        st.borrow()
            .message
            .as_ref()
            .unwrap()
            .text
            .ends_with("added (1/16)")
    );
    h.key_press(egui::Key::Escape);
    h.run_steps(1);
    assert_eq!(st.borrow().page, panel::Page::Game);
}

/// KEYS as a player uses it: a click on SHOOT's key cell, then a key: SHOOT takes it and the action that had it takes
/// SHOOT's old key; the bindings are marked for keys.txt. Esc while a cell waits cancels it, not the page.
#[test]
fn keys_click_then_press() {
    use std::cell::RefCell;
    let _gpu = gpu();
    let st = RefCell::new(panel::PanelState::new(
        Edition::E1011,
        Lang::En,
        forza_ui::sample::board(),
        forza_ui::sample::cards().into(),
    ));
    st.borrow_mut().page = panel::Page::Keys;
    let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(3);
    h.get_by_label("C").click(); // SHOOT's key
    h.run_steps(1);
    assert_eq!(st.borrow().capturing, Some(2));
    h.key_press(egui::Key::Escape);
    st.borrow_mut().vk_down = vec![(0x1B, false)];
    h.run_steps(1);
    assert_eq!(
        (st.borrow().capturing, st.borrow().page),
        (None, panel::Page::Keys)
    );
    h.get_by_label("C").click();
    h.run_steps(1);
    st.borrow_mut().vk_down = vec![(0x58, false)]; // X, PRESS's key
    h.run_steps(2);
    {
        let s = st.borrow();
        assert_eq!((s.keys[2], s.keys[1], s.keys_changed), (0x58, 0x43, true));
    }
    h.snapshot("panel_keys_swapped_en");
    // the operator's switches are folded away until ADVANCED opens
    assert!(h.query_by_label("TEST").is_none());
    h.get_by_label("Advanced").click();
    h.run_steps(3);
    assert!(st.borrow().keys_advanced);
    h.snapshot("panel_keys_advanced_en");
}

/// SETTINGS as a player uses it: COMPACT and a 2 s hover change at once and mark data\panel.txt for writing; RESTART
/// NOW needs a second click, and is refused while a club card session is open.
#[test]
fn settings_choices_and_restart() {
    use std::cell::RefCell;
    let _gpu = gpu();
    let st = RefCell::new(panel::PanelState::new(
        Edition::E1011,
        Lang::En,
        forza_ui::sample::board(),
        forza_ui::sample::cards().into(),
    ));
    {
        let mut s = st.borrow_mut();
        s.page = panel::Page::Settings;
        s.status.text = Some("en");
        s.status.card = panel::CardState::Open;
    }
    let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(3);
    h.get_by_label("COMPACT").click();
    h.get_by_label("2 s").click();
    h.run_steps(1);
    {
        let mut s = st.borrow_mut();
        assert!(
            s.file.compact && s.file_changed && s.file.hover_ms == 2000 && s.hover_delay == 2.0
        );
        s.file_changed = false;
    }
    // SAVE LOGS needs no server and no second click
    h.get_by_label("SAVE LOGS").click();
    h.run_steps(1);
    assert!(st.borrow().save_logs);
    h.get_by_label("RESTART NOW").click();
    h.run_steps(1);
    assert!(!st.borrow().restart);
    st.borrow_mut().status.card = panel::CardState::Closed;
    h.get_by_label("RESTART NOW").click();
    h.run_steps(1);
    h.run_steps(1);
    h.get_by_label_contains("CLICK AGAIN").click();
    h.run_steps(1);
    assert!(st.borrow().restart);
    h.snapshot("panel_settings_changed_en");
}

/// CLUB CARD as a player uses it: the view's club, rows and squad; PLAY THIS CLUB needs a second click and asks for that
/// card (data\panel.txt's card=, then the restart); a transferred card has no button; refused while a card session is
/// open; CLEAR BAD ENDINGS the same way (2010-11); Esc closes the page.
#[test]
fn club_card_switch_and_clear() {
    use forza_ui::panel::ClubPick;
    use std::cell::RefCell;
    let _gpu = gpu();
    let st = RefCell::new(panel::PanelState::new(
        Edition::E1011,
        Lang::En,
        forza_ui::sample::board(),
        forza_ui::sample::cards().into(),
    ));
    {
        let mut s = st.borrow_mut();
        s.page = panel::Page::ClubCard;
        s.club = forza_ui::sample::club_view();
    }
    let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(3);
    assert_eq!(
        h.query_all_by_label("PLAY THIS CLUB").count(),
        1,
        "a transferred card has no button"
    );
    h.get_by_label("PLAY THIS CLUB").click();
    h.run_steps(2);
    assert!(st.borrow().club_request.is_none());
    h.get_by_label("CLICK AGAIN").click();
    h.run_steps(2);
    assert_eq!(
        st.borrow_mut().club_request.take(),
        Some(ClubPick::Card {
            file: "porto_azzurro.bin".into(),
            club: "PORTO AZZURRO FC".into()
        })
    );
    st.borrow_mut().club.session = "open".into();
    h.get_by_label("NEW CLUB CARD").click();
    h.run_steps(2);
    h.get_by_label("NEW CLUB CARD").click();
    h.run_steps(2);
    assert!(
        st.borrow().club_request.is_none(),
        "refused during a session"
    );
    st.borrow_mut().club.session = "cut".into();
    h.get_by_label("CLEAR BAD ENDINGS").click();
    h.run_steps(2);
    h.get_by_label("CLICK AGAIN").click();
    h.run_steps(2);
    assert_eq!(st.borrow_mut().club_request.take(), Some(ClubPick::Fix));
    h.snapshot("panel_clubcard_view_en");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(st.borrow().page, panel::Page::Game);
}

/// The card dealt after a match over the game: NEW PLAYER CARD with the card, its stats and TOTAL; the picture's clicks
/// stay off while it shows; a click on it closes it.
#[test]
fn card_deal() {
    use std::cell::RefCell;
    let _gpu = gpu();
    let st = RefCell::new(panel::PanelState::new(
        Edition::E1011,
        Lang::En,
        forza_ui::sample::board(),
        forza_ui::sample::cards().into(),
    ));
    st.borrow_mut().deal = Some(panel::Deal::new(3));
    let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    assert!(st.borrow().picture_rect.is_none());
    h.snapshot("panel_deal_en");
    // a click on the card turns it over; one beside it closes the window
    let r = h.get_by_label_contains("NEW PLAYER CARD").rect();
    let card = egui::pos2(r.center().x, r.top() + 200.0);
    h.drag_at(card);
    h.drop_at(card);
    h.run_steps(2);
    assert!(st.borrow().deal.as_ref().is_some_and(|d| d.back));
    h.snapshot("panel_deal_back_en");
    let foot = egui::pos2(r.center().x, r.bottom() - 60.0);
    h.drag_at(foot);
    h.drop_at(foot);
    h.run_steps(2);
    assert!(st.borrow().deal.is_none() && st.borrow().picture_rect.is_some());
}

/// The close dialog after a close refused during a card session, over the whole middle: KEEP PLAYING answers Enter and
/// Esc, QUIT only its own click.
#[test]
fn close_dialog() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st = panel::PanelState::sample(Edition::E1011, lang, panel::Page::Game);
        st.close_warn = true;
        snap(
            &mut all,
            &format!("panel_close_dialog_{}", lang.code()),
            panel::SIZE,
            Edition::E1011,
            |ui| panel::show(ui, &mut st),
        );
    }
    all.unwrap();
}

#[test]
fn close_dialog_answers() {
    let _gpu = gpu();
    for (key, quit) in [
        (Some(egui::Key::Enter), false),
        (Some(egui::Key::Escape), false),
        (None, true),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1011, Lang::En, panel::Page::Game);
        st.close_warn = true;
        let st = std::cell::RefCell::new(st);
        let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(4);
        // nothing is answered by itself, and a click outside the buttons answers nothing
        assert!(!st.borrow().close_keep && !st.borrow().close_quit);
        match key {
            Some(k) => h.key_press(k),
            None => h.get_by_label("QUIT NOW").click(),
        }
        h.run_steps(4);
        let st = st.borrow();
        assert_eq!((st.close_keep, st.close_quit), (!quit, quit));
    }
}

/// The card lamp in the bottom bar in each state, and the bar in 2017-18 (no lamp).
#[test]
fn card_lamp() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    use panel::CardState::*;
    for lang in Lang::ALL {
        for (card, tag) in [(Open, "in_use"), (Closed, "safe"), (None, "none")] {
            let mut st = panel::PanelState::sample(Edition::E1011, lang, panel::Page::Game);
            st.status.card = card;
            snap(
                &mut all,
                &format!("panel_lamp_{tag}_{}", lang.code()),
                panel::SIZE,
                Edition::E1011,
                |ui| panel::show(ui, &mut st),
            );
        }
    }
    all.unwrap();
}

/// The crash screen in the board's place; RESTART THE GAME and SEND LOGS each ask once.
#[test]
fn crash_screen() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (ed, lang) in [
        (Edition::E1011, Lang::En),
        (Edition::E1011, Lang::It),
        (Edition::E1011, Lang::Ja),
        (Edition::E1718, Lang::En),
    ] {
        let mut st = panel::PanelState::sample(ed, lang, panel::Page::Game);
        st.crash = true;
        st.status.card = panel::CardState::Open;
        let name = format!(
            "panel_crash_{}{}",
            if ed == Edition::E1718 { "1718_" } else { "" },
            lang.code()
        );
        snap(&mut all, &name, panel::SIZE, ed, |ui| {
            panel::show(ui, &mut st)
        });
    }
    all.unwrap();
    for (label, restart) in [("RESTART THE GAME", true), ("SEND LOGS", false)] {
        let mut st = panel::PanelState::sample(Edition::E1011, Lang::En, panel::Page::Game);
        st.crash = true;
        let st = std::cell::RefCell::new(st);
        let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(4);
        h.get_by_label(label).click();
        h.run_steps(4);
        let st = st.borrow();
        assert_eq!((st.restart, st.open_report), (restart, !restart));
    }
}

/// The PAUSED page over the game's picture (Japanese: the longest lines), and RESUME on the page bar asking for the
/// pause's end.
#[test]
fn paused_page() {
    let _gpu = gpu();
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::Ja, panel::Page::Game);
    st.paused = Some(14);
    let st = std::cell::RefCell::new(st);
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    h.snapshot("panel_paused_ja");
    h.get_by_label("再開").click();
    h.run_steps(2);
    assert!(st.borrow().pause_asked, "RESUME did not ask");
}

/// The cabinet's triangles and round buttons send their action when clicked.
#[test]
fn cabinet_shapes_click() {
    use std::cell::RefCell;
    let _gpu = gpu();
    let st = RefCell::new(panel::PanelState::new(
        Edition::E1011,
        Lang::En,
        forza_ui::sample::board(),
        forza_ui::sample::cards().into(),
    ));
    let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(3);
    for (label, action) in [
        ("CENTRAL", "UP"),
        ("COUNTER", "DOWN"),
        ("PRESS", "PRESS"),
        ("SHOOT", "SHOOT"),
        ("KEEPER", "KEEPER"),
        ("START", "START"),
    ] {
        st.borrow_mut().pressed = None;
        h.get_by_label(label).click();
        h.run_steps(2);
        assert_eq!(st.borrow().pressed, Some(action), "{label}");
    }
}

/// The online lobby on sample data: every scenario in the three languages, at 1 and 2 pixels per point.
#[test]
fn lobby_screens() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (ppp, suffix) in [(1.0, ""), (2.0, "_2x")] {
        for s in lobby::Scenario::ALL {
            for lang in Lang::ALL {
                let mut v = lobby::LobbyView::sample(s);
                let mut h = harness(lobby::SIZE, ppp, Edition::E1718, |ui| {
                    lobby::show(ui, &mut v, lang);
                });
                h.run_steps(8);
                h.snapshot(format!("lobby_{}_{}{suffix}", s.tag(), lang.code()));
                all.extend_harness(&mut h);
            }
        }
    }
    all.unwrap();
}

/// Down, Down, Enter on the sample list joins the third row; Right switches to the VS rooms, F5 and Esc answer; H
/// hosts a VS room, Enter in it is READY.
#[test]
fn lobby_keys() {
    use lobby::LobbyAction::*;
    let _gpu = gpu();
    let v = std::cell::RefCell::new(lobby::LobbyView::sample(lobby::Scenario::List));
    let got = std::cell::RefCell::new(None);
    let mut h = harness(lobby::SIZE, 1.0, Edition::E1718, |ui| {
        if let Some(a) = lobby::show(ui, &mut v.borrow_mut(), Lang::En) {
            *got.borrow_mut() = Some(a);
        }
    });
    h.run_steps(4);
    let third = v.borrow().shops[2].id.clone();
    for key in [egui::Key::ArrowDown, egui::Key::ArrowDown, egui::Key::Enter] {
        h.key_press(key);
        h.run_steps(2);
    }
    assert_eq!(got.take(), Some(Join(third)));
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(v.borrow().kind, lobby::Kind::Vs);
    h.key_press(egui::Key::F5);
    h.run_steps(2);
    assert_eq!(got.take(), Some(Refresh));
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(got.take(), Some(Leave));
    // H (the pad's Y) hosts; in the room Enter is READY, once
    h.key_press(egui::Key::H);
    h.run_steps(2);
    assert_eq!(got.take(), Some(HostVs));
    let mut room = lobby::Room::new("K7QM", lobby::Kind::Vs, 2);
    room.mine = Some(0);
    room.seats[0] = Some(lobby::Occupant {
        club: "A7K2".into(),
        ready: false,
    });
    v.borrow_mut().room = Some(room);
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(got.take(), Some(Ready));
    if let Some(r) = &mut v.borrow_mut().room {
        r.seats[0].as_mut().unwrap().ready = true;
    }
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(got.take(), None, "READY twice");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(got.take(), Some(Leave));
}
