@echo off
rem WCCF 2017-18 kit - set up your copy of the game (once; run it again to check and repair).
rem Drag your copy of the decrypted game folder onto this file, or double-click it and answer the question.
"%~dp0python\python.exe" "%~dp0scripts\setup.py" %*
echo.
pause
