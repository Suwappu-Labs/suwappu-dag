# IQ-009 — Certificate availability: a certificate enters the DAG only with its block

**Status:** Recommendation, implemented on branch `claude/kasper-research-n9i4ct`
as sprint DAG-S35. Consensus-touching (admission rule, parent selection,
vote semantics): requires `consensus-reviewer` and **human consensus-team
sign-off** before it is treated as production-ready (same posture as
IQ-007 and IQ-008).
**Owner:** consensus
**Date:** 2026-09-08
**Tracking:** `/goal` item A11 (added by this IQ). Found by the DAG-S34
restart test; closes the "crashed author halts the mesh" class that
IQ-008 recorded as a pre-existing liveness hole.

## Question

A certificate is a single-author ML-DSA-signed header carrying
`payload_digest`; the block (intents + governance envelopes) travels as a
separate wire frame. Today a node inserts a certificate into its DAG as
soon as the header verifies, whether or not it holds the block. What
happens when the block never arrives anywhere — and what is the rule that
makes that impossible to matter?

## Background: the halt, observed

`try_commit` never applies a certificate whose authentic block it does not
hold: committing an empty block in its place would diverge the substrate
from every node that has the block, so the walk defers *entirely* — no
later leader may commit ahead of it (S31 consensus review, halt-not-fork).
That rule is correct. Its precondition is that the block is retrievable
from *someone*.

That precondition is not guaranteed. The round driver broadcasts the
block and then the certificate (`daemon.rs`, phase 4); a crash, a kill, or
a dropped connection between the two frames leaves every peer holding a
certificate no live node can serve a block for. If that certificate is a
leader, or lies in any later leader's causal history, every honest node
defers forever. The DAG-S34 restart test reproduced it: with one of four
validators stopped for four seconds, the three survivors' commit frontier
did not move until it returned and served the block from its restored
store. A public testnet with external validators sees crashes daily.

## What the literature says

- **Narwhal (Danezis, Kokoris-Kogias, Sonnino, Spiegelman; EuroSys 2022),
  §4.2.** A block is broadcast, workers/validators acknowledge storage,
  and `2f+1` acknowledgements form a *certificate of availability*. A
  header may reference only certificates, so every reference is a proof
  that `f+1` honest nodes store the payload and any node can retrieve it.
  Availability is a property of the DAG edge, not of luck.
- **Mysticeti (Babel et al., arXiv:2310.14821), §III.** The block *is* the
  DAG vertex: a node adds a block to its local DAG only once it has the
  full block and all its ancestors, and it references only blocks it
  holds. There is no separate payload to lose. Bullshark inherits
  Narwhal's certificates.
- **Sui consensus-core.** `BlockVerifier` rejects a block whose
  ancestors are not held; `DagState::accept_block` is the single
  admission point and requires the whole block. Commit never waits on a
  payload fetch.

The common invariant: **held in the DAG ⇒ payload held; referenced ⇒
retrievable from the referrer.** Our design lost it by letting the header
and the payload travel and be admitted independently.

## Options considered

1. **Narwhal availability certificates.** Add `2f+1` storage acks to the
   certificate before it can be referenced. Sound, but a wire and
   signature-format change to the certificate itself (ML-DSA-65
   signatures are ~3.3 KB; `2f+1` of them per certificate) and a new
   round trip before every proposal. Too large for the launch path.
2. **Commit an empty block when the payload is missing.** Rejected in
   the S31 review: nodes diverge on availability, so this forks the
   substrate with zero Byzantine behaviour.
3. **Admit a certificate only together with its block** (Mysticeti's
   rule, applied to our two-frame wire). A certificate whose block is not
   held is parked, not inserted; the block is fetched from the sender and
   one other peer; on arrival the parked certificate is admitted through
   the ordinary path (signature, parents, vote, commit). Parents are
   chosen only from the DAG, so an honest referrer holds every parent's
   block; the Validator-Ring vote is cast only at admission, so a vote
   doubles as an availability attestation. No certificate format change;
   no new round trip for the honest author (block precedes certificate on
   the wire already).

## Decision

Option 3.

