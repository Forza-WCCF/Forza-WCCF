# The 2017–18 edition (from WCCF-Kit-1718)

Runs **World Club Champion Football 2017–18 (SDDY, Ver. 2.00.00)** on one Windows PC: the original server programs
(control + 4 match engines), the projector and a player cabinet (seat 1), built on
[WCCF-Kit-1011](https://github.com/Forza-WCCF/WCCF-Kit-1011) (Kit 5). Holds none of the original files: setup
works from your own decrypted copy.

**Status:** imported as the 2017–18 edition of Forza-WCCF ([CREDITS.md](../../CREDITS.md)). Offline play, clubs
that survive restarts (local Aime card, AiMeDB and game server stand-ins) and direct VS between homes run in Rust.
Plan and status: [../ROADMAP.md](../ROADMAP.md); notes: [PROGRESS.md](PROGRESS.md), [RESEARCH.md](RESEARCH.md).

## Layout

| Path | What |
|---|---|
| `editions/2017-18/` | what goes into the ZIP: English and Italian tables, overlay assets, READMEs |
| `crates/launcher` (`Edition::E1718`) | `Launcher.exe`: setup, the run, the stand-ins |
| `crates/forza/src/setup/e1718.rs` | setup: guarded imports, seat 1 exe patches, controller patch (every original byte checked) |
| `native/2017-18/` | `mxhook` (`winmm.dll`), `logowin`, `overlay`, `netguard/` (`wccfnet.dll`: stubs the IP-helper calls that drop the host's DHCP lease and loads the panel; `netguard.dll` is built only for the Linux server) |
| `legacy/2017-18/` | retired Python (`setup1718.py`, `restart.sh`) and its tests |
| `tools/press.sh`, `cargo xtask cap` | `press.sh` is a WSL helper that taps cabinet buttons; `cap` runs on Windows and captures game windows |
| `tools/ida/` | headless IDA helpers (`ida_query.py`, `dump.py`) for your own analysis copies |
| [research/catalogue-findings.md](../research/catalogue-findings.md) | 17–18 card catalogue findings |
| `docs/2017-18/` | progress, research notes, game-server message table |

## Quick start (from a Windows shell)

Read [../SAFETY.md](../SAFETY.md) first.

1. Copy your decrypted game folder to an NTFS runtime folder as `<runtime>\game`.
2. In a Windows shell in the repository ([CONTRIBUTING.md](../../CONTRIBUTING.md)):
   `cargo xtask build && cargo xtask stage 2017-18`, then copy `target\kit\2017-18` to `<runtime>\kit`. Set up
   with `Launcher.exe <runtime>\game` (or drop that folder on `Launcher.exe`; `Launcher.exe undo <runtime>\game` reverts).
   Setup works in place (it writes `original\`, `seat1\` and `setup-hashes.json` beside `game\`), so point it at a
   copy, never at your decrypted original. The copy must still be untouched (its `client_Release.exe` imports
   `IPHLPAPI.DLL`); an already guarded copy fails setup's check.
   Start: `<runtime>\kit\Launcher.exe` (double-click; from a console `Launcher.exe local`). It refuses to
   start (exit 5) unless `<runtime>\setup-hashes.json` matches the guarded programs and they import `wccfnet.dll`.
   Stop: close the game's window, or `Launcher.exe stop` (`stop force` for a stuck game).
3. Keys: Enter START, X decide, arrows tactics, 5 coin, F11 window size.
