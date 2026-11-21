//! The install pipeline: verify disc -> extract game -> optional kit setup (English, shortcuts).
//! Runs on a worker thread; reports through `emit`, stops when `cancel` is set.
use crate::err::{InstallError as E, path as shown};
use crate::{disc, fetch};
use anyhow::{Result, ensure};
use forza::Edition;
use forza::apply::Failed;
use forza::text::{Lang, Mode};
use forza_ui::installer::Step;
use ntfs::{Ntfs, NtfsReadSeek, structured_values::NtfsFileNamespace};
use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const GAME_DIR: &str = "game";
const LEGACY_GAME_DIR: &str = "wccf-revival"; // v0.1.0 compatibility only; never rename existing data.
const HEADROOM: u64 = 600_000_000; // card pictures + seat folder when using the kit

pub struct Plan {
    pub dest: PathBuf,
    pub iso: Option<PathBuf>, // None = reuse the game files already in dest
    pub key: Option<PathBuf>,
    pub game_id: Option<&'static str>, // select an edition when reusing a shared install root
    pub verify: bool,
    pub setup: bool,
    pub english: bool,
    pub shortcuts: bool,
    pub kit: Option<PathBuf>, // the kit zip
    pub latest: bool, // no kit zip given: download the newest package (2017-18 only) and use it as if chosen
}

pub enum Event {
    Stage(Step),
    Progress(u64, u64), // done, total (0 total = unknown)
    /// A line of the log, which is English (support reads it). A line the player must act on is `ui.yml` text in the
    /// run's language instead, as the error messages are.
    Log(String),
    /// A step that failed without stopping the install (the card pictures), in the run's language: the Done page
    /// shows it under its rows, the log too.
    Missed(String),
    /// The desktop shortcut was not made, in the run's language: as [`Event::Missed`], and the Done page's shortcut
    /// row reads not made.
    NoShortcut(String),
}

fn game_id_in(game: &Path) -> Option<&'static str> {
    let mut ids = disc::DISCS.iter().filter(|d| {
        game.join(format!("local/{}_Table.dat", d.edition.game_id()))
            .is_file()
    });
    let id = ids.next()?.edition.game_id();
    ids.next().is_none().then_some(id)
}

/// Each edition owns its entire kit/settings/save tree, not just its game files.
pub fn version_paths(dest: &Path, id: &str) -> Result<(PathBuf, PathBuf)> {
    let disc = disc::DISCS
        .iter()
        .find(|d| d.edition.game_id() == id)
        .ok_or(E::UnknownEdition)?;
    let root = dest.join(disc.edition.folder());
    let legacy = dest.join(LEGACY_GAME_DIR);
    if !root.exists() && game_id_in(&legacy) == Some(id) {
        return Ok((dest.to_path_buf(), legacy)); // preserve older installs in place
    }
    let game = root.join(GAME_DIR);
    Ok((root, game))
}

/// The install folder for a folder the player picked: a Forza-WCCF install never lands loose in a drive's root or another
/// program's folder. `D:\` gives `D:\Forza-WCCF`; a folder named Forza-WCCF, or one already holding an install, stays.
pub fn install_folder(picked: &Path) -> PathBuf {
    let named = picked
        .file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("Forza-WCCF"));
    let holds_install = disc::DISCS
        .iter()
        .map(|d| d.edition.folder())
        .chain([LEGACY_GAME_DIR])
        .any(|d| picked.join(d).is_dir());
    if named || holds_install {
        picked.to_path_buf()
    } else {
        picked.join("Forza-WCCF")
    }
}

pub fn game_present(dest: &Path, id: &str) -> bool {
    version_paths(dest, id).is_ok_and(|(_, game)| {
        game.join("client_Release.exe").is_file() && game_id_in(&game) == Some(id)
    })
}

pub fn installed_game_id(dest: &Path) -> Option<&'static str> {
    let mut ids = disc::DISCS
        .iter()
        .filter(|d| game_present(dest, d.edition.game_id()));
    let id = ids.next()?.edition.game_id();
    ids.next().is_none().then_some(id)
}

