#!/usr/bin/env bash
# The NAT matrix (README.md): every NAT pair x every delay, then the open pool and the cross-room negative.
#   NATS="cone:restricted restricted:restricted symmetric:symmetric"  DELAYS="0 50 100 150"  LOSS=0
#   NAT_WORK=dir (results, logs)  NAT_NO_BUILD=1 (use the binaries already in target/release)
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../.." && pwd)
NATS=${NATS:-"cone:restricted restricted:restricted symmetric:symmetric"}
DELAYS=${DELAYS:-"0 50 100 150"}
export LOSS=${LOSS:-0}
NAT_WORK=${NAT_WORK:-$ROOT/target/nat-$(date +%Y%m%d)}
RUN=$NAT_WORK/run-$(date +%H%M%S)
mkdir -p "$RUN"
export BIN_DIR=$ROOT/target/release SHARED=$RUN/shared
# a clean client config: the host's may name a credential helper that is not there (docker-credential-wincred.exe)
export DOCKER_CONFIG=$NAT_WORK/dockercfg
mkdir -p "$DOCKER_CONFIG"; [ -f "$DOCKER_CONFIG/config.json" ] || echo '{}' >"$DOCKER_CONFIG/config.json"
dc() { docker compose -f "$HERE/compose.yaml" "$@"; }

if [ -z "${NAT_NO_BUILD:-}" ]; then
  (cd "$ROOT" && cargo build --release -p forza-server && cargo build --release -p vslink --example nat_peer)
fi
dc build -q server
trap 'dc down -t 0 >/dev/null 2>&1 || true' EXIT

# cell NAME NAT_A NAT_B DELAY ROLE_A ROLE_B [nat_peer flags]
cell() {
  local name=$1 dir=$RUN/$1 na=$2 nb=$3 d=$4 ra=$5 rb=$6
  shift 6
  mkdir -p "$dir"; rm -rf "$SHARED"; mkdir -p "$SHARED"
  NAT_A=$na NAT_B=$nb DELAY_MS=$d dc up -d --force-recreate --quiet-pull >"$dir/up.log" 2>&1 || { cat "$dir/up.log" >&2; return 1; }
  sleep 1
  local t0=$SECONDS
  dc exec -T client-a /opt/forza/examples/nat_peer client --role "$ra" "$@" >"$dir/a.json" 2>"$dir/a.log" &
  local pa=$!
  dc exec -T client-b /opt/forza/examples/nat_peer client --role "$rb" "$@" >"$dir/b.json" 2>"$dir/b.log" &
  wait $pa || true; wait $! || true
  dc logs --no-color server nat-a nat-b >"$dir/server.log" 2>&1
  jq -c -n --arg cell "$name" --arg nat "$na+$nb" --argjson delay "$d" --argjson secs $((SECONDS - t0)) \
    --slurpfile a "$dir/a.json" --slurpfile b "$dir/b.json" '
    ($a[0] // {}) as $a | ($b[0] // {}) as $b |
    {cell: $cell, nat: $nat, delay_ms: $delay, secs: $secs,
     paired: (($a.paired // false) and ($b.paired // false) and $a.opp == $b.token and $b.opp == $a.token
              and $a.seed == $b.seed and $a.home != $b.home),
     paired_any: (($a.paired // false) or ($b.paired // false)),
     link: (($a.link_up // false) and ($b.link_up // false)),
     path: "\($a.path // "-")/\($b.path // "-")",
     blob: (($a.block_rx_ok // false) and ($b.block_rx_ok // false)),
     pair_ms: ([$a.pair_ms, $b.pair_ms] | max), link_ms: ([$a.link_ms, $b.link_ms] | max),
     ping_ms: "\($a.ping_median_ms // "-")/\($b.ping_median_ms // "-")",
     a: $a, b: $b}' | tee -a "$RUN/results.jsonl" >/dev/null
}

for nats in $NATS; do
  for d in $DELAYS; do
    echo "== ${nats/:/+} ${d} ms" >&2
    cell "room-${nats/:/-}-$d" "${nats%%:*}" "${nats##*:}" "$d" host guest
  done
done
echo "== open pool, cross-room" >&2
cell open-pool restricted restricted 50 open open
cell cross-room restricted restricted 50 solo solo --pair-secs 20

# the gate: room and open cells pair, link up and swap the block both ways; the cross-room cell never pairs
jq -r -s '
  ["cell","paired","link","path a/b","block both ways","pair ms","link ms","ping ms a/b","PASS"],
  (.[] | [.cell, .paired, .link, .path, .blob, .pair_ms, .link_ms, .ping_ms,
          (if .cell == "cross-room" then (.paired_any | not) else (.paired and .link and .blob) end)])
  | @tsv' "$RUN/results.jsonl" | column -t -s $'\t' | tee "$RUN/results.txt"
if jq -e -s 'all(if .cell == "cross-room" then (.paired_any | not) else (.paired and .link and .blob) end)' \
  "$RUN/results.jsonl" >/dev/null; then
  echo "NAT gate: PASS ($RUN)"
else
  echo "NAT gate: FAIL ($RUN)"; exit 1
fi
