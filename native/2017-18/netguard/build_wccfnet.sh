#!/usr/bin/env bash
# Build wccfnet.dll (+ netguard.dll) with a 32-bit mingw-w64 compiler (Debian: apt install gcc-mingw-w64-i686).
#   CC=i686-w64-mingw32-gcc ./build_wccfnet.sh
# The committed DLLs were built from these sources; rebuild after any change.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
cc=${CC:-i686-w64-mingw32-gcc}
"$cc" -O2 -Wall -Wextra -shared -o wccfnet.dll wccfnet.c wccfnet.def
"$cc" -O2 -Wall -Wextra -shared -o netguard.dll netguard.c
