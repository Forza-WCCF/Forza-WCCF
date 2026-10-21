#!/bin/bash
# restart.sh TAG - from WSL: keep the last run's logs as $WCCF1718_RUNTIME/evidence/logs-TAG, stage the kit (cargo xtask
# stage 2017-18; its programs, Launcher.exe among them, come from `cargo xtask build` in the same checkout), sync it to
# the runtime and re-run setup (rewrites seat 1's exe and the controller from patches1718.py and records
# setup-hashes.json). It never starts or stops the game: WSL interop would start it without its guard
# (docs/SAFETY.md). Close the game's window first; start it from Windows: Launcher.exe in <runtime>\kit (double-click).
#   WCCF1718_GAME     decrypted game folder (needed by setup1718.py)
#   WCCF1718_RUNTIME  runtime folder (required)
# RETIRED (2026-10-21): setup1718.py needs the Python the staged kit no longer ships. Setup is Rust now (crates/forza/src/setup/e1718.rs),
# run by Launcher.exe. The rest of this script is kept for reference and no longer runs.
echo "restart.sh is retired. Stage with: cargo xtask build && cargo xtask stage 2017-18, copy target/kit/2017-18 to <runtime>\\kit," >&2
echo "then on Windows run: Launcher.exe <runtime>\\game (setup in place on that copy, never on the decrypted original; 'Launcher.exe undo <runtime>\\game' reverts), then Launcher.exe to play." >&2
exit 1
set -eo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RT=${WCCF1718_RUNTIME:?set WCCF1718_RUNTIME to the 2017-18 runtime folder}
E=$RT/evidence
if [ -e "$RT/kit/data/running.json" ]; then
  echo "restart.sh: a run is on ($RT/kit/data/running.json) - close the game's window first" >&2; exit 1
fi
mkdir -p "$E" "$RT/kit"
[ -n "$1" ] && [ -d "$RT/kit/data/logs" ] && rm -rf "$E/logs-$1" && cp -r "$RT/kit/data/logs" "$E/logs-$1"
ROOT=$(cd "$HERE/../../.." && pwd)
# WSL's cargo in its own target dir; the stage keeps the built programs
(cd "$ROOT" && CARGO_TARGET_DIR="$ROOT/target/wsl" cargo xtask stage 2017-18)
for p in Launcher.exe bin/forza-server.exe bin/winmm.dll bin/logowin.exe bin/FPR_Emu.exe overlay/inject.exe \
         overlay/forza_panel.dll overlay/netguard.dll source/build/wccfnet.dll; do
  [ -f "$ROOT/target/kit/2017-18/$p" ] || { echo "restart.sh: $p is missing - run cargo xtask build first" >&2; exit 1; }
done
cp -r "$ROOT/target/kit/2017-18/." "$RT/kit/"
python3 -B "$HERE/setup1718.py" 2>&1 | { grep -v -e CMD.EXE -e UNC -e wsl.localhost || true; } | tail -4
echo "staged and set up; start the game from Windows: Launcher.exe in the runtime's kit folder"
