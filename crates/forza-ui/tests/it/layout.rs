//! The panel, Launcher and lobby screens laid out without drawing: rects, labels and state after clicks. No GPU, so
//! CI's build-test runs them with the rest (the drawn screens are in `snapshots`).

use egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use forza_ui::Lang;
use forza_ui::theme::{self, Edition};
use forza_ui::{installer, launcher, lobby, panel, widgets};

use super::snapshots::{formations_state, harness, import_asked};

/// Every text shape drawn in the last frame, nested ones included: painted text (the STARTER and IMPORT marks) is
/// not on the accessibility tree, so it is checked where it lands.
fn drawn_texts(h: &egui_kittest::Harness<'_>) -> Vec<egui::epaint::TextShape> {
    fn walk(s: &egui::Shape, out: &mut Vec<egui::epaint::TextShape>) {
        match s {
            egui::Shape::Text(t) => out.push(t.clone()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = vec![];
    for s in &h.output().shapes {
        walk(&s.shape, &mut out);
    }
    out
}

/// 2010-11 keeps its pack choice (From the game | High-quality pack, Choose pack…); 2017-18 has the one download button.
#[test]
fn the_card_pictures_row_is_the_pack_choice_in_2010_11_and_the_download_in_2017_18() {
    for (ed, has, lacks) in [
        (
            Edition::E1011,
            "Choose pack…",
            "Download high-quality card pictures",
        ),
        (
            Edition::E1718,
            "Download high-quality card pictures",
            "Choose pack…",
        ),
    ] {
        let mut st = launcher::LauncherState::sample(ed, Lang::En, launcher::Scenario::NotSetUp);
        let mut h = harness(launcher::SIZE, 1.0, ed, |ui| {
            launcher::show(ui, &mut st);
        });
        h.run_steps(8);
        assert!(h.query_by_label(has).is_some(), "{ed:?}: {has}");
        assert!(h.query_by_label(lacks).is_none(), "{ed:?}: {lacks}");
    }
}

/// CLEAN UP's result shows whole on SETUP at the Launcher's size in every language (#397: in Italian it sat on its own
/// row under the button, and its frame and the bottom of its text ran under the scroll area's bottom edge). The success
/// line fits without scrolling; a long failure list is scrolled into view when it appears. As in the Launcher, the result
/// comes after SETUP is on screen: on a window's first frame the footer has no height yet, so nothing is under the fold.
#[test]
fn the_cleanup_result_fits_the_setup_page() {
    use egui::accesskit::Role;
    use forza_ui::widgets::Tone;
    for lang in Lang::ALL {
        let what = (1..=5)
            .map(|i| format!(r"data\leftover_{i}.bin: Access is denied. (os error 5)"))
            .collect::<Vec<_>>()
            .join("; ");
        let failed = forza_ui::text(lang, "launcher.cleanup_failed", &[("what", &what)]);
        for failure in [None, Some(failed)] {
            let mut st = launcher::LauncherState::sample(
                Edition::E1718,
                lang,
                launcher::Scenario::CleanedUp,
            );
            let sample = st.clean_status.take().expect("the scenario has a result");
            let result = failure.clone().map_or(sample, |f| (Tone::Refused, f));
            let note = result.1.clone();
            let st = std::cell::RefCell::new(st);
            let mut h = harness(launcher::SIZE, 1.0, Edition::E1718, |ui| {
                launcher::show(ui, &mut st.borrow_mut());
            });
            h.run_steps(4);
            st.borrow_mut().clean_status = Some(result);
            // a step is 0.25 s: the scroll to a new result (at most 0.2 s) ends well within these
            h.run_steps(4);
            let play = forza_ui::text(lang, "launcher.play", &[]);
            let play = h.get_by_role_and_label(Role::Button, &play).rect();
            let tab = forza_ui::text(lang, "launcher.setup", &[]);
            let tab = h.get_by_role_and_label(Role::RadioButton, &tab).rect();
            let r = h.get_by_label(&note).rect();
            let [_, _, top, bottom] = widgets::STATUS_LINE_MARGIN.map(f32::from);
            // the scroll area ends at the central panel's 14 px margin over the footer, whose 10 px margin sits over
            // PLAY, and starts 10 px under the tab row; the status line's frame adds its margins around the text
            let (lo, hi) = (tab.bottom() + 10.0, play.top() - 24.0);
            assert!(
                r.top() - top >= lo && r.bottom() + bottom <= hi,
                "{} failure={}: the result's frame {}..{} is outside the page {lo}..{hi}",
                lang.code(),
                failure.is_some(),
                r.top() - top,
                r.bottom() + bottom
            );
        }
    }
}

/// 2017-18 HOST ARCADE's dialog: "List my arcade", the arcade's name, HOST ARCADE and CANCEL over the ONLINE tab.
/// ONLINE has no PLAY: its JOIN joins the code typed, CONNECT in the Direct address fold plays online; PLAY on the
/// PLAY tab with Online chosen shows ONLINE instead of playing.
#[test]
fn launcher_online_starts_from_the_lobby() {
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

/// FREE and CREATE-A-TEAM sit side by side at the top of the PLAY tab and fit its smallest window (600 x 700) in every
/// language, above the footer's PLAY; a click chooses, and while a game runs neither answers.
#[test]
fn profile_buttons_fit_the_play_tab() {
    let size = launcher::window_size(Some(700.0));
    for lang in Lang::ALL {
        let t = |k: &str| forza_ui::text(lang, k, &[]);
        let st = std::cell::RefCell::new(launcher::LauncherState::sample(
            Edition::E1718,
            lang,
            launcher::Scenario::Ready,
        ));
        let got = std::cell::RefCell::new(None);
        let mut h = harness(size, 1.0, Edition::E1718, |ui| {
            if let Some(a) = launcher::show(ui, &mut st.borrow_mut()) {
                *got.borrow_mut() = Some(a);
            }
        });
        h.run_steps(4);
        let (free, cat) = (
            h.get_by_label(&t("launcher.profile_free")).rect(),
            h.get_by_label(&t("launcher.profile_cat")).rect(),
        );
        let play_top = h
            .get_all_by_label(&t("launcher.play"))
            .map(|n| n.rect().top())
            .fold(f32::MIN, f32::max);
        let tag = format!("{lang:?}: FREE {free:?}, CREATE-A-TEAM {cat:?}");
        assert!(
            free.left() >= 14.0 && cat.right() <= size.x - 14.0,
            "{tag}: inside the window"
        );
        assert!(free.right() <= cat.left(), "{tag}: side by side");
        assert!((free.top() - cat.top()).abs() < 0.5, "{tag}: one row");
        for r in [free, cat] {
            assert_eq!(r.height(), launcher::PROFILE_H, "{tag}");
            assert!(r.width() >= 250.0, "{tag}: large");
            assert!(r.bottom() < play_top, "{tag}: above PLAY at {play_top}");
        }
        h.get_by_label(&t("launcher.profile_cat")).click();
        h.run_steps(2);
        assert_eq!(got.take(), Some(launcher::Action::Profile), "{tag}");
        assert_eq!(
            st.borrow().profile,
            Some(forza::profile::Profile::CreateATeam)
        );
        st.borrow_mut().running = true;
        h.run_steps(2);
        h.get_by_label(&t("launcher.profile_free")).click();
        h.run_steps(2);
        assert_eq!(got.take(), None, "{tag}: locked while a game runs");
        assert_eq!(
            st.borrow().profile,
            Some(forza::profile::Profile::CreateATeam)
        );
    }
}

/// The cup rows fit the Launcher (its width is fixed; 700 tall is its smallest) in every language, on the PLAY tab and
/// in the host dialog: the four segments and the stepper inside the window, each segment's label no wider than its
/// segment at the 70 % `fit` floor.
#[test]
fn cup_rows_fit_the_launcher() {
    let size = launcher::window_size(Some(700.0));
    for lang in Lang::ALL {
        for (mode, dialog) in [
            (lobby::CupMode::In, false),
            (lobby::CupMode::At, false),
            (lobby::CupMode::In, true),
            (lobby::CupMode::At, true),
        ] {
            let scenario = if dialog {
                launcher::Scenario::Online
            } else {
                launcher::Scenario::Ready
            };
            let mut st = launcher::LauncherState::sample(Edition::E1718, lang, scenario);
            let v = &mut st.lobby;
            (v.cup.mode, v.cup.minutes, v.cup.at, v.clock) =
                (mode, 150, 23 * 60 + 55, Some(14 * 3600));
            if dialog {
                (v.arcade_rooms, v.kind) = (true, lobby::Kind::Arcades);
                v.host_ask = Some((
                    false,
                    String::new(),
                    String::new(),
                    lobby::CupChoice::default(),
                ));
            }
            let cup = v.cup;
            let mut h = harness(size, 1.0, Edition::E1718, |ui| {
                launcher::show(ui, &mut st);
            });
            h.run_steps(4);
            let tag = format!("{lang:?} {mode:?} dialog={dialog}");
            let t = |k: &str| forza_ui::text(lang, k, &[]);
            let at = lobby::clock_text(u32::from(cup.at) * 60);
            let m = cup.minutes.to_string();
            let segs = [
                t("lobby.cup_off"),
                t("lobby.cup_game_times"),
                forza_ui::text(lang, "lobby.cup_in", &[("m", &m)]),
                forza_ui::text(lang, "lobby.cup_at", &[("t", &at)]),
            ];
            let names = [t("lobby.cup_earlier"), t("lobby.cup_later")];
            let mut right = 0f32;
            for s in &segs {
                let r = h.get_by_label(s).rect();
                // the segment's own fit (down to 70 %) into its label room: wider still is an overflow
                let room = r.width() - 12.0;
                let painter = h.ctx.layer_painter(egui::LayerId::background());
                let w = widgets::fit(&painter, s, theme::bold(14.0), egui::Color32::WHITE, room)
                    .size()
                    .x;
                assert!(
                    w <= room,
                    "{tag}: {s:?} is {w} wide in a {} segment",
                    r.width()
                );
                right = right.max(r.right());
            }
            for n in &names {
                right = right.max(h.get_by_label(n).rect().right());
            }
            // the dialog is 480 wide plus its 20 px margins, centred
            let edge = if dialog {
                (size.x + 480.0) / 2.0
            } else {
                size.x - 14.0
            };
            assert!(
                right <= edge,
                "{tag}: the cup rows end at {right}, past {edge}"
            );
        }
    }
}

/// The Files page at the default size: each row's hint and check sit right under it, lined up with the text boxes, and
/// "Add files…" stays above the footer in every language.
#[test]
fn installer_files_rows_fit() {
    for lang in Lang::ALL {
        let mut st = installer::InstallerState::sample(lang, installer::Page::Files);
        let mut h = harness(installer::SIZE, 1.0, Edition::E1011, |ui| {
            installer::show(ui, &mut st);
        });
        h.run_steps(4);
        let t = |key: &str| forza_ui::text(lang, key, &[]);
        let rect = |key: &str| h.get_by_label(&t(key)).rect();
        let mut browse: Vec<_> = h
            .get_all_by_label(&t("launcher.browse"))
            .map(|b| b.rect())
            .collect();
        browse.sort_by(|a, b| a.top().total_cmp(&b.top()));
        assert_eq!(browse.len(), 3, "{lang:?}: three file rows");
        let rows = [
            ("installer.iso_hint", "installer.iso_ok"),
            ("installer.key_hint", "installer.key_found"),
            ("installer.package_hint", "installer.pkg_ok"),
        ];
        let left = rect(rows[0].0).left();
        for (i, (hint, check)) in rows.into_iter().enumerate() {
            let hint = rect(hint);
            let check = h
                .get_by_label(&forza_ui::text(lang, check, &[("edition", "2010–11")]))
                .rect();
            assert!(
                (0.0..=12.0).contains(&(check.top() - hint.bottom())),
                "{lang:?} row {i}: check {check:?} not right under its hint {hint:?}"
            );
            let gap = hint.top() - browse[i].bottom();
            assert!(
                (0.0..=8.0).contains(&gap),
                "{lang:?} row {i}: hint {gap} px under its row"
            );
            assert_eq!(hint.left(), left, "{lang:?} row {i}: hints line up");
            assert!(
                left > installer::RAIL_W + 24.0 + 120.0,
                "{lang:?}: hint left of the text box"
            );
            if let Some(next) = browse.get(i + 1) {
                assert!(
                    check.bottom() < next.top(),
                    "{lang:?} row {i}: check overlaps the next row"
                );
            }
        }
        let add = rect("installer.add_files");
        assert!(
            add.bottom() <= installer::SIZE.y - installer::FOOTER_H,
            "{lang:?}: Add files at {add:?} under the footer"
        );
    }
    // a refusal is at the top of the scrolled content, not laid out below the visible page: at the shortest window
    // the rows scroll, and a line under the scroll area would be clipped
    let refusal_text = forza_ui::text(Lang::En, "installer.err_iso", &[]);
    let mut st = installer::InstallerState::sample(Lang::En, installer::Page::Files);
    st.error = Some(refusal_text.clone());
    let short = egui::vec2(installer::SIZE.x, 520.0);
    let mut h = harness(short, 1.0, Edition::E1011, |ui| {
        installer::show(ui, &mut st);
    });
    h.run_steps(4);
    let refusal = h.get_by_label(&refusal_text).rect();
    assert!(
        refusal.top() > 0.0 && refusal.bottom() < short.y - installer::FOOTER_H,
        "the refusal at {refusal:?} is under the footer"
    );
}

/// On hold since 2026-10-31: 2017-18 shows no PAUSE button on the page bar, no VS ROOMS tab and no HOST VS; 2010-11
/// keeps its own. The arcade side (ARCADES tab, HOST ARCADE) stays.
#[test]
fn vs_lobby_and_pause_button_hidden_1718() {
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

/// RESET LINEUP is offered outside a match with a table kept, also while the board holds that table (the club office
/// between matches), and greyed out in a match or before any table was kept.
#[test]
fn board_reset_lineup_offered_outside_a_match() {
    for (in_match, kept, disabled) in [
        (false, true, false),
        (true, true, true),
        (false, false, true),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
        st.in_match = in_match;
        if kept {
            st.before_match = Some(forza::cards::table_after(&[], &st.board));
        }
        assert!(!st.reset_ready, "the table is the one the match began with");
        let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st)
        });
        h.run_steps(4);
        let button = h.get_by_label("RESET LINEUP");
        assert_eq!(button.accesskit_node().is_disabled(), disabled);
    }
}

/// The hint under the game window's board keeps a clear gap under its row of buttons and stays inside the board, at
/// the windows the panel's layout test covers and the game window players run (1972x1080), in English, Italian and
/// Japanese. Laid out only: no GPU.
#[test]
fn board_hint_clear_of_the_buttons() {
    const GAP: f32 = 8.0;
    for (w, h) in [
        (1440.0, 900.0),
        (1600.0, 900.0),
        (1280.0, 800.0),
        (1920.0, 1049.0),
        (1920.0, 1080.0),
        (1972.0, 1080.0),
        (2560.0, 1369.0),
        (2560.0, 1440.0),
        (3440.0, 1440.0),
    ] {
        // the sheet such a window shows, in points
        let size = Vec2::new(w, h) / panel::scale(w, h, 96.0);
        for lang in Lang::ALL {
            let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
            st.before_match = Some(forza::cards::table_after(&[], &st.board));
            let mut h = harness(size, 1.0, Edition::E1718, |ui| panel::show(ui, &mut st));
            h.run_steps(8);
            let at = |key: &str| h.get_by_label(&forza_ui::text(lang, key, &[])).rect();
            let buttons = ["board.pop_out", "board.reset", "form.open"]
                .map(|k| at(k).bottom())
                .into_iter()
                .fold(f32::MIN, f32::max);
            let hint = at("board.hint");
            // the board strip keeps a 16-point margin inside the board's column
            let board =
                panel::layout(egui::Rect::from_min_size(egui::Pos2::ZERO, size), false).board;
            let case = format!("{size:?} {}", lang.code());
            assert!(
                hint.top() >= buttons + GAP - 0.5,
                "{case}: the hint at {} sits {} under the buttons",
                hint.top(),
                hint.top() - buttons
            );
            assert!(
                hint.bottom() <= board.bottom() - 16.0 + 0.5,
                "{case}: the hint ends at {}, the board at {}",
                hint.bottom(),
                board.bottom()
            );
        }
    }
}

/// The board's row of buttons (POP OUT, RESET LINEUP, FORMATIONS) on the docked board at the divider's least width:
/// no two buttons overlap or crowd each other and all stay inside the board's 16-point margin, in English, Italian
/// and Japanese (the Italian labels are the longest). The divider's ◀▶ handle sits on that row, on the board's edge,
/// and meets none of the three buttons; also at the 1920-wide window's default board. Laid out only: no GPU.
#[test]
fn board_foot_buttons_apart_at_the_least_width() {
    for (w, h, set) in [
        (1440.0, 900.0, Some(panel::BOARD_MIN)),
        (1920.0, 1080.0, Some(panel::BOARD_MIN)),
        (1920.0, 1080.0, None),
    ] {
        let size = Vec2::new(w, h);
        let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let board = panel::layout_at(sheet, false, set).board;
        for lang in Lang::ALL {
            let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
            st.before_match = Some(forza::cards::table_after(&[], &st.board));
            st.file.board_width = set.map(|w| w as u16);
            let mut hs = harness(size, 1.0, Edition::E1718, |ui| panel::show(ui, &mut st));
            hs.run_steps(8);
            let at = |key: &str| hs.get_by_label(&forza_ui::text(lang, key, &[])).rect();
            let r = ["board.pop_out", "board.reset", "form.open"].map(at);
            let case = format!("{w}x{h} {} board {board:?}: {r:?}", lang.code());
            // right to left: POP OUT, RESET LINEUP, FORMATIONS, each the row's 6 points from the next (a label too
            // long for the row at the least text size pushes its button into the gap and over its neighbour)
            assert!(
                r[1].right() <= r[0].left() - 6.0 + 0.5,
                "{case}: RESET into POP OUT's gap"
            );
            assert!(
                r[2].right() <= r[1].left() - 6.0 + 0.5,
                "{case}: FORMATIONS into RESET's gap"
            );
            assert!(
                r[2].left() >= board.left() + 16.0 - 0.5,
                "{case}: past the board's left margin"
            );
            assert!(
                r[0].right() <= board.right() - 16.0 + 0.5,
                "{case}: past the board's right margin"
            );
            let handle = panel::divider_handle(&hs.ctx).expect("the divider's handle");
            let case = format!("{case} handle {handle:?}");
            assert!(
                (handle.center().x - board.left()).abs() < 0.5,
                "{case}: off the board's edge"
            );
            for b in r {
                assert!(!b.intersects(handle), "{case}: on a button");
                assert!(
                    b.y_range().contains(handle.center().y),
                    "{case}: off the buttons' row"
                );
            }
        }
    }
}

/// The pop-out board's foot (#285): DOCK THE BOARD, RESET LINEUP and SAVED LINEUPS keep their 6-point gaps inside the
/// board, in English, Italian and Japanese: at the window's least sheet (900 points on its short side, the cabinet
/// buttons under a board just taller than wide: the narrowest board, about 500), at the default window, and on a sheet
/// narrower than any window gets, where the labels do not fit at the least text size and wrap instead. The old row
/// pinned the last button to the row's edge there (`pos2((right - w).max(row.left()), row.top())`), on its neighbour.
#[test]
fn popout_board_foot_buttons_apart() {
    for (w, h, buttons) in [
        (900.0, 901.0, true),
        (960.0, 1232.0, false),
        (380.0, 900.0, false),
    ] {
        let size = Vec2::new(w, h);
        let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let (room, _) = panel::desk_parts(sheet, buttons, false);
        // the bench at the pitch's right, the sample's (no bench= line)
        let board = panel::desk_board_rect(room, true);
        for lang in Lang::ALL {
            let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
            st.desk = true;
            st.file.board_window = true;
            st.file.board_buttons = buttons;
            st.before_match = Some(forza::cards::table_after(&[], &st.board));
            let mut hs = harness(size, 1.0, Edition::E1718, |ui| panel::show(ui, &mut st));
            hs.run_steps(8);
            let at = |key: &str| hs.get_by_label(&forza_ui::text(lang, key, &[])).rect();
            let r = ["board.dock", "board.reset", "form.open"].map(at);
            let case = format!("{w}x{h} {} board {board:?}: {r:?}", lang.code());
            assert!(
                r[1].right() <= r[0].left() - 6.0 + 0.5,
                "{case}: RESET into DOCK's gap"
            );
            assert!(
                r[2].right() <= r[1].left() - 6.0 + 0.5,
                "{case}: FORMATIONS into RESET's gap"
            );
            assert!(
                r[2].left() >= board.left() + 16.0 - 0.5,
                "{case}: past the board's left margin"
            );
            assert!(
                r[0].right() <= board.right() - 16.0 + 0.5,
                "{case}: past the board's right margin"
            );
        }
    }
}

/// A card without a back picture (the test catalogue has no picture folder) offers no BACK in the catalogue's detail
/// pane nor its card view, in every language; the drawn back is gone (#333).
#[test]
fn a_card_without_a_back_picture_has_no_back_in_the_catalogue() {
    for (lang, back) in [(Lang::En, "BACK"), (Lang::It, "RETRO"), (Lang::Ja, "裏面")] {
        for view in [false, true] {
            let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Catalogue);
            st.catalogue.filter = Default::default();
            st.catalogue.focus = Some(7);
            if view {
                st.catalogue.view = Some(7);
            }
            let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
                panel::show(ui, &mut st)
            });
            h.run_steps(6);
            assert!(
                h.query_by_label(back).is_none(),
                "{lang:?} view {view}: {back} without a back picture"
            );
        }
    }
}