/// The edition a game id belongs to.
pub(crate) fn edition_of(game_id: &str) -> Option<Edition> {
    [Edition::E1011, Edition::E1718]
        .into_iter()
        .find(|e| e.game_id() == game_id)
}

/// The kit's edition must be the game's.
pub fn check_setup(game_id: Option<&str>, kit: Edition) -> Result<()> {
    ensure!(
        game_id.and_then(edition_of) == Some(kit),
        E::KitEdition {
            kit: kit.label(),
            game: game_id.and_then(edition_of).map_or("?", Edition::label),
        }
    );
    Ok(())
}

/// The kit zip may hold the files directly or inside one top folder (Forza-WCCF-2010-11/..., or the older WCCF-2010-11-kit/...). A kit has Launcher.exe at
/// its top and its edition's folders beside it ([`Edition::of_kit`]: `programs/` for 2010-11, `bin/` or `overlay/` for
/// 2017-18).
fn kit_prefix(zip: &mut zip::ZipArchive<File>) -> Result<(Edition, String)> {
    let names = zip
        .file_names()
        .map(|n| n.map(|n| n.into_owned()))
        .collect::<Result<Vec<String>, _>>()
        .map_err(|_| E::ZipName)?;
    let has = |p: &str| {
        names
            .iter()
            .any(|n| n == p || n.starts_with(&format!("{p}/")))
    };
    for n in names.iter().filter(|n| n.ends_with("Launcher.exe")) {
        let Some(prefix) = n.strip_suffix("Launcher.exe") else {
            continue;
        };
        if !(prefix.is_empty() || prefix.ends_with('/')) {
            continue;
        }
        if let Some(edition) = Edition::of_kit(|d| has(&format!("{prefix}{d}"))) {
            return Ok((edition, prefix.to_string()));
        }
    }
    Err(E::NotKitZip.into())
}

pub fn check_kit_zip(path: &Path) -> Result<Edition> {
    let f = File::open(path).map_err(|e| E::CannotOpen {
        path: shown(path),
        detail: e.to_string(),
    })?;
    let mut zip = zip::ZipArchive::new(f).map_err(|_| E::NotZip)?;
    kit_prefix(&mut zip).map(|(e, _)| e)
}

// tests set the free space a run sees
#[cfg(test)]
thread_local!(static FREE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) });

pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(test)]
    if let Some(free) = FREE.get() {
        return Some(free);
    }
    on_disk(path, |p| fs4::available_space(p))
}

pub fn total_space(path: &Path) -> Option<u64> {
    on_disk(path, |p| fs4::total_space(p))
}

/// `f` on the nearest folder of `path` that exists (the install folder may not yet).
fn on_disk(path: &Path, f: impl Fn(&Path) -> std::io::Result<u64>) -> Option<u64> {
    path.ancestors()
        .find(|p| p.exists())
        .and_then(|p| f(p).ok())
}

fn check(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), E::Cancelled);
    Ok(())
}

/// What [`apply`] does, decided by [`resolve`] before anything is written.
struct Resolved {
    dest: PathBuf,
    game: PathBuf,
    vol: Option<disc::Volume>,
    setup: Option<(PathBuf, Edition)>, // the package to set up and its edition
}

pub fn run(
    plan: &Plan,
    cancel: &AtomicBool,
    lang: forza_ui::Lang,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let r = resolve(plan, cancel, emit)?;
    apply(plan, r, cancel, lang, emit)
}

