# -*- coding: utf-8 -*-
"""make_tables.py - writes english\\screen_text.tsv, sega_rstring.tsv and cpu_names.tsv for 2017-18 (2.00.00).

    python3 make_tables.py GAME1718 REVD OLD_ENGLISH [TODO_DIR]

GAME1718 the 2017-18 game folder, REVD the 2010-11 Rev D "extracted" folder, OLD_ENGLISH the 2010-11 kit's english\\
folder.  Lines are carried from 2010-11 only where the 2017-18 Japanese is identical (same key first, else any key with
that Japanese); new_lines.tsv (key, english, note: one row per distinct Japanese, by a representative key) and
cpu_new.tsv (file, english) are this kit's new translations, spread to every key / team file with the same Japanese.
Only English, keys, file names and SHA-256 fingerprints are written.  With TODO_DIR it also lists, there (never in the
repo: they hold Sega's Japanese), the lines and teams still without English."""
import collections, csv, hashlib, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import english1718 as E


def load(root):
    raw = open(os.path.join(root, E.BIN), "rb").read()
    body, keys = E.read_strings(raw, open(os.path.join(root, E.HF), "rb").read())
    return {k: [E.at(body, n).decode("cp932") for n in nums] for k, (_, nums) in keys.items()}


def rows(path):
    if not os.path.isfile(path):
        return []
    with open(path, encoding="utf-8", newline="") as f:
        return list(csv.DictReader(f, delimiter="\t", quoting=csv.QUOTE_NONE))


def write(path, header, data):
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write("\t".join(header) + "\n")
        for r in data:
            f.write("\t".join(r) + "\n")


def team_names(root):
    d = os.path.join(root, E.TEAMS)
    out = {}
    for f in os.listdir(d):
        if re.match(r"cpu_\d+\.dat$", f):
            b = open(os.path.join(d, f), "rb").read()
            out[f] = (b[E.NAME_AT:E.NAME_END].split(b"\0")[0].decode("cp932"), hashlib.sha256(b).hexdigest().upper())
    return out


def main(game, revd, old_eng, todo=None):
    new, old = load(game), load(revd)
    isjp = lambda s: any(ord(c) >= 0x80 for c in s)
    scr = {r["key"]: r["english"] for r in rows(os.path.join(old_eng, "screen_text.tsv")) if (r["english"] or "").strip()}
    rst = {r["key"]: r["english"] for r in rows(os.path.join(old_eng, "sega_rstring.tsv"))}
    by_jp = lambda table: {jp: c.most_common(1)[0][0] for jp, c in _tm(old, table).items()}
    scr_jp, rst_jp = by_jp(scr), by_jp(rst)
    seg2 = {k: v[1] for k, v in old.items() if isjp(v[0]) and v[1] and not isjp(v[1])}
    seg2_jp = by_jp(seg2)                                  # Rev D's own English column, by identical Japanese
    mine = {r["key"]: (r["english"], r.get("note") or "") for r in rows(os.path.join(HERE, "new_lines.tsv"))}
    mine_jp = {new[k][0]: v for k, v in mine.items() if k in new}
    out_scr, out_rst, left = [], [], []
    for k in sorted(new, key=lambda k: (k.split("_")[0], k)):
        jp = new[k][0]
        if not isjp(jp):
            continue
        same = k in old and old[k][0] == jp
        if k in scr and same:
            out_scr.append((k, scr[k], "2010-11"))
        elif jp in scr_jp:
            out_scr.append((k, scr_jp[jp], "2010-11"))
        elif new[k][1] and not isjp(new[k][1]):
            continue                                      # Sega's own English column: english1718 reads it
        elif k in mine or jp in mine_jp:
            en, note = mine.get(k) or mine_jp[jp]
            out_scr.append((k, en, note or "new"))
        elif k in rst and same:
            out_rst.append((k, rst[k]))
        elif jp in rst_jp:
            out_rst.append((k, rst_jp[jp]))
        elif jp in seg2_jp:
            out_rst.append((k, seg2_jp[jp]))
        else:
            left.append(k)
    write(os.path.join(HERE, "screen_text.tsv"), ("key", "english", "note"), out_scr)
    write(os.path.join(HERE, "sega_rstring.tsv"), ("key", "english"), out_rst)
    # CPU teams: 2010-11 name by identical Japanese, else cpu_new.tsv by identical Japanese
    old_names = {}
    oldrows = {r["file"]: r["english"] for r in rows(os.path.join(old_eng, "cpu_names.tsv"))}
    for f, (jp, _) in team_names(revd).items():
        if f in oldrows:
            old_names.setdefault(jp, oldrows[f])
    teams = team_names(game)
    cnew = {teams[r["file"]][0]: r["english"] for r in rows(os.path.join(HERE, "cpu_new.tsv")) if r["file"] in teams}
    out_t, tleft = [], []
    for f in sorted(teams, key=lambda f: int(f[4:-4])):
        jp, h = teams[f]
        en = cnew.get(jp) or old_names.get(jp) or ("" if isjp(jp) else jp)
        (out_t if en else tleft).append((f, h, en))
    write(os.path.join(HERE, "cpu_names.tsv"), ("file", "sega_sha256", "english"), out_t)
    print("screen_text %d (2010-11 %d, new %d), sega_rstring %d, left %d (%d distinct); teams %d, left %d" % (
        len(out_scr), sum(1 for r in out_scr if r[2] == "2010-11"), sum(1 for r in out_scr if r[2] != "2010-11"),
        len(out_rst), len(left), len({new[k][0] for k in left}), len(out_t), len(tleft)))
    if todo:
        os.makedirs(todo, exist_ok=True)
        seen = collections.OrderedDict()
        for k in left:
            seen.setdefault(new[k][0], []).append(k)
        with open(os.path.join(todo, "left.tsv"), "w", encoding="utf-8") as f:
            for jp, ks in seen.items():
                f.write("%s\t%d\t%s\n" % (ks[0], len(ks), jp.replace("\n", "\\n")))
        with open(os.path.join(todo, "teams_left.tsv"), "w", encoding="utf-8") as f:
            for jp, fs in collections.OrderedDict((teams[t[0]][0], t[0]) for t in tleft).items():
                f.write("%s\t%s\n" % (fs, jp))


def _tm(old, table):
    tm = collections.defaultdict(collections.Counter)
    for k, v in old.items():
        if k in table:
            tm[v[0]][table[k]] += 1
    return tm


if __name__ == "__main__":
    main(*sys.argv[1:])
