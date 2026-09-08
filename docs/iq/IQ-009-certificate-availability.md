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
  identically, so no divergence.
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
  parent of that certificate is in its DAG. Exit gates:
  `cert_is_admitted_only_with_its_block` (unit) and
  `withholding_author_does_not_stall_the_mesh` (four-node, fault
  injected).

## Residuals

1. **Bounded parking.** `awaiting_block` is capped; under a flood of
   header-only certificates from a Byzantine author the cap drops the
   newest. The author's certificates are signature-verified before
   parking, so the flood is bounded by seated identities and the
   two-per-(author, round) cap from IQ-008 applies at admission.
2. **Latency.** A certificate whose block is delayed is admitted late;
   the author's own vote and its peers' votes follow the block. This is
   the Narwhal/Mysticeti cost model and is paid per certificate, not
   per commit.
3. **Blocks are still not committed to by the certificate chain beyond
   `payload_digest`.** A relay can delay a block; it cannot substitute
   one (the digest is signed). Unchanged from S31.
4. **Human sign-off** is required as for IQ-007/008: this changes which
   certificates an honest node references, hence the DAG topology under
   partial block loss, and must be reviewed as a consensus change.
