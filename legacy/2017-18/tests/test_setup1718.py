# -*- coding: utf-8 -*-
"""test_setup.py - the pure parts of the 2017-18 setup (no game): an unknown program is refused, the originals are
kept on the first run and used on every later one, the settings merge keeps other keys, a junction is made.
    python tests\\test_setup.py        (the kit's Python or any Python 3; the junction check needs Windows)"""
import os
import sys
import tempfile

KIT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(KIT, "kit", "scripts"))
sys.path.insert(0, os.path.join(KIT, "scripts"))          # the staged kit: tests\ and scripts\ side by side
import setup as S  # noqa: E402

with tempfile.TemporaryDirectory() as t:
    exe = {"client_Release.exe": S.sha(b"plain")}
    cur, bak = os.path.join(t, "game", "client_Release.exe"), os.path.join(t, "original", "client_Release.exe")
    os.makedirs(os.path.dirname(cur))
    open(cur, "wb").write(b"plain")
    assert S.original("client_Release.exe", cur, bak, exe) == b"plain" and open(bak, "rb").read() == b"plain"
    open(cur, "wb").write(b"patched")                      # a later run: the game's file is changed, the backup is used
    assert S.original("client_Release.exe", cur, bak, exe) == b"plain"
    open(bak, "wb").write(b"other")                        # a damaged backup is refused, not trusted
    try:
        S.original("client_Release.exe", cur, bak, exe)
        raise AssertionError("a wrong backup was accepted")
    except S.Failed as ex:
        assert "not the original" in str(ex)
    os.remove(bak)
    try:                                                    # no backup and an unknown program: refused, nothing kept
        S.original("client_Release.exe", cur, bak, exe)
        raise AssertionError("an unknown program was accepted")
    except S.Failed:
        assert not os.path.exists(bak)

    cfg = os.path.join(t, "data", "settings.json")
    S.merge_settings(cfg, language="it")
    S.merge_settings(cfg, game="X")
    assert S.json.load(open(cfg)) == {"language": "it", "game": "X"}

    if sys.platform == "win32":
        os.makedirs(os.path.join(t, "a"))
        S.make_junction(os.path.join(t, "b"), os.path.join(t, "a"))
        S.make_junction(os.path.join(t, "b"), os.path.join(t, "a"))   # again: nothing to do
        open(os.path.join(t, "a", "f"), "w").close()
        assert os.path.isfile(os.path.join(t, "b", "f"))
# free play: the one patch turns the default coin setting's byte 2 on, and refuses any other original byte
fp = [e for e in S.P.PATCHES if "FREE PLAY" in e[3]]
assert len(fp) == 1 and fp[0][1:3] == ("00", "01")
buf = bytes(fp[0][0] + 1)
assert S.P.apply(buf, fp)[fp[0][0]] == 1
try:
    S.P.apply(b"\x02" * (fp[0][0] + 1), fp)
    raise AssertionError("a changed original byte was accepted")
except ValueError:
    pass
assert S.find_game(os.path.join(os.sep, "no", "such")) is None
print("2017-18 setup: unknown programs refused, originals kept and reused, settings merged%s"
      % (", junction made" if sys.platform == "win32" else ""))
