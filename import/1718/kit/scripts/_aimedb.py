# -*- coding: utf-8 -*-
"""Stand-in for Sega's AiMeDB (Aime card -> Aime ID), for WCCF 2017-18 on one PC.

    python scripts/_aimedb.py PORT SECONDS LOGFILE DBFILE

The client's aimelib connects to aime.naominet.jp:22345; seat 1's exe names localhost instead
(tools/patches1718.py). Packets are AES-128-ECB with the key "Copyright(C)SEGA" (client VA 0x00D8AC30), with a
32-byte header: u16 magic 0xA13E, u16 version, u16 command, u16 length (whole packet), u16 status (1 = ok),
char game_id[6], u32 store_id, char keychip_id[12]. Commands follow the public Artemis server: 0x04 lookup
(access code) -> 0x06, 0x05 register -> 0x06, 0x01 FeliCa lookup -> 0x03, 0x09 log -> 0x0A, 0x0B campaign -> 0x0C,
0x0D touch -> 0x0E, 0x0F lookup2 -> 0x10, 0x11 FeliCa lookup2 -> 0x12, 0x13 log2 -> 0x14, 0x64 hello -> 0x65,
0x66 goodbye. Every packet is logged decrypted.
DBFILE (JSON, written atomically) maps access codes to Aime IDs; an unknown card gets a new ID on lookup, so a
card always has one club owner. Exit 0 after SECONDS, 2 when the port is taken.
"""
import ctypes
import ctypes.wintypes as w
import json
import os
import socket
import struct
import sys
import threading
import time

KEY = b"Copyright(C)SEGA"
HDR = struct.Struct("<HHHHH6sI12s")             # 32 bytes
MAGIC = 0xA13E

bc = ctypes.WinDLL("bcrypt")


class Aes:
    """AES-128-ECB through Windows CNG (no extra Python packages)."""

    def __init__(self, key):
        self.alg, self.key = w.HANDLE(), w.HANDLE()
        if bc.BCryptOpenAlgorithmProvider(ctypes.byref(self.alg), "AES", None, 0):
            raise OSError("BCryptOpenAlgorithmProvider")
        mode = ctypes.create_unicode_buffer("ChainingModeECB")
        bc.BCryptSetProperty(self.alg, "ChainingMode", mode, ctypes.sizeof(mode), 0)
        if bc.BCryptGenerateSymmetricKey(self.alg, ctypes.byref(self.key), None, 0, key, len(key), 0):
            raise OSError("BCryptGenerateSymmetricKey")

    def _run(self, fn, data):
        out = ctypes.create_string_buffer(len(data))
        n = w.ULONG()
        if fn(self.key, data, len(data), None, None, 0, out, len(data), ctypes.byref(n), 0):
            raise OSError("AES")
        return out.raw[:n.value]

    def encrypt(self, data):
        return self._run(bc.BCryptEncrypt, data)

    def decrypt(self, data):
        return self._run(bc.BCryptDecrypt, data)


class Db:
    def __init__(self, path):
        self.path, self.lock = path, threading.Lock()
        self.cards = {}
        if os.path.exists(path):
            self.cards = json.load(open(path, encoding="utf-8"))

    def aime_id(self, code, create=True):
        with self.lock:
            if code not in self.cards and create:
                self.cards[code] = max(self.cards.values(), default=0) + 1
                tmp = self.path + ".tmp"
                with open(tmp, "w", encoding="utf-8") as f:
                    json.dump(self.cards, f, indent=1)
                    f.flush()
                    os.fsync(f.fileno())
                os.replace(tmp, self.path)
            return self.cards.get(code, -1)


def reply(hdr, cmd, length, body=b""):
    magic, ver, _, _, _, game, store, keychip = hdr
    pkt = HDR.pack(MAGIC, ver, cmd, length, 1, game, store, keychip) + body
    return pkt + bytes(length - len(pkt))