/// A click away from the catalogue's search line (BACK in the detail pane) moves nothing and leaves the keys with the
/// search line: every rect is the same before the click, while it is pressed, on the click's frame and after it.
/// The field drew unfocused on the click's frame, a point taller, and shook everything under it (issue #265).
#[test]
fn a_click_in_the_catalogue_keeps_the_search_line_focused_and_still() {
    // BACK shows only for a card with a back picture (#333)
    let dir = tempfile::tempdir().expect("a temp folder");
    two_sizes(dir.path(), forza_ui::sample::cards()[7].number);
    for (size, ppp) in [(Vec2::new(1920.0, 1080.0), 1.25), (panel::SIZE, 1.0)] {
        let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Catalogue);
        st.message = None; // the sample's note line is no part of the claim
        st.catalogue.filter = Default::default();
        st.catalogue.focus = Some(7);
        let path = dir.path().to_owned();
        let mut h = harness(size, ppp, Edition::E1718, |ui| {
            forza_ui::board::set_picture_dir(ui.ctx(), Some(path.clone()));
            panel::show(ui, &mut st)
        });
        assert!(
            run_until(&mut h, |h| h.query_by_label("BACK").is_some()),
            "{size:?} {ppp}: BACK for a card with a back picture"
        );
        h.run_steps(2);
        let focused =
            |h: &Harness<'_>| h.get_by_role(egui::accesskit::Role::TextInput).is_focused();
        let at = h.get_by_label("BACK").rect().center();
        h.hover_at(at);
        h.run_steps(1);
        assert!(focused(&h), "{size:?} {ppp}: the search line has the keys");
        let before = catalogue_rects(&h);
        h.drag_at(at); // pressed
        h.run_steps(1);
        assert_eq!(catalogue_rects(&h), before, "{size:?} {ppp}: pressed");
        release_at(&h, at); // the click
        for frame in 0..4 {
            h.run_steps(1);
            assert_eq!(
                catalogue_rects(&h),
                before,
                "{size:?} {ppp}: frame {frame} after the click moved a rect"
            );
            assert!(focused(&h), "{size:?} {ppp}: frame {frame}: the keys left");
            if frame == 0 {
                // the click's own frame lights the side it turned to, its highlight and the accesskit state alike
                assert_eq!(
                    h.get_by_label("BACK").accesskit_node().toggled(),
                    Some(egui::accesskit::Toggled::True),
                    "{size:?} {ppp}: the click's frame still lights the other side"
                );
            }
        }
    }
}

