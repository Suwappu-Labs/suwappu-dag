# Devnet + testnet — quickstart

Two long-lived networks + a laptop option:

1. **Public testnet** — 7-region, points-bearing, durable.
   This is the network external developers and validator
   operators target day-to-day. Genesis lives on-chain until
   the mainnet cutover.
   See [§ Public testnet](#public-testnet).
2. **Devnet** — ephemeral 4-region cluster the foundation
   spins up on demand for protocol-change testing,
   performance experiments, or scenario reproductions. **Not**
   always-on. When it's down, build against testnet instead.
   See [§ Devnet (ephemeral)](#devnet-ephemeral).
3. **Local 4-node cluster** on your laptop via docker-compose.
   Use this only when you need to test changes to the validator
   itself with full host control. See [§ Local devnet](#local-devnet).

For foundation-internal seed-cluster procedures, see
[`OPERATIONS.md § 10`](OPERATIONS.md) and
[`terraform/testnet/README.md`](terraform/testnet/README.md).
For mainnet operator procedures, see
[`docs/architecture/governance-phasing.md`](docs/architecture/governance-phasing.md).

---

## Devnet (ephemeral)

The devnet is the foundation's mutable sandbox in AWS, brought up
on demand from [`terraform/devnet/`](terraform/devnet/) when we
need to test protocol changes, run performance experiments, or
reproduce incidents without touching the live testnet.

- **Not always-on.** When the stack is down, none of the URLs
  below resolve. Check `terraform/devnet` state before assuming
  it's up: `aws s3 ls s3://gsx-dag-tf-state/gsx-dag/devnet/`.
- **State is disposable.** Every fresh apply mints a new genesis
  unless you explicitly carry the EBS volumes over. Don't keep
  long-running dApp work pointed at this network.
- **For long-lived dApp testing, use the testnet** ([§ Public
  testnet](#public-testnet)). The testnet's chain history
  persists until mainnet cutover.

When the devnet is up, it provisions four `t4g.medium` validators
across us-east-1 / eu-west-1 / ap-southeast-1 / sa-east-1 under
the `gsx-dev-*` AWS-name prefix (not `gsx-devnet-*` — the testnet
owns that namespace; see
[`terraform/devnet/README.md`](terraform/devnet/README.md)).

| Network | |
|---|---|
| `network_id` | `gsx-devnet` |
| `chain_id` | `2025` |
| Validators | 4 (us-east-1, eu-west-1, ap-southeast-1, sa-east-1) |
| Faucet drip | 100 GSX, max 5 drips/hour per IP |
| Wipe policy | Stack is torn down and re-applied as needed; assume any state can disappear. |

Bring-up runbook lives in
[`OPERATIONS.md § 1`](OPERATIONS.md#1-bootstrap-a-fresh-devnet).
Endpoints (when up) follow the same pattern as testnet
(`rpc.devnet.gsx.*`, `faucet.devnet.gsx.*`, etc.), with the same
ALB-fronting gap called out in
[`terraform/testnet/README.md § Known limitations`](terraform/testnet/README.md#known-limitations).
Until the per-region NLB + Global Accelerator follow-up lands,
reach validators directly by EIP from `terraform output validators`.

---

## Public testnet

Long-lived 7-region cluster. External validator operators earn
points convertible to mainnet token at TGE; dApps developers can
use it as a stable target without running infra. Genesis was
minted on 2026-05-18; chain history is retained until the
mainnet cutover.

| Network | |
|---|---|
| `network_id` | `gsx-testnet-v1` |
| `chain_id` | `20251` |
| Seed validators | 7 (us-east-1, us-west-2, eu-west-1, eu-central-1, ap-southeast-1, ap-northeast-1, sa-east-1) |
| `rounds_per_epoch` | 4096 (4× devnet — longer epochs reduce governance churn) |
| Faucet drip | 100 GSX, max 5 drips/hour per IP |
| Wipe policy | None until mainnet cutover. State + points data is preserved across patch + minor releases. |

### Endpoints

Once the per-region NLB + Global Accelerator fronting lands
(tracked in `terraform/testnet/README.md` § "Known limitations"),
the public wildcard endpoint will be:

| Endpoint | URL |
|---|---|
| JSON-RPC | `https://rpc.testnet.gsx.globalsettlement.com` |
| WebSocket subscribe | `wss://ws.testnet.gsx.globalsettlement.com/ws` |
| Faucet (POST `/faucet { address }`) | `https://faucet.testnet.gsx.globalsettlement.com` |
| Block explorer | `https://explorer.testnet.gsx.globalsettlement.com` |
| Status page | `https://status.testnet.gsx.globalsettlement.com` |

Today the wildcard endpoint returns 503 — see the same § for
why. Until then, reach validators directly by EIP on port 9092:

| Region | EIP | JSON-RPC |
|---|---|---|
| us-east-1      | 52.5.240.86      | `http://52.5.240.86:9092/` |
| us-west-2      | 16.148.234.2     | `http://16.148.234.2:9092/` |
| eu-west-1      | 54.73.42.237     | `http://54.73.42.237:9092/` |
| eu-central-1   | 63.185.0.111     | `http://63.185.0.111:9092/` |
| ap-southeast-1 | 18.139.179.124   | `http://18.139.179.124:9092/` |
| ap-northeast-1 | 3.114.228.57     | `http://3.114.228.57:9092/` |
| sa-east-1      | 54.233.81.124    | `http://54.233.81.124:9092/` |

Pick the geographically closest one for lower RTT; all 7 serve
the same chain state once committed.

### Liveness probe

```sh
for ip in 52.5.240.86 16.148.234.2 54.73.42.237 63.185.0.111 \
          18.139.179.124 3.114.228.57 54.233.81.124; do
  echo "$ip $(curl -fsS --max-time 5 -X POST \
       -H 'Content-Type: application/json' \
       -d '{"jsonrpc":"2.0","id":1,"method":"gsx_getEpoch"}' \
       "http://$ip:9092/" | jq -r '.result.latest_committed_round')"
done
```

Every IP should return the same (or within ~5 of) a
monotonically advancing `latest_committed_round`.

### Trust posture

Larger than devnet but still **a testnet** — tokens have $0
economic security. The points program assigns weight per
[`docs/testnet/POINTS.md`](docs/testnet/POINTS.md) but the
token itself is non-tradeable until TGE. Replay-protected via
`chain_id = 20251`.

### Running a testnet validator (external operators)

See [`docs/testnet/VALIDATOR-OPERATORS.md`](docs/testnet/VALIDATOR-OPERATORS.md)
for the application + onboarding flow and the hardware spec.
External validators bring their own infra and peer into the
foundation's 7 seed regions over the public internet.

---

## Local devnet

A four-validator gsx-dag cluster on your laptop in one command.

This guide is for developers who want to **try the chain**: spin
nodes up, submit a transaction, see it commit, query state. It is
**not** how a mainnet validator is operated — keys are placeholders,
all four nodes share a host, and the network is closed to the
host's loopback.

## Prerequisites

- **Docker** + **Docker Compose v2** (`docker compose version` ≥ 2.20).
- **Python 3.8+** (for genesis generation — no third-party packages).
- **Rust 1.78+** (`up` builds the host-side `gsx-keygen` to mint the
  faucet ML-DSA key; `faucet` builds `gsx-faucet`).
- **~3 GB free disk** for the build image and four-node logs.
- **`curl`** (for sanity-checking JSON-RPC from the host).

If you don't have Docker, use `./scripts/devnet-local.sh up-baremetal`
instead — it builds `gsx-node` (single-package release) and runs four
processes on loopback (`127.0.0.1:9{0..3}90/91/92`). v0's RPC stays on
`127.0.0.1:9092` so the `curl` and `faucet` subcommands below work the
same way. See [Bare-metal alternative](#bare-metal-alternative) for
details.

## One-command bring-up

```sh
git clone https://github.com/GlobalSettlementNetwork/gsx-dag.git
cd gsx-dag
./scripts/devnet-local.sh up
```

What this does:

1. Builds the `gsx-keygen` binary host-side and runs
   `scripts/gen-devnet-genesis.py`. The script writes `target/devnet/`
   containing `genesis.toml`, per-validator `v{0..3}/{mldsa,bls}.sk`,
   and `faucet/{mldsa.sk, mldsa.pk, address.hex}` — a real ML-DSA-65
   keypair (validator-side keys are placeholders; the faucet's must be
   real because `verify_signed_intent` checks signatures on drips).
   The genesis seats the faucet as `authority_id = 4` and funds its
   address via a `[[prebalances]]` entry.
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

## Run the faucet

The faucet is a separate HTTP service that signs Transfer intents
with the seeded ML-DSA-65 key from `target/devnet/faucet/` and
submits them via the local cluster's JSON-RPC. In a second terminal:

```sh
./scripts/devnet-local.sh faucet
```

This builds `gsx-faucet` (single-package release build) and launches
it on `127.0.0.1:8080` against `http://127.0.0.1:9092`, pinned to the
genesis `network_id` (`gsx-devnet-local`). Ctrl-C stops it.

End-to-end smoke test from a third terminal:

```sh
# 1. Faucet health — short-circuits the most common misconfig
#    (address derivation drift between gen-script and runtime).
curl -s http://127.0.0.1:8080/health | python3 -m json.tool

# 2. Drip 100 GSX to a fresh address.
ADDR="0x$(openssl rand -hex 20)"
curl -sX POST -H 'Content-Type: application/json' \
     -d "{\"address\":\"$ADDR\"}" http://127.0.0.1:8080/faucet

# 3. Confirm the balance (wait a few seconds for commit).
sleep 5
curl -sX POST -H 'content-type: application/json' \
     -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"gsx_getBalance\",\"params\":{\"address\":\"$ADDR\"}}" \
     http://127.0.0.1:9092
```

If `/health` returns 503: the address in `target/devnet/genesis.toml`
`[[prebalances]]` doesn't match the runtime-derived address. Compare
`grep address target/devnet/genesis.toml` to the `faucet_address`
printed in the faucet's startup log; both should equal
`cat target/devnet/faucet/address.hex`.

## Submit a transaction

Once C2 lands the examples directory:

```sh
# Rust
cargo run -p gsx-client --example submit_transfer

# TypeScript
cd clients/ts-sdk && npm run example:submit
```

Until then, submit directly via the faucet (above), the JSON-RPC
`gsx_submitIntent` method, or the TCP/bincode client wire — see
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

If Docker isn't an option:

```sh
./scripts/devnet-local.sh up-baremetal
```

What this does:

1. Builds `gsx-keygen` and `gsx-node` (single-package release builds).
2. Runs `gen-devnet-genesis.py` to produce `target/devnet/` exactly
   like the docker path — same genesis, same faucet keypair, same
   `[[prebalances]]`.
3. Writes per-validator `node.toml`s with loopback ports:
   v{N} listens on `127.0.0.1:9{N}90` (peer), `:9{N}91` (client),
   `:9{N}92` (rpc). v0's RPC stays on `127.0.0.1:9092` so the rest
   of this guide (`curl`, `faucet`) is unchanged.
4. Starts the four `gsx-node` processes in the background. PIDs are
   written to `target/devnet/v{0..3}.pid`; logs to
   `target/devnet/v{0..3}.log`.

Tail logs and stop:

```sh
./scripts/devnet-local.sh logs-baremetal     # tail v0.log
./scripts/devnet-local.sh down-baremetal     # kill the four processes
```

`./scripts/devnet-local.sh reset` clears both flavors safely.

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
