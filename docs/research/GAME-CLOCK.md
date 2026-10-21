# Game clocks, timeouts, pause and 2× (2026-10-14)

Research for ROADMAP "Community requests": **Pause** and **Fast-forward 2× in menus**. Sources: the
original programs, analysed on copies only. 2010–11: `client_Release.exe`, `control_Release.exe` and
`match_Release.exe` in IDA. 2017–18: `client_Release.exe` and `control_Release.exe` in IDA; the 2017–18 match
engine was not opened. Nothing was launched. [V] = seen
in code, [A] = assumed. Addresses name the binary: `C10` client 2010–11, `K10` control 2010–11, `M10` match engine
2010–11, `C17` client 2017–18, `K17` control 2017–18.

## Conclusions

1. **Game time is frames, not milliseconds.** Both clients run one single-threaded loop ("[ces] Mode SingleThread")
   that updates once per frame and presents with the hardware interval ONE (vsync, 60 Hz), or a QPC busy-wait
   frame limiter when the hardware interval is not usable. Menu countdowns and request waits count frames
   (2017–18: a global frame counter feeds a 900-frame IC-card timeout and a 300-frame error timeout). Scaling
   `timeGetTime`/QPC alone neither pauses nor speeds the game. [V]
2. **The match is lock-step on the client frame.** The 2010–11 match engine simulates up to the frame index the
   client last sent in `TO MATCH` (≤ 18,000 frames = 5 min at 60 fps) and otherwise sleeps 1 ms; it has no
   timeout of its own in that loop. When the client stops, the match stops. [V M10] The 2017–18 client speaks the
   same `TO MATCH`/`FROM MATCH` shared-memory protocol to its own `match_Release.exe`. [V strings, A same design]
3. **The control runs the shop on wall-clock seconds.** Both controls keep a shop cycle (`CoStateManage`) — ENTRY
   40 s, CR_DATA_WAIT 5 s, MATCHING_WAIT 30 s, MATCHING 5 s, GAME (until all games end + 14 s, cap 980 s),
   GAME_RESULT_WAIT (+11 s), GAME_RESULT 60 s — advanced by `_time64` deltas, not by `timeGetTime`. The ENTRY
   countdown a seat shows comes from the control. A paused seat falls out of step with this cycle. [V]
4. **The link dies after ~30–66 s of a stopped seat.** The control sends `CONTROL_STATE` at most every 3 s
   (`_time64`) and drops a seat after 11 (2010–11) or 21 (2017–18) unanswered sends: ~36 s / ~66 s. The client
   raises error 2001 when no `CONTROL_STATE` came for 30 s (`timeGetTime`; 60 s in two 2017–18 states). [V]
5. **Pausing the seat alone is safe only under ~25 s.** For a longer pause the control's clock (`_time64`) and the
   client's `timeGetTime` must stop together; with them stopped, the control sends no `CONTROL_STATE`, counts no
   missed keep-alives and the 980 s GAME cap does not run. The match engine needs no clock change. [V for each
   clock, A for the combined behaviour until a guarded run shows it]
6. **The control already has a clock-stop switch.** Each tick both controls read a checkbox on their own window
   with `BM_GETCHECK`; while it is checked the shop clock does not advance and 2010–11 sends the ENTRY rest time as
   −1. Which checkbox that is (and whether the kit shows the control window) is open. [V K10 sub_446720,
   K17 sub_4557F0; A purpose]
7. **Fast-forward needs more frames per second, not a faster clock.** 2× = present every other frame (or run with
   presentation interval IMMEDIATE) and, where the software limiter runs, scale QPC 2× in the client. Wall-clock
   menus (ENTRY, MATCHING_WAIT, GAME_RESULT) shorten only if the control's `_time64` runs 2× too. [V mechanism,
   A effect]
8. **Unsafe modes: say no.** Pause and 2× are unsafe online (live P2P matches between shops in 2010–11, World
   Trophy, ghost/server requests), with any second seat linked (in-shop P2P matches; one shared control clock),
   and during start-up/network setup. Allow them only with one seat, offline, control and engines local. [V for the
   links, judgement for the rule]

## 1. Clocks and main loops

