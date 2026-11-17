//! The panel, Launcher and lobby screens laid out without drawing: rects, labels and state after clicks. No GPU, so
//! CI's build-test runs them with the rest (the drawn screens are in `snapshots`).

use egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use forza_ui::Lang;
use forza_ui::theme::{self, Edition};
use forza_ui::{installer, launcher, lobby, panel, widgets};

use super::snapshots::{formations_state, harness, import_asked};

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
                v.host_ask = Some((false, String::new(), lobby::CupChoice::default()));
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
                left > 196.0 + 24.0 + 120.0,
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
            add.bottom() <= installer::SIZE.y - 64.0,
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
        refusal.top() > 0.0 && refusal.bottom() < short.y - 64.0,
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
        let (board, _) = panel::desk_board_rect(room);
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
            h.run_steps(2);
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
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(got.take(), None);
    assert!(v.borrow().host_ask.is_none() && !v.borrow().list_arcade);
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

/// The big card floating over the pitch (the 1440x900 sheet) takes the pointer only on itself and its FRONT | BACK
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
    let live = area(&h, "board-big-card")
        .unwrap()
        .union(area(&h, "board-big-switch").unwrap());
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
