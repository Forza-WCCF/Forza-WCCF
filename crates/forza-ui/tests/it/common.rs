//! What `behaviour`, `fit`, `layout` and `snapshots` share: the harness the screens are driven in, and the sample
//! states more than one of them starts from.

use eframe::egui_wgpu;
use egui::{Ui, Vec2};
use egui_kittest::wgpu::{WgpuTestRenderer, create_render_state, default_wgpu_setup};
use egui_kittest::{Harness, LazyRenderer};
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
        .renderer(LazyRenderer::new(renderer))
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

/// kittest's own renderer on one wgpu device for the whole test process: a new device per rendered harness (kittest's
/// default) was most of a snapshot's time on WARP. The egui renderer, whose textures belong to one context, stays new.
fn renderer() -> WgpuTestRenderer {
    static DEVICE: std::sync::OnceLock<egui_wgpu::RenderState> = std::sync::OnceLock::new();
    let options = egui_wgpu::RendererOptions::PREDICTABLE;
    let mut state = DEVICE
        .get_or_init(|| create_render_state(default_wgpu_setup(), options))
        .clone();
    let fresh = egui_wgpu::Renderer::new(&state.device, state.target_format, options);
    state.renderer = std::sync::Arc::new(egui::mutex::RwLock::new(fresh));
    WgpuTestRenderer::from_render_state(state)
}

/// Steps `frames` frames, waiting on a picture that loads on a thread only while one is loading (a fixed 25 ms a
/// frame cost the catalogue tests 9 s), then on until none is, at most 10 s.
pub(super) fn steps_loaded(h: &mut Harness<'_>, frames: usize) {
    let start = std::time::Instant::now();
    for frame in 0.. {
        let pending = h.ctx.has_pending_images();
        if frame >= frames && !pending {
            return;
        }
        if pending {
            assert!(
                start.elapsed().as_secs() < 10,
                "pictures still loading after 10 s"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.run_steps(1);
    }
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
