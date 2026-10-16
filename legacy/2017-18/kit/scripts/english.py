# -*- coding: utf-8 -*-
r"""english.py - WCCF 2017-18 (SDDY 2.00.00) in English.  Built on this PC from YOUR game files, every line checked,
then swapped in with the original files kept; "off" puts the originals back.  Nothing of the original ships with the
kit: english\ holds only English text and fingerprints (SHA-256) of the original files.  The 2017-18 branch of the
2010-11 kit's english.py; Launcher.exe runs it at a start when the game text was changed (data\panel.txt english=).

    python english.py on       (ENGLISH.bat)       build, check, then put the English files in place
    python english.py off      (ENGLISH.bat off)   the original Japanese files back
    python english.py on it    (ENGLISH.bat on it) the same in Italian: italian\screen_text.tsv first, the English
                                                   wherever a line has none or its Italian does not fit
    python english.py check    (ENGLISH.bat check) say what "on" would do; changes nothing
    python english.py selftest GAME [SCRATCH [it]] round trip + known lines + on/off on copies; never touches GAME
    pictures=SPEC after on/check (or WCCF_PICTURES=SPEC): which pictures get their writing redrawn: all (default),
                 none, or names separated by commas (e.g. pictures=1112_SCM_Parameter); see pictures1718.py

What changes, in the game folder (seat 1 reads the same files through setup's folder links data\ and prog_data\):
  data\string\string_list.bin + .hf          the screen text as the SERVER (control_Release.exe) reads it
  prog_data\enumdata\StringListTableRecord.bin   the same screen text as the CLIENTS (seat 1 and the projector) read it:
                                             2017-18's client never opens string_list.*; it loads this table
  prog_data\player_data\player_data.bin      player names in the original Latin spelling (accents dropped) + skill
                                             names (the launcher maps this file for every program at start)
  prog_data\cpu_team\cpu_0..598.dat          the CPU teams' names
  data\wccf_data.xaf                         the writing inside pictures (english\pictures.json): each redrawn
                                             picture is ADDED at the archive's end and its entry pointed at it;
                                             "off" cuts it off again, byte for byte (pictures1718.py)
No program file changes: text inside the exes is left to the seat patch list (tools\patches1718.py).
The first "on" COPIES the original files to data\english_backup\ - only files whose fingerprint shows they are the
original 2.00.00 files; later runs build from that backup again.  Each game file is replaced in ONE step ("off" too),
so a switch stopped half way leaves every file in place, never a missing one.  Kept in Japanese on purpose: country
and prefecture names (the server builds its weather table keyed by their TEXT, control FUN_00422B50 from the area
table at VA 0x0067DEE8 - English ones crashed the 2010-11 server), league names and the network-ranking areas.
Where the English comes from: english\screen_text.tsv (this kit's translation; lines of the 2010-11 kit carried over
where 2017-18 has the identical Japanese), the original English that the game files already hold (the second language
column), and english\original_rstring.tsv (the original European English of the older WCCF, carried by identical
Japanese).  Each line takes the first of these that keeps the codes of the game's Japanese line; "on it" puts
italian\screen_text.tsv (key + Italian) before them all, and data\english_backup\language.txt says which is on.
Exit 0 done; 1 refused or a check failed (nothing changed); 2 no game set up (set it up first).
"""
import bisect
import collections
import csv
import hashlib
import json
import os
import re
import shutil
import struct
import sys
import unicodedata

import pictures1718 as P

