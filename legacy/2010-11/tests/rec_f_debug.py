# -*- coding: utf-8 -*-
"""rec_f_debug.py - records debugger.py's pure parts on synthetic memory into crates/launcher/tests/rec/debug.jsonl,
which crates/launcher/tests/rec_debug.rs replays against launcher::debug: the MSGPARSE recovery plan
(plan_412860_recovery) and the guard's "no plan" note, return-address detection, the log's byte escaping, the text
check, image end, export lookup (the _invoke_watson breakpoint), address naming, and SBWG_moSuppl's layout.
Step 10 piece F: kept until piece G deletes debugger.py (play.py still starts it until then).
Synthetic only: memory is a list of made-up regions, files are made up in a temporary folder; it starts nothing, makes
no shared memory and reads no game file.  Needs Windows' Python (debugger.py loads Windows DLLs at import).
    python legacy\\2010-11\\tests\\rec_f_debug.py"""
import json
import os
import random
import shutil
import struct
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
GAME = tempfile.mkdtemp(prefix="rec_f_debug_")
os.environ["WCCF_GAME"] = GAME                      # read at import: the made-up folder for mosuppl_bytes
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "code", "hardware"))
import debugger as D  # noqa: E402

OUT = os.path.join(REPO, "crates", "launcher", "tests", "rec", "debug.jsonl")
rng = random.Random(20261013)
recs = []


def model(regions):
    """ReadProcessMemory over made-up regions [(address, bytes)]: all n bytes from one region, or None"""
    def rd(addr, n):
        if addr <= 0:
            return None
        for start, data in regions:
            if start <= addr and addr + n <= start + len(data):
                return bytes(data[addr - start:addr - start + n])
        return None
    return rd


def mem(regions):
    """[address, bytes as hex with the zeros at the end left out, length]"""
    return [[a, bytes(d).rstrip(b"\0").hex(), len(d)] for a, d in regions]


def proc(regions, base=0x400000, name="x.exe"):
    p = D.Proc(1, 1, base, name)
    p.rd = model(regions)
    return p


# ---- plan_412860_recovery ---------------------------------------------------------------------------------------------
def frame(esp, slots, size=D.MSGPARSE_STACK_SCAN, fill=None):
    """a stack of `size` bytes at esp, dwords set at {offset: value}"""
    st = bytearray(fill(size) if fill else size)
    for off, v in slots.items():
        struct.pack_into("<I", st, off, v & 0xFFFFFFFF)
    return st


def plan(esp, stack_regions, base=0x400000, ret_off=D.MSGPARSE_RET_OFF):
    out = D.plan_412860_recovery(esp, model(stack_regions), base, base + 0x100000, ret_off)
    recs.append({"fn": "plan", "esp": esp, "base": base, "ret_off": ret_off, "mem": mem(stack_regions), "out": out})


def live(esp, off, ret, d_len=0x40, d_off=0x48, saved=(0x11111111, 0x22222222, 0x33333333, 0x44444444)):
    a = esp + off
    s = {off - 0x10: saved[0], off - 0xC: saved[1], off - 8: saved[2], off - 4: saved[3], off: ret,
         off + 4: a + d_len, off + 8: 0xB0F, off + 0xC: a + d_off}
    return s


ret = 0x400000 + D.MSGPARSE_RET_OFF
for esp, off in [(0x0019F000, 0x10), (0x0019F000, 0x40), (0x0019F004, 0x10C), (0x0012FF00, 0x80)]:
    plan(esp, [(esp, frame(esp, live(esp, off, ret)))])
SPAN = 0x90000
esp = 0x0019F000
for d_len, d_off in [(0, 0x10), (0x10, 0), (SPAN, SPAN), (SPAN + 1, 0x10), (0x10, SPAN + 1), (1, 1), (-4, 8)]:
    plan(esp, [(esp, frame(esp, live(esp, 0x20, ret, d_len, d_off)))])
