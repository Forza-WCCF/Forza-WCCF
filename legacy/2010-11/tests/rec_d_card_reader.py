# -*- coding: utf-8 -*-
"""rec_d_card_reader.py - records code/hardware/card_reader.py (the club card reader stand-in) on scratch pipes with
synthetic cards into crates/forza/tests/rec/card_reader.jsonl, which crates/forza/tests/standins.rs replays against
forza::standins::card_reader.  Step 10 piece D.  KEPT UNTIL PIECE G: play.py still starts the Python reader; piece G
deletes card_reader.py, test_icc_reader.py and this recorder together.
Synthetic only: cards are made of seeded random bytes (through the Card class for the header), in a temporary folder;
pipe names carry this process's pid (never wccf_icc_seat*); the reader's os.urandom is replaced by a seeded stream whose
bytes are recorded.  Each line of the file is one of
  {"t":"scenario","name","files":{path: hex},"card","new","urandom":[hex..]}   a fresh reader on a fresh folder
  {"t":"io","conn","req","split","resp"}                                       bytes written to the pipe -> bytes read back
  {"t":"put","name","hex"}                                                     a file the player's tools add mid-run
  {"t":"files","list":[[path,size,sha256]..]}                                  the folder, timestamps in names -> <t>
    python legacy\\2010-11\\tests\\rec_d_card_reader.py"""
import ctypes
import ctypes.wintypes as w
import hashlib
import json
import os
import random
import re
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "code"))
from hardware import card_reader as R  # noqa: E402

OUT = os.path.join(REPO, "crates", "forza", "tests", "rec", "card_reader.jsonl")
k = R.k
k.CreateFileW.restype = w.HANDLE
k.CreateFileW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, ctypes.c_void_p, w.DWORD, w.DWORD, w.HANDLE]
DLE, ETX, STX = 0x10, 0x03, 0x02


def synth(seed, counter=0xFFFF, uid=None):
    """a synthetic card file: the Card class's header, seeded random blocks"""
    rng = random.Random(seed)
    c = R.Card()
    if uid:
        c.uid = bytes.fromhex(uid)
    c.counter = counter
    for b in c.blocks:
        b[:] = rng.randbytes(16)
    return c.image()


def msg(cmd, data=b""):
    return R.frame(bytes([cmd, cmd, len(data)]) + data)


def listing(root):
    out = []
    for d, _, fs in os.walk(root):
        for f in fs:
            p = os.path.join(d, f)
            rel = os.path.relpath(p, root).replace("\\", "/")
            out.append([re.sub(r"\d{8}-\d{6}", "<t>", rel), os.path.getsize(p), hashlib.sha256(open(p, "rb").read()).hexdigest()])
    return sorted(out)


class Client:
    def __init__(self, pipe, out):
        self.pipe, self.out, self.h, self.conn = pipe, out, None, -1
        self.n, self.avail, self.buf = w.DWORD(), w.DWORD(), (ctypes.c_char * 4096)()

    def open(self):
        if self.h:
            k.CloseHandle(self.h)
            time.sleep(0.5)
        for _ in range(100):
            h = k.CreateFileW(self.pipe, 0xC0000000, 0, None, 3, 0, None)
            if h != R.INVALID:
                self.h, self.conn = h, self.conn + 1
                return
            time.sleep(0.05)
        raise SystemExit("no pipe")

    def tx(self, req, split=0):
        if split:
            k.WriteFile(self.h, req[:split], split, ctypes.byref(self.n), None)
            time.sleep(0.1)
            k.WriteFile(self.h, req[split:], len(req) - split, ctypes.byref(self.n), None)
        else:
            k.WriteFile(self.h, req, len(req), ctypes.byref(self.n), None)
        time.sleep(0.15)
        resp = b""
        while k.PeekNamedPipe(self.h, None, 0, None, ctypes.byref(self.avail), None) and self.avail.value:
            k.ReadFile(self.h, self.buf, min(self.avail.value, 4096), ctypes.byref(self.n), None)
            resp += bytes(self.buf.raw[:self.n.value])
            time.sleep(0.05)
        self.out.write(json.dumps({"t": "io", "conn": self.conn, "req": req.hex(), "split": split, "resp": resp.hex()}) + "\n")
        return resp

    def cmd(self, cmd, data=b"", split=0):
        """the game's four-step exchange: 02 -> 10; message -> 10 02; 10 -> reply; ack"""
        self.tx(bytes([STX]))
        self.tx(msg(cmd, data), split)
        r = self.tx(bytes([DLE]))
        self.tx(b"\x00")
        return r

    def done(self):
        k.CloseHandle(self.h)
        self.h = None


