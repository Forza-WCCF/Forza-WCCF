//! The hourly Universal Cup of a server's 2017-18 arcade (`wccf-linux-server cups --edition 2017-18`): the plan of one
//! control run and the control-log rule for a safe restart. The control reads its competition list only as it starts
//! (`local\schedule_option.conf`), so a run is planned before its start: friendly slots, then a cup, up to [`PER_RUN`]
//! cups, each opening near its hour (05:10 for the 05:00 hour, after the control's day change). Its game server sends
//! GAME_CTRL 1172 = 0 (`forza-server --no-timed-cups`): no timed cups, so no Universal Championship.
//!
//! Times are local seconds (Unix seconds plus the host's UTC offset), the clock the control reads.

/// How long the control's slots last, seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lengths {
    /// A friendly slot, start to start.
    pub slot: i64,
    /// A cup, from its first round's slot start to the start of the slot after its final.
    pub cup: i64,
}

/// ovh1 under Wine, no seat in the arcade (2026-11-09 scratch run: friendly slots 508.0 s apart, the cup's eight teams
/// in five rounds 456-523 s, its first round's slot start to the next friendly's 2,393 s). A Windows host with its projector runs 490 s slots and 530 s rounds (crates/forza-ui/src/lobby.rs `CUP_SLOT_SECS`,
/// `CUP_ROUND_SECS`); seats in a round lengthen it, which [`next_on_time`] catches.
pub const OVH1: Lengths = Lengths {
    slot: 508,
    cup: 2_393,
};
/// The supervisor's start to the control's first slot (`_tBoot` in its log): Xvfb, Wine, the guard test, the shared
/// memory, control. ovh1, 2026-11-09: 58 s.
pub const STARTUP: i64 = 60;
/// A run's first cup opens this much before its time: each further hourly cup comes about 30 s later (two friendlies
/// and a cup are 3,630 s), and a cup that opens early can be missed (entry closes about 95 s after it opens).
pub const EARLY: i64 = 15;
/// A later cup of the run opens at most this much before its time (a whole number of friendlies before it)...
pub const EARLY_MAX: i64 = 60;
/// ...and at most this much after it; a cup the slots would bring later starts a new run (a restart) instead.
pub const LATE_MAX: i64 = 2 * 60;
/// Cups per control run at most; then it restarts for the next ones. The control caps only its timed cups (four per
/// business day), not these schedule cups, so this is the owner's cap. The slot grid usually ends a run sooner: on
/// ovh1's lengths an hour is no whole number of friendly slots after a cup, so each run holds one cup.
pub const PER_RUN: usize = 4;
/// A friendly slot's matching (`sendMatchingInfo`) comes this long after the slot starts (90.3 s on ovh1).
pub const MATCHING_SECS: i64 = 90;
/// A cup's final: its matching to the start of the next slot (414.7 s on ovh1).
pub const FINAL_SECS: i64 = 415;
/// A cup whose hour has no start left with a friendly slot ahead of it opens this much later instead.
pub const LATE: i64 = 10 * 60;
/// No cup within this much of the control's day change, before or after it.
const CLEAR: i64 = 5 * 60;
/// The control's day change, 05:00 (a constant in control_Release.exe, 0x45D913): it clears the cup records a
/// running cup writes to (lobby.rs `CUP_DAY_CHANGE`).
const DAY_CHANGE: i64 = 5 * 3600;
const DAY: i64 = 24 * 3600;
const HOUR: i64 = 3600;
/// The longest schedule written: lobby.rs `CUP_MAX_MINUTES` keeps the Launcher's within 40 lines, which runs played.
const ENTRIES: u32 = 40;
/// A seat paired by the control plays until its slot ends: matching comes about 91 s into a slot of at most 530 s.
pub const MATCH_SECS: i64 = 460;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cup {
    /// When players are told it opens.
    pub target: i64,
    /// When the control opens it, by the slot lengths.
    pub opens: i64,
    /// When its final's slot ends, by the slot lengths.
    pub ends: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// When to start the supervisor.
    pub start: i64,
    /// Friendly slots before each cup.
    pub friendlies: Vec<u32>,
    pub cups: Vec<Cup>,
}

