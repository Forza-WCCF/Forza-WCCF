# Pause in local play (2026-10-19)

Research for the ROADMAP "Pause" and "Fast-forward 2×" bullets, following `GAME-CLOCK.md` (2026-10-14), which this
file extends and in two places corrects. Sources: copies of the original programs in
`<build-dir>\ida-pause-20261019\{1011,1718}\` (2010–11 from `ida-online-1011-20261014` and `ida-clock-20261014`,
2017–18 from the decrypted game `ringedge/1718-decryption-20261007-1944/game`), read with IDA through REA and with
`objdump`; the hook and panel sources (`native/2010-11/hook/wccfk.c`, `native/2017-18/mxhook/mxhook.c`, branch
`step9-eframe` `crates/panel`, `crates/launcher`, `crates/forza/src/standins`); one read-only look at the hook logs in
`D:\wccf-1011\data\logs\hook\`. Nothing was launched or changed in a runtime. [V] = seen in code, [A] = assumed.
Binary prefixes as in GAME-CLOCK: `C10`/`K10`/`M10` client/control/match engine 2010–11, `C17`/`K17`/`M17` 2017–18.
REA note: the REA server runs on Windows, so `open_binary` needs `E:\...` paths; `/mnt/<drive>/...` fails with EPERM.

## Conclusions

1. **The control's clock-stop switch is the "Stop" checkbox, ID 1035 (0x40B), in both editions.** It sits on the
   control's main dialog (resource 102, next to "NextState" 1034 and "DumpBuffer" 1036), a `BS_AUTOCHECKBOX`
   bound by `DDX_Control` to the `CButton` at dialog+0x7CC (its HWND at +0x7EC, the "+2028" of GAME-CLOCK). It has no
   click handler: the control polls it with `BM_GETCHECK`, so a `BM_SETCHECK` from another process acts exactly like a
   click. [V K10 0x4027AA, K17 0x40255A; message maps]
2. **Stop freezes only the shop clock.** While it is checked, the shop tick (K10 sub_446720, K17 sub_4557F0) throws
   the `_time64` delta away instead of adding it to the state time (+144) and game time (+128). That holds ENTRY
   (40 s), MATCHING_WAIT, the GAME cap (980 s), GAME_RESULT_WAIT and GAME_RESULT. **It does not stop the
   `CONTROL_STATE` send timer or the missed-answer counter**, which read `_time64` on their own (K10 sub_42E350,
   K17 sub_42F690) and never read the checkbox. A seat that stops answering is still dropped after ~36 s / ~66 s. [V]
3. **The original uses Stop for WCCF CUP mode.** The control checks it itself at start-up when a setting asks for it
   (K10 0x44B7A0, K17 0x45AC40) and when the cup starts (K10 0x44E8DE, 0x44F29B; K17 0x45E000, 0x45EEB9), and clears
   it at "WCCFCUP MODE OUT" (K10 0x404711, 0x45126D; K17 0x46220E) and in the NEXT button handler (ID 1048:
   K10 sub_403F50, K17 sub_404220). In ENTRY (state 1) Stop also changes how seat entries are answered (K10
   sub_42AE40, K17 near 0x42C2D5: with Stop, the matching start waits for a counter ≥ 1500 or a cup flag instead of
   "ENTRY time > 5 s"), and K17 then sends the ENTRY rest time as −1 when a cup flag is set (0x430271). In GAME
   (state 5) the clock freeze is its only effect. [V code, A the meaning of the counter and flags]
4. **The seat's link to the control runs on the game's main thread.** The 2010–11 link tick (sub_40A770 → sub_407050
   → sub_407A20) is a method of the client's "Management" task node (vtable 0x9E21D4 region, `ClientSequenceNode`),
   so holding the frame loop also stops the seat's answers (`KEEP_ALIVE`) and its own 30 s check. [V vtable, A that
   task nodes tick only from the frame loop, which GAME-CLOCK shows]
5. **The card reader, the I/O board and the FPR table run on their own threads** and keep polling during a hold:
   the 2010–11 card reader is a thread object (vtable 0xA1BAE4, run = sub_4D5900, started by the base class's
   `_beginthreadex` at 0x75F67D), the I/O board has "SetupDeviceThread" (0x82D8E0), the FPR reader polls FPR_Emu every
   10 ms from a thread (`native/fpr_emu/fpr_emu.c`). None of the stand-ins drops a quiet client except the 2017–18
   AiMeDB one (30 s idle per connection). [V]
6. **The 2010–11 software frame limiter does not catch up after a stall.** sub_75FCD0 restarts its frame count and
   base time when it is more than one frame late, so a long held frame never makes the game run fast afterwards;
   the client's QPC need not be virtualised. [V]
7. **The match engines need no clock change in a match.** M17 reads `timeGetTime` only in its start-up handshake
   (sub_406060, "START_UP_OK"), an option-file reader and its file loader; its QPC users are profiling; its main tick
   (sub_401460) runs the task tree and `Sleep(1)`. M10 is lock-step (GAME-CLOCK). The exception is M10 training,
   which ends after 23 QPC seconds. [V for the clock users, A that M17's sim is lock-step like M10]
8. **Audio keeps playing while frames are held.** Both clients create XAudio2 2.3 through `CoCreateInstance` (its
   CLSID is in both exes; no DirectSound, no WASAPI). 2010–11 has its own "ZDX" sound layer with a stream thread;
   2017–18 runs CRI Atom (ADX2) over XAudio2. Muting the process's audio session is the cheap fix; stopping the
   XAudio2 engine is the real pause. [V CLSID and strings, A thread behaviour]
9. **Cheapest safe design: pause only during the match itself, hold frames in the panel, and stop the clocks with
   one shared virtual clock that the three hook DLLs serve.** First a client-only pause capped at 20 s with the Stop
   checkbox guarding the GAME cap; then the shared clock (control `_time64` + `timeGetTime`, client `timeGetTime`,
   engine `timeGetTime` + QPC) for long pauses. Details in §5.

## 1. The Stop checkbox

| | 2010–11 (K10) | 2017–18 (K17) |
|---|---|---|
| Control | ID 1035, caption "Stop", style 0x50010003 (child, visible, tab stop, auto checkbox) in DIALOG 102 | same ID, caption, style and dialog |
| Binding | `DDX_Control(pDX, 0x40B, dlg+0x7CC)` at 0x4027AA; HWND at dialog+0x7EC | same, at 0x40255A |
| Click handler | none (message map has 1048 NEXT → sub_403F50, 1034 NextState → sub_404080) | none (1048 → sub_404220, 1034 → sub_404350) |
| Readers | shop tick sub_446720 (twice), ENTRY answers sub_42AE40 | shop tick sub_4557F0 (0x45586A, 0x455A62), ENTRY 0x42C2D5, `CONTROL_STATE` build 0x430271, svrMng state enables NEXT 0x45D64E |
| Set by the original | 0x44B7A0 (start-up, setting +0x3C0 = 1), 0x44E8DE, 0x44F29B (cup) | 0x45AC40, 0x45E000, 0x45EEB9 |
| Cleared by the original | 0x404711, 0x45126D ("WCCFCUP MODE OUT"), sub_403F50 (NEXT) | 0x46220E ("WCCFCUP MODE OUT"), sub_404220 (NEXT) |

What it freezes [V K10 sub_446720]: each tick reads `_time64` into +88 and `timeGetTime` into +96, takes the delta
since the last tick, and only when Stop is unchecked (and a second flag, `sub_43AE50()+275`, is clear) adds it to the
state time +144 and the game time +128. The base (+104) moves on either way, so the paused time is lost, never added
later. Everything the state machine sub_446A20 decides from +144/+128 waits: ENTRY 40 s, CR_DATA_WAIT 5 s,
MATCHING_WAIT 30 s, MATCHING 5 s, the GAME cap 980 s and the +14 s after all games end, GAME_RESULT_WAIT +11 s,
GAME_RESULT 60 s. The ENTRY rest time sent to the seats (40 − state time, sub_42AE40) stops counting with it.

What it does not freeze [V]: the `CONTROL_STATE` sender (every > 2 s of `_time64`, each send +1 on the seat's miss
counter) and the drop at > 11 (K10) / > 21 (K17) misses; the client's own 30 s check; the gmsvr link.

**Is `BM_SETCHECK` from outside enough?** For the shop clock, yes: the window is hidden by the launcher
(`crates/launcher/src/procs.rs` `hide_windows`, run/mod.rs:1349) but exists, the control polls the state every tick,
and `BM_SETCHECK` carries no pointer, so it works across processes. Find it with `EnumWindows` → the `#32770` window of
the kit's `control_Release.exe` (title "control" in 2010–11, blank in 2017–18) → `GetDlgItem(dlg, 1035)`. [V for the
code path, A for the window class until a window list from a run confirms it]

