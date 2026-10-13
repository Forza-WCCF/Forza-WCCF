# -*- coding: utf-8 -*-
r"""card_steps.py - the club card steps of a start and a stop, which the run in Launcher.exe (crates\launcher\src\run)
asks for while the card reader is not running (moved here from play.py; Python until step 11 ports cards\).
    python card_steps.py start SEAT [board]   a finished manager transfer put away, the CLUB CARD panel's "card=" switch
                                              and "card_fix=bad_endings", each taken out of data\panel.txt once done;
                                              "board": then the cards on the table follow the club in the slot
    python card_steps.py keep SEAT            after a stop: the cards on the table kept with their club
Anything wrong is said, and never stops the start: the game starts with the card that is in the slot.
"""
import os
import sys

import kit_common as K

PANEL = os.path.join(K.DATA, "panel.txt")         # the SETTINGS panel's choices (wccfpanel.dll writes it)
CARD = os.path.join(K.SAVE, "seat1_club.bin")


def say(msg):
    print(msg, flush=True)


def read_panel(path=None):
    """{name: value} from the SETTINGS panel's file: lines "name=value", "#" starts a note; {} if there is none"""
    out = {}
    try:
        with open(path or PANEL, encoding="ascii", errors="replace") as f:
            for line in f:
                name, eq, value = line.split("#", 1)[0].partition("=")
                if eq and name.strip():
                    out[name.strip().lower()] = value.strip()
    except OSError:
        pass
    return out


def drop_panel_line(name, path=None):
    """take a request done out of the panel's file (only while nothing of the game runs: the panel is not writing)"""
    path = path or PANEL
    try:
        with open(path, encoding="ascii", errors="replace", newline="") as f:
            lines = f.readlines()
    except OSError:
        return
    keep = [ln for ln in lines if ln.split("#", 1)[0].partition("=")[0].strip().lower() != name]
    if keep != lines:
        with open(path + ".tmp", "w", encoding="ascii", errors="replace", newline="") as f:
            f.writelines(keep)
        os.replace(path + ".tmp", path)


def apply_card_request():
    """the CLUB CARD panel's "card=FILE" / "card=new": which club card goes into the slot (wallet.switch: two renames,
    no card ever overwritten), then taken out of the file - only after the switch, so one stopped half way is finished
    at the next start.  First, a finished manager transfer is put away (wallet.finish_transfer)."""
    try:
        from cards import wallet
        _done, what = wallet.finish_transfer()
    except Exception as ex:                    # e.g. a card file held by another program: both stay, said
        what = "the manager transfer could not be finished now (%s) - both cards stay where they are" % ex
    if what:
        say("  club card: %s" % what)
    want = read_panel().get("card", "")
    if not want:
        return
    try:
        from cards import wallet
        _done, what = wallet.switch(want)
    except Exception as ex:                    # e.g. a card file held by another program: the slot stays as it is
        what = "the switch could not be done (%s) - the card in the slot stays" % ex
    say("  club card: %s" % what)
    drop_panel_line("card")


def apply_card_fix():
    """the CLUB CARD panel's "card_fix=bad_endings": the slot card's bad endings back to 0 (edit.clear_bad_endings: the
    game's own checks, the old card into data\\save\\backup first), then taken out of the file"""
    want = read_panel().get("card_fix", "").lower()
    if not want:
        return
    if want == "bad_endings":
        try:
            from cards import edit
            what = edit.clear_bad_endings(CARD)
        except Exception as ex:
            what = "the bad endings were not cleared (%s) - the card stays as it was" % ex
        say("  club card: %s" % what)
    drop_panel_line("card_fix")


def apply_board(seat):
    """the cards on the table follow the club card in the slot (formation.py) - done while nothing reads the table"""
    try:
        from cards import formation
        for what in formation.sync(seat):
            say("  table: %s" % what)
    except Exception as ex:
        say("  table: left as it is (%s)" % ex)


def keep(seat):
    """after a stop: the cards on the table kept with their club (data\\save holds the latest)"""
    try:
        from cards import formation
        formation.keep(seat)
    except Exception as ex:
        say("  (the cards on the table were not copied to their club: %s)" % ex)


def main(argv):
    if len(argv) in (2, 3) and argv[0] == "start" and argv[1:-1] in ([], ["board"]):
        apply_card_request()
        apply_card_fix()
        if argv[1:-1] == ["board"]:
            apply_board(argv[-1])
        return 0
    if len(argv) == 2 and argv[0] == "keep":
        keep(argv[1])
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass
    sys.exit(main(sys.argv[1:]))
