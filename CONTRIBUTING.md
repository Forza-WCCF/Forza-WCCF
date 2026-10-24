# Contributing

Issues and pull requests are welcome, in English, Italian or Japanese. For a bug, attach the file from SAVE LOGS (in
the panel) and say which edition and which version you play.

## Build

The build host is Windows 10 or 11. `cargo xtask` builds every program with Rust's MSVC toolchain (the 32-bit ones in and
beside the game, `native/`, for `i686-pc-windows-msvc`) and runs the Windows test programs natively. `forza-server` also
builds on plain Linux (`cargo build --release -p forza-server`).

1. Install [Visual Studio Build Tools 2022 or later](https://visualstudio.microsoft.com/downloads/) with the "Desktop
   development with C++" workload: the MSVC x86/x64 build tools and a Windows SDK (10.0.22000 or newer; CI uses
   10.0.26100): Rust links against its C runtime and the SDK, and the SDK's `rc.exe` gives each program its version
   information. Run `cargo xtask` from any shell, not only a Developer Prompt.
2. Install [Git for Windows](https://gitforwindows.org) and [rustup](https://rustup.rs). `rust-toolchain.toml` pins the
   version and the two Windows targets. Then set:

   ```sh
   git config --global core.autocrlf false
   git config --global core.longpaths true
   ```

3. Keep the repository on a local NTFS drive, near the drive root (`E:\forza-wccf`). Windows 11: put it on a
   [Dev Drive](https://learn.microsoft.com/windows/dev-drive/). Windows 10 has no Dev Drive; add Defender exclusions for
   the repository, `%USERPROFILE%\.cargo` and `%USERPROFILE%\.rustup` instead (admin PowerShell):

   ```powershell
   Add-MpPreference -ExclusionPath E:\forza-wccf, $env:USERPROFILE\.cargo, $env:USERPROFILE\.rustup
   ```

   A worktree beside the repository (`E:\forza-wccf-<branch>`) is another folder: add it too, or keep the worktrees in
   one excluded folder. The exclusions switch off Defender scanning for those folders, so keep the list short. Without
   them or a Dev Drive, builds run noticeably slower.
4. Install the commit-message hook once per clone:

   ```sh
   git config core.hooksPath xtask
   ```

5. Build and check:

   ```sh
   cargo xtask build          # the programs, staged in target/kit/<edition>/
   cargo xtask check          # fmt, clippy, tests, the guard and hook test programs, the run tests, the publisher-name grep
   cargo xtask check quick    # only the crates changed since the base branch
   cargo xtask check ci       # what CI runs: check without the GPU snapshot tests, ci profile (--release: release)
   cargo xtask package 2017-18 --name Forza-WCCF-2017-18-0.2.0
   ```

   One `cargo xtask check` at a time per PC: the tests use fixed local ports.

6. Faster builds (optional):

   - `cargo xtask build --fast` and `cargo xtask check --fast` use the `fast` profile: release code without LTO,
     incremental, and the tests share its dependencies. For local test kits only. CI's `check ci` builds the `ci`
     profile (the same without incremental, so its cache holds no incremental data); the weekly run and
     `cargo xtask package` build `release`.
   - [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`): `check` runs the tests with it when
     it is installed, one process per test on all cores. Arguments after `--` go to it, e.g.
     `cargo xtask check ci -- --partition count:1/2`.
   - A new worktree builds every crate from zero. Share the dependencies between worktrees with
     [sccache](https://github.com/mozilla/sccache) (`cargo install sccache --locked`, then
     `setx RUSTC_WRAPPER sccache`; `check` then builds non-incremental, so sccache can serve it), or with one output
     folder (`setx CARGO_TARGET_DIR E:\cargo-target`): the kits stay in each worktree's `target\`, but two builds
     in two worktrees then wait for each other. Add that folder to the Defender exclusions too.

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

## Commit messages

Commit messages and pull request titles follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/),
with a scope on every commit. The commit-message hook and CI check them. Pull requests land as merge commits, which keep
every commit and take the pull request title as their first line, so the title follows the same rules.

```text
type(scope)!: description

Optional body: what changed and why, wrapped at 72 columns.

Optional footers, for example:
BREAKING CHANGE: what a user or another crate has to change
Refs: #12
```

- **Header:** `type(scope): description`, one line. The type and the scope are lowercase; one space follows the
  colon. The description is short, in the imperative ("add", "fix", not "added"), lowercase after the colon except
  names, with no period at the end.
- **Types:** `feat` (a new feature for players or contributors), `fix` (a bug fix), `docs`, `refactor` (no change in
  behaviour), `perf`, `test`, `build` (the build, xtask, dependencies), `ci`, `style` (formatting only), `chore`
  (anything else, such as release housekeeping) and `revert`.
- **Scope:** required: the part of the project the commit changes. Use the crate or component name: `launcher`,
  `installer`, `forza`, `ui` (`crates/forza-ui`), `setup` (`crates/forza/src/setup`), `panel`, `server`, `vslink`,
  `linux-host`, `wccfnet` (`native/wccfnet`), `mxhook` (the 2010-11 server hook, `native/2010-11/server-hook`), `hook` (`native/hook`),
  `native` (`native/common`, `native/programs`, `native/icmp`, `native/netguard`), `fpr-emu`, `xtask`,
  `deploy`, `deps`, `release`; for documents the file or area: `readme`, `contributing`, `roadmap`, `safety`, `docs`.
  Add the edition when it matters: `fix(setup): ...` with "2017-18" in the description, rather than a new scope.
- **Body:** after one blank line. Say why, and anything a reviewer cannot see in the diff.
- **Footers:** after one blank line, `Token: value` or `Token #value`, with `-` for spaces in the token
  (`Reviewed-by`, `Refs`). `BREAKING CHANGE: ...` (or `BREAKING-CHANGE`) is always in capitals.
- **Breaking change:** a `!` before the colon and/or a `BREAKING CHANGE:` footer.
- **Versions** ([docs/RELEASING.md](docs/RELEASING.md)): `fix` is a patch, `feat` a minor version, a breaking change a
  major version (a minor one while the version is 0.x).
- **Revert:** `revert(scope): description of the reverted commit`, with `Refs:` naming the commit hash.
- Git's own `Merge ...` lines pass; `fixup!`, `squash!` and `amend!` commits are fine locally but must be squashed
  (`git rebase -i --autosquash`) before a pull request.

Examples:

```text
fix(launcher): give the keyboard back to the game after the match engine closes
feat(wccfnet): log the game's send and recv when WCCF_TRACE_NET=1
docs(contributing): adopt Conventional Commits with a required scope
feat(server)!: answer the 2017-18 lobby in protocol v2

BREAKING CHANGE: Launchers before 0.3.0 cannot read the lobby list.
```

## Languages

English, Italian and Japanese are first-class, in that order. New or changed user-facing text ships in all three, with
English as the source and the fallback. UI strings live in `crates/forza-ui/locales/ui.yml`, one entry per key with `en`,
`it` and `ja`. If you can write only one of the languages, say so in the pull request and someone will help with the
others.

## Be kind

Everyone here works on this in their free time. Be patient and assume good intent. The
[Code of Conduct](CODE_OF_CONDUCT.md) applies to issues, pull requests and every other project space.
