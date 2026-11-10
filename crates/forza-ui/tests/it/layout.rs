//! The panel, Launcher and lobby screens laid out without drawing: rects, labels and state after clicks. No GPU, so
//! CI's build-test runs them with the rest (the drawn screens are in `snapshots`).

use egui::Vec2;
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
            "Download high-quality pictures",
        ),
        (
            Edition::E1718,
            "Download high-quality pictures",
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
    assert!(h.get_by_label("HOST VS").rect().top() > h.get_by_label("MILANO CUP").rect().bottom());
}
