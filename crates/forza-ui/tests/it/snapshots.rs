//! Every screen rendered to PNG in English, Italian and Japanese: what a reviewer looks at, then the regression check
//! (`UPDATE_SNAPSHOTS=1` writes new ones). Rendered with wgpu on the machine's GPU.

use egui::{Ui, Vec2};
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, SnapshotResults};
use forza_ui::Lang;
use forza_ui::theme::Edition;
use forza_ui::{installer, launcher, lobby, panel};

// the harness and the shared sample states, which the other modules still import from here
pub(super) use super::common::{formations_state, harness, import_asked};

/// One test on the GPU at a time: wgpu devices made in parallel crashed the test process now and then (exit 116,
/// about 1 run in 4 on 2026-10-12); one after another they never did.
fn gpu() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether this is CI's runner, the only machine some stored pictures match (`card_view`'s x2 picture).
fn env_on_ci() -> bool {
    std::env::var_os("CI").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Steps `h` 40 frames (the sample's ADD toast is gone by then), then on until no picture is loading, at most 10 s:
/// the fixed wait alone could end while a picture still decoded on a loaded PC.
fn pictures_loaded(h: &mut Harness<'_>) {
    let start = std::time::Instant::now();
    // a frame that ends with nothing pending drew every picture it asked for
    for frame in 0.. {
        h.run_steps(1);
        if frame >= 40 && !h.ctx.has_pending_images() {
            h.run_steps(1);
            return;
        }
        assert!(
            start.elapsed().as_secs() < 10,
            "pictures still loading after 10 s"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Renders one screen into the test's results (kittest wants one result set per test).
fn snap(
    all: &mut SnapshotResults,
    name: &str,
    size: Vec2,
    edition: Edition,
    show: impl FnMut(&mut Ui),
) {
    snap_at(all, name, size, 1.0, edition, show);
}

/// [`snap`] on a screen of another density: Windows at 125, 150 or 200 %, where the panel's own rounding and
/// letterboxing bite and at 100 % they do not.
// no failing pixel is allowed (kittest's default) on purpose: the stored pictures come only from CI's WARP renderer
fn snap_at(
    all: &mut SnapshotResults,
    name: &str,
    size: Vec2,
    pixels_per_point: f32,
    edition: Edition,
    show: impl FnMut(&mut Ui),
) {
    let mut h = harness(size, pixels_per_point, edition, show);
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
        for (s, tag) in [
            (Ready, "ready"),
            (Running, "running"),
            (StopArmed, "stop"),
            (NotSetUp, "setup"),
            (Busy, "busy"),
            (Report, "report"),
            (CleanedUp, "cleanup"),
        ] {
            let mut st = launcher::LauncherState::sample(Edition::E1718, lang, s);
            snap(
                &mut all,
                &format!("launcher_{tag}_{}", lang.code()),
                launcher::SIZE,
                Edition::E1718,
                |ui| {
                    launcher::show(ui, &mut st);
                },
            );
        }
    }
    all.unwrap();
}

/// A setup that reports a percentage keeps its bar; the working band is for a job that reports none.
#[test]
fn launcher_busy_progress() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let mut st =
        launcher::LauncherState::sample(Edition::E1718, Lang::En, launcher::Scenario::Busy);
    st.progress = 0.42;
    snap(
        &mut all,
        "launcher_busy_progress_en",
        launcher::SIZE,
        Edition::E1718,
        |ui| {
            launcher::show(ui, &mut st);
        },
    );
    all.unwrap();
}

/// A 1280x720 screen (work area about 700 tall): the window fits it, PLAY and the status line stay in it, the tabs
/// scroll above them.
#[test]
fn launcher_on_a_short_screen() {
    let _gpu = gpu();
    let size = launcher::window_size(Some(700.0));
    let mut all = SnapshotResults::new();
    use launcher::Scenario::*;
    for (s, tag) in [
        (Ready, "ready"),
        (NotSetUp, "setup"),
        (Update, "update"),
        (Online, "online"),
    ] {
        let mut st = launcher::LauncherState::sample(Edition::E1718, Lang::En, s);
        let mut h = harness(size, 1.0, Edition::E1718, |ui| {
            launcher::show(ui, &mut st);
        });
        h.run_steps(8);
        h.snapshot(format!("launcher_short_{tag}_en"));
        all.extend_harness(&mut h);
    }
    all.unwrap();
}

#[test]
fn launcher_host_dialog() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Online);
        let v = &mut st.lobby;
        (v.arcade_rooms, v.kind, v.list_arcade) = (true, lobby::Kind::Arcades, true);
        (v.cup.mode, v.cup.minutes, v.clock) = (lobby::CupMode::In, 10, Some(14 * 3600));
        v.host_ask = Some((
            false,
            String::new(),
            String::new(),
            lobby::CupChoice::default(),
        ));
        snap(
            &mut all,
            &format!("launcher_host_dialog_{}", lang.code()),
            launcher::SIZE,
            Edition::E1718,
            |ui| {
                launcher::show(ui, &mut st);
            },
        );
    }
    all.unwrap();
}

