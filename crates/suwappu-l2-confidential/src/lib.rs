//! suwappu-l2-confidential — host-side PQ-confidential transfer
//! primitives for the L2 (Track H, issue #156).
//!
//! Implements the construction ratified in
//! `suwappu-strategy/docs/mainnet-plan.md` Track H: an engineering
//! composition of published primitives (Zcash-style note
//! commitments with SHA3-256 in place of every EC primitive,
//! plus the Lether-paradigm L2-confidential-transfer shape).
//! The closest academic precedent is Lether (IACR ePrint
//! 2026/076); this crate ships the first production deployment
//! of the paradigm on an SP1+Mysticeti rollup.
//!
//! ## Phase split
//!
//! - **Phase 1 (this PR / #156)** — primitives that depend only
//!   on SHA3-256 + the existing `suwappu-crypto::hash` domain-tag
//!   helpers from PR #170:
//!   - `Note` struct (value + randomness + owner pk + tree position)
//!   - `commit_note()` — note commitment
//!   - `derive_nullifier_key()` — nullifier key from seed
//!   - `compute_nullifier()` — nullifier for a (cm, position) pair
//!   - `derive_l2_address()` — 20-byte L2 address from ML-DSA pk
//! - **Phase 2** — primitives that need seeded ML-KEM keygen +
//!   AEAD (workspace dep choice pending):
//!   - Viewing-key derivation (per-account ML-KEM-768 keypair
//!     from `SHA3-256-domain(SUWAPPU_L2_VIEWING_KEY_V1, sk_seed)`)
//!   - Hybrid encryption envelope (ML-KEM + AES-256-GCM)
//!   - Memo encrypt/decrypt
//! - **Phase 3** — STARK-of-ML-DSA spend authorization. Lands
//!   in the L2 STM circuit work (H.3 / G1 / G2 phase 2).
//!
//! ## Construction sources
//!
//! Per `docs/iq/IQ-006-l2-state-root-commitment-surface.md` +
//! the Track H section of the strategic plan:
//!
//! - **Commitment**: `cm = SHA3-256-domain(SUWAPPU_L2_NOTE_COMMIT_V1,
//!   v_le ‖ r ‖ pk_owner)` — hiding from random `r`, binding
//!   from SHA3 collision-resistance. No homomorphism needed
//!   (balance arithmetic happens inside the STARK on cleartext
//!   — big simplification vs Lether's BDLOP scheme).
//! - **Nullifier**: `nf = SHA3-256-domain(SUWAPPU_L2_NULLIFIER_V1,
//!   nk ‖ cm ‖ position_le)` — mechanical SHA3-swap of
//!   Zcash Sapling's PRF^nf pattern.
//! - **Nullifier key**: `nk = SHA3-256-domain(SUWAPPU_L2_NF_KEY_V1,
//!   sk_seed)` — derives a per-account nullifier key from the
//!   master seed.
//! - **L2 address**: `addr = SHA3-256-domain(SUWAPPU_L2_ADDRESS_V1,
//!   pk_owner_mldsa)[..20]` — 20-byte truncation matches the
//!   Ethereum address shape; L1 + L2 share the same address
//!   space.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod nullifier_tree;

pub use nullifier_tree::{IndexedMerkleTree, Leaf, MembershipProof, NonMembershipProof, TreeError};

