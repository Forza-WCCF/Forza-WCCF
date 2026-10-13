# -*- coding: utf-8 -*-
r"""panel.py - does what the in-game panels ask for and cannot do from inside the game.  The run in Launcher.exe
(crates\launcher\src\run) starts it without a window with the cabinets; the end of the run ends it with everything else.
    python panel.py SECONDS [DATA]        (DATA: tests only)

RESTART NOW (data\restart.request) and SEND LOGS (data\sendlogs.request -> data\sendlogs.result) are done by the run in
Launcher.exe itself since step 10 (piece G); this helper keeps the rest until piece H ports it.

THE CLUB CARD panel's view (2026-10-06): data\club_view.txt, rewritten by view.py (with decode.py, the
reader the website will use) whenever the card, its backups or the run change - once the card has held still for a
second, so the game's bursts of saves are not read half way.  A view that cannot be written is said once and never
stops the helper: RESTART NOW does not depend on it.

THE STARTING VOLUMES (2026-10-06): sound.py - the projector muted, seat 1 very low - set once for each, as
soon as it has opened its sound (looked for every 0.5 s for 10 minutes after the start); never again, so a change in
the Volume Mixer stays.  The game folder comes from the run (WCCF_GAME); without it nothing is set.  Like the view, it
never stops the helper.
"""
import os
import sys
import time

import kit_common as K


def say(msg):
    print(time.strftime("%H:%M:%S"), msg, flush=True)


class ClubView:
    """the CLUB CARD panel's view of the card in DATA (data\\club_view.txt)"""

    def __init__(self, data):
        save = os.path.join(data, "save")
        self.card, self.backups = os.path.join(save, "seat1_club.bin"), os.path.join(save, "backup")
        self.cards = os.path.join(save, "cards")                       # your other club cards (wallet.py)
        self.running, self.out = os.path.join(data, "running.json"), os.path.join(data, "club_view.txt")
        self.catalogue = os.path.join(data, "cache", "catalogue.tsv")     # made by SETUP (was overlay\)
        self.seen, self.since, self.done, self.mod, self.said, self.retry_at = None, 0.0, None, None, False, 0.0

    def signature(self):
        def st(p):
            try:
                s = os.stat(p)
                return s.st_mtime_ns, s.st_size
            except OSError:
                return None
        def listing(folder):
            try:
                return tuple(sorted((n, st(os.path.join(folder, n))) for n in os.listdir(folder)))
            except OSError:
                return ()
        return st(self.card), listing(self.backups), st(self.running), listing(self.cards)

    def tick(self, now):
        sig = self.signature()
        if sig != self.seen:                    # changed: wait until it holds still for a second
            self.seen, self.since = sig, now
            return
        if sig == self.done or now - self.since < 1.0 or now < self.retry_at:
            return
        try:
            if self.mod is None:
                from cards import view
                self.mod = view
            lines = self.mod.build(card=self.card, catalogue=self.catalogue, backups=self.backups,
                                   running=self.running, cards=self.cards)
            self.mod.write(lines, self.out)
            self.done = sig                     # only once written: a failed write is tried again
            say("club card view written: %s" % (lines[0] if lines else "-"))
        except Exception as ex:                 # the view never stops the helper; again in 5 s
            self.retry_at = now + 5.0
            if not self.said:
                say("club card view could not be written (tried again every 5 s): %r" % ex)
                self.said = True


class SoundDefaults:
    """the projector muted and seat 1 at sound.GAME_VOLUME, each set once, when it has first opened its
    sound; mod: helpers.sound, or a stand-in (the tests)"""
    LOOK, FOR = 0.5, 600.0

    def __init__(self, game, now, mod=None):
        self.targets = {}
        if game:
            game = os.path.abspath(game)
            self.targets = {os.path.normcase(os.path.join(game, "client_Release.exe")): "projector",
                            os.path.normcase(os.path.join(os.path.dirname(game), "seat1", "client_Release.exe")):
                                "seat 1"}
        self.mod, self.done, self.next, self.until, self.said = mod, set(), 0.0, now + self.FOR, False

    def tick(self, now):
        if not self.targets or len(self.done) == len(self.targets) or now < self.next or now > self.until:
            return
        self.next = now + self.LOOK
        try:
            if self.mod is None:
                from helpers import sound
                self.mod = sound
            s = self.mod
            for pid, path, _level, _muted in s.sessions():
                role = self.targets.get(os.path.normcase(path or ""))
                if not role or role in self.done:
                    continue
                if role == "projector":
                    n = s.apply(pid, volume=s.GAME_VOLUME, mute=s.PROJECTOR_MUTED)
                    what = "muted" if s.PROJECTOR_MUTED else "at %d%%" % round(s.GAME_VOLUME * 100)
                else:
                    n, what = s.apply(pid, volume=s.GAME_VOLUME, mute=False), "at %d%%" % round(s.GAME_VOLUME * 100)
                if n:
                    self.done.add(role)
                    say("sound: %s starts %s (pid %d) - the Volume Mixer changes it from here" % (
                        "the projector" if role == "projector" else role, what, pid))
        except Exception as ex:                 # the volumes never stop the helper: the game plays as Windows has it
            if not self.said:
                say("sound: the starting volumes could not be set (%r) - tried again every %.1f s" % (ex, self.LOOK))
                self.said = True


def main(argv):
    if not argv or not argv[0].isdigit():
        print(__doc__)
        return 2
    life = int(argv[0])
    data = argv[1] if len(argv) > 1 else K.DATA
    view = ClubView(data)
    sound = SoundDefaults(os.environ.get("WCCF_GAME"), time.time())
    end = time.time() + life
    while time.time() < end:
        view.tick(time.time())
        sound.tick(time.time())
        time.sleep(0.5)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
