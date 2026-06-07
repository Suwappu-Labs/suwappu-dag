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
| LTP aggregate signatures | BLS12-381 | Hash-based + SP1-STARK aggregation, 2027–2029 |
| Optional verification mode | Groth16 over BN254 | Default to FRI by ~2030; Groth16 retained as opt-in |

### Why retain

**ECDSA secp256k1** is retained on the account-signing surface for
compatibility with the EVM wallet and HSM ecosystem; hybrid composition
preserves the post-quantum guarantee while the ML-DSA-65 component is unbroken.

**BLS12-381** is retained on the LTP aggregate-signature surface for the
aggregation efficiency required by the constant-size on-chain commitment;
production-grade hash-based + SP1-STARK aggregation is not yet at
gas-economics parity.

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

## Migration sequencing

1. **2026 mainnet (G2):** ML-DSA-65 + ML-KEM-768 live; ECDSA hybrid retained.
2. **2027–2029 (G3):** BLS12-381 aggregate signatures migrate to hash-based +
   SP1-STARK aggregation; FRI default for verification.
3. **~2030 (G4):** Pure ML-DSA-65 on EVM account signing; Groth16 retained as
   opt-in only.

Counterparties making forward-looking commitments on transaction signatures,
LTP aggregate signatures, or Groth16 verification should reference the
2027–2030 migration roadmap.
