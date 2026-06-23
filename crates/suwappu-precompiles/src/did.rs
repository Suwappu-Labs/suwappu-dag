//! Decentralized Identifier (DID) document and validation.
//!
//! Paper §8.1: "Every account carries a decentralized-identifier (DID)
//! field with attached credential proofs, readable natively from
//! precompiles in both VMs. The DID document is a Move resource at a
//! deterministic address derived from the account; the resource is
//! structurally enforced as singleton, and its update-permission set is
//! captured in the document's verification-relationship arrays under
//! the W3C DID specification."
//!
//! This module implements a phase-1 subset of W3C DID Core v1.0 [W3C
//! DID 2022]:
//!
//! - `Did` (the `did:` URI) — content-addressed 32-byte identifier.
//! - `VerificationMethod` — a public key associated with the DID.
//! - `VerificationRelationship` — the relationship (authentication,
//!   assertion, keyAgreement, capabilityInvocation, capabilityDelegation)
//!   that grants the VM authority on the DID surface.
//! - `Service` — endpoint metadata.
//! - `DidDocument` — the singleton resource at the DID's deterministic
//!   address.
//!
//! Structural validation (`DidDocument::validate`) enforces:
//!
//! 1. Every `VerificationRelationship` references a `VerificationMethod`
//!    that exists in `verification_methods`.
//! 2. Every `verification_methods[i].controller` matches `id` (Phase-1
//!    forbids cross-DID controllers; the launch-readiness DID phase 3
//!    will lift this).
//! 3. Each `VerificationMethod` has a unique id (no duplicates).
//! 4. The DID's `id` is non-zero (the `[0; 32]` value is reserved as
//!    "no DID").

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Decentralized identifier — content-addressed 32-byte ID under the
/// `did:suwappu:` method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Did(pub [u8; 32]);

impl Did {
    /// The reserved "no DID" / null sentinel.
    pub const NULL: Did = Did([0u8; 32]);
}

/// Identifier of a [`VerificationMethod`] inside a DID document.
///
/// W3C DID Core encodes this as `did#fragment`; we keep it numeric for
/// phase-1.
pub type VerificationMethodId = u32;

/// Verification-relationship arrays per W3C DID Core §5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VerificationRelationship {
    /// Used to authenticate the controller.
    Authentication,
    /// Used to sign verifiable credentials and other assertions.
    AssertionMethod,
    /// Used for key agreement (encryption).
    KeyAgreement,
    /// Used to invoke a capability (e.g., update the DID document).
    CapabilityInvocation,
    /// Used to delegate a capability to another DID.
    CapabilityDelegation,
}

/// Cryptographic primitive identifier for a verification method.
///
/// Phase-1 fixes ML-DSA-65 (FIPS 204); the hybrid ECDSA + ML-DSA-65 path
/// of paper §12 Table 1 row 1 lands with the account-signing surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyAlgorithm {
    /// ML-DSA-65 / FIPS 204.
    MlDsa65,
}

/// A public-key verification method on the DID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationMethod {
    /// Stable identifier of this method within the document.
    pub id: VerificationMethodId,
    /// DID that controls this key. Phase-1 always equals the document's `id`.
    pub controller: Did,
    /// Key algorithm — ML-DSA-65 in phase-1.
    pub algorithm: KeyAlgorithm,
    /// Public key bytes for `algorithm`.
    pub public_key: Vec<u8>,
}

/// Service endpoint (W3C DID Core §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    /// Stable identifier of this service within the document.
    pub id: u32,
    /// Service type (free-form string; e.g. `LinkedDomains`,
    /// `CredentialRegistry`).
    pub service_type: String,
    /// Service endpoint URL.
    pub endpoint: String,
}

/// Singleton DID resource at the DID's deterministic address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DidDocument {
    /// The DID itself.
    pub id: Did,
    /// Registered verification methods.
    pub verification_methods: Vec<VerificationMethod>,
    /// For each verification relationship, the set of method ids that
    /// hold it.
    pub relationships: BTreeMap<VerificationRelationship, BTreeSet<VerificationMethodId>>,
    /// Registered service endpoints.
    pub services: Vec<Service>,
}