/// The catalogue's search line and IMPORT LINEUP and SAVED LINEUPS at its right end share one centre line: a row shorter
/// than the field (36 points under a 40 point field) centred the buttons in the row while the field ran 4 points past
/// its foot (#361).
#[test]
fn the_catalogue_search_row_centres_its_buttons_on_the_field() {
    for lang in Lang::ALL {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Catalogue);
        st.message = None;
        let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st)
        });
        h.run_steps(3);
        let field = h.get_by_role(egui::accesskit::Role::TextInput).rect();
        for key in ["cat.import", "form.open"] {
            // SAVED LINEUPS is under the board too: the one nearest the search line
            let button = h
                .query_all_by_label(&forza_ui::text(lang, key, &[]))
                .map(|n| n.rect())
                .min_by(|a, b| {
                    let d = |r: &egui::Rect| (r.center().y - field.center().y).abs();
                    d(a).total_cmp(&d(b))
                })
                .unwrap_or_else(|| panic!("{lang:?}: {key}"));
            assert!(
                (button.center().y - field.center().y).abs() <= 0.5,
                "{lang:?} {key}: {button:?} off the search line's centre line {field:?}"
            );
        }
    }
}

/// On the catalogue page the FORMATIONS dialog's name line keeps the keys after a click opens it: typed letters land
/// in the name, not in the search line drawn under it (the dialog is drawn first, so its last request must win).
#[test]
fn catalogue_formations_name_line_keeps_the_keys() {
    let st = std::cell::RefCell::new(formations_state(Lang::En, false));
    {
        let mut st = st.borrow_mut();
        st.page = panel::Page::Catalogue;
        st.message = None;
        let f = st.formations.as_mut().expect("the dialog open");
        f.naming = Some((2, String::new()));
    }
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Name for slot 3")
        .type_text("Cup XI");
    h.run_steps(4);
    let s = st.borrow();
    assert_eq!(
        s.formations
            .as_ref()
            .and_then(|f| f.naming.as_ref())
            .map(|n| n.1.as_str()),
        Some("Cup XI"),
        "the letters land in the name line"
    );
    assert_eq!(
        s.catalogue.filter.search, "ital",
        "the search line under the dialog keeps its own text"
    );
    assert!(
        h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Name for slot 3")
            .is_focused(),
        "the name line keeps the keys"
    );
}

/// LOAD is greyed out and answers nothing while the own seat plays a match; SAVE works at any time.
#[test]
fn formations_load_refused_in_match_and_save_works() {
    let st = std::cell::RefCell::new(formations_state(Lang::En, true));
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    h.get_all_by_label("LOAD").next().unwrap().click();
    h.run_steps(4);
    assert!(st.borrow().formation_load.is_none());
    // SAVE on the empty slot 3 asks for a name, then stores it, match or not
    h.get_all_by_label("SAVE").nth(2).unwrap().click();
    h.run_steps(4);
    assert_eq!(
        st.borrow()
            .formations
            .as_ref()
            .unwrap()
            .naming
            .as_ref()
            .map(|n| n.0),
        Some(2)
    );
    h.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("Mine");
    h.run_steps(4);
    h.get_all_by_label("SAVE").last().unwrap().click();
    h.run_steps(4);
    assert_eq!(st.borrow().formation_save, Some((2, "Mine".to_owned())));
    // between matches the same click loads
    st.borrow_mut().in_match = false;
    h.run_steps(4);
    h.get_all_by_label("LOAD").next().unwrap().click();
    h.run_steps(4);
    assert_eq!(st.borrow().formation_load, Some(0));
}