/// The PLAY tab on 2017-18, This PC: the Universal Cup against 7 CPU teams, in 25 min at 14:00.
#[test]
fn launcher_play_cup() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Ready);
        let v = &mut st.lobby;
        (v.cup.mode, v.cup.minutes, v.clock) = (lobby::CupMode::In, 25, Some(14 * 3600));
        snap(
            &mut all,
            &format!("launcher_play_cup_{}", lang.code()),
            launcher::SIZE,
            Edition::E1718,
            |ui| {
                launcher::show(ui, &mut st);
            },
        );
    }
    all.unwrap();
}

/// The ONLINE tab on 2017-18 with the Direct address fold open (the club server and the VS link switch live there),
/// then closed.
#[test]
fn launcher_online_1718() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let closed = [(Lang::En, false, "launcher_online_1718_closed_en".to_owned())];
    let open = Lang::ALL.map(|l| (l, true, format!("launcher_online_1718_{}", l.code())));
    for (lang, direct, name) in open.into_iter().chain(closed) {
        let mut st =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Online);
        st.lobby.direct_open = direct;
        snap(&mut all, &name, launcher::SIZE, Edition::E1718, |ui| {
            launcher::show(ui, &mut st);
        });
    }
    all.unwrap();
}

/// The installer's sample, about a 2017-18 disc with 2010-11 already installed.
fn installer_1718(lang: Lang, page: installer::Page) -> installer::InstallerState {
    let mut st = installer::InstallerState::sample(lang, page);
    st.iso = r"D:\Images\disc-2017-18.iso".into();
    st.detected = Some(Edition::E1718);
    st.package_check = Some(installer::PackageCheck::Ok(Edition::E1718));
    st.package = r"D:\Images\WCCF-2017-18-kit.zip".into();
    st.done_path = r"C:\Games\Forza-WCCF\2017-2018".into();
    st.edition = Edition::E1718;
    st.installed = vec![Edition::E1011];
    st
}

#[test]
fn installer_pages() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        for page in installer::Page::ALL {
            let mut st = installer_1718(lang, page);
            let name = format!("installer_{}_{}", page.tag(), lang.code());
            snap(&mut all, &name, installer::SIZE, Edition::E1718, |ui| {
                installer::show(ui, &mut st);
            });
        }
        let mut st = installer_1718(lang, installer::Page::Files);
        st.confirm = true;
        let name = format!("installer_confirm_{}", lang.code());
        snap(&mut all, &name, installer::SIZE, Edition::E1718, |ui| {
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
        for page in [panel::Page::Keys, panel::Page::Settings] {
            let mut st = panel::PanelState::sample(Edition::E1718, lang, page);
            let name = format!("panel_{}_{}", page.tag(), lang.code());
            snap(&mut all, &name, panel::SIZE, Edition::E1718, |ui| {
                panel::show(ui, &mut st)
            });
        }
        // its arcade line on the game page, its own filters in the catalogue
        for page in [panel::Page::Game, panel::Page::Catalogue] {
            let mut st = panel::PanelState::sample(Edition::E1718, lang, page);
            let name = format!("panel_{}_1718_{}", page.tag(), lang.code());
            snap(&mut all, &name, panel::SIZE, Edition::E1718, |ui| {
                panel::show(ui, &mut st)
            });
        }
    }
    all.unwrap();
}

/// Windows at 150 % and 200 %: the Launcher on a 1.5 screen and the 2017-18 panel on a 2.0 one. The panel's sheet
/// scales by `min(H/900, W/1440)` through `pixels_per_point`, so a fraction of a point shows here first.
#[test]
fn scaled_screens() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let mut st =
        launcher::LauncherState::sample(Edition::E1718, Lang::En, launcher::Scenario::Ready);
    snap_at(
        &mut all,
        "launcher_ready_15x_en",
        launcher::SIZE,
        1.5,
        Edition::E1718,
        |ui| {
            launcher::show(ui, &mut st);
        },
    );
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    snap_at(
        &mut all,
        "panel_game_2x_en",
        panel::SIZE,
        2.0,
        Edition::E1718,
        |ui| panel::show(ui, &mut st),
    );
    all.unwrap();
}

/// COMPACT: the sheet folded down for a small window.
#[test]
fn panel_compact() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::It, panel::Page::Game);
    st.file.compact = true;
    snap(
        &mut all,
        "panel_compact_it",
        panel::SIZE,
        Edition::E1718,
        |ui| panel::show(ui, &mut st),
    );
    all.unwrap();
}

/// 2017-18: the FPS counter over the game picture, and SETTINGS with texture filtering at 8x and the counter on.
#[test]
fn panel_fps_and_textures_1718() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    st.fps = Some(30); // SETTINGS > FPS COUNTER on
    snap(
        &mut all,
        "panel_game_fps_1718_en",
        panel::SIZE,
        Edition::E1718,
        |ui| panel::show(ui, &mut st),
    );
    // the longest text
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::Ja, panel::Page::Settings);
    (st.file.textures, st.file.fps) = (8, true);
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st)
    });
    h.run_steps(8);
    // GRAPHICS sits under CLEAR AND RESTART, below the fold at this size (#361): scrolled to
    let fps = forza_ui::text(Lang::Ja, "settings.fps", &[]);
    h.get_by_label(&fps).scroll_to_me();
    h.run_steps(8); // the scroll lands on the 4th frame and its 0.1 s animation finishes by the 8th
    h.snapshot("panel_settings_textures_1718_ja");
    all.extend_harness(&mut h);
    all.unwrap();
}

