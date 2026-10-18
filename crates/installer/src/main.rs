#![cfg_attr(windows, windows_subsystem = "windows")]
//! Forza-WCCF installer: each RingEdge edition owns <dest>\<edition>\game and its kit/save tree.
//! The 2010–2011 kit is optional; 2017–2018 is extraction-only. `--iso/--key/--dest` runs headless.
//! The window is `forza::ui::installer` (the approved design) over the state kept here; this file runs what it asks.
mod disc;
mod err;
mod install;

use eframe::egui;
use forza::ui::installer::{self, Action, Field, InstallerState, Page, Space, Step};
use forza::ui::theme::{self, Edition};
use forza::ui::{Lang, text};
use install::{Event, Plan};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

enum Msg {
    Ev(Event),
    Done(anyhow::Result<()>),
}

struct Run {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
}

struct App {
    st: InstallerState,
    run: Option<Run>,
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

fn game_id(e: Edition) -> &'static str {
    match e {
        Edition::E1011 => "SBWG",
        Edition::E1718 => "SDDY",
    }
}

fn edition_of(id: &str) -> Edition {
    if id == "SDDY" {
        Edition::E1718
    } else {
        Edition::E1011
    }
}

impl App {
    fn new() -> Self {
        App {
            st: InstallerState::new(Lang::detect(), default_dest()),
            run: None,
        }
    }

    fn t(&self, key: &str) -> String {
        text(self.st.lang, key, &[])
    }

    fn dest(&self) -> PathBuf {
        let d = clean(&self.st.folder);
        std::path::absolute(&d).unwrap_or(d)
    }

    fn kit(&self) -> Option<PathBuf> {
        let path = clean(&self.st.package);
        (!path.as_os_str().is_empty()).then_some(path)
    }

    /// What the folder, the disc image and the package say, for the screen to show. Runs every frame, as the
    /// text boxes change under the person's hands.
    fn refresh(&mut self) {
        let (dest, kit) = (self.dest(), self.kit());
        let st = &mut self.st;
        st.installed = disc::DISCS
            .iter()
            .filter(|d| install::game_present(&dest, d.game_id))
            .map(|d| edition_of(d.game_id))
            .collect();
        if st.installed.is_empty() {
            st.reuse = false;
        } else if !st.installed.contains(&st.edition) {
            st.edition = st.installed[0];
        }
        let size = std::fs::metadata(clean(&st.iso)).ok().map(|m| m.len());
        let found = size.and_then(disc::by_size);
        st.detected = found.map(|d| edition_of(d.game_id));
        st.unsupported = size.is_some() && found.is_none();
        st.setup_possible = kit.is_some() && st.target() == Some(Edition::E1011);
        st.space = install::free_space(&dest).map(|free| Space {
            free,
            total: install::total_space(&dest).unwrap_or(free),
            drive: {
                let s = dest.to_string_lossy();
                if s.as_bytes().get(1) == Some(&b':') {
                    s[..2].to_string()
                } else {
                    String::new()
                }
            },
        });
        st.ascii_tip = !st.folder.is_ascii();
    }

    fn setup_enabled(&self) -> bool {
        self.st.options[1] && self.st.setup_possible
    }

    /// Validate the selected disc/key and, only when used, the optional kit before copying.
    fn check_files(&self) -> Result<(), String> {
        if !self.st.reuse {
            let (iso, key) = (clean(&self.st.iso), clean(&self.st.key));
            if !iso.is_file() {
                return Err(self.t("installer.err_iso"));
            }
            if !key.is_file() {
                return Err(self.t("installer.err_key"));
            }
            let key = std::fs::read(&key).map_err(|e| {
                text(
                    self.st.lang,
                    "installer.err_key_read",
                    &[("e", &e.to_string())],
                )
            })?;
            disc::open(&iso, &key).map_err(|e| err::localize(&e, self.st.lang))?;
        }
        if self.setup_enabled() {
            install::check_kit_zip(&self.kit().unwrap())
                .map_err(|e| err::localize(&e, self.st.lang))?;
        }
        Ok(())
    }

