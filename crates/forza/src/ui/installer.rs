//! The installer: a six-page wizard (680×470, resizable) with a step rail on the left and Back / Next below.
//!
//! [`show`] draws the state it is given and returns what the person asked for as an [`Action`]; the program owns the
//! logic (pickers, checks, the install thread) and writes the results back into [`InstallerState`]. The gallery and
//! the snapshots run it over [`InstallerState::sample`] and ignore the actions. The window has no system frame: the
//! rail and the top strip move it, and the corner buttons minimize and close.

use super::Lang;
use super::theme::{self, Edition};
use super::widgets::{
    self, Cab, TitleAction, Tone, action, cabinet, chip, field, progress, status_line, steps,
    toggle_row,
};
use egui::{Align2, Color32, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

pub const SIZE: egui::Vec2 = egui::vec2(680.0, 470.0);

/// What the install needs on disk, for the free-space bar and its message.
pub const NEEDED_BYTES: u64 = 6_000_000_000;

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

/// The stages of an install, in the order they run; a run lists the ones it will do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Download,
    Verify,
    Extract,
    Setup,
    English,
}

impl Step {
    fn key(self) -> &'static str {
        match self {
            Step::Download => "installer.stage_download",
            Step::Verify => "installer.stage_verify",
            Step::Extract => "installer.stage_extract",
            Step::Setup => "installer.stage_setup",
            Step::English => "installer.stage_english",
        }
    }

    /// The stage's name in `lang`.
    pub fn title(self, lang: Lang) -> String {
        super::text(lang, self.key(), &[])
    }
}

/// The text boxes that have a Browse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Folder,
    Iso,
    Key,
    Package,
}

/// What the lookup of the newest downloadable package found (2017–18 only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Latest {
    Idle,
    Checking,
    /// The version found, e.g. "0.2.0".
    Found(String),
    Failed,
}

/// What a click asked the program to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Back,
    Next,
    Install,
    Cancel,
    Close,
    Minimize,
    /// The person grabbed the window's own title area.
    Drag,
    Browse(Field),
    OpenFolder,
}

/// Disk space of the install folder's drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Space {
    pub free: u64,
    pub total: u64,
    /// "C:", or empty when unknown.
    pub drive: String,
}

#[derive(Clone, Debug)]
pub struct InstallerState {
    pub lang: Lang,
    pub page: Page,
    pub folder: String,
    pub iso: String,
    pub key: String,
    pub package: String,
    pub latest: Latest,
    /// Use the newest downloaded package; a package file in `package` overrides it.
    pub use_latest: bool,
    /// The edition the disc image is, by its size.
    pub detected: Option<Edition>,
    /// The disc image has a size no supported edition has.
    pub unsupported: bool,
    /// Editions already installed in the folder.
    pub installed: Vec<Edition>,
    /// Use the installed `edition` instead of a disc image.
    pub reuse: bool,
    pub edition: Edition,
    /// Check the disc image, set up the game, play in English, desktop shortcuts.
    pub options: [bool; 4],
    /// Setting up needs a package and the 2010–11 edition.
    pub setup_possible: bool,
    pub space: Option<Space>,
    /// The folder name has letters outside ASCII, which the game handles badly.
    pub ascii_tip: bool,
    /// A refusal for the page the person is on, already in their language.
    pub error: Option<String>,
    /// The stages this run will do, and the one it is in.
    pub steps: Vec<Step>,
    pub stage: usize,
    pub running: bool,
    pub fraction: f32,
    /// Bytes done and total of the stage, when it counts bytes.
    pub bytes: Option<(u64, u64)>,
    pub details_open: bool,
    pub log: Vec<String>,
    /// The finished install's folder, and what was done: extract, set up, English, shortcuts.
    pub done_path: String,
    pub did: [bool; 4],
}

impl InstallerState {
    /// A fresh wizard on its first page.
    pub fn new(lang: Lang, folder: String) -> Self {
        InstallerState {
            lang,
            page: Page::Welcome,
            folder,
            iso: String::new(),
            key: String::new(),
            package: String::new(),
            latest: Latest::Idle,
            use_latest: false,
            detected: None,
            unsupported: false,
            installed: vec![],
            reuse: false,
            edition: Edition::E1011,
            options: [true, true, false, true],
            setup_possible: false,
            space: None,
            ascii_tip: false,
            error: None,
            steps: vec![],
            stage: 0,
            running: false,
            fraction: 0.0,
            bytes: None,
            details_open: false,
            log: vec![],
            done_path: String::new(),
            did: [false; 4],
        }
    }