HERE = os.path.dirname(os.path.abspath(__file__))
KIT = os.path.dirname(HERE)                              # kit\english and kit\italian beside kit\scripts
LANGS = {"en": "English", "it": "Italian"}               # "on it": Italian, with the English wherever a line has none
BACK = MANIFEST = None                                    # kit\data\english_backup (set in main/selftest)
XAF_STATE = "wccf_data.xaf.json"                          # in BACK: the archive's way back while pictures are on
BIN, HF = os.path.join("data", "string", "string_list.bin"), os.path.join("data", "string", "string_list.hf")
ENUM = os.path.join("prog_data", "enumdata", "StringListTableRecord.bin")
PLAYERS = os.path.join("prog_data", "player_data", "player_data.bin")
TEAMS = os.path.join("prog_data", "cpu_team")
ORIGINAL = {BIN: "EAB584BA8F96698F0FC438DD8C46282B852E5B7C72F885110FEB189FD004CE43",  # 2.00.00 (DVR5059A, 2018-03-15)
        HF: "23D63FE4468D7F820CA8CF4424F1995C4FEA02C90827015DD1FB936BADE61D43",
        ENUM: "574E40B205632DC23E28852CBAD87DF45711EF05A46513F8FADF4E237BB93495",
        PLAYERS: "1F8D79BC980D9E67B3E2F6A975F1A669D4CE3C7EBB413DA09B4A8D5F5DC6FFFA"}
# families the game may use as DATA: kept Japanese unless switched on below (the 2010-11 kit's list, checked again
# for 2017-18: control FUN_00422B50 keys its weather map by the area + country/prefecture TEXT; cities by key)
DATA_FAMILIES = ("MAKE_", "NET_RANK", "GAM_OROGINAL", "GAM_REAL", "GAM_STA", "SYS_TEAM", "INF_SUPPORTER",
                 "SYS_COACH", "PRO_PRO", "INF_FRE")
SWITCHED_ON = ("SYS_TEAM", "PRO_PRO", "GAM_OROGINAL", "GAM_REAL", "GAM_STA", "INF_FRE", "SYS_COACH", "INF_SUPPORTER",
               "MAKE_city", "MAKE_area")
# every conversion the C runtime takes, so an English line keeps exactly the codes of the game's Japanese line
FMT = re.compile(r"%[-+ 0#]*(?:\*|[0-9]+)?(?:\.(?:\*|[0-9]+))?(?:hh|h|ll|l|L|I32|I64|I|w)?[cCdiouxXeEfgGaAnpsSZ%]")
CODES = re.compile(r"\$[A-Za-z](?:\[[^\]]*\])?")          # $c[RRGGBBAA] .. $c colour, $p[..] $p, $t[..] $t
REC = 0x620                                              # player_data.bin: one player (2010-11: 0x328)
SHORT_KANA, SHORT_LATIN, SKILL_EN, SKILL_JP, FIELD = 0x330, 0x370, 0x45F, 0x49F, 0x40
NAME_AT, NAME_END = 0x144, 0x17C                         # cpu_N.dat: the team name field (as 2010-11)
FOLD = {"Ø": "O", "ø": "o", "Æ": "AE", "æ": "ae", "ß": "SS", "Ł": "L", "ł": "l", "Đ": "D", "đ": "d", "Œ": "OE",
        "œ": "oe", "ı": "i", "Þ": "TH", "ð": "d"}


class Failed(Exception):
    pass


def sha(data):
    return hashlib.sha256(data).hexdigest().upper()


def sheet(name, folder="english"):
    """a table in the kit's english or italian folder"""
    with open(os.path.join(KIT, folder, name), encoding="utf-8", newline="") as fh:
        return list(csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE))


def original_file(game, rel):
    """the original: the backup once English has been on, else the game's own file"""
    b = os.path.join(BACK, rel)
    with open(b if os.path.isfile(b) else os.path.join(game, rel), "rb") as f:
        return f.read()


# ---------------------------------------------------------------- the two screen-text files
def read_enum(d):
    """StringListTableRecord.bin: u32 count, then per line u32 id + 8 x (u32 length, cp932 bytes, no end byte):
    key, the original comment, japanese, english, italian, spanish, french, german"""
    n, = struct.unpack_from("<I", d, 0)
    p, recs = 4, []
    for _ in range(n):
        rid, = struct.unpack_from("<I", d, p)
        p += 4
        f = []
        for _ in range(8):
            ln, = struct.unpack_from("<I", d, p)
            f.append(d[p + 4:p + 4 + ln])
            p += 4 + ln
        recs.append((rid, f))
    if p != len(d):
        raise Failed("StringListTableRecord.bin has %d bytes after its last line" % (len(d) - p))
    return recs