impl DidDocument {
    /// Construct an empty document carrying only the DID. No methods or
    /// services attached; not valid until verification methods are
    /// added.
    pub fn empty(id: Did) -> Self {
        Self {
            id,
            verification_methods: Vec::new(),
            relationships: BTreeMap::new(),
            services: Vec::new(),
        }
    }

    /// Canonical content hash of this document. Used by the resolver to
    /// detect identical-payload updates and to bind signatures to a
    /// specific revision.
    pub fn canonical_hash(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"SUWAPPU-DID-DOC-V1");
        hasher.update(&self.id.0);
        hasher.update(&(self.verification_methods.len() as u32).to_be_bytes());
        for vm in &self.verification_methods {
            hasher.update(&vm.id.to_be_bytes());
            hasher.update(&vm.controller.0);
            hasher.update(&[match vm.algorithm {
                KeyAlgorithm::MlDsa65 => 0x01,
            }]);
            hasher.update(&(vm.public_key.len() as u32).to_be_bytes());
            hasher.update(&vm.public_key);
        }
        hasher.update(&(self.relationships.len() as u32).to_be_bytes());
        for (rel, ids) in &self.relationships {
            hasher.update(&[match rel {
                VerificationRelationship::Authentication => 0x01,
                VerificationRelationship::AssertionMethod => 0x02,
                VerificationRelationship::KeyAgreement => 0x03,
                VerificationRelationship::CapabilityInvocation => 0x04,
                VerificationRelationship::CapabilityDelegation => 0x05,
            }]);
            hasher.update(&(ids.len() as u32).to_be_bytes());
            for id in ids {
                hasher.update(&id.to_be_bytes());
            }
        }
        hasher.update(&(self.services.len() as u32).to_be_bytes());
        for service in &self.services {
            hasher.update(&service.id.to_be_bytes());
            hasher.update(&(service.service_type.len() as u32).to_be_bytes());
            hasher.update(service.service_type.as_bytes());
            hasher.update(&(service.endpoint.len() as u32).to_be_bytes());
            hasher.update(service.endpoint.as_bytes());
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(hasher.finalize().as_bytes());
        out
    }

    /// Validate structural consistency of this document.
    pub fn validate(&self) -> Result<(), DidError> {
        // 1. The DID id must be non-null.
        if self.id == Did::NULL {
            return Err(DidError::NullDid);
        }

        // 2. Verification-method ids must be unique.
        let mut seen: BTreeSet<VerificationMethodId> = BTreeSet::new();
        for vm in &self.verification_methods {
            if !seen.insert(vm.id) {
                return Err(DidError::DuplicateMethodId(vm.id));
            }
            // 3. The controller of every method must be this DID.
            if vm.controller != self.id {
                return Err(DidError::CrossDidController {
                    method: vm.id,
                    controller: vm.controller,
                });
            }
        }

        // 4. Every relationship must reference a method that exists.
        for (rel, ids) in &self.relationships {
            for id in ids {
                if !seen.contains(id) {
                    return Err(DidError::DanglingRelationship {
                        relationship: *rel,
                        method: *id,
                    });
                }
            }
        }

        // 5. Service ids must be unique.
        let mut service_seen: BTreeSet<u32> = BTreeSet::new();
        for service in &self.services {
            if !service_seen.insert(service.id) {
                return Err(DidError::DuplicateServiceId(service.id));
            }
        }

        Ok(())
    }

    /// Get the public key bytes for `method_id` if present.
    pub fn public_key(&self, method_id: VerificationMethodId) -> Option<&[u8]> {
        self.verification_methods
            .iter()
            .find(|vm| vm.id == method_id)
            .map(|vm| vm.public_key.as_slice())
    }

    /// Returns `true` iff `method_id` is registered in the given
    /// verification relationship.
    pub fn has_relationship(
        &self,
        method_id: VerificationMethodId,
        rel: VerificationRelationship,
    ) -> bool {
        self.relationships
            .get(&rel)
            .is_some_and(|ids| ids.contains(&method_id))
    }
}