    pub fn sample(lang: Lang, page: Page) -> Self {
        InstallerState {
            iso: r"D:\Images\disc-2010-11.iso".into(),
            key: r"D:\Images\disc-2010-11.key".into(),
            detected: Some(Edition::E1011),
            installed: vec![Edition::E1718],
            options: [true, true, true, false],
            setup_possible: true,
            space: Some(Space {
                free: 41_200_000_000,
                total: 143_000_000_000,
                drive: "C:".into(),
            }),
            steps: vec![Step::Verify, Step::Extract, Step::Setup, Step::English],
            stage: 1,
            running: true,
            fraction: 0.62,
            bytes: Some((3_360_000_000, 5_420_000_000)),
            details_open: true,
            log: [
                "check: disc image 5.42 GB, sha-256 matches its key",
                "decrypt: 2 of 3 partitions",
                "extract: 41 230 / 66 512 files",
            ]
            .map(String::from)
            .into(),
            done_path: r"C:\Games\Forza-WCCF\2010-2011".into(),
            did: [true, true, true, false],
            package: r"D:\Images\WCCF-2010-11-kit.zip".into(),
            ..InstallerState::new(lang, r"C:\Games\Forza-WCCF".into())
        }
        .on(page)
    }

    fn on(mut self, page: Page) -> Self {
        self.page = page;
        self
    }

    /// The edition this install is about: the one reused, else the one the disc image is.
    pub fn target(&self) -> Option<Edition> {
        if self.reuse {
            Some(self.edition)
        } else {
            self.detected
        }
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e9)
}

/// The whole installer window. Returns the first action asked for this frame.
pub fn show(ui: &mut Ui, st: &mut InstallerState) -> Option<Action> {
    let lang = st.lang;
    let win = ui.max_rect();
    let mut out = None;
    egui::Panel::left("installer-rail")
        .exact_size(196.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new())
        .show(ui, |ui| {
            drag_area(ui, ui.max_rect(), "installer-drag-rail", &mut out);
            rail(ui, st.page, lang)
        });
    egui::Panel::bottom("installer-footer")
        .exact_size(64.0)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::NIGHT)
                .inner_margin(egui::Margin::symmetric(20, 10)),
        )
        .show(ui, |ui| footer(ui, st, lang, &mut out));
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
            let strip = Rect::from_min_max(
                pos2(ui.max_rect().left(), win.top()),
                pos2(win.right(), win.top() + 14.0),
            );
            drag_area(ui, strip, "installer-drag-top", &mut out);
            window_corner(ui, win, &mut out);
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.label(
                egui::RichText::new(st.page.title(lang))
                    .font(theme::display(30.0))
                    .color(theme::TEXT),
            );
            match st.page {
                Page::Welcome => welcome(ui, lang),
                Page::Where => where_(ui, st, lang, &mut out),
                Page::Files => files(ui, st, lang, &mut out),
                Page::Options => options(ui, st, lang),
                Page::Installing => installing(ui, st, lang),
                Page::Done => done(ui, st, lang, &mut out),
            }
            if st.page != Page::Installing
                && let Some(e) = &st.error
            {
                status_line(ui, Tone::Refused, e);
            }
        });
    out
}

/// Grabbing this area moves the window (the program starts the drag).
fn drag_area(ui: &mut Ui, rect: Rect, id: &'static str, out: &mut Option<Action>) {
    let r = ui.interact(rect, Id::new(id), Sense::click_and_drag());
    if r.drag_started_by(egui::PointerButton::Primary) {
        out.get_or_insert(Action::Drag);
    }
}