def write_enum(recs):
    out = [struct.pack("<I", len(recs))]
    for rid, f in recs:
        out.append(struct.pack("<I", rid) + b"".join(struct.pack("<I", len(x)) + x for x in f))
    return b"".join(out)


def read_strings(raw, hf_raw):
    body = raw[8:]
    keys = {}
    for i, line in enumerate(hf_raw.splitlines(keepends=True)[1:], 1):
        p = line.split()
        keys[p[0].decode("ascii")] = (i, [int(x) for x in p[1:]])
    return body, keys


def at(body, p):
    return body[p:body.find(b"\0", p)]


def blocked(k):
    f = "_".join(k.split("_")[:2])
    return f.startswith(DATA_FAMILIES) and not any(f.startswith(e) for e in SWITCHED_ON)


def kit_lines(lang="en"):
    """{key: [(text, source), ...]} best first: for "it" the kit's Italian, then the kit's English behind it as that
    key's fallback; a key the Italian sheet lacks is English"""
    lines = {}
    for r in sheet("screen_text.tsv"):
        if (r.get("english") or "").strip() and r["key"] not in lines:
            lines[r["key"]] = [(r["english"].strip("\r"), "kit")]
    if lang == "it":
        for r in sheet("screen_text.tsv", "italian"):
            if (r.get("italiano") or "").strip():
                lines.setdefault(r["key"], []).insert(0, (r["italiano"].strip("\r"), "kit Italian"))
    return lines


def screen_lines(body, keys, lang="en"):
    """each Japanese line's choices, best first: the kit's (kit_lines), then the original second column, then the
    original European English"""
    choices = kit_lines(lang)
    rstring = {r["key"]: r["english"] for r in sheet("original_rstring.tsv")}
    for k, (_, nums) in keys.items():
        if not any(c >= 0x80 for c in at(body, nums[0])):
            continue
        s2 = at(body, nums[1])
        if s2 and not any(c >= 0x80 for c in s2):
            choices.setdefault(k, []).append((s2.decode("ascii"), "original second column"))
        elif k in rstring:
            choices.setdefault(k, []).append((rstring[k], "original European English"))
    return choices


def encoded(jp, text):
    """the line as the game's bytes and None, or None and what is wrong with it: a line keeps exactly the printf and
    $ codes of the game's Japanese line"""
    if text.strip() == "<empty>":
        if FMT.findall(jp) or CODES.findall(jp):
            return None, "<empty> would drop the Japanese line's codes"
        return b"", None
    en = text.replace("\\n", "\n")
    if FMT.findall(jp) != FMT.findall(en):
        return None, "codes %s, the game's line has %s" % (FMT.findall(en), FMT.findall(jp))
    if FMT.sub("", en).count("%") > FMT.sub("", jp).count("%"):
        return None, "a % sign that is no printf code - the game would show junk or stop"
    if collections.Counter(CODES.findall(jp)) != collections.Counter(CODES.findall(en)):
        return None, "$ codes %s, the game's line has %s" % (CODES.findall(en), CODES.findall(jp))
    try:
        b = en.encode("cp932")
    except UnicodeEncodeError:
        return None, "a letter the game's font cannot show"
    if b"\0" in b:
        return None, "holds an end byte"
    return b, None


def encode_lines(body, keys, choices):
    """{key: cp932 bytes}, {key: source} for every line with a choice that fits, the first that does; and the kit's
    lines that did not fit (they give way to the next choice, never a line with other codes than the game's)"""
    enc, used, misfit = {}, {}, []
    for k, options in sorted(choices.items()):
        if k not in keys:
            misfit += ["%s: no such line in the game" % k for _, src in options if src.startswith("kit")]
            continue
        if blocked(k):
            continue
        jp = at(body, keys[k][1][0]).decode("cp932", "replace")
        for text, src in options:
            b, problem = encoded(jp, text)
            if not problem:
                enc[k], used[k] = b, src
                break
            if src.startswith("kit"):
                misfit.append("%s (%s): %s" % (k, src, problem))
    return enc, used, misfit


