//! Seat 1's frame pacing, measured where the game shows each frame (`hook_present`): the frames a second for the FPS
//! counter (SETTINGS > FPS COUNTER), and one line per match in `WCCF_LOGS\frame_times.txt` with the frame times'
//! average, 99th percentile, longest and the count over 50 ms (the game runs 30 frames a second: 33.3 ms each).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::time::Instant;

/// Frames in the last whole second (0 before the first).
static FPS: AtomicU32 = AtomicU32::new(0);

pub fn fps() -> u32 {
    FPS.load(Relaxed)
}

struct State {
    last: Instant,
    /// The second being counted: its start and frames so far.
    second: (Instant, u32),
    /// Frames since the match state was last asked.
    polls: u32,
    /// In a match: when it began and its frame times (ms).
    playing: Option<(Instant, Vec<f32>)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Every game frame, on the render thread. `playing` (this seat is in a match) is asked every 15 frames, half a
/// second; when a match ends its line goes to `done`.
// shortcut: a match still running when the game closes writes no line; add a flush on unload if that run matters
pub fn tick(playing: impl FnOnce() -> bool, done: impl FnOnce(&str)) {
    let Ok(mut g) = STATE.lock() else { return };
    let now = Instant::now();
    let Some(s) = g.as_mut() else {
        *g = Some(State {
            last: now,
            second: (now, 0),
            polls: 0,
            playing: None,
        });
        return;
    };
    let ms = (now - s.last).as_secs_f32() * 1000.0;
    s.last = now;
    s.second.1 += 1;
    let secs = (now - s.second.0).as_secs_f32();
    if secs >= 1.0 {
        FPS.store((s.second.1 as f32 / secs).round() as u32, Relaxed);
        s.second = (now, 0);
    }
    if let Some((_, times)) = &mut s.playing {
        times.push(ms);
    }
    s.polls += 1;
    if s.polls < 15 {
        return;
    }
    s.polls = 0;
    match (playing(), s.playing.take()) {
        (true, None) => s.playing = Some((now, Vec::with_capacity(30 * 600))),
        (true, kept) => s.playing = kept,
        (false, Some((t0, mut times))) => done(&match_line(
            (now - t0).as_secs_f32(),
            &mut times,
            crate::patches::limiter_sleeps(),
            crate::VSYNC.load(Relaxed) == crate::VSYNC_ON,
        )),
        (false, None) => {}
    }
}

/// Frames, average, 99th percentile, longest (ms) and the count over 50 ms. `times` comes back sorted.
fn summary(times: &mut [f32]) -> (usize, f32, f32, f32, usize) {
    let n = times.len();
    if n == 0 {
        return (0, 0.0, 0.0, 0.0, 0);
    }
    times.sort_by(f32::total_cmp);
    let avg = times.iter().sum::<f32>() / n as f32;
    let p99 = times[(n * 99).div_ceil(100) - 1];
    let over = times.iter().filter(|&&t| t > 50.0).count();
    (n, avg, p99, times[n - 1], over)
}

/// One match's line (the caller puts the date in front).
fn match_line(secs: f32, times: &mut [f32], sleeps: bool, vsync: bool) -> String {
    let (n, avg, p99, max, over) = summary(times);
    format!(
        "match {secs:.1} s: {n} frames, avg {avg:.2} ms, p99 {p99:.2} ms, max {max:.2} ms, {over} over 50 ms; frame wait \
         {}, vsync {}",
        if sleeps { "sleeps" } else { "spins" },
        if vsync { "on" } else { "off" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_line_format() {
        // 200 frames: 197 at 33.3 ms, two at 40, one hitch at 80 (the p99 is the 198th: 40)
        let mut t = vec![33.3f32; 197];
        t.extend([40.0, 80.0, 40.0]);
        assert_eq!(
            match_line(6.7, &mut t, true, false),
            "match 6.7 s: 200 frames, avg 33.60 ms, p99 40.00 ms, max 80.00 ms, 1 over 50 ms; frame wait sleeps, \
             vsync off"
        );
        assert_eq!(
            match_line(0.0, &mut [], false, true),
            "match 0.0 s: 0 frames, avg 0.00 ms, p99 0.00 ms, max 0.00 ms, 0 over 50 ms; frame wait spins, vsync on"
        );
        // one frame: it is every statistic
        assert_eq!(summary(&mut [51.0]), (1, 51.0, 51.0, 51.0, 1));
        // 100 frames: the p99 is the 99th, not the longest
        let mut t: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        assert_eq!(summary(&mut t).2, 99.0);
    }
}