/// Every refusal, and nothing written to the install: the edition, the package, a program running from the install,
/// the free space; then the slow reads (the disc opened, the package downloaded, the disc's verify pass).
fn resolve(plan: &Plan, cancel: &AtomicBool, emit: &dyn Fn(Event)) -> Result<Resolved> {
    check(cancel)?;
    let detected = plan
        .iso
        .as_ref()
        .and_then(|p| fs::metadata(p).ok())
        .and_then(|m| disc::by_size(m.len()))
        .map(|d| d.edition.game_id());
    ensure!(
        detected.is_none() || plan.game_id.is_none() || detected == plan.game_id,
        E::EditionMismatch
    );
    let id = detected
        .or(plan.game_id)
        .or_else(|| installed_game_id(&plan.dest))
        .ok_or(E::NeedGame)?;
    let (dest, game) = version_paths(&plan.dest, id)?;
    ensure!(plan.iso.is_none() || !game.exists(), E::GameFolderExists);
    let download = plan.latest && plan.kit.is_none() && plan.setup;
    ensure!(!download || id == Edition::E1718.game_id(), E::NoDownload);
    let edition = if plan.setup && (plan.kit.is_some() || download) {
        Some(edition_of(id).ok_or(E::UnknownEdition)?)
    } else {
        None
    };
    ensure!(!plan.english || edition.is_some(), E::EnglishNeedsSetup);
    let key = match &plan.iso {
        Some(_) => Some(
            fs::read(plan.key.as_ref().ok_or(E::NoKey)?).map_err(|e| E::KeyRead {
                detail: e.to_string(),
            })?,
        ),
        None => {
            ensure!(
                game_present(&plan.dest, id),
                E::GameMissing {
                    path: shown(&plan.dest)
                }
            );
            None
        }
    };
    let kit = plan.kit.as_ref().filter(|_| edition.is_some());
    if let Some(kit) = kit {
        check_setup(Some(id), check_kit_zip(kit)?)?;
    }
    if edition.is_some() {
        refuse_running(&dest)?;
    }
    // the game takes at most its slot of the disc: a deliberate upper bound, the volume's size is known only once the
    // disc is opened, after this check
    let extract = disc::DISCS
        .iter()
        .find(|d| d.edition.game_id() == id)
        .filter(|_| plan.iso.is_some())
        .map_or(0, |d| d.slot_end - d.slot_start);
    // setup's headroom only when it makes the card pictures: a new game, or an install without them
    let headroom = if plan.iso.is_some()
        || edition.is_some_and(|e| !dest.join(e.layout().cache).join("cards").is_dir())
    {
        HEADROOM
    } else {
        0
    };
    if plan.iso.is_some() || edition.is_some() {
        room(&dest, extract + headroom + kit.map_or(0, |k| kit_room(k)))?;
    }
    let vol = match (&plan.iso, key) {
        (Some(iso), Some(key)) => Some((iso, disc::open(iso, &key)?)),
        _ => None,
    };
    if let Some((_, (vol, _))) = &vol {
        emit(Event::Log(format!(
            "Detected {} ({})",
            vol.disc.name,
            vol.disc.edition.game_id()
        )));
    }
    let mut kit = kit.cloned();
    if download {
        emit(Event::Stage(Step::Download));
        let p = fetch::latest(fetch::PREFIX_1718)?;
        emit(Event::Log(format!("Downloading {}", p.zip_name)));
        let got = fetch::download(&p, cancel, &|d, t| emit(Event::Progress(d, t)))?;
        emit(Event::Log("Package checked against its SHA-256".into()));
        check_setup(Some(id), check_kit_zip(&got)?)?;
        room(&dest, extract + headroom + kit_room(&got))?;
        kit = Some(got);
    }
    if let Some((iso, (vol, table))) = &vol
        && plan.verify
    {
        emit(Event::Stage(Step::Verify));
        let total = (table.len() - 1) as u64 * 0x40000;
        disc::verify_blocks(
            iso,
            vol.disc,
            table,
            |d| {
                emit(Event::Progress(d, total));
            },
            || check(cancel),
        )?;
    }
    Ok(Resolved {
        dest,
        game,
        vol: vol.map(|(_, (vol, _))| vol),
        setup: kit.zip(edition),
    })
}

