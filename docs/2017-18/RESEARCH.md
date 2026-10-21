# 2017–18 research notes: club persistence

Goal: persistent club data for WCCF 2017–18 (SDDY) on one Windows PC: an emulated Aime card, a local AiMeDB,
and a local stand-in for the publisher's game server, so a club is created, saved and reloaded across restarts.
The Rust stand-ins built from these notes are in `crates/forza/src/standins/` and `crates/forza/src/online/gamesvr.rs`.
Running the original programs: [../SAFETY.md](../SAFETY.md).

Address arithmetic: client VA→file offset for .text/.rdata: off = VA − 0x400C00; .data: off = VA − 0x401200. The
client has ASLR at run time (preferred base 0x400000).

## Established facts

Runtime layout: `<runtime>\game` (control + projector) and `<runtime>\seat1` (the player cabinet, its own patched
exe; patches in `crates/forza/src/setup/e1718.rs`). Seat 1 boots to attract, START works,
reaches OFFLINE guest play (`Scene_ADV_Aime_GuestPlay_Offline.sbscene`; the Aime-screen object loads scenes into
fields +0x64 Reading, +0x68 GuestPlay, +0x6C GuestPlay_Offline, +0x70 GuestPlay_Maintenance; loader code around
client VA 0x78D3D0).

Device emulation: the kit's `winmm.dll` hook (source `native/2017-18/mxhook/mxhook.c`) maps serial ports to named
pipes via env `MXHOOK_COM="COM3=\\.\pipe\x;COM1=...;mxjvs=...;COM4=..."` (CreateFileA by name, handles comm APIs,
ReadFile/WriteFile through the pipe; see its comments). The pipe stand-ins (card reader, I/O board) are in
`crates/forza/src/standins/`. The hook also
answers gethostbyname for this PC only; it does NOT redirect other names.

Client serial activity observed: only `CreateFileA("\\.\COM2")` (fails today). Old IC reader (COM3/COM1, probe
func 0x5CB1D0) is dead code (no callers). Red on-screen notices seen: "二次元コード!" / "Aimeカード!" and
"二次元コードリーダーが接続されていません" (QR reader not connected). Client has aimelib Ver.3.07.7, classes
cAiMeLib, cAiMeDB, cAiMeNFCRW, AimeAuthSequenceNode, AimeDataNewSequenceNode, AimeBackUpSequenceNode,
ICCardErrorSequenceNode; string `\\.\COM%u` near "Copyright(C)SEGA" + AES tables (.rdata ~0xE2D7C0 file off).
Client debug-option table (name, category, default) around VA 0x1263034: IS_USE_AIME_READER default 1,
ALLNET_AUTH default 1. The client config `local/client_user_option.conf` currently sets ALLNET_AUTH=0, DHCP=0,
IS_RINGEDGE=1 (seat). AiMeDB: the client resolves the publisher's AiMeDB host (pointer at VA 0x153CE7C → string VA
0x122CE50) port 22345 via getaddrinfo (code ~VA 0xD84236). Setup patches that string in seat 1's exe to "localhost".

