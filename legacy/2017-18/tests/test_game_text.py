# -*- coding: utf-8 -*-
"""test_game_text.py - the 2017-18 game text in English or Italian (cargo xtask check runs it; no game): "it" puts the
Italian sheet first and keeps the English as each key's fallback, "en" never shows Italian, every Italian line is in the
game's font (cp932), and a line that would lose a code of the game's Japanese line gives way to the next choice.
english.py checks every line against the game's own Japanese at "on" (english.py selftest GAME SCRATCH it).
    python source\\test_game_text.py"""
import collections
import os
import sys

KIT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(KIT, "scripts"))
import english as E  # noqa: E402

en, it = E.kit_lines("en"), E.kit_lines("it")
italian = E.sheet("screen_text.tsv", "italian")
both = next(r["key"] for r in italian if r["key"] in en)
only_en = next(k for k in en if k not in {r["key"] for r in italian})
assert [s for _, s in it[both]] == ["kit Italian", "kit"], it[both]       # Italian first, the English behind it
assert it[both][1] == en[both][0], "the fallback is the committed English"
assert it[only_en] == en[only_en], "a key with no Italian stays English"
assert all(s == "kit" for v in en.values() for _, s in v), "English shows no Italian"

bad = ["%s: %d times" % (k, c) for k, c in collections.Counter(r["key"] for r in italian).items() if c > 1]
for r in italian:
    try:
        r["italiano"].encode("cp932")
    except UnicodeEncodeError:
        bad.append("%s: a letter the game's font cannot show" % r["key"])
assert not bad, "%d problem(s): %s" % (len(bad), "; ".join(bad[:5]))

# the markup: the game's line "<kana>$c[FF6633FF]%d$c"; an Italian line without the colour code is not used
body, keys = "あ$c[FF6633FF]%d$c\0".encode("cp932"), {"CLM_X_0000": (1, [0, 0])}
lose = {"CLM_X_0000": [("%d punti", "kit Italian"), ("$c[FF6633FF]%d$c points", "kit")]}
keep = {"CLM_X_0000": [("$c[FF6633FF]%d$c punti", "kit Italian"), ("$c[FF6633FF]%d$c points", "kit")]}
enc, used, misfit = E.encode_lines(body, keys, lose)
assert (enc, used, len(misfit)) == ({"CLM_X_0000": b"$c[FF6633FF]%d$c points"}, {"CLM_X_0000": "kit"}, 1), enc
assert E.encode_lines(body, keys, keep)[0] == {"CLM_X_0000": b"$c[FF6633FF]%d$c punti"}
print("2017-18 game text: %d Italian lines, the English behind each; a line that loses a code falls back"
      % len(italian))
