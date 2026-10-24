# Roadmap

Forza-WCCF is one repository for both editions (2010–11 and 2017–18) and the installer, with the shipped code in
Rust. It started from `WCCF-Kit-1011` `main` (`b40b84a`, Kit 6) and took in `WCCF-Kit-1718` (`a7249b9`) and
`WCCF-Installer` (`073d719`); who did what before the merge is in [CREDITS.md](../CREDITS.md).
[ARCHITECTURE.md](ARCHITECTURE.md) is the reference for the code, [RELEASING.md](RELEASING.md) has the version and
release rules, and [SAFETY.md](SAFETY.md) the rules for running the original programs.

Status, 2026-10-24: version 0.3.2, alpha (GitHub pre-releases). Both ZIPs ship without Python; in-app UPDATE is live
from 0.3.2. PR #25 (stand-in parity, problem reports, cabinet mat, UX fixes) is open; its server part runs on the
project's server. Release 1 (1.0.0) waits on the live gates below.

## Done

- **Steps 1–7:** one workspace and `cargo xtask build | check | package` on Windows (MSVC, `clang-cl` for the 32-bit
  C); `Launcher.exe` (PLAY + SETUP, first-run junction); a clean run (no consoles, one Job object, quit within 30 s);
  hover delay setting.
- **Steps 9–12:** every visual in egui (`crates/forza-ui`, gallery and snapshot tests, open fonts for EN/IT/JA); the
  run, the stand-ins, setup, game text and catalogue in Rust; no Python in the ZIPs.
- **UPDATE:** this repository's releases, one ZIP per edition, SHA-256 checked, `self-replace` for Launcher.exe, a
  notice until UPDATE is opened.
- **Online:** one `forza-server` (desk, relay, inbox, 2017–18 game server, matcher, reflect); the lobby (2010–11 shops,
  2017–18 VS rooms, Rejoin, gamepad); direct VS over `vslink`.
