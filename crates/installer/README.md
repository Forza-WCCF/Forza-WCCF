# Forza-WCCF

[![CI](https://github.com/Forza-WCCF/WCCF-Installer/actions/workflows/ci.yml/badge.svg)](https://github.com/Forza-WCCF/WCCF-Installer/actions/workflows/ci.yml)

The Forza-WCCF Windows x64 installer decrypts and extracts **your own** supported World Club
Champion Football RingEdge disc image. Rust + egui; no Python is needed for extraction.

**No game media, keys, game binaries, saves or runtime kit are distributed here.**
Extraction prepares the original files; it does not make them playable.
This is an experimental installer, not a verified playable-game release.

## Supported images

| Image | Game ID | Exact ISO size | Installer support |
|---|---|---:|---|
| 2010–2011 (Japan), Rev D / DVR-5004D | SBWG | 5,034,770,432 bytes | Extraction; optional 2010–2011 kit setup |
| 2017–2018 v2.00.00 / DVR-5059A | SDDY | 5,921,865,728 bytes | Extraction only |

The installer checks the known layout, metadata CRC and encrypted volume header.
Full game-slot block verification is enabled by default. Each image needs its
matching game keyfile: raw bytes or hex text. The SBTR system key does not unlock
the game slot. The system/OS-update slot is never extracted.

## Install

1. Download **[wccf-installer.exe](https://github.com/Forza-WCCF/WCCF-Installer/releases/download/v0.1.1/wccf-installer.exe)**
   from the [release](https://github.com/Forza-WCCF/WCCF-Installer/releases/tag/v0.1.1).
   The repository is private: GitHub sign-in and repository access are required.
   Keep the accompanying `LICENSE` and `THIRD-PARTY-LICENSES.json` with the EXE.
   Alternatively, download the ZIP containing the EXE and all documentation.
2. Compare the EXE (or ZIP) with the matching line in `SHA256SUMS.txt`; unpack only
   if you chose the ZIP.
3. Open `wccf-installer.exe`, choose your own ISO and matching keyfile, then an
   empty install folder. Allow about 6 GB free space and permission to write there.
4. Leave the kit blank for extraction only. New installs use separate trees:
   `<install folder>\2010-2011\game` and `<install folder>\2017-2018\game`.
   The default base folder is `C:\Games\Forza-WCCF`.

Existing game folders are **never replaced**. Reuse them in the wizard, or choose
another destination. Cancelled/failed extraction can leave a
`game.partial` staging folder inside its edition root; a retry replaces that
staging folder, not an existing game folder. Earlier installations are detected
and reused in place; no existing files or saves are moved. Back up saves before
using any external kit.

### Both editions can coexist

Each edition has its own `game`, kit/Python/scripts, `data`, settings, logs and
saves. Installing one never updates the other. Desktop shortcuts include the
edition: `Forza-WCCF 2010-2011 - PLAY/STOP`. The wizard lets you choose which
edition to reuse; use `--game SBWG` or `--game SDDY` in headless mode when both
are installed. New extraction chooses the edition from the disc layout.

Verify a download in PowerShell:

```powershell
Get-FileHash .\wccf-installer.exe -Algorithm SHA256
```

Compare the hash with the matching line in `SHA256SUMS.txt`. The binaries are
not Authenticode-signed; do not disable antivirus or bypass protection blindly.

### Optional kit (2010–2011 only)

A trusted, separately supplied `WCCF-2010-11-kit.zip` can run `scripts/setup.py`,
optionally `english.py`, and create Play/Stop shortcuts. The ZIP may contain the
kit directly or inside one top-level folder. Its bundled Windows Python and an
NTFS destination are required. **Kit setup executes code from the ZIP:** only
select a kit whose source and provenance you trust. An existing `data\` folder
is preserved during kit unpacking.

The community kit is a separate project:
[Forza-WCCF/WCCF-Kit](https://github.com/Forza-WCCF/WCCF-Kit).
No kit is fetched automatically or included in releases. SDDY setup is rejected
before destination writes unless `--no-setup` is selected.

### Runtime safety and limits

- Never launch the extracted original executables directly: unguarded RingEdge
  binaries have dropped host DHCP leases in local experiments.
- Only use a compatible, hash-verified guarded runtime: game copies importing
  `wccfnet.dll` instead of `IPHLPAPI`, with `netguard.dll` injection retained
  as secondary protection. That covers four identified IP-helper APIs, not a
  general sandbox. The installer neither provides nor validates those guards.
- The installer itself does not launch the game or change interfaces, routes,
  firewall rules or registry settings. External kit behavior is not verified here.
- The local 2017–2018 runtime reached attract/guest play, but player models did
  not render; completed matches and save/reload remain unverified. No runtime
  compatibility is promised by successful extraction.
- Windows GUI interaction has not been manually validated. This experimental
  prerelease is owner-approved using quick functional checks and prior extraction
  evidence. Automated checks use synthetic files and never start the game.

## Headless use

```powershell
# Extract either supported image; no kit needed:
.\wccf-installer.exe --iso "D:\Media\GAME.iso" --key "D:\Keys\GAME.key" --dest "D:\Games\Forza-WCCF"

# Reuse existing extracted files without reading media or keys:
.\wccf-installer.exe --dest "D:\Games\Forza-WCCF" --game SBWG
.\wccf-installer.exe --dest "D:\Games\Forza-WCCF" --game SDDY

# Optional trusted 2010–2011 kit:
.\wccf-installer.exe --iso REV_D.iso --key REV_D.key --dest "D:\Games\Forza-WCCF" --kit KIT.zip --english

.\wccf-installer.exe --help
.\wccf-installer.exe --version
```

Flags: `--no-verify` skips data-block CRC checks (header/metadata validation
still runs); `--no-setup` ignores a supplied kit; `--english` requires kit
setup. Headless mode does not create shortcuts. Errors return a nonzero exit code.
Because this is a Windows GUI-subsystem application, use `Start-Process -Wait -PassThru`
when you need to wait for headless completion from PowerShell.

## Build and test

Use Windows x64, Visual Studio C++ build tools / Windows SDK and rustup.
`rust-toolchain.toml` pins the compiler. `Cargo.lock` is committed.

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release --target x86_64-pc-windows-msvc
cargo xtask check      # from the repository root: also the license notices and the synthetic headless smoke run
powershell -NoProfile -File scripts/package.ps1
```

The release profile uses thin LTO, one codegen unit, abort-on-panic and stripped
symbols; no host-specific CPU tuning. Packaging stages the downloadable EXE,
README, changelog, MIT license and Windows dependency license notices in `dist/`,
plus a ZIP containing those files and checksums for the EXE and ZIP.

Optional read-only integration check, only for a concrete decryption regression
or an explicit request. Reuse passing evidence; do not repeat extraction or
game-tree hashing for documentation/packaging changes. Never supply media or
keys to hosted CI:

```powershell
$env:WCCF_ISO = 'D:\Media\GAME.iso'
$env:WCCF_KEY = 'D:\Keys\GAME.key'
cargo test --locked --release first_mib_matches_known_decryption -- --ignored
```

This checks independently recovered first-MiB hashes for either supported disc,
including hex-text keyfiles. Historical full SDDY extraction matched 6,816 files
byte-for-byte against independent recovery; that is extraction evidence, not a
claim about gameplay.

## Checks

Local only (Forza-WCCF monorepo; no CI, nothing published): `cargo fmt --check`, strict Clippy, unit tests,
`cargo xtask check` (which runs the synthetic headless smoke checks and compares `THIRD-PARTY-LICENSES.json` with the
Windows dependency tree) and `scripts/package.ps1`. A Cargo update refreshes `THIRD-PARTY-LICENSES.json`.

Report bugs without attaching game data, keys or saves; redact personal paths
and identifiers from logs. Installer source is MIT; dependency licenses are
listed separately. WCCF and publisher names identify compatible media; no affiliation
or game-data license is implied.
