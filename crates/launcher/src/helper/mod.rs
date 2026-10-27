//! What the in-game panels ask for and cannot do from inside the game (panel.py, sound.py and console.py's place; the
//! rest of panel.py, RESTART NOW and SAVE LOGS, is the run's own).  The run holds a [`Helper`] while the game is up;
//! dropping it stops its threads.
//!
//! THE CLUB CARD view (2026-10-06): data\club_view.txt is rewritten whenever the card, its backups, the other cards or
//! the run change, once the set has held still for a second (the game's bursts of saves are never read half way).  The
//! view itself is `forza::cards::steps::view`: the `build` the run gives [`Helper::club_view`] writes it.  A view that cannot be written is
//! said once and tried again every 5 s; it never stops the run.
//!
//! THE STARTING VOLUMES (2026-10-06): the projector muted and seat 1 very low ([`sound::GAME_VOLUME`]), each set once
//! per process as soon as it has opened its sound (looked for every 0.5 s, for 10 minutes after the process is first
//! seen); never again, so a change in the Volume Mixer stays.  The processes are found by their ids.
//!
//! A helper thread that ends before the run does (a panic, say) is noticed by [`Helper::died`]; neither helper is
//! essential, so the supervisor says it and the game goes on.

pub mod console;
pub use forza::sound;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use forza::standins::Log;

const TICK: Duration = Duration::from_millis(500);
/// The card and its backups hold still this long before the view is read.
const SETTLE: Duration = Duration::from_secs(1);
/// A view that failed is tried again after this.
const RETRY: Duration = Duration::from_secs(5);
/// How long after a process is first seen its sound is waited for.
const WAIT_FOR_SOUND: Duration = Duration::from_secs(600);

type Stamp = (SystemTime, u64);

fn stamp(p: &Path) -> Option<Stamp> {
    fs::metadata(p).and_then(|m| Ok((m.modified()?, m.len()))).ok()
}

fn listing(dir: &Path) -> Vec<(OsString, Option<Stamp>)> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| (e.file_name(), stamp(&e.path())))
        .collect();
    names.sort();
    names
}

/// What the view is made of, as the files look now.
#[derive(PartialEq, Clone)]
struct Signature {
    card: Option<Stamp>,
    backups: Vec<(OsString, Option<Stamp>)>,
    running: Option<Stamp>,
    cards: Vec<(OsString, Option<Stamp>)>,
}

/// The files the club card view follows (the kit's `data`).
pub struct ViewFiles {
    card: PathBuf,
    backups: PathBuf,
    cards: PathBuf,
    running: PathBuf,
}

impl ViewFiles {
    pub fn new(data: &Path) -> Self {
        let save = data.join("save");
        Self {
            card: save.join("seat1_club.bin"),
            backups: save.join("backup"),
            cards: save.join("cards"),
            running: data.join("running.json"),
        }
    }

    fn signature(&self) -> Signature {
        Signature {
            card: stamp(&self.card),
            backups: listing(&self.backups),
            running: stamp(&self.running),
            cards: listing(&self.cards),
        }
    }
}

struct ClubView {
    files: ViewFiles,
    seen: Option<Signature>,
    since: Instant,
    done: Option<Signature>,
    retry_at: Instant,
    said: bool,
}

impl ClubView {
    fn new(files: ViewFiles, now: Instant) -> Self {
        Self {
            files,
            seen: None,
            since: now,
            done: None,
            retry_at: now,
            said: false,
        }
    }

