# -*- coding: utf-8 -*-
"""test_pictures1718.py - the writing in the 2017-18 game's pictures (cargo xtask check runs it; no game, nothing live):
a scratch archive with one YS-packed DXT5 picture gets the redrawn picture added at its end and its entry pointed at
it; "on" twice gives the same file; a picture that is not the recipe's is skipped; "off" gives back the original
archive byte for byte, also after an "on" stopped before the entries were written; YS packing and the uncompressed
DDS formats (A8R8G8B8 with mips, 16-bit A4R4G4B4) round-trip.  Needs Pillow (the kit's python has it).
    python source\\test_pictures1718.py"""
import io
import json
import os
import shutil
import struct
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
KIT = os.path.dirname(HERE)                                   # the staged kit (source\..) or legacy/2017-18
sys.path.insert(0, os.path.join(KIT, "scripts") if os.path.isdir(os.path.join(KIT, "scripts"))
                else os.path.join(KIT, "kit", "scripts"))
import pictures1718 as P  # noqa: E402
import ys_lzw  # noqa: E402
from PIL import Image  # noqa: E402

FONT = next(p for p in (os.path.join(KIT, P.FONT), os.path.join(KIT, "..", "..", "assets", "fonts",
                                                                 os.path.basename(P.FONT))) if os.path.isfile(p))


def dds_header(w, h, mips, fourcc=b"", bits=0, masks=(0, 0, 0, 0)):
    pf_flags = 4 if fourcc else (0x41 if masks[3] else 0x40)
    return struct.pack("<4s7I44x2I4s5I5I", b"DDS ", 124, 0x1007 | (0x20000 if mips > 1 else 0), h, w, 0, 0, mips,
                       32, pf_flags, fourcc, bits, *masks, 0x1000, 0, 0, 0, 0)


# 1. YS packing: the decoder reads back what the packer wrote, across a table reset (more than 3,840 codes)
data = bytes((i * 7 + (i >> 5)) & 255 for i in range(40000)) + b"WCCF" * 3000
assert ys_lzw.decompress(P.ys_pack(data), len(data)) == data, "YS round trip"

# 2. uncompressed formats: an unchanged picture encodes back to the same bytes
top = Image.new("RGBA", (8, 8))
top.putdata([(x * 32, y * 32, 255 - x * 16, 255 if (x + y) % 3 else 0) for y in range(8) for x in range(8)])
lv = [top, top.resize((4, 4), Image.BOX), top.resize((4, 4), Image.BOX).resize((2, 2), Image.BOX)]
lv.append(lv[-1].resize((1, 1), Image.BOX))
argb = dds_header(8, 8, 4, bits=32, masks=(0xFF0000, 0xFF00, 0xFF, 0xFF000000)) + b"".join(
    i.tobytes("raw", "BGRA") for i in lv)