| Program | Imported clocks [V] | Main loop | How game time advances |
|---|---|---|---|
| C10 | `timeGetTime` (155 refs), `timeBeginPeriod(1)` once (sub_84D7A0), QPC/QPF (sub_720540 = QPC in µs), `GetTickCount` (sub_7FD660, ALL.Net side), `_time64`, `Sleep` | sub_84D870 → sub_84C9C0 → sub_84CA60: PeekMessage pump, then the task tick sub_8E0AA0, frame stats (16,666 µs reference) | one task tick per frame; sub_7605C0 → sub_75FCD0 is the end-of-frame limiter [V] |
| C10 present | sub_760A90 (`DeviceDX9.cpp`) | full screen: refresh 60, interval ONE (or 2/3/4 from the setting); windowed: interval ONE if the desktop is 60 Hz, else IMMEDIATE plus the software interval (this+44) | software limiter busy-waits on QPC for 16,667 µs × N (sub_75FCD0) [V] |
| K10 | `timeGetTime`, `timeBeginPeriod` (sub_48AC00), `_time64` | MFC window; `CoStateManage` tick sub_446720 | shop clock: `_time64` seconds since boot, added to game time (+128) and state time (+144) unless the checkbox is set [V] |
| M10 | `timeGetTime` (start-up 1 s retry ×120, sub_4046D0), `timeBeginPeriod(1)` (sub_4011E0), QPC (sub_5C4920 µs), `Sleep(1)` | sub_403250 → sub_5C3240 / sub_401790 | steps sim frames up to the client's frame index `i` (0x77EBF8, set from `TO MATCH` in sub_562F50); training mode steps once per call and ends after 23 QPC seconds [V] |
| C17 | `timeGetTime`, `timeBeginPeriod`, QPC/QPF, `GetTickCount`, `_time64`, `Sleep` | sub_401910 → sub_4016F0 ("CoreFrame"): one update per frame, frame counter `dword_6D9EB10` += 1 | frame-counted sequences (e.g. sub_77A7D0: 900 frames; sub_417BB0: 300 frames) [V]; teaGfxAPI interval code sub_C29F70 as in C10 [V strings, A logic] |
| K17 | as K10 | `CoStateManage` tick sub_4557F0, timing sub_455AD0 | identical cycle and checkbox; GmSync file-sync step in state 0 uses `timeGetTime` 5 s [V] |
| M17 | `WINMM.dll`, QPC, `_time64` imported [V strings] | not read | [A] as M10 |

All three 2010–11 programs load `wccfk.dll` through the renamed `WINMM` import (`setup.py` `setup_extracted`), and
all three 2017–18 programs import `WINMM.dll` by name, so the kit's `winmm.dll` proxy loads in each. Both hooks
already patch the game module's IAT (`native/2010-11/hook/wccfk.c` patch_iat_any, `native/2017-18/mxhook/mxhook.c`).
[V]

## 2. Timeouts that fire if one seat stops or runs 2×

