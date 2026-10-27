//! The install pipeline: verify disc -> extract game -> optional kit setup (English, shortcuts).
//! Runs on a worker thread; reports through `emit`, stops when `cancel` is set.
use crate::err::{InstallError as E, path as shown};
use crate::{disc, fetch};
use anyhow::{Result, ensure};
use forza::Edition;
use forza::text::{Lang, Mode};
use forza_ui::installer::Step;
use ntfs::{Ntfs, NtfsReadSeek, structured_values::NtfsFileNamespace};
use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
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
    Log(String),
}

fn game_id_in(game: &Path) -> Option<&'static str> {
    let mut ids = disc::DISCS.iter().filter(|d| {
        game.join(format!("local/{}_Table.dat", d.game_id))
            .is_file()
    });
    let id = ids.next()?.game_id;
    ids.next().is_none().then_some(id)
}

/// Each edition owns its entire kit/settings/save tree, not just its game files.
pub fn version_paths(dest: &Path, id: &str) -> Result<(PathBuf, PathBuf)> {
    let disc = disc::DISCS
        .iter()
        .find(|d| d.game_id == id)
        .ok_or(E::UnknownEdition)?;
    let root = dest.join(disc.edition);
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
        .map(|d| d.edition)
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
    let mut ids = disc::DISCS.iter().filter(|d| game_present(dest, d.game_id));
    let id = ids.next()?.game_id;
    ids.next().is_none().then_some(id)
}

/// The edition a game id belongs to.
fn edition_of(game_id: &str) -> Option<Edition> {
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
/// its top and its edition's folders beside it or in `app/` ([`Edition::of_kit`]: `programs/` for 2010-11, `bin/` or
/// `overlay/` for 2017-18).
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
        let prefix = &n[..n.len() - "Launcher.exe".len()];
        if !(prefix.is_empty() || prefix.ends_with('/')) {
            continue;
        }
        if let Some(edition) =
            Edition::of_kit(|d| has(&format!("{prefix}{d}")) || has(&format!("{prefix}app/{d}")))
        {
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

pub fn free_space(path: &Path) -> Option<u64> {
    path.ancestors()
        .find(|p| p.exists())
        .and_then(|p| fs4::available_space(p).ok())
}

pub fn total_space(path: &Path) -> Option<u64> {
    path.ancestors()
        .find(|p| p.exists())
        .and_then(|p| fs4::total_space(p).ok())
}

fn check(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), E::Cancelled);
    Ok(())
}

