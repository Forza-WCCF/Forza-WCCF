# -*- coding: utf-8 -*-
"""rec_b_scenes.py - records code/hardware/scenes.py (the xscnWatch scene service stand-in) on scratch names with a
requester that plays a match engine (it takes the mutex, writes slots, reads the board and the scene mappings) into
crates/forza/tests/rec/scenes.jsonl, which crates/forza/tests/scenes.rs replays against forza::standins::scenes.
Step 10 piece B.  KEPT UNTIL PIECE G: play.py still starts the Python service; piece G deletes scenes.py and this
recorder together.
Synthetic only: scene files are generated (formula below), in a temporary folder; the mutex, board, marker and mapping
names carry this process's pid (never xscnWatch::*, WCCF::xscnWatchStandIn or the game's %06d.xscn).  The service runs
from a copy of scenes.py with those names substituted.  Lines of the file:
  {"t":"scenario","name","files":{file name: [seed, size]},"pre":[[offset, dword]..]}  fresh folder; "pre" is written
                                          to a board the requester created before the service started (adoption)
  {"t":"step","name","writes":[[offset, dword]..],"open":[scene..],"board":hex,"maps":{scene: [size, sha256] | null},
   "log":[..]}                            writes under the mutex, then the board once it stops changing, the mappings
                                          the requester can open, and the service's log lines since the last step
                                          (timestamp cut; the folder is <dir>, the run length and the name prefix cut)
  {"t":"refused","log":[..]}              a second copy: exit code 2
File content: byte i = (i * 7 + seed * 13 + (i >> 8)) & 255.
    python legacy\\2010-11\\tests\\rec_b_scenes.py"""
import ctypes
import ctypes.wintypes as w
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
CODE = os.path.join(os.path.dirname(HERE), "code", "hardware")
OUT = os.path.join(REPO, "crates", "forza", "tests", "rec", "scenes.jsonl")
GAMEDIR = ""
PFX = "REC%d_" % os.getpid()
MUTEX, BOARD, MARKER = PFX + "Mutex", PFX + "ComBuf", PFX + "StandIn"
BOARD_SIZE = 0x5CC

k = ctypes.windll.kernel32
k.CreateMutexW.restype = k.OpenMutexW.restype = w.HANDLE
k.CreateMutexW.argtypes = [ctypes.c_void_p, w.BOOL, w.LPCWSTR]
k.OpenMutexW.argtypes = [w.DWORD, w.BOOL, w.LPCWSTR]
k.CreateFileMappingW.restype = k.OpenFileMappingW.restype = w.HANDLE
k.CreateFileMappingW.argtypes = [w.HANDLE, ctypes.c_void_p, w.DWORD, w.DWORD, w.DWORD, w.LPCWSTR]
k.OpenFileMappingW.argtypes = [w.DWORD, w.BOOL, w.LPCWSTR]
k.MapViewOfFile.restype = ctypes.c_void_p
k.MapViewOfFile.argtypes = [w.HANDLE, w.DWORD, w.DWORD, w.DWORD, ctypes.c_size_t]
k.UnmapViewOfFile.argtypes = [ctypes.c_void_p]
k.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
k.WaitForSingleObject.restype = w.DWORD
k.ReleaseMutex.argtypes = [w.HANDLE]
k.CloseHandle.argtypes = [w.HANDLE]
INVALID = w.HANDLE(-1).value
SYNCHRONIZE, FILE_MAP_ALL_ACCESS, PAGE_READWRITE = 0x100000, 0xF001F, 4


def content(seed, size):
    return bytes((i * 7 + seed * 13 + (i >> 8)) & 255 for i in range(size))


FILES = {  # file name -> (seed, size)
    "471_Big.xscn": (471, 3_000_000), "12_a.xscn": (12, 1000), "012_dup.xscn": (1, 10), "13_empty.xscn": (13, 0),
    "14_c.xscn": (14, 65537), "0099.XSCN": (99, 77), "readme.txt": (0, 5), "5_notscene.bin": (5, 5), "abc.xscn": (0, 5),
}
for n in range(100, 148):
    FILES["%d_s.xscn" % n] = (n, 100 + n)