/// Writes what [`resolve`] decided: the game extracted, the package over the install, setup, English, pictures, shortcuts.
fn apply(
    plan: &Plan,
    r: Resolved,
    cancel: &AtomicBool,
    lang: forza_ui::Lang,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let Resolved {
        dest,
        game,
        vol,
        setup,
    } = r;
    fs::create_dir_all(&dest).map_err(|e| E::CannotCreate {
        path: shown(&dest),
        detail: e.to_string(),
    })?;
    if setup.is_some() {
        probe_links(&dest)?;
    }
    if let Some(mut vol) = vol {
        extract(&mut vol, &game, cancel, emit)?;
    }
    if let Some((kit, edition)) = setup {
        emit(Event::Stage(Step::Setup));
        unpack_kit(&kit, &dest, lang, emit)?;
        check(cancel)?;
        let folder = game.to_string_lossy();
        kit_step(emit, |say| {
            forza::setup::kit_setup(edition, &dest, &folder, false, false, say)
        })?;
        if plan.english {
            check(cancel)?;
            emit(Event::Stage(Step::English));
            kit_step(emit, |say| {
                match forza::setup::find_edition_game(edition, &folder) {
                    Some(g) => {
                        let backup = dest.join("data").join("english_backup");
                        forza::text::run(edition, &g, &backup, &dest, Mode::On, Lang::En, say)
                    }
                    None => 2,
                }
            })?;
        }
        pictures(&dest, edition, &folder, lang, emit);
        if plan.shortcuts {
            shortcuts(&dest, edition, lang, emit);
        }
    } else {
        emit(Event::Log(forza_ui::text(
            lang,
            "installer.log_extract_only",
            &[],
        )));
    }
    Ok(())
}

/// The drive holds `need` bytes.
fn room(dest: &Path, need: u64) -> Result<()> {
    ensure!(free_space(dest).is_none_or(|f| f > need), E::NoSpace);
    Ok(())
}

/// The bytes a package takes on the install's drive: unpacked once beside the install, then copied over it.
fn kit_room(kit: &Path) -> u64 {
    File::open(kit)
        .ok()
        .and_then(|f| zip::ZipArchive::new(f).ok())
        .map_or(0, |mut z| 2 * unpacked_size(&mut z))
}

fn unpacked_size(zip: &mut zip::ZipArchive<File>) -> u64 {
    (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.size()))
        .sum()
}

/// No program runs from the install folder: its files cannot be replaced while one does.
fn refuse_running(dest: &Path) -> Result<()> {
    #[cfg(windows)]
    if let Some(p) = forza::procs::running_from(dest) {
        return Err(running(&p).into());
    }
    #[cfg(not(windows))]
    let _ = dest;
    Ok(())
}

fn running(program: &Path) -> E {
    E::ProgramRunning {
        name: program
            .file_name()
            .map_or(program.as_os_str(), |n| n)
            .to_string_lossy()
            .into_owned(),
    }
}

/// The kit links the game's files with hard links and junctions: the drive must be NTFS. Fail now, not after 4 GB.
fn probe_links(dest: &Path) -> Result<()> {
    let (a, b) = (dest.join(".probe-a"), dest.join(".probe-b"));
    let _ = (fs::remove_file(&a), fs::remove_file(&b));
    fs::write(&a, b"x").map_err(write_err(dest))?;
    let linked = fs::hard_link(&a, &b);
    let _ = (fs::remove_file(&a), fs::remove_file(&b));
    Ok(linked.map_err(|_| E::NotNtfsDrive)?)
}

struct Item {
    rel: PathBuf,
    rec: u64,
    dir: bool,
    size: u64,
}

/// A name from the disc must stay inside the destination, and must not be one Windows rewrites: a trailing space or
/// dot goes, and `CON`, `NUL` or `COM1` with any extension opens the device, not a file of the kit.
fn safe(name: &str) -> bool {
    if name.is_empty()
        || name.contains(['/', '\\', ':'])
        || name == "."
        || name == ".."
        || name.ends_with([' ', '.'])
    {
        return false;
    }
    // Windows also drops spaces before the dot: `CON .txt` is the device too
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes().get(3), Some(b'1'..=b'9')))
}

/// Keep a disc entry: a safe name, and none of the root's own metadata (`$MFT`, `$Bitmap`, `System Volume
/// Information`), which is not game data.
fn keep(dir: &Path, name: &str) -> bool {
    let root = dir.as_os_str().is_empty();
    safe(name) && !(root && (name.starts_with('$') || name == "System Volume Information"))
}

