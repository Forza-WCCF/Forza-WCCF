//! Stand-in for the player cabinet's Club Team Card reader (2010–11), served over a named pipe. Port of
//! `legacy/2010-11/code/hardware/card_reader.py` (step 10, piece D).
//!
//! The game opens "COM1"; the winmm hook, started with `MXHOOK_COM=COM1=PIPE_NAME`, hands it this pipe instead.
//!
//! Link layer, read from the game's side:
//!   - start-up probe: the game sends `02` and only needs ANY byte back within ~110 ms;
//!   - a command: game `02` -> reader `10`; game sends the MESSAGE; reader `10` then `02`; game `10`; reader sends
//!     the REPLY; game sends one ack byte (any value);
//!   - MESSAGE: `[cmd, cmd, len, data.., bcc]`, bcc = XOR of the bytes before it, every `10` doubled, then `10 03`.
//!     REPLY: `[cmd, status, len, data.., bcc]` in the same framing; status 0 = OK.
//!
//! The reader answers as one WITH a card on it. The card lives in the card file (4,112 bytes): every save writes a
//! temporary file in the same folder, flushes it to the disk and replaces the card in one step, so a kill, a crash
//! or a power cut never leaves a short card file; before the first save of a session (no save for 60 s) the card on
//! disk is copied into `backup\` beside it, newest 20 kept. A card file of the wrong size is refused, kept as
//! `<name>.<time>.bad`, and a NEW blank card is served in its place.
//!
//! The optional second file gives the reader room for a second card (a manager transfer): when the club card has
//! expired (its counter's low byte is 1) a blank card with its own id is put beside it, and the game runs the
//! transfer. Which cards lie on the reader is decided only at LoadKey, the start of the game's card check.

use super::Log;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;
use time::{OffsetDateTime, UtcOffset};

/// The card file: a 16-byte header + 256 blocks of 16.
pub const CARD_BYTES: usize = 16 + 256 * 16;
/// No save for this many seconds = a new session: back the card up first.
const BACKUP_GAP: u64 = 60;
/// Backups kept in `backup\` (the newest).
const BACKUP_KEEP: usize = 20;

const STX: u8 = 0x02;
const DLE: u8 = 0x10;
const ETX: u8 = 0x03;

fn cmd_name(cmd: u8) -> &'static str {
    match cmd {
        0x41 => "Request",
        0x42 => "Anticoll",
        0x43 => "Select",
        0x45 => "E?",
        0x4C => "LoadKey",
        0x4E => "RF Control",
        0x50 => "Card Status",
        0x52 => "Read Blocks",
        _ => "?",
    }
}

/// Fills the buffer with random bytes (the kit's `os.urandom`); tests supply their own.
pub type Random = Box<dyn FnMut(&mut [u8]) + Send>;

