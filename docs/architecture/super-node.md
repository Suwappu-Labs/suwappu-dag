# Super-node — consolidated 6-of-9 Authority role for LTP corridors

**Paper §**: 9 — Super-node role ([`suwappu-papers/papers/dag-l1`](https://github.com/Suwappu-Labs/suwappu-papers))
**Code**: `crates/suwappu-ltp/src/` (corridor attestation) · `crates/suwappu-authority/src/` (registry)
**IQs**: —
**Visuals**: [`docs/visuals/ltp.html`](../visuals/ltp.html) (right panel + ecosystem atlas)
**Sprint**: DAG-S15 (LTP 7-of-9 attestation) ✅ Closed · DAG-S16 (DA SLA) ✅ Closed · DAG-S17 (DID STARK) ✅ Closed

## What it does

A super-node is an Authority-Ring member that runs the LTP commitment +
materialization machinery in addition to consensus participation. Per
corridor, 7 of the 9 designated super-nodes must co-sign an LTP attestation
for it to finalize. The super-node role is structural — there's no separate
runtime, just an extension of the Authority's responsibilities scoped to a
corridor registry entry.

## Key invariants

- **Quorum of 7-of-9 per corridor (S15 exit gate):** `proptest_attestation.rs`
  × 10,000 cases — fewer than 7 distinct super-node signatures fails
  attestation; 7+ produces a verifiable aggregate.
- **DA SLA bounds (S16 exit gate):** `proptest_da.rs` × 10,000 cases —
  payload reconstruction succeeds within the SLA window when the corridor
  has at least 7 super-nodes online.
- **DID STARK pipeline (S17 exit gate):** `proptest_did_stark.rs` × 10,000
  cases — cross-chain DID rotations produce a STARK proof verifiable on
  the destination base chain.

## Cross-references

- **Engineering:** `crates/suwappu-ltp/src/attestation.rs` (the 7-of-9 aggregate
  signature path), `crates/suwappu-authority/src/lib.rs` (registry that names
  super-nodes per corridor).
- **Spec:** Paper §9 + §10 (LTP integration in [ltp-integration.md](ltp-integration.md)).
- **Design decisions:** super-node role is described in the paper; no IQ
  has been opened against it.
- **Visual:** the LTP-stack panel of [suwappu-dag.html](../visuals/suwappu-dag.html)
  and [suwappu-ecosystem-atlas.html](../visuals/suwappu-ecosystem-atlas.html) both
  include "Authority Ring 30–50 · Super-node 7/9".
