# -*- coding: utf-8 -*-
"""Throwaway recorder (step 10, piece J1): drives scripts/_aimedb.py (needs Windows: CNG) with synthetic packets
on an ephemeral port and writes crates/forza/tests/rec/aimedb.jsonl (on-the-wire AES bytes, hex).
    python rec_j1_aimedb.py OUT.jsonl"""
import json, os, socket, struct, sys, tempfile, threading, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "kit", "scripts"))
import _aimedb as a

aes = a.Aes(a.KEY)
db = a.Db(os.path.join(tempfile.mkdtemp(), "aimedb.json"))
srv = socket.socket()
srv.bind(("127.0.0.1", 0))
srv.listen(4)
threading.Thread(target=lambda: [threading.Thread(target=a.serve, args=(c, p, aes, db, lambda m: None),
                                                  daemon=True).start() for c, p in iter(srv.accept, None)],
                 daemon=True).start()


def pkt(cmd, body, length=None, ver=0x3087):
    n = length or (32 + len(body) + 15) // 16 * 16
    raw = a.HDR.pack(a.MAGIC, ver, cmd, n, 0, b"SDDY\0\0", 1234, b"A72E01234567") + body
    return aes.encrypt(raw + bytes(n - len(raw)))


CODE_A, CODE_B = "11112222333344445555", "99998888777766665555"
code = lambda s: bytes.fromhex(s)
conns = [
    [pkt(0x64, b""), pkt(0x04, code(CODE_A) + bytes(6)), pkt(0x04, code(CODE_B) + bytes(6)),
     pkt(0x04, code(CODE_A) + bytes(6)), pkt(0x05, code("00001111222233334444") + bytes(6)),
     pkt(0x0F, code(CODE_B) + bytes(6)), pkt(0x01, bytes.fromhex("0123456789abcdef") + bytes(8)),
     pkt(0x11, bytes.fromhex("0123456789abcdef") + bytes(8)), pkt(0x0B, bytes(16)), pkt(0x0D, bytes(16)),
     pkt(0x09, bytes(16)), pkt(0x13, bytes(16)), pkt(0x77, bytes(16)), pkt(0x66, b"")],
    [pkt(0x04, code(CODE_B) + bytes(6)), pkt(0x04, code("12345678901234567890") + bytes(6))],
    [b"\x00" * 16],                                      # bad start: the service closes
]
with open(sys.argv[1], "w") as out:
    for i, reqs in enumerate(conns):
        c = socket.create_connection(srv.getsockname())
        c.settimeout(0.4)
        for req in reqs:
            c.sendall(req[:20]); time.sleep(0.05); c.sendall(req[20:])   # a split read each time
            resp = b""
            while True:
                try:
                    d = c.recv(65536)
                except socket.timeout:
                    break
                if not d:
                    break
                resp += d
            out.write(json.dumps({"conn": i, "req": req.hex(), "resp": resp.hex()}) + "\n")
        c.close()
print("aimedb ok")
