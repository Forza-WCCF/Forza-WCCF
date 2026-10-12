# -*- coding: utf-8 -*-
"""rec_i2_inbox.py - records the log inbox (code/server/log_inbox.py) into crates/server/tests/rec/inbox.jsonl, which
crates/server/tests/inbox.rs replays against forza_server::inbox.  Scenarios: an upload kept, bad requests, the size
limits, "not a ZIP", the version sanitised to [0-9A-Za-z.-] and 24 characters (test_send_logs.py's cases), a PC that
hangs up half way, the quota per address (WCCF_INBOX_PER_HOUR shortened), eviction of the oldest (WCCF_INBOX_KEEP
shortened), 4 uploads at a time.  The code in "OK CODE" is random: replays and "ls" compare it as <CODE>.
Piece I2; see rec_i2_common.py for the format and for when this file goes.
    python3 legacy/2010-11/tests/rec_i2_inbox.py"""
import io
import os
import re
import sys
import tempfile
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rec_i2_common as C  # noqa: E402
from rec_i2_common import CODE, free_port, write  # noqa: E402

INBOX = os.path.join(CODE, "server", "log_inbox.py")


def a_zip(text=b"x"):
    b = io.BytesIO()
    with zipfile.ZipFile(b, "w") as z:
        z.writestr(zipfile.ZipInfo("a.txt", (2026, 10, 13, 0, 0, 0)), text)
    return b.getvalue()


class Inbox(C.Server):
    def __init__(self, name, per_hour=6, keep=1 << 30):
        port = free_port()
        env = {"WCCF_INBOX_PORT": str(port), "WCCF_INBOX_PER_HOUR": str(per_hour), "WCCF_INBOX_KEEP": str(keep)}
        tmp = tempfile.mkdtemp(prefix="reci2_")
        os.makedirs(os.path.join(tmp, "data", "logs"))        # the inbox does not make its log's folder
        super().__init__(name, INBOX, ["60", "{tmp}/data/logs/log_inbox.txt", "{tmp}/keep"], env, port,
                         cfg={"per_hour": per_hour, "keep": keep}, tmp=tmp)

    def upload(self, c, ver, data, tail=b""):
        self.open(c)
        self.send(c, "WCCFLOGS1 %d %s\n" % (len(data), ver))
        self.send(c, data + tail)
        return self.recv(c)

    def ls(self):
        d = self.path("keep")
        names = os.listdir(d) if os.path.isdir(d) else []
        found = sorted((re.sub(r"^\d{4}-\d\d-\d\d_\d{6}_[A-Z0-9]{5}_", "TS_<CODE>_", n), os.path.getsize(os.path.join(d, n)))
                       for n in names)
        self.ops.append({"op": "ls", "names": [n for n, _ in found], "sizes": [n for _, n in found]})


