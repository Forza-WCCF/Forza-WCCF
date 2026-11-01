# Asks a desk for its LOBBY from any PC and checks the answer: one JSON line, the right edition, no IPv4 address.
#   python3 lobby_check.py HOST[:PORT] EDITION [--room]
# --room (2017-18): also CREATE vs, then CLOSE it at once (leaves nothing behind). Exit 0 = pass.
import json, re, socket, sys

host, _, port = sys.argv[1].partition(":")
edition, room = sys.argv[2], "--room" in sys.argv


def say(line):
    with socket.create_connection((host, int(port or 20030)), timeout=8) as s:
        s.sendall(line.encode() + b"\n")
        buf = b""
        while not buf.endswith(b"\n"):
            d = s.recv(65536)
            if not d:
                break
            buf += d
        return buf.decode("utf-8", "replace").strip()


ok = True
ans = say("LOBBY " + edition)
print("LOBBY ->", ans[:300])
try:
    j = json.loads(ans)
    # "shops": the LOBBY wire field, kept from before the arcade rename
    good = j.get("v") == 1 and j.get("edition") == edition and isinstance(j.get("shops"), list)
except ValueError:
    good = False
ip_free = not re.search(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b", ans)
print(("PASS" if good else "FAIL") + " LOBBY answers JSON for " + edition)
print(("PASS" if ip_free else "FAIL") + " no IPv4 address in the answer")
ok &= good and ip_free
if room:
    r = say("CREATE vs").split()
    made = len(r) >= 4 and r[0] == "ROOM"
    print(("PASS" if made else "FAIL") + " CREATE vs -> " + " ".join(r[:3] + ["<ticket>"] * (len(r) > 3)))
    if made:
        c = say("CLOSE %s %s" % (r[1], r[3]))
        print(("PASS" if c == "OK" else "FAIL") + " CLOSE -> " + c)
        made &= c == "OK"
    ok &= made
sys.exit(0 if ok else 1)
