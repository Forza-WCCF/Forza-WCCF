//! The frame cost by part, for forza_panel.log: every 600 frames `frame split` (each part's average over them, and
//! how many egui passes ran) and `frame worst` (their slowest frame by part, with what it uploaded); `slow frame` at
//! once for a frame over 25 ms, so the log lines around it say what the panel was doing.

use crate::painter::Uploaded;
use forza_ui::idle::Wake;
use std::time::{Duration, Instant};

/// The parts, in frame order: the game's device state saved; the game's frame copied and our target bound; the
/// polls (files, the ping, the I/O board's lamps) and the input; the idle check ([`forza_ui::idle`]); egui's pass;
/// what the pass asked for (files written, keys sent); tessellation; texture uploads; the meshes drawn; the board
/// window's Present (popped out); the device put back.
#[derive(Clone, Copy)]
pub enum Part {
    State,
    Copy,
    Input,
    Check,
    Ui,
    Out,
    Tessellate,
    Upload,
    Draw,
    Present,
    Restore,
}

const NAMES: [&str; 11] = [
    "state",
    "copy",
    "input",
    "check",
    "ui",
    "out",
    "tessellate",
    "upload",
    "draw",
    "present",
    "restore",
];

const SLOW: Duration = Duration::from_millis(25);
/// `slow frame` lines a run writes at most.
const SLOW_LINES: u32 = 50;

#[derive(Clone, Copy, Default)]
struct Frame {
    t: [Duration; 11],
    up: Uploaded,
    passes: u32,
}

impl Frame {
    /// "state 0.12, copy 0.30, ..., other 0.01 ms" for `total` over `n` frames.
    fn parts(t: &[Duration; 11], total: Duration, n: u32) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0 / f64::from(n.max(1));
        let mut s: Vec<String> = NAMES
            .iter()
            .zip(t)
            .map(|(name, d)| format!("{name} {:.2}", ms(*d)))
            .collect();
        s.push(format!(
            "other {:.2}",
            ms(total.saturating_sub(t.iter().sum()))
        ));
        format!("{} ms", s.join(", "))
    }

    fn describe(&self, total: Duration) -> String {
        let u = &self.up;
        format!(
            "{:.2} ms - {}; egui ran {}, uploaded {} texture(s), {} bytes, largest {}x{}{}",
            total.as_secs_f64() * 1000.0,
            Frame::parts(&self.t, total, 1),
            self.passes,
            u.textures,
            u.bytes,
            u.largest[0],
            u.largest[1],
            if u.font {
                " (the font atlas among them)"
            } else {
                ""
            }
        )
    }
}

#[derive(Default)]
pub struct Split {
    now: Frame,
    since: Option<Instant>,
    sum: [Duration; 11],
    total: Duration,
    frames: u32,
    passes: u32,
    /// The window's passes by why the idle gate ran them: input, state, window, repaint.
    wakes: [u32; 4],
    /// The window's slowest frame: its time, parts and number.
    worst: Option<(Duration, Frame, u32)>,
    slow: u32,
}

impl Split {
    /// The frame's clock starts (what a frame that showed nothing marked is dropped).
    pub fn start(&mut self) {
        self.now = Frame::default();
        self.since = Some(Instant::now());
    }

    /// The time since the last mark goes to `part`.
    pub fn mark(&mut self, part: Part) {
        let now = Instant::now();
        if let (Some(s), Some(t)) = (self.since, self.now.t.get_mut(part as usize)) {
            *t += now - s;
        }
        self.since = Some(now);
    }

    /// What the idle gate said ([`forza_ui::idle::Idle::pass`]).
    pub fn woke(&mut self, why: Option<Wake>) {
        if let Some(n) = why.and_then(|w| self.wakes.get_mut(w as usize)) {
            *n += 1;
        }
    }

    /// An egui pass ran in this frame.
    pub fn passed(&mut self) {
        self.now.passes += 1;
    }

    pub fn uploaded(&mut self, u: Uploaded) {
        let a = &mut self.now.up;
        a.textures += u.textures;
        a.bytes += u.bytes;
        if u.largest[0] * u.largest[1] > a.largest[0] * a.largest[1] {
            a.largest = u.largest;
        }
        a.font |= u.font;
    }

    /// The frame `n` ended after `total`; a `slow frame` line when it took long.
    pub fn end(&mut self, total: Duration, n: u32) -> Option<String> {
        let f = std::mem::take(&mut self.now);
        self.since = None;
        for (s, t) in self.sum.iter_mut().zip(f.t) {
            *s += t;
        }
        self.total += total;
        self.frames += 1;
        self.passes += f.passes;
        if self.worst.as_ref().is_none_or(|w| total > w.0) {
            self.worst = Some((total, f, n));
        }
        (total > SLOW && self.slow < SLOW_LINES).then(|| {
            self.slow += 1;
            format!("slow frame {n}: {}", f.describe(total))
        })
    }

    /// The frames since the last call: `frame split` and `frame worst`; the window starts again.
    pub fn lines(&mut self) -> [String; 2] {
        let n = self.frames;
        let split = format!(
            "frame split: {} average over the last {n} frames; egui ran {} times (input {}, state {}, window {}, \
             repaint {})",
            Frame::parts(&self.sum, self.total, n),
            self.passes,
            self.wakes[0],
            self.wakes[1],
            self.wakes[2],
            self.wakes[3],
        );
        let worst = self.worst.take().map_or(String::new(), |(t, f, at)| {
            format!("frame worst of the last {n}: frame {at}, {}", f.describe(t))
        });
        (self.sum, self.total, self.frames, self.passes, self.wakes) = Default::default();
        [split, worst]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_split_lines_say_each_part() {
        let mut s = Split::default();
        for n in 1..=3 {
            s.start();
            s.now.t[Part::Ui as usize] = Duration::from_millis(2);
            s.now.t[Part::Upload as usize] = Duration::from_millis(if n == 2 { 30 } else { 0 });
            if n == 2 {
                s.passed();
                s.uploaded(Uploaded {
                    textures: 2,
                    bytes: 4096 * 4,
                    largest: [64, 32],
                    font: true,
                });
            }
            let total = Duration::from_millis(if n == 2 { 33 } else { 3 });
            let slow = s.end(total, n);
            assert_eq!(slow.is_some(), n == 2, "frame {n}");
        }
        let [split, worst] = s.lines();
        assert_eq!(
            split,
            "frame split: state 0.00, copy 0.00, input 0.00, check 0.00, ui 2.00, out 0.00, tessellate 0.00, \
             upload 10.00, draw 0.00, present 0.00, restore 0.00, other 1.00 ms average over the last 3 frames; \
             egui ran 1 times (input 0, state 0, window 0, repaint 0)"
        );
        assert_eq!(
            worst,
            "frame worst of the last 3: frame 2, 33.00 ms - state 0.00, copy 0.00, input 0.00, check 0.00, ui 2.00, \
             out 0.00, tessellate 0.00, upload 30.00, draw 0.00, present 0.00, restore 0.00, other 1.00 ms; egui ran \
             1, uploaded 2 texture(s), 16384 bytes, largest 64x32 (the font atlas among them)"
        );
        // the next window starts empty
        assert!(s.lines()[0].contains("over the last 0 frames; egui ran 0 times"));
    }
}