use suwappu_crypto::hash::{
    sha3_256_domain, SUWAPPU_L2_ADDRESS_V1, SUWAPPU_L2_NF_KEY_V1, SUWAPPU_L2_NOTE_COMMIT_V1,
    SUWAPPU_L2_NULLIFIER_V1,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Length of an ML-DSA-65 public key in bytes (FIPS 204 mldsa65 spec).
/// Re-declared here to avoid a runtime check on every call; the
/// `Note::pk_owner` field carries this width and `commit_note`
/// asserts it.
pub const ML_DSA_65_PUBLIC_KEY_BYTES: usize = 1312;

/// Length of a 20-byte L2 address (matches Ethereum's address
/// width; L1 + L2 share the same address space).
pub const L2_ADDRESS_BYTES: usize = 20;

/// Length of the nullifier-key derivation seed in bytes.
pub const NF_KEY_SEED_BYTES: usize = 32;

/// 32-byte note commitment (`cm`). Lands on L1 as the
/// public-handle for a confidential note. Hides the amount;
/// binds the (v, r, pk_owner) triple via SHA3 collision-
/// resistance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Commitment(pub [u8; 32]);

/// 32-byte nullifier (`nf`). Posted to L1 when a note is
/// spent; subsequent spends with the same `nf` are double-
/// spends and rejected by the L2 STM at proof-verification
/// time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Nullifier(pub [u8; 32]);

/// 32-byte nullifier key. Per-account, derived from the
/// master seed via `derive_nullifier_key`. Held by the owner;
/// required to compute nullifiers for owned notes.
///
/// The nullifier key is hashed (not signed) into the
/// nullifier, so it doesn't need to be a high-entropy
/// signature key — a 32-byte uniform random output of
/// SHA3 over the seed is sufficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NullifierKey(pub [u8; 32]);

/// A spendable confidential note. The owner sees all four
/// fields; observers see only `Commitment` on L1 and the
/// encrypted memo carrying `(v, r)` in the DA blob (phase 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// Amount in cleartext. Only the owner sees this — the
    /// commitment hides it from observers.
    pub v: u64,
    /// Randomness used for hiding. SHOULD be sampled uniformly
    /// from `[u8; 32]` per note; reuse breaks indistinguishability.
    pub r: [u8; 32],
    /// ML-DSA-65 public key of the note's owner (1312 bytes).
    /// The owner spends by producing a STARK-of-ML-DSA-signature
    /// over `(cm_in, nf_out, cm_out_list)` (H.3).
    pub pk_owner: Vec<u8>,
    /// Index of the note's commitment in the L2 commitment
    /// tree. Bound into the nullifier to prevent off-tree
    /// replays.
    pub position: u64,
}

/// Errors emitted by the Track H phase-1 primitives.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ConfidentialError {
    /// `Note.pk_owner` was not the canonical ML-DSA-65 width.
    /// The substrate-side handler rejects invalid notes at the
    /// dispatch boundary; this error variant fires when a
    /// caller has bypassed that gate.
    #[error("note.pk_owner length must be {expected}, got {got}")]
    BadPublicKeyLength {
        /// Required width (ML_DSA_65_PUBLIC_KEY_BYTES = 1312).
        expected: usize,
        /// Observed width.
        got: usize,
    },
}

/// Compute the note commitment
/// `cm = SHA3-256-domain(SUWAPPU_L2_NOTE_COMMIT_V1, v_le(8) ‖ r(32) ‖ pk_owner)`.
///
/// The `v_le` encoding is little-endian to match the EVM
/// convention used on the L2 (per Open Item #8 EVM-flip);
/// inside the STARK the same encoding decodes back to the
/// cleartext `v` for the range proof.
///
/// Returns `BadPublicKeyLength` if `note.pk_owner` is not
/// the 1312-byte ML-DSA-65 public key width.
pub fn commit_note(note: &Note) -> Result<Commitment, ConfidentialError> {
    if note.pk_owner.len() != ML_DSA_65_PUBLIC_KEY_BYTES {
        return Err(ConfidentialError::BadPublicKeyLength {
            expected: ML_DSA_65_PUBLIC_KEY_BYTES,
            got: note.pk_owner.len(),
        });
    }
    // Concatenate v_le(8) || r(32) || pk_owner(1312) = 1352 bytes.
    let mut buf = Vec::with_capacity(8 + 32 + ML_DSA_65_PUBLIC_KEY_BYTES);
    buf.extend_from_slice(&note.v.to_le_bytes());
    buf.extend_from_slice(&note.r);
    buf.extend_from_slice(&note.pk_owner);
    let digest = sha3_256_domain(SUWAPPU_L2_NOTE_COMMIT_V1, &buf);
    Ok(Commitment(digest))
}

