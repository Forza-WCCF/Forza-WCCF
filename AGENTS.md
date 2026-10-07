# Agent instructions

Read in order: `docs/TASKS.md` (what to do next), `docs/PROGRESS.md` (state, evidence), `docs/RESEARCH.md`
(addresses, protocols — do not re-derive). Update TASKS/PROGRESS when you finish or stop.

## Rules

- Never commit Sega files or data derived from them: game binaries, patched exes, IDA databases,
  decompiled code, extracted card pictures/catalogues, screenshots, logs.
- Never modify the decrypted originals; work on the runtime copy. Patches go in `tools/patches1718.py` with
  original bytes, and are checked before writing.
- Launch the game only through the guarded runtime (client/control import `wccfnet.dll` instead of
  IPHLPAPI; `netguard.dll` injected as secondary guard). Never change host interfaces, routes, DNS, hosts file
  or firewall. Record routes/ping before and after runs.
- No outbound connections to Sega/naominet hosts: redirect to local stand-ins.
- Only one session/agent starts, stops or rewrites the runtime at a time. Parallel agents work in their own
  folders and hand results back.
- Stop only processes whose path is inside the runtime folder.
- Capture only game windows (`tools/cap.ps1`, PrintWindow), never the screen.
- Claims of playability need observed evidence (capture or log), not a boot screen.

## Workstreams (briefs in `docs/agents/`)

reader (client offline decision + Aime reader), aimedb (AiMeDB stand-in), gamesvr (club persistence),
catalogue (card catalogue), board (card table).
