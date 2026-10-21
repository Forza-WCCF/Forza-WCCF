#!/bin/sh
# Install the Forza-WCCF iroh relay (docs/SELF-HOSTING.md, "The iroh relay"). Run as root from this folder:
#   sh install.sh relay.example.org PATH/TO/iroh-relay
# Needs: a DNS A (and AAAA) record for the name pointing at this host, certbot, TCP 80 free for certbot's HTTP-01
# check (or get the certificate your own way, put fullchain.pem and privkey.pem in /etc/forza/relay/ and skip certbot
# with CERTBOT=0), and the firewall lines for TCP 8443 and UDP 7842 (deploy/linux/nftables.conf).
set -eu
cd "$(dirname "$0")"
name=${1:?usage: install.sh RELAY_NAME IROH_RELAY_BINARY}
bin=${2:?usage: install.sh RELAY_NAME IROH_RELAY_BINARY}

install -m 755 "$bin" /usr/local/bin/iroh-relay
id irohrelay >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin irohrelay
install -d -m 755 /etc/forza
install -m 644 iroh-relay.toml /etc/forza/iroh-relay.toml
sed "s/^RELAY_NAME=.*/RELAY_NAME=\"$name\"/" deploy-hook.sh > /etc/forza/relay-deploy-hook.sh
chmod 755 /etc/forza/relay-deploy-hook.sh

if [ "${CERTBOT:-1}" = 1 ]; then
    # HTTP-01 on TCP 80 for the moment of the check; renewals run the hook, which copies the new files
    certbot certonly -n --agree-tos --register-unsafely-without-email --standalone \
        --deploy-hook /etc/forza/relay-deploy-hook.sh -d "$name"
    /etc/forza/relay-deploy-hook.sh
fi

install -m 644 forza-iroh-relay.service /etc/systemd/system/forza-iroh-relay.service
systemctl daemon-reload
systemctl enable --now forza-iroh-relay
sleep 2
systemctl --no-pager --lines=15 status forza-iroh-relay
ss -ltnup | grep -E ':(8443|7842|3340)\b'
echo "check from any PC: curl https://$name:8443/  (answers 200 \"Iroh Relay\")"