def slot(i, typ, pid, scene):
    return [[0x0C + 12 * i, typ], [0x0C + 12 * i + 4, pid], [0x0C + 12 * i + 8, scene]]


BUMP = lambda n: [[0, n]]  # noqa: E731
MANY = []
for b in range(3):
    ws = []
    for i in range(16):
        ws += slot(i, 1, 9000 + i, 100 + b * 16 + i)
    MANY.append(("many %d" % (b + 1), ws + BUMP(20 + b), [100 + b * 16, 147]))
FRESH = [
    ("boot", [], []),
    ("load 471", slot(0, 1, 1111, 471) + BUMP(1), [471, 12]),
    ("repeat, two pids", slot(0, 1, 2222, 471) + slot(3, 1, 3333, 471) + BUMP(2), [471]),
    ("one file, one missing", slot(1, 1, 4444, 12) + slot(2, 1, 4444, 999) + BUMP(3), [12, 999]),
    ("type 2, bad scenes, case and zeros", slot(0, 2, 5555, 12) + slot(1, 1, 5555, 0x80000000) + slot(2, 1, 5555, 0)
     + slot(3, 1, 0, 13) + slot(4, 1, 5555, 471) + slot(5, 1, 5555, 99) + BUMP(4), [99, 13, 0x80000000]),
    ("slot without a counter bump", slot(5, 1, 6666, 13), [13]),
    ("counter bump", BUMP(5), [13]),
    ("requester scribbles entries and header", [[0xCC + 40 * 5 + 8, 0xDEAD], [4, 99], [0x5C8, 1], [0xCC + 4, 7]], []),
    ("counter changes without slots", BUMP(0xFFFFFFFF), []),
    ("counter wraps, empty file", slot(7, 1, 7000, 14) + BUMP(0), [14]),
] + MANY
ADOPT_PRE = BUMP(5) + [[4, 7], [8, 9]] + slot(0, 1, 7777, 12) + slot(1, 1, 7777, 13) + [[0xCC, 999], [0xCC + 4, 2]]
ADOPT = [("boot", [], [12, 13, 999]), ("load 14", slot(2, 1, 7778, 14) + BUMP(6), [14])]


def norm(line):
    line = line[9:]  # "%7.2f  "
    line = line.replace(PFX, "").replace(GAMEDIR, "<dir>")
    return re.sub(r", for \d+ s$", "", line)


class Requester:
    def __init__(self, create):
        if create:
            self.mutex = k.CreateMutexW(None, False, MUTEX)
            self.hb = k.CreateFileMappingW(INVALID, None, PAGE_READWRITE, 0, BOARD_SIZE, BOARD)
        else:
            self.mutex = k.OpenMutexW(SYNCHRONIZE, False, MUTEX)
            self.hb = k.OpenFileMappingW(FILE_MAP_ALL_ACCESS, False, BOARD)
        assert self.mutex and self.hb, "board not there"
        self.view = k.MapViewOfFile(self.hb, FILE_MAP_ALL_ACCESS, 0, 0, 0)

    def locked(self, fn):
        assert k.WaitForSingleObject(self.mutex, 5000) in (0, 0x80)
        try:
            return fn()
        finally:
            k.ReleaseMutex(self.mutex)

    def write(self, writes):
        def go():
            for off, val in writes:
                ctypes.c_uint32.from_address(self.view + off).value = val
        self.locked(go)

    def snap(self):
        return self.locked(lambda: ctypes.string_at(self.view, BOARD_SIZE))

    def stable(self):
        time.sleep(0.5)
        prev = self.snap()
        for _ in range(60):
            time.sleep(0.3)
            cur = self.snap()
            if cur == prev:
                return cur
            prev = cur
        raise AssertionError("board never settled")

    def mapping(self, scene):
        h = k.OpenFileMappingW(FILE_MAP_ALL_ACCESS, False, "%s%06d.xscn" % (PFX, scene))
        if not h:
            return None
        v = k.MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, 0)
        size = ctypes.c_uint32.from_address(v).value
        data = ctypes.string_at(v + 4, size)
        k.UnmapViewOfFile(v)
        k.CloseHandle(h)
        return [size, hashlib.sha256(data).hexdigest()]


