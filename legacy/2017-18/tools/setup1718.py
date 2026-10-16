#!/usr/bin/env python3
"""Set up the WCCF 2017-18 (SDDY) runtime for the adapted 2010-11 kit. Run from WSL.

    WCCF1718_GAME=/path/to/decrypted/game  python3 legacy/2017-18/tools/setup1718.py   (needs target/kit/2017-18: cargo xtask stage 2017-18)

    WCCF1718_GAME     the decrypted 2017-18 game folder (client_Release.exe ...), read only - required
    WCCF1718_RUNTIME  the runtime folder on an NTFS drive (default <runtime>); it must already hold a
                      plain copy of the game in game\\ (see docs/PROGRESS.md "Setup")
Makes: game\\ (server + projector, guarded), seat1\\ (player cabinet: links + its own patched exe
and the card-table helper).

Never touches the decrypted originals (SRC). Guard: client/control import wccfnet.dll instead of IPHLPAPI.DLL
(the built wccfnet.dll stubs the four IP-helper calls); the original exe hashes are checked first.
"""
import hashlib
import json
import os
import shutil
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SRC = Path(os.environ.get("WCCF1718_GAME") or sys.exit("set WCCF1718_GAME to the decrypted 2017-18 game folder"))
RT = Path(os.environ.get("WCCF1718_RUNTIME", "<runtime>"))
ROOT = HERE.parent.parent.parent  # the repository root
# the built, staged kit (cargo xtask build) holds bin\ and overlay\; the guard wccfnet.dll is built beside it, in source\build (not shipped)
GAME, SEAT, KIT = RT / "game", RT / "seat1", ROOT / "target" / "kit" / "2017-18"
GUARDS = {"wccfnet.dll": KIT / "source" / "build" / "wccfnet.dll", "netguard.dll": KIT / "overlay" / "netguard.dll"}
WINMM = Path("/mnt/c/Windows/SysWOW64/winmm.dll")
# the name the original logowin.exe is kept under beside it (the kit's setup restores from it)
LOGOWIN_BACKUP = "logowin_" + bytes.fromhex("73656761").decode() + ".exe"
OLD, NEW = b"IPHLPAPI.DLL\0", b"wccfnet.dll\0\0"
# the card-table helper as the client names it, relative to seat 1's folder
FPR_EMU = "./FlatPanelReader_Emulator/FPR_Emu.exe"

CLIENT_KEYS = ("IS_RINGEDGE=%d\r\nALLNET_AUTH=0\r\nDHCP=0\r\nCONTROL_IP=127.0.0.1\r\nCONTROL_PORT=20002\r\n"
               "MATCH_IP=127.0.0.1\r\nMATCH_PORT=20003\r\nFPR_MODE=2\r\nFULL_SCREEN_MODE=0\r\nWAIT_PROJECTOR=1\r\n"
               "SATELLITE_NO=%d\r\n")
CONF_CONTROL = ("DHCP=0\r\nIS_RINGEDGE=0\r\nALLNET_AUTH=0\r\nACCEPT_ALL_IP=1\r\nCLIENT_MAX=9\r\nACCEPT_IP_0=127.0.0.1\r\n"
                "CONTROL_PORT=20002\r\nMATCH_PORT=20003\r\nFIX_REST_ENTRY_TIME=120\r\n"
                # online through the local game server stand-in (_gamesvr.py); "127.0.0.1" itself would mean
                # "use the address ALL.Net gave", so the stand-in is named localhost
                "FORCE_ONLINE=1\r\nGAME_SVR_IP=localhost\r\nGAME_SVR_PORT=20050\r\n"
                "WAIT_PROJECTOR=1\r\n")
JUNCTION_DIRS = ["AI", "data", "lighting", "Microsoft.VC90.CRT", "Microsoft.VC90.MFC", "prog_data", "script",
                 "shader", "update"]


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
    assert len(hits) == 1, hits
    d[hits[0]:hits[0] + len(OLD)] = NEW
    assert OLD not in bytes(d).upper().replace(b"IPHLPAPI.DLL\0\0", b"")  # no second plain import string
    return bytes(d)