/// DID-related error types.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum DidError {
    /// The DID `id` is the reserved `[0; 32]` null sentinel.
    #[error("did id is the null sentinel")]
    NullDid,

    /// Two verification methods share the same id.
    #[error("duplicate verification method id {0}")]
    DuplicateMethodId(VerificationMethodId),

    /// A verification method's controller does not match the document's
    /// `id`. Phase-1 forbids cross-DID controllers.
    #[error("cross-DID controller on method {method}")]
    CrossDidController {
        /// Offending method id.
        method: VerificationMethodId,
        /// Foreign controller DID.
        controller: Did,
    },

    /// A verification relationship references a method id that does not
    /// exist in the document.
    #[error(
        "dangling verification relationship: method {method} \
         missing for {relationship:?}"
    )]
    DanglingRelationship {
        /// Offending relationship.
        relationship: VerificationRelationship,
        /// Method id that was referenced but not registered.
        method: VerificationMethodId,
    },

    /// Two service entries share the same id.
    #[error("duplicate service id {0}")]
    DuplicateServiceId(u32),

    /// Resolver-side: DID already exists when `create` was called.
    #[error("did already registered")]
    AlreadyRegistered,

    /// Resolver-side: DID does not exist when `update` or `resolve` was
    /// called.
    #[error("did not registered")]
    NotRegistered,

    /// Update signature did not verify under any
    /// `CapabilityInvocation` key in the prior document.
    #[error("did update unauthorized: signature did not verify under capabilityInvocation set")]
    UnauthorizedUpdate,

    /// Update signature targeted a method id that is not present in the
    /// prior document.
    #[error("did update referenced unknown method id {0}")]
    UnknownMethod(VerificationMethodId),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(id: VerificationMethodId, did: Did, key_seed: u8) -> VerificationMethod {
        VerificationMethod {
            id,
            controller: did,
            algorithm: KeyAlgorithm::MlDsa65,
            public_key: vec![key_seed; 32],
        }
    }

    #[test]
    fn null_did_fails_validation() {
        let doc = DidDocument::empty(Did::NULL);
        assert_eq!(doc.validate(), Err(DidError::NullDid));
    }

    #[test]
    fn empty_non_null_doc_validates() {
        let doc = DidDocument::empty(Did([1; 32]));
        doc.validate().unwrap();
    }

    #[test]
    fn duplicate_method_id_fails() {
        let id = Did([1; 32]);
        let mut doc = DidDocument::empty(id);
        doc.verification_methods.push(vm(0, id, 0xAA));
        doc.verification_methods.push(vm(0, id, 0xBB));
        assert_eq!(doc.validate(), Err(DidError::DuplicateMethodId(0)));
    }

    #[test]
    fn cross_did_controller_fails() {
        let id = Did([1; 32]);
        let other = Did([2; 32]);
        let mut doc = DidDocument::empty(id);
        doc.verification_methods.push(vm(0, other, 0xAA));
        assert!(matches!(
            doc.validate(),
            Err(DidError::CrossDidController { .. })
        ));
    }

    #[test]
    fn dangling_relationship_fails() {
        let id = Did([1; 32]);
        let mut doc = DidDocument::empty(id);
        doc.verification_methods.push(vm(0, id, 0xAA));
        doc.relationships
            .entry(VerificationRelationship::Authentication)
            .or_default()
            .insert(1); // method 1 does not exist
        assert!(matches!(
            doc.validate(),
            Err(DidError::DanglingRelationship { method: 1, .. })
        ));
    }

    #[test]
    fn canonical_hash_is_deterministic() {
        let id = Did([1; 32]);
        let mut doc = DidDocument::empty(id);
        doc.verification_methods.push(vm(0, id, 0xAA));
        assert_eq!(doc.canonical_hash(), doc.canonical_hash());
    }

    #[test]
    fn canonical_hash_changes_with_methods() {
        let id = Did([1; 32]);
        let doc_a = DidDocument::empty(id);
        let mut doc_b = doc_a.clone();
        doc_b.verification_methods.push(vm(0, id, 0xAA));
        assert_ne!(doc_a.canonical_hash(), doc_b.canonical_hash());
    }
}