fn scan<T: Read + Seek>(ntfs: &Ntfs, fs: &mut T) -> Result<Vec<Item>> {
    let mut out = Vec::new();
    let mut stack = vec![(
        PathBuf::new(),
        ntfs.root_directory(fs)?.file_record_number(),
    )];
    while let Some((dir, rec)) = stack.pop() {
        let mut kids = Vec::new();
        {
            let dir_file = ntfs.file(fs, rec)?;
            let index = dir_file.directory_index(fs)?;
            let mut it = index.entries();
            while let Some(e) = it.next(fs) {
                let e = e?;
                let Some(name) = e.key() else { continue };
                let name = name?;
                if name.namespace() == NtfsFileNamespace::Dos {
                    continue; // the 8.3 duplicate of a long name
                }
                kids.push((
                    name.name().to_string_lossy(),
                    name.is_directory(),
                    e.file_reference().file_record_number(),
                ));
            }
        }
        for (name, is_dir, rec) in kids {
            if !keep(&dir, &name) {
                continue;
            }
            let size = match ntfs.file(fs, rec)?.data(fs, "") {
                Some(d) if !is_dir => d?.to_attribute()?.value_length(),
                _ => 0,
            };
            let rel = dir.join(&name);
            if is_dir {
                stack.push((rel.clone(), rec));
            }
            out.push(Item {
                rel,
                rec,
                dir: is_dir,
                size,
            });
        }
    }
    Ok(out)
}

/// A failed write as the player reads it: a full drive is the free-space message, anything else names the path (not
/// bare OS text, #301).
fn write_err(path: &Path) -> impl FnOnce(std::io::Error) -> E + '_ {
    move |e| match e.kind() {
        std::io::ErrorKind::StorageFull => E::NoSpace,
        _ => E::CannotWrite {
            path: shown(path),
            detail: e.to_string(),
        },
    }
}

/// `partial` made, `fill` run on it, and the tree taken with it when `fill` fails: a Cancel, a full disk or a read
/// error leaves no half game folder behind.
fn partial_tree(partial: &Path, fill: impl FnOnce() -> Result<()>) -> Result<()> {
    fs::create_dir_all(partial).map_err(write_err(partial))?;
    let done = fill();
    if done.is_err() {
        let _ = fs::remove_dir_all(partial);
    }
    done
}

/// Extract into `game`.partial beside it, then rename: a cancelled run never leaves a half game folder.
fn extract(
    vol: &mut disc::Volume,
    game: &Path,
    cancel: &AtomicBool,
    emit: &dyn Fn(Event),
) -> Result<()> {
    emit(Event::Stage(Step::Extract));
    emit(Event::Progress(0, 0));
    let ntfs = Ntfs::new(vol).map_err(|_| E::NotNtfsVolume)?;
    let items = scan(&ntfs, vol)?;
    let total: u64 = items.iter().map(|i| i.size).sum();
    let partial = game.with_extension("partial");
    if partial.exists() {
        fs::remove_dir_all(&partial).map_err(write_err(&partial))?;
    }
    partial_tree(&partial, || {
        emit(Event::Log(format!(
            "Copying {} game files from the disc",
            items.len()
        )));
        let (mut buf, mut done) = (vec![0u8; 1 << 20], 0u64);
        for it in &items {
            check(cancel)?;
            let path = partial.join(&it.rel);
            if it.dir {
                fs::create_dir_all(&path).map_err(write_err(&path))?;
                continue;
            }
            let mut out = File::create(&path).map_err(write_err(&path))?;
            let file = ntfs.file(vol, it.rec)?;
            if let Some(d) = file.data(vol, "") {
                let item = d?;
                let attr = item.to_attribute()?;
                let mut value = attr.value(vol)?;
                loop {
                    let n = value.read(vol, &mut buf)?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(buf.get(..n).unwrap_or_default())
                        .map_err(write_err(&path))?;
                    done += n as u64;
                    emit(Event::Progress(done, total));
                }
            }
        }
        ensure!(done == total, E::ExtractIncomplete);
        ensure!(
            partial.join("client_Release.exe").is_file(),
            E::ExtractIncomplete
        );
        check(cancel)?;
        ensure!(!game.exists(), E::GameFolderExists);
        fs::rename(&partial, game).map_err(write_err(game))?;
        Ok(())
    })
}