/// The server's notice: across the top of the game picture (2017-18 game page) and under the Launcher's status line.
#[test]
fn server_notice() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (lang, text) in [
        (Lang::En, "Server update at 23:00 CET, about 10 minutes."),
        (
            Lang::It,
            "Aggiornamento del server alle 23:00 CET, circa 10 minuti. Così è.",
        ),
        (Lang::Ja, "23:00(CET)にサーバーを更新します。約10分。"),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
        st.banner = Some(panel::Note::banner(text.into()));
        let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st)
        });
        h.run_steps(8);
        h.snapshot(format!("panel_notice_1718_{}", lang.code()));
        all.extend_harness(&mut h);
        let mut st =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Ready);
        st.server_notice = Some(text.into());
        snap(
            &mut all,
            &format!("launcher_notice_{}", lang.code()),
            launcher::SIZE,
            Edition::E1718,
            |ui| {
                launcher::show(ui, &mut st);
            },
        );
    }
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
            Edition::E1718,
            |ui| panel::widget_sheet(ui, &mut st),
        );
    }
    all.unwrap();
}

#[test]
fn board_interaction() {
    let _gpu = gpu();
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    let spots = st.board.spots;
    // the 2017-18 board's slots, not the 2010-11 helper's: its clicks missed every card (#398)
    let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, panel::SIZE);
    let at = |slot| panel::board_slot_center_1718(sheet, None, &spots, slot);
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
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
    // the preview of a centre back at the cursor, and the heat zone on the pitch
    h.hover_at(at(2));
    h.run_steps(8); // past the 1 s hover delay at 0.25 s a step
    h.snapshot("board_hover_en");
    // a click pins it: no back picture here, so no FRONT | BACK (#333)
    h.drag_at(at(2));
    h.run_steps(1);
    h.drop_at(at(2));
    h.hover_at(at(2));
    h.run_steps(3);
    h.snapshot("board_pinned_en");
}

/// The hover preview at the cursor, in the board's column (#333), at 1920x1080, 2560x1440 and 1440x900 (a card in the
/// top half), and in Japanese (the caption's widest words).
#[test]
fn hover_pane() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (size, slot, lang, name) in [
        (
            Vec2::new(1920.0, 1080.0),
            2,
            Lang::En,
            "panel_hover_1080_en",
        ),
        (
            Vec2::new(2560.0, 1440.0),
            2,
            Lang::En,
            "panel_hover_1440p_en",
        ),
        (panel::SIZE, 9, Lang::En, "panel_hover_1440x900_top_en"),
        (panel::SIZE, 2, Lang::Ja, "board_preview_ja"),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
        // the 2017-18 board as drawn (the older pitch_rect sum missed the card)
        let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let at = panel::board_slot_center_1718(sheet, None, &st.board.spots, slot);
        st.hover_delay = Some(0.0);
        let mut h = harness(size, 1.0, Edition::E1718, |ui| panel::show(ui, &mut st));
        h.run_steps(4);
        h.hover_at(at);
        h.run_steps(4);
        h.snapshot(name);
        all.extend_harness(&mut h);
    }
    all.unwrap();
}

/// Popped out, the board window: the board alone, filling the window (bars where the shape differs), its bench
/// beside the pitch from 2:3 wide and under it on taller windows; with the buttons switch on, the cabinet buttons
/// under a portrait board and beside a landscape one.
#[test]
fn popout_board_window() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (w, h, lang, buttons, name) in [
        (960.0, 1232.0, Lang::En, false, "popout_board_960x1232_en"),
        (1080.0, 1920.0, Lang::En, false, "popout_board_1080x1920_en"),
        (2360.0, 1640.0, Lang::En, false, "popout_board_2360x1640_en"),
        (1920.0, 1080.0, Lang::En, false, "popout_board_1920x1080_en"),
        (1920.0, 1080.0, Lang::It, false, "popout_board_1920x1080_it"),
        (
            1080.0,
            1920.0,
            Lang::En,
            true,
            "popout_board_buttons_1080x1920_en",
        ),
        (
            1920.0,
            1080.0,
            Lang::It,
            true,
            "popout_board_buttons_1920x1080_it",
        ),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
        st.desk = true;
        st.file.board_window = true;
        st.file.board_buttons = buttons;
        snap(&mut all, name, Vec2::new(w, h), Edition::E1718, |ui| {
            panel::show(ui, &mut st)
        });
    }
    all.unwrap();
}