    /// One look: a change starts the wait; once the files held still for a second, `build` writes the view (its first
    /// line is said); a failed build is tried again in 5 s and said once.
    fn tick(&mut self, now: Instant, build: &mut dyn FnMut() -> Result<String, String>, say: &dyn Fn(&str)) {
        let sig = self.files.signature();
        if self.seen.as_ref() != Some(&sig) {
            (self.seen, self.since) = (Some(sig), now);
            return;
        }
        if self.done.as_ref() == Some(&sig) || now - self.since < SETTLE || now < self.retry_at {
            return;
        }
        match build() {
            Ok(first) => {
                self.done = Some(sig); // only once written: a failed write is tried again
                say(&format!("club card view written: {first}"));
            }
            Err(why) => {
                self.retry_at = now + RETRY;
                if !self.said {
                    say(&format!(
                        "club card view could not be written (tried again every 5 s): {why}"
                    ));
                    self.said = true;
                }
            }
        }
    }
}

/// A process whose sound gets a start volume: found by its id, which `pid` gives once the process runs.
pub struct Target {
    pub name: &'static str,
    pub pid: Box<dyn Fn() -> Option<u32> + Send>,
    /// Muted too (the projector); else only the volume.
    pub mute: bool,
}

struct Slot {
    target: Target,
    seen: Option<(u32, Instant)>,
    done: Option<u32>,
}

/// Sets a (process id, volume, mute) and says how many sessions it found.
pub type Apply = Box<dyn Fn(u32, f32, bool) -> Result<usize, String> + Send>;

struct Volumes {
    slots: Vec<Slot>,
    apply: Apply,
    next: Instant,
    said: bool,
}

impl Volumes {
    fn new(targets: Vec<Target>, apply: Apply, now: Instant) -> Self {
        let slots = targets
            .into_iter()
            .map(|target| Slot {
                target,
                seen: None,
                done: None,
            })
            .collect();
        Self {
            slots,
            apply,
            next: now,
            said: false,
        }
    }

    fn tick(&mut self, now: Instant, say: &dyn Fn(&str)) {
        if now < self.next {
            return;
        }
        self.next = now + TICK;
        for s in &mut self.slots {
            let Some(pid) = (s.target.pid)() else { continue };
            if s.seen.map(|(p, _)| p) != Some(pid) {
                s.seen = Some((pid, now)); // a new process (a restarted cabinet) is set again
            }
            if s.done == Some(pid) || now - s.seen.map_or(now, |(_, t)| t) > WAIT_FOR_SOUND {
                continue;
            }
            match (self.apply)(pid, sound::GAME_VOLUME, s.target.mute) {
                Ok(0) => {}
                Ok(_) => {
                    s.done = Some(pid);
                    let what = if s.target.mute {
                        "starts muted".to_owned()
                    } else {
                        format!("starts at {}%", (sound::GAME_VOLUME * 100.0).round())
                    };
                    say(&format!("sound: {} {what} (pid {pid})", s.target.name));
                }
                // the volumes never stop the run: the game plays as Windows has it
                Err(why) if !self.said => {
                    self.said = true;
                    say(&format!("sound: could not set the starting volumes ({why}); retrying"));
                }
                Err(_) => {}
            }
        }
    }
}

/// The run's helper threads; dropping it stops and joins them.
#[derive(Default)]
pub struct Helper {
    stop: Arc<AtomicBool>,
    threads: Vec<(&'static str, Option<JoinHandle<()>>)>,
}

/// Waits `TICK` in short slices; false once `stop` is set.
fn nap(stop: &AtomicBool) -> bool {
    for _ in 0..5 {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        thread::sleep(TICK / 5);
    }
    !stop.load(Ordering::Relaxed)
}

impl Helper {
    /// Follows the club card's files and calls `build` (the view written) when they have changed and held still.
    pub fn club_view(
        &mut self,
        files: ViewFiles,
        mut build: impl FnMut() -> Result<String, String> + Send + 'static,
        say: Log,
    ) {
        let stop = self.stop.clone();
        let t = thread::spawn(move || {
            let mut view = ClubView::new(files, Instant::now());
            while !stop.load(Ordering::Relaxed) {
                view.tick(Instant::now(), &mut build, &*say);
                if !nap(&stop) {
                    return;
                }
            }
        });
        self.threads.push(("club card view", Some(t)));
    }

