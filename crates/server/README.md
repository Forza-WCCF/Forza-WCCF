# forza-server

One program for every online service of both editions. Std threads and blocking I/O; no tokio or iroh.

    forza-server [--edition 2010-11|2017-18] <service> [flags] [positional]

| Service | What | Port (flag, else env, else default) |
|---|---|---|
| `desk [LIFE] [LOG_FILE]` | seat desk, `LOBBY` | TCP `--port`, `WCCF_BROKER_PORT`, 20030 |
| `relay [LIFE] [LOG_FILE]` | in-shop match relay | UDP `--port`, `WCCF_RELAY_PORT`, 20040 |
| `gmsvr` | 2017-18 game server, VS matcher (runs until stopped) | TCP `--port`, 20060 |
| `reflect [LIFE] [LOG_FILE]` | 2017-18 VS reflect alone | UDP `--port`, 20070 |
| `all [LIFE] [LOGS_DIR]` | desk + relay + (2017-18) gmsvr | `--desk-port`, `--relay-port`, `--gmsvr-port` |

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

- No logs are collected: `REPORT` (older Launchers' problem reports) gets `NO` at once, and nothing listens on the old
  log inbox port (TCP 20050). Players save their logs (`forza::online::save_logs`) and send the ZIP themselves.
- A 2017-18 shop guest's own club (`forza::online::club`): `CLUB PUT seat aime token size` + the club's JSON ->
  `OK size` + the id map, `CLUB GET aime token stamp` -> `SAME` or `OK size stamp` + the club, `CLUB DONE aime token`
  -> `OK 0`; `NO why` for any refusal. Only with `WCCF_CLUB_DB` (the shop host's `gamesvr.db`, set by the Launcher);
  a PUT needs a seat held on this desk, at most 2 MiB.
- The projector hold no longer touches the firewall and is not enforced (`HOLD` stays `HOLD 0`). A Windows server that ran an
  older version: remove the leftover rule by hand with `netsh advfirewall firewall delete rule name="WCCF hold projector box"`.

Running a server of your own (both editions, the 2010-11 Wine host, the iroh relay, firewall, backups):
`docs/SELF-HOSTING.md`. `python/projector_relay.py` is the 2010-11 projector relay (TCP 20031), the one service not
yet in Rust: `python3 projector_relay.py LIFE [LOG_FILE]` with `WCCF_CONTROL_LOG` set to control's log.

Code: `src/` holds the desk and relay; the game server, the matcher and the reflect service are
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

## Shop rooms (2017-18)

A player's PC that hosts a shop (the Launcher's `server`, reached through its shop tunnel, `vslink::shop`) can open a
room in a public lobby. No matcher needed; they share `max_rooms` with VS rooms. One address holds at most 2 shop
rooms (`WCCF_SHOP_ROOMS_PER_ADDRESS`) and 8 VS rooms (`WCCF_ROOMS_PER_ADDRESS`; players behind one NAT share an
address), and one tunnel id one live room (`ERR host`, unless that room went quiet: below). A shop's own desk behind its tunnel (`WCCF_DESK_TUNNEL=1`,
the Launcher's 2017-18 server; loopback there is many guests, so it has no per-address limits) refuses every room
word with `ERR rooms are on the lobby` and only seats its own PCs.

| Ask | Answer |
|---|---|
| `CREATE shop ID [public]` or `CREATE shop ID public\|private NAME...` (ID = the shop tunnel's iroh EndpointId: 64 hex, a real ed25519 key; NAME the rest of the line, control characters dropped, spaces collapsed, at most 24 characters) | `ROOM id code t 1` (the host on seat 1, an 8-letter code), `FULL`, `ERR id`, `ERR host` |
| `JOIN CODE [n]` | `TICKET t n` (lowest free of 2-8, or n), `FULL`, `TAKEN`, `CLOSED` (no such code) |
| `SHOP t [ID]` | a guest's ticket (telling its own tunnel id): `HOST id`; the host's: `GUESTS id ...` (guests that told theirs); else `GONE` |
| `HEARTBEAT t`, `RELEASE t`, `CLOSE id t` | as for VS rooms: the host's `RELEASE`/`CLOSE` or lease end closes the room |

Rooms are private: the code, which the host passes on, is the only way in (`RESERVE` of a room's id answers
`CLOSED`). `JOIN` counts in the per-address `RESERVE`/`CREATE` limit; wrong codes are braked per address (10 per
10 min, then that address's `JOIN`s answer `ERR slow` for the rest of the window), under a desk-wide ceiling of 2,000
a minute (logged when it trips). A `CREATE shop` from a tunnel id whose room has had no `HEARTBEAT` for 15 s replaces
that room (a host that crashed and came back); a live room answers `ERR host`.
The key check on `CREATE shop` is about form only (a canonical ed25519 point not of small order): it proves nothing
about who holds the key, so anyone can open a room naming any id. What protects a host's tunnel is its admit list
(`vslink::shop::ShopConfig::admit` fed from `SHOP`'s `GUESTS`), which lets in only guests who joined its room. `LOBBY` lists only `public` rooms, as a `shops` row named
by its host (or "Arcade room") with x/8 and never the code, a tunnel id, a ticket or an address.

Two homes behind their own NAT, 0-150 ms apart, through the lobby, 243-246 and vslink: the Docker harness in
`tests/nat/` (opt-in, `tests/nat/run.sh`).

**What the run sends** (the Launcher side, B3):
1. The lobby's ticket reaches the run as today, `data\panel.txt` `ticket=t` (`play()` writes it); the run reads it
   and takes the line out, as `take_seat` does. A VS room run does not claim a desk seat (`WANT … TICKET` is for
   shops): it is a one-PC shop on this PC, with `club_server` = the lobby server's host and its game server port, so
   the control's 243s reach the game server whose matcher the desk feeds.
2. vslink stays on (`vs_link`), and its hello (900, sent at start and every 60 s) carries the ticket: payload
   `forza::online::vs::hello_payload(&endpoint_id, Some(t), Some(d))`, 72 bytes = +4..+36 the EndpointId, +36..+52
   the ticket in ASCII, NUL-padded (desk tickets are 16 hex characters; zeros for the open pool), +52..+72 the PC's
   det-math token (`forza::online::seat::dmath_token`, as on shop seat claims). Older vslinks send 36 bytes (open
   pool) or 52 (room), no token. The matcher pairs two entries only when their PCs' tokens are equal, a missing token
   counting as its own value (two older builds still pair, an older and a newer never do). A hello with a token
   whose room partner holds another gets 901 status 2 (`HELLO_DMATH_DIFFERS`), and the Launcher says
   `launcher.run_vs_dmath_differs` once. Nothing else changes: the 902 registration and the 243 the control sends
   are as now.
3. The Launcher keeps `HEARTBEAT t` every 20 s while the run lasts (`Held`'s beat already does), so the room and its
   pairing outlive the 60 s lease; leaving, and the run's end, send `CLOSE` (seat 1) or `RELEASE` (seat 2). Only a
   VS room's ticket goes into a 2017-18 run (SHOPS stays greyed there).

