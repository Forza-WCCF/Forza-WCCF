# Brief for WCCF 2017–18 persistence agents (written by the maintainer, 2026-10-07)

Goal: persistent club data for WCCF 2017–18 (SDDY) on one Windows PC: an emulated Aime card, a local AiMeDB,
and a local stand-in for Sega's game server, so a club is created, saved and reloaded across restarts.

## Hard rules (from <workspace>/AGENTS.md and the user)

- You do REVERSE ENGINEERING and WRITE CODE/NOTES ONLY in your own directory
  `<repo>/agents/<your-name>/` and your own IDA directory
  `<runtime>\ida-<your-name>\` (= `<runtime>/ida-<your-name>/`). Nothing else is writable.
- NEVER launch, stop or restart any game program (client/control/match/Launcher/GmSync/testmode) or the kit
  (play.py, PLAY*.bat, restart.sh, press.sh). the maintainer owns the running runtime `<runtime>`.
- Never modify originals: `<decrypted game>/`, `<runtime>\game`,
  `<runtime>\seat1`, `<runtime>\kit`, `<runtime>\ida\*` (read/copy only).
- Do not touch card/catalogue/player-board work (the user runs that in parallel): `kit/overlay/`, `cards.py`,
  `FPR_Emu`, `fpr_table*`.
- Never change host network, DNS, hosts file, firewall, routes. No outbound connections to Sega/naominet hosts.
- Do not launch further subagents.
- Report findings with addresses and evidence; mark guesses as guesses.

## Tools

- IDA Pro 9.3 headless: `"/mnt/c/Program Files/IDA Pro 9.3/idat.exe"` with the helper
  `<runtime>\ida\ida_query.py` (read its header: commands decomp:/xrefs:/callers:/str:/disasm:).
  Example (13 s per batch, run from WSL):
  `"/mnt/c/Program Files/IDA Pro 9.3/idat.exe" -A '-S<runtime>\ida\ida_query.py <runtime>\ida-NAME\out.txt decomp:0x5DC4A0 str:COACH_LIST' '-L<runtime>\ida-NAME\q.log' '<runtime>\ida-NAME\client_Release.exe.i64'`
  An .i64 is locked while open: ALWAYS use your own copy. The client database
  `<runtime>\ida\client_Release.exe.i64` is still being built when you start: poll until that file exists AND
  no `idat.exe` process has it open (`powershell.exe -NoProfile -Command "Get-Process idat -ErrorAction
  SilentlyContinue"` shows none, or check its size is stable for 60 s), then copy it to your IDA directory.
  The controller database is ready: `<runtime>\ida\control_Release.exe.i64`, and its full Hex-Rays dump is
  `<runtime>/ida/control.c` (grep it; 6,716 functions).
- radare2 (`r2`) works on the PE files for quick looks. Client VA→file offset for .text/.rdata: off = VA − 0x400C00;
  .data: off = VA − 0x401200. Client has ASLR at runtime (preferred base 0x400000; IDA uses 0x400000).
