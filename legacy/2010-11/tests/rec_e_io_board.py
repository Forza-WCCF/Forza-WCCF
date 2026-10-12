# -*- coding: utf-8 -*-
"""rec_e_io_board.py - records io_board.py on a scratch pipe into crates/forza/tests/rec/io_board.jsonl, which
crates/forza/tests/io_board.rs replays against forza::standins::io_board.
Step 10 piece E: kept until piece G deletes the Python it records (play.py still starts io_board.py and keyboard.py).
Starts io_board.py as a child on \\\\.\\pipe\\wccf_rec_e_<pid> with a scratch input file (never the game's pipe or
files), sends scripted JVS packets and stores each reply and the board's outputs file after it.  The child is killed at
the end.  Run with the staged kit's Python:
    python legacy\\2010-11\\tests\\rec_e_io_board.py"""
import ctypes
import ctypes.wintypes as w
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
CODE = os.path.join(os.path.dirname(HERE), "code")
OUT = os.path.join(REPO, "crates", "forza", "tests", "rec", "io_board.jsonl")
SYNC, MARK = 0xE0, 0xD0

k = ctypes.WinDLL("kernel32", use_last_error=True)
k.CreateFileW.restype = w.HANDLE
k.CreateFileW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, ctypes.c_void_p, w.DWORD, w.DWORD, w.HANDLE]
k.ReadFile.argtypes = [w.HANDLE, ctypes.c_void_p, w.DWORD, ctypes.POINTER(w.DWORD), ctypes.c_void_p]
k.WriteFile.argtypes = [w.HANDLE, ctypes.c_void_p, w.DWORD, ctypes.POINTER(w.DWORD), ctypes.c_void_p]
k.PeekNamedPipe.argtypes = [w.HANDLE, ctypes.c_void_p, w.DWORD, ctypes.c_void_p, ctypes.POINTER(w.DWORD), ctypes.c_void_p]


def esc(body):
    out = bytearray([SYNC])
    for b in body:
        out += bytes([MARK, b - 1]) if b in (SYNC, MARK) else bytes([b])
    return bytes(out)


def pkt(node, data, sum_add=0):
    data = bytes(data)
    n = len(data) + 1
    return esc(bytes([node, n]) + data + bytes([(node + n + sum(data) + sum_add) & 0xFF]))


def state(sys_=0, p1=(0, 0), p2=(0, 0), coin=(0, 0)):
    return {"sys": sys_, "p1": list(p1), "p2": list(p2), "coin": list(coin)}


def script():
    s = []
    send = lambda b, split=0: s.append({"send": bytes(b).hex(), "split": split})  # noqa: E731
    st = lambda **kw: s.append({"state": state(**kw)})  # noqa: E731
    P = pkt
    send(P(0xFF, [0x2F]))                                  # retransmit before any reply: nothing
    send(P(0x01, [0x10]))                                  # no address yet: nothing
    send(P(0xFF, [0xF0, 0xD9]))                            # reset: nothing
    send(P(0xFF, [0xF1, 0x01]))                            # set address 1
    send(P(0x01, [0x10, 0x11, 0x12, 0x13, 0x14]))
    send(P(0x01, [0x15] + list(b"NAMCO TEST\x00") + [0x10]))   # main board id, then a command
    send(P(0x01, [0x15] + list(b"no nul")))                # up to the end
    send(P(0x01, [0x15, 0x00, 0x11]))
    send(P(0x02, [0x10]))                                  # not our node
    st()
    for req in ([0x20, 2, 13], [0x20, 2, 2], [0x20, 1, 1], [0x20, 3, 2], [0x20, 0, 0], [0x20, 2, 4],
                [0x21, 2], [0x21, 3], [0x21, 0], [0x22, 8], [0x22, 0], [0x20, 2, 2, 0x21, 2, 0x22, 1]):
        send(P(0x01, req))
    st(sys_=0x80, p1=(0x82, 0x40), p2=(0x20, 0x80), coin=(5, 0x3FFF))
    for req in ([0x20, 2, 2], [0x20, 2, 13], [0x21, 2]):
        send(P(0x01, req))
    st(sys_=0xE0, p1=(0xE0, 0xD0), p2=(0xD0, 0xE0), coin=(0xE0, 0xD0))   # every byte that needs escaping
    for req in ([0x20, 2, 2], [0x21, 2]):
        send(P(0x01, req))
    st(coin=(0x1234, 3))
    send(P(0x01, [0x21, 2]))
    send(P(0x01, [0x2F]))                                  # retransmit: the reply just sent
    send(P(0x01, [0x30, 1, 0x00, 0x05, 0x21, 2]))          # coin decrease, then read
    send(P(0x01, [0x31, 2, 0x01, 0x00, 0x21, 2]))
    send(P(0x01, [0x30, 2, 0xFF, 0xFF, 0x21, 2]))          # clamps at 0
    send(P(0x01, [0x30, 0, 0, 1, 0x30, 3, 0, 1, 0x21, 2]))  # slots outside 1..2 do nothing
    send(P(0x01, [0x2F]))
    send(P(0x01, [0x32, 3, 0xAA, 0xBB, 0xCC]))
    send(P(0x01, [0x32, 3, 0xAA, 0xBB, 0xCC]))             # unchanged
    send(P(0x01, [0x32, 3, 0xAA, 0xBB, 0xCD, 0x10]))
    send(P(0x01, [0x32, 0]))
    send(P(0x01, [0x32, 5, 1, 2]))                         # fewer bytes than said
    send(P(0x01, [0x37, 2, 0x38, 3, 1, 2, 3, 0x10]))
    send(P(0x01, [0x38, 5, 1]))
    send(P(0x01, [0x99]))                                  # unknown
    send(P(0x01, [0x10, 0x99, 0x10]))                      # unknown after a good one: status 2, first report kept
    send(P(0x01, [0x20, 2]))                               # truncated commands are unknown
    send(P(0x01, [0x21]))
    send(P(0x01, [0x30, 1, 0]))
    send(P(0x01, [0x10, 0xF1]))
    send(P(0x01, [0x10, 0xF0]))
    send(P(0x01, [0x10, 0xF1, 0x05, 0x11]))                # address change mid packet
    send(P(0x01, [0x10]))                                  # old address: nothing
    send(P(0x05, [0x10]))
    send(P(0x05, [0x20, 2, 2]), split=3)                   # two writes
    send(P(0x05, [0x20, 2, 2]), split=7)
    send(P(0x05, [0x20, 2, 2]) + P(0x05, [0x21, 2]) + P(0x05, [0x11]))   # three packets in one write
    send(b"\x00\x01junk" + P(0x05, [0x11]))                # junk before the sync
    send(P(0x05, [0x11], sum_add=1))                       # wrong checksum: still answered
    send(bytes([SYNC, 0x05, 0x00, 0x05]))                  # length 0
    send(P(0xFF, [0xF0, 0xD9]))                            # reset
    send(P(0x05, [0x10]))                                  # gone
    send(P(0xFF, [0x2F]))                                  # retransmit after a reset: the last reply
    send(P(0xFF, [0xF1, 0x07, 0x11]))
    send(P(0x07, [0x2F]))
    return s


