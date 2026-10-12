# -*- coding: utf-8 -*-
"""rec_i2_relay.py - records the match relays (code/server/relay.py for 2010-11, kit/scripts/_relay.py for 2017-18)
into crates/server/tests/rec/relay.jsonl, which crates/server/tests/relay.rs replays against forza_server::relay.
Fake seats are UDP sockets on 127.0.0.1:0, so the F packets carry 127.0.0.1.  Scenarios: R and D packets, pings (the
64/65-byte limit), junk, seats claimed by a D packet, a stranger's claim of a live seat (A6, 2010-11 only), a seat
forgotten after WCCF_RELAY_FORGET (shortened), and 2017-18's relay, where any address may take a seat.
Piece I2; see rec_i2_common.py for the format and for when this file goes.
    python3 legacy/2010-11/tests/rec_i2_relay.py"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rec_i2_common as C  # noqa: E402
from rec_i2_common import CODE, KIT1718, free_port, write  # noqa: E402
import socket  # noqa: E402

RELAY = os.path.join(CODE, "server", "relay.py")
RELAY1718 = os.path.join(KIT1718, "_relay.py")
FORGET = 0.8


def relay(name, edition="2010-11", forget=FORGET):
    port = free_port(socket.SOCK_DGRAM)
    s = C.Server(name, RELAY if edition == "2010-11" else RELAY1718, ["60"],
                 {"WCCF_RELAY_PORT": str(port), "WCCF_RELAY_FORGET": str(forget)}, port, udp=True,
                 cfg={"edition": edition, "forget": forget})
    return s


def R(seat):
    return b"WRL1R" + bytes([seat])


def D(to, frm, payload=b""):
    return b"WRL1D" + bytes([to, frm]) + payload


def scenarios():
    out = []
    s = relay("basic", forget=10.0)
    for c in "abc":
        s.uopen(c)
    s.usend("a", R(1))
    s.usend("b", R(2))
    s.usend("a", D(2, 1, b"hello"))
    s.urecv("b")
    s.usend("b", D(1, 2, b"\x00\x01\x02back"))
    s.urecv("a")
    s.usend("a", D(2, 1))                       # an empty payload
    s.urecv("b")
    s.usend("a", D(5, 1, b"nobody home"))       # a seat nobody holds: dropped
    s.urecv("b")
    s.usend("a", D(1, 1, b"to myself"))
    s.urecv("a")
    s.usend("c", D(1, 3, b"a first packet claims seat 3"))
    s.urecv("a")
    s.usend("a", D(3, 1, b"and seat 3 answers"))
    s.urecv("c")
    s.usend("a", D(2, 1, bytes(range(256)) * 16 + b"x" * 49))   # 7 + 4145 = 4152 bytes
    s.urecv("b")
    s.usend("a", D(2, 1, b"y" * 4193))                          # 4200 bytes, the buffer's size
    s.urecv("b")
    s.usend("a", b"WRL1P" + b"tok")             # pings
    s.urecv("a")
    s.usend("a", b"WRL1P" + b"t" * 59)          # 64 bytes: answered
    s.urecv("a")
    s.usend("a", b"WRL1P" + b"t" * 60)          # 65 bytes: not
    s.urecv("a")
    s.usend("a", b"WRL1P")                      # 5 bytes: too short
    s.urecv("a")
    s.usend("a", b"WRL1P\x00")                  # 6 bytes: answered
    s.urecv("a")
    for junk in (b"", b"WRL1", b"XXXXR\x01", b"WRL1R\x00", b"WRL1R\x09", b"WRL1R", b"WRL1D\x02\x01", b"WRL1D\x00\x01x",
                 b"WRL1D\x09\x01x", b"WRL1D\x02\x00x", b"WRL1D\x02\x09x", b"WRL1Xabcdefg", b"wrl1R\x01", b"WRL2R\x01"):
        s.usend("c", junk)
    s.urecv("a")
    s.urecv("b")
    s.urecv("c")
    out.append(s.finish())

    s = relay("hijack")                         # a live seat is its owner's; a quiet one is free (A6)
    for c in "abc":
        s.uopen(c)
    s.usend("a", R(1))
    s.usend("b", R(2))
    s.usend("c", R(1))                          # a stranger claims live seat 1: rejected
    s.usend("c", D(2, 1, b"hijack"))            # and cannot send as seat 1
    s.urecv("b")
    s.usend("b", D(1, 2, b"for the owner"))
    s.urecv("a")
    s.urecv("c")
    s.usend("a", D(2, 1, b"the owner still works"))
    s.urecv("b")
    s.usend("c", R(2))
    s.usend("a", D(2, 1, b"seat 2 is still b"))
    s.urecv("b")
    s.urecv("c")
    s.sleep(FORGET + 0.4)                       # seat 1 (a) and seat 2 (b) go quiet
    s.usend("c", R(1))                          # now the stranger may have seat 1
    s.usend("b", D(1, 2, b"now c's"))
    s.urecv("c")
    s.urecv("a")
    s.usend("a", D(2, 1, b"a is out"))          # a is rejected: c holds seat 1 and is live
    s.urecv("b")
    out.append(s.finish())

    s = relay("forget")
    for c in "ab":
        s.uopen(c)
    s.usend("a", R(1))
    s.usend("b", R(2))
    s.sleep(FORGET + 0.4)
    s.usend("a", D(2, 1, b"b has gone quiet"))  # dropped: seat 2 not heard from
    s.urecv("b")
    s.usend("b", R(2))
    s.usend("a", D(2, 1, b"b is back"))
    s.urecv("b")
    s.sleep(FORGET / 2)
    s.usend("b", R(2))                          # a refresh keeps it
    s.sleep(FORGET / 2 + 0.1)
    s.usend("a", D(2, 1, b"refreshed"))
    s.urecv("b")
    out.append(s.finish())

    s = relay("relay1718", edition="2017-18")   # no owner rule: another address takes a live seat at once
    for c in "abc":
        s.uopen(c)
    s.usend("a", R(1))
    s.usend("b", R(2))
    s.usend("c", R(1))
    s.usend("b", D(1, 2, b"goes to c"))
    s.urecv("c")
    s.urecv("a")
    s.usend("a", D(2, 1, b"a takes it back"))
    s.urecv("b")
    s.usend("b", D(1, 2, b"to a"))
    s.urecv("a")
    s.urecv("c")
    s.usend("a", b"WRL1P" + b"tok")
    s.urecv("a")
    s.usend("a", D(7, 1, b"nobody"))
    s.urecv("b")
    out.append(s.finish())
    return out


if __name__ == "__main__":
    write("relay.jsonl", scenarios())
