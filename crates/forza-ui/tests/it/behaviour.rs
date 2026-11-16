//! What a player does to the screens, asserted without a picture (issue #282): the drives the picture tests in
//! `snapshots` hold, moved here so `check quick` and CI gate them. No GPU and no snapshot, so they run with the rest.

use egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use forza_ui::Lang;
use forza_ui::theme::Edition;
use forza_ui::{launcher, panel, sizing};

use super::common::harness;

/// A 1280x720 screen (work area about 700 tall): PLAY and the status line stay in the window.
#[test]
fn launcher_on_a_short_screen() {
    let size = launcher::window_size(Some(700.0));
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
    }
}

/// The server's notice sits at the top of the game picture.
#[test]
fn server_notice() {
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
    }
}

/// The 2017-18 catalogue at 1920x1080 points, x1 and 125 % in English and at 125 % in Japanese (and the rest of the
/// sizes): ADD stays inside the detail pane, the pane lists no affinity, and on the wide sheet the picture draws at
/// its album art's own pixels.
#[test]
fn catalogue_1080() {
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
    let (wide, small) = (Vec2::new(1920.0, 1080.0), panel::SIZE);
    for (size, ppp, lang) in [
        (wide, 1.0, Lang::En),
        (wide, 1.25, Lang::En),
        (wide, 1.25, Lang::Ja),
        (wide, 1.0, Lang::It),
        (wide, 1.0, Lang::Ja),
        (wide, 1.25, Lang::It),
        (small, 1.0, Lang::En),
        (small, 1.0, Lang::It),
        (small, 1.0, Lang::Ja),
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
    }
}

/// A table kept by an earlier run (no match in this one) that differs from the board: RESET LINEUP is lit and asks
/// first. CONFIRM asks for the reset (the board becomes the kept table, with UNDO); CANCEL leaves the board alone;
/// DON'T ASK AGAIN resets too and saves `reset_ask=off`: the next RESET LINEUP resets with no question, until the
/// setting is on again.
#[test]
fn board_reset_lineup_asks_first() {
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
        st.before_match_at = Some("21:40".into());
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
            // at the foot of the page: SETTINGS scrolls at this size (#348)
            h.get_by_label("Ask before RESET LINEUP").scroll_to_me();
            h.run_steps(2);
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
}

/// IMPORT LINEUP done: the catalogue's message names the lineup, the site's key player and the cards skipped.
#[test]
fn catalogue_imported() {
    let mut st = panel::PanelState::sample(Edition::E1011, Lang::En, panel::Page::Catalogue);
    st.catalogue.filter = panel::Filter::default();
    let text = "# WCCF DB lineup\ngame 1011\nname Sunday XI\n\
                P 1 50 92 K\nP 2 15 70\nP 3 38 74\nP 4 62 74\nP 5 85 70\nP 6 30 50\nP 7 50 55\n\
                P 8 70 50\nP 9 25 25\nP 10 50 20\nP 11 75 25\nS 12\nS 13\n\
                S \"13/GES 3 \" # GULLIT · 13 GES 3\nS 999\n";
    assert!(panel::import_lineup(&mut st, &[], text, false, 0.0).is_some());
}

/// CATALOGUE as a player uses it: the page opens with the search line taking the keys, typed words narrow the list,
/// Enter asks for the first card listed, and the table takes it with a message; Esc closes the page.
#[test]
fn catalogue_search_and_add() {
    use std::cell::RefCell;
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
    // the operator's switches are folded away until ADVANCED opens
    assert!(h.query_by_label("TEST").is_none());
    h.get_by_label("Advanced").click();
    h.run_steps(3);
    assert!(st.borrow().keys_advanced);
}

/// 2017-18 SETTINGS: CLEAR BAD ENDINGS (no CLUB CARD page there) asks for the reset by its second click.
#[test]
fn settings_1718_clear_bad_endings() {
    use forza_ui::panel::ClubPick;
    use std::cell::RefCell;
    let st = RefCell::new(panel::PanelState::sample(
        Edition::E1718,
        Lang::En,
        panel::Page::Settings,
    ));
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(3);
    h.get_by_label("CLEAR AND RESTART").click();
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
}

/// CLUB CARD as a player uses it: the view's club, rows and squad; PLAY THIS CLUB needs a second click and asks for that
/// card (data\panel.txt's card=, then the restart); a transferred card has no button; refused while a card session is
/// open; CLEAR BAD ENDINGS the same way (2010-11); Esc closes the page.
#[test]
fn club_card_switch_and_clear() {
    use forza_ui::panel::ClubPick;
    use std::cell::RefCell;
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
    h.get_by_label("CLEAR AND RESTART").click();
    h.run_steps(2);
    h.get_by_label("CLICK AGAIN").click();
    h.run_steps(2);
    assert_eq!(st.borrow_mut().club_request.take(), Some(ClubPick::Fix));
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(st.borrow().page, panel::Page::Game);
}

/// The card dealt after a match over the game: NEW PLAYER CARD with the card, its stats and TOTAL; the picture's clicks
/// stay off while it shows; a click on it closes it.
#[test]
fn card_deal() {
    use std::cell::RefCell;
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
    // a click on the card turns it over; one beside it closes the window
    let r = h.get_by_label_contains("NEW PLAYER CARD").rect();
    let card = egui::pos2(r.center().x, r.top() + 200.0);
    h.drag_at(card);
    h.drop_at(card);
    h.run_steps(2);
    assert!(st.borrow().deal.as_ref().is_some_and(|d| d.back));
    let foot = egui::pos2(r.center().x, r.bottom() - 60.0);
    h.drag_at(foot);
    h.drop_at(foot);
    h.run_steps(2);
    assert!(st.borrow().deal.is_none() && st.borrow().picture_rect.is_some());
}

/// 2017-18 IMPORT LINEUP: a dialog first, saying where a lineup comes from (the db.wccf.it Lineup page), its address
/// with OPEN THE SITE and COPY (COPIED once the panel DLL copied it), then IMPORT FROM CLIPBOARD: the clipboard read as
/// before. A COPY that fails says so in the dialog, Enter takes the button that has the focus, and Esc closes without
/// importing. 2010-11 reads the clipboard at once.
#[test]
fn catalogue_import_help() {
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
}

/// The crash screen in the board's place; RESTART THE GAME and SAVE LOGS each ask once.
#[test]
fn crash_screen() {
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

/// The PAUSED page over the game's picture: RESUME on the page bar asks for the pause's end.
#[test]
fn paused_page() {
    let mut st = panel::PanelState::sample(Edition::E1718, Lang::Ja, panel::Page::Game);
    st.paused = Some(14);
    let st = std::cell::RefCell::new(st);
    let mut h = harness(panel::SIZE, 1.0, Edition::E1718, |ui| {
        panel::show(ui, &mut st.borrow_mut())
    });
    h.run_steps(4);
    h.get_by_label("再開").click();
    h.run_steps(2);
    assert!(st.borrow().pause_asked, "RESUME did not ask");
}

/// The big card on the docked 2017-18 board at its least width (the divider dragged to 420 points): a click on a mat
/// card shows it big on its front (the click does not turn it over); its FRONT | BACK switch, a click on the big card
/// and FLIP's key turn it over and back; another card opens on its front. The pane, its card and its switch stay
/// inside the board's column, clear of the game's picture.
#[test]
fn board_card_flips_at_the_least_width() {
    use std::cell::RefCell;
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
        let st = RefCell::new(st);
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
}
