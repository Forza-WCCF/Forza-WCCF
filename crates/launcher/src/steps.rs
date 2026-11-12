//! A start's step times on this PC (`data\start_steps.txt`): the window's bar creeps toward the next step mark at the
//! pace its steps really take here, not at an estimate's.  The window measures each of a start's six steps as its line
//! arrives ([`write`]) and reads them back at the next start ([`read`]).  No file, or one that is not six positive
//! seconds, is no file at all: the bar keeps forza-ui's estimate.

use std::path::PathBuf;
use std::{fs, io};

use crate::Kit;

/// The steps of a local start: the run's "N/6" lines.
pub const STEPS: usize = 6;

/// `data\start_steps.txt`: the seconds each step took, one "N seconds" line each.
fn file(kit: &Kit) -> PathBuf {
    kit.data().join("start_steps.txt")
}

/// The step times this PC measured, for the bar; None when it measured none yet (or the file is not six seconds).
pub fn read(kit: &Kit) -> Option<[f32; STEPS]> {
    parse(&fs::read_to_string(file(kit)).ok()?)
}

/// `text` as six step times; None unless every step is there, and a positive number of seconds.
fn parse(text: &str) -> Option<[f32; STEPS]> {
    let mut secs = [0.0; STEPS];
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (n, s) = line.split_once(' ')?;
        let n: usize = n.parse().ok()?;
        let s: f32 = s.parse().ok()?;
        if !(1..=STEPS).contains(&n) || !s.is_finite() || s <= 0.0 {
            return None;
        }
        secs[n - 1] = s;
    }
    // a start that ended early leaves a step out: that file is not this PC's six
    secs.iter().all(|s| *s > 0.0).then_some(secs)
}

/// The least a step is saved as: a step that took almost nothing (the projector off) is written "0.00" otherwise,
/// which [`parse`] refuses, and the next Launcher start would fall back to the estimate.
const MIN_SECS: f32 = 0.01;

/// Saves this start's step times for the next one's bar.
pub fn write(kit: &Kit, secs: [f32; STEPS]) -> io::Result<()> {
    let mut text =
        String::from("# the seconds each step of a start took on this PC (the window's bar runs at this pace)\n");
    for (i, s) in secs.iter().enumerate() {
        text.push_str(&format!("{} {:.2}\n", i + 1, s.max(MIN_SECS)));
    }
    crate::choices::write(&file(kit), &text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch;

    /// A start's measured times are the next one's bar: written under data\, read back as they were.
    #[test]
    fn measured_step_times_are_read_back() {
        let dir = scratch();
        let kit = Kit::at(dir.path());
        assert_eq!(read(&kit), None, "nothing measured yet");
        // an instant step (the projector off) is written as the least, not as a 0.00 the read refuses
        let secs = [3.4, 0.0004, 1.25, 0.5, 7.75, 6.0];
        write(&kit, secs).unwrap();
        assert_eq!(read(&kit), Some([3.4, MIN_SECS, 1.25, 0.5, 7.75, 6.0]));
        let again = [1.0, 2.0, 3.0, 4.0, 5.0, 6.5];
        write(&kit, again).unwrap();
        assert_eq!(read(&kit), Some(again), "a later start takes over");
    }

    /// A hand-edited or half-written file is not this PC's six times: the bar keeps its estimate.
    #[test]
    fn a_file_without_six_seconds_is_no_file() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("1 2.0\n2 3.0\n"), None, "a step is missing");
        assert_eq!(
            parse(&(1..=6).map(|n| format!("{n} 0\n")).collect::<String>()),
            None,
            "no time"
        );
        assert_eq!(parse("1 2\n2 3\n3 4\n4 5\n5 6\n7 1\n"), None, "no sixth step");
        assert_eq!(parse("1 2\n2 3\n3 4\n4 5\n5 6\n6 nope\n"), None, "not a number");
        let good = "# a note\n1 2\n2 3\n3 4\n4 5\n5 6\n6 1.5\n";
        assert_eq!(parse(good), Some([2.0, 3.0, 4.0, 5.0, 6.0, 1.5]));
    }
}
