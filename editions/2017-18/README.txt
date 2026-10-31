Forza-WCCF 2017-18 (alpha)
==========================
(Italiano: README.it.txt - 日本語: README.ja.txt)

Alpha: playable, unfinished, needs testers. Read NOT IN THIS ALPHA and KNOWN ISSUES first.

WHAT IT IS
  World Club Champion Football 2017-18 on one Windows PC, one seat, from your own game files. Online VS against
  another home is optional. Forza-WCCF stands in for the arcade's hardware and servers: the game server (your club and
  saves), network and keychip, Aime card reader and I/O board. A panel around the game window has the cabinet's
  buttons and a formation board for your cards. Forza-WCCF holds no original files: everything comes from your copy.

YOU NEED
  - Windows 10 or 11, 64-bit.
  - Your own decrypted 2017-18 game folder (the one with client_Release.exe and control_Release.exe). The game is not
    included.

INSTALL (once)
  1. Unzip this download into a folder of its own with a plain path, e.g. C:\Games\Forza-WCCF-2017-18.
  2. Run Launcher.exe. If Windows warns about an unsigned program, see ANTIVIRUS AND SMARTSCREEN.
  3. Pick your language at the top right: English, Italiano or 日本語.
  4. Open SETUP, pick the game folder (Browse, or drag it in) and press SET UP. It takes about a minute. Your game
     folder is not changed; the original programs are kept in original\. SET UP again checks and repairs;
     UNDO SETUP reverses it.
  The 2010-11 and 2017-18 installs are separate, but they use the same local ports: close one before you start the
  other.

LAUNCHER
  PLAY      WHERE TO PLAY (This PC or Online), then PLAY. STOP THE GAME closes the game (during a match: CLICK AGAIN).
  SETUP     game folder, SET UP, UNDO SETUP; CARD PICTURES (From the game, or High-quality pack with Choose pack);
            Skip the tutorial.
  UPDATE    lists the published releases: pick one, then UPDATE. The game is set up again by itself; your clubs and
            settings are kept.
  DETAILS   what the Launcher did, with Copy and SAVE LOGS.
  The language at the top right sets the Launcher and the game's text: English, Italiano, or the original Japanese
  for 日本語. The game follows it at the next PLAY.

PLAY
  Press PLAY. The game starts in about 30 seconds. The Aime card is emulated. Your club and saves are in
  data\save\gamesvr: back that folder up.
  A new card starts with the manager licence, a 5-play tutorial and the club set-up (about 30 minutes). To skip them,
  tick Skip the tutorial under SETUP before PLAY: a new card gets a ready club and goes to the main screen.
  Put your card in soon after the start. After a match take it out (I), then close the game window.

KEYS
  Click the game window first.
      Enter   START (free play: the coin goes in for you)
      X       PRESS (decide)       C   SHOOT        B   KEEPER
      S       DATA                 D   KEY PLAYER
      arrows  tactics: Up CENTRAL, Down COUNTER, Left L-SIDE, Right R-SIDE
      I       Aime card in / out   5   coin
      P       pause a match
  The panel's buttons do the same: the triangles (CENTRAL, COUNTER, L-SIDE, R-SIDE), PRESS, DATA, SHOOT, KEEPER,
  KEY PLAYER and START. Change keys and controller buttons under KEYS.
  Formation board: drag a card to move it, drop it on another to swap. + CARD opens the CATALOGUE.
  Rare 2D-code cards work.
  Pause (P or PAUSE) holds a match for up to 20 seconds; P or RESUME resumes sooner. The next pause waits
  10 seconds. No pause online.

ONLINE
  1. Pick Online under PLAY. Open Direct address and type the server's address (HOST or HOST:PORT). The project's
     server only admits addresses on its allow list: ask its operator to add yours. In ARCADES, HOST ARCADE opens
     an arcade and shows its code; friends type it in Room code, or JOIN a listed arcade room. Arcade name, under
     HOST ARCADE, is the name others see in the list.
  2. HOST VS makes a room and shows its code (e.g. K7QM). Your friend picks that room and presses JOIN.
     QUICK MATCH joins the oldest open room, or opens one. LEAVE (Esc) gives up the seat.
  3. Both players press READY; the game starts after a 5-second countdown. If nobody joins within about 90 seconds,
     you play a CPU team.
  Lobby keys: Up/Down row, Left/Right or Tab list, Enter JOIN or READY, H HOST VS, F5 refresh, Esc LEAVE.
  Gamepad: D-pad or left stick move, A = Enter, B = Esc, X = refresh, Y = HOST VS, LB/RB = list.
  Under Direct address:
    Club server     where your club is kept. Empty: the server you chose.
    Direct VS link  connects your PC to the other player's (relay.wccf.it). Off: VS runs through the relay.
  Windows Firewall may ask about Launcher.exe the first time. Allow it; if you deny it, VS runs through the relay
  with more delay. Online, every card has its own Aime ID, so two homes never share a club.
  Pick This PC to go back to your club on this PC.

NOT IN THIS ALPHA
  - Career mode.
  - A render scale.

KNOWN ISSUES
  - The control program may crash after about 90 minutes idle. Restart the game; your club is kept.
  - Registering a team may wait about 95 seconds before it goes on.
  - An online match may drop to a CPU opponent after about 2 minutes, most often between AMD and Intel PCs.

ANTIVIRUS AND SMARTSCREEN
  The programs are unsigned, and Forza-WCCF changes copies of the game's programs (never the originals, kept in
  original\). Antivirus programs may treat this like a cheat tool and flag bin\winmm.dll. SOURCE.txt links its source.
  Each release is reported to Microsoft as a false positive before it is published.
  SmartScreen ("Windows protected your PC"): choose More info, then Run anyway.
  Defender flags a file: do not turn Defender off. Note the detection name. If the file came from the official
  download, restore it from Protection history. Report it: https://github.com/Forza-WCCF/Forza-WCCF/issues

SAFETY
  Forza-WCCF never changes this PC's network settings (addresses, routes, firewall), and the game never reaches the
  publisher's hosts: they go to stand-ins on this PC. Always start the game from Launcher.exe; it refuses to start a
  copy that SET UP did not make. If Windows asks whether control_Release.exe may use the network, either answer works.

LOGS AND BUGS
  Logs are in data\logs; the previous run's are in data\logs\previous. To report a bug, send the logs and say what
  you did and what you saw: Report a problem in Launcher.exe or SAVE LOGS in the panel's SETTINGS. Both save
  forza-wccf-logs-DATE-TIME.zip in your Downloads folder and show where; send that file to whoever asked for
  it. Nothing is uploaded. For a Defender or SmartScreen warning, add the file name and the warning's text.

FOLDERS
  bin\       Forza-WCCF's programs: the game server (forza-server.exe), the stand-ins, the card-table helper
  overlay\   the panel; SET UP adds the catalogue and the card pictures
  fonts\     the panel's fonts and their licences
  english\, italian\   the game's English and Italian text
  data\      made on your PC: settings, saves (save\gamesvr), logs
