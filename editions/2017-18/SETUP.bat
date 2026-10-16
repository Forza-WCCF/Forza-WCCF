@echo off
rem Forza-WCCF 2017-18 - set up your copy of the game (once; run it again to check and repair).
rem Drag your decrypted 2017-18 game folder (the one with client_Release.exe) onto this file, or double-click it and answer
rem the question. Setup (scripts\setup.py GAME_FOLDER, run with the bundled python) makes the guarded copy of the game's
rem programs, the card catalogue and the card pictures. "SETUP.bat undo" puts the game folder back the way it was.
rem Launcher.exe's Setup and updates does the same; this file is for a console.
"%~dp0python\python.exe" "%~dp0scripts\setup.py" %*
echo.
pause
