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

/// 2017-18 HOST ARCADE's dialog: "List my arcade", the arcade's name, HOST ARCADE and CANCEL over the ONLINE tab.
/// ONLINE has no PLAY: its JOIN joins the code typed, CONNECT in the Direct address fold plays online; PLAY on the
/// PLAY tab with Online chosen shows ONLINE instead of playing.
#[test]
fn launcher_online_starts_from_the_lobby() {
    let _gpu = gpu();
    let mut start =
        launcher::LauncherState::sample(Edition::E1718, Lang::En, launcher::Scenario::Online);
    (start.lobby.code, start.lobby.direct_open) = ("k7qm2x".into(), true);
    let st = std::cell::RefCell::new(start);
    let got = std::cell::RefCell::new(None);
    let mut h = harness(launcher::SIZE, 1.0, Edition::E1718, |ui| {
        if let Some(a) = launcher::show(ui, &mut st.borrow_mut()) {
            *got.borrow_mut() = Some(a);
        }
    });
    h.run_steps(4);
    // only the tab is called PLAY
    assert_eq!(h.query_all_by_label("PLAY").count(), 1, "no PLAY on ONLINE");
    h.get_by_label("JOIN").click();
    h.run_steps(2);
    assert_eq!(
        got.take(),
        Some(launcher::Action::Lobby(lobby::LobbyAction::JoinCode(
            "K7QM2X".into()
        )))
    );
    h.get_by_label("CONNECT").click();
    h.run_steps(2);
    assert_eq!(got.take(), Some(launcher::Action::Play));
    assert!(st.borrow().online);
    st.borrow_mut().tab = launcher::Tab::Play;
    h.run_steps(2);
    // the footer's PLAY: the lowest
    h.get_all_by_label("PLAY")
        .max_by(|a, b| a.rect().bottom().total_cmp(&b.rect().bottom()))
        .unwrap()
        .click();
    h.run_steps(2);
    assert_eq!(got.take(), None);
    assert_eq!(st.borrow().tab, launcher::Tab::Online);
}

#[test]
fn launcher_host_dialog() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Online);
        let v = &mut st.lobby;
        (v.shop_rooms, v.kind, v.list_shop) = (true, lobby::Kind::Shops, true);
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

