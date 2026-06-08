# IQ-009 — Post-quantum on-chain authorization for cross-chain value movement

**Status:** v3.1 — three rounds of `crypto-reviewer` + `consensus-reviewer`
pressure-test. R1: both BLOCK. R2: crypto BLOCK (distinct-signer) + consensus
CONCERNS. R3: **both CONCERNS, no BLOCKs** — security design converged; residual items
are implementation-spec, captured below. **Ready for the human crypto/consensus
owners + the implementation PR; not yet ratified.**
**Owner:** crypto / consensus / L2-bridge / execution.
**Date:** 2026-06-07 (v1 → v3 same day).
**Tracking:** Suwappu audit program **P5b**. Builds on
[IQ-006](IQ-006-l2-state-root-commitment-surface.md),
[IQ-007](IQ-007-intent-discriminant-stability.md),
[IQ-008](IQ-008-l2-burn-merkle-inclusion.md) (the `burn_nullifier` spent-set pattern
reused here). Primitive: `suwappu-mldsa-precompile`
(`MintCommit` + `verify_mint_authorization` + `verify_mint_quorum`, 19/19 green).

## Question

Every path that authorizes moving bridged value is, today, gated by **classical**
signatures a CRQC breaks:

- **Cross-chain finality attestation** — `suwappu-ltp::verify_attestation`
  (`crates/suwappu-ltp/src/attestation.rs:250`): a 7-of-9 **BLS12-381 aggregate**
  (Shor-broken).
- **L2 batch state-root proof** — `Intent::CommitL2StateRoot` trusts an **SP1 Groth16
  BN254** proof (IQ-006; Shor-broken).

The off-chain envelope is post-quantum (ML-KEM-768 / ML-DSA-65); the **on-chain
integrity of value authorization is not**. Until a value-movement path requires a
genuine on-chain PQ check, the bridge-level PQ claim is **not true**.

**Invariant tension (why an IQ):** invariant 2 lists BLS12-381 / Groth16/BN254 as
exception zones needing a documented migration target (unspecified until now);
invariant 3 (constant-size LTP commitment, §10.2) forbids adding ML-DSA's 3,309 B to
the on-chain commitment. *(Doc nit: SUWAPPUHELPER states the ML-KEM-768 ciphertext as
~1,568 B; FIPS-203 ML-KEM-768 is **1,088 B** — 1,568 is ML-KEM-1024. Correct on a
future invariant-3 edit; does not affect this IQ.)*

## Recommendation (v3)

Authorize value movement with a **joint-ring distinct-signer threshold of ML-DSA-65
signatures over dedicated keys**, carried as a transient witness on a versioned
Intent variant, replay-protected by a `(source_chain, commit_id)` nullifier, expiry-
and snapshot-bound, and activated by a committed-state epoch gate.

### Mechanism

1. **Signed message.** Members sign
   `MintCommit { source_chain, target_chain, commit_id, amount, recipient,
   expiry_epoch, snapshot_height }.canonical_encoding()` — `u32`-BE length-prefixed
   domain tag (`SUWAPPU-MINT-COMMIT-V1`), length-prefixed recipient, big-endian.
   **Implemented except `expiry_epoch` and `snapshot_height`** (the two fields added
   by this IQ — see Decisions); the landed encoding binds the other five.

2. **Joint-ring DISTINCT-SIGNER threshold (resolves crypto BLOCK + consensus
   NEW-3).** Value movement requires `k_auth`-of-`n_auth` Authority **AND**
   `k_val`-of-`n_val` Validator ML-DSA-65 signatures over that same encoding —
   evaluated as two independent `verify_mint_quorum` calls (one per ring) that must
   both pass. Critical correctness points the primitive now enforces and the
   substrate must uphold:
   - **Distinctness is on signer *index*, never on signature bytes.** ML-DSA signing
     is randomized — one key emits unboundedly many valid signatures over one
     message — so byte-counting collapses k-of-n to 1-of-1. `verify_mint_quorum`
     counts each registry index at most once.
   - **The signer pubkey is resolved by the substrate from committed registry state
     at `index`, never from the witness** (`QuorumSigner.pubkey`), so "a valid
     signature at index i" means "a signature under the chain-registered key i".
   - **Ring key sets must be disjoint.** `AdmitAuthority`/`AdmitValidator` only check
     intra-registry slot occupancy (`substrate.rs:2182` and the validator analog) —
     one operator can sit in both rings, letting a single corruption count toward
     both `k_auth` and `k_val`. The substrate must reject (or de-dup) any mint-auth
     key present in both rings, and/or enforce Authority ∩ Validator = ∅ at admission.

