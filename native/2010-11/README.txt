The source of every program Forza-WCCF 2010-11 ships, and how each is built.  The ZIP carries the built programs; git
holds only their source.  Every program is Rust, built from the Forza-WCCF repository with cargo xtask (a Windows PC
with Rust; this ZIP carries only the programs' crates, not the repository's Cargo.toml and Cargo.lock, which pin every
crate):

    cargo xtask build              every program, staged under target\ (the 2010-11 folder)
    cargo xtask check              the checks that need no game (key driver, panel, hook, guards) and the Rust checks
    cargo xtask check quick        fmt, clippy and tests of the crates changed since --base (needs one `cargo xtask build` first)
    cargo xtask package 2010-11    the ZIP in dist\, as players get it

Needs Visual Studio 2022 (or its Build Tools) with "Desktop development with C++" (the linker's C runtime and the
Windows SDK), and rustup.  The C runtime is linked in (.cargo\config.toml), so no Visual C++ runtime is needed; each
program carries its version information.  The game is 32-bit, and so are the programs that go into it or beside it:
they are built for i686-pc-windows-msvc.

  programs\wccfk.dll       native\hook (cargo build -p forza-hook --features e1011 --target i686-pc-windows-msvc);
                           its build.rs forwards the game's 3 timer functions to Windows' winmm
  programs\FPR_Emu.exe     native\programs\src\bin\fpr_emu.rs (forza-programs, --features programs)
  programs\inject.exe      native\programs\src\bin\inject.rs (must be 32-bit: it hands the game 32-bit LoadLibraryW's address)
  programs\forza_panel.dll the repository's crates\panel (one build for both editions; its fonts are fonts\ at the top
                           of the folder)
  native\common            what they share: a loaded program's import table, the parent process
  native\programs          also the test programs, not shipped: fakegame.rs (a stand-in for seat 1's game window, to see
                           the panel without the game; its header: the steps, and the settings that keep a test off the
                           live game), crashme.rs, test_dhcphook.rs

Launcher.exe               launcher\ (cargo build --release --locked -p launcher --target x86_64-pc-windows-msvc,
                           then copy target\x86_64-pc-windows-msvc\release\Launcher.exe to the Forza-WCCF folder).
                           Checks: cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings,
                           cargo test --workspace

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
