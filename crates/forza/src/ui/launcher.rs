//! The launcher: one frameless 600 px window to play, set up, update and read the output. Layout over sample state.

use super::Lang;
use super::theme::{self, Edition};
use super::widgets::{
    self, Cab, Tone, action, cabinet, field, fold, segmented, status_line, steps, title_bar,
};
use egui::{Color32, Ui, vec2};

pub const SIZE: egui::Vec2 = egui::vec2(600.0, 720.0);

/// A release in the update list.
#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    /// "launcher.kind_stable" or "launcher.kind_test"
    pub kind: &'static str,
    pub date: String,
    pub this: bool,
    pub newer: bool,
}

#[derive(Clone, Debug)]
pub struct LauncherState {
    pub edition: Edition,
    pub lang: Lang,
    pub online: bool,
    pub address: String,
    /// 0 English, 1 Italian, 2 the original Japanese.
    pub game_text: usize,
    pub set_up: bool,
    pub running: bool,
    pub confirm_stop: bool,
    pub step: (usize, usize),
    /// A message key and its tone.
    pub status: Option<(Tone, &'static str)>,
    pub setup_open: bool,
    pub folder: String,
    pub update_open: bool,
    pub update_status: Option<(Tone, &'static str)>,
    pub releases: Vec<Release>,
    pub output_open: bool,
    pub output: Vec<String>,
}

/// The states the owner reviews.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Ready,
    Running,
    StopConfirm,
    NotSetUp,
    Update,
}

impl LauncherState {
    pub fn sample(edition: Edition, lang: Lang, s: Scenario) -> Self {
        let release = |tag: &str, kind, date: &str, this, newer| Release {
            tag: tag.into(),
            kind,
            date: date.into(),
            this,
            newer,
        };
        let mut st = LauncherState {
            edition,
            lang,
            online: false,
            address: "192.168.1.20".into(),
            game_text: 0,
            set_up: true,
            running: false,
            confirm_stop: false,
            step: (0, 7),
            status: Some((Tone::Done, "launcher.status_ready")),
            setup_open: false,
            folder: r"C:\Games\Forza-WCCF\2010-2011".into(),
            update_open: false,
            update_status: None,
            releases: vec![
                release(
                    "v7.1.0-test.2",
                    "launcher.kind_test",
                    "2026-10-11",
                    false,
                    false,
                ),
                release("v7.0.1", "launcher.kind_stable", "2026-10-09", false, true),
                release("v7.0.0", "launcher.kind_stable", "2026-09-28", true, false),
                release("v6.4.2", "launcher.kind_stable", "2026-08-30", false, false),
            ],
            output_open: false,
            output: [
                "[15:02:11] start: seat 1, this PC",
                "[15:02:11] card table: 16 slots ready",
                "[15:02:12] network: stand-ins on 127.0.0.1",
                "[15:02:14] game: window 1440x900",
                "[15:02:19] club card: session open (KITAKAZE ESTE)",
            ]
            .map(String::from)
            .into(),
        };
        if edition == Edition::E1718 {
            st.folder = r"C:\Games\Forza-WCCF\2017-2018".into();
        }
        match s {
            Scenario::Ready => {}
            Scenario::Running | Scenario::StopConfirm => {
                st.running = true;
                st.online = true;
                st.step = (5, 7);
                st.status = Some((Tone::Notice, "launcher.status_starting"));
                st.output_open = true;
                st.confirm_stop = s == Scenario::StopConfirm;
            }
            Scenario::NotSetUp => {
                st.set_up = false;
                st.setup_open = true;
                st.folder = r"D:\Arcade\disc-image".into();
                st.status = Some((Tone::Notice, "launcher.status_not_set_up"));
            }
            Scenario::Update => {
                st.update_open = true;
                st.update_status = Some((Tone::Refused, "launcher.err_no_answer"));
            }
        }
        st
    }
}

/// The whole launcher window.
pub fn show(ui: &mut Ui, st: &mut LauncherState) {
    let lang = st.lang;
    let accent = st.edition.accent();
    egui::Panel::top("launcher-title")
        .exact_size(44.0)
        .frame(egui::Frame::new())
        .show_separator_line(false)
        .show(ui, |ui| {
            title_bar(ui, st.edition, &mut st.lang);
        });
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(theme::NIGHT)
                .inner_margin(egui::Margin::symmetric(16, 14)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;
                    play(ui, st, lang, accent);
                    let summary = if st.set_up {
                        tr!(lang, "launcher.setup_done")
                    } else {
                        tr!(lang, "launcher.setup_needed")
                    };
                    let mut open = st.setup_open;
                    fold(
                        ui,
                        &tr!(lang, "launcher.setup"),
                        &summary,
                        &mut open,
                        accent,
                        |ui| setup(ui, st, lang),
                    );
                    st.setup_open = open;
                    let mut open = st.update_open;
                    let summary = tr!(lang, "launcher.this_version", v = "7.0.0");
                    fold(
                        ui,
                        &tr!(lang, "launcher.update"),
                        &summary,
                        &mut open,
                        accent,
                        |ui| update(ui, st, lang),
                    );
                    st.update_open = open;
                    let mut open = st.output_open;
                    fold(
                        ui,
                        &tr!(lang, "launcher.output"),
                        &tr!(lang, "launcher.output_hint"),
                        &mut open,
                        accent,
                        |ui| output(ui, st, lang),
                    );
                    st.output_open = open;
                });
        });
    if st.confirm_stop {
        stop_modal(ui, st, lang);
    }
}

