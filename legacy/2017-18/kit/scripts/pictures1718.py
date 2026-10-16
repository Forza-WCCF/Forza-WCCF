# -*- coding: utf-8 -*-
r"""pictures1718.py - the writing inside the 2017-18 game's pictures in English or Italian (english.py runs it).

The game reads its UI pictures (DDS) only from data\wccf_data.xaf.  For each recipe in english\pictures.json the
player's own picture is unpacked from the archive and checked against the recipe's SHA-256 (a picture that differs is
skipped, never guessed at); the writing in each label box is cleared and the kit's words are drawn there with the
kit's font (fonts\BarlowCondensed-Bold.ttf, OFL), in the colour of the original writing (sampled from the player's
pixels); the result is encoded back to the picture's own DDS format, size and mip count, packed as the archive packs
(YS LZW), ADDED at the archive's end and its table-of-contents entry pointed at it.  No byte of the originals is
written over.  The way back (old entries, header, length) is saved first in data\english_backup\wccf_data.xaf.json;
"off" puts it back and cuts the file to its old length, byte for byte.  Japanese = the original pixels (no recipe
runs).  Nothing of the original ships with the kit: the recipes hold hashes, boxes and our own words only.

Recipe (english\pictures.json, "pictures" list):
  id, where (what the player sees), sha256 (of the unpacked original), files (archive paths with that picture),
  style {core: "auto" | "dark" | [r,g,b], halo: [r,g,b,a] | "core" (+ halo_alpha) | null, halo_px, blur, fill,
         squeeze, align, size},
  labels [{box: [x0,y0,x1,y1] (inclusive), en: words, optional clear: [x0,y0,x1,y1] (a larger area to clear),
           and any style key for this label only}]
  fill: "clear" (alpha 0, default), "rows" (each column blended from the row above the area to the row below),
        "cols" (each row from the column left of it to the one right of it) or [r,g,b,a].
italian\pictures.json: {id: [Italian words, one per label; null = the English]}.
Which pictures: WCCF_PICTURES or english.py's pictures=SPEC: "all" (default), "none", or names separated by commas
(a recipe id or a file name without .dds, e.g. 1112_SCM_Parameter).
"""
import hashlib
import io
import json
import os
import struct

import ys_lzw

XAF = os.path.join("data", "wccf_data.xaf")
SECTOR, ENTRY, TOC_AT = 2048, 0xB0, 0x100
FONT = os.path.join("fonts", "BarlowCondensed-Bold.ttf")
S = 4                                                    # words drawn 4x larger, then made small: smooth edges


class Failed(Exception):
    pass


def sha(data):
    return hashlib.sha256(data).hexdigest().upper()


# ---------------------------------------------------------------- recipes
def recipes(kit, lang="en"):
    """the recipes with each label's words in lang (Italian where italian\\pictures.json has them, else English)"""
    with open(os.path.join(kit, "english", "pictures.json"), encoding="utf-8") as f:
        out = json.load(f)["pictures"]
    it = {}
    if lang == "it":
        p = os.path.join(kit, "italian", "pictures.json")
        if os.path.isfile(p):
            with open(p, encoding="utf-8") as f:
                it = json.load(f)
    for r in out:
        words = it.get(r["id"]) or []
        if words and len(words) != len(r["labels"]):
            raise Failed("italian\\pictures.json: %s has %d words for %d labels" % (r["id"], len(words),
                                                                                   len(r["labels"])))
        for n, lab in enumerate(r["labels"]):
            lab["text"] = (words[n] if n < len(words) and words[n] else None) or lab["en"]
    return out


def wanted(spec):
    """None = every picture, else the set of names (lower case); "none" = the empty set"""
    spec = (spec or "all").strip().lower()
    if spec in ("all", "on", "1", ""):
        return None
    if spec in ("none", "off", "0"):
        return set()
    return {s.strip() for s in spec.split(",") if s.strip()}


def chosen(r, path, names):
    base = os.path.splitext(path.rsplit("/", 1)[-1])[0].lower()
    return names is None or r["id"].lower() in names or base in names


