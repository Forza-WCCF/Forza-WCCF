# -*- coding: utf-8 -*-
"""rec_i1_online.py - records code/online/seat.py and send_logs.py against the real server side (server/seat_desk.py and
log_inbox.py) into crates/forza/tests/rec/seat.jsonl and send_logs.jsonl, which crates/forza/tests/online.rs replays
against forza::online.  Step 10 piece I1.  KEPT UNTIL PIECE G: play.py and panel.py still use the Python client;
piece G deletes seat.py, send_logs.py, test_seat_lease.py, test_send_logs.py and this recorder together.
Loopback only: the desk and the inbox are the real scripts on ephemeral ports (WCCF_BROKER_PORT, WCCF_INBOX_PORT
from a free port, never 20030/20050), the client goes through a recording proxy.  Packs use synthetic kits in a temp
folder with made-up user names, set through USERPROFILE/USERNAME/COMPUTERNAME.  kit_common is stubbed (it needs
Windows; send_logs only uses KIT), so this runs on Linux; there the Python picks latin-1 where Windows picks mbcs.
Lines of seat.jsonl:  {"name","want","ping","retry","life","result":{..seat file..},"conns":[[["c"|"s"|"x",hex]..]..]}
Lines of send_logs.jsonl:
  {"t":"pack","name","files":{path:spec},"game","tail","note","env","entries":[[name,size,sha256]..],"n"}
  {"t":"version","text":hex|null,"result"}
  {"t":"send","name","ver","blobs":[hex..],"results":[[ok,what]..],"conns":[events..]}   events: ["c"|"s",hex]
spec: {"text":..} | {"hex":..} | {"stream":seed,"size":n} (sha256 of "seed:i" counter blocks, same in online.rs)
    python legacy/2010-11/tests/rec_i1_online.py"""
import hashlib
import io
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import types
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
CODE = os.path.join(os.path.dirname(HERE), "code")
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
REC = os.path.join(REPO, "crates", "forza", "tests", "rec")
sys.path.insert(0, CODE)
stub = types.ModuleType("kit_common")
stub.KIT = tempfile.gettempdir()
sys.modules["kit_common"] = stub
from online import seat as SEAT  # noqa: E402
from online import send_logs as SL  # noqa: E402


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


class Proxy:
    """loopback proxy that records each connection as a list of [dir, bytes]; drop_first: connection 0 is cut right
    after the client's first PING (the desk side closes first, then the client side)"""

    def __init__(self, target, drop_first=False):
        self.target, self.drop_first, self.conns, self.lock = target, drop_first, [], threading.Lock()
        self.srv = socket.socket()
        self.srv.bind(("127.0.0.1", 0))
        self.srv.listen(8)
        self.port = self.srv.getsockname()[1]
        threading.Thread(target=self.accept, daemon=True).start()

    def accept(self):
        while True:
            try:
                c, _ = self.srv.accept()
            except OSError:
                return
            events = []
            with self.lock:
                idx = len(self.conns)
                self.conns.append(events)
            try:
                up = socket.create_connection(("127.0.0.1", self.target), timeout=5)
            except OSError:
                c.close()
                continue
            threading.Thread(target=self.pump, args=(c, up, "c", events, idx), daemon=True).start()
            threading.Thread(target=self.pump, args=(up, c, "s", events, idx), daemon=True).start()

    def pump(self, src, dst, tag, events, idx):
        try:
            while True:
                data = src.recv(65536)
                if not data:
                    try:
                        dst.shutdown(socket.SHUT_WR)
                    except OSError:
                        pass
                    return
                with self.lock:
                    events.append([tag, data])
                if tag == "c" and self.drop_first and idx == 0 and data.startswith(b"PING"):
                    try:
                        dst.shutdown(socket.SHUT_RDWR)     # the other pump thread is in recv: close() alone sends no FIN
                    except OSError:
                        pass
                    dst.close()
                    time.sleep(0.15)
                    src.close()
                    with self.lock:
                        events.append(["x", b""])
                    return
                dst.sendall(data)
        except OSError:
            return

    def close(self):
        self.srv.close()

    def events(self, limit=4096):
        """merged: adjacent same-direction chunks are one event; bytes as hex"""
        out = []
        for ev in self.conns:
            merged = []
            for tag, data in ev:
                if merged and merged[-1][0] == tag and tag != "x":
                    merged[-1][1] += data
                else:
                    merged.append([tag, bytearray(data)])
            out.append([[t, bytes(d).hex()] for t, d in merged])
        return out


