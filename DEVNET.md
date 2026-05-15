# Local devnet — quickstart

A four-validator gsx-dag cluster on your laptop in one command.

This guide is for developers who want to **try the chain**: spin
nodes up, submit a transaction, see it commit, query state. It is
**not** how a mainnet validator is operated — keys are placeholders,
all four nodes share a host, and the network is closed to the
host's loopback. For mainnet operator procedures, see
[`docs/architecture/governance-phasing.md`](docs/architecture/governance-phasing.md).

## Prerequisites

- **Docker** + **Docker Compose v2** (`docker compose version` ≥ 2.20).
- **Python 3.8+** (for genesis generation — no third-party packages).
- **~3 GB free disk** for the build image and four-node logs.
- **`curl`** (for sanity-checking JSON-RPC from the host).
- *Optional*: **Rust 1.78+** if you want to run the SDK examples or
  drive `gsx-loadgen` against the cluster.

If you don't have Docker, the entire stack also builds with
`cargo build --release -p gsx-node` and runs four `gsx-node` processes
side-by-side on different ports — see [Bare-metal alternative](#bare-metal-alternative)
at the bottom.

## One-command bring-up

```sh
git clone https://github.com/GlobalSettlementNetwork/gsx-dag.git
cd gsx-dag
./scripts/devnet-local.sh up
```

What this does:

1. Runs `scripts/gen-devnet-genesis.py` to write
   `target/devnet/` containing `genesis.toml` and per-validator
   `v{0..3}/{node.toml, mldsa.sk, bls.sk}`.
2. Renders four `node.toml`s with peer-list entries pointing at
   the docker-compose bridge IPs (`172.30.0.10..13`).
3. Builds the `gsx-dag:devnet` Docker image (cold ~10 min; warm
   re-runs in seconds via cargo-chef).
4. Starts `v0`, `v1`, `v2`, `v3` and waits for v0's JSON-RPC
   healthcheck to pass.

When it returns, **v0's JSON-RPC is live at `http://127.0.0.1:9092`**
and its client-wire (TCP/bincode for `gsx-loadgen`) is on
`127.0.0.1:9091`.

## Sanity check

```sh
./scripts/devnet-local.sh curl
```

Expected output (the `current` epoch will advance over time):

```json
{
    "jsonrpc": "2.0",
    "id": 1,
    "result": {
        "current": 0,
        "last_boundary_round": 0,
        "rounds_per_epoch": 1024
    }
}
```

For an interactive walkthrough across more methods, see
[`examples/`](examples/) (lands with PR C2).

## Submit a transaction

Once C2 lands the examples directory:

```sh
# Rust
cargo run -p gsx-client --example submit_transfer

# TypeScript
cd clients/ts-sdk && npm run example:submit
```

Until then, you can submit directly via the JSON-RPC `gsx_submitIntent`
method or via the TCP/bincode client wire — see
[`docs/visuals/governance-flow.html`](docs/visuals/governance-flow.html)
for the protocol shape.

## Watch live events

The daemon publishes a WebSocket event stream at
`ws://127.0.0.1:9092/ws`. Both SDKs expose `subscribeEvents` over
that path:

```ts
import { Client } from "@gsx/client";

const client = new Client("http://127.0.0.1:9092");
client.subscribeEvents({
  onEvent: (ev) => console.log(ev.event, ev.round, ev.cert_hash),
});
```

```rust
use gsx_client::Client;

let client = Client::new("http://127.0.0.1:9092".into());
// Rust SDK's subscribe_events helper lands in the example crate
// alongside `subscribe_events.rs` (PR C2).
```

## Common operations

| Command | What it does |
|---|---|
| `./scripts/devnet-local.sh up` | Start the cluster (build image if needed). |
| `./scripts/devnet-local.sh down` | Stop containers, preserve `target/devnet/` so the next `up` is fast. |
| `./scripts/devnet-local.sh reset` | Stop + wipe `target/devnet/` and log volumes. Use after schema changes. |
| `./scripts/devnet-local.sh logs` | Tail v0's stdout (`Ctrl-C` to exit). |
| `./scripts/devnet-local.sh curl` | `gsx_getEpoch` against v0 — quick liveness check. |
| `docker compose logs v2 --tail=50` | One-off log fetch from a specific node. |
| `docker compose --profile indexer up` | Start `postgres` + `gsx-indexer` alongside the cluster. Indexer's HTTP API binds to `127.0.0.1:9093`. |

## Indexer (optional)

The indexer tails v0's event stream and persists committed blocks
to Postgres. Bring it up via the `indexer` profile:

```sh
docker compose --profile indexer up -d
curl http://127.0.0.1:9093/blocks/0   # fetch the genesis block
```

Schema lives in
[`crates/gsx-indexer/migrations/`](crates/gsx-indexer/migrations).
The indexer crate builds with `--features postgres`; without that
feature it falls back to an in-memory store (state lost on
restart). See A5 in
[`docs/audit/mainnet-readiness-2026-05-15.md`](docs/audit/mainnet-readiness-2026-05-15.md)
for the persistence-on-restart semantics.

## Troubleshooting

**`docker compose build` fails on cargo-chef step.**
The Dockerfile pins Rust 1.78 (workspace `rust-version`). If you
manually changed `Cargo.toml`'s `rust-version`, rebuild from clean:
`./scripts/devnet-local.sh reset && ./scripts/devnet-local.sh up`.

**`curl http://127.0.0.1:9092` returns connection-refused.**
v0's healthcheck takes 20-60s after `up` returns. Run
`./scripts/devnet-local.sh logs` and wait for the `client: listening
for intent submissions` line.

**One node won't commit; `committed=0` in its logs.**
This is the [IQ-004 single-cert orphan window](docs/iq/IQ-004-decide-slot-orphan-window.md)
under heavy local CPU contention. The A1 fix landed; if you see it
in a fresh devnet, file an issue with the `committed=`, `n=`, and
`epoch.cur=` values from the panic diagnostic.

**My host's port 9091/9092/5432/9093 is already in use.**
Edit the `ports:` mappings in `docker-compose.yml`. The
inside-container ports are stable so only the host-side mapping
changes.

## Bare-metal alternative

If Docker isn't an option, bring up four nodes side-by-side on the
host:

```sh
# 1. Build the binaries.
cargo build --release -p gsx-node --bin gsx-node

# 2. Generate genesis (loopback peer IPs).
python3 scripts/gen-devnet-genesis.py --num-nodes 4 --out-dir target/devnet

# 3. Write per-validator node.toml's. The bare-metal layout uses
#    127.0.0.1:909N for peer-listen and 127.0.0.1:90M for the
#    client-listen. Adapt scripts/devnet-local.sh's writer loop
#    (the `cat > target/devnet/vN/node.toml` block) by substituting
#    `127.0.0.1:909N` for `172.30.0.1N:9090` etc.

# 4. Start four shells, one per validator:
target/release/gsx-node --config target/devnet/v0/node.toml &
target/release/gsx-node --config target/devnet/v1/node.toml &
target/release/gsx-node --config target/devnet/v2/node.toml &
target/release/gsx-node --config target/devnet/v3/node.toml &
```

## Next steps

- **Build something on top:** see [`examples/`](examples/) (C2),
  the [Rust SDK](clients/rust-sdk), and the
  [TypeScript SDK](clients/ts-sdk).
- **Read the spec:** [`docs/architecture/`](docs/architecture/) tracks
  every paper section to its engineering doc and exit-gate proptest.
- **Contribute:** [`CONTRIBUTING.md`](CONTRIBUTING.md) (C3) covers PR
  workflow, sign-off policy, and specialist-reviewer expectations.
- **Report a security issue:** [`SECURITY.md`](SECURITY.md) (C3) —
  please don't open public issues for vulnerabilities.

## See also

- [`README.md`](README.md) — top-level project orientation.
- [`docs/README.md`](docs/README.md) — full documentation index.
- [`docs/architecture/security.md`](docs/architecture/security.md) —
  ingress hardening + fuzz catalog.
- [`docs/audit/mainnet-readiness-2026-05-15.md`](docs/audit/mainnet-readiness-2026-05-15.md) —
  current mainnet readiness posture.