/// Unpack the kit zip beside the install (data\update, as the Launcher's UPDATE) and apply it over dest
/// (Launcher.exe, README, python, scripts ...) with `forza::apply`: a failed write puts the old files back. An existing
/// data\ (club, keys) is kept.
fn unpack_kit(
    zip_path: &Path,
    dest: &Path,
    lang: forza_ui::Lang,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(zip_path)?).map_err(|_| E::NotZip)?;
    let (_, prefix) = kit_prefix(&mut zip)?;
    let keep_data = dest.join("data").exists();
    let work = dest.join("data").join("update");
    let top = work.join("kit");
    let total = unpacked_size(&mut zip);
    if work.exists() {
        fs::remove_dir_all(&work).map_err(write_err(&work))?;
    }
    partial_tree(&top, || {
        let mut done = 0;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            // enclosed_name rejects absolute paths and ".." (a crafted zip must stay inside dest)
            let name = f.enclosed_name().ok_or(E::KitUnsafePath)?;
            let Ok(rel) = name.strip_prefix(prefix.trim_end_matches('/')) else {
                continue;
            };
            // the package's data\ is unpacked even over an install with data: `forza::apply` checks every file the
            // package lists, then skips data\ in any capitals when copying (as the Launcher's UPDATE)
            if rel.as_os_str().is_empty() {
                continue;
            }
            let out = top.join(rel);
            if f.is_dir() {
                fs::create_dir_all(&out).map_err(write_err(&out))?;
                continue;
            }
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(write_err(parent))?;
            }
            let mut file = File::create(&out).map_err(write_err(&out))?;
            done += std::io::copy(&mut f, &mut file).map_err(write_err(&out))?;
            emit(Event::Progress(done, total));
        }
        Ok(())
    })
    .inspect_err(|_| {
        let _ = fs::remove_dir_all(&work);
    })?;
    let skip: &[&str] = if keep_data { &["data"] } else { &[] };
    let log = |l: &str| emit(Event::Log(l.to_owned()));
    let write_failed = |p: &Path, e: std::io::Error| write_err(p)(e);
    forza::apply::apply(&top, dest, &work, skip, write_failed, || Ok(()), &log).map_err(|f| {
        match f {
            Failed::Running(p) => running(&p),
            Failed::Damaged(bad) => E::KitFiles {
                files: bad.join(", "),
            },
            Failed::RolledBack(e) => {
                log("The files the package replaced were put back.");
                e
            }
            Failed::Stuck(e) => {
                log(&forza_ui::text(
                    lang,
                    "installer.log_stuck",
                    &[("path", &shown(&work.join("old")))],
                ));
                e
            }
        }
        .into()
    })
}

/// One of the kit's steps (setup, the game's text) in this process, its lines into the log; a failure is its FAIL
/// line.  It cannot be cancelled half way: the cancel is looked at between steps.
fn kit_step(emit: &dyn Fn(Event), step: impl FnOnce(forza::Say) -> i32) -> Result<()> {
    let failed = std::cell::RefCell::new(None);
    // the installer's log is English (support reads it)
    let say = |lead: &str, l: &forza::Line| {
        let line = &format!("{lead}{}", forza_ui::line(forza_ui::Lang::En, l));
        if lead.trim() == "FAIL" {
            *failed.borrow_mut() = Some(line.trim().to_owned());
        }
        emit(Event::Log(line.to_owned()));
    };
    emit(Event::Progress(0, 0));
    let rc = step(&say);
    ensure!(
        rc == 0,
        E::Script {
            detail: failed.into_inner().unwrap_or_else(|| format!("exit {rc}"))
        }
    );
    Ok(())
}

