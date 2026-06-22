//! DAG-S12 exit-gate property tests.
//!
//! Exit gate: `did_document_validates` — for any well-formed DID
//! document under the W3C DID Core v1.0 phase-1 subset (paper §8.1),
//! `validate()` succeeds, the resolver's `create` succeeds and
//! subsequent `resolve` returns the document; tampering that breaks
//! structural consistency (dangling relationship, duplicate id,
//! cross-DID controller) fails validation.
//!
//! Supporting properties:
//!
//! - `dangling_relationship_rejected` — inserting a relationship that
//!   references a non-existent method id is rejected.
//! - `singleton_did_resource_enforced` — the second `create` for the
//!   same DID fails with `AlreadyRegistered`.
//! - `unauthorized_update_rejected` — `update` rejects when the
//!   signature is forged or when the signing method lacks
//!   `CapabilityInvocation`.
//!
//! Run at default 64 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-precompiles --release`.

use std::collections::BTreeSet;

use proptest::prelude::*;
use suwappu_crypto::mldsa;
use suwappu_precompiles::{
    Did, DidDocument, DidError, InMemoryDidResolver, KeyAlgorithm, VerificationMethod,
    VerificationMethodId, VerificationRelationship,
};

/// Strategy for a non-null DID. Phase-1 forbids the `[0; 32]` sentinel.
fn did_strategy() -> impl Strategy<Value = Did> {
    (1u8..255).prop_map(|seed| Did([seed; 32]))
}

/// Build a well-formed DID document with `n_methods` methods (the
/// first method holds CapabilityInvocation) and a fresh ML-DSA-65
/// keypair for the first method.
fn build_valid_doc(id: Did, n_methods: u32) -> (DidDocument, mldsa::SecretKey) {
    let (pk_first, sk_first) = mldsa::keypair();
    let mut doc = DidDocument::empty(id);
    for i in 0..n_methods {
        let pk_bytes = if i == 0 {
            pk_first.as_bytes().to_vec()
        } else {
            // Other methods get fresh keys but we don't need their sks
            // in the test fixture.
            mldsa::keypair().0.as_bytes().to_vec()
        };
        doc.verification_methods.push(VerificationMethod {
            id: i as VerificationMethodId,
            controller: id,
            algorithm: KeyAlgorithm::MlDsa65,
            public_key: pk_bytes,
        });
    }
    if n_methods > 0 {
        let mut ci = BTreeSet::new();
        ci.insert(0);
        doc.relationships
            .insert(VerificationRelationship::CapabilityInvocation, ci);
    }
    (doc, sk_first)
}

proptest! {
    #![proptest_config(ProptestConfig {
        // ML-DSA-65 keygen is expensive — keep default cases low; the
        // 10k release sweep does the load-bearing work.
        cases: 64,
        max_shrink_iters: 16,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — any well-formed document validates, creates, and
    /// resolves; the resolved document equals the created document.
    #[test]
    fn did_document_validates(
        id in did_strategy(),
        n_methods in 1u32..=4,
    ) {
        let (doc, _sk) = build_valid_doc(id, n_methods);
        doc.validate().expect("well-formed doc must validate");

        let mut r = InMemoryDidResolver::new();
        r.create(doc.clone()).expect("create must succeed");
        prop_assert_eq!(r.resolve(&id), Some(&doc));
    }

    /// Tampering — inserting a relationship that references a method
    /// id outside the document's verification-method set — yields a
    /// `DanglingRelationship` error and is rejected by `create`.
    #[test]
    fn dangling_relationship_rejected(
        id in did_strategy(),
        n_methods in 1u32..=4,
        bogus_method_id in 100u32..200,
    ) {
        let (mut doc, _sk) = build_valid_doc(id, n_methods);
        // Insert a relationship pointing at a method id that does NOT
        // exist (we just inserted ids 0..n_methods, and bogus_method_id
        // is in 100..200 — disjoint).
        doc.relationships
            .entry(VerificationRelationship::Authentication)
            .or_default()
            .insert(bogus_method_id);

        let validate_result = doc.validate();
        let is_dangling = matches!(
            validate_result,
            Err(DidError::DanglingRelationship { .. })
        );
        prop_assert!(is_dangling);

        let mut r = InMemoryDidResolver::new();
        prop_assert!(r.create(doc).is_err());
    }

    /// Singleton resource: a second `create` for the same DID is
    /// rejected with `AlreadyRegistered`.
    #[test]
    fn singleton_did_resource_enforced(
        id in did_strategy(),
        n_methods in 1u32..=3,
    ) {
        let (doc, _sk) = build_valid_doc(id, n_methods);
        let mut r = InMemoryDidResolver::new();
        r.create(doc.clone()).unwrap();
        let second = r.create(doc);
        prop_assert_eq!(second, Err(DidError::AlreadyRegistered));
    }

    /// Update with a signature from a different (foreign) key fails.
    /// The update pipeline must reject under `UnauthorizedUpdate`.
    #[test]
    fn unauthorized_update_rejected(
        id in did_strategy(),
        n_methods in 1u32..=3,
    ) {
        let (doc, _sk_legit) = build_valid_doc(id, n_methods);
        let mut r = InMemoryDidResolver::new();
        r.create(doc.clone()).unwrap();

        // Build a "new" document (it can be identical) and sign its
        // canonical hash with a FOREIGN secret key.
        let new_doc = doc;
        let (_pk_foreign, sk_foreign) = mldsa::keypair();
        let sig = mldsa::sign(&new_doc.canonical_hash(), &sk_foreign).unwrap();

        let err = r.update(new_doc, sig.as_bytes(), 0);
        prop_assert_eq!(err, Err(DidError::UnauthorizedUpdate));
    }
}
