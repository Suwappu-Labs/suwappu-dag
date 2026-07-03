# Cryptographic posture

Paper §3.3 + §12. Implemented in [`suwappu-crypto`](../../crates/suwappu-crypto).

## Post-quantum-conservative surfaces

The chain is post-quantum-conservative on long-lived confidentiality surfaces
(LTP sealed session keys, shielded-execution payloads, DID credential proofs)
at launch.

| Primitive | Standard | Use site |
|---|---|---|
| **ML-DSA-65** | FIPS 204, 2024 | Authority-Ring signing, LTP integrity surface |
| **ML-KEM-768** | FIPS 203, 2024 | LTP sealed session keys (≈1,568 B in the 1,600 B on-chain commitment) |
| **SHA3-256** | FIPS 202 | LTP payload root, transport integrity, state-tree leaf encoding |
| **Poseidon2** | — | Arithmetic-friendly contexts; **not** on the LTP integrity surface |

Cryptographic-correctness assumptions:

- Existential unforgeability of ML-DSA-65 under chosen-message attack
- IND-CCA2 security of ML-KEM-768
- Collision resistance of SHA3-256 and Poseidon2

Verified by DAG-S1 sprint exit gate (`tests/proptest_roundtrips.rs`,
70,000 cases pass in 41 s release-mode).

## Classical-cryptography exception zones at launch

Three classical-cryptography surfaces are retained by design (paper Table 1);
each has a migration target.

| Surface | Algorithm | Migration target |
|---|---|---|
| EVM account TX signing | ECDSA secp256k1, hybrid-composed with ML-DSA-65 | Pure ML-DSA-65 by ~2030 |
| LTP aggregate signatures | BLS12-381 | Hash-based ML-DSA Merkle aggregate (on-chain), FRI/STARK recursion (verification) — **time-boxed, sunset by end-2028**; see [IQ-009](../iq/IQ-009-ltp-aggregate-pq-migration.md) |
| Optional verification mode | Groth16 over BN254 | Default to FRI by ~2030; Groth16 retained as opt-in |

### Why retain

**ECDSA secp256k1** is retained on the account-signing surface for
compatibility with the EVM wallet and HSM ecosystem; hybrid composition
preserves the post-quantum guarantee while the ML-DSA-65 component is unbroken.

**BLS12-381** is retained on the LTP aggregate-signature surface for the
aggregation efficiency required by the constant-size on-chain commitment;
production-grade hash-based + SP1-STARK aggregation is not yet at
gas-economics parity. This retention is **time-boxed, not open-ended**
(PQ-1 / [IQ-009](../iq/IQ-009-ltp-aggregate-pq-migration.md), Phase 0):
the exception has a **published sunset of end-2028**, comfortably inside
both regulatory horizons below, and migrates into the Phase-1 hash-based
ML-DSA Merkle aggregate.

The principled reason the aggregate *signature* can be time-boxed while
the co-located ML-KEM-768 *ciphertext* must be PQ **today** is a
threat-model distinction: **a signature is an ephemeral-integrity
surface, not a long-lived-confidentiality surface.** Harvest-now-decrypt-later
— the reason the KEM cannot wait — does not apply to a signature. A
BLS forgery has value only while a cryptographically-relevant quantum
computer exists *and* the attestation it forges is still being relied on
at a live destination; that deadline is materially later than the
confidentiality deadline governing the KEM. This is the correct rebuttal
to "why is half your constant-size commitment still classical?" — the two
halves have different clocks.

**Groth16** is offered as a verifier-side compactness option alongside FRI; it
does not produce different proof bodies for the same statement and does not
split the audit surface.

### Single-stack risk

Several verification surfaces share the SP1/Plonky3 stack: state commitments,
the reserve-coverage predicate, shielded execution, the cross-chain DID
synchronization path, and FRI mode verification. A failure mode in SP1 or
Plonky3 propagates across multiple subsystems.

**Mitigations:**

- Dual on-chain verification mode (Groth16 + FRI),
- Parameter-versioned circuits per subsystem,
- Audit lineage scoped specifically to SP1 and Plonky3.

The concentration is bounded; it is not eliminated.

## Constant-size LTP commitment

Per paper §10.2, every LTP attestation commits ≈1,600 B regardless of payload
complexity:

```text
ML-KEM-768 ciphertext   ≈ 1,568 B
BLS12-381 agg signature ≈    96 B
SHA3-256 payload root   =    32 B
─────────────────────────────────
Total                   ≈ 1,600 B
```

Implementation: [`suwappu-ltp::ON_CHAIN_COMMITMENT_BYTES`](../../crates/suwappu-ltp/src/lib.rs).
Property tested at the sprint exit gate (DAG-S15).

**What "constant-size" means after the PQ migration.** The ≈1,600 B figure
above holds *today* only because the aggregate is classical BLS (96 B for
any signer count). No post-quantum signature scheme reproduces a ~96-byte
native aggregate — the 2025–2026 literature is unanimous on this (see
[IQ-009 §2026 literature update](../iq/IQ-009-ltp-aggregate-pq-migration.md)
and [`../research/briefs/2026-new-entrants-and-papers.md`](../research/briefs/2026-new-entrants-and-papers.md) §3a).
The invariant that survives the migration is therefore **O(1) in signer
count** (and in payload), *not* a fixed 96-byte aggregate: the Phase-1
target replaces the 96 B BLS object with a **32-byte hash-based Merkle
root** on-chain and moves the ML-DSA-65 signature witnesses to the
Commitment-Node DA layer. The on-chain commitment stays constant-size and
becomes constant in signer count by construction; the tradeoff is a
verification-model change (fetch witnesses from DA) rather than a
commitment-size change. Read Invariant 3 as *constant in payload and
signer count*, which is the property that is actually load-bearing for
cross-chain cost scaling.

## Migration sequencing

1. **2026 mainnet (G2):** ML-DSA-65 + ML-KEM-768 live; ECDSA hybrid retained;
   BLS aggregate exception **time-boxed** with an end-2028 sunset (PQ-1 Phase 0).
2. **2027–2028 (G3):** BLS12-381 aggregate migrates to the hash-based ML-DSA
   Merkle aggregate (IQ-009 Phase 1); threshold ML-DSA (arXiv 2601.20917,
   standard 3.3 KB FIPS-204 signatures) is the near-term target for the
   Authority-ring checkpoint co-signature.
3. **2028+ (G3→G4):** optional FRI/STARK recursion for self-contained
   verification (IQ-009 Phase 2); FRI default for verification.
4. **~2030 (G4):** Pure ML-DSA-65 on EVM account signing; Groth16 retained as
   opt-in only.

### Regulatory horizons (the migration clock)

Two dated US horizons govern the exception sunsets, and both sit *after*
our targets:

- **Executive Order 14412** (2026-06-22): federal PQC mandate — key
  establishment by **2030-12-31**, digital signatures by **2031-12-31**
  (civilian systems).
- **NSA CNSA 2.0**: national-security systems — procurement preference
  **2027-01-01**, exclusive use ~**2035**.

Our BLS aggregate sunset (end-2028) and ECDSA/Groth16 targets (~2030)
land inside both windows. The Eurosystem has demonstrated PQC signatures
inside TARGET2-like wholesale settlement (BIS Papers No. 158), so the
migratability of the settlement surface is externally evidenced.

Counterparties making forward-looking commitments on transaction signatures,
LTP aggregate signatures, or Groth16 verification should reference the
2027–2030 migration roadmap and the two horizons above.