def main():
    sys.stdout = open(os.devnull, "w")                # the reader prints its log
    real_urandom = os.urandom

    class Out:                                        # collects lines so the scenario line can learn its urandom list
        def __init__(self):
            self.buf = []

        def write(self, s):
            self.buf.append(s)

    all_lines = []

    def run(name, **kw):
        o = Out()
        line, ur = scenario_core(o, name, **kw)
        line["urandom"] = ur
        all_lines.append(json.dumps(line) + "\n")
        all_lines.extend(o.buf[1:])

    def scenario_core(o, name, steps, files=None, card=None, new=None):
        root = tempfile.mkdtemp(prefix="wccf-rec-d-")
        rng = random.Random(name)
        ur = []

        def fake(n):
            b = rng.randbytes(n)
            ur.append(b.hex())
            return b
        os.urandom = fake
        for rel, data in (files or {}).items():
            p = os.path.join(root, rel)
            os.makedirs(os.path.dirname(p), exist_ok=True)
            open(p, "wb").write(data)
        pipe = r"\\.\pipe\rec_d_card_%d_%s" % (os.getpid(), name)
        argv = [pipe, "3600"]
        if card:
            argv += [os.devnull, os.path.join(root, card)]
            if new:
                argv.append(os.path.join(root, new))
        threading.Thread(target=R.main, args=(argv,), daemon=True).start()
        line = {"t": "scenario", "name": name, "files": {r: d.hex() for r, d in (files or {}).items()}, "card": card, "new": new}
        o.write(json.dumps(line) + "\n")
        time.sleep(0.3)
        c = Client(pipe, o)
        c.open()
        steps(c, root)
        c.done()
        time.sleep(0.4)
        o.write(json.dumps({"t": "files", "list": listing(root)}) + "\n")
        return line, ur

    def snap(c, root):
        c.out.write(json.dumps({"t": "files", "list": listing(root)}) + "\n")

    def put(c, root, rel, data):
        open(os.path.join(root, rel), "wb").write(data)
        c.out.write(json.dumps({"t": "put", "name": rel, "hex": data.hex()}) + "\n")

    UID = bytes.fromhex("deadbeef")

    # 1. no card file: the default NEW card, every command, no disk
    def s1(c, root):
        c.tx(bytes([STX]))                            # the start-up probe
        c.cmd(0x4E)
        c.cmd(0x4C)
        c.cmd(0x41)
        c.cmd(0x42)
        c.cmd(0x43, UID)
        c.cmd(0x43, bytes.fromhex("01020304"))        # no such card
        c.cmd(0x50)
        c.cmd(0x52, bytes([0, 0, 15]))
        c.cmd(0x52, bytes([0x0A, 0x01, 15]))          # wraps past block 255
        c.cmd(0x54, bytes([6, 0]))
        c.cmd(0x54, bytes([0x10, 0]))                 # block 16: the reply holds a 10 byte or not, per card
        c.cmd(0x55, bytes([3, 0]) + bytes(range(0x08, 0x18)))   # writes a 10 in the data
        c.cmd(0x54, bytes([3, 0]))
        c.cmd(0x55, bytes([4, 0]) + bytes(16))        # block 4: refused
        c.cmd(0x54, bytes([4, 0]))
        c.cmd(0x55, bytes([5, 0]) + b"\x01\x02")      # a short block is zero padded
        c.cmd(0x53, bytes([8, 0, 2]) + bytes(range(32)))
        c.cmd(0x53, bytes([3, 0, 3]) + b"\xaa" * 40)  # blocks 3, 4 (skipped), 5; the last is short
        c.cmd(0x52, bytes([0, 0, 15]))
        c.cmd(0x49, bytes([5, 0, 0, 0]))              # decrement by 0: no save
        c.cmd(0x49, bytes([5, 0, 3, 0]))
        c.cmd(0x49, bytes([5, 0, 0xFF, 0xFF]))        # wraps
        c.cmd(0x49, bytes([5]))                       # too short: amount 0
        c.cmd(0x45)                                   # halt
        c.cmd(0x41)                                   # nobody awake
        c.cmd(0x42)
        c.cmd(0x4E)                                   # RF control wakes it
        c.cmd(0x41)
        c.cmd(0x99, b"\x01\x02")                      # unknown, accepted
        c.cmd(0x4C, bytes(6))
        c.cmd(0x50)
    run("no_card_file", steps=s1)

    # 2. a card file: saves, the session backup (the first save only), the files after
    card2 = synth("c2", 0x0123, "a1b2c3d4")

    def s2(c, root):
        c.cmd(0x4C)
        c.cmd(0x42)
        c.cmd(0x43, bytes.fromhex("a1b2c3d4"))
        c.cmd(0x52, bytes([0, 0, 15]))
        snap(c, root)
        c.cmd(0x55, bytes([9, 0]) + bytes([0x10] * 16))
        snap(c, root)
        c.cmd(0x55, bytes([4, 0]) + bytes(16))
        c.cmd(0x53, bytes([20, 0, 4]) + bytes(range(64)))
        c.cmd(0x49, bytes([5, 0, 1, 0]))
        c.cmd(0x54, bytes([5, 0]))
        c.cmd(0x50)
        snap(c, root)
    run("card_file", files={"club.bin": card2}, card="club.bin", steps=s2)

    # 3. a card file that does not exist yet: made on the first save, no backup
    def s3(c, root):
        c.cmd(0x4C)
        snap(c, root)
        c.cmd(0x55, bytes([7, 0]) + b"\x42" * 16)
        snap(c, root)
        c.cmd(0x49, bytes([5, 0, 2, 0]))
    run("card_file_missing", card="data/club.bin", steps=s3, files={"data/keep.txt": b"x"})

    # 4. the link layer: split writes, strays, a restart in the middle, a wrong bcc, several commands in one write
    def s4(c, root):
        c.tx(bytes([0x55]))                           # a stray byte
        c.tx(bytes([STX, STX]))                       # the game started over
        c.tx(msg(0x41))
        c.tx(bytes([DLE]))
        c.tx(b"\x00")
        c.tx(bytes([STX]))
        c.tx(msg(0x54, bytes([6, 0])), 5)             # split inside the message
        c.tx(bytes([DLE]))
        c.tx(b"\x00")
        c.tx(bytes([STX]))
        c.tx(msg(0x54, bytes([0x10, 0x10])), 7)       # split inside the doubled 10s
        c.tx(bytes([DLE]))
        c.tx(b"\x00")
        bad = bytearray(msg(0x42))
        bad[-3] ^= 0x55                               # the bcc
        c.tx(bytes([STX]))
        c.tx(bytes(bad))
        c.tx(bytes([DLE]))
        c.tx(b"\x00")
        c.tx(bytes([STX]) + msg(0x42) + bytes([DLE, 0]))      # everything in one write, the ack last
        c.tx(bytes([STX]) + msg(0x50) + bytes([DLE, 0, STX]) + msg(0x41) + bytes([DLE, 0]))
        c.tx(bytes([STX]))
        c.tx(msg(0x41))
        c.tx(bytes([STX]))                            # a new start while waiting for the 10
        c.tx(msg(0x50))
        c.tx(bytes([DLE]))
        c.tx(bytes([0x77, 0x88]))                     # the ack and a stray
        c.tx(bytes([STX]))
        c.tx(msg(0x4C)[:4])                           # cut half way, then the port is reopened
        c.open()
        c.tx(bytes([STX]))
        c.tx(msg(0x42))
        c.tx(bytes([DLE]))
        c.tx(b"\x00")
        c.cmd(0x52, bytes([0, 0, 15]), 6)
    run("link_layer", steps=s4)

    # 5. a card file of the wrong size: refused, kept as .bad, a NEW card is served, the next save replaces it
    def s5(c, root):
        c.cmd(0x4C)
        c.cmd(0x42)
        c.cmd(0x54, bytes([6, 0]))
        snap(c, root)
        c.cmd(0x55, bytes([9, 0]) + b"\x11" * 16)
        snap(c, root)
    run("wrong_size", files={"club.bin": b"\x01" * 100}, card="club.bin", steps=s5)

    # 6. a manager transfer: the club card has expired (low byte 1), a blank card is put beside it
    def s6(c, root):
        c.cmd(0x4C)
        snap(c, root)
        c.cmd(0x4E)
        c.cmd(0x41)
        c.cmd(0x42)                                   # the club card
        c.cmd(0x43, bytes.fromhex("0badf00d"))
        c.cmd(0x45)                                   # halt it
        c.cmd(0x41)
        c.cmd(0x42)                                   # the blank card
        c.cmd(0x4E)
        # select the new card by the uid the reader drew
        uid = open(os.path.join(root, "new.bin"), "rb").read()[:4]
        c.cmd(0x43, uid)
        c.cmd(0x52, bytes([0, 0, 15]))
        c.cmd(0x55, bytes([9, 0]) + b"\x22" * 16)
        c.cmd(0x49, bytes([5, 0, 5, 0]))              # the new card is written: counter 0xFFFA
        c.cmd(0x43, bytes.fromhex("0badf00d"))
        c.cmd(0x49, bytes([5, 0, 1, 0]))              # the old card is used up: counter 0xFF00
        snap(c, root)
        c.cmd(0x4C)                                   # the next card check: the new card plays alone
        c.cmd(0x4E)
        c.cmd(0x42)
        c.cmd(0x54, bytes([9, 0]))
        c.cmd(0x4C)
        snap(c, root)
    run("transfer", files={"club.bin": synth("c6", 0xFF01, "0badf00d")}, card="club.bin", new="new.bin", steps=s6)

    # 7. a cut transfer: the old card was marked used up, the new one is still blank
    def s7(c, root):
        c.cmd(0x4C)
        c.cmd(0x4E)
        c.cmd(0x42)
        c.cmd(0x43, bytes.fromhex("0badf00d"))
        c.cmd(0x54, bytes([5, 0]))
        snap(c, root)
        c.cmd(0x42)
    run("transfer_cut", files={"club.bin": synth("c7", 0xFF00, "0badf00d"), "new.bin": synth("n7", 0xFFFF, "11223344")},
        card="club.bin", new="new.bin", steps=s7)

    # 8. the second file is a written card; wrong size; a blank card already there; no second file and counter 0
    def s8(c, root):
        c.cmd(0x4C)
        c.cmd(0x4E)
        c.cmd(0x41)
        c.cmd(0x42)
        snap(c, root)
    run("new_is_written", files={"club.bin": synth("c8", 0xFF01, "0badf00d"), "new.bin": synth("n8", 0x0100, "55667788")},
        card="club.bin", new="new.bin", steps=s8)
    run("new_wrong_size", files={"club.bin": synth("c9", 0xFF01, "0badf00d"), "new.bin": b"\x07" * 50},
        card="club.bin", new="new.bin", steps=s8)
    run("new_blank_present", files={"club.bin": synth("c10", 0xFF01, "0badf00d"), "new.bin": synth("n10", 0xFFFF, "55667788")},
        card="club.bin", new="new.bin", steps=s8)
    run("used_up_no_second", files={"club.bin": synth("c11", 0xFF00, "0badf00d")}, card="club.bin", new="new.bin", steps=s8)

    # 9. the new file appears after the reader started (the player's tools add it)
    def s9(c, root):
        c.cmd(0x4C)
        c.cmd(0x42)
        put(c, root, "new.bin", synth("n12", 0x0100, "99aabbcc"))
        c.cmd(0x4C)
        c.cmd(0x42)
        snap(c, root)
    run("new_appears", files={"club.bin": synth("c12", 0xFF00, "0badf00d")}, card="club.bin", new="new.bin", steps=s9)

    # 10. backup rotation: 25 old backups, one more
    def s10(c, root):
        c.cmd(0x55, bytes([9, 0]) + b"\x33" * 16)
        snap(c, root)
        c.cmd(0x55, bytes([10, 0]) + b"\x34" * 16)
        snap(c, root)
    seeded = {"backup/club.bin.20200101-%06d.bak" % i: bytes([i]) * 8 for i in range(25)}
    seeded["backup/other.bin.20200101-000000.bak"] = b"o"
    seeded["backup/club.bin.notes.txt"] = b"n"
    seeded["club.bin"] = synth("c13", 0x0042, "0badf00d")
    run("backup_rotation", files=seeded, card="club.bin", steps=s10)

    # 11. a reader answers a game that reconnects, with the card state kept
    def s11(c, root):
        c.cmd(0x55, bytes([9, 0]) + b"\x44" * 16)
        c.cmd(0x45)
        c.open()
        c.cmd(0x41)
        c.cmd(0x54, bytes([9, 0]))
    run("reconnect", files={"club.bin": synth("c14", 0x0200, "0badf00d")}, card="club.bin", steps=s11)

    os.urandom = real_urandom
    with open(OUT, "w") as f:
        f.writelines(all_lines)
    sys.stderr.write("card_reader: %d lines -> %s\n" % (len(all_lines), OUT))
    os._exit(0)                                       # the reader threads wait on their pipes forever


if __name__ == "__main__":
    main()
