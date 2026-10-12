#!/bin/bash
# restart.sh OLDTAG NEWTAG - from WSL: stop the kit's run (play.py stop, path-verified), keep its logs as
# $WCCF1718_RUNTIME/evidence/logs-OLDTAG, stage the kit (cargo xtask build) and sync it to the runtime, re-run setup (rewrites seat 1's exe
# and the controller from patches1718.py), start everything again and keep play.py's output as play-NEWTAG.txt.
#   WCCF1718_GAME     decrypted game folder (needed by setup1718.py)
#   WCCF1718_RUNTIME  runtime folder (default <runtime>)
set -e
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
(cd "$ROOT" && cargo xtask build)
cp -r "$ROOT/target/kit/2017-18/." "$RT/kit/"
python3 -B "$HERE/setup1718.py" 2>&1 | grep -v -e CMD.EXE -e UNC -e wsl.localhost | grep seat1/
cd "$RT/kit" && timeout 400 cmd.exe /c "set WCCF_GUARD_DLL=netguard.dll&& python\\python.exe scripts\\play.py local" 2>&1 | tee "$E/play-$2.txt" | head -12