/// Derive the per-account nullifier key
/// `nk = SHA3-256-domain(SUWAPPU_L2_NF_KEY_V1, sk_seed)`.
///
/// `sk_seed` MUST be 32 bytes of uniform random entropy
/// (typically the user's BIP-39-style master seed truncated
/// or HKDF-expanded to 32 bytes; the master-seed shape is
/// out of scope for this crate). Same seed always derives
/// the same `nk`.
pub fn derive_nullifier_key(sk_seed: &[u8; NF_KEY_SEED_BYTES]) -> NullifierKey {
    let digest = sha3_256_domain(SUWAPPU_L2_NF_KEY_V1, sk_seed);
    NullifierKey(digest)
}

/// Compute the nullifier for a spent note
/// `nf = SHA3-256-domain(SUWAPPU_L2_NULLIFIER_V1, nk ‖ cm ‖ position_le(8))`.
///
/// The STARK proves nullifier non-membership in the
/// previous-state nullifier set + membership in the new-state
/// set. Same `(nk, cm, position)` always derives the same
/// `nf` — and the STARK's PRF binding ensures the owner is the
/// only party who can compute it.
pub fn compute_nullifier(nk: &NullifierKey, cm: &Commitment, position: u64) -> Nullifier {
    let mut buf = Vec::with_capacity(32 + 32 + 8);
    buf.extend_from_slice(&nk.0);
    buf.extend_from_slice(&cm.0);
    buf.extend_from_slice(&position.to_le_bytes());
    let digest = sha3_256_domain(SUWAPPU_L2_NULLIFIER_V1, &buf);
    Nullifier(digest)
}

