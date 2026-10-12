# -*- coding: utf-8 -*-
"""rec_e_keys.py - records keyboard.py's key and controller logic (the vectors its --selftest checks, plus seeded random
ones) into crates/launcher/tests/rec/keys.jsonl, which crates/launcher/tests/rec_keys.rs replays against launcher::keys.
Step 10 piece E: kept until piece G deletes the Python it records (play.py still starts io_board.py and keyboard.py).
Pure logic only: keys.txt contents given as bytes, held-input sets, a stand-in for winmm; it reads no real key, no
controller and no window, and it starts nothing.  Run with the staged kit's Python:
    python legacy\\2010-11\\tests\\rec_e_keys.py"""
import json
import os
import random
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
CODE = os.path.join(os.path.dirname(HERE), "code")
sys.path.insert(0, CODE)
sys.path.insert(0, os.path.join(CODE, "hardware"))
import keyboard as KB  # noqa: E402

OUT = os.path.join(REPO, "crates", "launcher", "tests", "rec", "keys.jsonl")
NAMES = [a[0] for a in KB.ACTIONS]
recs = []


def rec(kind, **kw):
    recs.append(dict(kind=kind, **kw))


def bind(keys=None, pads=None):
    """apply a binding: the defaults with these keys over them, these controls"""
    KB.apply_keys(dict(KB.DEFAULTS, **(keys or {})), pads or {})
    return {"keys": keys or {}, "pads": pads or {}}


def fields(text):
    lines = text.splitlines()
    nums = lambda s: [int(x, 16) for x in s.split("=")[1].split()]  # noqa: E731
    return {"sys": nums(lines[1])[0], "p1": nums(lines[2]), "p2": nums(lines[3]),
            "coin": int(lines[4].split("=")[1])}


def pad_set(s):
    return sorted(s)


