# IQ-009 — LTP aggregate PQ migration (remove classical BLS12-381 while preserving constant-size)

**Status:** Recommendation, pending sign-off. No implementation
scheduled; this IQ fixes the migration *target* and *timeline* so the
BLS12-381 exception reads as a plan, not a hole.
**Owner:** crypto / LTP
**Date:** 2026-07-03
**Tracking:** `docs/research/competitive-gap-analysis.md` gap **G-8** /
workstream **PQ-1** (§6, P2). Migration-sequencing anchor:
`docs/architecture/cryptographic-posture.md` ("LTP aggregate
signatures: BLS12-381 → hash-based + SP1-STARK aggregation, 2027–2029").

## Question

**What is the migration target and timeline to remove the classical
BLS12-381 aggregate from the LTP constant-size commitment, and can a PQ
replacement preserve Invariant 3 (constant ≈1,600 B regardless of
payload)?**

This is the crux of the exception zone. It puts two load-bearing
invariants in direct tension:

- **Invariant 2 (PQ-conservative crypto surface)** wants every
  long-lived integrity surface on NIST-standardized PQ primitives.
  The LTP aggregate is the last consensus-adjacent integrity surface
  still on a classical primitive (BLS12-381), and it is the documented
  soft spot in our "PQ-by-default" headline (README § Bridge
  attestation states it explicitly: *"The BLS12-381 aggregate used in
  the LTP layer (§10.2) is a separate system and is classical, NOT
  post-quantum"*).
- **Invariant 3 (constant-size ≈1,600 B LTP commitment)** is what BLS
  buys us: it aggregates the 7-of-9 super-node signatures into a
  **single 96-byte object regardless of how many witnesses signed**.
  Every candidate PQ replacement is either much larger on-chain or
  moves the witness bytes off-chain — so naively swapping the primitive
  threatens the constant-size property.

The honest problem statement: BLS aggregation is *cheap and constant*
precisely because it exploits the pairing/linearity structure that a
quantum computer also breaks. PQ signature schemes that survive a CRQC
do not aggregate that way. There is no drop-in that is both PQ and
96 bytes.

## Evidence — where and why BLS is used, and the byte budget

**Use site.** BLS12-381 is used in exactly one place on the
consensus-critical surface: the LTP corridor super-node attestation
aggregate.

- `crates/suwappu-crypto/src/bls.rs` — `blst` min-pubkey-size variant
  (`BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_NUL_`): G1 public keys
  (48 B), G2 signatures (**96 B**). `aggregate()` /
  `aggregate_pubkeys()` / `verify_aggregate()` collapse N per-witness
  signatures over a *common* message into one 96-byte aggregate and
  one aggregate public key.
- `crates/suwappu-ltp/src/attestation.rs` — the 7-of-9 pipeline. Each
  witness signs `AttestationPayload::canonical_digest()` (SHA3-256
  under the `SUWAPPU-LTP-ATTEST-V1` domain tag); `attest()` verifies
  every signature individually, then aggregates. `CorridorAttestation`
  carries `aggregate_signature: Vec<u8>` (the 96 B) plus a
  `signers: BTreeSet<AuthorityId>`. Verification re-aggregates the
  named signers' public keys and does one `verify_aggregate`.

**Why BLS specifically** (`cryptographic-posture.md` § "Why retain"):
"the aggregation efficiency required by the constant-size on-chain
commitment; production-grade hash-based + SP1-STARK aggregation is not
yet at gas-economics parity." The constant-size property is the whole
value proposition of the LTP settlement pitch (competitive-gap-analysis
§4, moat 3).

**Byte budget (Invariant 3, paper §10.2, repo-canonical figures).**

```text
ML-KEM-768 sealed session key (ciphertext)  ≈ 1,568 B
BLS12-381 aggregate signature               =    96 B   <- the classical surface
SHA3-256 payload root                       =    32 B
─────────────────────────────────────────────────────
On-chain commitment (constant)              ≈ 1,600 B
```

Codified at `crates/suwappu-ltp/src/lib.rs`
(`ON_CHAIN_COMMITMENT_BYTES = 1_600`) and asserted in a unit test.
The load-bearing property is not the exact total; it is that the total
is **constant in the payload and constant in the signer count** (7, 8,
or 9). BLS delivers both for 96 B.

**PQ primitive sizes available in-repo** (the raw materials for any
replacement):

| Primitive | Where | Size | Aggregatable? |
|---|---|---|---|
| BLS12-381 agg sig | `suwappu-crypto/src/bls.rs` | **96 B for any N** | Yes (classical, quantum-broken) |
| ML-DSA-65 signature | `suwappu-crypto/src/mldsa.rs` | **≈3,309 B each** | No native aggregation (Fiat-Shamir-with-aborts) |
| ML-DSA-65 public key | same | ≈1,952 B | — |
| ML-KEM-768 ciphertext | `suwappu-crypto/src/mlkem.rs` | ≈1,568 B (repo canon) | n/a (KEM) |
| SHA3-256 root | `suwappu-crypto/src/hash.rs` | 32 B | Merkle/accumulator |
| SP1/Plonky3 FRI STARK proof | `suwappu-ltp/src/did_stark.rs` (DID rotation surface) | tens–hundreds of KB | n/a (PQ-safe, no pairings) |
| SP1 Groth16 BN254 proof | `suwappu-l2-verifier-precompile` (Track G) | ≈128–256 B | n/a (**classical** — pairing over BN254) |

The salient fact: seven ML-DSA-65 signatures are ≈23,163 B (7 × 3,309).
Putting them on-chain directly is ~14× the entire current commitment
and *grows with signer count* — an immediate Invariant 3 violation.
Everything below is about avoiding that.

## Options surveyed

For each option: (i) does it preserve the ≈1,600 B constant on-chain
commitment (Invariant 3)? (ii) is it actually PQ (Invariant 2)? (iii)
is destination-side verification tractable?

### Option A — SNARK/STARK-aggregated PQ signatures

Prove the statement *"≥7 of the 9 named corridor super-nodes produced a
valid ML-DSA-65 signature over `canonical_digest()`"* inside a succinct
proof, and commit the **proof** in place of the aggregate signature.
This mirrors Ethereum's leanXMSS-+-SNARK direction (aggregate many
hash-based signatures under one recursive proof).

Two proof systems, two very different verdicts:

- **A1 — Groth16 over BN254 (the Track G in-repo option).** Proof body
  ≈128–256 B — *smaller* than the 96 B it replaces is not quite true,
  but it comfortably fits the 1,600 B budget and is constant in signer
  count. **But BN254 is a pairing-friendly curve; Groth16/BN254 is
  itself classical and quantum-broken.** Swapping BLS12-381 for
  Groth16/BN254 trades one classical exception for another — it does
  **not** satisfy Invariant 2. This is a trap the gap analysis warns
  about implicitly: the repo already has SP1 Groth16 BN254 wired
  (`suwappu-l2-verifier-precompile`), so it is the tempting reach, and
  it is the wrong one for a *PQ* migration. Non-starter as stated.
  - *Invariant 3:* preserved. *Invariant 2:* **fails.**

- **A2 — FRI/STARK (SP1 on Plonky3, the in-repo PQ proof surface).**
  Hash-based (SHA3-256 on the DID surface), no pairings, no trusted
  setup — genuinely PQ, and we already run this exact stack for the
  cross-chain DID rotation path (`did_stark.rs`, DAG-S17). A STARK that
  proves "7 ML-DSA verifies passed" is PQ-clean. **The problem is
  size:** raw FRI/STARK proofs for a statement this heavy (7 lattice
  signature verifications in-circuit) are tens to low-hundreds of KB —
  roughly 100×–1000× the current 96 B, and 10×–60× the *entire*
  1,600 B commitment. Recursive compression can shrink this, but not to
  sub-KB at gas-economics parity today (this is exactly the
  "not yet at gas-economics parity" note in `cryptographic-posture.md`).
  - *Invariant 3:* **fails today** (proof body dwarfs the budget).
    *Invariant 2:* satisfied. *Verification:* tractable (one FRI verify),
    but the on-chain footprint is the blocker.

**Verdict:** A2 is the *right long-term shape* (PQ, self-contained
verification, constant in signer count) but violates Invariant 3 until
recursive-STARK proof sizes fall under ~1 KB at acceptable cost. A1 is
budget-friendly but not PQ. Neither ships today.

### Option B — Hash-based aggregate over ML-DSA-65 signatures (RECOMMENDED target)

Each super-node signs `canonical_digest()` with its **ML-DSA-65** key
(a primitive we already ship, `suwappu-crypto/src/mldsa.rs`). The
on-chain commitment carries a **32-byte SHA3-256 Merkle root** over the
set of witness signatures (an ordered accumulator keyed by
`AuthorityId`), replacing the 96-byte BLS aggregate. The actual
ML-DSA-65 signatures (≈23 KB for a 7-quorum) live **off-chain**, hosted
by the LTP Commitment Nodes under the existing DA SLA
(`crates/suwappu-ltp/src/da.rs`, DAG-S16) — the same content-addressed
DA channel that already carries variable-size LTP payloads.

New on-chain budget:

```text
ML-KEM-768 sealed session key (ciphertext)  ≈ 1,568 B
SHA3-256 signature-set Merkle root          =    32 B   <- replaces BLS 96 B, PQ
SHA3-256 payload root                        =    32 B
─────────────────────────────────────────────────────
On-chain commitment (constant)              ≈ 1,632 B  ≈ 1,600 B
```

- *Invariant 3:* **preserved — with caveat.** The on-chain commitment
  stays constant, and now constant in signer count *by construction*
  (a Merkle root is 32 B whether 7 or 9 witnesses signed). Total moves
  from ≈1,600 B to ≈1,632 B — inside the "≈1,600 B" tolerance the
  invariant is written to (Invariant 3 fixes the *shape and constancy*,
  not a byte-exact 1,600). **The caveat:** the destination can no
  longer verify the attestation from the on-chain commitment bytes
  alone. It must fetch the ML-DSA signature witnesses from the DA layer
  and re-verify them against the root. This changes the verification
  model from *self-contained on-chain* to *on-chain commitment +
  DA-fetched witness set*.
- *Invariant 2:* **satisfied.** ML-DSA-65 (FIPS 204) + SHA3-256
  (FIPS 202) only. No pairings, no BN254, no BLS. Both primitives are
  already in `suwappu-crypto` and property-tested (DAG-S1).
- *Verification tractability:* the destination performs up-to-9 ML-DSA
  verifies (each sub-millisecond) plus one Merkle-inclusion check per
  witness. Cheap in absolute terms, but it is 7–9 verifies instead of
  one aggregate verify, and it requires DA liveness. For an EVM
  destination precompile this is heavier gas than a single pairing
  check; it is the price of PQ without succinct aggregation.

**Why this is the recommended target:** it is the *only* option that
(a) uses solely NIST-standardized PQ primitives we already ship,
(b) keeps the on-chain commitment constant-size and constant in signer
count, and (c) requires no unshipped research or unfinished
proof-size compression. The cost — a DA dependency for verification —
is a model change we already operate (DAG-S16 DA SLA), not a new
subsystem. It also *strengthens* the constant-size story: the on-chain
surface no longer carries any signature material at all, just a root.

### Option C — Keep BLS as a documented, time-boxed exception with a hard sunset (RECOMMENDED interim)

Change nothing in code; convert the open-ended exception into an
explicitly **time-boxed** one with a published sunset date tied to a
threat model, and ship Option B before that date.

The threat-model argument that makes this honest rather than a stall:
**a BLS aggregate signature is an ephemeral-integrity surface, not a
long-lived-confidentiality surface.** Harvest-now-decrypt-later — the
reason ML-KEM-768 must be live *today* on the sealed-session-key half
of the very same commitment — does **not** apply to a signature. A BLS
forgery only has value while a CRQC exists *and* the attestation it
forges is still being relied upon at a live destination. The forgery
deadline is therefore "a CRQC capable of breaking BLS12-381 discrete
log within an attestation's validity window," which is materially later
than the confidentiality deadline that governs the KEM. This is the
principled reason the KEM must be PQ now while the aggregate signature
can be time-boxed — and it should be stated in the posture doc, because
it is the correct rebuttal to "why is half your constant-size
commitment still classical?"

- *Invariant 3:* preserved trivially (no change). *Invariant 2:* still
  in exception, but now *bounded and dated* rather than open-ended.
- *Verification:* unchanged (one pairing check).

Option C is the interim; Option B is what C sunsets *into*.

### Option D — Threshold ML-DSA / native PQ multisignature

Survey of whether a lattice or hash-based scheme aggregates natively
the way BLS does (one short object for N signers, PQ-safe):

- **ML-DSA has no native aggregation.** Fiat-Shamir-with-aborts
  signatures do not combine linearly; there is no standardized
  threshold or aggregate ML-DSA.
- **Research schemes exist but none are NIST-standardized:** lattice
  multi-signatures (e.g. DualMS, MuSig-style lattice constructions) and
  hash-based *synchronized aggregate* signatures (e.g. Chipmunk).
  Chipmunk-class schemes produce an aggregate that grows polylog in the
  signer count — small for large N, but still **KB-scale**, larger than
  both the 96 B BLS aggregate and the 32 B Option-B root, and they
  carry unaudited, non-standardized security assumptions.
- *Invariant 3:* would be preserved only in the constant-in-payload
  sense (aggregate is a few KB, not payload-dependent) but the total
  commitment would grow past ≈1,600 B into the low-KB range —
  a real budget expansion. *Invariant 2:* satisfied only once such a
  scheme is NIST-standardized, which it is not.

**Verdict:** watch-list, not a target. Revisit if NIST or an equivalent
body standardizes a PQ aggregate/threshold signature with a sub-KB
aggregate. Until then it fails the "standardized PQ primitive"
requirement of Invariant 2 and offers no budget advantage over
Option B.

## Recommendation

A **phased migration**, interim + target, with a dated sunset:

1. **Phase 0 — now → CNSA-2.0 window (2026 → 2027):** adopt **Option
   C**. Reclassify the BLS12-381 aggregate from an open-ended exception
   to a **time-boxed** one with a published sunset. Add the
   ephemeral-integrity-vs-confidentiality threat-model rationale to
   `cryptographic-posture.md` so the exception reads as a reasoned,
   dated decision. No code change. This closes the *narrative* half of
   gap G-8 immediately — the rebuttal ("your commitment is half
   classical") is answered with a plan and a date.

2. **Phase 1 — 2027 → 2028 (the target):** implement **Option B** —
   hash-based Merkle/accumulator aggregate over ML-DSA-65 super-node
   signatures, with the signature witnesses riding the DAG-S16
   Commitment Node DA SLA. This is the concrete deliverable that
   *removes* the classical primitive from the on-chain commitment while
   holding Invariant 3. It aligns with the existing
   `cryptographic-posture.md` sequencing ("BLS12-381 → hash-based +
   SP1-STARK aggregation, 2027–2029"); Option B is the hash-based
   first half of that sentence, shippable with primitives already in
   the repo.

3. **Phase 2 — 2028+ (watch, optional):** if recursive-STARK proof
   sizes on the SP1/Plonky3 stack fall under ~1 KB at gas-economics
   parity, migrate the *off-chain verification* from "DA-fetch N sigs +
   N ML-DSA verifies" to a single **FRI/STARK** proof (**Option A2**),
   restoring self-contained on-chain verification while staying PQ. This
   is the "+ SP1-STARK aggregation" second half of the posture-doc
   sequence. Explicitly **do not** take the Groth16/BN254 shortcut
   (Option A1): it is classical and would reintroduce the exact
   exception this IQ exists to remove.

**Timeline hook.** CNSA 2.0's 2027-01-01 acquisition deadline is the
institutional forcing function for PQ across the category, and it is
the same hook the positioning recommendation leans on
(competitive-gap-analysis §5). No competitor has PQ *aggregation*
shipped — Ethereum's leanXMSS+SNARK is roadmap-stage, Arc's PQ is
opt-in wallet-level only — so a dated Phase-0 box plus a Phase-1
delivery in the 2027–2028 window keeps us ahead of the field on the one
surface where our own docs concede a classical dependency. We are not
racing a shipped competitor here; we are closing our own rebuttal
before an analyst uses it.

**Does the recommended target preserve Invariant 3?** **Yes, with a
caveat.** The on-chain commitment stays constant-size (≈1,632 B) and
becomes constant in signer count by construction; the classical 96-byte
BLS object is replaced by a 32-byte PQ Merkle root. The caveat is a
*verification-model* change, not a commitment-size change: the
destination must fetch the ML-DSA signature witnesses (≈23 KB for a
7-quorum) from the DA layer rather than verifying from on-chain bytes
alone. Invariant 3 governs the on-chain commitment surface — that is
preserved; the "verifiable from the commitment alone" convenience is
what is traded.

## Implementation sketch (Phase 1 / Option B)

Not scheduled; recorded so the target is concrete.

1. **Witness signing** (`crates/suwappu-ltp/src/attestation.rs`):
   `SuperNode` gains an `mldsa_public_key: Vec<u8>` alongside (during
   migration) or replacing `bls_public_key`. `WitnessSignature.signature`
   carries an ML-DSA-65 detached signature over the unchanged
   `canonical_digest()` (`SUWAPPU-LTP-ATTEST-V1` domain tag stays
   byte-stable — corridor parity with `suwappu-lattice-protocol` must
   be re-pinned).
2. **On-chain aggregate = Merkle root** (new module, e.g.
   `attestation_pq.rs`): build an ordered SHA3-256 Merkle tree over
   `(authority_id, mldsa_sig)` leaves under a new
   `SUWAPPU-LTP-SIGROOT-V1` domain tag; `CorridorAttestation` replaces
   `aggregate_signature: Vec<u8>` with `signature_root: [u8; 32]` and
   keeps `signers: BTreeSet<AuthorityId>`.
3. **Witness set to DA** (`crates/suwappu-ltp/src/da.rs`): the ML-DSA
   signature set is stored as a DA blob, content-addressed by `Cid`,
   under the existing `DaSla`. Verification fetches it, checks each
   ML-DSA sig, and checks Merkle inclusion against `signature_root`.
4. **`ON_CHAIN_COMMITMENT_BYTES`** (`crates/suwappu-ltp/src/lib.rs`):
   update the constant and its unit test to the new
   ≈1,632 B decomposition (KEM 1,568 + sig root 32 + payload root 32),
   keeping the "constant in payload and signer count" property test.
5. **Exit gate:** a 10,000-case proptest (`proptest_pq_attestation.rs`)
   mirroring `proptest_attestation.rs`: 7-of-9 attests and verifies;
   6 is below quorum; tampered witness set breaks Merkle inclusion;
   on-chain commitment size is invariant across payload size and signer
   count (7/8/9).
6. **Subagents:** `crypto-reviewer` (ML-DSA use + Merkle domain
   separation + no side-channel in per-witness verify) and
   `consensus-reviewer` (7-of-9 quorum semantics unchanged; corridor
   parity). Per CLAUDE.md this touches `suwappu-crypto`/`suwappu-ltp`,
   so both are mandatory.

## Open questions

1. **DA-liveness coupling of cross-chain safety.** Option B makes
   attestation verifiability depend on Commitment Node DA liveness. Is
   the DAG-S16 SLA (default 100k-round retention, 16-round retrieval)
   strong enough to be a *safety* dependency for a destination bridge,
   or only a liveness one? Needs a threat-model pass before Phase 1.
2. **Exact sunset date for Option C.** Recommendation frames it as
   "before Phase 1 lands, within the CNSA-2.0 window." A specific dated
   commitment (e.g. 2028-06-30) should be ratified with this IQ so the
   box is real.
3. **Hybrid interim?** Should Phase 1 ship BLS *and* the ML-DSA Merkle
   root in parallel (dual-attest) for a transition window, letting
   destinations migrate verifiers independently — at the cost of
   temporarily carrying both on-chain (breaks the constant-size budget
   during the window)? Or hard-cut? Leaning hard-cut to protect
   Invariant 3, but destinations' upgrade cadence may force a hybrid.
4. **Corridor parity re-pin.** The `suwappu-lattice-protocol` sister
   repo enforces bit-for-bit corridor parity on the BLS DST +
   length-prefixed SHA3. Phase 1 changes the aggregate surface;
   the parity contract and its cross-repo tests must be re-specified
   in lockstep. Sequencing across two repos is non-trivial.
5. **Phase 2 proof-size threshold.** At what recursive-STARK proof size
   / prover cost does Option A2 become worth the added prover
   complexity over Option B's DA-fetch model? Track against SP1/Plonky3
   releases; do not commit a date.

## Decision

**Pending sign-off.** Recommended: Phase 0 Option C (time-box + dated
sunset + threat-model rationale, now), Phase 1 Option B (hash-based
ML-DSA Merkle aggregate, 2027–2028) as the migration target that
removes the classical primitive while preserving Invariant 3
(with the DA-verification caveat), Phase 2 Option A2 (FRI/STARK
aggregation) as an optional later step contingent on proof-size
parity. Groth16/BN254 (Option A1) and non-standardized PQ multisig
(Option D) are explicitly rejected as targets.

## 2026 literature update (2026-07-03)

A refresh of the PQ-aggregation literature ([`../research/briefs/2026-new-entrants-and-papers.md`](../research/briefs/2026-new-entrants-and-papers.md) §3a)
**confirms this IQ's direction and adds one upgrade.**

- **The field converged on exactly Option B/A2.** There is *no* 2025–2026
  PQ construction that reproduces BLS's ~96-byte native aggregate; the
  realistic PQ paths are (a) SNARK/STARK-recursed hash-based aggregates —
  constant-but-large (leanXMSS blueprint, IACR
  [2025/055](https://eprint.iacr.org/2025/055); Loquat ~145 KB, IACR
  [2024/868](https://eprint.iacr.org/2024/868.pdf); HAPPIER multi-level via
  Risc0; Flock for fast hash-sig aggregation) or (b) lattice multisig
  (Lemur ~73 KB/1024 signers, IACR [2026/1161](https://eprint.iacr.org/2026/1161.pdf)).
  This is direct external validation that Option A1 (Groth16/BN254) and a
  sub-KB native PQ aggregate (the Option-D hope) are dead ends, and that
  Option B (small on-chain root + witnesses to DA) then Option A2
  (FRI/STARK recursion) is the correct sequence.
- **Reframe "constant-size" explicitly.** Post-quantum, constant-size can
  mean **O(1)-in-signers** but *not* the 96-byte byte-count. Option B already
  honors this (32-byte on-chain root, witnesses to DA); the recommendation
  and `cryptographic-posture.md` should state it in those terms, because it
  is the precise thing a reviewer probes. Invariant 3 should be read as
  "constant in payload **and** signer count," not "≤ some fixed byte count
  that survives the PQ migration unchanged."
- **Option D upgrade — threshold ML-DSA is now practical for the *checkpoint
  co-signature*.** Option D surveyed only KB-scale, non-standardized
  multisigs and correctly rejected them for the LTP super-node aggregate.
  But **"FIPS 204-Compatible Threshold ML-DSA via Shamir Nonce DKG"**
  (Kao, arXiv [2601.20917](https://arxiv.org/abs/2601.20917), Jan 2026)
  emits **standard 3.3 KB signatures verifiable by unmodified FIPS-204
  verifiers** for an arbitrary t-of-n. This does not help the *LTP*
  aggregate (still want the 32-byte Merkle root there), but it is a clean
  near-term fit for the **Authority-ring joint checkpoint co-signature**
  (`suwappu-execution/src/checkpoint.rs`, DAG-S11) — turning that into a
  true PQ threshold with no custom verifier and chipping away at any
  classical dependency on the co-signature surface. Recommend spinning this
  out as its own IQ (checkpoint co-signature PQ) rather than folding it
  here, since it touches a different crate and surface.
- **Timeline hook strengthened by EO 14412.** Executive Order 14412
  (2026-06-22) adds dated *civilian* federal mandates (key establishment
  2030-12-31, signatures 2031-12-31) on top of CNSA 2.0's NSS horizon
  (procurement 2027-01-01, exclusive ~2035). The Phase-0 time-box should
  cite the **two-horizon** framing; the Phase-1 delivery window (2027–2028)
  sits comfortably ahead of both. BIS Papers No. 158 (Eurosystem PQC in
  TARGET2-like transfers) is external proof the wholesale-settlement pipes
  are migratable — useful supporting citation in the posture doc.

Net: **no change to the recommended phasing** (C → B → A2); the literature
validates it, the "constant-size = O(1)-in-signers" wording should be made
explicit, and threshold ML-DSA becomes a new adjacent IQ for the checkpoint
co-signature.

## See also

- `docs/architecture/cryptographic-posture.md` — the exception-zone
  table and 2027–2029 migration sequencing this IQ makes concrete.
- `docs/architecture/ltp-integration.md` — the Commit/Lattice/Materialize
  pipeline and the constant-size envelope.
- `README.md` § "Bridge attestation" — the honest-framing note naming
  the BLS aggregate as classical.
- `docs/research/competitive-gap-analysis.md` — gap G-8, workstream
  PQ-1, moat 3 (constant-size commitment).
- [`crates/suwappu-ltp/src/attestation.rs`](../../crates/suwappu-ltp/src/attestation.rs) —
  the 7-of-9 BLS aggregate pipeline (the surface that changes).
- [`crates/suwappu-ltp/src/da.rs`](../../crates/suwappu-ltp/src/da.rs) —
  the Commitment Node DA SLA that Option B's witness set rides.
- [`crates/suwappu-crypto/src/bls.rs`](../../crates/suwappu-crypto/src/bls.rs) —
  the BLS12-381 aggregate (96 B) being removed.
- [`crates/suwappu-crypto/src/mldsa.rs`](../../crates/suwappu-crypto/src/mldsa.rs) —
  ML-DSA-65 (≈3,309 B/sig), the PQ signing primitive Option B uses.
- [`crates/suwappu-ltp/src/did_stark.rs`](../../crates/suwappu-ltp/src/did_stark.rs) —
  the in-repo SP1/Plonky3 FRI surface Option A2 would reuse.
- [`crates/suwappu-ltp/src/lib.rs`](../../crates/suwappu-ltp/src/lib.rs) —
  `ON_CHAIN_COMMITMENT_BYTES` and the size assertion.
