# 2010-11 server hook

`winmm.dll` for the original control and its match engines on a Linux server under Wine (`crates/linux-host`). It
shares its start with the player hook `../hook/wccfk.c` but is its own branch: it forwards all 192 winmm exports to
`winmm_orig.dll` (Wine's own winmm, renamed; `winmm_fwd.h`, `winmm.def`), loads `netguard.dll` as the second guard,
and adds three server fixes. The player hook has the logowin stand-in, whole messages and the DHCP-guard checks
instead; neither is a superset of the other.

| Fix | What | Off |
|---|---|---|
| MSGPARSE guard | catches control's known fault in its message parser (`FUN_00412860`, a negative copy size from a seat's carried-over header count), unwinds that call with its length and offset zeroed, and drops that count | `MXHOOK_MSGGUARD=0` |
| SLEEP fix | control's `Sleep` calls that do not block under Wine become timed waits; its file reader's idle poll waits `MXHOOK_IDLE_MS` (default 4) | `MXHOOK_SLEEPFIX=0` |
| SPIN fix | control's DistributePic thread spinning on `timeGetTime` (a full core idle); with it, about 6 % | `MXHOOK_SPINFIX=0` |

The fixes turn on only in `control_Release.exe`; `=1` forces one on in another program (the self-tests). Each
process writes `mxhook_<pid>.log` to `MXHOOK_LOG_DIR`; control's says `MSGPARSE guard: on`, `SLEEP fix: on`,
`SPIN fix: on`. Like the player hook it blocks `IpReleaseAddress` and `IpRenewAddress`, and presents one network
address, `MXHOOK_NET_IP` (the supervisor sets 127.0.0.1).

## Build

    ./build.sh           # out/winmm.dll and the three self-tests (MinGW-w64, Debian: gcc-mingw-w64-i686)
    ./build.sh orig      # also out/winmm_orig.dll from Wine's 32-bit winmm.dll (on a host with wine32 installed)

The self-tests run under Wine beside `winmm.dll`, `winmm_orig.dll` and `netguard.dll` and print PASS:
`msgguard_test.exe` builds control's real caller frame (the hook from before the frame fix crashes it),
`MXHOOK_SLEEPFIX=1 sleepfix_test.exe`, `MXHOOK_SPINFIX=1 WINEDLLOVERRIDES=winmm=n,b spinfix_test.exe`.
`cargo xtask` does not build this hook: it runs only under Wine on a server.
