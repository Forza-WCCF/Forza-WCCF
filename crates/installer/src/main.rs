#![cfg_attr(windows, windows_subsystem = "windows")]
//! Forza-WCCF installer: each RingEdge edition owns <dest>\<edition>\game and its kit/save tree.
//! The 2010–2011 kit is optional; 2017–2018 is extraction-only. `--iso/--key/--dest` runs headless.
mod disc;
mod install;

use eframe::egui::{self, RichText};
use install::{Event, Plan};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Welcome,
    Where,
    Files,
    Options,
    Install,
    Done,
}

enum Msg {
    Ev(Event),
    Done(Result<(), String>),
}

struct Run {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    stage: String,
    progress: (u64, u64),
    log: Vec<String>,
    result: Option<Result<(), String>>,
}

struct App {
    page: Page,
    dest: String,
    iso: String,
    key: String,
    reuse: bool,
    verify: bool,
    setup: bool,
    english: bool,
    shortcuts: bool,
    kit: String,
    error: Option<String>,
    run: Option<Run>,
    setup_done: bool,
    game_id: &'static str,
}

fn default_dest() -> String {
    if cfg!(windows) {
        format!(
            "{}\\Games\\Forza-WCCF",
            std::env::var("SystemDrive").unwrap_or("C:".into())
        )
    } else {
        "Forza-WCCF".into()
    }
}

/// Paths pasted from Explorer's "Copy as path" come with quotes.
fn clean(s: &str) -> PathBuf {
    PathBuf::from(s.trim().trim_matches('"'))
}

fn gb(b: u64) -> String {
    if b >= 1_000_000_000 {
        format!("{:.1} GB", b as f64 / 1e9)
    } else {
        format!("{:.0} MB", b as f64 / 1e6)
    }
}

impl App {
    fn new() -> Self {
        App {
            page: Page::Welcome,
            dest: default_dest(),
            iso: String::new(),
            key: String::new(),
            reuse: false,
            verify: true,
            setup: true,
            english: false,
            shortcuts: true,
            kit: String::new(),
            error: None,
            run: None,
            setup_done: false,
            game_id: "SBWG",
        }
    }

    fn dest(&self) -> PathBuf {
        std::path::absolute(clean(&self.dest)).unwrap_or_else(|_| clean(&self.dest))
    }

    fn file_row(
        ui: &mut egui::Ui,
        label: &str,
        text: &mut String,
        enabled: bool,
        pick: impl FnOnce() -> Option<PathBuf>,
    ) {
        ui.label(RichText::new(label).strong());
        ui.horizontal(|ui| {
            ui.add_enabled_ui(enabled, |ui| {
                ui.add(egui::TextEdit::singleline(text).desired_width(ui.available_width() - 90.0));
                if ui.button("Browse…").clicked()
                    && let Some(p) = pick()
                {
                    *text = p.display().to_string();
                }
            });
        });
    }

    fn kit(&self) -> Option<PathBuf> {
        let path = clean(&self.kit);
        (!path.as_os_str().is_empty()).then_some(path)
    }

