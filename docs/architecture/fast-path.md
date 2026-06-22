# Fast path — single-owner lane + K-binding cross-check

**Paper §**: 6.4 — FastPay-style single-owner fast-path ([`suwappu-papers/papers/dag-l1`](https://github.com/Suwappu-Labs/suwappu-papers))
**Code**: `crates/suwappu-fastpath/src/` (lane, slashing) · `crates/suwappu-node/src/daemon.rs` (handler at `handle_fastpath_cert`, ~lines 499-820)
**IQs**: [IQ-003 — Fast-path lane architecture](../iq/IQ-003-fast-path-architecture.md)
**Visuals**: [`docs/visuals/mermaid/fast-path-and-slashing.md`](../visuals/mermaid/fast-path-and-slashing.md) *(coming with PR-2)*
**Sprint**: DAG-S8 (lane + K-binding) ✅ Closed · DAG-S9 (equivocation slashing) ✅ Closed

## What it does

Fast-path is a parallel lane for *single-owner* transactions: when a tx
touches one owned object only, an Authority signs the cert directly and
clients can take that signature as commit confirmation in one round-trip,
bypassing the full main-lane consensus latency. The lane is gated by a
K-binding cross-check (`K = 4` confirmations) against the main-lane index
so an Authority can't equivocate between fast-path and main-lane orderings
without leaving a publishable proof.

## Key invariants

- **Single-owner discipline (S8 exit gate):** `proptest_fast_path.rs` ×
  10,000 cases — a transaction touching more than one owned object is
  rejected at fast-path admission.
- **K-binding cross-check (IQ-003):** every fast-path cert is matched against
  the `main_lane_index` populated by `try_commit`; a mismatch within the
  K-window emits a slashing event.
- **100% slashing on equivocation (Invariant 5; S9 exit gate):**
  `proptest_fp_slashing.rs` × 10,000 cases — an Authority signing
  conflicting fast-path certs for the same `(object, nonce)` forfeits 100%
  of bonded stake plus immediate registry expulsion.

## Cross-references

- **Engineering:** `crates/suwappu-fastpath/src/lib.rs` (lane state machine),
  `crates/suwappu-fastpath/src/slashing.rs` (proof construction),
  `crates/suwappu-node/src/daemon.rs::handle_fastpath_cert` (handler wiring),
  the `inner.main_lane_index` populated by `try_commit` for the binding
  cross-check.
- **Spec:** Paper §6.4 + IQ-003 for the architecture choice (parallel-lane
  vs per-tx metadata).
- **Design decisions:** [IQ-003](../iq/IQ-003-fast-path-architecture.md) is
  pending sign-off; K-binding cross-check is wired but the proptest gate
  for it (IQ-003 §"Implementation sketch") is tracked separately.
- **Visual:** [fast-path-and-slashing](../visuals/mermaid/fast-path-and-slashing.md)
  *(coming with PR-2)*.
- **Safety implications:** see [safety-liveness.md](safety-liveness.md) for
  how the slashing invariant feeds Theorem 2.
