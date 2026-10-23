# Changelog

## 0.3.2

- The program no longer contains folder paths of the PC that built it.

## 0.3.0

- The installer's release file name carries the version, and the program has the Forza-WCCF logo.

## 0.2.2

- The window draws with OpenGL first: Vulkan flashed black on one graphics card and never opened the window on another.
  A renderer that fails falls back to the next one.

## 0.2.1

- The key can be an HTML key table, or a ZIP holding one: the rows of the image's game ID are tried until one unlocks it.
- 2017–2018: the wizard (and `--package latest`) downloads the newest package from GitHub releases and checks its SHA-256.
- The files are named `Forza-WCCF-Installer.exe` and `Forza-WCCF-Installer-<version>-windows-x64.zip`.
- No Options page: the disc is always checked and the game always set up; the Launcher sets the language. The Desktop
  shortcuts choice is on the folder page.
- A picked folder gets its own `Forza-WCCF` folder (D:\ gives D:\Forza-WCCF) unless it is one or holds an install.
- A larger window at the normal text size, DPI-aware, with version information.

## 0.2.0

- The package step runs inside the installer: setup and the game's text no longer need Python, and no Python is bundled.
- `--package` accepts a 2017–2018 package as well as a 2010–2011 one. The package's edition (read from the ZIP) must match the game's;
  a mismatch is refused before anything is written. The wizard still offers the package step for 2010–2011 only.
- A package ZIP must hold `Launcher.exe` as a file at its top.

## 0.1.1 — Forza-WCCF and coexisting editions

- Use Forza-WCCF in the wizard, window title, shortcuts, CLI and documentation.
- Install 2010–2011 and 2017–2018 under separate version roots, including each
  game, program files, settings, logs and saves. Shortcuts carry the edition name.
- Add `--game SBWG|SDDY` and wizard edition selection for unambiguous reuse.
- Preserve earlier installs in place; installing the other edition never touches them.
- Publish a newly branded EXE; leave the original 0.1.0 release unchanged.
- Quick synthetic coexistence checks only; no repeated media extraction or game-tree hashing.

## 0.1.0 — initial prerelease

- Windows x64 wizard and headless installer for user-supplied RingEdge images.
- WCCF 2010–2011 Rev D (SBWG) and 2017–2018 v2.00.00 (SDDY) game-slot extraction.
- Raw or hex-text keyfiles, metadata/header validation and optional full game-block CRC checks.
- Streaming AES-256-LRW decryption and NTFS extraction; no mounted or temporary volume image.
- Optional user-supplied 2010–2011 package; setup is rejected for SDDY before destination writes.
- Refuse to replace an existing game folder; existing files can be reused without disc inputs.
- Help/version output and strict headless argument validation.
- Windows CI, downloadable EXE, accompanying license notices, checksummed bundles and manually dispatched draft-release automation.

No game media, keys, extracted files, runtime files or player saves are included.
This release prepares files, not a verified playable game. Windows GUI interaction,
completed player matches and save/reload are not release claims.
