# Investigation Questions (IQs)

Decisions deferred to a written ratification step. Each IQ states the
question, the evidence, the options considered, the recommendation, and
the implementation sketch. Sign-off is recorded in the **Decision**
section of the file.

For the current ratification posture of each IQ and how it lands against
mainnet readiness, see
[`docs/audit/mainnet-readiness-2026-05-14.md`](../audit/mainnet-readiness-2026-05-14.md).

| IQ | Topic | Code status | Ratification |
|---|---|---|---|
| [IQ-001](./IQ-001-quorum-formula.md) | Commit-rule quorum formula (`⌈2n/3⌉+1` vs canonical `2f+1`) | Shipped — `crates/suwappu-consensus/src/commit.rs:61-66` | **Ratified 2026-05-14** ([suwappu-papers#1](https://github.com/Suwappu-Labs/suwappu-papers/pull/1)) |
| [IQ-002](./IQ-002-indirect-commit.md) | Indirect (retroactive) commit rule | Shipped — `crates/suwappu-consensus/src/commit.rs:126-202` | **Ratified 2026-05-14** ([suwappu-papers#1](https://github.com/Suwappu-Labs/suwappu-papers/pull/1)) |
| [IQ-003](./IQ-003-fast-path-architecture.md) | Fast-path lane architecture (parallel-lane vs per-tx metadata) | Handler + proposer wired (`crates/suwappu-node/src/daemon.rs:499-820`); K-binding cross-check not wired | Pending sign-off |
| [IQ-004](./IQ-004-decide-slot-orphan-window.md) | `decide_slot` single-cert orphaning when leader cert arrives after R+1 parent-set is frozen | Test-side mitigation shipped ([#44](https://github.com/Suwappu-Labs/suwappu-dag/pull/44)); consensus-side fix tracked in [#45](https://github.com/Suwappu-Labs/suwappu-dag/issues/45) | Pending sign-off |
| [IQ-005](./IQ-005-bincode-2x-migration.md) | bincode 2.x migration + 1-byte wire-frame version marker | Shipped F4 — `crates/suwappu-node/src/codec.rs` | **Ratified 2026-05-16** |
| [IQ-006](./IQ-006-l2-state-root-commitment-surface.md) | L2 state-root commitment surface (registry account vs Checkpoint extension vs parallel state field) | Spec only — Phase G2 implementation tracked under [#89](https://github.com/Suwappu-Labs/suwappu-dag/issues/89), sub-issue [#97](https://github.com/Suwappu-Labs/suwappu-dag/issues/97) | Recommendation, pending sign-off after Phase G2 lands |
| [IQ-007](./IQ-007-fee-abstraction-and-stablecoin-fees.md) | Fee abstraction — stablecoin-denominated + sponsored fees (FEE-1) | Spec only — no fee market today | Recommendation, pending sign-off |
| [IQ-008](./IQ-008-evm-developer-surface.md) | EVM developer surface — eth_* compat vs Intent-SDK-only (EVM-1) | Dual-VM projection only; no eth_* RPC | Recommendation, pending sign-off |
| [IQ-009](./IQ-009-ltp-aggregate-pq-migration.md) | LTP aggregate PQ migration — remove classical BLS12-381 while preserving constant-size (PQ-1 / gap G-8) | BLS12-381 aggregate live in `suwappu-ltp` (documented exception) | Recommendation, pending sign-off |
| [IQ-010](./IQ-010-cross-chain-interop-adapter.md) | Cross-chain interop — OFT/CCTP-class asset-mobility adapter vs LTP-only (INTEROP-1 / gap G-9) | LTP attestation only; no asset-mobility adapter; mint path not wired | Recommendation, pending sign-off |
