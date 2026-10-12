//! The installer: a six-page wizard (680×470, resizable) with a step rail on the left and Back / Next below.

use super::Lang;
use super::theme::{self, Edition};
use super::widgets::{
    self, Cab, Tone, action, cabinet, field, progress, status_line, steps, toggle_row,
};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};

pub const SIZE: egui::Vec2 = egui::vec2(680.0, 470.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Welcome,
    Where,
    Files,
    Options,
    Installing,
    Done,
}

impl Page {
    pub const ALL: [Page; 6] = [
        Page::Welcome,
        Page::Where,
        Page::Files,
        Page::Options,
        Page::Installing,
        Page::Done,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            Page::Welcome => "1_welcome",
            Page::Where => "2_where",
            Page::Files => "3_files",
            Page::Options => "4_options",
            Page::Installing => "5_installing",
            Page::Done => "6_done",
        }
    }

    fn title(self, lang: Lang) -> String {
        match self {
            Page::Welcome => tr!(lang, "installer.welcome"),
            Page::Where => tr!(lang, "installer.where"),
            Page::Files => tr!(lang, "installer.files"),
            Page::Options => tr!(lang, "installer.options"),
            Page::Installing => tr!(lang, "installer.installing"),
            Page::Done => tr!(lang, "installer.done"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct InstallerState {
    pub lang: Lang,
    pub page: Page,
    pub folder: String,
    pub iso: String,
    pub key: String,
    pub package: String,
    pub detected: Option<Edition>,
    /// Check the disc image, set up the game, play in English, desktop shortcuts.
    pub options: [bool; 4],
    pub stage: usize,
    pub fraction: f32,
    pub details_open: bool,
    pub log: Vec<String>,
}

impl InstallerState {
    pub fn sample(lang: Lang, page: Page) -> Self {
        InstallerState {
            lang,
            page,
            folder: r"C:\Games\Forza-WCCF".into(),
            iso: r"D:\Images\disc-2010-11.iso".into(),
            key: r"D:\Images\disc-2010-11.key".into(),
            package: String::new(),
            detected: Some(Edition::E1011),
            options: [true, true, true, false],
            stage: 2,
            fraction: 0.62,
            details_open: true,
            log: [
                "check: disc image 5.42 GB, sha-256 matches its key",
                "decrypt: 2 of 3 partitions",
                "extract: 41 230 / 66 512 files",
            ]
            .map(String::from)
            .into(),
        }
    }
}

pub fn show(ui: &mut Ui, st: &mut InstallerState) {
    let lang = st.lang;
    egui::Panel::left("installer-rail")
        .exact_size(196.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new())
        .show(ui, |ui| rail(ui, st.page, lang));
    egui::Panel::bottom("installer-footer")
        .exact_size(64.0)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::NIGHT)
                .inner_margin(egui::Margin::symmetric(20, 10)),
        )
        .show(ui, |ui| footer(ui, st, lang));
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(theme::NIGHT)
                .inner_margin(egui::Margin {
                    left: 24,
                    right: 20,
                    top: 20,
                    bottom: 0,
                }),
        )
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.label(
                egui::RichText::new(st.page.title(lang))
                    .font(theme::display(30.0))
                    .color(theme::TEXT),
            );
            match st.page {
                Page::Welcome => welcome(ui, lang),
                Page::Where => where_(ui, st, lang),
                Page::Files => files(ui, st, lang),
                Page::Options => options(ui, st, lang),
                Page::Installing => installing(ui, st, lang),
                Page::Done => done(ui, st, lang),
            }
        });
}