/// CONFIRM and Enter ask for the import; CANCEL and Esc drop the question and stay on the catalogue.
#[test]
fn import_dialog_answers() {
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

/// The cabinet's triangles and round buttons send their action when clicked.
#[test]
fn cabinet_shapes_click() {
    use std::cell::RefCell;
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

/// The cabinet deck's key buttons on the game page and their rects, as `ed` draws them: the keys a page or a dialog
/// must not get a press through.
fn deck_keys(ed: Edition) -> Vec<(String, egui::Rect)> {
    use std::cell::RefCell;
    let st = RefCell::new(panel::PanelState::sample(ed, Lang::En, panel::Page::Game));
    let mut h = harness(panel::SIZE, 1.0, ed, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(3);
    [
        "cab.central",
        "cab.counter",
        "cab.press",
        "cab.data",
        "cab.key_player",
        "cab.start",
        "cab.shoot",
        "cab.keeper",
    ]
    .into_iter()
    .filter_map(|k| {
        let l = forza_ui::text(Lang::En, k, &[]);
        let r = h.query_all_by_label(&l).next().map(|n| n.rect())?;
        Some((l, r))
    })
    .collect()
}

/// A click on an open page where a cabinet key sits under it sends no key to the game (#348): the deck was drawn and
/// live under every page (`cabinet_deck(ui, st, deck, lang, deck_scale, side);`, no page check) and its keys read the
/// raw pointer, so the page could not take the press from them.
#[test]
fn a_page_over_the_deck_sends_no_key() {
    use std::cell::RefCell;
    for ed in [Edition::E1011, Edition::E1718] {
        let keys = deck_keys(ed);
        assert!(keys.len() >= 4, "{ed:?}: the deck's keys {keys:?}");
        for page in [panel::Page::Keys, panel::Page::Settings] {
            for (label, r) in &keys {
                // a fresh panel per click: a click on SETTINGS' layout choice would take the deck away
                let st = RefCell::new(panel::PanelState::sample(ed, Lang::En, page));
                let mut h = harness(panel::SIZE, 1.0, ed, |ui| {
                    panel::show(ui, &mut st.borrow_mut())
                });
                h.run_steps(3);
                let at = r.center();
                h.hover_at(at);
                h.run_steps(1);
                h.drag_at(at);
                h.run_steps(1);
                h.drop_at(at);
                h.run_steps(2);
                assert_eq!(
                    st.borrow().pressed,
                    None,
                    "{ed:?} {}: {label} sent under the page",
                    page.tag()
                );
            }
        }
    }
}

/// A latched KEY PLAYER (a right-click keeps its key down in the game) goes up when a page takes the docked deck away:
/// the button that lets the latch go is gone with the deck (#349).
#[test]
fn a_page_releases_the_latched_key() {
    use std::cell::RefCell;
    for ed in [Edition::E1011, Edition::E1718] {
        let st = RefCell::new(panel::PanelState::sample(ed, Lang::En, panel::Page::Game));
        let mut h = harness(panel::SIZE, 1.0, ed, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(3);
        let label = forza_ui::text(Lang::En, "cab.key_player", &[]);
        let at = h
            .query_all_by_label(&label)
            .next()
            .unwrap_or_else(|| panic!("{ed:?}: the deck's {label}"))
            .rect()
            .center();
        h.hover_at(at);
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.run_steps(1);
        }
        assert_eq!(
            st.borrow().latched,
            Some("KEYPLAYER"),
            "{ed:?}: the right-click did not latch the key"
        );
        for page in [panel::Page::Keys, panel::Page::Settings] {
            st.borrow_mut().latched = Some("KEYPLAYER");
            st.borrow_mut().page = page;
            h.run_steps(2);
            assert_eq!(
                st.borrow().latched,
                None,
                "{ed:?} {}: the key stayed latched with the deck gone",
                page.tag()
            );
        }
    }
}

/// A click on a dialog's backdrop over a cabinet key sends no key (#349): the deck read the raw pointer, which a dialog
/// never takes, so CLOSE, IMPORT's question and help and FORMATIONS took every one of its keys as the game's.
#[test]
fn a_dialog_over_the_deck_sends_no_key() {
    use forza::cards::formations;
    use std::cell::RefCell;
    type Open = fn(&mut panel::PanelState);
    let modals: [(&str, Open); 4] = [
        ("the close warning", |st| st.close_warn = true),
        ("IMPORT's question", |st| {
            st.import_ask = Some(panel::ImportAsk {
                text: String::new(),
                cards: 11,
                skipped: 0,
            })
        }),
        ("IMPORT's help", |st| st.import_help = true),
        ("FORMATIONS", |st| {
            st.formations = Some(panel::Formations {
                slots: vec![None; formations::SLOTS],
                naming: None,
            })
        }),
    ];
    for ed in [Edition::E1011, Edition::E1718] {
        let keys = deck_keys(ed);
        assert!(keys.len() >= 4, "{ed:?}: the deck's keys {keys:?}");
        for (name, open) in modals {
            // the deck's first two keys, CENTRAL and COUNTER
            for (label, r) in keys.iter().take(2) {
                let st = RefCell::new(panel::PanelState::sample(ed, Lang::En, panel::Page::Game));
                open(&mut st.borrow_mut());
                let mut h = harness(panel::SIZE, 1.0, ed, |ui| {
                    panel::show(ui, &mut st.borrow_mut())
                });
                h.run_steps(3);
                let at = r.center();
                h.hover_at(at);
                h.run_steps(1);
                h.drag_at(at);
                h.run_steps(1);
                h.drop_at(at);
                h.run_steps(2);
                assert_eq!(
                    st.borrow().pressed,
                    None,
                    "{ed:?} under {name}: {label} sent"
                );
            }
        }
    }
}

/// The board window's deck (`board_buttons=on`) sends no key through FORMATIONS or RESET LINEUP's question opened on
/// that window (#361): its keys read the raw pointer, which the dialog's backdrop never takes. Without a dialog the
/// same click presses its key.
#[test]
fn a_dialog_on_the_board_window_covers_its_deck() {
    use forza::cards::formations;
    use std::cell::RefCell;
    type Open = fn(&mut panel::PanelState);
    let size = Vec2::new(900.0, 901.0);
    let desk = || {
        let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
        st.desk = true;
        st.file.board_window = true;
        st.file.board_buttons = true;
        st
    };
    let dialogs: [(&str, Open); 3] = [
        ("no dialog", |_| {}),
        ("FORMATIONS", |st| {
            st.formations_desk = true;
            st.formations = Some(panel::Formations {
                slots: vec![None; formations::SLOTS],
                naming: None,
            })
        }),
        // the question needs a kept table: without one it closes at once
        ("RESET LINEUP's question", |st| {
            st.before_match = Some(forza::cards::table_after(&[], &st.board));
            st.reset_ask = Some(true)
        }),
    ];
    let label = forza_ui::text(Lang::En, "cab.central", &[]);
    let at = {
        let st = RefCell::new(desk());
        let mut h = harness(size, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(3);
        h.query_all_by_label(&label)
            .next()
            .unwrap_or_else(|| panic!("the board window's {label}"))
            .rect()
            .center()
    };
    for (name, open) in dialogs {
        let st = RefCell::new(desk());
        open(&mut st.borrow_mut());
        let mut h = harness(size, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(3);
        let up = {
            let s = st.borrow();
            s.reset_ask.is_some() || s.formations.is_some()
        };
        assert_eq!(up, name != "no dialog", "{name} open");
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(2);
        let want = (name == "no dialog").then_some("UP");
        assert_eq!(st.borrow().pressed, want, "{name}: {label}");
    }
}

/// 2017-18 SETTINGS at 1440x900 is taller than its sheet: it scrolls, so its last toggles, scrolled to, sit inside the
/// sheet and clear of the page bar in every language (#348: Ask before RESET LINEUP ran over the bar's SETTINGS).
#[test]
fn settings_fit_their_sheet_1718() {
    use egui::accesskit::Role;
    for lang in Lang::ALL {
        let st = std::cell::RefCell::new(panel::PanelState::sample(
            Edition::E1718,
            lang,
            panel::Page::Settings,
        ));
        let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(4);
        let settings = forza_ui::text(lang, "panel.settings", &[]);
        let bar = h.get_by_role_and_label(Role::Button, &settings).rect();
        for key in ["settings.reset_ask", "settings.pad_bg"] {
            let label = forza_ui::text(lang, key, &[]);
            h.get_by_label(&label).scroll_to_me();
            h.run_steps(4);
            let r = h.get_by_label(&label).rect();
            // the sheet ends 12 points above the bar's row, the bar's buttons at or under it
            assert!(
                r.top() >= 0.0 && r.bottom() <= bar.top() - 12.0,
                "{} {key}: {r:?} not clear of the page bar's {bar:?}",
                lang.code()
            );
        }
    }
}

/// 2017-18 SETTINGS at 1440x900, unscrolled (#361): BENCH POSITION and its BELOW sit above the fold in every language.
/// CLEAR AND RESTART and its hint in NOW pushed them under it, behind the message line (BELOW's bottom at ~823 against
/// the fold's ~775); under LOGS they leave them in sight.
#[test]
fn bench_position_above_the_settings_fold_1718() {
    use egui::accesskit::Role;
    for lang in Lang::ALL {
        let st = std::cell::RefCell::new(panel::PanelState::sample(
            Edition::E1718,
            lang,
            panel::Page::Settings,
        ));
        let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(4);
        // the message line under the scrolled part: the sample's note, its text 7 points into its frame, the frame
        // under the scroll area's item spacing
        let note = forza_ui::text(lang, "panel.msg_saved", &[]);
        let fold = h.get_by_label(&note).rect().top() - 12.0;
        let label = forza_ui::text(lang, "settings.bench", &[]);
        let below = forza_ui::text(lang, "settings.bench_below", &[]);
        for (name, r) in [
            ("label", h.get_by_label(&label).rect()),
            (
                "BELOW",
                h.get_by_role_and_label(Role::RadioButton, &below).rect(),
            ),
        ] {
            assert!(
                r.bottom() <= fold,
                "{} {name}: {r:?} under the fold at {fold}",
                lang.code()
            );
        }
        // RIGHT and BELOW only, no AUTO
        let row = h
            .get_by_role_and_label(Role::RadioButton, &below)
            .rect()
            .center()
            .y;
        let segments = h
            .get_all_by_role(Role::RadioButton)
            .filter(|n| (n.rect().center().y - row).abs() < 1.0)
            .count();
        assert_eq!(segments, 2, "{}: BENCH POSITION's segments", lang.code());
    }
}

/// A player's path to SETTINGS' last toggles (#349 round 2): from the game page, SETTINGS on the page bar, the row
/// scrolled to, then clicked. The click turns that toggle and stays on SETTINGS; board_reset_lineup_asks_first saw it
/// open CARDS instead, so the scrolled-to row sat over the page bar there.
#[test]
fn settings_last_toggles_take_the_click_1718() {
    use egui::accesskit::Role;
    for lang in Lang::ALL {
        let st = std::cell::RefCell::new(panel::PanelState::sample(
            Edition::E1718,
            lang,
            panel::Page::Game,
        ));
        let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(4);
        let settings = forza_ui::text(lang, "panel.settings", &[]);
        h.get_by_role_and_label(Role::Button, &settings).click();
        h.run_steps(4);
        assert_eq!(st.borrow().page, panel::Page::Settings, "{}", lang.code());
        let bar = h.get_by_role_and_label(Role::Button, &settings).rect();
        type Read = fn(&panel::PanelState) -> bool;
        let toggles: [(&str, Read); 2] = [
            ("settings.reset_ask", |s| s.file.reset_ask_off),
            ("settings.pad_bg", |s| s.file.pad_background),
        ];
        for (key, value) in toggles {
            let label = forza_ui::text(lang, key, &[]);
            let before = value(&st.borrow());
            h.get_by_label(&label).scroll_to_me();
            h.run_steps(4); // a scroll_to_me lands on the 4th frame (2 leave the row where it was)
            let r = h.get_by_label(&label).rect();
            assert!(
                r.bottom() <= bar.top() - 12.0,
                "{} {key}: the row at {r:?} reaches the page bar's {bar:?}",
                lang.code()
            );
            h.get_by_label(&label).click();
            h.run_steps(4);
            let s = st.borrow();
            assert_eq!(
                s.page,
                panel::Page::Settings,
                "{} {key}: the click at {r:?} left SETTINGS",
                lang.code()
            );
            assert_ne!(
                value(&s),
                before,
                "{} {key}: the click at {r:?} did not turn it",
                lang.code()
            );
        }
    }
}

/// Down, Down, Enter on the sample list joins the third row; Right switches to the VS rooms, F5 and Esc answer; H
/// hosts a VS room, Enter in it is READY.
#[test]
fn lobby_keys() {
    use lobby::LobbyAction::*;
    let v = std::cell::RefCell::new(lobby::LobbyView::sample(lobby::Scenario::List));
    let got = std::cell::RefCell::new(None);
    let mut h = harness(lobby::SIZE, 1.0, Edition::E1718, |ui| {
        if let Some(a) = lobby::show(ui, &mut v.borrow_mut(), Lang::En) {
            *got.borrow_mut() = Some(a);
        }
    });
    h.run_steps(4);
    let third = v.borrow().arcades[2].id.clone();
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

/// 2017-18 with the shared arcade on: HOST ARCADE (and H) opens the host dialog (its HOST gives HostArcade with the switch as
/// set, CANCEL and Esc put it back), the code typed and JOINed in capitals, the listed room joined from its row; while
/// hosting only Esc (LEAVE) answers.
#[test]
fn lobby_arcade_rooms() {
    use lobby::LobbyAction::*;
    let mut start = lobby::LobbyView::sample(lobby::Scenario::ArcadeNoRoom);
    (start.notice, start.code) = (None, String::new());
    start.arcade_notice = "Open till 23:00".into();
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
    h.get_by_label("In 10 min").click();
    h.run_steps(2);
    assert_eq!(v.borrow().cup.mode, lobby::CupMode::In);
    // the stepper under it: + steps 10 -> 15 and the segment shows it
    h.get_by_label("5 min later").click();
    h.run_steps(2);
    assert_eq!(v.borrow().cup.minutes, 15);
    h.get_by_label("In 15 min");
    // the notice box takes one line of at most HOST_NOTICE_LEN characters; CANCEL puts the earlier text back
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Arcade notice")
        .focus();
    h.run_steps(1);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Arcade notice")
        .type_text(&"n".repeat(200));
    h.run_steps(2);
    let typed = v.borrow().arcade_notice.clone();
    assert!(
        typed.contains("Open till 23:00") && typed.contains("nnn"),
        "{typed}"
    );
    assert_eq!(
        typed.chars().count(),
        forza::online::lobby::HOST_NOTICE_LEN,
        "the box stops at HOST_NOTICE_LEN"
    );
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(got.take(), None);
    assert!(v.borrow().host_ask.is_none() && !v.borrow().list_arcade);
    assert_eq!(
        v.borrow().arcade_notice,
        "Open till 23:00",
        "CANCEL puts the notice back"
    );
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
    assert!(v.borrow().host_ask.is_none() && !v.borrow().list_arcade);
    h.key_press(egui::Key::H);
    h.run_steps(2);
    h.get_by_label("List my arcade when I host").click();
    h.get_by_label("At 21:00").click();
    // a cup whose championship would still run at 05:00: refused, HOST does nothing
    let cup_at = |clock, at| {
        let mut b = v.borrow_mut();
        (b.clock, b.cup.at) = (Some(clock), at);
    };
    cup_at(3 * 3600, 4 * 60 + 30);
    h.run_steps(2);
    h.get_by_label("The cup and its championship must end before 05:00.");
    h.get_all_by_label("HOST ARCADE").last().unwrap().click();
    h.run_steps(2);
    assert_eq!(got.take(), None, "a refused cup time keeps the dialog open");
    cup_at(22 * 3600, 23 * 60 + 30);
    h.run_steps(2);
    h.get_by_label(
        "Entries open at about 23:30. Insert your card before then and choose the Universal Cup in the game as soon \
as it can be selected; entries close about a minute and a half later.",
    );
    // the tab's HOST ARCADE, then the dialog's
    h.get_all_by_label("HOST ARCADE").last().unwrap().click();
    h.run_steps(2);
    assert_eq!(got.take(), Some(HostArcade));
    assert!(v.borrow().host_ask.is_none() && v.borrow().list_arcade);
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
    h.get_by_label(forza::online::lobby::ARCADE_ROOM_NAME)
        .click();
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

/// The LAN fold: HOST LAN ARCADE, a found arcade's JOIN and a typed address's JOIN each give their action; hosting hides it.
#[test]
fn lobby_lan() {
    use lobby::LobbyAction::*;
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
    // the JOINs: the list's, the two found arcades', the typed address's
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
    let mut v = lobby::LobbyView::sample(lobby::Scenario::List);
    v.list_fit = true;
    let mut h = harness(Vec2::new(lobby::SIZE.x, 470.0), 1.0, Edition::E1011, |ui| {
        lobby::show(ui, &mut v, Lang::En);
    });
    h.run_steps(6);
    let bottom = h.get_by_label_contains("Server address").rect().bottom();
    assert!(bottom <= 470.0, "the Server address fold ends at {bottom}");
    assert!(
        h.get_by_label("HOST A VS ROOM").rect().top()
            > h.get_by_label("MILANO CUP").rect().bottom()
    );
}

/// `list_fit`: in a tab too short even for the three-row list, the lobby scrolls as a whole. Its buttons ended under
/// the tab's edge (JOIN at 433..473 of 470 in Japanese) and the folds below it, with nothing to scroll.
#[test]
fn a_lobby_too_tall_for_its_tab_scrolls() {
    let mut v = lobby::LobbyView::sample(lobby::Scenario::ArcadeNoRoom);
    v.list_fit = true;
    let mut h = harness(Vec2::new(lobby::SIZE.x, 470.0), 1.0, Edition::E1718, |ui| {
        lobby::show(ui, &mut v, Lang::Ja);
    });
    h.run_steps(6);
    for label in ["参加", "LAN（同じネットワーク）"] {
        h.get_by_label(label).scroll_to_me();
        h.run_steps(8);
        let r = h.get_by_label(label).rect();
        assert!(r.top() >= 0.0 && r.bottom() <= 470.0, "{label} at {r:?}");
    }
}

/// The pinned card floating over the pitch (the 1440x900 sheet) takes the pointer at most on itself and its FRONT | BACK
/// switch: a click and a drag on a mat card under the rest of the pane reach the mat card.
#[test]
fn the_big_card_pane_lets_the_mat_cards_under_it_take_the_pointer() {
    let size = panel::SIZE;
    let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
    st.file.board_width = None;
    st.hover_delay = None; // only a click shows a card big
    let at =
        |st: &panel::PanelState, s| panel::board_slot_center_1718(sheet, None, &st.board.spots, s);
    let filled: Vec<usize> = (0..11).filter(|&s| st.board.slots[s].is_some()).collect();
    // the lowest card: its pane floats over the pitch's top half
    let a = *filled
        .iter()
        .max_by(|&&x, &&y| at(&st, x).y.total_cmp(&at(&st, y).y))
        .unwrap();
    let centers: Vec<(usize, egui::Pos2)> = filled.iter().map(|&s| (s, at(&st, s))).collect();
    let st = std::cell::RefCell::new(st);
    let mut h = harness(size, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    let area = |h: &Harness, name| h.ctx.memory(|m| m.area_rect(egui::Id::unique(name)));
    let click = |h: &mut Harness, p: egui::Pos2| {
        h.drag_at(p);
        h.run_steps(1);
        h.drop_at(p);
        h.run_steps(3);
    };
    h.run_steps(4);
    click(&mut h, centers.iter().find(|c| c.0 == a).unwrap().1);
    let big = area(&h, "board-big").expect("the card shown big");
    // a card without a back picture (none here) has neither: the whole pane lets the pointer through (#333)
    let live = [area(&h, "board-big-card"), area(&h, "board-big-switch")]
        .into_iter()
        .flatten()
        .fold(egui::Rect::NOTHING, |a, r| a.union(r));
    let (b, at_b) = *centers
        .iter()
        .find(|(s, p)| *s != a && big.contains(*p) && !live.expand(4.0).contains(*p))
        .expect("a mat card under the pane, off its card and switch");
    // a click on it shows it big instead: the pane leaves its half
    click(&mut h, at_b);
    assert_eq!(st.borrow().turned, None, "the big card did not flip");
    let moved = area(&h, "board-big").expect("the clicked card big");
    assert!(
        !moved.contains(at_b),
        "{b} under {big:?} took no click: {moved:?}"
    );
    // a drag from under the pane onto the lowest card swaps them
    click(&mut h, centers.iter().find(|c| c.0 == a).unwrap().1);
    assert!(
        area(&h, "board-big").unwrap().contains(at_b),
        "the pane over {b} again"
    );
    let (na, nb) = (st.borrow().board.number(a), st.borrow().board.number(b));
    let to = centers.iter().find(|c| c.0 == a).unwrap().1;
    h.drag_at(at_b);
    h.run_steps(1);
    for t in [0.25, 0.5, 0.75, 1.0] {
        h.hover_at(at_b.lerp(to, t));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
    let board = &st.borrow().board;
    assert_eq!(
        (board.number(a), board.number(b)),
        (nb, na),
        "the drag from under the pane swapped them"
    );
}

/// The mouse button let go at `at`, alone: `Harness::drop_at` also takes the pointer away, an event kittest gives a
/// frame of its own, so the step after it shows the frame after the click's, never the click's.
fn release_at(h: &Harness<'_>, at: egui::Pos2) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
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
/// Laid out only: no picture is taken, so no GPU (it ran in `snapshots` and did not gate a merge).
#[test]
fn catalogue_back_keeps_still() {
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
        release_at(&h, at); // the click
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

/// The Launcher's window: 600x700 on a screen whose work area is 700 tall (a 1280x720 one), its full size otherwise.
#[test]
fn launcher_window_sizes() {
    assert_eq!(launcher::window_size(Some(700.0)), egui::vec2(600.0, 700.0));
    assert_eq!(launcher::window_size(None), launcher::SIZE);
}

/// The pop-out windows are drawn one pixel to a point at the sizes their pictures are taken at: the board window at
/// 1.25, 1.5, 2 and 2.5 megapixels and the game window at 1920x1080 and 2560x1440.
#[test]
fn popout_windows_are_drawn_one_to_one() {
    for (w, h) in [
        (960.0, 1232.0),
        (1080.0, 1920.0),
        (2360.0, 1640.0),
        (1920.0, 1080.0),
    ] {
        assert_eq!(panel::board_scale(w, h, 96.0), 1.0, "board {w}x{h}");
    }
    for (w, h) in [(1920.0, 1080.0), (2560.0, 1440.0)] {
        assert_eq!(panel::scale(w, h, 96.0), 1.0, "game {w}x{h}");
    }
}

/// Writes a card's two sides of different pixel sizes and shapes (the pack's 180x254 scan front, the album's
/// 256x357 back), as an install has them.
fn two_sizes(dir: &std::path::Path, no: u32) {
    for (w, h, name) in [
        (180, 254, format!("{no}_f.jpg")),
        (256, 357, format!("{no}_b.jpg")),
    ] {
        image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, 255])
        })
        .save_with_format(dir.join(name), image::ImageFormat::Png)
        .expect("a picture written");
    }
}

/// The card picture drawn in `within` on the last frame (a textured mesh over a quarter of its width): its texture and rect.
fn picture_drawn(h: &Harness<'_>, within: egui::Rect) -> Option<(egui::TextureId, egui::Rect)> {
    h.output().shapes.iter().find_map(|c| match &c.shape {
        egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {
            let b = m.calc_bounds();
            (within.expand(1.0).contains_rect(b) && b.width() > within.width() / 4.0)
                .then_some((m.texture_id, b))
        }
        _ => None,
    })
}

/// Runs frames (the pictures load on other threads) until `ready` holds, at most about a second.
fn run_until(h: &mut Harness<'_>, mut ready: impl FnMut(&Harness<'_>) -> bool) -> bool {
    for _ in 0..40 {
        h.run_steps(1);
        if ready(h) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    false
}

/// FRONT and BACK in the catalogue's detail pane draw their pictures in the same rect even when the two sides have
/// different pixel sizes and shapes: each side sized or letterboxed by its own picture moved the picture's edges on
/// every turn (issue #265).
#[test]
fn both_sides_in_the_catalogue_pane_draw_in_one_rect() {
    let dir = tempfile::tempdir().expect("a temp folder");
    let ci = 7;
    two_sizes(dir.path(), forza_ui::sample::cards()[ci].number);
    for (size, ppp) in [(Vec2::new(1920.0, 1080.0), 1.25), (panel::SIZE, 1.0)] {
        let mut st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Catalogue);
        st.message = None;
        st.catalogue.filter = Default::default();
        st.catalogue.focus = Some(ci);
        let path = dir.path().to_owned();
        let mut h = harness(size, ppp, Edition::E1718, |ui| {
            forza_ui::board::set_picture_dir(ui.ctx(), Some(path.clone()));
            panel::show(ui, &mut st)
        });
        let shown = |h: &Harness<'_>, back| {
            panel::catalogue_picture_shown(&h.ctx) == Some((back, true))
                && panel::catalogue_other_side_ready(&h.ctx) == Some(true)
        };
        assert!(
            run_until(&mut h, |h| shown(h, false)),
            "{size:?} {ppp}: the front shown"
        );
        let slot = panel::catalogue_picture_rect(&h.ctx).expect("the picture slot");
        let front = picture_drawn(&h, slot).expect("the front's picture drawn");
        let at = h.get_by_label("BACK").rect().center();
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        release_at(&h, at);
        h.run_steps(1);
        assert!(
            shown(&h, true),
            "{size:?} {ppp}: the back shown on the click's frame"
        );
        let back = picture_drawn(&h, slot).expect("the back's picture drawn");
        assert_ne!(
            back.0, front.0,
            "{size:?} {ppp}: the front's picture still drawn"
        );
        assert_eq!(
            back.1, front.1,
            "{size:?} {ppp}: the back drawn in another rect than the front"
        );
    }
}

/// The big card on the board ([`forza_ui::board::card_view`], its large pane: the album art at its own pixels) draws
/// both sides in the same rect when they have different pixel sizes and shapes (issue #265).
#[test]
fn both_sides_of_the_big_board_card_draw_in_one_rect() {
    let card = forza_ui::sample::cards()[7].clone();
    let dir = tempfile::tempdir().expect("a temp folder");
    two_sizes(dir.path(), card.number);
    let back = std::cell::Cell::new(false);
    let path = dir.path().to_owned();
    let area = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), Vec2::new(600.0, 800.0));
    let mut h = harness(Vec2::new(800.0, 900.0), 1.0, Edition::E1718, |ui| {
        forza_ui::board::set_picture_dir(ui.ctx(), Some(path.clone()));
        forza_ui::board::card_view(ui, &card, area, back.get(), Lang::En);
    });
    let at = |h: &Harness<'_>| {
        h.ctx
            .memory(|m| m.area_rect(egui::Id::unique("board-big-card")))
    };
    assert!(
        run_until(&mut h, |h| at(h)
            .and_then(|r| picture_drawn(h, r))
            .is_some()),
        "the front's picture drawn big"
    );
    let r = at(&h).expect("the card shown big");
    assert!(r.width() >= 256.0, "the large pane, not the strip: {r:?}");
    let front = picture_drawn(&h, r).expect("the front's picture");
    back.set(true);
    assert!(
        run_until(&mut h, |h| picture_drawn(h, r)
            .is_some_and(|b| b.0 != front.0)),
        "the back's picture drawn big"
    );
    assert_eq!(at(&h), Some(r), "the big card moved");
    assert_eq!(
        picture_drawn(&h, r).map(|b| b.1),
        Some(front.1),
        "the back drawn in another rect than the front"
    );
}

/// The catalogue page keeps its place when the search line gives up the keys (a list, the FORMATIONS dialog or the
/// import question opens): the idle field was a point taller than the focused one and moved everything under it
/// (issue #265).
#[test]
fn the_catalogue_keeps_its_place_when_the_search_line_gives_up_the_keys() {
    for (size, ppp, lang) in [
        (Vec2::new(1920.0, 1080.0), 1.25, Lang::En),
        (panel::SIZE, 1.0, Lang::En),
        (panel::SIZE, 1.0, Lang::Ja),
    ] {
        let mut st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Catalogue);
        st.message = None;
        st.catalogue.filter = Default::default();
        st.catalogue.focus = Some(7);
        let st = std::cell::RefCell::new(st);
        let mut h = harness(size, ppp, Edition::E1718, |ui| {
            panel::show(ui, &mut st.borrow_mut())
        });
        h.run_steps(6);
        let search = |h: &Harness<'_>| {
            let n = h.get_by_role(egui::accesskit::Role::TextInput);
            (n.is_focused(), n.rect())
        };
        let slot = |h: &Harness<'_>| panel::catalogue_picture_rect(&h.ctx);
        let (focused, line) = search(&h);
        assert!(
            focused,
            "{size:?} {ppp} {lang:?}: the search line has the keys"
        );
        let pane = slot(&h);
        st.borrow_mut().import_help = true; // the import question
        h.run_steps(3);
        assert!(
            h.get_all_by_role(egui::accesskit::Role::TextInput)
                .all(|n| !n.is_focused()),
            "{size:?} {ppp} {lang:?}: the search line gave up the keys"
        );
        let idle = h
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .map(|n| n.rect())
            .find(|r| (r.top() - line.top()).abs() < 4.0)
            .expect("the search line");
        assert_eq!(
            idle, line,
            "{size:?} {ppp} {lang:?}: the search line changed size"
        );
        assert_eq!(
            slot(&h),
            pane,
            "{size:?} {ppp} {lang:?}: the pane under it moved"
        );
    }
}

