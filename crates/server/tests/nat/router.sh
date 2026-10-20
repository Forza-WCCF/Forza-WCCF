#!/bin/sh
# A home router: NAT on the WAN side, netem delay DELAY_MS/2 (and LOSS %) on its WAN egress, so the two homes see
# each other DELAY_MS apart (round trip) and the server DELAY_MS/2.
#   cone        MASQUERADE + every inbound UDP forwarded to the PC (endpoint-independent mapping and filtering)
#   restricted  MASQUERADE alone (Linux keeps the port: endpoint-independent mapping, port-restricted filtering)
#   symmetric   MASQUERADE --random-fully (a new port per destination)
set -eu
WAN=$(ip -o -4 addr show | awk '$4 ~ /^10\.99\./ {print $2}')
ip route del default 2>/dev/null || true # Docker's, through the host: a home reaches only the harness's WAN
case "$NAT" in
  cone)
    iptables -t nat -A POSTROUTING -o "$WAN" -j MASQUERADE
    iptables -t nat -A PREROUTING -i "$WAN" -p udp -j DNAT --to-destination "$LAN_CLIENT" ;;
  restricted) iptables -t nat -A POSTROUTING -o "$WAN" -j MASQUERADE ;;
  symmetric) iptables -t nat -A POSTROUTING -o "$WAN" -j MASQUERADE --random-fully ;;
  *) echo "NAT=$NAT: cone, restricted or symmetric" >&2; exit 2 ;;
esac
if [ "$DELAY_MS" != 0 ] || [ "$LOSS" != 0 ]; then
  tc qdisc replace dev "$WAN" root netem delay "$((DELAY_MS / 2))ms" loss "$LOSS%"
fi
echo "router $NAT on $WAN, delay $((DELAY_MS / 2)) ms each way out, loss $LOSS %"
exec sleep infinity