stale = live(esp, 0x20, ret, -0x100, 0x10)                          # a stale copy first, the live frame above it
stale.update(live(esp, 0x60, ret))
plan(esp, [(esp, frame(esp, stale))])
plan(esp, [(esp, frame(esp, {0x0C: ret, 0x10C + 4: ret}))])        # only where no frame fits
plan(esp, [(esp, frame(esp, live(esp, 0x20, ret))[:0x11C])])        # a short stack: no plan
plan(esp, [])                                                       # unreadable
plan(0xFFFFFF00, [(0xFFFFFF00, frame(0xFFFFFF00, live(0xFFFFFF00, 0x40, ret)))])        # near 4 GB
plan(0xFFFFFFC0, [(0xFFFFFFC0, frame(0xFFFFFFC0, live(0xFFFFFFC0, 0x40, ret)))])        # the frame wraps past 4 GB
plan(esp, [(esp, frame(esp, live(esp, 0x30, 0x10001234)))], base=0x10000000, ret_off=0x1234)
plan(esp, [(esp, frame(esp, live(esp, 0x30, ret)))], base=0x10000000)                   # another base: not it
for _ in range(150):
    esp = rng.randrange(0x10000, 0x7FFF0000) & ~3
    slots = {}
    for _ in range(rng.randrange(0, 4)):
        off = rng.randrange(0x10, 0x110, 4)
        slots.update(live(esp, off, ret, rng.choice([rng.randrange(-8, 0x100, 4), rng.randrange(SPAN - 8, SPAN + 8)]),
                          rng.randrange(0, 0x200, 4)))
    plan(esp, [(esp, frame(esp, {o: v for o, v in slots.items() if 0 <= o <= 0x11C},
                           fill=lambda n: bytes(rng.randrange(256) for _ in range(n))))])

# ---- the guard's note when it finds no plan (try_recover_412860 with a made-up thread) ----------------------------------
def no_plan(esp, stack_regions):
    p = proc(stack_regions, name="control_Release.exe")
    p.threads[7] = (1, 0)

    def get_ctx(_h, ref):
        ref._obj.Esp, ref._obj.Eip = esp, 0x4129C2
        return 1
    saved, D.k.Wow64GetThreadContext = D.k.Wow64GetThreadContext, get_ctx
    try:
        assert p.try_recover_412860(7) is None
    finally:
        D.k.Wow64GetThreadContext = saved
    recs.append({"fn": "no_plan", "esp": esp, "ret": ret, "mem": mem(stack_regions), "out": p.why})


esp = 0x0019F000
no_plan(esp, [])
no_plan(esp, [(esp, frame(esp, {}))])
no_plan(esp, [(esp, frame(esp, {0x0C: ret, 0x20: ret, 0x24: 5, 0x28: 6, 0x2C: 7, 0x40: ret, 0x80: ret}))])
no_plan(esp, [(esp, frame(esp, {0x118: ret, 0x11C: 9}))])
no_plan(esp, [(esp, frame(esp, {0x11C: ret}))])
for _ in range(20):
    no_plan(esp, [(esp, frame(esp, {rng.randrange(0, 0x120, 4): ret for _ in range(rng.randrange(0, 5))},
                              fill=lambda n: bytes(rng.randrange(256) for _ in range(n))))])

# ---- looks_like_return -------------------------------------------------------------------------------------------------
def lr(b):
    va = 0x401000
    recs.append({"fn": "looks_like_return", "bytes": b.hex(), "out": proc([(va - 7, b)]).looks_like_return(va)})


for b in [b"\x00\x00\xe8\x00\x00\x00\x00", b"\x00\xff\x15\x00\x00\x00\x00", b"\x00\xff\x90\x00\x00\x00\x00",
          b"\x00\xff\x97\x00\x00\x00\x00", b"\x00\xff\x98\x00\x00\x00\x00", b"\x00\x00\x00\xff\x54\x00\x00",
          b"\x00\x00\x00\x00\xff\x50\x00", b"\x00\x00\x00\x00\xff\x57\x00", b"\x00\x00\x00\x00\xff\x58\x00",
          b"\x00\x00\x00\x00\x00\xff\xd0", b"\x00\x00\x00\x00\x00\xff\xd7", b"\x00\x00\x00\x00\x00\xff\x10",
          b"\x00\x00\x00\x00\x00\xff\x17", b"\x00\x00\x00\x00\x00\xff\x18", b"\x00" * 7, b"\xff" * 7]:
    lr(b)
