//! Every screen in English, Italian and Japanese: no label is still too wide for its room at `widgets::fit`'s 70 %
//! floor (drawn over its neighbour otherwise, which only a picture would show), and no two clickable widgets share
//! space. No GPU.

use egui::accesskit::Role;
use egui::{Ui, Vec2};
use forza_ui::theme::Edition;
use forza_ui::{Lang, installer, launcher, lobby, panel, widgets};

use super::snapshots::harness;

/// Every screen the audit checks run over, at its default window size and at the smallest window it is laid out for, in
/// English, Italian and Japanese: panel pages, Launcher scenarios, installer pages (and its confirm dialog) and the
/// lobby. `check` gets the screen's name, its size, its edition, the labels an open page draws over (see
/// [`under_page`]) and how to draw it.
#[allow(clippy::type_complexity)] // a screen's drawer, as `harness` takes it
fn screens(mut check: impl FnMut(&str, Vec2, Edition, &[String], &mut dyn FnMut(&mut Ui))) {
    for lang in Lang::ALL {
        let l = lang.code();
        for ed in [Edition::E1011, Edition::E1718] {
            for page in panel::Page::ALL {
                let mut st = panel::PanelState::sample(ed, lang, page);
                // the panel fits its sheet to the window: a 1280x800 window lays out the same points as 1440x900
                let mut show = |ui: &mut Ui| panel::show(ui, &mut st);
                let name = format!("panel {} {ed:?} {l}", page.tag());
                let under = if page == panel::Page::Game {
                    vec![]
                } else {
                    under_page(lang)
                };
                check(&name, panel::SIZE, ed, &under, &mut show);
            }
        }
        use launcher::Scenario::*;
        for s in [
            Ready, Running, StopArmed, NotSetUp, Update, Busy, Report, CleanedUp, Online,
        ] {
            for ed in [Edition::E1011, Edition::E1718] {
                let mut st = launcher::LauncherState::sample(ed, lang, s);
                for size in [launcher::SIZE, launcher::window_size(Some(700.0))] {
                    let mut show = |ui: &mut Ui| {
                        launcher::show(ui, &mut st);
                    };
                    let name = format!("launcher {s:?} {ed:?} {l}");
                    check(&name, size, ed, &[], &mut show);
                }
            }
        }
        for page in installer::Page::ALL {
            let mut st = installer::InstallerState::sample(lang, page);
            for size in [installer::SIZE, Vec2::new(installer::SIZE.x, 520.0)] {
                let mut show = |ui: &mut Ui| {
                    installer::show(ui, &mut st);
                };
                let name = format!("installer {} {l}", page.tag());
                check(&name, size, Edition::E1011, &[], &mut show);
            }
        }
        let mut st = installer::InstallerState::sample(lang, installer::Page::Files);
        st.confirm = true;
        for size in [installer::SIZE, Vec2::new(installer::SIZE.x, 520.0)] {
            let mut show = |ui: &mut Ui| {
                installer::show(ui, &mut st);
            };
            let name = format!("installer confirm {l}");
            check(&name, size, Edition::E1011, &[], &mut show);
        }
        for s in lobby::Scenario::ALL {
            let mut v = lobby::LobbyView::sample(s);
            for size in [lobby::SIZE, Vec2::new(lobby::SIZE.x, 470.0)] {
                let mut show = |ui: &mut Ui| {
                    lobby::show(ui, &mut v, lang);
                };
                let name = format!("lobby {} {l}", s.tag());
                check(&name, size, Edition::E1718, &[], &mut show);
            }
        }
    }
}

/// The overflows `show` leaves on its first frames, as "screen: text (width in room)".
fn overflows(name: &str, size: Vec2, ed: Edition, show: impl FnMut(&mut Ui)) -> Vec<String> {
    let mut h = harness(size, 1.0, ed, show);
    widgets::probe(&h.ctx);
    h.run_steps(8);
    widgets::overflows(&h.ctx)
        .into_iter()
        .map(|o| format!("{name}: {:?} ({:.1} in {:.1})", o.text, o.width, o.room))
        .collect()
}

/// What a click has to reach: buttons, chips (labels that act as buttons), fields and the like.
fn clickable(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::Link
            | Role::CheckBox
            | Role::RadioButton
            | Role::Switch
            | Role::ComboBox
            | Role::TextInput
            | Role::MultilineTextInput
            | Role::Slider
            | Role::SpinButton
            | Role::MenuItem
            | Role::Tab
    )
}

