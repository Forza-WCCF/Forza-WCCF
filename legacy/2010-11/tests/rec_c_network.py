# -*- coding: utf-8 -*-
"""rec_c_network.py - records code/hardware/network.py (the master / installer / keychip / amNetwork / binary TCP
stand-ins) into crates/forza/tests/rec/network.jsonl, which crates/forza/tests/standins.rs replays against
forza::standins::network.  Step 10 piece C.  KEPT UNTIL PIECE G: play.py still starts the Python stand-in; piece G
deletes network.py and this recorder together.
Runs the Python services on ephemeral loopback ports (never 40100-40107; the module's port tables are replaced), and a
scripted client sends every request kind plus malformed and split ones.  Each line is
  {"svc": "master"|"installer"|"keychip"|"amnet"|"binary", "conn": n, "req": hex, "split": k, "resp": hex}
the first record of a connection has an empty req and holds the greeting; a request goes out in two writes at
byte `split` when it is non-zero.
    python legacy/2010-11/tests/rec_c_network.py"""
import json
import os
import socket
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "code"))
from hardware import network as N  # noqa: E402

OUT = os.path.join(REPO, "crates", "forza", "tests", "rec", "network.jsonl")


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def L(*lines):
    return [x if isinstance(x, bytes) else x.encode("latin-1") for x in lines]


KEYCHIP = [
    "keychip.version=?", "keychip.status=?&keychip.version=?", "keychip.appboot.gameid=?\r\n",
    "keychip.billing.playcount=x", "keychip.nope=?", "keychip.nope=7", "keychip.version=zzz",
    "keychip.billing.keyid=?&keychip.billing.mainid=?&keychip.billing.playlimit=?&keychip.billing.nearfull=?",
    "keychip.appboot.networkaddr=?&keychip.appboot.systemflag=?&keychip.appboot.region=?",
    "keychip.appboot.platformid=?&keychip.appboot.modeltype=?&keychip.appboot.formattype=?&keychip.appboot.dvdflag=?",
    "keychip.version=?&result=5", "keychip.x=result=3", "result=1&keychip.version=?",
    "mxmaster.foreground.getcount=?", "mxmaster.foreground.getcount=2", "mxmaster.foreground.next=?",
    "nothing", "a=b=c&d", "", "&", "&&x=1&", " k = v ", "keychip.version =  ? ", "x\x85y=\xa0?\x85", "\x1ckeychip.status=?\x1f",
    "k\xe9y=v\xe4l", "=", "?=?", "keychip.version=?\r\r", "a=1&a=2",
]
AMNET = [
    "request=query_dhcp_status&if=0", "request=query_dhcp_status", "request=update_dhcp_status&if=1&status=1",
    "request=query_nic_status&if=0", "request=query_ip_address&if=0", "request=query_subnetmask&if=2",
    "request=query_gateway&if=0", "request=query_primary_dns&if=0", "request=query_secondary_dns&if=0",
    "request=query_mac_address&if=0", "request=bogus&if=3", "request=", "if=0", "", "x", "request=query_ip_address&if=",
    "request=query_ip_address&request=query_gateway", " request = query_gateway & if = 4 ", "request=q\xe9&if=\xff",
    "request=query_dhcp_status&if=0&extra=1&more",
]
BIN = [b"\x00", bytes(range(256)) * 3, b"\xde\xad\xbe\xef" * 40, b"\n\r\n"]

PLAN = []  # (svc, conn, [(bytes, split)])
for svc in ("master", "installer", "keychip"):
    # one request per connection, like the launcher, then the whole list on one connection
    PLAN.append((svc, 0, [(x, 0) for x in L(*[q + "\r\n" for q in KEYCHIP])]))
    for i, q in enumerate(KEYCHIP[:6]):
        PLAN.append((svc, 1 + i, [(x, 0) for x in L(q + "\r\n")]))
    PLAN.append((svc, 10, [(x, 0) for x in L("keychip.version=?\n", "keychip.status=?\n")]))   # bare LF
    PLAN.append((svc, 11, [(b"keychip.version=?\r\nkeychip.status=?\r\n", 0),                 # pipelined
                           (b"a=1\r\n\r\n\r\nb=2\r\n", 0), (b"\r\n\n\r\n", 0),
                           (b"keychip.billing.keyid=?\r\n", 12), (b"keychip.version=?\r\n", 1),
                           (b"no newline yet", 0), (b" and now\r\n", 0),
                           (b"x=1\r\ny=2\r\nz", 0), (b"=3\r\n", 0)]))
    PLAN.append((svc, 12, [(b"\xff\xfe=?\r\n", 0), (b"k=" + b"v" * 5000 + b"\r\n", 2000)]))
PLAN.append(("amnet", 0, [(x, 0) for x in L(*[q + "\r\n" for q in AMNET])]))
for i, q in enumerate(AMNET[:5]):
    PLAN.append(("amnet", 1 + i, [(x, 0) for x in L(q + "\r\n")]))
PLAN.append(("amnet", 10, [(b"request=query_gateway&if=0\n", 0), (b"request=query_ip_address&if=1\r\nrequest=query_nic_status&if=1\r\n", 0),
                           (b"request=query_gateway&if=0\r\n", 9), (b"request=que", 0), (b"ry_mac_address&if=0\r\n", 0)]))
PLAN.append(("binary", 0, [(b, 0) for b in BIN]))


def talk(port, steps):
    c = socket.create_connection(("127.0.0.1", port))
    c.settimeout(0.35)
    out = []

    def rd():
        got = b""
        while True:
            try:
                d = c.recv(65536)
            except socket.timeout:
                return got
            if not d:
                return got
            got += d

    out.append((b"", 0, rd()))
    for req, split in steps:
        if split:
            c.sendall(req[:split])
            time.sleep(0.15)
            c.sendall(req[split:])
        else:
            c.sendall(req)
        time.sleep(0.05)
        out.append((req, split, rd()))
    c.close()
    return out


def main():
    ports = {n: free_port() for n in ("master", "installer", "keychip", "amnet", "binary")}
    N.TEXT = {ports["master"]: "master", ports["installer"]: "installer", ports["keychip"]: "keychip"}
    N.AMNET = {ports["amnet"]: "amnetwork"}
    N.BINARY = {ports["binary"]: "keychip-bin"}
    N.LOG = open(os.devnull, "w")
    N.T0 = time.time()
    N.DURATION = 3600
    import threading
    ts = [threading.Thread(target=N.serve, args=(p, n, h), daemon=True)
          for table, h in ((N.TEXT, N.text_client), (N.AMNET, N.amnet_client), (N.BINARY, N.binary_client))
          for p, n in table.items()]
    for t in ts:
        t.start()
    time.sleep(0.5)
    n = 0
    with open(OUT, "w", newline="\n") as f:
        for svc, conn, steps in PLAN:
            for req, split, resp in talk(ports[svc], steps):
                f.write(json.dumps({"svc": svc, "conn": conn, "req": req.hex(), "split": split, "resp": resp.hex()}) + "\n")
                n += 1
    N.DURATION = 0
    print("wrote %d records to %s" % (n, OUT))


if __name__ == "__main__":
    main()
