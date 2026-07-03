# IQ-012 — Threshold ML-DSA for the Authority-ring checkpoint co-signature

**Status:** Recommendation, pending sign-off. No implementation
scheduled; this IQ fixes a *target shape* for the DAG-S11 checkpoint
co-signature and, critically, adjudicates whether that shape is
compatible with the joint-quorum safety property (Theorem 2).
**Owner:** crypto / consensus
**Date:** 2026-07-03
**Tracking:** Refs [`IQ-009`](./IQ-009-ltp-aggregate-pq-migration.md)
(§ "2026 literature update", Option D) / workstream **PQ-1**. Spun out
of IQ-009 deliberately: the 2026 threshold-ML-DSA finding fits a
*different* surface (the checkpoint co-signature, `suwappu-execution`)
than the LTP super-node aggregate IQ-009 governs (`suwappu-ltp`), and it
interacts with a *different* invariant (Theorem 2, not Invariant 3), so
it deserves its own ratification.

## Question

**Should the Authority-ring joint checkpoint co-signature (DAG-S11)
adopt a t-of-n threshold ML-DSA scheme (arXiv
[2601.20917](https://arxiv.org/abs/2601.20917) — "FIPS 204-Compatible
Threshold ML-DSA via Shamir Nonce DKG", standard 3.3 KB FIPS-204
signature, unmodified verifier) in place of the N individual ML-DSA
signatures it carries today — and what does that buy for size,
verification, and the joint-quorum safety property (Theorem 2)?**

Two things make this non-trivial and worth a written decision:

1. **It is a compression question, not a de-classicalization question.**
   Unlike the LTP aggregate (IQ-009), the checkpoint co-signature has
   **no classical primitive to remove** — see Evidence below. Threshold
   ML-DSA does not close a PQ exception zone here; it collapses N
   PQ signatures into one PQ signature. The value is size + verification
   cost + a cleaner "true t-of-n" story, not Invariant-2 posture.

2. **It puts a threshold signature next to Theorem 2.** A t-of-n
   threshold signature is, by construction, *anonymous in the quorum*:
   it verifies against a single group public key and does **not** reveal
   which t of the n Authority members contributed. Load-bearing
   Invariant 1 (joint-quorum AND-gate safety, Theorem 2) and Invariant 5
   (fast-path equivocation = 100% slashing) both depend on
   **per-signer accountability**. The crux of this IQ is whether a
   threshold co-signature erases the accountability those invariants
   need, or whether that accountability lives on a different surface
   that a threshold checkpoint sig never touches.

## Evidence — how the checkpoint is co-signed today

**Current mechanism: N detached ML-DSA-65 signatures, no aggregate, no
classical primitive.** Confirmed at
[`crates/suwappu-execution/src/checkpoint.rs`](../../crates/suwappu-execution/src/checkpoint.rs):

- `Checkpoint { height, round, state_root, prev_checkpoint }` is hashed
  under a domain-separated BLAKE3 recipe
  (`BLAKE3("SUWAPPU-CHECKPOINT-V1" || height || round || state_root || prev_checkpoint)`,
  `checkpoint.rs:58-68`).
- `sign_checkpoint` (`checkpoint.rs:114-125`) produces one **ML-DSA-65
  detached signature** (`suwappu_crypto::mldsa::sign`) over that hash,
  wrapped as `CheckpointSignature { authority: AuthorityId, signature: Vec<u8> }`.
  The signature carries the **named** signer.
- `ratify_checkpoint` (`checkpoint.rs:130-161`) verifies **each**
  signature individually against the registry's published
  `public_key_bytes`, **dedups by `authority` id**, and enforces that
  the **distinct-signer count** meets `registry.quorum_threshold()`
  (`q = n − ⌊(n-1)/3⌋`, `suwappu-authority/src/registry.rs:135-141`).
  The result `CoSignedCheckpoint` retains the full **named signer set**
  (`signatures: Vec<CheckpointSignature>`).

Two facts fall out of this and drive the whole analysis:

- **There is no BLS on this surface.** The BLS12-381 exception zone that
  IQ-009 exists to close is the *LTP* super-node aggregate
  (`suwappu-crypto/src/bls.rs`, `suwappu-ltp/src/attestation.rs`). The
  checkpoint co-signature never uses it. Adopting threshold ML-DSA here
  removes **zero** classical dependency; it is pure compression of a set
  of PQ signatures already in place.
- **The checkpoint carries the signer identities, and today they are
  verifiable from the artifact.** `CoSignedCheckpoint` proves *which*
  ≥q Authority members attested to `(height, state_root)`. Any consumer
  that reads that set for participation/liveness/reward accounting is
  reading a property a threshold signature would erase.

**Size today.** For an Authority ring of n members with quorum
`q = n − ⌊(n-1)/3⌋`, the co-signature carries q detached ML-DSA-65
signatures at ≈3,309 B each. Representative n = 31 ⇒ q = 21 ⇒
≈**69 KB** of signature material per checkpoint, growing linearly in q.
Verification is q independent ML-DSA verifies.

**Where Theorem-2 accountability actually lives (the decisive
evidence).** The joint-quorum safety proof does **not** source its
accountability from the checkpoint signature set. It sources it from
**certificate authorship in the consensus DAG**:

- `authority_equivocators(dag, cand_a, cand_b, round)`
  ([`crates/suwappu-consensus/src/joint.rs:169-190`](../../crates/suwappu-consensus/src/joint.rs))
  derives the equivocator set by inspecting `c.author` on the *named*
  round-`r+1` supporting certificates in the DAG — not from any
  checkpoint.
- Equivocation detection (`suwappu-consensus/src/equivocation.rs`) and
  the 100%-slashing pipeline (`suwappu-authority/src/slashing.rs`,
  `slash_authority`) likewise operate on per-author certificates /
  fast-path certificates, never on the checkpoint co-signature.
- The checkpoint is **downstream** of consensus: it attests to the
  *already-committed* joint state root `(Σ_EVM, Σ_Move)` at a cadence
  boundary (`checkpoint.rs:1-26`). It is a settlement/finality
  attestation, not a vote in the safety-critical commit path.

This is the fact that unlocks the recommendation: **the checkpoint
co-signature is not the accountability surface for Theorem 2 or for
slashing.** That surface is certificate authorship, which a threshold
checkpoint signature does not touch.

## Options surveyed

For each: (i) size; (ii) verification cost; (iii) DKG / ceremony
complexity; (iv) the accountability / Theorem-2 interaction.

### Option A — Threshold ML-DSA via Shamir nonce DKG (RECOMMENDED target)

The Authority ring runs a one-time Shamir-nonce distributed key
generation to establish a single group ML-DSA-65 verification key. Any
t = quorum members jointly produce **one standard 3.3 KB FIPS-204
signature** over `Checkpoint::hash()`, verifiable by the **unmodified**
`suwappu_crypto::mldsa::verify` against the group key (arXiv
2601.20917's headline property: no custom verifier, no new primitive on
the verify path).

- **Size.** One ≈3,309 B signature for the whole quorum, **constant in
  q**. Replaces ≈69 KB (n = 31) with ≈3.3 KB — a ~21× reduction at
  n = 31, and O(1)-in-signers thereafter. This is the same
  "constant-in-signer-count" reframing IQ-009 lands for the LTP surface,
  achieved here without moving anything to DA.
- **Verification cost.** One ML-DSA verify instead of q. Directly
  relevant to any light consumer that checkpoint-syncs (walking the
  `prev_checkpoint` chain becomes one verify per checkpoint, not q).
- **DKG / ceremony.** The real cost. Requires (1) an initial DKG to
  seat the group key, and (2) **resharing on every Authority-set
  change** (admission, exit, slashing ejection — see
  `StakeTable::remove` / `EjectAuthority` at `joint.rs:84`). The
  Authority ring is dynamic, so this is not a one-shot ceremony; it is a
  standing operational protocol coupled to registry mutation. Threshold
  nonce generation also imposes a liveness/robustness cost per signing
  round (a stuck or malicious co-signer can force a restart).
- **Accountability / Theorem 2.** The threshold sig is anonymous in the
  quorum — it proves "≥t Authority members signed" but not **which** t.
  Per the Evidence section this is **acceptable for this surface
  specifically**, because Theorem-2 accountability and slashing evidence
  are sourced from certificate authorship in the consensus DAG, not from
  the checkpoint signer set. It is **not** acceptable to push threshold
  signing down onto the consensus vote path, where per-signer identity
  *is* the safety mechanism. The one real loss is **per-checkpoint
  participation accounting** (who co-signed height H): a threshold sig
  erases it, so any liveness/reward logic that reads
  `CoSignedCheckpoint.signatures` needs a replacement signal (see Open
  Questions).

### Option B — Keep N detached ML-DSA-65 signatures (status quo; safe fallback)

Change nothing. The co-signature stays a set of q named ML-DSA-65
detached signatures.

- **Size.** Linear in q (≈69 KB at n = 31); grows with ring size.
- **Verification cost.** q independent ML-DSA verifies per checkpoint.
- **DKG / ceremony.** **None.** No group key, no resharing on membership
  change — the single biggest operational advantage. Signing is local
  and non-interactive; a slow or Byzantine member simply does not appear
  in the set.
- **Accountability / Theorem 2.** **Maximal.** The named signer set is
  preserved and verifiable from the artifact; per-checkpoint
  participation is a free byproduct. No interaction risk with Theorem 2
  whatsoever.

This is the honest fallback: it is already fully PQ (Invariant 2 clean),
already correct, and carries no ceremony. Its only defects are size and
q-fold verification — neither of which violates a load-bearing
invariant, because **the checkpoint has no constant-size budget** (it is
not the on-chain LTP commitment governed by Invariant 3).

### Option C — Hash-based Merkle aggregate over the ML-DSA signatures

Mirror IQ-009 Option B: replace the q signatures on the co-signed
artifact with a 32-byte SHA3-256 Merkle root over the
`(authority_id, mldsa_sig)` leaves, and host the actual signatures
off-chain (DA), fetched and re-verified against the root.

- **Size.** 32-byte root on the artifact, constant in q.
- **Verification cost.** **Worse than status quo**: a verifier must
  fetch the witness set from DA *and* perform q ML-DSA verifies *and*
  q Merkle-inclusion checks. It trades bytes for a DA round-trip and
  strictly more verification work.
- **DKG / ceremony.** None (no group key), but it adds a **DA-liveness
  dependency** to checkpoint verification.
- **Accountability / Theorem 2.** Preserved (the named signatures still
  exist, just off-artifact) — but at the cost of the DA dependency.

**Verdict: poor fit here.** Option C earns its keep in IQ-009 because the
LTP commitment is a **constant-size on-chain surface under real byte
pressure (Invariant 3)** carrying variable-size payloads on the same DA
channel. The checkpoint is neither: it is a small, fixed-shape internal
finality artifact with **no on-chain constant-size budget** and no
existing DA carriage. Introducing a DA-liveness dependency on
*finality-checkpoint verification* to save bytes that aren't
budget-constrained is a bad trade. C is rejected for this surface.

## The crux — does a threshold signature interact safely with Theorem 2?

Stated sharply: **Theorem 2 requires provable, independent BFT quorums
in *both* rings; a t-of-n threshold signature is anonymous in its
quorum. Does adopting it at the checkpoint erase the accountability the
dual-ring safety/slashing model needs?**

**No — for the checkpoint co-signature specifically — because the
checkpoint is not the surface Theorem 2 reasons over.** The argument, in
three steps:

1. **Theorem 2's accountability is cert-authorship-sourced, not
   co-signature-sourced.** The proof (paper §11; `joint.rs:1-23`) bounds
   safety by the size of the *equivocator* sets: `authority_equivocators`
   over named round-`r+1` certificates
   (`joint.rs:169-190`) and `validator_double_vote_stake` over named
   votes (`joint.rs:196-216`). A safety violation forces
   ≥⌈|A|/3⌉ **named** Authority equivocators and >total/3 **named**
   Validator double-voters. None of this reads a checkpoint. The
   checkpoint co-signs the *committed* state root; it is a consequence of
   consensus, not an input to it.

2. **Slashing evidence is cert-sourced too.** Both Authority
   equivocation (Invariant 1 / DAG-S7) and fast-path equivocation
   (Invariant 5 / DAG-S9, 100% slashing) are proven from per-author
   certificates in the DAG / fast-path lane
   (`suwappu-consensus/src/equivocation.rs`,
   `suwappu-authority/src/slashing.rs`). A threshold checkpoint signature
   is never presented as slashing evidence, so making it anonymous
   removes no slashing capability.

3. **Therefore the anonymity a threshold sig introduces is confined to
   a surface where anonymity is safe.** What the checkpoint sig must
   prove is exactly the *threshold predicate* — "≥q Authority members,
   holding shares of the group key, attested to this state root" — and a
   t-of-n threshold ML-DSA sig proves precisely that. It provides the
   quorum-reached guarantee the checkpoint needs without needing to name
   the members, because naming them is not what makes the checkpoint (or
   Theorem 2) safe.

**The load-bearing boundary condition** (must be stated in the decision
so it cannot be misapplied): this reasoning holds **only** because the
checkpoint is downstream of the commit. A threshold signature **must not**
be pushed onto the consensus vote / certificate path, where per-signer
identity is the mechanism that makes equivocation detectable and
slashable. Anonymizing *that* surface would directly erase Theorem-2
accountability. IQ-012 authorizes threshold signing for the checkpoint
co-signature and **explicitly for nothing else**.

**The one genuine loss** (not a safety loss): per-checkpoint
*participation* attribution. Today `CoSignedCheckpoint.signatures` records
who co-signed height H — useful for liveness monitoring and any
availability-based reward/penalty. A threshold sig replaces that with a
yes/no threshold predicate. This is a **liveness-accounting** concern,
not a **safety** one, and it is the thing to weigh against the size win
(Open Questions Q2).

**Verdict on the tension:** the threshold-vs-accountability tension is
**real but localized and acceptable for the checkpoint co-signature**,
because Theorem-2 safety and slashing draw their per-signer accountability
from certificate authorship in the consensus DAG, not from the checkpoint
signer set — provided threshold signing is confined to the checkpoint and
per-checkpoint participation accounting is either shown to be unused or
re-provided out of band.

## Recommendation

**Option A (threshold ML-DSA), pending sign-off, gated on two
preconditions** — with **Option B (status quo) as the standing fallback
if either precondition is judged not worth its cost.** Option C is
rejected for this surface.

Rationale: the checkpoint today carries q separate ML-DSA signatures
(the case the task flags as favoring A), and the crux analysis shows a
threshold sig is safe on this surface. Adopting A buys an O(1)-in-signers
co-signature (~21× smaller at n = 31), single-verify checkpoint sync, and
a clean "true t-of-n PQ threshold, unmodified FIPS-204 verifier" story
that chips at the *narrative* of scattered per-node PQ signatures without
touching any classical exception zone. But — unlike the LTP migration —
**nothing is broken today**: the surface is already PQ and already
correct, and the win does not resolve a load-bearing invariant. So A is a
*quality/compression upgrade*, not a fix, and it must clear its
operational cost before it ships.

**Preconditions on A:**

- **P1 — Dynamic-set resharing.** A production-grade DKG **and
  resharing** protocol for a *dynamic* Authority ring (admission, exit,
  slashing ejection) must exist and be audited. A static one-shot DKG is
  insufficient; the ring changes and the group key must follow without a
  trust gap. If resharing is operationally heavier than the size win
  justifies, keep Option B.
- **P2 — Participation accounting.** Confirm no consumer depends on the
  per-checkpoint named signer set for safety, or re-provide the
  liveness/reward signal out of band (Open Questions Q2). If a consumer
  *does* depend on named signers for anything safety-adjacent, that is a
  design smell to fix first regardless.

If P1's resharing cost or P2's attribution loss is unacceptable, **Option
B is the recommendation** — it is fully PQ, ceremony-free, and violates no
invariant; its only cost is bytes and q-fold verification on a surface
with no size budget.

## Implementation sketch (Option A; not scheduled)

Recorded so the target is concrete.

1. **Crypto crate — threshold module.** Add
   `crates/suwappu-crypto/src/mldsa_threshold.rs` implementing the arXiv
   2601.20917 Shamir-nonce construction: `dkg()` → group `PublicKey` +
   per-member secret shares; `partial_sign(share, msg)`; `combine(partials)`
   → a **standard** `mldsa::Signature`. Crucially, **verification reuses
   `mldsa::verify` unchanged** (the group key is an ordinary ML-DSA-65
   public key) — no new verifier, no new NIST surface, so Invariant 2 is
   untouched. Add a `resharing()` entry point for membership change (P1).
2. **Checkpoint types** (`checkpoint.rs`): replace
   `CoSignedCheckpoint.signatures: Vec<CheckpointSignature>` with a
   `threshold_signature: Vec<u8>` (one 3.3 KB sig) + the quorum
   *predicate* metadata needed to bind it (group-key epoch / registry
   version). `sign_checkpoint` becomes a partial-sign + combine flow;
   `ratify_checkpoint` becomes **one** `mldsa::verify` against the
   epoch's group key plus a `q`-reached assertion carried by the DKG
   epoch, replacing the per-signer verify loop
   (`checkpoint.rs:130-161`). Keep the BLAKE3 `SUWAPPU-CHECKPOINT-V1`
   digest recipe byte-stable.
3. **Registry binding** (`suwappu-authority`): the `AuthorityRegistry`
   gains a group-verification-key field per DKG epoch, advanced on every
   admission/exit/slashing that triggers resharing. `quorum_threshold()`
   becomes the threshold `t` for the DKG.
4. **Participation signal (P2).** If liveness/reward needs per-checkpoint
   participation, add an out-of-band, *non-safety* attestation (e.g. a
   lightweight per-member liveness ping) rather than reintroducing named
   signatures on the checkpoint.
5. **Exit gate:** a 10,000-case proptest
   (`proptest_threshold_checkpoint.rs`) mirroring the DAG-S11 gate: any
   t-of-n honest partial set combines to a signature that verifies under
   the group key; any t-1 set fails to combine to a valid signature;
   tampering with any `Checkpoint` field breaks verification; a resharing
   round preserves verifiability across a membership change.
6. **Subagents (mandatory per CLAUDE.md):** `crypto-reviewer` (threshold
   nonce-DKG correctness, no nonce-reuse / share-leakage side channels,
   FIPS-204 verifier compatibility) **and** `consensus-reviewer`
   (Theorem-2 boundary: confirm threshold signing is confined to the
   checkpoint and that the equivocation/slashing evidence path is
   untouched).

## Open questions

1. **Resharing under slashing.** Slashing ejection
   (`slash_authority`, `EjectAuthority`) mutates the Authority set
   *adversarially and possibly frequently*. Does the 2601.20917 scheme
   support robust resharing that excludes an ejected (Byzantine) share
   holder without a trusted dealer? This is the gating feasibility
   question for P1.
2. **Is the per-checkpoint signer set actually consumed?** Audit every
   reader of `CoSignedCheckpoint.signatures` (indexer, liveness monitor,
   any reward logic). If any safety-relevant consumer exists, A is
   blocked until it is re-based off cert-authorship; if only
   liveness/reward consumers exist, quantify what the out-of-band signal
   in step 4 must provide.
3. **DKG-epoch vs checkpoint-chain interaction.** The checkpoint chain
   walks `prev_checkpoint` back to genesis; group-key rotation on
   resharing means a verifier must resolve the correct group-key epoch
   per checkpoint. Pin the epoch binding into the BLAKE3 digest or the
   registry lookup, and add it to the exit-gate proptest.
4. **Liveness cost of threshold nonce generation.** Interactive threshold
   signing adds a coordination round per checkpoint; under an Authority
   partition this could stall checkpoint production where q independent
   detached sigs (Option B) would not. Weigh against checkpoint cadence
   (IQ-006's cadence surface) before committing.
5. **Does the win justify the ceremony at all?** Because this is
   compression on a non-budget-constrained surface (not a Theorem-2 fix
   and not a PQ-exception fix), the honest null hypothesis is Option B.
   A should ship only if the size/verify win and the "true t-of-n"
   narrative clear the standing DKG+resharing operational cost.

## Decision

**Pending sign-off.** Recommended: **Option A** (t-of-n threshold ML-DSA
via Shamir nonce DKG, one 3.3 KB FIPS-204 signature, unmodified verifier)
as the target shape for the DAG-S11 checkpoint co-signature, **gated on
P1 (audited dynamic-set resharing) and P2 (participation-accounting
resolution)**, with **Option B (status quo N detached ML-DSA sigs) as the
standing fallback** if either precondition's cost is not justified by the
size/verification win. **Option C (hash-based Merkle + DA) is rejected**
for this surface (it adds a DA-liveness dependency to finality-checkpoint
verification with no offsetting byte-budget pressure). The threshold
scheme is authorized **for the checkpoint co-signature only** and
**explicitly not** for the consensus vote / certificate path, where
per-signer accountability is the Theorem-2 safety mechanism.

## See also

- [`crates/suwappu-execution/src/checkpoint.rs`](../../crates/suwappu-execution/src/checkpoint.rs) —
  the DAG-S11 checkpoint + N-detached-ML-DSA co-signature (the surface
  that would change).
- [`crates/suwappu-consensus/src/joint.rs`](../../crates/suwappu-consensus/src/joint.rs) —
  the joint-quorum AND-gate; `authority_equivocators` /
  `validator_double_vote_stake` are where Theorem-2 accountability
  actually lives.
- [`crates/suwappu-authority/src/slashing.rs`](../../crates/suwappu-authority/src/slashing.rs) ·
  [`registry.rs`](../../crates/suwappu-authority/src/registry.rs) —
  cert-sourced slashing and the `quorum_threshold` that becomes the DKG
  threshold `t`.
- [`crates/suwappu-crypto/src/mldsa.rs`](../../crates/suwappu-crypto/src/mldsa.rs) —
  ML-DSA-65 (≈3,309 B/sig); the unmodified verifier a threshold group key
  reuses.
- [`IQ-009`](./IQ-009-ltp-aggregate-pq-migration.md) — parent IQ; its
  "2026 literature update" spun this out, and its Option B/C is the
  contrast for why Merkle+DA fits the LTP surface but not this one.
- [`docs/research/briefs/2026-new-entrants-and-papers.md`](../research/briefs/2026-new-entrants-and-papers.md)
  §3a — arXiv [2601.20917](https://arxiv.org/abs/2601.20917) (threshold
  ML-DSA) and the leanXMSS / HAPPIER aggregation context.
</content>
</invoke>