- Windows Python with ctypes: `<runtime>\kit\python\python.exe` (kit's embeddable 3.13). Stand-ins run there.

## Established facts (do not re-derive)

Runtime: `<runtime>\game` (control + projector), `<runtime>\seat1` (player cabinet, own patched exe,
patches in `<repo>/patches1718.py`). Seat 1 boots to attract, START works,
reaches OFFLINE guest play (`Scene_ADV_Aime_GuestPlay_Offline.sbscene`; the Aime-screen object loads scenes into
fields +0x64 Reading, +0x68 GuestPlay, +0x6C GuestPlay_Offline, +0x70 GuestPlay_Maintenance; loader code around
client VA 0x78D3D0).

Device emulation: the kit's `winmm.dll` hook (source `kit/source/mxhook/mxhook.c`) maps serial ports to named
pipes via env `MXHOOK_COM="COM3=\\.\pipe\x;COM1=...;mxjvs=...;COM4=..."` (CreateFileA by name, handles comm APIs,
ReadFile/WriteFile through the pipe; see its comments). Existing pipe stand-ins to copy the style from:
`kit/scripts/_icc_reader.py` (2010–11 card reader), `kit/scripts/_jvs_board.py` (I/O board). The hook also
answers gethostbyname for this PC only; it does NOT redirect other names.

Client serial activity observed: only `CreateFileA("\\.\COM2")` (fails today). Old IC reader (COM3/COM1, probe
func 0x5CB1D0) is dead code (no callers). Red on-screen notices seen: "二次元コード!" / "Aimeカード!" and
"二次元コードリーダーが接続されていません" (QR reader not connected). Client has aimelib Ver.3.07.7, classes
cAiMeLib, cAiMeDB, cAiMeNFCRW, AimeAuthSequenceNode, AimeDataNewSequenceNode, AimeBackUpSequenceNode,
ICCardErrorSequenceNode; string `\\.\COM%u` near "Copyright(C)SEGA" + AES tables (.rdata ~0xE2D7C0 file off).
Client debug-option table (name, category, default) around VA 0x1263034: IS_USE_AIME_READER default 1,
ALLNET_AUTH default 1. The client config `local/client_user_option.conf` currently sets ALLNET_AUTH=0, DHCP=0,
IS_RINGEDGE=1 (seat). AiMeDB: client resolves `aime.naominet.jp` (pointer at VA 0x153CE7C → string VA 0x122CE50)
port 22345 via getaddrinfo (code ~VA 0xD84236). Plan: patch that string in seat 1's exe to "localhost".

Control → game server ("gmsvr"): TCP, control connects to GAME_SVR_IP:GAME_SVR_PORT (localhost:20050 now,
FORCE_ONLINE=1, control patched so its ALL.Net "network up" check FUN_004B73C0 returns 1). Framing (control
0x40A970 build / 0x40ABE0 check / 0x40AB80 checksum), 40-byte LE header: u32 0x4480C0F1, u32 0x3CDA9EB4,
u16 cmd @8, u16 seq @10, u32 status @12 (response payload used only if >0), u32 total size @16, u32 conn id @20,
u8 checksum @24 (= byte sum of header with byte 24 zeroed), char[15] @25. Request N → response N+1, matched on the
payload's first u32 (control's slot for the cabinet). Message table: `<repo>/gamesvr/msg_table.txt`
(e.g. 271/272 COACH_LIST 20/9292, 275/276 SAVE_DATA 32/6176, 277/278 SAVE_DATA_SET 8572/76, 279/280
COACH_LIST_ADD 36/16, 287/288 EXTENDS_NEW_ID, 289/290 INJUSTICE_COUNT, 291/292 EXTENDS_CHECK, 293/294 AIME_UNLOCK,
283/284 PASS_SET, 285/286 ITEM_LIST). Control is a RELAY for these: client builds the request, control fills
[0]=slot and shop id / revision 47565 (control.c ~line 51180, case 271/273/275/277...) and forwards; the response
goes back to the client. Control's own startup traffic (observed live with the stub answering zeros + status 1):
201→202 (16 B), 265→266, 237→238 (0x5A54 B), 233→234, 259→260, 263→264, 267→268, 44 SLS_AUTH_INI, 46 SLS_SVR_ACC
(no answer yet), 309→310, 261→262 every 6 s. The stub: `kit/scripts/_gamesvr.py` (log in
`<runtime>\kit\data\logs\gamesvr_log.txt`, raw request payloads in `<runtime>\kit\data\save\gamesvr\`).
Save data is a bit-packed field schema (field-name tables like CLUB_NAME, COACH_NAME… in both exes).

## Update 2026-10-07: catalogue and board agents

The user also asked for agents on the card CATALOGUE and the PLAYER BOARD (card table). Those two agents own
that work; the earlier "do not touch card/catalogue/player-board" rule now means: only the `catalogue` and
`board` agents work on it, and still only in their own directories (never directly in `kit/overlay/`,
`<runtime>\kit`, `seat1\`, `misc\`; the maintainer installs results). A mingw i686 toolchain for
building the kit's C tools: `<workspace>/ringedge/revd-continuation/netguard/toolchain`
(see `build_wccfnet.sh` there for flags). Kit sources: `<repo>/kit/source/`
(fpr_emu, overlay/wccfpanel.c, mxhook), kit scripts `kit/scripts/` (cards.py, codes_inferred.py, ys_lzw.py,
club_card_schema.py ...). The live overlay already runs on 17–18 seat 1 (panels, formation board, catalogue button);
its catalogue is currently empty (`kit/overlay/catalogue.tsv` is a 0-byte placeholder).
