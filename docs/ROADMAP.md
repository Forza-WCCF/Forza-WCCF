# Roadmap

Forza-WCCF is one repository for both editions (2010–11 and 2017–18) and the installer, with the shipped code moving
to Rust. It started from `WCCF-Kit-1011` `main` (`b40b84a`, Kit 6) and took in `WCCF-Kit-1718` (`a7249b9`) and
`WCCF-Installer` (`073d719`); who did what before the merge is in [CREDITS.md](../CREDITS.md).
[ARCHITECTURE.md](ARCHITECTURE.md) is the reference for the move to Rust, [RELEASING.md](RELEASING.md) has the version
rules, and [SAFETY.md](SAFETY.md) the rules for running the original programs.

Status, 2026-10-21: version 0.2.0, alpha. Steps 1–11 are done in code and both ZIPs ship without Python. The online
lobby (2010–11 shops, 2017–18 VS rooms) is built; its live checks are open. Release 1 (1.0.0) is both editions with no
Python inside and the online lobby, after the release gates below.

## The order at a glance

1. Panel loading back to `inject.exe`. Done.
2. UPDATE aimed at a placeholder. Done.
3. Import the 2017–18 kit and the installer, reorganize, say "the publisher" and "original". Done.
4. One Cargo workspace and `cargo xtask` build. Done.
5. `Launcher.exe` (PLAY + SETUP), first-run folder link. Done.
6. A clean run: one or two windows, no consoles, quit within 30 s. Done.
7. Formation hover delay, default 1 s. Done.
8. One SQLite database per installation. Decided: it comes with its first reader.
9. Every visual from scratch, designed in egui, inspired by the cabinets; fonts. Done; the C panel sources go after
   the live gate.
10. The run in Rust (debugger, stand-ins, server). Done.
11. Install steps in Rust. Done.
12. Python out of the ZIPs. Done for the ZIPs; `legacy/` goes when nothing in the build uses it.

Side track: DXVK (graphics options, Linux, fallback renderer). Planned features: career mode, online between shops
(hub, World Trophy), community requests (render scale, card pictures, card backs, pause, 2× menus), one program for
both editions (installer + launcher), forza-server at parity with the original servers.

## Release 1

Release 1 is the first player ZIPs of both editions plus the installer's `--kit` step, with no Python inside, and the
online lobby: 2010–11 shops from the project's server and 2017–18 VS rooms between homes.

**Track A - Python out of the ZIPs** (steps 11 and 12). Done: A1 2017–18 setup and A2 2010–11 setup on
`forza::setup`; A3 the 2017–18 catalogue on `forza::cards::catalogue`; A4 the 2010–11 club-card steps; A5 the Launcher,
its self-test and the installer call the Rust directly; A6 both ZIPs lose `python/` (2010–11 103 → 67 MB, 2017–18
95 → 60 MB). Gate per port: the files a job writes are byte-identical to the Python's on a copy of a game.

**Track B - the lobby (L0–L3) and what it needs.** Done: B1 the matcher and the direct VS link merged; B2 one server
binary (desk, relay, inbox, 2017–18 game server and matcher as subcommands); B3 2017–18 online (`GAME_SVR_ENTRY=1`, the
M3 messages 202, 223/224, 243–246, reflect, the AiMeDB 31-bit hash of the access code); B4 the lobby server words for
VS (`CREATE vs`, `READY`, `CLOSE`, pairing by room, never across rooms); B5 gamepad, Rejoin and the VS tab; B6 the
project's server runs the new desk and the 2017–18 game server. Open: B7, the live gates (a guarded join from the list
that plays from the next ENTRY, one near a round's end; a kill-and-rejoin on the same seat; M3 under the Docker NAT
harness at 0/50/100/150 ms with a 20-match soak; two PCs on two NATs play a whole match from a room code).

Risk: how long the 2017–18 control polls message 245 before its CPU fallback is not measured yet; it decides how long
a room can wait for a late opponent.

**Release gates**, once A and B are merged: the full `cargo xtask check`; one batch of guarded live runs from one test
build (a `--kit` install of each edition, the 2017–18 start from the first-run link, pause P1 per edition, a fresh
install from each ZIP and the stability soak in "Checks"); a play test; fixes for what they find; the full check on the
packaged commit.

## Step 1 — panel loading back to inject.exe — done

