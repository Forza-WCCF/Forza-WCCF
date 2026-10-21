#!/usr/bin/env bash
# Builds the 2010-11 server hook (winmm.dll for control and the match engines under Wine) and its three self-tests
# into out/ with MinGW-w64 for 32-bit x86 (Debian: apt install gcc-mingw-w64-i686). The self-tests use GCC's inline
# assembly, so clang-cl does not build them.
#   ./build.sh                           out/winmm.dll, out/{msgguard,sleepfix,spinfix}_test.exe
#   ./build.sh orig [WINE_WINMM]         also out/winmm_orig.dll: Wine's own 32-bit winmm.dll, renamed so Wine loads
#                                        it as a plain DLL (default /usr/lib/i386-linux-gnu/wine/i386-windows/winmm.dll)
# CC picks another compiler (default i686-w64-mingw32-gcc); CFLAGS_EXTRA adds flags (a private toolchain's -B, -I, -L).
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
cc=${CC:-i686-w64-mingw32-gcc}
# shellcheck disable=SC2206
flags=(-O2 -Wall -Wextra -Wno-unknown-pragmas -static-libgcc ${CFLAGS_EXTRA:-})
mkdir -p out
"$cc" "${flags[@]}" -shared -o out/winmm.dll mxhook.c winmm.def
"$cc" "${flags[@]}" -o out/sleepfix_test.exe sleepfix_test.c
"$cc" "${flags[@]}" -o out/spinfix_test.exe spinfix_test.c -lwinmm
"$cc" "${flags[@]}" -o out/msgguard_test.exe msgguard_test.c
if [ "${1:-}" = orig ]; then
    src=${2:-/usr/lib/i386-linux-gnu/wine/i386-windows/winmm.dll}
    # Wine marks its own DLLs "Wine builtin DLL" and would look for a builtin named winmm_orig; the hook forwards every
    # winmm export to winmm_orig.dll, so the copy must load as an ordinary DLL
    python3 - "$src" out/winmm_orig.dll <<'PY'
import sys
b = open(sys.argv[1], 'rb').read()
assert b.count(b'Wine builtin DLL\0') == 1, 'not a Wine builtin DLL'
open(sys.argv[2], 'wb').write(b.replace(b'Wine builtin DLL\0', b'WCCF renamed DLL\0'))
PY
fi
sha256sum out/*