/// #376: on a big drawn card face the club line, the season, the series chip (WGK, HOLE) and the rarity mark each
/// keep their own room inside the card, the picture keeps a room above them, and the club text fits its line: shrunk,
/// then cut with "…" on a whole character. On the pop-out board in English, Italian and Japanese at 1920x1080 and
/// 2360x1640 with and without the cabinet buttons, and on the narrowest big faces the app draws (the docked hover
/// face, the catalogue grid's least big tile), for the sample's clubs, a long one, one too long even shrunk, and a
/// Japanese one. The chip and the mark sat on the club line and the card's foot cut it ("Porto Azzurro").
#[test]
fn card_badges_keep_off_the_club_name() {
    use forza_ui::{board, sizing};
    let ctx = egui::Context::default();
    theme::install(&ctx, Edition::E1718, &theme::font_dir()).expect("the fonts in assets/fonts");
    // our fonts take effect from the next pass
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    // (name, cut with "…" on every big face)
    let clubs = [
        ("Porto Azzurro", false),
        (
            "Sporting Clube Internacional de Porto Azzurro e Real Olmedo",
            true,
        ),
        ("北風フットボールクラブ", false),
        ("北風フットボールクラブ・ユナイテッド東京シティ", true),
    ];
    let mut faces: Vec<(String, egui::Rect)> = vec![];
    for (w, h) in [(1920.0, 1080.0), (2360.0, 1640.0)] {
        for buttons in [false, true] {
            let sheet = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(w, h));
            let (room, _) = panel::desk_parts(sheet, buttons, false);
            // the sample's bench: at the pitch's right (no bench= line)
            let at = panel::desk_board_rect(room, true);
            let st = panel::PanelState::sample(Edition::E1718, Lang::En, panel::Page::Game);
            let rects = board::Mat::column(at.min, at.width()).slot_rects(&st.board.spots);
            for (slot, r) in rects.iter().enumerate() {
                faces.push((format!("{w}x{h} buttons {buttons} slot {slot}"), *r));
            }
        }
    }
    // the docked hover face (90x127 at x1) and the catalogue grid's least tile still drawn big
    for ppp in [1.0, 1.25, 1.5, 2.0] {
        let f = sizing::hover_face(ppp);
        faces.push((
            format!("hover face x{ppp}"),
            egui::Rect::from_min_size(egui::Pos2::ZERO, f),
        ));
        let least = (200..2000)
            .map(|avail| sizing::grid(avail as f32, ppp).card)
            .filter(|c| c.x >= 90.0)
            .min_by(|a, b| a.x.total_cmp(&b.x))
            .expect("a big grid tile");
        faces.push((
            format!("grid x{ppp}"),
            egui::Rect::from_min_size(egui::Pos2::ZERO, least),
        ));
    }
    assert!(
        faces.iter().any(|(_, r)| r.width() == 90.0),
        "the narrowest big face"
    );
    let mut big = 0;
    for lang in Lang::ALL {
        let st = panel::PanelState::sample(Edition::E1718, lang, panel::Page::Game);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let p = ui.painter();
            for (at, rect) in &faces {
                if rect.width() < 90.0 {
                    continue; // a small face: name only
                }
                for ci in st.board.slots.iter().flatten() {
                    let mut card = st.board.cards[*ci].clone();
                    let own = (card.club.clone(), false);
                    for (club, cut) in clubs
                        .iter()
                        .map(|&(c, cut)| (c.to_owned(), cut))
                        .chain([own])
                    {
                        card.club = club;
                        big += 1;
                        let f = board::face_parts(p, *rect, &card);
                        let case = format!("{} {at} {rect:?} {:?}: {f:?}", lang.code(), card.club);
                        assert!(f.pic.is_positive(), "{case}: no room for the picture");
                        let parts = [f.club, f.season, f.chip, f.mark];
                        for r in parts
                            .iter()
                            .chain([&f.pic])
                            .filter(|r| **r != egui::Rect::NOTHING)
                        {
                            assert!(
                                rect.expand(0.01).contains_rect(*r),
                                "{case}: {r:?} off the card"
                            );
                        }
                        for (i, a) in parts.iter().enumerate() {
                            for b in &parts[i + 1..] {
                                assert!(!a.intersects(*b), "{case}: {a:?} on {b:?}");
                            }
                        }
                        assert!(!f.pic.intersects(f.club), "{case}: the club on the picture");
                        // the drawn club text: its line's size, inside the card's margins
                        let g = board::club_galley(p, *rect, &card, egui::Color32::WHITE);
                        assert!(
                            (g.size() - f.club.size()).length() < 0.01,
                            "{case}: drawn {:?}",
                            g.size()
                        );
                        assert!(
                            g.size().x <= rect.width() - 12.0 + 0.01,
                            "{case}: {} wide",
                            g.size().x
                        );
                        let shown: String = g
                            .rows
                            .iter()
                            .flat_map(|r| r.row.glyphs.iter().map(|gl| gl.chr))
                            .collect();
                        if cut {
                            let size = g.job.sections[0].format.font_id.size;
                            assert!(
                                size < rect.width() * 0.09 - 0.01,
                                "{case}: not shrunk ({size})"
                            );
                            let kept = shown
                                .strip_suffix('…')
                                .unwrap_or_else(|| panic!("{case}: no … in {shown:?}"));
                            assert!(
                                !kept.is_empty() && card.club.starts_with(kept),
                                "{case}: cut as {shown:?}"
                            );
                        } else if !card.club.is_empty() {
                            assert_eq!(shown, card.club, "{case}: cut");
                        }
                    }
                }
            }
        });
        out.textures_delta.clear();
    }
    assert!(big >= 3 * 40 * 11 * 5, "{big} big faces checked");
}
/// COLLECTION's page at the Launcher's smallest window (600 x 700), in every language (plan step 1.4, (d)): every card
/// lies inside the page, and its STARTER or IMPORT mark inside its card. (e): a starter card's detail offers no
/// CONVERT and no SELL.
#[test]
fn collection_cards_and_marks_fit_the_page() {
    use forza_ui::collection::{CollectionView, Scenario};
    let size = launcher::window_size(Some(700.0));
    for lang in Lang::ALL {
        let t = |k: &str| forza_ui::text(lang, k, &[]);
        let mut st =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Ready);
        st.profile = Some(forza::profile::Profile::CreateATeam);
        let view = CollectionView::sample(Scenario::Page);
        let cards = view.cards.clone();
        st.collection = Some(view);
        st.tab = launcher::Tab::Collection;
        let mut h = harness(size, 1.0, Edition::E1718, |ui| {
            launcher::show(ui, &mut st);
        });
        h.run_steps(4);
        // the marks are painted, so they are checked where they land, not on the accessibility tree
        let drawn = drawn_texts(&h);
        let mut seen = 0;
        for (card, copies) in &cards {
            let Some(node) = h
                .query_all_by_role_and_label(egui::accesskit::Role::Button, &card.name)
                .next()
            else {
                continue; // scrolled below the window
            };
            let r = node.rect();
            seen += 1;
            let tag = format!("{lang:?} {}: {r:?}", card.name);
            assert!(
                r.left() >= 16.0 && r.right() <= size.x - 16.0,
                "{tag}: inside the page"
            );
            assert!(r.width() >= 80.0, "{tag}: a card, not a chip");
            let mark = if copies.starter > 0 {
                Some("cat.mark_starter")
            } else if copies.imported > 0 {
                Some("cat.mark_import")
            } else {
                None
            };
            if let Some(k) = mark {
                let text = t(k);
                assert!(
                    drawn.iter().any(|s| s.galley.text() == text
                        && r.contains_rect(egui::Rect::from_min_size(s.pos, s.galley.size()))),
                    "{tag}: the mark {text:?} is drawn whole inside the card"
                );
            }
        }
        assert!(seen >= 12, "{lang:?}: {seen} cards in view");
    }
    // (e) the detail of a starter card: it says why the card neither sells nor converts, and the gate CONVERT asks
    // later (step 2.3) refuses a starter-only card while an earned copy in the same collection passes it
    let view = CollectionView::sample(Scenario::Detail);
    let cards = view.cards.clone();
    let mut st =
        launcher::LauncherState::sample(Edition::E1718, Lang::En, launcher::Scenario::Ready);
    st.profile = Some(forza::profile::Profile::CreateATeam);
    st.collection = Some(view);
    st.tab = launcher::Tab::Collection;
    let mut h = harness(size, 1.0, Edition::E1718, |ui| {
        launcher::show(ui, &mut st);
    });
    h.run_steps(4);
    assert!(h.query_by_label("CLOSE").is_some(), "the detail is open");
    let t = |k: &str| forza_ui::text(Lang::En, k, &[]);
    assert!(
        h.query_by_label(&t("collection.no_sell")).is_some(),
        "the starter detail says the card cannot be sold or converted"
    );
    let (starter, earned) = (cards[0].clone(), cards[15].clone());
    assert_eq!(starter.1.earned, 0);
    let starter_row = forza::cards::collection_cards(std::slice::from_ref(&starter.0));
    assert_eq!(
        forza::collection::convert(&starter.1, &starter_row[0]),
        Err(forza::collection::NoConvert::NoEarned)
    );
    let earned_row = forza::cards::collection_cards(std::slice::from_ref(&earned.0));
    assert!(
        forza::collection::convert(&earned.1, &earned_row[0]).is_ok(),
        "a spare earned copy does convert"
    );
}

