# gsx-dag

**GSX DAG Layer 1** — a Mysticeti-style certificate-DAG settlement chain with a
dual-ring validator set, co-resident dual virtual machine, and post-quantum
cross-chain attestation.

This repository implements the consensus, execution wiring, fast-path lane,
application precompiles, LTP integration, and inter-validator transport
described in the v8 academic paper (`GlobalSettlementNetwork/gsx-papers`,
*GSX DAG Layer 1*).

The execution substrate (polymorphic balance map, dual-VM projectors, OCC
scheduler, state tree, anchor pipeline, recovery replay) is implemented in
[`GlobalSettlementNetwork/gsx-db`](https://github.com/GlobalSettlementNetwork/gsx-db)
and consumed here as a workspace dependency.

## Architecture

Four logical layers on a single chain (paper §4):

1. **Data availability and attestation** — LTP Commitment Nodes (§10)
2. **Consensus** — Mysticeti-C certificate DAG (§6)
3. **Execution** — co-resident dual VM (EVM + Move) over the polymorphic balance map (§7)
4. **Application** — registered-issuer precompile, Issuer Studio, Compliance Extension, policy-vocabulary engine (§8)

## Crate map

| Crate | Paper § | Owns |
|---|---|---|
| `gsx-crypto` | §3.3, §10, §12 | ML-DSA-65, ML-KEM-768, BLS12-381, SHA3-256, Poseidon2 |
| `gsx-consensus` | §6 | Mysticeti-C integration, certificate DAG, BFT linearization |
| `gsx-authority` | §5.1 | Authority Ring (PoA): admission, certificate production |
| `gsx-validator` | §5.2 | Validator Ring (PoS): ratification, slashing |
| `gsx-fastpath` | §6.4 | FastPay-style single-owner fast-path lane |
| `gsx-execution` | §7 | Wires `gsx-db` into the DAG block executor |
| `gsx-precompiles` | §8 | Registered-issuer, DID, policy-vocabulary, reserve-coverage |
| `gsx-ltp` | §10 | LTP attestation pipeline, super-node integration |
| `gsx-transport` | §6.3 | SCION + RaptorQ gossip |
| `gsx-node` | — | Top-level binary, config, telemetry |

## Build

```bash
cargo build --workspace
cargo test --workspace
PROPTEST_CASES=10000 cargo test --workspace --release
```

## Sprint cadence

Each sprint closes a load-bearing invariant via a 10,000-case property test.
See `docs/architecture/sprint-map.md` for the full schedule. See `CLAUDE.md`
for the collaboration contract used by Claude Code in this repo.

## License

Apache-2.0. See `LICENSE`.
