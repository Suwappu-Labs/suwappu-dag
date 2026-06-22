# Safety + liveness — joint-quorum AND-gate (Theorem 2)

**Paper §**: 11 — Safety + liveness ([`suwappu-papers/papers/dag-l1`](https://github.com/Suwappu-Labs/suwappu-papers))
**Code**: `crates/suwappu-consensus/src/joint.rs` · `crates/suwappu-consensus/src/commit.rs`
**IQs**: [IQ-001 quorum formula](../iq/IQ-001-quorum-formula.md) · [IQ-002 indirect commit](../iq/IQ-002-indirect-commit.md) · [IQ-004 decide_slot orphan window](../iq/IQ-004-decide-slot-orphan-window.md)
**Visuals**: [`docs/visuals/mermaid/commit-rule.md`](../visuals/mermaid/commit-rule.md) *(coming with PR-2)*
**Sprint**: DAG-S5 (joint-quorum AND-gate) ✅ Closed · DAG-S21.2 (indirect commit) ✅ Closed

## What it does

The chain's safety guarantee — **paper Theorem 2** — is that a safety
violation requires Byzantine corruption of *both* rings simultaneously
(the AND-gate). The Authority Ring decides ordering via Mysticeti-C
direct + indirect commit; the Validator Ring co-signs ordering via
stake-weighted joint quorum. Either ring alone cannot finalize a cert
that conflicts with the other's vote.

Liveness inherits from Mysticeti-C plus the indirect-decide path
(IQ-002): so long as honest authorities advance round-on-round and
≥`f+1` parents arrive within the leader-timeout window, the commit rule
keeps making forward progress. The one currently known liveness gap is
the `decide_slot` single-cert orphan window documented in IQ-004 — a
one-shot intent can be skipped if its leader cert arrives at peers
after they've already proposed at round R+1; mitigation is
[client-side resubmit + bigger deadline](../iq/IQ-004-decide-slot-orphan-window.md);
real fix tracked in [#45](https://github.com/Suwappu-Labs/suwappu-dag/issues/45).

## Key invariants

- **Joint-quorum AND-gate (Invariant 1, paper Theorem 2; S5 exit gate):**
  `proptest_joint_quorum.rs` × 10,000 cases — no candidate cert is
  finalized unless both `commit_leader` (Authority direct/indirect) and
  `validator_quorum_met` (Validator stake-weighted) agree.
- **Quorum integer-encoding (IQ-001):** `quorum_threshold(n) = n − ⌊(n−1)/3⌋`
  i.e. `2f+1` for `n=3f+1` — see `crates/suwappu-consensus/src/commit.rs:61-66`.
  The paper's `⌈2n/3⌉+1` collapses to unanimity for `n ∈ {1,4,7,…}` so the
  production formula diverges per IQ-001. Theorem 2's safety proof remains
  valid under this integer encoding.
- **Indirect commit (IQ-002):** `decide_slot` resolves a slot via the
  lowest directly-decided anchor at round ≥ R+2; this closes the
  paper-vs-production liveness gap for undecided rounds.

## Cross-references

- **Engineering:** `crates/suwappu-consensus/src/commit.rs` (quorum_threshold,
  try_direct_decide, try_indirect_decide, decide_slot, causal_history);
  `crates/suwappu-consensus/src/joint.rs` (validator_quorum_threshold,
  validator_quorum_met).
- **Spec:** Paper §11 + Definition 2 + Theorem 2.
- **Design decisions:** IQ-001 + IQ-002 ratified 2026-05-14 via
  [suwappu-papers#1](https://github.com/Suwappu-Labs/suwappu-papers/pull/1).
  IQ-004 pending.
- **Visual:** [commit-rule](../visuals/mermaid/commit-rule.md) *(coming
  with PR-2)* will show the direct → indirect → joint path including
  the IQ-004 orphan window.
- **Operator-side context:** under jitter, one slow daemon's round-R+1
  cert may not include the leader's R cert as parent → slot skipped →
  one-shot intents dropped. See IQ-004 + the
  `dag-decide-slot-single-cert-orphan-after-parent-set-frozen` skill.