/// Popped out, the game window: the picture on the left over the toast line and the page bar, the cabinet buttons
/// big in the right column and over them the details of the card touched last on the board window.
#[test]
fn popout_game_window() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (w, h, lang, name) in [
        (1920.0, 1080.0, Lang::En, "popout_game_1920x1080_en"),
        (2560.0, 1440.0, Lang::En, "popout_game_2560x1440_en"),
        (1920.0, 1080.0, Lang::It, "popout_game_1920x1080_it"),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
        st.file.board_window = true;
        st.popped = true;
        let ci = st.board.slots[2].expect("a card in slot 2");
        st.touched = Some(st.board.cards[ci].clone());
        snap(&mut all, name, Vec2::new(w, h), Edition::E1718, |ui| {
            panel::show(ui, &mut st)
        });
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
    let mut cases = vec![
        (1.0, Lang::En, "panel_card_view_en"),
        (0.625, Lang::Ja, "panel_card_view_single_ja"),
    ];
    // The x2 picture is stored from CI's renderer (WARP): it draws the board's NEAREST texture crisp, where a desktop GPU
    // blends it (0.625/0.375 of neighbouring texels, 658,626 pixels off on 2026-11-10). Only CI compares that picture.
    if env_on_ci() {
        cases.push((1.75, Lang::En, "panel_card_view_175_x2_en"));
    } else {
        println!(
            "card_view: panel_card_view_175_x2_en not compared (stored from CI's WARP render; this GPU blends the board's \
NEAREST texture)"
        );
    }
    for (ppp, lang, name) in cases {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Catalogue);
        st.catalogue.filter = Default::default();
        st.catalogue.view = Some(ci);
        let path = dir.path().to_owned();
        let mut first = true;
        let mut h = harness(panel::SIZE, ppp, Edition::E1718, |ui| {
            if first {
                forza_ui::board::set_picture_dir(ui.ctx(), Some(path.clone()));
                first = false;
            }
            panel::show(ui, &mut st)
        });
        pictures_loaded(&mut h);
        h.snapshot(name);
        all.extend_harness(&mut h);
    }
    all.unwrap();
}

/// The 2017-18 formation board as the cabinet's mat: at rest, a card in hand, and with the keeper's slot empty.
#[test]
fn board_mat() {
    let _gpu = gpu();
    let st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    let spots = st.board.spots;
    let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, panel::SIZE);
    let at = |slot| panel::board_slot_center_1718(sheet, None, &spots, slot);
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
/// forward with its partner list; the catalogue's detail pane shows the same card, its picture in the room the board's
/// lists take.
#[test]
fn board_links() {
    let _gpu = gpu();
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    st.links = forza_ui::sample::links();
    let spots = st.board.spots;
    let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, panel::SIZE);
    let at = |slot| panel::board_slot_center_1718(sheet, None, &spots, slot);
    let st = std::cell::RefCell::new(st);
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
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
    // a press on the pitch off the cards lets the clicked card's big pane go (the selection stays)
    h.drag_at(egui::pos2(1150.0, 280.0));
    h.drop_at(egui::pos2(1150.0, 280.0));
    {
        let mut s = st.borrow_mut();
        s.page = panel::Page::Catalogue;
        s.catalogue.filter = Default::default();
        s.catalogue.focus = Some(7);
    }
    h.hover_at(egui::pos2(500.0, 440.0));
    h.run_steps(4);
    h.snapshot("panel_catalogue_focus_en");
}

/// The 2017-18 catalogue at 1920x1080 points, x1 and 125 % in English and at 125 % in Japanese: the grid fills its
/// width (6 cards a row and more), the detail pane draws the focused card's picture at whole pixels (a made-up
/// 256x357 pattern of single-pixel lines, blurred if ever scaled by a fraction) at its album art's own size, larger
/// than a grid card, and its stat labels beside their values at the narrow pane (the Japanese ones are the widest).
#[test]
fn catalogue_1080() {
    let _gpu = gpu();
    let dir = tempfile::tempdir().expect("a temp folder");
    let ci = 7;
    let no = forza_ui::sample::cards()[ci].number;
    image::RgbaImage::from_fn(256, 357, |x, y| {
        if (x + y) % 2 == 0 || x % 32 == 0 || y % 32 == 0 {
            image::Rgba([200, 60, 40, 255])
        } else {
            image::Rgba([235, 235, 225, 255])
        }
    })
    .save_with_format(
        dir.path().join(format!("{no}_f.jpg")),
        image::ImageFormat::Png,
    )
    .expect("a picture written");
    let mut all = SnapshotResults::new();
    let (wide, small) = (Vec2::new(1920.0, 1080.0), panel::SIZE);
    // (window in points, scale, language, snapshot): at 1920x1080 the pane shows the picture at the album art's own
    // points and ADD without scrolling; at 1440x900 it shows ADD
    for (size, ppp, lang, name) in [
        (wide, 1.0, Lang::En, Some("panel_catalogue_1080_en")),
        (wide, 1.25, Lang::En, Some("panel_catalogue_1080_125_en")),
        (wide, 1.25, Lang::Ja, Some("panel_catalogue_1080_125_ja")),
        (wide, 1.0, Lang::It, None),
        (wide, 1.0, Lang::Ja, None),
        (wide, 1.25, Lang::It, None),
        (small, 1.0, Lang::En, None),
        (small, 1.0, Lang::It, None),
        (small, 1.0, Lang::Ja, None),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Catalogue);
        st.links = forza_ui::sample::links();
        st.catalogue.filter = Default::default();
        st.catalogue.focus = Some(ci);
        let path = dir.path().to_owned();
        let mut first = true;
        let mut h = harness(size, ppp, Edition::E1718, |ui| {
            if first {
                forza_ui::board::set_picture_dir(ui.ctx(), Some(path.clone()));
                first = false;
            }
            panel::show(ui, &mut st)
        });
        // the file loads on a thread: step until it shows
        for _ in 0..40 {
            h.run_steps(1);
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        if let Some(name) = name {
            h.snapshot(name);
            all.extend_harness(&mut h);
        }
    }
    all.unwrap();
}

