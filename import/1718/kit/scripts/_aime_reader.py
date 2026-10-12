# -*- coding: utf-8 -*-
"""Stand-in for WCCF 2017-18's Aime card reader (the publisher's 837-15084 NFC reader, TN32MSEC003S, + its LED board),
served over a named pipe.

    python scripts/_aime_reader.py PIPE_NAME SECONDS [LOGFILE] [CARDFILE]

The client's aimelib 3.07.7 opens "\\\\.\\COM2" (VA 0x00D8B0E0, synchronous); the winmm hook, started with
MXHOOK_COM=COM2=PIPE_NAME, hands it this pipe instead.

Frames (client parser VA 0x00D87040; the same protocol segatools emulates):
  E0 then [len addr seq cmd plen payload.. sum] from the game, [len addr seq cmd status plen payload.. sum] back;
  len counts itself through the payload, sum = byte sum of len..payload; after the E0 every E0/D0 byte is sent as
  D0, byte-1. addr 0 = the NFC reader, commands >= 0x80 go to the LED board at addr 8.
One Mifare Aime card is always on the reader. Its 20-digit access code lives in CARDFILE (a text file, made with
a random code the first time); block 2 bytes 6..15 carry it as BCD, as on a real card.
Exit 0 after SECONDS, 2 when the pipe cannot be made.
"""
import ctypes
import ctypes.wintypes as w
import os
import random
import sys
import time

FW = b"TN32MSEC003S F/W Ver1.2"            # prog_data/firmware/TN32MSEC003S_V12.hex: no firmware update wanted
HW = b"TN32MSEC003S H/W Ver3.0"
LED_INFO = b"15084\xff\x10\x00\x12"
NAMES = {0x30: "fw version", 0x32: "hw version", 0x40: "radio on", 0x41: "radio off", 0x42: "poll",
         0x43: "select tag", 0x44: "?44", 0x50: "set key B", 0x51: "auth B", 0x52: "read block", 0x54: "set key A",
         0x55: "auth A", 0x61: "send hex", 0x62: "reset", 0x71: "felica", 0x81: "led color", 0xF0: "led info",
         0xF5: "led reset"}

k = ctypes.windll.kernel32
k.CreateNamedPipeW.restype = w.HANDLE
k.CreateNamedPipeW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, w.DWORD, w.DWORD, w.DWORD, w.DWORD, ctypes.c_void_p]
k.ConnectNamedPipe.argtypes = [w.HANDLE, ctypes.c_void_p]
k.DisconnectNamedPipe.argtypes = [w.HANDLE]
k.ReadFile.argtypes = [w.HANDLE, ctypes.c_void_p, w.DWORD, ctypes.POINTER(w.DWORD), ctypes.c_void_p]
k.WriteFile.argtypes = [w.HANDLE, ctypes.c_void_p, w.DWORD, ctypes.POINTER(w.DWORD), ctypes.c_void_p]
k.PeekNamedPipe.argtypes = [w.HANDLE, ctypes.c_void_p, w.DWORD, ctypes.c_void_p, ctypes.POINTER(w.DWORD),
                            ctypes.c_void_p]
k.CloseHandle.argtypes = [w.HANDLE]
INVALID = w.HANDLE(-1).value
ERROR_PIPE_CONNECTED = 535


def escape(body):
    out = bytearray([0xE0])
    for b in body:
        out += bytes([0xD0, b - 1]) if b in (0xE0, 0xD0) else bytes([b])
    return bytes(out)


def response(req, status, payload=b""):
    body = bytes([6 + len(payload), req["addr"], req["seq"], req["cmd"], status, len(payload)]) + payload
    return escape(body + bytes([sum(body) & 0xFF]))


def parse(stream):
    """Bytes from the game -> (list of requests, bytes left over). Junk before an E0 is dropped."""
    reqs, i = [], 0
    while True:
        start = stream.find(b"\xe0", i)
        if start < 0:
            return reqs, b""
        body, j = bytearray(), start + 1
        while j < len(stream):
            b = stream[j]
            if b == 0xE0:                    # a new frame started: the old one was cut short
                break
            if b == 0xD0:
                if j + 1 >= len(stream):
                    return reqs, stream[start:]
                body.append(stream[j + 1] + 1)
                j += 2
            else:
                body.append(b)
                j += 1
            if body and len(body) == body[0] + 1:
                break
        if not body or len(body) < body[0] + 1:
            if j >= len(stream):
                return reqs, stream[start:]
            i = j
            continue
        n = body[0]
        reqs.append({"addr": body[1], "seq": body[2], "cmd": body[3], "payload": bytes(body[5:n]),
                     "ok": (sum(body[:n]) & 0xFF) == body[n]})
        i = j