Side effects and rules:
- Set it only while the shop state is 5 (GAME). In state 1 it changes seat admission; in 0 (boot/sync) it would
  hold start-up.
- Always clear it on resume, and never clear a Stop the panel did not set (a cup mode, a start-up setting). Read the
  state first; if it was already checked, leave it alone and do not offer a pause.
- The NEXT button (1048) and "WCCFCUP MODE OUT" clear it; offline neither runs, but a resume that finds it already
  cleared is not an error.
- While checked, the control's NEXT button becomes enabled in 2017–18 (0x45D64E); the window is hidden, so nobody
  sees it.
- Stop alone gives at most the link limits: ~25 s safe (§2).

## 2. Wall-clock timeouts during a pause

"Fires in P1" = with the seat's frames held and nothing else changed. Clock = what each limit reads. Numbers refer to
GAME-CLOCK §2 where they come from there.

| Where | Clock | Limit | During a held frame | P2 fix |
|---|---|---|---|---|
| K10 seat drop (GC #1/#2: sub_42E350 sends, sub_42BFA0 drops) | `_time64` (sends) | > 11 misses ≈ 33–36 s | **fires** at ~33 s: the seat sends no `KEEP_ALIVE` | control `_time64` frozen: no sends, no misses |
| K17 seat drop (GC #3: sub_42F690, sub_42D620) | `_time64` | > 21 misses ≈ 63–66 s | fires at ~63 s | same |
| C10 error 2001 (GC #4: sub_407A20) | `timeGetTime` since the last `CONTROL_STATE` | 30 s | fires on the first tick after resume if the gap was ≥ 30 s minus the age of the last `CONTROL_STATE` (≤ 3 s): **27 s worst case** | client `timeGetTime` frozen |
| C17 2001 / 2012 / reconnect (GC #5: sub_410310, sub_40F600) | `timeGetTime` | 30 s (60 s in states 3 and 16), 49 s reconnect, 50 s 2012 | same, 27 s worst case | same |
| Request waits C10/C17 (GC #6, #7, #9) | `timeGetTime` | 10 s, 5 s × 3, 3 s | fire after resume only if a request was open | pause only in the match, where none is open [A]; client `timeGetTime` frozen |
| Shop cycle (GC #13: K10 sub_446A20, K17 sub_455AD0) | shop clock from `_time64` | GAME cap 980 s from game start; others in §1 | the cap keeps running: many short pauses can end the game "TIMEOUT! game" | Stop checkbox (P1) or control `_time64` frozen (P2) |
| ENTRY countdown on the seat (GC #14) | the control's | 40 s | runs out while the seat is frozen | not allowed: no pause outside the match |
| Client↔engine mutex (GC #10: C10 sub_431E40; M17 0x609344 "MemmapCommErrorTimeout") | `WaitForSingleObject` | 30 ms per wait | not reached: the hold is at Present, outside any mapping call [A] | none |
| Engine start-up (GC #11, #12; M17 sub_406060) | QPC / `timeGetTime` | 60 s, 1 s × 120 | start-up only | not allowed in start-up |
| M10 training | QPC | 23 s | training ends early | engine QPC frozen, or no pause in training |
| C17 frame counters (GC #15–#17: 900, 300, 150) | frames | — | stop with the frames | none |
| P2P (`TIMEOUT_P2P`, `P2P_DISCON`; string also in M17) | sockets | not read | online only | pause refused online |
| Card reader (2010–11 COM1 pipe, `card_reader.rs`) | its own thread; the stand-in has no timeout | game side 110–150 ms per exchange | an exchange the main thread started may time out after resume (its deadline clock not identified) | no pause while COM1 has traffic (§5 guard); client `timeGetTime` frozen |
| I/O board (JVS pipe, `io_board.rs`) | own thread, 150 ms event waits | per poll | keeps polling, answered by the stand-in | none |
| FPR_Emu (2010–11 table) | game thread polls every 10 ms | none on either side | keeps polling | none |
| AiMeDB stand-in (2017–18, `aimedb.rs:204`) | `Instant` | 30 s idle per connection | closes an open connection | none needed: no AiMeDB connection is open in a match [A] |
| gmsvr stand-in (2017–18, `gamesvr.rs`) | none (100 ms read retries) | — | — | — |
| The 2010–11 projector (second client) | as C10 | as C10 | keeps running; with a frozen control it would see no `CONTROL_STATE` | P2: refuse while the projector runs, or give it the same clock [A] |

Other clock readers found, none of them a limit that matters offline [V lists]: K10 reads `_time64` in 60
functions, `timeGetTime` in 37, QPC in 7 (profiling; answers GAME-CLOCK's open question: K10 imports QPC), plus
`GetLocalTime`, `_ftime64_s`, `GetSystemTimeAsFileTime` and `GetTickCount` in a few; M17 `_time64` in 23 (dates and
logs). The control's DistributePic downloads (sub_44CBC0, sub_44D230) are online only.

## 3. Audio

| | 2010–11 | 2017–18 |
|---|---|---|
| Engine | own "ZDX" layer on XAudio2 ("ZDX_SetupSoundXAudio2", `IXAudio2VoiceCallback`); announcer banks `.acb` | CRI Atom / ADX2 on XAudio2 ("XAudio2 API is not available. Atom library operates without sound output."); movies `.usm` |
| Creation | `CoCreateInstance` (ole32 import, 5 call sites) with CLSID XAudio2 2.3 `{4c5e637a-16c7-4de3-9c46-5ed22181962d}` | same CLSID in the exe |
| Threads | a stream thread started at 0x71FF5A (routine 0x71FD20, next to `ZDXSA_SetOutVol`) [V start, A role] | CRI's server thread [A] |

While frames are held the BGM stream and every voice already started go on; no new cue starts. Options:

- **Mute (P1).** `ISimpleAudioVolume::SetMute` on the client's own audio session, the Volume Mixer switch the launcher
  already drives (`crates/launcher/src/helper/sound.rs` `apply(pid, None, Some(true))`, `read(pid)` first to keep
  the player's own mute). Works from inside the client (the panel) or from the launcher. The BGM is ahead by the
  paused time afterwards; harmless.
- **Stop the engine (P3).** Hook `CoCreateInstance` in the client's IAT, keep the `IXAudio2*` whose CLSID matches,
  call `StopEngine` (vtable slot 12) on pause and `StartEngine` (slot 11) on resume. Every voice, streams included,
  waits where it was. Risk: the ZDX stream thread and CRI's server see buffers not ending for minutes; neither is known
  to have a watchdog [A]. Do not call it from inside an XAudio2 callback.
- Do not freeze QPC in the client for this: CRI is linked into the exe and its timing calls would go through the
  hooked IAT with unknown effects [A].

## 4. Input and the panel

The panel (`crates/panel/src/lib.rs`, step9-eframe) detours `IDirect3DDevice9::Present`; each call copies the game's
frame, draws the panel on its own additional swap chain and presents that in place of the game's (`Render::frame`).
The game's loop is single-threaded (GAME-CLOCK §1), so **not returning from `hook_present` holds the whole game**:
updates, `TO MATCH` frames (so the engine), frame counters, the link tick. The hold loop, inside `hook_present`:

1. `PeekMessage(PM_REMOVE)` and dispatch, so the window never goes "Not responding" (Windows ghosts a window that
   retrieves no messages for 5 s). The subclassed `wndproc` already sees every message: while paused it swallows
   mouse and keys for the game (egui still gets them), takes the pause key, refuses F11 (a resize would send the
   game `WM_SIZE`, and a game-side device `Reset` inside our Present is unsafe [A]), and on `WM_CLOSE` resumes first
   and lets it through.
2. `Render::frame(dev, wnd)` with a "paused" flag: the game's back buffer still holds its last frame (the game is
   blocked inside Present), so the existing copy-and-compose path draws it with the PAUSED page on top, presented at
   the game's own interval (vsync) on our swap chain. The per-frame game patches (`frame_stats_tick`,
   `dispenser_tick`, `free_play_tick`) are skipped while paused.
3. If `frame` returns `None` (no own swap chain, a full-screen game) or our `Present` reports a lost device, leave
   the hold at once and return to the game. Pause is offered only when the own swap chain exists.

Reading the pause request:
- **Keyboard**: a key in `keys.txt` (new action `PAUSE`, default `P` or Pause/Break 0x13), seen by the panel's
  `wndproc` while paused too, because the hold loop pumps.
- **Panel button**: an egui button; egui keeps working inside the hold loop.
- **Controller**: the launcher's key driver (`crates/launcher/src/keys.rs`, `GetAsyncKeyState` and `joyGetPosEx` while
  a game window is in front) sees it regardless of the hold; it writes a pause request into the shared clock section
  (§5), which the panel reads every loop. While paused the driver should send no cabinet inputs to the I/O board
  stand-in, so nothing pressed during the pause reaches the game afterwards [A: the JVS thread latches the switch
  state].

The PAUSED page: one centred text and two buttons (Resume, and in P1 the seconds left of the 20 s cap), English
source with Italian and Japanese in the panel's text tables; nothing else is needed.

## 5. Design and milestones

### Where pause is allowed (all milestones)

Only when all hold: the launcher runs this PC's control and engines and one seat, offline (no online mode, no World
Trophy, no linked or network seat, no 2010–11 projector in P2); the seat's copy of the shop state is 5 (C10
`*(0x113700C)+12`, C17 `*(0x6D9EB08)+12`); the client↔engine traffic is live (the match runs between GO TO GAME and END
MATCH GAME); no card traffic in the last second (no COM1 bytes in 2010–11; no card/save exchange in 2017–18);
not in M10 training. This keeps every save (2010–11 card writes after the match, 2017–18 `SAVE_DATA_SET` relayed by
the control) and every ENTRY answer out of a pause. Menus are excluded: they need Stop's ENTRY behaviour and the
900-frame card screens, and the request was for the match. [judgement, A that saves never fall inside the match]

"Traffic is live" without a new address: the 2010–11 hook already logs `CreateFileMappingA "MemmapComm memmap
wccf_match_connect0"` (seen in `mxhook_27100.log`); its mutex is presumably "MemmapComm mutex wccf_match_connect0"
(format string in M17). Hooking `CreateMutexA`/`OpenMutexA`/`ReleaseMutex` in the client and timestamping each
release of that handle gives "the engine was talked to N ms ago". [A name, V mapping name]

### P1: client-only pause, capped at 20 s

- Panel: pause state, hold loop (§4), PAUSED page, 20 s cap (auto-resume) and a 10 s minimum run between pauses so at
  least three `CONTROL_STATE`/`KEEP_ALIVE` rounds reset the miss counter; mute through the session volume.
- Panel: set Stop (ID 1035) on the kit's control window when the pause starts and clear it on resume, only if it was
  clear before; this keeps the 980 s GAME cap from counting the pauses. Without it, P1 needs a per-match pause budget
  instead (unknown slack under 980 s) [A].
- Hooks: one export each in the 2010–11 and 2017–18 hook for the guard: COM1 traffic age (from `hook_WriteFile`/
  `hook_ReadFile`, already there for the vcom ports) and engine-traffic age (the mutex hooks above). The panel finds
  them with `GetProcAddress` on the hook module.
- Safe because: 20 s < 27 s (worst client 2001) < 33 s (K10 drop); the engine waits; frame counters stop. [V limits]

### P2: one shared virtual clock, pause up to a few minutes

A named section, `Local\ForzaWccfClock-<id>`, whose name the launcher passes to all three programs in an environment
variable (say `WCCF_CLOCK`) only for an allowed local run; without it every hook passes the real clock through. One
writer (the panel), read lock-free by the hooks (a sequence counter around the fields, as 32-bit processes cannot
read 64-bit values atomically without `cmpxchg8b`):

| Field | Meaning |
|---|---|
| `seq` | odd while the panel writes |
| `rate_num`/`rate_den` | 0/1 paused, 1/1 normal, 2/1 fast-forward |
| `base_real_qpc`, `base_virt_qpc` | the last change point; virtual = base_virt + (real − base_real) × rate |
| `request` | written only by the launcher's key driver: pause/resume asked |

Each hook converts the one virtual QPC value to its function: `timeGetTime` = virtual ms, `_time64` = boot wall time
+ virtual seconds, QPC = virtual ticks. The value is monotonic and never jumps forward on resume; after a pause every
clock simply lags the wall clock by the paused time (dates on a save are off by minutes; harmless offline).

| Program | Hooks to serve virtual time | Why |
|---|---|---|
| control (K10/K17) | `_time64` (MSVCR90 IAT), `timeGetTime` | shop clock, `CONTROL_STATE` sends and misses, GmSync waits |
| client (C10/C17) | `timeGetTime`; `_time64` for consistency | 2001/2012, request waits; not QPC (limiter resets, CRI risk) |
| match engine (M10/M17) | `timeGetTime`, QPC | M10 training 23 s; start-up waits stay outside a pause anyway |

With all three on one clock the control sends nothing and counts no misses while paused, the client's 30 s check sees
no gap, and the shop cycle waits; Stop is no longer needed. Threads other than the game's main thread also see the
clock stop; anything that spins until a deadline would spin for the pause (none found) [A].

Code to add:
- `native/2010-11/hook/wccfk.c`: replace the `timeGetTime` forwarder in `winmm_fwd.h` by an own export (the real one
  from System32's winmm by `GetProcAddress`); `patch_iat(exe, "MSVCR90.dll", "_time64", …)`; in the match engine
  `patch_iat(exe, "kernel32.dll", "QueryPerformanceCounter", …)`; open the section in `DllMain` from `WCCF_CLOCK`;
  the P1 guard exports; log one line per process saying whether the clock is virtual.
- `native/2017-18/mxhook/mxhook.c`: the same; its `winmm_fwd.h` forwards every winmm export to `winmm_orig`, so only
  the `timeGetTime` line becomes an own export.
- `crates/panel`: create and write the section; drive pause from the key, the button and `request`; drop the 20 s cap
  for a soft limit (say 10 min) and keep the Stop setting as a second guard on the 980 s cap.
- `crates/launcher`: create the name and the environment variable only for an allowed run; the `PAUSE` key action;
  no cabinet inputs while paused; a log line on each pause and resume.

### P3: uncapped pause

P2 without the soft limit, plus `IXAudio2::StopEngine` for a real audio pause and pause in the 2010–11 projector (its
hook on the same clock and its panel holding frames too). Only after a P2 run has shown clean control and client logs
over pauses of several minutes.

### The guarded live test (the maintainer, later)

Per edition, launched with the §5 guards of `AGENTS.md`, one seat, offline, a match against the CPU:
1. Evidence before: routes and a ping; a hash of the club card file (2010–11) or the `gamesvr.db` row (2017–18);
   the hook log of each process shows `mxhook attached` and, from P2, the clock line.
2. P1: three pauses of 15–20 s in the first half, one at half-time, one in the second half, and one forced to the
   cap. P2: one pause of 2 min and one of 5 min.
3. Pass when: the match reaches the result and the save; the club card (or the gmsvr row) changes only by that save
   and reads back in the next game; no error 2001, 2012, 3009, 3107 or 3153 on the seat; the control log has no
   `--DISCONNECT--`, `TIMEOUT!`, `GameManage::decline`; the AiMeDB/gmsvr stand-in logs show no timeout; the audio
   comes back; the game runs at normal speed after each resume (frame stats in the panel log); the window never shows
   "Not responding".
4. Record negative results, including the window list proving the control's `#32770` window and ID 1035.

## 6. Fast-forward 2×

The same section serves it: rate 2/1. Findings that change the ROADMAP plan:
- Stop must not be used for 2×: in ENTRY it changes seat admission (§1).
- The C10 limiter resets when it is more than a frame late (Conclusion 6), so "show every second frame" is enough
  where vsync paces the game; where the software limiter paces it, the client's QPC at 2× is needed, which brings the
  CRI timing risk of §3 into menus. Prefer frame skipping with the hardware interval [A].
- At 2× on the control's `_time64`, `CONTROL_STATE` goes out every second of real time and the miss limit halves in
  real time (~18 s / ~33 s); a 2× seat answers twice as often, so this only matters if the seat stalls.
- XAudio2 can slow or speed nothing globally; at 2× mute or leave the sound as is.

## Open

- The control window's class and title in a run (window list), and that `BM_SETCHECK` on the hidden dialog is read.
- The client-side flag for "match running" if the mutex counter proves awkward; the mutex name.
- Which clock bounds the 2010–11 card exchange on the main thread (the 110–150 ms limits), so the COM1 guard can be
  justified or dropped.
- M17's sim loop: confirmed only by the absence of clock users; one IDA pass on its `TO MATCH` handler (registered at
  sub_405760, polled at sub_5CF6D0) would make it [V].
- The meaning of K10 `sub_43AE50()+275` (a second shop-clock hold) and the ENTRY counter +140 ≥ 1500 under Stop.
- How much of the 980 s GAME cap a CPU match leaves free (for a P1 budget without Stop).
- XAudio2 `StopEngine` behaviour with the ZDX stream thread and CRI.