def main():
    # ---- tables
    rec("tables", actions=[[n, d, list(b) if b else None, what] for n, d, b, what in KB.ACTIONS],
        allowed=sorted(KB.ALLOWED_VKS))
    rec("key_labels", labels=[KB.key_label(v) for v in range(0, 0x100)])
    words = ["B3", " B 12 ", "hat up", "HATLEFT", "y-", "Z+", "B0", "B33", "B", "B-1", "Q+", "X", "HAT", "HAT NORTH",
             "", None, "0x0D", "B³", "B99999", "b32", "B1", "B0032", "x+", "V-", "u+", "HAT RIGHT", "hat\tdown",
             "B 1 2", "+X", "H A T U P"]
    rec("pad_codes", codes=[[KB.pad_token(c), KB.pad_label(c)] for c in range(0, 0x340)],
        words=[[x, KB.pad_code(x)] for x in words])
    # ---- keys.txt and its PAD lines
    pieces = ["START=0x20", "shoot = 0x56", "PRESS=V", "COIN=0x1B", "KEEPER=0x79", "NOPE=0x41", "TEST=0x7a", "", "# c",
              "SHOOT=0x58", "PRESS=0x43", "START=0x0D", "START=0x20", "SENSOR2=0x38", "SENSOR3=56", "CARD=0x49",
              "CARD=0x35", "COIN=0x49", "UP=0x25", "LEFT=0x26", "DOWN=0X28", "DATA=1_0", "DATA=0x5_3", "KEYPLAYER=+68",
              "SERVICE=-1", "SERVICE=0b1000001", "TEST=0o161", "TEST=0161", "TEST=00", "TEST=0x", "TEST= 0x70 # tail",
              "START", "=0x20", "PAD START=B10", "pad shoot = b2", "PAD UP=Y-", "PAD LEFT = hat left", "PAD NOPE=B1",
              "PAD DATA=B99", "PAD KEEPER=B2", "PAD COIN=B5", "PAD COIN=", "PAD TEST", "PAD  SERVICE  X  =B7",
              "PAD START=B1", "PAD SHOOT=B1", "PAD START=B4", "PAD DOWN=-", "PAD RIGHT=X+", "PAD PRESS=x+",
              "PAD  =B1", "PAD A B=B1", "PAD CARD=hat up", "PAD KEYPLAYER=Z-", "PADSTART=B1", "ENTER=0x0D", "up = 0x26",
              "START=0x20\rPRESS=0x56", "SHOOT=A'B", 'SHOOT="x"', "SHOOT=a\\b", "SHOOT=\tzz", "PAD SHOOT=q'z"]
    files = [b"", b"# test\nSTART=0x20\nshoot = 0x56\n\nPRESS=V\nCOIN=0x1B\nKEEPER=0x79\nNOPE=0x41\nTEST=0x7a\n",
             b"SHOOT=0x58\n", b"SHOOT=0x58\nPRESS=0x43\n", b"START=0x0D\nSTART=0x20\n",
             b"\xff\xfe garbage \x00\nSTART", b"SENSOR2=0x38\n",
             b"START=0x0D\nPAD START=B10\npad shoot = b2\nPAD UP=Y-\nPAD LEFT = hat left\nPAD NOPE=B1\n"
             b"PAD DATA=B99\nPAD KEEPER=B2\nPAD COIN=B5\nPAD COIN=\nPAD TEST\nPAD  SERVICE  X  =B7\n",
             b"PAD START=B1\nPAD SHOOT=B1\nPAD START=B4\n", b"\xff\xfe PAD START=B3 \x00\nPAD",
             b"START=\xe9\xe8\nPAD SHOOT=\xe9\n", b"NAME\xc3\xa9=1\n"]
    rng = random.Random(1011)
    for _ in range(400):
        n = rng.randrange(1, 9)
        text = "\n".join(rng.choice(pieces) for _ in range(n)) + rng.choice(["", "\n"])
        files.append(text.encode("latin-1", "replace"))
    d = tempfile.mkdtemp()
    p = os.path.join(d, "k.txt")
    for raw in files:
        with open(p, "wb") as f:
            f.write(raw)
        keys, notes = KB.load_keys(p)
        pads, pnotes = KB.load_pads(p)
        rec("load", file=raw.hex(), keys=[keys[n] for n in NAMES], notes=notes,
            pads=[[n, c] for n, c in pads.items()], pad_notes=pnotes)
    # ---- render / step / held / events on random inputs, several bindings
    binds = [bind(), bind({"START": 0x20, "SHOOT": 0x56}), bind({"PRESS": 0}, {"PRESS": KB.PAD_BUTTON + 1}),
             bind({}, {"START": KB.PAD_BUTTON + 10, "PRESS": KB.PAD_BUTTON + 1, "UP": KB.PAD_AXIS + 2,
                       "SENSOR2": KB.PAD_HAT, "CARD": KB.PAD_BUTTON + 12, "COIN": KB.PAD_AXIS + 11}),
             bind({"SENSOR2": 0x38, "SENSOR3": 0x39})]
    for b in binds:
        KB.apply_keys(dict(KB.DEFAULTS, **b["keys"]), b["pads"])
        pool = sorted({v for ins in KB.INPUTS.values() for v in ins} | {0, 0x1B, 0x79, 0x7A, 0x100, 0x101, 0x133})
        for _ in range(100):
            def pick():
                return rng.sample(pool, rng.randrange(0, 5))
            down, prev = set(pick()), set(pick())
            card, coin = rng.random() < 0.5, rng.randrange(0, 6)
            start = rng.choice([None, True, False])
            rec("input", bind=b, down=sorted(down), prev=sorted(prev), card=card, coin=coin, start=start,
                held=sorted(KB.held_actions(down)), render=fields(KB.render(down, card, coin, start)),
                step=list(KB.step(down, prev, card, coin)))
    # ---- free play's START timing, the eject rule
    for armed in (None, (0.15, 0.35), (1.0, 2.0)):
        for now in (0.0, 0.10, 0.15, 0.20, 0.34, 0.35, 0.40, 0.90, 1.0, 1.5, 2.0, 5.0):
            for enter in (False, True):
                rec("start_held", now=now, enter=enter, armed=armed, held=KB.start_held(now, enter, armed))
    outs_list = [None, [], [0], [0, 8], [0, 0x08, 0x60], [0, 0x00, 0x80], [0, 0xF7], [0, 0xFF], [8, 0]]
    for outs in outs_list:
        for card in (False, True):
            for since in (None, 7.0, 8.5, 5.0, 9.99):
                for waiting in (None, 9.0, 9.7, 9.9, 10.0):
                    r = KB.should_eject(10.0, card, since, outs, waiting)
                    rec("eject", now=10.0, card=card, since=since, outs=outs, waiting=waiting, out=[r[0], r[1]])
    # ---- a controller: PadDevice on scripted readings
    still = [0.0, 0.0, None, None, None, None]
    trig = [0.0, 0.0, None, None, -1.0, -1.0]
    inits = [(0, still, 0xFFFF), (0, trig, 0xFFFF), (0b1, still, 0), (0, [-1.0, 0.0] + [None] * 4, 0xFFFF),
             (0b101, [0.6, -0.7, 0.2, None, 1.0, 0.0], 9000)]
    for buttons, axes, pov in inits:
        dev = KB.PadDevice(buttons, axes, pov)
        reads = []
        for _ in range(60):
            r = (rng.choice([0, 1, 4, 5, 0b101, 0xFFFFFFFF, rng.randrange(1 << 32)]),
                 [None if a is None and rng.random() < .8 else rng.choice([-1.0, -0.6, -0.4, -0.25, 0.0, 0.25, 0.3,
                                                                           0.45, 0.5, 0.6, 1.0, rng.uniform(-1, 1)])
                  for a in axes],
                 rng.choice([0xFFFF, 0, 4500, 9000, 13500, 18000, 22500, 27000, 31500, 35999, 36000, 40000, 100]))
            reads.append([r[0], r[1], r[2], pad_set(dev.read(*r))])
        rec("pad_device", init=[buttons, axes, pov], reads=reads)
    rec("axis_values", cases=[[v, lo, hi, KB.pad_axis_value(v, lo, hi)] for v, lo, hi in
                              [(0, 0, 65535), (32767, 0, 65535), (65535, 0, 65535), (100, 0, 0), (5, 9, 3), (0, 10, 20),
                               (30, 10, 20), (32768, 0, 65535), (255, 0, 255), (128, 0, 255)]])
    rec("hat_dirs", cases=[[pov, sorted(KB.pad_hat_dirs(pov))] for pov in
                           [0, 4500, 4501, 9000, 13499, 13500, 13501, 18000, 22500, 27000, 31500, 31499, 35999, 36000,
                            0xFFFF, 100, 35000]])

    class Caps:
        def __init__(self, axes=2, caps=0x10, pid=0x0006, lim=None):
            self.wNumButtons, self.wNumAxes, self.wCaps, self.wMid, self.wPid = 12, axes, caps, 0x0079, pid
            self.szPname = "x"
            for i, a in enumerate(KB.AXES):
                lo, hi = (lim or {}).get(a, (0, 65535))
                setattr(self, "w%smin" % a, lo)
                setattr(self, "w%smax" % a, hi)

    class Info:
        def __init__(self, buttons=0, x=32767, y=32767, z=32767, r=32767, u=32767, v=32767, pov=0xFFFF):
            self.dwButtons, self.dwXpos, self.dwYpos, self.dwZpos = buttons, x, y, z
            self.dwRpos, self.dwUpos, self.dwVpos, self.dwPOV = r, u, v, pov

    def caps_json(c):
        return {"axes": c.wNumAxes, "caps": c.wCaps, "pid": c.wPid, "buttons": c.wNumButtons,
                "lim": [[getattr(c, "w%smin" % a), getattr(c, "w%smax" % a)] for a in KB.AXES]}

    def info_json(i):
        return {"buttons": i.dwButtons, "pos": [i.dwXpos, i.dwYpos, i.dwZpos, i.dwRpos, i.dwUpos, i.dwVpos],
                "pov": i.dwPOV}
    readings = [(Caps(), Info()), (Caps(), Info(buttons=0b100, y=0)), (Caps(axes=4, caps=0x1F), Info(u=0, v=0)),
                (Caps(axes=6, caps=0x1F, lim={"Z": (5, 5), "R": (0, 255)}), Info(z=5, r=255, pov=4500)),
                (Caps(axes=1, caps=0), Info(x=65535, pov=0)), (Caps(axes=3, caps=0x11), Info(z=0, pov=18000))]
    rec("pad_readings", cases=[[caps_json(c), info_json(i), [KB.pad_reading(c, i)[0], KB.pad_reading(c, i)[1],
                                                             KB.pad_reading(c, i)[2]]] for c, i in readings])

    # ---- Pads on a stand-in for winmm: scripted plug-ins, reads, unplugging
    class Api:
        def __init__(self):
            self.devs, self.changed = {}, 0

        def caps(self, jid):
            return self.devs[jid][0] if jid in self.devs else None

        def pos(self, jid):
            return self.devs[jid][1] if jid in self.devs else None

        def config_changed(self):
            self.changed += 1
    api, said = Api(), []
    pads = KB.Pads(api, say=said.append, name=lambda c: "pad %04X" % c.wPid, thread=False)
    ops = [("rescan",), ("poll",), ("set", 2, Caps(), Info()), ("rescan",), ("rescan",),
           ("set", 2, Caps(), Info(buttons=0b100, y=0)), ("poll",),
           ("set", 5, Caps(axes=4, caps=0x1F, pid=0x0CE6), Info(u=0, v=0)), ("rescan",),
           ("set", 5, Caps(axes=4, caps=0x1F, pid=0x0CE6), Info(buttons=0b1, u=0, v=0)), ("poll",),
           ("del", 2), ("poll",), ("set", 2, Caps(), Info(buttons=0b100)), ("rescan",), ("poll",),
           ("set", 2, Caps(), Info(buttons=0)), ("poll",), ("set", 2, Caps(), Info(buttons=0b100)), ("poll",),
           ("del", 5), ("del", 2), ("poll",), ("rescan",), ("set", 15, Caps(), Info()), ("set", 16, Caps(), Info()),
           ("rescan",), ("poll",)]
    script = []
    for op in ops:
        if op[0] == "set":
            api.devs[op[1]] = (op[2], op[3])
            script.append({"op": "set", "slot": op[1], "caps": caps_json(op[2]), "info": info_json(op[3])})
            continue
        if op[0] == "del":
            api.devs.pop(op[1], None)
            script.append({"op": "del", "slot": op[1]})
            continue
        n0 = len(said)
        held = None
        if op[0] == "rescan":
            pads.rescan()
        else:
            held = pad_set(pads.poll())
        script.append({"op": op[0], "held": held, "said": said[n0:], "changed": api.changed, "names": pads.names()})
    rec("pads", script=script)

    # ---- the driver's loop body on scripted ticks (keyboard.py main(): step, card_in_since, eject, free play, render)
    for bi, b in enumerate(binds[:4]):
        KB.apply_keys(dict(KB.DEFAULTS, **b["keys"]), b["pads"])
        pool = sorted({v for ins in KB.INPUTS.values() for v in ins})
        for init_card in (False, True):
            card, coin, t0 = init_card, 0, 100.0
            prev, armed, card_in_since = set(), None, (t0 if card else None)
            waiting_since, next_out_check = None, 0.0
            now, ticks = t0, []
            for _ in range(200):
                now = round(now + rng.choice([0.01, 0.01, 0.01, 0.05, 0.1, 0.2]), 3)
                down = set(rng.sample(pool, rng.randrange(0, 3))) if rng.random() < 0.6 else set(prev)
                outs = rng.choice([None, [0, 0x08, 0x60], [0, 0x00, 0x80], [0, 8], [0, 0]])
                was_in = card
                card, coin, events = KB.step(down, prev, card, coin)
                if card and not was_in:
                    card_in_since = now
                if now >= next_out_check:
                    next_out_check = now + 0.1
                    eject, waiting_since = KB.should_eject(now, card, card_in_since, outs, waiting_since)
                    if eject:
                        card, card_in_since, waiting_since = False, None, None
                        events.append("the game ejected the card - taken out (press I to put it back in)")
                acts = KB.held_actions(down)
                if "START" in acts - KB.held_actions(prev):
                    armed = (now + KB.START_DELAY, now + KB.START_MIN)
                start = KB.start_held(now, "START" in acts, armed)
                text = KB.render(down, card, coin, start)
                ticks.append({"now": now, "down": sorted(down), "outs": outs, "events": events,
                              "state": fields(text), "card": card})
                prev = down
            rec("ticks", bind=b, t0=t0, init_card=init_card, ticks=ticks)
    KB.apply_keys(KB.DEFAULTS)
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", newline="\n", encoding="utf-8") as f:
        for r in recs:
            f.write(json.dumps(r, ensure_ascii=True) + "\n")
    print("recorded %d records into %s" % (len(recs), OUT))
    print("selftest of the recorded module:", KB.selftest())


if __name__ == "__main__":
    main()