| # | Where | Clock | Limit | Effect | |
|---|---|---|---|---|---|
| 1 | K10 sub_42E3B0 / sub_42E350; K17 sub_42F6F0 / sub_42F690 | `_time64` | sends `CONTROL_STATE` when the state changes or > 2 s passed | each send +1 on the seat's miss counter | V |
| 2 | K10 sub_42BFA0 (reset in sub_4352C0 on `KEEP_ALIVE`) | counter of #1 | > 11 misses (~36 s) | `--DISCONNECT--`, error 2001 `TRCT`, seat dropped, `GameManage::decline` | V |
| 3 | K17 sub_42D620 | counter of #1 | > 21 misses (~66 s) | same | V |
| 4 | C10 sub_407A20 → sub_407050 | `timeGetTime` since last `CONTROL_STATE` | 30,000 ms | error 2001 "past %d millisec" | V |
| 5 | C17 sub_410310 → sub_40F600 | `timeGetTime` | 30,000 ms; 60,000 ms in client states 3 and 16; 49,000 ms → reconnect (sub_4C1DE0); 50,000 ms without link → error 2012 "CNS" | error 2001 / 2012 | V |
| 6 | C10 sub_407050, C17 sub_40F600 | `timeGetTime` | 10 s (+1288 / Block[355]) and 5 s × 3 retries (+872 / Block[225]) | gmsvr/echo request gives up | V |
| 7 | C10 sub_407050, C17 sub_40F600 | `timeGetTime` | 5 s after `GMSVR_ECHO` | echo marked failed (online only) | V |
| 8 | C10 sub_435460 | `timeGetTime` | 10 s window, 40 counts | ping statistics, state 21 only | V |
| 9 | C10 sub_44B670 | `timeGetTime` | 3 s | clears a "server busy" flag | V |
| 10 | C10 sub_431E40 (memmap to engine) | `WaitForSingleObject` | 30 ms per mutex wait | error 3009 `MemmapCommErrorTimeout` if the other side holds the mutex | V |
| 11 | C10 sub_431FF0, M10 sub_557630 | QPC (ms) | 60 s, 500 ms sleeps | start-up wait for the `SBWG_status` mapping | V |
| 12 | M10 sub_4046D0 | `timeGetTime` | 1 s × 120 | `START UP` handshake to the client | V |
| 13 | K10 sub_446A20; K17 sub_455AD0 | shop clock (`_time64`) | ENTRY 40 s (rest time sent to clients), CR_DATA_WAIT 5, MATCHING_WAIT 30, MATCHING 5, GAME: 14 s after all games end, **cap 980 s from game start**, GAME_RESULT_WAIT +11 s, GAME_RESULT 60 s (5 s in one mode) | state change; "TIMEOUT! game / game_result_wait / game_result" ends the game for all seats | V |
| 14 | C10 sub_408040, C17 sub_410E00 | from #13 | ENTRY rest seconds + 80 shown/used by the seat | the seat's entry countdown follows the control | V field, A display |
| 15 | C17 sub_77A7D0 | frame counter | 900 frames (15 s at 60 fps) | `ICCardTimeOut` scene | V |
| 16 | C17 sub_417BB0 | frame counter | 300 frames (5 s) | leaves to the next scene or `ClientError` | V |
| 17 | C17 gmsvr relay waits | frames | 150 polls (~2.5 s) | ghost/rank answer treated as none (`ONLINE-2017-18.md`) | V |
| 18 | P2P link (`TIMEOUT_P2P`, `P2P_DISCON`) C10 sub_407050, C17 sub_40F600 | socket state, option `TIMEOUT_P2P` | not read | a live opponent's link drops | V names, A limit |
| 19 | Card reader / IO board | the kit's stand-ins (wccfk/mxhook serial and pipe emulation, `FPR_Emu`) | polled per frame | no watchdog seen; 2× doubles the poll rate | A |

Effects of a stopped seat with everything else running: #2/#3 and #4/#5 fire (first at ~30 s), #13 keeps cycling
(the ENTRY countdown runs out, a GAME runs into the 980 s cap). At 2× on the seat only: #15–#17 halve in wall time,
#4–#9 and #13 do not change.

## 3. Match or menus at runtime

| Source | Address | Values | |
|---|---|---|---|
| Shop state in the control | K10 `dword_7D30C0`; K17 `dword_AB8A60` | 0 boot/sync, 1 ENTRY, 2 CR_DATA_WAIT, 3 MATCHING_WAIT, 4 MATCHING, 5 GAME, 6 GAME_RESULT_WAIT, 7 GAME_RESULT | V |
| The same state in the seat (from `CONTROL_STATE` byte 0) | C10 `*(dword_113700C)+12` (object sub_407870); C17 `*(dword_6D9EB08)+12` (object sub_4100B0) | as above | V |
| Seat main state | C10 `*(dword_1137060)` (1 = START UP); C17 `*(dword_6D9EBCC)` (< 3 before the link, 3/16 waiting states, 14, 23) | meaning per value open | V address, A values |
| Match engine messages | M10 sub_563170: `SYNC READY`, `GO TO GAME`, `ENTER H-TIME`, `END MATCH GAME`; client → control `REQ_MATCH` (C17 sub_4BC150) | start, half-time, end | V |
| Frames to the engine | C10/C17 `TO MATCH` writers (C10 sub_4E3060, sub_4E9F40) | present while a match runs | V names, A per-frame |

For the panel and the hook: read the seat's copy of the shop state (one dword). Menus = 0–3 and 7; match = 4–6. A
precise per-seat flag (the seat may sit out a cycle) is `TO MATCH` activity, which the hook can count in the shared
memory traffic. [A]

## 4. Recommended mechanism