PR #22 of `WCCF-Kit-1011` moved panel loading into the hook (`wccfk.dll` loaded `wccfpanel.dll` 3 s after the game
showed a window) and deleted `inject.exe`. This fork went back to Kit 6's start order: `inject.exe` loads the panel
into seat 1 and the projector once their windows are up. A plain `git revert` conflicted in 9 files, so the revert was
done by hand in Kit 6's layout. #22's reason stays on the watch list: Defender's behaviour detection flags
`VirtualAllocEx` + `CreateRemoteThread` (`Behavior:Win32/DefenseEvasion.A!ml` on an older injector). Defender's static
scan of the ZIP and of `inject.exe` finds nothing.

## Step 2 — UPDATE keeps working, aimed at a placeholder — done

`release.rs` pointed UPDATE at the `WCCF-Kit-1011` releases, so this build would have offered that repository's
release in its place. UPDATE now reads one constant, `UPDATE_URL = "https://updates.invalid/wccf/"` (`.invalid` never
resolves, RFC 2606), and reports "No update found" (EN/IT/JA) until a real target exists. Tests:
`placeholder_finds_no_update`, `placeholder_asks_nobody`. Before UPDATE is switched on, see
[RELEASING.md](RELEASING.md).

## Step 3 — import and reorganize (history kept)

The two other repositories were imported with their history under a staging prefix, then moved into place one commit
at a time. Commit authors were unified in the process; [CREDITS.md](../CREDITS.md) records who did what.

Layout:

```
wccf-mono/
├─ Cargo.toml  Cargo.lock  rust-toolchain.toml  .cargo/config.toml
├─ crates/
│  ├─ forza/      lib, both editions: paths, settings, logs, i18n (EN/IT/JA), processes, Edition, card formats,
│  │              setup, game text and catalogue steps, stand-ins, online, UI
│  ├─ launcher/   Launcher.exe: PLAY and SETUP in one program; run supervisor, debug loop
│  ├─ panel/      forza_panel.dll, the in-game panel (32-bit)
│  ├─ server/     forza-server (Windows + Linux): desk, relay, log inbox, 2017–18 game server, matcher
│  ├─ vslink/     the direct VS link between two homes
│  └─ installer/  the installer (from WCCF-Installer)
├─ xtask/         cargo xtask build | check | package
├─ native/        32-bit C that runs inside the game: hooks, guards, FPR_Emu, inject
├─ assets/fonts/  the open fonts every program loads, with their licenses
├─ editions/      what goes into each edition's ZIP: art, language tables, READMEs, CHANGELOG
├─ legacy/        the Python, until nothing uses it
├─ tools/         headless IDA helpers, capture tools
└─ docs/          ARCHITECTURE.md, ROADMAP.md, research notes, 2017–18 notes and protocol
```

A crate exists only where a second consumer or a second OS needs it. Windows-only modules sit behind
`#[cfg(windows)]`, so `forza-server` builds on Linux. One `Launcher.exe` serves both editions and reads the edition
from its own folder. Each edition still ships as its own ZIP (`Forza-WCCF-2010-11-<version>.zip`,
`Forza-WCCF-2017-18-<version>.zip`) and installs into its own folder (`<dest>\2010-2011`, `<dest>\2017-2018`) with its
own data, logs, saves and shortcuts, so the two coexist.

Text rules from this step: prose, docs and UI text say "the publisher" for the company and "original" for its files
and text, in EN/IT/JA. Strings the code must match in the original files (the JVS board ID, the card reader's AES
key, the keychip strings) are kept as bytes in one named constant with a comment that says only what it matches.
`cargo xtask check` greps the tree for the publisher's name (allow list: `xtask/scrub-allow.txt`).

## Step 4 — one workspace, one build

Done:
- 4.1 One root workspace, `rust-toolchain.toml` (x86_64 + i686 MSVC targets), static CRT for every MSVC target (the
  installer no longer needs the VC++ runtime), one `Cargo.lock`, workspace lints.
- 4.2 `cargo xtask build | check | package <edition>`. The kit is staged in `target/kit/<edition>/` from one path
  table in `xtask/src/main.rs`; checks run there; `package` refuses uncommitted changes in mapped paths. `check` runs
  fmt, clippy, the tests, the C self-tests and the publisher-name grep.