/// 2017-18: RESET LINEUP under the board, lit while a kept table differs from the board, greyed out in a match.
#[test]
fn board_reset_lineup() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (name, ready, in_match) in [
        ("board_reset_lineup_en", true, false),
        ("board_reset_lineup_in_match_en", true, true),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
        if ready {
            st.before_match = Some(forza::cards::table_after(&[], &st.board));
        }
        st.reset_ready = ready;
        st.in_match = in_match;
        snap(&mut all, name, panel::SIZE, Edition::E1718, |ui| {
            panel::show(ui, &mut st)
        });
    }
    all.unwrap();
}

/// The board's foot inside the board's own margin at the narrowest and widest panel the tests draw: the buttons and
/// the hint keep clear of the pitch's border, in English, Italian and Japanese (whose labels are the longest).
#[test]
fn board_foot_inside_the_board() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (size, tag) in [
        (panel::SIZE, "1440x900"),
        (Vec2::new(2560.0, 1440.0), "2560x1440"),
    ] {
        for lang in Lang::ALL {
            let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
            st.before_match = Some(forza::cards::table_after(&[], &st.board));
            snap(
                &mut all,
                &format!("board_foot_{tag}_{}", lang.code()),
                size,
                Edition::E1718,
                |ui| panel::show(ui, &mut st),
            );
        }
    }
    all.unwrap();
}

/// RESET LINEUP names the moment its table was kept in its hover text, and a reset leaves an UNDO under the board.
#[test]
fn board_reset_lineup_hint_and_undo() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
        st.reset_ready = true;
        st.before_match = Some(Vec::new());
        st.before_match_at = Some("21:40".into());
        let st = std::cell::RefCell::new(st);
        let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(4);
        h.get_by_label(&forza_ui::text(lang, "board.reset", &[]))
            .hover();
        h.run_steps(12);
        h.snapshot(format!("board_reset_hint_{}", lang.code()));
        all.extend_harness(&mut h);
        drop(h);
        let mut st = st.into_inner();
        st.reset_ready = false;
        st.reset_after = Some(Vec::new());
        st.import_undo = Some((Vec::new(), 0.0));
        snap(
            &mut all,
            &format!("board_reset_undo_{}", lang.code()),
            panel::SIZE,
            Edition::E1718,
            |ui| panel::show(ui, &mut st),
        );
    }
    all.unwrap();
}

/// A table kept by an earlier run that differs from the board: RESET LINEUP lit, and the question it asks first
/// (what CONFIRM, CANCEL and DON'T ASK AGAIN do is driven in `behaviour`).
#[test]
fn board_reset_lineup_asks_first() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    let now = forza::cards::table_after(&[], &st.board);
    // the kept table: the first pitch card and the first sub trade places
    let mut kept = now.clone();
    let p = kept.iter().position(|c| !c.on_bench()).unwrap();
    let b = kept.iter().position(|c| c.on_bench()).unwrap();
    (kept[p].no, kept[b].no) = (kept[b].no, kept[p].no);
    panel::keep_saved(&mut st, kept, std::time::SystemTime::now());
    st.before_match_at = Some("21:40".into()); // a fixed time for the picture
    panel::lineup_tick(&mut st, None, Some((false, false)), &now);
    let st = std::cell::RefCell::new(st);
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    // lit: a greyed-out button would not take the click
    h.get_by_label("RESET LINEUP").click();
    h.run_steps(8);
    h.snapshot("board_reset_ask_en");
    all.extend_harness(&mut h);
    all.unwrap();
}

/// The LINKS list draws Latin Extended-A letters (ć č š ž ł ő): Noto Sans JP has none of them, so Barlow follows it.
#[test]
fn links_list_latin_extended() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    snap(
        &mut all,
        "board_links_glyphs_en",
        Vec2::new(260.0, 150.0),
        Edition::E1718,
        |ui| {
            ui.add_space(6.0);
            for (name, v) in [
                ("Neven SUBOTIĆ", 5),
                ("Nemanja VIDIĆ", 4),
                ("Luka MODRIĆ Čeh", 3),
                ("Łukasz PISZCZEK", 3),
                ("Gábor KIRÁLY Őri", 2),
                ("Šime VRSALJKO Žiga", 1),
            ] {
                forza_ui::board::partner_row(ui, name, None, Some(v), 230.0);
            }
        },
    );
    all.unwrap();
}

