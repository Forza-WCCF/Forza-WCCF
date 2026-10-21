# Contributing

Issues and pull requests are welcome, in English, Italian or Japanese. For a bug, attach the file from SAVE LOGS (in
the panel) and say which edition and which version you play.

## Build

The build host is WSL 2 on Windows 10 or 11. Rust and the 32-bit C programs are cross-compiled for Windows, and
`cargo xtask check` runs the Windows test programs through WSL interop. `forza-server` also builds on plain Linux
(`cargo build --release -p forza-server`).

1. Install Rust with [rustup](https://rustup.rs). `rust-toolchain.toml` pins the version and the two Windows targets.
2. Install the C toolchain and cargo-xwin:

   ```sh
   sudo apt install clang lld llvm
   cargo install cargo-xwin
   ```

   `cargo xtask` runs `cargo xwin` for the Rust and `clang-cl` + `lld-link` + `llvm-rc` for the C. On the first build,
   xwin downloads the Microsoft CRT and Windows SDK into its cache. xtask sets `XWIN_ACCEPT_LICENSE=1`, so running it
   means you accept Microsoft's license terms for those files.
3. Build and check:

   ```sh
   cargo xtask build          # the programs, staged in target/kit/<edition>/
   cargo xtask check          # fmt, clippy, tests, the C self-tests, the publisher-name grep
   cargo xtask check quick    # only the crates changed since the base branch
   cargo xtask package 2017-18 --name Forza-WCCF-2017-18-0.2.0
   ```

   Outside `cargo xtask`, run raw `cargo xwin` with `XWIN_ACCEPT_LICENSE=1 XWIN_ARCH=x86,x86_64`, as xtask does.

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
