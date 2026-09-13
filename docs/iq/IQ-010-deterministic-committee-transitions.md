# IQ-010 — Deterministic committee transitions: the committee is a function of the commit sequence

**Status:** Recommendation, implemented on branch `claude/kasper-research-n9i4ct`
as sprint DAG-S36. Consensus-touching (leader rotation, quorum counting,
membership transitions, slashing evidence): requires `consensus-reviewer`
and **human consensus-team sign-off** before it is treated as
production-ready (same posture as IQ-007, IQ-008 and IQ-009).
**Owner:** consensus
**Date:** 2026-09-13
**Tracking:** `/goal` item A12 (added by this IQ). Opened by IQ-009
Residual 9 (the seventh DAG-S35 review pass named it the residual worth
the most scrutiny); the investigation found the rotation itself does not
survive a membership change either.

## Question

Which authorities form the committee that the DagBft-C commit rule runs
over when a node decides a leader slot, and when does that committee
change? Today the answer differs between honest nodes, and after any
change it is not even the set of seated authorities.

## Background: three defects on one surface

1. **The rule indexes authorities by `0..n`, not by membership.**
   `leader(round, n) = round mod n`, `parents_for_round` and
   `distinct_authors_at` iterate `0..n`, and `supporters` counts every
   author present in the DAG. Ids are caller-chosen (`AdmitAuthority
   { authority_id }`) and `AuthorityRegistry::remove` leaves gaps, while
   `n` is the registry *length*. After ejecting any id that is not the
   highest, `n` shrinks and the ids do not renumber: the leader slot of
   the missing id is skipped every `n`-th round, and the highest id is
   `≥ n`, so no honest proposer ever selects its certificates as parents
   and it can never support a leader. A four-node ring that ejects id 1
   has `n = 3`, threshold 3, and only ids 0 and 2 reachable as
   supporters — a permanent halt. Admitting a non-contiguous id (7 into
   a ring of four) never gives it a leader slot or a parent edge at all.
   The consensus-reviewer checklist names this class
   (`dagbft-leader-rotation-needs-active-manifest`); the code has it.

2. **Deferred activation fires on local admission** (Issue #18 /
   DAG-S27.7). A newly admitted authority is in the registries (its
   certificates verify and are admitted) but `n` and its stake are bumped
   in `ingest_cert` when *this node* first admits one of its certificates.
   Two consequences:
   - *Non-determinism.* Node P admits the first certificate (with its
     block, IQ-009) one round before node Q; in that window P decides
     leaders with `n + 1` and Q with `n`, so they can commit different
     certificates for one round — a substrate fork with zero Byzantine
     behaviour (IQ-009 Residual 9, sixth and seventh review passes).
   - *A quorum-intersection gap.* In the window the pending member's
     certificates already count as supporters while `n` excludes it, so
     two threshold-sized support sets among `n + 1` authors need only
     intersect in one author, who may be Byzantine. DagBft-C's safety
     argument needs an honest author in the intersection.

3. **The equivocation auto-eject fires on local detection**
   (DAG-S30.1). A node that admits two headers for one `(author, round)`
   rewrites its registries, stake table and `n` in the next `try_commit`;
   a node that received only one header does not. Same fork.

A fourth detail makes even a correctly gated change take effect at
different rounds on different nodes: `try_commit` reads `n` once per walk,
so a change applied by an epoch-boundary commit *inside* a walk is used by
the rest of that walk on one node and by the next slot on another. This
is the "jitter" Issue #18 observed and worked around by moving governance
to the boundary; the boundary alone does not remove it.

## What the literature says

- **Sui `consensus-core` / `Committee`** (`consensus/config/src/committee.rs`):
  the committee is a per-epoch, index-addressed set (`AuthorityIndex`
  is contiguous *within an epoch* precisely because the committee is
  rebuilt at every epoch); leader schedule, quorum thresholds and stake
  are all read from that object; it changes only through the end-of-epoch
  transaction, which is itself executed in the committed sequence.
- **Mysticeti (arXiv:2310.14821), §III–IV.** Leader slots are defined
  over the committee of the epoch; reconfiguration happens between
  epochs, never mid-epoch and never from one validator's observation.
