# -*- coding: utf-8 -*-
"""test_exe_text.py - the 2017-18 program text sheets (editions/2017-18/{english,italian}/exe_text.tsv) against the
game: every row's original bytes are at its rva in client_Release.exe (length + SHA-256, NUL after them), the room is
free (zeros after the NUL, the next text starts no earlier), and every translation fits its room, is ASCII and keeps the
original's printf codes and $c colour marks (the game uses each text as a format). Read-only on the exe.
    python legacy/2017-18/tests/test_exe_text.py [path\\to\\client_Release.exe]"""
import hashlib
import os
import re
import struct
import sys

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(REPO), "ringedge", "1718-decryption-20261007-1944", "game", "client_Release.exe")
SPEC = re.compile(rb"%[-+ #0]*\d*(?:\.\d+)?[a-zA-Z%]")

d = open(EXE, "rb").read()
pe = struct.unpack_from("<I", d, 0x3C)[0]
nsec, opt = struct.unpack_from("<H", d, pe + 6)[0], struct.unpack_from("<H", d, pe + 20)[0]
secs = [struct.unpack_from("<IIII", d, pe + 24 + opt + 40 * i + 8) for i in range(nsec)]  # vsize, rva, rsize, raw


def file_off(rva):
    return next(raw + rva - va for vs, va, rs, raw in secs if va <= rva < va + min(vs, rs))


def sheet(lang):
    path = os.path.join(REPO, "editions", "2017-18", lang, "exe_text.tsv")
    lines = [ln for ln in open(path, encoding="utf-8").read().split("\n") if ln and not ln.startswith("#")]
    head = lines[0].split("\t")
    assert head[:5] == ["rva", "length", "original_sha256", "room", lang], head
    return [ln.split("\t") for ln in lines[1:]]


bad, rows = [], {}
for lang in ("english", "italian"):
    rows[lang] = sheet(lang)
    for rva, length, sha, room, text, *_ in rows[lang]:
        rva, length, room = int(rva, 16), int(length), int(room)
        at = file_off(rva)
        orig = d[at:at + length]
        if hashlib.sha256(orig).hexdigest().upper() != sha or d[at + length] != 0:
            bad.append("%s 0x%X: not the original bytes" % (lang, rva))
            continue
        if any(d[at + length:at + room]) or room > ((length + 4) & ~3):
            bad.append("%s 0x%X: room %d is not free padding" % (lang, rva, room))
        new = text.replace("\\n", "\n").encode("ascii", "replace")
        if b"?" in new and "?" not in text:
            bad.append("%s 0x%X: not ASCII" % (lang, rva))
        if len(new) + 1 > room:
            bad.append("%s 0x%X: %d bytes + NUL > room %d" % (lang, rva, len(new), room))
        if SPEC.findall(new) != SPEC.findall(orig) or new.count(b"$c") != orig.count(b"$c"):
            bad.append("%s 0x%X: printf codes or $c marks differ from the original" % (lang, rva))
assert [r[:4] for r in rows["english"]] == [r[:4] for r in rows["italian"]], "the two sheets list other rows"
assert not bad, "%d problem(s):\n%s" % (len(bad), "\n".join(bad))
print("exe_text: %d rows per language match %s and fit" % (len(rows["english"]), os.path.basename(EXE)))