- 4.3–4.5 Both editions' trees moved into `editions/`, `legacy/` and `native/`; xtask compiles the 2017–18 C as well
  (mxhook, logowin, fpr_emu, netguard, and the guard `wccfnet.dll`). `check` runs `test_wccfnet.exe` and
  `test_netguard.exe`: the four IP-helper calls return 0 and log, and the real IP helper is never reached.
- 4.6 WSL or Linux is the one build host: `cargo xwin` for the Rust, `clang-cl` + `lld-link` + `llvm-rc` for the C
  (`sudo apt install clang lld llvm`; xwin fetches the MSVC CRT and SDK once). The C exports match the old MSVC
  builds exactly; two folders give byte-identical output (`/Brepro`, `/DEBUG:NONE`). Kits stage under
  `FORZA_STAGE_ROOT` (default `target/`).
- 4.7 No PowerShell. Builds run in WSL or Linux, so nothing needs Windows-side scripts: the installer's smoke test,
  license check and packaging became `cargo xtask` steps, and `cargo xtask cap <root> <outdir>` replaces the
  PrintWindow capture script. `cargo xtask check` fails on any tracked `*.ps1` or a mention of the Windows script host
  in xtask.
- 4.8 Windows is the build host (owner, 2026-10-21). This supersedes 4.6 and 4.7. `cargo xtask` runs on Windows 10 or
  11 with the MSVC toolchain: `cargo build` for the Rust and Visual Studio's `clang-cl` + `lld-link` + `llvm-rc` for the 32-bit C, with the
  tool paths and `INCLUDE`/`LIB` taken from the installed Visual Studio. The shipped 0.2.0 DLLs were clang-cl builds,
  so only the build host changes. The tests and the run tests start natively, so
  `xtask/winrun.sh`, cargo-xwin, the xwin cache, `wslpath` and `WSLENV` are gone. `check` stays the full gate,
  `check quick` is unchanged, `check ci` is `check` without the GPU snapshot tests, and `check win` is deleted. CI is
  one native Windows job. The commit-message hook installs with `git config core.hooksPath xtask`. WSL stays for the
  reverse-engineering tools, codex and the Linux server deploy. Reasons: a Windows start inherits no "Prefer System32
  images" (the reason for `winrun.sh`), and one host path replaces two. A full `cargo xtask check` took 12 min on
  Windows 10 (NVMe, Defender exclusions) against 13-28 min in WSL.

Crates over hand-written code: a job longer than a few lines goes to a widely used crate, maintained in the last year,
with an MIT, Apache or BSD license. `windows-sys` stays for thin Win32 calls. The root `Cargo.toml` lists the versions.

| Job | Crate | Replaces |
|---|---|---|
| UI text in EN/IT/JA, English fallback | `rust-i18n` | hand-written `match` tables |
| Language tables (TSV) | `csv` | `english.py`'s TSV reading |
| Original game text encodings | `encoding_rs` | Python codecs |
| Club card, `.xaf` archive and other binary formats | `binrw` | `struct.unpack` in the card scripts |
| PE files: the WINMM import rename, byte patches | `object` | `helpers/hook.py`, `patches1718.py` |
| Fingerprints of the original files | `sha2` | `hashlib` |
| Player data, catalogue, server records | `rusqlite` (bundled SQLite), `rusqlite_migration` | loose state files (step 8) |
| Files that must stay files, written whole | `tempfile` (`persist`) | hand-written temp-and-rename |
| Named pipes of the card reader and I/O board | `interprocess` | ctypes `CreateNamedPipe` |
| Aime reader on a COM port (2017–18) | `serialport` | the Python serial code |
| Game controllers in KEYS | `gilrs` | the key driver's controller polling |
| Registry | `winreg` | Python `winreg` |
| Crash dumps of the game | `minidump-writer` | ctypes `MiniDumpWriteDump` |
| UPDATE: download, check, replace the running exe | `ureq`, `sha2`, `zip`, `self-replace` | `curl` subprocess and a hand-written swap |
| First-run junction | `junction` | hand-written `DeviceIoControl` |
| Catalogue table, card pictures | `egui_extras` | the panel's hand-drawn list |
| Version info in the exes | `winresource` | an `rc.exe` search in `build.rs` |
| Protocol transcripts and outputs in tests | `insta` | hand-compared golden files |
| Clearer file errors (path in every message) | `fs-err` | bare `std::fs` errors |
| Build commands in xtask | `xshell` | PowerShell scripts |

