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
| [IQ-001](./IQ-001-quorum-formula.md) | Commit-rule quorum formula (`⌈2n/3⌉+1` vs canonical `2f+1`) | Shipped — `crates/suwappu-consensus/src/commit.rs:61-66` | **Ratified 2026-05-14** ([suwappu-papers#1](https://github.com/suwappu/suwappu-papers/pull/1)) |
| [IQ-002](./IQ-002-indirect-commit.md) | Indirect (retroactive) commit rule | Shipped — `crates/suwappu-consensus/src/commit.rs:126-202` | **Ratified 2026-05-14** ([suwappu-papers#1](https://github.com/suwappu/suwappu-papers/pull/1)) |
| [IQ-003](./IQ-003-fast-path-architecture.md) | Fast-path lane architecture (parallel-lane vs per-tx metadata) | Handler + proposer wired (`crates/suwappu-node/src/daemon.rs:499-820`); K-binding cross-check not wired | Pending sign-off |
| [IQ-004](./IQ-004-decide-slot-orphan-window.md) | `decide_slot` single-cert orphaning when leader cert arrives after R+1 parent-set is frozen | Test-side mitigation shipped ([#44](https://github.com/suwappu/suwappu-dag/pull/44)); consensus-side fix tracked in [#45](https://github.com/suwappu/suwappu-dag/issues/45) | Pending sign-off |
| [IQ-005](./IQ-005-bincode-2x-migration.md) | bincode 2.x migration + 1-byte wire-frame version marker | Shipped F4 — `crates/suwappu-node/src/codec.rs` | **Ratified 2026-05-16** |
| [IQ-006](./IQ-006-l2-state-root-commitment-surface.md) | L2 state-root commitment surface (registry account vs Checkpoint extension vs parallel state field) | Spec only — Phase G2 implementation tracked under [#89](https://github.com/suwappu/suwappu-dag/issues/89), sub-issue [#97](https://github.com/suwappu/suwappu-dag/issues/97) | Recommendation, pending sign-off after Phase G2 lands |
| [IQ-007](./IQ-007-intent-discriminant-stability.md) | `Intent` enum discriminant stability — pre-mainnet variant-insert churn, cutover criterion, post-cutover append-only + versioned-variant patterns | Policy doc only (no code change); governs `crates/suwappu-execution/src/substrate.rs`'s `pub enum Intent` | **Ratified 2026-05-25** ([#225](https://github.com/suwappu/suwappu-dag/issues/225)) |
| [IQ-008](./IQ-008-l2-burn-merkle-inclusion.md) | `L2BurnProven` merkle inclusion scheme (leaf/inner domain tags, sibling-direction bits) — closes the escrow-drain where byte-shape-only `merkle_path` gates let any committed `batch_id` mint unlimited withdrawals | Verification scheme ratified; substrate gate at `crates/suwappu-execution/src/substrate.rs` | **Ratified 2026-05-28** |
| [IQ-009](./IQ-009-pq-onchain-value-authorization.md) | Post-quantum on-chain authorization for cross-chain value movement — joint-ring (Authority AND Validator) k-of-n ML-DSA-65 threshold as a transient witness on versioned `L1LockV2`/`L2BurnProvenV2` intents, `commit_id` nullifier for replay, committed-state epoch activation; invariant-3-safe and re-derives Theorem 2 in the PQ primitive; migration target for the BLS12-381/Groth16 exception zones (invariant 2) | Primitive shipped (`suwappu-mldsa-precompile`, 16/16); substrate wiring gated on this IQ | **v3.1** — three review rounds (R1 both BLOCK, R2 crypto BLOCK signer-vs-signature, R3 both CONCERNS no-BLOCK); design converged, implementation checklist captured, ready for human owners |
