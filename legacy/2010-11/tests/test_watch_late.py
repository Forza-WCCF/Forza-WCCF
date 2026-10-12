# -*- coding: utf-8 -*-
"""test_watch_late.py - Launcher.exe's watcher and a game window closed while play.py is still starting (check.ps1 runs it
after build.ps1; no game).  Kit 5.5.1's watcher waited only for the launchers still running when it began, so a window
closed during the start (play.py waits up to 60 s for the overlay) left the server and the helpers running.
A scratch kit: the built Launcher.exe, the kit's Python and scripts; its play.py starts sleepers as the run's launchers,
logs "its window was closed" for them, and writes data\\running.json only after they have ended, as a slow start does.
Each case must end with the rest stopped: every window gone, or seat 1 gone while the projector still runs - from a
console (Launcher.exe leaves a watcher process) and from Launcher.exe's window (its own process watches, then ends).
    python source\\test_watch_late.py"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

KIT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(KIT, "scripts"))
import kit_common as K  # noqa: E402

STAND_IN = r'''import os, subprocess, sys, time
import play_real as play
if sys.argv[1:] == ["ended"]:
    sys.exit(play.ended())
roles = {}
for role, life, log in [("seat 1 launcher", 1, "run_seat1.txt"),
                        ("projector launcher", 20 if sys.argv[1:] == ["late-seat"] else 1, "run_projector.txt"),
                        ("scene service", 60, None)]:
    p = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(%d)" % life], creationflags=0x08000000)
    roles[p.pid] = role
    if log:
        with open(os.path.join(play.K.LOGS, log), "w") as f:
            f.write("  t+  1.0s %s - ending client_Release.exe\n" % play.WINDOW_CLOSED)
time.sleep(3)                       # still starting (the overlay) when the launchers end
play.write_running(roles, "local", None)
'''


def case(name, windowed=False):
    root = tempfile.mkdtemp(prefix="wccf-watch-late-")
    try:
        for d in ("data\\logs", "extracted", "seat1", "overlay"):
            os.makedirs(os.path.join(root, d))
        shutil.copy2(os.path.join(KIT, "Launcher.exe"), root)
        shutil.copytree(os.path.join(KIT, "python"), os.path.join(root, "python"))
        shutil.copytree(os.path.join(KIT, "scripts"), os.path.join(root, "scripts"),
                        ignore=shutil.ignore_patterns("__pycache__"))
        os.replace(os.path.join(root, "scripts", "play.py"), os.path.join(root, "scripts", "play_real.py"))
        with open(os.path.join(root, "scripts", "play.py"), "w", encoding="utf-8") as f:
            f.write(STAND_IN)
        for exe in ("client_Release.exe", "control_Release.exe", "match_Release.exe"):
            with open(os.path.join(root, "extracted", exe), "wb") as f:
                f.write(b"a marker, never run")
        with open(os.path.join(root, "seat1", ".wccf-kit"), "w") as f:
            f.write("scratch")
        open(os.path.join(root, "overlay", "catalogue.tsv"), "w").close()
        with open(os.path.join(root, "data", "settings.json"), "w") as f:
            json.dump({"game": os.path.join(root, "extracted")}, f)

        ended = os.path.join(root, "data", "logs", "run_ended.txt")
        # from a console Launcher.exe prints there and leaves a watcher process; without one (a shortcut, Explorer) it
        # is a window that goes once the game is up, its process watching the run, then ending with it
        detached = 0x00000008 if windowed else 0             # DETACHED_PROCESS: no console to join
        p = subprocess.Popen([os.path.join(root, "Launcher.exe"), name], cwd=root, stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=detached)
        if not windowed:
            p.wait(timeout=60)
        end = time.time() + 30
        text = ""
        while time.time() < end and "Stopped (" not in text:
            time.sleep(0.5)
            try:
                with open(ended, encoding="utf-8", errors="replace") as f:
                    text = f.read()
            except OSError:
                pass
        assert "Stopped (" in text, "%s: the watcher stopped nothing (run_ended.txt: %r)" % (name, text)
        if windowed:
            p.wait(timeout=15)        # the window's process ends with the run it watched
        mine = os.path.normcase(root) + os.sep
        end = time.time() + 10        # play.py ended and the watcher are still ending after its "Stopped" line
        while True:
            left = [q for q in K.processes() if os.path.normcase(q[3]).startswith(mine)]
            if not left or time.time() > end:
                break
            time.sleep(0.5)
        assert not left, "%s: left running: %s" % (name, left)
        assert not os.path.exists(os.path.join(root, "data", "running.json")), name
    finally:
        mine = os.path.normcase(root) + os.sep
        for p in K.processes():
            if os.path.normcase(p[3]).startswith(mine):
                K.kill(p[0])
        time.sleep(0.5)
        shutil.rmtree(root, ignore_errors=True)


case("late-all")     # every window closed during the start
case("late-seat")    # seat 1 closed during the start, the projector still up
case("late-all", windowed=True)     # Launcher.exe's window: it goes, its process watches and ends with the run
print("watch: a window closed during the start stops the rest (3 checks passed)")
