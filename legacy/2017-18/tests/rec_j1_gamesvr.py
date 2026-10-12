# -*- coding: utf-8 -*-
"""Throwaway recorder (step 10, piece J1): drives scripts/_gamesvr.py with synthetic requests on an ephemeral
port and writes crates/forza/tests/rec/gamesvr.jsonl. Pure stdlib: runs with any Python 3.
    python rec_j1_gamesvr.py OUT.jsonl"""
import hashlib, json, os, socket, struct, sys, tempfile, threading, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "kit", "scripts"))
import _gamesvr as g

g.LOG = open(os.devnull, "w")
save = tempfile.mkdtemp()
srv = socket.socket()
srv.bind(("127.0.0.1", 0))
srv.listen(4)
threading.Thread(target=lambda: [threading.Thread(target=g.serve, args=(c, a, save), daemon=True).start()
                                 for c, a in iter(srv.accept, None)], daemon=True).start()
c = socket.create_connection(srv.getsockname())
c.settimeout(0.4)


def recv_all():
    out = b""
    while True:
        try:
            d = c.recv(65536)
        except socket.timeout:
            return out
        if not d:
            return out
        out += d


def frame(cmd, seq, payload, conn=3, text=b"SYNTHETIC"):
    return g.header(cmd, seq, 0, len(payload), conn, text) + payload


reqs = []   # (bytes, split)
for cmd in [201, 223, 225, 227, 233, 237, 239, 243, 245, 247, 249, 255, 257, 259, 261, 263, 265, 267, 271, 273, 275,
            277, 279, 281, 283, 285, 287, 289, 291, 293, 295, 297, 299, 301, 303, 305, 307, 309, 311, 313, 315, 317]:
    reqs.append((frame(cmd, cmd & 0xFFFF, struct.pack("<II", 5 + cmd % 3, 123456 + cmd) + bytes(range(12))), 0))
reqs.append((frame(18, 1, b"\x09\x08\x07\x06\x05"), 0))                 # echo
reqs.append((frame(18, 2, b"\x01"), 0))                                 # echo, short payload
reqs.append((frame(9999, 3, b"\x01\x02\x03\x04"), 0))                   # unknown: no answer
reqs.append((frame(271, 4, b""), 0))                                    # no payload at all
reqs.append((frame(277, 5, struct.pack("<I", 7)), 0))                   # SAVE_DATA_SET with 4 bytes
reqs.append((frame(271, 6, struct.pack("<II", 9, 1) + bytes(8)), 20))   # split inside the header
reqs.append((frame(225, 7, struct.pack("<II", 2, 1)), 45))              # split inside the payload
reqs.append((b"\xaa" * 45 + frame(227, 8, struct.pack("<II", 3, 1)), 0))  # junk, resync
reqs.append((frame(227, 9, struct.pack("<II", 4, 1)) + frame(225, 10, struct.pack("<II", 6, 1)), 0))  # two frames

with open(sys.argv[1], "w") as out:
    for req, split in reqs:
        if split:
            c.sendall(req[:split]); time.sleep(0.15); c.sendall(req[split:])
        else:
            c.sendall(req)
        time.sleep(0.05)
        resp = recv_all()
        rec = {"req": req.hex(), "split": split, "resp_len": len(resp),
               "resp_sha256": hashlib.sha256(resp).hexdigest()}
        if len(resp) <= 400:
            rec["resp"] = resp.hex()
        out.write(json.dumps(rec) + "\n")
print("gamesvr: %d records" % len(reqs))