/// Owner, 2026-10-31, on hold: 2017-18 shows no PAUSE button on the page bar, no VS ROOMS tab and no HOST VS; 2010-11
/// keeps its own. The shop side (ARCADES tab, HOST ARCADE) stays.
#[test]
fn vs_lobby_and_pause_button_hidden_1718() {
    let _gpu = gpu();
    for (ed, shown) in [(Edition::E1718, panel::PAUSE_ON), (Edition::E1011, true)] {
        let st =
            std::cell::RefCell::new(panel::PanelState::sample(ed, Lang::En, panel::Page::Game));
        let mut h = harness(panel::SIZE, 1.0, ed, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(4);
        assert_eq!(
            h.query_all_by_label("PAUSE").count() == 1,
            shown,
            "{ed:?} PAUSE"
        );
    }
    for (ed, shown) in [(Edition::E1718, lobby::VS_LOBBY), (Edition::E1011, true)] {
        let st = launcher::LauncherState::sample(ed, Lang::En, launcher::Scenario::Online);
        let st = std::cell::RefCell::new(st);
        let mut h = harness(launcher::SIZE, 1.0, ed, |ui| {
            launcher::show(ui, &mut st.borrow_mut());
        });
        h.run_steps(4);
        let vs = h.query_all_by_label_contains("VS ROOMS").count() == 1;
        assert_eq!(vs, shown, "{ed:?} VS ROOMS tab");
        if ed == Edition::E1718 {
            assert_eq!(
                h.query_all_by_label("HOST ARCADE").count(),
                1,
                "HOST ARCADE stays"
            );
        }
        assert!(
            h.query_all_by_label_contains("ARCADES").count() >= 1,
            "{ed:?} ARCADES tab"
        );
    }
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

/// 2017-18: RESET LINEUP under the board, lit after a match when the table changed, greyed out in a match.
#[test]
fn board_reset_lineup() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for (name, ready, in_match) in [
        ("board_reset_lineup_en", true, false),
        ("board_reset_lineup_in_match_en", true, true),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
        st.reset_ready = ready;
        st.in_match = in_match;
        snap(&mut all, name, panel::SIZE, Edition::E1718, |ui| {
            panel::show(ui, &mut st)
        });
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
fn import_asked(lang: Lang) -> panel::PanelState {
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

/// FORMATIONS (issue #89) during a match: two slots filled (one from another club), LOAD greyed out with its reason,
/// slot 2 being named for SAVE (REPLACE: it is filled).
#[test]
fn formations_dialog() {
    use forza::cards::{Placed, formations};
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Catalogue);
        st.catalogue.filter = panel::Filter::default();
        let t = [Placed {
            slot: 1,
            no: 1,
            x: 0.0,
            y: 0.9,
        }];
        let slot = |name, club| {
            formations::slot_of(&formations::text(&t, st.edition, name, club), st.edition)
        };
        let mut slots = vec![None; formations::SLOTS];
        slots[0] = slot("4-3-3 attack", "KITAKAZE");
        slots[1] = slot("Cup XI", "OTHER FC");
        st.formations = Some(panel::Formations {
            slots,
            naming: Some((1, "Cup XI".into())),
        });
        st.in_match = true;
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

/// CONFIRM and Enter ask for the import; CANCEL and Esc drop the question and stay on the catalogue.
#[test]
fn import_dialog_answers() {
    let _gpu = gpu();
    for (key, label, go) in [
        (Some(egui::Key::Enter), "", true),
        (None, "CONFIRM", true),
        (Some(egui::Key::Escape), "", false),
        (None, "CANCEL", false),
    ] {
        let st = std::cell::RefCell::new(import_asked(Lang::En));
        let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(4);
        assert!(!st.borrow().import_go && st.borrow().import_ask.is_some());
        match key {
            Some(k) => h.key_press(k),
            None => h.get_by_label(label).click(),
        }
        h.run_steps(4);
        let s = st.borrow();
        assert_eq!(
            (s.import_go, s.import_ask.is_some()),
            (go, go),
            "{key:?} {label}"
        );
        // the dialog's keys are its own: no card added, the page still open
        assert!(s.add.is_none() && s.page == panel::Page::Catalogue);
        assert!(s.import_undo.is_none());
    }
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

/// HOST SHOP while the shop and this PC's seat run: the room code, COPY and LEAVE stay live, the rest is greyed.
#[test]
fn launcher_host_shop() {
    let _gpu = gpu();
    let mut all = SnapshotResults::new();
    for lang in Lang::ALL {
        let mut st =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Online);
        st.running = true;
        let v = &mut st.lobby;
        (v.shop_rooms, v.hosting, v.host_code) = (true, true, Some("K7QM2XPD".into()));
        v.kind = lobby::Kind::Shops;
        snap(
            &mut all,
            &format!("launcher_host_shop_{}", lang.code()),
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

/// The filters by their labels: each check box toggles its filter and the language marks pick one of four.
#[test]
fn lobby_filters() {
    let _gpu = gpu();
    let v = std::cell::RefCell::new(lobby::LobbyView::sample(lobby::Scenario::List));
    let mut h = harness(lobby::SIZE, 1.0, Edition::E1718, |ui| {
        lobby::show(ui, &mut v.borrow_mut(), Lang::En);
    });
    h.run_steps(4);
    let shown = |v: &lobby::LobbyView| v.visible().len();
    assert_eq!(shown(&v.borrow()), 3);
    h.get_by_label("Open arcades only").click();
    h.run_steps(2);
    assert!(v.borrow().open_only);
    assert_eq!(shown(&v.borrow()), 1);
    h.get_by_label("Open arcades only").click();
    h.get_by_label("Ping under 150 ms").click();
    h.run_steps(2);
    assert!(!v.borrow().open_only && v.borrow().ping_ok);
    assert_eq!(shown(&v.borrow()), 2);
    // the room language filter is hidden for now
    assert!(h.query_by_label("IT").is_none() && h.query_by_label("All").is_none());
}

/// 2017-18 with the shared shop on: HOST SHOP (and H) opens the host dialog (its HOST gives HostShop with the switch as
/// set, CANCEL and Esc put it back), the code typed and JOINed in capitals, the listed room joined from its row; while
/// hosting only Esc (LEAVE) answers.
#[test]
fn lobby_shop_rooms() {
    use lobby::LobbyAction::*;
    let _gpu = gpu();
    let mut start = lobby::LobbyView::sample(lobby::Scenario::ShopNoRoom);
    (start.notice, start.code) = (None, String::new());
    let v = std::cell::RefCell::new(start);
    let got = std::cell::RefCell::new(None);
    let mut h = harness(lobby::SIZE, 1.0, Edition::E1718, |ui| {
        if let Some(a) = lobby::show(ui, &mut v.borrow_mut(), Lang::En) {
            *got.borrow_mut() = Some(a);
        }
    });
    h.run_steps(4);
    h.get_by_label("HOST ARCADE").click();
    h.run_steps(2);
    assert_eq!(got.take(), None);
    assert!(v.borrow().host_ask.is_some());
    h.get_by_label("List my arcade when I host").click();
    h.get_by_label("In N min").click();
    h.run_steps(2);
    assert_eq!(v.borrow().cup.mode, lobby::CupMode::In);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(got.take(), None);
    assert!(v.borrow().host_ask.is_none() && !v.borrow().list_shop);
    assert_eq!(
        v.borrow().cup,
        lobby::CupChoice::default(),
        "CANCEL puts the cup back"
    );
    h.key_press(egui::Key::H);
    h.run_steps(2);
    h.get_by_label("List my arcade when I host").click();
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(got.take(), None, "Esc closes the dialog, not the lobby");
    assert!(v.borrow().host_ask.is_none() && !v.borrow().list_shop);
    h.key_press(egui::Key::H);
    h.run_steps(2);
    h.get_by_label("List my arcade when I host").click();
    h.get_by_label("At HH:MM").click();
    // a time that would open after midnight: refused, HOST does nothing
    let cup_at = |clock, at| {
        let mut b = v.borrow_mut();
        (b.clock, b.cup.at) = (Some(clock), at);
    };
    cup_at(23 * 3600 + 52 * 60, 23 * 60 + 59);
    h.run_steps(2);
    h.get_by_label("The cup can't open after midnight or before 05:00.");
    h.get_all_by_label("HOST ARCADE").last().unwrap().click();
    h.run_steps(2);
    assert_eq!(got.take(), None, "a refused cup time keeps the dialog open");
    cup_at(22 * 3600, 23 * 60 + 30);
    h.run_steps(2);
    h.get_by_label(
        "Opens at about 23:30. Card in before then; entry closes about 1.5 min after it opens.",
    );
    // the tab's HOST ARCADE, then the dialog's
    h.get_all_by_label("HOST ARCADE").last().unwrap().click();
    h.run_steps(2);
    assert_eq!(got.take(), Some(HostShop));
    assert!(v.borrow().host_ask.is_none() && v.borrow().list_shop);
    assert_eq!(
        (v.borrow().cup.mode, v.borrow().cup.at),
        (lobby::CupMode::At, 23 * 60 + 30),
        "HOST keeps the cup for the caller to save"
    );
    v.borrow_mut().code = " k7qm2xpd".into();
    h.run_steps(2);
    // one JOIN: the code typed, then the listed room row once clicked (the click empties the box)
    assert_eq!(h.get_all_by_label("JOIN").count(), 1);
    h.get_by_label("JOIN").click();
    h.run_steps(2);
    assert_eq!(got.take(), Some(JoinCode("K7QM2XPD".into())));
    h.get_by_label(forza::online::lobby::SHOP_ROOM_NAME).click();
    h.run_steps(2);
    assert!(v.borrow().code.is_empty());
    h.get_by_label("JOIN").click();
    h.run_steps(2);
    assert_eq!(got.take(), Some(Join("p1".into())));
    v.borrow_mut().hosting = true;
    h.run_steps(2);
    for key in [egui::Key::H, egui::Key::Enter] {
        h.key_press(key);
        h.run_steps(2);
        assert_eq!(got.take(), None, "{key:?} while hosting");
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(got.take(), Some(Leave));
}

/// The LAN fold: HOST LAN SHOP, a found shop's JOIN and a typed address's JOIN each give their action; hosting hides it.
#[test]
fn lobby_lan() {
    use lobby::LobbyAction::*;
    let _gpu = gpu();
    let v = std::cell::RefCell::new(lobby::LobbyView::sample(lobby::Scenario::LanList));
    let got = std::cell::RefCell::new(None);
    let size = lobby::SIZE + egui::vec2(0.0, 280.0);
    let mut h = harness(size, 1.0, Edition::E1718, |ui| {
        if let Some(a) = lobby::show(ui, &mut v.borrow_mut(), Lang::En) {
            *got.borrow_mut() = Some(a);
        }
    });
    h.run_steps(4);
    h.get_by_label("HOST LAN ARCADE").click();
    h.run_steps(2);
    assert_eq!(got.take(), Some(HostLan));
    // the JOINs: the list's, the two found shops', the typed address's
    let joins: Vec<_> = h.get_all_by_label("JOIN").collect();
    assert_eq!(joins.len(), 4);
    joins[1].click();
    h.run_steps(2);
    assert_eq!(got.take(), Some(JoinLan("192.168.1.20".into())));
    v.borrow_mut().lan_address = " 10.0.0.7:20031 ".into();
    h.run_steps(2);
    let last = h.get_all_by_label("JOIN").last().unwrap();
    last.click();
    h.run_steps(2);
    assert_eq!(got.take(), Some(JoinLan("10.0.0.7:20031".into())));
    v.borrow_mut().hosting = true;
    h.run_steps(2);
    assert!(h.query_by_label("HOST LAN ARCADE").is_none());
}

/// `list_fit`: in a short tab the list gives up rows so the actions and the Direct address fold stay in view, and
/// never drops under three rows.
#[test]
fn lobby_list_fits_a_short_tab() {
    let _gpu = gpu();
    let mut v = lobby::LobbyView::sample(lobby::Scenario::List);
    v.list_fit = true;
    let mut h = harness(Vec2::new(lobby::SIZE.x, 470.0), 1.0, Edition::E1011, |ui| {
        lobby::show(ui, &mut v, Lang::En);
    });
    h.run_steps(6);
    let bottom = h.get_by_label_contains("Direct address").rect().bottom();
    assert!(bottom <= 470.0, "the Direct address fold ends at {bottom}");
    assert!(h.get_by_label("HOST VS").rect().top() > h.get_by_label("MILANO CUP").rect().bottom());
}