### D1. Admission requires the block

`ingest_cert` verifies the signature, then requires a held block whose
`payload_digest` equals the certificate's. Absent, the certificate is
parked in `awaiting_block` (bounded like the orphan buffer) and
`GetBlock` is sent to the sender and one other peer; the sync sweeper
retries with the existing back-off and drops parked certificates once
their round falls below the gc round. A held block that does *not* match
the signed digest (a relay-poisoned squatter) is evicted at this point
and refetched. When the block arrives (`handle_block`), the parked
certificate is re-ingested and, if seated, voted for.

A block that arrives before its certificate is known is held as one of
at most two candidates per hash (bounded in total) and bound to the
signed `payload_digest` only when the certificate is admitted; a relay
that pre-squats a wrong payload therefore cannot cause the authentic
block to be dropped (the property test found exactly that interleaving
under first-write-wins).

### D2. The wire serves certificate and block together

`GetCert` and `GetCertsByRound` replies send the block *before* the
certificate so the receiver admits immediately; the round driver stores
its own block before inserting its own certificate. Live gossip keeps
the existing block-then-certificate order.

### D3. Consequences for the commit rule (unchanged code, changed
precondition)

- A leader decided `Direct` has a quorum of referrers; at least `f+1`
  are honest and hold its block, so the deferral in `try_commit` can no
  longer be permanent: it waits on a fetch, not on a resurrection. The
  deferral stays as defence in depth.
- A certificate no honest node can complete is never inserted by an
  honest node, never becomes a parent of an honest certificate, and
  therefore never enters an honest node's causal history. A Byzantine
  certificate referencing it stays an orphan on every honest node —
  identically, so the *DAG* does not diverge. The DAG-S30.1 auto-eject
  is the exception: it fires on local admission of a second header for
  one slot, and admission is now a function of block delivery, which
  the equivocator controls per peer. See Residual 9.