**One shared game clock, read by all three programs on the PC.** A named mapping (for example
`Local\ForzaWccfClock`: paused flag, scale, accumulated offset) written by the panel; the hook DLL already loaded in
each program (2010–11 `wccfk.dll`, 2017–18 `winmm.dll`) serves virtual time from it.

| Hook | C | K | M | Why |
|---|---|---|---|---|
| `timeGetTime` (own export instead of the forwarder) | yes | yes | yes | link timeouts #4–#9, #12 |
| `QueryPerformanceCounter` (IAT) | yes | if imported | yes | frame limiter, engine seconds |
| `_time64` (IAT, MSVCR90 import of the exe) | yes | **yes** | yes | the shop clock #1, #13 — the CRT's own call to `GetSystemTimeAsFileTime` is not reachable through the exe's IAT |
| `GetTickCount` (IAT) | yes | yes | — | minor users |
| Present (panel, seat window only) | yes | — | — | freezes or doubles frames |

**Pause.** The panel holds the seat's frame in its Present hook: it redraws the last frame with a PAUSED page and
keeps pumping window messages, so the game's update (and with it `TO MATCH`, the engine and every frame counter)
stops. At the same moment the shared clock stops for client, control and engine; on resume it continues from the
stopped value (offset kept, never a jump forward). The control then neither sends `CONTROL_STATE` nor counts misses,
the seat's 30 s check sees no gap, and the shop cycle (ENTRY, 980 s cap) waits. A cheaper first step: the control's
own clock-stop checkbox (§1 #6) set by `BM_SETCHECK`, plus a seat pause under 25 s. [A until a guarded run]

**2× in menus.** Allowed only when the seat's shop state is 0–3 or 7 and no match messages flow. The Present hook
returns at once for every second frame (the game runs two updates per vsync); where the software limiter is active
the client's QPC runs at 2×; the control's `_time64` and `timeGetTime` run at 2× so ENTRY and MATCHING_WAIT shorten
with it. Switch back to 1× when the shop state reaches 4 (MATCHING), before any `TO MATCH` frame. [A]

**Is pausing only the client safe when control and engines are local?** For under ~25 s, yes: the engine waits
(lock-step), the control's miss counter needs ~36 s (2010–11) / ~66 s (2017–18), the seat's own check 30 s. Longer,
no: the seat is dropped or raises error 2001, and in menus the ENTRY countdown runs out while the seat is frozen.

### Risks

- **Unsafe online or with linked seats.** A live opponent (another shop in 2010–11, another seat in the same shop
  in both editions) keeps running: P2P desync, `P2P_DISCON`, `TIMEOUT_P2P`. One control clock serves all seats, so
  freezing it freezes every seat's cycle. Block pause and 2× whenever online, World Trophy, a second seat or a
  network-entered seat is present. [V links, judgement rule]
- **Start-up and network setup** (C10 sub_40C920 "SEARCHING FOR THE SERVER", "WAITING FOR THE CONTROL", the 60 s
  mapping waits #11) use their own clocks; no pause before the first `CONTROL_STATE`. [V]
- **Audio** plays in real time: at 2× voices and jingles overlap or cut; during pause queued sounds may finish. [A]
- **Message pumping inside Present** re-enters the game's window procedure; input must be held by the panel, not
  delivered. [A]
- **Wall clock lag.** After pauses the programs' `_time64` stays behind the real clock by the paused time; harmless
  offline (dates and the WT window only matter online), reset at the next start. [A]
- **A held mutex.** Freezing with a thread suspended inside the client↔engine mapping lock would trip #10; the
  Present-based hold avoids that because the game is not inside a mapping call at Present. [A]
- **Saves at 2×** are frame-paced writes to the card stand-in; the gate in ROADMAP (save at 2× and with pauses,
  club card intact) covers it. [A]

## Open

- Which control-window checkbox `BM_GETCHECK` reads (HWND at dialog +2028) and whether the kit's control window
  exists or is hidden; one guarded run with a window list answers it.
- Whether the windowed kit runs the hardware interval or the software limiter (this+44 in C10 sub_760A90): read the
  "[teaGfxAPI] … interval" line in the client log.
- The 2017–18 match engine: confirm the lock-step loop (M10 sub_5C3240 pattern) and its imports.
- Whether K10 imports QPC (only `timeGetTime`/`_time64` were checked).
- The `TIMEOUT_P2P` default.
