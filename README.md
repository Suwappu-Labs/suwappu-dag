<div align="center">

# gsx-dag

The **post-quantum settlement chain**. Joint-quorum BFT safety on a
Mysticeti-C certificate-DAG, constant-size cross-chain attestation,
and an execution substrate built to settle regulated assets.

[![CI](https://img.shields.io/github/actions/workflow/status/GlobalSettlementNetwork/gsx-dag/ci.yml?branch=main&label=CI)](https://github.com/GlobalSettlementNetwork/gsx-dag/actions)
[![Latest release](https://img.shields.io/github/v/release/GlobalSettlementNetwork/gsx-dag?include_prereleases&label=release)](https://github.com/GlobalSettlementNetwork/gsx-dag/releases)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.78%2B-orange.svg)](./Cargo.toml)

[Quickstart](#quickstart) · [Architecture](#architecture) ·
[Repository map](#repository-map) · [Roadmap](./ROADMAP.md) ·
[Paper](https://github.com/GlobalSettlementNetwork/gsx-papers) ·
[Changelog](./CHANGELOG.md) · [Security](./SECURITY.md) ·
[Contributing](./CONTRIBUTING.md)

</div>

---

## Why gsx-dag

`gsx-dag` is the L1 of the **Global Settlement Network** — purpose-built
to clear cross-chain transfers under cryptography that survives the
post-quantum transition, with safety properties that survive single-ring
Byzantine corruption.

- **Quantum-resistant by default.** Every long-lived integrity surface
  uses NIST-standardized post-quantum primitives: **ML-DSA-65**
  (FIPS 204) for intent signing, **ML-KEM-768** (FIPS 203) for
  confidential transfer encryption. Classical-only chains migrate
  later; gsx-dag ships PQ from day one.
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
- **No live token.** Devnet GSX is fungible test currency.
- **The execution substrate lives elsewhere.**
  [`gsx-db`](https://github.com/GlobalSettlementNetwork/gsx-db) holds
  the polymorphic balance map, OCC scheduler, state tree, and dual-VM
  projectors; this repo is the *consensus + execution-adapter + bridge
  + L2* layer that wires it into a chain.

---

## Quickstart

Two paths — start with the public devnet unless you're changing the
validator itself. Full operator + developer details in
[`DEVNET.md`](./DEVNET.md).

### 1. Use the public devnet (recommended)

| Endpoint | URL |
|---|---|
| JSON-RPC | `https://rpc.devnet.gsx.globalsettlement.com` |
| WebSocket | `wss://ws.devnet.gsx.globalsettlement.com/ws` |
| Faucet | `https://faucet.devnet.gsx.globalsettlement.com` |
| Explorer | `https://explorer.devnet.gsx.globalsettlement.com` |
| Status | `https://status.devnet.gsx.globalsettlement.com` |

```bash
# Drip 100 GSX to a fresh address (max 5 drips/hour per IP).
ADDR="0x$(openssl rand -hex 20)"
curl -X POST -H 'Content-Type: application/json' \
  -d "{\"address\":\"$ADDR\"}" \
  https://faucet.devnet.gsx.globalsettlement.com/faucet

# Read epoch via JSON-RPC.
curl -sX POST https://rpc.devnet.gsx.globalsettlement.com \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"gsx_getEpoch","params":null}'
```

### 2. Run a local 4-node devnet (Docker)

```bash
git clone https://github.com/GlobalSettlementNetwork/gsx-dag.git
cd gsx-dag
./scripts/devnet-local.sh up
```

The script generates per-validator keys + genesis under `target/devnet/`
and brings up 4 containers on a private network. JSON-RPC for `v0` is
exposed on `127.0.0.1:9092`. Tear down with `./scripts/devnet-local.sh down`.

```bash
curl -sX POST http://127.0.0.1:9092 -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"gsx_getEpoch","params":null}'
```

The full RPC surface is documented in
[`crates/gsx-rpc/`](./crates/gsx-rpc).

### SDKs

- Rust: [`clients/rust-sdk/`](./clients/rust-sdk)
- TypeScript: [`clients/ts-sdk/`](./clients/ts-sdk)
- End-to-end example: `cargo run -p gsx-client --example submit_transfer`

---

## Architecture

gsx-dag separates **consensus**, **execution**, and **cross-chain
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
[`gsx-db`](https://github.com/GlobalSettlementNetwork/gsx-db) substrate
via the `gsx-execution` adapter. The substrate owns the polymorphic
balance map, OCC scheduler, state tree, and dual-VM projectors; this
repo owns the Intent surface that calls into it. Tokenomics (§3 / §4 /
§8) — inflation, delegation, slashing waterfall, withdrawal cooldown
— are implemented as state-transition arms on that substrate.

**Cross-chain (paper §10).** Every checkpoint is jointly co-signed by
the Authority and Validator Rings, then committed to LTP super-nodes
as a constant-size attestation. Other settlement venues observe these
attestations to recognize gsx-dag state without re-running consensus.

```mermaid
flowchart LR
    Client["SDK / RPC client"] --> Mempool
    Mempool --> Authority["Authority Ring<br/>40 slots"]
    Mempool --> Validator["Validator Ring<br/>200 slots"]
    Authority -.->|joint co-sign| DAG[("Mysticeti-C DAG")]
    Validator -.->|joint co-sign| DAG
    DAG --> Executor["gsx-execution → gsx-db"]
    Executor --> LTP["LTP attestation<br/>≈1,600 B constant"]
```

Deep-dive visuals (interactive Mermaid + HTML, rendered from spec
numbers, not scaffolded charts):

- [DAG L1 internals](docs/visuals/gsx-dag.html) ·
  [Substrate (gsx-db)](docs/visuals/gsx-db.html) ·
  [LTP attestation](docs/visuals/ltp.html)
- [Commit rule](docs/visuals/commit-rule.html) ·
  [Fast path + slashing](docs/visuals/fast-path-and-slashing.html) ·
  [Dual-VM projection](docs/visuals/dual-vm.html)
- [Governance flow](docs/visuals/governance-flow.html) ·
  [SCION transport](docs/visuals/scion-transport.html) ·
  [Ecosystem atlas](docs/visuals/gsx-ecosystem-atlas.html)

---

## Repository map

19 crates, grouped by surface. Each entry cites the paper section it
implements where applicable.

### Consensus + cryptography

| Crate | Paper § | Owns |
|---|---|---|
| `gsx-crypto` | §3.3, §10, §12 | ML-DSA-65, ML-KEM-768, BLS12-381, SHA3-256, Poseidon2 |
| `gsx-consensus` | §6 | Mysticeti-C integration, certificate DAG, BFT linearization |
| `gsx-fastpath` | §6.4 | Single-owner lane, K=4 equivocation binding |
| `gsx-authority` | §5.1 | Authority Ring registry + certificate production |
| `gsx-validator` | §5.2 | Validator Ring registry + ratification + slashing |

### Execution

| Crate | Paper § | Owns |
|---|---|---|
| `gsx-execution` | §7 | Wires `gsx-db` substrate into the DAG executor; Intent surface |
| `gsx-precompiles` | §8 | Registered-issuer, DID, policy-vocabulary, reserve-coverage |
| `gsx-mempool` | §7.2 | Mempool + tx-hash dedup + per-IP rate limit |

### Track G — L2 / ZK rollup (in flight)

| Crate | Paper § | Owns |
|---|---|---|
| `gsx-l2-bridge` | §11 | L1 ↔ L2 deposit / withdraw / proof-verified state |
| `gsx-l2-confidential` | §11.3 | Confidential-balance L2 surface (Track H integration) |
| `gsx-l2-sequencer` | §11.2 | Sequencer mempool + batch builder + force-include |
| `gsx-l2-verifier-precompile` | §11.4 | SP1 Groth16 BN254 verifier for L2 state-root commits |

### Cross-chain

| Crate | Paper § | Owns |
|---|---|---|
| `gsx-ltp` | §10 | LTP attestation pipeline + super-node integration |

### Network, interface, operator

| Crate | Paper § | Owns |
|---|---|---|
| `gsx-transport` | §6.3 | SCION path-authenticated gossip + RaptorQ shred/reconstruct |
| `gsx-rpc` | — | JSON-RPC + WebSocket API |
| `gsx-indexer` | — | Streaming Postgres indexer |
| `gsx-faucet` | — | Devnet / testnet faucet service |
| `gsx-validator-program` | — | Operator points-accumulator daemon |
| `gsx-node` | — | Top-level binary, config, telemetry |

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
[`CLAUDE.md`](./CLAUDE.md). The dependency graph and per-sprint exit
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
[Releases page](https://github.com/GlobalSettlementNetwork/gsx-dag/releases).
Per-release scope: [`CHANGELOG.md`](./CHANGELOG.md).

---

## Specs & research

- **Academic specifications**:
  [`gsx-papers`](https://github.com/GlobalSettlementNetwork/gsx-papers)
  (private repo — available on request). v8 DAG L1 paper
  (`gsx_dag_l1_academic_v7.pdf`) + companion LTP paper
  (`gsx_ltp_academic_v7.pdf`).
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
| [`gsx-papers`](https://github.com/GlobalSettlementNetwork/gsx-papers) | v8 academic specs (DAG L1 + LTP) |
| [`gsx-db`](https://github.com/GlobalSettlementNetwork/gsx-db) | Execution substrate (consumed here as workspace dep) |
| [`gsx-lattice-protocol`](https://github.com/GlobalSettlementNetwork/gsx-lattice-protocol) | LTP bridge gateway |
| [`op-stack-reth`](https://github.com/GlobalSettlementNetwork/op-stack-reth) | OP-stack reference fork (historical — Track G's L2 proof system has since moved to SP1 Groth16 BN254) |

---

## Security

Found a vulnerability? **Don't open a public issue.** See
[`SECURITY.md`](./SECURITY.md) for the coordinated-disclosure process.

## License

Apache-2.0 — see [`LICENSE`](./LICENSE).
