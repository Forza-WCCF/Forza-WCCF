# Roadmap

Forza-WCCF is one repository for both editions (2010–11 and 2017–18) and the installer, with the shipped code in
Rust. It started from `WCCF-Kit-1011` `main` (`b40b84a`, Kit 6) and took in `WCCF-Kit-1718` (`a7249b9`) and
`WCCF-Installer` (`073d719`); who did what before the merge is in [CREDITS.md](../CREDITS.md).
[ARCHITECTURE.md](ARCHITECTURE.md) is the reference for the code, [RELEASING.md](RELEASING.md) has the version and
release rules and the stability gate, [SAFETY.md](SAFETY.md) the rules for running the original programs, and
[CONTRIBUTING.md](../CONTRIBUTING.md) the rules for a change.

Status, 2026-10-27: version 0.4.1, alpha (GitHub pre-releases, 2017–18 first). Both editions' ZIPs build without Python
and update in-app. Next up: the 2017–18 shared arcade (M2), so players can play friendlies in one arcade over a LAN or the
internet. Release 1 (1.0.0) waits on the live gates below.

## Done

- **Steps 1–7:** one workspace and `cargo xtask build | check | package` on Windows (MSVC, `rust-lld`);
  `Launcher.exe` (PLAY + SETUP, first-run junction); a clean run (no consoles, one Job object, quit within 30 s);
  hover delay setting.
- **Steps 9–12:** every visual in egui (`crates/forza-ui`, gallery and snapshot tests, open fonts for EN/IT/JA); the
  run, the stand-ins, setup, game text and catalogue in Rust; no Python in the ZIPs.
