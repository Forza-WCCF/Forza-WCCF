#!/bin/bash
# press.sh BUTTON... [wait SECONDS] [cap NAME] - from WSL: tap seat 1's cabinet buttons through the I/O board stand-in's
# input file (as _keys_seat1.py writes it), 0.35 s held each; "cap NAME" captures the game windows (`cargo xtask cap`,
# PrintWindow: the game's own windows only, never the screen) into $WCCF1718_RUNTIME/evidence/NAME.
# Buttons: START PRESS SHOOT UP DOWN LEFT RIGHT SERVICE KEEPER DATA KEYPLAYER COIN.
HERE=$(cd "$(dirname "$0")" && pwd)
RT=${WCCF1718_RUNTIME:-<runtime>}
F=$RT/kit/scripts/_jvs_seat1_input.txt
declare -A P1=([START]=0x80 [PRESS]=0x02 [SHOOT]=0x01 [UP]=0x20 [DOWN]=0x10 [LEFT]=0x08 [RIGHT]=0x04 [SERVICE]=0x40)
declare -A P2B0=([KEEPER]=0x02) P2B1=([DATA]=0x80 [KEYPLAYER]=0x40)
COIN=$(grep -o 'coin1=[0-9]*' "$F" 2>/dev/null | cut -d= -f2); COIN=${COIN:-0}
w(){ printf '# press.sh\nsys=0x00\np1=%s 0x00\np2=%s %s\ncoin1=%s\ncoin2=0\n' "$1" "$2" "$3" "$COIN" > "$F.tmp" && mv "$F.tmp" "$F"; }
while [ $# -gt 0 ]; do
  b=$1; shift
  case $b in
    cap) (cd "$HERE/.." && cargo xtask cap "$RT" "$RT/evidence/$1"); shift; continue;;
    wait) sleep "$1"; shift; continue;;
    COIN) COIN=$((COIN+1)); w 0x00 0x00 0x00; sleep 0.3; continue;;
  esac
  w "${P1[$b]:-0x00}" "${P2B0[$b]:-0x00}" "${P2B1[$b]:-0x00}"; sleep 0.35; w 0x00 0x00 0x00; sleep 0.5
done
