//! Every screen rendered to PNG in English, Italian and Japanese: what a reviewer looks at, then the regression check
//! (`UPDATE_SNAPSHOTS=1` writes new ones). Rendered with wgpu on the machine's GPU.

use egui::{Ui, Vec2};
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, SnapshotResults};
use forza_ui::Lang;
use forza_ui::theme::{self, Edition};
use forza_ui::{installer, launcher, lobby, panel, sizing};

/// One test on the GPU at a time: wgpu devices made in parallel crashed the test process now and then (exit 116,
/// about 1 run in 4 on 2026-10-12); one after another they never did.
fn gpu() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn harness<'a>(
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
        for (s, ed, tag) in [
            (Ready, Edition::E1011, "ready"),
            (Running, Edition::E1718, "running"),
            (StopArmed, Edition::E1718, "stop"),
            (NotSetUp, Edition::E1011, "setup"),
            (Update, Edition::E1011, "update"),
            (Busy, Edition::E1718, "busy"),
            (Report, Edition::E1718, "report"),
            (CleanedUp, Edition::E1718, "cleanup"),
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
    assert_eq!(size, egui::vec2(600.0, 700.0));
    assert_eq!(launcher::window_size(None), launcher::SIZE);
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
        if s != Online {
            let play = h
                .get_all_by_label("PLAY")
                .max_by(|a, b| a.rect().bottom().total_cmp(&b.rect().bottom()))
                .unwrap()
                .rect();
            assert!(play.bottom() <= size.y - 14.0, "{tag}: PLAY at {play:?}");
        }
        h.snapshot(format!("launcher_short_{tag}_en"));
        all.extend_harness(&mut h);
    }
    all.unwrap();
}

