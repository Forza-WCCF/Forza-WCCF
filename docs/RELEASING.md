# Versions and test builds

## Version numbers

- One [semver](https://semver.org) version for the whole repo: `[workspace.package] version` in `Cargo.toml`. The
  Launcher, server, installer, panel and the edition ZIPs all carry it.
- 0.x means alpha: features missing, bugs expected. The count restarted from 7.0.0 at **0.2.0**, because the installer
  already published `v0.1.0` and `v0.1.1`.
- Each test build with new features or a batch of fixes raises the minor number (0.2.0, 0.3.0, …). A rebuild of a
  published build that only fixes it raises the patch number (0.2.1). 1.0.0 is Release 1 (both editions, no Python in
  the ZIPs, the online lobby).
- Package with the version in the name: `cargo xtask package 2017-18 --name Forza-WCCF-2017-18-0.2.0` writes
  `version.txt` = `kit-0.2.0`. Tag the commit `v0.2.0`.
- A version change also changes the `forza` entry in `crates/installer/THIRD-PARTY-LICENSES.json`; `cargo xtask check`
  fails until it matches.

## Alpha builds for players

- 2017-18 only, as a GitHub **pre-release** of this repository with the ZIP and the installer.
- Before publishing: the full `cargo xtask check` passes on the tagged commit, someone has played the build, and a
  maintainer approves the tag and the upload.
- Release notes in English, Italian and Japanese: this is an alpha, what changed, known bugs, and how to report one
  (SAVE LOGS in the panel, then attach the file to a GitHub issue).
- Player feedback: GitHub issues, in English, Italian or Japanese.

## Before in-app UPDATE is switched on

UPDATE is off (`UPDATE_URL` ends in `.invalid/`, `crates/launcher/src/release.rs`). Its version check compares
`kit-` numbers across every release of one repo and treats any `-suffix` as a test build. 2010-11 releases are at
`kit-8.x`, so a 0.x build would never count as newer. Before UPDATE is turned on, the check needs one release list per
edition and the `v` tags.
