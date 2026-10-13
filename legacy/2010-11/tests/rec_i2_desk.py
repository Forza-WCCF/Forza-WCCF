# -*- coding: utf-8 -*-
"""rec_i2_desk.py - records the seat desks (code/server/seat_desk.py for 2010-11, kit/scripts/_seat_broker.py for 2017-18)
into crates/server/tests/rec/desk.jsonl, which crates/server/tests/desk.rs replays against forza_server::desk byte for
byte.  Scenarios: seat claims and conflicts, the PING/PONG lease, lines that are junk or too long, FULL, reclaim after
a drop and the silent PC (WCCF_BROKER_SILENT shortened), the game log (test_seat_desk_game.py's cases), the projector
box on HOLD (test_hold_projector.py's cases, firewall through WCCF_HOLD_FW), and 2017-18's projector-to-the-first-PC.
Piece I2; see rec_i2_common.py for the format and for when this file goes.
    python3 legacy/2010-11/tests/rec_i2_desk.py"""
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rec_i2_common import CODE, KIT1718, free_port, write  # noqa: E402
import rec_i2_common as C  # noqa: E402

DESK = os.path.join(CODE, "server", "seat_desk.py")
BROKER = os.path.join(KIT1718, "_seat_broker.py")


def desk(name, silent=75.0, hold=None, edition="2010-11", fw=True):
    port = free_port()
    env = {"WCCF_BROKER_PORT": str(port), "WCCF_BROKER_SILENT": str(silent)}
    # the hold file and firewall scratch must exist before the desk starts: make the temp folder first
    tmp = tempfile.mkdtemp(prefix="reci2_")
    os.makedirs(os.path.join(tmp, "data", "logs"))
    if hold is not None:
        with open(os.path.join(tmp, "data", "hold_projector.txt"), "w", encoding="ascii") as f:
            f.write(hold)
    if fw is True:
        env["WCCF_HOLD_FW"] = os.path.join(tmp, "fw.txt")
    elif fw:
        env["WCCF_HOLD_FW"] = os.path.join(tmp, "data")          # a folder: the rule cannot be set
    log = os.path.join(tmp, "data", "logs", "seat_broker.txt")
    script = DESK if edition == "2010-11" else BROKER
    srv = C.Server(name, script, ["60", log], env, port, cfg={"edition": edition, "silent": silent, "hold": hold,
                                                           "fw": "folder" if fw == "folder" else "file"}, tmp=tmp)
    srv.log = log
    return srv


def joined(seat, ip):
    return ("t+  9.00s INFO  ClientManage  ClientManage::INIT() : client[0] : sateID=%d,version=7128,serial=PC\n"
            "t+  9.00s INFO  ClientManage  ClientManage::INIT() : ++ACCEPT++  '%s:4000', 2026/10/9(Fri) 18:20:43\n"
            % (seat, ip))


def left(seat):
    return ("t+ 99.00s INFO  ClientManage  ClientManage::INIT(): ---DISCONNECT--- sateID=%d,sockID=0, "
            "2026/10/9(Fri) 18:42:56\n" % seat)


