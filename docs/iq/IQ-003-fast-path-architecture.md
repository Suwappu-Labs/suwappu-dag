# IQ-003 — Fast-path lane architecture

**Status:** Ratified 2026-05-15 — parallel-lane architecture shipped (DAG-S8 + DAG-S9 closed); K-binding cross-check live in `crates/suwappu-node/src/daemon.rs::handle_fastpath_cert` (lines 837-872) with integration test `K_binding_violator_is_slashed` at lines 2556+
**Owner:** consensus + fastpath
**Date:** 2026-05-13 (ratified 2026-05-15)
**Sprint:** DAG-S22 (downstream of S21) ✅

## Question

Should the fast-path lane be implemented as a **parallel cert lane** (paper
§6.4, current design) with its own `FastPathCert` wire message and dedicated
quorum aggregation, or as **per-transaction metadata woven into the
main-lane DAG block** (Sui Mysticeti-FPC pattern)?

## Background

Paper §6.4 specifies a parallel fast-path certificate lane with:
- `⌈(2/3)|A|⌉ + 1` Authority Ring quorum (same formula as IQ-001)
- K=4 binding window — fast-path cert is bound to the main-lane round
  R such that the cert is committed iff the main-lane round R+K commits
- 100% slashing for fast-path equivocation (paper Invariant 5)

Current state:
- `crates/suwappu-fastpath/` ships the cert type, quorum aggregator, K-binding
  proptest, and equivocation slashing proptest (DAG-S8 + S9).
- `crates/suwappu-node/src/wire.rs:91` defines `WireMessage::FastPath(FastPathCert)`.
- `crates/suwappu-node/src/daemon.rs:296-298` handler is a **no-op**:
  ```rust
  WireMessage::FastPath(_) | WireMessage::Ltp(_) => {
      // Lanes handled in follow-on commit. Ignored on the main lane.
  }
  ```
- 100% slashing for fast-path equivocation is **unobservable** on a live
  cluster because the daemon doesn't process fast-path messages.

## Evidence

- **Sui Mysticeti-FPC** (blog.sui.io/mysticeti-v2-sui-consensus): fast path
  is not a separate gossip lane. Fast-path transactions are woven into the
  same DAG, marked with per-tx finalization metadata, and finalized via
  reliable broadcast when 2f+1 Authority votes accumulate *through the same
  DAG block stream*.
  - Equivocation model: object lock until end-of-epoch. No validator
    slashing — the client equivocator is self-DoSed.
- **Sui `consensus/core/src/commit_finalizer.rs:480-495`**:
  ```rust
  let reject_votes: BTreeMap<TransactionIndex, Stake> = self
      .transaction_vote_tracker
      .get_reject_votes(block_ref)
      ...
      if reject_stake < self.context.committee.quorum_threshold() { /* finalize */ }
  ```
- **Paper §6.4 (our design)**: explicit parallel lane with `FastPathCert`,
  K=4 binding, 100% validator slashing for equivocation. Engineered for
  permissioned-Authority correctness; differs deliberately from Sui's
  permissionless-Authority model.

## Options considered

1. **Keep parallel-lane design + wire it into the daemon** (DAG-S22).
   Honors paper §6.4 verbatim. Daemon adds:
   - Fast-path proposer step (single-owner intents bypass main DAG).
   - Fast-path receiver: aggregate quorum, K-binding check vs main-lane
     round R+4.
   - Equivocation detector: cross-check fast-path certs vs main-lane
     ordering, emit slashing event.
   - 100% slashing of bonded stake — paper Invariant 5.
2. **Migrate to Sui-style per-tx metadata in main-lane blocks.** Simpler
   wire surface (no `FastPathCert` message). But changes the slashing model
   from validator-slashing to client-self-DoS — breaks paper Invariant 5.
3. **Defer fast-path entirely.** Keep `suwappu-fastpath` as a library, never
   wire into the daemon. Acceptable for testnet; not acceptable for
   mainnet (loses the "sub-second finality for single-owner" property the
   paper claims).

## Recommendation

**Option 1** — keep the parallel-lane design and wire it into the daemon
in DAG-S22 (downstream of S21). This honors paper §6.4 and Invariant 5
verbatim, preserves the validator-slashing model the rest of the protocol
assumes, and exercises the existing 8 fast-path proptests at runtime.

The architectural divergence from Sui is intentional: our protocol is
permissioned (Authority Ring is a stake-gated PoA set), while Sui's is
permissionless. Validator-slashing for fast-path equivocation is the
correct enforcement primitive for our model.

Sequence: ship S21 (correctness) first; S22 (fast-path wiring) follows
once the main lane is committing reliably.

## Decision

- [ ] Approved by: ______________
- [ ] Date: ______________

## Implementation (DAG-S22, post-S21)

- New module `suwappu-node/src/fastpath_lane.rs`.
- Fast-path proposer: detect single-owner intents in `pending_intents`,
  emit `FastPathCert` instead of main-lane block.
- Fast-path receiver: aggregate quorum per fast-path cert.
- K-binding verifier: check fast-path cert vs main-lane round R+4 commit.
- Slashing emitter: cross-check fast-path equivocation, emit
  `EquivocationProof` via main-lane event log.
- New integration test: 4-node cluster, one Byzantine node signs
  conflicting fast-path certs, assert 100% slashing within 1 epoch.