/// A 1920×1080 screen: one point a pixel, a 1920×1080 sheet; the picture grows past the game's size, the board keeps its height's width.
#[test]
fn panel_1920x1080() {
    let _gpu = gpu();
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::It, panel::Page::Game);
    let mut h = harness(Vec2::new(1920.0, 1080.0), 1.0, Edition::E1718, |ui| {
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
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    snap(
        &mut all,
        "panel_game_1440p_1718_en",
        Vec2::new(2560.0, 1440.0),
        Edition::E1718,
        |ui| panel::show(ui, &mut st),
    );
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
    let mut h = harness(Vec2::new(860.0, 200.0), 1.0, Edition::E1718, |ui| {
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
    pictures_loaded(&mut h);
    h.snapshot("card_pictures");
}

/// IMPORT LINEUP done: the catalogue's message names the lineup, the site's key player and the cards skipped; the board
/// strip offers UNDO in its hint's place.
#[test]
fn catalogue_imported() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Catalogue);
    st.catalogue.filter = panel::Filter::default();
    let text = "# WCCF DB lineup\ngame 1718\nname Sunday XI\n\
                P 1 50 92 K\nP 2 15 70\nP 3 38 74\nP 4 62 74\nP 5 85 70\nP 6 30 50\nP 7 50 55\n\
                P 8 70 50\nP 9 25 25\nP 10 50 20\nP 11 75 25\nS 12\nS 13\n\
                S \"13/GES 3 \" # GULLIT · 13 GES 3\nS 999\n";
    panel::import_lineup(&mut st, &[], text, false, 0.0).expect("the lineup imported");
    snap(
        &mut all,
        "panel_catalogue_imported_en",
        panel::SIZE,
        Edition::E1718,
        |ui| panel::show(ui, &mut st),
    );
    all.unwrap();
}

/// Create-a-Team's CATALOGUE (plan step 1.3): only the collection's cards. (c) twelve starter cards, each with the
/// STARTER mark; (e) imported copies show the IMPORT mark, earned copies none.
#[test]
fn catalogue_owned_marks() {
    use forza::collection::Copies;
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let copies = |starter, imported, earned| Copies {
        starter,
        imported,
        earned,
        ..Copies::default()
    };
    let owned = [
        (
            "panel_catalogue_starters_1718_en",
            (1..=12).map(|n| (n, copies(1, 0, 0))).collect(),
        ),
        (
            "panel_catalogue_marks_1718_en",
            (1..=3)
                .map(|n| (n, copies(0, 1, 0)))
                .chain((4..=6).map(|n| (n, copies(0, 0, 1))))
                .collect(),
        ),
    ];
    for (name, owned) in owned {
        let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Catalogue);
        st.catalogue.filter = panel::Filter::default();
        st.set_owned(Some(owned));
        snap(&mut all, name, panel::SIZE, Edition::E1718, |ui| {
            panel::show(ui, &mut st)
        });
    }
    all.unwrap();
}

/// CATALOGUE as a player uses it: the page opens with the search line taking the keys, typed words narrow the list,
/// Enter asks for the first card listed, and the table takes it with a message; Esc closes the page.
#[test]
fn catalogue_search_and_add() {
    use std::cell::RefCell;
    let _gpu = gpu();
    let st = RefCell::new(panel::PanelState::sample(
        Edition::E1718,
        Lang::En,
        panel::Page::Catalogue,
    ));
    {
        let mut s = st.borrow_mut();
        s.catalogue.filter = panel::Filter::default();
        s.message = None;
    }
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    h.event(egui::Event::Text("italia porto".into()));
    h.run_steps(2);
    let first = st.borrow().cards[st.borrow().catalogue.results[0]].number;
    h.key_press(egui::Key::Enter);
    h.run_steps(1);
    let mut placed = Vec::new();
    {
        let mut s = st.borrow_mut();
        let _ = s.add.take();
        panel::add_card(&mut s, &mut placed, first);
    }
    h.run_steps(2);
    h.snapshot("panel_catalogue_added_en");
}

/// KEYS as a player uses it: a click on SHOOT's key cell, then a key: SHOOT takes it and the action that had it takes
/// SHOOT's old key; the bindings are marked for keys.txt. Esc while a cell waits cancels it, not the page.
#[test]
fn keys_click_then_press() {
    use std::cell::RefCell;
    let _gpu = gpu();
    let st = RefCell::new(panel::PanelState::new(
        Edition::E1718,
        Lang::En,
        forza_ui::sample::board(),
        forza_ui::sample::cards().into(),
    ));
    st.borrow_mut().page = panel::Page::Keys;
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(3);
    h.get_by_label("C").click(); // SHOOT's key
    h.run_steps(1);
    h.key_press(egui::Key::Escape);
    st.borrow_mut().vk_down = vec![(0x1B, false)];
    h.run_steps(1);
    h.get_by_label("C").click();
    h.run_steps(1);
    st.borrow_mut().vk_down = vec![(0x58, false)]; // X, PRESS's key
    h.run_steps(2);
    h.snapshot("panel_keys_swapped_en");
    // the operator's switches are folded away until ADVANCED opens
    h.get_by_label("Advanced").click();
    h.run_steps(3);
    h.snapshot("panel_keys_advanced_en");
}

/// 2017-18 SETTINGS: CLEAR BAD ENDINGS (no CLUB CARD page there) asks for the reset by its second click.
#[test]
fn settings_1718_clear_bad_endings() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Settings);
        let name = format!("panel_settings_1718_{}", lang.code());
        snap(&mut all, &name, panel::SIZE, Edition::E1718, |ui| {
            panel::show(ui, &mut st)
        });
    }
    all.unwrap();
}

