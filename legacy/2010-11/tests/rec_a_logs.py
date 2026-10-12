# -*- coding: utf-8 -*-
"""rec_a_logs.py - records play.py's archive_run and rotate_logs on scratch log folders into
crates/launcher/tests/rec/logs.jsonl, which crates/launcher/tests/rec_logs.rs replays against launcher::logs.
Step 10 piece A: kept until piece G deletes the Python it records (play.py).
Synthetic only: made-up logs (xorshift bytes, which do not compress) in a temporary folder; it starts nothing.
Zip names and times are local times: the first line holds this PC's UTC offset at the recorded times, and the replay
checks only where its own offset is the same.
    python legacy\\2010-11\\tests\\rec_a_logs.py"""
import json
import os
import re
import shutil
import sys
import tempfile
import time
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "code"))
import kit_common as K  # noqa: E402
import play  # noqa: E402

OUT = os.path.join(REPO, "crates", "launcher", "tests", "rec", "logs.jsonl")
T0 = 1_791_500_001                                  # odd: a zip keeps seconds in steps of 2


def data(seed, n):
    """xorshift32 bytes, the same in rec_logs.rs"""
    x, out = seed, bytearray(n)
    for i in range(n):
        x ^= (x << 13) & 0xFFFFFFFF
        x ^= x >> 17
        x ^= (x << 5) & 0xFFFFFFFF
        out[i] = x & 0xFF
    return bytes(out)


def make(base, files):
    """files: [rel, size, seed, mtime]"""
    for rel, size, seed, mtime in files:
        p = os.path.join(base, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data(seed, size))
        os.utime(p, (mtime, mtime))


def run(size, at, seed):
    return [["run_seat1.txt", size, seed, at], ["hook\\mxhook_1.log", 40, seed + 1, at - 3],
            ["hook\\deep\\dll.log", 10, seed + 2, at - 7], ["run_server.txt", 0, 1, at - 1]]


def tree(root):
    out = []
    for d, ds, fs in os.walk(root):
        for n in ds:
            out.append([os.path.relpath(os.path.join(d, n), root), None])
        for n in fs:
            p = os.path.join(d, n)
            with open(p, "rb") as f:
                body = f.read()
            out.append([os.path.relpath(p, root), "zip" if n.endswith(".zip") else body.decode("latin-1")])
    return sorted(out)


def record_archive(out, root):
    arc = os.path.join(root, "archive")
    calls = [("run0", run(40_000, T0, 11)), ("run1", run(40_000, T0 + 60, 12)), ("run2", run(40_000, T0 + 120, 13)),
             ("run3", run(40_000, T0 + 120, 14)), ("run4", run(40_000, T0 + 120, 15)),
             ("big", run(300_000, T0 + 600, 16)), ("empty", []), ("missing", None)]
    for name, files in calls:
        folder = os.path.join(root, name)
        if files is not None:
            os.makedirs(folder, exist_ok=True)
            make(folder, files)
        before = set(os.listdir(arc)) if os.path.isdir(arc) else set()
        ret = play.archive_run(folder, arc, 100_000)
        zips = sorted(os.listdir(arc)) if os.path.isdir(arc) else None
        made = sorted(set(zips or ()) - before)
        entries = []
        for m in made:
            with zipfile.ZipFile(os.path.join(arc, m)) as z:
                entries = [[i.filename, list(i.date_time), i.file_size, i.CRC, i.compress_type] for i in z.infolist()]
        out.write(json.dumps({"op": "archive", "folder": name, "files": files, "limit": 100_000, "ret": ret,
                              "zips": zips, "made": made, "entries": entries}) + "\n")
    return len(calls)


def record_rotate(out, root):
    said = []
    play.say = said.append
    old = [["previous\\run_seat1.txt", 30, 21, T0], ["previous\\hook\\mxhook_9.log", 20, 22, T0 - 2]]
    now = [["run_seat1.txt", 12, 31, T0 + 100], ["run_server.txt", 0, 1, T0 + 100], ["hook\\mxhook_1.log", 8, 32, T0 + 90]]
    cases = [
        ("no logs folder", None, [], None),
        ("first run", None, now, None),
        ("second run", None, old + now + [["archive\\run-old.zip", 5, 41, T0 - 900],
                                          ["previous-20261001-000000\\x.txt", 3, 42, T0 - 999]], None),
        ("previous held", None, old + now, "previous\\run_seat1.txt"),
        ("current log held", None, old + now, "run_seat1.txt"),
        ("previous is a file", None, now + [["previous", 4, 43, T0]], None),
    ]
    for name, _, files, held in cases:
        logs = os.path.join(root, name.replace(" ", "_"), "logs")
        if files:
            make(logs, files)
        K.LOGS = logs
        del said[:]
        h = open(os.path.join(logs, held), "rb") if held else None     # no FILE_SHARE_DELETE: as a viewer holds it
        try:
            play.rotate_logs()
        finally:
            if h:
                h.close()
        words = []
        for s in said:
            s = re.sub(r"\((\[WinError [^)]*|[^)]*)\)", "({ex})", s, count=1)
            words.append(re.sub(r"previous-\d{8}-\d{6}", "previous-{now}", s))
        t = [[re.sub(r"previous-\d{8}-\d{6}", "previous-{now}", p) if "20261001" not in p else p, b]
             for p, b in tree(logs)]
        out.write(json.dumps({"op": "rotate", "case": name, "files": files, "held": held, "said": words,
                              "tree": sorted(t)}) + "\n")
    return len(cases)


def main():
    root = tempfile.mkdtemp(prefix="forza-rec-logs-")
    try:
        with open(OUT, "w", encoding="utf-8", newline="\n") as out:
            offsets = {t: time.localtime(t).tm_gmtoff for t in (T0, T0 + 600)}
            out.write(json.dumps({"utc_offset": offsets[T0]}) + "\n")
            assert len(set(offsets.values())) == 1
            n = record_archive(out, os.path.join(root, "archive_run"))
            n += record_rotate(out, os.path.join(root, "rotate"))
        print("recorded %d cases into %s" % (n, OUT))
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
