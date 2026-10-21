# deploy

Templates for running the online pieces on a Linux host. The order to use them in, and what each needs:
`docs/SELF-HOSTING.md`. Every path assumes `FORZA_HOME=/srv/forza-wccf` and the service user `wccf`; change both
in the units if you pick others. Replace the placeholders (`203.0.113.10`, `relay.example.org`) with your own.

## linux/

| File | Where it goes | What |
|---|---|---|
| `harden.sh` | run once as root | installs `nftables.conf` with a 180 s auto-revert, turns LLMNR off, caps the journal, tightens sshd |
| `nftables.conf` | `/etc/nftables.conf` | inbound policy drop; SSH and the game ports (delete the lines of a piece you do not run) |
| `wccf-1011.sh` | `$FORZA_HOME/wccf-1011.sh` | 2010-11 `start`, `stop`, `status`: the supervisor, `forza-server all`, the projector relay |
| `wccf-1011.service` | `/etc/systemd/system/` | runs `wccf-1011.sh` as `wccf` |
| `wccf-1011-restart.service`, `.timer` | `/etc/systemd/system/` | a daily restart (Wine leaks slowly) |
| `ctrl_user_option.conf` | `$FORZA_HOME/1011/runtime/local/` | control's settings for a server (`WAIT_PROJECTOR=0`, no DHCP, any address) |
| `guard_imports.py` | run once on the runtime copy | makes the guarded `control_Release.exe` (imports `wccfnet.dll` instead of `IPHLPAPI.DLL`) |
| `forza-1718.service` | `/etc/systemd/system/` | 2017-18 `forza-server all`: desk + lobby, game server, VS reflect |
| `forza-1718-backup.service`, `.timer` | `/etc/systemd/system/` | nightly `sqlite3 .backup` of `gamesvr.db`, 14 days kept |
| `smoke.py` | run anywhere | the 2010-11 desk and projector relay on loopback ports: 5 checks, no game |
| `lobby_check.py` | run from a player's PC | asks a desk for its `LOBBY`; `--room` also opens and closes a VS room (2017-18) |
| `precheck.sh` | run as root before an update | is anyone connected or in the game, what runs, hashes of what an update replaces |
| `cpu.sh` | run as root | CPU of control and the four match engines over 30 s |
| `Dockerfile.forza-server` | build machine | `forza-server` for another architecture (e.g. arm64) |

## iroh-relay/

| File | Where it goes | What |
|---|---|---|
| `install.sh` | run once as root | installs the relay binary, user, config, certificate (certbot) and unit |
| `iroh-relay.toml` | `/etc/forza/iroh-relay.toml` | HTTPS relay TCP 8443, QUIC address discovery UDP 7842, 256 KB/s per client |
| `forza-iroh-relay.service` | `/etc/systemd/system/` | runs the relay as `irohrelay` |
| `deploy-hook.sh` | `/etc/forza/relay-deploy-hook.sh` | certbot's renewal hook: copies the certificate where the relay reads it |