class Service:
    def __init__(self, src, game):
        self.lines, self.mark = [], 0
        env = dict(os.environ, WCCF_GAME=game)
        self.p = subprocess.Popen([sys.executable, "-I", src, "300"], env=env, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT, text=True, encoding="utf-8")
        threading.Thread(target=lambda: [self.lines.append(l.rstrip("\n")) for l in self.p.stdout], daemon=True).start()

    def wait_line(self, text, secs=30):
        t0 = time.time()
        while time.time() - t0 < secs:
            if any(text in l for l in self.lines):
                return
            if self.p.poll() is not None:
                break
            time.sleep(0.05)
        raise AssertionError("no %r in %r" % (text, self.lines))

    def new_lines(self):
        out, self.mark = self.lines[self.mark:], len(self.lines)
        return sorted(norm(l) for l in out)

    def stop(self):
        if self.p.poll() is None:
            self.p.terminate()
        self.p.wait()


def run(out, name, steps, pre, src, game):
    out.write(json.dumps({"t": "scenario", "name": name, "pre": pre, "files": {n: list(v) for n, v in FILES.items()}}) + "\n")
    req = Requester(True) if pre else None
    if req:
        req.write(pre)
    svc = Service(src, game)
    try:
        svc.wait_line("board ready")
        req = req or Requester(False)
        for step, writes, opens in steps:
            if writes:
                req.write(writes)
            board = req.stable()
            maps = {str(s): req.mapping(s) for s in opens}
            out.write(json.dumps({"t": "step", "name": step, "writes": writes, "open": opens, "board": board.hex(),
                                  "maps": maps, "log": svc.new_lines()}) + "\n")
    finally:
        svc.stop()


def main():
    global GAMEDIR
    tmp = tempfile.mkdtemp(prefix="rec_b_")
    game = os.path.join(tmp, "game")
    GAMEDIR = os.path.join(game, "data", "motion", "launcherMotion", "xscn")
    os.makedirs(GAMEDIR)
    for n, (seed, size) in FILES.items():
        with open(os.path.join(GAMEDIR, n), "wb") as f:
            f.write(content(seed, size))
    text = open(os.path.join(CODE, "scenes.py"), encoding="utf-8").read()
    for old, new in (('"xscnWatch::Mutex"', '"%s"' % MUTEX), ('"xscnWatch::ComBuf"', '"%s"' % BOARD),
                     ('"WCCF::xscnWatchStandIn"', '"%s"' % MARKER), ('"%06d.xscn" % scene', '"%s%%06d.xscn" %% scene' % PFX)):
        assert text.count(old) == 1 or old.startswith('"xscnWatch'), old
        text = text.replace(old, new)
    src = os.path.join(tmp, "scenes_rec.py")
    open(src, "w", encoding="utf-8").write(text)
    with open(OUT, "w", encoding="utf-8", newline="\n") as out:
        run(out, "fresh", FRESH, [], src, game)
        run(out, "adopt", ADOPT, ADOPT_PRE, src, game)
        # a second copy is refused (marker mutex held)
        out.write(json.dumps({"t": "scenario", "name": "refused", "pre": [], "files": {n: list(v) for n, v in FILES.items()}}) + "\n")
        marker = k.CreateMutexW(None, False, MARKER)
        svc = Service(src, game)
        svc.p.wait(30)
        time.sleep(0.2)
        out.write(json.dumps({"t": "refused", "exit": svc.p.returncode, "log": svc.new_lines()}) + "\n")
        k.CloseHandle(marker)
    print("wrote", OUT)


if __name__ == "__main__":
    main()