def scenarios():
    out = []

    s = desk("claims")                                    # seats, conflicts, junk, lines
    s.ask("a", "WANT\n")
    s.ask("b", "WANT\n")
    s.ask("c", "WANT\n")
    s.ask("x", "WANT SEAT 2\n")
    s.ask("x", "WANT SEAT 9\n")
    s.ask("x", "WANT SEAT 0\n")
    s.ask("x", "HELLO\n")
    s.ask("x", "want\n")
    s.ask("x", "WANT SEAT x\n")
    s.ask("x", "WANT SEAT -1\n")
    s.ask("x", "WANT 3\n")
    s.ask("x", "WANT SEAT 2 extra\n")
    s.ask("x", "\n")
    s.ask("d", "  WANT SEAT   5 \r\n")
    s.ask("e", "WANT SEAT 05\n")
    s.ask("x", "HOLD\n")
    s.ask("x", "HOLD 1\n")
    s.send("a", "PING\n")
    s.recv("a")
    s.send("a", "PING\nPING\n")
    s.recv("a")
    s.send("a", "WHATEVER\nPING\n")
    s.recv("a")
    s.send("b", "PI")
    s.sleep(0.2)
    s.send("b", "NG\n")
    s.recv("b")
    s.open("p")
    s.send("p", "WANT\nPING\n")                           # two lines in one send
    s.recv("p")
    s.open("l")
    s.send("l", "W" * 300)                                # no end of line: dropped without an answer
    s.recv("l")
    s.open("n")
    s.send("n", "WANT\n")
    s.recv("n")
    s.close("b")
    s.sleep(0.3)
    s.ask("f", "WANT\n")
    s.close("a")
    s.close("p")
    s.sleep(0.3)
    s.ask("g", "WANT SEAT 1\n")
    s.ask("h", "WANT\n")
    out.append(s.finish())

    s = desk("full")                                      # the 8 seats, then FULL; one freed
    for i in range(8):
        s.ask("s%d" % i, "WANT\n")
    s.ask("x", "WANT\n")
    s.ask("x", "WANT SEAT 3\n")
    s.close("s4")
    s.sleep(0.3)
    s.ask("y", "WANT\n")
    s.ask("z", "WANT\n")
    out.append(s.finish())

    s = desk("reclaim", silent=1.5)                       # a drop frees the seat for the same PC; a silent PC is gone
    s.ask("a", "WANT SEAT 4\n")
    s.ask("x", "WANT SEAT 4\n")
    s.close("a")
    s.sleep(0.3)
    s.ask("a2", "WANT SEAT 4\n")
    s.ask("q", "WANT SEAT 6\n")                           # never pings
    for _ in range(4):
        s.sleep(0.4)
        s.send("a2", "PING\n")
        s.recv("a2", 0.1)
    s.ask("x", "WANT SEAT 6\n")                           # 6 has been silent for 2 s: gone
    s.recv("q", 0.1)
    s.ask("y", "WANT SEAT 4\n")                           # pinging kept 4
    s.log_has(s.log, "seat 4 free again", "seat 6 free again", "seat 4 refused - taken")
    out.append(s.finish())

    s = desk("game")                                      # test_seat_desk_game.py
    s.ask("a", "WANT\n")
    s.close("a")
    s.sleep(0.3)
    s.ctl("write", "t+  0.10s DEBUG  start\n" + joined(0, "10.0.0.5") + joined(1, "10.0.0.5"))
    s.ask("b", "WANT\n")
    s.ask("c", "WANT SEAT 1\n")
    s.ctl("append", joined(3, "127.0.0.1"))
    s.ask("d", "WANT SEAT 3\n")
    s.ctl("append", left(1) + left(0) + "t+ 99.50s INFO  ClientManage  ClientManage::INIT() : client[0] : sateID=5")
    s.ask("e", "WANT\n")
    s.ctl("append", ",version=7128,serial=PC\nt+ 99.60s INFO  ClientManage  ClientManage::INIT() : ++ACCEPT++  "
                    "'10.0.0.5:4001', 2026/10/9(Fri) 18:43:00\n")
    s.ask("f", "WANT\n")
    s.ask("g", "WANT\n")
    for c in "bcdefg":
        if c in s.conns:
            s.close(c)
    s.sleep(0.5)
    s.ctl("write", "t+  0.10s DEBUG  start\n" + joined(2, "10.0.0.5"))
    s.ask("h", "WANT\n")
    s.ask("i", "WANT\n")
    s.ctl("remove")
    s.close("h")
    s.close("i")
    s.sleep(0.5)
    s.ask("j", "WANT\n")
    s.log_has(s.log, "skipped - in the game (10.0.0.5)", "(in the game by 10.0.0.5)")
    out.append(s.finish())

    s = desk("hold", hold="10.0.0.9\n")                   # test_hold_projector.py
    s.fw()
    s.ask("h", "HOLD\n")
    s.ask("a", "WANT SEAT 1\n")
    s.ctl("write", "t+  0.10s DEBUG  start\n" + joined(2, "10.0.0.5"))
    s.fw("open")
    s.ask("h", "HOLD\n")
    s.ctl("append", joined(0, "10.0.0.9") + left(2))
    s.sleep(1.5)
    s.fw()
    s.ctl("append", left(0))
    s.fw("block 10.0.0.9")
    s.ctl("write", "t+  0.10s DEBUG  start\n" + joined(0, "10.0.0.9"))
    s.sleep(1.5)
    s.fw()
    s.log_has(s.log, "held - it joins the game once a player is in", "let in - a player is in the game")
    out.append(s.finish())

    s = desk("hold_none")                                 # no hold file: nothing held, no rule touched
    s.ask("h", "HOLD\n")
    s.sleep(1.2)
    s.fw()
    out.append(s.finish())

    s = desk("hold_bad", hold="not an address 999")
    s.ask("h", "HOLD\n")
    s.sleep(1.2)
    s.fw()
    out.append(s.finish())

    s = desk("hold_cannot", hold="10.0.0.9", fw="folder")  # the rule cannot be set: HOLD 0, seats still given
    s.sleep(1.0)
    s.log_has(s.log, "could not be set")
    s.ask("h", "HOLD\n")
    s.ask("a", "WANT\n")
    out.append(s.finish())

    s = desk("broker1718", edition="2017-18")             # the first PC shows the projector; no HOLD; no game log
    s.ask("a", "WANT\n")
    s.ask("b", "WANT\n")
    s.ask("c", "WANT SEAT 7\n")
    s.ask("x", "WANT SEAT 2\n")
    s.ask("x", "HOLD\n")
    s.ask("x", "WANT SEAT 9\n")
    s.send("a", "PING\n")
    s.recv("a")
    s.close("a")
    s.sleep(0.3)
    s.ask("d", "WANT\n")                                  # seat 1 again, and the projector with it
    s.close("d")
    s.close("b")
    s.sleep(0.3)
    s.ask("e", "WANT SEAT 5\n")                           # 7 and 5 are held; nobody has the projector but...
    s.ask("f", "WANT\n")
    for i in range(8):
        s.ask("z%d" % i, "WANT\n")
    out.append(s.finish())
    return out


if __name__ == "__main__":
    write("desk.jsonl", scenarios())