for _ in range(400):
    b = bytearray(rng.randrange(256) for _ in range(7))
    if rng.random() < 0.5:
        b[rng.choice([1, 3, 4, 5])] = 0xFF
    lr(bytes(b))

# ---- esc -------------------------------------------------------------------------------------------------------------
for b in [b"", b"plain text", b"back\\slash", b"\x00\x1f\x20\x7e\x7f\x80\xff", bytes(range(256))]:
    recs.append({"fn": "esc", "bytes": b.hex(), "out": D.esc(b)})
for _ in range(50):
    b = bytes(rng.randrange(256) for _ in range(rng.randrange(0, 40)))
    recs.append({"fn": "esc", "bytes": b.hex(), "out": D.esc(b)})

# ---- text_at ---------------------------------------------------------------------------------------------------------
def text(va, regions):
    """regions run to the end of their page, as memory does"""
    regions = [(a, bytes(d) + bytes(-(a + len(d)) & 0xFFF)) for a, d in regions]
    out = proc(regions).text_at(va)
    recs.append({"fn": "text_at", "va": va, "mem": mem(regions), "out": None if out is None else out.hex()})


VA = 0x00500000
for s in [b"hello\0", b"hi\0", b"abc\0", b"tab\there\r\n\0", b"bell\x07\0", b"%d points %s\0",
          "日本語のテキスト".encode("cp932") + b"\0", "ｶﾀｶﾅ".encode("cp932") + b"\0", b"\x82\xa0\x82\0",
          b"bad lead \x81\x20 x\0", b"\x80\xa0\xfd\xfe\xff\0", b"\xf0\x40\xf9\xfc user area\0", b"\x85\x40 gap\0",
          b"x" * 250, b"no end at all"]:
    text(VA, [(VA, s)])
text(0xFFFF, [(0xFFFF, b"low address\0")])
text(0x10000, [(0x10000, b"at 64K\0")])
page = 0x00600000
text(page - 5, [(page - 5, b"cross")])                                   # the next page unreadable
text(page - 5, [(page - 5, b"cross"), (page, b"ing\0")])                 # both readable, separate regions
text(page - 2, [(page - 2, b"\x82"), (page, b"\xa0end\0")])              # a character across the page edge
text(VA, [])
for _ in range(80):
    s = bytes(rng.randrange(0, 0x20) if rng.random() < 0.03 else rng.choice([rng.randrange(0x20, 0x7F),
              rng.randrange(0x80, 0x100)]) for _ in range(rng.randrange(0, 20))) + b"\0"
    text(VA, [(VA, s)])

# ---- image_end, export_va ----------------------------------------------------------------------------------------------
def pe(size_of_image, exports=None, lfa=0x80):
    """a made-up 32-bit image header, and its export directory at 0x1000 when `exports` ({name: rva}) is given"""
    hdr = bytearray(0x400)
    hdr[:2] = b"MZ"
    struct.pack_into("<I", hdr, 0x3C, lfa)
    if lfa + 0x54 <= len(hdr):
        struct.pack_into("<I", hdr, lfa + 0x50, size_of_image)
    regions = [(0x10000000, hdr)]
    if exports is not None and lfa + 0x80 <= len(hdr):
        struct.pack_into("<I", hdr, lfa + 0x78, 0x1000)
        names = sorted(exports)
        ed = bytearray(0xA00)
        n = len(names)
        funcs, nptr, ords, strs = 0x1100, 0x1400, 0x1700, 0x1800
        struct.pack_into("<IIII", ed, 0x18, n, funcs, nptr, ords)
        pos = strs
        for i, name in enumerate(names):
            struct.pack_into("<I", ed, funcs - 0x1000 + 4 * (n - 1 - i), exports[name])   # ordinals reversed
            struct.pack_into("<H", ed, ords - 0x1000 + 2 * i, n - 1 - i)
            struct.pack_into("<I", ed, nptr - 0x1000 + 4 * i, pos)
            ed[pos - 0x1000:pos - 0x1000 + len(name)] = name.encode()
            pos += len(name) + 1
        regions.append((0x10001000, ed))
    return regions


