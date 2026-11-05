# linux-host

The 2010-11 and 2017-18 arcades on Linux. The original control and its four match engines are 32-bit Windows programs; they run
under Wine on a virtual X display (Xvfb). This crate holds the Linux supervisor that starts, watches and stops them,
and the small Windows helpers it runs inside Wine. The seat desk and match relay are `forza-server`
(`crates/server`); the projector relay is `crates/server/python/projector_relay.py`. Setup from scratch:
`docs/SELF-HOSTING.md`.

| Program | Runs on | What |
|---|---|---|
| `wccf-linux-server` | Linux | `start`, `stop`, `status`, `serve` (the supervisor itself, which `start` detaches) |
| `sddy_maps.exe` | Wine, 32-bit | 2017-18 only (`native/programs`): makes and holds the `SDDY_*` shared memory the Launcher normally makes |
| `win32-helper.exe` | Wine, 32-bit | serves the motion tables and scene files control expects from the cabinet's shared memory, starts control under its debugger and logs a crash |
| `check-crt.exe` | Wine, 32-bit | checks Wine's `msvcr90.dll` rejects the 14-character `Match[Control]` label the way Windows does (the helper shortens it) |
| `check-network.exe` | Windows | from a player's PC: control's TCP 20002, two seat leases on 20030 and the UDP 20040 relay in both directions |

## Build

    cargo build --release -p linux-host                                   # on Linux: target/release/wccf-linux-server
    cargo build --release -p linux-host --target i686-pc-windows-msvc --bin win32-helper --bin check-crt  # on Windows
    cargo build --release -p linux-host --target x86_64-pc-windows-msvc --bin check-network               # on Windows

On the wrong system each program is an empty stand-in that says where it runs, so `cargo xtask check` and a Linux
build both pass.

## The server tree

The supervisor reads everything from one folder, `ROOT`: `WCCF_ROOT`, else three folders above the binary
(`ROOT/target/release/wccf-linux-server`).

| Path | What |
|---|---|
| `runtime/` | your own 2010-11 game folder with the guarded programs (`docs/SELF-HOSTING.md`), the server hook as `winmm.dll`, Wine's `winmm.dll` renamed `winmm_orig.dll`, `wccfnet.dll`, `netguard.dll`, `test_wccfnet.exe` |
| `runtime/local/ctrl_user_option.conf` | needs `WAIT_PROJECTOR=0` |
| `run/win32-helper.exe` | the helper above |
| `evidence/runtime.sha256` | `sha256sum` lines for the files that must not change; `start` refuses when one differs |
| `wineprefix/` | a 32-bit Wine prefix (`WINEARCH=win32 wineboot -i`) |
| `tools/usr` | a link to `/usr` when Xvfb comes from the system packages; `tools/bin/wine*` only for a private Wine |
| `logs/<unix time>/` | one folder per start: `game.log` (control), `xvfb.log`, `wineboot.log`, `guard-test.log`, `mxhook_<pid>.log` |
| `run/` | `server.pid`, `ready` (written when control and four engines run), `degraded`, `stop` |

`start` checks the hashes, records `ip -4 route` before and after in `evidence/`, starts Xvfb on display `:97`
(no TCP), `wineboot`, runs `test_wccfnet.exe` (the guard's self-test) and then the helper, and waits up to 270 s for
`run/ready`. It refuses to clean up a Wine process that is not this prefix's.

| Variable | Effect |
|---|---|
| `WCCF_ROOT` | the server tree |
| `--edition 2010-11\|2017-18` | after `start` or `serve`; 2010-11 when absent |
| `WCCF_EXTERNAL_DESK=1` | leave TCP 20030 and UDP 20040 to `forza-server desk` and `relay` (the supported setup) |

Without `WCCF_EXTERNAL_DESK=1` the supervisor runs its own minimal seat broker and relay; it predates
`forza-server` and knows no lobby or projector relay.

## The 2017-18 arcade

`start --edition 2017-18` uses a tree of its own (`WCCF_ROOT`) with its own `wineprefix/` and Xvfb display `:96`; the
arcade and a 2010-11 arcade can run on one host. Differences from 2010-11:

- `run/sddy_maps.exe` replaces `win32-helper.exe`; control runs as `wine control_Release.exe` in `runtime/`, and Wine's
  own crash backtrace lands in `game.log` (the supervisor turns the crash dialog off in the prefix).
- `runtime/local/ctrl_user_option.conf` must have `GAME_SVR_IP=localhost` once (not `127.0.0.1`) and a `CONTROL_PORT`
  other than 20002; ready needs that port listening in this prefix.
- The seat desk and relay are always external (`forza-server`).
- Ready also needs a `mxhook_<pid>.log` saying `edition: 2017-18 server` per live control and engine, none `REFUSED` or
  `REFUSING`; rechecked every 10 s. 2010-11 has no such gate.
- `server1718_test.exe` (the hook's Wine self-test) is run by hand, not at start.

## Wine paths

The cleanup check accepts Debian's `/usr/lib/wine/wine`, `wineserver32` and `wineserver64` with their server folders
under `/run/user/`, `/tmp/.wine-<uid>/` or `/tmp/wine-XXXXXX/`. Another distribution may put Wine elsewhere: then
`status` says "refusing cleanup of unowned pid"; add its paths in `verify_prefix` (`src/supervisor.rs`).

## Tests

`cargo test -p linux-host` (Linux): edition selection and config (2010-11 values unchanged), the hook-log check, seat allocation, collisions and release, malformed relay packets, forwarding and
expiry, and the `ready`/`degraded` files.
