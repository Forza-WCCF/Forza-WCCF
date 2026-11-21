use super::*;

// ---- the widget sheet ----

pub const SHEET_SIZE: egui::Vec2 = egui::vec2(1200.0, 820.0);

/// State for the sheet that shows every widget state side by side.
#[derive(Clone, Debug)]
pub struct WidgetSheet {
    pub lang: Lang,
    pub seg: usize,
    pub range: (u8, u8),
    pub total: (u8, u8),
    pub toggles: [bool; 1],
    pub launcher_lang: Lang,
}

impl WidgetSheet {
    pub fn sample(lang: Lang) -> Self {
        WidgetSheet {
            lang,
            seg: 1,
            range: (8, 17),
            total: (60, 100),
            toggles: [false],
            launcher_lang: lang,
        }
    }
}

pub fn widget_sheet(ui: &mut Ui, ws: &mut WidgetSheet) {
    let lang = ws.lang;
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::NIGHT).inner_margin(20))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            // the title bar, at the launcher's width
            let (bar, _) = ui.allocate_exact_size(vec2(600.0, 44.0), Sense::hover());
            let mut tb = ui.new_child(egui::UiBuilder::new().max_rect(bar));
            widgets::title_bar(&mut tb, Edition::E1011, "", &mut ws.launcher_lang);
            caps(ui, "CABINET: IDLE · LIT · PRESSED · HOVER · DISABLED");
            ui.horizontal(|ui| {
                let kinds = [
                    (Cab::Tactic, "cab.central"),
                    (Cab::Shoot, "cab.shoot"),
                    (Cab::Keeper, "cab.keeper"),
                    (Cab::KeyPlayer, "cab.key_player"),
                    (Cab::Data, "cab.data"),
                    (Cab::Go, "cab.start"),
                    (Cab::Go, "launcher.play"),
                ];
                for (kind, key) in kinds {
                    ui.vertical(|ui| {
                        let label = tr!(lang, key);
                        for f in [
                            Face {
                                lit: false,
                                glow: 0.0,
                                press: 0.0,
                                hover: 0.0,
                                enabled: true,
                            },
                            Face {
                                lit: true,
                                glow: 1.0,
                                press: 0.0,
                                hover: 0.0,
                                enabled: true,
                            },
                            Face {
                                lit: true,
                                glow: 1.0,
                                press: 1.0,
                                hover: 0.0,
                                enabled: true,
                            },
                            Face {
                                lit: false,
                                glow: 0.0,
                                press: 0.0,
                                hover: 1.0,
                                enabled: true,
                            },
                            Face {
                                lit: false,
                                glow: 0.0,
                                press: 0.0,
                                hover: 0.0,
                                enabled: false,
                            },
                        ] {
                            let (r, _) = ui.allocate_exact_size(vec2(112.0, 48.0), Sense::hover());
                            cabinet_face(ui.painter(), r, &label, kind, f);
                        }
                    });
                }
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    caps(ui, "CARDS");
                    ui.horizontal(|ui| {
                        let cards = crate::sample::cards();
                        for i in [5, 1, 3] {
                            let (r, _) = ui.allocate_exact_size(vec2(100.0, 140.0), Sense::hover());
                            board::card_face(ui.painter(), r, &cards[i], board::Look::default());
                        }
                    });
                    ui.horizontal(|ui| {
                        let cards = crate::sample::cards();
                        let looks = [
                            board::Look::default(),
                            board::Look {
                                lifted: true,
                                ..Default::default()
                            },
                            board::Look {
                                ghost: true,
                                ..Default::default()
                            },
                            board::Look {
                                target: true,
                                ..Default::default()
                            },
                            board::Look {
                                warn: true,
                                ..Default::default()
                            },
                            board::Look {
                                selected: true,
                                ..Default::default()
                            },
                        ];
                        for (j, look) in looks.into_iter().enumerate() {
                            ui.add_space(4.0);
                            let (r, _) = ui.allocate_exact_size(
                                board::CARD_SMALL + vec2(12.0, 16.0),
                                Sense::hover(),
                            );
                            board::card_face(
                                ui.painter(),
                                Rect::from_center_size(r.center(), board::CARD_SMALL),
                                &cards[6 + j],
                                look,
                            );
                        }
                    });
                });
            });
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(520.0);
                    status_line(ui, Tone::Done, &tr!(lang, "panel.msg_added"));
                    status_line(ui, Tone::Refused, &tr!(lang, "panel.msg_session_open"));
                    status_line(ui, Tone::Done, &tr!(lang, "panel.msg_saved"));
                    segmented(
                        ui,
                        &mut ws.seg,
                        &[
                            &tr!(lang, "launcher.this_pc"),
                            &tr!(lang, "launcher.online"),
                        ],
                        520.0,
                    );
                    ui.horizontal(|ui| {
                        chip(ui, &tr!(lang, "cat.all"), true, None);
                        chip(ui, &tr!(lang, "cat.legend"), false, None);
                        for p in Position::ALL {
                            chip(ui, p.code(), p == Position::Mf, Some(p.color()));
                        }
                        widgets::badge(ui, Edition::E1011);
                        widgets::badge(ui, Edition::E1718);
                    });
                    widgets::steps(ui, 3, 7, 520.0);
                    widgets::progress(ui, 0.62, 520.0);
                    widgets::toggle_row(
                        ui,
                        &mut ws.toggles[0],
                        &tr!(lang, "installer.opt_shortcuts"),
                        &tr!(lang, "installer.opt_shortcuts_hint"),
                    );
                });
                ui.add_space(20.0);
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    for (k, v) in STATS.iter().zip([4u8, 9, 12, 15, 18, 20]) {
                        stat_bar(ui, &tr!(lang, *k), v, 300.0);
                    }
                    range_slider(ui, &tr!(lang, "cat.off"), &mut ws.range, (1, 20), 300.0);
                    range_slider(ui, &tr!(lang, "cat.total"), &mut ws.total, (60, 100), 300.0);
                });
                ui.add_space(20.0);
                ui.vertical(|ui| {
                    for (t, s) in [
                        ("Space", KeyState::Bound),
                        ("", KeyState::Capturing),
                        ("", KeyState::Empty),
                        ("K", KeyState::Changed),
                    ] {
                        key_cell(ui, t, s, 200.0, lang);
                    }
                    for r in Rarity::ALL {
                        ui.horizontal(|ui| {
                            let (m, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                            board::rarity_mark(ui.painter(), m.center(), 18.0, r);
                            ui.label(
                                egui::RichText::new(tr!(lang, rarity_key(r)))
                                    .font(theme::display(16.0))
                                    .color(r.rim()),
                            );
                        });
                    }
                });
            });
        });
}

/// The share of its width each foot button keeps in a row `room` wide (`widths` include `pad`): 1 when they fit, else
/// the row less its `gaps` split in proportion; never under `pad` for any button (a row narrower than that is past
/// every design size: the buttons then run past its left edge, never down to nothing).
pub(super) fn foot_squeeze(widths: &[f32], gaps: f32, pad: f32, room: f32) -> f32 {
    let need = widths.iter().sum::<f32>() + gaps;
    if need <= room {
        return 1.0;
    }
    let least = widths.iter().copied().fold(f32::INFINITY, f32::min);
    ((room - gaps) / (need - gaps)).max(pad / least)
}