def start(script, args, env, ready_port):
    p = subprocess.Popen([sys.executable, "-B", os.path.join(CODE, "server", script)] + args,
                         env=dict(os.environ, WCCF_BIND="127.0.0.1", **env), stdout=subprocess.DEVNULL, cwd=env["CWD"])
    end = time.time() + 10
    while time.time() < end:
        try:
            socket.create_connection(("127.0.0.1", ready_port), timeout=1).close()
            return p
        except OSError:
            time.sleep(0.1)
    raise RuntimeError(script + " did not start")


def raw_claim(port, line):
    s = socket.create_connection(("127.0.0.1", port), timeout=5)
    s.sendall(line)
    s.recv(100)
    return s


def stream(seed, size):
    out, i = bytearray(), 0
    while len(out) < size:
        out += hashlib.sha256(("%s:%d" % (seed, i)).encode()).digest()
        i += 1
    return bytes(out[:size])


def seat_scenarios():
    out = []
    # name, want, life, pre (occupying connections), proxy options, silent
    plan = [("any", None, 1.1, "none"), ("seat5", 5, 1.1, "none"), ("seat8", 8, 0.5, "none"),
            ("taken", 2, 0.5, "taken"), ("full", None, 0.5, "full"), ("refused_seat9", 9, 0.5, "none"),
            ("reconnect", 3, 2.0, "drop"), ("no_broker", None, 0.5, "closed"), ("no_answer", None, 0.5, "silent")]
    for name, want, life, mode in plan:
        tmp = tempfile.mkdtemp(prefix="reci1_")
        port, pre, desk, silent = free_port(), [], None, None
        try:
            if mode == "closed":
                proxy_port, proxy = free_port(), None
            elif mode == "silent":
                silent = socket.socket()
                silent.bind(("127.0.0.1", 0))
                silent.listen(2)
                proxy = Proxy(silent.getsockname()[1])
                held = []
                threading.Thread(target=lambda: held.append(silent.accept()), daemon=True).start()
                proxy_port = proxy.port
            else:
                desk = start("seat_desk.py", ["60"], {"WCCF_BROKER_PORT": str(port), "CWD": tmp}, port)
                if mode == "taken":
                    pre.append(raw_claim(port, b"WANT SEAT 2\n"))
                if mode == "full":
                    pre += [raw_claim(port, b"WANT\n") for _ in range(8)]
                proxy = Proxy(port, drop_first=(mode == "drop"))
                proxy_port = proxy.port
            SEAT.PORT, SEAT.PING, SEAT.RETRY = proxy_port, 0.2, 0.2
            o = os.path.join(tmp, "seat.json")
            argv = ["127.0.0.1", o, str(life)] + ([] if want is None else [str(want)])
            SEAT.main(argv)
            result = json.load(open(o))
            if proxy:
                time.sleep(0.3)
            out.append({"name": name, "want": want, "ping": 0.2, "retry": 0.2, "life": life, "result": result,
                        "conns": proxy.events() if proxy else []})
        finally:
            for s in pre:
                s.close()
            if proxy:
                proxy.close()
            if silent:
                silent.close()
            if desk:
                desk.kill()
                desk.wait()
            shutil.rmtree(tmp, ignore_errors=True)
    return out


def spec_of(b):
    try:
        return {"text": b.decode("utf-8")}
    except UnicodeDecodeError:
        return {"hex": b.hex()}


