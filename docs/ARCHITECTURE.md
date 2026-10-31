# How Forza-WCCF fits together

Everything shipped is Rust, in one Cargo workspace. Windows builds the kits with `cargo xtask`; `forza-server` and
`wccf-linux-server` also build on Linux. Build steps: [CONTRIBUTING.md](../CONTRIBUTING.md); plan:
[ROADMAP.md](ROADMAP.md); guard rules: [SAFETY.md](SAFETY.md).

## Layout

| Path | What |
|---|---|
| `crates/launcher` | `Launcher.exe`: PLAY, SETUP, UPDATE, the online lobby, SAVE LOGS and problem reports (a ZIP in Downloads); the run supervisor (`run`), the guard check before every start (`guard.rs`), the Job object, the debugger, the key driver |
| `crates/forza` | shared code: setup per edition (`setup/e1011.rs`, `e1718.rs`), the cabinet stand-ins (`standins`), online play and the 2017–18 game server (`online`), the cards and the card table, game text |
| `crates/forza-ui` | every screen in egui: theme and fonts, widgets, the Launcher, installer, panel and lobby screens; snapshot tests; UI text in `locales/ui.yml` |
| `crates/panel` | `forza_panel.dll`, 32-bit, inside the game: draws `forza-ui`'s panel over the game's Direct3D 9 picture; the projector window |
| `crates/installer` | the installer: extracts the player's own disc image, optionally sets up the 2010–11 package |
| `crates/server` | `forza-server`: every online service of both editions ([README](../crates/server/README.md)) |
| `crates/linux-host` | `wccf-linux-server`: runs a shop (the original control and four match engines) under Wine on Linux ([README](../crates/linux-host/README.md)) |
| `crates/vslink` | the 2017–18 direct VS link over iroh |
| `native/hook` | `forza-hook`, the hook in the game's programs; one source, four DLLs by feature |
| `native/wccfnet`, `native/netguard` | the network guards (2017–18 players; Linux servers under Wine) |
| `native/detmath` | dmath.dll: the 2017–18 match engine's math made the same on every CPU (online seats stay in step) |
| `native/icmp`, `native/programs`, `native/common` | the 2017–18 stand-in `ICMP.DLL`; the 32-bit programs beside the game (`FPR_Emu.exe`, `inject.exe`, `sddy_maps.exe`) and the hook and guard test programs; their shared code |
| `xtask` | `cargo xtask build`, `check`, `package`, `cap` (window capture) |
| `editions/` | what goes into each ZIP besides the programs: READMEs, language tables, overlay assets |
| `deploy/` | Linux server templates ([SELF-HOSTING.md](SELF-HOSTING.md)) |
| `legacy/` | the retired Python and its tests, kept until nothing in the build uses them |
| `docs/2017-18/`, `docs/research/` | protocol and game research notes |

## A run on a player's PC

1. The installer extracts the game from the player's disc image; the Launcher's SETUP prepares the game folder
   (`forza::setup`): guarded copies, byte patches that check every original byte, game text, the catalogue and card
   pictures. Each edition keeps its own folder, settings, logs and saves.
2. PLAY: the Launcher checks the guard (2017–18: `setup-hashes.json` and the `wccfnet.dll` import; 2010–11: the
   `wccfk.dll` import), then starts the stand-ins as threads of its own process and the original programs: control,
   four match engines, the projector and seat 1. It keeps every child by its handle in one Job object and stops them
   all when the run ends.
3. In the game's processes:
   - **2017–18:** the programs import `wccfnet.dll` instead of `IPHLPAPI.DLL`; it stubs the four calls that change
     the host's network and loads the panel into seat 1 and the projector. `bin\winmm.dll` (hook, `e1718`) stands in
     for the cabinet's hardware; `ICMP.DLL` answers the cabinet-network pings at once.
   - **2010–11:** each program's `WINMM.dll` import points at `wccfk.dll` (hook, `e1011`), which blocks the DHCP
     release and renew, stands in for the hardware and fixes the server's message handling; `inject.exe` loads the
     panel.
4. The stand-ins (`forza::standins`) replace the cabinet and the publisher's network: the card reader and I/O board
   over named pipes, the 2017–18 Aime reader and AiMeDB, the network services and scenes. The 2017–18 game server
   (`forza::online::gamesvr`) runs locally for offline play.
5. The panel (`crates/panel`) detours the game's Direct3D 9 `Present`, draws the cabinet buttons, the formation
   board, catalogue, keys and settings around the game picture, and writes the card table the game reads.

## Online

- `forza-server` serves the seat desk and lobby, the in-shop match relay and, for 2017–18,
  the game server, VS matcher and reflect. The project runs it at `lobby.wccf.it`; anyone can host their own.
- A shared shop is a host running the original control and engines: a player's PC, or Linux under Wine through
  `wccf-linux-server` with the hook's server features (`e1011-server`, `e1718-server`) and `netguard.dll`.
- 2017–18 direct VS: `vslink` pairs two PCs through the game server and carries the game's datagrams over one iroh
  QUIC connection, direct when possible, through the relay (`relay.wccf.it`) otherwise.

## xtask

`cargo xtask build` builds every program (the 32-bit ones for `i686-pc-windows-msvc`) and stages each kit in
`target/kit/<edition>/`. `check` runs fmt, clippy, the tests, the guard and hook test programs, the run tests and the
publisher-name grep (`check quick`: changed crates only; `check ci`: what CI runs). `package <edition>` writes the ZIP
and its `.sha256`.

## Rules for the code

- `cargo fmt`, `clippy -D warnings`; `unsafe` only around Win32 calls, with a `SAFETY:` line.
- Comments say what the code does and why; dates and bug stories go in commit messages.
- One fact, one place: a file format or rule is read by one module that everything else calls.
- Ownership by handle: a process the Launcher starts is waited on and stopped through its handle or its Job object,
  never found again by name or pid.
- No fixed sleeps for synchronisation: wait on the thing (a port, a pipe, a window, a handle) with a deadline.
- Strings the code must match in original files are bytes in one named constant.
- A widely used crate, maintained in the last year, MIT/Apache/BSD, before hand-written code longer than a few lines.
  `windows-sys` stays for thin Win32 calls; the root `Cargo.toml` lists the versions. The choices so far:

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