3. **Versioned Intent variant (resolves consensus BLOCK-2 / IQ-007).** Add
   `Intent::L1LockV2` / `Intent::L2BurnProvenV2` carrying the `PqAuthWitness`, routed
   through the same apply handler as V1 (IQ-007 versioned-variant pattern, as
   `PostL2DAv2` at `substrate.rs:845`). **Seat the new variants at the enum tail**
   (a mid-enum insert is itself the IQ-007 break). Do **not** field-add to existing
   variants (witness would enter `blake3(bincode(intent))` = consensus tx-hash + SDK
   signing digest, `clients/rust-sdk/src/lib.rs:185-190`; cutover fired at `v0.3.0`).
   No `#[serde(default)]`-gives-wire-compat reasoning.

4. **`(source_chain, commit_id)` nullifier + atomic rollback (resolves BLOCK-1 +
   crypto/consensus concerns).** "Discard the signature" (invariant 3) and
   "single-use" (replay) are separate; v1 conflated them. Add a spent-set, **32 B
   value, constant-size**, keyed on the **same uniqueness fields the signature binds
   — `(source_chain, commit_id)`, not `commit_id` alone** (else two source chains
   reusing a `commit_id` falsely collide → liveness loss). Mirror
   `burn_nullifier.rs`; check-and-set **before** the mutation, in an apply arm that
   is **all-or-nothing**: a failed `credit`/`debit` must roll back the nullifier
   insert too (invariant 4 bundle atomicity), so neither a re-org nor an intra-apply
   error can permanently burn an unspent `commit_id`. State `commit_id`
   global-uniqueness-per-origin-lock as a verified precondition.

5. **Dedicated key + pinned snapshot + for-cause carve-out (resolves consensus
   NEW-1/NEW-2 + crypto CONCERN-3).**
   - *Dedicated key (NEW-1 contradiction):* the per-member `mldsa_public_key` in the
     registries IS the **consensus** key (`authority_registry.rs:116`, seated at
     `substrate.rs:2190`). Reusing it contradicts the key-separation requirement.
     Add a **distinct `mint_auth_mldsa_pk`** to `AuthorityRecord`/`ValidatorRecord`
     (registry encoding-version bump + keygen-at-admission + its own rotation path).
     Drop the "keys already exist" claim.
   - *Snapshot (CONCERN-3):* resolve signer keys from the committed registry snapshot
     pinned to the intent's `snapshot_height` (bound into the signed encoding,
     Mechanism 1), so rotation between sign- and apply-time cannot deterministically
     brick a valid authorization.
   - *For-cause carve-out (NEW-2):* a member **ejected for cause** (invariant 5:
     equivocation → 100% slash + expulsion) is revoked **immediately** — resolved
     against latest committed status, NOT the pinned snapshot — so a slashed key
     cannot keep counting toward quorum inside the expiry window.

6. **Committed-state activation gate (resolves consensus CONCERN-4).** No
   `MonadSpecId` exists in the DAG substrate; activate via a committed-state
   epoch/height gate (matches the deferred-activation pattern at `substrate.rs:261`).
   In-flight disposition falls out of the V2 variants: pre-epoch encode V1 (valid
   until epoch `E`), post-epoch encode V2.

