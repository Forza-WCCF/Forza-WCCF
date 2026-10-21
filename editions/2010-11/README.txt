Forza-WCCF 2010-11 (Rev D) on a PC
======================================

WHAT IT IS
  Community tools for your own copy of WCCF 2010-11 (Rev D): a launcher, stand-ins for the cabinet's hardware and an
  in-game panel. The server, the projector (the shared big screen) and a player cabinet (seat 1) run on one Windows PC.
  The game itself is not included, and Forza-WCCF holds none of the original files: setup makes everything from your copy.
  The panel around seat 1's picture has:
    cabinet buttons
    FORMATION BOARD   your cards on a table: drag to move, drop on a card to swap, right-click to take one off
    CATALOGUE         all 3,909 player cards: search, filters, sort, a slider per stat; a click puts a card on your table
    KEYS              your own keys or a game controller's buttons
    SETTINGS          server, language, view, logs
    CLUB CARD         your club card and your other clubs
  Release notes: CHANGELOG.md.

NOT RELEASED YET
  - No Python: setup, undo, the game's text (English, Italiano, Japanese) and the club card steps run inside
    Launcher.exe. Forza-WCCF has no python\ or code\ folder, and the command-line card editor (edit.py) is gone.
  - Online lobby: choose Online in Launcher.exe to see the shops of your servers. See ONLINE.
  - Pause: P holds a match for up to 20 seconds. See PAUSE.
  - The installer's package step sets up Forza-WCCF without Python (see the installer's README).

YOU NEED
  - Windows 10 or 11, 64-bit, with the game on an NTFS drive (C: and D: normally are)
  - your copy of the game, Rev D, as a folder of files. The game folder holds client_Release.exe, control_Release.exe
    and match_Release.exe.
  - DirectX 9: Microsoft's "DirectX End-User Runtime (June 2010)". Setup warns you if it is missing.