Control → game server ("gmsvr"): TCP, control connects to GAME_SVR_IP:GAME_SVR_PORT (localhost:20050 now,
FORCE_ONLINE=1, control patched so its ALL.Net "network up" check FUN_004B73C0 returns 1). Framing (control
0x40A970 build / 0x40ABE0 check / 0x40AB80 checksum), 40-byte LE header: u32 0x4480C0F1, u32 0x3CDA9EB4,
u16 cmd @8, u16 seq @10, u32 status @12 (response payload used only if >0), u32 total size @16, u32 conn id @20,
u8 checksum @24 (= byte sum of header with byte 24 zeroed), char[15] @25. Request N → response N+1, matched on the
payload's first u32 (control's slot for the cabinet). Message table: [protocol/gmsvr-message-table.txt](protocol/gmsvr-message-table.txt)
(e.g. 271/272 COACH_LIST 20/9292, 275/276 SAVE_DATA 32/6176, 277/278 SAVE_DATA_SET 8572/76, 279/280
COACH_LIST_ADD 36/16, 287/288 EXTENDS_NEW_ID, 289/290 INJUSTICE_COUNT, 291/292 EXTENDS_CHECK, 293/294 AIME_UNLOCK,
283/284 PASS_SET, 285/286 ITEM_LIST). Control is a RELAY for these: client builds the request, control fills
[0]=slot and shop id / revision 47565 (the control's relay switch, cases 271/273/275/277...) and forwards; the response
goes back to the client. Control's own startup traffic (observed live with the stub answering zeros + status 1):
201→202 (16 B), 265→266, 237→238 (0x5A54 B), 233→234, 259→260, 263→264, 267→268, 44 SLS_AUTH_INI, 46 SLS_SVR_ACC
(no answer yet), 309→310, 261→262 every 6 s. The first stub answered zeros of the right size and kept the raw
request payloads.
Save data is a bit-packed field schema (field-name tables like CLUB_NAME, COACH_NAME… in both exes).

## Update 2026-10-08: online + Aime

**Why the client chose OFFLINE.** The Aime screen (client VA 0x0077A0D0) calls `sub_78E0F0(9)` = OFFLINE guest play
unless byte 0x0683653F (top byte of `dword_683653C`) is set (or the debug option FAKE_FINAL). On a seat
(`dword_15DD9B4` = satellite no. ≠ 0) the per-frame update VA 0x0040F600 sets bytes 0x0683653C/D only when all of:
ALL.Net authorized (VA 0x00C78770 == 1); control accepted the seat (byte 0x06836541, set on its "OK"); the control
link object is connected; ALL.Net billing phase (status struct `dword_6D9EE54`+0xF8, filled by VA 0x004D1430 from
VA 0x00C78E00 kind 2) in 16..24 but not 19. Byte 0x0683653F then needs (CONTROL_STATE handler VA 0x00410310) the
Aime reader up (`dword_6D9EE50`+374, set by the reader state machine VA 0x004CF5D0) and control's online flag.
- ALL.Net here is the `alw` 1.00.06 library inside the client: an HTTP PowerOn to the publisher's server
  (`Pragma: DFI`, UA `ALL.Net_PC_Win2/ver1.0`) after link/DNS/gateway/traceroute/line-type probes of the host
  network. Not emulated (it would probe the real network); ALLNET_AUTH stays 0 and two seat patches answer instead.
- Billing phase codes (VA 0x00C78E00): alpb state 1,2,4..8 → 15 (init), 3 → 16 (test mode), 9 → 17, 10 → 18
  (accounting), 11 → VA 0x00C78CF0 (idle, 20+), 12 → 19 (OFFLINE), 13 → 25 (fatal).
- Client option table (name, category, default; 12-byte entries at VA 0x01263058, values at WccfUserOption+4 as
  ints and +200+index as bools, object `dword_6D9F288`): 1 VIEW_DEBUG_MODE … 5 IS_USE_AIME_READER, 6 ALLNET_AUTH,
  7 ERROR_3000, 8 IS_REGION_JAPAN, 9 FAKE_FINAL, 14 AIME, 16 GUEST_PLAY … 29 ANNOUNCE_EXPORT.

**Aime reader** (`crates/forza/src/standins/aime.rs`): aimelib opens `\\.\COM2` synchronously (VA 0x00D8B0E0); the publisher's
837-15084 NFC reader protocol (parser VA 0x00D87040), same as segatools' sg-nfc/sg-led. Observed boot sequence:
reset 62, fw 30 ("TN32MSEC003S F/W Ver1.2" = no NFC update), hw 32, LED reset F5 / info F0 (addr 8), Mifare key A
54 = "WCCFv2" (57 43 43 46 76 32), key B 50 = 60 90 D0 06 32 F5. With LED info "15084\xFF\x10\x00\x12" the client
flashes `837_15084_1_01.hex` to the LED board every boot (F4, then 187 × F3 Intel-HEX lines, ~13 s) — harmless,
open. Card read: radio on 40, poll 42, select 43, auth A 55, read blocks 1 and 2 (52; block 2 bytes 6..15 = access
code BCD), radio off 41.

**AiMeDB** (`crates/forza/src/standins/aimedb.rs`): host string VA 0x0122CE50 patched to localhost, TCP 22345, AES-128-ECB key
"Copyright(C)SEGA", 32-byte header (magic 0xA13E, version **0x3077**, game `SDDY`, store 0, keychip empty). Seen:
hello 0x64 → 0x65 then goodbye 0x66 at boot; lookup 0x04 (access code at 0x20, then `00 02 00 00 00 00`) → 0x06
(0x30 bytes, s32 Aime ID at 0x20) accepted. With an Aime ID the client sends COACH_LIST 271 / OLD_COACH_LIST 273
(aimeId at payload +4) and 317 through control, and shows the first-play TUTORIAL / NORMAL choice.
