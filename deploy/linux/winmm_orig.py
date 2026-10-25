#!/usr/bin/env python3
"""winmm_orig.dll for a server under Wine: Wine's own 32-bit winmm.dll, renamed so Wine loads it as a plain DLL.

Wine marks its own DLLs "Wine builtin DLL" and would look for a builtin named winmm_orig; the server hook (winmm.dll,
forza-hook e1011-server or e1718-server) forwards every winmm export to winmm_orig.dll, so the copy must load as an ordinary DLL.

    python3 deploy/linux/winmm_orig.py OUT [WINE_WINMM]

WINE_WINMM defaults to /usr/lib/i386-linux-gnu/wine/i386-windows/winmm.dll (Debian's wine32).
"""
import sys

MARK, RENAMED = b"Wine builtin DLL\0", b"WCCF renamed DLL\0"

if len(sys.argv) not in (2, 3):
    sys.exit(__doc__)
src = sys.argv[2] if len(sys.argv) == 3 else "/usr/lib/i386-linux-gnu/wine/i386-windows/winmm.dll"
b = open(src, "rb").read()
if b.count(MARK) != 1:
    sys.exit(f"{src}: not a Wine builtin DLL")
open(sys.argv[1], "wb").write(b.replace(MARK, RENAMED))