/// #374: the deal window holds its collection lines inside it and above its foot line, for every grant state and
/// language (the damaged note is the longest).
#[test]
fn the_deal_window_holds_its_collection_lines() {
    for lang in Lang::ALL {
        let t = |k: &str, n: &str| forza_ui::text(lang, k, &[("n", n)]);
        for (grant, key, n) in [
            (panel::Grant::Saving, "deal.saving", ""),
            (panel::Grant::Retrying, "deal.not_saved", ""),
            (panel::Grant::Damaged, "deal.damaged", ""),
            (
                panel::Grant::Saved {
                    tokens: 15,
                    copies: 2,
                    convert: Some(30),
                    floor: 120,
                },
                "deal.sell_from",
                "120",
            ),
        ] {
            let mut st = panel::PanelState::new(
                Edition::E1718,
                lang,
                forza_ui::sample::board(),
                forza_ui::sample::cards().into(),
            );
            let mut deal = panel::Deal::new(3);
            deal.grant = Some(grant);
            st.deal = Some(deal);
            let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
                panel::show(ui, &mut st);
            });
            h.run_steps(4);
            let (plain, rare) = (t("deal.title", ""), t("deal.title_rare", ""));
            let win = h
                .query_by_label(&plain)
                .or_else(|| h.query_by_label(&rare))
                .expect("the deal window")
                .rect();
            let drawn = drawn_texts(&h);
            let rect = |text: &str| {
                drawn
                    .iter()
                    .find(|s| s.galley.text() == text)
                    .map(|s| egui::Rect::from_min_size(s.pos, s.galley.size()))
                    .unwrap_or_else(|| panic!("{lang:?}: {text:?} not drawn"))
            };
            let line = rect(&t(key, n));
            let foot = rect(&t("deal.foot", ""));
            let tag = format!("{lang:?} {key}: line {line:?}, foot {foot:?}, window {win:?}");
            assert!(win.contains_rect(line), "{tag}: the line inside the window");
            assert!(
                line.bottom() <= foot.top(),
                "{tag}: the line above the foot"
            );
        }
    }
}