    /// Sets each target's starting volume once its process has opened its sound.
    pub fn start_volumes(&mut self, targets: Vec<Target>, apply: Apply, say: Log) {
        let stop = self.stop.clone();
        let t = thread::spawn(move || {
            let mut volumes = Volumes::new(targets, apply, Instant::now());
            while !stop.load(Ordering::Relaxed) {
                volumes.tick(Instant::now(), &*say);
                if !nap(&stop) {
                    return;
                }
            }
        });
        self.threads.push(("starting volumes", Some(t)));
    }

    /// The core audio call the run uses: volume and mute of every session of the process.
    pub fn core_audio() -> Apply {
        Box::new(|pid, volume, mute| sound::apply(pid, Some(volume), Some(mute)).map_err(|e| e.to_string()))
    }

    /// Threads that have ended before the helper was dropped, once each, with how: a panic is a bug, said.
    pub fn died(&mut self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for (name, slot) in &mut self.threads {
            if slot.as_ref().is_some_and(JoinHandle::is_finished)
                && let Some(t) = slot.take()
            {
                out.push((*name, panic_text(t.join())));
            }
        }
        out
    }
}

/// How a joined thread ended.
pub fn panic_text(r: thread::Result<()>) -> String {
    match r {
        Ok(()) => "its thread returned".to_owned(),
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            format!("its thread panicked: {msg}")
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for (_, t) in &mut self.threads {
            if let Some(t) = t.take() {
                let _ = t.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::sync::Mutex;

    /// The view follows its files on a clock the test moves.
    #[test]
    fn the_view_waits_for_the_files_to_hold_still_and_retries_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        fs::create_dir_all(data.join("save")).unwrap();
        let card = data.join("save").join("seat1_club.bin");
        fs::write(&card, b"one").unwrap();
        let t0 = Instant::now();
        let mut view = ClubView::new(ViewFiles::new(data), t0);
        let said = RefCell::new(Vec::new());
        let say = |l: &str| said.borrow_mut().push(l.to_owned());
        let builds = Cell::new(0);
        let fail = Cell::new(false);
        let mut build = || {
            builds.set(builds.get() + 1);
            if fail.get() {
                Err("no catalogue".to_owned())
            } else {
                Ok("CLUB".to_owned())
            }
        };
        let at = |s: f32| t0 + Duration::from_secs_f32(s);
        view.tick(at(0.0), &mut build, &say); // first look: a change
        view.tick(at(0.5), &mut build, &say); // held 0.5 s
        assert_eq!(builds.get(), 0);
        view.tick(at(1.1), &mut build, &say);
        assert_eq!(builds.get(), 1);
        assert_eq!(*said.borrow(), ["club card view written: CLUB"]);
        view.tick(at(3.0), &mut build, &say); // nothing changed: no new build
        assert_eq!(builds.get(), 1);
        // a burst of saves: each change restarts the wait
        fs::write(&card, b"two!").unwrap();
        fail.set(true);
        view.tick(at(4.0), &mut build, &say);
        view.tick(at(4.5), &mut build, &say);
        assert_eq!(builds.get(), 1);
        view.tick(at(5.1), &mut build, &say); // fails: said once
        view.tick(at(6.0), &mut build, &say); // within the 5 s: not tried
        assert_eq!(builds.get(), 2);
        view.tick(at(10.2), &mut build, &say); // tried again, fails again, not said again
        assert_eq!(builds.get(), 3);
        assert_eq!(said.borrow().len(), 2, "{:?}", said.borrow());
        fail.set(false);
        view.tick(at(15.3), &mut build, &say);
        assert_eq!(builds.get(), 4);
        assert_eq!(said.borrow().last().unwrap(), "club card view written: CLUB");
    }

    /// The projector is muted and seat 1 set once each; a process that has no sound yet is asked again; a new process
    /// (a restarted cabinet) is set again; an error is said once.
    #[test]
    fn the_volumes_are_set_once_per_process_when_its_sound_opens() {
        let pids = Arc::new(Mutex::new((None::<u32>, None::<u32>)));
        let calls = Arc::new(Mutex::new(Vec::<(u32, bool)>::new()));
        let (open, broken) = (
            Arc::new(Mutex::new(Vec::<u32>::new())),
            Arc::new(AtomicBool::new(false)),
        );
        let p = pids.clone();
        let q = pids.clone();
        let targets = vec![
            Target {
                name: "the projector",
                pid: Box::new(move || p.lock().unwrap().0),
                mute: true,
            },
            Target {
                name: "seat 1",
                pid: Box::new(move || q.lock().unwrap().1),
                mute: false,
            },
        ];
        let (c, o, b) = (calls.clone(), open.clone(), broken.clone());
        let apply: Apply = Box::new(move |pid, volume, mute| {
            assert_eq!(volume, sound::GAME_VOLUME);
            if b.load(Ordering::Relaxed) {
                return Err("no audio device".into());
            }
            c.lock().unwrap().push((pid, mute));
            Ok(usize::from(o.lock().unwrap().contains(&pid)))
        });
        let t0 = Instant::now();
        let mut v = Volumes::new(targets, apply, t0);
        let said = RefCell::new(Vec::new());
        let say = |l: &str| said.borrow_mut().push(l.to_owned());
        let at = |s: u64| t0 + Duration::from_secs(s);
        v.tick(at(0), &say); // no process yet: nothing asked
        assert!(calls.lock().unwrap().is_empty());
        *pids.lock().unwrap() = (Some(10), Some(20));
        v.tick(at(1), &say); // processes, no sound yet
        assert_eq!(*calls.lock().unwrap(), [(10, true), (20, false)]);
        *open.lock().unwrap() = vec![10, 20];
        v.tick(at(2), &say);
        v.tick(at(3), &say); // done: not asked again
        assert_eq!(calls.lock().unwrap().len(), 4);
        assert_eq!(said.borrow().len(), 2, "{:?}", said.borrow());
        assert!(said.borrow()[0].starts_with("sound: the projector starts muted (pid 10)"));
        assert!(said.borrow()[1].starts_with("sound: seat 1 starts at 4% (pid 20)"));
        // the cabinet restarts: a new process
        open.lock().unwrap().push(21);
        pids.lock().unwrap().1 = Some(21);
        v.tick(at(4), &say);
        assert_eq!(calls.lock().unwrap().last(), Some(&(21, false)));
        assert!(said.borrow()[2].contains("seat 1 starts at 4% (pid 21)"));
        // errors are said once; after the wait for a process's sound it is given up
        broken.store(true, Ordering::Relaxed);
        pids.lock().unwrap().1 = Some(22);
        v.tick(at(5), &say);
        v.tick(at(6), &say);
        assert_eq!(said.borrow().iter().filter(|l| l.contains("could not set")).count(), 1);
        broken.store(false, Ordering::Relaxed);
        let n = calls.lock().unwrap().len();
        v.tick(at(5 + 601), &say);
        assert_eq!(calls.lock().unwrap().len(), n, "pid 22 was given up after 10 minutes");
    }

    /// A helper thread that panics is noticed once, with its message; dropping the helper joins the others.
    #[test]
    fn a_helper_thread_that_panics_is_noticed() {
        let mut h = Helper::default();
        h.threads.push(("bad", Some(thread::spawn(|| panic!("boom")))));
        let t0 = Instant::now();
        let mut found = Vec::new();
        while found.is_empty() && t0.elapsed() < Duration::from_secs(5) {
            found = h.died();
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(found, [("bad", "its thread panicked: boom".to_owned())]);
        assert!(h.died().is_empty(), "said once");
    }
}