/// The board's card pictures, as Launcher's Setup makes them. A failure is logged; the install stands.
fn pictures(
    dest: &Path,
    edition: Edition,
    folder: &str,
    lang: forza_ui::Lang,
    emit: &dyn Fn(Event),
) {
    let say = |l: &str| emit(Event::Log(l.to_owned()));
    let Some(game) = forza::setup::find_edition_game(edition, folder) else {
        let line = forza_ui::text(lang, "installer.log_pictures_no_game", &[]);
        return emit(Event::Missed(line));
    };
    let ed = edition.code();
    let cards = dest.join(edition.layout().cache).join("cards");
    let lines = |lead: &str, l: &forza::Line| {
        say(&format!("{lead}{}", forza_ui::line(forza_ui::Lang::En, l)))
    };
    if let Err(e) = forza::cards::pictures::ensure(&game, ed, &cards, None, &lines) {
        let e = e.to_string();
        let line = forza_ui::text(lang, "launcher.run_pictures_failed", &[("error", &e)]);
        emit(Event::Missed(line));
    }
}

fn shortcuts(dest: &Path, edition: Edition, lang: forza_ui::Lang, emit: &dyn Fn(Event)) {
    #[cfg(windows)]
    {
        let made = lnk::desktop().and_then(|d| {
            let link = d.join(format!("Forza-WCCF {}.lnk", edition.label()));
            lnk::write(&link, &dest.join("Launcher.exe"), dest)
        });
        emit(match made {
            Ok(()) => Event::Log("Desktop shortcut created.".into()),
            Err(_) => Event::NoShortcut(forza_ui::text(lang, "installer.log_shortcut_failed", &[])),
        });
    }
    #[cfg(not(windows))]
    let _ = (dest, edition, lang, emit);
}

/// The desktop shortcut made with the shell's IShellLinkW (2026-10-29: a PowerShell child made by an unsigned installer
/// is what antivirus machine learning flags).
#[cfg(windows)]
mod lnk {
    use std::path::{Path, PathBuf};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, CoUninitialize, IPersistFile,
    };
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink,
    };
    use windows::core::{HSTRING, Interface, Result};

    pub fn desktop() -> Result<PathBuf> {
        // SAFETY: the returned string is ours to read and free
        unsafe {
            let p = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None)?;
            let s = p.to_string();
            CoTaskMemFree(Some(p.0.cast()));
            Ok(PathBuf::from(s.map_err(|_| windows::core::Error::empty())?))
        }
    }

    /// `f` with COM started on this thread (left alone when the thread already has it).
    fn com<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
        // SAFETY: paired with CoUninitialize below only when it succeeded
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let r = f();
        if hr.is_ok() {
            // SAFETY: matches the successful CoInitializeEx above
            unsafe { CoUninitialize() };
        }
        r
    }

    fn shell_link() -> Result<IShellLinkW> {
        // SAFETY: an in-process shell object, COM started by `com`
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
    }

    /// The .lnk at `link`: `target`, started in `dir` (what the PowerShell shortcut set, nothing more).
    pub fn write(link: &Path, target: &Path, dir: &Path) -> Result<()> {
        com(|| {
            let l = shell_link()?;
            // SAFETY: the strings live for each call
            unsafe {
                l.SetPath(&HSTRING::from(target))?;
                l.SetWorkingDirectory(&HSTRING::from(dir))?;
                l.cast::<IPersistFile>()?.Save(&HSTRING::from(link), true)
            }
        })
    }

    /// The target and working folder of the .lnk at `link`.
    #[cfg(test)]
    pub fn read(link: &Path) -> Result<(PathBuf, PathBuf)> {
        com(|| {
            let l = shell_link()?;
            let (mut t, mut d) = ([0u16; 260], [0u16; 260]);
            // SAFETY: the buffers outlive the calls; no find data asked for
            unsafe {
                l.cast::<IPersistFile>()?
                    .Load(&HSTRING::from(link), windows::Win32::System::Com::STGM_READ)?;
                l.GetPath(&mut t, std::ptr::null_mut(), 0)?;
                l.GetWorkingDirectory(&mut d)?;
            }
            let s = |b: &[u16]| {
                PathBuf::from(String::from_utf16_lossy(
                    &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())],
                ))
            };
            Ok((s(&t), s(&d)))
        })
    }
}

#[cfg(test)]
mod tests;
