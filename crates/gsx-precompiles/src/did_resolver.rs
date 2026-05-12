//! In-memory DID resolver.
//!
//! The phase-1 resolver is a `BTreeMap<Did, DidDocument>`. The
//! production resolver is a Move resource at a deterministic address
//! derived from the account; the trait shape is identical, so the
//! production wrapper is a substitution of the storage backend.
//!
//! Semantics:
//!
//! - `create(doc)` is singleton: a second `create` for the same `Did`
//!   fails with `DidError::AlreadyRegistered`.
//! - `update(doc, signature, signing_method_id)` requires the signature
//!   to verify under the *prior* document's verification method at the
//!   given id, AND that method must be registered in the
//!   `CapabilityInvocation` relationship. Paper §8.1: "its update-
//!   permission set is captured in the document's verification-
//!   relationship arrays under the W3C DID specification."
//! - `resolve(did)` returns the current document, or `None`.

use std::collections::BTreeMap;

use gsx_crypto::mldsa;

use crate::did::{Did, DidDocument, DidError, VerificationMethodId, VerificationRelationship};

/// Phase-1 in-memory DID resolver.
#[derive(Debug, Default, Clone)]
pub struct InMemoryDidResolver {
    docs: BTreeMap<Did, DidDocument>,
}

impl InMemoryDidResolver {
    /// Construct an empty resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of registered DIDs.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// `true` iff no DIDs are registered.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Resolve `did` to its current document.
    pub fn resolve(&self, did: &Did) -> Option<&DidDocument> {
        self.docs.get(did)
    }

    /// Register a new DID. Fails if a document already exists for the
    /// same `Did`, or if `doc` is structurally invalid.
    pub fn create(&mut self, doc: DidDocument) -> Result<(), DidError> {
        doc.validate()?;
        if self.docs.contains_key(&doc.id) {
            return Err(DidError::AlreadyRegistered);
        }
        self.docs.insert(doc.id, doc);
        Ok(())
    }