fn rail(ui: &mut Ui, page: Page, lang: Lang) {
    let rect = ui.max_rect();
    let p = ui.painter();
    widgets::gradient(p, rect, 0, theme::DEEP, theme::WELL, true);
    p.line_segment(
        [rect.right_top(), rect.right_bottom()],
        Stroke::new(1.0, theme::RIM),
    );
    widgets::mark(p, pos2(rect.left() + 30.0, rect.top() + 34.0), theme::AMBER);
    p.text(
        pos2(rect.left() + 50.0, rect.top() + 26.0),
        Align2::LEFT_CENTER,
        "FORZA-WCCF",
        theme::display(20.0),
        theme::TEXT,
    );
    let sub = widgets::fit(
        p,
        &tr!(lang, "installer.app"),
        theme::display(14.0),
        theme::TEXT_DIM,
        rect.width() - 60.0,
    );
    p.galley(
        pos2(rect.left() + 50.0, rect.top() + 36.0),
        sub,
        theme::TEXT_DIM,
    );
    let current = Page::ALL.iter().position(|x| *x == page).unwrap_or(0);
    let top = rect.top() + 84.0;
    let gap = 46.0;
    // the line that joins the steps
    let x = rect.left() + 30.0;
    p.line_segment(
        [pos2(x, top), pos2(x, top + gap * 5.0)],
        Stroke::new(2.0, theme::RIM),
    );
    p.line_segment(
        [pos2(x, top), pos2(x, top + gap * current as f32)],
        Stroke::new(2.0, theme::GREEN),
    );
    for (i, pg) in Page::ALL.iter().enumerate() {
        let c = pos2(x, top + gap * i as f32);
        let label = pg.title(lang);
        let (fill, ink, text) = match i.cmp(&current) {
            std::cmp::Ordering::Less => (theme::GREEN, theme::NIGHT, theme::TEXT_DIM),
            std::cmp::Ordering::Equal => (theme::AMBER, theme::INK, theme::TEXT),
            std::cmp::Ordering::Greater => (theme::DEEP, theme::TEXT_FAINT, theme::TEXT_FAINT),
        };
        if i == current {
            let s = egui::Shadow {
                offset: [0, 0],
                blur: 12,
                spread: 1,
                color: theme::AMBER.gamma_multiply(0.5),
            };
            p.add(s.as_shape(Rect::from_center_size(c, vec2(26.0, 26.0)), 13));
        }
        p.circle_filled(c, 13.0, fill);
        p.circle_stroke(
            c,
            13.0,
            Stroke::new(1.0, if i > current { theme::RIM } else { fill }),
        );
        if i < current {
            widgets::tone_icon(p, c, Tone::Done);
        } else {
            p.text(
                c + vec2(0.0, 1.0),
                Align2::CENTER_CENTER,
                (i + 1).to_string(),
                theme::display(17.0),
                ink,
            );
        }
        let font = if i == current {
            theme::bold(14.0)
        } else {
            theme::body(14.0)
        };
        let g = widgets::fit(p, &label, font, text, rect.right() - x - 30.0);
        p.galley(pos2(x + 22.0, c.y - g.size().y / 2.0), g, text);
    }
}

fn footer(ui: &mut Ui, st: &mut InstallerState, lang: Lang) {
    let r = ui.max_rect();
    ui.painter().line_segment(
        [
            pos2(r.left() - 20.0, r.top() - 10.0),
            pos2(r.right() + 20.0, r.top() - 10.0),
        ],
        Stroke::new(1.0, theme::RIM),
    );
    ui.horizontal_centered(|ui| {
        let first = st.page == Page::Welcome;
        let busy = st.page == Page::Installing;
        if st.page != Page::Done {
            ui.add_enabled_ui(!first && !busy, |ui| {
                action(ui, &tr!(lang, "installer.back"))
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (label, kind) = match st.page {
                Page::Options => (tr!(lang, "installer.install"), Cab::Go),
                Page::Installing => (tr!(lang, "common.cancel"), Cab::Plain),
                Page::Done => (tr!(lang, "common.close"), Cab::Go),
                _ => (tr!(lang, "installer.next"), Cab::Go),
            };
            cabinet(ui, vec2(150.0, 44.0), &label, kind, kind == Cab::Go);
        });
    });
}

/// A painted bullet line: a small amber block, the text, and a dim note.
fn bullet(ui: &mut Ui, text: &str, note: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(14.0, 20.0), Sense::hover());
        ui.painter().rect_filled(
            Rect::from_center_size(r.center(), vec2(8.0, 8.0)),
            2,
            theme::AMBER,
        );
        ui.label(
            egui::RichText::new(text)
                .font(theme::bold(14.0))
                .color(theme::TEXT),
        );
        ui.label(
            egui::RichText::new(note)
                .font(theme::body(13.0))
                .color(theme::TEXT_DIM),
        );
    });
}

