//! DAG-S17 exit-gate property tests.
//!
//! Exit gate: `did_stark_round_trip` — for any well-formed
//! `DidRotationStatement`, `prove_rotation` produces a proof and
//! `verify_rotation_proof` recovers the original statement
//! bit-for-bit; tampering with any field breaks verification.
//!
//! Supporting properties:
//!
//! - `cross_chain_statement_binding` — flipping `target_chain` (or
//!   `source_chain`) changes the canonical digest, so a proof for one
//!   chain pair does not verify for another.
//! - `unauthorized_rotation_rejected` — a signing method that lacks
//!   `CapabilityInvocation` in the old document is rejected at proof
//!   time.
//! - `wrong_signer_rejected` — proof produced with a key that does NOT
//!   correspond to the named method id fails verification.
//!
//! Run at default 64 cases under CI (ML-DSA-65 keygen is ~5 ms each);
//! sprint close runs `PROPTEST_CASES=10000 cargo test -p suwappu-ltp
//! --release`.

use std::collections::BTreeSet;

use proptest::prelude::*;
use suwappu_crypto::mldsa;
use suwappu_ltp::{prove_rotation, verify_rotation_proof, DidRotationStatement, DidStarkError};
use suwappu_precompiles::{
    Did, DidDocument, KeyAlgorithm, VerificationMethod, VerificationRelationship,
};

fn build_doc_with_ci_key(did_seed: u8) -> (DidDocument, mldsa::SecretKey) {
    let did = Did([did_seed; 32]);
    let (pk, sk) = mldsa::keypair();
    let mut doc = DidDocument::empty(did);
    doc.verification_methods.push(VerificationMethod {
        id: 0,
        controller: did,
        algorithm: KeyAlgorithm::MlDsa65,
        public_key: pk.as_bytes().to_vec(),
    });
    let mut ci = BTreeSet::new();
    ci.insert(0);
    doc.relationships
        .insert(VerificationRelationship::CapabilityInvocation, ci);
    (doc, sk)
}

fn statement_for(
    old: &DidDocument,
    new_hash_seed: u8,
    source: u64,
    target: u64,
    height: u64,
) -> DidRotationStatement {
    DidRotationStatement {
        did: old.id,
        old_doc_hash: old.canonical_hash(),
        new_doc_hash: [new_hash_seed; 32],
        source_chain: source,
        target_chain: target,
        source_height: height,
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        max_shrink_iters: 16,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — prove + verify round-trips the statement; tampering
    /// any field of the statement after proving breaks verification.
    #[test]
    fn did_stark_round_trip(
        did_seed in any::<u8>(),
        new_hash_seed in any::<u8>(),
        source in any::<u64>(),
        target in any::<u64>(),
        height in 0u64..=1_000_000,
    ) {
        let (old, sk) = build_doc_with_ci_key(did_seed);
        let stmt = statement_for(&old, new_hash_seed, source, target, height);
        let proof = prove_rotation(stmt.clone(), &old, 0, &sk).unwrap();
        let recovered = verify_rotation_proof(&proof, &old).unwrap();
        prop_assert_eq!(recovered, stmt);

        // Tamper any one field of the statement; verification must fail.
        let mut tampered = proof.clone();
        tampered.statement.new_doc_hash[0] ^= 1;
        let err = verify_rotation_proof(&tampered, &old);
        let is_invalid = matches!(err, Err(DidStarkError::InvalidSignature));
        prop_assert!(is_invalid);
    }

    /// Flipping the target chain breaks the proof — the digest binds
    /// the chain pair, so a proof for (source, target) does not verify
    /// for (source, target').
    #[test]
    fn cross_chain_statement_binding(
        did_seed in any::<u8>(),
        new_hash_seed in any::<u8>(),
        source in any::<u64>(),
        target in any::<u64>(),
        target_b in any::<u64>(),
        height in 0u64..=1_000_000,
    ) {
        prop_assume!(target != target_b);
        let (old, sk) = build_doc_with_ci_key(did_seed);
        let stmt = statement_for(&old, new_hash_seed, source, target, height);
        let mut proof = prove_rotation(stmt, &old, 0, &sk).unwrap();
        proof.statement.target_chain = target_b;
        let err = verify_rotation_proof(&proof, &old);
        let is_invalid = matches!(err, Err(DidStarkError::InvalidSignature));
        prop_assert!(is_invalid);
    }

    /// `prove_rotation` rejects a signing method that lacks the
    /// `CapabilityInvocation` relationship in the old document.
    #[test]
    fn unauthorized_rotation_rejected(
        did_seed in any::<u8>(),
        new_hash_seed in any::<u8>(),
        source in any::<u64>(),
        target in any::<u64>(),
        height in 0u64..=1_000_000,
    ) {
        // Build a doc with the key in Authentication only — not CI.
        let did = Did([did_seed; 32]);
        let (pk, sk) = mldsa::keypair();
        let mut doc = DidDocument::empty(did);
        doc.verification_methods.push(VerificationMethod {
            id: 0,
            controller: did,
            algorithm: KeyAlgorithm::MlDsa65,
            public_key: pk.as_bytes().to_vec(),
        });
        let mut auth = BTreeSet::new();
        auth.insert(0);
        doc.relationships
            .insert(VerificationRelationship::Authentication, auth);

        let stmt = statement_for(&doc, new_hash_seed, source, target, height);
        let err = prove_rotation(stmt, &doc, 0, &sk);
        prop_assert_eq!(err, Err(DidStarkError::UnauthorizedMethod(0)));
    }

    /// A proof produced with a foreign secret key (not bound to the
    /// document's method id 0) fails verification.
    #[test]
    fn wrong_signer_rejected(
        did_seed in any::<u8>(),
        new_hash_seed in any::<u8>(),
        source in any::<u64>(),
        target in any::<u64>(),
        height in 0u64..=1_000_000,
    ) {
        let (old, _sk_legit) = build_doc_with_ci_key(did_seed);
        let (_pk_foreign, sk_foreign) = mldsa::keypair();
        let stmt = statement_for(&old, new_hash_seed, source, target, height);

        // prove_rotation uses sk_foreign to sign, but old_doc's method 0
        // is bound to sk_legit. verify_rotation_proof will reject under
        // InvalidSignature.
        let proof = prove_rotation(stmt, &old, 0, &sk_foreign).unwrap();
        let err = verify_rotation_proof(&proof, &old);
        let is_invalid = matches!(err, Err(DidStarkError::InvalidSignature));
        prop_assert!(is_invalid);
    }
}
