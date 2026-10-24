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
- A version change also changes the `forza` and `forza-ui` entries in `crates/installer/THIRD-PARTY-LICENSES.json`;
  `cargo xtask check` fails until they match.

## Alpha builds for players

- 2017-18 only, as a GitHub **pre-release** of this repository with the ZIP and the installer.
- Before publishing: the full `cargo xtask check` passes on the tagged commit, someone has played the build, and a
  maintainer approves the tag and the upload.
- Release notes in English, Italian and Japanese: this is an alpha, what changed, known bugs, and how to report one
  (SAVE LOGS in the panel, then attach the file to a GitHub issue).
- Player feedback: GitHub issues, in English, Italian or Japanese.

## In-app UPDATE

The Launcher's UPDATE (both editions) reads this repository's releases from the GitHub API
(`UPDATE_URL`, `crates/launcher/src/release.rs`; `WCCF_UPDATE_URL` points it at a self-hosted list).

- A release is offered to an edition only with an asset named `Forza-WCCF-<edition>-<version>.zip`, `<version>` being
  the tag without its `v` (`v0.4.0` -> `Forza-WCCF-2017-18-0.4.0.zip`). The installer's ZIP is never taken.
- Versions compare as semver against `version.txt` (`kit-0.3.0`). A tag with a pre-release part (`v0.4.0-rc.1`) is a
  test build: listed, never announced. GitHub's pre-release flag does not count, so alpha releases are offered.
- The ZIP must match its `<zip>.sha256` asset (`cargo xtask package` writes it), else GitHub's `digest` of the asset;
  with neither, or a mismatch, UPDATE changes nothing.
- A release whose game files need the installer again: put `<!-- needs-installer -->` in the release notes. UPDATE then
  copies nothing and opens the release page.
- The Launcher checks at most once a day at start (`data\update-check.txt`) and every time the UPDATE page opens.
- A newer release that is not a test build shows a notice: a line at the top of the main page and the tag on the UPDATE
  entry. It stays (`data\update-available.txt`, kept across restarts while the release is still newer) until the
  player opens UPDATE or updates. Nothing installs by itself.