def main():
    tmp = tempfile.mkdtemp(prefix="forza-rec-io-")
    pipe = r"\\.\pipe\wccf_rec_e_%d" % os.getpid()
    inp = os.path.join(tmp, "in_input.txt")
    outp = os.path.join(tmp, "in_outputs.txt")

    def put(st):
        with open(inp, "w") as f:
            f.write("sys=0x%02x\np1=0x%02x 0x%02x\np2=0x%02x 0x%02x\ncoin1=%d\ncoin2=%d\n"
                    % (st["sys"], *st["p1"], *st["p2"], *st["coin"]))
    put(state())
    env = dict(os.environ, PYTHONPATH=CODE)
    child = subprocess.Popen([sys.executable, os.path.join(CODE, "hardware", "io_board.py"), pipe, "300",
                              os.path.join(tmp, "log.txt"), inp], env=env, stdout=subprocess.DEVNULL)
    try:
        h = None
        for _ in range(300):
            h = k.CreateFileW(pipe, 0xC0000000, 0, None, 3, 0, None)
            if h and h != w.HANDLE(-1).value:
                break
            time.sleep(0.1)
        else:
            raise SystemExit("cannot open " + pipe)
        recs = []
        for step in script():
            if "state" in step:
                put(step["state"])
                time.sleep(0.06)                         # a new modification time
                recs.append(step)
                continue
            raw = bytes.fromhex(step["send"])
            parts = [raw[:step["split"]], raw[step["split"]:]] if step["split"] else [raw]
            for i, part in enumerate(parts):
                n = w.DWORD()
                k.WriteFile(h, part, len(part), ctypes.byref(n), None)
                if i + 1 < len(parts):
                    time.sleep(0.15)
            got, t0, last = b"", time.time(), None
            while time.time() - t0 < 0.35 or (last and time.time() - last < 0.08):
                avail = w.DWORD()
                k.PeekNamedPipe(h, None, 0, None, ctypes.byref(avail), None)
                if avail.value:
                    buf = ctypes.create_string_buffer(avail.value)
                    k.ReadFile(h, buf, avail.value, ctypes.byref(n), None)
                    got += buf.raw[:n.value]
                    last = time.time()
                elif last and time.time() - last >= 0.08:
                    break
                else:
                    time.sleep(0.002)
            outputs = None
            try:
                with open(outp) as f:
                    outputs = f.read().splitlines()[1].strip().replace(" ", "")
            except (OSError, IndexError):
                pass
            recs.append({"send": step["send"], "split": step["split"], "reply": got.hex(), "outputs": outputs})
        os.makedirs(os.path.dirname(OUT), exist_ok=True)
        with open(OUT, "w", newline="\n") as f:
            for r in recs:
                f.write(json.dumps(r) + "\n")
        print("recorded %d steps (%d replies) into %s" % (len(recs), sum(1 for r in recs if r.get("reply")), OUT))
    finally:
        child.kill()
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
