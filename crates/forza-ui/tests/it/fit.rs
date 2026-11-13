//! Every screen in English, Italian and Japanese with `widgets::fit`'s ledger on: no label is still too wide for its
//! room at fit's 70 % floor (drawn over its neighbour otherwise, which only a picture would show). No GPU.

use egui::{Ui, Vec2};
use forza_ui::theme::Edition;
use forza_ui::{Lang, installer, launcher, lobby, panel, widgets};

use super::snapshots::harness;

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

#[test]
fn every_screen_fits() {
    let mut over = vec![];
    for lang in Lang::ALL {
        let l = lang.code();
        for ed in [Edition::E1011, Edition::E1718] {
            for page in panel::Page::ALL {
                let mut st = panel::PanelState::sample(ed, lang, page);
                let name = format!("panel {} {ed:?} {l}", page.tag());
                over.extend(overflows(&name, panel::SIZE, ed, |ui| {
                    panel::show(ui, &mut st)
                }));
            }
        }
        use launcher::Scenario::*;
        for s in [
            Ready, Running, StopArmed, NotSetUp, Update, Busy, Report, CleanedUp, Online,
        ] {
            for ed in [Edition::E1011, Edition::E1718] {
                let mut st = launcher::LauncherState::sample(ed, lang, s);
                let name = format!("launcher {s:?} {ed:?} {l}");
                over.extend(overflows(&name, launcher::SIZE, ed, |ui| {
                    launcher::show(ui, &mut st);
                }));
            }
        }
        for page in installer::Page::ALL {
            let mut st = installer::InstallerState::sample(lang, page);
            let name = format!("installer {} {l}", page.tag());
            over.extend(overflows(&name, installer::SIZE, Edition::E1011, |ui| {
                installer::show(ui, &mut st);
            }));
        }
        let mut st = installer::InstallerState::sample(lang, installer::Page::Files);
        st.confirm = true;
        let name = format!("installer confirm {l}");
        over.extend(overflows(&name, installer::SIZE, Edition::E1011, |ui| {
            installer::show(ui, &mut st);
        }));
        for s in lobby::Scenario::ALL {
            let mut v = lobby::LobbyView::sample(s);
            let name = format!("lobby {} {l}", s.tag());
            over.extend(overflows(&name, lobby::SIZE, Edition::E1718, |ui| {
                lobby::show(ui, &mut v, lang);
            }));
        }
    }
    assert!(
        over.is_empty(),
        "text wider than its room:\n{}",
        over.join("\n")
    );
}