def pack_strings(raw, hf_raw, enc):
    """string_list.bin/.hf with slot 0 (Japanese) of each key in enc replaced; same size, read back"""
    tag, body = raw[:4], raw[8:]
    hf_lines = hf_raw.splitlines(keepends=True)
    _, keys = read_strings(raw, hf_raw)
    # free space: a translated line's old Japanese, if nothing else points at it or inside it; then the zero padding
    refs = collections.defaultdict(set)
    for k, (_, nums) in keys.items():
        for s, n in enumerate(nums):
            refs[n].add((k, s))
    ref_sorted = sorted(refs)
    own, free = {}, []
    for k in enc:
        p = keys[k][1][0]
        end = p + len(at(body, p)) + 1
        lo, hi = bisect.bisect_left(ref_sorted, p), bisect.bisect_left(ref_sorted, end)
        if refs[p] == {(k, 0)} and hi - lo == 1:
            own[k] = (p, end)
    pad_at = max(len(body.rstrip(b"\0")) + 1, ref_sorted[-1] + 1) + 16
    new_body = bytearray(body)
    for k, (p, end) in own.items():
        new_body[p:end] = bytes(end - p)
    pos, spill = {}, []
    for k, b in sorted(enc.items(), key=lambda kv: (-len(kv[1]), kv[0])):
        if k in own and len(b) + 1 <= own[k][1] - own[k][0]:
            p = own[k][0]
            new_body[p:p + len(b)] = b
            pos[k] = p
            if own[k][1] - (p + len(b) + 1) > 1:
                free.append((p + len(b) + 1, own[k][1]))
        else:
            spill.append(k)
            if k in own:
                free.append(own[k])
    free.append((pad_at, len(body) - 1))
    for k in spill:
        b = enc[k]
        for i, (s, e) in enumerate(free):
            if e - s >= len(b) + 1:
                new_body[s:s + len(b)] = b
                pos[k] = s
                free[i] = (s + len(b) + 1, e)
                break
        else:
            raise Failed("no room in string_list.bin for %s" % k)
    new_bin = tag + len(new_body).to_bytes(4, "little") + bytes(new_body)
    new_hf = list(hf_lines)
    for k, p in pos.items():
        i = keys[k][0]
        m = re.match(rb"(\S+\s+)(\d+)", hf_lines[i])
        new_hf[i] = m.group(1) + str(p).encode("ascii") + hf_lines[i][m.end():]
    nb, wrong = bytes(new_body), 0                     # read back every slot of every key
    for line in new_hf[1:]:
        p = line.split()
        k, nums = p[0].decode("ascii"), [int(x) for x in p[1:]]
        old = keys[k][1]
        if nums[1:] != old[1:]:
            wrong += 1
        for s, n in enumerate(nums):
            if at(nb, n) != (enc[k] if (s == 0 and k in pos) else at(body, old[s])):
                wrong += 1
    if wrong or len(new_bin) != len(raw) or new_hf[0] != hf_lines[0]:
        raise Failed("the built screen text does not read back as intended (%d places) - nothing changed" % wrong)
    return new_bin, b"".join(new_hf), pos


def pack_enum(enum_raw, body, keys, enc):
    """StringListTableRecord.bin with the Japanese column of each key in enc replaced - only where its Japanese is the
    server file's (the codes were checked against that); read back"""
    recs = read_enum(enum_raw)
    done = {}
    for rid, f in recs:
        k = f[0].decode("ascii")
        if k in enc and k in keys and f[2] == at(body, keys[k][1][0]):
            f[2] = enc[k]
            done[k] = enc[k]
    new = write_enum(recs)
    back = {f[0].decode("ascii"): f for _, f in read_enum(new)}
    old = read_enum(enum_raw)
    if len(back) != len(old) or any(back[f[0].decode("ascii")][:2] + back[f[0].decode("ascii")][3:] != f[:2] + f[3:]
                                    or back[f[0].decode("ascii")][2] != done.get(f[0].decode("ascii"), f[2])
                                    for _, f in old):
        raise Failed("the built client screen text does not read back as intended - nothing changed")
    return new, done