class Card:
    def __init__(self, path):
        self.path = path
        code = None
        if path and os.path.exists(path):
            code = open(path, encoding="ascii").read().strip()
        if not (code and len(code) == 20 and code.isdigit()):
            code = "5" + "".join(random.choice("0123456789") for _ in range(19))
            if path:
                os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
                with open(path + ".tmp", "w", encoding="ascii") as f:
                    f.write(code + "\n")
                os.replace(path + ".tmp", path)
        self.code = code
        self.uid = bytes.fromhex(code[-8:])           # 4-byte Mifare UID, stable per card

    def block(self, n):
        if n == 0:
            return self.uid + bytes([self.uid[0] ^ self.uid[1] ^ self.uid[2] ^ self.uid[3]]) + bytes(11)
        if n == 2:
            return bytes(6) + bytes.fromhex(self.code)
        return bytes(16)


def answer(card, req):
    cmd, p = req["cmd"], req["payload"]
    if cmd == 0x30:
        return 0, FW
    if cmd == 0x32:
        return 0, HW
    if cmd == 0x42:                                    # one Mifare card: count, type 0x10, id length 4, UID
        return 0, bytes([1, 0x10, 4]) + card.uid
    if cmd == 0x52:                                    # payload: UID(4) block(1)
        return 0, card.block(p[4] if len(p) > 4 else 0)
    if cmd == 0xF0:
        return 0, LED_INFO
    if cmd == 0x81:                                    # LED colour: the board does not answer
        return None, b""
    return 0, b""                                      # radio on/off, select, keys, auth, reset, hex data


def selftest():
    """python _aime_reader.py selftest - frame round trip incl. escaped bytes and a split read."""
    body = bytes([5 + 3, 0, 0xD0, 0x42, 3, 0xE0, 0x01, 0xD0])
    raw = escape(body + bytes([sum(body) & 0xFF]))
    reqs, left = parse(raw[:4])
    assert reqs == [] and left == raw[:4]
    reqs, left = parse(left + raw[4:])
    assert left == b"" and len(reqs) == 1 and reqs[0]["ok"], reqs
    assert reqs[0]["cmd"] == 0x42 and reqs[0]["seq"] == 0xD0 and reqs[0]["payload"] == b"\xe0\x01\xd0"
    r = response(reqs[0], 0, b"\xe0")
    assert r[0] == 0xE0 and b"\xd0\xdf" in r[1:]
    print("selftest ok")


def main(argv):
    if argv[:1] == ["selftest"]:
        selftest()
        return 0
    if len(argv) < 2 or not argv[1].isdigit():
        print(__doc__)
        return 2
    pipe_name, seconds = argv[0], int(argv[1])
    logf = open(argv[2], "a", encoding="utf-8") if len(argv) > 2 and argv[2] != "NUL" else None
    card = Card(argv[3] if len(argv) > 3 else None)
    t0 = time.time()

    def log(msg):
        line = "%8.3f  %s" % (time.time() - t0, msg)
        print(line, flush=True)
        if logf:
            logf.write(line + "\n")
            logf.flush()

    h = k.CreateNamedPipeW(pipe_name, 3, 0, 1, 4096, 4096, 0, None)
    if not h or h == INVALID:
        log("cannot create pipe %s (err %d) - another reader running?" % (pipe_name, k.GetLastError()))
        return 2
    log("Aime reader stand-in on %s for %d s; card access code %s on the reader" % (pipe_name, seconds, card.code))
    buf = (ctypes.c_char * 4096)()
    got = w.DWORD()
    while time.time() - t0 < seconds:
        if not k.ConnectNamedPipe(h, None) and k.GetLastError() != ERROR_PIPE_CONNECTED:
            time.sleep(1)
            continue
        log("game opened the reader port")
        pending, last_poll_log = b"", 0
        while time.time() - t0 < seconds:
            avail = w.DWORD()
            if not k.PeekNamedPipe(h, None, 0, None, ctypes.byref(avail), None):
                log("game closed the port")
                break
            if avail.value == 0:
                time.sleep(0.001)
                continue
            k.ReadFile(h, buf, min(avail.value, 4096), ctypes.byref(got), None)
            reqs, pending = parse(pending + bytes(buf.raw[:got.value]))
            for r in reqs:
                status, data = answer(card, r)
                quiet = r["cmd"] in (0x42, 0x81) and time.time() - last_poll_log < 5
                if not quiet:
                    log("<- addr %d seq %3d cmd %02x %-10s [%s]%s  -> %s" % (
                        r["addr"], r["seq"], r["cmd"], NAMES.get(r["cmd"], "?"), r["payload"].hex(" "),
                        "" if r["ok"] else " SUM WRONG", "no answer" if status is None else
                        "st %d [%s]" % (status, data.hex(" "))))
                    if r["cmd"] in (0x42, 0x81):
                        last_poll_log = time.time()
                if status is not None:
                    out = response(r, status, data)
                    n = w.DWORD()
                    k.WriteFile(h, out, len(out), ctypes.byref(n), None)
        k.DisconnectNamedPipe(h)
    log("stopping after %d s" % seconds)
    k.CloseHandle(h)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
