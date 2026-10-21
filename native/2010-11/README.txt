The source of every program Forza-WCCF 2010-11 ships, and how each is built.  The ZIP carries the built programs; git
holds only their source (2026-10-08).  It is built from the Forza-WCCF repository with cargo xtask (a Windows
PC with Rust; this ZIP carries no Cargo files):

    cargo xtask build              the C programs (32-bit, MSVC) and the launchers (Rust), staged under target\ (the 2010-11 folder)
    cargo xtask check              the checks that need no game (key driver, panel) and the Rust checks
    cargo xtask check quick        fmt, clippy and tests of the crates changed since --base (needs one `cargo xtask build` first)
    cargo xtask package 2010-11    the ZIP in dist\, as players get it

Needs Visual Studio 2022 (or its Build Tools) with "Desktop development with C++", and rustup.  The
commands each program is built with, for the record (xtask adds /Brepro, so the same
source gives the same file, and keeps the compiler's leftovers in source\build\):

The C source of every program Forza-WCCF ships. Each was built with Microsoft's C compiler for 32-bit x86
(Visual Studio Build Tools, "vcvarsall.bat x86"), the runtime linked in (/MT), so no Visual C++ runtime is needed.

  programs\wccfk.dll   hook\wccfk.c (+ winmm_fwd.h: forwards the game's 3 timer functions to Windows' winmm)
                       cl /nologo /LD /O2 /MT /W3 /D_CRT_SECURE_NO_WARNINGS wccfk.c /Fe:wccfk.dll
  programs\FPR_Emu.exe card_table\fpr_emu.c
                       cl /nologo /O2 /MT /W3 /D_CRT_SECURE_NO_WARNINGS fpr_emu.c /Fe:FPR_Emu.exe /link user32.lib
  programs\forza_panel.dll   the repository's crates\panel (Rust, i686-pc-windows-msvc; built by cargo xtask build;
                       its fonts are fonts\ at the top of the folder).
  programs\inject.exe  panel\inject.c (must be 32-bit: it hands the game 32-bit LoadLibraryA's address)
                       cl /nologo /O2 /MT /W3 /D_CRT_SECURE_NO_WARNINGS inject.c /Fe:inject.exe
  panel\fakegame.c     not shipped: a stand-in for seat 1's game window to see the panel without the game (its
                       header: the steps, and the settings that keep a test off the live game)
                       cl /nologo /O2 /MT /W3 /D_CRT_SECURE_NO_WARNINGS fakegame.c /Fe:fakegame.exe /link d3d9.lib user32.lib

Launcher.exe              launcher\ (Rust; the repository's Cargo.toml and Cargo.lock pin every crate; this ZIP carries only
                       the launcher's sources, so build from a checkout of the repository with cargo xtask build)
                       cargo build --release --locked -p launcher --target x86_64-pc-windows-msvc
                       then copy target\x86_64-pc-windows-msvc\release\Launcher.exe to the Forza-WCCF folder.
                       The C runtime is linked in (.cargo\config.toml); checks: cargo fmt --all --check,
                       cargo clippy --workspace --all-targets -- -D warnings, cargo test --workspace

art\skin.tex is a picture (the panel's art, 1728x1080, BGRA) rendered from the project's skin page; it has no
source here.

What each does, in short:
  wccfk.dll     loaded by the game under its own name (setup renames the game's WINMM.dll import to it);
                passes the game's three timer calls on to Windows' winmm, and stands in for the arcade's devices: serial ports to the card reader
                and I/O board stand-ins (named pipes), the RingEdge memory, the network address (127.0.0.1)
  FPR_Emu.exe   answers the game's card-table polls with the cards in seat1\fpr_table0.txt
  forza_panel.dll the panel around seat 1's picture (drawn with the game's own Direct3D 9 device)
  inject.exe    loads forza_panel.dll into seat 1's running game (and the projector's, for its money)
  Launcher.exe  the one program: setup, the game's text, the club card's steps and the run, which stops
                everything it started once a game window is closed (launcher\src)
