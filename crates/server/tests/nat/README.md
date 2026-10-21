# NAT harness: 2017-18 direct VS between two homes (Docker)

ROADMAP S0 / M3 / L3, the part a script can show before two Windows PCs play. Opt-in: `cargo xtask check` never runs
it.

    crates/server/tests/nat/run.sh                      # build, the whole matrix, the gate (about 4 minutes)
    NATS="cone:restricted" DELAYS="100" crates/server/tests/nat/run.sh    # one cell

Needs Docker with compose (`docker info`), jq, and the Linux Rust toolchain; the script builds `forza-server` and
`vslink`'s `nat_peer` example (`cargo build --release`), mounts `target/release` into the containers and pulls
`debian:trixie-slim` once. Results and every log go to `target/nat-<date>/run-<time>/` (`NAT_WORK=`
elsewhere): `results.txt` (the table), `results.jsonl` (both homes' lines per cell), and per cell `a.log`, `b.log`
(vslink's log), `server.log`. `NAT_NO_BUILD=1` skips the cargo builds. Exit 0 = the gate passed.

## What runs

| Container | Network | Runs |
|---|---|---|
| `server` 10.99.0.10 | WAN | `forza-server --edition 2017-18 all` with VS on (desk 20030 with the room words, game server 20060 with the matcher, reflect 20070, relay 20040) and a test iroh relay (`nat_peer relay`, HTTPS 3340, QUIC 7842, self-signed) |
| `nat-a`, `nat-b` | WAN + one home | a home router (`router.sh`): NAT kind and `tc netem` on its WAN side |
| `client-a`, `client-b` | home A 192.168.11.0/24, home B 192.168.12.0/24 | `nat_peer client`: one home's Launcher and game on the network |

NAT kinds: `cone` (MASQUERADE + every inbound UDP to the PC), `restricted` (MASQUERADE alone: Linux keeps the port,
port-restricted filtering), `symmetric` (`MASQUERADE --random-fully`, a new port per destination). Delay D: each
router delays its WAN egress by D/2, so the two homes are D apart (round trip) and each home D/2 from the server.
`LOSS=1` adds 1 % loss there; the team block has no resend (the game's own recovery is unknown), so expect the block
check to fail under loss: use it to watch pairing and the link, not as the gate.

`nat_peer client` (`crates/vslink/examples/nat_peer.rs`) does, in order: the lobby (`CREATE vs` on the host,
`RESERVE` on the guest, which reads the room id from a shared folder as a friend reads a code; `READY`, `HEARTBEAT`
then every 20 s), vslink started as the run starts it (the room ticket in its hello, reflect on `127.0.0.1:20090`, the
harness's relay instead of the public one), the control's 201 (202 must name the loopback reflect), the client's
reflect probe (the echo carries the seat's 127/8 token), 223, 243 with the token, 245 every 500 ms until 246, then the
client's peer traffic to the opponent's token: hello/ack, a 3,232-byte team block as 202 DIVIDE chunks of 16 bytes
each way (checked byte for byte), 20 pings. It leaves the room (`CLOSE` / `RELEASE`) and prints one JSON line.

The gate per cell: **paired** = both got 246, each naming the other's token, the same seed, exactly one Home side;
**link** = hello answered both ways; **path** = vslink's last path log (`direct` once iroh's hole punch works, else
`relay`); **block both ways** = each side got the other's whole block; pair ms and link ms from the client's start.
Then `open-pool` (no room on either side: M3's open pool must pair them) and `cross-room` (two hosts of two rooms:
must never pair in 20 s).

## Results, 2026-10-20 (`nat-harness` branch)

All 14 cells passed (pairing, link, block both ways; the cross-room pair never paired). Pairing took 0.5-1.8 s and
the link 0.7-2.3 s from the clients' start, growing with the delay; ping medians matched D. Paths: `cone+restricted`
direct at every delay; `restricted+restricted` direct at 50-150 ms but relay at 0 ms (twice: with no delay both
punches cross at once, and the router's conntrack entry for the peer's unsolicited packet makes MASQUERADE pick
another port for the reply flow, as a symmetric NAT would); `symmetric+symmetric` relay at every delay, as expected.
The open pool paired and went direct.

## What it cannot show

- The game. The fake client sends the layouts our server reads (201, 223, 243, 245) and peer datagrams shaped like
  the client's (magic, type at +8); whether the real control and client accept a 127/8 token, how long the control
  polls 245 before its CPU fallback, the real DIVIDE pacing, lock-step GAME_IO over a relayed link and errors
  2001/2012 over a whole match are M3's two-PC gate.
- Windows networking (the Windows Firewall prompt, Wi-Fi, real home routers: CGNAT, hairpinning, UPnP).
- The public relay (`relay.wccf.it`) and its TLS chain: the harness uses its own relay with certificate checks off.
- Timings for the S0 baseline (RSS, CPU): `load.rs` and a 2-vCPU host's numbers.
