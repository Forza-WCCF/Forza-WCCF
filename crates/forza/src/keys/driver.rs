//! Seat 1's key and controller driver (2010–11), ported from `keyboard.py` (step 10, piece E).
//!
//! Keys count only while a game window (`client_Release.exe`) is the one in front, so typing anywhere else sends
//! nothing. The held inputs become the cabinet's switch bytes, which the driver sets on the [`Shared`] state the I/O
//! board stand-in (`crate::standins::io_board`) reads; the Python passed them through `_jvs_seat1_input.txt`.
//!
//! Which key does what comes from `keys.txt` (written by the KEYS panel in seat 1's window; missing = the defaults in
//! [`ACTIONS`]), read again whenever it changes. Each action can also have one control on a game controller, from
//! `PAD NAME=B3` lines, read with winmm's joystick calls. The game itself runs in FREE PLAY (2017-18: setup changes the
//! default coin setting; 2010-11: the panel sets it), so START puts in no coin; the COIN key stays for the KEYS page. 2010-11 keeps the coin on START. The club card goes in and out with a key; the game's own ejection (output
//! byte 1 bit 0x08, the "waiting for a card" signal) takes it out.
//!
//! The logic below is pure and replayed against `keyboard.py` (`tests/rec/keys.jsonl`); only [`spawn`] touches Windows.

use crate::standins::io_board::{CARD_BIT, Shared};
use crate::standins::{Handle, Log};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

/// Which switch bytes an action sets: (`sys`, `p1` or `p2`), the byte, the bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Sys,
    P1,
    P2,
}

pub struct Action {
    /// Its name in `keys.txt` (the on-screen buttons' names, `forza_panel.dll`).
    pub name: &'static str,
    /// The default key (virtual-key code); 0 = none.
    pub vk: u32,
    /// The cabinet input it holds; `None` for CARD and COIN, which are edges.
    pub bits: Option<(Line, usize, u8)>,
    pub what: &'static str,
}

const fn a(
    name: &'static str,
    vk: u32,
    bits: Option<(Line, usize, u8)>,
    what: &'static str,
) -> Action {
    Action {
        name,
        vk,
        bits,
        what,
    }
}

/// 2010-11 wiring (game button table DAT_009fbd30): the cabinet's extra buttons are on the 2ND JVS player; p1 byte 1
/// 0x80 is a card-dispenser sensor and 0x20 is never read. SENSOR2 and SENSOR3 have NO default key: on the original's
/// club-card recovery screen SW1 means "overwrite the card with the backup", so one stray key could roll the card back.
pub const ACTIONS: [Action; 16] = [
    a("START", 0x0D, Some((Line::P1, 0, 0x80)), "START"),
    a("PRESS", 0x58, Some((Line::P1, 0, 0x02)), "PRESS (decide)"),
    a("SHOOT", 0x43, Some((Line::P1, 0, 0x01)), "SHOOT"),
    a(
        "KEEPER",
        0x42,
        Some((Line::P2, 0, 0x02)),
        "KEEPER (game button 8)",
    ),
    a(
        "DATA",
        0x53,
        Some((Line::P2, 1, 0x80)),
        "DATA (game button 5)",
    ),
    a(
        "KEYPLAYER",
        0x44,
        Some((Line::P2, 1, 0x40)),
        "KEY PLAYER - a guess (game button 9)",
    ),
    a(
        "UP",
        0x26,
        Some((Line::P1, 0, 0x20)),
        "tactics: CENTRAL (up)",
    ),
    a(
        "DOWN",
        0x28,
        Some((Line::P1, 0, 0x10)),
        "tactics: COUNTER (down)",
    ),
    a(
        "LEFT",
        0x25,
        Some((Line::P1, 0, 0x08)),
        "tactics: L-SIDE (left)",
    ),
    a(
        "RIGHT",
        0x27,
        Some((Line::P1, 0, 0x04)),
        "tactics: R-SIDE (right)",
    ),
    a("CARD", 0x49, None, "club card in / out of the slot"),
    a("COIN", 0x35, None, "one coin"),
    a(
        "TEST",
        0,
        Some((Line::Sys, 0, 0x80)),
        "TEST (operator menu)",
    ),
    a("SERVICE", 0x71, Some((Line::P1, 0, 0x40)), "SERVICE"),
    a(
        "SENSOR2",
        0,
        Some((Line::P2, 0, 0x10)),
        "the publisher's service switch SW1 (bit 28) - NO default key: on the publisher's club-card recovery screen SW1 = \
         'overwrite the card with the backup', so one stray key could roll the card back; set a key in KEYS only if you \
         need it (A8)",
    ),
    a(
        "SENSOR3",
        0,
        Some((Line::P2, 0, 0x08)),
        "the publisher's service switch SW2 (bit 27) - 'back' on the publisher's service screens; NO default key, set one in \
         KEYS if you need it",
    ),
];

const CARD: usize = 10;
const COIN: usize = 11;
const START: usize = 0;

use super::{PAD_AXIS, PAD_BUTTON, PAD_HAT, is_allowed, key_label, pad_code, pad_label};
#[cfg(windows)]
pub use win::{Winmm, pad_name};

/// An axis direction is pressed past half way, let go back inside 0.3 (no flicker).
const PAD_ON: f64 = 0.5;
const PAD_OFF: f64 = 0.3;

// ---- keys.txt

/// The default key of every action, in [`ACTIONS`] order.
pub fn default_keys() -> [u32; 16] {
    std::array::from_fn(|i| ACTIONS[i].vk)
}

