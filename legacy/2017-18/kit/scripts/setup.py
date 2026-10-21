# -*- coding: utf-8 -*-
# RETIRED: the source of truth for 2017-18 setup is the Rust port, crates/forza/src/setup/e1718.rs (run by Launcher.exe). This
# file lacks ICMP.DLL and the newer patches (title movie, GAME_READY fallback); do not align or extend it.
r"""setup.py - set up your copy of WCCF 2017-18 for Forza-WCCF (once; run it again to check and repair).

    python\python.exe scripts\setup.py GAME      ("undo GAME" puts the originals back)   GAME = the decrypted game folder (client_Release.exe, control_Release.exe)

Works in place on that folder, so give it your own copy.  The runtime is the folder that holds GAME:
    original\    the untouched client_Release.exe and control_Release.exe, kept on the first run; every later run
                 patches from them, so running setup again never stacks a patch (a second run changes nothing)
    game\        the server and projector: guarded programs (they import wccfnet.dll, which stubs the four IP-helper
                 calls, instead of IPHLPAPI.DLL), the kit's winmm.dll / logowin.exe stand-ins, the settings files
    seat1\       the player's cabinet: junctions to game's folders, hard links to its files, its own patched
                 client_Release.exe, the card-table helper
    setup-hashes.json   the SHA-256 of what setup made; Launcher.exe starts nothing unless every file still matches
and writes the kit's data\settings.json "game" and overlay\catalogue.tsv (every card, for the board).
Only the two original programs with the SHA-256 below are accepted."""
import hashlib
import json
import os
import shutil
import struct
import sys

import kit_common as K
import patches1718 as P

# the decrypted 2017-18 game's two programs (every patch below checks its own original bytes as well)
PINNED = {
    "client_Release.exe": "8b55456f04ad4766fa1226738951954ab77f77ae2e4d7d0b57b88544a283e5ca",
    "control_Release.exe": "0539087f72a238fab5a69f0f294205ff186028c7b9d3b9a1f95973fc19f27c5f",
}
OLD, NEW = b"IPHLPAPI.DLL\0", b"wccfnet.dll\0\0"
# the name the original logowin.exe is kept under beside it
LOGOWIN_BACKUP = "logowin_" + bytes.fromhex("73656761").decode() + ".exe"
# the card-table helper as the client names it, relative to seat 1's folder
FPR_EMU = "FlatPanelReader_Emulator/FPR_Emu.exe"
CLIENT_KEYS = ("IS_RINGEDGE=%d\r\nALLNET_AUTH=0\r\nDHCP=0\r\nCONTROL_IP=127.0.0.1\r\nCONTROL_PORT=20002\r\n"
               "MATCH_IP=127.0.0.1\r\nMATCH_PORT=20003\r\nFPR_MODE=2\r\nFULL_SCREEN_MODE=0\r\nWAIT_PROJECTOR=1\r\n"
               "SATELLITE_NO=%d\r\n")
CONF_CONTROL = ("DHCP=0\r\nIS_RINGEDGE=0\r\nALLNET_AUTH=0\r\nACCEPT_ALL_IP=1\r\nCLIENT_MAX=9\r\nACCEPT_IP_0=127.0.0.1\r\n"
                "CONTROL_PORT=20002\r\nMATCH_PORT=20003\r\nFIX_REST_ENTRY_TIME=120\r\n"
                # online through the local game server stand-in; "127.0.0.1" itself would mean "use the address
                # ALL.Net gave", so the stand-in is named localhost
                "FORCE_ONLINE=1\r\nGAME_SVR_IP=localhost\r\nGAME_SVR_PORT=20050\r\n"
                "WAIT_PROJECTOR=1\r\n")
JUNCTION_DIRS = ["AI", "data", "lighting", "Microsoft.VC90.CRT", "Microsoft.VC90.MFC", "prog_data", "script",
                 "shader", "update"]


class Failed(Exception):
    pass


def sha(b):
    return hashlib.sha256(b).hexdigest()


def rva_off(d, rva):
    nt = struct.unpack_from("<I", d, 60)[0]
    n, opt = struct.unpack_from("<H", d, nt + 6)[0], struct.unpack_from("<H", d, nt + 20)[0]
    for i in range(n):
        _, vs, va, rs, raw = struct.unpack_from("<8sIIII", d, nt + 24 + opt + i * 40)
        if va <= rva < va + max(vs, rs):
            return raw + rva - va
    raise ValueError(hex(rva))