def build_strings(game, lang="en"):
    raw, hf_raw, enum_raw = original_file(game, BIN), original_file(game, HF), original_file(game, ENUM)
    for rel, d in ((BIN, raw), (HF, hf_raw), (ENUM, enum_raw)):
        if sha(d) != ORIGINAL[rel]:
            raise Failed("%s is not the original 2.00.00 file - the English is made for that only" % rel)
    body, keys = read_strings(raw, hf_raw)
    enc, used, misfit = encode_lines(body, keys, screen_lines(body, keys, lang))
    new_bin, new_hf, pos = pack_strings(raw, hf_raw, enc)
    new_enum, done = pack_enum(enum_raw, body, keys, enc)
    by = collections.Counter(used[k] for k in pos)
    note = "%d lines in %s (%s); %d in the clients' table" % (
        len(pos), LANGS[lang], ", ".join("%s %d" % kv for kv in sorted(by.items())), len(done))
    if misfit:
        note += "; %d of the kit's lines do not fit this game's line and gave way to the next choice (%s)" % (
            len(misfit), misfit[0])
    return {BIN: new_bin, HF: new_hf, ENUM: new_enum}, note


# ---------------------------------------------------------------- player and team names
def fold(s):
    s = "".join(FOLD.get(c, c) for c in s)
    s = "".join(c for c in unicodedata.normalize("NFKD", s) if not unicodedata.combining(c))
    return "".join(c if 32 <= ord(c) < 127 else "?" for c in s)


def put_field(field, value, size):
    """value + end byte, then the field's own fill (whatever followed the original end byte)"""
    end = field.find(b"\0")
    fill = field[end + 1:end + 2] if 0 <= end < size - 1 else b"\0"
    out = value[:size - 1] + b"\0"
    return out + fill * (size - len(out))


