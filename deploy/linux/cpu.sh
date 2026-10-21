#!/bin/sh
# CPU share of control_Release.exe and the four match engines over 30 s (percent of one core).
for p in $(pgrep -f 'control_Release|match_Release'); do echo "$p $(cut -d' ' -f14,15 /proc/"$p"/stat)"; done > /tmp/cpu.a
sleep 30
while read -r p u s; do
  # shellcheck disable=SC2046 # the two stat fields become $1 and $2
  set -- $(cut -d' ' -f14,15 /proc/"$p"/stat)
  echo "$(tr '\0' ' ' < /proc/"$p"/cmdline | grep -o '[a-z]*_Release.exe' | head -1) pid=$p cpu=$(( ($1 + $2 - u - s) * 100 / 30 / $(getconf CLK_TCK) ))%"
done < /tmp/cpu.a