for regions in [pe(0x5000), pe(0x123000), pe(0x5000, lfa=0x3B0), pe(0x5000, lfa=0x3AC), [], [(0x10000000, b"XX" + bytes(0x3FE))]]:
    p = proc(regions)
    recs.append({"fn": "image_end", "base": 0x10000000, "mem": mem(regions), "out": p.image_end(0x10000000)})
NAMES = {"_invoke_watson": 0x2468, "_invalid_parameter": 0x1357, "abort": 0x100, "malloc": 0x200, "printf": 0x300,
         "_invoke_watsonX": 0x999, "_invoke": 0x888}
for exports in [NAMES, {"_invoke_watson": 0x42}, {"other": 1}, {}, {k: v for k, v in NAMES.items() if k != "_invoke_watson"},
                {"n%03d" % i: i * 16 for i in range(60)} | {"_invoke_watson": 0x5555}]:
    regions = pe(0x50000, exports)
    for want in ["_invoke_watson", "abort", "zzz", "n050"]:
        out = proc(regions).export_va(0x10000000, want)
        recs.append({"fn": "export_va", "base": 0x10000000, "want": want, "mem": mem(regions), "out": out})
regions = pe(0x50000, {"_invoke_watson": 0x42}, lfa=0x3A0)                          # header too short for the table
recs.append({"fn": "export_va", "base": 0x10000000, "want": "_invoke_watson", "mem": mem(regions),
             "out": proc(regions).export_va(0x10000000, "_invoke_watson")})

# ---- name_of ---------------------------------------------------------------------------------------------------------
idx = ([0x401000, 0x401100, 0x402000], ["FUN_00401000", "Cls::method", "FUN_00402000"], [0x80, 0x100, 0])
dlls = [(0x10000000, 0x10050000, "msvcr90.dll"), (0x77000000, 0x77100000, "ntdll.dll")]
for index in [idx, ([], [], [])]:
    p = D.Proc(1, 1, 0x400000, "x.exe")
    p.end, p.index, p.dlls = 0x500000, index, dlls
    for va in [0x400000, 0x400FFF, 0x401000, 0x40107F, 0x401080, 0x401100, 0x4011FF, 0x401200, 0x402000, 0x4FFFFF,
               0x500000, 0x10000000, 0x1004FFFF, 0x10050000, 0x77000010, 0, 0xFFFFFFFF]:
        recs.append({"fn": "name_of", "index": index, "base": 0x400000, "end": 0x500000, "name": "x.exe", "dlls": dlls,
                     "va": va, "out": p.name_of(va)})

# ---- mosuppl_bytes ---------------------------------------------------------------------------------------------------
os.makedirs(os.path.join(GAME, "AI", "motion_data"))
for sizes in [(0, 0, 0, 0), (1, 2, 3, 4), (4, 8, 5, 7), (13, 0, 1, 1024)]:
    files = {}
    for name, n in zip(D.MOSUPPL, sizes):
        files[name] = bytes(rng.randrange(1, 256) for _ in range(n))
        with open(os.path.join(GAME, "AI", "motion_data", name), "wb") as f:
            f.write(files[name])
    recs.append({"fn": "mosuppl", "files": {k: v.hex() for k, v in files.items()}, "out": D.mosuppl_bytes().hex()})

shutil.rmtree(GAME, ignore_errors=True)
with open(OUT, "w", encoding="utf-8", newline="\n") as f:
    for r in recs:
        f.write(json.dumps(r, sort_keys=True) + "\n")
print("%d records -> %s" % (len(recs), OUT))