def build_names(game):
    pd = bytearray(original_file(game, PLAYERS))
    if sha(pd) != ORIGINAL[PLAYERS] or len(pd) % REC:
        raise Failed("prog_data\\player_data\\player_data.bin is not the original 2.00.00 file")
    for i in range(len(pd) // REC):
        r = i * REC
        kana = bytes(pd[r + SHORT_KANA:r + SHORT_KANA + FIELD]).split(b"\0")[0].decode("cp932", "replace")
        latin = bytes(pd[r + SHORT_LATIN:r + SHORT_LATIN + FIELD]).decode("utf-16-le", "replace").split("\0")[0].strip()
        if not latin:
            continue
        name = fold(latin)
        m = re.match(r"^([Ａ-Ｚ])．", kana)                # a katakana initial (Ｍ．ディアッラ) kept: M.DIARRA
        if m:
            ini = chr(ord(m.group(1)) - 0xFEE0) + "."
            if not name.startswith(ini):
                name = ini + name
        pd[r + SHORT_KANA:r + SHORT_KANA + FIELD] = put_field(bytes(pd[r + SHORT_KANA:r + SHORT_KANA + FIELD]),
                                                              name.encode("ascii"), FIELD)
        sk = bytes(pd[r + SKILL_EN:r + SKILL_EN + FIELD]).split(b"\0")[0]
        if sk and all(32 <= c < 127 for c in sk) and b"%" not in sk:
            pd[r + SKILL_JP:r + SKILL_JP + FIELD] = put_field(bytes(pd[r + SKILL_JP:r + SKILL_JP + FIELD]), sk, FIELD)
    files, skipped = {PLAYERS: bytes(pd)}, []
    for row in sheet("cpu_names.tsv"):
        rel = os.path.join(TEAMS, row["file"])
        d = bytearray(original_file(game, rel))
        en = row["english"]
        if sha(d) != row["original_sha256"]:
            skipped.append(row["file"])
            continue
        if len(en) > NAME_END - NAME_AT - 1 or "%" in en or not all(32 <= ord(c) < 127 for c in en):
            raise Failed("the kit's team name for %s does not fit" % row["file"])
        d[NAME_AT:NAME_END] = put_field(bytes(d[NAME_AT:NAME_END]), en.encode("ascii"), NAME_END - NAME_AT)
        files[rel] = bytes(d)
    note = "%d players, %d CPU teams" % (len(pd) // REC, len(files) - 1)
    if skipped:
        note += "; %d team files are not the original 2.00.00 ones and stay as they are (%s...)" % (
            len(skipped), skipped[0])
    return files, note


def build_all(game, lang="en"):
    files, notes = {}, []
    for fn in (build_strings, build_names):
        f, n = fn(game, lang) if fn is build_strings else fn(game)
        files.update(f)
        notes.append(n)
    return files, notes


# ---------------------------------------------------------------- swapping files in and out
def original_of(h):
    """the original file's fingerprint in a backup-list entry (beside "english"): research copies of this script wrote
    it under another key, so the one key that is not "english" is read whatever it is called"""
    return h["original"] if "original" in h else next(v for k, v in h.items() if k != "english")


def load_manifest():
    try:
        with open(MANIFEST, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def save_manifest(m):
    os.makedirs(BACK, exist_ok=True)
    with open(MANIFEST + ".tmp", "w", encoding="utf-8") as f:
        json.dump(m, f, indent=1)
    os.replace(MANIFEST + ".tmp", MANIFEST)


def put(path, data):
    with open(path + ".tmp", "wb") as f:
        f.write(data)
    os.replace(path + ".tmp", path)


def backup_copy(src, dst):
    """the original file copied into the backup and read back; it stays in place until its replacement lands in one
    step"""
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    with open(src, "rb") as f:
        data = f.read()
    put(dst, data)
    with open(dst, "rb") as f:
        if f.read() != data:
            raise Failed("could not copy %s to the backup" % src)


def original_hash(rel):
    if rel in ORIGINAL:
        return ORIGINAL[rel]
    for row in sheet("cpu_names.tsv"):
        if os.path.join(TEAMS, row["file"]) == rel:
            return row["original_sha256"]
    return None


def turn_on(game, files, lang="en"):
    man = load_manifest()

    def unsaved(rel):                                    # not listed, or its backup gone (an "off" stopped half way)
        return rel not in man or not os.path.isfile(os.path.join(BACK, rel))
    for rel in files:                                    # first: every file to be backed up must be the original
        if unsaved(rel):
            with open(os.path.join(game, rel), "rb") as f:
                if sha(f.read()) != original_hash(rel):
                    raise Failed("%s is neither the original nor backed up - refusing to replace it" % rel)
    for rel, data in files.items():
        tgt = os.path.join(game, rel)
        if unsaved(rel):
            backup_copy(tgt, os.path.join(BACK, rel))
            man[rel] = {"original": original_hash(rel)}
            save_manifest(man)                           # after each backup: an interruption never loses one
        put(tgt, data)                                   # one step: the game's file is never missing
        with open(tgt, "rb") as f:
            if sha(f.read()) != sha(data):
                raise Failed("%s did not write correctly - run ENGLISH.bat off" % rel)
        man[rel]["english"] = sha(data)
    save_manifest(man)
    put(os.path.join(BACK, "language.txt"), (lang + "\n").encode("ascii"))   # Launcher.exe reads it (game_language)


def turn_off(game):
    man = load_manifest()
    lost = []
    for rel, h in man.items():
        bak, tgt = os.path.join(BACK, rel), os.path.join(game, rel)
        if os.path.isfile(bak):
            with open(bak, "rb") as f:
                data = f.read()
            if sha(data) != original_of(h):
                raise Failed("the backup of %s is not the original file - left as it is" % rel)
            put(tgt, data)                               # one step: the original file back, never a moment without one
            os.remove(bak)
        elif os.path.isfile(tgt):                        # no backup: put back already, or lost
            with open(tgt, "rb") as f:
                if sha(f.read()) != original_of(h):
                    lost.append(rel)
    if lost:
        raise Failed("%d file(s) are not the original and have no backup of the original file, so they stay as they "
                     "are (%s%s)" % (len(lost), lost[0], ", ..." if len(lost) > 1 else ""))
    for f in (MANIFEST, os.path.join(BACK, "language.txt")):
        if os.path.isfile(f):
            os.remove(f)
    for dp, dn, fn in os.walk(BACK, topdown=False):
        if not os.listdir(dp):
            os.rmdir(dp)
    return len(man)


# ---------------------------------------------------------------- self-check (on copies only)
def selftest(game, scratch, lang="en"):
    """round trip, known lines, then on / off on a copy of the changed files under SCRATCH; GAME is only read"""
    global BACK, MANIFEST
    raw, hf_raw = open(os.path.join(game, BIN), "rb").read(), open(os.path.join(game, HF), "rb").read()
    enum_raw = open(os.path.join(game, ENUM), "rb").read()
    body, keys = read_strings(raw, hf_raw)
    assert pack_strings(raw, hf_raw, {})[:2] == (raw, hf_raw), "string_list round trip"
    assert write_enum(read_enum(enum_raw)) == enum_raw, "StringListTableRecord round trip"
    assert pack_enum(enum_raw, body, keys, {})[0] == enum_raw, "StringListTableRecord rebuild"
    copy = os.path.join(scratch, "game")
    rels = [BIN, HF, ENUM, PLAYERS] + [os.path.join(TEAMS, r["file"]) for r in sheet("cpu_names.tsv")]
    for rel in rels:
        os.makedirs(os.path.dirname(os.path.join(copy, rel)), exist_ok=True)
        shutil.copyfile(os.path.join(game, rel), os.path.join(copy, rel))
    before = {rel: sha(open(os.path.join(copy, rel), "rb").read()) for rel in rels}
    BACK = os.path.join(scratch, "english_backup")
    MANIFEST = os.path.join(BACK, "manifest.json")
    shutil.rmtree(BACK, ignore_errors=True)
    files, notes = build_all(copy, lang)
    turn_on(copy, files, lang)
    turn_on(copy, build_all(copy, lang)[0], lang)        # a second "on" builds from the backup, same result
    nb, nk = read_strings(open(os.path.join(copy, BIN), "rb").read(), open(os.path.join(copy, HF), "rb").read())
    en = {f[0].decode(): f[2] for _, f in read_enum(open(os.path.join(copy, ENUM), "rb").read())}
    orig = {f[0].decode(): f[2] for _, f in read_enum(enum_raw)}
    for k in ("MAKE_country_000", "MAKE_prefecture_000"):    # data families: Japanese in both files
        assert at(nb, nk[k][1][0]) == at(body, keys[k][1][0]) == en[k], k
    for k in ("CLM_MANAGER_result_0002", "MAKE_city_000"):   # known lines: the kit's language in both files
        assert at(nb, nk[k][1][0]).isascii() and en[k] == at(nb, nk[k][1][0]), k
    for k in nk:                                          # every line: the clients' table says what the server's says
        if k in en and orig[k] == at(body, keys[k][1][0]):
            assert en[k] == at(nb, nk[k][1][0]), "client and server differ at " + k
    pd = open(os.path.join(copy, PLAYERS), "rb").read()
    assert pd[500 * REC + SHORT_KANA:500 * REC + SHORT_KANA + 7] == b"VANOLI\0", "player 500's short name"
    pnote = selftest_pictures(game, copy, lang)
    n = turn_off(copy)
    after = {rel: sha(open(os.path.join(copy, rel), "rb").read()) for rel in rels}
    assert after == before, "off did not restore the original files byte for byte"
    assert not os.path.exists(BACK), "backup folder left behind"
    shutil.rmtree(copy)
    print("selftest ok (%s): round trips identical; %s; on, on again, off -> %d files back byte-identical; %s"
          % (LANGS[lang], "; ".join(notes), n, pnote))
    return 0


def file_sha(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 24), b""):
            h.update(b)
    return h.hexdigest().upper()


def selftest_pictures(game, copy, lang):
    """the pictures on a copy of the archive: on, on again (the same file), off (the original archive byte for byte)"""
    src, xaf = os.path.join(game, P.XAF), os.path.join(copy, P.XAF)
    os.makedirs(os.path.dirname(xaf), exist_ok=True)
    shutil.copyfile(src, xaf)
    before, state = file_sha(xaf), os.path.join(BACK, XAF_STATE)
    assert before == file_sha(src), "the archive copy"
    pics, notes = P.build(copy, KIT, lang, os.environ.get("WCCF_PICTURES"), state)
    P.apply(copy, pics, state)
    on = file_sha(xaf)
    P.apply(copy, P.build(copy, KIT, lang, os.environ.get("WCCF_PICTURES"), state)[0], state)
    assert file_sha(xaf) == on, "pictures on twice: not the same archive"
    grown = os.path.getsize(xaf) - os.path.getsize(src)
    assert P.restore(copy, state) == len(pics), "pictures off"
    assert file_sha(xaf) == before, "pictures off did not give back the original archive byte for byte"
    os.remove(xaf)
    return "%s; on, on again (same archive, %d bytes added), off -> wccf_data.xaf byte-identical" % (
        "; ".join(notes), grown)


def main(argv):
    global BACK, MANIFEST
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass
    spec = os.environ.get("WCCF_PICTURES")
    for a in [a for a in argv if a.lower().startswith("pictures=")]:
        spec = a.split("=", 1)[1]
        argv.remove(a)
    mode = (argv[0].lower() if argv else "on")
    if mode == "selftest" and len(argv) >= 2:
        scratch = argv[2] if len(argv) > 2 else os.path.join(HERE, "_selftest")
        return selftest(argv[1], scratch, argv[3] if len(argv) > 3 else "en")
    lang = argv[1].lower() if len(argv) > 1 else "en"
    if mode not in ("on", "off", "check") or lang not in LANGS:
        print(__doc__)
        return 1
    import kit_common as K
    BACK = os.path.join(K.DATA, "english_backup")
    MANIFEST = os.path.join(BACK, "manifest.json")
    game = K.find_game(K.load_settings().get("game"))
    if not game:
        print("The game is not set up yet - set it up first.")
        return 2
    print("Forza-WCCF 2017-18 - %s %s - game: %s" % (LANGS[lang], mode, game))
    try:
        if K.game_processes(game):
            if mode != "check":
                raise Failed("the game is running - close it first, then run this again")
            print("  NOTE  the game is running: \"on\" and \"off\" will refuse until it is closed")
        state = os.path.join(BACK, XAF_STATE)
        if mode == "off":
            if not load_manifest() and not os.path.isfile(state):
                print("  The game's text is the original Japanese already - nothing to do.")
                return 0
            p = P.restore(game, state)
            n = turn_off(game)
            print("  ok    the original Japanese files are back (%d files, %d pictures)." % (n, p))
            return 0
        print("  building from your game files ...", flush=True)
        files, notes = build_all(game, lang)
        pics, pnotes = P.build(game, KIT, lang, spec, state)
        for n in notes + pnotes:
            print("  ok    " + n)
        if mode == "check":
            man = load_manifest()
            print("  check only - nothing changed.  \"on\" would replace %d files (%d of them backed up first) and "
                  "redraw %d pictures." % (len(files), sum(1 for r in files if r not in man), len(pics)))
            return 0
        turn_on(game, files, lang)
        P.apply(game, pics, state)
        print("  ok    %d files and %d pictures in place, each read back.  The originals are in data\\english_backup "
              "(ENGLISH.bat off puts them back)." % (len(files), len(pics)))
    except (Failed, P.Failed, OSError) as ex:
        print("  FAIL  %s" % ex)
        return 1
    print("%s is on. Start the game with Launcher.exe." % LANGS[lang])
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
