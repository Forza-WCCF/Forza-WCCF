# Tasks

Status keys: [x] done, [~] partial, [ ] open. Facts and addresses: `RESEARCH.md`.

## Club persistence (main goal)

Why: 2010–11 stores the club on the IC club card; 2017–18 identifies the player by an Aime card and keeps the
club on Sega's game server (client → control relay → gmsvr). Offline → guest play only, nothing saved.

- [x] Controller online against a local game server: `FORCE_ONLINE=1`, `GAME_SVR_IP=localhost` (literal
  `127.0.0.1` means "use ALL.Net's address"), `GAME_SVR_PORT=20050`, control patch FUN_004B73C0 → 1.
- [x] gmsvr framing + message table (`protocol/gmsvr-message-table.txt`).
- [~] `kit/scripts/_gamesvr.py`: logging stub, answers zeros + status 1, keeps SAVE_DATA_SET payloads.
- [x] Client online: ALL.Net authorized + billing phase IDLE patches (ALL.Net itself stays off), Aime reader up.
  The Aime screen now reads the card instead of OFFLINE guest play (RESEARCH.md "Update 2026-10-08").
- [x] Aime reader on COM2: `kit/scripts/_aime_reader.py` (one Mifare card, access code in
  `data\save\seat1_aime.txt`). Open: the LED board firmware is re-flashed every boot (~13 s); report the version
  the client wants. Card on/off control (now always on the reader).
- [x] AiMeDB: `kit/scripts/_aimedb.py` on 127.0.0.1:22345 (hello, lookup → Aime ID, `data\save\aimedb.json`);
  seat 1 patch aime.naominet.jp → localhost. Register/FeliCa/log/campaign answers follow Artemis, not yet seen.
- [ ] Decode COACH_LIST 271/272, SAVE_DATA 275/276, SAVE_DATA_SET 277/278, COACH_LIST_ADD 279/280,
  EXTENDS_*, INJUSTICE_COUNT, AIME_UNLOCK; persist clubs per Aime ID (atomic writes, backups). — `agents/gamesvr.md`
- [ ] Tutorial / new club flow with the zero-answer game server: next stop after the TUTORIAL / NORMAL choice.
- [ ] Live test: new club → match → save → restart seat → same club loads.

## Cards

- [x] Catalogue: `research/catalogue/cards1718.py` checked against printed card backs (FINDINGS.md): 8,396 real
  cards, 38 placeholders, newest season first, self-check `python cards1718.py GAME`. Install = run it with the
  game and `kit\overlay` as OUT. Overlay cap raised 4,096 → 16,384; the rebuilt `kit/overlay/wccfpanel.dll` (MSVC x86, build line in
  `kit/source/README.txt`) loads `catalogue 8396 cards` live (run15, 2026-10-08).
- [x] CATALOGUE search: ported the 2010-11 kit's club/country search (d750f94) and card roles (76d246f) and
  improved them: several words must all match (name, club, country, season, line FW/MF/DF/GK, role rated 8+),
  the overlay finds nationality and role columns by the header (2010-11 and 2017-18 catalogues differ), roles shown
  only when all 16 are 1..10. Checked in fakegame (2010-11 kit's harness): brazil 957, "barcelona 2017" 38,
  "italy cb" 159, "brazil fw" 217, 7049 → 1 = counts computed from the catalogue. Open: 2017-18's own cards
  (2015-16 on, 3,120 cards) hold 0 in the role field; where 17-18 keeps their positions is not found.
- [~] Card drop (a test, shown only): ported from the 2010-11 kit (WCCF-Kit-1011 #17). 17-18 trigger: the owed-card
  function FUN_005c0970 (= 2010-11 FUN_004cf650, base + 0x1C0970, ASLR) jumps to the panel's counter; its callers
  are the "Game Over" / match-end paths (sub_7DCD10, sub_7DCD90, flag byte_6D9EFC8). Checked in fakegame (odds,
  the hook on a copy of the function's head, the window with the 17-18 catalogue). Open: a live match; whether one
  match counts once.
- [ ] Card table / formation board for 17–18 (FPR_MODE=2 emulator vs "Flat Reader 3 (COM%d)"). — `agents/board.md`

## Gameplay issues

- [x] Players invisible / headless in matches: the launcher's player and model data mappings were missing.
  `_debug_launch.py` now makes `SDDY_player_data.bin`, `SDDY_partnership.bin`, `SDDY_model_data.bin` (u32 size +
  `prog_data/player_data/<file>`, Launcher.exe VA 0x00403890); every process found them and the tutorial line-up
  shows full player models (2026-10-08). `player_data_diff.bin` is not shipped in 2.00.00 (no mapping, as the
  launcher does).
- [ ] LEFT/RIGHT tap via JVS input did not move the guest-prompt selection; check 17–18 button bits.
- [ ] QR reader (COM2) "not connected" notice.