- **Tendermint / CometBFT.** Validator-set updates returned by `EndBlock`
  take effect at height `H + 2`, a function of the committed chain;
  `DuplicateVoteEvidence` is gossiped, *included in a block*, and applied
  when that block executes — never on local observation
  (Buchman, "Tendermint: Byzantine Fault Tolerance in the Age of
  Blockchains", 2016; CometBFT spec, "Evidence").
- **Ethereum consensus specs.** `ProposerSlashing` / `AttesterSlashing`
  are block operations; the validator set changes only through the
  state transition of the beacon chain (activation and exit queues), so
  every node derives the same active set from the same finalized state.

The common invariant: **the committee in force for deciding a slot is a
deterministic function of the committed sequence, and membership
evidence — a liveness proof or a slashing proof — is carried in blocks.**

## Options considered

1. **Renumber ids on every membership change** so `0..n` stays valid.
   Breaks the binding between an authority's id and its registered key
   (certificates are signed under the id), and every stored certificate,
   vote and stake row would have to be rewritten. Rejected.
2. **Keep the `0..n` rule and restrict governance** (admit only
   `id == n`, eject only the highest id). Not a validator set; rejected.
3. **Committee-indexed rule with boundary-only, commit-derived
   transitions.** The committee is the sorted set of active member ids;
   the leader of round `r` is `committee[r mod |C|]`; supporters and
   quorum counts range over `C`; parents may be any registered author's
   certificates. The committee changes only in the epoch-boundary
   governance drain, and the two observation-driven transitions become
   commit-derived: activation requires a *committed* certificate of the
   pending member; ejection requires *committed* equivocation evidence.
   The commit walk re-reads the committee before every slot decision.

## Decision

Option 3.

### D1. The commit rule is defined over a `Committee`

`suwappu_consensus::Committee` is a sorted, de-duplicated vector of
authority ids. `leader_of(round, &committee)` is
`committee[round mod |C|]`; `quorum_threshold(|C|)` is unchanged;
`supporters` counts only committee members; `decide_slot_for`,
`try_direct_decide_for`, `try_indirect_decide_for`, `finalize_for` and
`joint_commit_for` take the committee. The `n`-based functions remain as
wrappers over `Committee::contiguous(n)` so every DAG-S3..S5 gate is
unchanged; the new gate proves the two agree on contiguous committees and
that the committee rule is invariant under relabelling and under
certificates by non-members.

### D2. Membership has two states; both change only at the boundary

- **Registered**: in the Authority (and mirrored Validator) registry.
  Its certificates verify, are admitted, may be selected as parents by
  honest proposers and therefore may be committed; its votes carry no
  stake; it holds no leader slot and does not count toward a quorum.
- **Active**: additionally in the committee and the stake table.

The only code path that changes the committee, the registries or the
stake table is `apply_governance_intent`, run in the epoch-boundary drain
of `apply_commit` — a function of the commit sequence. Activation of a
registered member happens at the first boundary *after one of its
certificates has been committed* (`live_proven`, a commit-derived set
carried in the snapshot root), which keeps Issue #18's liveness property
— an authority that never shows up never enters the denominator or the
rotation — without any node-local observation. `pending_stake` and the
`ingest_cert` promotion are removed.

### D3. Ejection is by on-chain evidence

`Intent::EquivocationEvidence { cert_a, cert_b }` carries two full
certificates. Any node whose DAG holds two headers for one
`(author, round)` emits the intent in its next block (at most
`MAX_EVIDENCE_PER_BLOCK`). At commit, every node verifies the evidence
against the seated registry — same author and round, distinct hashes,
both signatures valid under the author's registered key, author seated
— and queues the ejection for the boundary drain; invalid or duplicate
evidence is dropped. The substrate treats the intent as a no-op. The
DAG-S30.1 local auto-eject is removed; `EjectAuthority` (governance
co-signed) remains for evidence the protocol does not carry.

### D4. The commit walk reads the committee per slot

`try_commit` reads the committee (and the stake table) immediately before
each `decide_slot_for`, after any `apply_commit` in the walk may have
crossed a boundary, so the slot after a boundary is decided with the new
committee on every node regardless of how the walk was chunked.

### D5. The fast path uses the committee

Fast-path signers must be committee members; the fast-path quorum size is
derived from `|C|`.

### D6. Checkpoints and snapshots bind the committee

`RegistrySet` carries the committee (replacing `n_authorities`), so
`registry_root` (V2) commits to it; `SnapshotBody` carries `live_proven`,
so `snapshot_root` (V2) commits to the activation state. A served snapshot
is rejected unless its committee is a subset of its Authority registry.

## Invariants

- **I-CT1 (determinism).** The committee used to decide leader round `r`
  is a function of the committed sequence up to that decision. Two nodes
  with the same committed prefix decide `r` with the same committee.
- **I-CT2 (rotation covers the committee).** Every leader slot names a
  committee member, and every member holds slots.
- **I-CT3 (counting matches the denominator).** Only committee members'
  certificates count as support; the quorum-intersection argument holds
  over `|C|`.

## Exit gate

- `crates/suwappu-consensus/tests/proptest_committee.rs` × 10k:
  `contiguous_committee_matches_n_rule`,
  `decisions_are_invariant_under_relabelling`,
  `non_member_certificates_never_count`,
  `committee_finality_is_monotone`.
- Daemon: `middle_ejection_keeps_the_mesh_committing` (four nodes; id 1
  equivocates, evidence is committed, every node ejects at the same
  boundary and commits keep flowing with agreeing roots),
  `activation_is_a_commit_sequence_function` (a non-contiguous id is
  admitted, activates at the first boundary after a committed
  certificate on every node, and then holds leader slots),
  `bogus_evidence_is_dropped`, plus the IQ-007 and IQ-009 gates
  unchanged.

## Residuals

1. **IQ-004 late flips** reorder the commit sequence between nodes for
   everything commit-derived — governance, activation, ejection and the
   substrate alike. This IQ moves membership into that class; it does not
   close the class. Tracked in IQ-004 / #45.
2. **Activation and ejection wait for the boundary** (up to
   `rounds_per_epoch`). An equivocator stays seated until then; its
   certificates are capped at two per slot and it is within `f`.
3. **Registered, inactive members can grow the DAG** (their certificates
   are admitted and may be parents). Bounded by the per-slot cap, the
   ring ceiling (50) and the retention window.
4. **The Validator Ring is still a mirror** of the Authority Ring
   (`/goal` A8); stake weight activation follows the committee.
5. **Evidence size.** Two ML-DSA-65-signed certificates ≈ 7 KB per
   intent, at most `MAX_EVIDENCE_PER_BLOCK` per block; a node that has
   emitted evidence for a slot does not re-emit it.
6. **The consensus-reviewer checklist item 2** ("use the `pending_stake`
   deferred-activation pattern") is superseded: activation is still
   deferred until liveness is proven, but the proof is a committed
   certificate, not a local admission.
7. **Human sign-off** is required as for IQ-007/008/009: this changes the
   leader schedule and the quorum denominator's definition.