assert P.encode(argb, top) == argb, "A8R8G8B8 with 4 mips round trip"
q = Image.new("RGBA", (8, 4))
q.putdata([(r * 17, (r * 5 % 16) * 17, (15 - r) * 17, (r * 3 % 16) * 17) for r in range(16)] * 2)
a4 = dds_header(8, 4, 0, bits=16, masks=(0xF00, 0xF0, 0xF, 0xF000)) + bytes(
    b for p in zip(*[iter(q.tobytes())] * 4) for b in (((p[1] // 17) << 4 | p[2] // 17), ((p[3] // 17) << 4 | p[0] // 17)))
assert P.encode(a4, Image.open(io.BytesIO(a4)).convert("RGBA")) == a4, "A4R4G4B4 round trip"

# 3. the archive: one packed DXT5 picture ("the original writing" = a white block) in sector 2
pic = Image.new("RGBA", (64, 32), (0, 0, 0, 0))
pic.paste((255, 255, 255, 255), (8, 8, 56, 24))
b = io.BytesIO()
pic.save(b, "DDS", pixel_format="DXT5")
original_dds = b.getvalue()
packed = P.ys_pack(original_dds)
scratch = tempfile.mkdtemp(prefix="wccf_pictures1718_")
game, kit = os.path.join(scratch, "game"), os.path.join(scratch, "kit")
for d in ("data", ):
    os.makedirs(os.path.join(game, d))
os.makedirs(os.path.join(kit, "english"))
os.makedirs(os.path.join(kit, "italian"))
os.makedirs(os.path.join(kit, "fonts"))
shutil.copyfile(FONT, os.path.join(kit, P.FONT))
state = os.path.join(scratch, "english_backup", "wccf_data.xaf.json")
xaf = os.path.join(game, P.XAF)
toc = bytearray(2 * P.ENTRY)                                   # a folder "ui" and its file "pic.dds"
toc[:2], toc[0x80] = b"ui", 0
struct.pack_into("<i", toc, 0x84, -1)
e = memoryview(toc)[P.ENTRY:]
e[:7], e[0x80], e[0x81] = b"pic.dds", 1, 1
struct.pack_into("<i", e, 0x84, 0)
struct.pack_into("<II", e, 0x94, len(original_dds), len(packed))
struct.pack_into("<I", e, 0xA0, 1)
hdr = bytearray(P.TOC_AT)
struct.pack_into("<4s3I", hdr, 0, b"xaf0", 2, P.SECTOR, 2)
sectors = 1 + -(-len(packed) // P.SECTOR)
struct.pack_into("<3Q", hdr, 0x18, sectors, 1, sectors - 1)
original = bytes(hdr + toc).ljust(P.SECTOR, b"\0") + packed + bytes(-len(packed) % P.SECTOR)
with open(xaf, "wb") as f:
    f.write(original)


def archive():
    with open(xaf, "rb") as f:
        return f.read()


def recipe(h):
    with open(os.path.join(kit, "english", "pictures.json"), "w", encoding="utf-8") as f:
        json.dump({"pictures": [{"id": "test_pic", "sha256": h, "files": ["ui/pic.dds"],
                                 "style": {"core": "auto", "halo": [0, 0, 0, 200], "halo_px": 1},
                                 "labels": [{"box": [6, 6, 57, 25], "en": "OK"}]}]}, f)


recipe(P.sha(original_dds))
with open(os.path.join(kit, "italian", "pictures.json"), "w", encoding="utf-8") as f:
    json.dump({"test_pic": ["Sì"]}, f)
pics, notes = P.build(game, kit, "en", "all", state)
assert list(pics) == [1] and pics[1][0] == "ui/pic.dds", notes
assert P.apply(game, pics, state) == 1
on = archive()
assert len(on) > len(original) and on[P.SECTOR:len(original)] == original[P.SECTOR:], "added at the end, originals kept"
assert struct.unpack_from("<Q", on, 0x18)[0] == len(on) // P.SECTOR, "the header's sector count grows"
ne = on[P.TOC_AT + P.ENTRY:P.TOC_AT + 2 * P.ENTRY]
assert ne[0x81] == 1 and struct.unpack_from("<I", ne, 0xA0)[0] == len(original) // P.SECTOR, "entry -> added picture"
with open(xaf, "rb") as f:
    new_dds = P.read_entry(f, ne)
assert len(new_dds) == len(original_dds) and new_dds[:128] == original_dds[:128], "same DDS header and size"
new = Image.open(io.BytesIO(new_dds)).convert("RGBA")
assert new.tobytes() != pic.convert("RGBA").tobytes() and new.getpixel((9, 9))[3] < 100, "the old writing is cleared"
assert new.getchannel("A").getbbox() is not None, "the words are drawn"

assert P.apply(game, P.build(game, kit, "en", "all", state)[0], state) == 1 and archive() == on, "on twice: same file"
it = P.build(game, kit, "it", "all", state)[0]
assert it[1][1] != pics[1][1], "Italian words differ from the English"
assert P.build(game, kit, "en", "pic", state)[0].keys() == {1}, "a subset by file name"
assert P.build(game, kit, "en", "none", state)[0] == {}, "none"
try:
    P.build(game, kit, "en", "no_such_picture", state)
    raise AssertionError("a subset that names nothing must refuse")
except P.Failed:
    pass

assert P.restore(game, state) == 1 and archive() == original, "off gives back the original archive byte for byte"
assert not os.path.exists(state) and P.restore(game, state) == 0

P.apply(game, pics, state)                                     # stopped half way: way back saved, entry not written
with open(xaf, "r+b") as f:
    f.seek(P.TOC_AT + P.ENTRY)
    f.write(bytes(e))
    f.seek(0x18)
    f.write(hdr[0x18:0x30])
assert P.restore(game, state) == 1 and archive() == original, "off after an interrupted on: the original archive"

recipe("00" * 32)                                              # a different picture: skipped, nothing written
pics, notes = P.build(game, kit, "en", "all", state)
assert pics == {} and "not the picture the recipe is for" in notes[1], notes
assert P.apply(game, pics, state) == 0 and archive() == original

shutil.rmtree(scratch)
print("2017-18 pictures: 16 checks passed")