def pack_scenario(name, files, game, tail, note, env):
    """files: {path under the temp root: bytes or ("stream", seed, size)}"""
    tmp = tempfile.mkdtemp(prefix="reci1_")
    saved = {k: os.environ.get(k) for k in ("USERPROFILE", "USERNAME", "COMPUTERNAME")}
    try:
        specs = {}
        for rel, body in files.items():
            p = os.path.join(tmp, rel)
            os.makedirs(os.path.dirname(p), exist_ok=True)
            if isinstance(body, tuple):
                specs[rel], body = {"stream": body[1], "size": body[2]}, stream(body[1], body[2])
            else:
                specs[rel] = spec_of(body)
            with open(p, "wb") as f:
                f.write(body)
        for k, v in saved.items():
            os.environ.pop(k, None)
        os.environ.update(env)
        gpath = os.path.join(tmp, game) if game else None
        blob, n = SL.pack(os.path.join(tmp, "kit"), gpath, tail, note) if tail else SL.pack(
            os.path.join(tmp, "kit"), gpath, note=note)
        z = zipfile.ZipFile(io.BytesIO(blob))
        entries = [[i.filename, i.file_size, hashlib.sha256(z.read(i)).hexdigest()] for i in z.infolist()]
        return {"t": "pack", "name": name, "files": specs, "game": game, "tail": tail, "note": note, "env": env,
                "entries": entries, "n": n}
    finally:
        for k, v in saved.items():
            os.environ.pop(k, None)
            if v is not None:
                os.environ[k] = v
        shutil.rmtree(tmp, ignore_errors=True)


def pack_scenarios():
    K = "kit/"
    e = {"USERPROFILE": "C:\\Users\\Zelda", "USERNAME": "Zelda", "COMPUTERNAME": "ZELDA-PC"}
    body = ("start\n" + "x" * 5000 + "\nopened C:\\Users\\Zelda\\Desktop\\a.txt and c:/users/ZELDA/b.txt\n"
            "user Zelda on ZELDA-PC; zelda-pc again; Zeldaesque and PCZELDA-PCX stay\nthe end\n").encode()
    basic = {K + "data/logs/run_seat1.txt": body, K + "data/logs/run_net.txt": b"short C:\\Users\\Zelda\n",
             K + "data/logs/previous/run_seat1.txt": b"before\n", K + "data/logs/hook/mxhook_1.log": b"hook ZELDA\n",
             K + "data/logs/archive/old.zip": b"an old run", K + "data/logs/wccfpanel_projector.log": b"proj\n",
             K + "data/save/seat1_club.bin": b"THE CLUB CARD", K + "data/panel.txt": b"server=1.2.3.4\n",
             K + "data/keys.txt": b"k=1\n", K + "data/running.json": b"{}", K + "version.txt": b"kit-5.5\n",
             "game/seat1/wccfpanel.log": b"panel Zelda\n", "game/client.exe": b"MZ"}
    out = [pack_scenario("basic", basic, "game/client.exe", 1000, None, e),
           pack_scenario("basic_note", basic, "game/client.exe", 1000, "why: a test, by Zelda", e),
           pack_scenario("no_game", basic, None, 1000, None, e),
           pack_scenario("empty", {K + "other.txt": b"x"}, None, 1000, None, e),
           pack_scenario("short_names", basic, None, 1000, None,
                         {"USERPROFILE": "C:\\Users\\Al", "USERNAME": "Al", "COMPUTERNAME": "PC"}),
           pack_scenario("no_names", basic, None, 1000, None, {})]
    uni = ("C:\\Users\\Zo\u00eb".encode("utf-8") + b" and " + "C:\\Users\\Zo\u00eb".encode("latin-1") + b" and Zo\xc3\xab "
           + "Zo\u00eb".encode("latin-1") + b"\n")
    out.append(pack_scenario("unicode", {K + "data/logs/run_a.txt": uni}, None, 1000, "note Zo\u00eb",
                             {"USERPROFILE": "C:\\Users\\Zo\u00eb", "USERNAME": "Zo\u00eb", "COMPUTERNAME": "ZELDA-PC"}))
    fake = b"MDMP" + b"\0" * 60 + "Zelda".encode("utf-16-le") + b"\0" * 9 + b"zELDA" + b"\0" * 7 + b"C:\\Users\\Zelda" + b"\0"
    dumps = {K + "data/dumps/2026101%d-120000_client_Release_1_lean.dmp" % i: fake for i in range(4)}
    dumps[K + "data/dumps/20261013-120000_client_Release_1_full.dmp"] = fake
    dumps[K + "data/dumps/notes.txt"] = b"x"
    dumps[K + "version.txt"] = b"kit-5.5"
    out.append(pack_scenario("dumps", dumps, None, 1000, "why", e))
    out.append(pack_scenario("big_tail", {K + "data/logs/run_big.txt": ("stream", "big", 40000),
                                          K + "data/logs/run_small.txt": b"s\n"}, None, 4096, None, e))
    out.append(pack_scenario("default_tail_small", {K + "data/logs/run_a.txt": ("stream", "d", 3 * 1024 * 1024)},
                             None, 0, None, e))
    out.append(pack_scenario("shrink", {K + "data/logs/run_%d.txt" % i: ("stream", "s%d" % i, 3 * 1024 * 1024)
                                        for i in range(5)}, None, 0, None, e))
    return out