# ---------------------------------------------------------------- drawing
def colour(im, box, how):
    """the original writing's colour: the median of the brightest ("auto") or darkest ("dark") 40% of the opaque
    pixels in the box; or the recipe's [r,g,b]"""
    if isinstance(how, list):
        return tuple(how[:3])
    d = im.crop((box[0], box[1], box[2] + 1, box[3] + 1)).tobytes()
    px = [d[i:i + 4] for i in range(0, len(d), 4) if d[i + 3] > 200]
    if not px:
        return (255, 255, 255)
    px.sort(key=lambda p: p[0] * 3 + p[1] * 6 + p[2], reverse=how != "dark")
    px = px[:max(1, len(px) * 2 // 5)]
    return tuple(sorted(p[c] for p in px)[len(px) // 2] for c in range(3))


def clear(im, area, fill):
    x0, y0, x1, y1 = area
    if fill in ("rows", "cols"):
        src = im.copy()
        w, h = im.size
        for x in range(x0, x1 + 1):
            for y in range(y0, y1 + 1):
                if fill == "rows":
                    a, b, t = src.getpixel((x, max(0, y0 - 1))), src.getpixel((x, min(h - 1, y1 + 1))), \
                        (y - y0 + 1) / (y1 - y0 + 2)
                else:
                    a, b, t = src.getpixel((max(0, x0 - 1), y)), src.getpixel((min(w - 1, x1 + 1), y)), \
                        (x - x0 + 1) / (x1 - x0 + 2)
                im.putpixel((x, y), tuple(round(a[c] + (b[c] - a[c]) * t) for c in range(4)))
    else:
        im.paste(tuple(fill) if isinstance(fill, list) else (0, 0, 0, 0), (x0, y0, x1 + 1, y1 + 1))


def draw(im, box, text, st, core, font_path):
    """text in box: as tall as the box allows (inside the halo), squeezed sideways down to st squeeze, then smaller"""
    from PIL import Image, ImageDraw, ImageFilter, ImageFont
    x0, y0, x1, y1 = box
    pad = st.get("halo_px", 0) if st.get("halo") else 0
    w, h = x1 - x0 + 1 - 2 * pad, st.get("size") or (y1 - y0 + 1 - 2 * pad)
    ref = ImageFont.truetype(font_path, 100).getbbox("Hgjy", anchor="ls")
    font = ImageFont.truetype(font_path, max(4, round(100 * h * S / (ref[3] - ref[1]))))
    l, t, r, b = font.getbbox(text, anchor="ls")
    m = Image.new("L", (r - l + 2, b - t + 2))
    ImageDraw.Draw(m).text((1 - l, 1 - t), text, font=font, fill=255, anchor="ls")
    m = m.crop(m.getbbox() or (0, 0, 1, 1))              # the ink only: centred as the eye sees it
    sx = min(1.0, w * S / m.width)
    u = min(1.0, sx / st.get("squeeze", 0.62))           # squeezed more than this: smaller instead
    sx = max(sx, st.get("squeeze", 0.62)) * u
    mw, mh = max(1, round(m.width * sx / S)), max(1, round(m.height * u / S))
    m = m.resize((mw, mh), Image.LANCZOS)
    align = st.get("align", "center")
    x = x0 + pad if align == "left" else x1 - pad + 1 - mw if align == "right" else x0 + (x1 - x0 + 1 - mw) // 2
    mask = Image.new("L", im.size)
    mask.paste(m, (x, y0 + (y1 - y0 + 1 - mh) // 2))
    halo = st.get("halo")
    if halo:
        hc = tuple(core) + (st.get("halo_alpha", 160),) if halo == "core" else tuple(halo)
        g = mask.filter(ImageFilter.MaxFilter(2 * st.get("halo_px", 1) + 1))
        if st.get("blur", 0.6):
            g = g.filter(ImageFilter.GaussianBlur(st.get("blur", 0.6)))
        im.alpha_composite(Image.merge("RGBA", Image.new("RGB", im.size, hc[:3]).split()
                                       + (g.point(lambda v: v * hc[3] // 255),)))
    im.alpha_composite(Image.merge("RGBA", Image.new("RGB", im.size, tuple(core)).split() + (mask,)))


def redraw(im, r, font_path):
    """every label of recipe r cleared first, then drawn (a neighbour's halo is never cleared away)"""
    orig = im.copy()
    for lab in r["labels"]:
        st = dict(r.get("style", {}), **lab)
        clear(im, lab.get("clear", lab["box"]), st.get("fill", "clear"))
    for lab in r["labels"]:
        st = dict(r.get("style", {}), **lab)
        draw(im, lab["box"], lab["text"], st, colour(orig, lab["box"], st.get("core", "auto")), font_path)
    return im


# ---------------------------------------------------------------- DDS: the original's format, size and mips
def pack_pixels(im, bits, masks):
    if bits == 32 and masks == (0xFF0000, 0xFF00, 0xFF, 0xFF000000):
        return im.tobytes("raw", "BGRA")
    chans = []
    for band, m in zip(im.split(), masks):
        if m:
            sh, n = (m & -m).bit_length() - 1, bin(m).count("1")
            chans.append(([((v * ((1 << n) - 1) + 127) // 255) << sh for v in range(256)], band.tobytes()))
    out = bytearray()
    nb = bits // 8
    for i in range(im.width * im.height):
        out += sum(t[d[i]] for t, d in chans).to_bytes(nb, "little")
    return bytes(out)


def encode(orig, im):
    """a DDS like orig (its 128-byte header kept): same format, size and mip count; the mips made from im"""
    from PIL import Image
    nm = max(1, struct.unpack_from("<I", orig, 28)[0])
    pf_flags, fourcc, bits = struct.unpack_from("<I4sI", orig, 80)
    masks = struct.unpack_from("<4I", orig, 92)
    body, lv = [], im
    for n in range(nm):
        if n:
            lv = lv.resize((max(1, lv.width // 2), max(1, lv.height // 2)), Image.BOX)
        if pf_flags & 4:
            if fourcc not in (b"DXT1", b"DXT3", b"DXT5"):
                raise Failed("DDS format %r" % fourcc)
            b = io.BytesIO()
            lv.save(b, "DDS", pixel_format=fourcc.decode())
            body.append(b.getvalue()[128:])
        else:
            body.append(pack_pixels(lv, bits, masks))
    out = orig[:128] + b"".join(body)
    if len(out) != len(orig):
        raise Failed("the redrawn picture's size %d differs from the original's %d" % (len(out), len(orig)))
    return out


# ---------------------------------------------------------------- YS packing (the exact inverse of ys_lzw)
def ys_pack(data):
    """YS LZW as the archive packs: early width change, a fresh table after 3,840 codes; ys_lzw reads it back"""
    out, acc, nb = bytearray(ys_lzw.MAGIC), 0, 0
    i, n = 0, len(data)
    while i < n:
        table, nxt, dn, dw, j = {}, 256, 256, 9, 0
        while i < n and j < 3840:
            code = data[i]
            i += 1
            while i < n and (code, data[i]) in table:
                code = table[(code, data[i])]
                i += 1
            acc, nb, j = (acc << dw) | code, nb + dw, j + 1
            while nb >= 8:
                nb -= 8
                out.append((acc >> nb) & 255)
            acc &= (1 << nb) - 1
            if i < n:
                table[(code, data[i])] = nxt
                nxt += 1
            if j >= 2:
                dn += 1
                if dn + 1 == (1 << dw) and dw < 12:
                    dw += 1
    if nb:
        out.append((acc << (8 - nb)) & 255)
    return bytes(out)


# ---------------------------------------------------------------- the archive
def load_state(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def toc(xaf):
    """{path: entry number} for every file in the archive"""
    with open(xaf, "rb") as f:
        hdr = f.read(TOC_AT)
        if hdr[:4] != b"xaf0":
            raise Failed("data\\wccf_data.xaf is not an xaf0 archive")
        n = struct.unpack_from("<I", hdr, 12)[0]
        raw = f.read(n * ENTRY)
    names, parents = [], []
    for i in range(n):
        e = raw[i * ENTRY:(i + 1) * ENTRY]
        names.append(e[:0x80].split(b"\0")[0].decode("latin-1"))
        parents.append(struct.unpack_from("<i", e, 0x84)[0])
    out = {}
    for i in range(n):
        if raw[i * ENTRY + 0x80]:
            parts, p = [names[i]], parents[i]
            while p >= 0:
                parts.append(names[p])
                p = parents[p]
            out["/".join(reversed(parts))] = i
    return out


def read_entry(f, e):
    size, stored = struct.unpack_from("<II", e, 0x94)
    f.seek(struct.unpack_from("<I", e, 0xA0)[0] * SECTOR)
    data = f.read(stored)
    return ys_lzw.decompress(data, size) if stored < size else data


def build(game, kit, lang, spec, state_path):
    """{entry number: (path, new entry data)}, notes: every chosen picture whose original matches its recipe, redrawn.
    Reads the original entries (from the saved way back while pictures are on)."""
    names = wanted(spec)
    if names is not None and not names:
        return {}, ["pictures: none chosen, they stay as the original"]
    font = os.path.join(kit, FONT)
    if not os.path.isfile(font):
        return {}, ["pictures stay as the original: %s is missing" % FONT]
    from PIL import Image
    xaf = os.path.join(game, XAF)
    saved = (load_state(state_path) or {}).get("entries", {})
    index = toc(xaf)
    out, skipped, seen = {}, [], set()
    with open(xaf, "rb") as f:
        for r in recipes(kit, lang):
            for p in r["files"]:
                if not chosen(r, p, names):
                    continue
                seen.add(p)
                if p not in index:
                    skipped.append("%s (not in the archive)" % p)
                    continue
                i = index[p]
                if str(i) in saved:
                    e = bytes.fromhex(saved[str(i)])
                else:
                    f.seek(TOC_AT + i * ENTRY)
                    e = f.read(ENTRY)
                data = read_entry(f, e)
                if sha(data) != r["sha256"].upper():
                    skipped.append("%s (not the picture the recipe is for)" % p)
                    continue
                im = Image.open(io.BytesIO(data))
                im.load()
                out[i] = (p, encode(data, redraw(im.convert("RGBA"), r, font)))
    if names is not None and not seen:
        raise Failed("pictures=%s names no picture of english\\pictures.json" % spec)
    notes = ["%d pictures with their writing in %s" % (len(out), {"it": "Italian"}.get(lang, "English"))]
    if skipped:
        notes.append("%d pictures stay as they are: %s%s" % (len(skipped), skipped[0], ", ..." if len(skipped) > 1
                                                             else ""))
    return out, notes


def restore(game, state_path):
    """the original entries and header back, the added pictures cut off; 0 if none were added"""
    state = load_state(state_path)
    if not state:
        return 0
    with open(os.path.join(game, XAF), "r+b") as f:
        if f.read(4) != b"xaf0" or f.seek(0, 2) < state["length"]:
            raise Failed("data\\wccf_data.xaf is shorter than before the pictures - left as it is")
        for i, raw in state["entries"].items():
            f.seek(TOC_AT + int(i) * ENTRY)
            f.write(bytes.fromhex(raw))
        f.seek(0x18)
        f.write(bytes.fromhex(state["header"]))
        f.flush()
        os.fsync(f.fileno())
        f.truncate(state["length"])
    os.remove(state_path)
    return len(state["entries"])


def apply(game, pics, state_path):
    """from the original archive every time (never stacked): the way back saved, the pictures added at the end (YS
    packed when that is smaller), the header grown, the entries pointed at them, each read back"""
    restore(game, state_path)
    if not pics:
        return 0
    with open(os.path.join(game, XAF), "r+b") as f:
        hdr = f.read(TOC_AT)
        length = f.seek(0, 2)
        total, toc_sectors, data_sectors = struct.unpack_from("<3Q", hdr, 0x18)
        if hdr[:4] != b"xaf0" or length % SECTOR or total * SECTOR != length:
            raise Failed("data\\wccf_data.xaf is not as the original was made (its length) - pictures left as they are")
        state = {"length": length, "header": hdr[0x18:0x30].hex(), "entries": {}}
        for i in pics:
            f.seek(TOC_AT + i * ENTRY)
            state["entries"][str(i)] = f.read(ENTRY).hex()
        os.makedirs(os.path.dirname(state_path), exist_ok=True)
        with open(state_path + ".tmp", "w", encoding="utf-8") as s:   # the way back, before the first byte is written
            json.dump(state, s, indent=1)
        os.replace(state_path + ".tmp", state_path)
        pos, entries = length, {}
        for i, (p, blob) in sorted(pics.items()):
            packed = ys_pack(blob)
            if len(packed) >= len(blob) or ys_lzw.decompress(packed, len(blob)) != blob:
                packed = blob
            f.seek(pos)
            f.write(packed + bytes(-len(packed) % SECTOR))
            e = bytearray.fromhex(state["entries"][str(i)])
            if packed is blob:
                e[0x81] = e[0x82] = 0                    # stored as is (3,541 original entries are)
            struct.pack_into("<II", e, 0x94, len(blob), len(packed))
            struct.pack_into("<I", e, 0xA0, pos // SECTOR)
            entries[i] = bytes(e)
            pos += len(packed) + (-len(packed) % SECTOR)
        f.flush()
        os.fsync(f.fileno())
        added = (pos - length) // SECTOR
        f.seek(0x18)
        f.write(struct.pack("<3Q", total + added, toc_sectors, data_sectors + added))
        for i, e in entries.items():
            f.seek(TOC_AT + i * ENTRY)
            f.write(e)
        f.flush()
        os.fsync(f.fileno())
        for i, (p, blob) in pics.items():
            f.seek(TOC_AT + i * ENTRY)
            e = f.read(ENTRY)
            if e != entries[i] or read_entry(f, e) != blob:
                raise Failed("%s did not write correctly - run ENGLISH.bat off" % p)
    return len(pics)