/// The game screen's chrome an open panel page is drawn over: the cabinet deck's buttons and the page bar's
/// (`panel::Layout::pages`). The page's sheet hides them, yet their buttons stay live, so a click meant for the page
/// also sends the deck's key: the audit's one real find (HANDOFF.md #299 has the issue text). Out of this audit until
/// the deck and the bar are off while a page is open.
fn under_page(lang: Lang) -> Vec<String> {
    [
        // the deck: the machine's keys, the play keys, the tactic pad (panel::CABS)
        "cab.central",
        "cab.counter",
        "cab.l_side",
        "cab.r_side",
        "cab.press",
        "cab.data",
        "cab.key_player",
        "cab.start",
        "cab.shoot",
        "cab.keeper",
        "cab.card",
        "cab.coin",
        // the page bar
        "panel.add_card",
        "panel.keys",
        "panel.settings",
        "panel.club_card",
        "pause.button",
    ]
    .into_iter()
    .map(|key| forza_ui::text(lang, key, &[]))
    .collect()
}

/// The clickable widgets of one screen that sit on each other, as "screen: role label {rect} on role label {rect}".
/// Two of them on the page's own layer may not share space (a button drawn over its neighbour takes the clicks meant
/// for it), in any language. A dialog's buttons are in a layer of their own and the widgets under it are clipped or
/// covered, so this reads the page's window: what a player sees and can click. `under` holds the labels the screen's
/// open page is drawn over, which [the panel keeps live](under_page).
fn overlaps(
    name: &str,
    size: Vec2,
    ed: Edition,
    under: &[String],
    show: impl FnMut(&mut Ui),
) -> Vec<String> {
    use egui_kittest::kittest::NodeT as _;
    let mut h = harness(size, 1.0, ed, show);
    h.run_steps(8);
    let window = h.ctx.content_rect();
    let page = |r: egui::Rect| h.ctx.layer_id_at(r.center()) == Some(egui::LayerId::background());
    let mut clicks: Vec<(String, egui::Rect)> = h
        .root()
        .children_recursive()
        .filter_map(|n| {
            let a = n.accesskit_node();
            let label = a.label();
            // a disabled widget takes no click (a cream PLAY under a setup run), nor one a page is drawn over
            if a.is_hidden() || a.is_disabled() || !clickable(a.role()) {
                return None;
            }
            if label
                .as_deref()
                .is_some_and(|l| under.iter().any(|u| u == l))
            {
                return None;
            }
            let b = a.bounding_box()?;
            let r = egui::Rect::from_min_max(
                egui::pos2(b.x0 as f32, b.y0 as f32),
                egui::pos2(b.x1 as f32, b.y1 as f32),
            );
            // the rect a click reaches: the widget's own cut by its clip rect (a scroll area's viewport, say), empty
            // once it is scrolled out of sight. egui keeps the two together, under the id it stores as this node's
            // accesskit id.
            // SAFETY: the value is the high-entropy one egui stored for this very widget (`Id::accesskit_id`, the way
            // back `from_high_entropy_bits` names); the lookup only reads the rects of the frame just run.
            let id = unsafe { egui::Id::from_high_entropy_bits(a.locate().0.0) };
            let r = h.ctx.read_response(id).map_or(r, |resp| resp.interact_rect);
            // of no size, scrolled out of the window or clipped away, or in a dialog over the page
            let shown = r.width() > 1.0 && r.height() > 1.0 && window.contains_rect(r) && page(r);
            shown.then_some((format!("{:?} {label:?}", a.role()), r))
        })
        .collect();
    clicks.sort_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y));
    let mut over = vec![];
    for (i, (a, ra)) in clicks.iter().enumerate() {
        for (b, rb) in &clicks[i + 1..] {
            if shared(*ra, *rb).is_some() {
                over.push(format!("{name} {size:?}: {a} {ra:?} on {b} {rb:?}"));
            }
        }
    }
    over
}

/// The room two clickables share, when it is room a player can aim a click at: `None` for a widget inside another (a
/// button in its row), for two that only touch at an edge, and for a sliver under a point either way — egui's rounding
/// draws a rect a hair past the row or the clip it sits in, where no click lands.
fn shared(a: egui::Rect, b: egui::Rect) -> Option<egui::Rect> {
    if a.contains_rect(b) || b.contains_rect(a) {
        return None;
    }
    let share = a.intersect(b);
    (share.width() > 1.0 && share.height() > 1.0).then_some(share)
}

#[test]
fn every_screen_fits() {
    let mut over = vec![];
    screens(|name, size, ed, _, show| {
        over.extend(overflows(name, size, ed, show));
    });
    assert!(
        over.is_empty(),
        "text wider than its room:\n{}",
        over.join("\n")
    );
}

/// UI audit invariant 3 (#299): no two clickable widgets on each other, the check that would have caught the pop-out
/// board's foot buttons (#285) in any language.
#[test]
fn no_two_clickables_overlap() {
    let mut over = vec![];
    screens(|name, size, ed, under, show| {
        over.extend(overlaps(name, size, ed, under, show));
    });
    assert!(
        over.is_empty(),
        "clickable widgets on each other:\n{}",
        over.join("\n")
    );
}

