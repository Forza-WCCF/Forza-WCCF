# Running your own Forza-WCCF server

Every online piece of Forza-WCCF can run on your own Linux host. The project runs public instances
(`lobby.wccf.it` and `relay.wccf.it`), but nothing needs them: a Launcher pointed at your host uses only your host.
This guide sets up the pieces in order. The templates are in `deploy/` (`deploy/README.md` lists them). Their
paths assume `FORZA_HOME=/srv/forza-wccf` and a service user `wccf`.

## 1. The pieces

| Piece | For | Program | Ports |
|---|---|---|---|
| 2010-11 shop server | 2010-11 online play | the original control and four match engines under Wine, started by `wccf-linux-server` (`crates/linux-host`) with the server hook `winmm.dll` (`native/2010-11/server-hook`) | TCP 20002 |
| 2010-11 seat desk, match relay, log inbox | seats, the lobby, in-shop matches, player logs | `forza-server --edition 2010-11 all` (`crates/server`) | TCP 20030, UDP 20040, TCP 20050 |
| 2010-11 projector relay | the projector (big screen) on every player's PC | `projector_relay.py` (`crates/server/python`), the one service still in Python | TCP 20031 |
| 2017-18 server | the lobby, VS rooms, clubs and saves (SQLite), VS pairing | `forza-server --edition 2017-18 all` | TCP 20032, TCP 20060, UDP 20061 |
| iroh relay | 2017-18 direct VS when two PCs cannot reach each other | upstream `iroh-relay` | TCP 8443, UDP 7842 |

Run any subset. A 2017-18-only host needs no Wine and no game files beyond the two data files in section 6.

## 2. Host

- Debian 13 (or another distribution with Wine 10, Xvfb and systemd), x86-64. Wine runs the 32-bit original
  programs, so the 2010-11 server needs `wine32:i386`.
- 2 vCPU and 4 GB RAM carry the 2010-11 server (about 0.9 GB in use, control at about 6 % of a core idle with the
  spin fix, the engines 6-8 % each) and the 2017-18 server (under 512 MB) together.
- About 5 GB of disk for the 2010-11 game folder and the Wine prefix.
- A public IPv4 address. A DNS name is optional for the game servers and required for the iroh relay's certificate.

## 3. What you bring

The repository and its builds ship no game files. You need your own copies:

- 2010-11: the game folder (the Rev D programs and their data), the same one the Forza-WCCF Launcher sets up on a
  player's PC. `guard_imports.py` checks control against the original's pinned hash.
- 2017-18: `prog_data/player_data/player_data.bin` (the 2D-code cards) and `prog_data/cpu_team/cpu_50.dat` (the club
  a new player starts with when the tutorial is skipped), from your game folder.

Keep them on the server with mode 600. Never put them, the databases or the logs in a repository.

## 4. Build

On any Linux machine with the repository's Rust toolchain (`rust-toolchain.toml`). The server's glibc must be the
same or newer than the build machine's; build on the server itself when unsure.

    cargo build --release -p forza-server -p linux-host
    # target/release/forza-server, target/release/wccf-linux-server

For the 2010-11 server also (`cargo install cargo-xwin` once; it fetches the Windows CRT and SDK):

    cargo xwin build --release -p linux-host --target i686-pc-windows-msvc --bin win32-helper --bin check-crt
    # target/i686-pc-windows-msvc/release/win32-helper.exe, check-crt.exe
    cargo xtask build
    # the network guard: target/kit/2017-18/bin/wccfnet.dll, overlay/netguard.dll, source/build/test_wccfnet.exe
    native/2010-11/server-hook/build.sh          # needs gcc-mingw-w64-i686
    # native/2010-11/server-hook/out/winmm.dll and the three self-tests

The iroh relay is upstream's binary at the iroh version in `Cargo.lock` (1.4.0 now), built the same way:

    cargo install --locked iroh-relay@1.4.0 --features server

Another architecture for `forza-server`: `deploy/linux/Dockerfile.forza-server`.

## 5. Base setup (root)

    adduser --system --group --home /srv/forza-wccf wccf
    apt install nftables python3 sqlite3 zstd curl
    sh deploy/linux/harden.sh       # then open a NEW SSH session and run: systemctl stop nft-revert.timer

