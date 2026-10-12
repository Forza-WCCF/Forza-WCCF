# -*- coding: utf-8 -*-
"""rec_a_card.py - records kit_common.card_session and play.py's club_session on synthetic club card files into
crates/launcher/tests/rec/card.jsonl, which crates/launcher/tests/rec_card.rs replays against launcher::card.
Step 10 piece A: kept until piece G deletes the Python it records (kit_common.py, play.py).
Synthetic only: made-up card files (zeros, a few seeded bytes, the session bits, a checksum) in a temporary folder -
never a player's card; it starts nothing.  A card is stored as its size and its non-zero bytes [offset, byte].
    python legacy\\2010-11\\tests\\rec_a_card.py"""
import json
import os
import random
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "code"))
import kit_common as K  # noqa: E402
import play  # noqa: E402
from cards import wallet  # noqa: E402

OUT = os.path.join(REPO, "crates", "launcher", "tests", "rec", "card.jsonl")
WRITTEN = 1_791_500_000                       # every card's write time (seconds); the run starts before, at or after


def card(rng, good=(0,), flag=1, bad=0, plus=0, empty=None, new=False, size=K.CARD_BYTES):
    """a card whose copies in `good` have a right checksum (+ plus), the session flag and `bad` bad endings; the
    others a wrong one; `empty`: that copy all zeros"""
    raw = bytearray(K.CARD_BYTES)
    for i, start in enumerate(K._COPIES):
        for _ in range(12):
            raw[start + rng.randrange(K._PAYLOAD)] = rng.randrange(1, 256)
        bit = lambda b, on: raw.__setitem__(start + (b >> 3), (raw[start + (b >> 3)] & ~(0x80 >> (b & 7))) |  # noqa
                                            ((0x80 >> (b & 7)) if on else 0))
        bit(K._FLAG_BIT, flag)
        for k in range(K._COUNT_BITS):
            bit(K._COUNT_BIT + k, (bad >> (K._COUNT_BITS - 1 - k)) & 1)
        total = sum(int.from_bytes(raw[start + j:start + j + 4], "little") for j in range(0, K._PAYLOAD, 4))
        stored = (total + plus) & 0xFFFFFFFF if i in good else (total + 7) & 0xFFFFFFFF
        raw[start + K._PAYLOAD:start + K._PAYLOAD + 4] = stored.to_bytes(4, "little")
        if empty == i:
            raw[start:start + 0x690] = bytes(0x690)
    if new:
        raw[5] = raw[6] = 0xFF
    raw[1] = 0x2A                                 # a header byte, to see the header is not read
    return bytes(raw[:size]) + bytes(max(0, size - K.CARD_BYTES))


def stored(raw):
    return {"size": len(raw), "set": [[i, b] for i, b in enumerate(raw) if b]}


def put(path, raw):
    if raw is None:
        if os.path.exists(path):
            os.remove(path)
        return
    with open(path, "wb") as f:
        f.write(raw)
    os.utime(path, ns=(WRITTEN * 10**9, WRITTEN * 10**9))


def main():
    rng = random.Random(20261013)
    cards = [
        ("no file", None),
        ("open in A", card(rng, flag=1, bad=0)),
        ("open in A, 3 bad", card(rng, flag=1, bad=3)),
        ("closed in A, 127 bad", card(rng, flag=0, bad=127)),
        ("open in B only", card(rng, good=(1,), flag=1, bad=2)),
        ("closed in B only", card(rng, good=(1,), flag=0, bad=5)),
        ("both good", card(rng, good=(0, 1), flag=1, bad=1)),
        ("checksum + 1", card(rng, flag=0, bad=4, plus=1)),
        ("checksum + 2", card(rng, flag=0, bad=4, plus=2)),
        ("no good copy", card(rng, good=(), flag=1)),
        ("copy A empty", card(rng, good=(1,), flag=1, empty=0)),
        ("copy B empty", card(rng, good=(0,), flag=1, empty=1)),
        ("new", card(rng, new=True)),
        ("short", card(rng, size=K.CARD_BYTES - 1)),
        ("long", card(rng, size=K.CARD_BYTES + 16)),
        ("empty file", b""),
    ]
    sinces = [None, WRITTEN - 10, WRITTEN, WRITTEN + 10]
    root = tempfile.mkdtemp(prefix="forza-rec-card-")
    n = 0
    try:
        slot, new, running = (os.path.join(root, f) for f in ("slot.bin", "new.bin", "running.json"))
        with open(OUT, "w", encoding="utf-8", newline="\n") as out:
            for name, raw in cards:
                put(slot, raw)
                for since in sinces:
                    state, bad = K.card_session(slot, since)
                    out.write(json.dumps({"card": name, "raw": None if raw is None else stored(raw), "since": since,
                                          "state": state, "bad": bad}) + "\n")
                    n += 1
            # the club in play: slot and new card, the run's start from running.json's write time (or none)
            play.CARD, wallet.NEW, play.RUNNING = slot, new, running
            pick = [c for c in cards if c[0] in ("no file", "open in A", "closed in A, 127 bad", "new", "short")]
            for since in (None, WRITTEN - 10, WRITTEN + 10):
                put(running, None if since is None else b"{}")
                if since is not None:
                    os.utime(running, (since, since))
                for sname, sraw in pick:
                    for nname, nraw in pick:
                        put(slot, sraw)
                        put(new, nraw)
                        state, bad = play.club_session()
                        out.write(json.dumps({"slot": sname, "new": nname, "since": since, "state": state,
                                              "bad": bad}) + "\n")
                        n += 1
        print("recorded %d cases into %s" % (n, OUT))
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