fn welcome(ui: &mut Ui, lang: Lang) {
    ui.label(egui::RichText::new(tr!(lang, "installer.welcome_body")).color(theme::TEXT_DIM));
    ui.add_space(4.0);
    widgets::caps(ui, &tr!(lang, "installer.you_need"));
    bullet(
        ui,
        &tr!(lang, "installer.need_iso"),
        &tr!(lang, "installer.need_iso_note"),
    );
    bullet(ui, &tr!(lang, "installer.need_key"), "");
    bullet(
        ui,
        &tr!(lang, "installer.need_package"),
        &tr!(lang, "installer.optional"),
    );
    bullet(ui, &tr!(lang, "installer.need_space"), "");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        widgets::badge(ui, Edition::E1011);
        widgets::badge(ui, Edition::E1718);
        ui.label(
            egui::RichText::new(tr!(lang, "installer.side_by_side"))
                .font(theme::body(13.0))
                .color(theme::TEXT_DIM),
        );
    });
}

fn path_row(ui: &mut Ui, label: &str, value: &mut String, lang: Lang) {
    ui.horizontal(|ui| {
        field(ui, label, 120.0);
        let browse = tr!(lang, "launcher.browse");
        let bw = ui
            .painter()
            .layout_no_wrap(browse.clone(), theme::bold(14.0), theme::TEXT)
            .size()
            .x
            + 40.0;
        ui.add(
            egui::TextEdit::singleline(value)
                .font(theme::mono(13.0))
                .desired_width(ui.available_width() - bw)
                .margin(vec2(8.0, 7.0)),
        );
        action(ui, &browse);
    });
}

fn where_(ui: &mut Ui, st: &mut InstallerState, lang: Lang) {
    path_row(ui, &tr!(lang, "installer.folder"), &mut st.folder, lang);
    ui.horizontal(|ui| {
        field(ui, &tr!(lang, "installer.free"), 120.0);
        let w = ui.available_width();
        let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        let p = ui.painter();
        let bar = Rect::from_min_size(r.min + vec2(0.0, 8.0), vec2(w - 120.0, 14.0));
        p.rect_filled(bar, 3, theme::WELL);
        p.rect_filled(
            Rect::from_min_size(bar.min, vec2(bar.width() * 0.71, bar.height())),
            3,
            theme::RIM_HI,
        );
        // the 6 GB the install takes, on top of what is used
        p.rect_filled(
            Rect::from_min_size(
                bar.min + vec2(bar.width() * 0.71, 0.0),
                vec2(bar.width() * 0.05, bar.height()),
            ),
            0,
            theme::AMBER,
        );
        p.text(
            r.right_center(),
            Align2::RIGHT_CENTER,
            tr!(lang, "installer.free_of", free = "41.2", drive = "C:"),
            theme::display(16.0),
            theme::TEXT,
        );
    });
    ui.add_space(4.0);
    widgets::caps(ui, &tr!(lang, "installer.layout"));
    egui::Frame::new()
        .fill(theme::WELL)
        .corner_radius(theme::RADIUS)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mono =
                |t: &str, c: Color32| egui::RichText::new(t).font(theme::mono(13.0)).color(c);
            ui.label(mono(r"C:\Games\Forza-WCCF\", theme::TEXT));
            ui.horizontal(|ui| {
                ui.label(mono("  ├─ 2010-2011", Edition::E1011.accent()));
                ui.label(
                    egui::RichText::new(tr!(lang, "installer.this_install"))
                        .font(theme::body(12.0))
                        .color(theme::TEXT_DIM),
                );
            });
            ui.label(mono("  └─ 2017-2018", Edition::E1718.accent()));
        });
    status_line(ui, Tone::Notice, &tr!(lang, "installer.space_ok"));
}

