@echo off
rem Forza-WCCF 2017-18 - set up your copy of the game (once; run it again to check and repair).
rem Drag your decrypted 2017-18 game folder (the one with client_Release.exe) onto this file, or double-click it and answer
rem the question. Setup (scripts\setup.py GAME_FOLDER, run with the bundled python) makes the guarded copy of the game's
rem programs and the card catalogue; "Launcher.exe pictures" then makes the card pictures. "SETUP.bat undo" puts the game
rem folder back the way it was (no pictures then).
rem Launcher.exe's Setup and updates does the same; this file is for a console.
"%~dp0python\python.exe" "%~dp0scripts\setup.py" %*
if errorlevel 1 goto done
if /i "%~1"=="undo" goto done
"%~dp0Launcher.exe" pictures
:done
echo.
pause
