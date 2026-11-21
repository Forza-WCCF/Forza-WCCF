# Roadmap

Forza-WCCF is one repository for both editions (2010–11 and 2017–18) and the installer, with the shipped code in
Rust. It started from `WCCF-Kit-1011` `main` (`b40b84a`, Kit 6) and took in `WCCF-Kit-1718` (`a7249b9`) and
`WCCF-Installer` (`073d719`); who did what before the merge is in [CREDITS.md](../CREDITS.md).
[ARCHITECTURE.md](ARCHITECTURE.md) is the reference for the code, [RELEASING.md](RELEASING.md) has the version and
release rules and the stability gate, [SAFETY.md](SAFETY.md) the rules for running the original programs, and
[CONTRIBUTING.md](../CONTRIBUTING.md) the rules for a change.

Status, 2026-11-15: version 0.7.3, alpha (GitHub pre-releases, 2017–18 first). Open work is in the GitHub issues;
Release 1 (1.0.0) waits on the live gates in #314.

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
- **Squad import:** a lineup text parser and exporter for the card table (#52); IMPORT LINEUP on the
  catalogue page, with UNDO (#54); step 4: share links (#102) and COPY LINEUP.
- **2017–18 arcade on Linux:** the server hook `e1718-server` (#56); `wccf-linux-server --edition 2017-18` and
  `sddy_maps.exe` (#57); the hourly cup arcade runs under Wine on the project's server (#221). Setup:
  [SELF-HOSTING.md](SELF-HOSTING.md).
- **M2 step 1, 2017–18 LAN arcade** (#58): the server and remote-seat modes are on by default;
  `"shop_1718": false` in `data\settings.json` turns them off; arcade rooms over the iroh tunnel (#59), HOST / JOIN by
  code (#60), LAN auto-discovery, the relay.wccf.it backup (#64), a typed VS room code.
- **Build speed:** a `fast` profile and sccache by default for local builds, a stale kit rebuilt before
  `check quick` (#49); one workspace build for `check` (#53); a faster CI (#36, #51, #55).
- **Text:** 2017–18 game text in Italian; English and Italian team, club and nationality names in both editions;
  plainer player text in EN/IT/JA (#29).

## Next

1. **Release 1 gates** (#314): the B7 live gates, the release gates and a play test.
2. **Stand-in leftovers (2017–18)** (#316): rankings and points, SE purchases.
3. **"My cards" collection mode:** Create-a-Team (PRs #233–#236).
4. **Translations** (#317): countries, prefectures and leagues; the catalogue showing Italian club names.

## Planned

- **One program, `Forza-WCCF.exe`** (paused, #318): D1–D4 done; D5–D10 open.
- **Career mode** (#319): built on branch `career`, with its data design; live run.
- **Online between arcades** (#320): N1–N5, M4, M5, rankings, lobby L4, server parity S7.
- **Community requests:** a sharper game picture (#321); P2, one shared virtual clock for every process (#322).
- **DXVK** (#323): an app-local `d3d9.dll` as a fallback renderer.
- **Links on the board** for 2017–18 (#324).
- **Cleanup:** `legacy/` goes when nothing in the build uses it (#325).

## Online lobby view

ONLINE lists the arcades (ARCADE x/8) and VS rooms (VS x/2) a server knows; the address stays the fallback under "Direct
address". A lobby only picks which arcade or VS room the next run joins: the game still matches in its own ENTRY window,
so a join says "you play from the next ENTRY, in about m:ss". L0 sample view, L1 2010–11 arcades from the desk
(`LOBBY`/`RESERVE`/`TICKET`), L2 gamepad and Rejoin, L3 2017–18 VS rooms (`CREATE`/`READY`, pairing by room): built,
live gates open (B7, #314). L4 2017–18 arcades via the hub: planned (#320). Choices: random club ids only; CREATE is VS
only (an arcade is a server with the control and engines); every room has a code; the project's server is the default;
region and language declared, no geo-IP; other versions greyed with "needs version X"; both VS runs start after READY
and a 5 s countdown.
