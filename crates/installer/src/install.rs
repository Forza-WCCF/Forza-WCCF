//! The install pipeline: verify disc -> extract game -> optional kit setup (English, shortcuts).
//! Runs on a worker thread; reports through `emit`, stops when `cancel` is set.
use crate::disc;
use anyhow::{Context, Result, bail, ensure};
use ntfs::{Ntfs, NtfsReadSeek, structured_values::NtfsFileNamespace};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
}

pub enum Event {
    Stage(String),
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
        .context("Unknown game edition.")?;
    let root = dest.join(disc.edition);
    let legacy = dest.join(LEGACY_GAME_DIR);
    if !root.exists() && game_id_in(&legacy) == Some(id) {
        return Ok((dest.to_path_buf(), legacy)); // preserve older installs in place
    }
    let game = root.join(GAME_DIR);
    Ok((root, game))
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

pub fn check_setup(game_id: Option<&str>) -> Result<()> {
    ensure!(
        game_id == Some("SBWG"),
        "The 2010–2011 kit does not support this game version. Leave the kit blank or turn off setup to extract only."
    );
    Ok(())
}

/// The kit zip may hold the files directly or inside one top folder (WCCF-2010-11-kit/...).
fn kit_prefix(zip: &mut zip::ZipArchive<File>) -> Result<String> {
    let names = zip
        .file_names()
        .map(|n| n.map(|n| n.into_owned()))
        .collect::<Result<Vec<String>, _>>()
        .context("That zip has an unreadable file name.")?;
    let setup = names
        .iter()
        .find(|n| *n == "scripts/setup.py" || n.ends_with("/scripts/setup.py"));
    let setup = setup.context("That zip is not the WCCF kit (scripts/setup.py is missing).")?;
    Ok(setup.trim_end_matches("scripts/setup.py").to_string())
}

pub fn check_kit_zip(path: &Path) -> Result<()> {
    let f = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(f).context("That file is not a valid zip.")?;
    kit_prefix(&mut zip).map(drop)
}

pub fn free_space(path: &Path) -> Option<u64> {
    path.ancestors()
        .find(|p| p.exists())
        .and_then(|p| fs4::available_space(p).ok())
}

fn check(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "Cancelled.");
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
        "The selected edition does not match the disc image."
    );
    let id = detected.or(plan.game_id).or_else(|| installed_game_id(&plan.dest))
        .context("Choose --game SBWG (2010–2011) or --game SDDY (2017–2018) when both or neither edition is installed.")?;
    let (dest, game) = version_paths(&plan.dest, id)?;
    ensure!(
        plan.iso.is_none() || !game.exists(),
        "The game folder already exists. Reuse it without --iso/--key, or choose a new install folder; existing files are never replaced."
    );
    let setup = plan.setup && plan.kit.is_some();
    ensure!(
        !plan.english || setup,
        "English requires a kit and setup; omit --english for extraction only."
    );
    let opened = if let Some(iso) = &plan.iso {
        let key = fs::read(plan.key.as_ref().context("no key file")?)
            .context("cannot read the key file")?;
        Some(disc::open(iso, &key)?)
    } else {
        ensure!(
            game_present(&plan.dest, id),
            "The game files are not in {}",
            plan.dest.display()
        );
        None
    };
    if setup {
        check_setup(Some(id))?;
        check_kit_zip(plan.kit.as_ref().unwrap())?;
    }
    fs::create_dir_all(&dest).with_context(|| format!("cannot create {}", dest.display()))?;
    if setup {
        probe_links(&dest)?;
    }
    if let Some((mut vol, table)) = opened {
        emit(Event::Log(format!(
            "Detected {} ({})",
            vol.disc.name, vol.disc.game_id
        )));
        if plan.verify {
            emit(Event::Stage("Checking the disc image".into()));
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
            "Not enough free space on that drive: allow about 6 GB."
        );
        extract(&mut vol, &dest, cancel, emit)?;
    }
    if setup {
        let kit = plan.kit.as_ref().unwrap();
        emit(Event::Stage("Unpacking the kit".into()));
        unpack_kit(kit, &dest, emit)?;
        emit(Event::Stage("Setting up the game for this PC".into()));
        check(cancel)?;
        script(&dest, "setup.py", Some(&game), emit, cancel)?;
        if plan.english {
            emit(Event::Stage("Switching the game to English".into()));
            script(&dest, "english.py", None, emit, cancel)?;
        }
        if plan.shortcuts {
            let edition = disc::DISCS
                .iter()
                .find(|d| d.game_id == id)
                .unwrap()
                .edition;
            shortcuts(&dest, edition, emit);
        }
    } else {
        emit(Event::Log("Extraction only: no kit, English patches or shortcuts applied. These files are not a ready-to-play runtime.".into()));
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
    linked.context(
        "This drive cannot hold the game: it must be formatted as NTFS (normal for C: and D:). \
        Pick a folder on another drive.",
    )
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
    emit(Event::Stage("Reading the game's file list".into()));
    emit(Event::Progress(0, 0));
    let ntfs = Ntfs::new(vol).context("the decrypted volume is not NTFS - wrong key file?")?;
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
    emit(Event::Stage(format!(
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
        let mut out =
            File::create(&path).with_context(|| format!("cannot write {}", path.display()))?;
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
        "extraction finished but client_Release.exe is missing"
    );
    check(cancel)?;
    ensure!(
        !game.exists(),
        "The game folder already exists; existing files are never replaced."
    );
    fs::rename(&partial, &game)?;
    Ok(())
}

/// Unpack the kit zip into dest (README, PLAY.bat, python, scripts ...); an existing data\ (club, keys) is kept.
fn unpack_kit(zip_path: &Path, dest: &Path, emit: &dyn Fn(Event)) -> Result<()> {
    let mut zip =
        zip::ZipArchive::new(File::open(zip_path)?).context("The kit is not a valid zip.")?;
    let prefix = kit_prefix(&mut zip)?;
    let keep_data = dest.join("data").exists();
    let total: u64 = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.size()))
        .sum();
    let mut done = 0;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        // enclosed_name rejects absolute paths and ".." (a crafted zip must stay inside dest)
        let name = f
            .enclosed_name()
            .context("the kit zip contains an unsafe path")?;
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
            &mut File::create(&out).with_context(|| format!("cannot write {}", out.display()))?,
        )?;
        emit(Event::Progress(done, total));
    }
    Ok(())
}

