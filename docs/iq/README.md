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
| [IQ-001](./IQ-001-quorum-formula.md) | Commit-rule quorum formula (`⌈2n/3⌉+1` vs canonical `2f+1`) | Shipped — `crates/gsx-consensus/src/commit.rs:61-66` | **Ratified 2026-05-14** ([gsx-papers#1](https://github.com/GlobalSettlementNetwork/gsx-papers/pull/1)) |
| [IQ-002](./IQ-002-indirect-commit.md) | Indirect (retroactive) commit rule | Shipped — `crates/gsx-consensus/src/commit.rs:126-202` | **Ratified 2026-05-14** ([gsx-papers#1](https://github.com/GlobalSettlementNetwork/gsx-papers/pull/1)) |
| [IQ-003](./IQ-003-fast-path-architecture.md) | Fast-path lane architecture (parallel-lane vs per-tx metadata) | Handler + proposer wired (`crates/gsx-node/src/daemon.rs:499-820`); K-binding cross-check not wired | Pending sign-off |
| [IQ-004](./IQ-004-decide-slot-orphan-window.md) | `decide_slot` single-cert orphaning when leader cert arrives after R+1 parent-set is frozen | Test-side mitigation shipped ([#44](https://github.com/GlobalSettlementNetwork/gsx-dag/pull/44)); consensus-side fix tracked in [#45](https://github.com/GlobalSettlementNetwork/gsx-dag/issues/45) | Pending sign-off |
| [IQ-005](./IQ-005-bincode-2x-migration.md) | bincode 2.x migration + 1-byte wire-frame version marker | Shipped F4 — `crates/gsx-node/src/codec.rs` | **Ratified 2026-05-16** |