/// Derive the 20-byte L2 address from an ML-DSA-65 public key
/// via `addr = SHA3-256-domain(SUWAPPU_L2_ADDRESS_V1, pk)[..20]`.
///
/// The truncation-to-20-bytes matches the Ethereum address
/// shape so L1 + L2 share the same address space. Per the
/// Track H spec, confidential-address derivation reuses the
/// existing reserved-address gate from `suwappu-execution::reserved`
/// — no special-casing needed at the address-space level.
///
/// Returns `BadPublicKeyLength` if `pk` is not 1312 bytes.
pub fn derive_l2_address(pk: &[u8]) -> Result<[u8; L2_ADDRESS_BYTES], ConfidentialError> {
    if pk.len() != ML_DSA_65_PUBLIC_KEY_BYTES {
        return Err(ConfidentialError::BadPublicKeyLength {
            expected: ML_DSA_65_PUBLIC_KEY_BYTES,
            got: pk.len(),
        });
    }
    let digest = sha3_256_domain(SUWAPPU_L2_ADDRESS_V1, pk);
    let mut out = [0u8; L2_ADDRESS_BYTES];
    out.copy_from_slice(&digest[..L2_ADDRESS_BYTES]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// Build a Note with a deterministic-but-valid pk_owner
    /// (1312 bytes of `pk_byte` repeated). Used in unit tests
    /// where the actual ML-DSA-65 keypair content is irrelevant
    /// to the construction's semantics — only its width is
    /// checked.
    fn make_note(v: u64, r_byte: u8, pk_byte: u8, position: u64) -> Note {
        Note {
            v,
            r: [r_byte; 32],
            pk_owner: vec![pk_byte; ML_DSA_65_PUBLIC_KEY_BYTES],
            position,
        }
    }

    // ----- commit_note -----

    #[test]
    fn commit_note_is_deterministic() {
        let note = make_note(100, 0xab, 0xcd, 5);
        let cm1 = commit_note(&note).unwrap();
        let cm2 = commit_note(&note).unwrap();
        assert_eq!(cm1, cm2);
    }

    #[test]
    fn commit_note_distinguishes_amount() {
        let n1 = make_note(100, 0xab, 0xcd, 5);
        let n2 = make_note(101, 0xab, 0xcd, 5);
        assert_ne!(commit_note(&n1).unwrap(), commit_note(&n2).unwrap());
    }

    #[test]
    fn commit_note_distinguishes_randomness() {
        let n1 = make_note(100, 0xab, 0xcd, 5);
        let n2 = make_note(100, 0xac, 0xcd, 5);
        assert_ne!(commit_note(&n1).unwrap(), commit_note(&n2).unwrap());
    }

    #[test]
    fn commit_note_distinguishes_owner() {
        let n1 = make_note(100, 0xab, 0xcd, 5);
        let n2 = make_note(100, 0xab, 0xce, 5);
        assert_ne!(commit_note(&n1).unwrap(), commit_note(&n2).unwrap());
    }

    #[test]
    fn commit_note_rejects_short_pk() {
        let mut bad = make_note(100, 0xab, 0xcd, 5);
        bad.pk_owner.truncate(100);
        assert!(matches!(
            commit_note(&bad),
            Err(ConfidentialError::BadPublicKeyLength {
                expected: ML_DSA_65_PUBLIC_KEY_BYTES,
                got: 100,
            })
        ));
    }

    #[test]
    fn commit_note_rejects_long_pk() {
        let mut bad = make_note(100, 0xab, 0xcd, 5);
        bad.pk_owner.push(0xff);
        assert!(matches!(
            commit_note(&bad),
            Err(ConfidentialError::BadPublicKeyLength { .. })
        ));
    }

    // ----- derive_nullifier_key -----

    #[test]
    fn nullifier_key_is_deterministic() {
        let seed = [0xabu8; 32];
        assert_eq!(derive_nullifier_key(&seed), derive_nullifier_key(&seed));
    }

    #[test]
    fn nullifier_key_distinguishes_seeds() {
        let nk_a = derive_nullifier_key(&[0xab; 32]);
        let nk_b = derive_nullifier_key(&[0xac; 32]);
        assert_ne!(nk_a, nk_b);
    }

    // ----- compute_nullifier -----

    #[test]
    fn nullifier_is_deterministic() {
        let nk = derive_nullifier_key(&[0xab; 32]);
        let cm = Commitment([0xcd; 32]);
        assert_eq!(
            compute_nullifier(&nk, &cm, 5),
            compute_nullifier(&nk, &cm, 5)
        );
    }

    #[test]
    fn nullifier_distinguishes_position() {
        let nk = derive_nullifier_key(&[0xab; 32]);
        let cm = Commitment([0xcd; 32]);
        assert_ne!(
            compute_nullifier(&nk, &cm, 5),
            compute_nullifier(&nk, &cm, 6)
        );
    }

    #[test]
    fn nullifier_distinguishes_commitment() {
        let nk = derive_nullifier_key(&[0xab; 32]);
        let cm_a = Commitment([0xcd; 32]);
        let cm_b = Commitment([0xce; 32]);
        assert_ne!(
            compute_nullifier(&nk, &cm_a, 5),
            compute_nullifier(&nk, &cm_b, 5)
        );
    }

    #[test]
    fn nullifier_distinguishes_key() {
        let cm = Commitment([0xcd; 32]);
        let nk_a = derive_nullifier_key(&[0xab; 32]);
        let nk_b = derive_nullifier_key(&[0xac; 32]);
        assert_ne!(
            compute_nullifier(&nk_a, &cm, 5),
            compute_nullifier(&nk_b, &cm, 5)
        );
    }

    // ----- derive_l2_address -----

    #[test]
    fn l2_address_is_deterministic() {
        let pk = vec![0xab; ML_DSA_65_PUBLIC_KEY_BYTES];
        assert_eq!(
            derive_l2_address(&pk).unwrap(),
            derive_l2_address(&pk).unwrap()
        );
    }

    #[test]
    fn l2_address_distinguishes_keys() {
        let pk_a = vec![0xab; ML_DSA_65_PUBLIC_KEY_BYTES];
        let pk_b = vec![0xac; ML_DSA_65_PUBLIC_KEY_BYTES];
        assert_ne!(
            derive_l2_address(&pk_a).unwrap(),
            derive_l2_address(&pk_b).unwrap()
        );
    }

    #[test]
    fn l2_address_is_20_bytes() {
        let pk = vec![0xab; ML_DSA_65_PUBLIC_KEY_BYTES];
        let addr = derive_l2_address(&pk).unwrap();
        assert_eq!(addr.len(), 20);
    }

    #[test]
    fn l2_address_rejects_wrong_width_pk() {
        let pk = vec![0xab; 100];
        assert!(matches!(
            derive_l2_address(&pk),
            Err(ConfidentialError::BadPublicKeyLength { .. })
        ));
    }

    // ----- cross-domain distinctness -----

    /// Four derivations over identical-shape inputs produce
    /// four distinct outputs. Defends against accidental
    /// domain-tag aliasing in the Track H plumbing.
    #[test]
    fn domains_separate_under_identical_input() {
        let buf = vec![0u8; 32];
        let h_note = sha3_256_domain(SUWAPPU_L2_NOTE_COMMIT_V1, &buf);
        let h_nf = sha3_256_domain(SUWAPPU_L2_NULLIFIER_V1, &buf);
        let h_nk = sha3_256_domain(SUWAPPU_L2_NF_KEY_V1, &buf);
        let h_addr = sha3_256_domain(SUWAPPU_L2_ADDRESS_V1, &buf);
        let digests = [h_note, h_nf, h_nk, h_addr];
        for (i, a) in digests.iter().enumerate() {
            for (j, b) in digests.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    // ----- proptests -----

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            max_shrink_iters: 16,
            .. ProptestConfig::default()
        })]

        /// Commitment is deterministic over arbitrary valid notes.
        #[test]
        fn prop_commit_is_deterministic(
            v in any::<u64>(),
            r_byte in any::<u8>(),
            pk_byte in any::<u8>(),
            position in any::<u64>(),
        ) {
            let note = make_note(v, r_byte, pk_byte, position);
            let cm1 = commit_note(&note).unwrap();
            let cm2 = commit_note(&note).unwrap();
            prop_assert_eq!(cm1, cm2);
        }

        /// Different randomness produces different commitments
        /// (the hiding property the construction depends on).
        #[test]
        fn prop_randomness_changes_commitment(
            v in any::<u64>(),
            r_a in any::<u8>(),
            r_b in any::<u8>(),
            pk_byte in any::<u8>(),
            position in any::<u64>(),
        ) {
            prop_assume!(r_a != r_b);
            let n_a = make_note(v, r_a, pk_byte, position);
            let n_b = make_note(v, r_b, pk_byte, position);
            prop_assert_ne!(commit_note(&n_a).unwrap(), commit_note(&n_b).unwrap());
        }

        /// Nullifier round-trip is deterministic.
        #[test]
        fn prop_nullifier_deterministic(
            seed_byte in any::<u8>(),
            cm_byte in any::<u8>(),
            position in any::<u64>(),
        ) {
            let nk = derive_nullifier_key(&[seed_byte; 32]);
            let cm = Commitment([cm_byte; 32]);
            prop_assert_eq!(
                compute_nullifier(&nk, &cm, position),
                compute_nullifier(&nk, &cm, position)
            );
        }

        /// Different positions produce different nullifiers
        /// (off-tree replay defense).
        #[test]
        fn prop_position_changes_nullifier(
            seed_byte in any::<u8>(),
            cm_byte in any::<u8>(),
            pos_a in any::<u64>(),
            pos_b in any::<u64>(),
        ) {
            prop_assume!(pos_a != pos_b);
            let nk = derive_nullifier_key(&[seed_byte; 32]);
            let cm = Commitment([cm_byte; 32]);
            prop_assert_ne!(
                compute_nullifier(&nk, &cm, pos_a),
                compute_nullifier(&nk, &cm, pos_b)
            );
        }
    }
}