7. **Expiry binding + safe pruning (resolves CONCERN-4 + crypto prune-window).** Bind
   `expiry_epoch` into the encoding so an authorization is not valid forever. A
   nullifier entry may be pruned **only once the expiry check is guaranteed to reject
   it**, derived from the **same committed-epoch source** as the expiry predicate (a
   strict subset), so pruning can never open a replay window.

8. **Scoping (consensus NIT).** The committee-key set derives from the same consensus
   stake table; `L1Lock`/`L2BurnProven` mutate shared escrow (`substrate.rs:2561`) →
   **main-lane only**, no fast-path/checkpoint interaction.

## Resolved decisions

| # | Question | Decision |
|---|---|---|
| A | Required vs phased | Phased committed-state epoch gate (M6). |
| B | Which key / ring | Both rings, joint AND-gate, **dedicated `mint_auth_mldsa_pk`** (M2, M5). |
| C | Single vs threshold | **Distinct-signer** k-of-n per ring; transient `(k_auth+k_val)×3,309 B` calldata (M2). |
| D | Substrate location | Confirm `Intent`/apply arms in `suwappu-execution` vs external `suwappu-db` before landing V2 variants + nullifier + registry field. Implementation item. |
| E | Encoding additions | Add `expiry_epoch` + `snapshot_height` to `MintCommit` (M1, M5, M7). |
| F | Groth16 state-root proof | Out of scope — separate PQ-proof migration IQ. |

## Revert-fails exit gate (substrate)

Removing the gate makes a bad mint succeed → RED. Must include:
- **Tamper:** any bound field flipped → reject.
- **Threshold:** `k-1` distinct signers (either ring) → reject; full `k_auth`-AND-`k_val` → accept.
- **Single-signer collapse:** `k` signatures all from ONE index → counts once → reject (the round-2 BLOCK; covered at the primitive by `quorum_rejects_many_sigs_from_one_signer`).
- **Witness-key vs committed-state (round-3 must-fix):** a signature valid under an *attacker-chosen* key at a claimed `index` → reject, because the substrate resolved the pubkey from committed registry state at `index` (not the witness). An out-of-range / unoccupied `index` → reject. *(The primitive deliberately trusts the supplied pubkey — `quorum_pubkey_is_caller_resolved_contract` demonstrates why the substrate MUST overwrite it.)*
- **Ring overlap:** a key seated in both rings counted toward both thresholds → reject.
- **Double-submit (replay):** same valid witness twice → second rejects on the `(source_chain, commit_id)` nullifier.
- **For-cause ejection:** a slashed signer's pinned key inside the expiry window → not counted → reject if it drops below threshold.
- **Rotation:** key rotated after the pinned `snapshot_height` → still accepts (no bricking).
- **Expiry:** past `expiry_epoch` → reject.

## Review log

- **v1 → both BLOCK.** crypto: no replay protection; single-key forgery. consensus:
  single-key collapses Theorem 2; field-add breaks IQ-007.
- **v2 → crypto BLOCK, consensus CONCERNS.** crypto: joint threshold counted
  signatures not signers → randomized ML-DSA lets one key satisfy a ring alone.
  consensus: dedicated-vs-consensus-key contradiction; no ring-disjointness; for-cause
  ejection within expiry window; + nits (rollback invariant, enum-tail seating).
  Round-2 confirmed RESOLVED: replay nullifier, IQ-007 V2 variants, epoch gate,
  key-separation doc, length-prefix nit.
- **v3 → resolutions:** distinct-signer threshold via `verify_mint_quorum`
  (signer-index dedup, pubkey resolved from committed state) + the
  single-signer-collapse exit test; dedicated `mint_auth_mldsa_pk` registry field;
  ring-disjointness; for-cause carve-out vs pinned snapshot; nullifier keyed on
  `(source_chain, commit_id)` with all-or-nothing rollback; expiry + safe-prune;
  enum-tail seating. Primitive: added `QuorumSigner`/`verify_mint_quorum` (19/19).

## Round-3 review + resolutions (v3.1)

R3 returned **CONCERNS from both, no BLOCKs**: the round-2 distinct-signer BLOCK is
confirmed fixed+tested in code (`verify_mint_quorum`, 20/20). Residual items —
implementation-spec, each with a committed fix here:

