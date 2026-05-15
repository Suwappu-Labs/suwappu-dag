# Architecture

Section-mapped engineering documentation for the GSX DAG Layer 1
implementation. Every document here corresponds to one section of the v8
academic paper (`gsx-papers/papers/dag-l1`); the paper is the design spec,
this directory is the engineering view.

## Visuals

Inline-rendered diagrams (Mermaid, native on GitHub + GitBook) sit in
[`../visuals/README.md`](../visuals/README.md). Standalone HTML
presentations: [Visual Index](../visuals/index.html) ·
[Ecosystem Atlas](../visuals/gsx-ecosystem-atlas.html) ·
[GSX DAG](../visuals/gsx-dag.html) ·
[GSX DB](../visuals/gsx-db.html) ·
[LTP](../visuals/ltp.html). Sources:
[Mermaid](../visuals/mermaid/), [Excalidraw](../visuals/excalidraw/).

## Sections

| Doc | Paper § | Topic |
|---|---|---|
| [overview.md](overview.md) | §4 | Four-layer stack on a single chain |
| [validator-rings.md](validator-rings.md) | §5 | Authority Ring (PoA) + Validator Ring (PoS) |
| [consensus.md](consensus.md) | §6 | Mysticeti-C certificate DAG + commit rule |
| [transport.md](transport.md) | §6.3 | SCION + RaptorQ inter-validator transport |
| [fast-path.md](fast-path.md) | §6.4 | FastPay-style single-owner fast-path lane |
| [execution.md](execution.md) | §7 | Co-resident dual VM over polymorphic balance map |
| [gsx-db-bridge.md](gsx-db-bridge.md) | §7 | Workspace-dep boundary to the `gsx-db` substrate |
| [application.md](application.md) | §8 | Precompiles: DID, registered-issuer, reserve-coverage |
| [super-node.md](super-node.md) | §9 | Consolidated 6-of-9 Authority super-node role |
| [ltp-integration.md](ltp-integration.md) | §10 | Lattice Transfer Protocol cross-chain settlement |
| [safety-liveness.md](safety-liveness.md) | §11 | Joint-quorum AND-gate safety (Theorem 2) |
| [cryptographic-posture.md](cryptographic-posture.md) | §12 | PQ posture + exception zones |
| [governance-phasing.md](governance-phasing.md) | §14 | Phase G2 → G3 → G4 governance |
| [sprint-map.md](sprint-map.md) | — | Sprint backlog + dependency DAG |

Investigation Questions (design decisions under ratification) live alongside the
architecture docs in [`../iq/`](../iq/). Cross-repo: the substrate's own
architecture docs are in
[`GlobalSettlementNetwork/gsx-db/docs/`](https://github.com/GlobalSettlementNetwork/gsx-db/tree/main/docs);
LTP runtime details are in
[`GlobalSettlementNetwork/gsx-lattice-protocol/docs/`](https://github.com/GlobalSettlementNetwork/gsx-lattice-protocol/tree/main/docs).

## Generated API docs

Auto-published from `main` on every merge by
[`.github/workflows/docs.yml`](../../.github/workflows/docs.yml):

- **Rust:** <https://globalsettlementnetwork.github.io/gsx-dag/rust/> — every
  workspace crate, public surface only (`cargo doc --no-deps`,
  `RUSTDOCFLAGS=-D warnings`). The landing page redirects to `gsx_node` (the
  integration crate); browse to other crates via the sidebar. The Rust SDK
  ([`gsx_client`](https://globalsettlementnetwork.github.io/gsx-dag/rust/gsx_client/))
  is the recommended entrypoint for external developers building Rust clients.
- **TypeScript:** <https://globalsettlementnetwork.github.io/gsx-dag/ts/> —
  TypeDoc for [`@gsx/client`](../../clients/ts-sdk/). The SDK README is rendered
  on the index page.

External-facing enums (`gsx_execution::Intent`, `gsx_rpc::error::RpcError`) are
marked `#[non_exhaustive]` so adding new variants is a non-breaking change for
downstream crates; consumers must include a wildcard arm when matching.
`gsx_consensus::commit::LeaderStatus` is deliberately exhaustive — its three
states (Direct / Skip / Undecided) are the paper's canonical commit-rule
outcomes (paper §6 + Theorem 2), so a fourth state would be a paper-level
amendment and a major-version bump. See
[`../../clients/rust-sdk/src/lib.rs`](../../clients/rust-sdk/src/lib.rs) and
[`../../clients/ts-sdk/README.md`](../../clients/ts-sdk/README.md) for the full
0.x → 1.0 stability policy.