- Votes are cast at admission only, so `validator_quorum_met` for a
  leader also certifies that a stake quorum holds its block (a weaker
  cousin of Narwhal's availability certificate, at zero wire cost).

### D4. Snapshots

A served snapshot's certificate window must carry a matching block for
every certificate (`verify_served_snapshot`); the capture side satisfies
this by construction. The recovery replay already carries block and
certificate in one record.

### D5. Fault injection

`State::withhold_blocks` is the first fault-injection knob for `/goal` B2:
when set, the round driver broadcasts certificates but not blocks. The
exit-gate test sets it on one validator, lets it author a few
certificates (including as leader), stops the validator, and asserts the
other three keep committing and agree on their roots. Under the previous
rule this halts at the first withheld leader certificate.

## Invariant candidate

- **I-AV1 (availability by reference).** For every certificate in an
  honest node's DAG, that node holds the certificate's block, and every
  parent of that certificate is in its DAG (or tombstoned below the gc
  round). Exit gates: `availability_is_an_admission_invariant` (× 10k:
  the invariant after every event of an arbitrary interleaving of
  certificate, block and wrong-payload-block arrivals with an optional
  prune, plus admission liveness without a prune),
  `cert_is_admitted_only_with_its_block` (unit) and
  `withholding_author_does_not_stall_the_mesh` (four-node, fault
  injected). The property models one equivocating author and a
  poisoned-round block, so it also pins the two-per-(author, round) cap
  for parked and admitted certificates, binds blocks to certificates on
  digest, author and round, and asserts the DAG-S30.1 auto-eject (the
  proof is drained explicitly at the end: the author is unseated iff two
  of its headers were admitted in one slot); admission liveness is
  asserted for non-equivocating authors, plus a positive control that a
  directly admissible header of the equivocator is admitted.

## Residuals

1. **Bounded parking.** A certificate is parked only after the same
   gc floor, round-window ceiling and two-per-(author, round) cap that
   gate admission (all run before the block check; the per-slot cap is
   re-evaluated under the DAG write guard at insert so it is atomic
   across inbox tasks), and the parking buffer applies the per-slot cap
   to parked certificates as well. A seated Byzantine author therefore
   holds at most `2 × (2 × gc_depth + lag)` parked headers (≈1,024 at
   `GC_DEPTH = 256`), and `f` of them ≈ `1,024 f` — which exceeds the
   4,096-entry buffer at n ≥ 13. The buffer therefore evicts rather
   than refusing honest late arrivals: the victim is the author with the
   most (author, round) slots holding two parked headers — an
   equivocation signature no honest author has — then the most parked
   overall, and its lowest-round entry goes (closest to the gc reap,
   least needed at the frontier); an honest author whose blocks are
   being withheld holds one parked header per round and is never
   preferred over an equivocator. Block
   fetches use the certificate leg's per-hash exponential back-off, a
   per-tick budget of 256 frames per leg (oldest-due first), and a
   two-peer fan-out whose start rotates with the hash *and the attempt*
   over a sorted peer list, so successive retries of one hash walk every
   peer and a block held by exactly one of them is eventually asked of
   it. `suwappu_getSyncStatus.awaiting_block` counts parked
   certificates separately from `needed_blocks`.
2. **Latency.** A certificate whose block is delayed is admitted late;
   the author's own vote and its peers' votes follow the block. This is
   the Narwhal/Mysticeti cost model and is paid per certificate, not
   per commit.
3. **Blocks are still not committed to by the certificate chain beyond
   `payload_digest`.** A relay can delay a block; it cannot substitute
   one (the digest is signed). Unchanged from S31.
4. **Human sign-off** is required as for IQ-007/008: this changes which
   certificates an honest node references, hence the DAG topology under
   partial block loss, and must be reviewed as a consensus change. The
   Validator-Ring vote as an implicit availability attestation (D3) is a
   new load-bearing semantic for `validator_quorum_met` and belongs in
   the paper-facing text.
5. **Votes for never-admitted candidates.** `store_vote` accepts any
   candidate hash from a Validator-Ring member; a certificate that is
   parked and then dropped (cap, ceiling) leaves its votes until the
   round is pruned. Bounded by the ring and the retention window;
   carried from IQ-008 Residual 5.
6. **Snapshots from a pre-S35 binary** may hold certificates without
   blocks; install skips those (and their blocks) on both the
   own-recovery and the peer path and backfill re-supplies them.
7. **Candidate pre-squatting costs one refetch.** A relay can fill a
   certificate's two candidate slots with wrong payloads before the
   authentic block arrives; the authentic block is then dropped once,
   the certificate parks on arrival, and the fetch retrieves it (the
   parked path is not slot-limited). Candidates are distinct by full
   header (digest, author, round), so the authentic payload replayed
   under a foreign round is a separate candidate and never shadows the
   real one. The candidate key set is capped at 4,096 hashes and tested
   before any entry is created, so the buffer costs O(1) per frame under
   the state mutex.
8. **A block's `author` and `round` are bound to the certificate.**
   `payload_digest` is the only field the certificate signs; `author`,
   `round` and `cert_hash` are what the block *claims*. A block is
   stored for a certificate only if all three match it (the round is the
   retention key of the block store, so it must not be attacker-chosen),
   a candidate for a not-yet-known certificate is accepted only with a
   claimed round inside the admissible window and is bound on the same
   three fields, and a served snapshot's blocks are filtered the same
   way. The property models a poisoned-round block. Commit-critical
   block fetches (a deferred commit, not a parked certificate) are never
   subject to the per-tick budget.
9. **Registry mutations driven by local admission, which is now
   availability-dependent** (fourth- and sixth-pass findings;
   pre-existing shape and unchanged reach). Two mutations of the
   Authority Ring's `n_authorities` and stake table fire on *local
   admission* with no commit gating, and IQ-009 changes when local
   admission happens (it now waits on the block):
   - **The DAG-S30.1 auto-eject.** Author A equivocates at round r with
     headers X and Y, broadcasts X with its block to all, hands Y with
     its block to exactly one honest node P, and stops. P admits both,
     forms the proof, and the drain rewrites its Authority and Validator
     registries, stake table, pending stake and `n_authorities`. No
     other node receives Y's *header*, so no other node ejects (block
     availability is not the discriminator here: P holds Y's block and
     would serve it; IQ-009 in fact shrinks this split, since any node
     that does receive the header can pull the block from P and eject
     too).
   - **Deferred activation (Issue #18 / DAG-S27.7).** A newly-admitted
     authority's first certificate promotes its pending stake and bumps
     `n_authorities` at `ingest_cert`, on the node that admits it, when
     it admits it. With zero Byzantine behaviour, node P admits the
     certificate with its block one round before node Q (whose copy of
     the block is delayed), and in that window P evaluates the commit
     rule with `n + 1` and Q with `n`.
   The consequence in Invariant-1 terms is the same for both: a
   divergent `n_authorities` is a divergent `round mod N` leader
   rotation and a divergent joint-quorum denominator between two honest
   nodes, hence a divergent commit order — a substrate fork and a
   permanent checkpoint-root split — not merely a registry that differs.
   Neither is a regression (before IQ-009 the same windows opened on
   header delivery instead of block delivery), but D3's "no divergence"
   claim covers the DAG only, not the registry. The fix for both is to
   route the mutation through the committed epoch boundary like every
   other registry change (`pending_governance`; compare the ejects
   inside `apply_commit`, which are deterministic), so the ring changes
   at a committed round on every node; that is a DAG-S30.1 / S27.7
   change with its own review and is a sign-off item here, not a code
   change in this sprint.
10. **Bounds on the buffers IQ-009 added.** `awaiting_block` ≤ 4,096
    signed headers. `block_candidates` ≤ 4,096 hashes × 2 headers and
    ≤ 32 MiB encoded (a candidate is unauthenticated payload up to a
    1 MiB frame, so the entry cap alone would allow 8 GiB), and each
    configured peer label holds at most a `1/n` share of the keys and
    the bytes (floored, so the shares never sum above the caps). The
    share is keyed on the peer's self-declared label — the configured
    wire is unauthenticated (IQ-008 Residual 5) — so it isolates the
    candidates of peers that are honest about their identity from one
    that pins its own share at the ceiling; a host that connects under
    every configured label can still pin the whole buffer, at a cost of
    one park-and-refetch per certificate for the retention window, not
    of admission. The orphan buffer holds one copy per certificate
    (identity is the hash preimage: author, round, digest, parents) and
    at most two per (author, round), like the parking buffer. A block
    bound at certificate arrival is released again when the certificate
    is refused (terminal DAG rejection, orphan buffer or slot full,
    window or gc refusal of a parked header) *unless the DAG holds the
    certificate*, decided under the DAG guard while `ingest_cert`
    re-checks the block under the write guard before every insert, so
    a release and an admission of one hash never interleave; the block
    store is therefore bounded by the retention window (every entry is
    above the gc round and is reaped by the next prune), and in steady
    state by the certificates the DAG holds plus the parked and
    orphaned ones. `block_fetch_history` survives eviction (a replayed
    header resumes its back-off rather than fanning out afresh) and is
    capped at four times the parking buffer, trimmed at prune to the
    live set plus entries younger than four maximum back-offs. The
    snapshot reassembly buffer accepts chunks only from the peer the
    snapshot was requested from, at most 1,024 of them and none above
    the 768 KiB chunk size — 768 MiB of pre-verification memory from
    that one peer, once per bootstrap; the pre-hash window bound is
    derived from the chain-bound committees alone, never from the
    peer-supplied `n_authorities`, though a snapshot claiming no commit
    still widens it to `ck.round + 1` rounds and so pays for one hash
    of its decoded body before the consistency checks refuse it (the
    per-certificate signature loop is never reached).
11. **Per-frame scans under the state mutex.** The candidate window
    check computes `highest_quorum_round` for a `Block` frame whose
    certificate is not known, a frame that costs the sender no
    signature — O(retention window) per frame; and the orphan dedup and
    per-slot count scan the whole orphan buffer, O(4,096) per
    unknown-parent certificate (downstream of an ML-DSA verification,
    so proportionate, but under the same mutex). Memoising the anchor
    in `inner` (tracked follow-up from S34) and indexing the orphan
    buffer by (author, round) are due before the public testnet rather
    than after.
