# WCCF RingEdge Installer v0.1.0

**Download `wccf-installer.exe` directly** from the release assets (Windows x64,
11,473,920 bytes). The optional ZIP contains the same EXE and all documentation.
`SHA256SUMS.txt` lists the EXE and ZIP checksums. Keep `LICENSE` and
`THIRD-PARTY-LICENSES.json` with the EXE.

The repository remains **private**. GitHub sign-in and repository access are
required to download. No game media, keys, game binaries, runtime kit or saves
are included; supply your own supported disc image and matching keyfile.

## Supported media

- **2010–2011 Rev D / SBWG:** decryption/extraction; optional trusted, separately
  supplied 2010–2011 kit setup.
- **2017–2018 v2.00.00 / SDDY:** decryption/extraction only.

Wizard or headless mode; existing game folders are never replaced.
`--help` and `--version` are supported. Read the accompanying README.

## Build and checks

The EXE is the previously validated native Windows x64 MSVC/Rust 1.99.0 release
build from executable-source commit
`25c0f6f2fd3de91d6b975f758d1c8cadab608729`. The release tag also includes
packaging, documentation and workflow updates; executable source, Cargo.lock
and toolchain are unchanged, so the EXE was not rebuilt.

Quick native headless functional and packaging checks passed for the distributable
EXE. They cover help/version, malformed arguments, missing destination, synthetic
SDDY reuse, English rejection, existing-folder preservation, asset allowlist
and version-tag rejection. No game was launched.

Prior format/Clippy/unit checks and both first-MiB disc checks passed. The prior
full SDDY extraction matched 6,816 independently recovered files with zero
mismatches. That evidence is reused; no media extraction, disc CRC sweep or
game-tree hashing was repeated to publish this release. Checksums were generated
only for the small files being distributed.

The earlier hosted CI run was cancelled on user pause after format, lint, tests
and dependency notices passed; its hosted release build/package did not complete.
This release uses the successfully validated local Windows build.

## Known limits

This is an **experimental prerelease**, published on explicit owner instruction
using quick functional checks and recorded evidence. Manual Windows GUI interaction
has not been validated. Binaries are unsigned. Completed gameplay and save/reload
are not claimed.

Extraction is not a ready-to-play runtime. Do not run original unguarded game
executables: use only a compatible guarded runtime. No host interface, route
or firewall changes are needed or made by the installer.