/// Minimize and close in the window's top right corner.
fn window_corner(ui: &mut Ui, win: Rect, out: &mut Option<Action>) {
    let r = Rect::from_min_size(
        pos2(win.right() - 4.0 - 82.0, win.top() + 4.0),
        vec2(82.0, 32.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(r)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    match widgets::window_buttons(&mut child) {
        TitleAction::Minimize => _ = out.get_or_insert(Action::Minimize),
        TitleAction::Close => _ = out.get_or_insert(Action::Close),
        TitleAction::None => {}
    }
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

fn footer(ui: &mut Ui, st: &mut InstallerState, lang: Lang, out: &mut Option<Action>) {
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
        let busy = st.page == Page::Installing && st.running;
        if st.page != Page::Done
            && ui
                .add_enabled_ui(!first && !busy, |ui| {
                    action(ui, &tr!(lang, "installer.back"))
                })
                .inner
                .clicked()
        {
            out.get_or_insert(Action::Back);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (label, kind, act) = match st.page {
                Page::Options => (tr!(lang, "installer.install"), Cab::Go, Action::Install),
                Page::Installing if busy => {
                    (tr!(lang, "common.cancel"), Cab::Plain, Action::Cancel)
                }
                Page::Installing => (tr!(lang, "common.close"), Cab::Plain, Action::Close),
                Page::Done => (tr!(lang, "common.close"), Cab::Go, Action::Close),
                _ => (tr!(lang, "installer.next"), Cab::Go, Action::Next),
            };
            if cabinet(ui, vec2(150.0, 44.0), &label, kind, kind == Cab::Go).clicked() {
                out.get_or_insert(act);
            }
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
    ui.spacing_mut().item_spacing.y = 6.0;
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
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(tr!(lang, "installer.welcome_note"))
            .font(theme::body(12.0))
            .color(theme::TEXT_DIM),
    );
}

/// A label, a text box and a Browse button.
fn path_row(
    ui: &mut Ui,
    label: &str,
    value: &mut String,
    lang: Lang,
    which: Field,
    out: &mut Option<Action>,
) {
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
        if action(ui, &browse).clicked() {
            out.get_or_insert(Action::Browse(which));
        }
    });
}

fn edition_name(e: Edition) -> &'static str {
    match e {
        Edition::E1011 => "2010-2011",
        Edition::E1718 => "2017-2018",
    }
}

fn where_(ui: &mut Ui, st: &mut InstallerState, lang: Lang, out: &mut Option<Action>) {
    path_row(
        ui,
        &tr!(lang, "installer.folder"),
        &mut st.folder,
        lang,
        Field::Folder,
        out,
    );
    if let Some(sp) = &st.space {
        ui.horizontal(|ui| {
            field(ui, &tr!(lang, "installer.free"), 120.0);
            let w = ui.available_width();
            let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
            let p = ui.painter();
            let bar = Rect::from_min_size(r.min + vec2(0.0, 8.0), vec2(w - 120.0, 14.0));
            let total = sp.total.max(1) as f32;
            let used = ((sp.total.saturating_sub(sp.free)) as f32 / total).clamp(0.0, 1.0);
            let need = (NEEDED_BYTES as f32 / total).min(1.0 - used);
            p.rect_filled(bar, 3, theme::WELL);
            p.rect_filled(
                Rect::from_min_size(bar.min, vec2(bar.width() * used, bar.height())),
                3,
                theme::RIM_HI,
            );
            // the 6 GB the install takes, on top of what is used
            p.rect_filled(
                Rect::from_min_size(
                    bar.min + vec2(bar.width() * used, 0.0),
                    vec2(bar.width() * need, bar.height()),
                ),
                0,
                theme::AMBER,
            );
            p.text(
                r.right_center(),
                Align2::RIGHT_CENTER,
                tr!(
                    lang,
                    "installer.free_of",
                    free = gb(sp.free),
                    drive = sp.drive
                ),
                theme::display(16.0),
                theme::TEXT,
            );
        });
    }
    ui.add_space(4.0);
    widgets::caps(ui, &tr!(lang, "installer.layout"));
    let target = st.target();
    egui::Frame::new()
        .fill(theme::WELL)
        .corner_radius(theme::RADIUS)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mono =
                |t: &str, c: Color32| egui::RichText::new(t).font(theme::mono(13.0)).color(c);
            ui.label(mono(
                &format!(r"{}\", st.folder.trim().trim_end_matches('\\')),
                theme::TEXT,
            ));
            for (i, e) in [Edition::E1011, Edition::E1718].into_iter().enumerate() {
                ui.horizontal(|ui| {
                    let branch = if i == 0 { "  ├─ " } else { "  └─ " };
                    ui.label(mono(&format!("{branch}{}", edition_name(e)), e.accent()));
                    let note = if st.installed.contains(&e) {
                        tr!(lang, "installer.installed_here")
                    } else if target == Some(e) {
                        tr!(lang, "installer.this_install")
                    } else {
                        String::new()
                    };
                    ui.label(
                        egui::RichText::new(note)
                            .font(theme::body(12.0))
                            .color(theme::TEXT_DIM),
                    );
                });
            }
        });
    match &st.space {
        Some(sp) if sp.free < NEEDED_BYTES => status_line(
            ui,
            Tone::Refused,
            &tr!(lang, "installer.space_low", free = gb(sp.free)),
        ),
        Some(_) => status_line(ui, Tone::Notice, &tr!(lang, "installer.space_ok")),
        None => status_line(ui, Tone::Notice, &tr!(lang, "installer.space_unknown")),
    };
    if st.ascii_tip {
        status_line(ui, Tone::Notice, &tr!(lang, "installer.ascii_tip"));
    }
}