fn play(ui: &mut Ui, st: &mut LauncherState, lang: Lang, accent: Color32) {
    widgets::section(ui, &tr!(lang, "launcher.play_section"), accent, |ui| {
        let label_w = 150.0;
        let w = ui.available_width() - label_w;
        ui.add_enabled_ui(!st.running, |ui| {
            ui.horizontal(|ui| {
                field(ui, &tr!(lang, "launcher.where"), label_w);
                let mut i = st.online as usize;
                if segmented(
                    ui,
                    &mut i,
                    &[
                        &tr!(lang, "launcher.this_pc"),
                        &tr!(lang, "launcher.online"),
                    ],
                    w,
                ) {
                    st.online = i == 1;
                }
            });
            if st.online {
                ui.horizontal(|ui| {
                    field(ui, &tr!(lang, "launcher.address"), label_w);
                    ui.add(
                        egui::TextEdit::singleline(&mut st.address)
                            .font(theme::mono(15.0))
                            .desired_width(w)
                            .margin(vec2(10.0, 7.0)),
                    );
                });
            }
            ui.horizontal(|ui| {
                field(ui, &tr!(lang, "launcher.game_text"), label_w);
                segmented(
                    ui,
                    &mut st.game_text,
                    &["English", "Italiano", &tr!(lang, "launcher.original_ja")],
                    w,
                );
            });
            ui.horizontal(|ui| {
                ui.add_space(label_w);
                ui.label(
                    egui::RichText::new(tr!(lang, "launcher.game_text_hint"))
                        .font(theme::body(12.0))
                        .color(theme::TEXT_DIM),
                );
            });
        });
        ui.add_space(4.0);
        let size = vec2(ui.available_width(), 64.0);
        let (label, kind) = if st.running {
            (tr!(lang, "launcher.stop"), Cab::Stop)
        } else {
            (tr!(lang, "launcher.play"), Cab::Go)
        };
        let resp = ui
            .add_enabled_ui(st.set_up, |ui| cabinet(ui, size, &label, kind, true))
            .inner;
        if resp.clicked() && st.running {
            st.confirm_stop = true;
        }
        if st.running {
            steps(ui, st.step.0, st.step.1, ui.available_width());
        }
        if let Some((tone, key)) = st.status {
            status_line(ui, tone, &tr!(lang, key));
        }
    });
}

fn setup(ui: &mut Ui, st: &mut LauncherState, lang: Lang) {
    ui.label(
        egui::RichText::new(tr!(lang, "launcher.folder_hint"))
            .font(theme::body(13.0))
            .color(theme::TEXT_DIM),
    );
    ui.horizontal(|ui| {
        let browse = tr!(lang, "launcher.browse");
        let bw = ui
            .painter()
            .layout_no_wrap(browse.clone(), theme::bold(14.0), theme::TEXT)
            .size()
            .x
            + 40.0;
        ui.add(
            egui::TextEdit::singleline(&mut st.folder)
                .font(theme::mono(14.0))
                .desired_width(ui.available_width() - bw)
                .margin(vec2(10.0, 7.0)),
        );
        action(ui, &browse);
    });
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 8.0) / 2.0;
        cabinet(
            ui,
            vec2(w, 44.0),
            &tr!(lang, "launcher.set_up"),
            Cab::Go,
            !st.set_up,
        );
        ui.add_enabled_ui(st.set_up, |ui| {
            cabinet(
                ui,
                vec2(w, 44.0),
                &tr!(lang, "launcher.undo"),
                Cab::Plain,
                false,
            )
        });
    });
}