def guarded(data):
    """the exe with its one IPHLPAPI.DLL import renamed to wccfnet.dll"""
    d = bytearray(data)
    nt = struct.unpack_from("<I", d, 60)[0]
    off = rva_off(d, struct.unpack_from("<I", d, nt + 24 + 96 + 8)[0])
    hits = []
    while struct.unpack_from("<I", d, off + 12)[0]:
        name = rva_off(d, struct.unpack_from("<I", d, off + 12)[0])
        if bytes(d[name:name + len(OLD)]).upper() == OLD:
            hits.append(name)
        off += 20
    if len(hits) != 1:
        raise Failed("the program has %d IPHLPAPI.DLL imports, expected 1" % len(hits))
    d[hits[0]:hits[0] + len(OLD)] = NEW
    return bytes(d)


def put(path, data):
    with open(path, "wb") as f:
        f.write(data)
    with open(path, "rb") as f:
        if sha(f.read()) != sha(data):
            raise Failed("%s was not written whole" % path)


def read(path):
    with open(path, "rb") as f:
        return f.read()


def original(exe, current, backup, pinned=PINNED):
    """The untouched `exe` bytes: from `backup` when it exists, else from `current`, which is then copied to `backup`.
    Anything whose SHA-256 is not the pinned one is refused."""
    src = backup if os.path.isfile(backup) else current
    data = read(src)
    if sha(data) != pinned[exe]:
        raise Failed("%s is not the original WCCF 2017-18 program (SHA-256 %s, expected %s). Give setup the decrypted "
                     "2017-18 game folder." % (src, sha(data), pinned[exe]))
    if src == current:
        os.makedirs(os.path.dirname(backup), exist_ok=True)
        put(backup, data)
    return data


def make_junction(link, target):
    if os.path.lexists(link):
        return
    import _winapi
    _winapi.CreateJunction(target, link)
    if not os.path.isdir(link):
        raise Failed("could not make the folder link %s" % link)


def merge_settings(path, **keys):
    """settings.json with `keys` set; every other key stays"""
    try:
        with open(path, encoding="utf-8") as f:
            s = json.load(f)
    except (OSError, ValueError):
        s = {}
    s.update(keys)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path + ".tmp", "w", encoding="utf-8") as f:
        json.dump(s, f, indent=1)
    os.replace(path + ".tmp", path)


def find_game(path):
    if not path:
        return None
    path = os.path.abspath(path.strip().strip('"'))
    for p in (path, os.path.join(path, "game")):
        if all(os.path.isfile(os.path.join(p, n)) for n in PINNED):
            return p
    return None


def copy_in(src, dst, what):
    if not os.path.isfile(src):
        raise Failed("the kit is missing %s (%s)" % (src, what))
    shutil.copyfile(src, dst)


def setup_game(game, kit=K.KIT):
    """game\\ (server + projector, guarded) and seat1\\; returns what setup-hashes.json holds"""
    rt = os.path.dirname(game)
    seat = os.path.join(rt, "seat1")
    bin_, overlay = os.path.join(kit, "bin"), os.path.join(kit, "overlay")
    report = {}
    for exe in PINNED:
        orig = original(exe, os.path.join(game, exe), os.path.join(rt, "original", exe))
        g = guarded(orig)
        if exe == "control_Release.exe":
            g = P.apply(g, P.CONTROL_PATCHES)
        else:
            g = P.apply(g, P.PROJECTOR_PATCHES)   # the projector's status icon needs the same ALL.Net answers as seat 1
        if not os.path.isfile(os.path.join(game, exe)) or read(os.path.join(game, exe)) != g:
            put(os.path.join(game, exe), g)
        report[exe] = {"original": sha(orig), "guarded": sha(g)}
    for dll, built in (("wccfnet.dll", os.path.join(bin_, "wccfnet.dll")),
                       ("netguard.dll", os.path.join(overlay, "netguard.dll"))):
        copy_in(built, os.path.join(game, dll), "the network guard")
        report[dll] = sha(read(os.path.join(game, dll)))
    winmm = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "SysWOW64", "winmm.dll")
    copy_in(winmm, os.path.join(game, "winmm_orig.dll"), "Windows' 32-bit winmm.dll")
    copy_in(os.path.join(bin_, "winmm.dll"), os.path.join(game, "winmm.dll"), "the cabinet stand-in")
    if not os.path.exists(os.path.join(game, LOGOWIN_BACKUP)):
        os.replace(os.path.join(game, "logowin.exe"), os.path.join(game, LOGOWIN_BACKUP))
    copy_in(os.path.join(bin_, "logowin.exe"), os.path.join(game, "logowin.exe"), "the logo stand-in")
    os.makedirs(os.path.join(game, "local"), exist_ok=True)
    put(os.path.join(game, "local", "client_user_option.conf"), (CLIENT_KEYS % (0, 0)).encode())
    put(os.path.join(game, "local", "ctrl_user_option.conf"), CONF_CONTROL.encode())

    # seat 1: junctions to game\'s folders, hard links to its files, its own exe and settings
    os.makedirs(os.path.join(seat, "local"), exist_ok=True)
    for n in JUNCTION_DIRS:
        make_junction(os.path.join(seat, n), os.path.join(game, n))
    for name in os.listdir(game):
        low = name.lower()
        src, dst = os.path.join(game, name), os.path.join(seat, name)
        if (not os.path.isfile(src) or low == "client_release.exe"
                or low.endswith((".log", ".bin", ".bmp", ".tmp", ".txt")) or os.path.exists(dst)):
            continue
        os.link(src, dst)
    want = P.apply(guarded(read(os.path.join(rt, "original", "client_Release.exe"))))
    seat_exe = os.path.join(seat, "client_Release.exe")
    if not os.path.isfile(seat_exe) or read(seat_exe) != want:
        put(seat_exe, want)
    report["seat1/client_Release.exe"] = sha(want)
    put(os.path.join(seat, "local", "client_user_option.conf"), (CLIENT_KEYS % (1, 1)).encode())
    shutil.copyfile(os.path.join(game, "local", "SDDY_Table.dat"), os.path.join(seat, "local", "SDDY_Table.dat"))
    # the 2017-18 client starts the card-table helper from its own folder; with none there the game reads no cards
    if ("./" + FPR_EMU).encode() not in want:
        raise Failed("seat 1's client no longer names %s" % FPR_EMU)
    helper = os.path.join(seat, *FPR_EMU.split("/"))
    os.makedirs(os.path.dirname(helper), exist_ok=True)
    copy_in(os.path.join(bin_, "FPR_Emu.exe"), helper, "the card-table helper")
    with open(os.path.join(rt, "setup-hashes.json"), "w") as f:
        f.write(json.dumps(report, indent=2) + "\n")
    return report