impl Run {
    /// When the last cup's final ends: from then on a safe moment restarts the control.
    pub fn ends(&self) -> i64 {
        self.cups.last().map_or(self.start, |c| c.ends)
    }

    /// `local\schedule_option.conf`: the friendlies and cups, then friendlies up to [`ENTRIES`] lines, so a restart
    /// that waits on players brings no cup at an odd time for hours.
    pub fn schedule(&self) -> String {
        let mut out =
            String::from("# Forza-WCCF hourly Universal Cup (wccf-linux-server cups)\r\n");
        let mut used = 0;
        for &n in &self.friendlies {
            out += &"FriendlyMatch\r\n".repeat(n as usize);
            out += "UniversalCup\r\n";
            used += n + 1;
        }
        out += &"FriendlyMatch\r\n".repeat(ENTRIES.saturating_sub(used) as usize);
        out
    }
}

/// The cup time of the hour starting at `hour`: on the hour, 05:10 for the 05:00 hour.
fn target(hour: i64) -> i64 {
    if hour.rem_euclid(DAY) == DAY_CHANGE {
        hour + 10 * 60
    } else {
        hour
    }
}

/// No day change from [`CLEAR`] before a cup opens to [`CLEAR`] after its final.
pub fn clear_of_day_change(opens: i64, l: Lengths) -> bool {
    (opens - CLEAR - DAY_CHANGE).rem_euclid(DAY) + l.cup + 2 * CLEAR <= DAY
}

/// The run for a control started no sooner than `now`. Its first cup is the first hour's that a start still reaches
/// with at least one friendly slot ahead of it, else that hour's [`LATE`] time, else the next hour's; its next cups
/// follow hour by hour, as many as open within [`EARLY_MAX`] and [`LATE_MAX`] of their times and stay clear of the day
/// change, at most [`PER_RUN`].
pub fn plan(now: i64, l: Lengths) -> Run {
    let mut hour = now - now.rem_euclid(HOUR);
    let (start, n, first, cup_hour) = loop {
        let t = target(hour);
        let found = [t, t + LATE].into_iter().find_map(|t| {
            let lead = t - EARLY - STARTUP - now;
            let n = lead.div_euclid(l.slot);
            (n >= 1 && clear_of_day_change(t - EARLY, l))
                .then(|| (t - EARLY - STARTUP - n * l.slot, n, t))
        });
        if let Some((start, n, t)) = found {
            break (start, n, t, hour);
        }
        hour += HOUR;
    };
    let mut run = Run {
        start,
        friendlies: vec![u32::try_from(n).unwrap_or(1)],
        cups: vec![Cup {
            target: first,
            opens: first - EARLY,
            ends: first - EARLY + l.cup,
        }],
    };
    let mut hour = cup_hour;
    while run.cups.len() < PER_RUN {
        hour += HOUR;
        let t = target(hour);
        let after = run.ends();
        let n = (t - EARLY_MAX - after + l.slot - 1)
            .div_euclid(l.slot)
            .max(1);
        let opens = after + n * l.slot;
        let lines: u32 = run.friendlies.iter().map(|n| n + 1).sum();
        if opens - t > LATE_MAX
            || !clear_of_day_change(opens, l)
            || lines + u32::try_from(n).unwrap_or(ENTRIES) + 1 > ENTRIES
        {
            break;
        }
        run.friendlies.push(u32::try_from(n).unwrap_or(1));
        run.cups.push(Cup {
            target: t,
            opens,
            ends: opens + l.cup,
        });
    }
    run
}

/// After the run's cup `done` (counted from 1) ended at `ended` (the start of the slot after its final): whether the
/// run's next cup still opens within [`EARLY_MAX`] and [`LATE_MAX`] of its time. Seats in the rounds lengthen them;
/// false (or no next cup) means a restart for a new run.
pub fn next_on_time(run: &Run, done: usize, ended: i64, l: Lengths) -> bool {
    match (run.cups.get(done), run.friendlies.get(done)) {
        (Some(next), Some(&n)) => {
            (-EARLY_MAX..=LATE_MAX).contains(&(ended + i64::from(n) * l.slot - next.target))
        }
        _ => false,
    }
}

