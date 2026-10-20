The C source of every program the kit ships. `cargo xtask build` compiles each with Microsoft's C compiler for 32-bit
x86 (Visual Studio Build Tools; xtask loads "vcvarsall.bat x86"): cl /O2 /MT /W3 /Brepro /D_CRT_SECURE_NO_WARNINGS,
the runtime linked in (/MT) so no Visual C++ runtime is needed, and a VERSIONINFO resource (product "WCCF 2017-18 kit").
The programs land in target\kit\2017-18 at the paths below; `cargo xtask package 2017-18` zips that folder.

  bin\winmm.dll        mxhook\mxhook.c (+ winmm_fwd.h: forwards all 192 winmm functions to winmm_orig.dll)   /LD
  bin\FPR_Emu.exe      ..\fpr_emu\fpr_emu.c                                       link: user32.lib
  bin\logowin.exe      logowin\logowin_standin.c                                  link: /SUBSYSTEM:WINDOWS
  overlay\forza_panel.dll   the repository's crates\panel (Rust, i686-pc-windows-msvc), one build for both kits; its
                       fonts are fonts\ at the kit's top. overlay\wccfpanel.c is the C panel it replaced, no longer
                       built or shipped
  overlay\inject.exe   overlay\inject.c (must be 32-bit: it hands the game 32-bit LoadLibraryA's address)
  overlay\netguard.dll netguard\netguard.c   /LD    (secondary guard: patches the game's IPHLPAPI imports)
  source\build\wccfnet.dll   netguard\wccfnet.c   /LD   (the primary guard, not in the kit's ZIP: setup copies it into the
                       runtime's game folder, where the game programs import it instead of IPHLPAPI.DLL; the four stubs
                       are exported undecorated, six read-only calls are forwarded to iphlpapi, all by linker pragmas)
  source\build\test_wccfnet.exe, test_netguard.exe   netguard\test_*.c: `cargo xtask check` loads each guard from a scratch
                       folder and proves the four calls return 0 without reaching the real IP helper

overlay\skin.tex is a picture (the panel's art, 1728x1080, BGRA) rendered from the project's skin page; it has no
source here.

What each does, in short:
  winmm.dll     loaded by the game in place of Windows' winmm.dll (the game's folder is searched first); passes
                every winmm call through, and stands in for the arcade's devices: serial ports to the card reader
                and I/O board stand-ins (named pipes), the RingEdge memory, the network address (127.0.0.1)
  FPR_Emu.exe   answers the game's card-table polls with the cards in seat1\fpr_table0.txt
  logowin.exe   the original start-up notice window, replaced by one that only records the notice
  forza_panel.dll the panel around seat 1's picture (drawn with the game's own Direct3D 9 device)
  inject.exe    loads forza_panel.dll into seat 1's running game
