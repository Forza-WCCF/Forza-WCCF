#!/bin/bash
# restart.sh OLDTAG NEWTAG - from WSL: stop the kit's run (play.py stop, path-verified), keep its logs as
# $WCCF1718_RUNTIME/evidence/logs-OLDTAG, stage the kit (cargo xtask stage 2017-18; its programs come from a
# `cargo xtask build` on Windows in the same checkout) and sync it to the runtime, re-run setup (rewrites seat 1's exe
# and the controller from patches1718.py), start everything again and keep play.py's output as play-NEWTAG.txt.
#   WCCF1718_GAME     decrypted game folder (needed by setup1718.py)
#   WCCF1718_RUNTIME  runtime folder (default <runtime>)
set -eo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RT=${WCCF1718_RUNTIME:-<runtime>}
E=$RT/evidence
mkdir -p "$E"
if [ -d "$RT/kit/python" ]; then
  (cd "$RT/kit" && cmd.exe /c "python\\python.exe scripts\\play.py stop force" 2>&1 | tail -1)
  [ -d "$RT/kit/data/logs" ] && rm -rf "$E/logs-$1" && cp -r "$RT/kit/data/logs" "$E/logs-$1"
fi
mkdir -p "$RT/kit"
ROOT=$(cd "$HERE/../../.." && pwd)
# WSL's cargo in its own target dir (the Windows build owns target/debug); the stage keeps the Windows-built programs
(cd "$ROOT" && CARGO_TARGET_DIR="$ROOT/target/wsl" cargo xtask stage 2017-18)
for p in bin/winmm.dll bin/logowin.exe bin/FPR_Emu.exe overlay/inject.exe overlay/wccfpanel.dll overlay/netguard.dll \
         source/build/wccfnet.dll; do
  [ -f "$ROOT/target/kit/2017-18/$p" ] || { echo "restart.sh: $p is missing - run cargo xtask build on Windows first" >&2; exit 1; }
done
cp -r "$ROOT/target/kit/2017-18/." "$RT/kit/"
python3 -B "$HERE/setup1718.py" 2>&1 | { grep -v -e CMD.EXE -e UNC -e wsl.localhost || true; } | tail -4
cd "$RT/kit" && timeout 400 cmd.exe /c "set WCCF_GUARD_DLL=netguard.dll&& python\\python.exe scripts\\play.py local" 2>&1 | tee "$E/play-$2.txt" | head -12