pub fn run(plan: &Plan, cancel: &AtomicBool, emit: &dyn Fn(Event)) -> Result<()> {
    check(cancel)?;
    let detected = plan
        .iso
        .as_ref()
        .and_then(|p| fs::metadata(p).ok())
        .and_then(|m| disc::by_size(m.len()))
        .map(|d| d.game_id);
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
    let mut kit = plan.kit.clone();
    if plan.latest && kit.is_none() && plan.setup {
        ensure!(id == forza::Edition::E1718.game_id(), E::NoDownload);
        emit(Event::Stage(Step::Download));
        let p = fetch::latest(fetch::PREFIX_1718)?;
        emit(Event::Log(format!("Downloading {}", p.zip_name)));
        kit = Some(fetch::download(&p, cancel, &|d, t| {
            emit(Event::Progress(d, t))
        })?);
        emit(Event::Log("Package checked against its SHA-256".into()));
    }
    let setup = plan.setup && kit.is_some();
    ensure!(!plan.english || setup, E::EnglishNeedsSetup);
    let opened = if let Some(iso) = &plan.iso {
        let key = fs::read(plan.key.as_ref().ok_or(E::NoKey)?).map_err(|e| E::KeyRead {
            detail: e.to_string(),
        })?;
        Some(disc::open(iso, &key)?)
    } else {
        ensure!(
            game_present(&plan.dest, id),
            E::GameMissing {
                path: shown(&plan.dest)
            }
        );
        None
    };
    if setup {
        check_setup(Some(id), check_kit_zip(kit.as_ref().unwrap())?)?;
    }
    fs::create_dir_all(&dest).map_err(|e| E::CannotCreate {
        path: shown(&dest),
        detail: e.to_string(),
    })?;
    if setup {
        probe_links(&dest)?;
    }
    if let Some((mut vol, table)) = opened {
        emit(Event::Log(format!(
            "Detected {} ({})",
            vol.disc.name, vol.disc.game_id
        )));
        if plan.verify {
            emit(Event::Stage(Step::Verify));
            let total = (table.len() - 1) as u64 * 0x40000;
            disc::verify_blocks(
                plan.iso.as_ref().unwrap(),
                vol.disc,
                &table,
                |d| {
                    emit(Event::Progress(d, total));
                },
                || check(cancel),
            )?;
        }
        ensure!(
            free_space(&dest).is_none_or(|f| f > vol.size + HEADROOM),
            E::NoSpace
        );
        extract(&mut vol, &dest, cancel, emit)?;
    }
    if setup {
        let kit = kit.as_ref().unwrap();
        let edition = edition_of(id).unwrap();
        emit(Event::Stage(Step::Setup));
        unpack_kit(kit, &dest, emit)?;
        // a 2017-18 install from before app\ under the new kit: its old top folders go into app\
        forza::migrate::run(&dest)?;
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
                        let sheets = forza::kit::Kit::at(&dest).sheets();
                        forza::text::run(edition, &g, &backup, &sheets, Mode::On, Lang::En, say)
                    }
                    None => 2,
                }
            })?;
        }
        pictures(&dest, edition, &folder, emit);
        if plan.shortcuts {
            shortcuts(&dest, edition, emit);
        }
    } else {
        emit(Event::Log("Extraction only: not ready to play.".into()));
    }
    Ok(())
}

/// The kit links the game's files with hard links and junctions: the drive must be NTFS. Fail now, not after 4 GB.
fn probe_links(dest: &Path) -> Result<()> {
    let (a, b) = (dest.join(".probe-a"), dest.join(".probe-b"));
    let _ = (fs::remove_file(&a), fs::remove_file(&b));
    fs::write(&a, b"x")?;
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

/// A name from the disc must stay inside the destination.
fn safe(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', ':']) && name != "." && name != ".."
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
            let root = dir.as_os_str().is_empty();
            if !safe(&name)
                || (root && (name.starts_with('$') || name == "System Volume Information"))
            {
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

/// Extract into <dest>/game.partial, then rename: a cancelled run never leaves a half game folder.
fn extract(
    vol: &mut disc::Volume,
    dest: &Path,
    cancel: &AtomicBool,
    emit: &dyn Fn(Event),
) -> Result<()> {
    emit(Event::Stage(Step::Extract));
    emit(Event::Progress(0, 0));
    let ntfs = Ntfs::new(vol).map_err(|_| E::NotNtfsVolume)?;
    let items = scan(&ntfs, vol)?;
    let total: u64 = items.iter().map(|i| i.size).sum();
    let (game, partial) = (
        dest.join(GAME_DIR),
        dest.join(format!("{GAME_DIR}.partial")),
    );
    if partial.exists() {
        fs::remove_dir_all(&partial)?;
    }
    fs::create_dir_all(&partial)?;
    emit(Event::Log(format!(
        "Copying {} game files from the disc",
        items.len()
    )));
    let (mut buf, mut done) = (vec![0u8; 1 << 20], 0u64);
    for it in &items {
        check(cancel)?;
        let path = partial.join(&it.rel);
        if it.dir {
            fs::create_dir_all(&path)?;
            continue;
        }
        let mut out = File::create(&path).map_err(|e| E::CannotWrite {
            path: shown(&path),
            detail: e.to_string(),
        })?;
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
                out.write_all(&buf[..n])?;
                done += n as u64;
                emit(Event::Progress(done, total));
            }
        }
    }
    ensure!(
        partial.join("client_Release.exe").is_file(),
        E::ExtractIncomplete
    );
    check(cancel)?;
    ensure!(!game.exists(), E::GameFolderExists);
    fs::rename(&partial, &game)?;
    Ok(())
}

/// Unpack the kit zip into dest (Launcher.exe, README, python, scripts ...); an existing data\ (club, keys) is kept.
fn unpack_kit(zip_path: &Path, dest: &Path, emit: &dyn Fn(Event)) -> Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(zip_path)?).map_err(|_| E::NotZip)?;
    let (_, prefix) = kit_prefix(&mut zip)?;
    let keep_data = dest.join("data").exists();
    let total: u64 = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.size()))
        .sum();
    let mut done = 0;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        // enclosed_name rejects absolute paths and ".." (a crafted zip must stay inside dest)
        let name = f.enclosed_name().ok_or(E::KitUnsafePath)?;
        let Ok(rel) = name.strip_prefix(prefix.trim_end_matches('/')) else {
            continue;
        };
        if rel.as_os_str().is_empty() || (keep_data && rel.starts_with("data")) {
            continue;
        }
        let out = dest.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        done += std::io::copy(
            &mut f,
            &mut File::create(&out).map_err(|e| E::CannotWrite {
                path: shown(&out),
                detail: e.to_string(),
            })?,
        )?;
        emit(Event::Progress(done, total));
    }
    Ok(())
}