/// SETTINGS as a player uses it: COMPACT and a 2 s hover change at once and mark data\panel.txt for writing; RESTART
/// NOW needs a second click, and is refused while a club card session is open.
#[test]
fn settings_choices_and_restart() {
    use std::cell::RefCell;
    let _gpu = gpu();
    let st = RefCell::new(panel::PanelState::new(
        Edition::E1718,
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
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(3);
    h.get_by_label("COMPACT").click();
    h.get_by_label("2 s").click();
    h.run_steps(1);
    st.borrow_mut().file_changed = false;
    // SAVE LOGS needs no server and no second click
    h.get_by_label("SAVE LOGS").click();
    h.run_steps(1);
    h.get_by_label("RESTART NOW").click();
    h.run_steps(1);
    st.borrow_mut().status.card = panel::CardState::Closed;
    h.get_by_label("RESTART NOW").click();
    h.run_steps(1);
    h.run_steps(1);
    h.get_by_label_contains("CLICK AGAIN").click();
    h.run_steps(1);
    h.snapshot("panel_settings_changed_en");
}

/// Create-a-Team's deal of a card already owned: saved, the match's tokens, the copies held, CONVERT's pay and the
/// market's lowest price; no buttons (CONVERT and selling live in the collection). A 2017-18 card of value 150.
#[test]
fn card_deal_second_copy() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st = panel::PanelState::new(
            Edition::E1718,
            lang,
            forza_ui::sample::board(),
            forza_ui::sample::cards().into(),
        );
        let mut deal = panel::Deal::new(3);
        deal.grant = Some(panel::Grant::Saved {
            tokens: 15,
            copies: 2,
            convert: Some(30),
            floor: 120,
        });
        st.deal = Some(deal);
        snap(
            &mut all,
            &format!("panel_deal_copy_{}", lang.code()),
            panel::SIZE,
            Edition::E1718,
            |ui| panel::show(ui, &mut st),
        );
    }
    all.unwrap();
}

#[test]
fn import_dialog() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Catalogue);
        st.catalogue.filter = panel::Filter::default();
        let text = "game 1718\nP 1 50 92\nP 2 15 70\nP 3 38 74\nS 999\n";
        assert!(panel::ask_import(&mut st, text, false));
        snap(
            &mut all,
            &format!("panel_import_dialog_{}", lang.code()),
            panel::SIZE,
            Edition::E1718,
            |ui| panel::show(ui, &mut st),
        );
    }
    all.unwrap();
}

/// 2017-18 IMPORT LINEUP: a dialog first, saying where a lineup comes from (the db.wccf.it Lineup page), its address
/// with OPEN THE SITE and COPY (COPIED once the panel DLL copied it), then IMPORT FROM CLIPBOARD: the clipboard read as
/// before. A COPY that fails says so in the dialog, Enter takes the button that has the focus, and Esc closes without
/// importing. 2010-11 reads the clipboard at once.
#[test]
fn catalogue_import_help() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let t = |lang, key| forza_ui::text(lang, key, &[]);
    for lang in Lang::ALL {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Catalogue);
        st.message = None;
        let st = std::cell::RefCell::new(st);
        let mut h = harness(Vec2::new(1920.0, 1080.0), 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(4);
        h.get_by_label(&t(lang, "cat.import")).click();
        h.run_steps(3);
        h.get_by_label(&t(lang, "cat.site_open")).click();
        h.run_steps(1);
        // Enter takes the focused button: on COPY it copies and does not start the import
        h.get_by_label(&t(lang, "cat.site_copy")).focus();
        h.run_steps(1);
        h.key_press(egui::Key::Enter);
        h.run_steps(1);
        // the DLL took both clicks; then its COPY failed: the refusal is drawn in the dialog, not on the page's line
        st.borrow_mut().site_copy = false;
        st.borrow_mut().site_open = false;
        st.borrow_mut().site_copied = false;
        st.borrow_mut()
            .say_site(forza_ui::widgets::Tone::Refused, "cat.msg_site_not_copied");
        h.run_steps(2);
        // the DLL copied it
        st.borrow_mut().site_copied = true;
        st.borrow_mut().site_note = None;
        h.run_steps(3);
        h.snapshot(format!("panel_import_help_{}", lang.code()));
        all.extend_harness(&mut h);
    }
    all.unwrap();
}

/// FORMATIONS (issue #89) during a match: two slots filled (one from another club), LOAD greyed out with its reason,
/// slot 2 being named for SAVE (REPLACE: it is filled).
#[test]
fn formations_dialog() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st = formations_state(lang, true);
        st.formations.as_mut().unwrap().naming = Some((1, "Cup XI".into()));
        st.page = panel::Page::Catalogue;
        snap(
            &mut all,
            &format!("panel_formations_{}", lang.code()),
            panel::SIZE,
            Edition::E1718,
            |ui| panel::show(ui, &mut st),
        );
    }
    all.unwrap();
}

/// 2017-18: FORMATIONS in the row under the board (with RESET LINEUP and POP OUT), on the game window's board and on
/// the pop-out board's window; the dialog opened from the pop-out board shows there.
#[test]
fn board_formations_button() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
        st.reset_ready = true;
        snap(
            &mut all,
            &format!("board_formations_{}", lang.code()),
            panel::SIZE,
            Edition::E1718,
            |ui| panel::show(ui, &mut st),
        );
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
        st.desk = true;
        st.file.board_window = true;
        st.reset_ready = true;
        snap(
            &mut all,
            &format!("popout_board_formations_{}", lang.code()),
            Vec2::new(960.0, 1232.0),
            Edition::E1718,
            |ui| panel::show(ui, &mut st),
        );
    }
    let mut st = formations_state(Lang::En, false);
    st.desk = true;
    st.formations_desk = true;
    st.file.board_window = true;
    snap(
        &mut all,
        "popout_board_formations_dialog_en",
        Vec2::new(960.0, 1232.0),
        Edition::E1718,
        |ui| panel::show(ui, &mut st),
    );
    all.unwrap();
}

