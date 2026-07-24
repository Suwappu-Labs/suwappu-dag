# Audit-prep brief: Track H PQ-confidential construction

**Status:** Audit-prep doc — NDA'd partner portal target (per
Track D D.4 partner-portal spec). Closes H.6 (#160). Shared
with Track A cryptography (#115 Spearbit/Veridise) + zk-circuit
(#117 Zellic/Veridise-ZK) audit firms at engagement kickoff.

**Audience:** audit firms (Track A wave 1 + 2 engagements),
foundation security team, Tier A buyers' compliance teams,
academic reviewers post-mainnet.

**Authoritative inputs:**
- `suwappu-strategy/docs/mainnet-plan.md` Track H §"The concrete
  construction"
- `crates/suwappu-l2-confidential/src/lib.rs` (PR #180) — the
  shipped phase-1 primitives
- `crates/suwappu-crypto/src/hash.rs` (PRs #170 + #181) — domain
  tags + HKDF
- Lether (IACR ePrint 2026/076) — the closest academic
  precedent

---

## 1. Purpose of this document

Track H's confidential-transfer construction is **the first
production deployment** of a PQ-confidential L2 paradigm on
an SP1 + DagBft rollup. No prior production exists; the
closest academic precedent is Lether (eprint 2026/076), which
is research-stage.

Audit findings on novel cryptographic constructions tend to
gravitate toward two failure modes:

1. **Over-finding**: auditor flags every domain tag, every
   length-prefix encoding, every hash composition as "novel /
   unaudited / unspecified", lengthening the audit report by
   weeks without adding security signal.
2. **Under-finding**: auditor accepts the construction at
   face value because it "looks like Zcash" and misses
   subtle SHA3-vs-BLAKE2 / domain-tag / length-encoding
   bugs that the original Zcash audits caught.

This brief is designed to **frame the construction
correctly** so auditors spend their time on the substantive
parts:

- **Standard primitives** (FIPS 202/203/204 + IETF BLS) →
  catalog only, no detailed analysis needed
- **Mechanical SHA3-swaps** of Zcash patterns → call out
  what's swapped + verify the swap is faithful + audit the
  domain-tag choices
- **Genuinely novel logic** → focus the audit budget here

---

## 2. Construction taxonomy

Every primitive in Track H falls into one of three categories.
The audit focus follows accordingly.

### 2.1 Standard primitives (no novel cryptography)

| Primitive | Source | Width | Audit focus |
|---|---|---|---|
| SHA3-256 | FIPS 202 (NIST) | 32 B | Implementation correctness only (covered by `sha3` crate v0.10 + the existing `suwappu-crypto::hash` proptest gates) |
| HKDF-SHA3-256 | RFC 5869 + FIPS 202 | configurable | RustCrypto `hkdf` v0.12 (covered by upstream audits + the `suwappu-crypto` proptest gates) |
| ML-DSA-65 (FIPS 204) | NIST PQC standard | pk=1312 B, sk=4032 B, sig=3309 B (detached) | Covered by `pqcrypto-mldsa` 0.1 → `PQClean` reference impl; audited at PQC competition |
| ML-KEM-768 (FIPS 203) | NIST PQC standard | pk=1184 B, sk=2400 B, ct=1568 B | Covered by `pqcrypto-mlkem` 0.1 → PQClean; same provenance |
| BLS12-381 (IETF draft) | RFC + `blst` 0.3 | pk=48 B (G1), sig=96 B (G2) | Covered by `blst` v0.3.16 — Supranational-audited; existing DAG-S1 proptest gate |
| `sha3_256_domain(tag, data)` | suwappu-crypto local | 32 B | Verify the length-prefix (`u32::BE(tag.len()) || tag || data`) prevents boundary-shift attacks. Audited at suwappu-crypto unit-test level. |

**Audit-firm expectation**: catalog these, confirm the
upstream audits + version pins, NO detailed analysis required.
Total time budget: ~half a day.

### 2.2 Mechanical SHA3-swaps of published Zcash patterns

Track H replaces Zcash's elliptic-curve primitives with
SHA3-256 calls. Each replacement preserves the **structural
pattern** of the Zcash primitive; only the underlying hash
changes.

| Track H primitive | Zcash analogue | Swap |
|---|---|---|
| Note commitment | Zcash Sapling note commitment (`NoteCommit^Sapling`) | Pedersen-hash-over-Jubjub → `SHA3-256-domain(SUWAPPU_L2_NOTE_COMMIT_V1, v_le ‖ r ‖ pk_owner)` |
| Nullifier | Zcash Sapling `PRF^nf` | BLAKE2s_PRF → `SHA3-256-domain(SUWAPPU_L2_NULLIFIER_V1, nk ‖ cm ‖ position_le)` |
| Nullifier key derivation | Zcash Sapling `nk` derivation | Jubjub-scalar-mult → `SHA3-256-domain(SUWAPPU_L2_NF_KEY_V1, sk_seed)` |
| Viewing-key derivation (phase 2) | Zcash Sapling IVK derivation | Pallas-scalar-mult → `ML-KEM-768.KeyGen(SHA3-256-domain(SUWAPPU_L2_VIEWING_KEY_V1, sk_seed))` |
| L2 address derivation | Ethereum address pattern | `keccak256(secp256k1_pk)[12..]` → `SHA3-256-domain(SUWAPPU_L2_ADDRESS_V1, ml_dsa_65_pk)[..20]` |

**Audit-firm expectation**: verify each swap is faithful to
its Zcash analogue (the substitution doesn't introduce new
adversary advantage), audit the domain-tag choices for
collision-resistance + boundary-shift safety, confirm the
length encoding is unambiguous. Total time budget: ~3–5
days.

**Specifically what to check** (regression-style review):

- Does the SHA3-domain length-prefix in `sha3_256_domain`
  prevent the same input from being misinterpreted as a
  different (tag, data) partition? (Tested at unit-test
  level; auditor confirms.)
- Is the field-ordering inside the commitment unambiguous
  with respect to variable-length `pk_owner`? (Yes:
  pk_owner is fixed at 1312 B per the ML-DSA-65 spec; tested
  at unit-test level.)
- Does the nullifier key `nk` leak information about the
  `sk_seed`? (No: SHA3 is modeled as a random oracle per
  the FIPS 202 security argument.)
- Is the `position` binding into the nullifier sufficient to
  prevent off-tree replay attacks? (Yes: position is unique
  per note in the commitment tree; the STARK proves the
  position is the correct path-index.)

### 2.3 Genuinely novel logic

A short list. The audit budget concentrates here.

| Component | Novelty | Audit focus |
|---|---|---|
| **STARK-of-ML-DSA spend authorization** (H.3, lands later) | Verifying an ML-DSA-65 signature inside a SP1 zkVM guest program. Prior art: `sp1-ntt-gadget` (kota1026); academic: arya-STARK (eprint 2025/2238). **No published audit of any of these.** | Soundness of the in-circuit ML-DSA verification: does the STARK fail to verify exactly when the signature is invalid? Side-channel: does the prover leak the secret key? Constraint-system completeness. **Audit firms: Zellic / Veridise-ZK (Track A.5 #117) primary owner.** |
| **PQ-only confidential construction** | NO production system uses ML-KEM-768 + ML-DSA-65 + SHA3-256 for a confidential transfer. The closest funded effort (EF ZKnox, Mar 2025) is at primitive selection; the closest academic (Lether, IACR 2026/076) is at protocol design. | High-level cryptographic argument: does the composition of standard PQ primitives + standard Zcash patterns inherit the security properties of each component? Or do compositional gaps emerge? |
| **Domain-tag namespace** | All 5 L2 domain tags (`SUWAPPU_L2_NOTE_COMMIT_V1`, `SUWAPPU_L2_NULLIFIER_V1`, `SUWAPPU_L2_NF_KEY_V1`, `SUWAPPU_L2_VIEWING_KEY_V1`, `SUWAPPU_L2_ADDRESS_V1`) are foundation-pinned. Each derives a different protocol-critical value from the same input space. | Verify the namespace is collision-free + future-extensible. Check that `_V1` versioning provides a clean upgrade path. |
| **Phase-2 hybrid encryption envelope** | ML-KEM-768 + AES-256-GCM hybrid for memo encryption. Not novel in isolation (standard hybrid PKE pattern), but the **specific composition** with viewing-key derivation needs audit. | Verify the AEAD key derivation from the ML-KEM shared secret is sound + that the nonce derivation (planned: `SHA3-256(cm)[..12]`) is unique per encryption. |

**Audit-firm expectation**: this is the substantive part of
the audit. Total time budget: 2–4 weeks across the two
audit firms.

---

## 3. What this construction does NOT claim

Important framing for the audit firms: this is the **first**
production deployment of the paradigm, NOT a "STARK-proven
new PQ scheme." Specifically:

- Track H does **NOT** claim novel cryptographic security
  properties beyond what FIPS 202/203/204 + standard hybrid
  PKE + Zcash patterns already give
- Track H does **NOT** introduce a new commitment scheme, a
  new signature scheme, or a new KEM
- Track H does **NOT** depend on any unaudited primitive
  beyond SP1 + Plonky3 (which the L2 itself depends on
  regardless of Track H)
- Track H **DOES** depend on the security argument that the
  composition is faithful — see §4 for the explicit threat
  model

The auditor's job is to validate (1) that this framing is
honest and (2) that the implementation matches the spec.

---

## 4. Threat model

The audit firms should evaluate the construction against
exactly these adversary capabilities:

### 4.1 In-scope adversaries

| Adversary | Capability | What we promise |
|---|---|---|
| Network observer | Sees all L1 + DA blob bytes | Cannot learn `v` (amount) for any confidential transfer |
| Network observer with collected ML-DSA pubkeys | Same + knows the sender's public identity | Same — confidentiality is over `v`, not over `pk_owner` |
| Malicious sequencer | Can choose what enters batches | Cannot force-mint or steal balances; bridge accounting invariant holds (per G3.2 #101 PR #184) |
| Malicious prover | Can choose what proof to submit | Cannot get an invalid batch accepted (verifier precompile rejects); the STARK soundness gives this |
| Compromised viewing-key holder | Decrypts a delegator's memos | Sees `v` for that delegator's incoming notes; cannot forge spends |
| Malicious delegator | Front-runs / griefs other delegators | Cannot extract value from confidential transfers; cannot retroactively rewrite commitments |
| Quantum adversary post-Shor's | Breaks classical ECDSA + EC-DH | Track H confidentiality + spend authorization survives (ML-DSA + ML-KEM + SHA3 are PQ); the secp256k1 EVM tx-auth layer is classical (per Open Item #8 flip) but is OUT OF SCOPE for confidential value — see PQ-honesty matrix in `suwappu-strategy/docs/mainnet-plan.md` Track H |

### 4.2 Out-of-scope adversaries

| Adversary | Why out-of-scope |
|---|---|
| Adversary who controls the foundation's domain-tag namespace | Foundation-pinned tags are part of the trust model |
| Adversary who breaks FIPS 202/203/204 | Out of scope by NIST standardization assumption |
| Adversary who breaks SP1 + Plonky3 soundness | Out of scope (covered by separate Track G audit; the L2 itself depends on this regardless of Track H) |
| Adversary who compromises the user's master seed | Out of scope (custody is the user's responsibility per E.2 #143) |
| Adversary who corrupts the DA layer | Out of scope (DA availability is a Track G concern; if the DA blob is lost, the user's note is lost even though the commitment is on-chain — accepted limitation, documented in mainnet-plan Track H) |

---

## 5. The explicit unpublished-construction flag

**ATTENTION audit-firm**: please surface the following items
EXPLICITLY in the published audit report:

1. **Nullifier construction is unpublished**: the SHA3-swap of
   Zcash Sapling's PRF^nf is faithful (we believe) but has not
   been formally audited against the Zcash spec before this
   audit.
2. **Nullifier-key derivation is unpublished**: similar
   reasoning — `nk = SHA3-256-domain(SUWAPPU_L2_NF_KEY_V1, sk_seed)`
   is a SHA3-swap of Zcash's Jubjub-scalar-mult, faithful but
   not formally audited.
3. **Viewing-key derivation (phase 2) will be unpublished**:
   when phase 2 lands, the ML-KEM-768 seeded keygen will be
   audited as part of that PR's scope.
4. **L2 address derivation is unpublished but obvious**:
   `SHA3-256-domain(SUWAPPU_L2_ADDRESS_V1, ml_dsa_pk)[..20]`
   matches Ethereum's keccak-truncation pattern; the SHA3
   swap is mechanical.

Surfacing these in the audit report serves two purposes:

- **Honesty**: pre-empts post-launch criticism that the
  construction was "snuck through" without disclosure
- **Forward defense**: if a flaw is later found in one of
  these constructions, the audit report's explicit
  flagging shifts the burden of "should have caught it"
  away from the audit firm + foundation onto the broader
  research community that didn't independently verify the
  swap

---

## 6. Per-component test corpus

The audit firms can verify the construction's behavior
against the existing test suite + add their own:

| Component | Test location | Coverage |
|---|---|---|
| `commit_note` | `crates/suwappu-l2-confidential/src/lib.rs` `tests` mod | determinism, distinguishes amount/randomness/owner, rejects wrong-width pk |
| `derive_nullifier_key` | same | determinism, distinguishes seeds |
| `compute_nullifier` | same | determinism, distinguishes position/commitment/key |
| `derive_l2_address` | same | determinism, distinguishes keys, 20-byte output |
| `sha3_256_domain` | `crates/suwappu-crypto/src/hash.rs` | length-prefix correctness, boundary-shift defense, KAT for empty + "abc" |
| `hkdf_sha3_256` | same | determinism, info-label independence, salt independence, variable-length output |
| Domain-tag distinctness | both crates | distinctness + identical-input separation |
| Proptest property tests | both crates | 256 cases at default, 10k for sprint exit gates per CLAUDE.md |

Audit firms should run the existing proptest at 100k cases
(`PROPTEST_CASES=100000 cargo test -p suwappu-l2-confidential`)
and add adversarial inputs (malformed pk, near-collision
inputs, boundary-shift attempts).

---

## 7. Engagement deliverables

The audit firms should produce, at minimum:

| Deliverable | Recipient | Format |
|---|---|---|
| **Published audit report** (sanitized) | Public — `docs/audit/<firm>-2026/` | PDF + signed integrity hash |
| **Internal audit findings** (full) | Foundation security team | PDF + raw issue tracker dump |
| **Test corpus contributions** | suwappu-dag main branch | PR adding adversarial proptest cases |
| **Construction-provenance statement** | Audit report appendix | One paragraph confirming the §3 framing is honest |
| **Composition-soundness argument** | Audit report body | High-level argument that the composition inherits each primitive's security |

Each deliverable is reviewed by the foundation board before
the audit firm's payment is released (per the standard
Track A engagement-letter terms — see Track A.3 + A.5
engagement specs).

---

## 8. Cross-references

- **Track H construction**: `suwappu-strategy/docs/mainnet-plan.md`
  Track H §"The concrete construction"
- **Phase 1 code**: `crates/suwappu-l2-confidential/src/lib.rs`
  (PR #180)
- **Domain tags + HKDF**: `crates/suwappu-crypto/src/hash.rs`
  (PR #170)
- **PQ-honesty matrix**: `suwappu-strategy/docs/mainnet-plan.md`
  Track H §"PQ-honesty posture" — explicit list of where PQ
  matters + where classical primitives are accepted at
  compatibility boundaries
- **Custody requirements**: `docs/validator-custody-requirements.md`
  (E.2, PR #181) — user master-seed custody is the
  out-of-scope adversary boundary
- **Verifier precompile**: `crates/suwappu-l2-verifier-precompile/src/lib.rs`
  (G2.2 phase 1, PR #179) — Track H circuits flow through
  this format-gate
- **Lether** (academic precedent): IACR ePrint 2026/076 —
  the foundation will help audit firms obtain a reading
  copy of the manuscript if needed
- **arya-STARK** (related work): IACR ePrint 2025/2238 —
  related Dilithium-in-STARK work

---

## 9. Change log

| Date | Change | Source |
|---|---|---|
| 2026-05-17 | Initial draft | H.6 (issue #160); audit-prep brief for Track H confidential construction, shared with Track A.3 + A.5 audit firms at engagement kickoff |
