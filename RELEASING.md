# Release checklist

Repository: `Forza-WCCF/WCCF-Installer` (private).
Forza-WCCF `0.1.1` is an experimental installer prerelease, not a playable-game kit.

## Quick checks, reuse evidence

- Use the existing validated EXE when source, Cargo.lock and toolchain are unchanged.
  Documentation/packaging changes need no rebuild or media-scale check.
- Run `scripts/smoke.ps1` for help/version, invalid arguments, synthetic reuse
  and existing-folder preservation. It never extracts media or launches the game.
- Run `scripts/package.ps1` after changing release packaging or documentation.
  It checks notice completeness and archive contents, and creates checksums for
  the small EXE/ZIP only.
- Reuse recorded decryption/extraction evidence. Do not repeatedly extract, sweep
  disc CRCs or hash game trees. Repeat a media check only on explicit request or
  a concrete regression; record the reason and changed behavior first.
- Keep source commit, binary provenance, actual validation and known limits in
  release notes. Unperformed checks must not be marked passed.
- No media, keys, game binaries, runtime kit, saves, personal logs or credentials
  belong in source control, hosted CI or release assets.

CI builds fresh code (Linux cross-build, Windows smoke check). Manual Windows GUI interaction is
not yet validated. The owner explicitly approved publishing the experimental
prerelease with quick functional checks and prior extraction evidence. No completed
player-match, save/reload or ready-to-play claim is made.

## Package a validated local EXE

```powershell
# Build only if executable source/dependencies changed:
cargo build --locked --release --target x86_64-pc-windows-msvc
# For an existing build elsewhere, pass -Exe PATH to both scripts:
powershell -NoProfile -File scripts/smoke.ps1
powershell -NoProfile -File scripts/package.ps1
```

`dist/` contains the EXE, ZIP, SHA256SUMS.txt, README, changelog, MIT license and
Windows third-party notices. Only that allowlisted payload is distributable.
Do not distribute a lone EXE without the accompanying license notices.

## Tag and publish with owner approval

```sh
git tag -a v0.1.1 -m "Forza-WCCF 0.1.1 prerelease"
git push origin main v0.1.1
gh release create v0.1.1 dist/* --verify-tag --prerelease \
  --title "Forza-WCCF v0.1.1" --notes-file RELEASE_NOTES.md
```

Pushing a tag does not trigger another build. The direct EXE download is
`releases/download/v0.1.1/wccf-installer.exe`; repository access is required.
Keep experimental versions marked prerelease. Publication does not make the
private repository public. Do not modify assets of a published release.

## Optional hosted draft build

```sh
gh workflow run release.yml --ref v0.1.1
```

Run this only when a fresh hosted build is wanted **before** publication. It
reuses CI, checks tag/package agreement, verifies assets and creates a draft
prerelease. It refuses to overwrite a published release. Publish a draft only
with owner approval; no long-lived token or signing secret is needed.

Binaries are unsigned. Add Authenticode only when an owner supplies and approves
a signing identity. Do not tell users to disable antivirus or Windows protection.
