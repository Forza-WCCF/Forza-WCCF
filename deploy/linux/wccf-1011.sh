#!/usr/bin/env bash
# The 2010-11 server: the supervisor (Xvfb + Wine control + 4 match engines), then `forza-server all` (seat desk
# TCP 20030, match relay UDP 20040, log inbox TCP 20050) and the projector relay (TCP 20031).
#   start  - supervisor first, then the two services (they read control's log, which the supervisor names)
#   stop   - the services, then the supervisor
#   status - supervisor status, the services' pids, the listening ports
# FORZA_HOME (default /srv/forza-wccf) holds 1011/ (the supervisor tree, crates/linux-host/README.md) and relay/
# (forza-server, projector_relay.py, data/). Run as the service user (wccf-1011.service does).
set -euo pipefail
H="${FORZA_HOME:-/srv/forza-wccf}"
D="$H/relay"
S="$H/1011"
SUP="$S/target/release/wccf-linux-server"
LOGS="$D/data/logs"
RUN="$D/run"
LIFE="${LIFE:-90000}"                       # seconds each service lives (25 h; the daily restart comes first)
SERVICES="forza_server projector_relay"
mkdir -p "$LOGS" "$RUN"

exe_of() {  # the command line that runs a service
    case "$1" in
    forza_server) echo "$D/forza-server --edition 2010-11 all $LIFE $LOGS" ;;
    projector_relay) echo "python3 -u $D/projector_relay.py $LIFE $LOGS/projector_relay.txt" ;;
    esac
}

pid_of() {  # a pid file's pid, only if that process is still our service
    local f="$RUN/$1.pid" pid pat
    case "$1" in
    forza_server) pat="$D/forza-server " ;;
    projector_relay) pat="$D/projector_relay\.py" ;;
    esac
    [ -f "$f" ] || return 1
    pid=$(cat "$f")
    tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -qE "$pat" && echo "$pid"
}

case "${1:-status}" in
start)
    grep -q '^WAIT_PROJECTOR=0' "$S/runtime/local/ctrl_user_option.conf" || { echo "REFUSED: WAIT_PROJECTOR is not 0"; exit 1; }
    [ -x "$D/forza-server" ] || { echo "REFUSED: $D/forza-server is missing"; exit 1; }
    [ -f "$D/projector_relay.py" ] || { echo "REFUSED: $D/projector_relay.py is missing"; exit 1; }
    WCCF_ROOT="$S" WCCF_EXTERNAL_DESK=1 "$SUP" start
    game_log="$(sed -n 's/.* logs=\([^ ]*\).*/\1/p' "$S/run/ready")/game.log"
    [ -f "$game_log" ] || { echo "control's log $game_log is missing"; exit 1; }
    echo "control log: $game_log"
    for name in $SERVICES; do
        if pid_of "$name" >/dev/null; then echo "$name already running"; continue; fi
        # shellcheck disable=SC2046
        WCCF_CONTROL_LOG="$game_log" setsid nohup $(exe_of "$name") > "$LOGS/run_$name.txt" 2>&1 < /dev/null &
        echo $! > "$RUN/$name.pid"
        sleep 1
        # shellcheck disable=SC2015 # echo does not fail, so the || branch only runs when pid_of fails
        pid_of "$name" >/dev/null && echo "$name: pid $!" || { echo "$name did not start:"; tail -3 "$LOGS/run_$name.txt"; }
    done
    ;;
stop)
    for name in $SERVICES; do
        if pid=$(pid_of "$name"); then kill "$pid" && echo "$name stopped (pid $pid)"; fi
        rm -f "$RUN/$name.pid"
    done
    WCCF_ROOT="$S" "$SUP" stop || true
    ;;
status)
    WCCF_ROOT="$S" "$SUP" status || true
    for name in $SERVICES; do
        echo "$name: $(pid_of "$name" || echo 'not running')"
    done
    ss -ltnup 2>/dev/null | grep -E ':(20002|20030|20031|20040|20050)\b' || echo "no game/service ports listening"
    ;;
*) echo "usage: $0 start|stop|status"; exit 2 ;;
esac