/// 1.5: START OVER on a damaged collection sits under its line (the page's foot has none then) and asks first; the
/// question fits the Launcher's smallest window in every language: title, body and both buttons inside the dialog,
/// the buttons apart, and START OVER there confirms.
#[test]
fn start_over_asks_first_and_its_question_fits() {
    use egui::accesskit::Role;
    use forza_ui::collection::{CollectionAction, CollectionView, Scenario};
    use std::cell::RefCell;
    let size = launcher::window_size(Some(700.0));
    let window = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    for lang in Lang::ALL {
        let t = |k: &str| forza_ui::text(lang, k, &[]);
        let tag = format!("{lang:?}");
        let mut start =
            launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Ready);
        start.profile = Some(forza::profile::Profile::CreateATeam);
        let mut view = CollectionView::sample(Scenario::Page);
        view.damaged = true;
        start.collection = Some(view);
        start.tab = launcher::Tab::Collection;
        let st = RefCell::new(start);
        let got = RefCell::new(None);
        let mut h = harness(size, 1.0, Edition::E1718, |ui| {
            if let Some(a) = launcher::show(ui, &mut st.borrow_mut()) {
                *got.borrow_mut() = Some(a);
            }
        });
        h.run_steps(4);
        let yes = t("collection.reset");
        let button = h.get_by_label(&yes);
        let r = button.rect();
        assert!(
            window.contains_rect(r) && r.bottom() < size.y / 2.0,
            "{tag}: {r:?} under the damaged line"
        );
        button.click();
        h.run_steps(2);
        let reset = |a| launcher::Action::Collection(a);
        assert_eq!(got.take(), Some(reset(CollectionAction::ResetAsk)), "{tag}");
        // the Launcher's answer (main.rs `ask`)
        st.borrow_mut().confirm = Some(launcher::Confirm::new(
            &t("collection.reset_ask"),
            yes.clone(),
            reset(CollectionAction::Reset),
        ));
        h.run_steps(4);
        // the modal's frame (egui gives it `Role::Dialog`, fit.rs) has no box of its own: its content's
        let dialog = h
            .root()
            .children_recursive()
            .filter(|n| n.accesskit_node().role() == Role::Dialog)
            .find(|n| {
                n.children_recursive()
                    .any(|c| c.accesskit_node().role() == Role::Button)
            })
            .expect("the question");
        let d = dialog
            .children_recursive()
            .filter_map(|n| n.accesskit_node().bounding_box())
            .map(|b| {
                egui::Rect::from_min_max(
                    egui::pos2(b.x0 as f32, b.y0 as f32),
                    egui::pos2(b.x1 as f32, b.y1 as f32),
                )
            })
            .reduce(|a, b| a.union(b))
            .expect("the dialog's content");
        assert!(
            window.contains_rect(d),
            "{tag}: the dialog {d:?} inside the window"
        );
        let button = |label: &str| {
            let found: Vec<egui::Rect> = dialog
                .children_recursive()
                .filter(|n| {
                    n.accesskit_node().role() == Role::Button
                        && n.accesskit_node().label().as_deref() == Some(label)
                })
                .map(|n| n.rect())
                .collect();
            assert_eq!(found.len(), 1, "{tag}: {label:?} in the dialog");
            found[0]
        };
        let (y, n) = (button(&yes), button(&t("common.cancel")));
        assert!(
            d.contains_rect(y) && d.contains_rect(n),
            "{tag}: {y:?} {n:?} inside {d:?}"
        );
        assert!(!y.shrink(0.5).intersects(n), "{tag}: the buttons overlap");
        let ask = t("collection.reset_ask");
        let (title, body) = ask.split_once("\n\n").unwrap();
        let drawn = drawn_texts(&h);
        for line in [title, body] {
            let at = drawn
                .iter()
                .find(|s| s.galley.text() == line)
                .map(|s| egui::Rect::from_min_size(s.pos, s.galley.size()))
                .unwrap_or_else(|| panic!("{tag}: {line:?} drawn"));
            assert!(d.contains_rect(at), "{tag}: {line:?} {at:?} inside {d:?}");
            assert!(at.bottom() <= y.top(), "{tag}: {line:?} above the buttons");
        }
        h.get_all_by_label(&yes)
            .find(|n| n.rect() == y)
            .unwrap()
            .click();
        h.run_steps(2);
        assert_eq!(got.take(), Some(reset(CollectionAction::Reset)), "{tag}");
        assert!(st.borrow().confirm.is_none(), "{tag}: the question closes");
    }
}