def version_scenarios():
    out = []
    for text in (b"kit-5.5\n", b"KIT-6 extra", b"5.5", b"", b"kit-", b"  \n", b"kit-5.5-beta2\r\n", None):
        tmp = tempfile.mkdtemp(prefix="reci1_")
        try:
            if text is not None:
                with open(os.path.join(tmp, "version.txt"), "wb") as f:
                    f.write(text)
            out.append({"t": "version", "text": None if text is None else text.hex(), "result": SL.version(tmp)})
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
    return out


def send_scenarios():
    out = []
    z = io.BytesIO()
    with zipfile.ZipFile(z, "w") as f:
        f.writestr(zipfile.ZipInfo("a.txt", (2026, 10, 13, 0, 0, 0)), b"x")
    good = z.getvalue()
    plan = [("ok", [good], "5.5", {}), ("not_zip", [b"hello"], "5.5", {}),
            ("too_many", [good, good], "5.5", {"WCCF_INBOX_PER_HOUR": "1"}), ("version_other", [good], "6.0-beta2", {}),
            ("no_inbox", [good], "5.5", None)]
    for name, blobs, ver, env in plan:
        tmp = tempfile.mkdtemp(prefix="reci1_")
        port, proc, proxy = free_port(), None, None
        try:
            if env is not None:
                proc = start("log_inbox.py", ["60", os.path.join(tmp, "inbox.txt"), os.path.join(tmp, "keep")],
                             dict(env, WCCF_INBOX_PORT=str(port), CWD=tmp), port)
                proxy = Proxy(port)
                port = proxy.port
            results = [list(SL.send("127.0.0.1", b, ver, port=port, timeout=10)) for b in blobs]
            time.sleep(0.3)
            out.append({"t": "send", "name": name, "ver": ver, "blobs": [b.hex() for b in blobs], "results": results,
                        "conns": proxy.events() if proxy else []})
        finally:
            if proxy:
                proxy.close()
            if proc:
                proc.kill()
                proc.wait()
            shutil.rmtree(tmp, ignore_errors=True)
    return out


def write(name, rows):
    with open(os.path.join(REC, name), "w", encoding="utf-8", newline="\n") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=True) + "\n")
    print(name, len(rows), "records")


if __name__ == "__main__":
    os.makedirs(REC, exist_ok=True)
    write("seat.jsonl", seat_scenarios())
    write("send_logs.jsonl", pack_scenarios() + version_scenarios() + send_scenarios())
