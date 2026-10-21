Forza-WCCF 2017-18 - 0.2.0 (alpha)
==================================
(Italiano: README.it.txt - 日本語: README.ja.txt)

Alpha: playable, unfinished, needs testers. Read WHAT IS NOT IN THIS ALPHA and KNOWN ISSUES first.

NEW IN THIS RELEASE (not released yet)
  - No Python: setup, the card catalogue, the card pictures and the game's text run inside Launcher.exe. The download
    is smaller (no python\ or scripts\ folder).
  - Online play. Choose Online in Launcher.exe; the LOBBY lists VS ROOMS. HOST VS makes a room with a 4-letter code,
    QUICK MATCH takes the oldest open room (or opens one), both players press READY, a 5-second countdown starts the
    game. Keyboard and gamepad.
  - Online, every card has its own Aime ID: two homes never share one club.
  - The Club server and Direct VS link rows moved under ONLINE, in the "Direct address" fold.
  - Pause: P (or the PAUSE button) holds a match for up to 20 seconds.
  - The installer's package step takes this edition's ZIP (see the installer's README).

WHAT IT IS
  World Club Champion Football 2017-18 on one Windows PC, one seat, from YOUR OWN game files. Online VS with another
  home is optional. Forza-WCCF replaces the arcade's hardware and servers: game server (your club and saves), network
  and keychip, Aime card reader, I/O board. A panel around the game window has the cabinet's buttons and a formation
  board for your cards. Forza-WCCF holds no original files; everything from the game is made from your copy.

YOU NEED
  - Windows 10 or 11, 64-bit.
  - Your own decrypted 2017-18 game folder (the one with client_Release.exe and control_Release.exe).
    Forza-WCCF does not include the game. You need your own copy of the original game files.
  - Nothing else (no WSL).

INSTALL (once)
  1. Unzip this download into a folder of its own, with a plain path (e.g. C:\Games\Forza-WCCF-2017-18).
  2. Run Launcher.exe. Windows may warn about an unsigned program: see KNOWN ISSUES.
  3. Open Setup and updates, choose your game folder, press Set up (about a minute). Setup makes a guarded copy of the
     game's programs, the card catalogue and the card pictures. Your game folder is not changed; the original programs
     are kept in original\. Running Setup again checks and repairs.
  The 2010-11 and 2017-18 installs are separate: own game files, data, logs and saves. They use the same local ports,
  so close one before you start the other.

PLAY
  Press PLAY in Launcher.exe. The game starts in about 30 seconds. The Aime card is emulated. Your club and saves are
  in data\save\gamesvr (back it up). Game text: English, Italiano or the original Japanese; pick it in Launcher.exe
  before PLAY.
  A new card starts with the manager licence, a 5-play tutorial and the club set-up (about 30 minutes). To skip them,
  tick "Skip the tutorial" in Launcher.exe before PLAY: a new card gets a ready club and goes to the main screen.
  Click the game window, then:
      Enter   START (free play: a coin goes in for you)
      X       PRESS (decide)       C   SHOOT        B   KEEPER
      S       DATA                 D   KEY PLAYER
      arrows  tactics: Up CENTRAL, Down COUNTER, Left L-SIDE, Right R-SIDE
      I       Aime card in / out   5   a coin        F1  test menu
  or click the panel's buttons: the triangles (CENTRAL, COUNTER, L-SIDE, R-SIDE), PRESS, DATA, SHOOT, KEEPER,
  KEY PLAYER and START. The panel's FORMATION BOARD shows your cards: drag one to move it, drop it on another to swap.
  Rare 2D-code cards work (the game server lists them as registered).
  Put your card in soon after starting; take it out (I) after a match, then close the game window.
  P       pause a match (20 seconds at most, then it resumes; P or RESUME resumes sooner; the next pause waits
          10 seconds). Not online, not with another seat linked, not while the projector runs. Rebind under KEYS.

ONLINE (VS between two homes)
  1. In Launcher.exe pick Online. Open "Direct address" and type the server's address (HOST or HOST:PORT); the
     lobby asks every address you typed. The project's server admits only addresses on its allow list: ask its
     operator to add yours. SHOPS stays greyed for 2017-18; use VS ROOMS.
  2. HOST VS makes a room and shows its code (e.g. K7QM). Tell your friend, who picks that room and presses JOIN.
     QUICK MATCH joins the oldest open room, or opens one. LEAVE (Esc) gives up the seat.
  3. Each player presses READY; a 5-second countdown starts the game. If you are ready and nobody joins within about
     90 seconds, the game starts with a CPU team.
  4. Lobby keys: Up/Down row, Left/Right or Tab the list, Enter join or READY, H HOST VS, F5 refresh, Esc leave.
     Gamepad: D-pad or left stick move, A = Enter, B = Esc, X = refresh, Y = HOST VS, LB/RB = the list.
  "Club server" (under Direct address) is where your club is kept; empty means the server you chose. "Direct VS
  link" (relay.wccf.it) connects your PC to the other player's; untick it to turn it off. Windows Firewall may ask
  about Launcher.exe the first time: allow it for direct connections. If you deny it, VS runs through the relay with
  more delay. Pick This PC to return to your own club on this PC.

WHAT IS NOT IN THIS ALPHA
  - Career mode.
  - LAN play and shops with several seats: one seat per PC. Online is VS between two homes only.
  - A render scale.

KNOWN ISSUES
  - The control program may crash after about 90 minutes idle. Restart the game; your club is kept.
  - Registering a team may wait about 95 seconds before it goes on.
  - Online VS between two homes has not been played end to end on two PCs yet; treat it as a test.
  - The programs are unsigned. For SmartScreen and Defender see ANTIVIRUS AND SMARTSCREEN.
  - Antivirus programs may flag bin\winmm.dll (sits between
    the game and Windows). Its source is in source\.

ANTIVIRUS AND SMARTSCREEN
  The programs are unsigned. Launcher.exe starts the game's programs under the Windows debugging interface and loads
  two Forza-WCCF DLLs into them: forza_panel.dll draws the panel on the game's frame (the game loads it through
  wccfnet.dll); the guarded copies' wccfnet.dll stubs the game's network-adapter calls, so the game cannot drop this
  PC's network lease. Setup
  patches copies of the game's programs, never the originals (backed up in original\). Antivirus programs may treat
  this like a cheat tool. Each release's files are submitted to Microsoft as a false positive before publication.
  SmartScreen ("Windows protected your PC"): choose "More info", then "Run anyway".
  If Defender flags a file: do not turn Defender off. Note the detection name, restore the file from Protection
  history only if it came from the official download, and report it: https://github.com/Forza-WCCF/Forza-WCCF/issues

SAFETY
  Forza-WCCF never changes this PC's network settings (addresses, routes, firewall). A guard replaces the game's own
  network calls before it starts; Launcher.exe refuses to start the game without the guard. The game never reaches the
  publisher's hosts: they are redirected to stand-ins on this PC. If Windows asks whether control_Release.exe may use
  the network, either answer works.

LOGS AND BUGS
  Logs are in data\logs; the previous run's are in data\logs\previous. To report a bug, send the logs and say what you
  did and what you saw. The panel's SETTINGS has SEND LOGS (needs a server address); otherwise zip data\logs and send
  it by hand. For a Defender or SmartScreen warning, add the file name and the warning's text.

FOLDERS
  bin\       winmm.dll (the hook), FPR_Emu.exe (card-table helper), logowin.exe (stand-in), forza-server.exe
  overlay\   forza_panel.dll (the panel); setup adds the catalogue
             and the card pictures
  fonts\     the panel's fonts and their licences
  english\, italian\   the game's English and Italian text - text only
  source\    the C source of the programs in bin\ and overlay\
  data\      made on your PC: settings, saves (save\gamesvr), logs
