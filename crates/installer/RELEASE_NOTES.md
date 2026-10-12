# Forza-WCCF v0.1.1

Download **`wccf-installer.exe`** directly from the release assets (Windows x64).
The EXE is 11,480,064 bytes; the optional ZIP contains the EXE and documentation. Keep `LICENSE` and
`THIRD-PARTY-LICENSES.json` alongside the EXE; `SHA256SUMS.txt` covers both downloads.

## Forza-WCCF, two coexisting editions

- Forza-WCCF branding in the installer window, wizard, CLI, documentation and shortcuts.
- Default base folder: `C:\Games\Forza-WCCF`.
- **2010–2011 Rev D / SBWG:** `<base>\2010-2011\game`, with its own kit,
  settings, logs and saves. Optional trusted 2010–2011 kit setup is supported.
- **2017–2018 v2.00.00 / SDDY:** `<base>\2017-2018\game`, isolated from 2010–2011.
  Extraction only; the 2010–2011 kit cannot be applied to it.
- The wizard selects an installed edition to reuse. Headless mode uses
  `--game SBWG` or `--game SDDY` when both coexist. New extraction selects
  the version from its disc layout.
- Earlier installation folders remain reusable in place. Nothing moves or
  replaces their game files or saves. Shortcuts include the edition name.

The original 0.1.0 release remains unchanged; use this newly branded EXE for
new installs. This is installation coexistence, not a claim about two active
game runtimes running simultaneously.

## Checks and limits

Rebuilt with native Windows x64 MSVC/Rust 1.99.0 because executable branding
and install-path routing changed. Formatting, strict Clippy and four unit tests
passed; native release build and functional/package checks passed. Checks cover CLI edition validation,
versioned destination routing, both installed editions, ambiguity rejection,
separate saves, optional-kit version gating and legacy reuse. No game or kit
script is executed by synthetic checks. Decryption/NTFS extraction is unchanged;
prior passing media evidence is reused. No disc extraction, CRC sweep or
game-tree hashing is repeated for this release.

Manual Windows GUI interaction remains unvalidated. Binaries are unsigned.
This is an experimental prerelease, not a verified playable-game package; no
completed match or save/reload claim is made. Never launch original unguarded
extracted executables; use only a compatible guarded runtime.

The repository remains **private**; downloads require GitHub access. No game
media, keys, runtime kit or saves are included. Supply your own supported image
and matching keyfile.
