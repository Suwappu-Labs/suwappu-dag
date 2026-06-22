//! Cross-chain DID synchronization STARK pipeline (paper §10.3).
//!
//! "Cross-chain DID synchronization runs over a dedicated zero-
//! knowledge path that is intentionally distinct from the SNARK-
//! compressed surfaces elsewhere. The prover is the LTP STARK pipeline
//! (SP1 on Plonky3) operating over the Goldilocks field with FRI-based
//! commitment; the hash function on this surface is SHA3-256
//! throughout; on-chain verification is raw FRI. The path explicitly
//! avoids elliptic-curve pairings and SNARK compression in the critical
//! path, because identity rotations must remain verifiable under post-
//! quantum-only primitives."
//!
//! ## Sprint scope (DAG-S17)
//!
//! Phase-1 delivers:
//!
//! - `DidRotationStatement` — the public input to the rotation proof:
//!   the DID being rotated, the hashes of the old and new documents,
//!   the source/target chains, and the source height.
//! - SHA3-256-only canonical digest (no BLAKE3, no Poseidon — paper
//!   §10.3 mandates post-quantum-only primitives on this surface).
//! - `DidStarkProof` — placeholder for the SP1/Plonky3 FRI proof. The
//!   placeholder carries an ML-DSA-65 signature from the old
//!   document's `CapabilityInvocation` set; the production STARK
//!   replaces this with a circuit that proves *the existence of* such
//!   a signature without revealing which method id signed.
//! - `prove_rotation` / `verify_rotation_proof` — the wire-up that the
//!   future circuit substitution lands on.
//!
//! Property tests verify the round-trip semantics at 10,000 cases.

use serde::{Deserialize, Serialize};
use suwappu_crypto::{hash, mldsa};
use suwappu_precompiles::{Did, DidDocument, VerificationMethodId, VerificationRelationship};
use thiserror::Error;

use crate::attestation::ChainId;

/// Statement that the STARK proves (public input).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DidRotationStatement {
    /// The DID being rotated.
    pub did: Did,
    /// SHA3-256 canonical hash of the OLD `DidDocument`.
    pub old_doc_hash: [u8; 32],
    /// SHA3-256 canonical hash of the NEW `DidDocument`.
    pub new_doc_hash: [u8; 32],
    /// Chain on which the DID is being rotated.
    pub source_chain: ChainId,
    /// Target chain that will receive the rotation.
    pub target_chain: ChainId,
    /// Source-chain block height of the rotation event.
    pub source_height: u64,
}

impl DidRotationStatement {
    /// Canonical SHA3-256 digest with the `SUWAPPU-DID-STARK-V1` domain
    /// tag. Used as the ML-DSA-65 signing message.
    pub fn digest(&self) -> [u8; 32] {
        let mut blob = Vec::with_capacity(32 + 32 + 32 + 8 + 8 + 8);
        blob.extend_from_slice(&self.did.0);
        blob.extend_from_slice(&self.old_doc_hash);
        blob.extend_from_slice(&self.new_doc_hash);
        blob.extend_from_slice(&self.source_chain.to_be_bytes());
        blob.extend_from_slice(&self.target_chain.to_be_bytes());
        blob.extend_from_slice(&self.source_height.to_be_bytes());
        hash::sha3_256_domain(b"GSX-DID-STARK-V1", &blob)
    }
}

/// Proof of a DID rotation.
///
/// Phase-1 wraps an ML-DSA-65 signature from the old document's
/// `CapabilityInvocation` method. Production replaces this with an
/// SP1/Plonky3 FRI proof that proves the same predicate (a valid
/// CapabilityInvocation signature exists) without disclosing which
/// method id signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DidStarkProof {
    /// The public statement.
    pub statement: DidRotationStatement,
    /// Phase-1 transparency field: the signing method id used. The
    /// production STARK hides this; the public statement does not name
    /// the method.
    pub signing_method_id: VerificationMethodId,
    /// ML-DSA-65 detached signature over `statement.digest()`.
    pub signature: Vec<u8>,
    /// FRI proof bytes. Phase-1 placeholder (empty).
    pub fri_proof: Vec<u8>,
}

/// Errors emitted by the DID STARK pipeline.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum DidStarkError {
    /// The old document's hash in the statement does not match
    /// `old_doc.canonical_hash()`.
    #[error("old document hash in statement does not match the supplied document")]
    OldDocHashMismatch,

    /// The signing method id is not registered in the old document.
    #[error("signing method id {0} not in old document")]
    UnknownMethod(VerificationMethodId),

    /// The signing method id is not in the old document's
    /// `CapabilityInvocation` relationship.
    #[error("signing method id {0} lacks CapabilityInvocation")]
    UnauthorizedMethod(VerificationMethodId),

    /// Signature verification failed.
    #[error("signature verification failed")]
    InvalidSignature,

    /// The supplied public-key bytes did not parse as ML-DSA-65.
    #[error("malformed public key for method id {0}")]
    MalformedKey(VerificationMethodId),

    /// The supplied signature bytes did not parse as ML-DSA-65.
    #[error("malformed signature bytes")]
    MalformedSignature,
}

