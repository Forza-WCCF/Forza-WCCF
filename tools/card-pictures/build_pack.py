"""Builds the 2017-18 high-quality card picture pack (format 2) from a local copy of the source pictures.

The pack is data made from original files: it never goes into this repository. This script is the only part that
does. Needs Python 3.11+, Pillow and numpy.

    python -I build_pack.py --src <extract> --db <cards.db> --out <dir> [--name card-pictures-2017-18-1.zip]

<extract>: the folder with players-jpeg/old_front/<season>/<id>_F.jpg, players-jpeg/old_back/<season>/<id>_B.jpg
(664x911 JPEGs) and data/match-cards-db.csv (cardId -> candidate cards "<ver>/<card>;..."). <cards.db>: the card
database with `cards` (front = path of a 180x254 scan, relative to the database's folder) and `game_link`.

Every source front is compared with the scan of each candidate card (correlation of a band of grayscale detail at 60x85:
the photo, not the frame's colours). A candidate is taken when it scores at least ACCEPT and beats the next one by MARGIN; equal
scores (the same scan stored twice) take both. Version 30 (2017-18 Ver.3.0) is never linked: the 2017-18 game ends at
Ver.2.0. A game card number with several pictures takes the best-scoring one.

Out: the ZIP (stored, fixed timestamps, so the same input gives the same bytes), <zip>.sha256, matches.tsv (the
decision for every source front) and summary.txt (the counts).
"""

import argparse
import csv
import hashlib
import os
import sqlite3
import zipfile
from collections import Counter, defaultdict

import numpy as np
from PIL import Image, ImageFilter

# the same picture scores 0.65-0.95 (dim holo scans the lowest), another card of the same player up to 0.55; nothing
# scored 0.55-0.65 (4,340 fronts, 6,229 pairs, the edges checked by eye, 2026-11-07)
ACCEPT = 0.60
MARGIN = 0.10
GAME = '1718'
SKIP_VER = 30  # 17-18 V3.0
SIZE = (60, 85)
FORMAT = 'forza-wccf card pictures 2\n'
# the stats per game card number, in the order the pictures are picked (same as crates/forza/src/cards/hq.rs)
STATS = f"""SELECT l.game_card_no, c.position, c.off, c.def, c.tec, c.pow, c.spe, c.sta, c.tot
    FROM game_link l JOIN cards c USING (ver, card)
    WHERE l.game = ? AND l.ver <> {SKIP_VER} AND c.off IS NOT NULL AND c.tot IS NOT NULL
    ORDER BY l.game_card_no, l.method != 'name+stats', c.card LIKE '%EXTRA%', c.ver, c.card"""


