# Installer checks and releases

- Work directly; no subagents unless explicitly requested.
- Use quick functional checks and reuse passing extraction/decryption evidence.
  Do not repeatedly extract game media, sweep CRCs, or hash recovered game trees.
  A media-scale check needs an explicit request or a concrete regression, with
  the changed behavior and reason recorded first.
- Documentation/packaging changes do not require rebuilding an unchanged EXE.
  Use `scripts/smoke.ps1` and `scripts/package.ps1`; release checksums are only
  for the small distributable EXE/ZIP. Stop when the designated checks pass.
- Never upload media, keys, game binaries, runtime kits, saves, personal logs
  or credentials. Distribute only the installer and its accompanying notices.
- Brand the project Forza-WCCF. 2010–2011 and 2017–2018 must coexist under
  separate version roots; never share or overwrite their kits, settings,
  logs, saves or shortcuts. Preserve earlier installs in place.
- Never launch extracted game executables or change host network settings.
- Keep this repository private. Publish only on owner instruction; label actual
  evidence and remaining limits. Do not silently overwrite published assets.