    fn selected_game_id(&self) -> Option<&'static str> {
        if self.reuse && install::game_present(&self.dest(), self.game_id) {
            Some(self.game_id)
        } else {
            std::fs::metadata(clean(&self.iso))
                .ok()
                .and_then(|m| disc::by_size(m.len()))
                .map(|d| d.game_id)
        }
    }

    fn setup_enabled(&self) -> bool {
        self.setup && self.kit().is_some() && self.selected_game_id() == Some("SBWG")
    }

    /// Validate the selected disc/key and, only when used, the optional kit before copying.
    fn check_files(&self) -> Result<(), String> {
        if !(self.reuse && install::game_present(&self.dest(), self.game_id)) {
            let (iso, key) = (clean(&self.iso), clean(&self.key));
            if !iso.is_file() {
                return Err("Choose the disc image (.iso) file.".into());
            }
            if !key.is_file() {
                return Err("Choose the key file.".into());
            }
            let key = std::fs::read(&key).map_err(|e| format!("Cannot read the key file: {e}"))?;
            disc::open(&iso, &key).map_err(|e| format!("{e:#}"))?;
        }
        if self.setup_enabled() {
            install::check_kit_zip(&self.kit().unwrap()).map_err(|e| format!("{e:#}"))?;
        }
        Ok(())
    }

    fn start(&mut self, ctx: &egui::Context) {
        let setup = self.setup_enabled();
        if let Some(id) = self.selected_game_id() {
            self.game_id = id;
        }
        self.setup_done = setup;
        let reuse = self.reuse && install::game_present(&self.dest(), self.game_id);
        let plan = Plan {
            dest: self.dest(),
            iso: (!reuse).then(|| clean(&self.iso)),
            key: (!reuse).then(|| clean(&self.key)),
            game_id: reuse.then_some(self.game_id),
            verify: self.verify,
            setup,
            english: self.english && setup,
            shortcuts: self.shortcuts && setup,
            kit: self.kit(),
        };
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (c, ctx) = (cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let emit = |e| {
                let _ = tx.send(Msg::Ev(e));
                ctx.request_repaint();
            };
            let r = install::run(&plan, &c, &emit).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Done(r));
            ctx.request_repaint();
        });
        self.run = Some(Run {
            rx,
            cancel,
            stage: "Starting".into(),
            progress: (0, 0),
            log: vec![],
            result: None,
        });
        self.page = Page::Install;
    }

    fn poll(&mut self) {
        let Some(run) = &mut self.run else { return };
        while let Ok(m) = run.rx.try_recv() {
            match m {
                Msg::Ev(Event::Stage(s)) => {
                    run.log.push(format!("— {s}"));
                    run.stage = s;
                    run.progress = (0, 0);
                }
                Msg::Ev(Event::Progress(d, t)) => run.progress = (d, t),
                Msg::Ev(Event::Log(l)) => run.log.push(l),
                Msg::Done(r) => run.result = Some(r),
            }
        }
        if matches!(run.result, Some(Ok(()))) {
            self.page = Page::Done;
        }
    }

    fn body(&mut self, ui: &mut egui::Ui) {
        match self.page {
            Page::Welcome => {
                ui.heading("Forza-WCCF");
                ui.add_space(8.0);
                ui.label(
                    "Decrypt and extract World Club Champion Football 2010–2011 (Rev D) or \
                          2017–2018 v2.00.00 (DVR-5059A), from your own disc image.",
                );
                ui.add_space(8.0);
                ui.label(RichText::new("You will need").strong());
                ui.label("•  the game's disc image (an .iso file, about 5–6 GB)");
                ui.label("•  the matching key file for it");
                ui.label("•  optional: WCCF-2010-11-kit.zip for 2010–2011 setup only");
                ui.label("•  about 6 GB of free space (NTFS is needed only for kit setup)");
                ui.add_space(8.0);
                ui.label("Without a kit, this only extracts the original files; it does not make them playable. \
                          Nothing is sent anywhere, and your disc image is never changed.");
            }
            Page::Where => {
                ui.heading("Where should the game go?");
                ui.add_space(8.0);
                Self::file_row(ui, "Install folder", &mut self.dest, true, || {
                    rfd::FileDialog::new()
                        .set_title("Install folder")
                        .pick_folder()
                });
                let dest = self.dest();
                ui.add_space(6.0);
                match install::free_space(&dest) {
                    Some(f) => ui.label(format!(
                        "Free space on that drive: {}   (needed: about 6 GB)",
                        gb(f)
                    )),
                    None => ui.label("Needed: about 6 GB"),
                };
                if !self.dest.is_ascii() {
                    ui.colored_label(egui::Color32::from_rgb(200, 130, 0),
                        "Tip: the game has trouble with accented or non-English letters in folder names. \
                         A plain path like C:\\Games\\Forza-WCCF is safest.");
                }
                for disc in &disc::DISCS {
                    if install::game_present(&dest, disc.game_id) {
                        ui.colored_label(
                            egui::Color32::from_rgb(0, 140, 60),
                            format!("{} is installed here.", disc.edition),
                        );
                    }
                }
                ui.add_space(6.0);
                ui.label("Both editions can coexist: 2010-2011\\game and 2017-2018\\game, each with its own kit, settings and saves.");
            }
            Page::Files => {
                ui.heading("Your disc image and key file");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Edition to reuse:");
                    for disc in &disc::DISCS {
                        ui.selectable_value(&mut self.game_id, disc.game_id, disc.edition);
                    }
                });
                let have = install::game_present(&self.dest(), self.game_id);
                if have {
                    ui.checkbox(
                        &mut self.reuse,
                        "The game files are already here: use them (no disc needed)",
                    );
                    ui.add_space(6.0);
                }
                let on = !(have && self.reuse);
                Self::file_row(ui, "Disc image (.iso)", &mut self.iso, on, || {
                    rfd::FileDialog::new()
                        .add_filter("Disc image", &["iso"])
                        .pick_file()
                });
                ui.add_space(6.0);
                Self::file_row(ui, "Key file", &mut self.key, on, || {
                    rfd::FileDialog::new().pick_file()
                });
                ui.add_space(6.0);
                Self::file_row(
                    ui,
                    "Optional kit (WCCF-2010-11-kit.zip; 2010–2011 only)",
                    &mut self.kit,
                    true,
                    || {
                        rfd::FileDialog::new()
                            .add_filter("Kit zip", &["zip"])
                            .pick_file()
                    },
                );
                ui.add_space(6.0);
                ui.weak("Tip: you can also drag the files onto this window.");
                if on {
                    let iso = clean(&self.iso);
                    if let Ok(m) = std::fs::metadata(&iso) {
                        if let Some(disc) = disc::by_size(m.len()) {
                            ui.label(format!("Detected: {}", disc.name));
                        } else {
                            ui.colored_label(egui::Color32::from_rgb(200, 60, 60),
                                "Unsupported image size. Use 2010–2011 Rev D or 2017–2018 DVR-5059A.");
                        }
                    }
                }
            }
            Page::Options => {
                ui.heading("Options");
                ui.add_space(8.0);
                ui.checkbox(&mut self.verify, "Check the disc image first");
                ui.weak(
                    "      Finds a damaged copy before the long unpacking. Adds about a minute.",
                );
                ui.add_space(6.0);
                let can_setup = self.kit().is_some() && self.selected_game_id() == Some("SBWG");
                ui.add_enabled_ui(can_setup, |ui| {
                    ui.checkbox(
                        &mut self.setup,
                        "Set up the game with the optional 2010–2011 kit",
                    );
                });
                if can_setup {
                    ui.weak("      Installs the community kit and prepares the card catalogue (about a minute).");
                } else {
                    ui.weak("      Extraction only. The kit is optional and does not support 2017–2018.");
                }
                ui.add_space(6.0);
                ui.add_enabled_ui(self.setup_enabled(), |ui| {
                    ui.checkbox(&mut self.english, "Play in English");
                    ui.weak("      Translates most of the on-screen text. Can be undone later (ENGLISH.bat off).");
                    ui.add_space(6.0);
                    ui.checkbox(&mut self.shortcuts, "Add Play and Stop shortcuts to the desktop");
                });
            }
            Page::Install => {
                ui.heading("Installing");
                ui.add_space(8.0);
                let Some(run) = &self.run else { return };
                match &run.result {
                    None => {
                        ui.label(RichText::new(&run.stage).strong());
                        let (d, t) = run.progress;
                        if t > 0 {
                            ui.add(
                                egui::ProgressBar::new(d as f32 / t as f32)
                                    .show_percentage()
                                    .text(format!("{} of {}", gb(d), gb(t))),
                            );
                        } else {
                            ui.add(egui::ProgressBar::new(0.0).animate(true));
                        }
                        ui.add_space(6.0);
                        ui.weak("This takes a few minutes. You can leave the window open and do something else.");
                    }
                    Some(Err(e)) => {
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 60, 60),
                            RichText::new(if e == "Cancelled." {
                                "Installation cancelled."
                            } else {
                                "The installation stopped."
                            })
                            .strong(),
                        );
                        ui.label(e);
                    }
                    Some(Ok(())) => {}
                }
                ui.add_space(8.0);
                egui::CollapsingHeader::new("Show details").show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for l in &run.log {
                                ui.monospace(l);
                            }
                        });
                });
            }
            Page::Done => {
                ui.heading("All done!");
                ui.add_space(8.0);
                let installed = install::version_paths(&self.dest(), self.game_id)
                    .unwrap()
                    .0;
                ui.label(format!(
                    "Forza-WCCF is installed in {}",
                    installed.display()
                ));
                ui.add_space(8.0);
                if self.setup_done {
                    ui.label(RichText::new("Optional kit setup complete").strong());
                    ui.label("Read your kit's instructions before using Play/Stop. The installer does not verify gameplay or saves.");
                    ui.label("Only launch a hash-verified guarded runtime with network protection. Do not run the original executables directly.");
                    ui.add_space(8.0);
                    ui.weak("The installer does not change host interfaces, routes or firewall settings.");
                    ui.weak("A compatible runtime may also require DirectX 9 (June 2010 runtime).");
                } else {
                    ui.label(RichText::new("Decryption and extraction complete").strong());
                    ui.label("No kit, English patches or Play/Stop shortcuts were applied.");
                    ui.label("The original executables are not a ready-to-play runtime. Do not launch them \
                              without a compatible loader and network protection.");
                }
                ui.add_space(10.0);
                if ui.button("Open install folder").clicked() {
                    open_folder(&installed);
                }
            }
        }
    }
}

