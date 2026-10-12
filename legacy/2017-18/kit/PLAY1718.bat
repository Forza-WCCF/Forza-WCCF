@echo off
rem WCCF 2017-18 on the adapted 2010-11 kit. netguard.dll (secondary network guard) is injected into every game program.
set WCCF_GUARD_DLL=netguard.dll
"%~dp0python\python.exe" "%~dp0scripts\play.py" %*
