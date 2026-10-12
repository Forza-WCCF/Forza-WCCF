# -*- coding: utf-8 -*-
"""Local stand-in for the publisher's WCCF 2017-18 game server ("gmsvr"), which control_Release.exe relays the cabinets'
club requests to (control.c: NetSvrConnect / NetSvrReqmngGame; set GAME_SVR_IP=localhost, GAME_SVR_PORT, FORCE_ONLINE=1).

    python _gamesvr.py PORT SECONDS LOGFILE SAVEDIR

Framing (control 0x40A970 build, 0x40ABE0 check, 0x40AB80 checksum), 40-byte header, little-endian:
    0  u32 0x4480C0F1   4  u32 0x3CDA9EB4      magic
    8  u16 command     10  u16 sequence        12 u32 status (1 = ok; a response's payload is used only if > 0)
   16  u32 total size (header + payload)       20 u32 connection id
   24  u8  checksum = sum of the 40 header bytes with this one 0, mod 256     25 char[15] text
Request N is answered by N + 1, matched on the payload's first u32 (the cabinet's slot) - msg_table.txt.
This version logs every message and answers each request with an all-zero payload of the right size (status 1);
SAVE_DATA_SET payloads are kept in SAVEDIR. Exit 0 after SECONDS.
"""
import os
import socket
import struct
import sys
import threading
import time

MAGIC = (0x4480C0F1, 0x3CDA9EB4)
# response payload sizes (control.c response dispatcher 0x408C50 and the message table)
RESP = {202: 16, 224: 92, 226: 260, 228: 12, 234: 1092, 238: 0x5A54, 240: 12, 244: 12, 246: 248, 248: 12, 250: 504,
        256: 1008, 258: 12, 260: 10168, 262: 2040, 264: 10080, 266: 200, 268: 33600, 272: 9292, 274: 1288,
        276: 6176, 278: 76, 280: 16, 282: 8, 284: 8, 286: 704, 288: 12, 290: 12, 292: 20, 294: 8, 296: 16, 298: 16,
        300: 8, 302: 8, 304: 8, 306: 1004, 308: 3332, 310: 8, 312: 3352, 314: 8, 316: 8, 318: 32}
NAMES = {}
LOCK = threading.Lock()
T0 = time.time()
LOG = None


def log(s):
    with LOCK:
        LOG.write("%8.2f  %s\n" % (time.time() - T0, s))
        LOG.flush()


def checksum(h):
    h = bytearray(h)
    h[24] = 0
    return sum(h) & 0xFF


def header(cmd, seq, status, size, conn, text=b"000000000000000"):
    h = bytearray(40)
    struct.pack_into("<IIHHIII", h, 0, MAGIC[0], MAGIC[1], cmd, seq, status, 40 + size, conn)
    h[25:40] = text[:15].ljust(15, b"\0")
    h[24] = checksum(h)
    return bytes(h)


def answer(cmd, seq, conn, payload, savedir):
    """the response to one request: (command, payload) or None"""
    if cmd == 18:                                   # GMSVR_ECHO keep-alive
        return 18, payload[:4].ljust(4, b"\0")
    rcmd = cmd + 1
    if rcmd not in RESP:
        return None
    body = bytearray(RESP[rcmd])
    if len(body) >= 4 and len(payload) >= 4:
        body[0:4] = payload[0:4]                    # the cabinet's slot: how control matches the answer
    if cmd == 277:                                  # SAVE_DATA_SET: keep it
        os.makedirs(savedir, exist_ok=True)
        aime = struct.unpack_from("<I", payload, 4)[0] if len(payload) >= 8 else 0
        with open(os.path.join(savedir, "save_set_%u_%d.bin" % (aime, int(time.time()))), "wb") as f:
            f.write(payload)
    return rcmd, bytes(body)


def serve(conn_sock, addr, savedir):
    log("connected %s:%d" % addr)
    buf = b""
    conn_sock.settimeout(1.0)
    while True:
        try:
            d = conn_sock.recv(65536)
        except socket.timeout:
            continue
        except OSError as ex:
            log("recv error %s" % ex)
            break
        if not d:
            break
        buf += d
        while len(buf) >= 40:
            m0, m1, cmd, seq, status, size, conn = struct.unpack_from("<IIHHIII", buf, 0)
            if (m0, m1) != MAGIC:
                i = buf.find(struct.pack("<II", *MAGIC), 1)
                log("resync: dropped %d bytes" % (len(buf) if i < 0 else i))
                buf = b"" if i < 0 else buf[i:]
                continue
            if len(buf) < size:
                break
            payload = buf[40:size]
            text = buf[25:40].split(b"\0")[0]
            buf = buf[size:]
            log("<- cmd %d seq %d status %d payload %d conn %d text %r first %s" % (
                cmd, seq, status, len(payload), conn, text, payload[:48].hex()))
            if payload:
                with open(os.path.join(savedir, "req_%d_%d.bin" % (cmd, int((time.time() - T0) * 1000))), "wb") as f:
                    f.write(payload)
            r = answer(cmd, seq, conn, payload, savedir)
            if r:
                rcmd, body = r
                conn_sock.sendall(header(rcmd, seq, 1, len(body), conn) + body)
                log("-> cmd %d payload %d" % (rcmd, len(body)))
            else:
                log("   (no answer for cmd %d)" % cmd)
    log("closed %s:%d" % addr)
    conn_sock.close()


def main(argv):
    global LOG
    port, seconds, logfile, savedir = int(argv[0]), float(argv[1]), argv[2], argv[3]
    os.makedirs(savedir, exist_ok=True)
    LOG = open(logfile, "a", encoding="utf-8")
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port))
    s.listen(4)
    s.settimeout(1.0)
    log("game server stand-in listening on 127.0.0.1:%d" % port)
    end = time.time() + seconds
    while time.time() < end:
        try:
            c, a = s.accept()
        except socket.timeout:
            continue
        threading.Thread(target=serve, args=(c, a, savedir), daemon=True).start()
    return 0


def selftest():
    h = header(271, 7, 1, 20, 3)
    assert struct.unpack_from("<II", h) == MAGIC and h[24] == checksum(h) and struct.unpack_from("<I", h, 16)[0] == 60
    cmd, body = answer(271, 7, 3, struct.pack("<II", 5, 1234) + bytes(12), ".")
    assert cmd == 272 and len(body) == 9292 and body[:4] == struct.pack("<I", 5)
    print("selftest ok")


if __name__ == "__main__":
    if sys.argv[1:] == ["--selftest"]:
        selftest()
    else:
        sys.exit(main(sys.argv[1:]))