fn os_random() -> Random {
    Box::new(|b| getrandom::fill(b).expect("the OS random source"))
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `YYYYMMDD-HHMMSS` on this PC's clock (UTC if Windows cannot say the offset).
fn stamp() -> String {
    let utc = OffsetDateTime::now_utc();
    let t = UtcOffset::local_offset_at(utc).map_or(utc, |o| utc.to_offset(o));
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// payload + bcc, every 10 doubled, then 10 03.
fn frame(payload: &[u8]) -> Vec<u8> {
    let bcc = payload.iter().fold(0, |a, b| a ^ b);
    let mut out = Vec::new();
    for &b in payload.iter().chain([&bcc]) {
        out.push(b);
        if b == DLE {
            out.push(DLE);
        }
    }
    out.extend([DLE, ETX]);
    out
}

/// Bytes of one framed message (up to and including 10 03) -> payload incl. bcc, or None while it is incomplete.
fn unframe(raw: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        if b == DLE {
            let next = *raw.get(i + 1)?;
            if next == DLE {
                out.push(DLE);
                i += 2;
                continue;
            }
            if next == ETX {
                return Some(out);
            }
        }
        out.push(b);
        i += 1;
    }
    None
}

#[derive(Debug)]
enum CardError {
    /// The file is not `CARD_BYTES` long: not loaded and not touched.
    Size(String),
    Io(io::Error),
}

/// A Club Team Card's memory and state. The default is a FORMATTED BLANK card the game accepts as NEW (-> Club
/// Make): 256 flat 16-byte blocks.
struct Card {
    uid: [u8; 4],
    status_type: u8,
    blocks: Vec<[u8; 16]>,
    /// block 5's value; 0xFFFF = NEW card
    counter: u16,
    present: bool,
    halted: bool,
    path: Option<PathBuf>,
    log: Option<Log>,
    last_save: Option<Instant>,
    /// the newest card image not yet safely on disk
    pending: Option<Vec<u8>>,
}

impl Card {
    fn new() -> Self {
        let mut blocks = vec![[0u8; 16]; 256];
        blocks[0][..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x22, 0x18, 0x02, 0x00]);
        blocks[4][8..12].copy_from_slice(&[0x95, 0x71, 0x66, 0x40]); // sig dword LE = 0x40667195
        blocks[6][12..].copy_from_slice(&[0x00, 0x98, 0x96, 0x81]); // serial 10,000,001 big-endian
        Self {
            uid: [0xDE, 0xAD, 0xBE, 0xEF],
            status_type: 0x18, // 'P' Card Status byte - anything but 0x20
            blocks,
            counter: 0xFFFF,
            present: true,
            halted: false,
            path: None,
            log: None,
            last_save: None,
            pending: None,
        }
    }

    /// The card in `path`, or the default card when the file does not exist.
    fn open(path: Option<&Path>) -> Result<Self, CardError> {
        let mut c = Self::new();
        c.path = path.map(Path::to_path_buf);
        if let Some(p) = path.filter(|p| p.exists()) {
            c.load(p)?;
        }
        Ok(c)
    }

    /// a fresh BLANK card with its own id, for a transfer: two cards with the kit's one UID could not be told apart
    /// (the game re-selects each card by UID). Block 0 = UID + its check byte (the XOR of the four) + 18 02 00; its
    /// own serial, above the 10,000,000 the game requires.
    fn blank(path: &Path, random: &mut Random) -> Self {
        let mut c = Self::new();
        let mut uid = c.uid;
        // 0x88 is MIFARE's cascade tag, never a UID's first byte
        while uid == c.uid || uid[0] == 0x88 {
            random(&mut uid);
        }
        c.uid = uid;
        c.blocks[0] = [0; 16];
        c.blocks[0][..4].copy_from_slice(&uid);
        c.blocks[0][4] = uid[0] ^ uid[1] ^ uid[2] ^ uid[3];
        c.blocks[0][5..8].copy_from_slice(&[0x18, 0x02, 0x00]);
        let mut r = [0u8; 4];
        random(&mut r[1..]);
        let serial = 10_000_002 + u32::from_be_bytes(r);
        c.blocks[6][12..].copy_from_slice(&serial.to_be_bytes());
        c.path = Some(path.to_path_buf());
        c
    }

    fn load(&mut self, path: &Path) -> Result<(), CardError> {
        let raw = fs::read(path).map_err(CardError::Io)?;
        if raw.len() != CARD_BYTES {
            return Err(CardError::Size(format!(
                "{} is {} bytes, a card file is {CARD_BYTES} - not loaded and not touched; restore it from {}",
                path.display(),
                raw.len(),
                path.parent()
                    .and_then(|p| fs::canonicalize(p).ok())
                    .unwrap_or_default()
                    .join("backup")
                    .display()
            )));
        }
        self.uid.copy_from_slice(&raw[..4]);
        self.status_type = raw[4];
        self.counter = u16::from_le_bytes([raw[5], raw[6]]);
        for (i, b) in self.blocks.iter_mut().enumerate() {
            b.copy_from_slice(&raw[16 + i * 16..32 + i * 16]);
        }
        Ok(())
    }

    fn image(&self) -> Vec<u8> {
        let mut out = self.uid.to_vec();
        out.extend([
            self.status_type,
            self.counter as u8,
            (self.counter >> 8) as u8,
        ]);
        out.extend([0u8; 9]);
        for b in &self.blocks {
            out.extend(b);
        }
        out
    }

    fn note(&self, msg: &str) {
        if let Some(l) = &self.log {
            l(msg);
        }
    }

    /// Copy the card file as it is on disk into `backup\` beside it; keep the newest `BACKUP_KEEP`.
    fn backup(&self) -> io::Result<Option<PathBuf>> {
        let Some(path) = self.path.as_ref().filter(|p| p.exists()) else {
            return Ok(None);
        };
        let folder = fs::canonicalize(path.parent().unwrap_or(Path::new(".")))?.join("backup");
        fs::create_dir_all(&folder)?;
        let base = path.file_name().unwrap_or_default().to_string_lossy();
        let dst = folder.join(format!("{base}.{}.bak", stamp()));
        fs::copy(path, &dst)?;
        let mut old: Vec<String> = fs::read_dir(&folder)?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|f| f.starts_with(&format!("{base}.")) && f.ends_with(".bak"))
            .collect();
        old.sort();
        for f in &old[..old.len().saturating_sub(BACKUP_KEEP)] {
            fs::remove_file(folder.join(f))?;
        }
        Ok(Some(dst))
    }

    /// The whole card goes to a temporary file beside the card, is flushed to the disk, then replaces the card in one
    /// step: a kill, a crash or a power cut at any moment leaves the old card or the new one, never a short file. If
    /// the disk write cannot complete (the file stays busy for 1 s - an antivirus scan, a backup tool), the newest
    /// card is KEPT IN MEMORY and retried later; the card is NEVER written in place.
    fn save(&mut self) {
        if self.path.is_none() {
            return;
        }
        let now = Instant::now();
        if self
            .last_save
            .is_none_or(|t| now.duration_since(t).as_secs_f64() > BACKUP_GAP as f64)
        {
            match self.backup() {
                Ok(Some(kept)) => self.note(&format!("  card backed up -> {}", kept.display())),
                Ok(None) => {}
                Err(e) => self.note(&format!("  card backup FAILED ({e}) - saving anyway")),
            }
        }
        self.last_save = Some(now);
        self.pending = Some(self.image());
        self.flush();
    }

    /// Write `pending` atomically; on failure keep it for the next try.
    fn flush(&mut self) {
        let Some(data) = &self.pending else { return };
        match self.replace_with(data) {
            // the game is mid-session: never stop the reader over a save
            Ok(()) => self.pending = None,
            Err(e) => self.note(&format!(
                "  card NOT saved to disk this time ({e}) - kept safe in memory; the card on disk is still the last good one, a retry will save it"
            )),
        }
    }

    fn replace_with(&self, data: &[u8]) -> io::Result<()> {
        let path = self.path.as_ref().expect("a card with a path");
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
        let mut tmp = NamedTempFile::new_in(dir.unwrap_or(Path::new(".")))?;
        tmp.write_all(data)?;
        tmp.as_file().sync_all()?; // on the disk before it replaces the card (a power cut)
        for attempt in 0..50 {
            match tmp.persist(path) {
                Ok(_) => {
                    if attempt > 0 {
                        self.note(&format!(
                            "  card saved after {attempt} refused replaces (the file was busy)"
                        ));
                    }
                    return Ok(());
                }
                // another program holds the card file for a moment (access denied, sharing violation)
                Err(e) if busy(&e.error) => {
                    tmp = e.file;
                    sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e.error),
            }
        }
        Err(io::Error::other("the card file stayed busy for 1 s"))
    }
}

