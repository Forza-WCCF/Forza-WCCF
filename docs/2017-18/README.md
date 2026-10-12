# WCCF-Kit-1718

Runs **World Club Champion Football 2017–18 (SDDY, Ver. 2.00.00)** on one Windows PC: the publisher's server
(control + 4 match engines), the projector and a player cabinet (seat 1), built on
[WCCF-Kit-1011](https://github.com/Forza-WCCF/WCCF-Kit-1011) (Kit 5). Holds none of the original files: setup
works from your own decrypted copy.

**Status:** boots to attract, START works, OFFLINE guest play (CPU match) is playable with the 2010–11 side
panels around a 1440×900 picture. Online with an emulated Aime card (local reader + AiMeDB stand-ins): the client
reads the card, gets an Aime ID and reaches the first-play TUTORIAL / NORMAL choice. Club persistence (game server)
is in progress. See [docs/PROGRESS.md](docs/PROGRESS.md) and [docs/TASKS.md](docs/TASKS.md).

## Layout

| Path | What |
|---|---|
| `kit/` | Kit 5 + 2017–18 changes (`PLAY1718.bat`, `scripts/play.py`, `_debug_launch.py`, `_ringedge_services.py`, new `_gamesvr.py`) |
| `tools/setup1718.py`, `patches1718.py` | build the runtime: guarded imports, seat 1 exe patches, controller patch (every original byte checked) |
| `tools/restart.sh`, `press.sh` (`cargo xtask cap`) | WSL helpers: restart the stack, tap cabinet buttons, capture game windows |
| `tools/netguard/` | `wccfnet.dll` / `netguard.dll` (+ sources): stub the IP-helper calls that drop the host's DHCP lease |
| `tools/ida/` | headless IDA helpers (`ida_query.py`, `dump.py`) |
| `docs/` | progress, tasks, research notes, game-server message table, agent workstream briefs |
| `research/catalogue/` | work-in-progress 17–18 card catalogue maker |

## Quick start (from WSL)

1. Copy the decrypted game folder to an NTFS runtime folder as `<runtime>/game` (default `<runtime>`).
2. `WCCF1718_GAME=<decrypted game> legacy/2017-18/tools/restart.sh first run1` (setup + start), after `cargo xtask
   build` on Windows in the same checkout. The decrypted game is the untouched original whose `client_Release.exe`
   still imports `IPHLPAPI.DLL` (here: `ringedge/1718-decryption-20261007-1944/game`); the `D:\wccf-1718-*` copies are
   already guarded and fail setup's check. Or, after setup, run
   `<runtime>\kit\PLAY1718.bat` on Windows. Stop: `kit\STOP.bat force`.
3. Keys: Enter START, X decide, arrows tactics, 5 coin, F11 window size.

Working with an AI coding agent: start with [AGENTS.md](AGENTS.md).
