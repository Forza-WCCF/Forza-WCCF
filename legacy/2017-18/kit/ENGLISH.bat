@echo off
rem WCCF 2017-18 kit - the game in English, built from your own game files (the originals are kept in data\english_backup).
rem "ENGLISH.bat off" puts the original Japanese files back; "ENGLISH.bat check" only says what it would do.
"%~dp0python\python.exe" "%~dp0scripts\english.py" %*
echo.
pause
