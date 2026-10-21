# Smoke test of the 2010-11 seat desk (forza-server desk) and the projector relay on throwaway loopback ports (39030,
# 39031, a fake control on 39002); touches no game port and starts no game.
#   python3 smoke.py [FORZA_SERVER] [PROJECTOR_RELAY_PY]     (default: $FORZA_HOME/relay/..., FORZA_HOME=/srv/forza-wccf)
# Exit 0 = all 5 checks pass.
import os, socket, struct, subprocess, sys, tempfile, time

D = os.path.join(os.environ.get("FORZA_HOME", "/srv/forza-wccf"), "relay")
DESK = sys.argv[1] if len(sys.argv) > 1 else os.path.join(D, "forza-server")
RELAY = sys.argv[2] if len(sys.argv) > 2 else os.path.join(D, "projector_relay.py")
tmp = tempfile.mkdtemp(prefix="projsmoke_")
ctl_log = os.path.join(tmp, "game.log")
open(ctl_log, "w").close()                                   # control's log: nobody in the game
env = dict(os.environ, WCCF_BIND="127.0.0.1", WCCF_BROKER_PORT="39030", WCCF_PROJECTOR_PORT="39031",
           WCCF_CONTROL_PORT="39002", WCCF_CONTROL_LOG=ctl_log)
fake = socket.socket(); fake.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
fake.bind(("127.0.0.1", 39002)); fake.listen(4); fake.settimeout(0.2)
cmds = {"seat_desk": [DESK, "desk", "120", os.path.join(tmp, "seat_desk.txt")],
        "projector_relay": [sys.executable, "-u", RELAY, "120", os.path.join(tmp, "projector_relay.txt")]}
procs = [subprocess.Popen(c, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT) for c in cmds.values()]
ok = []
def check(name, cond):
    ok.append(bool(cond)); print(("PASS " if cond else "FAIL ") + name, flush=True)
def ask(line):
    s = socket.create_connection(("127.0.0.1", 39030), timeout=5); s.sendall(line); r = s.recv(100); s.close(); return r
def pack(name, body):
    return struct.pack("<II16sI", 1, 28 + len(body), name.encode(), len(body)) + body
def upstream():
    try:
        c, _ = fake.accept(); c.close(); return True
    except socket.timeout:
        return False
try:
    time.sleep(1.5)
    check("desk WANT RELAY -> SEAT 1 PROJECTOR 39031", ask(b"WANT RELAY\n") == b"SEAT 1 PROJECTOR 39031\n")
    check("desk WANT -> PROJECTOR 0", ask(b"WANT\n").endswith(b" PROJECTOR 0\n"))
    p = socket.create_connection(("127.0.0.1", 39031), timeout=5)
    p.sendall(pack("INIT", b"\0" * 32))
    r = p.recv(100)
    check("relay answers INIT with INIT REPLY OK\\0", r == pack("INIT REPLY", b"OK\0"))
    time.sleep(3)
    check("no connection to control while no player is in", not upstream())
    with open(ctl_log, "a") as f:                            # a player joins: seat 1 in control's log
        f.write("ClientManage::INIT() : client[1] : sateID=1,version=7128,serial=\n"
                "ClientManage::INIT() : ++ACCEPT++  '198.51.100.9:2898', 2026/10/17\n")
    end = time.time() + 4; seen = False
    while time.time() < end and not seen:
        seen = upstream()
    check("relay opens control's connection once a player is in", seen)
    p.close()
finally:
    for pr in procs:
        pr.terminate(); pr.wait(5)
    fake.close()
    for n in ("seat_desk", "projector_relay"):
        print("--- " + n); print(open(os.path.join(tmp, n + ".txt")).read().strip())
print("%d of %d passed" % (sum(ok), len(ok)))
sys.exit(0 if all(ok) else 1)