/// The ONLINE tab: the lobby inside the launcher, the Direct address fold closed, then open.
#[test]
fn launcher_online() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (open, name) in [
        (false, "launcher_online_en"),
        (true, "launcher_online_direct_en"),
    ] {
        let mut st =
            launcher::LauncherState::sample(Edition::E1011, Lang::En, launcher::Scenario::Online);
        st.lobby.direct_open = open;
        snap(&mut all, name, launcher::SIZE, Edition::E1011, |ui| {
            launcher::show(ui, &mut st);
        });
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
        v.host_ask = Some((false, String::new(), lobby::CupChoice::default()));
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
            // SETTINGS on 2017-18: its projector and controller switches
            let ed = if matches!(page, panel::Page::Keys | panel::Page::Settings) {
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
        // the pages the 2010-11 shots above cover, as 2017-18 draws them: its arcade line on the game page, its
        // own filters in the catalogue, the club card's share code
        for page in [
            panel::Page::Game,
            panel::Page::Catalogue,
            panel::Page::ClubCard,
        ] {
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
    let mut st = panel::PanelState::sample(Edition::E1011, Lang::It, panel::Page::Game);
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
    snap(
        &mut all,
        "panel_settings_textures_1718_ja",
        panel::SIZE,
        Edition::E1718,
        |ui| panel::show(ui, &mut st),
    );
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
        let banner = h.get_by_label(text).rect();
        assert!(
            banner.top() < 120.0,
            "{lang:?}: the notice sits at the top of the picture, not at {banner:?}"
        );
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
    // a click keeps the big card; its BACK shows the back, the position map with the ratings and the stats
    h.drag_at(at(2));
    h.run_steps(1);
    h.drop_at(at(2));
    h.hover_at(at(2));
    h.run_steps(3);
    h.get_by_label("BACK").click();
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
        st.hover_delay = Some(0.0);
        let mut h = harness(size, 1.0, Edition::E1011, |ui| panel::show(ui, &mut st));
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
    for (w, h, ed, lang, buttons, name) in [
        (
            960.0,
            1232.0,
            Edition::E1718,
            Lang::En,
            false,
            "popout_board_960x1232_en",
        ),
        (
            1080.0,
            1920.0,
            Edition::E1718,
            Lang::En,
            false,
            "popout_board_1080x1920_en",
        ),
        (
            2360.0,
            1640.0,
            Edition::E1011,
            Lang::En,
            false,
            "popout_board_2360x1640_en",
        ),
        (
            1920.0,
            1080.0,
            Edition::E1718,
            Lang::En,
            false,
            "popout_board_1920x1080_en",
        ),
        (
            1920.0,
            1080.0,
            Edition::E1011,
            Lang::It,
            false,
            "popout_board_1920x1080_it",
        ),
        (
            1080.0,
            1920.0,
            Edition::E1718,
            Lang::En,
            true,
            "popout_board_buttons_1080x1920_en",
        ),
        (
            1920.0,
            1080.0,
            Edition::E1011,
            Lang::It,
            true,
            "popout_board_buttons_1920x1080_it",
        ),
    ] {
        let mut st = panel::PanelState::sample(ed, lang, panel::Page::Game);
        st.desk = true;
        st.file.board_window = true;
        st.file.board_buttons = buttons;
        assert_eq!(panel::board_scale(w, h, 96.0), 1.0);
        snap(&mut all, name, Vec2::new(w, h), ed, |ui| {
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
    for (w, h, ed, lang, name) in [
        (
            1920.0,
            1080.0,
            Edition::E1718,
            Lang::En,
            "popout_game_1920x1080_en",
        ),
        (
            2560.0,
            1440.0,
            Edition::E1718,
            Lang::En,
            "popout_game_2560x1440_en",
        ),
        (
            1920.0,
            1080.0,
            Edition::E1011,
            Lang::It,
            "popout_game_1920x1080_it",
        ),
        (
            2560.0,
            1440.0,
            Edition::E1011,
            Lang::En,
            "popout_game_2560x1440_1011_en",
        ),
    ] {
        let mut st = panel::PanelState::sample(ed, lang, panel::Page::Game);
        st.file.board_window = true;
        st.popped = true;
        let ci = st.board.slots[2].expect("a card in slot 2");
        st.touched = Some((st.board.cards[ci].clone(), 0.0));
        assert_eq!(panel::scale(w, h, 96.0), 1.0);
        snap(&mut all, name, Vec2::new(w, h), ed, |ui| {
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
/// forward with its partner list; the catalogue's detail pane shows the same card, its picture in the room the board's
/// lists take.
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
        let seen = panel::catalogue_detail_rect(&h.ctx).expect("the detail pane drawn");
        let add = match lang {
            Lang::En => "+ ADD",
            Lang::It => "+ AGGIUNGI",
            Lang::Ja => "＋追加",
        };
        let add = h.get_by_label(add).rect();
        assert!(
            add.bottom() <= seen.bottom(),
            "{size:?} {ppp} {lang:?}: ADD at {add:?}, the pane shows {seen:?}"
        );
        // the detail pane lists no affinity: the card's partners and their "+N more" chip are gone (the board's
        // lists stay; players read affinity in their own tool)
        assert!(
            h.query_by_label("+1 more").is_none(),
            "{size:?} {ppp} {lang:?}: the pane still lists the card's partners"
        );
        if size == wide {
            // the detail picture's slot: the album art at its own pixels, and never under them, so larger than a grid
            // card (at most 170 points wide, its scan some 240 tall)
            let pic = panel::catalogue_picture_rect(&h.ctx).expect("the picture drawn");
            assert!(
                pic.width() * ppp >= sizing::ALBUM.x - 1.0
                    && pic.height() * ppp >= sizing::ALBUM.y - 1.0,
                "{size:?} {ppp} {lang:?}: the picture {pic:?}, the album art {}",
                sizing::ALBUM / ppp
            );
        }
        if let Some(name) = name {
            h.snapshot(name);
            all.extend_harness(&mut h);
        }
    }
    all.unwrap();
}

/// Every widget's rect and the detail pane's (its picture slot too), by role and label: what a click must not move.
fn catalogue_rects(h: &Harness<'_>) -> Vec<(String, egui::Rect)> {
    use egui_kittest::kittest::NodeT as _;
    let mut v: Vec<_> = h
        .root()
        .children_recursive()
        .filter_map(|n| {
            let a = n.accesskit_node();
            let b = a.bounding_box()?;
            let r = egui::Rect::from_min_max(
                egui::pos2(b.x0 as f32, b.y0 as f32),
                egui::pos2(b.x1 as f32, b.y1 as f32),
            );
            Some((format!("{:?} {:?}", a.role(), a.label()), r))
        })
        .collect();
    v.extend(panel::catalogue_detail_rect(&h.ctx).map(|r| ("detail pane".to_owned(), r)));
    v.extend(panel::catalogue_picture_rect(&h.ctx).map(|r| ("picture slot".to_owned(), r)));
    v.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.min.x.total_cmp(&b.1.min.x))
            .then(a.1.min.y.total_cmp(&b.1.min.y))
    });
    v
}

/// BACK in the detail pane (a scan front smaller than the album art, an album back, as an install has them) moves
/// nothing: every rect (the search line, its buttons, the filters, the grid, the pane, the picture slot, the stats,
/// ADD) is the same on the frame before the click, while it is pressed, right after it and as the frames go on; and
/// the back's picture loads while the card sits in the pane, before BACK is clicked, so no drawn stand-in flashes.
#[test]
fn catalogue_back_keeps_still() {
    let _gpu = gpu();
    let dir = tempfile::tempdir().expect("a temp folder");
    let ci = 7;
    let no = forza_ui::sample::cards()[ci].number;
    for (w, h, name) in [
        (180, 254, format!("{no}_f.jpg")),
        (256, 357, format!("{no}_b.jpg")),
    ] {
        image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, 255])
        })
        .save_with_format(dir.path().join(name), image::ImageFormat::Png)
        .expect("a picture written");
    }
    for (size, ppp) in [
        (Vec2::new(1920.0, 1080.0), 1.0),
        (Vec2::new(1920.0, 1080.0), 1.25),
        (Vec2::new(1440.0, 900.0), 1.0),
        (panel::SIZE, 1.0),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Catalogue);
        st.message = None; // the sample's note line at the pane's foot is no part of the claim
        st.catalogue.filter = Default::default();
        st.catalogue.filter.search = st.cards[ci].name.clone();
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
        // the front (the grid's own picture) is ready at once; the pane starts the back two frames after the card
        // arrives. Waiting for that load is the claim: a change that starts it a second later, or on the click as
        // before, leaves the back unloaded past the 30 frames a player's read takes here.
        let mut loaded = false;
        for _ in 0..30 {
            h.run_steps(1);
            loaded = panel::catalogue_other_side_ready(&h.ctx) == Some(true);
            if loaded {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(
            loaded,
            "{size:?} {ppp}: the back's picture is not loaded before the click"
        );
        assert_eq!(
            panel::catalogue_picture_shown(&h.ctx),
            Some((false, true)),
            "{size:?} {ppp}: the front's picture shown"
        );
        let at = h.get_by_label("BACK").rect().center();
        h.hover_at(at);
        h.run_steps(1);
        let before = catalogue_rects(&h);
        h.drag_at(at); // pressed
        h.run_steps(1);
        assert_eq!(
            catalogue_rects(&h),
            before,
            "{size:?} {ppp}: BACK pressed moved a rect"
        );
        h.drop_at(at); // released: the click
        let clicked = std::time::Instant::now();
        let mut shown_after = None;
        for frame in 0..20 {
            h.run_steps(1);
            assert_eq!(
                catalogue_rects(&h),
                before,
                "{size:?} {ppp}: frame {frame} after BACK moved a rect"
            );
            let shown = panel::catalogue_picture_shown(&h.ctx);
            if shown == Some((true, true)) && shown_after.is_none() {
                shown_after = Some((frame, clicked.elapsed()));
            }
            eprintln!("{size:?} {ppp}: frame {frame} after BACK: {shown:?}");
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        eprintln!("{size:?} {ppp}: back picture shown at (frame, time) {shown_after:?}");
        assert_eq!(
            shown_after.map(|(f, _)| f),
            Some(0),
            "{size:?} {ppp}: the back's picture is not ready on the click's frame"
        );
    }
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

/// A table kept by an earlier run (no match in this one) that differs from the board: RESET LINEUP is lit and asks
/// first. CONFIRM asks for the reset (the board becomes the kept table, with UNDO); CANCEL leaves the board alone;
/// DON'T ASK AGAIN resets too and saves `reset_ask=off`: the next RESET LINEUP resets with no question, until the
/// setting is on again.
#[test]
fn board_reset_lineup_asks_first() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for choice in ["CONFIRM", "DON'T ASK AGAIN", "CANCEL"] {
        let reset = choice != "CANCEL";
        let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
        let now = forza::cards::table_after(&[], &st.board);
        // the kept table: the first pitch card and the first sub trade places
        let mut kept = now.clone();
        let p = kept.iter().position(|c| !c.on_bench()).unwrap();
        let b = kept.iter().position(|c| c.on_bench()).unwrap();
        (kept[p].no, kept[b].no) = (kept[b].no, kept[p].no);
        panel::keep_saved(&mut st, kept.clone(), std::time::SystemTime::now());
        st.before_match_at = Some("21:40".into()); // a fixed time for the picture
        panel::lineup_tick(&mut st, None, Some((false, false)), &now);
        assert!(st.reset_ready && !st.in_match && !st.match_seen);
        let shown = |b: &forza::board::Board| (0..16).map(|i| b.number(i)).collect::<Vec<_>>();
        let before = shown(&st.board);
        let after = shown(&forza::cards::board(&kept, &st.cards));
        let st = std::cell::RefCell::new(st);
        let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(4);
        // lit: a greyed-out button would not take the click
        h.get_by_label("RESET LINEUP").click();
        h.run_steps(8);
        assert_eq!(st.borrow().reset_ask, Some(false));
        assert!(!st.borrow().reset_lineup, "nothing replaced before CONFIRM");
        if choice == "CONFIRM" {
            h.snapshot("board_reset_ask_en");
            all.extend_harness(&mut h);
        }
        h.get_by_label(choice).click();
        h.run_steps(4);
        {
            let mut s = st.borrow_mut();
            assert!(s.reset_ask.is_none());
            assert_eq!(s.reset_lineup, reset, "{choice}");
            assert_eq!(
                s.file.reset_ask_off,
                choice == "DON'T ASK AGAIN",
                "{choice}"
            );
            assert_eq!(s.file_changed, choice == "DON'T ASK AGAIN", "{choice}");
            // the panel DLL's step after the frame
            let new = panel::take_reset(&mut s, &now, false);
            if reset {
                assert_eq!(new.as_ref(), Some(&kept));
                assert_eq!(shown(&s.board), after);
                assert_eq!(s.import_undo.as_ref().map(|u| &u.0), Some(&now));
            } else {
                assert_eq!(new, None);
                assert_eq!(shown(&s.board), before);
                assert!(s.import_undo.is_none());
            }
        }
        h.run_steps(4);
        assert_eq!(h.query_by_label("UNDO").is_some(), reset);
        if choice == "DON'T ASK AGAIN" {
            // asked no more: the next click resets at once
            h.get_by_label("RESET LINEUP").click();
            h.run_steps(4);
            assert!(st.borrow().reset_ask.is_none(), "no question");
            assert!(st.borrow().reset_lineup, "reset at once");
            st.borrow_mut().reset_lineup = false;
            // SETTINGS' "Ask before RESET LINEUP" on: the question is back
            st.borrow_mut().page = panel::Page::Settings;
            h.run_steps(4);
            h.get_by_label("Ask before RESET LINEUP").click();
            h.run_steps(4);
            assert!(!st.borrow().file.reset_ask_off, "the setting is on");
            st.borrow_mut().page = panel::Page::Game;
            h.run_steps(4);
            h.get_by_label("RESET LINEUP").click();
            h.run_steps(4);
            assert_eq!(st.borrow().reset_ask, Some(false), "asked again");
            assert!(!st.borrow().reset_lineup);
        }
    }
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
    pictures_loaded(&mut h);
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

/// 2017-18 SETTINGS: CLEAR BAD ENDINGS (no CLUB CARD page there) asks for the reset by its second click.
#[test]
fn settings_1718_clear_bad_endings() {
    use forza_ui::panel::ClubPick;
    use std::cell::RefCell;
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
    let st = RefCell::new(panel::PanelState::sample(
        Edition::E1718,
        Lang::En,
        panel::Page::Settings,
    ));
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(3);
    h.get_by_label("CLEAR BAD ENDINGS").click();
    h.run_steps(2);
    assert!(st.borrow().club_request.is_none(), "armed, not asked");
    h.get_by_label("CLICK AGAIN").click();
    h.run_steps(2);
    assert_eq!(st.borrow_mut().club_request.take(), Some(ClubPick::Fix));
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
            s.file.compact
                && s.file_changed
                && s.file.hover_ms == Some(2000)
                && s.hover_delay == Some(2.0)
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

/// IMPORT LINEUP's question over the catalogue: the cards that come in and those skipped, CONFIRM and CANCEL.
pub(super) fn import_asked(lang: Lang) -> panel::PanelState {
    let mut st = panel::PanelState::sample(Edition::E1011, lang, panel::Page::Catalogue);
    st.catalogue.filter = panel::Filter::default();
    let text = "game 1011\nP 1 50 92\nP 2 15 70\nP 3 38 74\nS 999\n";
    assert!(panel::ask_import(&mut st, text, false));
    st
}

#[test]
fn import_dialog() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st = import_asked(lang);
        snap(
            &mut all,
            &format!("panel_import_dialog_{}", lang.code()),
            panel::SIZE,
            Edition::E1011,
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
        assert!(
            st.borrow().import_help && !st.borrow().import,
            "{lang:?}: the dialog first"
        );
        h.get_by_label(&t(lang, "cat.import_help"));
        // the address the dialog shows is the one the DLL opens and copies, and the import reads the site's share
        // links off that board route
        h.get_by_label(panel::LINEUP_PAGE.trim_start_matches("https://"));
        assert!(
            forza::cards::lineup::share_id(&format!("{}?s=Ab3dE7x", panel::LINEUP_PAGE)).is_some(),
            "{} is not the board route the site's share links use",
            panel::LINEUP_PAGE
        );
        h.get_by_label(&t(lang, "cat.site_open")).click();
        h.run_steps(1);
        assert!(
            st.borrow().site_open,
            "{lang:?}: OPEN THE SITE asks the DLL"
        );
        // Enter takes the focused button: on COPY it copies and does not start the import
        h.get_by_label(&t(lang, "cat.site_copy")).focus();
        h.run_steps(1);
        h.key_press(egui::Key::Enter);
        h.run_steps(1);
        assert!(
            st.borrow().site_copy && !st.borrow().import && st.borrow().import_help,
            "{lang:?}: Enter on COPY must not import"
        );
        // the DLL took both clicks; then its COPY failed: the refusal is drawn in the dialog, not on the page's line
        st.borrow_mut().site_copy = false;
        st.borrow_mut().site_open = false;
        st.borrow_mut().site_copied = false;
        st.borrow_mut()
            .say_site(forza_ui::widgets::Tone::Refused, "cat.msg_site_not_copied");
        h.run_steps(2);
        h.get_by_label(&t(lang, "cat.msg_site_not_copied"));
        assert!(
            st.borrow().message.is_none(),
            "{lang:?}: the refusal went to the page's note line"
        );
        // the DLL copied it
        st.borrow_mut().site_copied = true;
        st.borrow_mut().site_note = None;
        h.run_steps(3);
        h.get_by_label(&t(lang, "cat.site_copied"));
        h.snapshot(format!("panel_import_help_{}", lang.code()));
        all.extend_harness(&mut h);
        h.get_by_label(&t(lang, "cat.import_paste")).click();
        h.run_steps(2);
        let s = st.borrow();
        assert!(
            s.import && !s.import_help && !s.site_copied,
            "{lang:?}: the import asked, the dialog closed"
        );
    }
    // Esc closes the dialog without reading the clipboard
    let st = std::cell::RefCell::new(panel::PanelState::sample(
        Edition::E1718,
        Lang::En,
        panel::Page::Catalogue,
    ));
    st.borrow_mut().message = None;
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    h.get_by_label("IMPORT LINEUP").click();
    h.run_steps(2);
    assert!(st.borrow().import_help, "the dialog is open");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(
        !st.borrow().import_help && !st.borrow().import,
        "Esc must close the dialog without importing"
    );
    // 2010-11: IMPORT LINEUP reads the clipboard at once
    let st1011 = std::cell::RefCell::new(panel::PanelState::sample(
        Edition::E1011,
        Lang::En,
        panel::Page::Catalogue,
    ));
    let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st1011.borrow_mut())
    });
    h.run_steps(4);
    h.get_by_label("IMPORT LINEUP").click();
    h.run_steps(2);
    assert!(st1011.borrow().import && !st1011.borrow().import_help);
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

/// A FORMATIONS dialog with two filled slots (the second from another club).
pub(super) fn formations_state(lang: Lang, in_match: bool) -> panel::PanelState {
    use forza::cards::{Placed, formations};
    let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
    let t = [Placed {
        slot: 1,
        no: 1,
        x: 0.0,
        y: 0.9,
    }];
    let slot = |name, club, day: &str| {
        let mut s = formations::slot_of(&formations::text(&t, st.edition, name, club), st.edition);
        if let Some(s) = s.as_mut() {
            s.saved = day.into();
        }
        s
    };
    let mut slots = vec![None; formations::SLOTS];
    slots[0] = slot("4-3-3 attack", "KITAKAZE", "2026-10-30");
    slots[1] = slot("Cup XI", "OTHER FC", "2026-11-01");
    st.formations = Some(panel::Formations {
        slots,
        naming: None,
    });
    st.in_match = in_match;
    st
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

/// The crash screen in the board's place; RESTART THE GAME and SAVE LOGS each ask once.
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
    for (label, restart) in [("RESTART THE GAME", true), ("SAVE LOGS", false)] {
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

/// The big card on the docked 2017-18 board at its least width (the divider dragged to 420 points): a click on a mat
/// card shows it big on its front (the click does not turn it over); its FRONT | BACK switch, a click on the big card
/// and FLIP's key turn it over and back; another card opens on its front. The pane, its card and its switch stay
/// inside the board's column, clear of the game's picture.
#[test]
fn board_card_flips_at_the_least_width() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in [Lang::En, Lang::It, Lang::Ja] {
        let size = panel::SIZE;
        let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
        st.file.board_width = Some(panel::BOARD_MIN as u16);
        st.hover_delay = None; // no big card by resting: only a click shows one
        let l = panel::layout_at(sheet, false, Some(panel::BOARD_MIN));
        assert!(
            (l.board.width() - panel::BOARD_MIN).abs() < 0.5,
            "{:?}",
            l.board
        );
        let slots: Vec<usize> = (0..11)
            .filter(|&s| st.board.slots[s].is_some())
            .take(2)
            .collect();
        let number = |st: &panel::PanelState, s: usize| st.board.number(s).unwrap();
        let (a, b) = (number(&st, slots[0]), number(&st, slots[1]));
        let at =
            |s| panel::board_slot_center_1718(sheet, Some(panel::BOARD_MIN), &st.board.spots, s);
        let (at_a, at_b) = (at(slots[0]), at(slots[1]));
        let st = std::cell::RefCell::new(st);
        let mut h = harness(size, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        let (front, back) = match lang {
            Lang::En => ("FRONT", "BACK"),
            Lang::It => ("FRONTE", "RETRO"),
            Lang::Ja => ("表面", "裏面"),
        };
        let click = |h: &mut Harness, p: egui::Pos2| {
            h.drag_at(p);
            h.run_steps(1);
            h.drop_at(p);
            h.run_steps(3);
        };
        h.run_steps(4);
        assert!(h.query_by_label(back).is_none(), "nothing big yet");
        // a mat card clicked: big, on its front
        click(&mut h, at_a);
        assert_eq!(st.borrow().turned, None, "a mat card click does not flip");
        let big = h
            .ctx
            .memory(|m| m.area_rect(egui::Id::unique("board-big")))
            .expect("the card shown big");
        let (f, k) = (h.get_by_label(front).rect(), h.get_by_label(back).rect());
        for r in [big, f, k] {
            assert!(
                l.board.contains_rect(r),
                "{lang:?}: {r:?} outside the board {:?}",
                l.board
            );
            assert!(!r.intersects(l.picture), "{lang:?}: {r:?} on the picture");
        }
        assert!(
            big.contains_rect(f.union(k)),
            "{lang:?}: the switch outside its pane"
        );
        // the big card: over the switch, inside the pane
        let card = egui::pos2(f.center().x, f.top() - 40.0);
        assert!(
            big.contains(card) && card.y > big.top() + 20.0,
            "{lang:?}: {big:?}"
        );
        // BACK, then FRONT
        h.get_by_label(back).click();
        h.run_steps(3);
        assert_eq!(st.borrow().turned, Some(a), "BACK shows the back");
        if lang == Lang::En {
            h.snapshot("board_flip_420_back_en");
            all.extend_harness(&mut h);
        }
        h.get_by_label(front).click();
        h.run_steps(3);
        assert_eq!(st.borrow().turned, None, "FRONT shows the face");
        // a click on the big card turns it over, and over again
        click(&mut h, card);
        assert_eq!(st.borrow().turned, Some(a), "the big card flips");
        click(&mut h, card);
        assert_eq!(st.borrow().turned, None);
        // FLIP's key: one press, one turn
        for want in [Some(a), None, Some(a)] {
            st.borrow_mut().down[forza::keys::FLIP] = true;
            h.run_steps(2);
            st.borrow_mut().down[forza::keys::FLIP] = false;
            h.run_steps(2);
            assert_eq!(st.borrow().turned, want, "FLIP's key");
        }
        // not while a controller button is being bound
        st.borrow_mut().pad_capturing = Some(0);
        st.borrow_mut().down[forza::keys::FLIP] = true;
        h.run_steps(2);
        st.borrow_mut().down[forza::keys::FLIP] = false;
        st.borrow_mut().pad_capturing = None;
        h.run_steps(2);
        assert_eq!(st.borrow().turned, Some(a), "FLIP while a pad is bound");
        // another card opens on its front
        click(&mut h, at_b);
        assert!(h.query_by_label(back).is_some(), "the other card big");
        assert_eq!(st.borrow().turned, None, "{b} opens on its front");
    }
    all.unwrap();
}