- **2017–18 stand-in parity** (PR #25): whole saves round-trip, GAME_CTRL defaults, the shop and purchases, Best
  Eleven, VS echoes, and a guard test that every answer is handled or proven safe with its parser address.
- **Problem reports** (PR #25): the Launcher and the panel's crash screen send logs plus the player's words to the
  project's server (`REPORT` on the desk port).
- **Panel:** cabinet formation mat, links on the board (2010–11), pause P1, quit dialog, card lamp, crash screen,
  catalogue sized to the card pictures.
- **Text:** 2017–18 game text in Italian; English and Italian team, club and nationality names in both editions.

## Next

1. **Play test PR #25** (owner, live): card-in keeps levels and league; the shop opens and a bought item applies;
   Best Eleven; the mat and a bench rub; the quit dialog and the crash screen; Italian team names.
2. **Release 1 gates.** B7 live gates: a guarded join from the list that plays from the next ENTRY (one near a round's
   end); a kill-and-rejoin on the same seat; M3 under the Docker NAT harness at 0/50/100/150 ms with a 20-match soak;
   two PCs on two NATs play a whole match from a room code. Then the release gates: full check, guarded live runs from
   one test build (a `--kit` install of each edition, first-run link, pause, a fresh install from each ZIP, the soak in
   "Checks"), a play test, fixes, full check on the packaged commit.
3. **2010–11 reports:** the 2010–11 desk on the project's server gets a `--reports` folder (restarts that server).
4. **Stand-in leftovers (2017–18):** 234 team ranking (the control's "NO ENTRY" defaults), 250 today's play count,
   300 QR deletions that stick, 266 usage ratios; the shop's sale list from the game data (rotation, for-sale flags),
   SE purchases (286 +93 unverified); rankings, GT/UC points and prizes once their formula is traced.
5. **UX review follow-ups:** installer (the package is required, SmartScreen and antivirus help in the installer,
   "Start the Launcher now"), one-mouse controls (hold KEY PLAYER / KEEPER, keys on the buttons, drag a card off to
   remove it), a typed VS room code, a CPU countdown in the room, a "My cards" collection mode.
6. **Translations:** Italian for about 5,000 2017–18 lines (team talk, coach messages, supporters) and 441 2010–11
   supporter lines; countries, prefectures and leagues (the server reads them as data, check it first); the catalogue
   showing Italian club names.

## Planned

- **One program, `Forza-WCCF.exe`** (paused): D1–D4 done; D5 rename, D6 the installer inside, D7 the install step runs
  the kit's setup and makes shortcuts, D8 home screen, D9 coexistence gate (installing or updating one edition leaves
  the other byte-identical), D10 READMEs in EN/IT/JA.
- **Career mode:** built on branch `career` (owned cards, deals after matches, `data\forza.db` with backups); live run,
  2010–11 first.
- **Online between shops:** N1 shop server, N2 central hub (`forza-hub`), N3 open lobby, N4 matches between shops,
  N5 World Trophy; 2017–18 M4 hub, M5 ghosts, rankings, tournaments; lobby L4 (2017–18 shops via the hub), L5 (2010–11
  VS); server parity S2 (2010–11 shop server without Python), S4–S5 (2010–11 game server, VS and World Trophy between
  homes), S7 (hub and rankings). Self-hosting: [SELF-HOSTING.md](SELF-HOSTING.md).
- **Community requests:** a sharper game picture (render resolution); P2, one shared virtual clock for every process,
  and 2× menus on it (gate: saves at 2× and with pauses load back unchanged).
- **DXVK** (not started): an app-local `d3d9.dll` as a fallback renderer, graphics options and the Linux/Wine path;
  setting `Renderer: Direct3D 9 / Vulkan (DXVK)`, default Direct3D 9.
- **Links on the board** for 2017–18.
- **Data:** no database until a reader needs one; career mode's `data\forza.db` is the first. Design: one database
  per installation, `rusqlite_migration`, WAL, the catalogue with FTS5; language tables stay TSV in git.
- **Later, on hold** (owner, 2026-10-22): the in-game C to Rust (`wccfnet`, `fpr_emu`, then the hooks, `retour` for
  detours); `legacy/` goes when nothing in the build uses it.

## Online lobby view

ONLINE lists the shops (SHOP x/8) and VS rooms (VS x/2) a server knows; the address stays the fallback under "Direct
address". A lobby only picks which shop or VS room the next run joins: the game still matches in its own ENTRY window,
so a join says "you play from the next ENTRY, in about m:ss". L0 sample view, L1 2010–11 shops from the desk
(`LOBBY`/`RESERVE`/`TICKET`), L2 gamepad and Rejoin, L3 2017–18 VS rooms (`CREATE`/`READY`, pairing by room): built,
live gates open (B7). L4 2017–18 shops via the hub, L5 2010–11 VS: planned. Choices: random club ids only; CREATE is VS
only (a shop is a server with the control and engines); every room has a code; the project's server is the default;
region and language declared, no geo-IP; other versions greyed with "needs version X"; both VS runs start after READY
and a 5 s countdown.

## Working rules

- One commit per finished piece, each building. Contributor notes: [CONTRIBUTING.md](../CONTRIBUTING.md).
- Stability first: a crash, a hang or a run that leaves processes behind outranks any new feature. Every bug found in
  play gets its fix and the one check that would have caught it.
- Text: "the publisher" for the company and "original" for its files and text, in EN/IT/JA. Strings the code must
  match in original files are bytes in one named constant. `cargo xtask check` greps for the publisher's name
  (allow list: `xtask/scrub-allow.txt`).
- A widely used crate, maintained in the last year, MIT/Apache/BSD, before hand-written code longer than a few lines.
  `windows-sys` stays for thin Win32 calls; the root `Cargo.toml` lists the versions.

| Job | Crate |
|---|---|
| UI text in EN/IT/JA, English fallback | `rust-i18n` |
| Language tables (TSV) | `csv` |
| Original game text encodings | `encoding_rs` |
| Binary formats (club card, `.xaf`) | `binrw` |
| PE files: import renames, byte patches | `object` |
| Fingerprints, UPDATE hashes | `sha2` |
| Player data, server records | `rusqlite`, `rusqlite_migration` |
| Files written whole | `tempfile` |
| Card reader and I/O board pipes | `interprocess` |
| Aime reader on a COM port | `serialport` |
| Game controllers | `gilrs` |
| Registry | `winreg` |
| Crash dumps | `minidump-writer` |
| UPDATE download and self-replace | `ureq`, `zip`, `self-replace` |
| First-run junction | `junction` |
| Catalogue table, card pictures | `egui_extras` |
| Version info in the exes | `winresource` |
| Protocol transcripts in tests | `insta` |
| File errors with the path | `fs-err` |
| Build commands in xtask | `xshell` |

## Checks

Stability gate before a ZIP goes to a player: the full `cargo xtask check` on the commit that is packaged, then a soak
run through the guard ([SAFETY.md](SAFETY.md)): at least three matches back to back, RESTART NOW once, a card in and
out, online and offline, and a quit, with no crash snapshot, no hang, nothing left running and the routes unchanged.
The logs are read for warnings, not only for the pass lines. Crash snapshots and problem reports stay on, so a crash in
a player's hands is seen. Before a build goes to players as a release, someone plays it.