`harden.sh` installs `deploy/linux/nftables.conf`. Delete its lines for the pieces you do not run first (section 9).

## 6. The 2017-18 server

    install -d -o wccf -g wccf -m 750 /srv/forza-wccf/1718/{data,logs,backup} /srv/forza-wccf/1718/game-data/prog_data/{player_data,cpu_team}
    install -o wccf -g wccf -m 755 target/release/forza-server /srv/forza-wccf/1718/
    install -o wccf -g wccf -m 600 player_data.bin /srv/forza-wccf/1718/game-data/prog_data/player_data/
    install -o wccf -g wccf -m 600 cpu_50.dat /srv/forza-wccf/1718/game-data/prog_data/cpu_team/
    install -o wccf -g wccf -m 644 /dev/null /srv/forza-wccf/1718/allow.txt
    cp deploy/linux/forza-1718*.service deploy/linux/forza-1718-backup.timer /etc/systemd/system/
    # in forza-1718.service: --vs-public-ip = this host's public IPv4
    systemctl daemon-reload && systemctl enable --now forza-1718 forza-1718-backup.timer

- The game server has no sign-in. It serves loopback and the addresses in `allow.txt` (one IP or CIDR per line, `#`
  comments, re-read when the file changes). The desk, the lobby and the VS reflect answer anyone. To serve everyone,
  replace `--allow-file …` with `--allow-any` in the unit.
- Check from a player's PC: `python3 deploy/linux/lobby_check.py game.example.org:20032 2017-18 --room`.
- The VS options, the room words and the pairing rules: `crates/server/README.md`.

## 7. The 2010-11 server

Packages (root):

    dpkg --add-architecture i386 && apt update
    apt install wine wine32:i386 wine64 xvfb xauth fonts-liberation

The tree (`crates/linux-host/README.md` describes each part), as `wccf`:

    H=/srv/forza-wccf
    mkdir -p $H/1011/{runtime,run,logs,evidence,target/release,tools} $H/relay/data/logs
    cp -a GAME_FOLDER/. $H/1011/runtime/                       # your 2010-11 game folder
    python3 deploy/linux/guard_imports.py $H/1011/runtime          # control now imports wccfnet.dll
    cp deploy/linux/ctrl_user_option.conf $H/1011/runtime/local/
    cp native/2010-11/server-hook/out/winmm.dll $H/1011/runtime/
    native/2010-11/server-hook/build.sh orig && cp native/2010-11/server-hook/out/winmm_orig.dll $H/1011/runtime/
    cp target/kit/2017-18/bin/wccfnet.dll target/kit/2017-18/overlay/netguard.dll \
       target/kit/2017-18/source/build/test_wccfnet.exe $H/1011/runtime/
    cp target/i686-pc-windows-msvc/release/win32-helper.exe $H/1011/run/
    cp target/release/wccf-linux-server $H/1011/target/release/
    ln -s /usr $H/1011/tools/usr                                   # Xvfb and its xkb files from the system
    WINEARCH=win32 WINEPREFIX=$H/1011/wineprefix xvfb-run wineboot -i
    (cd $H/1011 && sha256sum runtime/{control_Release.exe,match_Release.exe,wccfnet.dll,netguard.dll,winmm.dll,winmm_orig.dll} \
        run/win32-helper.exe > evidence/runtime.sha256)
    cp target/release/forza-server crates/server/python/projector_relay.py $H/relay/
    cp deploy/linux/wccf-1011.sh $H/

`orig` copies Wine's own 32-bit `winmm.dll`, so run that step on a host with `wine32` installed (or copy
`/usr/lib/i386-linux-gnu/wine/i386-windows/winmm.dll` from one). The supervisor refuses to start when a file in
`evidence/runtime.sha256` changed; write the file again after each deliberate change.

Before the first start, check the services alone (no game, loopback ports):

    python3 deploy/linux/smoke.py $H/relay/forza-server $H/relay/projector_relay.py      # 5 of 5 passed

