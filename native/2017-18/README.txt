The source of every program Forza-WCCF 2017-18 ships. Every program is Rust: `cargo xtask build` builds each from the
Forza-WCCF repository (this ZIP carries only the programs' crates), the game's 32-bit ones for i686-pc-windows-msvc,
the C runtime linked in (no Visual C++ runtime needed), each with its version information. The programs land under
target\ (the 2017-18 folder) at the paths below; `cargo xtask package 2017-18` zips that folder.

  bin\winmm.dll        native\hook (forza-hook --features e1718); its build.rs forwards all 192 winmm functions to
                       winmm_orig.dll (winmm_exports.txt)
  bin\wccfnet.dll      native\wccfnet (the guard: setup copies it into the runtime's game folder, where the game programs
                       import it instead of IPHLPAPI.DLL; the four stubs are its own exports, six read-only calls are
                       forwarded to iphlpapi by its build.rs. When the Launcher sets FORZA_PANEL, it also loads that
                       DLL (the panel) into the game)
  bin\dmath.dll        native\detmath (setup copies it into the game folder, where match_Release.exe imports it
                       instead of PSAPI.DLL; its build.rs forwards the four psapi calls. It makes the match engine's
                       math the same on every CPU: examples\v4.rs is the check program that prints its results' hash)
  bin\ICMP.DLL         native\icmp (the game's pings of the arcade network return at once instead of timing out)
  bin\FPR_Emu.exe      native\programs\src\bin\fpr_emu.rs (forza-programs, --features programs)
  overlay\forza_panel.dll   the repository's crates\panel, one build for both editions; its fonts are fonts\ at the top
                       of the folder.
  native\common        what they share: a loaded program's import table, the parent process
  source\build\netguard.dll   native\netguard (not in the ZIP, not used by 2017-18: the Linux server's 2010-11 hook
                       loads it)
  source\build\server-hook\winmm.dll   native\hook (forza-hook --features e1011-server; not in the ZIP, not used by
                       2017-18): the Linux server's 2010-11 hook under Wine, with its self-tests msgguard_test.exe,
                       sleepfix_test.exe, spinfix_test.exe (native\programs\src\bin)
  source\build\test_wccfnet.exe, test_netguard.exe, test_icmp.exe, test_panel.exe, test_logowin.exe
                       native\programs\src\bin\test_*.rs: `cargo xtask check` loads each guard from a scratch folder,
                       proves the four calls return 0 without reaching the real IP helper, that FORZA_PANEL makes
                       wccfnet.dll load a DLL once a window shows, and that winmm.dll answers a start of logowin.exe
                       without starting anything

overlay\skin.tex is a picture (the panel's art, 1728x1080, BGRA) rendered from the project's skin page; it has no
source here.

What each does, in short:
  winmm.dll     loaded by the game in place of Windows' winmm.dll (the game's folder is searched first); passes
                every winmm call through, and stands in for the arcade's devices: serial ports to the card reader
                and I/O board stand-ins (named pipes), the RingEdge memory, the network address (127.0.0.1); answers
                control's start of the original logowin.exe (its white notice screen) itself: nothing starts, the
                notice goes to game\logowin_messages.txt
  FPR_Emu.exe   answers the game's card-table polls with the cards in seat1\fpr_table0.txt
  forza_panel.dll the panel around seat 1's picture (drawn with the game's own Direct3D 9 device)
  wccfnet.dll   the guard; loads forza_panel.dll into seat 1 and the projector when the Launcher asks (FORZA_PANEL)
  dmath.dll     replaces the engine's approximate SSE instructions and its atan2 with exact ones, so two seats
                on different CPUs compute the same match (online friendlies no longer drop to the CPU); logs to
                dmath.log beside it
  ICMP.DLL      answers the game's pings at once: no reply, as when the arcade network is not there
