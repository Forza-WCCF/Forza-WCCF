Forza-WCCF 2010-11 (Rev D) on a PC
==================================

WHAT IT IS
  Tools for your own copy of WCCF 2010-11 (Rev D): a launcher, stand-ins for the cabinet's hardware and an in-game
  panel. The server, the projector (the shared big screen) and one player cabinet (seat 1) run on one Windows PC.
  The game is not included, and Forza-WCCF holds none of the original files: SET UP makes everything from your copy.
  Release notes: CHANGELOG.md.

YOU NEED
  - Windows 10 or 11, 64-bit, with the game on an NTFS drive (C: and D: normally are).
  - Your copy of the game, Rev D, as a folder of files: the one with client_Release.exe, control_Release.exe and
    match_Release.exe.
  - DirectX 9: Microsoft's "DirectX End-User Runtime (June 2010)". SET UP warns you if it is missing.

SETUP (once)
  1. Unzip Forza-WCCF into its own folder. Keep the game in its own folder too, on a plain path such as D:\Games\WCCF:
     the game fails on non-English letters in folder names.
  2. Open Launcher.exe and pick your language at the top right (English, Italiano or Japanese).
  3. Drag the game folder onto Launcher.exe, or open SETUP, pick the game folder (Browse, or drag it in) and press
     SET UP. It checks for Rev D, makes the changes in WHAT SETUP CHANGES and builds the card catalogue and pictures
     (about a minute). SET UP again checks and repairs.

LAUNCHER
  PLAY      WHERE TO PLAY (This PC or Online), then PLAY. STOP THE GAME closes the game (during a match: CLICK AGAIN).
  SETUP     game folder, SET UP, UNDO SETUP, CARD PICTURES (From the game, or High-quality pack with Choose pack).
  UPDATE    lists the published releases: pick one, then UPDATE. The game is set up again by itself; your clubs, keys
            and settings in data\ are kept.
  DETAILS   what the Launcher did, with Copy and SAVE LOGS.
  The language at the top right sets the Launcher and the game's text (see GAME TEXT). The game follows it at the
  next PLAY.

PLAY
  PLAY starts the server, the projector window, seat 1's window with the panel, and the key driver in about
  30 seconds. Only the projector and seat 1 show windows. "Launcher.exe show" also shows the server's console;
  "Launcher.exe debug" opens every window and keeps full logs.
  Sound: the projector starts muted and seat 1 at 4% of the PC's volume. Change either in Windows' Volume Mixer; the
  next start resets them.
  The game runs in rounds: an entry window, then the matches. Forza-WCCF keeps the entry window open for 2 minutes.
  Put your club card in soon after the start to play in the first round. To change the time, edit FIX_REST_ENTRY_TIME
  (seconds) in local\ctrl_user_option.conf in the game folder.
  Your club is saved in data\save\seat1_club.bin. Back it up.
  The version is at the top right of the game. Give it when you report a problem.

KEYS
  Click seat 1's window first. Keys count only while a game window is in front.
      I       club card in / out (a new card makes your club: manager licence, club name)
      Enter   START (free play: no coin needed)
      X       decide          C   shoot          arrows   tactics
      F11     window or full screen
      P       pause a match
  The buttons around the picture do the same.
  KEYS lists every cabinet button with a KEYBOARD and a CONTROLLER column.
    Keyboard: click a cell and press the key. A key that another button has swaps over. Esc, F10, F11, Alt and the
    Windows keys stay free.
    Controller: click a cell, then press a button or move the stick or D-pad. A USB encoder under real cabinet
    buttons, an arcade stick and a gamepad work. An encoder that types keys goes in the KEYBOARD column.
    Right-click a CONTROLLER cell to clear it.
    Changes work at once and are kept in data\keys.txt. Reset keys (or deleting that file) restores the keys above.

PANEL
  Around seat 1's picture: the cabinet buttons, the formation board and the pages + CARD, KEYS, SETTINGS, CLUB CARD
  and PAUSE.
  Formation board: your cards on a table. Drag to move, drop on a card to swap, right-click to take one off.
  + CARD opens the CATALOGUE of all 3,909 player cards: search, filters, sort, a slider per stat. A click puts a card
  on your table.

PAUSE
  P or PAUSE holds a match for up to 20 seconds: a PAUSED page covers the picture and the sound is muted. P or RESUME
  ends it sooner. The next pause waits 10 seconds. Change the key under KEYS.
  It works only in a match on this PC with no other seat linked and no projector running; otherwise the panel says
  why.

