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
  It runs setup again once first on a runtime an older setup made (a lower `setup-version` in the record, or a kit
  file there that is not this version's: an update or files unzipped over it) or one the check refuses (no or an
  unreadable record, a guarded file an update or an antivirus put back), then checks again; a failed run of it, or a
  check that still fails, starts nothing.
- `cargo xtask check` proves that the built `wccfnet.dll` stubs the four calls (`test_wccfnet`).
- `wccfnet.dll` also loads the panel (`forza_panel.dll`) into seat 1 and the projector when the Launcher sets
  `FORZA_PANEL`; the control program never gets it. `wccfnet.log` records "panel: loaded".
- `wccfnet.dll` refuses the publisher's addresses the programs hold as numbers (`BLOCKED_ADDRS` in
  `native/common/src/hosts.rs`: the control's built-in game server) as a destination of `connect` (error
  `WSAECONNREFUSED`) and `sendto` (`WSAEHOSTUNREACH`), IPv4 and IPv4-mapped IPv6. Every other address passes:
  loopback, the LAN, the local stand-ins and the project's own servers. This check is 2017–18 only. `wccfnet.log` has,
  once per program, `pid=<pid> publisher addresses hooked in: connect=1 sendto=1` (1 = hooked, 0 = not imported or
  not patched) and, the first time a program tries a blocked address,
  `pid=<pid> blocked publisher address <a.b.c.d>:<port> (connect|sendto), later attempts refused unlogged`; later
  attempts at the same address from the same program are refused without a line.
- `wccfnet.dll` also keeps the 2017–18 client's window hidden through the boot screens (seat 1 and the projector, the
  client studied only): it points the program's `ShowWindow` import at itself, holds the one first show of the game
  window and puts the import back at once; every other call passes through, and a check that fails leaves the import
  alone. The panel shows the window at the start warning picture, and 60 s after the hold the window shows anyway.
  `wccfnet.log` has `boot hold: ShowWindow import patched` and `boot hold: window ... kept hidden`, or a `refused`
  line; `boot hold: no reveal from the panel in 60 s` means the fallback showed it. No network call is involved.
- To confirm a run: the 32-bit module list shows `wccfnet.dll` in the control program and in both clients, and
  `forza_panel.dll` in seat 1 and the projector; `wccfnet.log` has `publisher addresses hooked in: connect=1 sendto=1`
  for each program and no `blocked publisher address` line in a normal run (a line means a program tried to reach the
  publisher: check `GAME_SVR_IP` in `ctrl_user_option.conf`).

### 2010–11

- The hook (`mxhook`, built as `wccfk.dll`) blocks `IpReleaseAddress` and `IpRenewAddress` inside each game process.
  Setup puts `wccfk.dll` in place of each program's `WINMM.dll` import.
- The Launcher refuses to start a program that does not import `wccfk.dll`.
- To confirm a run: one `mxhook_<pid>.log` for each game process.
- Never start the 2010–11 programs from WSL interop (`/mnt/c/...exe` from a Linux shell, or `cmd.exe /c` from WSL).
  WSL gives every program it starts "Prefer System32 images", so the game loads Windows' `winmm.dll` instead of the
  hook and runs unguarded. From WSL, start the Launcher through Explorer (`explorer.exe 'C:\path\Launcher.exe'`).
  A start from Explorer or a native Windows shell (PowerShell, cmd, Git Bash) is guarded: a check on 2026-10-22 from
  PowerShell found "Prefer System32 images" off and the hook in both clients.

### Linux server (Wine)

- A server that runs the original programs under Wine uses the same `mxhook` `winmm.dll`; confirm one
  `mxhook_<pid>.log` for each game process.

## The host and the network

- Never change host interfaces, routes, DNS, the hosts file or firewall settings to make a game work.
- No connections to the publisher's network hosts. Forza-WCCF redirects them to local stand-ins (the Aime reader,
  AiMeDB, the game server). Record the routes and a ping before and after a test run; they must not change.
- One game instance per working folder. Stop only processes you have checked by their path.

## What stays out of the repository

Original files, anything derived from them, logs, saves and game screenshots never go into the repository; the full
list is in [CONTRIBUTING.md](../CONTRIBUTING.md#what-never-goes-into-the-repository). Work on a runtime copy, never on
your decrypted original.

## Screenshots and captures

Capture only game windows (PrintWindow, as `cargo xtask cap` does), never the whole screen.

## Keep the editions apart

The 2010–11 and 2017–18 installations coexist. Each keeps its own game files, runtime, settings, logs, saves and
shortcuts; installing or updating one must not replace the other.