fn busy(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32)
}

/// The club card for the reader, or - if the file is unreadable (wrong size) - a NEW blank card so the game still
/// opens and the CLUB CARD panel can switch to or restore one (never lock the player out). The unreadable file is
/// kept as `<name>.<time>.bad` and never written over.
fn open_card(cardfile: Option<&Path>, log: &dyn Fn(&str)) -> io::Result<Card> {
    match Card::open(cardfile) {
        Ok(c) => Ok(c),
        Err(CardError::Io(e)) => Err(e),
        Err(CardError::Size(e)) => {
            log(&format!("CARD FILE REFUSED: {e}"));
            let path = cardfile.expect("a size error comes from a file");
            let bad = format!("{}.{}.bad", path.display(), stamp());
            match fs::copy(path, &bad) {
                Ok(_) => log(&format!("  the unreadable file is kept as {bad}")),
                Err(e2) => log(&format!("  could not keep a copy of it ({e2})")),
            }
            let mut card = Card::new();
            card.path = Some(path.to_path_buf());
            log(
                "  a NEW blank card is on the reader so the game still opens - restore your club from data\\save\\backup if you have a good copy",
            );
            Ok(card)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Which {
    Main,
    Second,
}

/// Answers one command (cmd, data) -> (status, reply data). status 0 = OK. Implements the 12 commands the game
/// uses; block 4 writes MUST fail (a successful one = the game rejects the card). The cards on the reader are
/// `field`; `card` is the selected one, which every block command reads or writes. As a real reader: Anticoll
/// answers with the first card that is awake, a Halt puts the selected card to sleep, so the next Request reaches
/// the other one, and RF Control wakes them all.
struct Reader {
    main: Card,
    log: Log,
    new_path: Option<PathBuf>,
    /// the card beside it: a blank for a transfer, then the new club
    second: Option<Card>,
    /// NEWCARDFILE could not be loaded: never written over
    bad_new: bool,
    field: Vec<Which>,
    card: Which,
    random: Random,
}

impl Reader {
    fn new(card: Card, log: Log, new_path: Option<PathBuf>, random: Random) -> Self {
        Self {
            main: card,
            log,
            new_path,
            second: None,
            bad_new: false,
            field: vec![Which::Main],
            card: Which::Main,
            random,
        }
    }

    fn get(&self, w: Which) -> &Card {
        match w {
            Which::Main => &self.main,
            Which::Second => self.second.as_ref().expect("a second card"),
        }
    }

    fn get_mut(&mut self, w: Which) -> &mut Card {
        match w {
            Which::Main => &mut self.main,
            Which::Second => self.second.as_mut().expect("a second card"),
        }
    }

    /// which cards lie on the reader for the card check now starting
    fn lay_out(&mut self) {
        let Some(new_path) = self.new_path.clone() else {
            return;
        };
        if self.second.is_none() && !self.bad_new && new_path.exists() {
            let mut c = Card::new();
            c.path = Some(new_path.clone());
            match c.load(&new_path) {
                Ok(()) => {
                    c.log = self.main.log.clone();
                    self.second = Some(c);
                }
                Err(e) => {
                    self.bad_new = true;
                    // ponytail: the Python lets an I/O error end the reader; here it counts as a refusal
                    let why = match e {
                        CardError::Size(s) => s,
                        CardError::Io(e) => e.to_string(),
                    };
                    (self.log)(&format!(
                        "NEW CARD FILE REFUSED: {why} - only the club card is on the reader"
                    ));
                }
            }
        }
        let mut low = self.main.counter & 0xFF;
        if low == 0 && self.second.as_ref().is_some_and(|s| s.counter == 0xFFFF) {
            // the publisher's order marks the old card used up (job 4) BEFORE it writes the new one: a cut in
            // between leaves the old club untouched on a "used up" card and a still-blank new one - set the old
            // card back to expired and the transfer simply runs again
            self.main.counter = 0xFF01;
            self.main.save();
            (self.log)(
                "  the last transfer was cut half way - the old card is expired again (0xFF01), it runs again",
            );
            low = 1;
        }
        let field = if low == 0 && self.second.is_some() {
            vec![Which::Second] // transfer done: the old card is off the reader
        } else if low == 1 && !self.bad_new {
            if self.second.is_none() {
                let mut sec = Card::blank(&new_path, &mut self.random);
                sec.log = self.main.log.clone();
                sec.save();
                (self.log)(&format!(
                    "  contract ended: a NEW blank card (id {}) is on the reader beside the club card - the game moves the manager to it",
                    hex(&sec.uid)
                ));
                self.second = Some(sec);
            }
            if self.second.as_ref().is_some_and(|s| s.counter == 0xFFFF) {
                vec![Which::Main, Which::Second]
            } else {
                (self.log)(&format!(
                    "  {} is a written card, not a blank - not put beside the expired one",
                    new_path.display()
                ));
                vec![Which::Main]
            }
        } else {
            vec![Which::Main]
        };
        if field != self.field {
            let list: Vec<String> = field
                .iter()
                .map(|&w| {
                    let c = self.get(w);
                    format!("{} (counter 0x{:04X})", hex(&c.uid), c.counter)
                })
                .collect();
            (self.log)(&format!("  on the reader now: {}", list.join(", ")));
        }
        self.field = field;
        if !self.field.contains(&self.card) {
            self.card = self.field[0];
        }
    }

    /// A periodic retry of any card whose last save could not reach the disk; called while the port is idle.
    fn retry_saves(&mut self) {
        if self.main.pending.is_some() {
            self.main.flush();
        }
        if let Some(c) = self.second.as_mut().filter(|c| c.pending.is_some()) {
            c.flush();
        }
    }

    fn awake(&self) -> Vec<Which> {
        let mut v = self.field.clone();
        v.retain(|&w| {
            let c = self.get(w);
            c.present && !c.halted
        });
        v
    }

    fn answer(&mut self, cmd: u8, data: &[u8]) -> (u8, Vec<u8>) {
        // ponytail: the Python dies with an IndexError on a command whose data is too short; a missing byte is 0 here
        let d = |i: usize| data.get(i).copied().unwrap_or(0);
        let le16 = |i: usize| usize::from(d(i)) | (usize::from(d(i + 1)) << 8);
        let sel = self.card;
        match cmd {
            0x4E => {
                // 'N' RF Control: wakes every card on the reader
                for w in self.field.clone() {
                    self.get_mut(w).halted = false;
                }
                (0, vec![])
            }
            0x4C => {
                // 'L' LoadKey: accept any key - the game's card check starts here
                self.lay_out();
                (0, vec![])
            }
            0x41 => {
                // 'A' Request: is a card awake?
                (if self.awake().is_empty() { 1 } else { 0 }, vec![])
            }
            0x42 => {
                // 'B' Anticoll -> the first awake card's 4-byte UID
                match self.awake().first() {
                    Some(&w) => (0, self.get(w).uid.to_vec()),
                    None => (1, vec![]),
                }
            }
            0x43 => {
                // 'C' Select: the card with that UID
                let want: Vec<u8> = data.iter().take(4).copied().collect();
                for w in self.field.clone() {
                    if self.get(w).uid[..] == want[..] {
                        self.card = w;
                        return (0, vec![]);
                    }
                }
                (1, vec![])
            }
            0x45 => {
                // 'E' Halt
                self.get_mut(sel).halted = true;
                (0, vec![])
            }
            0x49 => {
                // 'I' Decrement block 5 by amount (data: 05 00 lo hi)
                let amount = if data.len() >= 4 {
                    u16::from(data[2]) | (u16::from(data[3]) << 8)
                } else {
                    0
                };
                let counter = self.get(sel).counter.wrapping_sub(amount);
                self.get_mut(sel).counter = counter;
                if amount != 0 {
                    (self.log)(&format!("  counter -{amount} -> 0x{counter:04X}"));
                    self.get_mut(sel).save();
                }
                (0, vec![counter as u8, (counter >> 8) as u8, 0, 0])
            }
            0x50 => (0, vec![self.get(sel).status_type]), // 'P' Card Status -> type byte
            0x52 => {
                // 'R' Read Blocks: start LE16, count (always 0x0F)
                let (start, count) = (le16(0), usize::from(d(2)));
                let c = self.get(sel);
                let mut out: Vec<u8> = (0..count)
                    .flat_map(|i| c.blocks[(start + i) & 0xFF])
                    .collect();
                // ponytail: the reply's length is one byte; the Python dies on 16 blocks or more, 255 bytes are sent here
                out.truncate(255);
                (0, out)
            }
            0x54 => (0, self.get(sel).blocks[le16(0) & 0xFF].to_vec()), // 'T' Read one block
            0x55 => {
                // 'U' Write one block: block LE16 + 16 bytes
                let blk = le16(0) & 0xFF;
                if blk == 4 {
                    return (4, vec![]); // block 4 is write-protected - MUST fail
                }
                let c = self.get_mut(sel);
                c.blocks[blk] = block_of(data, 2);
                c.save();
                (0, vec![])
            }
            0x53 => {
                // 'S' Write Blocks: start LE16, n, n*16 bytes
                let (start, n) = (le16(0), usize::from(d(2)));
                let c = self.get_mut(sel);
                for i in 0..n {
                    let blk = (start + i) & 0xFF;
                    if blk != 4 {
                        c.blocks[blk] = block_of(data, 3 + i * 16);
                    }
                }
                c.save();
                (0, vec![])
            }
            _ => {
                (self.log)(&format!("  (unknown command {cmd:02x} - accepted)"));
                (0, vec![])
            }
        }
    }
}

/// `data[from..from + 16]`, cut short or zero-padded to 16 bytes.
fn block_of(data: &[u8], from: usize) -> [u8; 16] {
    let mut b = [0u8; 16];
    let s = data.get(from..).unwrap_or(&[]);
    let n = s.len().min(16);
    b[..n].copy_from_slice(&s[..n]);
    b
}

#[derive(PartialEq)]
enum State {
    Idle,
    Msg,
    WaitDle,
    WaitAck,
}

/// One connection's link layer: bytes in from the game, bytes out to it.
struct Link {
    state: State,
    raw: Vec<u8>,
    cmd_seen: (u8, u8, Vec<u8>),
}

impl Link {
    fn new() -> Self {
        Self {
            state: State::Idle,
            raw: vec![],
            cmd_seen: (0, 0, vec![]),
        }
    }

    fn feed(&mut self, incoming: &[u8], reader: &mut Reader, out: &mut Vec<u8>) {
        let log = reader.log.clone();
        let mut send = |data: &[u8], what: &str| {
            out.extend(data);
            log(&format!("  -> {what:<10} {}", hex(data)));
        };
        let state_name = |s: &State| match s {
            State::Idle => "IDLE",
            State::Msg => "MSG",
            State::WaitDle => "WAIT_DLE",
            State::WaitAck => "WAIT_ACK",
        };
        (reader.log)(&format!(
            "<-  {}   (state {})",
            hex(incoming),
            state_name(&self.state)
        ));
        for &b in incoming {
            match self.state {
                State::Idle => {
                    if b == STX {
                        send(&[DLE], "DLE");
                        self.state = State::Msg;
                        self.raw.clear();
                    } else {
                        (reader.log)(&format!("  (ignored stray byte {b:02x})"));
                    }
                }
                State::Msg => {
                    if self.raw.is_empty() && b == STX {
                        send(&[DLE], "DLE"); // the game started over
                        continue;
                    }
                    self.raw.push(b);
                    let Some(payload) = unframe(&self.raw) else {
                        continue;
                    };
                    // ponytail: the Python dies on a frame without cmd/len; it is dropped here
                    if payload.len() < 3 {
                        (reader.log)("  (frame too short - ignored)");
                        self.state = State::Idle;
                        continue;
                    }
                    let bcc = payload[..payload.len() - 1].iter().fold(0, |a, b| a ^ b);
                    let (cmd, length) = (payload[0], payload[2]);
                    let data = &payload[3..payload.len() - 1];
                    let ok = if bcc == payload[payload.len() - 1] {
                        "bcc ok".to_string()
                    } else {
                        format!("BCC WRONG (want {bcc:02x})")
                    };
                    (reader.log)(&format!(
                        "  CMD {cmd:02x} {:<11} len {length} data [{}]  {ok}",
                        cmd_name(cmd),
                        hex(data)
                    ));
                    let (status, rdata) = reader.answer(cmd, data);
                    self.cmd_seen = (cmd, status, rdata);
                    send(&[DLE, STX], "DLE STX");
                    self.state = State::WaitDle;
                }
                State::WaitDle => {
                    if b == DLE {
                        let (cmd, status, rdata) = &self.cmd_seen;
                        let mut p = vec![*cmd, *status, rdata.len() as u8];
                        p.extend(rdata);
                        send(&frame(&p), &format!("REPLY st={status}"));
                        self.state = State::WaitAck;
                    } else if b == STX {
                        send(&[DLE], "DLE");
                        self.state = State::Msg;
                        self.raw.clear();
                    }
                }
                State::WaitAck => self.state = State::Idle, // any byte is the ack
            }
        }
    }
}

#[cfg(windows)]
pub use pipe::{spawn, spawn_with_random};

#[cfg(windows)]
mod pipe {
    use super::*;
    use crate::standins::Handle;
    use std::fs::File;
    use std::io::Read;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use windows_sys::Win32::Foundation::{
        ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_NOWAIT, PIPE_READMODE_BYTE,
        PIPE_TYPE_BYTE, PeekNamedPipe,
    };

    /// Creates the named pipe `pipe_name` (`\\.\pipe\...`, one instance) and serves the reader on it until the
    /// returned handle is dropped. `card_file` holds the club card (made on its first save when missing);
    /// `new_card_file` gives room for a second card (a manager transfer). Fails when the pipe cannot be made
    /// (another reader running) or the card file cannot be read.
    pub fn spawn(
        pipe_name: &str,
        card_file: Option<&Path>,
        new_card_file: Option<&Path>,
        log: Log,
    ) -> io::Result<Handle> {
        spawn_with_random(pipe_name, card_file, new_card_file, log, os_random())
    }

    /// [`spawn`] with the source of the new card's id and serial replaced (tests replay recorded bytes).
    pub fn spawn_with_random(
        pipe_name: &str,
        card_file: Option<&Path>,
        new_card_file: Option<&Path>,
        log: Log,
        random: Random,
    ) -> io::Result<Handle> {
        let mut card = open_card(card_file, &*log)?;
        card.log = Some(log.clone());
        let wide: Vec<u16> = pipe_name.encode_utf16().chain([0]).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call; no security attributes are passed.
        let raw = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT, // no-wait: every call returns, so `stop` is seen
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is a valid pipe handle we own; the File closes it.
        let mut pipe = File::from(unsafe { OwnedHandle::from_raw_handle(raw) });
        let kind = if card.counter == 0xFFFF {
            "NEW (blank, counter 0xFFFF)".to_string()
        } else {
            format!("existing (counter 0x{:04X})", card.counter)
        };
        log(&format!(
            "card reader stand-in on {pipe_name}; a card IS on the reader: {kind}{}{}",
            card_file.map_or(String::new(), |p| format!(" <- {}", p.display())),
            new_card_file
                .filter(|_| card_file.is_some())
                .map_or(String::new(), |p| format!(
                    "; room for a second card (a manager transfer) in {}",
                    p.display()
                ))
        ));
        let new_path = new_card_file
            .filter(|_| card_file.is_some())
            .map(Path::to_path_buf);
        let mut reader = Reader::new(card, log.clone(), new_path, random);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::spawn(move || {
            let h = pipe.as_raw_handle();
            let mut last_retry = Instant::now(); // throttle for retrying a save that could not reach the disk
            let mut buf = [0u8; 4096];
            while !flag.load(Ordering::Relaxed) {
                log("waiting for the game to open its COM port...");
                let connected = loop {
                    // SAFETY: `h` is the pipe handle `pipe` owns; a null overlapped is allowed for a non-overlapped pipe.
                    let ok = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0
                        // SAFETY: reads the calling thread's last error.
                        || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                    if ok {
                        break true;
                    }
                    if flag.load(Ordering::Relaxed) {
                        break false;
                    }
                    thread::sleep(Duration::from_millis(10));
                };
                if !connected {
                    break;
                }
                log("game connected");
                let mut link = Link::new();
                while !flag.load(Ordering::Relaxed) {
                    let mut avail = 0u32;
                    // SAFETY: `h` is valid; only the bytes-available out-pointer is given.
                    let alive = unsafe {
                        PeekNamedPipe(
                            h,
                            std::ptr::null_mut(),
                            0,
                            std::ptr::null_mut(),
                            &mut avail,
                            std::ptr::null_mut(),
                        )
                    };
                    if alive == 0 {
                        log("game closed the port");
                        break;
                    }
                    if avail == 0 {
                        if last_retry.elapsed() > Duration::from_secs(2) {
                            reader.retry_saves();
                            last_retry = Instant::now();
                        }
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    let Ok(n) = pipe.read(&mut buf[..(avail as usize).min(4096)]) else {
                        continue;
                    };
                    let mut out = Vec::new();
                    link.feed(&buf[..n], &mut reader, &mut out);
                    let _ = pipe.write_all(&out);
                }
                // SAFETY: `h` is the pipe handle `pipe` owns.
                unsafe { DisconnectNamedPipe(h) };
            }
            log("stopping");
        });
        Ok(Handle::new(stop, thread))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn quiet() -> Log {
        Arc::new(|_| {})
    }

    fn card_at(path: &Path) -> Card {
        let mut c = Card::open(Some(path)).unwrap_or_else(|_| panic!("card"));
        c.log = Some(quiet());
        c
    }

    /// Port of `test_icc_reader.py`: atomic saves, the session backup, a failed save kept in memory, a wrong-size
    /// file kept as .bad with a NEW card served.
    #[test]
    fn icc_reader_saving_and_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("club.bin");

        // 1. a NEW card saves atomically: exactly CARD_BYTES, loads back equal, no temporary file left
        let mut c = card_at(&path);
        c.blocks[7][0] = 0x55;
        c.save();
        assert_eq!(fs::metadata(&path).unwrap().len() as usize, CARD_BYTES);
        let names = |d: &Path| {
            let mut v: Vec<_> = fs::read_dir(d)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            v.sort();
            v
        };
        assert!(
            !names(dir.path())
                .iter()
                .any(|n| n.ends_with(".tmp") || n.starts_with(".tmp"))
        );
        assert_eq!(card_at(&path).blocks[7][0], 0x55);

        // 2. a new session backs the card up first
        c.last_save = None;
        c.blocks[7][0] = 0x56;
        c.save();
        let baks = names(&dir.path().join("backup"));
        assert!(baks.len() == 1 && baks[0].ends_with(".bak"));
        // a save inside the 60 s gap makes no new backup
        c.save();
        assert_eq!(names(&dir.path().join("backup")).len(), 1);

        // 3. a save that cannot reach the disk never writes in place; the newest card is kept in memory
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let good = fs::read(&path).unwrap();
            let held = fs::OpenOptions::new()
                .read(true)
                .share_mode(1) // FILE_SHARE_READ: a replace is refused while it is open
                .open(&path)
                .unwrap();
            c.blocks[7][0] = 0x99;
            c.save();
            assert!(
                c.pending.is_some(),
                "a failed save keeps the newest card in memory"
            );
            assert_eq!(
                fs::read(&path).unwrap(),
                good,
                "the on-disk card is unchanged"
            );
            drop(held);
            c.flush(); // the file is free again
            assert!(c.pending.is_none());
            assert_eq!(card_at(&path).blocks[7][0], 0x99);
        }

        // 4. a wrong-size card file does not lock out: kept as .bad, a NEW card is served
        fs::write(&path, [1u8; 100]).unwrap();
        let msgs = std::sync::Mutex::new(Vec::<String>::new());
        let mut card =
            open_card(Some(&path), &|m| msgs.lock().unwrap().push(m.to_string())).unwrap();
        assert_eq!(card.counter, 0xFFFF);
        assert_eq!(card.path.as_deref(), Some(path.as_path()));
        let bad: Vec<_> = names(dir.path())
            .into_iter()
            .filter(|n| n.ends_with(".bad"))
            .collect();
        assert!(bad.len() == 1 && fs::read(dir.path().join(&bad[0])).unwrap() == [1u8; 100]);
        card.log = Some(quiet());
        card.save();
        assert_eq!(fs::metadata(&path).unwrap().len() as usize, CARD_BYTES);

        // 5. a good card file loads without a refusal
        msgs.lock().unwrap().clear();
        let card2 = open_card(Some(&path), &|m| msgs.lock().unwrap().push(m.to_string())).unwrap();
        assert_eq!(card2.counter, 0xFFFF);
        assert!(!msgs.lock().unwrap().iter().any(|m| m.contains("REFUSED")));
    }

    /// Backups older than the newest 20 go; other files stay.
    #[test]
    fn backup_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("club.bin");
        let folder = dir.path().join("backup");
        fs::create_dir(&folder).unwrap();
        for i in 0..25 {
            fs::write(folder.join(format!("club.bin.20200101-{i:06}.bak")), b"x").unwrap();
        }
        fs::write(folder.join("other.bin.20200101-000000.bak"), b"x").unwrap();
        let mut c = card_at(&path);
        c.save(); // no file yet: no backup
        c.last_save = None;
        c.save(); // backs up the file just made
        let mut left: Vec<_> = fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left.len(), 21);
        assert_eq!(left[0], "club.bin.20200101-000006.bak");
        assert_eq!(left[18], "club.bin.20200101-000024.bak");
        assert!(left[19].starts_with("club.bin.2") && left[19] > left[18]);
        assert_eq!(left[20], "other.bin.20200101-000000.bak");
    }

    #[test]
    fn frame_round_trip() {
        let f = frame(&[0x4C, 0x4C, 0x10, 0x01]);
        assert_eq!(f, [0x4C, 0x4C, 0x10, 0x10, 0x01, 0x11, 0x10, 0x03]);
        assert_eq!(unframe(&f).unwrap(), [0x4C, 0x4C, 0x10, 0x01, 0x11]);
        assert!(unframe(&f[..f.len() - 1]).is_none());
    }
}