Later, when the hooks move to Rust: `retour` for detours, checked on `i686` first.

## Step 5 — Launcher.exe: PLAY and SETUP in one program — done

PLAY.exe and SETUP.exe became one `Launcher.exe` (crate `launcher`): play, setup, language, UPDATE and settings in one
window. Updating from Kit 6 deletes the old `PLAY.exe`/`SETUP.exe` through `files.txt`.

First run: a player often unzips into a path with spaces, accents or a user name in non-English letters, and the
32-bit game has trouble with such paths. On its first run `Launcher.exe` links its own folder to the installer's place,
`%SystemDrive%\Games\Forza-WCCF\2010-2011` or `...\2017-2018`, so both ways of installing end in the same place and the
editions stay apart.
- A directory junction, not a symbolic link: junctions need no administrator rights or Developer Mode. On a network
  or FAT drive the launcher skips the link, runs from where it is, and says why if the path is not plain ASCII.
- A real folder (an installer install) or a junction to another existing Forza-WCCF folder is left alone. A junction
  whose target is gone is pointed at this folder.
- After linking, the launcher restarts from the link path and starts every game process through it. Windows reports
  such a process under the link's target, so paths are compared in one resolved form.

## Step 6 — a clean run: one or two windows, no consoles, quit within 30 s — done

The player sees seat 1's game window, plus the projector's window when the projector is on. No console, no terminal
text, no server window: `Launcher.exe` is a Windows-subsystem program, every child starts with `CREATE_NO_WINDOW` and
logs to `data/logs/`, and the server programs start hidden.

Quit: closing seat 1's window (or the projector's, or Quit in the launcher) stops every process of the run within
30 s, normally under 5 s. Every process of the run sits in one Job object with `KILL_ON_JOB_CLOSE`
(`launcher::job::Job`), so nothing outlives `Launcher.exe`, even after a crash or a kill. The stop is ordered with
deadlines: `WM_CLOSE` to the game windows, a wait on the job up to the deadline, then `TerminateJobObject`. Never kill
by name or by a remembered pid. Children start suspended, join the job, then resume, so nothing runs outside it.

## Step 7 — formation hover delay: a setting, default 1 s — done

Resting the mouse on a card in the FORMATION BOARD shows it big after `hover_delay_ms` (default 1000, in
`data/panel.txt`, adjustable in SETTINGS, EN/IT/JA). It used to be a fixed 3 s.

## Step 8 — data: one SQLite database per installation

Decided: no database while every file a table would hold is still read live by a program that cannot open SQLite (the
game through the card reader's pipe, `FPR_Emu`, the panel's files). A database would only mirror those files and add
sync code with its own failure modes. The database comes with its first reader; career mode's `data\forza.db` is the
first. Files that stay files are written whole (`tempfile`).

