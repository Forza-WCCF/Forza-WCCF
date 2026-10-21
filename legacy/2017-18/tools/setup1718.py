#!/usr/bin/env python3
"""Dev wrapper for the WSL flow: runs the kit's setup.py (the player's setup) on a runtime folder.

    WCCF1718_GAME=/path/to/decrypted/game  python3 legacy/2017-18/tools/setup1718.py   (needs target/kit/2017-18)

    WCCF1718_GAME     the untouched decrypted game; its two programs seed <runtime>\\original\\ when it is missing
    WCCF1718_RUNTIME  the runtime folder on an NTFS drive (required), with a copy of the game in game\\
The work is kit\\scripts\\setup.py, run by the staged kit's Windows Python through WSL interop.
"""
import os
import shutil
import subprocess
import sys
from pathlib import Path

SRC = Path(os.environ.get("WCCF1718_GAME") or sys.exit("set WCCF1718_GAME to the decrypted 2017-18 game folder"))
RT = Path(os.environ.get("WCCF1718_RUNTIME") or sys.exit("set WCCF1718_RUNTIME to the 2017-18 runtime folder"))
KIT = Path(__file__).resolve().parents[3] / "target" / "kit" / "2017-18"

(RT / "original").mkdir(exist_ok=True)
for exe in ("client_Release.exe", "control_Release.exe"):
    if not (RT / "original" / exe).exists():
        shutil.copyfile(SRC / exe, RT / "original" / exe)
game = subprocess.check_output(["wslpath", "-w", str(RT / "game")], text=True).strip()
sys.exit(subprocess.call([str(KIT / "python" / "python.exe"), "scripts\\setup.py", game], cwd=KIT))