fn action_index(name: &str) -> Option<usize> {
    ACTIONS.iter().position(|a| a.name == name)
}

/// The file's lines as Python's `open(.., encoding="ascii", errors="replace").read().splitlines()` gives them.
fn lines(raw: &[u8]) -> Vec<String> {
    let (mut out, mut cur, mut prev_cr) = (Vec::new(), String::new(), false);
    for &b in raw {
        let c = if b < 0x80 { b as char } else { '\u{FFFD}' };
        if c == '\n' && prev_cr {
            prev_cr = false;
            continue;
        }
        prev_cr = c == '\r';
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c'..='\x1f') {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Python's `repr()` of a string.
fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7F => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// Python's `int(s, 0)` for an already stripped string.
fn parse_int0(s: &str) -> Option<i128> {
    let (neg, s) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let lower = s.to_ascii_lowercase();
    let (base, digits, prefixed) = if let Some(r) = lower.strip_prefix("0x") {
        (16, r.to_string(), true)
    } else if let Some(r) = lower.strip_prefix("0o") {
        (8, r.to_string(), true)
    } else if let Some(r) = lower.strip_prefix("0b") {
        (2, r.to_string(), true)
    } else {
        (10, lower, false)
    };
    // one underscore between digits (and right after a base prefix)
    let body = if prefixed {
        digits.strip_prefix('_').unwrap_or(&digits)
    } else {
        &digits
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let plain: String = body.chars().filter(|c| *c != '_').collect();
    if !prefixed && plain.len() > 1 && plain.starts_with('0') && plain.bytes().any(|b| b != b'0') {
        return None;
    }
    let v = i128::from_str_radix(&plain, base).ok()?;
    if !plain.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some(if neg { -v } else { v })
}

fn hex02(v: i128) -> String {
    if v < 0 {
        format!("-{:X}", -v)
    } else {
        format!("{v:02X}")
    }
}

/// `keys.txt` -> (the key of every action in [`ACTIONS`] order, notes). Lines "NAME=0x58"; a bad, unknown or forbidden
/// line is skipped (that action keeps its default); a key given twice stays with the first action, and the later one
/// goes back to its default if that is free, else it has no key (0). Never fails: the file may be mid-swap.
pub fn load_keys(raw: &[u8]) -> ([u32; 16], Vec<String>) {
    load_keys_with(raw, default_keys())
}

/// [`load_keys`] with other default keys (in [`ACTIONS`] order): the 2017-18 kit's SW1 and SW2 have keys.
pub fn load_keys_with(raw: &[u8], defaults: [u32; 16]) -> ([u32; 16], Vec<String>) {
    let mut keys = defaults;
    let mut notes = Vec::new();
    let mut given: Vec<usize> = Vec::new();
    for ln in lines(raw) {
        let ln = ln.split('#').next().unwrap_or("").trim().to_string();
        if ln.is_empty() {
            continue;
        }
        let (name, val) = ln.split_once('=').unwrap_or((&ln, ""));
        let name = name.trim().to_uppercase();
        if name.split_whitespace().next() == Some("PAD") {
            continue; // a controller line: load_pads() reads those
        }
        if name == "PAUSE" {
            continue; // the panel's own (forza::keys::PAUSE): the game held, no cabinet input
        }
        let Some(idx) = action_index(&name) else {
            notes.push(format!("unknown action {}", py_repr(&name)));
            continue;
        };
        let val = val.trim();
        let Some(vk) = parse_int0(val) else {
            notes.push(format!("{name}: not a key code: {}", py_repr(val)));
            continue;
        };
        if !is_allowed(vk) {
            notes.push(format!("{name}: key 0x{} cannot be used", hex02(vk)));
            continue;
        }
        keys[idx] = vk as u32;
        if !given.contains(&idx) {
            given.push(idx);
        }
    }
    let order: Vec<usize> = given
        .iter()
        .copied()
        .chain((0..16).filter(|i| !given.contains(i)))
        .collect();
    let mut owner: Vec<(u32, usize)> = Vec::new(); // key -> the action that has it
    for (i, &n) in order.iter().enumerate() {
        let vk = keys[n];
        if vk != 0
            && let Some(&(_, who)) = owner.iter().find(|(k, _)| *k == vk)
        {
            let d = defaults[n];
            let later = order[i + 1..].iter().any(|&m| keys[m] == d);
            keys[n] = if !owner.iter().any(|(k, _)| *k == d) && !later {
                d
            } else {
                0
            };
            notes.push(format!(
                "{}: key 0x{:02X} is {}'s - {}",
                ACTIONS[n].name,
                vk,
                ACTIONS[who].name,
                if keys[n] != 0 {
                    "back to its default"
                } else {
                    "left with no key"
                }
            ));
        }
        if keys[n] != 0 {
            owner.push((keys[n], n));
        }
    }
    (keys, notes)
}

/// `keys.txt`'s "PAD NAME=WORD" lines -> (the control of every action, 0 = none; notes). No file and no line = no
/// controls. "PAD NAME=" or "=-" = none. A bad action or word is skipped; the same action twice: the last line counts;
/// a control given to two actions stays with the one whose line came first, the other gets none.
pub fn load_pads(raw: &[u8]) -> ([u32; 16], Vec<String>) {
    let mut notes = Vec::new();
    let mut given = [0u32; 16];
    let mut order: Vec<usize> = Vec::new();
    for ln in lines(raw) {
        let body = ln.split('#').next().unwrap_or("").trim().to_string();
        let Some((name, val)) = body.split_once('=') else {
            continue;
        };
        let words: Vec<&str> = name.split_whitespace().collect();
        if words.len() != 2 || words[0].to_uppercase() != "PAD" {
            continue;
        }
        let (act, val) = (words[1].to_uppercase(), val.trim());
        let Some(idx) = action_index(&act) else {
            notes.push(format!("PAD: unknown action {}", py_repr(&act)));
            continue;
        };
        let none = val.is_empty() || val == "-";
        let code = if none { 0 } else { pad_code(val) };
        if !none && code == 0 {
            notes.push(format!(
                "PAD {act}: not a controller control: {}",
                py_repr(val)
            ));
            continue;
        }
        given[idx] = code;
        if !order.contains(&idx) {
            order.push(idx);
        }
    }
    let mut pads = [0u32; 16];
    let mut owner: Vec<(u32, usize)> = Vec::new();
    for idx in order {
        let code = given[idx];
        if code == 0 {
            continue;
        }
        if let Some(&(_, who)) = owner.iter().find(|(c, _)| *c == code) {
            notes.push(format!(
                "PAD {}: {} is {}'s - left with none",
                ACTIONS[idx].name,
                pad_label(code),
                ACTIONS[who].name
            ));
            continue;
        }
        owner.push((code, idx));
        pads[idx] = code;
    }
    (pads, notes)
}

// ---- keys and controls -> switches

/// The switch bytes the held inputs make.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Switches {
    pub sys: u8,
    pub p1: [u8; 2],
    pub p2: [u8; 2],
}

/// Seconds between that coin and START reaching the game.
const START_DELAY: f64 = 0.15;
/// Seconds after the press until which START stays held even if the key was only tapped.
const START_MIN: f64 = 0.35;
/// The card comes out by itself when the game lets it go: output byte 1 bit 0x08 is on only while the game waits for a
/// card. Not in the first `EJECT_GRACE` s after an insert, and the signal must hold `EJECT_HOLD` s.
const EJECT_BYTE: usize = 1;
const EJECT_BIT: u8 = 0x08;
const EJECT_GRACE: f64 = 2.5;
const EJECT_HOLD: f64 = 0.25;

/// What holds each action: its key and its controller control.
#[derive(Clone, Debug)]
pub struct Bindings {
    pub keys: [u32; 16],
    pub pads: [u32; 16],
    /// Every START press first puts a coin in (the driver paying; the recorded replay of `keyboard.py` expects it).
    /// The Launcher turns it off for 2017-18 (setup makes the game itself FREE PLAY); 2010-11's free play is the
    /// panel's in-memory patch, so its START still pays.
    pub start_coin: bool,
}

impl Bindings {
    pub fn new(keys: [u32; 16], pads: [u32; 16]) -> Self {
        Self {
            keys,
            pads,
            start_coin: true,
        }
    }

    pub fn defaults() -> Self {
        Self::new(default_keys(), [0; 16])
    }

    /// The keys to ask Windows about (every action's key, none for 0).
    pub fn polled(&self) -> Vec<u32> {
        self.keys.iter().copied().filter(|k| *k != 0).collect()
    }

    pub fn has_pads(&self) -> bool {
        self.pads.iter().any(|c| *c != 0)
    }

    /// The actions held by these inputs (key codes and controller controls), one bit per action.
    pub fn held_actions(&self, down: &BTreeSet<u32>) -> u16 {
        (0..16).fold(0, |m, i| {
            let held = [self.keys[i], self.pads[i]]
                .iter()
                .any(|x| *x != 0 && down.contains(x));
            m | (u16::from(held) << i)
        })
    }

    /// The switch bytes for these held inputs, the card state, and free play's timing (`start`: `None` = START as its
    /// inputs say; `Some(held)` = held or not, whatever they say).
    pub fn render(&self, down: &BTreeSet<u32>, card: bool, start: Option<bool>) -> Switches {
        let mut acts = self.held_actions(down);
        if let Some(s) = start {
            acts = (acts & !(1 << START)) | (u16::from(s) << START);
        }
        let mut f = Switches::default();
        for (i, act) in ACTIONS.iter().enumerate() {
            if let Some((line, byte, bit)) = act.bits
                && acts >> i & 1 == 1
            {
                match line {
                    Line::Sys => f.sys |= bit,
                    Line::P1 => f.p1[byte] |= bit,
                    Line::P2 => f.p2[byte] |= bit,
                }
            }
        }
        if card {
            f.p2[0] |= CARD_BIT;
        }
        f
    }

    /// One poll: an action's first moment held toggles the card / adds a coin. Returns (card, coin, events).
    pub fn step(
        &self,
        down: &BTreeSet<u32>,
        prev: &BTreeSet<u32>,
        mut card: bool,
        mut coin: i64,
    ) -> (bool, i64, Vec<String>) {
        let mut events = Vec::new();
        let (now_a, was_a) = (self.held_actions(down), self.held_actions(prev));
        let edges = now_a & !was_a;
        if edges >> CARD & 1 == 1 {
            card = !card;
            events.push(
                if card {
                    "card IN the slot"
                } else {
                    "card OUT of the slot"
                }
                .to_string(),
            );
        }
        if edges >> COIN & 1 == 1 {
            coin += 1;
            events.push(format!("coin inserted (counter {coin})"));
        }
        if self.start_coin && edges >> START & 1 == 1 {
            coin += 1;
            events.push(format!("coin put in for you - free play (counter {coin})"));
        }
        for (i, act) in ACTIONS.iter().enumerate() {
            if act.bits.is_some() && edges >> i & 1 == 1 {
                events.push(format!("{} down", act.what));
            }
        }
        for (i, act) in ACTIONS.iter().enumerate() {
            if act.bits.is_some() && (was_a & !now_a) >> i & 1 == 1 {
                events.push(format!("{} up", act.what));
            }
        }
        (card, coin, events)
    }
}

/// Free play: is START held at time `now`? `armed` = (from, until), set when START went down.
pub fn start_held(now: f64, start_down: bool, armed: Option<(f64, f64)>) -> bool {
    match armed {
        Some((from, until)) if now >= from => start_down || now < until,
        _ => false,
    }
}

/// -> (take the card out?, new `waiting_since`). `waiting_since` = when the "waiting for a card" bit came on.
pub fn should_eject(
    now: f64,
    card: bool,
    card_in_since: Option<f64>,
    outs: Option<&[u8]>,
    waiting_since: Option<f64>,
) -> (bool, Option<f64>) {
    let waiting = outs.is_some_and(|o| o.len() > EJECT_BYTE && o[EJECT_BYTE] & EJECT_BIT != 0);
    if !waiting {
        return (false, None);
    }
    let since = waiting_since.unwrap_or(now);
    let ok =
        card && card_in_since.is_some_and(|t| now - t >= EJECT_GRACE) && now - since >= EJECT_HOLD;
    (ok, Some(since))
}

/// The driver's state between polls: the loop body of `keyboard.py`'s `main()`, without the operating system.
pub struct Driver {
    pub bindings: Bindings,
    shared: Arc<Shared>,
    card: bool,
    prev: BTreeSet<u32>,
    armed: Option<(f64, f64)>,
    card_in_since: Option<f64>,
    waiting_since: Option<f64>,
    next_out_check: f64,
}

impl Driver {
    /// Starts with the card as the shared state has it (in, if a run was restarted with it in).
    pub fn new(bindings: Bindings, shared: Arc<Shared>, now: f64) -> Self {
        let card = shared.card_in();
        Self {
            bindings,
            shared,
            card,
            prev: BTreeSet::new(),
            armed: None,
            card_in_since: card.then_some(now),
            waiting_since: None,
            next_out_check: 0.0,
        }
    }

    /// One poll at time `now` (seconds) with these inputs held; sets the switches and coin on the shared state and
    /// returns what to tell the log.
    pub fn tick(&mut self, now: f64, down: &BTreeSet<u32>) -> Vec<String> {
        let coin0 = self.shared.inputs().coin[0];
        let was_in = self.card;
        let (card, coin, mut events) = self.bindings.step(down, &self.prev, self.card, coin0);
        self.card = card;
        if coin != coin0 {
            self.shared.add_coin(0, coin - coin0);
        }
        if self.card && !was_in {
            self.card_in_since = Some(now);
        }
        if now >= self.next_out_check {
            self.next_out_check = now + 0.1;
            let outs = self.shared.outputs();
            let (eject, waiting) = should_eject(
                now,
                self.card,
                self.card_in_since,
                outs.as_deref(),
                self.waiting_since,
            );
            self.waiting_since = waiting;
            if eject {
                (self.card, self.card_in_since, self.waiting_since) = (false, None, None);
                events.push(
                    "the game ejected the card - taken out (press I to put it back in)".into(),
                );
            }
        }
        let acts = self.bindings.held_actions(down);
        if acts & !self.bindings.held_actions(&self.prev) & (1 << START) != 0 {
            self.armed = Some((now + START_DELAY, now + START_MIN));
        }
        let start = start_held(now, acts & (1 << START) != 0, self.armed);
        self.set(down, Some(start));
        self.prev = down.clone();
        events
    }

    fn set(&self, down: &BTreeSet<u32>, start: Option<bool>) {
        let s = self.bindings.render(down, self.card, start);
        self.shared.set_switches(s.sys, s.p1, s.p2);
    }

    /// Lets go of every button, keeps the card and the coins (the driver is stopping).
    pub fn release(&self) {
        self.set(&BTreeSet::new(), None);
    }
}

// ---- controllers

/// One controller as winmm describes it (`JOYCAPSW`, as far as used).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caps {
    pub num_buttons: u32,
    pub num_axes: u32,
    pub flags: u32,
    pub mid: u16,
    pub pid: u16,
    /// (min, max) of X Y Z R U V.
    pub lim: [(u32, u32); 6],
}

/// One reading of a controller (`JOYINFOEX`, as far as used).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub buttons: u32,
    pub pos: [u32; 6],
    pub pov: u32,
}

