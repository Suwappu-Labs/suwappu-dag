<div align="center">

# gsx-dag

**The GSX DAG Layer 1** — a Mysticeti-style certificate-DAG settlement
chain with a dual-ring validator set, co-resident dual virtual machine,
and post-quantum cross-chain attestation.

[![Build](https://img.shields.io/github/actions/workflow/status/GlobalSettlementNetwork/gsx-dag/ci.yml?branch=main&label=CI)](https://github.com/GlobalSettlementNetwork/gsx-dag/actions)
[![Latest release](https://img.shields.io/github/v/release/GlobalSettlementNetwork/gsx-dag?include_prereleases&label=release)](https://github.com/GlobalSettlementNetwork/gsx-dag/releases)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.90%2B-orange.svg)](./rust-toolchain.toml)

[Roadmap](./ROADMAP.md) · [Changelog](./CHANGELOG.md) ·
[Paper](https://github.com/GlobalSettlementNetwork/gsx-papers) ·
[Security](./SECURITY.md) · [Contributing](./CONTRIBUTING.md)

</div>

---

## What is gsx-dag

`gsx-dag` is the L1 of the **Global Settlement Network**: a high-throughput
settlement chain designed to clear cross-chain transfers under a
post-quantum-conservative cryptographic posture. It's the *consensus +
execution* layer in a three-layer stack:

1. **Data availability + attestation** — LTP Commitment Nodes (paper §10).
2. **Consensus + execution** — this repo (paper §6–§7).
3. **Application** — registered-issuer precompile, Issuer Studio, Compliance
   Extension, policy-vocabulary engine (paper §8).

The execution substrate (polymorphic balance map, dual-VM projectors, OCC
scheduler, state tree, anchor pipeline, recovery replay) lives in
[`GlobalSettlementNetwork/gsx-db`](https://github.com/GlobalSettlementNetwork/gsx-db)
and is consumed here as a workspace dependency.

### What's different about this chain

- **Joint-quorum AND-gate safety** (paper Theorem 2) — a safety
  violation requires Byzantine corruption of *both* the Authority Ring
  (40 slots) *and* the Validator Ring (200 slots). Single-ring failure
  doesn't fork the chain.
- **PQ-conservative crypto** — every long-lived integrity surface uses
  NIST-standardized post-quantum primitives (ML-DSA-65 / FIPS 204,
  ML-KEM-768 / FIPS 203). Classical primitives are retained only on
  documented exception zones with migration targets.
- **Constant-size LTP commitment** — every cross-chain attestation
  commits ≈1,600 B regardless of payload size.
- **Fast-path lane** — single-owner transactions clear under
  K=4-binding equivocation slashing; 100% bond forfeiture on
  equivocation.

See [`ROADMAP.md`](./ROADMAP.md) for what's shipped and what's next.

---

## Quickstart

### Run a local 4-node devnet (Docker Compose)

```bash
git clone https://github.com/GlobalSettlementNetwork/gsx-dag.git
cd gsx-dag
docker compose -f DEVNET.yml up
```

That brings up 4 validators on `localhost:9090`–`9093` and starts
committing rounds within ~3 seconds. The faucet drops 1 GSX to any
address on request:

```bash
curl -X POST http://localhost:8080/faucet -d '{"to":"0x…20bytes…"}'
```

### Run a single validator locally

```bash
cargo build --release -p gsx-node
./target/release/gsx-node --config config.example.toml
```

### Submit an intent via JSON-RPC

```bash
curl -X POST http://localhost:9092/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"gsx_getEpoch"}'
```

The full RPC surface is documented in
[`crates/gsx-rpc/`](./crates/gsx-rpc).

### SDKs

- Rust: [`clients/rust-sdk/`](./clients/rust-sdk)
- TypeScript: [`clients/ts-sdk/`](./clients/ts-sdk)

---

## Crate map

| Crate | Paper § | Owns |
|---|---|---|
| `gsx-crypto` | §3.3, §10, §12 | ML-DSA-65, ML-KEM-768, BLS12-381, SHA3-256, Poseidon2 |
| `gsx-consensus` | §6 | Mysticeti-C integration, certificate DAG, BFT linearization |
| `gsx-authority` | §5.1 | Authority Ring (PoA): admission, certificate production |
| `gsx-validator` | §5.2 | Validator Ring (PoS): ratification, slashing |
| `gsx-fastpath` | §6.4 | FastPay-style single-owner fast-path lane |
| `gsx-execution` | §7 | Wires `gsx-db` into the DAG block executor; substrate state surface |
| `gsx-precompiles` | §8 | Registered-issuer, DID, policy-vocabulary, reserve-coverage |
| `gsx-ltp` | §10 | LTP attestation pipeline, super-node integration |
| `gsx-transport` | §6.3 | SCION + RaptorQ gossip |
| `gsx-rpc` | — | JSON-RPC + WebSocket API |
| `gsx-indexer` | — | Streaming indexer with Postgres backend |
| `gsx-node` | — | Top-level binary, config, telemetry |

---

## Architecture diagrams

Live in [`docs/visuals/`](docs/visuals/) with inline-rendered Mermaid:

- [Visual index](docs/visuals/README.md) — start here
- [GSX DAG (this repo)](docs/visuals/gsx-dag.html) ·
  [GSX DB (substrate)](docs/visuals/gsx-db.html) ·
  [LTP (attestation)](docs/visuals/ltp.html)
- [Ecosystem atlas](docs/visuals/gsx-ecosystem-atlas.html)

---

## Development

```bash
# Full check before pushing
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Exit-gate property tests (sprint closure)
PROPTEST_CASES=10000 cargo test --workspace --release
```

Sprint cadence + collaboration contract are in
[`CLAUDE.md`](./CLAUDE.md). The dependency graph and per-sprint exit
gates are in
[`docs/architecture/sprint-map.md`](./docs/architecture/sprint-map.md).

See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for the PR workflow,
review process, and branch-naming conventions.

---

## Status

- **Codebase**: pre-mainnet (`0.x` line) — see
  [`CHANGELOG.md`](./CHANGELOG.md) for shipped versions and
  [`ROADMAP.md`](./ROADMAP.md) for what's next.
- **Latest release**: see the
  [Releases page](https://github.com/GlobalSettlementNetwork/gsx-dag/releases).
- **Public devnet / testnet**: in flight (Phase 4 / Phase 5 of the
  roadmap); RPC endpoints + faucet + status page coming online during
  the G2–G8 rollout.

---

## Related repositories

| Repo | Role |
|---|---|
| [`gsx-papers`](https://github.com/GlobalSettlementNetwork/gsx-papers) | v8 academic specs (DAG L1 + LTP) |
| [`gsx-db`](https://github.com/GlobalSettlementNetwork/gsx-db) | Execution substrate (consumed here as workspace dep) |
| [`gsx-lattice-protocol`](https://github.com/GlobalSettlementNetwork/gsx-lattice-protocol) | LTP bridge gateway |
| [`op-stack-reth`](https://github.com/GlobalSettlementNetwork/op-stack-reth) | OP-stack L2 sequencer fork (Track G) |

---

## License

Apache-2.0. See [`LICENSE`](./LICENSE).

## Security

Found a vulnerability? Don't open a public issue — see
[`SECURITY.md`](./SECURITY.md) for the coordinated-disclosure process.