fn open_folder(p: &Path) {
    let opener = if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(p).spawn();
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            if let (Some(p), Page::Files) = (f.path, self.page) {
                let ext = p.extension().map(|e| e.to_ascii_lowercase());
                let field = match ext.as_deref().and_then(|e| e.to_str()) {
                    Some("iso") => &mut self.iso,
                    Some("zip") => &mut self.kit,
                    _ => &mut self.key,
                };
                *field = p.display().to_string();
            }
        }
        let running = self.run.as_ref().is_some_and(|r| r.result.is_none());
        let failed = self
            .run
            .as_ref()
            .is_some_and(|r| matches!(r.result, Some(Err(_))));
        egui::Panel::bottom("nav").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if let Some(e) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(200, 60, 60), e);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let page = self.page;
                    match page {
                        Page::Install if running => {
                            if ui.button("Cancel").clicked() {
                                self.run
                                    .as_ref()
                                    .unwrap()
                                    .cancel
                                    .store(true, Ordering::Relaxed);
                            }
                        }
                        Page::Install if failed => {
                            if ui.button("Back").clicked() {
                                self.page = Page::Options;
                            }
                        }
                        Page::Done => {
                            if ui.button("Close").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                        _ => {
                            let last = page == Page::Options;
                            if ui.button(if last { "Install" } else { "Next" }).clicked() {
                                self.error = None;
                                let step: Result<(), String> = match page {
                                    Page::Where if self.dest.trim().is_empty() => {
                                        Err("Choose an install folder.".into())
                                    }
                                    Page::Files | Page::Options => self.check_files(),
                                    _ => Ok(()),
                                };
                                match step {
                                    Err(e) => self.error = Some(e),
                                    Ok(()) if last => self.start(&ctx),
                                    Ok(()) => {
                                        if page == Page::Where {
                                            if let Some(id) =
                                                install::installed_game_id(&self.dest())
                                            {
                                                self.game_id = id;
                                            }
                                            self.reuse =
                                                install::game_present(&self.dest(), self.game_id);
                                        }
                                        self.page = match page {
                                            Page::Welcome => Page::Where,
                                            Page::Where => Page::Files,
                                            _ => Page::Options,
                                        };
                                    }
                                }
                            }
                            if page != Page::Welcome && ui.button("Back").clicked() {
                                self.error = None;
                                self.page = match page {
                                    Page::Where => Page::Welcome,
                                    Page::Files => Page::Where,
                                    _ => Page::Files,
                                };
                            }
                        }
                    }
                });
            });
            ui.add_space(6.0);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(6.0);
            self.body(ui);
        });
    }
}

