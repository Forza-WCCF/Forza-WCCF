# -*- coding: utf-8 -*-
"""test_junction_paths.py - a program started through a junction is found by the kit's path checks (cargo xtask check
runs it; nothing live, no game). Windows reports such a program under the junction's target, so kit_common.same_form of
the junction path must equal the path kit_common.processes reads (2026-10-12: a kit started through its first-run link
C:\\Games\\Forza-WCCF\\2010-2011 could not stop its own helpers). A copy of cmd.exe waiting on its input stands in.
    python source\\test_junction_paths.py"""
import os
import shutil
import subprocess
import sys
import tempfile
import time

KIT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(KIT, "code"))
import kit_common as K  # noqa: E402

CREATE_NO_WINDOW = 0x08000000


def main():
    top = tempfile.mkdtemp(prefix="forza-junction-")   # %TEMP%: a local NTFS drive, where junctions work
    real, link = os.path.join(top, "real"), os.path.join(top, "link")
    os.mkdir(real)
    shutil.copy(os.path.join(os.environ["SystemRoot"], "System32", "cmd.exe"), real)
    made = subprocess.run(["cmd", "/c", "mklink", "/J", link, real], capture_output=True, creationflags=CREATE_NO_WINDOW)
    assert made.returncode == 0, made.stderr
    child = subprocess.Popen([os.path.join(link, "cmd.exe")], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                             creationflags=CREATE_NO_WINDOW)
    try:
        time.sleep(0.5)
        seen = [p[3] for p in K.processes() if p[0] == child.pid]
        assert seen and seen[0], "the stand-in's path could not be read"
        want = K.same_form(os.path.join(link, "cmd.exe"))
        assert os.path.normcase(seen[0]) == want, (seen[0], want)
        print("junction paths: a program started through %s is found as %s" % (link, seen[0]))
    finally:
        child.kill()
        child.wait()
        os.rmdir(link)                                  # the junction only, never its target's files
        shutil.rmtree(top, ignore_errors=True)


if __name__ == "__main__":
    main()
