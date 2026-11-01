# Versions and test builds

## Version numbers

- The release number is the git tag (`v<number>`, [semver](https://semver.org)); no commit bumps it. `cargo xtask`
  reads it with `git describe` and sets `FORZA_VERSION` for the build: the edition ZIPs (`version.txt`, `kit-<number>`),
  the file names and the three crates that show it to players (`launcher`, `wccf-installer`, `forza-panel`: their
  version information and `env!("FORZA_VERSION")`, from `crates/launcher/version_rc.rs`) carry it. The commit tagged
  `v0.6.1` builds `0.6.1`; a later commit builds `0.6.2-dev+<short sha>` (the next patch, so the Launcher offers the next
  release, never the one the build came from); a clone without tags builds `0.0.0-dev+<short sha>`.
  `[workspace.package] version` in `Cargo.toml` only serves a plain `cargo build`.
- Every other crate has its own `version` in its `Cargo.toml`, embedded in its programs' version information. Bump a
  crate's own version (patch for a fix, minor for a feature) in the PR that changes it, if at all; never bump an unchanged
  crate (`git diff --stat v<last> -- <crate dir> native/common`): an unchanged helper program (FPR_Emu.exe,
  logowin.exe, wccfnet.dll, forza-server.exe) then rebuilds byte-identical, and the hash an antivirus once judged
  stays the same.
  FPR_Emu.exe is 277 KB (2026-10-31): its icon is `assets/logo/forza-wccf-small.ico` (16-48 px), not the 122 KB
  `forza-wccf.ico`. Two local builds, also in separate target folders, give one hash. The bytes follow the toolset: a
  local MSVC 14.44 build and a CI 14.51 build of the same commit differ by about 2.8 KB. Putting 14.44 in front of
  cargo does **not** give the local bytes: a run with `vcvarsall -vcvars_ver=14.44` produced a `FPR_Emu.exe`
  byte-identical to the 14.51 one (`FlsAlloc` CRT imports and all, 2026-11-01, `E:\wccf-builds\av-bisect-20261101\`),
  because rustc takes the CRT's lib directories from its own detection of the newest MSVC, not from PATH. A hash
  recorded here therefore still differs between a build PC and the runner: record the CI build's SHA-256 in
  `editions/2017-18/fpr_emu.sha256` and have `cargo xtask package` compare it, failing when the exe changed and
  `native/programs` and `native/common` did not.
- 0.x means alpha: features missing, bugs expected. The count restarted from 7.0.0 at **0.2.0**, because the installer
  already published `v0.1.0` and `v0.1.1`.
- Each test build with new features or a batch of fixes raises the minor number (0.2.0, 0.3.0, …). A rebuild of a
  published build that only fixes it raises the patch number (0.2.1). 1.0.0 is Release 1 (both editions, no Python in
  the ZIPs, the online lobby).
- `cargo xtask package 2017-18` on the commit tagged `v0.2.0` writes `Forza-WCCF-2017-18-0.2.0.zip` with `version.txt` =
  `kit-0.2.0`. `--name` changes only the file name.
- A change of the `forza` or `forza-ui` version also changes its entry in `crates/installer/THIRD-PARTY-LICENSES.json`;
  `cargo xtask check` fails until they match.

## Alpha builds for players

- 2017-18 only, as a GitHub **pre-release** of this repository with the ZIP and the installer.
- Before publishing: the [stability gate](#stability-gate) passes on the tagged commit, someone has played the build,
  and a maintainer approves the tag and the upload.
- Release notes in English, Italian and Japanese, only in the GitHub release body (no CHANGES.txt in the ZIP), under the
  headings `## English`, `## Italiano` and `## 日本語`: this is an alpha, what changed, known bugs, and how to report one
  (SAVE LOGS in the panel, then attach the file to a GitHub issue). The Launcher's UPDATE tab shows the section in the
  player's language (English when there is none), up to the `SHA-256 sums` line, as plain text with VIEW ON GITHUB.
- Player feedback: GitHub issues, in English, Italian or Japanese.

## Stability gate

Before a ZIP goes to a player: the full `cargo xtask check` on the commit that is packaged, then a soak run through
the guard ([SAFETY.md](SAFETY.md)): at least three matches back to back, RESTART NOW once, a card in and out, online
and offline, and a quit, with no crash snapshot, no hang, nothing left running and the routes unchanged. The logs are
read for warnings, not only for the pass lines. Crash snapshots and problem reports stay on, so a crash in a player's
hands is seen.

## Antivirus verdicts

Microsoft's engine, the one players' Defender runs, decides a release: the VirusTotal job in `release.yml` fails on a
Microsoft hit and no other engine stops it (`av-bisect-20261101/RESULTS.md`). The verdict is not stable, but it is
mostly one-sided: 0.6.0's `FPR_Emu.exe` (sha256 `2cd8bf99…`) has been flagged on every scan but one on 2026-11-01 —
four on VirusTotal including two byte-changed copies, and five of six on a Windows PC, with one clean reading in
between (`av-oracle-20261101/`). Treat a build of it as flagged until a scan of *that file* says otherwise, and never
as clean because an earlier scan did. What helps:

- a Microsoft hit gets one fresh analysis before it stops the release, as the workflow does now: re-scan rather than
  rebuild, and never change bytes just to change a hash (two byte-patched copies of a flagged file stayed flagged);
- a program whose sources did not change keeps its bytes; do not rebuild it for a release, and do not change a
  program's bytes hoping the verdict clears: it is one engine's opinion of those bytes, measured to stay put;
- scanning on a Windows PC before tagging is advisory only, not a gate: the verdict flips both ways. If a scan is
  wanted, copy the file first and scan the copy: real-time protection deletes the sample about 30 s after the scan,
  and `MpCmdRun` exits 2 both for a detection and for a missing file, so read `Scanning <path> found N threats` and
  the `Threat` lines.

Obfuscation and packing raise the verdict instead of lowering it (Microsoft says so outright), and signing without a
paid certificate leaves `!ml` verdicts untouched: `E:\wccf-builds\codesign-20261101\OPTIONS.md`.

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
- The release list brings each release's notes. The notes of the installed version and of the newest release are kept
  once per version in `data\release-notes\<version>.md`; UPDATE shows the kept copy when GitHub does not answer.
- The Launcher checks at most once a day at start (`data\update-check.txt`) and every time the UPDATE page opens.
- A newer release that is not a test build shows a notice: a line at the top of the main page and the tag on the UPDATE
  entry. It stays (`data\update-available.txt`, kept across restarts while the release is still newer) until the
  player opens UPDATE or updates. Nothing installs by itself.
