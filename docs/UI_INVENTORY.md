# UI inventory (step 9.1)

Status 2026-10-27: a historical design input. Step 9 is done: the screens are in `crates/forza-ui`, the panel is
`crates/panel`, and the C panels surveyed below were removed (#28). The "today" notes describe 2026-10-12.

What each screen of Forza-WCCF does and needs, for step 9's new design. It names no current visuals on purpose: the
design starts from this list, `docs/CABINET_LOOK.md` and the constraints at the end. Surveyed 2026-10-12 from
`crates/launcher`, `crates/installer`, `native/2010-11/panel/wccfpanel.c` and `native/2017-18/overlay/wccfpanel.c`.

Three programs: the **launcher** (a desktop window, before and around a run), the **installer** (a wizard, once), and
the **panel** (inside the game window, around the game picture, during play). All three show text in English, Italian
and Japanese; English is the source and the fallback.

## Launcher

Purpose: start and stop the game, set it up, update Forza-WCCF. One window; it stays usable while the game runs.

- **Play**
  - Where to play: on this PC, or online with a server address (IPv4; an invalid address is refused with a message).
  - Game text: English, Italian or the original Japanese. Applied at the next start; switching takes a minute of
    work by a script, with progress.
  - PLAY. While the game runs the same action becomes STOP THE GAME, with a confirmation; a stop can be refused while
    a club card session is open (the reason is shown).
  - Progress of a start (a step count, `N/M`) and a one-line status.
- **Launcher language**: English, Italiano, 日本語. Order of choice: the `WCCF_KIT_LANG` variable, the saved choice,
  the Windows UI language, English.
- **Setup** (folded away until needed; opens by itself when no game folder is set)
  - Game folder: typed, browsed, or dropped onto the window.
  - Set up / repair; undo setup (with a confirmation).
  - "Not set up yet" state: PLAY explains what to do.
- **Update**
  - This version; a list of releases (tag, kind, date, marks "this version" and "newer"; test builds listed but never
    offered as newer).
  - UPDATE with a confirmation; what's new (opens the release notes in the browser).
  - After an update: offer to set up the game folder again (and re-apply the game text).
  - Errors: no answer from the server (retry), download failed, unzip failed, not a Forza-WCCF package, cannot write,
    close the game first.
- **Output**: the running script's output, read-only, for when something fails ("the output below says why").
- **First run**: if the folder's path is not plain ASCII, link the folder to `C:\Games\Forza-WCCF\<edition>` and
  restart from there; messages when the name is taken, the drive cannot hold a link, or linking fails.
- Busy state: closing is refused while a job runs, with a message.
- Command line (no window shown): `local`, `server`, `remote ADDRESS`, `debug`, `stop`, `status`, `show`, `restart`,
  a folder path (setup), `undo`.
- SAVE LOGS and REPORT A PROBLEM write the logs ZIP to the Downloads folder and show its path.

## Installer

Purpose: make a game installation from the player's disc image, once per edition. A wizard with Back / Next.

1. **Welcome**: what is needed (disc image `.iso` of about 5–6 GB, its key file, optionally the 2010–11 package),
   about 6 GB free.
2. **Where**: install folder (typed or browsed; default `C:\Games\Forza-WCCF`), free space on that drive (warning
   below 6 GB). The two editions live side by side in `2010-2011` and `2017-2018`.
3. **Files**: disc image and key file (browse or drop), or reuse an edition already installed there (no disc needed);
   the optional 2010–11 package. Shows the detected edition, or why the image is not supported.
4. **Options**: check the disc image first; set up the game (2010–11 with the package); play in English; desktop
   shortcuts. The Next action reads Install.
5. **Installing**: current stage, progress, details log (folded), Cancel; on failure, the reason and Back.
6. **Done**: where it is installed, what was and was not done, open the folder, close.
- A headless command line exists (`--iso --key --dest ...`) and needs no screen.
- Today English only.

## Panel (in the game window, both editions)

Purpose: everything a cabinet has that a PC lacks, around the game picture. Two strips beside the picture; pages open
over the picture one at a time, and while a page is open its keys do not reach the game.

- **Cabinet buttons**: CENTRAL, COUNTER, L-SIDE, R-SIDE (tactics), PRESS, DATA, SHOOT, KEEPER, KEY PLAYER, START,
  CARD; 2017–18 adds COIN and a BACK placeholder that sends nothing yet. Press and hold send the bound key's
  down/up. The bindings come from KEYS, so a change applies at once.
- **Formation board** (the card table): a pitch with up to 16 player cards; 11 on the pitch, the rest on the bench.
  - Drag a card to move it; drop on another card to swap (the target is marked while dragging).
  - Dragging a card off the board takes it off (no right-click: a long press on a touch screen is one).
  - Resting the mouse on a card shows it big with a caption after `hover_delay_ms` (2010–11: setting, default 1000).
  - Every move updates the game's card table at once.
- **Catalogue** ("+ CARD"): find a card and put it on the table.
  - Search line (words ANDed over name, club, country, season, position, role; digits match the card number).
  - Filters: position (ALL, GK, DF, MF, FW), rarity (ANY, ALL-TIME, LEGEND, RARE, SPECIAL, REGULAR), country, club.
  - Sort: NAME, TOTAL, OFF, DEF, TEC, POW, SPD, STA.
  - Range sliders: OFF, DEF, TEC, POW, SPD, STA (1–20), TOTAL (60–100); RESET.
  - Results with card pictures; a stats pane for the card under the mouse (position, club, country, TOTAL, six stats
    with bars, an inferred role labelled as inferred).
  - Messages: added, already on the table, table full (16).
  - Typing searches; Backspace; the wheel scrolls.
- **Keys**: 16 actions (START, PRESS, SHOOT, KEEPER, DATA, KEY PLAYER, CARD, COIN, UP, DOWN, LEFT, RIGHT, TEST,
  SERVICE, SW1, SW2), each with a keyboard key and a controller button. Click, then press; a key taken by another
  action swaps over; right-click clears; only allowed keys. Controllers are found when plugged in.
- **Settings**
  - Now (status, refreshed every second): server (this PC / online, linked or not), club card session, bad endings
    count, last saved, latest backup, game text; the version.
  - Next start: play on this PC / online + address; game text; RESTART NOW (click twice within 8 s; refused during a
    card session; says if no helper took it).
  - View: layout CABINET (strips with buttons) or COMPACT (no cabinet buttons); card hover delay (2010–11: 0.5, 1, 2,
    3 s).
  - Logs: SAVE LOGS (the ZIP goes to the Downloads folder; shows the path or why it failed).
- **Club card**: the player's saved club, read-only: session open / closed / did not end normally / new / unreadable /
  none yet. YOUR CARDS: other saved clubs; pick one (or a new card) for the next start, then RESTART NOW; not during a
  session. 2010–11 also: CLEAR BAD ENDINGS (click twice). Esc closes.
- **Card deal**: after a match, the dealt card is shown.
- **Messages**: one line, three tones (done, refused or failed, notice), timed.
- **Online**: a ping meter (shown only when playing online).
- **F11**: maximized or window.
- **Projector** (the second game window): no panel pages; only the money display (amounts with thousands separators,
  fitted to their boxes).

Differences today, which step 9 removes (one panel for both editions): 2010–11 alone has the hover setting, SEND LOGS,
automatic reports and CLEAR BAD ENDINGS; 2017–18 alone has the COIN button and default keys for SW1/SW2.

## Text to carry over translated

English only today, so step 9 ships it in English, Italian and Japanese: the whole panel (no language mechanism in
the C panel), the installer, the launcher's console messages, and the SEND LOGS messages. The launcher window's words
are already in all three (`crates/launcher/src/bin/launcher/text.rs`).

## Constraints

- Text lengths: Italian runs about 30 % longer than English; Japanese needs a CJK font and is shorter in characters
  but wider per glyph. Every label must fit in all three.
- Panel geometry: the game draws at 1440×900. The panel works on a 1440×900 design sheet: left strip 0–190, right
  strip 1060–1440, the game picture scaled into the middle with its aspect kept; the sheet scales by
  `min(H/900, W/1440)` with the strips pinned to the outer edges (1920×1080 gives 1.2). Starts maximized; F11 toggles.
- The panel draws inside a 32-bit Direct3D 9 game; nothing it draws may cost the game frames.
- Mouse first; every action also reachable by keyboard, and the cabinet actions by controller.
- Both editions, side by side on one PC; the screens must say which edition they belong to.
- Launcher: small, fixed-size window today (600 px wide), DPI-aware.
- Installer: about 680×470, resizable.