const HAS_Z: u32 = 0x1;
const HAS_R: u32 = 0x2;
const HAS_U: u32 = 0x4;
const HAS_V: u32 = 0x8;
const HAS_POV: u32 = 0x10;

/// An axis reading as -1 .. +1, the middle 0 (a range that is no range: 0).
pub fn pad_axis_value(v: u32, lo: u32, hi: u32) -> f64 {
    if hi <= lo {
        return 0.0;
    }
    (2.0 * (f64::from(v) - f64::from(lo)) / (f64::from(hi) - f64::from(lo)) - 1.0).clamp(-1.0, 1.0)
}

/// The hat's directions held, 0-3 = up right down left; `pov` in hundredths of a degree, centred = 0xFFFF (anything
/// above 35999). A diagonal holds both of its directions.
pub fn pad_hat_dirs(pov: u32) -> BTreeSet<u32> {
    if pov > 35999 {
        return BTreeSet::new();
    }
    let pov = i64::from(pov);
    (0..4u32)
        .filter(|i| {
            let d = (pov - i64::from(*i) * 9000).abs() % 36000;
            d.min(36000 - d) <= 4500
        })
        .collect()
}

/// (buttons, X Y Z R U V as -1..1 or `None` where the controller has no such axis, pov) from winmm's answers.
pub fn pad_reading(caps: &Caps, info: &Info) -> (u32, [Option<f64>; 6], u32) {
    let has = [
        caps.num_axes >= 1,
        caps.num_axes >= 2,
        caps.flags & HAS_Z != 0,
        caps.flags & HAS_R != 0,
        caps.flags & HAS_U != 0,
        caps.flags & HAS_V != 0,
    ];
    let axes = std::array::from_fn(|i| {
        let (lo, hi) = caps.lim[i];
        (has[i] && hi > lo).then(|| pad_axis_value(info.pos[i], lo, hi))
    });
    let pov = if caps.flags & HAS_POV != 0 {
        info.pov
    } else {
        0xFFFF
    };
    (info.buttons, axes, pov)
}