/// Run one of the kit's scripts with its bundled Python and stream its output into the log.
fn script(
    dest: &Path,
    name: &str,
    arg: Option<&Path>,
    emit: &dyn Fn(Event),
    cancel: &AtomicBool,
) -> Result<()> {
    let python = dest.join("python/python.exe");
    if !cfg!(windows) || !python.is_file() {
        bail!("The kit's setup runs on Windows only (python\\python.exe was not found).");
    }
    let mut cmd = Command::new(python);
    cmd.arg(dest.join("scripts").join(name))
        .args(arg)
        .current_dir(dest)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn()?;
    let err = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        BufReader::new(err)
            .lines()
            .map_while(Result::ok)
            .for_each(|l| drop(tx2.send(l)))
    });
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        BufReader::new(out)
            .lines()
            .map_while(Result::ok)
            .for_each(|l| drop(tx.send(l)))
    });
    emit(Event::Progress(0, 0));
    let mut failed = None;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(line) => {
                if line.contains("FAIL") {
                    failed = Some(line.trim().to_string());
                }
                emit(Event::Log(line));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(_) => {}
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            bail!("Cancelled.");
        }
    }
    let status = child.wait()?;
    ensure!(
        status.success(),
        "{}",
        failed.unwrap_or_else(|| format!("{name} stopped with {status}"))
    );
    Ok(())
}

fn shortcuts(dest: &Path, edition: &str, emit: &dyn Fn(Event)) {
    if !cfg!(windows) {
        return;
    }
    let q = |p: PathBuf| p.display().to_string().replace('\'', "''");
    let ps = format!(
        "$d=[Environment]::GetFolderPath('Desktop');$w=New-Object -ComObject WScript.Shell;\
         foreach($n in 'PLAY','STOP'){{$s=$w.CreateShortcut(\"$d\\Forza-WCCF {edition} - $n.lnk\");\
         $s.TargetPath='{}\\'+$n+'.bat';$s.WorkingDirectory='{}';$s.Save()}}",
        q(dest.into()),
        q(dest.into())
    );
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-Command", &ps]);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000);
    match cmd.status() {
        Ok(s) if s.success() => emit(Event::Log("Desktop shortcuts created.".into())),
        _ => emit(Event::Log(
            "Could not create the desktop shortcuts (PLAY.bat in the install folder still works)."
                .into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let cancel = AtomicBool::new(false);
        run(&plan, &cancel, &|_| {}).unwrap(); // no kit, even with setup's default true
        assert!(check_setup(Some("SBWG")).is_ok());
        assert!(check_setup(None).is_err());
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
        zip.start_file("scripts/setup.py", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"not executed").unwrap();
        zip.finish().unwrap();
        plan.kit = Some(kit);
        assert!(
            run(&plan, &cancel, &|_| {})
                .unwrap_err()
                .to_string()
                .contains("does not support")
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
}
