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
- `all --edition 2017-18` runs the game server with the desk and the relay; with VS on, both use one matcher and the
  desk's `LOBBY` lists its waiting entries and pairs as VS rooms. `--no-gmsvr` leaves the game server out (the
  Launcher's server mode, whose PC runs its own stand-in).
- Both editions on one host: give the second one its own ports, e.g.
  `forza-server --edition 2017-18 all --desk-port 20032 --relay-port 20042 --gmsvr-port 20060`.
- The store is SQLite in WAL mode with `synchronous=NORMAL` (`gamesvr.db` plus `-wal` and `-shm` beside it); back it
  up with `sqlite3 gamesvr.db ".backup FILE"`, not a file copy.

Code: `src/` holds the desk, relay and inbox; the game server, the matcher and the reflect service are
`forza::online::{gamesvr, vs}`, shared with the Launcher's local stand-in. Builds: `cargo build --release -p forza-server`
(Linux), `cargo xtask build` (Windows, `programs\forza-server.exe` in the 2010-11 kit).