fn files(ui: &mut Ui, st: &mut InstallerState, lang: Lang) {
    path_row(ui, &tr!(lang, "installer.iso"), &mut st.iso, lang);
    path_row(ui, &tr!(lang, "installer.key"), &mut st.key, lang);
    path_row(ui, &tr!(lang, "installer.package"), &mut st.package, lang);
    ui.horizontal(|ui| {
        field(ui, &tr!(lang, "installer.detected"), 120.0);
        if let Some(e) = st.detected {
            widgets::badge(ui, e);
        }
    });
    status_line(ui, Tone::Done, &tr!(lang, "installer.iso_ok"));
    ui.label(
        egui::RichText::new(tr!(lang, "installer.reuse_hint"))
            .font(theme::body(12.0))
            .color(theme::TEXT_DIM),
    );
}

fn options(ui: &mut Ui, st: &mut InstallerState, lang: Lang) {
    let rows = [
        ("installer.opt_check", "installer.opt_check_hint"),
        ("installer.opt_setup", "installer.opt_setup_hint"),
        ("installer.opt_english", "installer.opt_english_hint"),
        ("installer.opt_shortcuts", "installer.opt_shortcuts_hint"),
    ];
    ui.spacing_mut().item_spacing.y = 2.0;
    for (i, (t, h)) in rows.iter().enumerate() {
        toggle_row(ui, &mut st.options[i], &tr!(lang, *t), &tr!(lang, *h));
    }
}

fn installing(ui: &mut Ui, st: &mut InstallerState, lang: Lang) {
    let w = ui.available_width();
    ui.label(
        egui::RichText::new(tr!(lang, "installer.stage_extract"))
            .font(theme::bold(16.0))
            .color(theme::TEXT),
    );
    progress(ui, st.fraction, w);
    steps(ui, st.stage, 5, w);
    let mut open = st.details_open;
    widgets::fold(
        ui,
        &tr!(lang, "installer.details"),
        "",
        &mut open,
        theme::AMBER,
        |ui| {
            egui::Frame::new()
                .fill(theme::WELL)
                .corner_radius(theme::RADIUS)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for l in &st.log {
                        ui.label(
                            egui::RichText::new(l)
                                .font(theme::mono(12.0))
                                .color(Color32::from_rgb(0xA8, 0xD8, 0xB0)),
                        );
                    }
                });
        },
    );
    st.details_open = open;
}

fn done(ui: &mut Ui, st: &mut InstallerState, lang: Lang) {
    status_line(ui, Tone::Done, &tr!(lang, "installer.done_body"));
    ui.horizontal(|ui| {
        field(ui, &tr!(lang, "installer.folder"), 120.0);
        ui.label(
            egui::RichText::new(format!(r"{}\2010-2011", st.folder))
                .font(theme::mono(13.0))
                .color(theme::TEXT),
        );
    });
    let rows = [
        (true, "installer.did_extract"),
        (true, "installer.did_setup"),
        (true, "installer.did_english"),
        (false, "installer.not_shortcuts"),
    ];
    for (ok, k) in rows {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
            if ok {
                widgets::tone_icon(ui.painter(), r.center(), Tone::Done);
            } else {
                ui.painter().line_segment(
                    [r.center() - vec2(6.0, 0.0), r.center() + vec2(6.0, 0.0)],
                    Stroke::new(2.0, theme::TEXT_FAINT),
                );
            }
            ui.label(egui::RichText::new(tr!(lang, k)).color(if ok {
                theme::TEXT
            } else {
                theme::TEXT_DIM
            }));
        });
    }
    ui.add_space(4.0);
    action(ui, &tr!(lang, "installer.open_folder"));
}
