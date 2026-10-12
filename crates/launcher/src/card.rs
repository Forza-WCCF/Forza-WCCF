//! Is a club card session open?  The game marks one open on the card itself: at START it writes USER_INJUSTICE_FLAG = 1
//! (and counts the session in USER_INJUSTICE_NUM if the last one never closed), and the locker-room save after each
//! match writes 0.  A cut in between (a crash, a stop) loses that match and costs a "bad ending" (penalties from 2).
//! Positions from the program's own field tables (docs\research\CLUB-CARD-2010-11.md): copy A = blocks 8-112, copy B =
//! blocks 113-217, each 0x68C bytes of MSB-first bits and a checksum the game reads.  Port of kit_common.card_session
//! and play.py's club_session.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

/// Seat 1's club card in data\save.
pub const SLOT: &str = "seat1_club.bin";
/// The second card of a manager transfer, beside it.
pub const NEW: &str = "seat1_club.new.bin";

/// The kit's card file: a 16-byte header and 256 blocks of 16 bytes.
const CARD_BYTES: usize = 16 + 256 * 16;
/// Where copy A and copy B start in the file.
const COPIES: [usize; 2] = [16 + 8 * 16, 16 + 8 * 16 + 0x690];
const PAYLOAD: usize = 0x68C;
const FLAG_BIT: usize = 595; // USER_INJUSTICE_FLAG
const COUNT_BIT: usize = 596; // USER_INJUSTICE_NUM, 7 bits
const COUNT_BITS: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    /// No card file.
    None,
    /// Wrong size, an empty copy, or no copy with a good checksum.
    Unreadable,
    /// No club made yet.
    New,
    /// Marked open, and written since this run started: a match or Club Make is on.
    Open,
    /// Marked open but not written since this run started: the last session never closed.
    Cut,
    /// The last save was a locker-room save.
    Closed,
}

impl Session {
    /// kit_common's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Unreadable => "unreadable",
            Self::New => "new",
            Self::Open => "open",
            Self::Cut => "cut",
            Self::Closed => "closed",
        }
    }

    /// What a stop says about it (play.py's SESSION_WORDS).
    pub fn words(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Cut => "the last one did not end normally",
            Self::Closed => "closed",
            Self::New => "new card",
            Self::None => "no card file",
            Self::Unreadable => "card file unreadable",
        }
    }
}

fn bits(buf: &[u8], start: usize, width: usize) -> u32 {
    (start..start + width).fold(0, |v, i| (v << 1) | u32::from((buf[i >> 3] >> (7 - (i & 7))) & 1))
}

fn word(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// The session state and bad endings (when known) of a card's bytes, read as the game reads it: copy A if its checksum
/// is good, else copy B.  `written_since`: the file was written after the run started (None: every mark counts as open).
pub fn session_of(raw: &[u8], written_since: Option<bool>) -> (Session, Option<u32>) {
    if raw.len() != CARD_BYTES {
        return (Session::Unreadable, None);
    }
    if u16::from_le_bytes([raw[5], raw[6]]) == 0xFFFF {
        return (Session::New, None);
    }
    let copies = COPIES.map(|start| {
        let copy = &raw[start..start + 0x690];
        let total = (0..PAYLOAD).step_by(4).fold(0u32, |t, i| t.wrapping_add(word(copy, i)));
        (copy, total, word(copy, PAYLOAD))
    });
    if copies.iter().any(|&(_, total, stored)| total == 0 && stored == 0) {
        return (Session::Unreadable, None); // the game refuses a card with an empty copy (error 0xA)
    }
    for (copy, total, stored) in copies {
        if stored == total || stored == total.wrapping_add(1) {
            let bad = Some(bits(copy, COUNT_BIT, COUNT_BITS));
            if bits(copy, FLAG_BIT, 1) == 0 {
                return (Session::Closed, bad);
            }
            return (
                if written_since == Some(false) {
                    Session::Cut
                } else {
                    Session::Open
                },
                bad,
            );
        }
    }
    (Session::Unreadable, None)
}

/// The session state of a card file; `since` = when this run started.
pub fn session(card: &Path, since: Option<SystemTime>) -> (Session, Option<u32>) {
    let (Ok(raw), Ok(meta)) = (fs::read(card), fs::metadata(card)) else {
        return (Session::None, None);
    };
    let written_since = since.map(|s| meta.modified().is_ok_and(|m| m > s));
    session_of(&raw, written_since)
}

/// The club in play: the slot card, or during a manager transfer the new card beside it - an open session on either one
/// is open.
pub fn club_session(slot: &Path, new: &Path, since: Option<SystemTime>) -> (Session, Option<u32>) {
    let (s, bad) = session(slot, since);
    let (n, new_bad) = session(new, since);
    if n == Session::Open || (n == Session::Cut && s != Session::Open) {
        (n, new_bad)
    } else {
        (s, bad)
    }
}
