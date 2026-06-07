# IQ-002 — Indirect (retroactive) commit rule

**Status:** Ratified 2026-05-14 via [suwappu-papers#1](https://github.com/suwappu/suwappu-papers/pull/1)
**Owner:** consensus
**Date:** 2026-05-13 (ratified 2026-05-14)
**Sprint:** DAG-S21.2 ✅

## Question

Should `suwappu-consensus` implement only the direct commit rule (one-round
support check) as currently shipped, or also implement the indirect
(causal-closure / retroactive) commit rule that resolves undecided leader
slots via later anchors?

## Background

Current `crates/suwappu-consensus/src/commit.rs:93-102`:

```rust
pub fn commit_leader(dag: &DagStore, round: Round, n: CommitteeSize) -> Option<CertHash> {
    let author = leader(round, n);
    let leader_hash = cert_at(dag, round, author)?;
    let support = supporters(dag, leader_hash, round + 1);
    if support.len() as u32 >= quorum_threshold(n) {
        Some(leader_hash)
    } else {
        None
    }
}
```

This is the *direct* rule only. If round R's leader receives fewer than
`quorum_threshold(n)` direct supporters at round R+1, `commit_leader(R)`
returns `None` permanently — even if round R+3 later commits cleanly.

Paper §6.2 / §11 delegates the commit-rule details to `\cite{Mysticeti2023}`
without spelling out indirect commit. Definition 2 + Theorem 2 are stated in
terms of "a leader is committed" without distinguishing direct vs indirect.

## Evidence

- **DAG-Rider, Bullshark, Mysticeti academic** literature: indirect commit
  is canonical and essential. A leader at round R that is "undecided" by
  the direct rule is committed retroactively when a later anchor at round
  R' > R+2 is directly committed and R is in its causal history.
- **Sui Lutris** (`consensus/core/src/base_committer.rs:86-145`): ships
  **both** `try_direct_decide` AND `try_indirect_decide`. Wave length = 3
  (leader → voting → decision). `try_indirect_decide` walks back from a
  newly-directly-decided anchor and resolves all undecided slots between.
- **Decentralized Thoughts Mysticeti analysis** (2026-03-06): "If neither
  [direct-commit nor direct-skip] condition is satisfied, the slot remains
  undecided, and the validator leverages the indirect decision rule:
  (1) Find an Anchor: search for the lowest slot with round R' > R + 2 that
  is already decided by the direct commit rule. (2) Inherit Decision."
- **Our production perf testnet** (2026-05-13): exactly 1 commit in 9 hours
  (round 0 only). Every subsequent round failed its direct quorum check
  independently with no recovery path. This is the exact failure mode that
  indirect commit prevents.

## Options considered

1. **Direct + Indirect with 3-round wave** (Sui pattern). `LeaderStatus =
   Direct(Hash) | Skip | Undecided`. Direct rule at R+1, indirect rule
   resolves R from a later directly-decided anchor at R+k where k ≥ 2.
2. **Direct + reduce wave length to 2** (skip the explicit "voting round"
   layer; supporters at R+1 *and* their support at R+2 must concur).
   Untested in the literature; not recommended.
3. **Direct only, weaken quorum threshold further**. Trades safety for
   liveness. Violates Theorem 2.
4. **Direct only, accept that some rounds never commit**. Status quo.
   Causes the May-13 stall.

## Recommendation

**Option 1.** Implement the 3-round wave with both decision rules,
following Sui's `base_committer.rs` pattern. This is the most-tested
DAG-BFT commit construction in production.

Public API changes:

- New enum `LeaderStatus { Direct(CertHash), Skip, Undecided }`.
- New function `try_direct_decide(dag, slot, n) -> LeaderStatus` — current
  `commit_leader` becomes this with `Some/None → Direct/Undecided`.
- New function `try_indirect_decide(dag, slot, anchor, n) -> LeaderStatus`
  — walks causal history from `anchor` checking if `slot` is reachable.
- New function `decide_slot(dag, slot, n) -> LeaderStatus` — top-level
  driver that tries direct first, falls back to indirect using the lowest
  later anchor that is directly-decided.
- `joint_commit` updated to consume `LeaderStatus::Direct` only (Validator
  Ring vote check still applies on the decided hash).

Theorem 2 proof sketch in paper §11 must be amended to reference the
indirect rule: safety holds because both rules commit subsets of the same
total-order on the DAG, and the causal-closure walk is monotone.

## Decision

- [x] Approved by: tomasuwappu (operator)
- [x] Date: 2026-05-14
- [x] Paper Theorem 2 amendment landed in suwappu-papers PR: [suwappu/suwappu-papers#1](https://github.com/suwappu/suwappu-papers/pull/1)
- [x] Property test `indirect_commit_resolves_undecided_slots` added (10k cases)

**Ratification context.** Code shipped at `crates/suwappu-consensus/src/commit.rs:107-202`
(`LeaderStatus`, `try_direct_decide`, `try_indirect_decide`, `decide_slot`,
`finalize`) with unit tests at lines 388 (inherited-Direct) and 465
(permanent-Skip). Paper Theorem 2 proof sketch extended to cover both
direct and indirect decision rules with a monotonicity argument.
Ratified alongside IQ-001 in the same suwappu-papers PR. Tracked at
[suwappu/suwappu-dag#24](https://github.com/suwappu/suwappu-dag/issues/24).

## Implementation

- New module `suwappu-consensus/src/wave.rs` for the 3-round wave machinery.
- Refactor `commit_leader` into `try_direct_decide`.
- Add `try_indirect_decide` + `decide_slot`.
- New proptest `indirect_commit_resolves_undecided_slots` (10k cases).
- Update `joint_commit` to consume `LeaderStatus`.
- Update `commit_finality` proptest to cover indirect-commit append-only
  property (causal closure is monotone, so finality is preserved).

## Addendum: governance application is epoch-boundary atomic (Issue #18, 2026-05-14)

Phase G governance intents (`AdmitAuthority` / `ExitAuthority` /
`EjectAuthority`) are no longer applied at commit time. They are
queued in `StateInner::pending_governance` and drained when
`EpochState::boundary_crossed_by(cert_round)` fires, so every daemon
mutates the registries at the same boundary round. This closes the
transitional quorum-threshold asymmetry window (most visible on the
n=5→n=4 eject path where `quorum_threshold(5)=4 → quorum_threshold(4)=3`)
that previously stalled commits across the mesh. Non-governance
intents (`Transfer`) still execute at commit time via `execute_block`.