Design for when it comes: one `data\forza.db` per installation, never shared (`meta.edition` refuses the other
edition's file); a separate `server.db` for the server; `rusqlite_migration`, one migration per step that brings a
writer; WAL with a busy timeout (DELETE mode on network drives); `synchronous=FULL` for club cards and the last 20
versions of each card kept for recovery; the catalogue with FTS5 (`unicode61` with `remove_diacritics 2`, so "kaka"
finds "KAKÀ", and `trigram` for parts of Japanese names). Not in the database: formats the game owns, the language
tables (TSV in git, so diffs stay readable), logs. Upgrading from Kit 6 imports the old files and moves them to
`data/kit6/` unchanged; nothing is deleted.

## DXVK — graphics options, Linux, and a fallback renderer

Not started. DXVK translates Direct3D 9 to Vulkan through an app-local `d3d9.dll` (zlib license, so it can ship with
its license file). DXVK 3.1.1 has rendered the 2017–18 projector under Wine, 3D match included, once the projector's
2x MSAA scene target was patched to 4x (lavapipe has no 2x). Uses:
1. A fallback when a PC gives no Direct3D 9 device.
2. Graphics options: `dxvk.conf` covers anisotropic filtering and a frame cap; the engine's own MSAA and the 960×600 3D
   pass still need game-side changes.
3. Linux and Wine: a Wine-based run uses DXVK rather than Wine's own `d3d9`, which crashed on the same MSAA case.

If it ships: a setting `Renderer: Direct3D 9 / Vulkan (DXVK)`, default Direct3D 9. Setup renames the game's `d3d9.dll`
import to a name of our own, as it does for WINMM, so no DLL in the game folder poses as a Windows file.

## Step 9 — every visual component from scratch, designed in egui

Done. The Win32 windows of PLAY/SETUP, the C panel's drawing code, `art/skin.tex` and the installer's screens were
replaced, not restyled. Every feature is kept. Every user-facing string ships in English, Italian and Japanese.

1. Feature inventory: [UI_INVENTORY.md](UI_INVENTORY.md).
2. Cabinet reference: [CABINET_LOOK.md](CABINET_LOOK.md), what makes the cabinet feel like itself, in words,
   measurements and colour values, with sources. Inspiration only: no logos, trademarks or original pictures.
3. Design in egui: `crates/forza/src/ui/theme.rs` sets every `egui::Style` value; bespoke widgets painted with
   `Painter` and meshes (the cabinet's buttons, the pitch and player cards, card frames and rarity marks, stat bars,
   key capture); the launcher draws its own title bar. `cargo xtask gallery` shows every screen and widget state with
   sample data in EN, IT and JA, and `egui_kittest` snapshots of them are regression tests.
4. Build: the launcher and the installer in eframe on `forza::ui::theme`; the panel is Rust too, a 32-bit `cdylib`
   (`forza_panel.dll`) drawing egui through a small Direct3D 9 painter of its own. 9.4a–9.4c done; both kits ship
   `forza_panel.dll`. After the 9.4c live gate the C panel sources, their `source\` shipping, `skin.tex` and
   `WCCF_SKIN` go.

### Links on the board - built for 2010–11

When a player is selected or the squad is built, the board shows that player's link group, and the pitch shows
coloured links between linked players. The links live in the original `prog_data/player_data/partnership.bin`, keyed
by the second katakana full name at `player_data.bin` record +0x177. Decoded in
[research/LINKS-2010-11.md](research/LINKS-2010-11.md); `forza::cards::links` reads the file from the player's own
game folder at run time, read-only. 2017–18 next.

### Fonts

Before step 9 each program used a different font and none covered all three languages (the C panel had an ASCII-only
atlas and folded player names to ASCII). Every program now loads the open fonts in `assets/fonts/` (licenses beside
them), which cover Italian accents and Japanese.

## One program: Forza-WCCF.exe installs and launches both editions - planned, paused

`Launcher.exe` and the installer become one `Forza-WCCF.exe`. The same build ships alone and inside each edition's
folder; every copy shows both editions; each edition runs only through the copy in its own folder, which keeps its
guard, settings, saves and UPDATE apart. Steps:
- **D1** one `forza::Edition` (done). **D2** one xtask kit table (done).
- **D3** the first-run junction for both editions, no hard-coded exe name (done).
- **D4** `launcher::home::rows` and the state table: one row per edition, where its folder is and what state it is in
  (done). The home screen decides the edition by folder name plus the game's `<id>_Table.dat`, never by layout alone.
- **D5** rename to `Forza-WCCF.exe`. **D6** the installer inside the binary. **D7** the install step runs the kit's
  own setup and makes shortcuts through `IShellLinkW`. **D8** the home screen. **D9** coexistence gate: installing or
  updating one edition leaves the other's tree byte-identical. **D10** READMEs in EN/IT/JA.

## Step 10 — the run in Rust (ARCHITECTURE phases 2–3, both editions) — done

The run engine (`play.py` and its helpers) moved to Rust, 2010–11 first; the 2017–18 run is an `Edition` value of the
same code, not a second port. Each move deleted the Python it replaced in the same commit. The Python was the
specification: a throwaway recorder drove each module with synthetic inputs into `crates/*/tests/rec/<x>.jsonl`, and
the Rust test replays the recording.

| Python | Went to |
|---|---|
| `debugger.py` / `_debug_launch.py` | `launcher`: debug loop, crash dumps, shared memory |
| `card_reader.py`, `io_board.py`, `keyboard.py`, `scenes.py`, `network.py` | `forza::standins` |
| 2017–18 `_aime_reader.py`, `_aimedb.py`, `_gamesvr.py` | `forza::standins` (Aime on COM2, AiMeDB, game server) |
| `panel.py`, `sound.py`, `console.py` | `launcher` (restart, send logs, start volumes) |
| `online/seat.py`, `send_logs.py` | `forza::online` |
| `seat_desk.py`, `relay.py`, `log_inbox.py`; 2017–18 `_seat_broker.py`, `_relay.py` | `forza-server` |

Guarded live runs of the Rust run passed for both editions: every game process loads the guard, closing seat 1 ends
the run in seconds with nothing left behind, and routes and addresses are unchanged. A dropped server link after the
player closes the window is logged as ENDED, not as a fault, so it sends no false report.

## Career mode - built on branch `career`, not played yet

A career owns cards: a starter pack, then deals after matches. `data\forza.db` (`careers`, `owned`, `deals`, one
transaction per deal) is copied whole into the career's `backup\` at each start (newest 20); a career's club card lives
in `data\careers\<name>\`, and a deleted career moves to `.deleted\`, never destroyed. In a career the panel keeps only
owned cards on the table and the catalogue lists owned cards. Live run to do, 2010–11 first.

## Online between shops: shop servers, a central hub, the World Trophy - planned

Goal: play between different 8-cabinet groups ("shops"), bring back the World Trophy, and run a server program for
Linux and Windows that is cheap to host, with shop servers that link to one central server and an open lobby players
join by themselves.

- **N0 - decode (research, no live run).** Done for both editions:
  [research/ONLINE-2017-18.md](research/ONLINE-2017-18.md), [research/ONLINE-2010-11.md](research/ONLINE-2010-11.md).
- **N1 - the shop server.** `forza-server` grows into the one program a shop runs.
- **N2 - the central hub (`forza-hub`).** Shop servers dial out to it.
- **N3 - join an open lobby** ("any open shop" next to the address).
- **N4 - matches between shops.** **N5 - World Trophy**: the hub runs the tournaments by the game's own rules.

### A server of our own for 2017–18, and direct VS

2017–18 has live matching between shops, as 2010–11 does. The control sends MATCHING_ENTRY 243 (280 bytes per seat:
card id, the seat's address and outside port, entry kind, club, coach, shop, last 7 opponents) and polls
MATCHING_RESULT 245/246 (248 bytes: the opponent's address and port, a shared seed, the shop, club and coach names);
the opponent becomes seat 90 + n, and no answer falls back to a CPU opponent. Login 202 hands out a reflect server for
NAT. So direct VS over the internet is the game's own design: our game server pairs the entries, and no new control is
needed.

- **M1 - a club survives restarts (one PC).** Passed live: new card, tutorial, club creation, save, restart, the club
  loads, a match, save, restart, the club loads with its result.
- **M1b - skip the tutorial** with a club made outside the game (optional at setup).
- **M2 - two PCs, one shop (LAN).** 2017–18 has no remote seats in a shared shop yet; the Launcher refuses that run.
- **M3 - direct VS over the internet.** Built: messages 202, 223/224, 243–246 in the game server, reflect, and the
  `vslink` crate between the two homes (iroh, with a relay as NAT fallback). Gate: Docker NAT harness, then two PCs.
- **M4 - the hub** (N2/N3). **M5 - ghosts, rankings, tournaments** (N4/N5).

### forza-server: feature parity with the original servers, leaner - planned

Parity: one Rust binary answers every message the controls send, and a message without a decoded layout gets a safe
answer. Every home already runs its own control and match engines, as 2017–18 direct VS does, so the server is game
server + reflect + relay + lobby; a shared shop with remote seats stays the one case that needs a control on the
server.

- **S0 - baseline and harness**: `crates/server/tests/load.rs` (5.3 MiB RSS, 0 % idle CPU, 1.5 % CPU for 4 pairs at
  60 packets/s) and the Docker NAT harness (`crates/server/tests/nat/`). Done.
- **S1 - one binary.** Done. **S2 - the 2010–11 shop server without Python.** **S3 - 2017–18 game server complete.**
- **S4 - 2010–11 game server.** **S5 - 2010–11 VS and World Trophy between homes.** **S6 - lobby words** (done for
  L1/L3). **S7 - hub and rankings.**

Self-hosting: [SELF-HOSTING.md](SELF-HOSTING.md).

### Online lobby view

When the player chooses ONLINE, the lobby lists the shops (SHOP x/8) and VS rooms (VS x/2) a server knows; the address
moves into a "Direct address" fold and stays the fallback. A lobby only picks which shop or VS room the next run joins:
the game still matches in its own ENTRY window, so each shop row shows its phase and the time to its next ENTRY, and a
join says "you play from the next ENTRY, in about m:ss".

- **L0 - the view on sample data.** Done.
- **L1 - 2010–11 shop list from the desk**: `LOBBY`/`RESERVE`/`TICKET`, the phase from the control's log, the
  `servers` setting. Built; live gate open.
- **L2 - gamepad and Rejoin.** Built; live gate open.
- **L3 - 2017–18 VS rooms**: `CREATE`/`READY`, pairing by room; Quick match is the open pool. Built; live gate open.
- **L4 - the hub behind the list**: 2017–18 SHOPS appear. **L5 - 2010–11 VS**, after N4.

Choices: the lobby sits in place under ONLINE (gamepad focus); random club ids only; no hosting a SHOP from the
Launcher (a shop is a server with the control and engines), CREATE is VS only; every room has a code; the `servers`
setting with the project's server as default; region and language declared, no geo-IP; other versions greyed with
"needs version X"; both VS runs start after READY and a 5 s countdown.

## Community requests

Requests from the players' community:
- **Sharper game picture (render resolution).** Planned.
- **Card search pictures** at a larger size than setup's 128×128. Planned.
- **A bigger pitch and a card preview with the card's back.** Covered by step 9.
- **Pause.** Research: [research/GAME-CLOCK.md](research/GAME-CLOCK.md). **P1** client-only pause, capped (offline,
  one seat, 20 s cap, 10 s gap): done in the Rust panel. **P2** one shared virtual clock for every process: planned.
- **Fast-forward 2× in menus.** 2× means more frames, not a faster clock: it is P2's clock at rate 2. Gate: saves at
  2× and with pauses load back unchanged.

## 2017–18 game text in Italian - built

The 2017–18 game text applier puts `editions/2017-18/italian/screen_text.tsv` first and the English behind each key.
The choice is the one both editions share: the Launcher's Game text (English / Italiano / original Japanese) and the
panel's SETTINGS.

## Step 11 — install steps in Rust (ARCHITECTURE phase 4) — done

Setup, the game text, the catalogue, card pictures and the club-card steps run in `forza` (`forza::setup`,
`forza::text`, `forza::cards`), and the installer calls them directly. Each job is ported once for both editions; the
edition is a parameter or a table, never a second module. Gate: byte-identical outputs on a copy of a game (catalogue,
patched files; card pictures pixel-identical).

## Step 12 — drop Python from the ZIPs

Both ZIPs ship without `python/` (`cargo xtask check` fails if one has it). `legacy/` goes once nothing in the build
uses it; Python then stays only in `tools/`.

## Later

The rest of the in-game C to Rust (`i686-pc-windows-msvc`): `netguard`/`wccfnet` and `fpr_emu` first, then the hooks
(`wccfk.c`, `mxhook.c`). Both hooks block `IpReleaseAddress`/`IpRenewAddress`; in Rust that guard becomes one module
both use, with `test_wccfnet`'s check extended to it. 2010–11's setup renames the game's WINMM import to the kit's own
DLL, which forwards only the three timer calls the game imports; 2017–18's `mxhook` still poses as `winmm.dll` and
forwards every export. 2017–18 moves to the 2010–11 scheme, as its `wccfnet.dll` IPHLPAPI rename already does.

## Simple improvements, taken along the way

- `tracing` + `tracing-appender` replace hand-written log rotation.
- Dated change-history comments leave the code as it is ported; history lives in git.
- Waits on the thing (handle, pipe, port, window) with a deadline instead of fixed sleeps.

## Working rules

- One commit per finished piece, each building. Contributor notes: [CONTRIBUTING.md](../CONTRIBUTING.md).
- Each move to Rust deletes the code it replaces in the same commit.
- A common, maintained crate before hand-written code (table in step 4).
- Stability first: a crash, a hang or a run that leaves processes behind outranks any new feature. Every bug found in
  play gets its fix and the one check that would have caught it.

## Checks

Stability gate before a ZIP goes to a player: the full `cargo xtask check` on the commit that is packaged, then a soak
run through the guard ([SAFETY.md](SAFETY.md)): at least three matches back to back, RESTART NOW once, a card in and
out, online and offline, and a quit, with no crash snapshot, no hang, nothing left running and the routes unchanged.
The logs are read for warnings, not only for the pass lines. Crash snapshots and the automatic reports stay on, so a
crash in a player's hands is seen. Before a build goes to players as a release, someone plays it.