SETUP (once)
  1. Unzip Forza-WCCF into its own folder. Keep the game in its own folder too, on a plain path such as D:\Games\WCCF
     (the game fails on non-English letters in folder names).
  2. Drag the game folder onto Launcher.exe. Or open Launcher.exe, "Setup and updates", pick the game folder (BROWSE or
     drag it in) and click SET UP / REPAIR.
  3. Setup checks for Rev D, makes the changes listed under WHAT SETUP CHANGES, then builds the card catalogue and
     pictures from your files (about a minute). Run it again to check and repair.
  Launcher.exe also has LANGUAGE (English, Italiano, Japanese; default is Windows' language), GAME TEXT (see ENGLISH),
  UNDO SETUP and UPDATE, all under "Setup and updates".
  UPDATE: there is no release site yet, so it says "No update found". To update by hand, unzip the new version over the old
  folder, open Launcher.exe, then SET UP / REPAIR (and GAME TEXT again if you use English). Your data\ folder (clubs,
  keys, settings, logs) is never touched.

PLAY
  Launcher.exe starts the server, the projector window, seat 1's window with the panel, and the key driver in about
  30 seconds. Only the projector and seat 1 show windows. "Launcher.exe show" also shows the server's console;
  "Launcher.exe debug" opens every window and keeps full logs.
  Sound: the projector starts muted and seat 1 at 4% of the PC's volume. Change either in Windows' Volume Mixer; the
  next start resets them.
  The game runs in rounds: an entry window, then the matches. Forza-WCCF keeps the entry window open for 2 minutes.
  Put your card in soon after starting to play in the first round. To change it, edit FIX_REST_ENTRY_TIME (seconds) in
  local\ctrl_user_option.conf in the game folder.
  Click seat 1's window, then:
      I       club card in / out (a new card makes your club: manager license, club name)
      Enter   START (free play: no coin needed)
      X       decide          C   shoot          arrows   tactics
      F1      test menu       F11 window size
      P       pause a match
  The buttons around the picture do the same. Keys count only while a game window is in front.
  KEYS (right of START) lists every cabinet button with a KEY box and a CONTROLLER box.
    Key: click a KEY box and press the key. A key another button has swaps over. Esc, F10, F11, Alt and the Windows
    keys stay free.
    Controller: click a CONTROLLER box, then press a button or move the stick or d-pad. A USB encoder under real
    cabinet buttons, an arcade stick and a gamepad work. RESET > CONTROLLERS shows what Windows sees. Right-click a
    CONTROLLER box to clear it. An encoder that types keys goes in the KEY boxes.
    Changes work at once and are kept in data\keys.txt. RESET (or deleting that file) restores the keys above.
  Your club is saved in data\save\seat1_club.bin. Back it up.
  The version is at the top right of the game. Give it when you report a problem.

PAUSE
  P or the PAUSE button holds a match for up to 20 seconds: a PAUSED page covers the last frame and the sound is
  muted. P or RESUME ends it early. The next pause waits 10 seconds. Change the key in KEYS.
  It works only in a match on this PC with no other seat linked and no projector running. Otherwise the panel gives
  the reason.

SETTINGS (left side, under CARD)
  NOW           link to the server, card session, bad endings, last save and backup, game text
  NEXT START    THIS PC, or ONLINE with a server's address; ENGLISH or JAPANESE (same as GAME TEXT in Launcher.exe).
                Kept in data\panel.txt.
  RESTART NOW   click twice; restarts the game with these choices. Not during a match. If server and language are
                unchanged, only your cabinet restarts.
  VIEW          LAYOUT CABINET (buttons around the game) or COMPACT (larger game, no buttons: use your keys or
                controller; SETTINGS and CLUB CARD move under CATALOGUE and KEYS). Takes effect at once.
  LOGS          SEND LOGS (click twice) sends this game's logs to the server you play on. Your club card is never
                sent, and your Windows user name and PC name are removed. You get a code: post it in Discord with
                what went wrong.

CLUB CARD (left side, under SETTINGS)
  Shows your club card: club, manager, contract, league, prize money, card health (session, bad endings, saved
  copies, backups) and the squad (red = injured). It updates at each save.
  YOUR CARDS (more than one club)
    NEW CLUB CARD   puts this club aside and a blank card in the slot. Put it in (I) and the game makes a new club.
    PLAY THIS CLUB  puts that club back in the slot.
    Click twice. Each switch restarts your cabinet (about a minute) and never runs during a match. No card is ever
    deleted; the other clubs are in data\save\cards.
  Each club keeps its own formation board. A new club gets a 4-4-2 with the substitutes on the bench. The board sits
  beside the card file (a .board folder). A table that belongs to no card goes to data\save\boards_unknown.
  CLEAR BAD ENDINGS (under CARD HEALTH, when the card has a bad ending): click twice. The game restarts with the bad
  endings at 0; the old card goes to data\save\backup first. Not during a match. Trade rights or money already taken
  stay taken.
  When a contract ends, put the card in again after the last match: the game moves your manager to a new card and you
  make a new club. The old card is kept in YOUR CARDS as "transferred".

QUIT
  After a match the card comes out by itself (or press I). Then close the game window (X or Alt+F4). Everything
  Forza-WCCF started stops too: the projector, the server, its match engines and helpers. data\logs\run_ended.txt lists what
  was stopped. Closing the projector's window does the same.
  During a match, closing seat 1 ends it without a save and gives the card a bad ending (trade rights are lost at 2).
  The first close only shows a warning; close again within 8 seconds to quit anyway. Closing the projector during a
  match leaves seat 1 playing.
  "Launcher.exe stop" ends a server started with "Launcher.exe server". "Launcher.exe stop force" ends a stuck game.
  A game window that crashes stops nothing else; the projector keeps running. Its log (data\logs\run_seat1.txt or
  run_projector.txt) says what happened.

ONLINE
  Lobby: in Launcher.exe choose Online. Under "Direct address" type a server (HOST or HOST:PORT). SHOPS lists its
  shops: NAME, PLAYERS (x/8), PING, LANG and STATE (OPEN, IN MATCH, FULL, CLOSED, NO ANSWER), with the time to the
  next ENTRY. Filters: language, open only, ping under 150 ms.
  Pick a row and press JOIN to take a seat, then PLAY. LEAVE gives the seat up. If the game or Launcher closes,
  REJOIN (offered for 75 seconds) takes the same seat back. VS ROOMS is greyed out ("not available yet").
  Keys: Up/Down row, Left/Right or Tab list, Enter join, F5 refresh, Esc leave.
  Gamepad: D-pad or left stick move, A = Enter, B = Esc, X = refresh, LB/RB = list.
  Each PC runs only its own cabinet. Without the lobby: "Launcher.exe remote ADDRESS" (for example
  Launcher.exe remote 192.168.1.20), or ONLINE in SETTINGS. "Launcher.exe local" has its own projector.
  Hosting: "Launcher.exe server" starts only the server, in the background. The window that started it closes after
  30 seconds. The PC must allow TCP ports 20002, 20030 and 20050 and UDP port 20040; behind a router, forward them.
  The host can play too: "Launcher.exe remote" with its own address (ipconfig shows it). Logs that players send are
  kept in data\player_logs.
  When an online game ends badly (a crash, or a lost link to the server: Error 3000), Forza-WCCF sends this PC's logs to
  the server, same as SEND LOGS. Your club card is never sent. After a crash, a memory snapshot goes along;
  data\dumps keeps the newest 5. If the server cannot be reached, the next start sends them.
  To turn this off, add the line auto_logs=0 to data\panel.txt.

ENGLISH, ITALIANO (optional)
  GAME TEXT in Launcher.exe switches the game's language. JAPANESE puts the original files back. Close the game first.
  ENGLISH covers about 7,200 lines of screen text, player names (original Latin spelling, e.g. M.DIARRA), skill names,
  CPU team names, the projector's "Next match" ticker, dates, money in dollars with commas ($3,982,700) and the shop
  name ("Local Shop"). It is built on your PC from your game files, and every line is checked. The originals are kept
  in data\english_backup.
  Still Japanese: country and prefecture names (English ones crash the server), the network-ranking areas, and
  writing inside pictures (logos, titles, the six team stats, OK / BACK).
  Four rare screens show money in two pieces ("$2 million$299000"): a fan event, a golden age, a financial crisis and
  the end of a contract.
  Awards saved before English was on keep their Japanese names.
  ITALIANO works the same way. Each line is Forza-WCCF's Italian, else the original Italian of the older European WCCF,
  else English; never Japanese. The game's font has no accented letters, so they are written with an apostrophe
  (perche', citta'). Player and CPU team names and supporters' nicknames stay in English. The in-game SETTINGS panel
  offers only ENGLISH and JAPANESE for now.

WHAT SETUP CHANGES   (UNDO SETUP in Launcher.exe reverses all of it)
  game folder   adds wccfk.dll (Forza-WCCF's hook) and points the game's programs at it; adds local\client_user_option.conf
                (projector) and local\ctrl_user_option.conf (server)
  beside it     seat1\  the player cabinet: links to the game folder's files, its own copy of client_Release.exe with
                        15 changes, its own settings
                misc\FlatPanelReader_Emulator\exe\FPR_Emu.exe  the card-table helper the game starts
  Nothing else on the PC changes.

GOOD TO KNOW
  - Club card saves go to a temporary file first, so a crash or power cut cannot leave a broken card. Before the first
    save of each session the card is copied to data\save\backup (the newest 20 are kept; CLUB CARD lists them). To
    restore one, close the game and copy it over data\save\seat1_club.bin.
  - Windows may ask whether control_Release.exe may use the network. Either answer works; everything stays on this PC.
  - Antivirus programs may flag programs\wccfk.dll (it sits between the game and Windows) and programs\inject.exe (it
    loads the panel into the game). Their source is in source\.
  - One copy at a time on a PC.
  - If both pictures freeze while the sound goes on, Windows took the graphics device from the game (a display change,
    Ctrl+Alt+Del, an administrator prompt). Close the game window (or "Launcher.exe stop force"), then start again.
  - Logs are in data\logs ("Launcher.exe debug" keeps more; the card reader's full log grows about 60 MB an hour).
    The previous run is in data\logs\previous; older runs are zipped in data\logs\archive (about 200 MB, oldest
    removed first). If a game window closes by itself, its log (run_seat1.txt, run_projector.txt or run_server.txt)
    says why. Share that log.
  - If seat 1 closes at the start of a match, the game drew a text it cannot format (a stray %). The panel shows the
    text as it is and writes a "badfmt:" line to seat1\forza_panel.log. Share that line.
  - The game counts every earned player card as owed ("You are owed N Player Card(s)") because there is no dispenser.
    The panel tells the game nothing is owed.
  - Windows 11 slows the timers of minimized or covered programs, which made the projector's live matches stutter. The
    Forza-WCCF switches that throttling off for the game's programs (see data\logs\run_server.txt).
  - The game draws 30 pictures a second and syncs them to the screen only when the main monitor runs at exactly 60 Hz.
    At other refresh rates movement can look uneven; 60 and 120 Hz are even.

FOLDERS
  programs\   wccfk.dll (hook), forza_panel.dll (panel), inject.exe (loads the panel into seat 1, and into the
              projector for money with commas), FPR_Emu.exe (card-table helper)
  fonts\      the panel's fonts and licences (Noto Sans JP, Barlow Condensed, Hack)
  languages\  one folder per language. english\: screen_text.tsv, original_rstring.tsv, cpu_names.tsv, exe_text.tsv,
              pictures.tsv. italian\: the Italian text. Text and fingerprints only.
  source\     source of every program in programs\ (C) and of Launcher.exe (Rust, source\launcher); how they were
              built: source\README.txt
  data\       made on your PC: settings (panel.txt), club card (save\, with backup\ and your other clubs in cards\),
              keys (keys.txt), logs, and cache\ (catalogue and card pictures; rebuilt if deleted)
