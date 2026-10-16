#!/bin/bash
# cargo's runner for the Windows test programs under WSL (xtask's `rust()`). Most run as WSL starts them (interop).
# The launcher's run tests do not: wslhost.exe starts programs with "Prefer System32 images", which the run refuses
# (AGENTS.md section 5), and every program they start inherits it. They go through explorer.exe instead, as the game
# itself is started, from the same working folder; their output and exit code come back through files.
exe=$1
shift
case $(basename "$exe") in
run-*.exe) ;;
*) exec "$exe" "$@" ;;
esac
mkdir -p <build-dir>/winrun  # a local NTFS folder: explorer.exe will not start a .bat from \\wsl.localhost
out=$(mktemp -d <build-dir>/winrun/run.XXXXXX)
w() { wslpath -w "$1"; }
printf '@echo off\r\npushd "%s"\r\n"%s" %s > "%s" 2>&1\r\necho %%ERRORLEVEL%% > "%s"\r\n' \
  "$(w "$PWD")" "$(w "$exe")" "$*" "$(w "$out")\\log.txt" "$(w "$out")\\exit.txt" > "$out/run.bat"
(cd /mnt/c && /mnt/c/Windows/explorer.exe "$(w "$out/run.bat")") || true
for _ in $(seq 1200); do [ -s "$out/exit.txt" ] && break; sleep 0.5; done
tr -d '\r' < "$out/log.txt" 2>/dev/null
code=$(tr -dc '0-9' < "$out/exit.txt" 2>/dev/null)
rm -rf "$out"
exit "${code:-1}"
