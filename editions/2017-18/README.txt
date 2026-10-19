Forza-WCCF 2017-18 - 0.1.0-alpha.1
==================================
(Italiano: README.it.txt - 日本語: README.ja.txt)

This is an alpha: it plays, it is not finished, and it needs testers. Read WHAT IS NOT IN THIS ALPHA and KNOWN ISSUES
before you start.

WHAT IT IS
  World Club Champion Football 2017-18 on one Windows PC, offline, one seat, from YOUR OWN game files. Forza-WCCF
  stands in for the arcade's hardware and servers: the game server (your club and its saves), the network and keychip,
  the Aime card reader, the I/O board. Around the game window there is a panel with the cabinet's buttons and a
  formation board for your cards. Forza-WCCF holds none of the original files: everything that comes from the game is
  made from your copy.

YOU NEED
  - Windows 10 or 11, 64-bit.
  - Your own decrypted 2017-18 game folder (the one with client_Release.exe and control_Release.exe).
    <where to get the game: owner fills in>
  - Nothing else: no WSL, no Python install (Python is inside).

INSTALL (once)
  1. Unzip this download into a folder of its own, with a plain path (e.g. C:\Games\Forza-WCCF-2017-18).
  2. Run Launcher.exe. Windows may warn about an unsigned program: see KNOWN ISSUES.
  3. Open Setup and updates, choose your game folder, press Set up. Setup makes a guarded copy of the game's
     programs (your original folder is not changed; the original programs are kept in original\), the card catalogue and the card pictures; it takes about a
     minute. Running it again checks and repairs.
  The 2010-11 and 2017-18 installs stay separate, each with its own game files, data, logs and saves. They cannot run
  at the same time on one PC: both use the same local ports. Close one before you start the other.

PLAY
  Press PLAY in Launcher.exe. The game takes about 30 seconds to start. Your Aime card is emulated; your club and its
  saves are kept on this PC, in data\save\gamesvr (back that folder up). Game text: English, Italiano or the original
  Japanese - pick it in Launcher.exe before PLAY.
  First time: a new card starts with the manager licence, a tutorial of 5 plays and the club set-up, about 30 minutes.
  Tick the "Skip the tutorial" box in Launcher.exe before PLAY: a new card then gets a ready club and goes straight to the main screen.
  Click the game window, then:
      Enter   START (free play: a coin goes in for you)
      X       PRESS (decide)       C   SHOOT        B   KEEPER
      S       DATA                 D   KEY PLAYER
      arrows  tactics: Up CENTRAL, Down COUNTER, Left L-SIDE, Right R-SIDE
      I       Aime card in / out   5   a coin        F1  test menu
  or click the panel's buttons: the triangles (CENTRAL, COUNTER, L-SIDE, R-SIDE), PRESS, DATA, SHOOT, KEEPER,
  KEY PLAYER and START. The panel's FORMATION BOARD shows your cards: drag one to move it, drop it on another to swap.
  Rare 2D-code cards work: the game server lists them as registered.
  Put your card in soon after starting; take it out (I) after a match, then close the game window.
  Online VS (with a club server set in Launcher.exe): "Direct VS link" connects you to the other player's PC. The first time,
  Windows Firewall may ask about Launcher.exe: allow it for direct connections; if you deny it, VS still works through
  the relay (relay.wccf.it), only with more delay. Untick the box to turn the link off.

WHAT IS NOT IN THIS ALPHA
  - Career mode.
  - Online or LAN play: one PC, one seat.
  - Installer support for 2017-18: install by unzipping, as above.
  - Pause, and a render scale.

KNOWN ISSUES
  - The control program may crash after about 90 minutes idle. Close the game and start again; your club is kept.
  - Registering a team may wait about 95 seconds before it goes on.
  - The programs are not signed. Windows SmartScreen may say "Windows protected your PC": choose "More info", then
    "Run anyway". If Microsoft Defender flags a file, do not turn Defender off: report the file name and what
    Defender said (see BUGS).
  - Antivirus programs may dislike overlay\inject.exe (it loads the panel into the game) and bin\winmm.dll (it sits
    between the game and Windows). Their source code is in source\.

ANTIVIRUS AND SMARTSCREEN
  The programs are unsigned. To work, Launcher.exe starts the game's programs under the Windows debugging interface
  and loads two of Forza-WCCF's DLLs into them: forza_panel.dll draws the panel on the game's frame; netguard.dll and
  the guarded copies' wccfnet.dll stub the game's network-adapter calls, so the game cannot drop this PC's network
  lease. Setup patches copies of the game's programs, never the originals, which it backs up in original\. Antivirus
  programs may treat this like a cheat tool. Each release's files are submitted to Microsoft as a false positive before
  it is published.
  If Defender still flags a file: do not turn Defender off. Note the detection name, restore the file from Protection
  history only if it came from the official download, and report it: <report link: owner fills in>
  SmartScreen: choose "More info", then "Run anyway".

SAFETY
  Forza-WCCF never changes this PC's network settings (addresses, routes, firewall). The game's own network calls are
  replaced by a guard before the game starts, and Launcher.exe refuses to start the game if the guard is not in place.
  The game never reaches the publisher's hosts: they are redirected to stand-ins on this PC. Windows may ask whether
  control_Release.exe may use the network: either answer works, everything stays on this PC.

LOGS AND BUGS
  Logs are in data\logs; the previous run's are in data\logs\previous. To report a bug, send the logs: the panel's
  SETTINGS has SEND LOGS (it needs a server address to send to; otherwise zip data\logs and send it by hand), and say
  what you did and what you saw. For a Defender or SmartScreen warning, add the file name and the warning's text.

FOLDERS
  bin\       winmm.dll (the hook), FPR_Emu.exe (card-table helper), logowin.exe (stand-in), forza-server.exe
  overlay\   forza_panel.dll (the panel), inject.exe, netguard.dll (network guard), skin.tex; setup adds the catalogue
             and the card pictures
  fonts\     the panel's fonts and their licences
  scripts\   setup, the card maker and the English/Italian text tools
  english\, italian\   the game's English and Italian text - text only
  source\    the C source of the programs in bin\ and overlay\
  python\    Python 3.13 with Pillow
  data\      made on your PC: settings, saves (save\gamesvr), logs
