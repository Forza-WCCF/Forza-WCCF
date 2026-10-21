#!/bin/sh
# Read-only look at the server before an update or restart: is anyone playing, what runs, which files an update replaces.
#   sudo sh precheck.sh          exit 0 = nobody connected, 1 = a player is connected (or in the game)
# Changes nothing. FORZA_HOME: as in wccf-1011.sh.
set -u
H="${FORZA_HOME:-/srv/forza-wccf}"
R=$H/relay; S=$H/1011; Y=$H/1718
rc=0
echo "== $(date '+%F %T %Z') $(hostname)"

echo "== remote peers on the game ports (loopback is the projector relay's own link to control)"
peers=$(ss -Htn state established '( sport = :20002 or sport = :20030 or sport = :20031 or sport = :20032 or sport = :20060 )' |
        awk '{print $3, $4}' | grep -v -E '(^| )(127\.0\.0\.1|\[::1\]|\[::ffff:127\.0\.0\.1\]):' || true)
if [ -n "$peers" ]; then echo "$peers"; echo "PLAYER CONNECTED"; rc=1; else echo "none"; fi

echo "== seats in the game now (control's game.log: ACCEPT minus DISCONNECT)"
logs=$(sed -n 's/.* logs=\([^ ]*\).*/\1/p' "$S/run/ready" 2>/dev/null)
if [ -f "$logs/game.log" ]; then
  seats=$(awk '/sateID=[0-9]+/ && /client\[/ {match($0,/sateID=[0-9]+/); s=substr($0,RSTART+7,RLENGTH-7)}
               /\+\+ACCEPT\+\+/ {if (s!="") inn[s]=1}
               /---DISCONNECT---/ {match($0,/sateID=[0-9]+/); delete inn[substr($0,RSTART+7,RLENGTH-7)]}
               END {for (k in inn) if (k+0>=1) printf "%s ", k}' "$logs/game.log")
  echo "log: $logs/game.log; player seats: ${seats:-none}"
  [ -z "$seats" ] || { echo "PLAYER IN THE GAME"; rc=1; }
  grep -E '^state ' "$logs/game.log" | tail -1
else
  echo "no game.log (control not started?)"
fi

echo "== services"
systemctl is-active wccf-1011 forza-1718 forza-1718-backup.timer forza-iroh-relay nftables 2>/dev/null | paste -sd' '
sudo -u wccf FORZA_HOME="$H" "$H/wccf-1011.sh" status 2>&1 | tail -6
ss -ltnup | grep -E ':(20002|20030|20031|20032|20040|20042|20050|20060|20061|8443|7842)\b' || true

echo "== files an update replaces (sha256)"
sha256sum "$H/wccf-1011.sh" "$R/forza-server" "$R/projector_relay.py" "$Y/forza-server" /etc/nftables.conf \
  /etc/systemd/system/forza-1718*.service /etc/systemd/system/forza-1718*.timer 2>/dev/null
ls "$Y/data" 2>/dev/null
echo "== precheck: $([ $rc = 0 ] && echo 'OK, nobody connected' || echo "STOP (rc $rc)")"
exit $rc