def undo(game):
    """the game folder as it was: the original programs and logo back, the kit's files, seat1\\ and the record removed"""
    rt = os.path.dirname(game)
    for exe in PINNED:
        shutil.copyfile(os.path.join(rt, "original", exe), os.path.join(game, exe))
    backup = os.path.join(game, LOGOWIN_BACKUP)
    if os.path.isfile(backup):
        os.replace(backup, os.path.join(game, "logowin.exe"))
    for f in ("winmm.dll", "winmm_orig.dll", "wccfnet.dll", "netguard.dll"):
        if os.path.isfile(os.path.join(game, f)):
            os.remove(os.path.join(game, f))
    seat = os.path.join(rt, "seat1")
    for n in JUNCTION_DIRS:          # a junction goes with rmdir; rmtree would walk into game\
        if os.path.lexists(os.path.join(seat, n)):
            os.rmdir(os.path.join(seat, n))
    shutil.rmtree(seat, ignore_errors=True)
    if os.path.isfile(os.path.join(rt, "setup-hashes.json")):
        os.remove(os.path.join(rt, "setup-hashes.json"))
    s = K.load_settings()
    s.pop("game", None)
    K.save_settings(s)
    print("Undone: the original programs are back in %s. Your club card stays in data\\save." % game)


def setup_catalogue(game, force=False):
    """overlay\\catalogue.tsv, for the board; the card pictures are the Launcher's (forza::cards::pictures)"""
    out = os.path.join(K.OVERLAY, "catalogue.tsv")
    if os.path.isfile(out) and not force:
        print("  ok    the card catalogue is made (run setup with 'cards' to make it again)")
        return
    import cards1718
    if cards1718.main([game, K.OVERLAY, "--catalogue-only"]) != 0:
        raise Failed("the card catalogue could not be made from this game")


def main(argv):
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass
    undoing = argv[:1] == ["undo"]
    force_cards = "cards" in argv
    rest = [a for a in argv if a not in ("cards", "undo")]
    game = find_game(rest[0] if rest else K.load_settings().get("game"))
    if rest and not game:
        print("no WCCF 2017-18 game files in %s (client_Release.exe, control_Release.exe)" % rest[0])
        return 2
    if not game:
        print("Where is the game? Drag your copy of the decrypted 2017-18 game folder into this window, then press Enter:")
        try:
            game = find_game(input("> "))
        except EOFError:
            game = None
        if not game:
            print("no WCCF 2017-18 game files there - run setup again and give the game folder")
            return 2
    print("Forza-WCCF 2017-18 - %s - game: %s" % ("undo" if undoing else "setup", game))
    try:
        if undoing:
            undo(game)
            return 0
        rep = setup_game(game)
        for k, v in rep.items():
            print("  ok    %s" % k)
        for d in (K.LOGS, K.SAVE):
            os.makedirs(d, exist_ok=True)
        merge_settings(K.SETTINGS, game=game)
        setup_catalogue(game, force_cards)
    except (Failed, OSError, ValueError, struct.error) as ex:
        print("  FAIL  %s" % ex)
        return 1
    print("Setup done. Start the game with Launcher.exe.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
