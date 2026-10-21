#!/bin/sh
# Base hardening of a fresh Debian host for a Forza-WCCF server (docs/SELF-HOSTING.md). Run as root from this folder.
# The firewall reverts by itself in 180 s: once a NEW SSH login works, run `systemctl stop nft-revert.timer`.
set -eu
cd "$(dirname "$0")"
install -m 0644 nftables.conf /etc/nftables.conf
nft -c -f /etc/nftables.conf
systemd-run --unit=nft-revert --on-active=180 /usr/sbin/nft flush ruleset
nft -f /etc/nftables.conf
systemctl enable nftables
mkdir -p /etc/systemd/resolved.conf.d /etc/systemd/journald.conf.d
if systemctl is-active -q systemd-resolved; then  # LLMNR answers on TCP/UDP 5355 on every interface
    printf '[Resolve]\nLLMNR=no\nMulticastDNS=no\n' > /etc/systemd/resolved.conf.d/10-forza.conf
    systemctl restart systemd-resolved
fi
printf '[Journal]\nSystemMaxUse=500M\n' > /etc/systemd/journald.conf.d/10-forza.conf
systemctl restart systemd-journald
printf 'X11Forwarding no\nMaxAuthTries 3\n' > /etc/ssh/sshd_config.d/11-forza.conf
sshd -t && systemctl reload ssh
printf 'net.core.default_qdisc = fq\nnet.ipv4.tcp_congestion_control = bbr\n' > /etc/sysctl.d/90-forza.conf
sysctl -q --system
echo "now log in again in a new SSH session, then: systemctl stop nft-revert.timer"