fn files(ui: &mut Ui, st: &mut InstallerState, lang: Lang, out: &mut Option<Action>) {
    if !st.installed.is_empty() {
        toggle_row(
            ui,
            &mut st.reuse,
            &tr!(lang, "installer.reuse"),
            &tr!(lang, "installer.reuse_hint"),
        );
        if st.reuse && st.installed.len() > 1 {
            ui.horizontal(|ui| {
                field(ui, &tr!(lang, "installer.detected"), 120.0);
                for e in st.installed.clone() {
                    if chip(ui, e.label(), st.edition == e, Some(e.accent())).clicked() {
                        st.edition = e;
                    }
                }
            });
        }
    }
    let disc = !st.reuse;
    ui.add_enabled_ui(disc, |ui| {
        path_row(
            ui,
            &tr!(lang, "installer.iso"),
            &mut st.iso,
            lang,
            Field::Iso,
            out,
        );
        path_row(
            ui,
            &tr!(lang, "installer.key"),
            &mut st.key,
            lang,
            Field::Key,
            out,
        );
    });
    // 2017-18: the newest published package, or a file; 2010-11 has no published package, so only the file
    let newest = match (&st.latest, st.target()) {
        (Latest::Found(v), Some(Edition::E1718)) => Some(v.clone()),
        _ => None,
    };
    let mut from_file = true;
    if let Some(v) = &newest {
        let mut pick = usize::from(!(st.use_latest && st.package.trim().is_empty()));
        ui.horizontal(|ui| {
            field(ui, &tr!(lang, "installer.package"), 120.0);
            let labels = [
                tr!(lang, "installer.pkg_newest", version = v),
                tr!(lang, "installer.pkg_file"),
            ];
            let labels = [labels[0].as_str(), labels[1].as_str()];
            let width = ui.available_width();
            if widgets::segmented(ui, &mut pick, &labels, width) {
                st.use_latest = pick == 0;
                if st.use_latest {
                    st.package.clear();
                }
            }
        });
        from_file = pick == 1;
        if !from_file {
            status_line(
                ui,
                Tone::Done,
                &tr!(lang, "installer.pkg_newest_note", version = v),
            );
        }
    }
    if from_file {
        path_row(
            ui,
            &tr!(
                lang,
                if newest.is_some() {
                    "installer.pkg_file_row"
                } else {
                    "installer.package"
                }
            ),
            &mut st.package,
            lang,
            Field::Package,
            out,
        );
    }
    if st.target() == Some(Edition::E1718) {
        match st.latest {
            Latest::Checking => {
                status_line(ui, Tone::Notice, &tr!(lang, "installer.latest_checking"));
            }
            Latest::Failed => {
                status_line(ui, Tone::Notice, &tr!(lang, "installer.latest_failed"));
            }
            _ => {}
        }
    }
    ui.horizontal(|ui| {
        field(ui, &tr!(lang, "installer.detected"), 120.0);
        if let Some(e) = st.target() {
            widgets::badge(ui, e);
        }
    });
    if st.reuse {
        status_line(
            ui,
            Tone::Done,
            &tr!(
                lang,
                "installer.reusing",
                edition = edition_name(st.edition)
            ),
        );
    } else if let Some(e) = st.detected {
        status_line(
            ui,
            Tone::Done,
            &tr!(lang, "installer.iso_ok", edition = e.label()),
        );
    } else if st.unsupported {
        status_line(ui, Tone::Refused, &tr!(lang, "installer.iso_unsupported"));
    }
    ui.label(
        egui::RichText::new(tr!(lang, "installer.drop_hint"))
            .font(theme::body(12.0))
            .color(theme::TEXT_DIM),
    );
}