- **All Rust** (#28): the hooks, the guards (`wccfnet`, `netguard`), the 32-bit programs beside the game and the
  2010–11 server hook; the C sources, `clang-cl` and MinGW are gone.
- **Releases:** 0.3.3 (2026-10-24), 0.4.0 and 0.4.1 (2026-10-26).
- **UPDATE:** this repository's releases, one ZIP per edition, SHA-256 checked, `self-replace` for Launcher.exe, a
  notice until UPDATE is opened; the new Launcher.exe starts after an update (#47).
- **Online:** one `forza-server` (desk, relay, 2017–18 game server, matcher, reflect); the lobby (2010–11 arcades,
  2017–18 VS rooms, Rejoin, gamepad); direct VS over `vslink`.
- **2017–18 stand-in parity** (#25): whole saves round-trip, GAME_CTRL defaults, the item shop and purchases, Best
  Eleven, VS echoes, and a guard test that every answer is handled or proven safe with its parser address.
- **Problem reports:** SAVE LOGS (the Launcher, the panel's SETTINGS and crash screen) writes the logs ZIP, with
  the player's words, to the Downloads folder for the player to send; servers collect no logs (2026-10-27).
- **Panel:** cabinet formation mat, links on the board (2010–11), pause P1, quit dialog, card lamp, crash screen,
  catalogue sized to the card pictures; the 2017–18 album card art for the card pictures (#32); hotkey labels while
  Alt is held (#39); a pop-out formation board with pitch artwork and the hover pane in the board column (#48).
- **UX review** (#31, #33): the installer picks the newest package, has a Start Launcher button and a SmartScreen
  hint; keycaps on the deck buttons; right-click latches KEY PLAYER and KEEPER; drag a card off the board with UNDO;
  KEEP WAITING or PLAY CPU NOW before a CPU match; RETRY in the offline lobby.
- **Squad import, steps 1–3:** a lineup text parser and exporter for the card table (#52); IMPORT LINEUP on the
  catalogue page, with UNDO (#54).
- **2017–18 arcade on Linux:** the server hook `e1718-server` (#56); `wccf-linux-server --edition 2017-18` and
  `sddy_maps.exe` (#57). Built and unit-tested; no Wine run yet. Setup: [SELF-HOSTING.md](SELF-HOSTING.md).
- **M2 step 1, 2017–18 LAN arcade** (#58): the server and remote-seat modes are on by default;
  `"shop_1718": false` in `data\settings.json` turns them off. Not yet played live.
- **Build speed:** a `fast` profile and sccache by default for local builds, a stale kit rebuilt before
  `check quick` (#49); one workspace build for `check` (#53); a faster CI (#36, #51, #55).
- **Text:** 2017–18 game text in Italian; English and Italian team, club and nationality names in both editions;
  plainer player text in EN/IT/JA (#29).

## Next

1. **Play test 0.4.x** (live): card-in keeps levels and league; the item shop opens and a bought item applies;
   Best Eleven; the mat and a bench rub; the quit dialog and the crash screen; Italian team names; the pop-out board
   and IMPORT LINEUP.
2. **2017–18 shared arcade, online friendlies (M2):** play the LAN arcade (#58) live; then the iroh arcade tunnel and lobby
   arcade rooms (#59) and HOST / JOIN by code in the Launcher (#60), both merged; the first Wine run of the Linux arcade,
   both editions (#57 changed the 2010–11 path too).
3. **LAN play with auto-discovery:** seats on the same network find the arcade without a typed address.
4. **iroh relay backup (#64):** relay.wccf.it, public relays only when it is unreachable (done on merge).
   Running a relay: [SELF-HOSTING.md](SELF-HOSTING.md#8-the-iroh-relay-2017-18-vs).
5. **Squad import, step 4:** share links for a lineup; COPY LINEUP in the panel (the exporter is in #52).
6. **Release 1 gates.** B7 live gates: a guarded join from the list that plays from the next ENTRY (one near a round's
   end); a kill-and-rejoin on the same seat; M3 under the Docker NAT harness at 0/50/100/150 ms with a 20-match soak;
   two PCs on two NATs play a whole match from a room code. Then the release gates: full check, guarded live runs from
   one test build (a `--kit` install of each edition, first-run link, pause, a fresh install from each ZIP, the soak in
   [RELEASING.md](RELEASING.md#stability-gate)), a play test, fixes, full check on the packaged commit.
7. **Logs collected nowhere:** done (2026-10-27); ovh1 gets the server without the desk's `REPORT` store.
8. **Stand-in leftovers (2017–18):** 234 team ranking (the control's "NO ENTRY" defaults), 250 today's play count,
   300 QR deletions that stick, 266 usage ratios; the item shop's sale list from the game data (rotation, for-sale flags),
   SE purchases (286 +93 unverified); rankings, GT/UC points and prizes once their formula is traced.
9. **UX review leftovers:** antivirus help in the installer, a typed VS room code, a "My cards" collection mode.
10. **Translations:** Italian for about 5,000 2017–18 lines (team talk, coach messages, supporters) and 441 2010–11
   supporter lines; countries, prefectures and leagues (the server reads them as data, check it first); the catalogue
   showing Italian club names.

## Planned

- **One program, `Forza-WCCF.exe`** (paused): D1–D4 done; D5 rename, D6 the installer inside, D7 the install step runs
  the kit's setup and makes shortcuts, D8 home screen, D9 coexistence gate (installing or updating one edition leaves
  the other byte-identical), D10 READMEs in EN/IT/JA.
- **Career mode:** built on branch `career` (owned cards, deals after matches, `data\forza.db` with backups); live run,
  2010–11 first.
- **Online between arcades:** N1 arcade server, N2 central hub (`forza-hub`), N3 open lobby, N4 matches between arcades,
  N5 World Trophy; 2017–18 M4 hub, M5 ghosts, rankings, tournaments; lobby L4 (2017–18 arcades via the hub), L5 (2010–11
  VS); server parity S2 (2010–11 arcade server without Python), S4–S5 (2010–11 game server, VS and World Trophy between
  homes), S7 (hub and rankings). Self-hosting: [SELF-HOSTING.md](SELF-HOSTING.md).
- **Community requests:** a sharper game picture (render resolution); P2, one shared virtual clock for every process,
  and 2× menus on it (gate: saves at 2× and with pauses load back unchanged).
- **DXVK** (not started): an app-local `d3d9.dll` as a fallback renderer, graphics options and the Linux/Wine path;
  setting `Renderer: Direct3D 9 / Vulkan (DXVK)`, default Direct3D 9.
- **Links on the board** for 2017–18.
- **Data:** no database until a reader needs one; career mode's `data\forza.db` is the first. Design: one database
  per installation, `rusqlite_migration`, WAL, the catalogue with FTS5; language tables stay TSV in git.
- **Cleanup:** `legacy/` (the retired Python and its tests) goes when nothing in the build uses it.

## Online lobby view

ONLINE lists the arcades (ARCADE x/8) and VS rooms (VS x/2) a server knows; the address stays the fallback under "Direct
address". A lobby only picks which arcade or VS room the next run joins: the game still matches in its own ENTRY window,
so a join says "you play from the next ENTRY, in about m:ss". L0 sample view, L1 2010–11 arcades from the desk
(`LOBBY`/`RESERVE`/`TICKET`), L2 gamepad and Rejoin, L3 2017–18 VS rooms (`CREATE`/`READY`, pairing by room): built,
live gates open (B7). L4 2017–18 arcades via the hub, L5 2010–11 VS: planned. Choices: random club ids only; CREATE is VS
only (an arcade is a server with the control and engines); every room has a code; the project's server is the default;
region and language declared, no geo-IP; other versions greyed with "needs version X"; both VS runs start after READY
and a 5 s countdown.