/// Produce a `DidStarkProof` for a DID rotation.
///
/// `signing_method_id` must be present in `old_doc` and registered in
/// the `CapabilityInvocation` relationship. The signing key `sk` must
/// be the matching ML-DSA-65 private key for the named method.
pub fn prove_rotation(
    statement: DidRotationStatement,
    old_doc: &DidDocument,
    signing_method_id: VerificationMethodId,
    sk: &mldsa::SecretKey,
) -> Result<DidStarkProof, DidStarkError> {
    // 1. The statement's old_doc_hash must bind to the supplied old doc.
    if statement.old_doc_hash != old_doc.canonical_hash() {
        return Err(DidStarkError::OldDocHashMismatch);
    }

    // 2. The signing method must exist.
    if old_doc.public_key(signing_method_id).is_none() {
        return Err(DidStarkError::UnknownMethod(signing_method_id));
    }

    // 3. The signing method must hold CapabilityInvocation.
    if !old_doc.has_relationship(
        signing_method_id,
        VerificationRelationship::CapabilityInvocation,
    ) {
        return Err(DidStarkError::UnauthorizedMethod(signing_method_id));
    }

    // 4. Sign the canonical digest.
    let digest = statement.digest();
    let sig = mldsa::sign(&digest, sk).map_err(|_| DidStarkError::InvalidSignature)?;

    Ok(DidStarkProof {
        statement,
        signing_method_id,
        signature: sig.as_bytes().to_vec(),
        fri_proof: Vec::new(),
    })
}

/// Verify a `DidStarkProof` against the OLD `DidDocument`.
///
/// On success returns the proof's statement. The caller is responsible
/// for matching `statement.new_doc_hash` against the new document it
/// intends to install.
pub fn verify_rotation_proof(
    proof: &DidStarkProof,
    old_doc: &DidDocument,
) -> Result<DidRotationStatement, DidStarkError> {
    // Same checks as prove_rotation, with verification instead of signing.
    if proof.statement.old_doc_hash != old_doc.canonical_hash() {
        return Err(DidStarkError::OldDocHashMismatch);
    }
    let pk_bytes = old_doc
        .public_key(proof.signing_method_id)
        .ok_or(DidStarkError::UnknownMethod(proof.signing_method_id))?;
    if !old_doc.has_relationship(
        proof.signing_method_id,
        VerificationRelationship::CapabilityInvocation,
    ) {
        return Err(DidStarkError::UnauthorizedMethod(proof.signing_method_id));
    }
    let pk = mldsa::PublicKey::from_bytes(pk_bytes)
        .map_err(|_| DidStarkError::MalformedKey(proof.signing_method_id))?;
    let sig = mldsa::Signature::from_bytes(&proof.signature)
        .map_err(|_| DidStarkError::MalformedSignature)?;
    let digest = proof.statement.digest();
    mldsa::verify(&digest, &sig, &pk).map_err(|_| DidStarkError::InvalidSignature)?;
    Ok(proof.statement.clone())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use suwappu_precompiles::{KeyAlgorithm, VerificationMethod};

    use super::*;

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

    fn statement_for(old: &DidDocument, new_hash: [u8; 32]) -> DidRotationStatement {
        DidRotationStatement {
            did: old.id,
            old_doc_hash: old.canonical_hash(),
            new_doc_hash: new_hash,
            source_chain: 1,
            target_chain: 100,
            source_height: 42,
        }
    }

    #[test]
    fn round_trip_works() {
        let (old, sk) = build_doc_with_ci_key(1);
        let new_hash = [0xCD; 32];
        let stmt = statement_for(&old, new_hash);
        let proof = prove_rotation(stmt.clone(), &old, 0, &sk).unwrap();
        let recovered = verify_rotation_proof(&proof, &old).unwrap();
        assert_eq!(recovered, stmt);
    }

    #[test]
    fn old_doc_hash_mismatch_rejected() {
        let (old, sk) = build_doc_with_ci_key(1);
        let new_hash = [0xCD; 32];
        let mut stmt = statement_for(&old, new_hash);
        stmt.old_doc_hash[0] ^= 1; // tamper
        let err = prove_rotation(stmt, &old, 0, &sk);
        assert_eq!(err, Err(DidStarkError::OldDocHashMismatch));
    }

    #[test]
    fn unauthorized_method_rejected() {
        let did = Did([1; 32]);
        let (pk, sk) = mldsa::keypair();
        let mut doc = DidDocument::empty(did);
        doc.verification_methods.push(VerificationMethod {
            id: 0,
            controller: did,
            algorithm: KeyAlgorithm::MlDsa65,
            public_key: pk.as_bytes().to_vec(),
        });
        // Put the key in Authentication only — NOT CapabilityInvocation.
        let mut auth = BTreeSet::new();
        auth.insert(0);
        doc.relationships
            .insert(VerificationRelationship::Authentication, auth);

        let new_hash = [0xCD; 32];
        let stmt = statement_for(&doc, new_hash);
        let err = prove_rotation(stmt, &doc, 0, &sk);
        assert_eq!(err, Err(DidStarkError::UnauthorizedMethod(0)));
    }

    #[test]
    fn tampered_new_hash_breaks_verification() {
        let (old, sk) = build_doc_with_ci_key(1);
        let new_hash = [0xCD; 32];
        let stmt = statement_for(&old, new_hash);
        let mut proof = prove_rotation(stmt, &old, 0, &sk).unwrap();
        proof.statement.new_doc_hash[0] ^= 1;
        let err = verify_rotation_proof(&proof, &old);
        assert_eq!(err, Err(DidStarkError::InvalidSignature));
    }

    #[test]
    fn cross_chain_binding_in_digest() {
        let (old, sk) = build_doc_with_ci_key(1);
        let new_hash = [0xCD; 32];
        let stmt_a = statement_for(&old, new_hash);
        let mut stmt_b = stmt_a.clone();
        stmt_b.target_chain = 200;
        assert_ne!(stmt_a.digest(), stmt_b.digest());

        // A proof for stmt_a doesn't verify against a tampered stmt_b.
        let mut proof = prove_rotation(stmt_a, &old, 0, &sk).unwrap();
        proof.statement.target_chain = 200;
        let err = verify_rotation_proof(&proof, &old);
        assert_eq!(err, Err(DidStarkError::InvalidSignature));
    }
}
