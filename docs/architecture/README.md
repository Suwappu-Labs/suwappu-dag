# Architecture

Section-mapped engineering documentation for the GSX DAG Layer 1
implementation. Every document here corresponds to one section of the v8
academic paper (`gsx-papers/papers/dag-l1`); the paper is the design spec,
this directory is the engineering view.

| Doc | Paper § | Topic |
|---|---|---|
| [overview.md](overview.md) | §4 | Four-layer stack on a single chain |
| [validator-rings.md](validator-rings.md) | §5 | Authority Ring (PoA) + Validator Ring (PoS) |
| [consensus.md](consensus.md) | §6 | Mysticeti-C certificate DAG + commit rule |
| [transport.md](transport.md) | §6.3 | SCION + RaptorQ inter-validator transport |
| [fast-path.md](fast-path.md) | §6.4 | FastPay-style single-owner fast-path lane |
| [execution.md](execution.md) | §7 | Co-resident dual VM over polymorphic balance map |
| [application.md](application.md) | §8 | Precompiles: DID, registered-issuer, reserve-coverage |
| [super-node.md](super-node.md) | §9 | Consolidated 6-authority super-node role |
| [ltp-integration.md](ltp-integration.md) | §10 | Lattice Transfer Protocol cross-chain settlement |
| [safety-liveness.md](safety-liveness.md) | §11 | Joint-quorum AND-gate safety theorem |
| [cryptographic-posture.md](cryptographic-posture.md) | §12 | PQ posture + exception zones |
| [governance-phasing.md](governance-phasing.md) | §14 | Phase G2 → G3 → G4 governance |
| [sprint-map.md](sprint-map.md) | — | Sprint backlog + dependency DAG |