/// A slot as control's log shows it: its matching (`sendMatchingInfo() : match_tbl`, then the cup's pairs one a line,
/// or `= NULL`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Friendly,
    /// A cup round and its pairs: one is the final.
    CupRound(u32),
}

/// Reads control's log line by line for [`Slot`]s.
#[derive(Default)]
pub struct Slots {
    /// In a `match_tbl`: the pairs so far.
    table: Option<u32>,
}

impl Slots {
    pub fn feed(&mut self, line: &str) -> Option<Slot> {
        if line.contains("sendMatchingInfo() : match_tbl ") {
            self.table = Some(0);
            return None;
        }
        let pairs = self.table?;
        if !line.contains("ClientManage") {
            return None;
        }
        if line.contains("]=(") {
            self.table = Some(pairs + 1);
            return None;
        }
        // the table's end: `\n` under Wine, an empty line on Windows; no pairs: `= NULL`
        self.table = None;
        Some(if pairs == 0 {
            Slot::Friendly
        } else {
            Slot::CupRound(pairs)
        })
    }
}

/// A control log line (game.log, Wine's `+debugstr`) that pairs a player's seat for a match: `CPU170 Away -> sate1 Home`,
/// `send match info : (...)Home -> sate3 Away`. The projector's `-> sate=0(sock=1)` is not one.
pub fn pairs_a_seat(line: &str) -> bool {
    line.contains("ClientManage")
        && line.match_indices("-> sate").any(|(i, m)| {
            line.get(i + m.len()..)
                .and_then(|r| r.chars().next())
                .is_some_and(|c| c.is_ascii_digit())
        })
}

