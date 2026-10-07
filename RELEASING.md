# Release checklist

Repository: `Forza-WCCF/WCCF-Installer` (private).
Release `0.1.0` is an experimental installer prerelease, not a playable-game kit.

## Before tagging

- [ ] Windows CI is green for the exact commit.
- [ ] The wizard's browse/drag-and-drop, extraction-only, reuse and cancellation
      flows are manually checked on Windows. A cancelled run leaves only the
      staging folder and never removes an existing game folder.
- [ ] Read-only first-MiB checks pass for each supported image/key pair locally.
      Do not upload media, keys or real-game logs to GitHub Actions.
- [ ] Extraction behavior changes get a full local byte-for-byte extraction check.
      Do not launch the extracted game to validate the installer.
- [ ] README/changelog describe actual evidence and known runtime limits.
- [ ] MIT license and Windows third-party license/font notices are reviewed and
      complete. Run `scripts/check-licenses.ps1` after any Cargo.lock update;
      the checked-in bundle records the upstream notice sources.
- [ ] Git tracks source/docs/workflows only: no ISO, keys, game EXE/DLL, kit,
      saves, personal logs, dumps or credentials. Never upload the whole workspace.
- [ ] Package version, changelog heading and intended `v<version>` tag agree.

## Make the draft

```sh
git switch main
git pull --ff-only
# Update Cargo.toml version and CHANGELOG.md for later releases; keep Cargo.lock in sync.
git tag -a v0.1.0 -m "WCCF RingEdge installer 0.1.0 prerelease"
git push origin v0.1.0
```

The tag workflow builds/tests the exact tag with read-only permissions, packages
`wccf-installer-v0.1.0-windows-x64.zip` and `SHA256SUMS.txt`, then uses a
separate write-enabled job to create a **draft prerelease**. It includes the
source commit and toolchain in its notes. Failed checks create no release.
A rerun can refresh the assets only while the release is still a draft; it will
not overwrite a published release. No long-lived token or signing secret is needed.

## Approve, do not auto-publish

Download the draft ZIP, compare its checksum, verify its contents and perform the
manual wizard check on that exact binary. Record any remaining limits in the draft
notes, then publish only when approved by the owner. Keep experimental versions
marked prerelease. Creating a draft does not make this private repository public.

Unsigned binaries may trigger Windows reputation warnings. Do not tell users to
disable protection. Add Authenticode signing only when an owner supplies and
approves a signing identity; it is intentionally not configured here.

## Local packaging

```powershell
cargo build --locked --release --target x86_64-pc-windows-msvc
powershell -NoProfile -File scripts/smoke.ps1
powershell -NoProfile -File scripts/check-licenses.ps1
powershell -NoProfile -File scripts/package.ps1
```

Only the script's allowlisted payload goes in the ZIP. Do not distribute a lone
EXE without its dependency license notices. Local evidence belongs under ignored
`target/`, not in the repository or release archive.