SETTINGS
  NOW           link to the server, club card session, bad endings, last save, latest backup, version.
  NEXT START    WHERE TO PLAY: This PC, or Online with a server's address. Kept in data\panel.txt.
  RESTART NOW   click, then CLICK AGAIN: restarts the game with these choices. Not during a match. If the server is
                unchanged, only your cabinet restarts.
  VIEW          LAYOUT: CABINET (buttons around the game) or COMPACT (larger game, no buttons: use your keys or
                controller). CARD HOVER DELAY. Both take effect at once.
  LOGS          SEND LOGS (click, then CLICK AGAIN) sends this game's logs to the server you play on. Your club card is
                never sent, and your Windows user name and PC name are removed. You get a code: post it in Discord
                with what went wrong. SAVE LOGS saves them to a file. Automatic crash reports: see ONLINE.

CLUB CARD
  Shows your club card: club, manager, contract, league, prize money, card health (session, bad endings, saved
  copies, backups) and the squad (red = injured). It updates at each save.
  A card gets a bad ending when a match ends without a save, for example when seat 1 is closed during a match. At
  2 bad endings the card loses its trade rights.
  YOUR CARDS (more than one club)
    NEW CLUB CARD   puts this club aside and a blank card in the slot. Put it in (I) and the game makes a new club.
    PLAY THIS CLUB  puts that club back in the slot.
    Click, then CLICK AGAIN. Each switch restarts your cabinet (about a minute) and never runs during a match. No
    card is ever deleted; the other clubs are in data\save\cards.
  Each club keeps its own formation board. A new club gets a 4-4-2 with the substitutes on the bench. The board sits
  beside the card file (a .board folder). A table that belongs to no card goes to data\save\boards_unknown.
  CLEAR BAD ENDINGS (under CARD HEALTH, when the card has one): click, then CLICK AGAIN. The game restarts with the
  bad endings at 0; the old card goes to data\save\backup first. Not during a match. Trade rights or money already
  lost stay lost.
  When a contract ends, put the card in again after the last match: the game moves your manager to a new card and you
  make a new club. The old card stays in YOUR CARDS as TRANSFERRED.

QUIT
  After a match the club card comes out by itself (or press I). Then close the game window (X or Alt+F4). Everything
  Forza-WCCF started stops too: the projector, the server, its match engines and helpers. data\logs\run_ended.txt
  lists what was stopped. Closing the projector's window does the same.
  During a match, closing seat 1 ends the match without a save and gives the card a bad ending. The first close only
  shows a warning; close again within 8 seconds to quit anyway. Closing the projector during a match leaves seat 1
  playing.
  "Launcher.exe stop" ends a server started with "Launcher.exe server". "Launcher.exe stop force" ends a stuck game.
  A game window that crashes stops nothing else; the projector keeps running. Its log (data\logs\run_seat1.txt or
  run_projector.txt) says what happened.

ONLINE
  Lobby: pick Online under PLAY. Under Direct address type a server (HOST or HOST:PORT). SHOPS lists its shops: NAME,
  PLAYERS (x/8), PING, LANG and STATE (OPEN, IN MATCH, FULL, CLOSED, NO ANSWER), with the time to the next ENTRY.
  Filters: language, Open only, ping under 150 ms.
  Pick a row and press JOIN to take a seat, then PLAY. LEAVE gives the seat up. If the game or the Launcher closes,
  Rejoin (offered for 75 seconds) takes the same seat back. VS ROOMS is greyed out for 2010-11.
  Keys: Up/Down row, Left/Right or Tab list, Enter JOIN, F5 refresh, Esc LEAVE.
  Gamepad: D-pad or left stick move, A = Enter, B = Esc, X = refresh, LB/RB = list.
  Each PC runs only its own cabinet. Without the lobby: "Launcher.exe remote ADDRESS" (for example
  Launcher.exe remote 192.168.1.20), or Online under NEXT START in SETTINGS. "Launcher.exe local" has its own
  projector.
  Hosting: "Launcher.exe server" starts only the server, in the background; the window that started it closes after
  30 seconds. The PC must allow TCP ports 20002, 20030 and 20050 and UDP port 20040; behind a router, forward them.
  The host can play too: "Launcher.exe remote" with its own address (ipconfig shows it). Logs that players send are
  kept in data\player_logs.
  When an online game ends badly (a crash, or a lost link to the server: Error 3000), Forza-WCCF sends this PC's logs
  to the server, as SEND LOGS does. Your club card is never sent. After a crash a memory snapshot goes along;
  data\dumps keeps the newest 5. If the server cannot be reached, the next start sends them. To turn this off, untick
  Automatic crash reports under LOGS in SETTINGS.