fn update(ui: &mut Ui, st: &mut LauncherState, lang: Lang) {
    use egui_extras::{Column, TableBuilder};
    TableBuilder::new(ui)
        .id_salt("releases")
        .striped(true)
        .column(Column::exact(150.0))
        .column(Column::exact(110.0))
        .column(Column::exact(100.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for k in [
                "launcher.col_tag",
                "launcher.col_kind",
                "launcher.col_date",
                "launcher.col_mark",
            ] {
                h.col(|ui| {
                    ui.label(
                        egui::RichText::new(tr!(lang, k))
                            .font(theme::display(14.0))
                            .color(theme::TEXT_FAINT),
                    );
                });
            }
        })
        .body(|mut body| {
            for r in &st.releases {
                body.row(26.0, |mut row| {
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(&r.tag)
                                .font(theme::mono(13.0))
                                .color(theme::TEXT),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(tr!(lang, r.kind))
                                .font(theme::body(13.0))
                                .color(theme::TEXT_DIM),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(&r.date)
                                .font(theme::body(13.0))
                                .color(theme::TEXT_DIM),
                        );
                    });
                    row.col(|ui| {
                        if r.this {
                            widgets::chip(
                                ui,
                                &tr!(lang, "launcher.mark_this"),
                                true,
                                Some(theme::BLUE),
                            );
                        } else if r.newer {
                            widgets::chip(ui, &tr!(lang, "launcher.mark_newer"), true, None);
                        }
                    });
                });
            }
        });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        cabinet(
            ui,
            vec2(200.0, 44.0),
            &tr!(lang, "launcher.update_button"),
            Cab::Go,
            true,
        );
        ui.add_space(4.0);
        action(ui, &tr!(lang, "launcher.whats_new"));
    });
    if let Some((tone, key)) = st.update_status {
        status_line(ui, tone, &tr!(lang, key));
    }
}

fn output(ui: &mut Ui, st: &mut LauncherState, lang: Lang) {
    egui::Frame::new()
        .fill(theme::WELL)
        .corner_radius(theme::RADIUS)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .max_height(120.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for l in &st.output {
                        ui.label(
                            egui::RichText::new(l)
                                .font(theme::mono(12.5))
                                .color(Color32::from_rgb(0xA8, 0xD8, 0xB0)),
                        );
                    }
                });
        });
    ui.horizontal(|ui| {
        action(ui, &tr!(lang, "launcher.copy"));
        action(ui, &tr!(lang, "settings.send_logs"));
    });
}

fn stop_modal(ui: &mut Ui, st: &mut LauncherState, lang: Lang) {
    egui::Modal::new(egui::Id::new("stop-confirm"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(
            egui::Frame::new()
                .fill(theme::DEEP)
                .stroke(egui::Stroke::new(1.0, theme::RED))
                .corner_radius(theme::RADIUS_BIG)
                .inner_margin(20),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(400.0);
            ui.label(
                egui::RichText::new(tr!(lang, "launcher.stop_title"))
                    .font(theme::display(26.0))
                    .color(theme::TEXT),
            );
            ui.label(egui::RichText::new(tr!(lang, "launcher.stop_body")).color(theme::TEXT_DIM));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if cabinet(
                    ui,
                    vec2(196.0, 48.0),
                    &tr!(lang, "launcher.stop"),
                    Cab::Stop,
                    true,
                )
                .clicked()
                {
                    st.confirm_stop = false;
                }
                if cabinet(
                    ui,
                    vec2(196.0, 48.0),
                    &tr!(lang, "common.cancel"),
                    Cab::Plain,
                    false,
                )
                .clicked()
                {
                    st.confirm_stop = false;
                }
            });
        });
}