def detail(path, big=False):
    im = Image.open(path)
    if big:
        im.draft('L', (im.size[0] // 2, im.size[1] // 2))
    g = im.convert('L').resize(SIZE, Image.BOX)
    # a band of detail: blurred scans and sharp prints compare alike, the frame's flat colours drop out
    a = np.asarray(g.filter(ImageFilter.GaussianBlur(1)), np.float32) - np.asarray(g.filter(ImageFilter.GaussianBlur(4)), np.float32)
    a = a[5:-5, 5:-5]  # the outer frame
    a -= a.mean()
    return (a / (np.linalg.norm(a) + 1e-6)).ravel()


def decide(scored):
    """[(score, cand)] -> the candidates taken, or a reason for none."""
    if not scored:
        return [], 'no candidate scan'
    s = sorted(scored, reverse=True)
    best = s[0][0]
    if best < ACCEPT:
        return [], 'no close scan'
    tied = [c for v, c in s if v == best]
    rest = [v for v, _ in s if v != best]
    if rest and best - rest[0] < MARGIN:
        return [], 'two close scans'
    return tied, 'taken'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--src', required=True)
    ap.add_argument('--db', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--name', default='card-pictures-2017-18-1.zip')
    a = ap.parse_args()
    fronts, backs = {}, {}
    for side, into in (('old_front', fronts), ('old_back', backs)):
        root = os.path.join(a.src, 'players-jpeg', side)
        for season in sorted(os.listdir(root)):
            for f in os.listdir(os.path.join(root, season)):
                into[str(int(f.split('_')[0]))] = os.path.join(root, season, f)
    rows = {r['cardId']: r for r in csv.DictReader(
        open(os.path.join(a.src, 'data', 'match-cards-db.csv'), encoding='utf-8-sig'))}
    con = sqlite3.connect(f'file:{a.db}?mode=ro', uri=True)
    scan = {f'{v}/{c}': f for v, c, f in con.execute('SELECT ver, card, front FROM cards WHERE front IS NOT NULL')}
    links = defaultdict(list)
    for v, c, no in con.execute('SELECT ver, card, game_card_no FROM game_link WHERE game = ? AND ver <> ?',
                                (GAME, SKIP_VER)):
        links[f'{v}/{c}'].append(no)
    base = os.path.dirname(os.path.abspath(a.db))
    os.makedirs(a.out, exist_ok=True)
    count, best, cache = Counter(), {}, {}
    with open(os.path.join(a.out, 'matches.tsv'), 'w', encoding='utf-8') as log:
        log.write('cardId\tkind\tdecision\ttaken\tgame_numbers\tscores\n')
        for cid in sorted(fronts, key=int):
            cands = [x for x in (rows.get(cid, {}).get('soden_card') or '').split(';') if x]
            kind = 'none' if not cands else 'one' if len(cands) == 1 else 'several'
            scored = []
            if cands:
                d = detail(fronts[cid], True)
                for c in cands:
                    p = scan.get(c) and os.path.join(base, scan[c])
                    if p and os.path.exists(p):
                        cache.setdefault(p, None)
                        if cache[p] is None:
                            cache[p] = detail(p)
                        scored.append((round(float(d @ cache[p]), 4), c))
            taken, why = decide(scored) if cands else ([], 'no candidate')
            if any(c.startswith(f'{SKIP_VER}/') for c in taken):
                taken, why = [], 'Ver.3.0 card'
            nos = sorted({n for c in taken for n in links.get(c, [])})
            if taken and not nos:
                why = 'taken, not in the game'
            count[(kind, why)] += 1
            top = max((v for v, _ in scored), default=0)
            for n in nos:
                if n not in best or top > best[n][0]:
                    best[n] = (top, cid)
            log.write(f"{cid}\t{kind}\t{why}\t{';'.join(taken)}\t{';'.join(map(str, nos))}\t"
                      f"{';'.join(f'{c}={v}' for v, c in scored)}\n")
    used = sorted({cid for _, cid in best.values()}, key=int)
    zpath = os.path.join(a.out, a.name)
    stamp = (2026, 1, 1, 0, 0, 0)

    def put(z, name, data):
        info = zipfile.ZipInfo(name, stamp)
        info.compress_type = zipfile.ZIP_STORED
        info.external_attr = 0o644 << 16
        z.writestr(info, data)

    with zipfile.ZipFile(zpath, 'w') as z:
        put(z, 'pack.txt', FORMAT)
        for cid in used:
            put(z, f'pictures/{cid}_F.jpg', open(fronts[cid], 'rb').read())
            if cid in backs:
                put(z, f'pictures/{cid}_B.jpg', open(backs[cid], 'rb').read())
        tsv = ''.join(f"{n}\t{cid}_F.jpg\t{cid + '_B.jpg' if cid in backs else ''}\n"
                      for n, (_, cid) in sorted(best.items()))
        put(z, '2017-2018/cards.tsv', tsv)
        stats, last = [], None
        for no, pos, *v in con.execute(STATS, (GAME,)):
            if no != last:
                stats.append('\t'.join(map(str, [no, pos or '', *[x or 0 for x in v]])) + '\n')
            last = no
        put(z, '2017-2018/hq_stats.tsv', ''.join(stats))
    with open(zpath, 'rb') as f:
        h = hashlib.file_digest(f, 'sha256').hexdigest()
    open(zpath + '.sha256', 'w').write(f'{h}  {a.name}\n')
    with open(os.path.join(a.out, 'summary.txt'), 'w') as s:
        for (kind, why), n in sorted(count.items()):
            s.write(f'{kind}\t{why}\t{n}\n')
        s.write(f'game numbers\t{len(best)}\npictures\t{len(used)} fronts, {sum(c in backs for c in used)} backs\n'
                f'zip\t{os.path.getsize(zpath)} bytes\nsha256\t{h}\n')
    print(open(os.path.join(a.out, 'summary.txt')).read(), end='')


if __name__ == '__main__':
    main()