/// #374: the start picker and the deal at the Launcher's smallest window, in every language: the start tiles in view
/// lie inside the page and apart; the 11 dealt cards and the 3 RARE cards too.
#[test]
fn the_start_picker_and_the_deal_fit_the_page() {
    use egui::accesskit::Role;
    use forza_ui::collection::{CollectionView, Scenario};
    let size = launcher::window_size(Some(700.0));
    let apart = |rects: &[egui::Rect], tag: &str| {
        for (i, a) in rects.iter().enumerate() {
            assert!(
                a.left() >= 16.0 && a.right() <= size.x - 16.0,
                "{tag}: {a:?} inside the page"
            );
            for b in &rects[i + 1..] {
                assert!(!a.shrink(0.5).intersects(*b), "{tag}: {a:?} overlaps {b:?}");
            }
        }
    };
    for lang in Lang::ALL {
        for s in [Scenario::Choose, Scenario::Pick] {
            let mut st =
                launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Ready);
            st.profile = Some(forza::profile::Profile::CreateATeam);
            let view = CollectionView::sample(s);
            let names: std::collections::BTreeSet<String> = match &view.step {
                forza_ui::collection::Step::Pick { dealt, .. } => {
                    dealt.iter().map(|c| c.name.clone()).collect()
                }
                _ => Default::default(),
            };
            st.collection = Some(view);
            st.tab = launcher::Tab::Collection;
            let mut h = harness(size, 1.0, Edition::E1718, |ui| {
                launcher::show(ui, &mut st);
            });
            h.run_steps(4);
            // the step's own tiles: the radio buttons under its title (the Launcher's own sit above it)
            let head = if s == Scenario::Choose {
                "collection.choose_title"
            } else {
                "collection.pick_title"
            };
            let head = forza_ui::text(lang, head, &[]);
            let top = drawn_texts(&h)
                .iter()
                .find(|t| t.galley.text() == head)
                .map(|t| t.pos.y)
                .expect("the step's title");
            let tag = format!("{lang:?} {s:?}");
            let tiles: Vec<egui::Rect> = h
                .query_all_by_role(Role::RadioButton)
                .map(|n| n.rect())
                .filter(|r| r.top() > top && r.bottom() <= size.y)
                .collect();
            match s {
                Scenario::Choose => {
                    assert!(tiles.len() >= 4, "{tag}: {} starts in view", tiles.len())
                }
                _ => {
                    assert_eq!(tiles.len(), 3, "{tag}: the RARE cards");
                    let dealt: Vec<egui::Rect> = names
                        .iter()
                        .flat_map(|n| h.query_all_by_role_and_label(Role::Image, n))
                        .map(|n| n.rect())
                        .collect();
                    assert!(dealt.len() >= 11, "{tag}: {} dealt cards", dealt.len());
                    apart(&dealt, &tag);
                }
            }
            apart(&tiles, &tag);
        }
    }
}

/// #374: the pack store at the Launcher's smallest window, in every language: the odds of each card and their notes lie
/// inside the page and apart, above BUY, which is in view; the nation tiles lie apart and carry the language's names.
/// The opened pack's 3 cards lie apart with each badge under its own card.
#[test]
fn the_store_and_the_opened_pack_fit_the_page() {
    use egui::accesskit::Role;
    use forza_ui::collection::{CollectionView, Scenario, nation};
    let size = launcher::window_size(Some(700.0));
    let inside = |r: egui::Rect, tag: &str| {
        assert!(
            r.left() >= 16.0 && r.right() <= size.x - 16.0,
            "{tag}: {r:?} inside the page"
        );
    };
    let apart = |rects: &[egui::Rect], tag: &str| {
        for (i, a) in rects.iter().enumerate() {
            inside(*a, tag);
            for b in &rects[i + 1..] {
                assert!(!a.shrink(0.5).intersects(*b), "{tag}: {a:?} overlaps {b:?}");
            }
        }
    };
    let rect = |s: &egui::epaint::TextShape| egui::Rect::from_min_size(s.pos, s.galley.size());
    for lang in Lang::ALL {
        let t = |k: &str, args: &[(&str, &str)]| forza_ui::text(lang, k, args);
        for s in [Scenario::Store, Scenario::Open] {
            let mut st =
                launcher::LauncherState::sample(Edition::E1718, lang, launcher::Scenario::Ready);
            st.profile = Some(forza::profile::Profile::CreateATeam);
            st.collection = Some(CollectionView::sample(s));
            st.tab = launcher::Tab::Collection;
            let mut h = harness(size, 1.0, Edition::E1718, |ui| {
                launcher::show(ui, &mut st);
            });
            h.run_steps(4);
            let tag = format!("{lang:?} {s:?}");
            let drawn = drawn_texts(&h);
            if s == Scenario::Store {
                let korea = nation("Korea Republic", lang);
                let head = t("collection.odds_title", &[("pack", korea)]);
                let top = drawn
                    .iter()
                    .find(|d| d.galley.text() == head)
                    .map(|d| d.pos.y)
                    .unwrap_or_else(|| panic!("{tag}: {head:?} not drawn"));
                let price = forza::collection::NATION_PACK.to_string();
                let buy = h
                    .query_by_label(&t("collection.buy", &[("n", &price)]))
                    .expect("BUY")
                    .rect();
                assert!(buy.bottom() <= size.y, "{tag}: BUY {buy:?} in view");
                inside(buy, &tag);
                let between: Vec<&egui::epaint::TextShape> = drawn
                    .iter()
                    .filter(|d| rect(d).top() >= top && rect(d).bottom() <= buy.top() + 0.5)
                    .collect();
                // row by row: a label in a wrapped line starts its galley at the line's left, before its own text
                let odds: Vec<egui::Rect> = between
                    .iter()
                    .flat_map(|d| {
                        d.galley
                            .rows
                            .iter()
                            .filter(|r| !r.row.glyphs.is_empty())
                            .map(|r| r.rect_without_leading_space().translate(d.pos.to_vec2()))
                    })
                    .collect();
                let tag = format!(
                    "{tag} {:?}",
                    between.iter().map(|d| d.galley.text()).collect::<Vec<_>>()
                );
                for n in ["1", "2", "3"] {
                    let card = t("collection.odds_card", &[("n", n)]);
                    assert!(
                        drawn.iter().any(|d| d.galley.text() == card
                            && d.pos.y > top
                            && d.pos.y < buy.top()),
                        "{tag}: {card:?} between the title and BUY"
                    );
                }
                apart(&odds, &tag);
                let tiles: Vec<egui::Rect> = h
                    .query_all_by_role(Role::RadioButton)
                    .map(|n| n.rect())
                    .filter(|r| r.top() > buy.bottom() && r.bottom() <= size.y)
                    .collect();
                apart(&tiles, &tag);
                if lang == Lang::Ja {
                    assert!(
                        forza::collection::NATIONS
                            .iter()
                            .all(|n| nation(n, lang) != *n),
                        "every nation has its Japanese name"
                    );
                    assert!(h.query_by_label(korea).is_some(), "{tag}: {korea}");
                }
            } else {
                let view = CollectionView::sample(s);
                let forza_ui::collection::Step::Open { cards, .. } = &view.step else {
                    panic!("no pack open")
                };
                let mut faces = vec![];
                for o in cards {
                    let face = h
                        .query_all_by_role_and_label(Role::Image, &o.card.name)
                        .next()
                        .unwrap_or_else(|| panic!("{tag}: {} not shown", o.card.name))
                        .rect();
                    let badge = if o.copies <= 1 {
                        t("collection.new_card", &[])
                    } else {
                        t("collection.copy_n", &[("n", &o.copies.to_string())])
                    };
                    assert!(
                        drawn.iter().any(|d| d.galley.text() == badge
                            && rect(d).left() >= face.left()
                            && rect(d).right() <= face.right()
                            && rect(d).top() >= face.bottom()),
                        "{tag}: {badge:?} under its card {face:?}"
                    );
                    faces.push(face);
                }
                apart(&faces, &tag);
            }
        }
    }
}