def put(path, data):
    path.write_bytes(data)
    assert sha(path.read_bytes()) == sha(data), path


def main():
    report = {}
    for exe in ("client_Release.exe", "control_Release.exe"):
        orig = (SRC / exe).read_bytes()
        cur = (GAME / exe).read_bytes()
        g = guarded(orig)
        if exe == "control_Release.exe":
            from patches1718 import apply, CONTROL_PATCHES
            g = apply(g, CONTROL_PATCHES)
        if cur != g:
            if sha(cur) not in (sha(orig), sha(guarded(orig))):
                sys.exit("%s is neither the original nor its guarded copy" % exe)
            put(GAME / exe, g)
        report[exe] = {"original": sha(orig), "guarded": sha(g)}
    for dll, built in GUARDS.items():
        shutil.copyfile(built, GAME / dll)
        report[dll] = sha((GAME / dll).read_bytes())
    shutil.copyfile(WINMM, GAME / "winmm_orig.dll")
    shutil.copyfile(KIT / "bin" / "winmm.dll", GAME / "winmm.dll")
    if not (GAME / LOGOWIN_BACKUP).exists():
        os.replace(GAME / "logowin.exe", GAME / LOGOWIN_BACKUP)
    shutil.copyfile(KIT / "bin" / "logowin.exe", GAME / "logowin.exe")
    (GAME / "local" / "client_user_option.conf").write_bytes((CLIENT_KEYS % (0, 0)).encode())
    (GAME / "local" / "ctrl_user_option.conf").write_bytes(CONF_CONTROL.encode())

    # seat 1: junctions to game\'s folders, hard links to its files, its own exe and settings (as the kit's setup)
    (SEAT / "local").mkdir(parents=True, exist_ok=True)
    for n in JUNCTION_DIRS:
        if not (SEAT / n).exists():
            os.system('/mnt/c/Windows/System32/cmd.exe /c mklink /J "%s" "%s" >/dev/null' % (win(SEAT / n), win(GAME / n)))
        assert (SEAT / n).exists(), n
    for f in GAME.iterdir():
        low = f.name.lower()
        if not f.is_file() or low == "client_release.exe" or low.endswith((".log", ".bin", ".bmp", ".tmp", ".txt")):
            continue
        dst = SEAT / f.name
        if not dst.exists():
            os.link(f, dst)
    seat_exe = SEAT / "client_Release.exe"
    want = seat_client()
    if not seat_exe.exists() or seat_exe.read_bytes() != want:
        put(seat_exe, want)
    report["seat1/client_Release.exe"] = sha(want)
    (SEAT / "local" / "client_user_option.conf").write_bytes((CLIENT_KEYS % (1, 1)).encode())
    shutil.copyfile(GAME / "local" / "SDDY_Table.dat", SEAT / "local" / "SDDY_Table.dat")
    # the 2017-18 client starts the card-table helper from its own folder (2010-11: ..\misc\...\exe\); with no
    # helper there the game reads no cards ("0 Card(s)" at team registration, live run 4)
    if FPR_EMU.encode() not in want:
        sys.exit("seat 1's client no longer names %s" % FPR_EMU)
    helper = SEAT / FPR_EMU
    helper.parent.mkdir(exist_ok=True)
    shutil.copyfile(KIT / "bin" / "FPR_Emu.exe", helper)
    (RT / "setup-hashes.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


def win(p):
    return os.popen("wslpath -w '%s'" % p).read().strip()


def seat_client():
    """guarded client + seat 1 patches (patches1718.PATCHES, each checked against the original bytes)"""
    from patches1718 import apply
    return apply(guarded((SRC / "client_Release.exe").read_bytes()))


if __name__ == "__main__":
    main()