/// One controller: what it rested on when first seen, and its controls pressed at the last read. The REST RULE (the
/// same in the KEYS panel): an axis direction the controller rests on when first seen - a trigger resting at one end,
/// a throttle, an axis it does not use - is never a press while it stays plugged in; a button or hat direction held
/// when first seen counts once it has been let go.
pub struct PadDevice {
    rest: [i8; 6],
    held0: u32,
    hat0: BTreeSet<u32>,
    act: BTreeSet<u32>,
}

impl PadDevice {
    pub fn new(buttons: u32, axes: &[Option<f64>; 6], pov: u32) -> Self {
        let mut d = Self {
            rest: axes.map(|v| match v {
                Some(v) if v < -PAD_ON => -1,
                Some(v) if v > PAD_ON => 1,
                _ => 0,
            }),
            held0: buttons,
            hat0: pad_hat_dirs(pov),
            act: BTreeSet::new(),
        };
        d.read(buttons, axes, pov);
        d
    }

    /// The controls pressed now (control numbers), from one reading.
    pub fn read(&mut self, buttons: u32, axes: &[Option<f64>; 6], pov: u32) -> BTreeSet<u32> {
        self.held0 &= buttons;
        let mut now: BTreeSet<u32> = (0..32u32)
            .filter(|i| (buttons & !self.held0) >> i & 1 == 1)
            .map(|i| PAD_BUTTON + i + 1)
            .collect();
        for (axis, v) in axes.iter().enumerate() {
            let Some(v) = v else { continue };
            for (plus, side) in [(0u32, -1i8), (1, 1)] {
                let code = PAD_AXIS + 2 * axis as u32 + plus;
                let limit = if self.act.contains(&code) {
                    PAD_OFF
                } else {
                    PAD_ON
                };
                if side != self.rest[axis] && v * f64::from(side) > limit {
                    now.insert(code);
                }
            }
        }
        let dirs = pad_hat_dirs(pov);
        self.hat0 = self.hat0.intersection(&dirs).copied().collect();
        now.extend(dirs.difference(&self.hat0).map(|d| PAD_HAT + d));
        self.act.clone_from(&now);
        now
    }
}

