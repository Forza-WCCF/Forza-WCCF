# Safety rules for running the original programs

The original game programs were written for arcade cabinets on the publisher's network. Run unguarded on a PC, they
call Windows' IP-helper functions and release the PC's network address: an unguarded start has dropped the host's
DHCP leases. Forza-WCCF starts them only behind a guard, and these rules apply to anyone who runs, tests or changes
that code.

## Start a game only through its edition's guard

Each guard covers the IP-helper calls listed here. It is not a general sandbox.

### 2017–18

- Setup (`Launcher.exe <game folder>`, `crates/forza/src/setup/e1718.rs`) makes guarded copies of
  `client_Release.exe` and `control_Release.exe` that import `wccfnet.dll` instead of `IPHLPAPI.DLL` for the four
  identified calls. It works from untouched originals, refuses originals that differ from its pinned hashes, and records
  the hashes of what it made in `<runtime>\setup-hashes.json`.
- The Launcher checks those hashes and the `wccfnet.dll` import before every start, and refuses to start otherwise.
- `cargo xtask check` proves that the built `wccfnet.dll` stubs the four calls (`test_wccfnet`).
- `wccfnet.dll` also loads the panel (`forza_panel.dll`) into seat 1 and the projector when the Launcher sets
  `FORZA_PANEL`; the control program never gets it. `wccfnet.log` records "panel: loaded".
- To confirm a run: the 32-bit module list shows `wccfnet.dll` in the control program and in both clients, and
  `forza_panel.dll` in seat 1 and the projector.

### 2010–11

- The hook (`mxhook`, built as `wccfk.dll`) blocks `IpReleaseAddress` and `IpRenewAddress` inside each game process.
  Setup puts `wccfk.dll` in place of each program's `WINMM.dll` import.
- The Launcher refuses to start a program that does not import `wccfk.dll`.
- To confirm a run: one `mxhook_<pid>.log` for each game process.
- Never start the 2010–11 programs from WSL interop (`/mnt/c/...exe` from a Linux shell, or `cmd.exe /c` from WSL).
  WSL gives every program it starts "Prefer System32 images", so the game loads Windows' `winmm.dll` instead of the
  hook and runs unguarded. Start the Launcher from Explorer (from WSL: `explorer.exe 'C:\path\Launcher.exe'`).
  Builds and tests run natively on Windows; that does not change this rule for a start from a WSL shell.

### Linux server (Wine)

- A server that runs the original programs under Wine uses the same `mxhook` `winmm.dll`; confirm one
  `mxhook_<pid>.log` for each game process.

## The host and the network

- Never change host interfaces, routes, DNS, the hosts file or firewall settings to make a game work.
- No connections to the publisher's network hosts. Forza-WCCF redirects them to local stand-ins (the Aime reader,
  AiMeDB, the game server). Record the routes and a ping before and after a test run; they must not change.
- One game instance per working folder. Stop only processes you have checked by their path.

## What stays out of the repository

- Original files and anything derived from them: game binaries, patched executables, disc images, IDA or Ghidra
  databases, decompiled code, extracted card pictures and catalogues.
- Logs, saves, club cards and screenshots of a game.
- Patches keep the original bytes and check them before writing. Work on a runtime copy, never on your decrypted
  original.

## Screenshots and captures

Capture only game windows (PrintWindow, as `cargo xtask cap` does), never the whole screen.

## Keep the editions apart

The 2010–11 and 2017–18 installations coexist. Each keeps its own game files, runtime, settings, logs, saves and
shortcuts; installing or updating one must not replace the other.