fn options(ui: &mut Ui, st: &mut InstallerState, lang: Lang) {
    ui.spacing_mut().item_spacing.y = 2.0;
    let (can, on) = (st.setup_possible, st.setup_possible && st.options[1]);
    let setup_hint = if can {
        tr!(lang, "installer.opt_setup_hint")
    } else {
        tr!(lang, "installer.opt_setup_na")
    };
    let rows = [
        (
            true,
            "installer.opt_check",
            tr!(lang, "installer.opt_check_hint"),
        ),
        (can, "installer.opt_setup", setup_hint),
        (
            on,
            "installer.opt_english",
            tr!(lang, "installer.opt_english_hint"),
        ),
        (
            on,
            "installer.opt_shortcuts",
            tr!(lang, "installer.opt_shortcuts_hint"),
        ),
    ];
    for (i, (enabled, t, h)) in rows.into_iter().enumerate() {
        // a row that cannot apply shows off and keeps the chosen value for when it can
        let mut v = enabled && st.options[i];
        ui.add_enabled_ui(enabled, |ui| toggle_row(ui, &mut v, &tr!(lang, t), &h));
        if enabled {
            st.options[i] = v;
        }
    }
}

fn installing(ui: &mut Ui, st: &mut InstallerState, lang: Lang) {
    let w = ui.available_width();
    let name = match st.steps.get(st.stage) {
        Some(s) => s.title(lang),
        None => tr!(lang, "installer.stage_start"),
    };
    ui.label(
        egui::RichText::new(name)
            .font(theme::bold(16.0))
            .color(theme::TEXT),
    );
    progress(ui, st.fraction, w);
    if let Some((d, t)) = st.bytes {
        ui.label(
            egui::RichText::new(tr!(lang, "installer.bytes_of", done = gb(d), total = gb(t)))
                .font(theme::body(12.0))
                .color(theme::TEXT_DIM),
        );
    }
    steps(ui, st.stage, st.steps.len().max(1), w);
    if !st.running
        && let Some(e) = &st.error
    {
        status_line(ui, Tone::Refused, e);
    }
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
                    egui::ScrollArea::vertical()
                        .max_height(96.0)
                        .auto_shrink([false, true])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for l in &st.log {
                                ui.label(
                                    egui::RichText::new(l)
                                        .font(theme::mono(12.0))
                                        .color(Color32::from_rgb(0xA8, 0xD8, 0xB0)),
                                );
                            }
                        });
                });
        },
    );
    st.details_open = open;
}

fn done(ui: &mut Ui, st: &mut InstallerState, lang: Lang, out: &mut Option<Action>) {
    // the extraction-only page has one more note; a short window scrolls instead of hiding the button
    egui::ScrollArea::vertical().show(ui, |ui| done_body(ui, st, lang, out));
}

fn done_body(ui: &mut Ui, st: &mut InstallerState, lang: Lang, out: &mut Option<Action>) {
    let body = if st.did[1] {
        "installer.done_body"
    } else {
        "installer.done_body_extract"
    };
    ui.spacing_mut().item_spacing.y = 3.0;
    status_line(ui, Tone::Done, &tr!(lang, body));
    ui.horizontal(|ui| {
        field(ui, &tr!(lang, "installer.folder"), 120.0);
        ui.label(
            egui::RichText::new(&st.done_path)
                .font(theme::mono(13.0))
                .color(theme::TEXT),
        );
    });
    let rows = [
        (st.did[0], "installer.did_extract", "installer.did_extract"),
        (st.did[1], "installer.did_setup", "installer.not_setup"),
        (st.did[2], "installer.did_english", "installer.not_english"),
        (
            st.did[3],
            "installer.did_shortcuts",
            "installer.not_shortcuts",
        ),
    ];
    for (ok, yes, no) in rows {
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
            ui.label(
                egui::RichText::new(tr!(lang, if ok { yes } else { no })).color(if ok {
                    theme::TEXT
                } else {
                    theme::TEXT_DIM
                }),
            );
        });
    }
    if !st.did[1] {
        status_line(ui, Tone::Notice, &tr!(lang, "installer.extract_only_note"));
    }
    ui.add_space(-2.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    for key in ["installer.done_guard", "installer.done_dx"] {
        ui.label(
            egui::RichText::new(tr!(lang, key))
                .size(12.0)
                .color(theme::TEXT_DIM),
        );
    }
    ui.spacing_mut().item_spacing.y = 8.0;
    ui.add_space(4.0);
    if action(ui, &tr!(lang, "installer.open_folder")).clicked() {
        out.get_or_insert(Action::OpenFolder);
    }
}