def scenarios():
    out = []
    z = a_zip()

    s = Inbox("upload")
    s.upload("a", "5.5", z)
    s.ls()
    s.upload("b", "5.5", z, b"trailing bytes are ignored")
    s.ls()
    s.upload("c", "5.5", b"PK\x03\x04")                   # just the magic
    s.ls()
    s.open("d")                                           # the upload in pieces
    s.send("d", "WCCFLOGS1 ")
    s.sleep(0.2)
    s.send("d", "%d 5.5\n" % len(z))
    s.sleep(0.2)
    s.send("d", z[:10])
    s.sleep(0.2)
    s.send("d", z[10:])
    s.recv("d")
    s.open("e")                                           # head and body in one send, then the PC shuts its end
    s.send("e", ("WCCFLOGS1 %d 5.5\n" % len(z)).encode() + z)
    s.shut("e")
    s.recv("e")
    s.ls()
    out.append(s.finish())

    s = Inbox("refusals")
    s.ask("a", "HELLO\n")
    s.ask("a", "WCCFLOGS1 x 5.5\n")
    s.ask("a", "WCCFLOGS1 5\n")
    s.ask("a", "WCCFLOGS1 5 5.5 extra\n")
    s.ask("a", "WCCFLOGS1 -5 5.5\n")
    s.ask("a", "wcclogs1 5 5.5\n")
    s.ask("a", "WCCFLOGS1 5 5.5\r\nhello")                # \r is whitespace; the body starts after \n
    s.ask("a", "\n")
    s.ask("a", "WCCFLOGS1 0 5.5\n")
    s.ask("a", "WCCFLOGS1 9000000 5.5\n")
    s.ask("a", "WCCFLOGS1 8388609 5.5\n")
    s.ask("a", "WCCFLOGS1 99999999999999999999999 5.5\n")
    s.ask("a", "WCCFLOGS1 000 5.5\n")
    s.ask("a", "WCCFLOGS1 5 5.5\nhello")                  # not a ZIP
    s.ask("a", "WCCFLOGS1 5 5.5\nPK\x03\x05x")
    s.ask("a", "x" * 130)                                 # no end of line in 130 bytes
    s.ask("a", "x" * 300 + "\n")
    s.open("n")
    s.send("n", "WCCFLOGS1 5 5.5")                        # no end of line yet: waits
    s.shut("n")
    s.recv("n")
    s.ls()
    out.append(s.finish())

    s = Inbox("versions", per_hour=20)                                 # sanitised to [0-9A-Za-z.-], 24 characters
    for i, v in enumerate(("5.5", "6.0-beta2", "a/b\\c:d*e?f", "v" * 40, "x.y-z_w", "..", "café", "1.2.3.4.5.6.7.8.9.10.11.12.13")):
        s.upload("v%d" % i, v, z)
    s.open("raw")                                         # raw bytes over 127: one "_" each
    s.send("raw", b"WCCFLOGS1 %d v\xc3\xa9x\xff\n" % len(z) + z)
    s.recv("raw")
    s.ls()
    out.append(s.finish())

    s = Inbox("hangup")                                   # a PC that hangs up half way keeps nothing
    s.open("a")
    s.send("a", b"WCCFLOGS1 1000 5.5\n" + z[:20])
    s.close("a")
    s.sleep(0.4)
    s.ls()
    s.log_has(s.path("data", "logs", "log_inbox.txt"), "upload failed - the PC hung up after 20 of 1000 bytes")
    s.upload("b", "5.5", z)                               # and the inbox goes on
    s.ls()
    s.open("c")                                           # hangs up before the end of its head
    s.send("c", "WCCFLOGS1 ")
    s.close("c")
    s.sleep(0.3)
    s.upload("d", "5.5", z)
    s.ls()
    out.append(s.finish())

    s = Inbox("quota", per_hour=3)                        # junk and too big are not counted; "not a ZIP" is
    s.upload("a", "5.5", z)
    s.ask("x", "HELLO\n")
    s.ask("x", "WCCFLOGS1 9000000 5.5\n")
    s.upload("b", "5.5", b"hello")
    s.upload("c", "5.5", z)
    s.upload("d", "5.5", z)
    s.ask("x", "WCCFLOGS1 5 5.5\nhello")
    s.ask("x", "HELLO\n")
    s.ls()
    s.log_has(s.path("data", "logs", "log_inbox.txt"), "127.0.0.1: refused - too many from this address")
    out.append(s.finish())

    z1, z2, z3, z4 = (a_zip(b"x" * n) for n in (10, 20, 30, 40))
    keep = len(z1) + len(z2) + 5                          # room for the newest two of these, not three
    s = Inbox("eviction", keep=keep)
    for i, zz in enumerate((z1, z2, z3, z4)):
        s.upload("u%d" % i, "v%d" % (i + 1), zz)
        s.ls()
        s.sleep(0.3)
    s.log_has(s.path("data", "logs", "log_inbox.txt"), "inbox full: removed the oldest")
    out.append(s.finish())

    s = Inbox("busy")                                     # 4 uploads at a time
    for i in range(4):
        s.open("i%d" % i)
        s.send("i%d" % i, "WCCFLOGS1 100 5.5\n")          # started, body never comes
    s.sleep(0.3)
    s.open("x")
    s.recv("x")
    s.ask("y", "HELLO\n")
    s.close("i0")
    s.sleep(0.3)
    s.upload("a", "5.5", z)                               # a slot is free again
    s.ls()
    for i in range(1, 4):
        s.close("i%d" % i)
    s.sleep(0.3)
    s.upload("b", "5.5", z)
    s.ls()
    out.append(s.finish())
    return out


if __name__ == "__main__":
    write("inbox.jsonl", scenarios())