/// Windows' standard joystick calls (or a stand-in for tests): `None` for an empty slot.
pub trait PadApi: Send + Sync {
    fn caps(&self, jid: u32) -> Option<Caps>;
    fn pos(&self, jid: u32) -> Option<Info>;
    /// Windows reads its controller list again (finds one plugged in after the start).
    fn config_changed(&self);
}

type Namer = Box<dyn Fn(&Caps) -> String + Send + Sync>;

struct PadsInner {
    api: Box<dyn PadApi>,
    say: Log,
    name: Namer,
    /// slot, caps, device, name - in the order found.
    devs: Mutex<Vec<(u32, Caps, PadDevice, String)>>,
}

/// Every controller plugged in, read through the api. [`Pads::poll`] asks only the controllers already found and
/// answers the controls held now on any of them; looking for controllers ([`Pads::rescan`]) runs on its own thread
/// every 2 s when made with [`Pads::spawn`] - an empty slot is known to be slow on some PCs, so never inside the 10 ms
/// key loop. Windows' controller list is read again only while none is found, so a controller in use is never
/// disturbed. `say` gets one line per controller found or gone.
pub struct Pads {
    inner: Arc<PadsInner>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Pads {
    /// Without the rescan thread (tests call [`Pads::rescan`] themselves).
    pub fn new(api: Box<dyn PadApi>, say: Log, name: Namer) -> Self {
        Self {
            inner: Arc::new(PadsInner {
                api,
                say,
                name,
                devs: Mutex::new(Vec::new()),
            }),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    pub fn spawn(api: Box<dyn PadApi>, say: Log, name: Namer) -> Self {
        let mut p = Self::new(api, say, name);
        let (inner, stop) = (p.inner.clone(), p.stop.clone());
        p.thread = Some(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                inner.rescan();
                for _ in 0..40 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }));
        p
    }

    pub fn rescan(&self) {
        self.inner.rescan();
    }

    /// The controls held now on every controller found (a controller that stops answering is let go and said).
    pub fn poll(&self) -> BTreeSet<u32> {
        let ids: Vec<u32> = self.inner.devs().iter().map(|d| d.0).collect();
        let mut held = BTreeSet::new();
        for jid in ids {
            let info = self.inner.api.pos(jid);
            let mut devs = self.inner.devs();
            let Some(at) = devs.iter().position(|d| d.0 == jid) else {
                continue;
            };
            let Some(info) = info else {
                let (_, _, _, name) = devs.remove(at);
                drop(devs);
                (self.inner.say)(&format!("controller gone: {name}"));
                continue;
            };
            let (_, caps, dev, _) = &mut devs[at];
            let (b, axes, pov) = pad_reading(caps, &info);
            held.extend(dev.read(b, &axes, pov));
        }
        held
    }

    pub fn names(&self) -> Vec<String> {
        self.inner.devs().iter().map(|d| d.3.clone()).collect()
    }
}

impl Drop for Pads {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl PadsInner {
    fn devs(&self) -> std::sync::MutexGuard<'_, Vec<(u32, Caps, PadDevice, String)>> {
        self.devs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn rescan(&self) {
        let known: Vec<u32> = self.devs().iter().map(|d| d.0).collect();
        if known.is_empty() {
            self.api.config_changed();
        }
        for jid in 0..16 {
            if known.contains(&jid) {
                continue;
            }
            let Some(caps) = self.api.caps(jid) else {
                continue;
            };
            let Some(info) = self.api.pos(jid) else {
                continue;
            };
            let (b, axes, pov) = pad_reading(&caps, &info);
            let (dev, name) = (PadDevice::new(b, &axes, pov), (self.name)(&caps));
            let buttons = caps.num_buttons;
            self.devs().push((jid, caps, dev, name.clone()));
            (self.say)(&format!("controller found: {name} ({buttons} buttons)"));
        }
    }
}

// ---- the thread

/// Seconds since the driver started.
fn since(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64()
}

fn file_stamp(path: &std::path::Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Starts the driver thread: it reads `keys_path` (missing = the defaults, read again when it changes), polls the
/// keyboard and controllers every 10 ms while a game window is in front, and sets the switches on `shared`. Dropping the
/// handle stops it and lets go of every button.
#[cfg(windows)]
pub fn spawn(
    shared: Arc<Shared>,
    keys_path: std::path::PathBuf,
    start_coin: bool,
    log: Log,
) -> Handle {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = thread::spawn(move || {
        let t0 = Instant::now();
        let read = |p: &std::path::Path| std::fs::read(p).unwrap_or_default();
        let load = |p: &std::path::Path| {
            let raw = read(p);
            let ((keys, notes), (pads, pnotes)) = (load_keys(&raw), load_pads(&raw));
            (
                Bindings {
                    start_coin,
                    ..Bindings::new(keys, pads)
                },
                notes.into_iter().chain(pnotes).collect::<Vec<_>>(),
            )
        };
        let (bindings, notes) = load(&keys_path);
        let mut driver = Driver::new(bindings, shared.clone(), since(t0));
        let (mut kstamp, mut next_keys_check) = (file_stamp(&keys_path), 0.0);
        log(
            "WCCF 2010-11 seat 1 - keyboard and controller (both count only while a game window is in front)",
        );
        for (i, act) in ACTIONS.iter().enumerate() {
            let b = &driver.bindings;
            let pad = if b.pads[i] != 0 {
                pad_label(b.pads[i])
            } else {
                String::new()
            };
            log(&format!(
                "  {:<9} {:<12} {}",
                key_label(b.keys[i]),
                pad,
                act.what
            ));
        }
        log(&format!(
            "keys from {}{}",
            keys_path.display(),
            if keys_path.exists() {
                ""
            } else {
                " (not there yet: the defaults)"
            }
        ));
        for n in &notes {
            log(&format!("  keys.txt: {n}"));
        }
        log(&format!(
            "now: card {}.",
            if shared.card_in() { "IN" } else { "OUT" }
        ));
        log(if start_coin {
            "FREE PLAY: every Enter (START) puts a coin in for you first - no need for 5."
        } else {
            "FREE PLAY is the game's own: Enter (START) puts in no coin (5 is still a coin)."
        });
        log(
            "CARD: when the game ejects the card it comes out by itself - press I to put it back in.",
        );
        let mut pads: Option<Pads> = None;
        let mut os = win::Foreground::default();
        while !flag.load(Ordering::Relaxed) {
            let now = since(t0);
            if now >= next_keys_check {
                // the KEYS panel saved new keys: use them at once
                next_keys_check = now + 0.5;
                let st = file_stamp(&keys_path);
                if st != kstamp {
                    kstamp = st;
                    let (b, notes) = load(&keys_path);
                    let old = driver.bindings.clone();
                    let changed: Vec<String> = (0..16)
                        .filter(|&i| b.keys[i] != old.keys[i])
                        .map(|i| format!("{} = {}", ACTIONS[i].name, key_label(b.keys[i])))
                        .collect();
                    let pchanged: Vec<String> = (0..16)
                        .filter(|&i| b.pads[i] != old.pads[i])
                        .map(|i| format!("{} = {}", ACTIONS[i].name, pad_label(b.pads[i])))
                        .collect();
                    if !changed.is_empty() || !pchanged.is_empty() {
                        driver.bindings = b;
                    }
                    if !changed.is_empty() {
                        log(&format!("  keys: {}", changed.join(", ")));
                    }
                    if !pchanged.is_empty() {
                        log(&format!("  controller: {}", pchanged.join(", ")));
                    }
                    for n in notes {
                        log(&format!("  keys.txt: {n}"));
                    }
                }
            }
            if driver.bindings.has_pads() && pads.is_none() {
                log("controllers: looking for them (keys.txt gives one a job)");
                pads = Some(Pads::spawn(
                    Box::new(win::Winmm),
                    log.clone(),
                    Box::new(win::pad_name),
                ));
            }
            let front = os.game_in_front();
            let mut down: BTreeSet<u32> = if front {
                driver
                    .bindings
                    .polled()
                    .into_iter()
                    .filter(|vk| win::key_down(*vk))
                    .collect()
            } else {
                BTreeSet::new()
            };
            if front
                && driver.bindings.has_pads()
                && let Some(p) = &pads
            {
                down.extend(p.poll()); // controller controls: numbers above every key code
            }
            for e in driver.tick(now, &down) {
                log(&e);
            }
            thread::sleep(Duration::from_millis(10));
        }
        driver.release();
    });
    Handle::new(stop, thread)
}

/// Every controller through Windows' winmm joystick calls, looked for every 2 s ([`Pads::spawn`]): the Launcher's
/// lobby reads the gamepad with it.
#[cfg(windows)]
pub fn winmm_pads(say: Log) -> Pads {
    Pads::spawn(Box::new(win::Winmm), say, Box::new(win::pad_name))
}

#[cfg(windows)]
mod win {
    use super::{Caps, Info, PadApi};
    use crate::procs;
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::Media::Multimedia::{
        JOY_RETURNPOVCTS, JOYCAPSW, JOYINFOEX, joyGetDevCapsW, joyGetPosEx,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetCursorPos, GetForegroundWindow, GetPropW, GetWindowThreadProcessId,
        IsWindow, IsWindowVisible, SetForegroundWindow, WindowFromPoint,
    };

    pub fn key_down(vk: u32) -> bool {
        // SAFETY: plain call with a key code; no pointers.
        let state = unsafe { GetAsyncKeyState(vk as i32) };
        state as u16 & 0x8000 != 0
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    fn prop(hwnd: HWND, name: &str) -> bool {
        let w = wide(name);
        // SAFETY: `w` is a NUL-terminated UTF-16 string that outlives the call.
        !unsafe { GetPropW(hwnd, w.as_ptr()) }.is_null()
    }

    /// Is a game window in front - what decides whether keys count.
    #[derive(Default)]
    pub struct Foreground {
        cache: HashMap<isize, bool>,
        /// The game window that was in front last, until it is given back or something else takes over.
        last: Option<HWND>,
    }

    fn image_is(hwnd: HWND, name: &str) -> bool {
        let mut pid = 0u32;
        // SAFETY: `pid` is a valid out-pointer.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        procs::image(pid).and_then(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
        }) == Some(true)
    }

    fn class_of(hwnd: HWND) -> String {
        let mut b = [0u16; 128];
        // SAFETY: the buffer and its length go together.
        let n = unsafe { GetClassNameW(hwnd, b.as_mut_ptr(), 128) }.max(0) as usize;
        String::from_utf16_lossy(&b[..n])
    }

    /// Puts the game window back in front. A background process may not take the foreground, so a tap on ALT first
    /// (the keyboard is then ours to give it away).
    fn bring_back(hwnd: HWND) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            KEYEVENTF_KEYUP, VK_MENU, keybd_event,
        };
        // SAFETY: plain calls; `hwnd` was checked to be a window a moment ago.
        unsafe {
            keybd_event(VK_MENU as u8, 0, 0, 0);
            keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
            SetForegroundWindow(hwnd);
        }
    }