/// The crash screen in the board's place; RESTART THE GAME and SAVE LOGS each ask once.
#[test]
fn crash_screen() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
        st.crash = true;
        st.status.card = panel::CardState::Open;
        let name = format!("panel_crash_1718_{}", lang.code());
        snap(&mut all, &name, panel::SIZE, Edition::E1718, |ui| {
            panel::show(ui, &mut st)
        });
    }
    all.unwrap();
}

/// The card dealt after a match over the game: NEW PLAYER CARD with the card, its stats and TOTAL; the picture's clicks
/// stay off while it shows; a click on it closes it.
#[test]
fn card_deal() {
    use std::cell::RefCell;
    let _gpu = gpu();
    let st = RefCell::new(panel::PanelState::new(
        Edition::E1718,
        Lang::En,
        forza_ui::sample::board(),
        forza_ui::sample::cards().into(),
    ));
    st.borrow_mut().deal = Some(panel::Deal::new(3));
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    h.snapshot("panel_deal_en");
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
}

/// HOST ARCADE while the arcade and this PC's seat run: the room code, COPY and LEAVE stay live, the rest is greyed.
#[test]
fn launcher_host_arcade() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Online);
        st.running = true;
        let v = &mut st.lobby;
        (v.arcade_rooms, v.hosting, v.host_code) = (true, true, Some("K7QM2XPD".into()));
        v.kind = lobby::Kind::Arcades;
        snap(
            &mut all,
            &format!("launcher_host_arcade_{}", lang.code()),
            launcher::SIZE,
            Edition::E1718,
            |ui| {
                launcher::show(ui, &mut st);
            },
        );
    }
    all.unwrap();
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
                // the LAN fold opens under the list: in the Launcher it is scrolled to
                let lan = matches!(s, lobby::Scenario::LanList | lobby::Scenario::LanNone);
                let size = lobby::SIZE + egui::vec2(0.0, if lan { 280.0 } else { 0.0 });
                let mut h = harness(size, ppp, Edition::E1718, |ui| {
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

/// The pinned card on the docked 2017-18 board at its least width (the divider dragged to BOARD_MIN): a click on a mat
/// card pins it on its front; with no back picture its switch's row stays empty (#333).
#[test]
fn board_card_flips_at_the_least_width() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, panel::SIZE);
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    st.file.board_width = Some(panel::BOARD_MIN as u16);
    st.hover_delay = None; // no big card by resting: only a click shows one
    let slots: Vec<usize> = (0..11)
        .filter(|&s| st.board.slots[s].is_some())
        .take(2)
        .collect();
    let at = |s| panel::board_slot_center_1718(sheet, Some(panel::BOARD_MIN), &st.board.spots, s);
    let at_a = at(slots[0]);
    let st = std::cell::RefCell::new(st);
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    let click = |h: &mut Harness, p: egui::Pos2| {
        h.drag_at(p);
        h.run_steps(1);
        h.drop_at(p);
        h.run_steps(3);
    };
    h.run_steps(4);
    // a mat card clicked: big, on its front
    click(&mut h, at_a);
    // pinned, no back picture here: the front, the caption with its facts, the switch's row empty (#333)
    h.snapshot("board_pinned_420_en");
    all.extend_harness(&mut h);
    all.unwrap();
}

/// Create-a-Team's COLLECTION (plan step 1.4, (c)): the start picker, the deal with its 3 RARE cards, the page with the
/// STARTER and IMPORT marks and a starter card's detail, the pack store (a Nation pack stepped down) and an opened
/// pack, in English, Italian and Japanese; and the PLAY tab's COLLECTION button.
#[test]
fn launcher_collection() {
    use forza_ui::collection::{CollectionView, Scenario::*};
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    let state = |lang, s| {
        let mut st =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Ready);
        st.profile = Some(forza::profile::Profile::CreateATeam);
        st.collection = Some(CollectionView::sample(s));
        st.tab = launcher::Tab::Collection;
        st
    };
    for lang in Lang::ALL {
        for (s, tag) in [
            (Choose, "choose"),
            (Pick, "pick"),
            (Page, "page"),
            (Detail, "detail"),
            (Spare, "detail_spare"),
            (Store, "store"),
            (Open, "open"),
            (Market, "market"),
            (MarketOptIn, "market_optin"),
        ] {
            let mut st = state(lang, s);
            snap(
                &mut all,
                &format!("launcher_collection_{tag}_{}", lang.code()),
                launcher::SIZE,
                Edition::E1718,
                |ui| {
                    launcher::show(ui, &mut st);
                },
            );
        }
    }
    let mut st = state(Lang::En, Choose);
    st.tab = launcher::Tab::Play;
    snap(
        &mut all,
        "launcher_play_create_a_team_en",
        launcher::SIZE,
        Edition::E1718,
        |ui| {
            launcher::show(ui, &mut st);
        },
    );
    all.unwrap();
}
