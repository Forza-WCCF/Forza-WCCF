# -*- coding: utf-8 -*-
"""rec_i2_common.py - shared by rec_i2_desk.py, rec_i2_relay.py and rec_i2_inbox.py (step 10, piece I2).  KEPT UNTIL
AFTER PIECE G: play.py still starts the Python server for "server" mode; once G merges, play.py switches to forza-server and
the Python server files, their tests (test_relay.py, test_seat_desk_game.py, test_hold_projector.py, test_send_logs.py)
and these recorders are deleted together; the .jsonl recordings and the Rust replays stay.

The Python server pieces (code/server/*.py, 2017-18 kit/scripts/_seat_broker.py and _relay.py) are platform-independent
(sockets, files, threads; only the desk's firewall call is Windows - the recordings use WCCF_HOLD_FW), so these run on
Linux python3.  Loopback only, ports from 127.0.0.1:0, never 20030/20040/20050 or the game's.

A recording is {"name", "cfg": {...}, "ops": [...]}; each op is recorded as it ran, with its result:
  {"op":"open","c":id}  {"op":"send","c":id,"hex"}  {"op":"recv","c":id,"wait":s,"got":hex,"eof":bool}
  {"op":"shut"|"close","c":id}  {"op":"sleep","s"}  {"op":"ctl","mode":"append"|"write"|"remove","hex"}
  {"op":"fw","until":str|null,"lines":[..]}  {"op":"log","has":[substr..],"found":[bool..]}  {"op":"ls","names":[..]}
UDP (relay): {"op":"usend","c":id,"hex"}  {"op":"urecv","c":id,"wait","got":[hex..]}"""
import json
import os
import socket
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
CODE = os.path.join(os.path.dirname(HERE), "code")
KIT1718 = os.path.join(os.path.dirname(os.path.dirname(HERE)), "2017-18", "kit", "scripts")
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
REC = os.path.join(REPO, "crates", "server", "tests", "rec")


def free_port(kind=socket.SOCK_STREAM):
    s = socket.socket(socket.AF_INET, kind)
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def write(name, rows):
    os.makedirs(REC, exist_ok=True)
    with open(os.path.join(REC, name), "w", encoding="utf-8", newline="\n") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=True) + "\n")
    print("%s: %d records" % (name, len(rows)))


class Server:
    """a Python server script on one port; ops run against it as they are called and are recorded"""

    def __init__(self, name, script, args, env, port, udp=False, cfg=None, tmp=None):
        self.rec = {"name": name, "cfg": cfg or {}, "ops": []}
        self.tmp = tmp or tempfile.mkdtemp(prefix="reci2_")
        self.conns, self.port, self.udp = {}, port, udp
        full = dict(os.environ, WCCF_BIND="127.0.0.1", **env)
        self.proc = subprocess.Popen([sys.executable, "-B", script] + [a.replace("{tmp}", self.tmp) for a in args],
                                     env=full, stdout=subprocess.DEVNULL, cwd=self.tmp)
        if not udp:
            end = time.time() + 10
            while time.time() < end:
                try:
                    socket.create_connection(("127.0.0.1", port), timeout=1).close()
                    break
                except OSError:
                    time.sleep(0.1)
            else:
                raise RuntimeError(script + " did not start")
        elif udp:
            time.sleep(0.5)
        self.ops = self.rec["ops"]

    def path(self, *p):
        return os.path.join(self.tmp, *p)

    def finish(self):
        for c in self.conns.values():
            try:
                c.close()
            except OSError:
                pass
        self.proc.kill()
        self.proc.wait()
        return self.rec

    # --- TCP
    def open(self, c):
        self.conns[c] = socket.create_connection(("127.0.0.1", self.port), timeout=5)
        self.ops.append({"op": "open", "c": c})

    def send(self, c, data):
        if isinstance(data, str):
            data = data.encode("latin-1")
        self.conns[c].sendall(data)
        self.ops.append({"op": "send", "c": c, "hex": data.hex()})

    def recv(self, c, wait=0.3):
        s, got, eof = self.conns[c], b"", False
        s.settimeout(wait)
        while True:
            try:
                d = s.recv(65536)
            except socket.timeout:
                break
            except OSError:
                eof = True
                break
            if not d:
                eof = True
                break
            got += d
        self.ops.append({"op": "recv", "c": c, "wait": wait, "got": got.hex(), "eof": eof})
        return got

    def ask(self, c, data, wait=0.3):
        self.open(c)
        self.send(c, data)
        return self.recv(c, wait)

    def shut(self, c):
        self.conns[c].shutdown(socket.SHUT_WR)
        self.ops.append({"op": "shut", "c": c})

    def close(self, c):
        self.conns.pop(c).close()
        self.ops.append({"op": "close", "c": c})

    def sleep(self, s):
        time.sleep(s)
        self.ops.append({"op": "sleep", "s": s})

    # --- UDP (the relay): fake seats are sockets of their own
    def uopen(self, c):
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.bind(("127.0.0.1", 0))
        self.conns[c] = s
        self.ops.append({"op": "uopen", "c": c})

    def usend(self, c, data):
        self.conns[c].sendto(data, ("127.0.0.1", self.port))
        self.ops.append({"op": "usend", "c": c, "hex": data.hex()})

    def urecv(self, c, wait=0.25):
        s, got = self.conns[c], []
        s.settimeout(wait)
        while True:
            try:
                got.append(s.recvfrom(65536)[0].hex())
            except (socket.timeout, OSError):
                break
        self.ops.append({"op": "urecv", "c": c, "wait": wait, "got": got})
        return got

    # --- files
    def ctl(self, mode, text=b""):
        if isinstance(text, str):
            text = text.encode("utf-8")
        p = self.path("data", "logs", "_dbg_control.txt")
        if mode == "remove":
            os.remove(p)
        else:
            os.makedirs(os.path.dirname(p), exist_ok=True)
            with open(p, "ab" if mode == "append" else "wb") as f:
                f.write(text)
        self.ops.append({"op": "ctl", "mode": mode, "hex": text.hex()})

    def fw(self, until=None, secs=4.0):
        p = self.path("fw.txt")
        end = time.time() + secs

        def lines():
            try:
                with open(p, encoding="ascii") as f:
                    return f.read().split("\n")[:-1]
            except OSError:
                return []
        while until is not None and time.time() < end and lines()[-1:] != [until]:
            time.sleep(0.1)
        self.ops.append({"op": "fw", "until": until, "lines": lines()})

    def log_has(self, logfile, *subs):
        try:
            text = open(logfile, encoding="utf-8").read()
        except OSError:
            text = ""
        self.ops.append({"op": "log", "has": list(subs), "found": [s in text for s in subs]})