def handle(hdr, data, db, log):
    cmd = hdr[2]
    if cmd in (0x04, 0x05, 0x0F):                 # access code (10 bytes BCD) at 0x20
        code = data[0x20:0x2A].hex()
        aid = db.aime_id(code)
        log("   access code %s -> Aime ID %d" % (code, aid))
        n = 0x130 if cmd == 0x0F else 0x30
        return reply(hdr, 0x10 if cmd == 0x0F else 0x06, n, struct.pack("<iB", aid, 0))
    if cmd in (0x01, 0x11):                        # FeliCa: IDm at 0x20 -> its own access code
        idm = data[0x20:0x28]
        code = "%020d" % int.from_bytes(idm, "big")
        aid = db.aime_id(code)
        log("   FeliCa IDm %s -> access code %s, Aime ID %d" % (idm.hex(), code, aid))
        if cmd == 0x01:
            return reply(hdr, 0x03, 0x30, struct.pack("<I", 0) + bytes.fromhex(code))
        return reply(hdr, 0x12, 0x140, struct.pack("<iB", aid, 0) + bytes(0x2C - 5) + bytes.fromhex(code))
    if cmd == 0x0B:
        return reply(hdr, 0x0C, 0x200)
    if cmd == 0x0D:
        return reply(hdr, 0x0E, 0x50, struct.pack("<I", 0x6F) + struct.pack("<I", 1))
    if cmd in (0x09, 0x13):
        return reply(hdr, cmd + 1, 0x20)
    if cmd == 0x64:
        return reply(hdr, 0x65, 0x20)
    if cmd == 0x66:
        return None
    log("   UNKNOWN command 0x%02X: answered header-only as 0x%02X" % (cmd, cmd + 1))
    return reply(hdr, cmd + 1, 0x20)


def serve(conn, peer, aes, db, log):
    log("connection from %s:%d" % peer)
    conn.settimeout(30)
    buf = b""
    try:
        while True:
            while len(buf) < 16:
                more = conn.recv(4096)
                if not more:
                    log("closed by the game")
                    return
                buf += more
            head = aes.decrypt(buf[:16])
            magic, ver, cmd, length = struct.unpack_from("<HHHH", head)
            if magic != MAGIC or length < 32 or length % 16:
                log("bad packet start %s (decrypted %s) - closing" % (buf[:16].hex(), head.hex()))
                return
            while len(buf) < length:
                more = conn.recv(4096)
                if not more:
                    return
                buf += more
            data, buf = aes.decrypt(buf[:length]), buf[length:]
            hdr = HDR.unpack_from(data)
            log("<- cmd 0x%02X ver 0x%04X len 0x%X game %r store %d keychip %r body %s" % (
                cmd, ver, length, hdr[5], hdr[6], hdr[7], data[32:].hex()))
            out = handle(hdr, data, db, log)
            if out is None:
                log("   goodbye")
                return
            log("-> cmd 0x%02X len 0x%X" % (struct.unpack_from("<H", out, 4)[0], len(out)))
            conn.sendall(aes.encrypt(out))
    except OSError as e:
        log("connection error: %s" % e)
    finally:
        conn.close()


def selftest():
    """python _aimedb.py selftest - AES known answer (FIPS-197 C.1) and a lookup round trip."""
    a = Aes(bytes(range(16)))
    ct = a.encrypt(bytes.fromhex("00112233445566778899aabbccddeeff"))
    assert ct.hex() == "69c4e0d86a7b0430d8cdb78070b4c55a", ct.hex()
    assert a.decrypt(ct).hex() == "00112233445566778899aabbccddeeff"
    db = Db(os.path.join(os.environ.get("TEMP", "."), "aimedb_selftest.json"))
    db.cards = {}
    req = HDR.pack(MAGIC, 0x3087, 0x04, 0x30, 0, b"SDDY\0\0", 1, b"A72E01234567") + bytes.fromhex("01" * 10) + bytes(6)
    out = handle(HDR.unpack_from(req), req, db, lambda m: None)
    assert struct.unpack_from("<HHHHH", out) == (MAGIC, 0x3087, 0x06, 0x30, 1) and len(out) == 0x30
    assert struct.unpack_from("<i", out, 32)[0] == 1
    os.remove(db.path)
    print("selftest ok")


def main(argv):
    if argv[:1] == ["selftest"]:
        selftest()
        return 0
    if len(argv) < 4:
        print(__doc__)
        return 2
    port, seconds, logname, dbname = int(argv[0]), int(argv[1]), argv[2], argv[3]
    logf = open(logname, "a", encoding="utf-8")
    t0 = time.time()
    lock = threading.Lock()

    def log(msg):
        with lock:
            line = "%8.3f  %s" % (time.time() - t0, msg)
            print(line, flush=True)
            logf.write(line + "\n")
            logf.flush()

    aes, db = Aes(KEY), Db(dbname)
    s = socket.socket()
    try:
        s.bind(("127.0.0.1", port))
    except OSError as e:
        log("cannot listen on 127.0.0.1:%d: %s" % (port, e))
        return 2
    s.listen(4)
    s.settimeout(1)
    log("AiMeDB stand-in on 127.0.0.1:%d for %d s; %d known cards (%s)" % (port, seconds, len(db.cards), dbname))
    while time.time() - t0 < seconds:
        try:
            conn, peer = s.accept()
        except socket.timeout:
            continue
        threading.Thread(target=serve, args=(conn, peer, aes, db, log), daemon=True).start()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