fn parse_headless(args: &[String]) -> anyhow::Result<Option<Plan>> {
    use anyhow::{Context, bail, ensure};
    if args.is_empty() {
        return Ok(None);
    }
    let mut dest = None;
    let mut plan = Plan {
        dest: PathBuf::new(),
        iso: None,
        key: None,
        game_id: None,
        kit: None,
        verify: true,
        setup: true,
        english: false,
        shortcuts: false,
    };
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let slot = match flag.as_str() {
            "--game" => {
                ensure!(plan.game_id.is_none(), "Duplicate argument --game.");
                let value = args.next().context("--game requires SBWG or SDDY.")?;
                plan.game_id = Some(
                    disc::DISCS
                        .iter()
                        .find(|d| d.game_id == value)
                        .context("--game requires SBWG (2010–2011) or SDDY (2017–2018).")?
                        .game_id,
                );
                continue;
            }
            "--dest" => &mut dest,
            "--iso" => &mut plan.iso,
            "--key" => &mut plan.key,
            "--kit" => &mut plan.kit,
            "--no-verify" => {
                plan.verify = false;
                continue;
            }
            "--no-setup" => {
                plan.setup = false;
                continue;
            }
            "--english" => {
                plan.english = true;
                continue;
            }
            _ => bail!("Unknown argument {flag}; use --help."),
        };
        ensure!(slot.is_none(), "Duplicate argument {flag}.");
        let value = args
            .next()
            .filter(|v| !v.starts_with("--") && !v.trim().is_empty())
            .with_context(|| format!("{flag} needs a path."))?;
        *slot = Some(PathBuf::from(value));
    }
    plan.dest = dest.context("Headless mode requires --dest DIR; use --help.")?;
    ensure!(
        plan.iso.is_some() == plan.key.is_some(),
        "Provide both --iso and --key, or neither to reuse existing files."
    );
    Ok(Some(plan))
}

