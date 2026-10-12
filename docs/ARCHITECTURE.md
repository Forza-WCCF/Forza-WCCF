# Architecture and the move to Rust

Status 2026-10-10. Decision asked by the owner: does the kit still need `play.py`, `english.py` and the rest?
Answer: no, not as the shipped product. A game launcher is normally one native program that owns its processes, with
the data work done at install time. The kit grew the other way: a Rust window in front of 9,286 lines of Python
(27 scripts) and a 35 MB embedded Python. The plan below moves the shipped kit to Rust in phases, each gated by
checks that compare the new code against the Python it replaces. Python stays for research tools outside the kit.

## What runs today

```
PLAY.exe (Rust window)
 └─ play.py (orchestration, 1,071 lines) ── writes data\running.json, then exits
     ├─ _xscn_watch.py            scene service (shared memory)
     ├─ _debug_launch.py control  Python as a debugger ── control_Release.exe ── 4 × match_Release.exe
     ├─ _debug_launch.py client   Python as a debugger ── client_Release.exe (projector)
     ├─ _ringedge_services.py     keychip / master / installer (TCP 40100-40107)
     ├─ _icc_reader.py            card reader (pipe wccf_icc_seat1)
     ├─ _jvs_board.py             I/O board (pipe wccf_jvs_seat1) ◄─ file ◄─ _keys_seat1.py (key driver)
     ├─ _debug_launch.py client   ── client_Release.exe (seat 1) ── winmm.dll (C hook) ── wccfpanel.dll (C panel)
     └─ _kit_helper.py            RESTART NOW, SEND LOGS, club view, start volumes (polls files the panel writes)
PLAY.exe stays without a window as the watcher: reads running.json, waits on the launchers' pids, runs play.py ended
```

| Kind | Files | Lines | Where it should live |
|---|---|---|---|
| Orchestration (`play.py`, `_debug_launch.py`) | 2 | 1,991 | PLAY.exe (phase 1, 2) |
| Runtime stand-ins and helpers (`_icc_reader`, `_jvs_board`, `_keys_seat1`, `_ringedge_services`, `_xscn_watch`, `_kit_helper`, `_seat_*`, `_relay`, `_log_inbox`, `_no_quickedit`) | 11 | 3,099 | threads of PLAY.exe's run (phase 3); server parts also built for Linux |
| Install tools (`setup.py`, `english.py`, `cards.py`) | 3 | 1,233 | PLAY.exe's Setup (phase 4) |
| Data libraries (card schema, decoder, boards, wallet, view, LZW, sound) | 9 | 2,402 | a `kit` library crate, beside what uses them |
| Player tools (`edit_club_card.py`, `send_logs.py`) | 2 | 561 | PLAY.exe subcommands |

Eight scripts are byte-identical in the 2017-18 kit (`_jvs_board`, `_no_quickedit`, `_xscn_watch`, `_relay`,
`club_card_schema`, `codes_inferred`, `ys_lzw`, `sound_defaults`); `_xscn_watch.py` is also vendored by the Linux
server. Today every shared fix is ported by copying files.

## What makes it hard to change

Each item is a source of bugs found in this repo's history, and each goes away with the move rather than by patching.

- **Process ownership is rebuilt from text.** `play.py` exits after the start; who owns what is reconstructed from
  `running.json` pids, process scans and log lines (`"launched client_Release.exe pid N"`, `"its window was closed"`,
  `"maximized"` in the panel's log). The watcher race fixed on 2026-10-10 came from this.
- **Sleeps as synchronisation.** Fixed `time.sleep(1.5)`, `sleep(2)`, polling loops with timeouts in every start
  and stop path.
- **Python as a debugger.** Every debug event freezes all of the game's threads until Python answers; the code has
  its own timing report for this (`_debug_launch.py`, events over 50 ms).
- **The same fact in two or three languages.** English on/off, `panel.txt` parsing, settings: C (panel), Python and
  Rust each read them their own way.
- **Near-duplicate functions.** `set_control_ip`, `set_satellite_no` and `set_conf_value` are one function three
  times; the Rust side had pause/countdown code for a console nobody sees any more.
- **Change history in comments.** 110 dated lines in the scripts ("2026-10-06: ...") explain past bugs inline. That
  history belongs in git; comments should say what the code does now.
- **Edition copies.** 2010-11 and 2017-18 share code by copying files and porting commits by hand.
- **Big files.** `wccfpanel.c` 6,069 lines, `play.py` 1,071, `_keys_seat1.py` 990.

## Target

```
PLAY.exe (Rust, one process)
 ├─ window: play options, setup, update                              (done 2026-10-10)
 ├─ run supervisor: starts in order, keeps every child by its handle, knows the state in memory, answers
 │   the panel's restart request itself                                (done 2026-10-10, phase 1)
 ├─ debug launcher: the debug loop, shared memory, crash reports, MSGPARSE guard
 ├─ stand-ins as threads: keychip/master/installer, card reader, I/O board, scene service, key driver
 └─ setup / English / catalogue as install steps
winmm.dll, wccfpanel.dll, FPR_Emu.exe stay C: 32-bit code inside the game's process, already small and tested
```

Server-side parts (seat desk, match relay, log inbox, scene service) live in the same crate and build for Linux too,
replacing the vendored Python copies. An edition switch (2010-11 / 2017-18) replaces file copies between the repos.

## Phases and their gates

Every phase deletes the Python it replaces in the same PR, and keeps `check.ps1` green. Before a Python component is
removed, its behaviour is recorded (inputs → outputs, or a protocol transcript) and the Rust code is checked against
that record: the Python is the specification.

1. **Supervisor** (`play.py` → `launcher::run`) - done 2026-10-10 (branch `run-in-rust`; guarded live run passed:
   start, close seat 1, everything stopped). PLAY.exe starts the run itself and keeps it: no `running.json` for
   ownership, no watcher process, no log scraping; the Python stand-ins and `_debug_launch.py` stay child processes.
   `_kit_helper.py`'s RESTART NOW moves into the supervisor (PLAY is alive for the whole run). Gate: the ported
   unit tests (`test_ended`, card fix, log archive), `test_watch_late.py`, a run of stand-in programs, and one
   guarded live run on the Windows runtime (start, play screen, close seat 1, everything gone, routes unchanged).
2. **Debug launcher** (`_debug_launch.py` → Rust). Gate: stand-in programs that fault on purpose (access violation,
   invalid parameter, the MSGPARSE read) produce the same reports and recoveries; the live projector match timing is
   no worse.
3. **Stand-ins** to threads, one PR each, each with a transcript test recorded from the Python version: ringedge
   services, card reader, I/O board + key driver, scene service, then the server parts (with Linux builds).
4. **Install steps**: `cards.py` (the `image` crate in place of Pillow), `setup.py`, `english.py`; then the card
   decoder/editor. Gate: byte-identical outputs on the owner's game copy (catalogue, pictures, patched files). Then the
   kit ZIP drops `python\` (35 MB).

The 2017-18 kit follows each phase through the edition switch instead of a file port.

## Rules for the code

- Rust: `cargo fmt`, `clippy -D warnings`, `unsafe` only around Win32 calls with a `SAFETY:` line.
- Comments say what the code does and why; dates and bug stories go in commit messages.
- One fact, one place: a file format or rule is read by one module that everything else calls.
- Ownership by handle: a process the kit starts is waited on and stopped through its handle (or its Job object),
  never found again by name or pid.
- No fixed sleeps for synchronisation: wait on the thing (a port, a pipe, a window, a handle) with a deadline.
