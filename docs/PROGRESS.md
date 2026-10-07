# Progress

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

## Setup

1. Decrypt the disc (outside this repo) and copy the game folder to `<runtime>/game` on NTFS.
2. `WCCF1718_GAME=<decrypted game> tools/setup1718.py` (or `tools/restart.sh`): guards client/control,
   installs Kit 5 hook pieces and configs, builds `seat1\` (links + patched exe), `misc\` FPR_Emu.
3. `<runtime>\kit\PLAY1718.bat`; stop with `STOP.bat force`.

## Tools for continuing

- IDA Pro 9.3 headless + `tools/ida/ida_query.py` (decomp/xrefs/callers/str/disasm, ~13 s per batch; use your
  own database copy, open databases are locked). `tools/ida/dump.py` writes every function's pseudo-C.
- radare2; client .text/.rdata file offset = VA − 0x400C00 (.data − 0x401200); client uses ASLR.
