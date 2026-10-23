//! The design gallery: every screen and widget state with sample data, in English, Italian or Japanese, for either
//! edition. `cargo xtask gallery` builds and starts it. With `FORZA_CARDS=<folder>` (the kit's `data\cache\cards`
//! on this PC), the sample cards wear the first real pictures of that folder; without it, the drawn stand-in.

use eframe::egui;
use egui::{Rect, Ui, vec2};
use forza_ui::theme::{self, Edition};
use forza_ui::widgets::{chip, segmented};
use forza_ui::{Lang, installer, launcher, panel};

/// One entry of the gallery.
#[derive(Clone, Copy, PartialEq)]
enum Item {
    Launcher(launcher::Scenario),
    Installer(installer::Page),
    Panel(panel::Page),
    Compact,
    Widgets,
}

impl Item {
    fn all() -> Vec<(Item, &'static str)> {
        use launcher::Scenario::*;
        let mut v = vec![
            (Item::Launcher(Ready), "Launcher"),
            (Item::Launcher(Running), "running"),
            (Item::Launcher(StopArmed), "stop?"),
            (Item::Launcher(NotSetUp), "not set up"),
            (Item::Launcher(Update), "update"),
            (Item::Launcher(Busy), "busy"),
            (Item::Launcher(Online), "online"),
        ];
        for p in installer::Page::ALL {
            v.push((Item::Installer(p), p.tag()));
        }
        for p in panel::Page::ALL {
            v.push((Item::Panel(p), p.tag()));
        }
        v.push((Item::Compact, "compact"));
        v.push((Item::Widgets, "widgets"));
        v
    }
}

enum Screen {
    Launcher(Box<launcher::LauncherState>),
    Installer(Box<installer::InstallerState>),
    Panel(Box<panel::PanelState>),
    Widgets(panel::WidgetSheet),
}

struct Gallery {
    item: Item,
    lang: Lang,
    edition: Edition,
    screen: Screen,
    ctx: egui::Context,
    /// Card numbers that have a picture in `FORZA_CARDS`, lowest first; empty without it.
    pictures: Vec<u32>,
}

/// The card numbers with a `<number>.png` in `dir`, lowest first.
fn picture_numbers(dir: &std::path::Path) -> Vec<u32> {
    let mut v: Vec<u32> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.path().file_stem()?.to_str()?.parse().ok())
        .collect();
    v.sort_unstable();
    v
}

impl Gallery {
    fn rebuild(&mut self) {
        let (l, e) = (self.lang, self.edition);
        self.screen = match self.item {
            Item::Launcher(s) => {
                Screen::Launcher(Box::new(launcher::LauncherState::sample(e, l, s)))
            }
            Item::Installer(p) => {
                Screen::Installer(Box::new(installer::InstallerState::sample(l, p)))
            }
            Item::Panel(p) => Screen::Panel(Box::new(panel::PanelState::sample(e, l, p))),
            Item::Compact => {
                let mut st = panel::PanelState::sample(e, l, panel::Page::Game);
                st.file.compact = true;
                Screen::Panel(Box::new(st))
            }
            Item::Widgets => Screen::Widgets(panel::WidgetSheet::sample(l)),
        };
        // real pictures: the sample cards take the numbers of the folder's first pictures
        if let Screen::Panel(st) = &mut self.screen {
            for (card, n) in st.board.cards.iter_mut().zip(&self.pictures) {
                card.number = *n;
            }
        }
        // the accent follows the edition
        self.ctx.set_global_style(theme::style(e));
    }
}

impl eframe::App for Gallery {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let mut changed = false;
        egui::Panel::top("gallery-bar")
            .frame(
                egui::Frame::new()
                    .fill(theme::DEEP)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (item, label) in Item::all() {
                        if chip(ui, label, self.item == item, None).clicked() {
                            self.item = item;
                            changed = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let mut l = Lang::ALL.iter().position(|x| *x == self.lang).unwrap_or(0);
                    if segmented(ui, &mut l, &["English", "Italiano", "日本語"], 300.0) {
                        self.lang = Lang::ALL[l];
                        changed = true;
                    }
                    let mut e = (self.edition == Edition::E1718) as usize;
                    if segmented(
                        ui,
                        &mut e,
                        &[Edition::E1011.label(), Edition::E1718.label()],
                        200.0,
                    ) {
                        self.edition = if e == 1 {
                            Edition::E1718
                        } else {
                            Edition::E1011
                        };
                        changed = true;
                    }
                });
            });
        if changed {
            self.rebuild();
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::WELL))
            .show(ui, |ui| {
                let size = match self.screen {
                    Screen::Launcher(_) => launcher::SIZE,
                    Screen::Installer(_) => installer::SIZE,
                    Screen::Panel(_) => panel::SIZE,
                    Screen::Widgets(_) => panel::SHEET_SIZE,
                };
                // each screen at its real size, in the top-left corner (scroll the window for the 1440×900 panel)
                egui::ScrollArea::both().show(ui, |ui| {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    let mut child = ui.new_child(
                        egui::UiBuilder::new().max_rect(Rect::from_min_size(rect.min, size)),
                    );
                    child.set_clip_rect(rect.intersect(ui.clip_rect()));
                    match &mut self.screen {
                        Screen::Launcher(st) => {
                            launcher::show(&mut child, st);
                        }
                        Screen::Installer(st) => {
                            installer::show(&mut child, st);
                        }
                        Screen::Panel(st) => panel::show(&mut child, st),
                        Screen::Widgets(st) => panel::widget_sheet(&mut child, st),
                    }
                });
            });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Forza-WCCF design gallery")
            .with_inner_size(vec2(1460.0, 1000.0)),
        ..Default::default()
    };
    eframe::run_native(
        "forza-gallery",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx, Edition::E1011, &theme::font_dir())?;
            let cards = std::env::var_os("FORZA_CARDS").map(std::path::PathBuf::from);
            let pictures = cards.as_deref().map(picture_numbers).unwrap_or_default();
            forza_ui::board::set_picture_dir(&cc.egui_ctx, cards);
            let item = Item::Launcher(launcher::Scenario::Ready);
            let mut g = Gallery {
                item,
                lang: Lang::En,
                edition: Edition::E1011,
                screen: Screen::Widgets(panel::WidgetSheet::sample(Lang::En)),
                ctx: cc.egui_ctx.clone(),
                pictures,
            };
            g.rebuild();
            Ok(Box::new(g))
        }),
    )
}