fn headless(args: &[String]) -> anyhow::Result<bool> {
    if args.len() == 1 {
        match args[0].as_str() {
            "--help" | "-h" => {
                println!(
                    "Forza-WCCF Installer {}\n\n\
                    No arguments: open the Windows wizard.\n\
                    Extract: --iso GAME.iso --key MATCHING_KEY --dest DIR\n\
                    Reuse:   --dest DIR --game SBWG|SDDY\n\
                    Editions coexist in DIR/2010-2011 and DIR/2017-2018 with separate kits/settings/saves.\n\
                    Options: --kit KIT.zip (trusted 2010-2011 kit only), --no-setup, --english, --no-verify\n\
                    Info:    --help, --version\n\n\
                    Supply your own image and key. No game files or keys are included.\n\
                    Existing game folders are never replaced. Extraction does not make the game playable.",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(true);
            }
            "--version" | "-V" => {
                println!("Forza-WCCF Installer {}", env!("CARGO_PKG_VERSION"));
                return Ok(true);
            }
            _ => {}
        }
    }
    let Some(plan) = parse_headless(args)? else {
        return Ok(false);
    };
    let last = std::cell::Cell::new(0u64);
    let emit = |e: Event| match e {
        Event::Stage(s) => println!("== {s}"),
        Event::Log(l) => println!("{l}"),
        Event::Progress(d, t) if t > 0 && d / (256 << 20) != last.replace(d / (256 << 20)) => {
            println!("   {} / {}", gb(d), gb(t))
        }
        Event::Progress(..) => {}
    };
    install::run(&plan, &AtomicBool::new(false), &emit)?;
    println!("done");
    Ok(true)
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match headless(&args) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(e) => {
            eprintln!("ERROR: {e:#}");
            std::process::exit(1);
        }
    }
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([680.0, 470.0])
            .with_min_inner_size([560.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Forza-WCCF Installer",
        opts,
        Box::new(|cc| {
            cc.egui_ctx.set_zoom_factor(1.15);
            Ok(Box::new(App::new()))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_arguments_are_validated_before_installing() {
        let parse =
            |args: &[&str]| parse_headless(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(parse(&[]).unwrap().is_none());
        let plan = parse(&[
            "--iso",
            "disc.iso",
            "--key",
            "key.txt",
            "--dest",
            "install folder",
            "--no-setup",
            "--no-verify",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(plan.iso, Some(PathBuf::from("disc.iso")));
        assert_eq!(plan.dest, PathBuf::from("install folder"));
        assert!(!plan.setup && !plan.verify && !plan.shortcuts);
        assert!(parse(&["--dest", "reuse"]).unwrap().unwrap().iso.is_none());
        assert_eq!(
            parse(&["--dest", "reuse", "--game", "SDDY"])
                .unwrap()
                .unwrap()
                .game_id,
            Some("SDDY")
        );
        for args in [
            vec!["--iso", "disc.iso"],
            vec!["--dest"],
            vec!["--dest", ""],
            vec!["--dest", "--no-setup"],
            vec!["--dest", "a", "--dest", "b"],
            vec!["--dest", "a", "--iso", "disc.iso"],
            vec!["--dest", "a", "--key", "key.txt"],
            vec!["--dest", "a", "--typo"],
            vec!["--dest", "a", "--game"],
            vec!["--dest", "a", "--game", "unknown"],
            vec!["--dest", "a", "--game", "SBWG", "--game", "SDDY"],
        ] {
            assert!(
                parse(&args).is_err(),
                "accepted invalid arguments: {args:?}"
            );
        }
    }
}
