# -*- coding: utf-8 -*-
"""Throwaway recorder (step 10, piece J1): drives scripts/_aime_reader.py (needs Windows: named pipe) with synthetic
frames on a pipe named after this process and writes crates/forza/tests/rec/aime.jsonl. The first line holds the
synthetic card's access code; the Rust replay writes the same card file.
    python rec_j1_aime.py OUT.jsonl"""
import ctypes, ctypes.wintypes as w, json, os, sys, tempfile, threading, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "kit", "scripts"))
import _aime_reader as r

CODE = "51234567890123456789"
pipe = r"\\.\pipe\rec_j1_aime_%d" % os.getpid()
card = os.path.join(tempfile.mkdtemp(), "card.txt")
open(card, "w").write(CODE + "\n")
threading.Thread(target=r.main, args=([pipe, "12", "NUL", card],), daemon=True).start()
k = r.k
k.CreateFileW.restype = w.HANDLE
k.CreateFileW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, ctypes.c_void_p, w.DWORD, w.DWORD, w.HANDLE]


def connect():
    for _ in range(100):
        h = k.CreateFileW(pipe, 0xC0000000, 0, None, 3, 0, None)
        if h != r.INVALID:
            return h
        time.sleep(0.05)
    raise SystemExit("no pipe")


def frame(addr, seq, cmd, payload=b""):
    body = bytes([5 + len(payload), addr, seq, cmd, len(payload)]) + payload
    return r.escape(body + bytes([sum(body) & 0xFF]))


UID = bytes.fromhex(CODE[-8:])
good = [frame(0, 1, 0x30), frame(0, 2, 0x32), frame(0, 3, 0x40), frame(0, 4, 0x42), frame(0, 5, 0x43, UID),
        frame(0, 6, 0x54, bytes(range(6))), frame(0, 7, 0x55, UID + b"\x02"), frame(0, 8, 0x52, UID + b"\x00"),
        frame(0, 9, 0x52, UID + b"\x02"), frame(0, 10, 0x52, UID + b"\x05"), frame(0, 11, 0x52, UID),
        frame(0, 12, 0x61, b"\xe0\xd0\x01"), frame(0, 0xD0, 0x62), frame(0, 14, 0x71, bytes(3)),
        frame(8, 15, 0xF0), frame(8, 16, 0x81, b"\xff\x00\x10"), frame(8, 17, 0xF5), frame(8, 18, 0x42)]
bad_sum = bytearray(frame(0, 20, 0x30)); bad_sum[-1] ^= 0x55
sessions = [
    [(g, 0) for g in good] + [(bytes(bad_sum), 0), (b"\x01\x02\x03" + frame(0, 21, 0x32), 0),
                              (frame(0, 22, 0x30) + frame(0, 23, 0x32), 0), (frame(0, 24, 0x42, UID), 4),
                              (frame(0, 25, 0x61, b"\xd0\xe0"), 7)],
    [(frame(0, 1, 0x30), 0), (frame(0, 2, 0x42), 0)],        # the game reopened the port
]
n = w.DWORD(); avail = w.DWORD(); buf = (ctypes.c_char * 4096)()
with open(sys.argv[1], "w") as out:
    out.write(json.dumps({"card": CODE}) + "\n")
    for i, reqs in enumerate(sessions):
        h = connect()
        for req, split in reqs:
            if split:
                k.WriteFile(h, req[:split], split, ctypes.byref(n), None); time.sleep(0.1)
                k.WriteFile(h, req[split:], len(req) - split, ctypes.byref(n), None)
            else:
                k.WriteFile(h, req, len(req), ctypes.byref(n), None)
            time.sleep(0.2)
            resp = b""
            while k.PeekNamedPipe(h, None, 0, None, ctypes.byref(avail), None) and avail.value:
                k.ReadFile(h, buf, min(avail.value, 4096), ctypes.byref(n), None)
                resp += bytes(buf.raw[:n.value]); time.sleep(0.05)
            out.write(json.dumps({"conn": i, "req": req.hex(), "split": split, "resp": resp.hex()}) + "\n")
        k.CloseHandle(h)
        time.sleep(0.5)
print("aime ok")