Then (root):

    cp deploy/linux/wccf-1011.service deploy/linux/wccf-1011-restart.* /etc/systemd/system/
    systemctl daemon-reload && systemctl enable --now wccf-1011 wccf-1011-restart.timer
    sudo -u wccf FORZA_HOME=/srv/forza-wccf /srv/forza-wccf/wccf-1011.sh status

Start takes about 30 s. `status` shows the supervisor ready, 4 of 4 match engines and ports 20002, 20030, 20031,
20040 and 20050 listening. Each Wine process writes `mxhook_<pid>.log` in `1011/logs/<start time>/`; control's must
say `MSGPARSE guard: on`, `SLEEP fix: on`, `SPIN fix: on`. `deploy/linux/cpu.sh` prints the CPU of control and the
engines. From a player's PC: `python3 deploy/linux/lobby_check.py game.example.org 2010-11`, and on Windows
`check-network.exe game.example.org` (`crates/linux-host`) tests TCP 20002, two seat leases and both relay directions.

## 8. The iroh relay (2017-18 VS)

Two PCs in a VS match connect directly over iroh; when no direct path works, their traffic goes through a relay.
The relay needs a DNS name with a certificate that Windows trusts.

    sh deploy/iroh-relay/install.sh relay.example.org ~/.cargo/bin/iroh-relay

`install.sh` gets a Let's Encrypt certificate with certbot's HTTP-01 check, which needs TCP 80 open for that moment
(add `tcp dport 80 accept` to the firewall while it runs). With a certificate from elsewhere, put `fullchain.pem` and
`privkey.pem` in `/etc/forza/relay/` (group `irohrelay`) and run it with `CERTBOT=0`. Renewals run
`/etc/forza/relay-deploy-hook.sh`, and the relay re-reads the files (`cert_mode = "Reloading"`). Check from any PC:
`curl https://relay.example.org:8443/` answers 200 "Iroh Relay".

A relay with a private CA works too: the players set `vs_relay_ca` (section 10) to that CA's PEM file, which then
becomes the only root the Launcher trusts for the relay.

## 9. Ports and firewall

| Port | Proto | Piece | Who connects |
|---|---|---|---|
| 22 | TCP | SSH | you |
| 20002 | TCP | 2010-11 control | players (capped per source: 16 connections, 10 new a second) |
| 20030 | TCP | 2010-11 seat desk and lobby | players |
| 20031 | TCP | 2010-11 projector relay | players |
| 20040 | UDP | 2010-11 match relay | players |
| 20050 | TCP | 2010-11 log inbox | players (SEND LOGS, crash reports) |
| 20032 | TCP | 2017-18 desk and lobby | players |
| 20060 | TCP | 2017-18 game server | players in `allow.txt` |
| 20061 | UDP | 2017-18 VS reflect | players |
| 20042 | UDP | 2017-18 match relay | nobody: keep it closed |
| 8443 | TCP | iroh relay (HTTPS) | players |
| 7842 | UDP | iroh relay (QUIC address discovery) | players |

`deploy/linux/nftables.conf` opens exactly these (not 20042). TCP 20003 (match engines) and UDP 20020 stay local.

## 10. Pointing players at your server

The Launcher's lobby asks the servers in the edition's `data\settings.json` `servers` list; without one, the
address typed under "Direct address" (if any) and then the default, `lobby.wccf.it`. Each entry is `host[:port]`;
the desk port defaults to 20030 (2010-11) or 20032 (2017-18). For 2017-18 the game server is the same host on TCP
20060.

| Setting | Where | Effect |
|---|---|---|
| `"servers": ["game.example.org"]` | `data\settings.json` | the lobby lists these servers only |
| Direct address | the Launcher's online screen | one server, typed in |
| `WCCF_LOBBY=game.example.org[:port]` | environment | replaces the default `lobby.wccf.it` |
| `"vs_relay": "https://relay.example.org:8443"` | `data\settings.json` | the iroh relay for 2017-18 VS |
| `"vs_relay_ca": "C:\\path\\ca.pem"` | `data\settings.json` | trust only this CA for the relay (with `vs_relay` or `WCCF_VS_RELAY`) |
| `WCCF_VS_RELAY=https://relay.example.org:8443` | environment | the relay; wins over `vs_relay` |
| `WCCF_UPDATE_URL=…` | environment | where the Launcher looks for updates |