- **(crypto) Witness-pubkey-vs-committed-state needs an exit test + a misuse-resistant
  API.** `verify_mint_quorum` counts a valid signature under the *supplied* pubkey and
  cannot check it against `registry[index]` — that is the substrate's duty. **Resolved:**
  added the witness-key + out-of-range-index exit-gate cases above; the preferred
  substrate API passes a registry/resolver (not a witness pubkey) so `QuorumSigner`
  carries only `index + signature` and the contract is unbypassable, not
  documentation-enforced.
- **(consensus-A) Snapshot resolution has no backing accessor + no range check.** The
  `Substrate` trait (`substrate.rs:919`) exposes only current state + `current_block_height`;
  there is no at-height accessor. **Resolved:** registries change only at epoch
  boundaries (admissions queue for next epoch, `substrate.rs:88-91`) → registry state is
  piecewise-constant per epoch; maintain an **epoch-keyed registry-snapshot record at a
  reserved address**, resolve via existing `read_bytes`. **`snapshot_height` MUST be
  range-checked**: reject unless `pruned_floor < snapshot_height ≤ current_committed_height`.
- **(consensus-B) Provisioning the dedicated key is itself an IQ-007 surface; registry
  bumps to v4.** `mint_auth_mldsa_pk` reaches the registry via an admission intent, and
  `AdmitAuthority` (`substrate.rs:2155`) is an existing variant — a field-add there is the
  same `blake3(bincode)` break M3 forbids. **Resolved:** provision via a tail
  `AdmitAuthorityV2`/`AdmitValidatorV2` (or a dedicated `SetMintAuthKey`) intent, never a
  field-add. The registry record version bumps to **v4** (v3 is taken by
  `exit_block_height`, `authority_registry.rs:41`). Re-check ring-disjointness on
  **rotation**, not only admission.
- **(consensus-C) Height-only activation can stall mint liveness.** Pre-v4 records have no
  dedicated key and can't contribute to quorum until they rotate one in. **Resolved:**
  predicate the activation gate on **provisioning coverage** (≥ `k_auth` Authority + ≥
  `k_val` Validator dedicated keys present), not height alone; or seed dedicated keys at
  genesis / forced rotation before the gate.
- **(consensus-NEW-2 follow-up) For-cause carve-out same-block ordering.** Resolving
  ejection against "latest committed status" while resolving keys against the pinned
  snapshot is deterministic **only if the intra-block apply order is fixed**; state that an
  eject intent ordered before a mint intent in the same block revokes the signer for that
  mint. **Resolved:** intra-block apply order governs (the existing deterministic schedule).
- **Liveness statement:** a below-threshold / revoked / expired authorization may be
  **re-issued** with a fresh `snapshot_height`/`expiry_epoch`; the nullifier is set only on
  successful all-or-nothing apply (M4), so no legitimate movement is permanently bricked.

These are now the **implementation checklist** for the PR that wires the substrate; none
blocks the design. The human crypto/consensus owners should confirm B (registry/intent
encoding) against IQ-007 and C (activation coverage) against the genesis/rotation plan.

## Status of the primitive

`suwappu-mldsa-precompile` implements the FIPS-204 verifier, the `MintCommit`
commit-binding, and the distinct-signer `verify_mint_quorum`, with revert-fails tests
(20/20, `cargo test -p suwappu-mldsa-precompile`, incl. the pubkey-resolution contract
test). Remaining primitive work for
implementation: the `expiry_epoch` + `snapshot_height` fields on `MintCommit`
(Decision E). The substrate composes per-ring `verify_mint_quorum` calls into the
joint AND-gate and owns ring-disjointness, ejection, snapshot, nullifier, and the
epoch gate. The future EVM precompile at `0x0101` is separately blocked on
`production-evm-executor` (PRs #25-31), best done by inlining the verify into
`suwappu-revm` to avoid a circular gsx-revm↔gsx-dag dep.