    impl Foreground {
        fn is_game(&mut self, hwnd: HWND) -> bool {
            if let Some(g) = self.cache.get(&(hwnd as isize)) {
                return *g;
            }
            let mut pid = 0u32;
            // SAFETY: `pid` is a valid out-pointer.
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            let game = procs::image(pid)
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()))
                .is_some_and(|n| n == "client_release.exe");
            if self.cache.len() > 64 {
                self.cache.clear();
            }
            self.cache.insert(hwnd as isize, game);
            game
        }

        /// A game window in front - or the combined window of fpr_panel.py (it marks itself WCCF_GAME_HOST), unless
        /// its card search box has the keyboard (it sets WCCF_TYPING) and the mouse is not over the game: typing a
        /// player's name presses no buttons. The in-game card browser (forza_panel.dll) sets WCCF_TYPING on the GAME
        /// window itself while it is open - no buttons then either.
        pub fn game_in_front(&mut self) -> bool {
            // SAFETY: no arguments.
            let fg = unsafe { GetForegroundWindow() };
            if !fg.is_null() && self.is_game(fg) {
                self.last = Some(fg);
                return !prop(fg, "WCCF_TYPING");
            }
            // The match engine's window (a child of the client) closed and the focus fell to nothing or the desktop:
            // give it back to the game once. Never when the player went to another program.
            if let Some(last) = self.last.take_if(|_| fg.is_null() || nothing_in_front(&class_of(fg)))
                // SAFETY: plain calls with a window handle.
                && unsafe { IsWindow(last) != 0 && IsWindowVisible(last) != 0 }
            {
                bring_back(last);
                return false;
            }
            if fg.is_null() {
                return false;
            }
            if self.last.is_some() && image_is(fg, "match_release.exe") {
                return false; // the match engine is up: keep `last` for when it closes
            }
            self.last = None;
            if !prop(fg, "WCCF_GAME_HOST") {
                return false;
            }
            let mut pt = POINT { x: 0, y: 0 };
            // SAFETY: `pt` is a valid out-pointer.
            if unsafe { GetCursorPos(&mut pt) } != 0 {
                // SAFETY: plain call with a point by value.
                let under = unsafe { WindowFromPoint(pt) };
                if !under.is_null() && self.is_game(under) {
                    return true;
                }
            }
            !prop(fg, "WCCF_TYPING")
        }
    }

    /// The focus sits on the desktop or the taskbar: nobody chose it, so the game may have it back.
    pub fn nothing_in_front(class: &str) -> bool {
        matches!(class, "Progman" | "WorkerW" | "Shell_TrayWnd")
    }

    /// X, Y, Z, R, U, V, the hat and the buttons (windows-sys has no JOY_RETURNALL).
    const JOY_RETURNALL: u32 = 0xFF;

    #[link(name = "winmm", kind = "raw-dylib")]
    unsafe extern "system" {
        // not in windows-sys
        fn joyConfigChanged(dwflags: u32) -> u32;
    }

    pub struct Winmm;

    impl PadApi for Winmm {
        fn caps(&self, jid: u32) -> Option<Caps> {
            // SAFETY: JOYCAPSW is plain data, valid when zeroed.
            let mut c: JOYCAPSW = unsafe { std::mem::zeroed() };
            // SAFETY: `c` is a valid JOYCAPSW and its size is passed.
            if unsafe { joyGetDevCapsW(jid as usize, &mut c, size_of::<JOYCAPSW>() as u32) } != 0 {
                return None;
            }
            Some(Caps {
                num_buttons: c.wNumButtons,
                num_axes: c.wNumAxes,
                flags: c.wCaps,
                mid: c.wMid,
                pid: c.wPid,
                lim: [
                    (c.wXmin, c.wXmax),
                    (c.wYmin, c.wYmax),
                    (c.wZmin, c.wZmax),
                    (c.wRmin, c.wRmax),
                    (c.wUmin, c.wUmax),
                    (c.wVmin, c.wVmax),
                ],
            })
        }

        fn pos(&self, jid: u32) -> Option<Info> {
            // SAFETY: JOYINFOEX is plain data, valid when zeroed.
            let mut i: JOYINFOEX = unsafe { std::mem::zeroed() };
            i.dwSize = size_of::<JOYINFOEX>() as u32;
            i.dwFlags = JOY_RETURNALL | JOY_RETURNPOVCTS as u32;
            // SAFETY: `i` is a valid JOYINFOEX with its size set.
            if unsafe { joyGetPosEx(jid, &mut i) } != 0 {
                return None;
            }
            Some(Info {
                buttons: i.dwButtons,
                pos: [i.dwXpos, i.dwYpos, i.dwZpos, i.dwRpos, i.dwUpos, i.dwVpos],
                pov: i.dwPOV,
            })
        }

        fn config_changed(&self) {
            // SAFETY: plain call.
            unsafe { joyConfigChanged(0) };
        }
    }

    /// The controller's own name as Windows keeps it under its USB ids ("DualSense Wireless Controller"), else the
    /// ids - winmm's own name is the same for every controller; ASCII (the font).
    pub fn pad_name(caps: &Caps) -> String {
        let key = wide(&format!(
            "System\\CurrentControlSet\\Control\\MediaProperties\\PrivateProperties\\Joystick\\OEM\\VID_{:04X}&PID_{:04X}",
            caps.mid, caps.pid
        ));
        let value = wide("OEMName");
        let mut name = String::new();
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let mut buf = [0u16; 256];
            let mut len = size_of_val(&buf) as u32;
            // SAFETY: the strings are NUL-terminated and outlive the call; `buf` and `len` describe the output buffer.
            let rc = unsafe {
                RegGetValueW(
                    root,
                    key.as_ptr(),
                    value.as_ptr(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut len,
                )
            };
            if rc == 0 {
                let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                name = String::from_utf16_lossy(&buf[..end]).trim().to_string();
            }
            if !name.is_empty() {
                break;
            }
        }
        if name.is_empty() {
            name = format!("controller {:04X}:{:04X}", caps.mid, caps.pid);
        }
        name.chars()
            .map(|c| if (' '..'\u{7f}').contains(&c) { c } else { '?' })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_puts_in_a_coin_only_when_asked() {
        let (start, none) = (BTreeSet::from([0x0D]), BTreeSet::new());
        let mut b = Bindings::defaults();
        assert_eq!(b.step(&start, &none, false, 0).1, 1);
        b.start_coin = false;
        assert_eq!(b.step(&start, &none, false, 0).1, 0);
        assert_eq!(b.step(&BTreeSet::from([0x35]), &none, false, 0).1, 1); // COIN still works
    }

    /// The panel's PAUSE line is no action of the driver's: no note, no key taken from anyone.
    #[test]
    fn pause_line_is_the_panels() {
        let (keys, notes) = load_keys(b"PAUSE=0x58\nSHOOT=0x47\n");
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(keys[action_index("PRESS").unwrap()], 0x58);
        assert_eq!(keys[action_index("SHOOT").unwrap()], 0x47);
    }

    /// Only the desktop and the taskbar count as "nobody chose this": a browser or the Launcher keeps the focus.
    #[cfg(windows)]
    #[test]
    fn only_the_desktop_is_given_back() {
        assert!(win::nothing_in_front("Progman") && win::nothing_in_front("Shell_TrayWnd"));
        assert!(!win::nothing_in_front("Chrome_WidgetWin_1") && !win::nothing_in_front(""));
    }
}