## 11. Player logs

SEND LOGS (the SETTINGS panel) and the 2010-11 automatic reports after a run that went wrong go to the server the
player plays on, TCP 20050: your server, not the project's. They land in `/srv/forza-wccf/relay/data/player_logs/` as
`DATE_TIME_CODE_ADDRESS_VERSION.zip`; CODE is the 5-character code the player sees. Limits: 8 MB an upload,
`WCCF_INBOX_PER_HOUR` (6) uploads an hour per address, `WCCF_INBOX_KEEP` (1 GB) in total, the oldest go first.
Keep TCP 20050 closed if you do not want them; the Launcher then keeps a failed report and tries again at the next start.

## 12. Backups

- 2017-18: `forza-1718-backup.timer` writes `1718/backup/gamesvr-DATE.db.zst` every night (`sqlite3 .backup`, safe while the
  server runs, then zstd; 14 days kept). Copy that folder off the host. Restore: stop `forza-1718`, unpack one backup
  over the database (`zstd -d -f gamesvr-DATE.db.zst -o 1718/data/gamesvr.db`), delete `gamesvr.db-wal` and `-shm`, start.
- 2010-11: `relay/data/` (logs and player uploads). The 2010-11 server keeps no player state of its own; the
  cards and saves live on the players' PCs.
- Keep a copy of `1011/runtime/` and `evidence/runtime.sha256`; the Wine prefix can be made again.

## 13. Updating

1. `sudo sh deploy/linux/precheck.sh`: exit 0 means nobody is connected or in the game. Update only then.
2. Build the new binary, copy it beside the old one as `NAME.new`, keep the old one as `NAME.prev`, then `mv` the
   new one over it.
3. 2017-18: `systemctl restart forza-1718`. 2010-11: `systemctl restart wccf-1011`; after a new `winmm.dll` or
   `win32-helper.exe`, write `evidence/runtime.sha256` again first.
4. Check as at the first start (`status`, `lobby_check.py`). To go back, move `NAME.prev` back and restart.

## 14. Troubleshooting

| Symptom | Look at | Fix |
|---|---|---|
| `start` says a hash differs | `evidence/runtime.sha256` | a file in `runtime/` changed: put the right one back, or write the list again after a deliberate change |
| `refusing cleanup of unowned pid` | `wccf-linux-server status` | Wine lives elsewhere on this distribution: add its paths in `verify_prefix` (`crates/linux-host/src/supervisor.rs`) |
| a match engine exits with 0xC0000417 | `1011/logs/<start>/game.log` | Wine's CRT rejected control's 14-character `Match[Control]` label; the helper shortens it. Check Wine's CRT: `check-crt.exe` copied into `runtime/`, run there with the server's `WINEPREFIX` (PASS) |
| control at a full core while idle | `deploy/linux/cpu.sh`, control's `mxhook_<pid>.log` | `SPIN fix: on` should be there. If the spin fix is suspected in a crash, start with `MXHOOK_SPINFIX=0` in `wccf-1011.service` (`Environment=`); control then uses a full core idle |
| control dies with `FATAL … code=0xc0000005` | control's `mxhook_<pid>.log`: `MSGPARSE guard` and `stack esp+` lines | the guard catches the known parser fault; a stepped-aside fault logs the addresses to report. `MXHOOK_MSGGUARD=0` turns the guard off |
| the service is "active" but control is gone | `wccf-1011.sh status` | the unit is oneshot: `systemctl restart wccf-1011` |
| players get "Network, Error" on the projector | the projector relay's log, `relay/data/logs/projector_relay.txt` | the relay calls control only once a player is in; a projector that stalls 100 s is cut and may reconnect |
| 2017-18 players are refused | `1718/logs/gmsvr.txt` | add their address to `allow.txt` |
| VS falls back to a CPU team | the player's `vslink_log.txt` | the relay URL, its certificate, UDP 20061 and the relay ports |

The server runs the original programs unchanged in memory except for the hook's fixes, and the original protocols
have no authentication. Expose only the ports in section 9.
