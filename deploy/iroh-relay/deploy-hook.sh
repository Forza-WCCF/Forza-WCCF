#!/bin/sh
# certbot deploy hook: copy the relay's certificate where the relay user can read it; cert_mode "Reloading" picks it up.
# RELAY_NAME: the relay's DNS name (install.sh writes it into the installed copy).
set -eu
RELAY_NAME="${RELAY_NAME:-relay.example.org}"
src=/etc/letsencrypt/live/$RELAY_NAME
dst=/etc/forza/relay
install -d -m 750 -o root -g irohrelay "$dst"
install -m 644 -o root -g irohrelay "$src/fullchain.pem" "$dst/fullchain.pem"
install -m 640 -o root -g irohrelay "$src/privkey.pem" "$dst/privkey.pem"
