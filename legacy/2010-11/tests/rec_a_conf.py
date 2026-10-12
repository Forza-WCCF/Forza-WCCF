# -*- coding: utf-8 -*-
"""rec_a_conf.py - records play.py's set_control_ip, set_satellite_no and set_conf_value on synthetic settings files
into crates/launcher/tests/rec/conf.jsonl, which crates/launcher/tests/rec_conf.rs replays against launcher::conf.
Step 10 piece A: kept until piece G deletes the Python it records (play.py).
Synthetic only: made-up files in a temporary folder; it starts nothing.  Bytes are stored as Latin-1 text.
    python legacy\\2010-11\\tests\\rec_a_conf.py"""
import json
import os
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "code"))
import play  # noqa: E402

OUT = os.path.join(REPO, "crates", "launcher", "tests", "rec", "conf.jsonl")
FILES = [
    b"",
    b"CONTROL_IP=127.0.0.1\r\nMATCH_IP=127.0.0.1\r\nSATELLITE_NO=1\r\nOTHER=x\r\n",
    b"CONTROL_IP=127.0.0.1\nMATCH_IP=127.0.0.1\nSATELLITE_NO=1\nOTHER=x\n",
    b"CONTROL_IP=10.0.0.2\nMATCH_IP=10.0.0.2\nSATELLITE_NO=3",                  # no line end at the end
    b"A=1\r\nCONTROL_IP=1.2.3.4\nCONTROL_IPX=9\r\nMATCH_IP=\r\nSATELLITE_NO=12\r\n",   # mixed line ends
    b"A=1\rCONTROL_IP=1.2.3.4\rSATELLITE_NO=2\n",                               # lone CRs: one line to the game
    b" CONTROL_IP=1.1.1.1\r\nSATELLITE_NO =4\r\n",                             # a space before or after the key
    b"CONTROL_IP=1.2.3.4\r\nCONTROL_IP=5.6.7.8\r\nSATELLITE_NO=1\r\nSATELLITE_NO=2\r\n",   # twice
    b"NAME=\x82\xa0\x82\xa2\r\nCONTROL_IP=127.0.0.1\r\nA.B=\xff\xfe\r\nSATELLITE_NO=1\r\n",   # Shift-JIS, odd bytes
    b"WAIT_PROJECTOR=1\nKEY=value=with=equals\n",
]
OPS = [
    ("set_control_ip", ["127.0.0.1"]), ("set_control_ip", ["192.168.1.20"]),
    ("set_satellite_no", [1]), ("set_satellite_no", [5]),
    ("set_conf_value", ["SATELLITE_NO", "1"]), ("set_conf_value", ["WAIT_PROJECTOR", "0"]),
    ("set_conf_value", ["A.B", "2"]), ("set_conf_value", ["KEY", "value=with=equals"]), ("set_conf_value", ["NEW", ""]),
]


def main():
    root = tempfile.mkdtemp(prefix="forza-rec-conf-")
    n = 0
    try:
        conf = os.path.join(root, "client_user_option.conf")
        with open(OUT, "w", encoding="utf-8", newline="\n") as out:
            for raw in FILES + [None]:                     # None: no file
                for fn, args in OPS:
                    if os.path.exists(conf):
                        os.remove(conf)
                    if raw is not None:
                        with open(conf, "wb") as f:
                            f.write(raw)
                    rec = {"fn": fn, "args": args, "before": None if raw is None else raw.decode("latin-1")}
                    try:
                        rec["changed"] = getattr(play, fn)(conf, *args)
                    except play.Failed as ex:
                        rec["error"] = str(ex).replace(conf, "{conf}")
                    except OSError:
                        rec["error"] = "os"
                    if raw is not None:
                        with open(conf, "rb") as f:
                            rec["after"] = f.read().decode("latin-1")
                    rec["left"] = sorted(os.listdir(root))
                    out.write(json.dumps(rec) + "\n")
                    n += 1
        print("recorded %d cases into %s" % (n, OUT))
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