/// "HH:MM:SS" of local seconds.
pub fn clock(t: i64) -> String {
    let s = t.rem_euclid(DAY);
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = HOUR;
    /// A day far from the epoch, at local midnight.
    const D: i64 = 20_400 * DAY;
    /// A Windows host's lengths (490 s slots, five 530 s rounds): two friendlies and a cup are 3,630 s.
    const WINDOWS: Lengths = Lengths {
        slot: 490,
        cup: 5 * 530,
    };

    fn at(h: i64, m: i64) -> i64 {
        D + h * H + m * 60
    }

    fn targets(r: &Run) -> Vec<i64> {
        r.cups.iter().map(|c| c.target).collect()
    }

    /// What every plan keeps: the start lines the first cup up with the slot grid, the next cups follow the last one's
    /// end by whole friendly slots, each opens near its time, clear of the day change, all within the schedule's lines.
    fn check(now: i64, r: &Run, l: Lengths) {
        let at = clock(now);
        assert!(r.start >= now && r.start < now + l.slot, "{at}");
        assert_eq!(
            r.start + STARTUP + i64::from(r.friendlies[0]) * l.slot,
            r.cups[0].opens,
            "{at}"
        );
        assert_eq!(r.cups[0].opens, r.cups[0].target - EARLY, "{at}");
        for (k, w) in r.cups.windows(2).enumerate() {
            assert_eq!(
                w[0].ends + i64::from(r.friendlies[k + 1]) * l.slot,
                w[1].opens,
                "{at}"
            );
            let hour = |c: &Cup| c.target - c.target.rem_euclid(H);
            assert_eq!(hour(&w[1]) - hour(&w[0]), H, "{at}");
        }
        for c in &r.cups {
            assert!(
                (-EARLY_MAX..=LATE_MAX).contains(&(c.opens - c.target)),
                "{at}: {} opens {}",
                clock(c.target),
                clock(c.opens)
            );
            assert!(clear_of_day_change(c.opens, l), "{at}: {}", clock(c.opens));
            assert_eq!(c.ends, c.opens + l.cup);
            let base = target(c.target - c.target.rem_euclid(H));
            assert!(
                c.target == base || c.target == base + LATE,
                "{at}: {}",
                clock(c.target)
            );
        }
        assert!(!r.cups.is_empty() && r.cups.len() <= PER_RUN);
        assert_eq!(
            r.schedule().lines().filter(|l| !l.starts_with('#')).count(),
            ENTRIES as usize
        );
        assert_eq!(r.schedule().matches("UniversalCup").count(), r.cups.len());
    }

    #[test]
    fn four_cups_on_the_hour_when_the_slots_fit_the_hour() {
        let r = plan(at(11, 45), WINDOWS);
        check(at(11, 45), &r, WINDOWS);
        assert_eq!(targets(&r), [at(12, 0), at(13, 0), at(14, 0), at(15, 0)]);
        assert_eq!(r.friendlies[1..], [2, 2, 2]);
    }

    #[test]
    fn on_ovh1_s_slots_a_run_holds_one_cup() {
        // 2,393 s of cup and 2 or 3 x 508 s of friendlies: the next hour's cup would open 3.4 min early or 5 min late
        for m in (0..24 * 60).step_by(11) {
            assert_eq!(plan(D + m * 60, OVH1).cups.len(), 1);
        }
    }

    #[test]
    fn the_restart_after_a_run_reaches_the_next_hour() {
        for l in [WINDOWS, OVH1] {
            let first = plan(at(11, 45), l);
            let last = first.cups.last().unwrap().target;
            // a safe moment right after the last final
            let next = plan(first.ends(), l);
            check(first.ends(), &next, l);
            assert_eq!(next.cups[0].target, last + H);
            assert!(next.start >= first.ends());
        }
    }

    #[test]
    fn a_restart_too_late_for_the_hour_delays_the_cup_10_minutes_or_skips_it() {
        // 11:51: no start left before 12:00 with a slot ahead (60 s startup + 508 s + 15 s early)
        assert_eq!(plan(at(11, 51), OVH1).cups[0].target, at(12, 10));
        // 12:01: too late for 12:10 too: 13:00
        assert_eq!(plan(at(12, 1), OVH1).cups[0].target, at(13, 0));
        // 11:50:17 is the last start for 12:00
        assert_eq!(plan(at(11, 50) + 17, OVH1).cups[0].target, at(12, 0));
        assert_eq!(plan(at(11, 50) + 18, OVH1).cups[0].target, at(12, 10));
    }

    #[test]
    fn the_5_00_cup_runs_at_5_10_and_no_cup_crosses_the_day_change() {
        let r = plan(at(1, 45), WINDOWS);
        assert_eq!(targets(&r), [at(2, 0), at(3, 0), at(4, 0), at(5, 10)]);
        assert!(r.cups[2].ends + CLEAR <= at(5, 0));
        assert!(r.cups[3].opens - CLEAR >= at(5, 0));
        for l in [WINDOWS, OVH1] {
            assert_eq!(plan(at(4, 50), l).cups[0].target, at(5, 10));
            assert_eq!(plan(at(3, 45), l).cups[0].target, at(4, 0));
            // 04:30 asked for 05:00's slot: the 05:10 cup, never one at 05:00
            assert!(plan(at(4, 30), l).cups.iter().all(|c| c.target != at(5, 0)));
        }
    }

    #[test]
    fn the_day_change_rule() {
        for (h, m, ok) in [
            (4, 0, true),
            (4, 10, true),
            (4, 12, false),
            (4, 15, false),
            (5, 0, false),
            (5, 4, false),
            (5, 5, true),
            (23, 0, true),
            (0, 0, true),
        ] {
            assert_eq!(clear_of_day_change(at(h, m), WINDOWS), ok, "{h}:{m}");
        }
        // ovh1's shorter cup: 04:15 still ends 04:55 + 5 min
        assert!(clear_of_day_change(at(4, 15), OVH1));
        assert!(!clear_of_day_change(at(4, 17), OVH1));
    }

    #[test]
    fn every_start_of_a_day_plans_cups_on_time() {
        for l in [WINDOWS, OVH1] {
            for m in (0..24 * 60).step_by(7) {
                let now = D + m * 60 + 13;
                let r = plan(now, l);
                check(now, &r, l);
                assert!(r.cups[0].target - now <= 2 * H, "{}", clock(now));
            }
        }
    }

    #[test]
    fn a_long_cup_ends_the_run_early() {
        let r = plan(at(11, 45), WINDOWS);
        // cup 1 ended on time: cup 2 stays
        assert!(next_on_time(&r, 1, r.cups[0].ends, WINDOWS));
        // seats made its rounds 5 min longer: cup 2 would open 13:05, a restart instead
        assert!(!next_on_time(&r, 1, r.cups[0].ends + 300, WINDOWS));
        // 2 min shorter: 13:58, too early
        assert!(!next_on_time(&r, 1, r.cups[0].ends - 120, WINDOWS));
        // the last cup has no next
        assert!(!next_on_time(&r, 4, r.ends(), WINDOWS));
    }

    #[test]
    fn the_schedule_file() {
        let r = Run {
            start: 0,
            friendlies: vec![2, 1],
            cups: vec![],
        };
        let s = r.schedule();
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].starts_with('#'));
        assert_eq!(
            lines[1..6],
            [
                "FriendlyMatch",
                "FriendlyMatch",
                "UniversalCup",
                "FriendlyMatch",
                "UniversalCup"
            ]
        );
        assert_eq!(lines.len(), 1 + 40);
        assert!(s.ends_with("FriendlyMatch\r\n"));
    }

    fn line(s: &str) -> String {
        format!(
            "1908915.086:0170:0174:warn:debugstr:OutputDebugStringA \"DEBUG  ClientManage  {s}\""
        )
    }

    #[test]
    fn seat_pairing_lines() {
        assert!(pairs_a_seat(&line("CPU170 Away -> sate1 Home")));
        assert!(pairs_a_seat(&line("CPU173 Home -> sate2 Away")));
        assert!(pairs_a_seat(&line(
            "send match info : (ip127.0.0.1,port20003,sateID3)Away -> sate2 Home"
        )));
        assert!(!pairs_a_seat(&line(
            "ClientManage::relayGameReady(): send(GAME_READY) sate=0x1e data=1"
        )));
        assert!(!pairs_a_seat(
            "DEBUG  CoLiveManage  CoLiveManage::sendMatchGameSatellite(): send(LIVE_INFO) -> sate=0(sock=1) : isLive=1,-1"
        ));
        assert!(!pairs_a_seat(&line(
            "ClientManage::sendMatchingInfo() : match_tbl "
        )));
    }

    #[test]
    fn slots_from_control_s_log() {
        // ovh1's game.log, 2026-11-09: a friendly slot, a cup round, the final
        let log = [
            "ClientManage::sendMatchingInfo() : gtMatch_tbl ",
            "= NULL",
            "\\n",
            "ClientManage::sendMatchingInfo() : match_tbl ",
            "= NULL",
            "\\n",
            "ClientManage::sendMatchingInfo() : friendly_match_tbl ",
            "= NULL",
            "ClientManage::sendMatchingInfo() : match_tbl ",
            "[0]=(346,357,-187006887), ",
            "[1]=(451,359,-2065227398), ",
            "\\n",
            "ClientManage::sendMatchingInfo() : friendly_match_tbl ",
            "CPU170 Away -> sate1 Home",
            "ClientManage::sendMatchingInfo() : match_tbl ",
            "[0]=(357,390,726951777), ",
            "\\n",
        ];
        let mut s = Slots::default();
        let got: Vec<Slot> = log.iter().filter_map(|l| s.feed(&line(l))).collect();
        assert_eq!(got, [Slot::Friendly, Slot::CupRound(2), Slot::CupRound(1)]);
        // a Windows log's empty table is a blank line
        let mut s = Slots::default();
        assert_eq!(
            s.feed(&line("ClientManage::sendMatchingInfo() : match_tbl ")),
            None
        );
        assert_eq!(s.feed(&line("")), Some(Slot::Friendly));
    }
}