/// One of the kit's steps (setup, the game's text) in this process, its lines into the log; a failure is its FAIL
/// line.  It cannot be cancelled half way: the cancel is looked at between steps.
fn kit_step(emit: &dyn Fn(Event), step: impl FnOnce(&dyn Fn(&str)) -> i32) -> Result<()> {
    let failed = std::cell::RefCell::new(None);
    let say = |line: &str| {
        if line.contains("FAIL") {
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
fn pictures(dest: &Path, edition: Edition, folder: &str, emit: &dyn Fn(Event)) {
    let say = |l: &str| emit(Event::Log(l.to_owned()));
    let Some(game) = forza::setup::find_edition_game(edition, folder) else {
        return say("Card pictures failed: game folder not found. The board shows placeholders.");
    };
    let ed = if edition == Edition::E1011 {
        "1011"
    } else {
        "1718"
    };
    let cards = forza::kit::app_dir(dest)
        .join(edition.layout().cache)
        .join("cards");
    if let Err(e) = forza::cards::pictures::ensure(&game, ed, &cards, None, &say) {
        say(&format!(
            "Card pictures failed: {e}. The board shows placeholders."
        ));
    }
}

fn shortcuts(dest: &Path, edition: Edition, emit: &dyn Fn(Event)) {
    if !cfg!(windows) {
        return;
    }
    let q = |p: PathBuf| p.display().to_string().replace('\'', "''");
    let ps = format!(
        "$d=[Environment]::GetFolderPath('Desktop');$w=New-Object -ComObject WScript.Shell;\
         $s=$w.CreateShortcut(\"$d\\Forza-WCCF {}.lnk\");\
         $s.TargetPath='{}\\Launcher.exe';$s.WorkingDirectory='{}';$s.Save()",
        edition.label(),
        q(dest.into()),
        q(dest.into())
    );
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-Command", &ps]);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000);
    match cmd.status() {
        Ok(s) if s.success() => emit(Event::Log("Desktop shortcut created.".into())),
        _ => emit(Event::Log(
            "Could not create the desktop shortcut. Use Launcher.exe in the install folder.".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picked_folder_gets_its_own_forza_wccf_folder() {
        let t = std::env::temp_dir().join(format!("install-folder-test-{}", std::process::id()));
        fs::create_dir_all(t.join("old/2017-2018")).unwrap();
        assert_eq!(install_folder(&t), t.join("Forza-WCCF")); // a plain folder (or a drive root) gets its own
        assert_eq!(install_folder(&t.join("forza-wccf")), t.join("forza-wccf"));
        assert_eq!(install_folder(&t.join("old")), t.join("old")); // an existing install stays where it is
        fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn pictures_step_runs_ensure_and_survives_failure() {
        let dir = std::env::temp_dir().join(format!("pictures-step-test-{}", std::process::id()));
        let game = dir.join("game");
        fs::create_dir_all(&game).unwrap();
        let lines = std::cell::RefCell::new(Vec::new());
        let emit = |e: Event| {
            if let Event::Log(l) = e {
                lines.borrow_mut().push(l);
            }
        };
        // no game files: the step reports and returns instead of failing the install
        pictures(&dir, Edition::E1718, &game.to_string_lossy(), &emit);
        assert!(
            lines
                .borrow()
                .iter()
                .any(|l| l.contains("Card pictures failed"))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn kit_is_optional_and_never_applied_to_sddy() {
        let root = std::env::temp_dir().join(format!("optional-kit-test-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let (edition_root, game) = version_paths(&root, "SDDY").unwrap();
        fs::create_dir_all(game.join("local")).unwrap();
        fs::write(game.join("client_Release.exe"), b"unchanged").unwrap();
        fs::write(game.join("local/SDDY_Table.dat"), b"SDDY").unwrap();
        fs::write(
            edition_root.join(".probe-a"),
            b"do not touch on extraction-only installs",
        )
        .unwrap();
        let mut plan = Plan {
            dest: root.clone(),
            iso: None,
            key: None,
            game_id: Some("SDDY"),
            verify: true,
            setup: true,
            english: false,
            shortcuts: true,
            kit: None,
            latest: false,
        };
        let cancel = AtomicBool::new(false);
        run(&plan, &cancel, &|_| {}).unwrap(); // no kit, even with setup's default true
        assert_eq!(GAME_DIR, "game");
        assert!(game_present(&root, "SDDY"));
        assert_eq!(installed_game_id(&root), Some("SDDY"));
        let (old_root, old_game) = version_paths(&root, "SBWG").unwrap();
        fs::create_dir_all(old_game.join("local")).unwrap();
        fs::write(
            old_game.join("client_Release.exe"),
            b"old edition unchanged",
        )
        .unwrap();
        fs::write(old_game.join("local/SBWG_Table.dat"), b"SBWG").unwrap();
        for (dir, bytes) in [
            (&old_root, b"old save".as_slice()),
            (&edition_root, b"new save".as_slice()),
        ] {
            fs::create_dir_all(dir.join("data/save")).unwrap();
            fs::write(dir.join("data/save/club.bin"), bytes).unwrap();
        }
        assert_ne!(old_root, edition_root);
        assert!(game_present(&root, "SBWG"));
        assert_eq!(installed_game_id(&root), None); // never choose an edition silently
        plan.game_id = None;
        assert!(
            run(&plan, &cancel, &|_| {})
                .unwrap_err()
                .to_string()
                .contains("--game")
        );
        plan.game_id = Some("SBWG");
        run(&plan, &cancel, &|_| {}).unwrap();
        plan.game_id = Some("SDDY");
        run(&plan, &cancel, &|_| {}).unwrap();
        assert_eq!(
            fs::read(old_game.join("client_Release.exe")).unwrap(),
            b"old edition unchanged"
        );
        assert_eq!(
            fs::read(old_root.join("data/save/club.bin")).unwrap(),
            b"old save"
        );
        assert_eq!(
            fs::read(edition_root.join("data/save/club.bin")).unwrap(),
            b"new save"
        );

        let kit = root.join("kit.zip");
        let mut zip = zip::ZipWriter::new(File::create(&kit).unwrap());
        for n in ["Launcher.exe", "programs/wccfk.dll"] {
            zip.start_file(n, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"not executed").unwrap();
        }
        zip.finish().unwrap();
        plan.kit = Some(kit);
        assert!(
            run(&plan, &cancel, &|_| {})
                .unwrap_err()
                .to_string()
                .contains("is for 2010–11")
        );
        assert!(!edition_root.join("scripts").exists());
        plan.setup = false;
        run(&plan, &cancel, &|_| {}).unwrap(); // --no-setup ignores a supplied kit
        plan.english = true;
        assert!(run(&plan, &cancel, &|_| {}).is_err());
        plan.english = false;
        plan.iso = Some(root.join("not-read.iso"));
        plan.key = Some(root.join("not-read.key"));
        assert!(
            run(&plan, &cancel, &|_| {})
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        assert!(!edition_root.join(format!("{GAME_DIR}.partial")).exists());
        assert_eq!(
            fs::read(game.join("client_Release.exe")).unwrap(),
            b"unchanged"
        );
        assert_eq!(
            fs::read(edition_root.join(".probe-a")).unwrap(),
            b"do not touch on extraction-only installs"
        );
        // The old flat install remains usable; installing the other edition gets its own root.
        let legacy = root.join(LEGACY_GAME_DIR);
        fs::rename(&game, &legacy).unwrap();
        fs::remove_dir_all(&edition_root).unwrap();
        assert_eq!(
            version_paths(&root, "SDDY").unwrap(),
            (root.clone(), legacy.clone())
        );
        assert!(game_present(&root, "SDDY"));
        plan.iso = None;
        plan.key = None;
        plan.kit = None;
        run(&plan, &cancel, &|_| {}).unwrap();
        assert_eq!(version_paths(&root, "SBWG").unwrap().0, old_root);
        assert_eq!(
            fs::read(legacy.join("client_Release.exe")).unwrap(),
            b"unchanged"
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn kit_zip(dir: &Path, name: &str, files: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        for n in files {
            zip.start_file(*n, zip::write::SimpleFileOptions::default())
                .unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn kit_zip_edition_and_prefix() {
        let dir = std::env::temp_dir().join(format!("kit-zip-test-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let prefix = |files: &[&str]| {
            let p = kit_zip(&dir, "k.zip", files);
            kit_prefix(&mut zip::ZipArchive::new(File::open(p).unwrap()).unwrap())
                .map_err(|e| e.to_string())
        };
        let a = ["Launcher.exe", "programs/wccfk.dll"];
        let b = ["Launcher.exe", "bin/x.dll"];
        assert_eq!(prefix(&a), Ok((Edition::E1011, String::new())));
        assert_eq!(prefix(&b), Ok((Edition::E1718, String::new())));
        assert_eq!(
            prefix(&["K/Launcher.exe", "K/programs/wccfk.dll"]),
            Ok((Edition::E1011, "K/".into()))
        );
        assert_eq!(
            prefix(&["K/Launcher.exe", "K/overlay/forza_panel.dll"]),
            Ok((Edition::E1718, "K/".into()))
        );
        assert_eq!(
            prefix(&["K/Launcher.exe", "K/app/bin/wccfnet.dll"]),
            Ok((Edition::E1718, "K/".into())),
            "the 2017-18 kit with its programs in app/"
        );
        // an old kit with only scripts/, a missing Launcher.exe and a foreign zip are not kits
        for files in [
            &["Launcher.exe", "scripts/setup.py"][..],
            &["programs/wccfk.dll"],
            &["Launcher.exe", "xprograms/wccfk.dll"],
            &["programs/wccfk.dll", "Launcher.exe/child"],
            &["K/Launcher.exe", "programs/wccfk.dll"],
            &["readme.txt"],
        ] {
            assert!(
                prefix(files)
                    .unwrap_err()
                    .contains("not a Forza-WCCF package"),
                "{files:?}"
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn kit_edition_must_match_the_game() {
        assert!(check_setup(Some("SBWG"), Edition::E1011).is_ok());
        assert!(check_setup(Some("SDDY"), Edition::E1718).is_ok());
        assert!(check_setup(Some("SDDY"), Edition::E1011).is_err());
        assert!(check_setup(Some("SBWG"), Edition::E1718).is_err());
        assert!(check_setup(None, Edition::E1011).is_err());
    }
}
