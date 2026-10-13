//! Every screen rendered to PNG in English, Italian and Japanese: what the owner reviews, then the regression check
//! (`UPDATE_SNAPSHOTS=1` writes new ones). Rendered with wgpu on the machine's GPU.

use egui::{Ui, Vec2};
use egui_kittest::{Harness, SnapshotResults};
use forza::ui::Lang;
use forza::ui::theme::{self, Edition};
use forza::ui::{installer, launcher, panel};

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
            (StopConfirm, Edition::E1718, "stop"),
            (NotSetUp, Edition::E1011, "setup"),
            (Update, Edition::E1011, "update"),
        ] {
            let mut st = launcher::LauncherState::sample(ed, lang, s);
            snap(
                &mut all,
                &format!("launcher_{tag}_{}", lang.code()),
                launcher::SIZE,
                ed,
                |ui| launcher::show(ui, &mut st),
            );
        }
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
                installer::show(ui, &mut st)
            });
        }
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
    st.compact = true;
    snap(
        &mut all,
        "panel_compact_it",
        panel::SIZE,
        Edition::E1011,
        |ui| panel::show(ui, &mut st),
    );
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
    let mut h = harness(panel::SIZE, 1.0, Edition::E1011, |ui| {
        panel::show(ui, &mut st)
    });
    h.run_steps(4);
    // a box-to-box midfielder in hand over the striker on the left of midfield: the heat zone and a FITS badge
    let from = panel::board_slot_center(6);
    let to = panel::board_slot_center(5);
    h.drag_at(from);
    h.run_steps(2);
    h.hover_at(to);
    h.run_steps(3);
    h.snapshot("board_drag_en");
    h.drop_at(to);
    h.run_steps(2);
    // the big card of a centre back, with its position map and the heat zone on the pitch
    h.hover_at(panel::board_slot_center(2));
    h.run_steps(8); // past the 1 s hover delay at 0.25 s a step
    h.snapshot("board_hover_en");
}

/// A 1920×1080 screen: the sheet scales by 1.2 and the 1600-point-wide sheet gives the extra width to the middle.
#[test]
fn panel_1920x1080() {
    let _gpu = gpu();
    let mut st = panel::PanelState::sample(Edition::E1011, Lang::It, panel::Page::Game);
    let mut h = harness(Vec2::new(1600.0, 900.0), 1.2, Edition::E1011, |ui| {
        panel::show(ui, &mut st)
    });
    h.run_steps(8);
    h.snapshot("panel_game_1080_it");
}