/// The audit's pair rule still catches a button on another (#285's foot buttons), and only that: two widgets side by
/// side, a button in its own row, or a field drawn a hair past its row are no room a click can aim at.
#[test]
fn a_button_on_another_is_caught() {
    let a = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 30.0));
    let on = egui::Rect::from_min_size(egui::pos2(50.0, 10.0), egui::vec2(100.0, 30.0));
    assert!(shared(a, on).is_some(), "{a:?} on {on:?}");
    for (at, size) in [
        ((100.0, 0.0), (80.0, 30.0)), // beside it, one edge shared
        ((0.0, 30.0), (100.0, 30.0)), // under it, one edge shared
        ((0.0, 29.0), (100.0, 30.0)), // a field a point into the row above: rounding
        ((10.0, 10.0), (80.0, 10.0)), // a button in its own row
    ] {
        let b = egui::Rect::from_min_size(egui::pos2(at.0, at.1), egui::vec2(size.0, size.1));
        assert!(shared(a, b).is_none(), "{a:?} on {b:?}");
    }
}

/// The text painted exactly as `text`: its laid-out rect (what `widgets::fit` measures) and whether it was cut with "…".
fn painted(h: &egui_kittest::Harness<'_>, text: &str) -> (egui::Rect, bool) {
    h.output()
        .shapes
        .iter()
        .find_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.text() == text => {
                Some((t.galley.rect.translate(t.pos.to_vec2()), t.galley.elided))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{text:?} is not painted"))
}

/// #286: text painted in a fixed room shrinks, then is cut with "…", and never runs past its room, in every language:
/// a club and its manager from the game on the club card (300 points), a wallet club beside its NOT PLAYABLE (a
/// 170-point button), the crash screen's title (its column, 440 at most) and the free space right of the installer's
/// bar (112). The old plain `p.text(..)` calls drew the long manager and free-space lines at full width, past their
/// rooms, and the old `widgets::fit` left a long club name wider than its room at the 70 % floor.
#[test]
fn fixed_rooms_fit_long_text() {
    let ja = "櫻庭和志".repeat(12);
    let names = [
        "H. SAKURABA DI SAN GIOVANNI DELLA VALLE DEL FIUME AZZURRO E DEI MONTI LONTANI",
        ja.as_str(),
    ];
    for lang in Lang::ALL {
        for name in names {
            let raw = format!(
                "state|ok\nhead|CLUB\nrow|CLUB|{name}  (founded 2026-09-02)|w\n\
                 row|MANAGER|{name}  (born 1980-04-01)|w\nwallet|n.bin|{name}|W1|transferred\n"
            );
            let mut st = panel::PanelState::sample(Edition::E1011, lang, panel::Page::ClubCard);
            st.club = forza::club_view::ClubView::parse(raw.as_bytes());
            let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
                panel::show(ui, &mut st)
            });
            h.run_steps(8);
            let manager = forza_ui::text(lang, "club.manager", &[("name", name)]);
            let (r, cut) = painted(&h, &manager);
            assert!(r.width() <= 300.0 && cut, "{manager:?}: {r:?}, cut {cut}");
            let (r, cut) = painted(&h, name);
            assert!(r.width() <= 300.0 && cut, "club {name:?}: {r:?}, cut {cut}");
            let not = forza_ui::text(lang, "club.transferred", &[]);
            let (b, _) = painted(&h, &not);
            assert!(b.width() <= 170.0, "{not:?}: {b:?}");
            // the wallet keeps 39 characters of a club's name, beside the NOT PLAYABLE button
            let club: String = name.chars().take(39).collect();
            let (r, _) = painted(&h, &club);
            assert!(r.right() < b.left(), "wallet {club:?}: {r:?} on {b:?}");
        }
        let mut st = panel::PanelState::sample(Edition::E1011, lang, panel::Page::Game);
        st.crash = true;
        let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
            panel::show(ui, &mut st)
        });
        h.run_steps(8);
        let title = forza_ui::text(lang, "panel.crash_title", &[]);
        let (r, _) = painted(&h, &title);
        assert!(r.width() <= 440.0, "{title:?}: {r:?}");

        let mut st = installer::InstallerState::sample(lang, installer::Page::Where);
        let sp = st.space.as_mut().expect("the sample has a drive");
        sp.free = u64::MAX;
        sp.total = u64::MAX;
        let mut h = harness(installer::SIZE, 1.0, Edition::E1011, |ui| {
            installer::show(ui, &mut st);
        });
        h.run_steps(8);
        let free = forza_ui::text(
            lang,
            "installer.free_of",
            &[("free", "18446744073.7"), ("drive", "C:")],
        );
        let (r, _) = painted(&h, &free);
        assert!(r.width() <= 112.0, "{free:?}: {r:?}");
    }
}
