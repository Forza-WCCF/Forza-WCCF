//! What `behaviour`, `fit`, `layout` and `snapshots` share: the harness the screens are driven in, and the sample
//! states more than one of them starts from.

use egui::{Ui, Vec2};
use egui_kittest::Harness;
use forza_ui::Lang;
use forza_ui::panel;
use forza_ui::theme::{self, Edition};

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

/// IMPORT LINEUP's question over the catalogue: the cards that come in and those skipped, CONFIRM and CANCEL.
pub(super) fn import_asked(lang: Lang) -> panel::PanelState {
    let mut st = panel::PanelState::sample(Edition::E1011, lang, panel::Page::Catalogue);
    st.catalogue.filter = panel::Filter::default();
    let text = "game 1011\nP 1 50 92\nP 2 15 70\nP 3 38 74\nS 999\n";
    assert!(panel::ask_import(&mut st, text, false));
    st
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
