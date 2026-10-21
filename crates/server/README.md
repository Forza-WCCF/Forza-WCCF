# forza-server

One program for every online service of both editions. Std threads and blocking I/O; no tokio or iroh.

    forza-server [--edition 2010-11|2017-18] <service> [flags] [positional]

| Service | What | Port (flag, else env, else default) |
|---|---|---|
| `desk [LIFE] [LOG_FILE]` | seat desk, `LOBBY` | TCP `--port`, `WCCF_BROKER_PORT`, 20030 |
| `relay [LIFE] [LOG_FILE]` | in-shop match relay | UDP `--port`, `WCCF_RELAY_PORT`, 20040 |
| `inbox LIFE [LOG_FILE] [FOLDER]` | log inbox (2010-11) | TCP `--port`, `WCCF_INBOX_PORT`, 20050 |
| `gmsvr` | 2017-18 game server, VS matcher (runs until stopped) | TCP `--port`, 20060 |
| `reflect [LIFE] [LOG_FILE]` | 2017-18 VS reflect alone | UDP `--port`, 20070 |
| `all [LIFE] [LOGS_DIR]` | desk + relay + (2010-11) inbox or (2017-18) gmsvr | `--desk-port`, `--relay-port`, `--inbox-port`, `--gmsvr-port` |

- `LIFE` is in seconds (default 12 h); exit 0 = ran its time, 1 = a port could not be bound.
- `gmsvr` and `reflect` need `--edition 2017-18`. The game server binds `WCCF_BIND`, else 127.0.0.1, and serves only
  loopback and `--allow`/`--allow-file` addresses unless `--allow-any`. Its switches (`--store`, `--game-data`,
  `--skip-tutorial`, `--dump`, `--vs-public-ip` + `--reflect-port`, `--vslink-port`) are the same under `all`.
- `all --edition 2017-18` runs the game server with the desk and the relay; with VS on, both use one matcher, so the
  desk's VS rooms pair on the game server (below). `--no-gmsvr` leaves the game server out (the Launcher's server
  mode, whose PC runs its own stand-in).
- Both editions on one host: give the second one its own ports, e.g.
  `forza-server --edition 2017-18 all --desk-port 20032 --relay-port 20042 --gmsvr-port 20060`.
- The store is SQLite in WAL mode with `synchronous=NORMAL` (`gamesvr.db` plus `-wal` and `-shm` beside it); back it
  up with `sqlite3 gamesvr.db ".backup FILE"`, not a file copy.

- The projector hold no longer touches the firewall and is not enforced (`HOLD` stays `HOLD 0`). A Windows server that ran an
  older version: remove the leftover rule by hand with `netsh advfirewall firewall delete rule name="WCCF hold projector box"`.

Code: `src/` holds the desk, relay and inbox; the game server, the matcher and the reflect service are
`forza::online::{gamesvr, vs}`, shared with the Launcher's local stand-in. Builds: `cargo build --release -p forza-server`
(Linux), `cargo xtask build` (Windows, `programs\forza-server.exe` in the 2010-11 kit).

## VS rooms (2017-18)

The desk's words (`forza::online::lobby` has the client side; one line each way, the desk hangs up):

| Ask | Answer |
|---|---|
| `CREATE vs` | `ROOM id code t 1` (this PC on seat 1) or `FULL` (200 rooms) |
| `CREATE vs open` (QUICK MATCH) | `ROOM id code t 2` on seat 2 of the oldest open room still waiting, else a new open room (`… t 1`) |
| `RESERVE room-id [2]` | `TICKET t 2`, `FULL` (two players), `TAKEN` (the seat asked for), `CLOSED` (no such room) |
| `READY id t` | `OK` or `GONE` |
| `CLOSE id t` | `OK` or `GONE`; the host's ticket closes the room and drops the guest's, a guest's leaves its seat |
| `HEARTBEAT t`, `RELEASE t` | as for shop seats; the host's `RELEASE` closes the room |

The client reads a `ROOM id code t` without the seat (a desk before the field) as seat 1. A room ticket lives 60 s past its last `HEARTBEAT` (the shop lease); the host's lease ending closes the room.
`RESERVE` and `CREATE` together: 20 per address per 60 s, else `ERR slow`. `LOBBY` lists each room as `{"id",
"code","players","max":2,"kind":2,"occupants":[{"seat":1,"club":"A7K2","ready":true},…]}`: id (8 hex), code and club
ids (4 of `ABCDEFGHJKMNPQRSTUVWXYZ23456789`) are random and drawn apart from the tickets; no address, ticket, card or
Aime number. The desk's log names rooms by code only.

**Pairing.** The desk tells the shared matcher which room each live ticket sits in. The run's vslink tells the game
server which ticket it holds, in its hello (900); its token registration (902) ties that hello's EndpointId to the
token the control then sends as the seat's address in MATCHING_ENTRY 243 (+8 address, +12 port). On 245 the matcher
pairs an entry only within its pool: no ticket = M3's open pool (round, rematch history and the rest as before); a
room's ticket = the other PC of that room, across rounds and history; a ticket the desk no longer holds (room
closed, lease ended) = nobody, never the open pool. Rooms need `all` (desk and game server in one process, VS on);
a desk without the matcher (2010-11, `desk` alone, `--no-gmsvr`) holds no rooms: `CREATE`, `READY` and `CLOSE`
answer `ERR` as unknown words, `RESERVE room-id` answers `CLOSED`.

Two homes behind their own NAT, 0-150 ms apart, through the lobby, 243-246 and vslink: the Docker harness in
`tests/nat/` (opt-in, `tests/nat/run.sh`).

**What the run sends** (the Launcher side, B3):
1. The lobby's ticket reaches the run as today, `data\panel.txt` `ticket=t` (`play()` writes it); the run reads it
   and takes the line out, as `take_seat` does. A VS room run does not claim a desk seat (`WANT … TICKET` is for
   shops): it is a one-PC shop on this PC, with `club_server` = the lobby server's host and its game server port, so
   the control's 243s reach the game server whose matcher the desk feeds.
2. vslink stays on (`vs_link`), and its hello (900, sent at start and every 60 s) carries the ticket: payload
   `forza::online::vs::hello_payload(&endpoint_id, Some(t))`, 52 bytes = +4..+36 the EndpointId, +36..+52 the
   ticket in ASCII, NUL-padded (desk tickets are 16 hex characters). Without a ticket, the 36-byte hello as today
   (open pool). Nothing else changes: the 902 registration and the 243 the control sends are as now.
3. The Launcher keeps `HEARTBEAT t` every 20 s while the run lasts (`Held`'s beat already does), so the room and its
   pairing outlive the 60 s lease; leaving, and the run's end, send `CLOSE` (seat 1) or `RELEASE` (seat 2). Only a
   VS room's ticket goes into a 2017-18 run (SHOPS stays greyed there).