    /// Update an existing DID document.
    ///
    /// The update is authorized iff `signature` verifies under the
    /// PRIOR document's `VerificationMethod[signing_method_id]` public
    /// key AND that method is registered under `CapabilityInvocation`.
    /// The signed payload is the canonical hash of the NEW document.
    pub fn update(
        &mut self,
        new_doc: DidDocument,
        signature: &[u8],
        signing_method_id: VerificationMethodId,
    ) -> Result<(), DidError> {
        new_doc.validate()?;
        let prior = self.docs.get(&new_doc.id).ok_or(DidError::NotRegistered)?;

        // 1. The signing method must exist in the prior doc.
        let pk_bytes = prior
            .public_key(signing_method_id)
            .ok_or(DidError::UnknownMethod(signing_method_id))?;

        // 2. The signing method must be in the CapabilityInvocation
        //    relationship in the prior doc.
        if !prior.has_relationship(
            signing_method_id,
            VerificationRelationship::CapabilityInvocation,
        ) {
            return Err(DidError::UnauthorizedUpdate);
        }

        // 3. The signature must verify over the new doc's canonical hash.
        let pk =
            mldsa::PublicKey::from_bytes(pk_bytes).map_err(|_| DidError::UnauthorizedUpdate)?;
        let sig =
            mldsa::Signature::from_bytes(signature).map_err(|_| DidError::UnauthorizedUpdate)?;
        let digest = new_doc.canonical_hash();
        mldsa::verify(&digest, &sig, &pk).map_err(|_| DidError::UnauthorizedUpdate)?;

        // Authorized — install.
        self.docs.insert(new_doc.id, new_doc);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::did::{KeyAlgorithm, Service, VerificationMethod, VerificationRelationship};

    fn keypair() -> (Vec<u8>, mldsa::SecretKey) {
        let (pk, sk) = mldsa::keypair();
        (pk.as_bytes().to_vec(), sk)
    }

    fn build_doc(id: Did, pk_bytes: Vec<u8>) -> DidDocument {
        let mut doc = DidDocument::empty(id);
        doc.verification_methods.push(VerificationMethod {
            id: 0,
            controller: id,
            algorithm: KeyAlgorithm::MlDsa65,
            public_key: pk_bytes,
        });
        doc.relationships
            .entry(VerificationRelationship::CapabilityInvocation)
            .or_default()
            .insert(0);
        doc
    }

    #[test]
    fn create_then_resolve() {
        let mut r = InMemoryDidResolver::new();
        let id = Did([1; 32]);
        let (pk, _sk) = keypair();
        let doc = build_doc(id, pk);
        r.create(doc.clone()).unwrap();
        assert_eq!(r.resolve(&id), Some(&doc));
    }

    #[test]
    fn create_twice_fails() {
        let mut r = InMemoryDidResolver::new();
        let id = Did([1; 32]);
        let (pk, _sk) = keypair();
        let doc = build_doc(id, pk);
        r.create(doc.clone()).unwrap();
        assert_eq!(r.create(doc), Err(DidError::AlreadyRegistered));
    }

    #[test]
    fn update_unknown_did_fails() {
        let mut r = InMemoryDidResolver::new();
        let id = Did([1; 32]);
        let (pk, _sk) = keypair();
        let doc = build_doc(id, pk);
        let err = r.update(doc, &[0u8; 64], 0);
        assert_eq!(err, Err(DidError::NotRegistered));
    }

    #[test]
    fn update_with_authorized_signature() {
        let mut r = InMemoryDidResolver::new();
        let id = Did([1; 32]);
        let (pk, sk) = keypair();
        let doc = build_doc(id, pk);
        r.create(doc.clone()).unwrap();

        // Build a new doc with an additional service entry; sign its
        // canonical hash with the CapabilityInvocation key.
        let mut new_doc = doc;
        new_doc.services.push(Service {
            id: 0,
            service_type: "LinkedDomains".to_string(),
            endpoint: "https://example.invalid/.well-known/did.json".to_string(),
        });
        let sig = mldsa::sign(&new_doc.canonical_hash(), &sk).unwrap();
        r.update(new_doc.clone(), sig.as_bytes(), 0).unwrap();
        assert_eq!(r.resolve(&id), Some(&new_doc));
    }

    #[test]
    fn update_with_non_capability_method_fails() {
        let mut r = InMemoryDidResolver::new();
        let id = Did([1; 32]);
        let (pk, sk) = keypair();
        // Build a doc with the key in AUTHENTICATION only — not
        // CapabilityInvocation.
        let mut doc = DidDocument::empty(id);
        doc.verification_methods.push(VerificationMethod {
            id: 0,
            controller: id,
            algorithm: KeyAlgorithm::MlDsa65,
            public_key: pk,
        });
        let mut auth = BTreeSet::new();
        auth.insert(0);
        doc.relationships
            .insert(VerificationRelationship::Authentication, auth);
        r.create(doc.clone()).unwrap();

        let sig = mldsa::sign(&doc.canonical_hash(), &sk).unwrap();
        let err = r.update(doc, sig.as_bytes(), 0);
        assert_eq!(err, Err(DidError::UnauthorizedUpdate));
    }

    #[test]
    fn update_with_invalid_signature_fails() {
        let mut r = InMemoryDidResolver::new();
        let id = Did([1; 32]);
        let (pk, _sk) = keypair();
        let doc = build_doc(id, pk);
        r.create(doc.clone()).unwrap();

        // Sign with a DIFFERENT key.
        let (_other_pk, other_sk) = keypair();
        let sig = mldsa::sign(&doc.canonical_hash(), &other_sk).unwrap();
        let err = r.update(doc, sig.as_bytes(), 0);
        assert_eq!(err, Err(DidError::UnauthorizedUpdate));
    }
}
