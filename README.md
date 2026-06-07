```text
╔═══════════════════════════════════════════════════════╗
║                                                       ║
║   suwappu-dag · post-quantum settlement chain             ║
║   joint-quorum BFT  ·  Mysticeti-C  ·  PQ crypto      ║
║                                                       ║
╚═══════════════════════════════════════════════════════╝
```

<div align="center">

# suwappu-dag

The post-quantum settlement chain — joint-quorum BFT safety on a
Mysticeti-C certificate-DAG, constant-size cross-chain attestation,
and an execution substrate built to settle regulated assets.

[![CI](https://img.shields.io/github/actions/workflow/status/suwappu/suwappu-dag/ci.yml?branch=main&label=CI)](https://github.com/suwappu/suwappu-dag/actions)
[![Latest release](https://img.shields.io/github/v/release/suwappu/suwappu-dag?include_prereleases&label=release)](https://github.com/suwappu/suwappu-dag/releases)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.78%2B-orange.svg)](./Cargo.toml)

[Quickstart](#quickstart) · [Architecture](#architecture) ·
[Repository map](#repository-map) · [Roadmap](./ROADMAP.md) ·
[Paper](https://github.com/suwappu/suwappu-papers) ·
[Changelog](./CHANGELOG.md) · [Security](./SECURITY.md) ·
[Contributing](./CONTRIBUTING.md)

</div>

---

## Why suwappu-dag

`suwappu-dag` is the L1 of the **Suwappu** — purpose-built
to clear cross-chain transfers under cryptography that survives the
post-quantum transition, with safety properties that survive single-ring
Byzantine corruption.

- **Quantum-resistant by default.** Every long-lived integrity surface
  uses NIST-standardized post-quantum primitives: **ML-DSA-65**
  (FIPS 204) for intent signing, **ML-KEM-768** (FIPS 203) for
  confidential transfer encryption. Classical-only chains migrate
  later; suwappu-dag ships PQ from day one.
- **Safety that survives a single-ring compromise.** Joint-quorum
  AND-gate (paper Theorem 2) requires Byzantine corruption of **both**
  a 40-slot Authority Ring **and** a 200-slot Validator Ring to fork
  the chain. No single ring is a single point of failure.
- **Constant-size cross-chain settlement.** Every LTP attestation
  commits **≈1,600 B regardless of payload** (paper §10.2) —
  ML-KEM-768 ciphertext (1,568 B) + BLS12-381 aggregate (96 B) +
  SHA3-256 root (32 B). Cross-chain cost stays bounded as volume grows.
- **Sub-second finality on the fast path.** Single-owner intents clear
  through a dedicated lane with **K=4 equivocation binding** and
  **100% bond slashing** for any Authority Node that signs a conflicting
  certificate (paper §6.4). Built for settlement, not speculation.
- **Paper-driven, proof-gated.** Every load-bearing claim is implemented
  by a named crate, traced to a specific paper section in the
  [visual map](docs/visuals/), and gated on **10,000-case property
  tests**. 20 sprints closed, 19 crates, 3 released phases.

### What this is NOT (yet)

- **Not on mainnet.** Current line is `0.x`; mainnet GA targets v1.0
  in the M18–M24 window (see [`ROADMAP.md`](./ROADMAP.md)).
- **No live token.** Devnet SUWAPPU is fungible test currency.
- **The execution substrate lives elsewhere.**
  [`suwappu-db`](https://github.com/suwappu/suwappu-db) holds
  the polymorphic balance map, OCC scheduler, state tree, and dual-VM
  projectors; this repo is the *consensus + execution-adapter + bridge
  + L2* layer that wires it into a chain.

---

## Quickstart

Start with the public devnet unless you're changing the validator
itself. Full operator + developer details in [`DEVNET.md`](./DEVNET.md).

### Public devnet endpoints

| Endpoint | URL |
|---|---|
| JSON-RPC | `https://rpc.devnet.suwappu.globalsettlement.com` |
| WebSocket | `wss://ws.devnet.suwappu.globalsettlement.com/ws` |
| Faucet | `https://faucet.devnet.suwappu.globalsettlement.com` |
| Explorer | `https://explorer.devnet.suwappu.globalsettlement.com` |
| Status | `https://status.devnet.suwappu.globalsettlement.com` |

### Talk to it in 60 seconds

Three commands using the Rust SDK examples in
[`examples/rust/`](./examples/rust) — runnable straight from a fresh
clone against the public devnet:

```bash
git clone https://github.com/suwappu/suwappu-dag.git
cd suwappu-dag/examples/rust
export SUWAPPU_RPC_URL=https://rpc.devnet.suwappu.globalsettlement.com

cargo run --bin query_epoch
cargo run --bin query_balance -- 0x0101010101010101010101010101010101010101
cargo run --bin subscribe_events
```

The same three calls in TypeScript via
[`clients/ts-sdk/`](./clients/ts-sdk):

```ts
import { Client } from "@suwappu/client";

const client = new Client("https://rpc.devnet.suwappu.globalsettlement.com");

console.log(await client.getEpoch());
console.log(await client.getBalance("0x0101010101010101010101010101010101010101"));

// subscribeEvents is callback-driven; Node 20/21 callers should also pass
// `WebSocket: (await import("ws")).WebSocket` in the options bag.
const sub = client.subscribeEvents({
  onEvent: (event) => console.log(event),
  onError: (err) => console.warn("ws:", err),
});
// ...later: sub.close();
```

### Get tokens from the faucet

```bash
# 100 SUWAPPU per drip, max 5 drips/hour per IP.
ADDR="0x$(openssl rand -hex 20)"
curl -X POST -H 'Content-Type: application/json' \
  -d "{\"address\":\"$ADDR\"}" \
  https://faucet.devnet.suwappu.globalsettlement.com/faucet
```

### Submit a signed Intent (educational)

The chain accepts intents signed by **seated authorities only** —
permissionless user transactions land via the L2 sequencer (Track G,
in flight on `suwappu-l2-sequencer` + `suwappu-l2-bridge`). Today, the public
devnet's faucet service is the only path for an unseated address to
land a Transfer on L1.

The construct-sign-submit pipeline is worth knowing anyway —
[`examples/rust/submit_transfer.rs`](./examples/rust/submit_transfer.rs)
has the full flow with the on-chain digest recipe. The headline:

```rust
use suwappu_execution::Intent;

let intent = Intent::Transfer { from, to, amount };
let intent_bincode = bincode::serialize(&intent)?;

// Signing digest: blake3(b"SUWAPPU_INTENT_V1" || network_id || intent_bincode).
// Both submitter and validator MUST compute the digest the same way —
// any divergence rejects the signature.
let mut hasher = blake3::Hasher::new();
hasher.update(b"SUWAPPU_INTENT_V1");
hasher.update(network_id.as_bytes());
hasher.update(&intent_bincode);
let digest = *hasher.finalize().as_bytes();

let (pubkey, sk) = suwappu_crypto::mldsa::keypair();
let signature = suwappu_crypto::mldsa::sign(&digest, &sk)?;
let pubkey_hash: [u8; 32] = *blake3::hash(pubkey.as_bytes()).as_bytes();

let client = suwappu_client::Client::new(&rpc_url);
client.submit_intent_raw(&intent_bincode, signature.as_bytes(), pubkey_hash).await?;
// → UnknownSigner unless `pubkey_hash` is in the AuthorityRegistry.
```

ML-DSA-65 signatures are 3,309 B fixed-width. The example prints the
constructed bytes and gracefully reports the expected `UnknownSigner`
rejection against a stock devnet.

### Run a local 4-node devnet (Docker)

```bash
./scripts/devnet-local.sh up
```

Generates per-validator keys + genesis under `target/devnet/` and
brings up 4 containers on a private network. JSON-RPC for `v0` is
exposed on `127.0.0.1:9092` — the same Rust + TS commands above work
unmodified once `SUWAPPU_RPC_URL` is unset (or set to the local URL). Tear
down with `./scripts/devnet-local.sh down`.

### SDKs and reference

- Rust SDK: [`clients/rust-sdk/`](./clients/rust-sdk)
- TypeScript SDK: [`clients/ts-sdk/`](./clients/ts-sdk)
- Rust examples: [`examples/rust/`](./examples/rust)
- Full RPC surface: [`crates/suwappu-rpc/`](./crates/suwappu-rpc)

### Other entry points

- **Build a dApp on the public testnet.** The **testnet** is the
  durable network (7 regions, persists until mainnet cutover);
  the devnet endpoints above are ephemeral and brought up on
  demand for protocol/perf work. See
  [`DEVNET.md § Public testnet`](DEVNET.md#public-testnet) for the
  testnet endpoints, faucet, and SDK examples. Don't anchor
  long-lived dApp testing to the devnet.
- **Run a testnet validator.** See
  [`docs/testnet/VALIDATOR-OPERATORS.md`](docs/testnet/VALIDATOR-OPERATORS.md)
  for the application + onboarding flow, hardware spec, and
  points formula.
- **Operate the foundation seed cluster.** See
  [`OPERATIONS.md § 10`](OPERATIONS.md) and
  [`terraform/testnet/README.md`](terraform/testnet/README.md)
  for the bootstrap procedure, CodeBuild + SSM gotchas, and the
  fronting follow-up.

---

## Architecture

suwappu-dag separates **consensus**, **execution**, and **cross-chain
attestation** into three crisply-bounded surfaces, then wires them
together with a single rule: every commit is the joint product of two
independently-bonded validator rings.

**Consensus (paper §6).** Authority Nodes propose intents and produce
Mysticeti-C certificates. The DAG store linearizes those certificates
into a totally-ordered commit stream under a commit rule whose orphan
window is property-tested at 10,000 cases. A single-owner fast-path
lane (paper §6.4) clears non-contested intents sub-second with K=4
equivocation binding — any Authority Node that double-signs forfeits
100% of bond.

**Execution (paper §7).** Each committed intent is applied to the
[`suwappu-db`](https://github.com/suwappu/suwappu-db) substrate
via the `suwappu-execution` adapter. The substrate owns the polymorphic
balance map, OCC scheduler, state tree, and dual-VM projectors; this
repo owns the Intent surface that calls into it. Tokenomics (§3 / §4 /
§8) — inflation, delegation, slashing waterfall, withdrawal cooldown
— are implemented as state-transition arms on that substrate.

**Cross-chain (paper §10).** Every checkpoint is jointly co-signed by
the Authority and Validator Rings, then committed to LTP super-nodes
as a constant-size attestation. Other settlement venues observe these
attestations to recognize suwappu-dag state without re-running consensus.

```mermaid
flowchart LR
    Client["SDK / RPC client"] --> Mempool
    Mempool --> Authority["Authority Ring<br/>40 slots"]
    Mempool --> Validator["Validator Ring<br/>200 slots"]
    Authority -.->|joint co-sign| DAG[("Mysticeti-C DAG")]
    Validator -.->|joint co-sign| DAG
    DAG --> Executor["suwappu-execution → suwappu-db"]
    Executor --> LTP["LTP attestation<br/>≈1,600 B constant"]
```

Deep-dive visuals (interactive Mermaid + HTML, rendered from spec
numbers, not scaffolded charts):

- [DAG L1 internals](docs/visuals/suwappu-dag.html) ·
  [Substrate (suwappu-db)](docs/visuals/suwappu-db.html) ·
  [LTP attestation](docs/visuals/ltp.html)
- [Commit rule](docs/visuals/commit-rule.html) ·
  [Fast path + slashing](docs/visuals/fast-path-and-slashing.html) ·
  [Dual-VM projection](docs/visuals/dual-vm.html)
- [Governance flow](docs/visuals/governance-flow.html) ·
  [SCION transport](docs/visuals/scion-transport.html) ·
  [Ecosystem atlas](docs/visuals/suwappu-ecosystem-atlas.html)

---

## Repository map

19 crates, grouped by surface. Each entry cites the paper section it
implements where applicable.

### Consensus + cryptography

| Crate | Paper § | Owns |
|---|---|---|
| `suwappu-crypto` | §3.3, §10, §12 | ML-DSA-65, ML-KEM-768, BLS12-381, SHA3-256, Poseidon2 |
| `suwappu-consensus` | §6 | Mysticeti-C integration, certificate DAG, BFT linearization |
| `suwappu-fastpath` | §6.4 | Single-owner lane, K=4 equivocation binding |
| `suwappu-authority` | §5.1 | Authority Ring registry + certificate production |
| `suwappu-validator` | §5.2 | Validator Ring registry + ratification + slashing |

### Execution

| Crate | Paper § | Owns |
|---|---|---|
| `suwappu-execution` | §7 | Wires `suwappu-db` substrate into the DAG executor; Intent surface |
| `suwappu-precompiles` | §8 | Registered-issuer, DID, policy-vocabulary, reserve-coverage |
| `suwappu-mempool` | §7.2 | Mempool + tx-hash dedup + per-IP rate limit |

### Track G — L2 / ZK rollup (in flight)

| Crate | Paper § | Owns |
|---|---|---|
| `suwappu-l2-bridge` | §11 | L1 ↔ L2 deposit / withdraw / proof-verified state |
| `suwappu-l2-confidential` | §11.3 | Confidential-balance L2 surface (Track H integration) |
| `suwappu-l2-sequencer` | §11.2 | Sequencer mempool + batch builder + force-include |
| `suwappu-l2-verifier-precompile` | §11.4 | SP1 Groth16 BN254 verifier for L2 state-root commits |

### Cross-chain

| Crate | Paper § | Owns |
|---|---|---|
| `suwappu-ltp` | §10 | LTP attestation pipeline + super-node integration |

### Network, interface, operator

| Crate | Paper § | Owns |
|---|---|---|
| `suwappu-transport` | §6.3 | SCION path-authenticated gossip + RaptorQ shred/reconstruct |
| `suwappu-rpc` | — | JSON-RPC + WebSocket API |
| `suwappu-indexer` | — | Streaming Postgres indexer |
| `suwappu-faucet` | — | Devnet / testnet faucet service |
| `suwappu-validator-program` | — | Operator points-accumulator daemon |
| `suwappu-node` | — | Top-level binary, config, telemetry |

---

## Development

```bash
# Per-crate checks (workspace-wide commands can saturate low-RAM
# machines — see CONTRIBUTING.md for the rationale).
cargo fmt -p <crate>
cargo clippy -p <crate> --all-targets -- -D warnings
cargo test -p <crate>

# Sprint-closure exit gate: 10,000 proptest cases (release profile).
PROPTEST_CASES=10000 cargo test --release -p <crate>
```

CI runs the full workspace matrix (`rustfmt` / `clippy` / `test` /
`cargo-deny`) on every PR — push to a feature branch and let CI
validate. See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for the PR
workflow, the local-vs-CI guidance, branch-naming conventions, and
DCO sign-off requirements.

Sprint cadence + collaboration contract are in
[`SUWAPPUHELPER.md`](./SUWAPPUHELPER.md). The dependency graph and per-sprint exit
gates are in
[`docs/architecture/sprint-map.md`](./docs/architecture/sprint-map.md).

---

## Status

- **Codebase**: pre-mainnet `0.x` line. Sprints DAG-S1 → DAG-S20 all
  closed; every sprint gated on 10,000-case proptest exit. 19 crates,
  3 released phases.
- **v0.3.0** (2026-05-18) — economic surface: per-slot stake,
  inflation, delegation, atomic slashing waterfall.
- **v0.2.0** — state surface: force-include, bridge whitelist,
  multi-chain VK registry, validator-set registries.
- **v0.1.0** — consensus + crypto + LTP attestation + SCION transport
  + full validator composition (DAG-S1 → DAG-S20).
- **Phase 4 — public devnet** in flight (4 regions; RPC + faucet +
  explorer + status). G1 apply-ready, G4 live; G2 / G3 / G5 / G6 /
  G7 / G8 tracked in
  [`ROADMAP.md`](./ROADMAP.md#phase-4--public-devnet-in-flight).
- **Phase 5 — incentivized testnet** next (Q3 2026, 7 regions,
  external operator points program).
- **Mainnet GA** targets v1.0 in the M18–M24 window.

Released versions:
[Releases page](https://github.com/suwappu/suwappu-dag/releases).
Per-release scope: [`CHANGELOG.md`](./CHANGELOG.md).

---

## Specs & research

- **Academic specifications**:
  [`suwappu-papers`](https://github.com/suwappu/suwappu-papers)
  (private repo — available on request). v8 DAG L1 paper
  (`suwappu_dag_l1_academic_v7.pdf`) + companion LTP paper
  (`suwappu_ltp_academic_v7.pdf`).
- **Architecture diagrams**: [`docs/visuals/`](docs/visuals/) —
  interactive Mermaid + HTML for the DAG, substrate, LTP commitment
  surface, fast path, dual-VM, governance flow, SCION transport, and
  the ecosystem atlas.
- **Investigation Questions (IQ docs)**: [`docs/iq/`](docs/iq/) —
  written ratifications for load-bearing decisions (quorum formula,
  indirect commit, fast-path architecture, decide_slot orphan window,
  bincode 2.x migration, L2 state-root commitment surface).
- **Sprint dependency graph**:
  [`docs/architecture/sprint-map.md`](./docs/architecture/sprint-map.md).

---

## Related repositories

| Repo | Role |
|---|---|
| [`suwappu-papers`](https://github.com/suwappu/suwappu-papers) | v8 academic specs (DAG L1 + LTP) |
| [`suwappu-db`](https://github.com/suwappu/suwappu-db) | Execution substrate (consumed here as workspace dep) |
| [`suwappu-lattice-protocol`](https://github.com/suwappu/suwappu-lattice-protocol) | LTP bridge gateway |
| [`op-stack-reth`](https://github.com/suwappu/op-stack-reth) | OP-stack reference fork (historical — Track G's L2 proof system has since moved to SP1 Groth16 BN254) |

---

## Security

Found a vulnerability? **Don't open a public issue.** See
[`SECURITY.md`](./SECURITY.md) for the coordinated-disclosure process.

## License

Apache-2.0 — see [`LICENSE`](./LICENSE).