    fn start(&mut self, ctx: &egui::Context) {
        let setup = self.setup_enabled();
        let st = &self.st;
        let id = st.target().map(game_id);
        let reuse = st.reuse;
        let plan = Plan {
            dest: self.dest(),
            iso: (!reuse).then(|| clean(&st.iso)),
            key: (!reuse).then(|| clean(&st.key)),
            game_id: reuse.then_some(game_id(st.edition)),
            verify: st.options[0],
            setup,
            english: st.options[2] && setup,
            shortcuts: st.options[3] && setup,
            kit: self.kit(),
        };
        let mut steps = vec![];
        if !reuse && plan.verify {
            steps.push(Step::Verify);
        }
        if !reuse {
            steps.push(Step::Extract);
        }
        if setup {
            steps.push(Step::Setup);
        }
        if plan.english {
            steps.push(Step::English);
        }
        let did = [!reuse, setup, plan.english, plan.shortcuts];
        let done_path = id
            .and_then(|id| install::version_paths(&plan.dest, id).ok())
            .map(|p| p.0.display().to_string())
            .unwrap_or_default();
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (c, ctx) = (cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let emit = |e| {
                let _ = tx.send(Msg::Ev(e));
                ctx.request_repaint();
            };
            let r = install::run(&plan, &c, &emit);
            let _ = tx.send(Msg::Done(r));
            ctx.request_repaint();
        });
        self.run = Some(Run { rx, cancel });
        let st = &mut self.st;
        st.steps = steps;
        (st.stage, st.fraction, st.bytes, st.running) = (0, 0.0, None, true);
        (st.log, st.error, st.did, st.done_path) = (vec![], None, did, done_path);
        st.page = Page::Installing;
    }

    fn poll(&mut self) {
        let Some(run) = &self.run else { return };
        let (lang, st) = (self.st.lang, &mut self.st);
        while let Ok(m) = run.rx.try_recv() {
            match m {
                Msg::Ev(Event::Stage(s)) => {
                    st.log.push(format!("— {}", s.title(lang)));
                    st.stage = st.steps.iter().position(|x| *x == s).unwrap_or(st.stage);
                    (st.fraction, st.bytes) = (0.0, None);
                }
                Msg::Ev(Event::Progress(d, t)) => {
                    st.fraction = if t > 0 { d as f32 / t as f32 } else { 0.0 };
                    st.bytes = (t > 0).then_some((d, t));
                }
                Msg::Ev(Event::Log(l)) => st.log.push(l),
                Msg::Done(r) => {
                    st.running = false;
                    match r {
                        Ok(()) => st.page = Page::Done,
                        Err(e) if err::is_cancelled(&e) => {
                            st.error = Some(text(lang, "installer.cancelled", &[]))
                        }
                        Err(e) => {
                            st.error = Some(format!(
                                "{} {}",
                                text(lang, "installer.stopped", &[]),
                                err::localize(&e, lang)
                            ))
                        }
                    }
                }
            }
        }
    }

    fn browse(&mut self, field: Field) {
        let mut dialog = rfd::FileDialog::new();
        let picked = match field {
            Field::Folder => dialog.set_title(self.t("installer.folder")).pick_folder(),
            Field::Iso => {
                dialog = dialog.add_filter(self.t("installer.iso"), &["iso"]);
                dialog.pick_file()
            }
            Field::Key => dialog.pick_file(),
            Field::Package => {
                dialog = dialog.add_filter("zip", &["zip"]);
                dialog.pick_file()
            }
        };
        if let Some(p) = picked {
            let st = &mut self.st;
            let slot = match field {
                Field::Folder => &mut st.folder,
                Field::Iso => &mut st.iso,
                Field::Key => &mut st.key,
                Field::Package => &mut st.package,
            };
            *slot = p.display().to_string();
        }
    }

    fn next(&mut self, ctx: &egui::Context) {
        let page = self.st.page;
        self.st.error = None;
        let step = match page {
            Page::Where if self.st.folder.trim().is_empty() => Err(self.t("installer.err_folder")),
            Page::Files | Page::Options => self.check_files(),
            _ => Ok(()),
        };
        match step {
            Err(e) => self.st.error = Some(e),
            Ok(()) if page == Page::Options => self.start(ctx),
            Ok(()) => {
                if page == Page::Where {
                    if let Some(id) = install::installed_game_id(&self.dest()) {
                        self.st.edition = edition_of(id);
                    }
                    self.st.reuse = install::game_present(&self.dest(), game_id(self.st.edition));
                }
                self.st.page = match page {
                    Page::Welcome => Page::Where,
                    Page::Where => Page::Files,
                    _ => Page::Options,
                };
            }
        }
    }

    fn back(&mut self) {
        self.st.error = None;
        self.st.page = match self.st.page {
            Page::Where => Page::Welcome,
            Page::Files => Page::Where,
            Page::Options => Page::Files,
            _ => Page::Options,
        };
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
            if self.st.page == Page::Files {
                let p = f.path();
                let ext = p.extension().map(|e| e.to_ascii_lowercase());
                let st = &mut self.st;
                let field = match ext.as_deref().and_then(|e| e.to_str()) {
                    Some("iso") => &mut st.iso,
                    Some("zip") => &mut st.package,
                    _ => &mut st.key,
                };
                *field = p.display().to_string();
            }
        }
        self.refresh();
        let act = installer::show(ui, &mut self.st);
        match act {
            Some(Action::Drag) => ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag),
            Some(Action::Minimize) => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            Some(Action::Close) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Some(Action::Cancel) => {
                if let Some(run) = &self.run {
                    run.cancel.store(true, Ordering::Relaxed);
                }
            }
            Some(Action::Back) => self.back(),
            Some(Action::Next | Action::Install) => self.next(&ctx),
            Some(Action::Browse(f)) => self.browse(f),
            Some(Action::OpenFolder) => open_folder(Path::new(&self.st.done_path)),
            None => {}
        }
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
        Event::Stage(s) => println!("== {}", s.title(Lang::En)),
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
            .with_title("Forza-WCCF Installer")
            .with_decorations(false)
            .with_inner_size(installer::SIZE)
            .with_min_inner_size([560.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Forza-WCCF Installer",
        opts,
        Box::new(|cc| {
            // one exe, no kit around it yet: the fonts travel inside
            let fonts = theme::fonts_from(
                include_bytes!("../../../assets/fonts/NotoSansJP-VF.ttf"),
                include_bytes!("../../../assets/fonts/BarlowCondensed-Bold.ttf"),
                include_bytes!("../../../assets/fonts/Hack-Regular.ttf"),
            );
            theme::install_fonts(&cc.egui_ctx, Edition::E1011, fonts);
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
