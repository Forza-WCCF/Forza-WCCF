# Changelog

## 0.1.0 — initial prerelease

- Windows x64 wizard and headless installer for user-supplied RingEdge images.
- WCCF 2010–2011 Rev D (SBWG) and 2017–2018 v2.00.00 (SDDY) game-slot extraction.
- Raw or hex-text keyfiles, metadata/header validation and optional full game-block CRC checks.
- Streaming AES-256-LRW decryption and NTFS extraction; no mounted or temporary volume image.
- Optional user-supplied 2010–2011 kit; setup is rejected for SDDY before destination writes.
- Refuse to replace an existing game folder; existing files can be reused without disc inputs.
- Help/version output and strict headless argument validation.
- Windows CI, downloadable EXE, accompanying license notices, checksummed bundles and manually dispatched draft-release automation.

No game media, keys, extracted files, runtime kit or player saves are included.
This release prepares files, not a verified playable game. Windows GUI interaction,
completed player matches and save/reload are not release claims.
