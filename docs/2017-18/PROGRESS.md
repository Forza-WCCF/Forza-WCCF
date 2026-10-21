# Progress

## 2026-10-09 — card drop (a test), not yet seen live

Ported the 2010-11 kit's player card after each match (WCCF-Kit-1011 PR #17): a window shows a catalogue card,
2.5 % RARE or better, else 70 % REGULAR / 30 % SPECIAL; nothing saved. 2010-11 triggers on the club card's session
mark; 17-18 has none, so the panel hooks the game's own owed-card function FUN_005c0970 (the 2010-11 dispenser fix's
FUN_004cf650, identical but its addresses; the exe is ASLR, so base + 0x1C0970 and the global read from its mov).
It now jumps to a counter: no card is owed (the dispenser fix, which never matched 17-18 before: "FUN_004cf650 is
not the code studied"), and each hand-out shows a card. fakegame: odds PASS, hook PASS (counted 1), window draws
with the 8,396-card catalogue. Not yet run in the game.

## 2026-10-08 — players have models

Invisible / headless players fixed: the real Launcher.exe maps prog_data/player_data/{player_data, partnership,
model_data}.bin as "SDDY_<file>" (u32 size + bytes) before it raises the version byte; the kit's launcher stand-in
made only the version mapping, so control, the match engines, the projector and the seat found no player/model
data. With the mappings the online tutorial (manager licence → tutorial match) shows full player models.

## 2026-10-08 — online with an Aime card

Seat 1 is online: the online icon is green, the Aime screen reads the (emulated) card, AiMeDB gives Aime ID 1,
the client asks the game server (through control) for the coach list and shows the first-play TUTORIAL / NORMAL
choice (NORMAL "選択不可" for a new player) instead of OFFLINE guest play. New: three seat patches (ALL.Net
authorized, billing phase IDLE, AiMeDB host → localhost), `_aime_reader.py` (COM2), `_aimedb.py` (port 22345),
`play.py` starts both for 17–18. Safety: routes identical, ping 4/4, HTTPS 200; no publisher host contacted
(ALL.Net off, AiMeDB local).

## 2026-10-07 — boot to guest play, controller online (paused)

Observed on Windows 11 + WSL, game `DVR5059A_2_00_00` (SDDY, build 2018-03-15):

- Server: original `control_Release.exe` + 4 match engines (guarded imports), listening on 20002, round cycle
  reaches GAME_READY. The 17–18 controller waits 60 s for shared memory `SDDY_ WCCF 17-18 Ver2.0` (byte 0 = 1),
  else requests `\\.\mxhwreset`; `_debug_launch.py` creates it. No xscn scene service in 17–18.
- Seat 1 needed 13 patches (`tools/patches1718.py`), each ported from Kit 5 and checked: keychip 0949, JVS mode 0,
  resolution 0910, camera 0xC05/5801, main board 5303/0025/0026, 1440×900, smooth scaling, picture fills the
  back buffer. Network check pings 192.168.46.254 (ICMP, 5 s timeout), harmless.
- Seat 1 reaches attract; START via the JVS stand-in reaches OFFLINE guest play; a guest CPU match was played
  (players invisible). Kit 5 side panels (`wccfpanel.dll`) work unchanged around the 1440×900 picture.
- Controller connects to `_gamesvr.py` and runs its startup exchange (201, 265, 237, 233, 259, 263, 267, 44,
  46, 309, then 261 every 6 s) with all-zero answers.
- Safety: routes identical before/after, ping 4/4, HTTPS 200; no host network/firewall changes.

## Setup and tools

Setup and the run are Rust now: see [README.md](README.md) for the quick start and [../SAFETY.md](../SAFETY.md) for
the guard. Address arithmetic for quick looks with radare2: client .text/.rdata file offset = VA − 0x400C00 (.data
− 0x401200); the client uses ASLR.
