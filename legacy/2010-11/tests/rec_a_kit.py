# -*- coding: utf-8 -*-
"""rec_a_kit.py - records kit_common.find_game, seat_dir and misc_dir over a scratch tree into
crates/launcher/tests/rec/kit.jsonl, which crates/launcher/tests/rec_kit.rs replays against launcher::kit.
Step 10 piece A: kept until piece G deletes the Python it records (kit_common.py goes with play.py).
Synthetic only: empty files named like the game's programs in a temporary folder; it starts nothing.
    python legacy\\2010-11\\tests\\rec_a_kit.py"""
import json
import os
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "code"))
import kit_common as K  # noqa: E402

OUT = os.path.join(REPO, "crates", "launcher", "tests", "rec", "kit.jsonl")
EXES = ("client_Release.exe", "control_Release.exe", "match_Release.exe")
TREE = [os.path.join("plain", "game", n) for n in EXES] + \
       [os.path.join("above", "extracted", n) for n in EXES] + \
       [os.path.join("both", n) for n in EXES] + [os.path.join("both", "extracted", n) for n in EXES] + \
       [os.path.join("short", "game", n) for n in EXES[:2]] + \
       [os.path.join("with space", "game", n.lower()) for n in EXES]
INPUTS = ["", "plain\\game", "plain", "above", "above\\extracted", "both", "short\\game", "nowhere",
          "  plain\\game  ", '"with space\\game"', ' "above" ', "plain\\game\\", "plain/game", "plain\\x\\..\\game"]


def main():
    root = tempfile.mkdtemp(prefix="forza-rec-kit-")
    try:
        for f in TREE:
            os.makedirs(os.path.dirname(os.path.join(root, f)), exist_ok=True)
            open(os.path.join(root, f), "wb").close()
        rel = lambda p: None if p is None else os.path.relpath(p, root)   # noqa: E731
        with open(OUT, "w", encoding="utf-8", newline="\n") as out:
            out.write(json.dumps({"tree": TREE}) + "\n")
            for i in INPUTS:
                # an input is a path under the scratch root, its spaces and quotes kept around it; "" stays empty
                arg = (i[:len(i) - len(i.lstrip(' "'))] + os.path.join(root, i.lstrip(' "'))) if i else ""
                game = K.find_game(arg)
                out.write(json.dumps({"input": i, "game": rel(game),
                                      "seat": rel(K.seat_dir(game)) if game else None,
                                      "misc": rel(K.misc_dir(game)) if game else None}) + "\n")
        print("recorded %d cases into %s" % (len(INPUTS), OUT))
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