GAME TEXT
  The language at the top right of Launcher.exe picks the game's text: English, Italiano, or the original Japanese
  for Japanese. Close the game first; the change applies at the next PLAY.
  English covers about 7,200 lines of screen text, player names (original Latin spelling, e.g. M.DIARRA), skill
  names, CPU team names, the projector's "Next match" ticker, dates, money in dollars with commas ($3,982,700) and
  the shop name ("Local Shop"). It is built on your PC from your game files, and every line is checked. The original
  files are kept in data\english_backup.
  Still Japanese: country and prefecture names (English ones crash the server), the network-ranking areas, and
  writing inside pictures (logos, titles, the six team stats, OK / BACK).
  Four rare screens show money in two pieces ("$2 million$299000"): a fan event, a golden age, a financial crisis and
  the end of a contract.
  Awards saved before English was on keep their Japanese names.
  Italiano works the same way. Each line is Forza-WCCF's Italian, else the original Italian of the older European
  WCCF, else English; never Japanese. The game's font has no accented letters, so they are written with an
  apostrophe (perche', citta'). Player and CPU team names and supporters' nicknames stay in English.

WHAT SETUP CHANGES   (UNDO SETUP reverses all of it)
  game folder   adds wccfk.dll and points the game's programs at it; adds local\client_user_option.conf (projector)
                and local\ctrl_user_option.conf (server)
  beside it     seat1\  the player cabinet: links to the game folder's files, its own changed copy of
                        client_Release.exe, its own settings
                misc\FlatPanelReader_Emulator\exe\FPR_Emu.exe  the card-table helper the game starts
  Nothing else on the PC changes.

GOOD TO KNOW
  - Club card saves go to a temporary file first, so a crash or power cut cannot leave a broken card. Before the first
    save of each session the card is copied to data\save\backup (the newest 20 are kept; CLUB CARD lists them). To
    restore one, close the game and copy it over data\save\seat1_club.bin.
  - Windows may ask whether control_Release.exe may use the network. Either answer works; everything stays on this PC.
  - Antivirus programs may flag programs\wccfk.dll and programs\inject.exe, which change how the game runs. Their
    source is in source\. If Defender flags a file, do not turn it off: restore the file from Protection history
    only if it came from the official download, and report it:
    https://github.com/Forza-WCCF/Forza-WCCF/issues
  - Run one copy of Forza-WCCF at a time on a PC.
  - If both pictures freeze while the sound goes on, Windows took the graphics device from the game (a display
    change, Ctrl+Alt+Del, an administrator prompt). Close the game window (or "Launcher.exe stop force"), then start
    again.
  - Logs are in data\logs ("Launcher.exe debug" keeps more; the card reader's full log grows about 60 MB an hour).
    The previous run is in data\logs\previous; older runs are zipped in data\logs\archive (about 200 MB, oldest
    removed first). If a game window closes by itself, its log (run_seat1.txt, run_projector.txt or run_server.txt)
    says why. Share that log.
  - If seat 1 closes at the start of a match, the game drew a text it cannot format (a stray %). The panel shows the
    text as it is and writes a "badfmt:" line to seat1\forza_panel.log. Share that line.
  - The game draws 30 pictures a second and syncs them to the screen only when the main monitor runs at exactly
    60 Hz. At other refresh rates movement can look uneven; 60 and 120 Hz are even.

FOLDERS
  programs\   Forza-WCCF's programs: wccfk.dll, forza_panel.dll (the panel), inject.exe (starts the panel in seat 1
              and the projector), forza-server.exe (the server), FPR_Emu.exe (card-table helper)
  fonts\      the panel's fonts and licences (Noto Sans JP, Barlow Condensed, Hack)
  languages\  english\ and italian\: the game's English and Italian text (text and fingerprints only)
  source\     source of every program in programs\ (Rust, source\native) and of Launcher.exe (source\launcher); how they were
              built: source\README.txt
  data\       made on your PC: settings (panel.txt), club card (save\, with backup\ and your other clubs in cards\),
              keys (keys.txt), logs, and cache\ (catalogue and card pictures; rebuilt if deleted)
