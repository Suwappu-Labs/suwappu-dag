# IQ-013 — Post-quantum, auditable confidential transfers (CONF-1 / Track H)

**Status:** Recommendation, pending sign-off.
**Owner:** crypto / L2
**Date:** 2026-07-03
**Tracking:** CONF-1 (confidential-transfer gap); feature-parity-matrix
row **D1** ("Confidential-but-auditable transfers"); the institutional
parity-bar section of `docs/research/feature-parity-matrix.md`
(sub-transaction privacy with a regulator viewing path — the Canton-class
requirement). Track H epic (issue #156); confidential-balance L2 crate
`crates/suwappu-l2-confidential/`.

## Question

**What is the construction for post-quantum, auditable confidential
transfers on suwappu-dag?** Concretely:

1. **How are amounts hidden** — an ML-KEM-768-sealed ciphertext, a lattice
   (homomorphic) commitment, or a hash commitment?
2. **How is validity proven without revealing the amount** — a
   lattice/PQ range proof, or a range check arithmetized inside a
   zk (FRI/STARK) circuit? (Balance conservation: outputs ≤ inputs, no
   negative amounts, no overflow.)
3. **How does a regulator/auditor viewing key get selective disclosure** —
   how does an authorized party recover the cleartext amount and
   counterparties for a specific account or transaction without holding
   the spend key?
4. **Can it stay tractable given the linear-commitment-size obstacle** for
   lattice range proofs (Esgin et al., IACR 2021/1674 — commitment/proof
   size linear in the bit-width of the committed message)?

This is a **top-priority institutional differentiator**. The parity
matrix flags confidential-but-auditable transfers as a Canton-class
parity requirement (D1), and our angle — **PQ + zk** — is architecturally
stronger than Arc's TEE (no trusted-hardware assumption) and Solana's /
ERC-7984's classical ZK (not quantum-safe), *if we ship it*. It must
honor:

- **Invariant 2 (PQ-conservative):** every long-lived confidentiality and
  integrity surface uses NIST-standardized PQ primitives. A classical
  hiding/soundness assumption on a value that persists on-chain is a
  harvest-now-decrypt-later exposure and is rejected.
- **Invariant 3 (bounded on-chain footprint):** the per-transfer on-chain
  commitment surface must stay bounded; a scheme whose on-chain bytes grow
  with the amount bit-width (the linear-commitment-size obstacle) is in
  direct tension with this and must be analyzed.
- **Invariant 4 (substrate):** lane separation, schedule determinism,
  bundle atomicity, tree determinism, replay equivalence inherited from
  suwappu-db — the L2 executor wires these through and cannot weaken them.

## Current code state — what Phase 1 shipped vs. what Phase 2 needs

Grounded in `crates/suwappu-l2-confidential/src/lib.rs` (Track H, issue
#156) as of this branch:

**Phase 1 — SHIPPED** (depends only on SHA3-256 + the
`suwappu-crypto::hash` domain-tag helpers):

- `Note { v: u64, r: [u8;32], pk_owner: Vec<u8> (ML-DSA-65, 1312 B),
  position: u64 }` — the spendable note.
- `commit_note()` → `cm = SHA3-256-domain(SUWAPPU_L2_NOTE_COMMIT_V1,
  v_le(8) ‖ r(32) ‖ pk_owner(1312))`. A **32-byte hash commitment** —
  hiding from the random `r`, binding from SHA3 collision-resistance.
  **No homomorphism**: balance arithmetic is not done on commitments; it
  happens inside the (Phase 3) STARK on the cleartext `v`. This is the
  central design choice and the reason the linear-commitment-size
  obstacle does not bind (see Option B).
- `derive_nullifier_key()`, `compute_nullifier()` — SHA3-swapped Zcash
  Sapling PRF^nf pattern; double-spend prevention.
- `derive_l2_address()` — 20-byte L2 address from the ML-DSA-65 pk;
  shared L1/L2 address space.
- Unit + proptests (256 cases — **below the 10k sprint-exit-gate bar**;
  a Track H exit gate will need `PROPTEST_CASES=10000`).

**Phase 2 — UNBUILT** (the CONF-1 gap the parity matrix names). Needs
seeded ML-KEM-768 keygen + an AEAD; the workspace AEAD dep is not yet
chosen:

- **Viewing-key derivation** — a per-account ML-KEM-768 keypair from
  `SHA3-256-domain(SUWAPPU_L2_VIEWING_KEY_V1, sk_seed)`. The domain tag
  `SUWAPPU_L2_VIEWING_KEY_V1` **already exists** in
  `crates/suwappu-crypto/src/hash.rs:34` but is **not yet consumed** by
  the crate — the derivation function is unwritten.
- **Hybrid encryption envelope** — ML-KEM-768 + AES-256-GCM (or an
  ML-KEM + AEAD combiner) for the note memo carrying `(v, r)`.
- **Memo encrypt/decrypt** — the selective-disclosure payload.

**Phase 3 — UNBUILT** — STARK-of-ML-DSA spend authorization + the range
/ balance-conservation circuit, landing in the L2 STM circuit work
(H.3 / G1 / G2 phase 2).

`suwappu-crypto` **has** ML-KEM-768 today (`src/mlkem.rs`: `keypair`,
`encapsulate`, `decapsulate` over `pqcrypto-mlkem`) — but only a
**system-randomness** `keypair()`; there is **no seeded/deterministic
keygen**, which viewing-key derivation requires. That is a Phase-2
prerequisite in `suwappu-crypto`, not just in the L2 crate.

The L2 prover stack (Track G) is **SP1 zkVM**; the L1-side verifier
precompile (`crates/suwappu-l2-verifier-precompile/`) currently verifies
an **SP1 Groth16 BN254** wrap (260 B, classical pairing) — see IQ-006.
SP1's *native* proof is a **FRI-based STARK** (hash-commitment, PQ-plausible);
the Groth16 wrap is a classical on-chain-cheapness convenience, not a
soundness requirement. This distinction is load-bearing for Option B.

## Options surveyed

### Option A — arXiv 2603.05005 reference design (lattice encrypted amounts + compact PQ range proof + re-commitment + auditor path) — RECOMMENDED as the reference to evaluate against

"A Practical Post-Quantum Distributed Ledger Protocol for Financial
Institutions" (arXiv 2603.05005, Mar 2026): lattice-based
**publicly-verifiable + auditable** confidential transfers, a
**re-commitment** primitive (rerandomize a committed amount so it can be
disclosed/relinked to an auditor without revealing the spend key), and a
**compact PQ range proof**. It explicitly argues Ring-CT is unsuitable
for institutions and targets exactly CONF-1's shape: encrypted amounts +
PQ range proofs + a regulator viewing path.

- **PQ? YES** — lattice hiding + lattice range proof; no classical
  assumption on the confidential surface. Satisfies Invariant 2.
- **Bounded footprint? THE CRUX.** The design leans on a "compact" PQ
  range proof, but the general lattice range-proof result (Esgin et al.,
  IACR 2021/1674) is that commitment/proof size can be **linear in the
  message bit-width**. For a `u64` amount that is a real per-transfer byte
  cost, in tension with Invariant 3. The word "compact" in the abstract
  is precisely the claim to **verify against the paper's concrete
  parameters** before adopting — do not take it on faith.
- **Auditor path? YES, native** — the re-commitment primitive is designed
  for selective disclosure to a regulator; it is the closest published
  match to the institutional viewing-key requirement.
- **Track H fit?** Would **replace** the shipped Phase-1 hash-commitment
  path with a lattice (homomorphic) commitment and a native lattice range
  proof — a larger crypto surface, a different `Note` shape, and the full
  Esgin size-tension inherited head-on. Higher assurance ceiling, higher
  build + audit cost, and it discards the Phase-1 work.

**Verdict:** the **reference design of record** — the closest published
PQ-auditable construction, and the yardstick every choice below is
measured against. Recommend a crypto-reviewer-led read to (a) confirm the
"compact" range-proof parameters are actually bounded for `u64` amounts,
and (b) mine the re-commitment/auditor primitive for Option B's viewing
path — but **not** the near-term build target, because it throws away the
Phase-1 hash-commitment structure that sidesteps the size obstacle.

### Option B — ML-KEM-768-sealed amounts + a FRI/STARK range proof on the existing SP1/Plonky3 stack (extends Phase 1) — RECOMMENDED to prototype

Keep the shipped Phase-1 shape and finish it:

- **Hiding:** amount lives in cleartext inside the owner's `Note`; the
  on-chain handle is the **32-byte SHA3-256 note commitment** (hiding from
  random `r`). The amount + randomness are additionally **ML-KEM-768-sealed**
  in the note memo (Phase 2 hybrid envelope) so the recipient — and an
  authorized viewer — can recover them.
- **Validity without revealing the amount:** the range check (`0 ≤ v <
  2^64`, no overflow) and balance conservation (Σ inputs = Σ outputs) are
  **arithmetized as constraints inside the L2 STARK** over the cleartext
  `v` (Phase 3), *not* proven by a standalone lattice range proof. The
  proof is FRI-based (hash-commitment) and its size is **constant per
  batch** (amortized over all transfers), independent of the amount
  bit-width.
- **Auditor viewing key:** a per-account ML-KEM-768 viewing keypair
  derived from `SHA3-256-domain(SUWAPPU_L2_VIEWING_KEY_V1, sk_seed)`
  (Phase 2). Selective disclosure = the account holder (or a policy escrow)
  hands the regulator the viewing **secret** key; the regulator decapsulates
  the memo envelopes for that account and recovers `(v, r, counterparties)`
  — read-only, no spend authority. Borrow the re-commitment idea from
  Option A if per-transaction (rather than per-account) disclosure
  granularity is required.

- **PQ? YES, if the proof stays FRI/STARK.** ML-KEM-768 seal + SHA3-256
  commitment + FRI range proof are all PQ. **Critical constraint:** the
  soundness proof must be the **native SP1 FRI/STARK**, *not* the Groth16
  BN254 wrap the L1 verifier precompile uses today — Groth16/BN254 is
  classical (Invariant 2 would be violated if confidential-transfer
  *soundness* rested on it). This forces either a FRI verifier on L1 or an
  explicit, documented exception-zone argument (analogous to IQ-009's BLS
  handling) if the Groth16 wrap is retained for on-chain cheapness.
- **Bounded footprint? YES** — 32-byte commitment + 32-byte nullifier per
  note; range/balance proof is one constant-size STARK per batch. **The
  linear-commitment-size obstacle does not bind**, because there is no
  lattice range proof over a lattice commitment: the range check is a
  circuit constraint over cleartext, so amount bit-width never enters the
  on-chain byte count. This is the decisive Invariant-3 advantage over
  Option A.
- **Auditor path? YES** — ML-KEM-768 viewing key (above).
- **Track H fit? NATIVE** — it *is* the Phase 1/2/3 plan already in the
  crate's module doc. Phase 1 shipped; this option is "build Phase 2 +
  Phase 3," reusing Track G's SP1 prover and the existing hash/nullifier
  primitives.

**Verdict:** the **prototype target.** Lowest marginal cost (extends
shipped code), strongest Invariant-3 story, leverages the existing FRI
stack. Gate on `crypto-reviewer` for: FRI-vs-Groth16 soundness surface,
ML-KEM viewing-key KDF, memo-envelope AEAD choice, and in-circuit range
/ conservation constraints.

### Option C — classical twisted-ElGamal + Bulletproofs (ERC-7984 / Solana Confidential Transfers pattern) — REJECTED (non-PQ)

The deployed-standard baseline: Pedersen/twisted-ElGamal homomorphic
commitments + Bulletproofs range proofs, as in Solana Token-2022
Confidential Transfers and the ERC-7984 confidential-token standard.

- **PQ? NO.** Both the ElGamal hiding and the Bulletproofs soundness rest
  on the **discrete-log assumption** (classical). A confidential amount is
  a long-lived on-chain confidentiality surface — harvest-now-decrypt-later
  breaks it under a quantum adversary. **Directly violates Invariant 2.**
- Named only to make the failure explicit and to mark the differentiation:
  the entire deployed confidential-transfer ecosystem is classical, so a
  PQ-confidential design is genuinely novel, *and* re-using it would
  forfeit the one thing that makes CONF-1 a differentiator.

**Verdict:** REJECTED. Invariant-2 violation; retained only as the "why
PQ matters here" contrast.

### Option D — TEE-based confidential execution (Arc's approach) — REJECTED (trusted hardware)

Arc ships opt-in confidential transfers via a **Trusted Execution
Environment** producing attested results, plus "Arc Privacy" (confidential
smart contracts with compliance/audit access).

- **PQ? N/A / moot** — a TEE moves the trust to the hardware attestation,
  not to a cryptographic hardness assumption. But it **reintroduces a
  trusted-hardware assumption** (SGX/TDX-class), with a documented history
  of side-channel and attestation-key compromises.
- Auditor path exists (Arc offers selective disclosure), and footprint is
  fine — but the assumption is exactly the one our thesis rejects: the
  parity matrix positions our zk+PQ angle as **"stronger than Arc's TEE —
  no trusted-hardware assumption."** Adopting a TEE would erase the
  differentiator.

**Verdict:** REJECTED. Reintroduces trusted hardware; contradicts the
stated CONF-1 positioning.

## Recommendation

**Evaluate A as the reference; prototype B on the existing FRI stack;
gate on `crypto-reviewer`. Reject C (non-PQ) and D (trusted hardware).**

- Adopt **arXiv 2603.05005 (Option A)** as the reference design of record.
  Crypto-reviewer-led read to (1) confirm whether its "compact" PQ range
  proof is actually bounded for `u64` amounts (the Esgin linear-size
  question), and (2) extract its re-commitment/auditor primitive for B's
  viewing path.
- Build out **Option B** as the shipping construction: finish Phase 2
  (ML-KEM-768 viewing keys + hybrid AEAD memo envelope) and Phase 3
  (in-circuit range + balance-conservation STARK), extending the Phase-1
  hash-commitment code already in `suwappu-l2-confidential`.
- Fold Option A's lattice construction back in **only if** the
  crypto-reviewer read shows its compact range proof is genuinely bounded
  *and* the higher assurance justifies discarding B's simpler,
  Invariant-3-clean structure.

## Implementation sketch

`suwappu-l2-confidential` (extend the crate; Phase 2 + Phase 3):

```rust
// Phase 2 — viewing key (needs SEEDED ML-KEM-768 keygen in suwappu-crypto)
pub struct ViewingKey {
    pub pk: mlkem::PublicKey,   // published; lets senders seal memos to the account
    pub sk: mlkem::SecretKey,   // disclosed to an auditor for read-only recovery
}
pub fn derive_viewing_key(sk_seed: &[u8; 32]) -> ViewingKey;
//   seed = SHA3-256-domain(SUWAPPU_L2_VIEWING_KEY_V1, sk_seed)  // tag already in hash.rs
//   -> mlkem::keypair_from_seed(seed)   // NEW in suwappu-crypto/src/mlkem.rs (deterministic)

// Phase 2 — hybrid memo envelope (ML-KEM-768 + AEAD)
pub struct EncryptedMemo {
    pub kem_ct: mlkem::Ciphertext,  // ≈1,568 B ML-KEM-768 ciphertext (sealed session key)
    pub aead_ct: Vec<u8>,           // AES-256-GCM over (v ‖ r ‖ recipient/context)
    pub nonce: [u8; 12],
}
pub fn seal_memo(view_pk: &mlkem::PublicKey, note: &Note) -> EncryptedMemo;
pub fn open_memo(view_sk: &mlkem::SecretKey, memo: &EncryptedMemo)
    -> Result<(u64 /*v*/, [u8;32] /*r*/), ConfidentialError>;   // == auditor disclosure path
```

Primitives:

- `suwappu-crypto::mlkem` — **add seeded/deterministic `keypair_from_seed`**
  (the current `keypair()` is system-randomness only). Prerequisite for a
  reproducible viewing key.
- `suwappu-crypto::hash::SUWAPPU_L2_VIEWING_KEY_V1` — already defined;
  wire it into `derive_viewing_key`.
- AEAD dep — choose the workspace AES-256-GCM (or ML-KEM + AEAD combiner)
  crate; `cargo-deny` review required.

Range / conservation (Phase 3, L2 STM circuit — Track G / H.3):

- In-circuit constraints over cleartext `v`: `0 ≤ v < 2^64`, Σ inputs =
  Σ outputs, nullifier non-membership (prev state) + membership (new
  state), and a STARK-of-ML-DSA-65 spend authorization over
  `(cm_in, nf_out, cm_out_list)`.
- **Soundness must ride the native SP1 FRI/STARK, not the Groth16 BN254
  wrap** (Invariant 2). If the Groth16 wrap is retained for cheap L1
  verification, document it as an explicit exception zone with a migration
  target (mirror IQ-009's treatment of the classical BLS aggregate), and
  scope a FRI verifier for the confidential-transfer soundness surface.

## Open questions

1. **The linear-commitment-size obstacle (the crux).** For Option A:
   confirm whether arXiv 2603.05005's "compact" range proof is actually
   bounded for `u64` amounts, or whether it inherits the Esgin
   (2021/1674) linear-in-bit-width blow-up. For Option B this is moot by
   construction (range check is a circuit constraint, not a lattice
   proof) — **which is precisely why B is the recommended prototype.**
2. **FRI-verifier on L1.** Keeping confidential-transfer soundness PQ
   means an L1 FRI/STARK verifier, or a documented exception if the
   Groth16 BN254 wrap is used. Cost, proof-size, and audit implications?
   Coordinate with IQ-006 (verifier precompile) and IQ-009 (exception-zone
   discipline).
3. **Prover cost.** In-circuit STARK-of-ML-DSA-65 (1312-byte pk, 3.3 KB
   sig) spend authorization is expensive. What is the per-transfer / per-
   batch proving time on the SP1 stack, and does it fit the L2 batch
   cadence (5–10 s)?
4. **Disclosure granularity.** Per-account viewing key (hand over one
   ML-KEM `sk`, auditor sees all of that account's memos) vs.
   per-transaction disclosure (Option A's re-commitment). Which does the
   institutional/regulator requirement actually demand, and does travel-
   rule (D3 / COMP-1) need counterparty linkage the memo must carry?
5. **Seeded ML-KEM keygen determinism.** `pqcrypto-mlkem` may not expose
   deterministic seeded keygen directly; confirm a FIPS-203-conformant
   derandomized keygen path (or an alternative ML-KEM crate) is available
   for `keypair_from_seed`.
6. **Exit-gate proptests.** Phase-1 tests run 256 cases; a Track H sprint
   exit gate needs `PROPTEST_CASES=10000` per repo convention.

## Decision

**Pending sign-off.**

## See also

- `crates/suwappu-l2-confidential/src/lib.rs` — Phase-1 primitives
  (shipped) + the Phase-2/3 plan in the module doc.
- `crates/suwappu-crypto/src/mlkem.rs` — ML-KEM-768 (needs seeded keygen
  for viewing keys).
- `crates/suwappu-crypto/src/hash.rs:34` — `SUWAPPU_L2_VIEWING_KEY_V1`
  domain tag (defined, unused).
- `crates/suwappu-l2-verifier-precompile/src/lib.rs` — SP1 Groth16 BN254
  L1 verifier (the classical wrap; the FRI-vs-Groth16 soundness question).
- `docs/research/feature-parity-matrix.md` — row D1 + the institutional
  parity-bar section (CONF-1 as a Canton-class requirement).
- `docs/research/briefs/2026-new-entrants-and-papers.md` §3c — arXiv
  2603.05005; the ERC-7984 / Solana classical caution; the Esgin
  (2021/1674) linear-size caution.
- `docs/research/briefs/arc.md` (TEE confidential + Arc Privacy) ·
  `docs/research/briefs/tempo.md` (opt-in confidential balances).
- IQ-006 (verifier precompile / L2 state-root surface) · IQ-009 (PQ
  aggregate exception-zone discipline).
</content>
</invoke>
