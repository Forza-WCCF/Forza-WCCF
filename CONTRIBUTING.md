# Contributing

Issues and pull requests are welcome, in English, Italian or Japanese. For a bug, attach the file from SAVE LOGS (in
the panel) and say which edition and which version you play.

## Build

The build host is Windows 10 or 11. `cargo xtask` builds the Rust with the MSVC toolchain and the 32-bit C programs with the
clang-cl that ships with Visual Studio, and runs the Windows test programs natively. `forza-server` also builds on plain Linux
(`cargo build --release -p forza-server`).

1. Install [Visual Studio Build Tools 2022 or later](https://visualstudio.microsoft.com/downloads/) with the "Desktop
   development with C++" workload: the MSVC x86/x64 build tools and a Windows SDK (10.0.22000 or newer; CI uses
   10.0.26100). Add the components "C++ Clang Compiler for Windows" and "MSBuild support for LLVM (clang-cl)
   toolset". `cargo xtask` finds `clang-cl`, `lld-link`, `llvm-rc` and the `INCLUDE`/`LIB` environment itself; run it
   from any shell, not only a Developer Prompt.
2. Install [Git for Windows](https://gitforwindows.org) and [rustup](https://rustup.rs). `rust-toolchain.toml` pins the
   version and the two Windows targets. Then set:

   ```sh
   git config --global core.autocrlf false
   git config --global core.longpaths true
   ```

3. Keep the repository on a local NTFS drive, near the drive root (`E:\wccf-mono`). Windows 11: put it on a
   [Dev Drive](https://learn.microsoft.com/windows/dev-drive/). Windows 10 has no Dev Drive; add Defender exclusions for
   the repository, `%USERPROFILE%\.cargo` and `%USERPROFILE%\.rustup` instead (admin PowerShell):

   ```powershell
   Add-MpPreference -ExclusionPath E:\wccf-mono, $env:USERPROFILE\.cargo, $env:USERPROFILE\.rustup
   ```

   The exclusions switch off Defender scanning for those folders, so keep the list to these three. Without them or a
   Dev Drive, builds run noticeably slower.
4. Install the commit-message hook once per clone:

   ```sh
   git config core.hooksPath xtask
   ```

5. Build and check:

   ```sh
   cargo xtask build          # the programs, staged in target/kit/<edition>/
   cargo xtask check          # fmt, clippy, tests, the C self-tests, the run tests, the publisher-name grep
   cargo xtask check quick    # only the crates changed since the base branch
   cargo xtask check ci       # what CI runs: check without the GPU snapshot tests
   cargo xtask package 2017-18 --name Forza-WCCF-2017-18-0.2.0
   ```

   One `cargo xtask check` at a time per PC: the tests use fixed local ports.

WSL is optional and only for the reverse-engineering tools and the Linux server deploy (`deploy/`). Do not start the
build or the games from a WSL shell ([docs/SAFETY.md](docs/SAFETY.md)).

The design and the plan: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/ROADMAP.md](docs/ROADMAP.md). Versions and
releases: [docs/RELEASING.md](docs/RELEASING.md).

## Safety

Read [docs/SAFETY.md](docs/SAFETY.md) before you run a game. Start the original programs only through the Launcher and
its guard, never from a WSL shell, and never change your PC's network settings to make a game work.

## What never goes into the repository

Original game files and anything derived from them: game binaries, patched executables, disc images, IDA or Ghidra
databases, decompiled code, extracted card pictures and catalogues. Also logs, saves, club cards and screenshots of a
game. Patches keep the original bytes and check them before they write.

In code, docs and commit messages, say "the publisher" for the game's company and "original" for its files and text.
`cargo xtask check` fails on the name.

## Changes

- One commit per finished piece, and each commit builds.
- A fix comes with the one small check that would have caught it, when that is cheap.
- A common, maintained crate before hand-written code for anything beyond a few lines.
- Stability first: a crash, a hang or a run that leaves processes behind outranks a new feature.

## Languages

English, Italian and Japanese are first-class, in that order. New or changed user-facing text ships in all three, with
English as the source and the fallback. UI strings live in `crates/forza/locales/ui.yml`, one entry per key with `en`,
`it` and `ja`. If you can write only one of the languages, say so in the pull request and someone will help with the
others.

## Be kind

Everyone here works on this in their free time. Be patient and assume good intent. The
[Code of Conduct](CODE_OF_CONDUCT.md) applies to issues, pull requests and every other project space.
